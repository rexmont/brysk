//! The name index behind quick search, the macOS half of `rf-win::indexer`.
//!
//! A background walk of the user's folders builds an in-memory map from a lower-case name to
//! the places it exists (the Windows version also persists it and replays the NTFS change
//! journal; here the walk is redone at start, a few seconds after the window is up). FSEvents
//! (`notify`, recursive) keeps it current while the app runs: a changed name is replaced or
//! removed one at a time, and a dropped-event burst rebuilds the affected root.
//!
//! Memory is dominated by the number of names, so the index keeps one copy of each distinct
//! name (as folded text and as first-seen text) in two byte arenas, and one 12-byte record per
//! place that name exists. There is no `HashMap<String, Vec<u32>>` and no per-record `String`:
//! a few hundred thousand entries cost a few dozen megabytes, not hundreds.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
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

/// A slot in [`Index::recs`] that no longer names anything (a removed entry). Records are not
/// moved when one goes: the name's linked list just skips it.
const NO_REC: u32 = u32::MAX;

/// One place a name exists: its folder and the next record with the same name.
#[derive(Clone, Copy)]
struct Rec {
    dir: u32,
    name: u32,
    next: u32,
}

/// A distinct name, stored once in the two arenas of [`Index`].
struct Name {
    orig_off: u32,
    orig_len: u32,
    fold_off: u32,
    fold_len: u32,
    /// First record that carries this name (`NO_REC`: none right now).
    head: u32,
}

struct Index {
    dirs: Vec<Box<Path>>,
    /// Path hash -> a folder id (collisions are resolved by comparing the paths).
    dir_hash: HashMap<u64, u32>,
    recs: Vec<Rec>,
    /// Slots of removed records, reused by the next [`Index::add`] (a busy folder would
    /// otherwise grow the record vector for the whole session).
    free_recs: Vec<u32>,
    /// Records that still name something.
    live: u32,
    names: Vec<Name>,
    orig: Vec<u8>,
    folded: Vec<u8>,
    /// Folded-name hash -> first name id with that hash.
    by_hash: HashMap<u64, u32>,
    /// Name id -> next name id with the same hash.
    hash_next: Vec<u32>,
}

const CACHE_MAGIC: &[u8; 5] = b"RFIX2";
const CACHE_MAGIC_V1: &[u8; 5] = b"RFIX1";
/// A string in a cache longer than this is corrupt, not a path.
const MAX_STRING: usize = 1 << 20;
/// More changed folders than this in one burst: walking the root again is cheaper than
/// refreshing each of them (and the set cannot grow without bound).
const MAX_CHANGE_DIRS: usize = 4096;

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Hands freed pages back to the system. The index is replaced by a fresh walk while the old
/// one (tens of megabytes) is still mapped; without this the allocator keeps that memory
/// resident for the life of the process.
fn release_free_memory() {
    #[cfg(target_os = "macos")]
    unsafe {
        unsafe extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        let _ = malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

impl Index {
    fn new(root: &Path) -> Index {
        let mut index = Index {
            dirs: Vec::new(),
            dir_hash: HashMap::new(),
            recs: Vec::new(),
            free_recs: Vec::new(),
            live: 0,
            names: Vec::new(),
            orig: Vec::new(),
            folded: Vec::new(),
            by_hash: HashMap::new(),
            hash_next: Vec::new(),
        };
        index.push_dir(root);
        index
    }

    fn push_dir(&mut self, path: &Path) -> u32 {
        let id = self.dirs.len() as u32;
        let h = fnv(path.as_os_str().as_encoded_bytes());
        self.dirs.push(Box::from(path));
        // The first folder with a hash keeps the map entry; a (vanishingly rare) collision is
        // still found by the linear fallback in `dir_id`.
        self.dir_hash.entry(h).or_insert(id);
        id
    }

    /// The id of `path` among [`Self::dirs`], adding it when it is new.
    fn dir_id(&mut self, path: &Path) -> u32 {
        let h = fnv(path.as_os_str().as_encoded_bytes());
        if let Some(&id) = self.dir_hash.get(&h) {
            if self.dirs[id as usize].as_ref() == path {
                return id;
            }
            // a 64-bit hash collision: find the right one, then add the new path
            if let Some(i) = self.dirs.iter().position(|d| d.as_ref() == path) {
                return i as u32;
            }
        }
        self.push_dir(path)
    }

    fn dir_lookup(&self, path: &Path) -> Option<u32> {
        let h = fnv(path.as_os_str().as_encoded_bytes());
        let id = *self.dir_hash.get(&h)?;
        if self.dirs[id as usize].as_ref() == path {
            return Some(id);
        }
        self.dirs.iter().position(|d| d.as_ref() == path).map(|i| i as u32)
    }

    fn name_of(&self, id: u32) -> &str {
        let n = &self.names[id as usize];
        std::str::from_utf8(&self.orig[n.orig_off as usize..(n.orig_off + n.orig_len) as usize]).unwrap_or("")
    }

    fn folded_of(&self, id: u32) -> &str {
        let n = &self.names[id as usize];
        std::str::from_utf8(&self.folded[n.fold_off as usize..(n.fold_off + n.fold_len) as usize]).unwrap_or("")
    }

    /// The name's id, adding it when it is new. `folded` is `fold_into(name, …)`.
    fn intern(&mut self, name: &str, folded: &str) -> u32 {
        let h = fnv(folded.as_bytes());
        let mut at = self.by_hash.get(&h).copied().unwrap_or(NO_REC);
        while at != NO_REC {
            if self.name_of(at) == name {
                return at;
            }
            at = self.hash_next[at as usize];
        }
        let id = self.names.len() as u32;
        let orig_off = self.orig.len() as u32;
        self.orig.extend_from_slice(name.as_bytes());
        let fold_off = self.folded.len() as u32;
        self.folded.extend_from_slice(folded.as_bytes());
        self.names.push(Name {
            orig_off,
            orig_len: name.len() as u32,
            fold_off,
            fold_len: folded.len() as u32,
            head: NO_REC,
        });
        self.hash_next.push(self.by_hash.get(&h).copied().unwrap_or(NO_REC));
        self.by_hash.insert(h, id);
        id
    }

    fn add(&mut self, dir: u32, name: &str, scratch: &mut String) {
        fold_into(name, scratch);
        let id = self.intern(name, scratch);
        let head = self.names[id as usize].head;
        let rec = match self.free_recs.pop() {
            Some(rec) => {
                self.recs[rec as usize] = Rec { dir, name: id, next: head };
                rec
            }
            None => {
                self.recs.push(Rec { dir, name: id, next: head });
                (self.recs.len() - 1) as u32
            }
        };
        self.names[id as usize].head = rec;
        self.live += 1;
    }

    fn unlink(&mut self, id: u32, rec: u32) {
        let mut prev = NO_REC;
        let mut cur = self.names[id as usize].head;
        while cur != NO_REC {
            let next = self.recs[cur as usize].next;
            if cur == rec {
                if prev == NO_REC {
                    self.names[id as usize].head = next;
                } else {
                    self.recs[prev as usize].next = next;
                }
                self.recs[cur as usize].dir = NO_REC;
                self.free_recs.push(cur);
                self.live -= 1;
                return;
            }
            prev = cur;
            cur = next;
        }
    }

    fn remove(&mut self, dir: u32, name: &str) {
        let mut folded = String::new();
        fold_into(name, &mut folded);
        let h = fnv(folded.as_bytes());
        let mut at = self.by_hash.get(&h).copied().unwrap_or(NO_REC);
        while at != NO_REC {
            if self.name_of(at) == name {
                let mut cur = self.names[at as usize].head;
                while cur != NO_REC {
                    if self.recs[cur as usize].dir == dir {
                        self.unlink(at, cur);
                        return;
                    }
                    cur = self.recs[cur as usize].next;
                }
                return;
            }
            at = self.hash_next[at as usize];
        }
    }

    /// Forgets everything below `path` (a folder that is gone). The folder ids stay: other
    /// records may still name them, and an id is never reused.
    fn remove_tree(&mut self, path: &Path) {
        let mut gone: Vec<u32> = Vec::new();
        for (i, r) in self.recs.iter().enumerate() {
            if r.dir != NO_REC && self.dirs[r.dir as usize].starts_with(path) {
                gone.push(i as u32);
            }
        }
        for rec in gone {
            self.unlink(self.recs[rec as usize].name, rec);
        }
    }

    fn search(&self, pattern: &str, under: &Path, include_hidden: bool, max: usize) -> Vec<PathBuf> {
        let pat = fold(pattern);
        let glob = pattern.contains('*') || pattern.contains('?');
        let spec: Vec<char> = if glob { pat.chars().collect() } else { Vec::new() };
        let ascii: Option<Vec<u8>> = glob.then(|| spec.iter().all(|c| c.is_ascii()).then(|| spec.iter().map(|&c| c as u8).collect())).flatten();
        let mut out = Vec::new();
        for id in 0..self.names.len() as u32 {
            let name = self.folded_of(id);
            if !include_hidden && name.starts_with('.') {
                continue;
            }
            let matched = if glob { glob_match_str(&spec, ascii.as_deref(), name) } else { name.contains(&pat) };
            if !matched {
                continue;
            }
            let orig = self.name_of(id);
            let mut rec = self.names[id as usize].head;
            while rec != NO_REC {
                let r = &self.recs[rec as usize];
                // A path is below `under` exactly when its folder is: the name is one component.
                if r.dir != NO_REC && self.dirs[r.dir as usize].starts_with(under) {
                    out.push(self.dirs[r.dir as usize].join(orig));
                    if out.len() >= max {
                        return out;
                    }
                }
                rec = r.next;
            }
        }
        out
    }

    /// What the index costs, near enough for the Settings page and the logs.
    fn heap_bytes(&self) -> usize {
        let dirs: usize = self.dirs.capacity() * std::mem::size_of::<Box<Path>>()
            + self.dirs.iter().map(|d| d.as_os_str().len() + 16).sum::<usize>();
        dirs
            + self.dir_hash.capacity() * 13
            + self.recs.capacity() * std::mem::size_of::<Rec>()
            + self.free_recs.capacity() * 4
            + self.names.capacity() * std::mem::size_of::<Name>()
            + self.orig.capacity()
            + self.folded.capacity()
            + self.by_hash.capacity() * 13
            + self.hash_next.capacity() * 4
    }

    /// Writes the index (folders once, names once) as `RFIX2`, streamed: no whole-file buffer.
    fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        {
            let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
            w.write_all(CACHE_MAGIC)?;
            write_u32(&mut w, self.dirs.len() as u32)?;
            for d in &self.dirs {
                let b = d.as_os_str().as_encoded_bytes();
                write_u32(&mut w, b.len() as u32)?;
                w.write_all(b)?;
            }
            write_u32(&mut w, self.names.len() as u32)?;
            for n in &self.names {
                let orig = &self.orig[n.orig_off as usize..(n.orig_off + n.orig_len) as usize];
                let folded = &self.folded[n.fold_off as usize..(n.fold_off + n.fold_len) as usize];
                write_u32(&mut w, orig.len() as u32)?;
                w.write_all(orig)?;
                write_u32(&mut w, folded.len() as u32)?;
                w.write_all(folded)?;
            }
            write_u32(&mut w, self.live)?;
            for r in &self.recs {
                if r.dir != NO_REC {
                    write_u32(&mut w, r.dir)?;
                    write_u32(&mut w, r.name)?;
                }
            }
            w.flush()?;
        }
        std::fs::rename(&tmp, path)
    }

    /// The index written by this version, else the older `RFIX1` one (`None`: no cache).
    fn load(path: &Path) -> Option<Index> {
        let f = File::open(path).ok()?;
        let len = f.metadata().ok()?.len() as usize;
        let mut r = BufReader::with_capacity(1 << 20, f);
        let mut magic = [0u8; 5];
        r.read_exact(&mut magic).ok()?;
        match &magic {
            m if m == CACHE_MAGIC => load_v2(&mut r, len),
            m if m == CACHE_MAGIC_V1 => load_v1(&mut r, len),
            _ => None,
        }
    }
}

fn load_v2(r: &mut impl Read, len: usize) -> Option<Index> {
    let mut index = Index {
        dirs: Vec::new(),
        dir_hash: HashMap::new(),
        recs: Vec::new(),
        free_recs: Vec::new(),
        live: 0,
        names: Vec::new(),
        orig: Vec::new(),
        folded: Vec::new(),
        by_hash: HashMap::new(),
        hash_next: Vec::new(),
    };
    let n_dirs = read_u32(r)? as usize;
    if n_dirs == 0 || n_dirs > len / 4 {
        return None;
    }
    index.dirs.reserve_exact(n_dirs);
    let mut buf = Vec::new();
    for _ in 0..n_dirs {
        read_bytes(r, &mut buf, len)?;
        let text = String::from_utf8_lossy(&buf);
        index.push_dir(Path::new(text.as_ref()));
    }
    let n_names = read_u32(r)? as usize;
    if n_names > len / 8 {
        return None;
    }
    index.names.reserve_exact(n_names);
    index.hash_next.reserve_exact(n_names);
    for _ in 0..n_names {
        read_bytes(r, &mut buf, len)?;
        let orig_off = index.orig.len() as u32;
        index.orig.extend_from_slice(&buf);
        read_bytes(r, &mut buf, len)?;
        let fold_off = index.folded.len() as u32;
        index.folded.extend_from_slice(&buf);
        let n = &index.orig[orig_off as usize..];
        let f = &index.folded[fold_off as usize..];
        let h = fnv(f);
        index.names.push(Name {
            orig_off,
            orig_len: n.len() as u32,
            fold_off,
            fold_len: f.len() as u32,
            head: NO_REC,
        });
        let id = (index.names.len() - 1) as u32;
        index.hash_next.push(index.by_hash.get(&h).copied().unwrap_or(NO_REC));
        index.by_hash.insert(h, id);
    }
    let n_recs = read_u32(r)? as usize;
    if n_recs > len / 8 {
        return None;
    }
    index.recs.reserve_exact(n_recs);
    for _ in 0..n_recs {
        let dir = read_u32(r)?;
        let name = read_u32(r)?;
        if dir as usize >= index.dirs.len() || name as usize >= index.names.len() {
            return None;
        }
        let head = index.names[name as usize].head;
        index.recs.push(Rec { dir, name, next: head });
        index.names[name as usize].head = (index.recs.len() - 1) as u32;
        index.live += 1;
    }
    Some(index)
}

/// The older cache lists full paths only, one record each. Read it once; the walk that follows
/// writes the compact form. (A cache from before this version still gives instant results.)
fn load_v1(r: &mut impl Read, len: usize) -> Option<Index> {
    let n = read_u32(r)? as usize;
    if n > len / 2 {
        return None;
    }
    let mut index = Index {
        dirs: Vec::new(),
        dir_hash: HashMap::new(),
        recs: Vec::with_capacity(n),
        free_recs: Vec::new(),
        live: 0,
        names: Vec::new(),
        orig: Vec::new(),
        folded: Vec::new(),
        by_hash: HashMap::new(),
        hash_next: Vec::new(),
    };
    let mut ids: HashMap<Box<[u8]>, u32> = HashMap::new();
    let mut buf = Vec::new();
    let mut scratch = String::new();
    for _ in 0..n {
        read_bytes_u16(r, &mut buf, len)?;
        let Some(slash) = buf.iter().rposition(|&b| b == b'/') else { continue };
        let Some(name) = buf.get(slash + 1..).filter(|n| !n.is_empty()) else { continue };
        let dir = &buf[..slash];
        let id = match ids.get(dir) {
            Some(&id) => id,
            None => {
                let text = String::from_utf8_lossy(dir);
                let id = index.push_dir(Path::new(text.as_ref()));
                ids.insert(dir.into(), id);
                id
            }
        };
        index.add(id, &String::from_utf8_lossy(name), &mut scratch);
    }
    Some(index)
}

fn write_u32(w: &mut impl Write, v: u32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn read_u32(r: &mut impl Read) -> Option<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

fn read_bytes(r: &mut impl Read, out: &mut Vec<u8>, len: usize) -> Option<()> {
    let n = read_u32(r)? as usize;
    if n > MAX_STRING || n > len {
        return None;
    }
    out.clear();
    out.resize(n, 0);
    r.read_exact(out).ok()?;
    Some(())
}

fn read_bytes_u16(r: &mut impl Read, out: &mut Vec<u8>, len: usize) -> Option<()> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b).ok()?;
    let n = u16::from_le_bytes(b) as usize;
    if n > len {
        return None;
    }
    out.clear();
    out.resize(n, 0);
    r.read_exact(out).ok()?;
    Some(())
}

/// Case folding for matching, with the Turkish letters as the plain ones a keyboard without
/// them types: "sarki" finds "Şarkı listesi.mp3", as the quick search's own matching does.
/// Names are kept as they are; only the index's key is folded.
fn fold_into(name: &str, out: &mut String) {
    out.clear();
    out.reserve(name.len());
    for c in name.chars().flat_map(char::to_lowercase) {
        if c == '\u{307}' {
            continue;
        }
        out.push(match c {
            'ş' => 's',
            'ğ' => 'g',
            'ü' | 'û' => 'u',
            'ö' => 'o',
            'ç' => 'c',
            'ı' | 'î' => 'i',
            'â' | 'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            c => c,
        });
    }
}

fn fold(name: &str) -> String {
    let mut out = String::new();
    fold_into(name, &mut out);
    out
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

/// [`glob_match`] for a folded name, walking bytes when both sides are ASCII (the common case:
/// no `Vec<char>` per indexed name).
fn glob_match_str(spec: &[char], spec_ascii: Option<&[u8]>, name: &str) -> bool {
    if let Some(spec) = spec_ascii
        && name.is_ascii()
    {
        let name = name.as_bytes();
        let (mut pi, mut ni) = (0, 0);
        let (mut star, mut mark) = (usize::MAX, 0);
        while ni < name.len() {
            if pi < spec.len() && spec[pi] == b'*' {
                star = pi;
                mark = ni;
                pi += 1;
            } else if pi < spec.len() && (spec[pi] == b'?' || spec[pi] == name[ni]) {
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
        while pi < spec.len() && spec[pi] == b'*' {
            pi += 1;
        }
        return pi == spec.len();
    }
    glob_match(spec, &name.chars().collect::<Vec<_>>())
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

/// Whether `dir` (inside `root`) lies in a folder the index leaves out. Changes below such a
/// folder (a build's `target/`, a tool's dot-folder) cannot change the index, so the watcher
/// drops them instead of re-reading the folder for nothing.
fn under_excluded(list: &[String], root: &Path, dir: &Path) -> bool {
    let rel = dir.strip_prefix(root).unwrap_or(dir);
    rel.components().any(|c| {
        let n = c.as_os_str().to_string_lossy();
        skip_inside(&n) || list.iter().any(|x| x.eq_ignore_ascii_case(&n))
    })
}

fn build(slot: &Slot) -> Option<Index> {
    let start = Instant::now();
    let Ok(rd) = std::fs::read_dir(&slot.root) else {
        slot.state.store(State::Failed as u8, Ordering::Relaxed);
        return None;
    };
    let mut index = Index::new(&slot.root);
    let mut scratch = String::new();
    let mut stack = vec![(rd, 0u32, 0usize)];
    while let Some((rd, dir_id, depth)) = stack.pop() {
        if slot.cancel.load(Ordering::Relaxed) {
            return None;
        }
        for e in rd.flatten() {
            let raw = e.file_name();
            let name = raw.to_string_lossy();
            if is_excluded(&name) {
                continue;
            }
            index.add(dir_id, &name, &mut scratch);
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() && !ft.is_symlink() && !skip_inside(&name) && depth < slot.depth {
                if let Ok(sub) = std::fs::read_dir(e.path()) {
                    let id = index.dir_id(&e.path());
                    stack.push((sub, id, depth + 1));
                }
            }
        }
    }
    let n = u64::from(index.live);
    slot.count.store(n, Ordering::Relaxed);
    slot.mem.store(index.heap_bytes() as u64, Ordering::Relaxed);
    log::info!(
        "index: {} ready, {n} names, {} MB in {:?}",
        slot.root.display(),
        index.heap_bytes() >> 20,
        start.elapsed()
    );
    Some(index)
}

/// Puts a freshly built index in place and remembers it for the next start.
fn install(slot: &Slot, index: Index) {
    let n = u64::from(index.live);
    let mem = index.heap_bytes() as u64;
    if let Err(e) = index.save(&slot.cache) {
        log::warn!("index: cannot save {}: {e}", slot.cache.display());
    }
    {
        let mut g = slot.index.write().unwrap();
        *g = index;
    }
    // the replaced index is gone; give its pages back instead of keeping them empty
    release_free_memory();
    slot.count.store(n, Ordering::Relaxed);
    slot.mem.store(mem, Ordering::Relaxed);
    slot.state.store(State::Ready as u8, Ordering::Release);
}

/// Adds `dir`'s entries and the new folders below it to the index; `depth` is how many folder
/// levels below the slot's root they are (the root's own entries: 0).
fn index_dir(slot: &Slot, index: &mut Index, dir: &Path, depth: usize, scratch: &mut String) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let dir_id = index.dir_id(dir);
    for e in rd.flatten() {
        if slot.cancel.load(Ordering::Relaxed) {
            return;
        }
        let raw = e.file_name();
        let name = raw.to_string_lossy();
        if is_excluded(&name) {
            continue;
        }
        index.add(dir_id, &name, scratch);
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && !skip_inside(&name) && depth < slot.depth {
            index_dir(slot, index, &e.path(), depth + 1, scratch);
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
    // read the disk before taking the lock: a slow folder must not hold up searches
    let entries: Vec<(String, bool)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if is_excluded(&name) {
                        return None;
                    }
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    Some((name, is_dir))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut index = slot.index.write().unwrap();
    let Some(dir_id) = index.dir_lookup(dir).map(|i| i as u32) else { return };
    let old: HashSet<String> =
        index.recs.iter().filter(|r| r.dir == dir_id).map(|r| index.name_of(r.name).to_string()).collect();
    let now: HashSet<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    let mut scratch = String::new();
    let mut gone: Vec<(&str, PathBuf)> = Vec::new();
    for n in &old {
        if now.contains(n.as_str()) {
            continue;
        }
        let p = dir.join(n);
        // a folder that went takes its contents with it
        if index.dir_lookup(&p).is_some() {
            gone.push((n.as_str(), p));
        }
        index.remove(dir_id, n);
    }
    for (_, p) in &gone {
        index.remove_tree(p);
    }
    for (name, is_dir) in entries {
        // a name that is still there keeps its record: removing and re-adding every entry of a
        // busy folder (a download lands, a file is saved) was the cost of every watcher burst
        if old.contains(&name) {
            continue;
        }
        index.add(dir_id, &name, &mut scratch);
        if is_dir && !skip_inside(&name) && depth < slot.depth {
            index_dir(slot, &mut index, &dir.join(&name), depth + 1, &mut scratch);
        }
    }
    slot.count.store(u64::from(index.live), Ordering::Relaxed);
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
                    let n = u64::from(old.live);
                    let mem = old.heap_bytes() as u64;
                    *slot.index.write().unwrap() = old;
                    slot.count.store(n, Ordering::Relaxed);
                    slot.mem.store(mem, Ordering::Relaxed);
                    slot.state.store(State::Ready as u8, Ordering::Release);
                    // the old cache's parse buffers (and an older format's index) are dead now
                    release_free_memory();
                    log::info!("index: {} loaded from cache, {n} names, {} MB", slot.root.display(), mem >> 20);
                }
                if let Some(fresh) = build(&slot) {
                    install(&slot, fresh);
                }
            });
        }

        // FSEvents: changed names are updated one at a time; a rescan rebuilds the root.
        let (tx, rx) = std::sync::mpsc::sync_channel::<notify::Event>(1024);
        let overflow = Arc::new(AtomicBool::new(false));
        let over = overflow.clone();
        let mut watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            match res {
                Ok(ev) => {
                    if !matches!(ev.kind, notify::EventKind::Access(_))
                        && let Err(std::sync::mpsc::TrySendError::Full(_)) = tx.try_send(ev)
                    {
                        // the burst is beyond what this thread took: one walk beats a queue
                        over.store(true, Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    over.store(true, Ordering::Relaxed);
                    log::warn!("index: watcher error: {e}");
                }
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
            let mut last_save = Instant::now();
            while let Ok(first) = rx.recv() {
                let mut rescan = first.need_rescan() || overflow.swap(false, Ordering::Relaxed);
                // one snapshot of the exclusions for this burst, and one entry per changed folder
                let excluded_now: Vec<String> = excluded().read().unwrap().clone();
                let mut seen: HashSet<PathBuf> = HashSet::new();
                let mut dirs: Vec<PathBuf> = Vec::new();
                let take = |ev: notify::Event, rescan: &mut bool, seen: &mut HashSet<PathBuf>, dirs: &mut Vec<PathBuf>| {
                    if ev.need_rescan() || ev.paths.is_empty() {
                        *rescan = true;
                    }
                    for p in ev.paths {
                        if !p.starts_with(&slot.root) {
                            continue;
                        }
                        // A change below this folder does not need its parent re-read (the
                        // subfolder's own event does); only this folder's own listing changed.
                        let Some(d) = p.parent().filter(|d| d.starts_with(&slot.root)) else { continue };
                        if !seen.insert(d.to_path_buf()) {
                            continue;
                        }
                        // a build's target/ or a tool's dot-folder cannot change the index
                        if under_excluded(&excluded_now, &slot.root, d) {
                            continue;
                        }
                        dirs.push(d.to_path_buf());
                        if dirs.len() > MAX_CHANGE_DIRS || seen.len() > MAX_CHANGE_DIRS * 2 {
                            *rescan = true;
                        }
                    }
                };
                take(first, &mut rescan, &mut seen, &mut dirs);
                let began = Instant::now();
                // swallow the burst; a folder being written to must not start a walk per file
                while began.elapsed() < Duration::from_millis(500) {
                    match rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(ev) => {
                            take(ev, &mut rescan, &mut seen, &mut dirs);
                            if overflow.swap(false, Ordering::Relaxed) {
                                rescan = true;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        Err(_) => return,
                    }
                }
                if slot.cancel.load(Ordering::Relaxed) {
                    return;
                }
                if rescan {
                    if let Some(fresh) = build(&slot) {
                        install(&slot, fresh);
                    }
                } else if !dirs.is_empty() {
                    for d in dirs {
                        refresh_dir(&slot, &d);
                    }
                    // one measurement per burst, not one per changed folder (it walks the
                    // folders); the same look decides whether the cache is due a write
                    if let Ok(index) = slot.index.try_read() {
                        slot.mem.store(index.heap_bytes() as u64, Ordering::Relaxed);
                        // Keep the cache close to the live index (a start reads it while the walk
                        // runs): write at most every five minutes, and never over a search's back.
                        if last_save.elapsed() > Duration::from_secs(300) && index.save(&slot.cache).is_ok() {
                            last_save = Instant::now();
                        }
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

    /// The compact index: one record per place, one name per spelling, and a cache that reads
    /// back the same answers (using the real file format, not a mock)
    #[test]
    fn keeps_one_copy_of_each_name_and_round_trips_the_cache() {
        let d = std::env::temp_dir().join(format!("rf-mac-index-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::create_dir_all(d.join("b")).unwrap();
        let root = std::fs::canonicalize(&d).unwrap();

        let mut index = Index::new(&root);
        let mut scratch = String::new();
        let a = index.dir_id(&root.join("a"));
        let b = index.dir_id(&root.join("b"));
        for dir in [a, b, a] {
            index.add(dir, "aynı ad.txt", &mut scratch);
        }
        index.add(a, "başka.txt", &mut scratch);
        assert_eq!(index.live, 4);
        assert_eq!(index.names.len(), 2, "the same file name is kept once");
        assert_eq!(index.search("aynı", &root, true, 10).len(), 3);
        assert_eq!(index.search("başka", &root.join("a"), true, 10).len(), 1);
        assert!(index.search("başka", &root.join("b"), true, 10).is_empty());

        // remove and remove_tree take one place and a whole folder
        index.remove(a, "başka.txt");
        assert!(index.search("başka", &root, true, 10).is_empty());
        index.remove_tree(&root.join("b"));
        assert_eq!(index.search("aynı", &root, true, 10).len(), 2);

        let cache = d.join("cache.bin");
        index.save(&cache).unwrap();
        let loaded = Index::load(&cache).unwrap();
        assert_eq!(loaded.live, 2);
        assert_eq!(loaded.search("aynı", &root, true, 10).len(), 2);
        assert!(loaded.heap_bytes() < 1 << 20, "a handful of names must not take a megabyte");

        // the older cache (full paths only) still reads
        let mut old = Vec::new();
        old.extend_from_slice(CACHE_MAGIC_V1);
        let paths = [format!("{}/a/eski.txt", root.display()), format!("{}/b/eski.txt", root.display())];
        old.extend_from_slice(&(paths.len() as u32).to_le_bytes());
        for p in &paths {
            old.extend_from_slice(&(p.len() as u16).to_le_bytes());
            old.extend_from_slice(p.as_bytes());
        }
        let old_path = d.join("old.bin");
        std::fs::write(&old_path, &old).unwrap();
        let loaded = Index::load(&old_path).unwrap();
        assert_eq!(loaded.live, 2);
        assert_eq!(loaded.search("eski", &root, true, 10).len(), 2);

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_folded_key_finds_turkish_names() {
        assert_eq!(fold("Şarkı Listesi"), "sarki listesi");
        assert_eq!(fold("İŞLEM"), "islem");
        assert_eq!(fold("ılık"), "ilik");
    }
}
