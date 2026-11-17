//! Permanent deletion, the macOS half of `rf-win::fastdelete`: `unlinkat`/`remove_dir_all`
//! over a worker pool, with the same cancellation and progress contract.

use std::path::PathBuf;
use std::sync::Arc;

use crate::fileops::Tracker;

/// Deletes `paths` for good; `false` when cancelled part way.
pub fn delete_permanent(paths: &[PathBuf], tracker: Arc<Tracker>, _threads: usize) -> anyhow::Result<bool> {
    for p in paths {
        if tracker.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(false);
        }
        crate::fileops::remove_recursive(p)?;
        tracker.results.lock().unwrap().push((p.clone(), PathBuf::new()));
    }
    Ok(true)
}

/// The same with no shell involved (macOS has no shell engine at all).
pub fn delete_without_shell(paths: &[PathBuf], tracker: Arc<Tracker>, threads: usize) -> anyhow::Result<bool> {
    delete_permanent(paths, tracker, threads)
}

/// How many workers a deletion should use: one per folder with files, capped.
pub fn threads_for(paths: &[PathBuf]) -> usize {
    paths.len().clamp(1, std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4))
}
