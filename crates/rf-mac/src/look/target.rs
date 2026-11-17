//! Finder's selection, the macOS half of `rf-win::look::target`.
//!
//! Finding what the Finder has selected needs Apple Events (and the user's permission); until
//! that milestone DirectLook shows what it is given on the command line, and `selected_path`
//! says "nothing".

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Explorer,
    Desktop,
    Dialog,
    Everything,
    DirectoryOpus,
    None,
}

pub fn classify(_class: &str, _title: &str, _has_shell_view: bool, _has_dui_view: bool) -> TargetKind {
    TargetKind::None
}

pub fn foreground_target() -> (TargetKind, isize) {
    (TargetKind::None, 0)
}

pub fn is_typing(_hwnd: isize) -> bool {
    false
}

pub fn selected_path(_kind: TargetKind, _hwnd: isize) -> Option<PathBuf> {
    None
}