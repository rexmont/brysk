//! File operations, the macOS half of `rf-win::fileops`.
//!
//! Copy and move are plain POSIX calls here (the Windows version goes through the shell's
//! IFileOperation for its undo record and Recycle-Bin behavior). The tracked variants keep the
//! same progress/cancel contract the Status Center uses; the parallel engine and byte-for-byte
//! verification are in `fastcopy`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Copy,
    Move,
}

impl Transfer {
    pub fn other(self) -> Transfer {
        match self {
            Transfer::Copy => Transfer::Move,
            Transfer::Move => Transfer::Copy,
        }
    }
}

/// Counts bytes for the progress bar.
pub struct ByteProgress {
    done: AtomicU64,
    total: AtomicU64,
}

impl Default for ByteProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl ByteProgress {
    pub fn new() -> Self {
        Self { done: AtomicU64::new(0), total: AtomicU64::new(0) }
    }
    pub fn set(&self, done: u64, total: u64) {
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }
    pub fn add(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }
    pub fn get(&self) -> (u64, u64) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }
}

/// Progress, cancellation, pause and rate limit for one transfer, as the Status Center holds
/// them. Same fields as the Windows tracker so `rf-app` is unchanged.
pub struct Tracker {
    pub bytes: Arc<ByteProgress>,
    pub on_progress: Box<dyn Fn(u32, u32, Option<String>) + Send + Sync>,
    pub cancel: Arc<AtomicBool>,
    pub results: std::sync::Mutex<Vec<(PathBuf, PathBuf)>>,
    pub paused: Arc<AtomicBool>,
    pub limit: AtomicU64,
    pub verify: AtomicU8,
    pub ask: Option<Box<dyn Fn(&[crate::fastcopy::Conflict]) -> Option<Vec<crate::fastcopy::Resolve>> + Send + Sync>>,
}

impl Tracker {
    pub fn set_verify(&self, v: crate::fastcopy::Verify) {
        self.verify.store(v as u8, Ordering::Relaxed);
    }
    pub fn verify_mode(&self) -> crate::fastcopy::Verify {
        match self.verify.load(Ordering::Relaxed) {
            1 => crate::fastcopy::Verify::Quick,
            2 => crate::fastcopy::Verify::Full,
            _ => crate::fastcopy::Verify::Off,
        }
    }

    pub fn new(on_progress: Box<dyn Fn(u32, u32, Option<String>) + Send + Sync>, cancel: Arc<AtomicBool>) -> Self {
        Self {
            bytes: Arc::new(ByteProgress::new()),
            on_progress,
            cancel,
            results: std::sync::Mutex::new(Vec::new()),
            paused: Arc::new(AtomicBool::new(false)),
            limit: AtomicU64::new(0),
            verify: AtomicU8::new(0),
            ask: None,
        }
    }
}

/// A name not taken in `dir`: "report.txt", "report (2).txt", …
pub fn unique_name(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let fmt = |n: u32| if n <= 1 { format!("{stem}{ext}") } else { format!("{stem} ({n}){ext}") };
    let mut n = 1;
    loop {
        let p = dir.join(fmt(n));
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

/// Renames `path` inside its folder. `false` when the name did not change.
pub fn rename(path: &Path, new_name: &str) -> anyhow::Result<bool> {
    let Some(parent) = path.parent() else { return Ok(false) };
    if path.file_name().is_some_and(|n| n == new_name) {
        return Ok(false);
    }
    let to = parent.join(new_name);
    if to.exists() {
        anyhow::bail!("Bu adda bir öğe zaten var");
    }
    std::fs::rename(path, &to)?;
    Ok(true)
}

/// Rename that never replaces an existing item (an atomic `RENAME_EXCL` on macOS is not
/// exposed by std; the check-then-rename is the same guarantee the Windows version has for
/// one process).
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "exists"));
    }
    std::fs::rename(from, to)
}

/// Copies or moves `sources` into `dest_dir` without progress (small drag-and-drop cases).
pub fn transfer(kind: Transfer, sources: &[PathBuf], dest_dir: &Path) -> anyhow::Result<bool> {
    for src in sources {
        let name = src.file_name().ok_or_else(|| anyhow::anyhow!("ad yok"))?;
        let dest = unique_name(dest_dir, &name.to_string_lossy(), "");
        match kind {
            Transfer::Copy => copy_recursive(src, &dest)?,
            Transfer::Move => {
                if std::fs::rename(src, &dest).is_err() {
                    copy_recursive(src, &dest)?;
                    remove_recursive(src)?;
                }
            }
        }
    }
    Ok(true)
}

/// The tracked variant used by the Status Center: reports bytes and honours cancel/pause.
pub fn transfer_tracked(kind: Transfer, sources: &[PathBuf], dest_dir: &Path, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    crate::fastcopy::transfer(kind, sources, dest_dir, tracker)
}

pub fn delete_tracked(paths: &[PathBuf], permanent: bool, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    for p in paths {
        if tracker.cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        if permanent {
            crate::fastdelete::delete_permanent(std::slice::from_ref(p), tracker.clone(), 1)?;
        } else {
            crate::recycle::trash(p)?;
        }
        tracker.results.lock().unwrap().push((p.clone(), PathBuf::new()));
    }
    Ok(true)
}

pub fn copy_recursive(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let md = std::fs::symlink_metadata(src)?;
    if md.file_type().is_symlink() {
        let target = std::fs::read_link(src)?;
        std::os::unix::fs::symlink(target, dest)?;
    } else if md.is_dir() {
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_recursive(&e.path(), &dest.join(e.file_name()))?;
        }
        let _ = std::fs::set_permissions(dest, md.permissions());
    } else {
        std::fs::copy(src, dest)?;
    }
    Ok(())
}

pub fn remove_recursive(src: &Path) -> anyhow::Result<()> {
    let md = std::fs::symlink_metadata(src)?;
    if md.is_dir() && !md.file_type().is_symlink() {
        std::fs::remove_dir_all(src)?;
    } else {
        std::fs::remove_file(src)?;
    }
    Ok(())
}
/// A new folder in `dir` ("New folder", "New folder (2)").
pub fn new_folder(dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let p = unique_name(dir, name, "");
    std::fs::create_dir(&p)?;
    Ok(p)
}

/// A new empty text file in `dir`.
pub fn new_text_file(dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let p = unique_name(dir, name, "");
    std::fs::write(&p, b"")?;
    Ok(p)
}
