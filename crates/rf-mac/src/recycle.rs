//! The Trash, the macOS half of `rf-win::recycle`: listing, restoring and emptying.
//!
//! There is one Trash per volume (`~/.Trash` for the home volume, `/Volumes/X/.Trashes/<uid>`
//! for the others, and none on most network shares). Listing shows the home Trash; restore
//! puts an item back where it came from when that place is known (macOS keeps no "original
//! location" record for items deleted by other apps, so it is the home folder).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One item in the Trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecycleItem {
    pub name: String,
    /// Folder the item was deleted from; empty when unknown (macOS keeps no record).
    pub original_location: String,
    /// Deletion time, Unix milliseconds (UTC); 0 when unknown.
    pub deleted_ms: i64,
    pub size: u64,
    /// Real location inside the Trash; also the key used by [`restore`].
    pub parsing_path: String,
    pub is_dir: bool,
}

pub fn trash_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".Trash"))
}

// ---------------------------------------------------------------- where a trashed item came from

/// macOS keeps no readable record of an item's original folder for other apps (Finder's Put
/// Back data is private), so this is our own small journal: trashed path → original path.
fn journal_path() -> Option<PathBuf> {
    if let Some(cfg) = std::env::var_os("RUSTYFILES_CONFIG_DIR") {
        return Some(PathBuf::from(cfg).join("trash.json"));
    }
    dirs::data_dir().map(|d| d.join("Brysk").join("trash.json"))
}

/// One read-modify-write of the journal at a time: two deletions (or a deletion and an empty)
/// running together must not drop each other's entries.
static JOURNAL_LOCK: Mutex<()> = Mutex::new(());

fn load_journal_unlocked() -> HashMap<String, String> {
    journal_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Atomic replace: a reader never sees half a JSON file (an empty read would lose every
/// remembered original folder until the next write).
fn save_journal_unlocked(map: &HashMap<String, String>) {
    let Some(p) = journal_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(text) = serde_json::to_string(map) else { return };
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

fn with_journal(f: impl FnOnce(&mut HashMap<String, String>)) {
    let _guard = JOURNAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = load_journal_unlocked();
    f(&mut map);
    save_journal_unlocked(&map);
}

fn load_journal() -> HashMap<String, String> {
    let _guard = JOURNAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_journal_unlocked()
}

fn record_original(original: &Path, trashed: &Path) {
    // The item's full original path: the Trash may have given it another name (Finder and
    // NSFileManager add a timestamp when the name is taken), and restore must put it back as
    // it was called.
    let trashed = trashed.to_string_lossy().into_owned();
    let original = original.to_string_lossy().into_owned();
    with_journal(|map| {
        map.insert(trashed, original);
    });
}

fn original_of(trashed: &str) -> Option<PathBuf> {
    load_journal().get(trashed).map(PathBuf::from)
}

/// A free name for `name` in `dir`, the Finder's way: "report 2.txt" (the extension stays at
/// the end) and "Photos.2024 2" for a folder, whose dots are part of its name.
fn available(dir: &Path, name: &str, is_dir: bool) -> PathBuf {
    if is_dir {
        return crate::fileops::unique_name(dir, name, "");
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_string());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    crate::fileops::unique_name(dir, &stem, &ext)
}

fn forget(trashed: &Path) {
    let trashed = trashed.to_string_lossy().to_string();
    with_journal(|map| {
        map.remove(&trashed);
    });
}

/// Removes a file, a symlink or a folder for good. A symlink to a folder is removed as a link,
/// never by walking into what it points at.
fn remove_path(path: &Path) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(path)?;
    if md.file_type().is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// [`remove_path`] where an item already gone counts as done: its journal entry can go too.
/// A missing item whose Trash folder is missing too may be on an unmounted volume, and its
/// entry stays so the item shows again when the volume returns.
fn remove_for_good(path: &Path) -> std::io::Result<()> {
    match remove_path(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && path.parent().is_some_and(Path::exists) => Ok(()),
        other => other,
    }
}

/// Moves a path back out of the Trash: a rename on one volume, a copy and a delete when the
/// fallback put an item of another volume into the home Trash.
fn move_path(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::rename(from, to).or_else(|_| {
        crate::fileops::copy_recursive(from, to)?;
        crate::fileops::remove_recursive(from)
    })
}

/// Moves `path` to the Trash with `NSFileManager`, so it lands in the right volume's Trash,
/// keeps Finder's "Put Back" record when the system writes one, and needs no guessing about
/// `/Volumes/*/.Trashes`. Falls back to a plain move into `~/.Trash` when AppKit refuses (an
/// unusual volume, no permission).
pub fn trash(path: &Path) -> anyhow::Result<PathBuf> {
    use objc2_foundation::{NSString, NSURL};

    // NSFileManager is Foundation and thread-safe: no AppKit/main-queue hop is needed.
    let fm = objc2_foundation::NSFileManager::defaultManager();
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let mut resulting: Option<objc2::rc::Retained<NSURL>> = None;
    match fm.trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting)) {
        Ok(()) => {
            if let Some(dest) = resulting.and_then(|u| u.path()).map(|s| PathBuf::from(s.to_string())) {
                record_original(path, &dest);
                return Ok(dest);
            }
        }
        Err(e) => log::warn!("trash: {e}"),
    }

    // Fallback: the home volume's Trash, with a name that is free.
    let dir = trash_dir().ok_or_else(|| anyhow::anyhow!("Çöp Kutusu bulunamadı"))?;
    std::fs::create_dir_all(&dir)?;
    let name = path.file_name().ok_or_else(|| anyhow::anyhow!("ad yok"))?.to_string_lossy().into_owned();
    let is_dir = std::fs::symlink_metadata(path).map(|m| m.file_type().is_dir()).unwrap_or(false);
    let dest = available(&dir, &name, is_dir);
    std::fs::rename(path, &dest).or_else(|_| {
        crate::fileops::copy_recursive(path, &dest)?;
        crate::fileops::remove_recursive(path)
    })?;
    record_original(path, &dest);
    Ok(dest)
}

/// When the item landed in the Trash: moving it there is a rename, so the inode's change time
/// is that moment (the Finder's "Date Added"). The modification time is the file's own and can
/// be years older.
fn deleted_ms(md: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    // (a filesystem without a change time: its modification time is the best there is)
    let (secs, nsec) = if md.ctime() > 0 { (md.ctime(), md.ctime_nsec()) } else { (md.mtime(), md.mtime_nsec()) };
    secs.saturating_mul(1000).saturating_add(nsec / 1_000_000)
}

/// Everything in the Trash.
pub fn list() -> anyhow::Result<Vec<RecycleItem>> {
    let Some(dir) = trash_dir() else { return Ok(Vec::new()) };
    let journal = load_journal();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |path: PathBuf| {
        let key = path.to_string_lossy().into_owned();
        if !seen.insert(key.clone()) {
            return;
        }
        let Ok(md) = std::fs::symlink_metadata(&path) else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name == ".DS_Store" || name == ".localized" {
            return;
        }
        out.push(RecycleItem {
            name,
            original_location: journal.get(&key).and_then(|p| Path::new(p).parent().map(|d| d.to_string_lossy().into_owned())).unwrap_or_default(),
            deleted_ms: deleted_ms(&md),
            size: if md.is_dir() { 0 } else { md.len() },
            parsing_path: key,
            is_dir: md.is_dir(),
        });
    };
    // The Finder's own Trash cannot be listed without Full Disk Access; our journal can.
    // Enumerating is tried first (a user who granted access sees everything), then every known
    // trashed path is added (and stat works even when listing does not).
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            add(e.path());
        }
    }
    for path in journal.keys() {
        add(PathBuf::from(path));
    }
    out.sort_by(|a, b| b.deleted_ms.cmp(&a.deleted_ms));
    Ok(out)
}

/// Puts `(original, trashed)` pairs back without listing the Trash (undo uses this: listing the
/// Finder's Trash is not allowed without Full Disk Access, moving a known path is).
pub fn restore_pairs(pairs: &[(PathBuf, PathBuf)]) -> anyhow::Result<()> {
    for (original, trashed) in pairs {
        if trashed.as_os_str().is_empty() {
            continue;
        }
        // (symlink_metadata: a link trashed with its target gone is still restorable)
        let Ok(md) = std::fs::symlink_metadata(trashed) else {
            // (its Trash folder gone too: maybe the volume is unmounted — keep the record)
            if trashed.parent().is_some_and(Path::exists) {
                forget(trashed);
            }
            continue;
        };
        let is_dir = md.file_type().is_dir();
        let dir = original.parent().unwrap_or_else(|| Path::new("/"));
        std::fs::create_dir_all(dir)?;
        let name = original.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "item".into());
        let dest = available(dir, &name, is_dir);
        move_path(trashed, &dest)?;
        forget(trashed);
    }
    Ok(())
}

/// Puts items back, into their original folder and (when the journal knows it) their original
/// name. Falls back to the home folder for items trashed by someone else.
pub fn restore(items: &[RecycleItem]) -> anyhow::Result<()> {
    let home = dirs::home_dir();
    for it in items {
        let from = PathBuf::from(&it.parsing_path);
        // The listing may be a moment old: an item deleted for good meanwhile is left alone.
        let Ok(md) = std::fs::symlink_metadata(&from) else {
            if from.parent().is_some_and(Path::exists) {
                forget(&from);
            }
            continue;
        };
        let is_dir = md.file_type().is_dir();
        let original = original_of(&it.parsing_path);
        let dir = original
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| (!it.original_location.is_empty()).then(|| PathBuf::from(&it.original_location)))
            .or_else(|| home.clone())
            .ok_or_else(|| anyhow::anyhow!("ev klasörü yok"))?;
        let name = original.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| it.name.clone());
        std::fs::create_dir_all(&dir)?;
        let dest = available(&dir, &name, is_dir);
        move_path(&from, &dest)?;
        forget(&from);
    }
    Ok(())
}

/// The Trash behind the bin page (`rf_core::recycle_bin::BinBackend`).
pub struct ShellBin;

impl ShellBin {
    pub fn info() -> (u64, u64) {
        info().unwrap_or((0, 0))
    }
}

impl rf_core::recycle_bin::BinBackend for ShellBin {
    fn list_keys(&self) -> anyhow::Result<Vec<String>> {
        Ok(list()?.into_iter().map(|i| i.parsing_path).collect())
    }

    fn delete_keys(&self, keys: &[String]) -> anyhow::Result<()> {
        // The shared bin code folds keys to lower case (Windows' paths are case-insensitive);
        // APFS can be case-sensitive, so look the real path up before removing it.
        let known: Vec<PathBuf> = list()?.into_iter().map(|i| PathBuf::from(i.parsing_path)).collect();
        let mut first_error: Option<anyhow::Error> = None;
        for key in keys {
            // A case-sensitive volume can hold two items whose names differ only in case; then
            // the folded key is ambiguous and nothing is deleted rather than the wrong one.
            let mut matches = known.iter().filter(|p| p.to_string_lossy().to_lowercase() == key.as_str());
            let (Some(path), None) = (matches.next(), matches.next()) else { continue };
            match remove_for_good(path) {
                Ok(()) => forget(path),
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e.into());
                    }
                }
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Empties the Trash. `confirm` is kept for the signature: the caller asks first.
pub fn empty(_confirm: bool) -> anyhow::Result<()> {
    let Some(dir) = trash_dir() else { return Ok(()) };
    let mut removed: HashSet<String> = HashSet::new();
    // The home Trash: everything in it goes, as Finder's Empty Trash does. Without Full Disk
    // Access it cannot be listed; the journal's paths are still removable one by one.
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if remove_for_good(&p).is_ok() {
                removed.insert(p.to_string_lossy().into_owned());
            }
        }
    }
    // Other volumes' Trash and the items the listing above could not reach: our journal knows
    // them by path, and removing a known path is allowed.
    for key in load_journal().keys() {
        if removed.contains(key) {
            continue;
        }
        if remove_for_good(&PathBuf::from(key)).is_ok() {
            removed.insert(key.clone());
        }
    }
    // Keep what stayed (a permission failure, another user's file): the bin still lists it.
    with_journal(|map| map.retain(|k, _| !removed.contains(k)));
    Ok(())
}

pub fn info() -> anyhow::Result<(u64, u64)> {
    let items = list()?;
    let count = items.len() as u64;
    let bytes = items.iter().map(|i| i.size).sum();
    Ok((count, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trashing_remembers_the_original_folder_and_restore_puts_it_back() {
        let base = std::env::temp_dir().join(format!("rf-mac-trash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("work")).unwrap();
        let original = base.join("work/note.txt");
        std::fs::write(&original, b"hello").unwrap();
        // The journal goes to the test's config folder, not the user's real one.
        unsafe { std::env::set_var("RUSTYFILES_CONFIG_DIR", base.join("cfg")) };

        let trashed = trash(&original).expect("trash");
        assert!(!original.exists());
        let items = list().unwrap();
        let item = items.into_iter().find(|i| i.parsing_path == trashed.to_string_lossy()).expect("the item is in the Trash");
        assert_eq!(item.original_location, base.join("work").to_string_lossy());
        restore(&[item]).expect("restore");
        assert_eq!(std::fs::read(&original).unwrap(), b"hello");
        assert!(!trashed.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_restore_number_keeps_a_files_extension_last_and_a_folders_dots() {
        let base = std::env::temp_dir().join(format!("rf-mac-available-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("rapor.txt"), b"").unwrap();
        std::fs::write(base.join("rapor 2.txt"), b"").unwrap();
        assert_eq!(available(&base, "rapor.txt", false).file_name().unwrap().to_string_lossy(), "rapor 3.txt");
        // "Photos.2024" is a folder name, not a stem and an extension
        std::fs::create_dir_all(base.join("Photos.2024")).unwrap();
        std::fs::create_dir_all(base.join("Photos.2024 2")).unwrap();
        assert_eq!(available(&base, "Photos.2024", true).file_name().unwrap().to_string_lossy(), "Photos.2024 3");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn removing_a_link_to_a_folder_leaves_the_folder_alone() {
        let base = std::env::temp_dir().join(format!("rf-mac-remove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("target")).unwrap();
        std::fs::write(base.join("target/keep.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(base.join("target"), base.join("link")).unwrap();

        remove_path(&base.join("link")).unwrap();
        assert!(std::fs::symlink_metadata(base.join("link")).is_err());
        assert_eq!(std::fs::read(base.join("target/keep.txt")).unwrap(), b"x");
        let _ = std::fs::remove_dir_all(&base);
    }
}
