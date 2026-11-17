//! The Trash, the macOS half of `rf-win::recycle`: listing, restoring and emptying.
//!
//! There is one Trash per volume (`~/.Trash` for the home volume, `/Volumes/X/.Trashes/<uid>`
//! for the others, and none on most network shares). Listing shows the home Trash; restore
//! puts an item back where it came from when that place is known (macOS keeps no "original
//! location" record for items deleted by other apps, so it is the home folder).

use std::path::{Path, PathBuf};

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

fn load_journal() -> std::collections::HashMap<String, String> {
    journal_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_journal(map: &std::collections::HashMap<String, String>) {
    let Some(p) = journal_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(map) {
        let _ = std::fs::write(p, text);
    }
}

fn record_original(original: &Path, trashed: &Path) {
    let mut map = load_journal();
    // The item's full original path: the Trash may have given it another name (Finder and
    // NSFileManager add a timestamp when the name is taken), and restore must put it back as
    // it was called.
    map.insert(trashed.to_string_lossy().into_owned(), original.to_string_lossy().into_owned());
    save_journal(&map);
}

fn original_of(trashed: &str) -> Option<PathBuf> {
    load_journal().get(trashed).map(PathBuf::from)
}

/// A free name for `name` in `dir`, keeping its stem and extension.
fn available(dir: &Path, name: &str) -> PathBuf {
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_string());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    crate::fileops::unique_name(dir, &stem, &ext)
}

fn forget(trashed: &Path) {
    let mut map = load_journal();
    if map.remove(&trashed.to_string_lossy().to_string()).is_some() {
        save_journal(&map);
    }
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
    let dest = crate::fileops::unique_name(&dir, &name, "");
    std::fs::rename(path, &dest).or_else(|_| {
        crate::fileops::copy_recursive(path, &dest)?;
        crate::fileops::remove_recursive(path)
    })?;
    record_original(path, &dest);
    Ok(dest)
}

fn ms(t: std::time::SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
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
            deleted_ms: ms(md.modified().unwrap_or(std::time::UNIX_EPOCH)),
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
        if trashed.as_os_str().is_empty() || !trashed.exists() {
            continue;
        }
        let dir = original.parent().unwrap_or_else(|| Path::new("/"));
        std::fs::create_dir_all(dir)?;
        let name = original.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "item".into());
        let dest = available(dir, &name);
        std::fs::rename(trashed, &dest)?;
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
        let dest = available(&dir, &name);
        std::fs::rename(&from, &dest)?;
        forget(&from);
    }
    Ok(())
}

/// The Trash as the shell shows it (the namespace this app browses on Windows).
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
        for k in keys {
            let p = PathBuf::from(k);
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
        Ok(())
    }
}

/// Empties the Trash. `confirm` is kept for the signature: the caller asks first.
pub fn empty(_confirm: bool) -> anyhow::Result<()> {
    let Some(dir) = trash_dir() else { return Ok(()) };
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    } else {
        // Without Full Disk Access the Trash cannot be listed; the items this app trashed are
        // known by path, and removing a known path is allowed.
        for p in load_journal().keys() {
            let p = PathBuf::from(p);
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }
    save_journal(&Default::default());
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
}
