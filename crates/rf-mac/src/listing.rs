//! Directory listing, the macOS half of `rf-win::listing`.
//!
//! The list is built from `readdir` + `lstat`, with the same `rf_core::Entry` shape the UI
//! already understands. Hidden means "name starts with a dot" (the Finder rule); macOS has no
//! separate "protected operating system file" attribute a dotfile should carry, so `.git` and
//! `.config` show with "show hidden items" alone. A symbolic link is marked like a junction is
//! on Windows. Packages (`.app`, `.photoslibrary`) stay folders the user can enter, exactly as
//! they are on disk.

use std::path::Path;
use std::time::UNIX_EPOCH;

use rf_core::entry::attrs;
use rf_core::{Entry, EntryKind};

#[derive(Debug, thiserror::Error)]
pub enum ListError {
    #[error("Klasör bulunamadı")]
    NotFound,
    #[error("Erişim reddedildi")]
    AccessDenied,
    #[error("Geçersiz klasör adı")]
    InvalidName,
    #[error("Sürücü hazır değil")]
    NotReady,
    #[error("Ağ yolu bulunamadı")]
    NetworkPath,
    #[error("Listeleme hatası (kod {0})")]
    Os(u32),
    #[error("İptal edildi")]
    Cancelled,
}

fn errno(e: &std::io::Error) -> ListError {
    match e.raw_os_error() {
        Some(libc::ENOENT) => ListError::NotFound,
        Some(libc::EACCES) | Some(libc::EPERM) => ListError::AccessDenied,
        Some(libc::ENOTDIR) | Some(libc::ENAMETOOLONG) => ListError::InvalidName,
        Some(libc::ENOTCONN) | Some(libc::EHOSTDOWN) | Some(libc::EHOSTUNREACH) | Some(libc::ETIMEDOUT) => ListError::NetworkPath,
        Some(code) => ListError::Os(code as u32),
        None => ListError::Os(0),
    }
}

fn ms(t: Option<std::time::SystemTime>) -> i64 {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn entry_of(name: &str, md: &std::fs::Metadata) -> Entry {
    use std::os::unix::fs::MetadataExt;
    let is_dir = md.is_dir();
    let mut at = 0u32;
    if name.starts_with('.') {
        at |= attrs::HIDDEN;
    }
    if is_dir {
        at |= attrs::DIRECTORY;
    }
    if md.file_type().is_symlink() {
        at |= attrs::REPARSE_POINT;
    }
    // A cloud placeholder (iCloud "optimise storage") is not local: mark it like OneDrive's
    // online-only files so the list shows the cloud state.
    if md.blocks() == 0 && !is_dir && md.size() > 0 {
        at |= attrs::OFFLINE | attrs::RECALL_ON_DATA_ACCESS;
    }
    let mut e = Entry::new(name, if is_dir { EntryKind::Dir } else { EntryKind::File }, md.size(), ms(md.modified().ok()), ms(md.created().ok()), at);
    e.link = md.file_type().is_symlink();
    e
}

/// Enumerates `dir` and streams entries to `on_batch` in chunks so the UI can show the first
/// items immediately. `cancelled` is polled between batches.
pub fn list_dir(
    dir: &Path,
    batch_size: usize,
    cancelled: &dyn Fn() -> bool,
    on_batch: &mut dyn FnMut(Vec<Entry>),
) -> Result<usize, ListError> {
    let rd = std::fs::read_dir(dir).map_err(|e| errno(&e))?;
    let mut batch = Vec::with_capacity(batch_size.max(1));
    let mut count = 0usize;
    for item in rd {
        if cancelled() {
            return Err(ListError::Cancelled);
        }
        let Ok(item) = item else { continue };
        let name = item.file_name();
        let Some(name) = name.to_str() else {
            // A name that is not UTF-8 still lists (lossy), rather than vanishing.
            let lossy = name.to_string_lossy().into_owned();
            if let Ok(md) = item.metadata() {
                batch.push(entry_of(&lossy, &md));
                count += 1;
            }
            continue;
        };
        // `DirEntry::metadata` follows a link; `symlink_metadata` keeps it a link.
        let md = match std::fs::symlink_metadata(item.path()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        batch.push(entry_of(name, &md));
        count += 1;
        if batch.len() >= batch_size.max(1) {
            on_batch(std::mem::take(&mut batch));
            if cancelled() {
                return Err(ListError::Cancelled);
            }
        }
    }
    if !batch.is_empty() {
        on_batch(batch);
    }
    Ok(count)
}

/// One name inside `dir`, for a watcher's incremental update.
pub fn stat_entry(dir: &Path, name: &str) -> Result<Option<Entry>, ListError> {
    let p = dir.join(name);
    match std::fs::symlink_metadata(&p) {
        Ok(md) => Ok(Some(entry_of(name, &md))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(errno(&e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_dot_folder_is_hidden_not_a_system_file() {
        let d = std::env::temp_dir().join(format!("rf-mac-dot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join("visible.txt"), b"x").unwrap();
        let mut out = Vec::new();
        list_dir(&d, 16, &|| false, &mut |b| out.extend(b)).unwrap();
        let git = out.iter().find(|e| e.name.as_ref() == ".git").expect(".git is listed");
        assert!(git.is_hidden(), "a dot folder is hidden");
        assert!(!git.is_system(), "but it is not a protected system file");
        let plain = out.iter().find(|e| e.name.as_ref() == "visible.txt").unwrap();
        assert!(!plain.is_hidden());
        let _ = std::fs::remove_dir_all(&d);
    }
}
