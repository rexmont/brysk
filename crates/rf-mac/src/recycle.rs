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

/// Moves `path` to the Trash with `NSFileManager`, so it lands in the right volume's Trash,
/// keeps Finder's "Put Back" record when the system writes one, and needs no guessing about
/// `/Volumes/*/.Trashes`. Falls back to a plain move into `~/.Trash` when AppKit refuses (an
/// unusual volume, no permission).
pub fn trash(path: &Path) -> anyhow::Result<PathBuf> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{MainThreadMarker, NSString, NSURL};

    let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
    let p = path.to_path_buf();
    dispatch2::DispatchQueue::main().exec_async(move || {
        let out = if MainThreadMarker::new().is_some() {
            let fm = objc2_foundation::NSFileManager::defaultManager();
            let url = NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()));
            let mut resulting: Option<objc2::rc::Retained<NSURL>> = None;
            match fm.trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting)) {
                Ok(()) => resulting.and_then(|u| u.path()).map(|s| PathBuf::from(s.to_string())),
                Err(e) => {
                    log::warn!("trash: {e}");
                    None
                }
            }
        } else {
            // The main queue was not running (a test or a headless tool): the caller falls back.
            let _ = NSWorkspace::sharedWorkspace();
            None
        };
        let _ = tx.send(out);
    });
    if let Ok(Some(dest)) = rx.recv_timeout(std::time::Duration::from_secs(10)) {
        return Ok(dest);
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
    Ok(dest)
}

fn ms(t: std::time::SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Everything in the Trash.
pub fn list() -> anyhow::Result<Vec<RecycleItem>> {
    let Some(dir) = trash_dir() else { return Ok(Vec::new()) };
    let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == ".DS_Store" || name == ".localized" {
            continue;
        }
        let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
        let size = if md.is_dir() { 0 } else { md.len() };
        out.push(RecycleItem {
            name,
            original_location: String::new(),
            deleted_ms: ms(md.modified().unwrap_or(std::time::UNIX_EPOCH)),
            size,
            parsing_path: e.path().display().to_string(),
            is_dir: md.is_dir(),
        });
    }
    out.sort_by(|a, b| b.deleted_ms.cmp(&a.deleted_ms));
    Ok(out)
}

/// Puts items back, into their original folder when it is known and the home folder otherwise.
pub fn restore(items: &[RecycleItem]) -> anyhow::Result<()> {
    let home = dirs::home_dir();
    for it in items {
        let from = PathBuf::from(&it.parsing_path);
        let dir = if it.original_location.is_empty() {
            home.clone().ok_or_else(|| anyhow::anyhow!("ev klasörü yok"))?
        } else {
            PathBuf::from(&it.original_location)
        };
        std::fs::create_dir_all(&dir)?;
        let dest = crate::fileops::unique_name(&dir, &it.name, "");
        std::fs::rename(&from, &dest)?;
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
    let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(()) };
    for e in rd.flatten() {
        let p = e.path();
        let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    }
    Ok(())
}

pub fn info() -> anyhow::Result<(u64, u64)> {
    let items = list()?;
    let count = items.len() as u64;
    let bytes = items.iter().map(|i| i.size).sum();
    Ok((count, bytes))
}