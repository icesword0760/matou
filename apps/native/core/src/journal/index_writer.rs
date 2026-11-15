use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::Serialize;

use super::tail_index::TailIndexSnapshot;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub(crate) fn file_options() -> OpenOptions {
    let opts = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = opts;
        opts.mode(0o600);
        opts
    }
    #[cfg(windows)]
    opts
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return fs::File::open(path)?.sync_all();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
        OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?
            .sync_all()
    }
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize, tail_index: bool) -> Result<()> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".tmp-{}-{stamp}-{sequence}", std::process::id()));
    let temporary = PathBuf::from(name);
    let mut file = file_options()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let written = (|| {
        serde_json::to_writer(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok::<_, anyhow::Error>(())
    })();
    drop(file);
    let result = written.and_then(|()| {
        fs::rename(&temporary, path)?;
        if let Err(error) =
            sync_directory(path.parent().context("sidecar has no parent directory")?)
            && !(cfg!(windows) && tail_index)
        {
            return Err(error.into());
        }
        Ok(())
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Default)]
struct Pending {
    newest: Option<TailIndexSnapshot>,
    writing: bool,
    stopped: bool,
}

type Shared = Arc<(Mutex<Pending>, Condvar)>;

pub(super) struct IndexWriter {
    shared: Shared,
    handle: Option<JoinHandle<()>>,
}

impl IndexWriter {
    pub(super) fn new(path: PathBuf) -> Result<Self> {
        let shared: Shared = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let reader = shared.clone();
        let handle = thread::Builder::new()
            .name("journal-tail-index".into())
            .spawn(move || {
                let (lock, wake) = &*reader;
                loop {
                    let mut pending = wake
                        .wait_while(lock.lock().unwrap(), |job| {
                            job.newest.is_none() && !job.stopped
                        })
                        .unwrap();
                    let Some(snapshot) = pending.newest.take() else {
                        break;
                    };
                    pending.writing = true;
                    drop(pending);
                    // This cache is best effort. A later snapshot can replace it.
                    let _ = write_json(&path, &snapshot, true);
                    let mut pending = lock.lock().unwrap();
                    pending.writing = false;
                    wake.notify_all();
                }
            })?;
        Ok(Self {
            shared,
            handle: Some(handle),
        })
    }

    pub(super) fn schedule(&self, snapshot: TailIndexSnapshot) {
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap().newest = Some(snapshot);
        wake.notify_all();
    }

    pub(super) fn wait(&self) {
        let (lock, wake) = &*self.shared;
        drop(
            wake.wait_while(lock.lock().unwrap(), |job| {
                job.writing || job.newest.is_some()
            })
            .unwrap(),
        );
    }

    pub(super) fn close(&mut self) {
        self.wait();
        self.stop();
    }

    fn stop(&mut self) {
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap().stopped = true;
        wake.notify_all();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for IndexWriter {
    fn drop(&mut self) {
        self.stop();
    }
}
