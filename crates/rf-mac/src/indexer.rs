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
        self.by_name.entry(fold(name)).or_default().push(idx);
    }

    fn remove(&mut self, dir: u32, name: &str) {
        let d = dir;
        let key = fold(name);
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

    /// Forgets everything below `path` (a folder that is gone). The folder ids stay: other
    /// records may still name them, and an id is never reused.
    fn remove_tree(&mut self, path: &Path) {
        let gone: Vec<(u32, String)> = self
            .recs
            .iter()
            .filter(|r| !r.name.is_empty() && self.dirs[r.dir as usize].starts_with(path))
            .map(|r| (r.dir, r.name.to_string()))
            .collect();
        for (dir, name) in gone {
            self.remove(dir, &name);
        }
    }

    fn search(&self, pattern: &str, under: &Path, include_hidden: bool, max: usize) -> Vec<PathBuf> {
        let pat = fold(pattern);
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

impl Index {
    /// Writes every path to the cache (length-prefixed bytes: a file name may contain anything).
    fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut buf: Vec<u8> = Vec::with_capacity(self.recs.len() * 64);
        buf.extend_from_slice(b"RFIX1");
        buf.extend_from_slice(&(self.recs.len() as u32).to_le_bytes());
        for (i, r) in self.recs.iter().enumerate() {
            let _ = i;
            let p = self.path(i as u32);
            let bytes = p.to_string_lossy();
            let b = bytes.as_bytes();
            let n = b.len().min(u16::MAX as usize);
            buf.extend_from_slice(&(n as u16).to_le_bytes());
            buf.extend_from_slice(&b[..n]);
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, path)
    }

    fn load(path: &Path) -> Option<Index> {
        let buf = std::fs::read(path).ok()?;
        if buf.len() < 9 || &buf[..5] != b"RFIX1" {
            return None;
        }
        let n = u32::from_le_bytes(buf[5..9].try_into().ok()?) as usize;
        let mut index = Index { dirs: Vec::new(), recs: Vec::with_capacity(n), by_name: HashMap::new() };
        let mut dir_ids: HashMap<PathBuf, u32> = HashMap::new();
        let mut at = 9;
        for _ in 0..n {
            if at + 2 > buf.len() {
                return None;
            }
            let len = u16::from_le_bytes(buf[at..at + 2].try_into().ok()?) as usize;
            at += 2;
            if at + len > buf.len() {
                return None;
            }
            let text = String::from_utf8_lossy(&buf[at..at + len]).into_owned();
            at += len;
            let p = PathBuf::from(&text);
            let Some(parent) = p.parent() else { continue };
            let dir_id = *dir_ids.entry(parent.to_path_buf()).or_insert_with(|| {
                index.dirs.push(Arc::from(parent));
                (index.dirs.len() - 1) as u32
            });
            if let Some(name) = p.file_name() {
                index.add(dir_id, &name.to_string_lossy());
            }
        }
        Some(index)
    }
}

/// Case folding for matching, with the Turkish letters as the plain ones a keyboard without
/// them types: "sarki" finds "Şarkı listesi.mp3", as the quick search's own matching does.
/// Names are kept as they are; only the index's key is folded.
fn fold(name: &str) -> String {
    rf_core::entry::fold_case(name)
        .chars()
        .map(|c| match c {
            'ş' => 's',
            'ğ' => 'g',
            'ü' | 'û' => 'u',
            'ö' => 'o',
            'ç' => 'c',
            'î' => 'i',
            'â' | 'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            c => c,
        })
        .collect()
}

fn glob_match(spec: &[char], name: &[char]) -> bool {
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while ni < name.len() {
        if pi < spec.len() && spec[pi] == '*' {
            star = pi;
            mark = ni;
            pi += 1;
        } else if pi < spec.len() && (spec[pi] == '?' || spec[pi] == name[ni]) {
            pi += 1;
            ni += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < spec.len() && spec[pi] == '*' {
        pi += 1;
    }
    pi == spec.len()
}

struct Slot {
    root: PathBuf,
    /// How many folder levels below the root are indexed (Applications: one; the home folder:
    /// everything). A folder at the limit is listed, its inside is not.
    depth: usize,
    /// The file the index is remembered in (`<dir>/index-<root hash>.bin`).
    cache: PathBuf,
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

const DEFAULT_EXCLUDED: [&str; 12] = [
    ".git",
    "node_modules",
    ".Trash",
    ".cache",
    ".Spotlight-V100",
    ".fseventsd",
    "target",
    "Library",
    "DerivedData",
    "Pods",
    ".npm",
    ".cargo",
];

/// Folders whose inside is never indexed: app/plugin bundles are shown as one entry, and a
/// dot-folder (a tool's profile) is not something a name search should walk.
fn skip_inside(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with(".app")
        || name.ends_with(".framework")
        || name.ends_with(".bundle")
        || name.ends_with(".plugin")
        || name.ends_with(".kext")
        || name.ends_with(".appex")
        || name.ends_with(".xpc")
        || name.ends_with(".photoslibrary")
        || name.ends_with(".fcpbundle")
        || name.ends_with(".logicx")
}

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

fn build(slot: &Slot) -> Option<Index> {
    let start = Instant::now();
    let Ok(rd) = std::fs::read_dir(&slot.root) else {
        slot.state.store(State::Failed as u8, Ordering::Relaxed);
        return None;
    };
    let mut index = Index::new(&slot.root);
    let mut stack = vec![(rd, 0u32, 0usize)];
    while let Some((rd, dir_id, depth)) = stack.pop() {
        if slot.cancel.load(Ordering::Relaxed) {
            return None;
        }
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_excluded(&name) {
                continue;
            }
            index.add(dir_id, &name);
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let is_link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            if is_dir && !is_link && !skip_inside(&name) && depth < slot.depth {
                if let Ok(sub) = std::fs::read_dir(e.path()) {
                    let path: Arc<Path> = Arc::from(e.path());
                    index.dirs.push(path);
                    let id = (index.dirs.len() - 1) as u32;
                    stack.push((sub, id, depth + 1));
                }
            }
        }
    }
    let n = index.recs.len() as u64;
    slot.count.store(n, Ordering::Relaxed);
    slot.mem.store(n * 64, Ordering::Relaxed);
    log::info!("index: {} ready, {n} names in {:?}", slot.root.display(), start.elapsed());
    Some(index)
}

/// Puts a freshly built index in place and remembers it for the next start.
fn install(slot: &Slot, index: Index) {
    let n = index.recs.len() as u64;
    if let Err(e) = index.save(&slot.cache) {
        log::warn!("index: cannot save {}: {e}", slot.cache.display());
    }
    *slot.index.write().unwrap() = index;
    slot.count.store(n, Ordering::Relaxed);
    slot.mem.store(n * 64, Ordering::Relaxed);
    slot.state.store(State::Ready as u8, Ordering::Release);
}

/// Adds `dir`'s entries and the new folders below it to the index; `depth` is how many folder
/// levels below the slot's root they are (the root's own entries: 0).
fn index_dir(slot: &Slot, index: &mut Index, dir: &Path, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let dir_id = match index.dirs.iter().position(|d| d.as_ref() == dir) {
        Some(i) => i as u32,
        None => {
            index.dirs.push(Arc::from(dir));
            (index.dirs.len() - 1) as u32
        }
    };
    for e in rd.flatten() {
        if slot.cancel.load(Ordering::Relaxed) {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if is_excluded(&name) {
            continue;
        }
        index.add(dir_id, &name);
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && !skip_inside(&name) && depth < slot.depth {
            index_dir(slot, index, &e.path(), depth + 1);
        }
    }
}

/// Re-reads one directory of a slot (a change arrived for it). A folder that appeared with its
/// contents (a download, an app in a vendor's folder) is walked here; one that went takes its
/// contents with it, so no result names a path that is no longer there.
fn refresh_dir(slot: &Slot, dir: &Path) {
    if slot.cancel.load(Ordering::Relaxed) {
        return;
    }
    let depth = dir.strip_prefix(&slot.root).map(|p| p.components().count()).unwrap_or(0);
    let mut index = slot.index.write().unwrap();
    let Some(dir_id) = index.dirs.iter().position(|d| d.as_ref() == dir).map(|i| i as u32) else { return };
    let old: Vec<String> = index.recs.iter().filter(|r| r.dir == dir_id && !r.name.is_empty()).map(|r| r.name.to_string()).collect();
    let entries: Vec<(String, bool)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    (!is_excluded(&name)).then(|| (name, e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
                })
                .collect()
        })
        .unwrap_or_default();
    for n in &old {
        let p = dir.join(n);
        if !entries.iter().any(|(name, _)| name == n) && index.dirs.iter().any(|d| d.as_ref() == p) {
            index.remove_tree(&p);
        }
        index.remove(dir_id, n);
    }
    for (name, is_dir) in entries {
        let is_new = !old.contains(&name);
        index.add(dir_id, &name);
        if is_new && is_dir && !skip_inside(&name) && depth < slot.depth {
            index_dir(slot, &mut index, &dir.join(&name), depth + 1);
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
        let depth = if root == Path::new("/Applications") { 1 } else { usize::MAX };
        let mut hash: u32 = 0x811c_9dc5;
        for b in root.to_string_lossy().bytes() {
            hash = (hash ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
        let cache = _dir.join(format!("index-{hash:08x}.bin"));
        let slot = Arc::new(Slot {
            root: root.clone(),
            depth,
            cache,
            state: AtomicU8::new(State::Building as u8),
            count: AtomicU64::new(0),
            mem: AtomicU64::new(0),
            cancel: Arc::new(AtomicBool::new(false)),
            index: RwLock::new(Index::new(&root)),
            watcher: Mutex::new(None),
        });
        slots.push(slot.clone());

        // Last run's index is usable at once; the walk refreshes it behind the user's back.
        {
            let slot = slot.clone();
            let _ = std::thread::Builder::new().name("rf-index".into()).spawn(move || {
                if let Some(old) = Index::load(&slot.cache) {
                    let n = old.recs.len() as u64;
                    *slot.index.write().unwrap() = old;
                    slot.count.store(n, Ordering::Relaxed);
                    slot.mem.store(n * 64, Ordering::Relaxed);
                    slot.state.store(State::Ready as u8, Ordering::Release);
                    log::info!("index: {} loaded from cache, {n} names", slot.root.display());
                }
                if let Some(fresh) = build(&slot) {
                    install(&slot, fresh);
                }
            });
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
                    if let Some(fresh) = build(&slot) {
                        install(&slot, fresh);
                    }
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
        std::fs::write(d.join("Şarkı listesi.txt"), b"t").unwrap();
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
        // Turkish letters are found by the plain ones a keyboard without them types
        assert!(names("sarki").contains(&"Şarkı listesi.txt".to_string()), "{:?}", names("sarki"));
        assert!(names("ŞARKI").contains(&"Şarkı listesi.txt".to_string()));
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

        // a folder that arrives with its contents (a download) is searchable without a restart
        std::fs::create_dir_all(d.join("gelen/alt")).unwrap();
        std::fs::write(d.join("gelen/alt/icerde.txt"), b"i").unwrap();
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(5) && !names("icerde").contains(&"gelen/alt/icerde.txt".to_string()) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(names("icerde").contains(&"gelen/alt/icerde.txt".to_string()), "the watcher did not walk the new folder: {:?}", names("icerde"));

        // and a folder that goes takes its contents with it
        std::fs::remove_dir_all(d.join("gelen")).unwrap();
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(5) && !names("icerde").is_empty() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(names("icerde").is_empty(), "the watcher kept the deleted folder's file: {:?}", names("icerde"));

        stop();
        let _ = std::fs::remove_dir_all(&d);
    }
}
