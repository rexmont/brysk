//! Drag and drop helpers, the macOS half of `rf-win::dnd`.
//!
//! GPUI hands drops to the app as a list of paths on macOS too, so the transfer rules are the
//! same as the Windows ones and only the OLE drag-out is absent (GPUI's own drag source does
//! that job).

use std::path::{Path, PathBuf};

use crate::fileops::Transfer;

pub type DROPEFFECT = u32;
pub const DROPEFFECT_NONE: DROPEFFECT = 0;
pub const DROPEFFECT_COPY: DROPEFFECT = 1;
pub const DROPEFFECT_MOVE: DROPEFFECT = 2;
pub const DROPEFFECT_LINK: DROPEFFECT = 4;

/// Starts a native drag of `items` (Windows: OLE DoDragDrop). GPUI's drag source is what the
/// file list uses on macOS; nothing to start from here.
pub fn drag_out(_owner: isize, _ui_thread: u32, _items: &[PathBuf]) -> anyhow::Result<DROPEFFECT> {
    Ok(DROPEFFECT_NONE)
}

pub fn drag_out_copy_only(_owner: isize, _ui_thread: u32, _items: &[PathBuf]) -> anyhow::Result<DROPEFFECT> {
    Ok(DROPEFFECT_NONE)
}

/// An external drop onto `dest_dir`: Ctrl copies, Shift moves, otherwise the Windows rule
/// (same volume moves, another volume copies). macOS has no drive letters, so "same volume"
/// is the same mount point.
pub fn default_transfer(sources: &[PathBuf], dest_dir: &Path, ctrl: bool, shift: bool) -> Transfer {
    if ctrl {
        return Transfer::Copy;
    }
    if shift {
        return Transfer::Move;
    }
    let mount = |p: &Path| crate::drives::mount_of(p).map(|d| d.root);
    match (sources.first().and_then(|s| mount(s)), mount(dest_dir)) {
        (Some(a), Some(b)) if a == b => Transfer::Move,
        _ => Transfer::Copy,
    }
}

/// A drop that makes no sense: into itself or a folder below it, or into the folder it is
/// already in.
pub fn is_invalid_drop(sources: &[PathBuf], dest_dir: &Path) -> bool {
    sources.iter().any(|s| dest_dir == s || dest_dir.starts_with(s)) || sources.iter().all(|s| s.parent() == Some(dest_dir))
}