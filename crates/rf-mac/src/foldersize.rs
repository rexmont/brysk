//! Folder sizes, the macOS half of `rf-win::foldersize`: the Size column, the preview pane and
//! Properties share one cache, and measuring walks the tree on background threads.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

pub use crate::hash::Cancelled;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderSize {
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
    pub allocated: Option<u64>,
}

impl Default for FolderSize {
    fn default() -> Self {
        Self { bytes: 0, files: 0, dirs: 0, allocated: None }
    }
}

fn key(dir: &Path) -> String {
    dir.to_string_lossy().trim_end_matches('/').to_lowercase()
}

fn dir_mtime(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

struct CacheEntry {
    mtime: Option<SystemTime>,
    size: FolderSize,
    seq: u64,
    provisional: bool,
}

pub struct FolderSizeCache {
    map: Mutex<HashMap<String, CacheEntry>>,
    seq: AtomicU64,
    capacity: usize,
    path: Mutex<Option<PathBuf>>,
}

impl Default for FolderSizeCache {
    fn default() -> Self {
        Self::new(PERSIST_LIMIT)
    }
}

pub const PERSIST_LIMIT: usize = 20_000;

impl FolderSizeCache {
    pub fn new(capacity: usize) -> Self {
        Self { map: Mutex::new(HashMap::new()), seq: AtomicU64::new(1), capacity: capacity.max(16), path: Mutex::new(None) }
    }

    pub fn get(&self, dir: &Path) -> Option<FolderSize> {
        let mtime = dir_mtime(dir)?;
        let map = self.map.lock().unwrap();
        let e = map.get(&key(dir))?;
        (!e.provisional && e.mtime == Some(mtime)).then_some(e.size)
    }

    pub fn provisional(&self, dir: &Path) -> Option<FolderSize> {
        let map = self.map.lock().unwrap();
        map.get(&key(dir)).filter(|e| e.provisional).map(|e| e.size)
    }

    pub fn insert(&self, dir: &Path, size: FolderSize) {
        self.insert_at(dir, size, dir_mtime(dir), false);
    }

    fn insert_at(&self, dir: &Path, size: FolderSize, mtime: Option<SystemTime>, provisional: bool) {
        let mut map = self.map.lock().unwrap();
        if map.len() >= self.capacity {
            let mut seqs: Vec<u64> = map.values().map(|e| e.seq).collect();
            seqs.sort_unstable();
            let cut = seqs[seqs.len() / 2];
            map.retain(|_, e| e.seq > cut);
        }
        map.insert(key(dir), CacheEntry { mtime, size, seq: self.seq.fetch_add(1, Ordering::Relaxed), provisional });
    }

    pub fn invalidate(&self, dir: &Path) {
        let k = key(dir);
        let prefix = format!("{k}/");
        self.map.lock().unwrap().retain(|p, _| *p != k && !p.starts_with(&prefix));
    }

    pub fn clear(&self) {
        self.map.lock().unwrap().clear();
    }

    pub fn get_or_compute(
        &self,
        dir: &Path,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(&FolderSize),
    ) -> anyhow::Result<FolderSize> {
        if let Some(s) = self.get(dir) {
            progress(&s);
            return Ok(s);
        }
        let before = dir_mtime(dir);
        let size = folder_size(dir, cancelled, progress)?;
        self.insert_at(dir, size, before, false);
        Ok(size)
    }

    /// Where the cache lives: loaded now, written on a timer.
    pub fn persist(&self, path: PathBuf) {
        if let Some(loaded) = read_cache(&path) {
            for (k, s) in loaded {
                let mtime = dir_mtime(Path::new(&k));
                self.insert_at(Path::new(&k), s, mtime, true);
            }
        }
        *self.path.lock().unwrap() = Some(path.clone());
        let me = self as *const FolderSizeCache as usize;
        std::thread::Builder::new().name("foldersize-save".into()).spawn(move || loop {
            std::thread::sleep(Duration::from_secs(30));
            // SAFETY: `shared_cache()` is a process-lifetime `OnceLock`.
            let cache = unsafe { &*(me as *const FolderSizeCache) };
            if let Some(p) = cache.path.lock().unwrap().clone() {
                let _ = cache.save_to(&p, PERSIST_LIMIT);
            }
        }).ok();
    }

    pub fn save_to(&self, path: &Path, limit: usize) -> std::io::Result<()> {
        let rows: Vec<(u64, String, FolderSize)> = {
            let map = self.map.lock().unwrap();
            map.iter().map(|(k, e)| (e.seq, k.clone(), e.size)).collect()
        };
        let mut rows = rows;
        rows.sort_unstable_by_key(|r| r.0);
        let skip = rows.len().saturating_sub(limit);
        let mut text = String::from("rustyfiles foldersizes 1\n");
        for (idx, (_, k, s)) in rows[skip..].iter().enumerate() {
            let _ = idx;
            text.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", s.bytes, s.files, s.dirs, s.allocated.unwrap_or(0), k));
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, text)
    }
}

fn read_cache(path: &Path) -> Option<Vec<(String, FolderSize)>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let mut it = line.splitn(5, '\t');
        let bytes = it.next()?.parse().ok()?;
        let files = it.next()?.parse().ok()?;
        let dirs = it.next()?.parse().ok()?;
        let allocated = it.next()?.parse::<u64>().ok()?;
        let k = it.next()?.to_string();
        out.push((k, FolderSize { bytes, files, dirs, allocated: (allocated > 0).then_some(allocated) }));
    }
    Some(out)
}

/// Folder sizes shared by the Size column, the preview pane and Properties.
pub fn shared_cache() -> &'static FolderSizeCache {
    static CACHE: OnceLock<FolderSizeCache> = OnceLock::new();
    CACHE.get_or_init(FolderSizeCache::default)
}

/// State of one folder in a [`SizeBatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeState {
    Partial(u64),
    Provisional(FolderSize),
    Done(FolderSize),
}

struct Slot {
    bytes: AtomicU64,
    done: AtomicBool,
    seed: Option<FolderSize>,
    result: Mutex<Option<FolderSize>>,
}

/// Sizes of several folders, measured together by background threads.
pub struct SizeBatch {
    slots: Vec<Slot>,
    stop: AtomicBool,
}

impl SizeBatch {
    pub fn start(dirs: Vec<PathBuf>) -> Arc<Self> {
        let b = Arc::new(SizeBatch {
            slots: dirs.iter().map(|d| Slot { bytes: AtomicU64::new(0), done: AtomicBool::new(false), seed: shared_cache().provisional(d), result: Mutex::new(None) }).collect(),
            stop: AtomicBool::new(false),
        });
        for (i, dir) in dirs.into_iter().enumerate() {
            let batch = b.clone();
            std::thread::Builder::new().name("foldersize".into()).spawn(move || {
                let mut size = FolderSize::default();
                walk(&dir, &mut size, &batch);
                shared_cache().insert(&dir, size);
                batch.slots[i].bytes.store(size.bytes, Ordering::Relaxed);
                *batch.slots[i].result.lock().unwrap() = Some(size);
                batch.slots[i].done.store(true, Ordering::Release);
            }).ok();
        }
        b
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn state(&self, i: usize) -> SizeState {
        let s = &self.slots[i];
        if s.done.load(Ordering::Acquire) {
            SizeState::Done(self.totals(i))
        } else if let Some(seed) = s.seed {
            SizeState::Provisional(seed)
        } else {
            SizeState::Partial(s.bytes.load(Ordering::Relaxed))
        }
    }

    pub fn totals(&self, i: usize) -> FolderSize {
        let s = &self.slots[i];
        s.result.lock().unwrap().unwrap_or(FolderSize { bytes: s.bytes.load(Ordering::Relaxed), ..FolderSize::default() })
    }

    pub fn finished(&self) -> bool {
        self.slots.iter().all(|s| s.done.load(Ordering::Acquire))
    }

    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn walk(dir: &Path, size: &mut FolderSize, batch: &SizeBatch) {
    if batch.stop.load(Ordering::Relaxed) {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
        if md.is_dir() && !md.file_type().is_symlink() {
            size.dirs += 1;
            walk(&e.path(), size, batch);
        } else {
            size.files += 1;
            size.bytes += md.len();
            size.allocated = Some(size.allocated.unwrap_or(0) + md.blocks() * 512);
        }
        if batch.stop.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// Totals for everything under `dir` (blocking: call off the UI thread).
pub fn folder_size(dir: &Path, cancelled: &dyn Fn() -> bool, progress: &mut dyn FnMut(&FolderSize)) -> anyhow::Result<FolderSize> {
    anyhow::ensure!(dir.is_dir(), "Klasör bulunamadı: {}", dir.display());
    shared_cache().invalidate(dir);
    let b = SizeBatch::start(vec![dir.to_path_buf()]);
    let mut since = std::time::Instant::now();
    while !b.finished() {
        if cancelled() {
            b.cancel();
            return Err(Cancelled.into());
        }
        std::thread::sleep(Duration::from_millis(5));
        if since.elapsed() >= Duration::from_millis(100) {
            since = std::time::Instant::now();
            progress(&b.totals(0));
        }
    }
    let total = b.totals(0);
    progress(&total);
    Ok(total)
}