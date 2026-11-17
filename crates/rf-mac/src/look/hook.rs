//! The system-wide key hook (Space over the Finder), the macOS half of `rf-win::look::hook`.
//!
//! macOS delivers global keys through an event tap, which needs Accessibility permission; that
//! is part of the Finder-integration milestone. The handle is kept so the shared lifecycle
//! code (`HookHandle::stop`) compiles unchanged.

use super::keys::LookAction;
use super::target::TargetKind;

#[derive(Debug, Clone)]
pub struct LookEvent {
    pub action: LookAction,
    pub target: TargetKind,
    pub hwnd: isize,
}

pub fn set_own_windows(_hwnds: Vec<isize>) {}

pub struct HookHandle;

impl HookHandle {
    pub fn stop(self) {}
}

/// Installs the hook; nothing is observed on macOS yet.
pub fn start(_on_event: impl Fn(LookEvent) + Send + 'static) -> HookHandle {
    HookHandle
}