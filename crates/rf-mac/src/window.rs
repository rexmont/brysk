//! Window helpers GPUI does not expose, the macOS half of `rf-win::window`.
//!
//! GPUI owns its `NSWindow`s; the interface passes the address of the window's `NSView`
//! (`raw_window_handle::AppKitWindowHandle`), and these functions reach the owning window
//! through `-[NSView window]`. Coordinates are physical pixels with a top-left origin, the
//! convention the rest of the app uses: the AppKit frame (points, bottom-left origin) is
//! converted with the window's backing scale and the primary screen's height.

use std::sync::{Mutex, OnceLock};

use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSFloatingWindowLevel, NSNormalWindowLevel, NSScreen, NSView, NSWindow};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

use crate::HWND;

fn window(hwnd: HWND) -> Option<Retained<NSWindow>> {
    if hwnd == 0 {
        return None;
    }
    // SAFETY: the application passes the pointer of one of its own NSViews, which outlives
    // this call (GPUI owns it).
    let view = unsafe { &*(hwnd as *const NSView) };
    view.window()
}

fn primary_height() -> Option<f64> {
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    screens.iter().next().map(|s| s.frame().size.height)
}

fn scale(window: &NSWindow) -> f64 {
    window.backingScaleFactor()
}

fn is_maximized(window: &NSWindow) -> bool {
    let (Some(screen), frame) = (window.screen(), window.frame()) else { return false };
    let visible = screen.visibleFrame();
    (frame.origin.x - visible.origin.x).abs() < 2.
        && (frame.origin.y - visible.origin.y).abs() < 2.
        && (frame.size.width - visible.size.width).abs() < 2.
        && (frame.size.height - visible.size.height).abs() < 2.
}

/// The frame as `[left, top, right, bottom]` physical pixels (top-left origin), and whether
/// the window fills the screen's visible frame.
pub fn rect(hwnd: HWND) -> Option<([i32; 4], bool)> {
    let w = window(hwnd)?;
    let frame = w.frame();
    let s = scale(&w);
    let ph = primary_height()?;
    let l = (frame.origin.x * s).round() as i32;
    let t = ((ph - frame.origin.y - frame.size.height) * s).round() as i32;
    let r = ((frame.origin.x + frame.size.width) * s).round() as i32;
    let b = ((ph - frame.origin.y) * s).round() as i32;
    Some(([l, t, r, b], is_maximized(&w)))
}

pub fn place(hwnd: HWND, r: [i32; 4]) {
    let Some(w) = window(hwnd) else { return };
    let s = scale(&w);
    let Some(ph) = primary_height() else { return };
    let width = (r[2] - r[0]) as f64 / s;
    let height = (r[3] - r[1]) as f64 / s;
    let frame = NSRect::new(NSPoint::new(r[0] as f64 / s, ph - r[3] as f64 / s), NSSize::new(width, height));
    w.setFrame_display(frame, true);
}

/// Fills the screen's visible frame (the menu bar and Dock stay clear).
pub fn maximize(hwnd: HWND) {
    let Some(w) = window(hwnd) else { return };
    let Some(screen) = w.screen() else { return };
    w.setFrame_display(screen.visibleFrame(), true);
}

pub fn set_min_size(hwnd: HWND, size: Option<(u16, u16)>) {
    static SAVED: OnceLock<Mutex<Vec<(HWND, NSSize)>>> = OnceLock::new();
    let saved = SAVED.get_or_init(|| Mutex::new(Vec::new()));
    let Some(w) = window(hwnd) else { return };
    let s = scale(&w);
    match size {
        Some((width, height)) => {
            let mut saved = saved.lock().unwrap();
            if !saved.iter().any(|(h, _)| *h == hwnd) {
                saved.push((hwnd, w.minSize()));
            }
            w.setMinSize(NSSize::new(width as f64 / s, height as f64 / s));
        }
        None => {
            let old = {
                let mut saved = saved.lock().unwrap();
                saved.iter().position(|(h, _)| *h == hwnd).map(|i| saved.remove(i).1)
            };
            w.setMinSize(old.unwrap_or(NSSize::new(0., 0.)));
        }
    }
}

pub fn set_topmost(hwnd: HWND, on: bool) -> anyhow::Result<()> {
    let Some(w) = window(hwnd) else { return Ok(()) };
    w.setLevel(if on { NSFloatingWindowLevel } else { NSNormalWindowLevel });
    Ok(())
}

pub fn is_topmost(hwnd: HWND) -> bool {
    window(hwnd).map(|w| w.level() >= NSFloatingWindowLevel).unwrap_or(false)
}

/// macOS always activates an app whose window was clicked; the shell helpers that ask are
/// only used to decide whether a popup may take focus, and taking it is harmless here.
pub fn app_in_front() -> bool {
    MainThreadMarker::new().map(|mtm| NSApplication::sharedApplication(mtm).isActive()).unwrap_or(false)
}

pub fn set_app_id(_id: &str) {}

pub fn set_dark_mode(_hwnd: HWND, _dark: bool) {}

pub fn set_small_corners(_hwnd: HWND) {}

pub fn set_round_corners(_hwnd: HWND) {}

pub fn set_owner(_hwnd: HWND, _owner: HWND) {}

pub fn raise_above_owner(_hwnd: HWND) {}

pub fn enable(_hwnd: HWND) {}

pub fn hand_foreground(_owner: HWND) {}

pub fn bring_to_front(hwnd: HWND) {
    if let Some(w) = window(hwnd) {
        w.makeKeyAndOrderFront(None);
    }
}

pub fn bring_to_front_quietly(hwnd: HWND) {
    // `orderFrontRegardless` is the one ordering that also works for a window of an app that
    // is not active (DirectLook's preview over Brysk): it comes forward without taking the
    // keyboard, which stays in the file list so the arrow keys keep stepping the selection.
    if let Some(w) = window(hwnd) {
        w.orderFrontRegardless();
    }
}

pub fn is_foreground(_hwnd: HWND) -> bool {
    app_in_front()
}

pub fn screen_color(_hwnd: HWND, _x: i32, _y0: i32, _y1: i32) -> Option<[u8; 3]> {
    None
}

pub fn describe(hwnd: HWND) -> String {
    window(hwnd).map(|w| w.title().to_string()).unwrap_or_default()
}

pub fn owner_of(_hwnd: HWND) -> HWND {
    0
}

pub fn is_above(_a: HWND, _b: HWND) -> Option<bool> {
    None
}

pub fn exists(hwnd: HWND) -> bool {
    window(hwnd).is_some()
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
            let lights = [("close", objc2_app_kit::NSWindowButton::CloseButton), ("min", objc2_app_kit::NSWindowButton::MiniaturizeButton), ("zoom", objc2_app_kit::NSWindowButton::ZoomButton)]
                .into_iter()
                .filter_map(|(name, which)| w.standardWindowButton(which).map(|b| (name, b.frame())))
                .map(|(name, f)| format!("{name}=({:.1},{:.1},{:.1},{:.1})", f.origin.x, f.origin.y, f.size.width, f.size.height))
                .collect::<Vec<_>>()
                .join(" ");
            eprintln!(
                "  title={:?} py={:.0} frame=({:.0},{:.0},{:.0},{:.0}) visible={} alpha={:.2} level={} {lights}",
                w.title().to_string(),
                frame.size.height,
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