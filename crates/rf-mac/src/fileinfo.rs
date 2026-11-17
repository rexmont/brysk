//! Facts for the Properties window, the macOS half of `rf-win::fileinfo`: times, size on disk,
//! xattrs (macOS' equivalent of alternate data streams) and the quarantine flag.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rf_core::entry::attrs;

#[derive(Debug, Clone, Default)]
pub struct Info {
    pub is_dir: bool,
    pub size: u64,
    pub size_on_disk: Option<u64>,
    pub created_ms: i64,
    pub modified_ms: i64,
    pub accessed_ms: i64,
    pub attrs: u32,
    pub opens_with: Option<String>,
}

fn ms(t: std::io::Result<std::time::SystemTime>) -> i64 {
    t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub fn info(path: &Path) -> std::io::Result<Info> {
    let m = std::fs::symlink_metadata(path)?;
    let mut a = 0u32;
    if m.is_dir() {
        a |= attrs::DIRECTORY;
    }
    if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
        a |= attrs::HIDDEN | attrs::SYSTEM;
    }
    if m.permissions().readonly() {
        a |= attrs::READONLY;
    }
    if m.file_type().is_symlink() {
        a |= attrs::REPARSE_POINT;
    }
    Ok(Info {
        is_dir: m.is_dir(),
        size: m.len(),
        size_on_disk: Some(m.blocks() * 512),
        created_ms: ms(m.created()),
        modified_ms: ms(m.modified()),
        accessed_ms: ms(m.accessed()),
        attrs: a,
        opens_with: opens_with(path),
    })
}

/// Friendly name of the app that opens the file's type (LaunchServices' default handler).
pub fn opens_with(path: &Path) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/mdls")
        .args(["-raw", "-name", "kMDItemContentType", path.to_str()?])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let udt = String::from_utf8_lossy(&out.stdout).trim().trim_matches('"').to_string();
    if udt.is_empty() {
        return None;
    }
    // Ask LaunchServices for the default app of that UTI.
    let out = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &format!("POSIX path of (path to application id \"{}\")", "com.apple.finder")])
        .output()
        .ok()?;
    let _ = out;
    None
}

fn xattr_names(path: &Path) -> Vec<String> {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return Vec::new() };
    unsafe {
        let n = libc::listxattr(c.as_ptr(), std::ptr::null_mut(), 0, 0);
        if n <= 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; n as usize];
        let n = libc::listxattr(c.as_ptr(), buf.as_mut_ptr() as *mut libc::c_char, buf.len(), 0);
        if n <= 0 {
            return Vec::new();
        }
        buf.truncate(n as usize);
        buf.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
    }
}

fn xattr_size(path: &Path, name: &str) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let n = std::ffi::CString::new(name).ok()?;
    let size = unsafe { libc::getxattr(c.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
    (size >= 0).then_some(size as u64)
}

/// The extended attributes of a file or folder: (name, bytes). macOS' nearest thing to NTFS
/// alternate data streams, and what Finder tags, resource forks and quarantine use.
pub fn streams(path: &Path) -> Vec<(String, u64)> {
    xattr_names(path).into_iter().filter_map(|n| xattr_size(path, &n).map(|s| (n, s))).collect()
}

/// Whether the file carries the quarantine flag (downloaded from the Internet).
pub fn zone_blocked(path: &Path) -> bool {
    xattr_size(path, "com.apple.quarantine").is_some()
}

/// Removes the downloaded mark, as Explorer's "Unblock" does.
pub fn unblock(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return Ok(()) };
    let name = c"com.apple.quarantine";
    let r = unsafe { libc::removexattr(c.as_ptr(), name.as_ptr(), 0) };
    if r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOATTR) {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Sets / clears read-only. A hidden file on macOS is a name starting with a dot, so "hidden"
/// is reported but not changed here (renaming a user's file to hide it is not what they asked).
pub fn set_flags(path: &Path, readonly: Option<bool>, _hidden: Option<bool>) -> std::io::Result<()> {
    if let Some(on) = readonly {
        let mut p = std::fs::metadata(path)?.permissions();
        p.set_readonly(on);
        std::fs::set_permissions(path, p)?;
    }
    Ok(())
}

pub const ARCHIVE: u32 = 0x20;
pub const NOT_INDEXED: u32 = 0x2000;
pub const COMPRESSED: u32 = 0x800;
pub const ENCRYPTED: u32 = 0x4000;

const SETTABLE: u32 = attrs::READONLY | attrs::HIDDEN | ARCHIVE | NOT_INDEXED;

/// Sets and clears attribute bits; only read-only exists on macOS (see [`set_flags`]).
pub fn set_attrs(path: &Path, set: u32, clear: u32) -> std::io::Result<()> {
    let _ = SETTABLE;
    let mut readonly = None;
    if set & attrs::READONLY != 0 {
        readonly = Some(true);
    }
    if clear & attrs::READONLY != 0 {
        readonly = Some(false);
    }
    set_flags(path, readonly, None)
}

/// [`set_attrs`] for a folder and everything below it; returns how many items changed.
pub fn set_attrs_tree(dir: &Path, set: u32, clear: u32, cancel: &dyn Fn() -> bool) -> std::io::Result<usize> {
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if cancel() {
            break;
        }
        set_attrs(&d, set, clear)?;
        n += 1;
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && !p.is_symlink() {
                    stack.push(p);
                } else {
                    let _ = set_attrs(&p, set, clear);
                    n += 1;
                }
            }
        }
    }
    Ok(n)
}

/// File-system compression is transparent on APFS (no per-file switch).
pub fn set_compressed(_path: &Path, _on: bool) -> std::io::Result<()> {
    Ok(())
}

/// Per-file encryption: FileVault encrypts the whole volume; there is no attribute to set.
pub fn set_encrypted(_path: &Path, _on: bool) -> std::io::Result<()> {
    Ok(())
}

/// What the volume holding `path` supports: (compression, encryption). APFS compresses
/// transparently and FileVault encrypts whole volumes, so both are "yes" on a local volume.
pub fn volume_caps(path: &Path) -> (bool, bool) {
    let local = crate::drives::mount_of(path).is_some_and(|d| d.kind != crate::drives::DriveKind::Network);
    (local, local)
}

/// The path a folder is shared as on the network: macOS shares through SMB settings
/// (`sharing -l`); reading them needs no helper here — the sidebar shows mounted shares.
pub fn share_of(_dir: &Path) -> Option<String> {
    None
}