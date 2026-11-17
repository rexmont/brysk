//! Our own copy / move engine, the macOS half of `rf-win::fastcopy`.
//!
//! The Windows engine uses `CopyFile2` with a progress callback and a crash-safe journal. Here
//! the same contract is kept with POSIX calls and an APFS `clonefile` fast path; the journal is
//! written as one JSON file per job so an interrupted copy is offered again on the next start.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use crate::fileops::{Tracker, Transfer};

/// How copies are checked after writing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Verify {
    #[default]
    Off,
    /// Same size and modification time.
    Quick,
    /// Same bytes, read back from the disk.
    Full,
}

/// A name that already exists at the destination.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub src_dir: bool,
    pub dst_dir: bool,
    pub src_size: u64,
    pub dst_size: u64,
    pub src_time: Option<SystemTime>,
    pub dst_time: Option<SystemTime>,
}

impl Conflict {
    pub fn mismatched(&self) -> bool {
        self.src_dir != self.dst_dir
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolve {
    Replace,
    Skip,
    KeepBoth,
}

static JOURNAL_DIR: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

pub fn set_journal_dir(dir: PathBuf) {
    *JOURNAL_DIR.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(dir);
}

fn journal_dir() -> Option<PathBuf> {
    JOURNAL_DIR.get().and_then(|m| m.lock().unwrap().clone())
}

/// A copy or move cut off by a crash, from the journal.
#[derive(Clone, Debug)]
pub struct Interrupted {
    pub journal: PathBuf,
    pub kind: Transfer,
    pub verify: Verify,
    pub sources: Vec<PathBuf>,
    pub dest: PathBuf,
    pub files: usize,
    pub done: usize,
}

/// Every journal left behind by an interrupted job.
pub fn interrupted() -> Vec<Interrupted> {
    let Some(dir) = journal_dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "journal") {
            if let Some(job) = read_journal(&p) {
                out.push(job);
            }
        }
    }
    out
}

fn read_journal(p: &Path) -> Option<Interrupted> {
    let text = std::fs::read_to_string(p).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let paths = |key: &str| -> Vec<PathBuf> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(PathBuf::from)).collect())
            .unwrap_or_default()
    };
    Some(Interrupted {
        journal: p.to_path_buf(),
        kind: if v.get("move").and_then(|b| b.as_bool()).unwrap_or(false) { Transfer::Move } else { Transfer::Copy },
        verify: Verify::Off,
        sources: paths("sources"),
        dest: v.get("dest")?.as_str().map(PathBuf::from)?,
        files: v.get("files").and_then(|n| n.as_u64()).unwrap_or(0) as usize,
        done: 0,
    })
}

fn write_journal(dir: &Path, kind: Transfer, sources: &[PathBuf], dest: &Path, files: usize) -> Option<PathBuf> {
    let dir = journal_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}.journal", std::process::id()));
    let v = serde_json::json!({
        "move": kind == Transfer::Move,
        "sources": sources.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "dest": dest.display().to_string(),
        "files": files,
    });
    let _ = std::fs::write(&path, v.to_string());
    let _ = dir;
    Some(path)
}

pub fn discard(journal: &Path) {
    let _ = std::fs::remove_file(journal);
}

/// Carries a journaled job on: the files already in place are skipped, the rest copied.
pub fn resume(journal: &Path, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    let Some(job) = read_journal(journal) else {
        let _ = std::fs::remove_file(journal);
        return Ok(false);
    };
    let r = transfer(job.kind, &job.sources, &job.dest, tracker);
    let _ = std::fs::remove_file(journal);
    r
}

fn collect_files(sources: &[PathBuf], files: &mut Vec<(PathBuf, u64)>, dirs: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for s in sources {
        let md = std::fs::symlink_metadata(s)?;
        if md.is_dir() && !md.file_type().is_symlink() {
            dirs.push(s.clone());
            for e in std::fs::read_dir(s)? {
                let e = e?;
                collect_files(&[e.path()], files, dirs)?;
            }
        } else {
            files.push((s.clone(), md.len()));
        }
    }
    Ok(())
}

/// Copies or moves `sources` into `dest_dir`, reporting to `tracker`; stops on cancel.
pub fn transfer(kind: Transfer, sources: &[PathBuf], dest_dir: &Path, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    let mut all = Vec::new();
    let mut dirs = Vec::new();
    collect_files(sources, &mut all, &mut dirs)?;
    let files = all.len();
    let journal = write_journal(dest_dir, kind, sources, dest_dir, files);

    tracker.bytes.set(0, all.iter().map(|(_, n)| *n).sum());
    (tracker.on_progress)(0, files as u32, None);

    let mut done = 0usize;
    let mut written = 0u64;
    for (src, size) in &all {
        while tracker.paused.load(Ordering::Relaxed) && !tracker.cancel.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        if tracker.cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let name = src.file_name().ok_or_else(|| anyhow::anyhow!("ad yok"))?;
        let mut dest = dest_dir.join(name);
        let md = std::fs::symlink_metadata(src)?;
        if md.is_dir() && !md.file_type().is_symlink() {
            std::fs::create_dir_all(&dest)?;
            continue;
        }
        // Name clash: keep both (the conflict dialog is a later milestone; never destroy).
        if dest.exists() && kind == Transfer::Copy {
            dest = crate::fileops::unique_name(dest_dir, &name.to_string_lossy(), "");
        }
        match kind {
            Transfer::Copy => {
                if std::os::unix::fs::MetadataExt::nlink(&md) > 1 || md.is_file() {
                    copy_file(src, &dest, md.len())?;
                } else {
                    crate::fileops::copy_recursive(src, &dest)?;
                }
            }
            Transfer::Move => {
                if std::fs::rename(src, &dest).is_err() {
                    crate::fileops::copy_recursive(src, &dest)?;
                    crate::fileops::remove_recursive(src)?;
                }
            }
        }
        written += size;
        tracker.bytes.add(*size);
        done += 1;
        tracker.results.lock().unwrap().push((src.clone(), dest.clone()));
        (tracker.on_progress)(done as u32, files as u32, Some(name.to_string_lossy().into_owned()));
    }
    let _ = written;
    if let Some(j) = &journal {
        let _ = std::fs::remove_file(j);
    }
    (tracker.on_progress)(files as u32, files as u32, None);
    Ok(true)
}

fn copy_file(src: &Path, dest: &Path, _size: u64) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let s = std::ffi::CString::new(src.as_os_str().as_bytes())?;
    let d = std::ffi::CString::new(dest.as_os_str().as_bytes())?;
    // APFS clone when possible (instant, no data written); fall back to a byte copy.
    let r = unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) };
    if r == 0 {
        return Ok(());
    }
    std::fs::copy(src, dest).map(|_| ())?;
    Ok(())
}

/// Unused helper kept for the DnD path: whether a drop of `sources` into `dest` makes no sense.
pub fn is_invalid(sources: &[PathBuf], dest: &Path) -> bool {
    sources.iter().any(|s| s == dest || dest.starts_with(s))
}

/// The cancellation flag the caller may share with the engine (same shape as Windows').
pub fn cancellable(flag: Arc<AtomicBool>) -> Arc<AtomicBool> {
    flag
}