//! Mounted volumes, the macOS counterpart of `rf-win::drives`.
//!
//! Volumes can be mounted anywhere, including Mountain Duck's application support folder.
//! The macOS interface uses volume names and mount paths; letters only serve the shared API.

use std::ffi::CStr;
use std::os::unix::ffi::OsStrExt;
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
    /// A compatibility letter, repeated after Z; never used for macOS navigation.
    pub letter: char,
    pub label: String,
    pub kind: DriveKind,
    pub total: u64,
    pub free: u64,
    pub fs: String,
}

impl Drive {
    pub fn display_name(&self) -> String {
        if !self.label.is_empty() {
            return self.label.clone();
        }
        if self.root == Path::new("/") {
            return "Macintosh HD".into();
        }
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Disk".into())
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
    mount_mask(&drives_from_mounts(&mount_snapshot()))
}

fn mount_mask(drives: &[Drive]) -> u32 {
    let mut h = 0x811C_9DC5u32;
    for drive in drives {
        for part in [drive.root.as_os_str().as_bytes(), drive.fs.as_bytes()] {
            for &b in part {
                h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
            }
            h = h.wrapping_mul(0x0100_0193);
        }
        h = (h ^ drive.kind as u32).wrapping_mul(0x0100_0193);
    }
    h
}

// ---------------------------------------------------------------- enumeration

/// Cached kernel data only: disconnected network volumes must not stall discovery.
fn mount_snapshot() -> Vec<libc::statfs> {
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count < 0 {
        return Vec::new();
    }
    let mut mounts = vec![unsafe { std::mem::zeroed::<libc::statfs>() }; count as usize + 1];
    loop {
        let Ok(bytes) = libc::c_int::try_from(std::mem::size_of_val(mounts.as_slice())) else { return Vec::new() };
        let count = unsafe { libc::getfsstat(mounts.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
        if count < 0 {
            return Vec::new();
        }
        if (count as usize) < mounts.len() {
            mounts.truncate(count as usize);
            return mounts;
        }
        // A volume may have connected between the count and the snapshot.
        mounts.resize(mounts.len() * 2, unsafe { std::mem::zeroed() });
    }
}

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

fn drives_from_mounts(mounts: &[libc::statfs]) -> Vec<Drive> {
    let mut out = Vec::new();
    for st in mounts {
        let root = PathBuf::from(std::ffi::OsStr::from_bytes(unsafe { CStr::from_ptr(st.f_mntonname.as_ptr()) }.to_bytes()));
        let fs = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) }.to_string_lossy().into_owned();
        if root != Path::new("/") && (st.f_flags & libc::MNT_DONTBROWSE as u32 != 0 || matches!(fs.as_str(), "devfs" | "autofs")) {
            continue;
        }
        let name = root.file_name().map(|n| n.to_string_lossy()).unwrap_or("Macintosh HD".into());
        let label = name.strip_suffix(".localized").unwrap_or(&name).to_string();
        out.push(Drive {
            root,
            letter: 'A',
            label,
            kind: kind_of(&fs, st.f_flags),
            total: st.f_blocks.saturating_mul(u64::from(st.f_bsize)),
            free: st.f_bavail.saturating_mul(u64::from(st.f_bsize)),
            fs,
        });
    }
    out.sort_by_cached_key(|d| (d.root != Path::new("/"), d.kind == DriveKind::Network, d.label.to_lowercase(), d.root.clone()));
    for (i, drive) in out.iter_mut().enumerate() {
        drive.letter = (b'A' + i.min(25) as u8) as char;
    }
    out
}

/// The startup volume, then physical volumes, then network mounts, each group sorted by name.
pub fn list_drives() -> Vec<Drive> {
    let mut out = drives_from_mounts(&mount_snapshot());
    if let Some(name) = labels().get(Path::new("/")) {
        if let Some(root) = out.iter_mut().find(|d| d.root == Path::new("/")) {
            root.label = name.clone();
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
    let mount = PathBuf::from(unsafe { CStr::from_ptr(st.f_mntonname.as_ptr()) }.to_string_lossy().into_owned());
    // /Volumes/Macintosh HD may be a link to /; ordinary directories are not mounts either.
    if mount != root { return None; }
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

/// Unmount/eject the volume containing `path`. Never offer the startup volume.
pub fn eject(path: &Path) -> anyhow::Result<()> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};
    let drive = mount_of(path).ok_or_else(|| anyhow::anyhow!("Disk bulunamadı"))?;
    anyhow::ensure!(drive.root.parent() == Some(Path::new("/Volumes")), "Başlangıç diski çıkarılamaz");
    objc2::rc::autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&drive.root.to_string_lossy()));
        NSWorkspace::sharedWorkspace().unmountAndEjectDeviceAtURL_error(&url).map_err(|e| anyhow::anyhow!("{e}"))
    })
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
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &mut st) } != 0 { return None; }
    let root = PathBuf::from(unsafe { CStr::from_ptr(st.f_mntonname.as_ptr()) }.to_string_lossy().into_owned());
    let label = if root == Path::new("/") { labels().get(&root).cloned() } else { root.file_name().map(|n| n.to_string_lossy().into_owned()) };
    volume_of(&root, 'A', label)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(root: &str, fs: &str, flags: u32) -> libc::statfs {
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        for (field, text) in [(&mut st.f_mntonname[..], root), (&mut st.f_fstypename[..], fs)] {
            assert!(text.len() < field.len());
            for (slot, byte) in field.iter_mut().zip(text.bytes()) {
                *slot = byte as libc::c_char;
            }
        }
        st.f_flags = flags;
        st.f_bsize = 4096;
        st.f_blocks = 100;
        st.f_bavail = 25;
        st
    }

    #[test]
    fn custom_network_mounts_follow_physical_volumes() {
        let duck = "/Users/alex/Library/Application Support/Mountain Duck/Volumes.noindex/Google Drive.localized";
        let drives = drives_from_mounts(&[
            mount(duck, "nfs", libc::MNT_LOCAL as u32),
            mount("/Volumes/Z Backup", "exfat", libc::MNT_LOCAL as u32),
            mount("/Volumes/Archive", "smbfs", 0),
            mount("/", "apfs", libc::MNT_LOCAL as u32),
            mount("/Volumes/alpha", "apfs", libc::MNT_LOCAL as u32),
        ]);
        assert_eq!(drives.iter().map(|d| d.root.as_path()).collect::<Vec<_>>(), ["/", "/Volumes/alpha", "/Volumes/Z Backup", "/Volumes/Archive", duck].map(Path::new));
        assert_eq!(drives[4].label, "Google Drive");
        assert_eq!(drives[4].kind, DriveKind::Network);
        assert_eq!((drives[4].total, drives[4].free), (409600, 102400));
    }

    #[test]
    fn system_mounts_stay_hidden() {
        let drives = drives_from_mounts(&[
            mount("/", "apfs", libc::MNT_DONTBROWSE as u32),
            mount("/System/Volumes/Data", "apfs", libc::MNT_DONTBROWSE as u32),
            mount("/Library/Developer/CoreSimulator/Volumes/iOS", "apfs", libc::MNT_DONTBROWSE as u32),
            mount("/dev", "devfs", libc::MNT_LOCAL as u32),
            mount("/home", "autofs", 0),
        ]);
        assert_eq!(drives.len(), 1);
        assert_eq!(drives[0].root, Path::new("/"));
    }

    #[test]
    fn mount_changes_outside_volumes_refresh_the_sidebar() {
        let root = mount("/", "apfs", libc::MNT_LOCAL as u32);
        let duck = mount("/custom-mounts/Cloud", "nfs", 0);
        let before = mount_mask(&drives_from_mounts(&[root]));
        let after = mount_mask(&drives_from_mounts(&[root, duck]));
        assert_ne!(before, after);
        assert_eq!(after, mount_mask(&drives_from_mounts(&[duck, root])), "kernel enumeration order is irrelevant");
        let mut changed_space = duck;
        changed_space.f_bavail = 1;
        assert_eq!(after, mount_mask(&drives_from_mounts(&[root, changed_space])), "free space is not a mount change");
        assert_eq!(before, mount_mask(&drives_from_mounts(&[root])), "disconnect restores the original set");
    }

    #[test]
    fn macos_volumes_are_not_limited_by_windows_drive_letters() {
        let mounts: Vec<_> = (0..30).map(|i| mount(&format!("/custom-mounts/{i}"), "nfs", 0)).collect();
        assert_eq!(drives_from_mounts(&mounts).len(), 30);
    }

    #[test]
    fn cached_kernel_snapshot_includes_the_startup_volume() {
        let drives = drives_from_mounts(&mount_snapshot());
        assert_eq!(drives.first().map(|d| d.root.as_path()), Some(Path::new("/")));
    }
}
