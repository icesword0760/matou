use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
    thread,
};

use anyhow::{Context, Result, ensure};
use flate2::{Compression, write::GzEncoder};

use super::{
    codec::read_segment,
    index_writer::{file_options, sync_directory},
    range::Segment,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Job {
    session_id: String,
    index: u64,
    path: PathBuf,
}

#[derive(Default)]
struct Queue {
    queued: VecDeque<Job>,
    outstanding: HashSet<Job>,
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;
static QUEUE: OnceLock<Shared> = OnceLock::new();

fn queue() -> &'static Shared {
    QUEUE.get_or_init(|| {
        let shared: Shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        for _ in 0..2 {
            let worker = shared.clone();
            thread::spawn(move || {
                let (lock, wake) = &*worker;
                loop {
                    let mut state = wake
                        .wait_while(lock.lock().unwrap(), |state| state.queued.is_empty())
                        .unwrap();
                    let job = state.queued.pop_front().unwrap();
                    drop(state);
                    // A failed archive keeps the raw file. A later scan can retry it.
                    let _ = compress(&job.path);
                    lock.lock().unwrap().outstanding.remove(&job);
                    wake.notify_all();
                }
            });
        }
        shared
    })
}

pub(super) fn schedule(session_id: &str, segment: &Segment) {
    let job = Job {
        session_id: session_id.to_owned(),
        index: segment.index,
        path: segment.path.clone(),
    };
    let (lock, wake) = &**queue();
    let mut state = lock.lock().unwrap();
    if state.outstanding.insert(job.clone()) {
        state.queued.push_back(job);
        wake.notify_all();
    }
}

pub fn wait_for_compression() {
    if let Some(shared) = QUEUE.get() {
        let (lock, wake) = &**shared;
        drop(
            wake.wait_while(lock.lock().unwrap(), |state| !state.outstanding.is_empty())
                .unwrap(),
        );
    }
}

pub(super) fn candidates<'a>(
    descriptors: &'a [Segment],
    active: &Path,
    active_bytes: u64,
    hot_limit: u64,
    protected: &BTreeSet<u64>,
) -> Vec<&'a Segment> {
    let compressed: BTreeSet<_> = descriptors
        .iter()
        .filter(|entry| entry.compressed)
        .map(|entry| entry.index)
        .collect();
    let mut raw = BTreeMap::<u64, &Segment>::new();
    for entry in descriptors {
        if entry.compressed || compressed.contains(&entry.index) || entry.path == active {
            continue;
        }
        raw.entry(entry.index)
            .and_modify(|other| {
                if entry.path < other.path {
                    *other = entry;
                }
            })
            .or_insert(entry);
    }
    let mut hot = active_bytes;
    let mut hot_indexes = BTreeSet::new();
    for entry in raw.values().rev() {
        if (active_bytes > 0 || !hot_indexes.is_empty())
            && hot.saturating_add(entry.bytes) > hot_limit
        {
            break;
        }
        hot_indexes.insert(entry.index);
        hot = hot.saturating_add(entry.bytes);
    }
    raw.into_values()
        .filter(|entry| !hot_indexes.contains(&entry.index) && !protected.contains(&entry.index))
        .collect()
}

fn compress(path: &Path) -> Result<()> {
    ensure!(
        path.extension().is_some_and(|ext| ext == "mtj"),
        "journal compression requires a raw .mtj segment"
    );
    let gzip = path.with_extension("mtj.gz");
    let partial = path.with_extension("mtj.gz.partial");
    let directory = path.parent().context("journal segment has no parent")?;
    if fs::metadata(&gzip).is_ok_and(|metadata| metadata.len() > 0)
        && read_segment(&gzip).is_ok_and(|mut reader| reader.all(|frame| frame.is_ok()))
    {
        remove_file_if_present(path)?;
        sync_directory(directory)?;
        return Ok(());
    }
    remove_file_if_present(&gzip)?;
    remove_file_if_present(&partial)?;
    let finish = (|| -> Result<()> {
        let mut source = File::open(path)?;
        let dest = file_options().write(true).create_new(true).open(&partial)?;
        let mut stream = GzEncoder::new(dest, Compression::new(6));
        io::copy(&mut source, &mut stream)?;
        stream.finish()?.sync_all()?;
        fs::rename(&partial, &gzip)?;
        if let Err(error) = sync_directory(directory) {
            // Keep the raw source and let a later scan retry publication.
            let _ = remove_file_if_present(&gzip);
            return Err(error.into());
        }
        Ok(())
    })();
    if finish.is_err() {
        let _ = remove_file_if_present(&partial);
    }
    finish?;
    fs::remove_file(path)?;
    sync_directory(directory)?;
    Ok(())
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
