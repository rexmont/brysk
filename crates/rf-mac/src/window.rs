//! Window helpers GPUI does not expose, the macOS half of `rf-win::window`.
//!
//! GPUI owns its `NSWindow`s; the interface only needs these for the Windows shell features
//! (compact overlay above other apps, owning popups, foreground checks). On macOS the window
//! is an `NSWindow` GPUI controls, and the handle the interface passes (`crate::HWND`) is a
//! placeholder because it comes from a Win32 raw handle. Everything here is a no-op that
//! reports "unknown", so the features that depend on them stay off; the ones the UI can do
//! itself (always-on-top) will move to GPUI's own window options in a later milestone.

use crate::HWND;

pub fn set_topmost(_hwnd: HWND, _on: bool) -> anyhow::Result<()> {
    Ok(())
}

pub fn is_topmost(_hwnd: HWND) -> bool {
    false
}

/// macOS always activates an app whose window was clicked; the shell helpers that ask are
/// only used to decide whether a popup may take focus, and taking it is harmless here.
pub fn app_in_front() -> bool {
    true
}

pub fn rect(_hwnd: HWND) -> Option<([i32; 4], bool)> {
    None
}

pub fn place(_hwnd: HWND, _r: [i32; 4]) {}

pub fn maximize(_hwnd: HWND) {}

pub fn set_min_size(_hwnd: HWND, _size: Option<(u16, u16)>) {}

pub fn set_app_id(_id: &str) {}

pub fn set_dark_mode(_hwnd: HWND, _dark: bool) {}

pub fn set_small_corners(_hwnd: HWND) {}

pub fn set_round_corners(_hwnd: HWND) {}

pub fn set_owner(_hwnd: HWND, _owner: HWND) {}

pub fn raise_above_owner(_hwnd: HWND) {}

pub fn enable(_hwnd: HWND) {}

pub fn hand_foreground(_owner: HWND) {}

pub fn bring_to_front(_hwnd: HWND) {}

pub fn bring_to_front_quietly(_hwnd: HWND) {}

pub fn is_foreground(_hwnd: HWND) -> bool {
    false
}

pub fn screen_color(_hwnd: HWND, _x: i32, _y0: i32, _y1: i32) -> Option<[u8; 3]> {
    None
}

pub fn describe(_hwnd: HWND) -> String {
    String::new()
}

pub fn owner_of(_hwnd: HWND) -> HWND {
    0
}

pub fn is_above(_a: HWND, _b: HWND) -> Option<bool> {
    None
}

pub fn exists(_hwnd: HWND) -> bool {
    false
}

pub fn foreground() -> HWND {
    0
}
/// Developer aid: prints every NSWindow (frame, visibility, alpha, class) to stderr.
pub fn dump_windows() {
    dispatch2::DispatchQueue::main().exec_async(|| {
        use objc2::runtime::AnyObject;
        let Some(mtm) = objc2_foundation::MainThreadMarker::new() else { return };
        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        let windows = app.windows();
        eprintln!("windows: {}", windows.len());
        for w in windows.iter() {
            let frame = w.frame();
            eprintln!(
                "  title={:?} frame=({:.0},{:.0},{:.0},{:.0}) visible={} alpha={:.2} level={}",
                
                w.title().to_string(),
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
                w.isVisible(),
                w.alphaValue(),
                w.level(),
            );
            let _: &AnyObject = &w;
        }
    });
}
