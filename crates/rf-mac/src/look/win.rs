//! DirectLook's window helpers, the macOS half of `rf-win::look::win`.
//!
//! GPUI owns the window; the placement calls that Windows needs (topmost, taskbar, owner) are
//! done through GPUI's own window options in the shared code. The work area comes from
//! CoreGraphics so the preview can be centred without AppKit main-thread access.

use crate::HWND;

/// The work area of the display under the cursor, in logical points: `[x, y, width, height]`.
pub fn work_area_at_cursor() -> [f32; 4] {
    let id = unsafe { core_graphics::display::CGMainDisplayID() };
    let b = unsafe { core_graphics::display::CGDisplayBounds(id) };
    // Leave the menu bar; the Dock may take more, which the user sees as the window's frame.
    [b.origin.x as f32, 25.0, b.size.width as f32, (b.size.height as f32 - 25.0).max(200.0)]
}

/// The work area of the display the window is on; the main display until GPUI exposes which
/// screen a window is on.
pub fn work_area_of(_hwnd: HWND) -> [f32; 4] {
    work_area_at_cursor()
}

pub fn allow_any_foreground() {}

pub fn set_topmost(hwnd: HWND, on: bool) {
    let _ = crate::window::set_topmost(hwnd, on);
}

pub fn set_rect(_hwnd: HWND, _rect: [f32; 4], _scale: f32) {}

pub fn set_show_in_taskbar(_hwnd: HWND, _show: bool) {}

pub fn process_running(exe: &str) -> bool {
    let stem = exe.trim_end_matches(".exe");
    std::process::Command::new("/usr/bin/pgrep")
        .args(["-x", stem])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Whether a point of a client area (device px) lies in the area's top `strip` rows. Pure.
pub fn in_top_strip(p: (i32, i32), width: i32, strip: i32) -> bool {
    let _ = width;
    p.1 >= 0 && p.1 < strip
}

pub fn pointer_in_top_strip(_hwnd: HWND, _strip: i32) -> bool {
    false
}