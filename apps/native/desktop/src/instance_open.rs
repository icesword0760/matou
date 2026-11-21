//! The desktop owner is elected before it opens a window or starts a Runtime.
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};

use crate::runtime_bridge::LaunchOptions;

pub struct DesktopOptions {
    pub runtime: LaunchOptions,
    pub paths: Vec<String>,
}

impl DesktopOptions {
    pub fn from_args(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut paths = Vec::new();
        let mut runtime = Vec::new();
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .context("native launcher requires a path after each flag")?;
            if flag == "--open-workspace" {
                let path = value
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("workspace path is not UTF-8"))?;
                if !path.trim().is_empty() {
                    paths.push(absolute_workspace_path(path.trim())?);
                }
            } else {
                runtime.extend([flag, value]);
            }
        }
        Ok(Self {
            runtime: LaunchOptions::from_args(runtime)?,
            paths,
        })
    }
}

pub fn absolute_workspace_path(path: &str) -> Result<String> {
    let mut resolved = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    resolved
        .into_os_string()
        .into_string()
        .map_err(|_| anyhow::anyhow!("workspace path is not UTF-8"))
}

#[derive(Debug)]
pub enum OpenRequest {
    // Even an invalid explicit path brings the first window forward on a second launch.
    SecondLaunch(Vec<String>),
    Directories(Vec<String>),
    Reopen,
}

impl OpenRequest {
    pub fn existing_directories(mut self) -> Self {
        match &mut self {
            Self::Directories(paths) | Self::SecondLaunch(paths) => {
                paths.retain(|path| Path::new(path).is_dir());
            }
            Self::Reopen => {}
        }
        self
    }
}

const MAX_REQUEST: usize = 64 * 1024;
const ACK: u8 = 1;
const RETRY: Duration = Duration::from_millis(20);
const FORWARD_LIMIT: Duration = Duration::from_secs(2);
const REQUEST_LIMIT: Duration = Duration::from_millis(500);

pub enum Instance {
    Primary(InstanceOwner),
    Forwarded,
}

pub struct InstanceOwner {
    sender: async_channel::Sender<OpenRequest>,
    stop: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
    #[cfg(unix)]
    wake: std::os::unix::net::UnixStream,
    _lock: File,
}

impl InstanceOwner {
    pub fn stop_forwarding(&self) {
        self.sender.close();
        self.stop.store(true, Ordering::Release);
        #[cfg(unix)]
        let _ = self.wake.shutdown(std::net::Shutdown::Write);
        #[cfg(windows)]
        if let Some(listener) = &self.listener {
            listener.thread().unpark();
        }
    }
}

impl Drop for InstanceOwner {
    fn drop(&mut self) {
        self.stop_forwarding();
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

pub fn claim(
    data_root: &Path,
    paths: &[String],
    sender: async_channel::Sender<OpenRequest>,
) -> Result<Instance> {
    fs::create_dir_all(data_root).context("create native data root")?;
    let root = data_root
        .canonicalize()
        .context("locate native data root")?;
    let endpoint = transport::Endpoint::new(&root)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(endpoint.lock_path())
        .context("open desktop instance lock")?;
    let deadline = Instant::now() + FORWARD_LIMIT;
    loop {
        match lock.try_lock() {
            Ok(()) => {
                let stop = Arc::new(AtomicBool::new(false));
                #[cfg(unix)]
                let (wake, wake_reader) = std::os::unix::net::UnixStream::pair()?;
                let listener = endpoint.listen(
                    sender.clone(),
                    stop.clone(),
                    #[cfg(unix)]
                    wake_reader,
                )?;
                return Ok(Instance::Primary(InstanceOwner {
                    sender,
                    stop,
                    listener: Some(listener),
                    #[cfg(unix)]
                    wake,
                    _lock: lock,
                }));
            }
            Err(TryLockError::Error(error)) => {
                return Err(error).context("lock native desktop instance");
            }
            Err(TryLockError::WouldBlock) => {}
        }
        match endpoint.connect() {
            Ok(mut stream) => {
                // Once connected the owner may have received the request. Never start a
                // second Runtime or retry the request after an ambiguous acknowledgement.
                let body = serde_json::to_vec(&(root.as_os_str().as_encoded_bytes(), paths))?;
                ensure!(
                    body.len() <= MAX_REQUEST,
                    "desktop open request is too large"
                );
                stream
                    .write_all(&(body.len() as u32).to_be_bytes())
                    .context("send desktop open request length")?;
                stream
                    .write_all(&body)
                    .context("send desktop open request")?;
                let mut reply = [0];
                stream
                    .read_exact(&mut reply)
                    .context("wait for desktop open acknowledgement")?;
                ensure!(reply == [ACK], "primary desktop rejected the open request");
                return Ok(Instance::Forwarded);
            }
            Err(error) if transport::retryable(&error) && Instant::now() < deadline => {
                thread::sleep(RETRY)
            }
            Err(error) => return Err(error).context("forward to primary desktop instance"),
        }
    }
}

fn receive(
    stream: &mut (impl Read + Write),
    root: &Path,
    sender: &async_channel::Sender<OpenRequest>,
) -> Result<()> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    ensure!(length <= MAX_REQUEST, "desktop open request is too large");
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    let (data_root, paths): (Vec<u8>, Vec<String>) = serde_json::from_slice(&body)?;
    ensure!(
        root.as_os_str().as_encoded_bytes() == data_root,
        "desktop open request data root differs"
    );
    ensure!(
        paths.iter().all(|path| Path::new(path).is_absolute()),
        "workspace path is relative"
    );
    sender
        .try_send(OpenRequest::SecondLaunch(paths))
        .context("queue desktop open request")?;
    stream.write_all(&[ACK])?;
    Ok(())
}

#[cfg(unix)]
mod transport {
    use super::*;
    use anyhow::bail;
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, FileTypeExt, MetadataExt},
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    };

    #[repr(C)]
    struct PollFd {
        fd: std::ffi::c_int,
        events: std::ffi::c_short,
        revents: std::ffi::c_short,
    }

    unsafe extern "C" {
        fn geteuid() -> u32;
        // Darwin uses an unsigned int for nfds_t.
        fn poll(fds: *mut PollFd, count: u32, timeout_ms: i32) -> i32;
    }

    const POLLIN: i16 = 0x0001;

    pub struct Endpoint {
        root: PathBuf,
        dir: PathBuf,
        socket: PathBuf,
    }

    impl Endpoint {
        pub fn new(root: &Path) -> Result<Self> {
            let dir = root.join(".instance");
            private_dir(&dir)?;
            let mut socket = dir.join("open.sock");
            // Darwin's sockaddr_un has 104 bytes for the path, including NUL.
            if socket.as_os_str().as_bytes().len() >= 104 {
                let short = PathBuf::from(format!("/tmp/matou-instance-{}", unsafe { geteuid() }));
                private_dir(&short)?;
                socket = short.join(format!(
                    "{:016x}.sock",
                    root_key(root.as_os_str().as_bytes().iter().copied())
                ));
            }
            Ok(Self {
                root: root.into(),
                dir,
                socket,
            })
        }

        pub fn lock_path(&self) -> PathBuf {
            self.dir.join("lock")
        }

        pub fn connect(&self) -> io::Result<UnixStream> {
            let stream = UnixStream::connect(&self.socket)?;
            stream.set_read_timeout(Some(REQUEST_LIMIT))?;
            stream.set_write_timeout(Some(REQUEST_LIMIT))?;
            Ok(stream)
        }

        pub fn listen(
            self,
            sender: async_channel::Sender<OpenRequest>,
            stop: Arc<AtomicBool>,
            wake: UnixStream,
        ) -> Result<JoinHandle<()>> {
            match self.socket.symlink_metadata() {
                // The owner lock excludes peers using this root. A different root key or
                // leftover socket must never replace an endpoint that still accepts connections.
                Ok(info) if info.file_type().is_socket() => match UnixStream::connect(&self.socket)
                {
                    Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                        fs::remove_file(&self.socket)?
                    }
                    Err(error) => return Err(error).context("inspect existing desktop socket"),
                    Ok(_) => bail!("desktop socket already has a listener"),
                },
                Ok(_) => bail!("desktop socket name is occupied by a non-socket"),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("inspect desktop socket location"),
            }
            let listener =
                UnixListener::bind(&self.socket).context("bind private desktop socket")?;
            let socket = SocketFile(self.socket);
            listener.set_nonblocking(true)?;
            Ok(thread::Builder::new()
                .name("matou-instance-open".into())
                .spawn(move || {
                    let _socket = socket;
                    let mut fds = [
                        PollFd {
                            fd: listener.as_raw_fd(),
                            events: POLLIN,
                            revents: 0,
                        },
                        PollFd {
                            fd: wake.as_raw_fd(),
                            events: POLLIN,
                            revents: 0,
                        },
                    ];
                    while !stop.load(Ordering::Acquire) {
                        if unsafe { poll(fds.as_mut_ptr(), 2, -1) } < 0 {
                            let error = io::Error::last_os_error();
                            if error.kind() == io::ErrorKind::Interrupted {
                                continue;
                            }
                            eprintln!("Wait for desktop open request: {error}");
                            break;
                        }
                        if fds[1].revents != 0 {
                            break;
                        }
                        match listener.accept() {
                            Ok((mut stream, _)) => {
                                let result = stream
                                    .set_nonblocking(false)
                                    .and_then(|()| stream.set_read_timeout(Some(REQUEST_LIMIT)))
                                    .and_then(|()| stream.set_write_timeout(Some(REQUEST_LIMIT)))
                                    .map_err(anyhow::Error::from)
                                    .and_then(|()| receive(&mut stream, &self.root, &sender));
                                if let Err(error) = result {
                                    eprintln!("Receive desktop open request: {error:#}");
                                }
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                                ) => {}
                            Err(error) => {
                                eprintln!("Accept desktop open request: {error}");
                                break;
                            }
                        }
                    }
                })?)
        }
    }

    struct SocketFile(PathBuf);
    impl Drop for SocketFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn private_dir(path: &Path) -> Result<()> {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context("create private desktop channel directory"),
        }
        let info = path.symlink_metadata()?;
        ensure!(
            info.is_dir() && info.uid() == unsafe { geteuid() } && info.mode() & 0o077 == 0,
            "desktop channel directory must be private and owned by the current user"
        );
        Ok(())
    }

    pub fn retryable(error: &io::Error) -> bool {
        matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused | io::ErrorKind::WouldBlock
        )
    }
}

// A fixed hash keeps endpoint names stable across compiler and application upgrades.
// The wire message also checks the complete canonical data root before admitting a request.
fn root_key(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(windows)]
mod transport {
    use super::*;
    use std::{
        mem::{size_of, zeroed},
        os::windows::{
            ffi::OsStrExt,
            fs::OpenOptionsExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING,
            INVALID_HANDLE_VALUE,
        },
        Security::{
            ACL, ACL_REVISION, AddAccessAllowedAce, GetTokenInformation, InitializeAcl,
            InitializeSecurityDescriptor, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
            SetSecurityDescriptorDacl, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
        },
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_NOWAIT,
                PIPE_REJECT_REMOTE_CLIENTS, SetNamedPipeHandleState,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    };

    pub struct Endpoint {
        root: PathBuf,
        pipe: String,
    }

    impl Endpoint {
        pub fn new(root: &Path) -> Result<Self> {
            let key = root_key(root.as_os_str().encode_wide().flat_map(u16::to_le_bytes));
            Ok(Self {
                root: root.into(),
                pipe: format!(r"\\.\pipe\matou-desktop-open-{key:016x}"),
            })
        }

        pub fn lock_path(&self) -> PathBuf {
            self.root.join(".desktop-instance-lock")
        }

        pub fn connect(&self) -> io::Result<Pipe<File>> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .security_qos_flags(SECURITY_IDENTIFICATION | SECURITY_SQOS_PRESENT)
                .open(&self.pipe)?;
            // Nonblocking on both ends so a failed exchange has a bounded wait.
            check(unsafe {
                SetNamedPipeHandleState(file.as_raw_handle(), &PIPE_NOWAIT, null(), null())
            })?;
            Ok(Pipe::new(file))
        }

        pub fn listen(
            self,
            sender: async_channel::Sender<OpenRequest>,
            stop: Arc<AtomicBool>,
        ) -> Result<JoinHandle<()>> {
            let file = private_pipe(&self.pipe)?;
            Ok(thread::Builder::new()
                .name("matou-instance-open".into())
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        let handle = file.as_raw_handle();
                        match check(unsafe { ConnectNamedPipe(handle, null_mut()) }) {
                            Err(error) if code(&error) == ERROR_PIPE_CONNECTED => {
                                let mut pipe = Pipe::new(&file);
                                match receive(&mut pipe, &self.root, &sender) {
                                    Ok(()) => {
                                        // Windows discards unread bytes on disconnect. The client
                                        // closes after reading the acknowledgement.
                                        let _ = pipe.read(&mut [0]);
                                    }
                                    Err(error) => {
                                        eprintln!("Receive desktop open request: {error:#}")
                                    }
                                }
                                unsafe {
                                    DisconnectNamedPipe(handle);
                                }
                            }
                            Err(error) if code(&error) == ERROR_NO_DATA => unsafe {
                                DisconnectNamedPipe(handle);
                            },
                            Err(error) if code(&error) == ERROR_PIPE_LISTENING => {
                                thread::park_timeout(RETRY)
                            }
                            Ok(()) => {}
                            Err(error) => {
                                eprintln!("Accept desktop open request: {error}");
                                break;
                            }
                        }
                    }
                })?)
        }
    }

    pub struct Pipe<T> {
        file: T,
        deadline: Instant,
    }

    impl<T: std::borrow::Borrow<File>> Pipe<T> {
        fn new(file: T) -> Self {
            Self {
                file,
                deadline: Instant::now() + REQUEST_LIMIT,
            }
        }

        fn transfer(
            &mut self,
            retry_zero: bool,
            mut op: impl FnMut(&File) -> io::Result<usize>,
        ) -> io::Result<usize> {
            loop {
                match op(self.file.borrow()) {
                    Err(error) if code(&error) == ERROR_NO_DATA => {}
                    Ok(0) if retry_zero => {}
                    result => return result,
                }
                if Instant::now() >= self.deadline {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    impl<T: std::borrow::Borrow<File>> Read for Pipe<T> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.transfer(false, |mut file| file.read(buffer))
        }
    }

    impl<T: std::borrow::Borrow<File>> Write for Pipe<T> {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.transfer(true, |mut file| file.write(buffer))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn private_pipe(pipe: &str) -> Result<File> {
        let mut token = null_mut();
        check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        // An aligned buffer stores TOKEN_USER and the variable-length SID it references.
        let mut user = [0usize; 32];
        let mut bytes = 0;
        check(unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                user.as_mut_ptr().cast(),
                size_of::<[usize; 32]>() as u32,
                &mut bytes,
            )
        })?;
        let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut acl = [0u32; 32];
        let mut descriptor: SECURITY_DESCRIPTOR = unsafe { zeroed() };
        let acl_ptr = acl.as_mut_ptr().cast::<ACL>();
        let descriptor_ptr = (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast();
        check(unsafe { InitializeAcl(acl_ptr, size_of::<[u32; 32]>() as u32, ACL_REVISION) })?;
        check(unsafe {
            AddAccessAllowedAce(
                acl_ptr,
                ACL_REVISION,
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                sid,
            )
        })?;
        check(unsafe { InitializeSecurityDescriptor(descriptor_ptr, 1) })?;
        check(unsafe { SetSecurityDescriptorDacl(descriptor_ptr, 1, acl_ptr, 0) })?;
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor_ptr,
            bInheritHandle: 0,
        };
        let pipe: Vec<u16> = pipe.encode_utf16().chain([0]).collect();
        let handle = unsafe {
            CreateNamedPipeW(
                pipe.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                16,
                MAX_REQUEST as u32 + 4,
                0,
                &security,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error()).context("create private desktop named pipe");
        }
        Ok(unsafe { File::from_raw_handle(handle) })
    }

    fn check(ok: i32) -> io::Result<()> {
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn code(error: &io::Error) -> u32 {
        error.raw_os_error().unwrap_or_default() as u32
    }

    pub fn retryable(error: &io::Error) -> bool {
        error.kind() == io::ErrorKind::NotFound || code(error) == ERROR_PIPE_BUSY
    }
}
