//! Our own copy / move engine, the macOS half of `rf-win::fastcopy`.
//!
//! Copies keep every metadata bit: `copyfile(COPYFILE_ALL)` carries permissions, times, ACLs,
//! extended attributes and Finder tags, and an APFS clone is used when both ends are on the
//! same volume. Files are copied in chunks for byte-accurate progress, pause, cancellation and
//! a speed limit; anything that already exists at the destination is asked about once (replace,
//! skip, keep both) instead of ever being overwritten silently. A move on one volume is a
//! rename; across volumes it is a verified copy after which the source is deleted.

use std::collections::HashMap;
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

fn plan_walk(src: &Path, dst: &Path, dirs: &mut Vec<PathBuf>, files: &mut Vec<PlanFile>) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(src)?;
    if md.file_type().is_symlink() || md.is_file() {
        files.push(PlanFile { src: src.to_path_buf(), dst: dst.to_path_buf(), size: md.len() });
        return Ok(());
    }
    dirs.push(dst.to_path_buf());
    for e in std::fs::read_dir(src)? {
        let e = e?;
        plan_walk(&e.path(), &dst.join(e.file_name()), dirs, files)?;
    }
    Ok(())
}

fn unique_for(dst: &Path) -> PathBuf {
    let parent = dst.parent().unwrap_or_else(|| Path::new("/"));
    let stem = dst.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "item".into());
    let ext = dst.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    crate::fileops::unique_name(parent, &stem, &ext)
}

fn conflict_for(f: &PlanFile) -> Option<Conflict> {
    let src_md = std::fs::symlink_metadata(&f.src).ok()?;
    let dst_md = std::fs::symlink_metadata(&f.dst).ok()?;
    Some(Conflict {
        src: f.src.clone(),
        dst: f.dst.clone(),
        src_dir: false,
        dst_dir: dst_md.is_dir(),
        src_size: src_md.len(),
        dst_size: dst_md.len(),
        src_time: src_md.modified().ok(),
        dst_time: dst_md.modified().ok(),
    })
}

fn resolve_conflicts(files: &mut [PlanFile], tracker: &Tracker) -> anyhow::Result<Option<HashMap<PathBuf, Resolve>>> {
    let conflicts: Vec<(usize, Conflict)> = files.iter().enumerate().filter_map(|(i, f)| conflict_for(f).map(|c| (i, c))).collect();
    if conflicts.is_empty() {
        return Ok(Some(HashMap::new()));
    }
    let list: Vec<Conflict> = conflicts.iter().map(|(_, c)| c.clone()).collect();
    let answers = match tracker.ask.as_ref() {
        // A question nobody answers is a cancelled operation (the dialog was closed).
        Some(ask) => ask(&list),
        // An untracked operation never overwrites: keep both.
        None => Some(vec![Resolve::KeepBoth; list.len()]),
    };
    let Some(answers) = answers else { return Ok(None) };
    if answers.len() != list.len() {
        anyhow::bail!("the conflict answers do not match the conflicts");
    }
    let mut out = HashMap::new();
    for ((i, c), r) in conflicts.into_iter().zip(answers) {
        // A file cannot replace a folder (or the reverse): keep both whatever was answered.
        let r = if c.mismatched() { Resolve::KeepBoth } else { r };
        out.insert(files[i].dst.clone(), r);
    }
    Ok(Some(out))
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

/// Copies or moves `sources` into `dest_dir`, asking once about every name that exists there.
pub fn transfer(kind: Transfer, sources: &[PathBuf], dest_dir: &Path, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    if sources.is_empty() {
        return Ok(true);
    }
    std::fs::create_dir_all(dest_dir)?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for src in sources {
        let name = src.file_name().ok_or_else(|| anyhow::anyhow!("ad yok: {}", src.display()))?;
        plan_walk(src, &dest_dir.join(name), &mut dirs, &mut files)?;
    }

    let total_bytes: u64 = files.iter().map(|f| f.size).sum();
    let total_files = files.len();
    let journal = write_journal(kind, sources, dest_dir, total_files);
    tracker.bytes.set(0, total_bytes);
    (tracker.on_progress)(0, total_files as u32, None);

    let Some(mut answers) = resolve_conflicts(&mut files, &tracker)? else {
        return Ok(false);
    };

    // Directories first (a merge is harmless), shallowest first.
    dirs.sort_by_key(|d| d.components().count());
    for d in &dirs {
        if tracker.cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        if let Ok(md) = std::fs::symlink_metadata(d)
            && !md.is_dir()
        {
            anyhow::bail!("{}: hedefte aynı adlı bir dosya var", d.display());
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
        match answers.remove(&f.dst) {
            Some(Resolve::Skip) => {
                done += 1;
                (tracker.on_progress)(done as u32, total_files as u32, Some(name));
            }
            Some(Resolve::KeepBoth) => {
                let dst = unique_for(&f.dst);
                copy_one(&f.src, &dst, f.size, &tracker)?;
                if kind == Transfer::Move {
                    crate::fileops::remove_recursive(&f.src)?;
                }
                tracker.results.lock().unwrap().push((f.src.clone(), dst));
                done += 1;
                (tracker.on_progress)(done as u32, total_files as u32, Some(name));
            }
            other => {
                let replace = other == Some(Resolve::Replace);
                let same_volume = same_device(&f.src, &f.dst);
                if kind == Transfer::Move && same_volume {
                    if replace
                        && let Ok(md) = std::fs::symlink_metadata(&f.dst)
                        && md.is_dir()
                    {
                        std::fs::remove_dir_all(&f.dst)?;
                    }
                    // (rename replaces an existing file atomically)
                    std::fs::rename(&f.src, &f.dst)?;
                    tracker.results.lock().unwrap().push((f.src.clone(), f.dst.clone()));
                    tracker.bytes.add(f.size);
                } else if kind == Transfer::Move {
                    // Across volumes: copy first, delete the source only once the copy checks out.
                    let dst = if replace && f.dst.exists() {
                        let tmp = f.dst.with_extension(format!("Brysk-part-{}", std::process::id()));
                        copy_one(&f.src, &tmp, f.size, &tracker)?;
                        std::fs::rename(&tmp, &f.dst)?;
                        f.dst.clone()
                    } else {
                        copy_one(&f.src, &f.dst, f.size, &tracker)?;
                        f.dst.clone()
                    };
                    if tracker.verify_mode() != Verify::Off {
                        verify_one(&f.src, &dst, tracker.verify_mode())?;
                    }
                    crate::fileops::remove_recursive(&f.src)?;
                    tracker.results.lock().unwrap().push((f.src.clone(), dst));
                } else if replace && f.dst.exists() {
                    let tmp = f.dst.with_extension(format!("Brysk-part-{}", std::process::id()));
                    copy_one(&f.src, &tmp, f.size, &tracker)?;
                    let md = std::fs::symlink_metadata(&f.dst)?;
                    if md.is_dir() {
                        std::fs::remove_dir_all(&f.dst)?;
                    }
                    std::fs::rename(&tmp, &f.dst)?;
                    tracker.results.lock().unwrap().push((f.src.clone(), f.dst.clone()));
                } else {
                    copy_one(&f.src, &f.dst, f.size, &tracker)?;
                    tracker.results.lock().unwrap().push((f.src.clone(), f.dst.clone()));
                }
                done += 1;
                (tracker.on_progress)(done as u32, total_files as u32, Some(name));
            }
        }
    }

    // A move removed each file; the emptied source folders go too (deepest first).
    if kind == Transfer::Move {
        let mut src_dirs = Vec::new();
        for src in sources {
            if std::fs::symlink_metadata(src).map(|m| m.is_dir()).unwrap_or(false) {
                src_dirs.push(src.clone());
            }
        }
        src_dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for d in src_dirs {
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    if let Some(j) = &journal {
        let _ = std::fs::remove_file(j);
    }
    (tracker.on_progress)(total_files as u32, total_files as u32, None);
    Ok(true)
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
    use std::sync::atomic::AtomicBool;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-mac-copy-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tracker(ask: Option<Box<dyn Fn(&[Conflict]) -> Option<Vec<Resolve>> + Send + Sync>>) -> Arc<Tracker> {
        let mut t = Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false)));
        t.ask = ask;
        Arc::new(t)
    }

    #[test]
    fn a_copy_keeps_both_instead_of_overwriting() {
        let d = scratch("keepboth");
        let src = d.join("a.txt");
        std::fs::write(&src, b"new").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("a.txt"), b"old").unwrap();
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &dest, tracker(None)).unwrap());
        assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"old");
        assert_eq!(std::fs::read(dest.join("a (2).txt")).unwrap(), b"new");
    }

    #[test]
    fn a_move_never_overwrites_without_an_answer() {
        let d = scratch("move");
        let src = d.join("a.txt");
        std::fs::write(&src, b"new").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("a.txt"), b"old").unwrap();
        assert!(transfer(Transfer::Move, std::slice::from_ref(&src), &dest, tracker(None)).unwrap());
        assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"old");
        assert_eq!(std::fs::read(dest.join("a (2).txt")).unwrap(), b"new");
        assert!(!src.exists());
    }

    #[test]
    fn replace_is_applied_when_answered() {
        let d = scratch("replace");
        let src = d.join("a.txt");
        std::fs::write(&src, b"new").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("a.txt"), b"old").unwrap();
        let t = tracker(Some(Box::new(|list| Some(vec![Resolve::Replace; list.len()]))));
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &dest, t).unwrap());
        assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"new");
    }

    #[test]
    fn a_big_copy_with_full_verification_matches_byte_for_byte() {
        let d = scratch("verify");
        let src = d.join("big.bin");
        let data: Vec<u8> = (0..(3 << 20)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        let dest = d.join("dest");
        std::fs::create_dir(&dest).unwrap();
        let t = tracker(None);
        t.verify.store(Verify::Full as u8, Ordering::Relaxed);
        // A limit forces the chunked path (no clone).
        t.limit.store(50 << 20, Ordering::Relaxed);
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &dest, t).unwrap());
        assert_eq!(std::fs::read(dest.join("big.bin")).unwrap(), data);
    }

    #[test]
    fn folders_merge_and_new_names_are_kept() {
        let d = scratch("folders");
        let src = d.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/x.txt"), b"one").unwrap();
        std::fs::write(src.join("y.txt"), b"two").unwrap();
        let dest = d.join("dest");
        std::fs::create_dir_all(dest.join("src/sub")).unwrap();
        std::fs::write(dest.join("src/sub/x.txt"), b"existing").unwrap();
        assert!(transfer(Transfer::Copy, std::slice::from_ref(&src), &dest, tracker(None)).unwrap());
        assert_eq!(std::fs::read(dest.join("src/sub/x.txt")).unwrap(), b"existing");
        assert_eq!(std::fs::read(dest.join("src/sub/x (2).txt")).unwrap(), b"one");
        assert_eq!(std::fs::read(dest.join("src/y.txt")).unwrap(), b"two");
    }
}
