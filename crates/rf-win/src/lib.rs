//! rf-win: Windows platform layer (fast enumeration, Shell COM, icons/thumbnails, file ops, watcher).
//!
//! Every function here may block (disk, network, shell extensions). Never call them on the UI thread.

#[cfg(windows)]
pub mod accent;
#[cfg(windows)]
pub mod activate;
#[cfg(windows)]
pub mod appvolume;
#[cfg(windows)]
pub mod apppkg;
#[cfg(windows)]
pub mod apps;
#[cfg(windows)]
pub mod datefmt;
#[cfg(windows)]
pub mod share;
#[cfg(windows)]
pub mod apps_direct;
#[cfg(windows)]
pub mod bench;
#[cfg(windows)]
pub mod cert;
#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
pub mod vfiles;
#[cfg(windows)]
pub mod complete;
#[cfg(windows)]
pub mod ctxmenu;
#[cfg(windows)]
pub mod default_fm;
#[cfg(windows)]
pub mod desktop;
#[cfg(windows)]
pub mod design;
#[cfg(windows)]
pub mod dialog_reg;
#[cfg(windows)]
pub mod dialogs;
#[cfg(windows)]
pub mod diskusage;
#[cfg(windows)]
pub mod elf;
#[cfg(windows)]
pub mod dnd;
#[cfg(windows)]
pub mod dock;
#[cfg(windows)]
pub mod docs;
#[cfg(windows)]
pub mod drives;
#[cfg(windows)]
pub mod encoding;
#[cfg(windows)]
pub mod engine_guard;
#[cfg(windows)]
pub mod fastcopy;
#[cfg(windows)]
pub mod fastdelete;
#[cfg(windows)]
pub mod fileops;
#[cfg(windows)]
pub mod foldersize;
#[cfg(windows)]
pub mod sysparams;
#[cfg(windows)]
pub mod git;
#[cfg(windows)]
pub mod glass;
#[cfg(windows)]
pub mod hotkey;
#[cfg(windows)]
pub mod hash;
#[cfg(windows)]
pub mod indexer;
#[cfg(windows)]
pub mod input;
#[cfg(windows)]
pub mod inputlang;
#[cfg(windows)]
pub mod known;
#[cfg(windows)]
pub mod libraries;
#[cfg(windows)]
pub mod ftp;
#[cfg(windows)]
pub mod keys;
#[cfg(windows)]
pub mod look;
#[cfg(windows)]
pub mod listing;
#[cfg(windows)]
pub mod localized;
#[cfg(windows)]
pub mod nameindex;
#[cfg(windows)]
pub mod organize;
#[cfg(windows)]
pub mod osver;
#[cfg(windows)]
pub mod openrules;
#[cfg(windows)]
pub mod ownfolder;
#[cfg(windows)]
pub mod taskbar_hide;
#[cfg(windows)]
pub mod taskbar_progress;
#[cfg(windows)]
pub mod usn;
#[cfg(windows)]
pub mod network;
#[cfg(windows)]
pub mod pe;
#[cfg(windows)]
pub mod security;
#[cfg(windows)]
pub mod signature;
#[cfg(windows)]
pub mod compat;
#[cfg(windows)]
pub mod folder_custom;
#[cfg(windows)]
pub mod fileactions;
#[cfg(windows)]
pub mod fileinfo;
#[cfg(windows)]
pub mod journal;
#[cfg(windows)]
pub mod editors;
#[cfg(windows)]
pub mod jumplist;
#[cfg(windows)]
pub mod jumplists;
#[cfg(windows)]
pub mod wsearch;
#[cfg(windows)]
pub mod preview_host;
#[cfg(windows)]
pub mod props;
#[cfg(windows)]
pub mod propcols;
#[cfg(windows)]
pub mod recycle;
#[cfg(windows)]
pub mod recycle_settings;
#[cfg(windows)]
pub mod search;
#[cfg(windows)]
pub mod runcmd;
#[cfg(windows)]
pub mod runfind;
#[cfg(windows)]
pub mod settingscat;
#[cfg(windows)]
pub mod shell;
#[cfg(windows)]
pub mod shellfolder;
#[cfg(windows)]
pub mod sysstatus;
#[cfg(windows)]
pub mod shortcut;
#[cfg(windows)]
pub mod startup;
#[cfg(windows)]
pub mod single_instance;
#[cfg(windows)]
pub mod snip;
#[cfg(windows)]
pub mod handover;
#[cfg(windows)]
pub mod shell_window;
#[cfg(windows)]
pub mod shellhost;
#[cfg(windows)]
pub mod sta;
#[cfg(windows)]
pub mod stayup;
#[cfg(windows)]
pub mod switcher;
#[cfg(windows)]
pub mod removal;
#[cfg(windows)]
pub mod pane_picture;
#[cfg(windows)]
pub mod rotate;
#[cfg(windows)]
pub mod thumbs;
#[cfg(windows)]
pub mod tray;
#[cfg(windows)]
pub mod tray_wire;
#[cfg(windows)]
pub mod trayhost;
#[cfg(windows)]
pub mod watch;
#[cfg(windows)]
pub mod weather;
// the weather from Windows' location service instead of a typed city (see the module)
#[cfg(windows)]
pub mod weather_location;
#[cfg(windows)]
pub mod window;
#[cfg(windows)]
pub mod winkey;
#[cfg(windows)]
pub mod volume;
#[cfg(windows)]
pub mod tasklist;
// (not called `notify`: that is the name of a crate this one uses)
#[cfg(windows)]
pub mod toasts;
#[cfg(windows)]
pub mod tree_watch;
#[cfg(windows)]
pub mod wide;
#[cfg(windows)]
pub mod workfit;

#[cfg(windows)]
/// Held by tests that measure short times and by tests that load every core (the glass tests draw with the software renderer, which
/// uses them all), so the two kinds never run at the same moment: with both in one parallel run the keyboard hook's "under one
/// millisecond" test failed in half of the runs.
#[cfg(test)]
pub(crate) static TEST_QUIET: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(windows)]
pub use drives::{Drive, DriveKind, list_drives};
#[cfg(windows)]
pub use known::{KnownFolder, known_folders};
#[cfg(windows)]
pub use listing::{ListError, list_dir, stat_entry};

/// On macOS this crate IS the platform layer: everything `rf-app` calls comes from `rf-mac`
/// through this re-export, under the same module paths.
#[cfg(not(windows))]
pub use rf_mac::*;

/// Milliseconds since this process was created (start-up timing).
#[cfg(windows)]
pub fn process_age_ms() -> u64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut created, mut a, mut b, mut c) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        if GetProcessTimes(GetCurrentProcess(), &mut created, &mut a, &mut b, &mut c).is_err() {
            return 0;
        }
        let now = GetSystemTimePreciseAsFileTime();
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        t(now).saturating_sub(t(created)) / 10_000
    }
}
