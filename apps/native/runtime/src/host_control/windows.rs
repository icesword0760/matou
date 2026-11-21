use super::{IO_POLL, MAX_CONNECTIONS, Pending, ServerState, serve_frames};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::c_void,
    io::{self, Read, Write},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    ptr::null_mut,
    sync::{Arc, atomic::Ordering, mpsc::SyncSender},
    thread,
};
use tokio::{
    net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions},
    runtime::{Builder, Handle, Runtime},
    time::{sleep, timeout},
};

const PIPE_PREFIX: &str = r"\\.\pipe\matou-control-";

pub(super) struct Endpoint {
    pub(super) path: PathBuf,
}

pub(super) struct Listener {
    pipe: NamedPipeServer,
    security: LogonSecurity,
    runtime: Runtime,
    path: PathBuf,
}

impl Endpoint {
    pub(super) fn bind(_: &Path) -> Result<(Self, Listener)> {
        let path = PathBuf::from(format!("{PIPE_PREFIX}{}", uuid::Uuid::new_v4()));
        let security = LogonSecurity::new().context("restrict the pipe to the current logon")?;
        let runtime = Builder::new_current_thread().enable_all().build()?;
        let pipe = {
            let _entered = runtime.enter();
            security.create_pipe(&path, true)?
        };
        Ok((
            Self { path: path.clone() },
            Listener {
                pipe,
                security,
                runtime,
                path,
            },
        ))
    }
}

pub(crate) async fn connect(endpoint: &Path) -> io::Result<NamedPipeClient> {
    if !endpoint
        .to_str()
        .and_then(|name| name.strip_prefix(PIPE_PREFIX))
        .is_some_and(|suffix| uuid::Uuid::parse_str(suffix).is_ok())
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }

    const ERROR_PIPE_BUSY: i32 = 231;
    loop {
        match ClientOptions::new().open(endpoint) {
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => sleep(IO_POLL).await,
            result => return result,
        }
    }
}

pub(super) fn accept_connections(
    Listener {
        mut pipe,
        security,
        runtime,
        path,
    }: Listener,
    state: &Arc<ServerState>,
    sender: &SyncSender<Pending>,
) {
    let mut clients = Vec::<thread::JoinHandle<()>>::new();
    runtime.block_on(async {
        while !state.stopping() {
            clients.retain(|worker| !worker.is_finished());
            match timeout(IO_POLL, pipe.connect()).await {
                Err(_) => continue,
                Ok(Err(_)) => break,
                Ok(Ok(())) => {}
            }
            let Ok(next) = security.create_pipe(&path, false) else {
                break;
            };
            let connected = std::mem::replace(&mut pipe, next);
            if clients.len() >= MAX_CONNECTIONS {
                continue;
            }
            let state = state.clone();
            let sender = sender.clone();
            let runtime = Handle::current();
            if let Ok(worker) = thread::Builder::new()
                .name("host-control-client".into())
                .spawn(move || {
                    let mut connection = PipeConnection {
                        pipe: connected,
                        runtime,
                    };
                    let _ = serve_frames(&mut connection, &state, &sender);
                })
            {
                clients.push(worker);
            }
        }
        state.stopping.store(true, Ordering::Release);
        drop(pipe);
        while clients.iter().any(|worker| !worker.is_finished()) {
            sleep(IO_POLL).await;
        }
    });
    for worker in clients {
        let _ = worker.join();
    }
}

struct PipeConnection {
    pipe: NamedPipeServer,
    runtime: Handle,
}

impl Read for PipeConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.runtime.block_on(async {
            timeout(IO_POLL, self.pipe.readable()).await??;
            self.pipe.try_read(buf)
        })
    }
}

impl Write for PipeConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.runtime.block_on(async {
            timeout(IO_POLL, self.pipe.writable()).await??;
            self.pipe.try_write(buf)
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[repr(C)]
struct TokenGroups {
    count: u32,
    sid: *mut c_void,
    attributes: u32,
}

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
    fn GetTokenInformation(
        token: *mut c_void,
        class: i32,
        info: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
    fn ConvertSidToStringSidW(sid: *mut c_void, output: *mut *mut u16) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl: *const u16,
        revision: u32,
        descriptor: *mut *mut c_void,
        size: *mut u32,
    ) -> i32;
}

struct LocalAllocation<T>(*mut T);

impl<T> Drop for LocalAllocation<T> {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0.cast()) };
    }
}

struct LogonSecurity(LocalAllocation<c_void>);

// The immutable descriptor is owned by the listener and may be moved to its thread.
unsafe impl Send for LogonSecurity {}

impl LogonSecurity {
    fn new() -> Result<Self> {
        const TOKEN_QUERY: u32 = 0x0008;
        const TOKEN_LOGON_SID: i32 = 28;
        let mut token = null_mut();
        check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        // One TOKEN_GROUPS entry and its variable-length logon SID.
        let mut groups = [0usize; 32];
        let mut size = 0;
        check(unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TOKEN_LOGON_SID,
                groups.as_mut_ptr().cast(),
                size_of_val(&groups) as u32,
                &mut size,
            )
        })?;
        let group = unsafe { &*groups.as_ptr().cast::<TokenGroups>() };
        ensure!(
            group.count == 1 && !group.sid.is_null(),
            "missing logon SID"
        );
        let mut sid = null_mut();
        check(unsafe { ConvertSidToStringSidW(group.sid, &mut sid) })?;
        let sid = LocalAllocation(sid);
        let length = (0..)
            .find(|offset| unsafe { *sid.0.add(*offset) } == 0)
            .unwrap();
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid.0, length) })?;
        let sddl: Vec<u16> = format!("D:P(A;;GRGW;;;{sid})")
            .encode_utf16()
            .chain([0])
            .collect();
        let mut descriptor = null_mut();
        check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        })?;
        Ok(Self(LocalAllocation(descriptor)))
    }

    fn create_pipe(&self, path: &Path, first: bool) -> io::Result<NamedPipeServer> {
        let mut attributes = SecurityAttributes {
            length: size_of::<SecurityAttributes>() as u32,
            descriptor: self.0.0,
            inherit: 0,
        };
        // The call consumes these attributes synchronously and the handle is non-inheritable.
        unsafe {
            ServerOptions::new()
                .reject_remote_clients(true)
                .first_pipe_instance(first)
                .create_with_security_attributes_raw(
                    path,
                    (&mut attributes as *mut SecurityAttributes).cast(),
                )
        }
    }
}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
