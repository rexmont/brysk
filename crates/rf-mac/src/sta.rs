//! "Run on the shell thread", the macOS half of `rf-win::sta`.
//!
//! macOS has no COM apartments: AppKit work belongs on the main thread (and, unlike COM, is
//! not faked convincingly). The callers only use this to keep dialog/clipboard work off GPUI's
//! update, so the job runs on a thread of its own with the same timeout contract.

use std::time::Duration;

use crate::HWND;

/// Runs `f` on a thread of its own. `f` receives a placeholder owner (`0`): the AppKit panel
/// APIs used by the macOS dialogs are application-modal, not window-owned.
pub fn run(f: impl FnOnce(HWND) + Send + 'static) {
    let _ = std::thread::Builder::new().name("mac-panel".into()).spawn(move || f(0));
}

/// Runs `f` and waits for its answer, at most `limit`.
pub fn call<R: Send + 'static>(limit: Duration, f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let _ = std::thread::Builder::new().name("mac-call".into()).spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).ok()
}