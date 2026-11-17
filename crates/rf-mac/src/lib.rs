//! rf-mac: the macOS platform layer of rustyfiles.
//!
//! `rf-win` re-exports this crate on macOS (its Windows modules are cfg-gated) and is itself
//! the Windows platform layer, so `rf-app` keeps calling `rf_win::<module>` on both. The modules below mirror
//! `crates/rf-win`'s public surface; portable modules are included from that tree with
//! `#[path]` so the two platforms cannot drift, and the rest are macOS implementations or
//! typed stubs for subsystems that do not exist here (see docs/port-macos.md).
//!
//! Every function in this crate may block (disk, network, AppKit); never call them on the UI
//! thread unless the module says otherwise.

/// The opaque window handle the interface passes around. On Windows this is a real `HWND`;
/// here it is a pointer-sized id (`0` = none) and the window helpers are no-ops.
pub type HWND = isize;

pub fn process_age_ms() -> u64 {
    use std::sync::OnceLock;
    static SINCE: OnceLock<std::time::Instant> = OnceLock::new();
    SINCE.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

// ---- modules with a macOS implementation ----
pub mod accent;
pub mod clipboard;
pub mod ctxmenu;
pub mod datefmt;
pub mod dialogs;
pub mod diskusage;
pub mod dnd;
pub mod drives;
pub mod encoding;
pub mod fastcopy;
pub mod fastdelete;
pub mod fileinfo;
pub mod fileops;
pub mod foldersize;
pub mod glass;
pub mod handover;
pub mod hotkey;
pub mod indexer;
pub mod input;
pub mod known;
pub mod listing;
pub mod search;
pub mod localized;
pub mod look;
pub mod osver;
pub mod preview_host;
pub mod recycle;
pub mod shell;
pub mod single_instance;
pub mod sta;
pub mod thumbs;
pub mod tray;
pub mod window;

#[path = "../../rf-win/src/watch.rs"]
pub mod watch;
#[path = "../../rf-win/src/tree_watch.rs"]
pub mod tree_watch;

// ---- portable modules, included from the Windows tree (no copy, no drift) ----
#[path = "../../rf-win/src/hash.rs"]
pub mod hash;
#[path = "../../rf-win/src/git.rs"]
pub mod git;
#[path = "../../rf-win/src/design.rs"]
pub mod design;
#[path = "../../rf-win/src/elf.rs"]
pub mod elf;
#[path = "../../rf-win/src/openrules.rs"]
pub mod openrules;

pub use drives::{Drive, DriveKind, list_drives};
pub use known::{KnownFolder, known_folders};
pub use listing::{ListError, list_dir, stat_entry};

mod stubs;
pub use stubs::*;

/// A Windows `RECT` in physical pixels, kept for the signatures the interface shares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}