//! Our own copy / move engine, the macOS half of `rf-win::fastcopy`.
//!
//! Copies keep every metadata bit: `copyfile(COPYFILE_ALL)` carries permissions, times, ACLs,
//! extended attributes and Finder tags, and an APFS clone is used when both ends are on the
//! same volume. Files are copied in chunks for byte-accurate progress, pause, cancellation and
//! a speed limit. A name that already exists at the destination is never overwritten: the copy
//! keeps both, using the Finder's names ("report copy.txt", "Photos copy 2"). Copying into the
//! folder an item already lives in is that same case, so paste-in-place completes at once
//! instead of asking a question. A move on one volume is a rename; across volumes it is a
//! verified copy after which the source is deleted. Moving an item into the folder it is
//! already in changes nothing, and a folder cannot be copied or moved into itself.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use anyhow::Context;

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

fn write_journal(kind: Transfer, sources: &[PathBuf], dest: &Path, files: usize) -> Option<PathBuf> {
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
    Some(path)
}

pub fn discard(journal: &Path) {
    let _ = std::fs::remove_file(journal);
}

/// Carries a journaled job on: what is already in place is copied again (correctly, never
/// overwritten), and the journal goes once the job ends.
pub fn resume(journal: &Path, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    let Some(job) = read_journal(journal) else {
        let _ = std::fs::remove_file(journal);
        return Ok(false);
    };
    let r = transfer(job.kind, &job.sources, &job.dest, tracker);
    let _ = std::fs::remove_file(journal);
    r
}

// ------------------------------------------------------------------ planning

struct PlanFile {
    src: PathBuf,
    dst: PathBuf,
    size: u64,
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// A directory on disk, not a symlink that points at one.
fn is_real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).map(|m| m.is_dir() && !m.file_type().is_symlink()).unwrap_or(false)
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The Finder's keep-both name next to `dst`: "report copy.txt", then "report copy 2.txt".
/// Names this operation already planned are taken too (`reserved`).
fn keep_both(dst: &Path, is_dir: bool, reserved: &HashSet<PathBuf>) -> PathBuf {
    let parent = dst.parent().unwrap_or_else(|| Path::new("/"));
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "item".into());
    crate::fileops::duplicate_name_avoiding(parent, &name, is_dir, reserved)
}

/// A macOS package (.app, .bundle, .framework…): one item, never merged with another.
fn package_at(p: &Path) -> bool {
    p.file_name().is_some_and(|n| rf_core::is_package_name(&n.to_string_lossy()))
}

/// Walks one source and gives every entry a destination that does not overwrite anything:
/// folders with the same name merge, everything else keeps both.
fn plan_walk(
    src: &Path,
    dst: &Path,
    dirs: &mut Vec<(PathBuf, PathBuf, bool)>,
    files: &mut Vec<PlanFile>,
    reserved: &mut HashSet<PathBuf>,
) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(src)?;
    if md.file_type().is_symlink() || md.is_file() {
        let dst = if exists(dst) || reserved.contains(dst) { keep_both(dst, false, reserved) } else { dst.to_path_buf() };
        reserved.insert(dst.clone());
        files.push(PlanFile { src: src.to_path_buf(), dst, size: md.len() });
        return Ok(());
    }
    if !md.is_dir() {
        return Err(std::io::Error::other(format!("{}: bu tür özel dosyalar kopyalanamaz", src.display())));
    }
    // A folder pair merges; a file (or a symlink) in the way makes the folder a "copy".
    // A package is an item, not a folder pair: two apps never merge.
    let package = package_at(src);
    let merge = is_real_dir(dst) && !reserved.contains(dst) && !package;
    let dst = if merge || !(exists(dst) || reserved.contains(dst)) { dst.to_path_buf() } else { keep_both(dst, !package, reserved) };
    reserved.insert(dst.clone());
    dirs.push((src.to_path_buf(), dst.clone(), !exists(&dst)));
    for e in std::fs::read_dir(src)? {
        let e = e?;
        plan_walk(&e.path(), &dst.join(e.file_name()), dirs, files, reserved)?;
    }
    Ok(())
}

/// Where a top-level source lands. `None` means a move is already there (nothing to do).
///
/// Copying an item into the folder it already lives in is the Finder's duplicate, never a
/// question: `report.txt` becomes `report copy.txt` and the original stays. A clash in another
/// folder keeps both too (a folder pair merges), so an operation can always finish.
fn top_target(src: &Path, dest_dir: &Path, kind: Transfer, reserved: &HashSet<PathBuf>) -> anyhow::Result<Option<PathBuf>> {
    let name = src.file_name().ok_or_else(|| anyhow::anyhow!("ad yok: {}", src.display()))?;
    let desired = dest_dir.join(name);
    let src_dir = is_real_dir(src);
    let src_package = package_at(src);
    // a package keeps its suffix when it is renamed: "Safari copy.app", not "Safari.app copy"
    let copy_is_dir = src_dir && !src_package;
    let dest_canon = dest_dir.canonicalize().with_context(|| format!("{} okunamadı", dest_dir.display()))?;

    // A folder inside itself would recurse forever (and for a move it would delete the copy).
    // The destination folder itself counts: pasting a folder into the folder is not a way to
    // make "name copy" — that is what pasting into its parent does.
    if src_dir {
        let src_canon = src.canonicalize().with_context(|| format!("{} okunamadı", src.display()))?;
        if dest_canon.starts_with(&src_canon) {
            let verb = if kind == Transfer::Move { "taşınamaz" } else { "kopyalanamaz" };
            anyhow::bail!("{}: bir klasör kendi içine {}", src.display(), verb);
        }
    }

    if same_path(src, &desired) {
        return Ok(match kind {
            // Cut and paste in the same folder changes nothing.
            Transfer::Move => None,
            // Paste a copy next to the original: "name copy".
            Transfer::Copy => Some(keep_both(&desired, copy_is_dir, reserved)),
        });
    }

    let taken = exists(&desired) || reserved.contains(&desired);
    if !taken || (src_dir && !src_package && is_real_dir(&desired)) {
        return Ok(Some(desired));
    }
    Ok(Some(keep_both(&desired, copy_is_dir, reserved)))
}

/// Removes the folders a move left behind, but only the empty ones: a file that appeared while
/// the move was running is never thrown away with them.
fn remove_empty_tree(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if is_real_dir(&e.path()) {
            remove_empty_tree(&e.path());
        }
    }
    let _ = std::fs::remove_dir(dir);
}

// ------------------------------------------------------------------ copying

#[repr(C)]
struct CopyfileState(#[allow(dead_code)] [u8; 0]);

unsafe extern "C" {
    fn copyfile(from: *const std::ffi::c_char, to: *const std::ffi::c_char, state: *mut CopyfileState, flags: u32) -> i32;
}

const COPYFILE_ACL: u32 = 1 << 0;
const COPYFILE_STAT: u32 = 1 << 1;
const COPYFILE_XATTR: u32 = 1 << 2;
const COPYFILE_DATA: u32 = 1 << 3;
const COPYFILE_SECURITY: u32 = 1 << 4;
const COPYFILE_METADATA: u32 = COPYFILE_SECURITY | COPYFILE_ACL | COPYFILE_STAT | COPYFILE_XATTR;
const COPYFILE_ALL: u32 = COPYFILE_METADATA | COPYFILE_DATA;
const COPYFILE_CLONE: u32 = 1 << 24;

fn cpath(p: &Path) -> anyhow::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    Ok(std::ffi::CString::new(p.as_os_str().as_bytes())?)
}

/// Copies one regular file (or a symlink) with all metadata; progress in chunks for anything
/// large enough to matter.
fn copy_one(src: &Path, dst: &Path, size: u64, tracker: &Tracker) -> anyhow::Result<()> {
    let md = std::fs::symlink_metadata(src)?;
    if md.file_type().is_symlink() {
        let target = std::fs::read_link(src)?;
        let _ = std::fs::remove_file(dst);
        std::os::unix::fs::symlink(target, dst)?;
        return Ok(());
    }

    // A same-volume copy that needs no progress/limit: clonefile through copyfile (instant, and
    // every attribute comes along).
    let same_volume = same_device(src, dst);
    if same_volume && tracker.limit.load(Ordering::Relaxed) == 0 {
        let (s, d) = (cpath(src)?, cpath(dst)?);
        if unsafe { copyfile(s.as_ptr(), d.as_ptr(), std::ptr::null_mut(), COPYFILE_ALL | COPYFILE_CLONE) } == 0 {
            tracker.bytes.add(size);
            verify_one(src, dst, tracker.verify_mode())?;
            return Ok(());
        }
    }

    // Chunked copy: honours pause, cancel and the speed limit, and counts bytes.
    let mut input = std::fs::File::open(src).with_context(|| format!("{} açılamadı", src.display()))?;
    let mut output = std::fs::File::create(dst).with_context(|| format!("{} yazılamadı", dst.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut copied = 0u64;
    let started = std::time::Instant::now();
    loop {
        if tracker.cancel.load(Ordering::Relaxed) {
            drop(output);
            let _ = std::fs::remove_file(dst);
            anyhow::bail!(crate::hash::Cancelled);
        }
        while tracker.paused.load(Ordering::Relaxed) && !tracker.cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(30));
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n])?;
        copied += n as u64;
        let done = tracker.bytes.get().0 + n as u64;
        tracker.bytes.set(done, tracker.bytes.get().1);
        let limit = tracker.limit.load(Ordering::Relaxed);
        if limit > 0 {
            // Keep the average rate at the limit.
            let expected = copied as f64 / limit as f64;
            let elapsed = started.elapsed().as_secs_f64();
            if expected > elapsed {
                std::thread::sleep(Duration::from_secs_f64((expected - elapsed).min(0.25)));
            }
        }
    }
    output.flush()?;
    drop(output);
    // Times, permissions, xattrs, ACLs, Finder tags: copied after the bytes.
    let (s, d) = (cpath(src)?, cpath(dst)?);
    unsafe {
        copyfile(s.as_ptr(), d.as_ptr(), std::ptr::null_mut(), COPYFILE_METADATA);
    }
    verify_one(src, dst, tracker.verify_mode())?;
    Ok(())
}

fn same_device(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let da = a.parent().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.dev());
    let db = b.parent().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.dev());
    matches!((da, db), (Some(x), Some(y)) if x == y)
}

fn verify_one(src: &Path, dst: &Path, verify: Verify) -> anyhow::Result<()> {
    if verify == Verify::Off {
        return Ok(());
    }
    let a = std::fs::metadata(src)?;
    let b = std::fs::metadata(dst)?;
    if a.len() != b.len() {
        anyhow::bail!("{}: boyut uyuşmuyor ({} / {})", src.display(), a.len(), b.len());
    }
    if verify == Verify::Quick {
        if let (Ok(ta), Ok(tb)) = (a.modified(), b.modified()) {
            let d = ta.duration_since(tb).or_else(|e| Ok::<_, std::time::SystemTimeError>(e.duration())).unwrap_or_default();
            anyhow::ensure!(d < Duration::from_secs(2), "{}: değişme zamanı uyuşmuyor", src.display());
        }
        return Ok(());
    }
    // Full: byte for byte, in chunks (the cache does not make this free, but it is the truth).
    let mut fa = std::fs::File::open(src)?;
    let mut fb = std::fs::File::open(dst)?;
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let na = fa.read(&mut ba)?;
        let nb = fb.read(&mut bb)?;
        if na != nb || ba[..na] != bb[..nb] {
            anyhow::bail!("{}: kopya kaynakla aynı değil", src.display());
        }
        if na == 0 {
            break;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ transfer

/// Copies or moves `sources` into `dest_dir`. Clashes keep both with Finder-style names; the
/// operation never waits for a question and never overwrites.
pub fn transfer(kind: Transfer, sources: &[PathBuf], dest_dir: &Path, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    if sources.is_empty() {
        return Ok(true);
    }
    std::fs::create_dir_all(dest_dir)?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut moved_dirs = Vec::new();
    let mut new_roots = Vec::new();
    let mut reserved: HashSet<PathBuf> = HashSet::new();
    for src in sources {
        match top_target(src, dest_dir, kind, &reserved)? {
            Some(dst) => {
                if is_real_dir(src) && !exists(&dst) {
                    new_roots.push((src.clone(), dst.clone()));
                }
                if kind == Transfer::Move && is_real_dir(src) {
                    moved_dirs.push(src.clone());
                }
                plan_walk(src, &dst, &mut dirs, &mut files, &mut reserved)?;
            }
            // A move into the folder the item is already in: already there, nothing to do.
            None => {}
        }
    }

    let total_bytes: u64 = files.iter().map(|f| f.size).sum();
    let total_files = files.len();
    let journal = write_journal(kind, sources, dest_dir, total_files);
    tracker.bytes.set(0, total_bytes);
    (tracker.on_progress)(0, total_files as u32, None);

    // Directories first (a merge is harmless), shallowest first.
    dirs.sort_by_key(|(_, d, _)| d.components().count());
    for (_, d, _) in &dirs {
        if tracker.cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        std::fs::create_dir_all(d)?;
    }

    let mut done = 0usize;
    for f in &files {
        if tracker.cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        while tracker.paused.load(Ordering::Relaxed) && !tracker.cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(30));
        }
        let name = f.src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if kind == Transfer::Move && same_device(&f.src, &f.dst) {
            // One volume: the rename is the move (and the destination is free by planning).
            std::fs::rename(&f.src, &f.dst)?;
            tracker.bytes.add(f.size);
        } else {
            copy_one(&f.src, &f.dst, f.size, &tracker)?;
            // Across volumes (or a copy): the source goes only after the copy checked out.
            if kind == Transfer::Move {
                crate::fileops::remove_recursive(&f.src)?;
            }
        }
        tracker.results.lock().unwrap().push((f.src.clone(), f.dst.clone()));
        done += 1;
        (tracker.on_progress)(done as u32, total_files as u32, Some(name));
    }

    // Apply directory metadata after writing children (which changes their timestamps).
    for (src, dst, created) in dirs.iter().rev() {
        if *created {
            let (s, d) = (cpath(src)?, cpath(dst)?);
            if unsafe { copyfile(s.as_ptr(), d.as_ptr(), std::ptr::null_mut(), COPYFILE_METADATA) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
    // A move emptied the source folders; they go too (deepest first). Empty only: never
    // remove something the operation did not copy.
    if kind == Transfer::Move {
        moved_dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for d in moved_dirs {
            if is_real_dir(&d) {
                remove_empty_tree(&d);
            }
        }
    }

    // New folders/packages are one undo item, including empty directories. A merged folder
    // keeps its per-file record so undo never deletes items that were already there.
    let mut results = tracker.results.lock().unwrap();
    for (src, dst) in new_roots {
        results.retain(|(s, d)| !(s.starts_with(&src) && d.starts_with(&dst)));
        results.push((src, dst));
    }
    drop(results);

    if let Some(j) = &journal {
        let _ = std::fs::remove_file(j);
    }
    (tracker.on_progress)(total_files as u32, total_files as u32, None);
    Ok(true)
}

/// Undo a move to its exact original name, without merging or replacing another item.
pub fn move_back(from: &Path, to: &Path) -> anyhow::Result<()> {
    if let Some(parent) = to.parent() { std::fs::create_dir_all(parent)?; }
    match crate::fileops::rename_no_replace(from, to) {
        Ok(()) => return Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {},
        Err(e) => return Err(e.into()),
    }
    let parent = to.parent().ok_or_else(|| anyhow::anyhow!("ad yok"))?;
    let stage = crate::fileops::unique_name(parent, ".brysk-undo", "");
    std::fs::create_dir(&stage)?;
    let staged = stage.join("item");
    let result = (|| -> anyhow::Result<()> {
        let (s, d) = (cpath(from)?, cpath(&staged)?);
        // copyfile.h: recurse, exclusive destination, preserve symlinks themselves.
        let flags = COPYFILE_ALL | (1 << 15) | (1 << 17) | (1 << 18) | (1 << 19);
        if unsafe { copyfile(s.as_ptr(), d.as_ptr(), std::ptr::null_mut(), flags) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        crate::fileops::rename_no_replace(&staged, to)?;
        crate::fileops::remove_recursive(from)
    })();
    let _ = std::fs::remove_dir_all(stage);
    result
}

/// Unused helper kept for the DnD path: whether a drop of `sources` into `dest` makes no sense.
pub fn is_invalid(sources: &[PathBuf], dest: &Path) -> bool {
    sources.iter().any(|s| s == dest || dest.starts_with(s))
}

/// The cancellation flag the caller may share with the engine (same shape as Windows').
pub fn cancellable(flag: Arc<AtomicBool>) -> Arc<AtomicBool> {
    flag
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-mac-copy-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tracker() -> Arc<Tracker> {
        Arc::new(Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false))))
    }

    /// A tracker whose question channel counts calls: every one of these tests has to finish
    /// without one (that is what used to hang a paste into the same folder).
    fn asking(calls: Arc<AtomicUsize>) -> Arc<Tracker> {
        let mut t = Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false)));
        t.ask = Some(Box::new(move |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            None
        }));
        Arc::new(t)
    }

    fn read(p: &Path) -> Vec<u8> {
        std::fs::read(p).unwrap()
    }

    #[test]
    fn a_copy_of_a_conflicting_file_keeps_both() {
        let d = scratch("keepboth");
        let src = d.join("a.txt");
        std::fs::write(&src, b"new").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("a.txt"), b"old").unwrap();
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &dest, tracker()).unwrap());
        assert_eq!(read(&dest.join("a.txt")), b"old");
        assert_eq!(read(&dest.join("a copy.txt")), b"new");
    }

    #[test]
    fn a_move_of_a_conflicting_file_keeps_both_and_moves_the_source() {
        let d = scratch("move");
        let src = d.join("a.txt");
        std::fs::write(&src, b"new").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("a.txt"), b"old").unwrap();
        assert!(transfer(Transfer::Move, std::slice::from_ref(&src), &dest, tracker()).unwrap());
        assert_eq!(read(&dest.join("a.txt")), b"old");
        assert_eq!(read(&dest.join("a copy.txt")), b"new");
        assert!(!src.exists());
    }

    #[test]
    fn copying_a_file_into_its_own_folder_makes_a_copy_without_asking() {
        let d = scratch("same-file");
        let src = d.join("a.txt");
        std::fs::write(&src, b"new").unwrap();
        let asked = Arc::new(AtomicUsize::new(0));
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &d, asking(asked.clone())).unwrap());
        assert_eq!(asked.load(Ordering::Relaxed), 0, "a same-folder copy must not ask");
        assert_eq!(read(&src), b"new");
        assert_eq!(read(&d.join("a copy.txt")), b"new");
    }

    #[test]
    fn another_copy_in_the_same_folder_gets_a_number() {
        let d = scratch("same-file-2");
        let src = d.join("a.txt");
        std::fs::write(&src, b"original").unwrap();
        std::fs::write(d.join("a copy.txt"), b"first").unwrap();
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &d, tracker()).unwrap());
        assert_eq!(read(&d.join("a copy 2.txt")), b"original");
        assert_eq!(read(&d.join("a copy.txt")), b"first");
    }

    #[test]
    fn copying_a_folder_into_its_own_folder_makes_a_copy_folder() {
        let d = scratch("same-folder");
        let src = d.join("Photos");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("one.txt"), b"one").unwrap();
        std::fs::write(src.join("sub/two.txt"), b"two").unwrap();
        let asked = Arc::new(AtomicUsize::new(0));
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &d, asking(asked.clone())).unwrap());
        assert_eq!(asked.load(Ordering::Relaxed), 0);
        // The original is untouched and there is no Photos/Photos copy.
        assert_eq!(read(&src.join("one.txt")), b"one");
        assert!(!src.join("Photos copy").exists());
        assert_eq!(read(&d.join("Photos copy/one.txt")), b"one");
        assert_eq!(read(&d.join("Photos copy/sub/two.txt")), b"two");
    }

    #[test]
    fn a_dangling_symlink_can_be_copied() {
        let d = scratch("dangling");
        let link = d.join("gone");
        std::os::unix::fs::symlink("/nonexistent/target", &link).unwrap();
        let dest = d.join("dest");
        std::fs::create_dir(&dest).unwrap();
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&link), &dest, tracker()).unwrap());
        assert_eq!(std::fs::read_link(dest.join("gone")).unwrap(), PathBuf::from("/nonexistent/target"));
    }

    #[test]
    fn an_app_bundle_is_one_item_never_merged_and_keeps_its_suffix_last() {
        let d = scratch("app-bundle");
        let src = d.join("Safari.app");
        std::fs::create_dir_all(src.join("Contents/MacOS")).unwrap();
        std::fs::write(src.join("Contents/MacOS/Safari"), b"new").unwrap();
        // duplicating it in its own folder: "Safari copy.app"
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &d, tracker()).unwrap());
        assert_eq!(read(&d.join("Safari copy.app/Contents/MacOS/Safari")), b"new");
        // pasting it where another Safari.app is: keep both, do not merge into the existing app
        let dest = d.join("dest");
        std::fs::create_dir_all(dest.join("Safari.app/Contents/MacOS")).unwrap();
        std::fs::write(dest.join("Safari.app/Contents/MacOS/Safari"), b"old").unwrap();
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &dest, tracker()).unwrap());
        assert_eq!(read(&dest.join("Safari.app/Contents/MacOS/Safari")), b"old");
        assert_eq!(read(&dest.join("Safari copy.app/Contents/MacOS/Safari")), b"new");
    }

    #[test]
    fn a_folder_with_a_dot_keeps_its_whole_name() {
        let d = scratch("dotted-folder");
        let src = d.join("Photos.2024");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("one.txt"), b"one").unwrap();
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &d, tracker()).unwrap());
        assert_eq!(read(&d.join("Photos.2024 copy/one.txt")), b"one");
    }

    #[test]
    fn a_second_same_folder_folder_copy_gets_a_number() {
        let d = scratch("same-folder-2");
        let src = d.join("Photos");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("one.txt"), b"one").unwrap();
        std::fs::create_dir_all(d.join("Photos copy")).unwrap();
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &d, tracker()).unwrap());
        assert_eq!(read(&d.join("Photos copy 2/one.txt")), b"one");
    }

    #[test]
    fn moving_a_file_into_its_own_folder_changes_nothing() {
        let d = scratch("same-move-file");
        let src = d.join("a.txt");
        std::fs::write(&src, b"new").unwrap();
        assert!(transfer(Transfer::Move, std::slice::from_ref(&src), &d, tracker()).unwrap());
        assert_eq!(read(&src), b"new");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
    }

    #[test]
    fn moving_a_folder_into_its_own_folder_keeps_it() {
        let d = scratch("same-move-folder");
        let src = d.join("Photos");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("one.txt"), b"one").unwrap();
        assert!(transfer(Transfer::Move, std::slice::from_ref(&src), &d, tracker()).unwrap());
        assert_eq!(read(&src.join("one.txt")), b"one");
        assert!(src.join("sub").is_dir());
        assert!(!d.join("Photos copy").exists());
    }

    #[test]
    fn a_folder_cannot_be_copied_into_itself() {
        let d = scratch("inside-copy");
        let src = d.join("Photos");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        // Into a descendant…
        let err = transfer(Transfer::Copy, std::slice::from_ref(&src), &src.join("sub"), tracker()).unwrap_err();
        assert!(err.to_string().contains("kendi içine"), "{err}");
        assert!(!src.join("sub/Photos").exists());
        // …or into the folder itself (pasting into the selected folder).
        let err = transfer(Transfer::Copy, std::slice::from_ref(&src), &src, tracker()).unwrap_err();
        assert!(err.to_string().contains("kendi içine"), "{err}");
        assert!(!src.join("Photos").exists());
    }

    #[test]
    fn a_folder_cannot_be_moved_into_itself() {
        let d = scratch("inside-move");
        let src = d.join("Photos");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        let err = transfer(Transfer::Move, std::slice::from_ref(&src), &src.join("sub"), tracker()).unwrap_err();
        assert!(err.to_string().contains("kendi içine"), "{err}");
        let err = transfer(Transfer::Move, std::slice::from_ref(&src), &src, tracker()).unwrap_err();
        assert!(err.to_string().contains("kendi içine"), "{err}");
        assert!(src.join("sub").is_dir());
    }

    #[test]
    fn two_sources_with_one_name_are_not_given_one_destination() {
        let d = scratch("two-sources");
        let a = d.join("a.txt");
        std::fs::write(&a, b"one").unwrap();
        let sub = d.join("sub");
        std::fs::create_dir(&sub).unwrap();
        let b = sub.join("a.txt");
        std::fs::write(&b, b"two").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir(&dest).unwrap();
        assert!(transfer(Transfer::Copy, &[a, b], &dest, tracker()).unwrap());
        assert_eq!(read(&dest.join("a.txt")), b"one");
        assert_eq!(read(&dest.join("a copy.txt")), b"two");
    }

    #[test]
    fn two_same_named_folders_stay_two_folders() {
        let d = scratch("two-folders");
        let one = d.join("one/Photos");
        let two = d.join("two/Photos");
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        std::fs::write(one.join("one.txt"), b"one").unwrap();
        std::fs::write(two.join("two.txt"), b"two").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir(&dest).unwrap();
        assert!(transfer(Transfer::Copy, &[one, two], &dest, tracker()).unwrap());
        assert_eq!(read(&dest.join("Photos/one.txt")), b"one");
        assert_eq!(read(&dest.join("Photos copy/two.txt")), b"two");
    }

    #[test]
    fn a_big_copy_with_full_verification_matches_byte_for_byte() {
        let d = scratch("verify");
        let src = d.join("big.bin");
        let data: Vec<u8> = (0..(3 << 20)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        let dest = d.join("dest");
        std::fs::create_dir(&dest).unwrap();
        let t = tracker();
        t.verify.store(Verify::Full as u8, Ordering::Relaxed);
        // A limit forces the chunked path (no clone).
        t.limit.store(50 << 20, Ordering::Relaxed);
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &dest, t).unwrap());
        assert_eq!(read(&dest.join("big.bin")), data);
    }

    #[test]
    fn folders_merge_and_nested_conflicts_keep_both() {
        let d = scratch("folders");
        let src = d.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/x.txt"), b"one").unwrap();
        std::fs::write(src.join("y.txt"), b"two").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir_all(dest.join("src/sub")).unwrap();
        std::fs::write(dest.join("src/sub/x.txt"), b"existing").unwrap();
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &dest, tracker()).unwrap());
        assert_eq!(read(&dest.join("src/sub/x.txt")), b"existing");
        assert_eq!(read(&dest.join("src/sub/x copy.txt")), b"one");
        assert_eq!(read(&dest.join("src/y.txt")), b"two");
    }

    #[test]
    fn moving_a_folder_merges_and_removes_the_source() {
        let d = scratch("move-folder");
        let src = d.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/y.txt"), b"two").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir_all(dest.join("src/sub")).unwrap();
        std::fs::write(dest.join("src/sub/x.txt"), b"existing").unwrap();
        assert!(transfer(Transfer::Move, std::slice::from_ref(&src), &dest, tracker()).unwrap());
        assert_eq!(read(&dest.join("src/sub/x.txt")), b"existing");
        assert_eq!(read(&dest.join("src/sub/y.txt")), b"two");
        assert!(!src.exists(), "the emptied source folder goes");
    }

    #[test]
    fn a_folder_named_like_a_file_keeps_both() {
        let d = scratch("type-clash");
        let src = d.join("Photos");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("one.txt"), b"one").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("Photos"), b"a file, not a folder").unwrap();
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &dest, tracker()).unwrap());
        assert_eq!(read(&dest.join("Photos")), b"a file, not a folder");
        assert_eq!(read(&dest.join("Photos copy/one.txt")), b"one");
    }
}

#[cfg(test)]
mod undo_tests {
    use super::*;

    #[test]
    fn a_new_package_has_one_undo_record_and_a_move_restores_its_exact_name() {
        let root = std::env::temp_dir().join(format!("brysk-undo-package-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let app = root.join("Demo.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::create_dir_all(app.join("Contents/Empty")).unwrap();
        std::fs::write(app.join("Contents/MacOS/Demo"), b"app").unwrap();
        let dest = root.join("dest");
        std::fs::create_dir_all(dest.join("Demo.app")).unwrap();
        let tracker = Arc::new(Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false))));
        assert!(transfer(Transfer::Move, std::slice::from_ref(&app), &dest, tracker.clone()).unwrap());
        let moved = dest.join("Demo copy.app");
        assert_eq!(*tracker.results.lock().unwrap(), vec![(app.clone(), moved.clone())]);
        assert!(!app.exists());
        move_back(&moved, &app).unwrap();
        assert_eq!(std::fs::read(app.join("Contents/MacOS/Demo")).unwrap(), b"app");
        assert!(app.join("Contents/Empty").is_dir());
        assert!(dest.join("Demo.app").is_dir());
        assert!(!moved.exists());
        std::fs::write(root.join("occupied"), b"keep").unwrap();
        assert!(move_back(&app, &root.join("occupied")).is_err());
        assert!(app.is_dir());
        assert_eq!(std::fs::read(root.join("occupied")).unwrap(), b"keep");
        std::fs::remove_dir_all(root).unwrap();
    }
}
