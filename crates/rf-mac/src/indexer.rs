//! The name index behind quick search, the macOS half of `rf-win::indexer`.
//!
//! A background walk of the user's folders builds an in-memory map from a lower-case name to
//! the places it exists (the Windows version also persists it and replays the NTFS change
//! journal; here the walk is redone at start, a few seconds after the window is up). FSEvents
//! (`notify`, recursive) keeps it current while the app runs: a changed name is replaced or
//! removed one at a time, and a dropped-event burst rebuilds the affected root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use notify::Watcher;

/// Nothing usable yet: the first walk is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Building = 0,
    Ready = 1,
    Failed = 2,
}

struct Rec {
    dir: u32,
    name: Box<str>,
}

struct Index {
    dirs: Vec<Arc<Path>>,
    recs: Vec<Rec>,
    by_name: HashMap<String, Vec<u32>>,
}

impl Index {
    fn new(root: &Path) -> Index {
        Index { dirs: vec![Arc::from(root)], recs: Vec::new(), by_name: HashMap::new() }
    }

    fn add(&mut self, dir: u32, name: &str) {
        let idx = self.recs.len() as u32;
        self.recs.push(Rec { dir, name: name.into() });
        self.by_name.entry(name.to_lowercase()).or_default().push(idx);
    }

    fn remove(&mut self, dir: u32, name: &str) {
        let d = dir;
        let key = name.to_lowercase();
        let Some(list) = self.by_name.get_mut(&key) else { return };
        if let Some(pos) = list.iter().position(|&i| self.recs[i as usize].dir == d && self.recs[i as usize].name.as_ref() == name) {
            let idx = list.remove(pos);
            self.recs[idx as usize].name = Box::from("");
        }
        if list.is_empty() {
            self.by_name.remove(&key);
        }
    }

    fn path(&self, idx: u32) -> PathBuf {
        let r = &self.recs[idx as usize];
        self.dirs[r.dir as usize].join(&*r.name)
    }

    fn search(&self, pattern: &str, under: &Path, include_hidden: bool, max: usize) -> Vec<PathBuf> {
        let pat = pattern.to_lowercase();
        let glob = pattern.contains('*') || pattern.contains('?');
        let mut out = Vec::new();
        if glob {
            let spec: Vec<char> = pat.chars().collect();
            for (name, list) in &self.by_name {
                if !include_hidden && name.starts_with('.') {
                    continue;
                }
                if glob_match(&spec, &name.chars().collect::<Vec<_>>()) {
                    for &i in list {
                        let p = self.path(i);
                        if p.starts_with(under) {
                            out.push(p);
                            if out.len() >= max {
                                return out;
                            }
                        }
                    }
                }
            }
        } else {
            // exact/substring: every indexed name that contains the query
            for (name, list) in &self.by_name {
                if !include_hidden && name.starts_with('.') {
                    continue;
                }
                if name.contains(&pat) {
                    for &i in list {
                        let p = self.path(i);
                        if p.starts_with(under) {
                            out.push(p);
                            if out.len() >= max {
                                return out;
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

fn glob_match(spec: &[char], name: &[char]) -> bool {
    if spec.is_empty() {
        return name.is_empty();
    }
    match spec[0] {
        '*' => (0..=name.len()).any(|i| glob_match(&spec[1..], &name[i..])),
        '?' => !name.is_empty() && glob_match(&spec[1..], &name[1..]),
        c => name.first() == Some(&c) && glob_match(&spec[1..], &name[1..]),
    }
}

struct Slot {
    root: PathBuf,
    state: AtomicU8,
    count: AtomicU64,
    mem: AtomicU64,
    cancel: Arc<AtomicBool>,
    index: RwLock<Index>,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

struct Service {
    slots: Vec<Arc<Slot>>,
}

fn service() -> &'static Mutex<Option<Service>> {
    static S: OnceLock<Mutex<Option<Service>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn excluded() -> &'static RwLock<Vec<String>> {
    static E: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
    E.get_or_init(|| RwLock::new(default_excluded()))
}

const DEFAULT_EXCLUDED: [&str; 8] = [".git", "node_modules", ".Trash", ".cache", ".Spotlight-V100", ".fseventsd", "target", "Library"];

pub fn default_excluded_names() -> &'static [&'static str] {
    &DEFAULT_EXCLUDED
}

fn default_excluded() -> Vec<String> {
    DEFAULT_EXCLUDED.iter().map(|s| s.to_string()).collect()
}

pub fn set_excluded(dirs: &[PathBuf]) {
    let mut e = excluded().write().unwrap();
    *e = default_excluded();
    for d in dirs {
        if let Some(name) = d.file_name() {
            e.push(name.to_string_lossy().into_owned());
        }
    }
}

fn is_excluded(name: &str) -> bool {
    let e = excluded().read().unwrap();
    e.iter().any(|x| x.eq_ignore_ascii_case(name))
}

fn walk(slot: &Slot) {
    let start = Instant::now();
    let Ok(rd) = std::fs::read_dir(&slot.root) else {
        slot.state.store(State::Failed as u8, Ordering::Relaxed);
        return;
    };
    let mut index = slot.index.write().unwrap();
    *index = Index::new(&slot.root);
    let mut stack = vec![(rd, 0u32)];
    while let Some((rd, dir_id)) = stack.pop() {
        if slot.cancel.load(Ordering::Relaxed) {
            return;
        }
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_excluded(&name) {
                continue;
            }
            index.add(dir_id, &name);
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let is_link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            if is_dir && !is_link {
                if let Ok(sub) = std::fs::read_dir(e.path()) {
                    let path: Arc<Path> = Arc::from(e.path());
                    index.dirs.push(path);
                    let id = (index.dirs.len() - 1) as u32;
                    stack.push((sub, id));
                }
            }
        }
        if start.elapsed() > Duration::from_millis(500) {
            let n = index.recs.len() as u64;
            slot.count.store(n, Ordering::Relaxed);
            slot.mem.store(n * 64, Ordering::Relaxed);
        }
    }
    let n = index.recs.len() as u64;
    slot.count.store(n, Ordering::Relaxed);
    slot.mem.store(n * 64, Ordering::Relaxed);
    slot.state.store(State::Ready as u8, Ordering::Release);
    log::info!("index: {} ready, {n} names in {:?}", slot.root.display(), start.elapsed());
}

/// Re-reads one directory of a slot (a change arrived for it).
fn refresh_dir(slot: &Slot, dir: &Path) {
    if slot.cancel.load(Ordering::Relaxed) {
        return;
    }
    let mut index = slot.index.write().unwrap();
    let Some(dir_id) = index.dirs.iter().position(|d| d.as_ref() == dir).map(|i| i as u32) else { return };
    let old: Vec<String> = index.recs.iter().filter(|r| r.dir == dir_id).map(|r| r.name.to_string()).collect();
    for n in old {
        index.remove(dir_id, &n);
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !is_excluded(&name) {
                index.add(dir_id, &name);
            }
        }
    }
    let n = index.recs.len() as u64;
    slot.count.store(n, Ordering::Relaxed);
}

/// Starts (or restarts) the index for `roots`. Returns at once; the walk is on its own threads.
pub fn start(roots: Vec<PathBuf>, _dir: PathBuf) {
    stop();
    let mut slots = Vec::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        // FSEvents reports canonical paths (`/private/var` for `/var`); the index keeps the
        // same spelling so a changed parent matches the walk's folders.
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let slot = Arc::new(Slot {
            root: root.clone(),
            state: AtomicU8::new(State::Building as u8),
            count: AtomicU64::new(0),
            mem: AtomicU64::new(0),
            cancel: Arc::new(AtomicBool::new(false)),
            index: RwLock::new(Index::new(&root)),
            watcher: Mutex::new(None),
        });
        slots.push(slot.clone());

        // The walk.
        {
            let slot = slot.clone();
            let _ = std::thread::Builder::new().name("rf-index".into()).spawn(move || walk(&slot));
        }

        // FSEvents: changed names are updated one at a time; a rescan rebuilds the root.
        let (tx, rx) = std::sync::mpsc::channel::<notify::Event>();
        let mut watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            match res {
                Ok(ev) => {
                    if !matches!(ev.kind, notify::EventKind::Access(_)) {
                        let _ = tx.send(ev);
                    }
                }
                Err(e) => log::warn!("index: watcher error: {e}"),
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                log::warn!("index: no watcher for {}: {e}", root.display());
                continue;
            }
        };
        if let Err(e) = watcher.watch(&root, notify::RecursiveMode::Recursive) {
            log::warn!("index: cannot watch {}: {e}", root.display());
            continue;
        }
        *slot.watcher.lock().unwrap() = Some(watcher);
        let slot = slot.clone();
        let _ = std::thread::Builder::new().name("rf-index-watch".into()).spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut rescan = first.need_rescan();
                let mut paths = first.paths;
                let began = Instant::now();
                // swallow the burst
                while began.elapsed() < Duration::from_millis(500) {
                    match rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(ev) => {
                            rescan |= ev.need_rescan();
                            paths.extend(ev.paths);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        Err(_) => return,
                    }
                }
                if slot.cancel.load(Ordering::Relaxed) {
                    return;
                }
                if rescan || paths.is_empty() {
                    walk(&slot);
                } else {
                    let mut dirs: Vec<PathBuf> = Vec::new();
                    for p in paths {
                        if let Some(d) = p.parent() {
                            if d.starts_with(&slot.root) && !dirs.contains(&d.to_path_buf()) {
                                dirs.push(d.to_path_buf());
                            }
                        }
                    }
                    for d in dirs {
                        refresh_dir(&slot, &d);
                    }
                }
            }
        });
    }
    *service().lock().unwrap() = Some(Service { slots });
}

pub fn stop() {
    if let Some(s) = service().lock().unwrap().take() {
        for slot in &s.slots {
            slot.cancel.store(true, Ordering::Relaxed);
            *slot.watcher.lock().unwrap() = None;
        }
    }
}

pub fn status() -> Vec<(PathBuf, State, usize, usize)> {
    let g = service().lock().unwrap();
    let Some(s) = g.as_ref() else { return Vec::new() };
    s.slots
        .iter()
        .map(|slot| {
            let state = match slot.state.load(Ordering::Relaxed) {
                x if x == State::Ready as u8 => State::Ready,
                x if x == State::Failed as u8 => State::Failed,
                _ => State::Building,
            };
            (slot.root.clone(), state, slot.count.load(Ordering::Relaxed) as usize, slot.mem.load(Ordering::Relaxed) as usize)
        })
        .collect()
}

fn slot_for(under: &Path) -> Option<Arc<Slot>> {
    let g = service().lock().unwrap();
    let s = g.as_ref()?;
    s.slots.iter().find(|s| under.starts_with(&s.root)).cloned()
}

/// The index's answer, or `None` when the root is not ready (the caller walks the disk).
pub fn search(pattern: &str, under: &Path, include_hidden: bool, max: usize) -> Option<Vec<PathBuf>> {
    let slot = slot_for(under)?;
    if slot.state.load(Ordering::Acquire) != State::Ready as u8 {
        return None;
    }
    let index = slot.index.read().unwrap();
    Some(index.search(pattern, under, include_hidden, max))
}

/// [`search`] with a rank for each name (the caller's "what you open often comes first").
pub fn search_ranked(
    pattern: &str,
    under: &Path,
    include_hidden: bool,
    max: usize,
    rank: &(dyn Fn(&str) -> Option<i32> + Sync),
) -> Option<Vec<(i32, PathBuf)>> {
    let slot = slot_for(under)?;
    if slot.state.load(Ordering::Acquire) != State::Ready as u8 {
        return None;
    }
    let index = slot.index.read().unwrap();
    let paths = index.search(pattern, under, include_hidden, max.saturating_mul(4));
    let mut out: Vec<(i32, PathBuf)> = paths
        .into_iter()
        .map(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            (rank(&name).unwrap_or(0), p)
        })
        .collect();
    out.sort_by_key(|(r, _)| std::cmp::Reverse(*r));
    out.truncate(max);
    Some(out)
}

/// The folders indexed by default: the home folder, Applications, and every local volume but
/// the read-only system one.
pub fn local_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir() {
        out.push(home);
    }
    out.push(PathBuf::from("/Applications"));
    for d in crate::drives::list_drives() {
        if d.root == Path::new("/") || d.kind == crate::drives::DriveKind::Network {
            continue;
        }
        out.push(d.root);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_ready(root: &Path) -> bool {
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(10) {
            if status().iter().any(|(r, s, _, _)| r == root && *s == State::Ready) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn indexes_searches_and_follows_changes() {
        let d = std::env::temp_dir().join(format!("rf-mac-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::create_dir_all(d.join("node_modules")).unwrap();
        std::fs::write(d.join("alpha.txt"), b"a").unwrap();
        std::fs::write(d.join("sub/beta.txt"), b"b").unwrap();
        std::fs::write(d.join("node_modules/gamma.txt"), b"c").unwrap();
        let canon = std::fs::canonicalize(&d).unwrap();

        start(vec![d.clone()], d.join("index"));
        assert!(wait_ready(&canon), "the index never became ready");

        let names = |q: &str| -> Vec<String> {
            search(q, &canon, true, 100)
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.strip_prefix(&canon).unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert!(names("alpha").contains(&"alpha.txt".to_string()));
        assert!(names("beta").contains(&"sub/beta.txt".to_string()));
        // excluded folders are not indexed
        assert!(names("gamma").is_empty(), "{:?}", names("gamma"));
        // a glob
        assert!(names("*.txt").iter().any(|n| n == "alpha.txt"));

        // FSEvents: a new file appears, a removed one goes.
        std::fs::write(d.join("later.txt"), b"x").unwrap();
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(5) && !names("later").contains(&"later.txt".to_string()) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(names("later").contains(&"later.txt".to_string()), "the watcher did not add the new file");
        std::fs::remove_file(d.join("later.txt")).unwrap();
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(5) && names("later").contains(&"later.txt".to_string()) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(names("later").is_empty(), "the watcher did not remove the deleted file");

        stop();
        let _ = std::fs::remove_dir_all(&d);
    }
}
