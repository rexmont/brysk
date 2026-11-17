//! Mounted volumes, the macOS counterpart of `rf-win::drives`.
//!
//! There are no drive letters: a volume is a mount point (`/`, `/Volumes/Data`, a mounted SMB
//! share). The interface still shows a `letter` in the display name it builds from `Drive`, so
//! each volume gets one by position (A, B, C…) — the theme's Explorer look keeps its
//! "Local Disk (C:)" shape, but the letter is only a label here, never a path.

use std::ffi::CStr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
    RamDisk,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Drive {
    /// The mount point: `/`, `/Volumes/Data`, …
    pub root: PathBuf,
    /// A display-only letter (see the module note).
    pub letter: char,
    pub label: String,
    pub kind: DriveKind,
    pub total: u64,
    pub free: u64,
    pub fs: String,
}

impl Drive {
    pub fn display_name(&self) -> String {
        let label = if self.label.is_empty() {
            match self.kind {
                DriveKind::Removable => "USB Sürücü",
                DriveKind::Network => "Ağ Sürücüsü",
                DriveKind::Optical => "DVD Sürücü",
                _ => "Yerel Disk",
            }
            .to_string()
        } else {
            self.label.clone()
        };
        if self.root == Path::new("/") {
            format!("{label} (Macintosh HD)")
        } else {
            format!("{label} ({})", self.root.display())
        }
    }

    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { 1.0 - self.free as f32 / self.total as f32 }
    }
}

// ---------------------------------------------------------------- drive masks

/// The Windows settings keep hidden drives as a bit mask of letters. macOS has no letters and
/// no such setting: the mask is always empty, so nothing is hidden and nothing is written.
pub fn letter_bit(letter: char) -> Option<u32> {
    let l = letter.to_ascii_uppercase();
    l.is_ascii_uppercase().then(|| 1u32 << (l as u8 - b'A'))
}

pub fn mask_of_letters<S: AsRef<str>>(letters: &[S]) -> u32 {
    letters.iter().filter_map(|s| s.as_ref().trim().chars().next().and_then(letter_bit)).fold(0, |m, b| m | b)
}

pub fn letters_of_mask(mask: u32) -> Vec<char> {
    (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| (b'A' + i) as char).collect()
}

pub fn set_hidden(_mask: u32) {}
pub fn hidden() -> u32 {
    0
}
pub fn policy_hidden() -> u32 {
    0
}
pub fn drive_mask() -> u32 {
    0
}

// ---------------------------------------------------------------- enumeration

fn kind_of(fs: &str, flags: u32) -> DriveKind {
    const MNT_LOCAL: u32 = 0x0000_1000;
    match fs {
        "smbfs" | "nfs" | "afpfs" | "webdav" | "ftp" => DriveKind::Network,
        "cd9660" | "udf" => DriveKind::Optical,
        "devfs" => DriveKind::RamDisk,
        _ if flags & MNT_LOCAL == 0 => DriveKind::Network,
        "apfs" | "hfs" | "exfat" | "msdos" | "ntfs" => DriveKind::Fixed,
        _ => DriveKind::Unknown,
    }
}

fn labels() -> &'static std::collections::HashMap<PathBuf, String> {
    use std::sync::OnceLock;
    static LABELS: OnceLock<std::collections::HashMap<PathBuf, String>> = OnceLock::new();
    LABELS.get_or_init(|| {
        // `diskutil` names the volume the way the Finder shows it (e.g. "Macintosh HD - Data");
        // it is a local, fast call, and only for the names.
        let mut out = std::collections::HashMap::new();
        if let Ok(o) = std::process::Command::new("/usr/sbin/diskutil").args(["info", "-plist", "/"]).output() {
            if let Ok(text) = String::from_utf8(o.stdout) {
                // Keep it dependency-free: the plist has one <key>VolumeName</key><string>…</string>.
                if let Some(i) = text.find("<key>VolumeName</key>") {
                    let rest = &text[i..];
                    if let (Some(a), Some(b)) = (rest.find("<string>"), rest.find("</string>")) {
                        if b > a {
                            out.insert(PathBuf::from("/"), rest[a + 8..b].to_string());
                        }
                    }
                }
            }
        }
        out
    })
}

/// The mounted volumes, root first, then `/Volumes/*` in Finder order.
pub fn list_drives() -> Vec<Drive> {
    let mut out = Vec::new();
    let names = labels();

    // The root volume, always.
    if let Some(d) = volume_of(Path::new("/"), 'A', names.get(Path::new("/")).cloned()) {
        out.push(d);
    }
    // Everything else mounted under /Volumes (an APFS data volume, an external disk, a share).
    let mut seen: Vec<String> = out.iter().map(|d| d.fs.clone()).collect();
    let mut letter = b'B';
    if let Ok(rd) = std::fs::read_dir("/Volumes") {
        let mut vols: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        vols.sort_by_key(|p| p.file_name().map(|n| n.to_ascii_lowercase()));
        for p in vols {
            if letter > b'Z' {
                break;
            }
            if let Some(d) = volume_of(&p, letter as char, p.file_name().map(|n| n.to_string_lossy().into_owned())) {
                // a boot helper volume is listed by the Finder too; keep it, but skip duplicates
                if seen.contains(&d.fs) && d.root != Path::new("/") {
                    continue;
                }
                seen.push(d.fs.clone());
                out.push(d);
                letter += 1;
            }
        }
    }
    out
}

fn volume_of(root: &Path, letter: char, fallback_label: Option<String>) -> Option<Drive> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(root.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let fs = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) }.to_string_lossy().into_owned();
    let mut total = 0u64;
    let mut free = 0u64;
    let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
    let c2 = std::ffi::CString::new(root.as_os_str().as_bytes()).ok()?;
    if unsafe { libc::statvfs(c2.as_ptr(), &mut vfs) } == 0 {
        total = vfs.f_blocks as u64 * vfs.f_frsize as u64;
        free = vfs.f_bavail as u64 * vfs.f_frsize as u64;
    }
    let label = fallback_label.or_else(|| Some(unsafe { CStr::from_ptr(st.f_mntonname.as_ptr()) }.to_string_lossy().into_owned()))
        .unwrap_or_default();
    Some(Drive { root: root.to_path_buf(), letter, label, kind: kind_of(&fs, st.f_flags), total, free, fs })
}

pub fn visible_drives() -> Vec<Drive> {
    list_drives()
}

/// Whether `path` is on removable media (an external disk or a mounted image).
pub fn is_removable(path: &Path) -> bool {
    mount_of(path).is_some_and(|d| d.kind == DriveKind::Removable)
}

/// A network share is slower to read: the folder listing follows the same rule as on Windows.
pub fn slow_to_read(path: &Path) -> bool {
    mount_of(path).is_some_and(|d| d.kind == DriveKind::Network)
}

pub fn seek_penalty(path: &Path) -> Option<bool> {
    // No cheap per-volume rotation query on macOS (an APFS fusion volume is both): unknown.
    let _ = path;
    None
}

/// The volume a path lives on.
pub fn mount_of(path: &Path) -> Option<Drive> {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut best: Option<Drive> = None;
    for d in list_drives() {
        if canon.starts_with(&d.root) && best.as_ref().is_none_or(|b| b.root.as_os_str().len() < d.root.as_os_str().len()) {
            best = Some(d);
        }
    }
    best
}