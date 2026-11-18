//! Optional Finder Dock-click redirection, ported from mishamyrt/finder-replace (MIT).
//! Copyright (c) 2026 Mikhail Khrustik. See NOTICE for the full upstream licence.
//! AX hit testing and the event tap run on a dedicated thread, never the app's UI thread.

use std::ffi::{CStr, c_void};
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::mach_port::{CFMachPort, CFMachPortInvalidate, CFMachPortRef};
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURL;
use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGRect};

const FINDER: &str = "/System/Library/CoreServices/Finder.app";
const DOCK: &[u8] = b"/System/Library/CoreServices/Dock.app/Contents/MacOS/Dock";
const DOWN: u32 = 1;
const UP: u32 = 2;
const DRAG: u32 = 6;
const MODIFIERS: u64 = 0x0002_0000 | 0x0004_0000 | 0x0008_0000 | 0x0010_0000;
const AX_BUDGET: Duration = Duration::from_millis(250);
const RETRY: Duration = Duration::from_millis(500);

static ENABLED: AtomicBool = AtomicBool::new(false);
static APP_AVAILABLE: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicU8 = AtomicU8::new(0);
static WORKER: OnceLock<std::thread::Thread> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Off,
    NeedsAccessibility,
    Active,
    TapUnavailable,
    AppUnavailable,
}

pub enum Event {
    Open,
    StatusChanged,
}

pub fn status() -> Status {
    match STATUS.load(Ordering::Relaxed) {
        1 => Status::NeedsAccessibility,
        2 => Status::Active,
        3 => Status::TapUnavailable,
        4 => Status::AppUnavailable,
        _ => Status::Off,
    }
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if let Some(thread) = WORKER.get() {
        thread.unpark();
    }
}

/// Called only by the main Brysk process. No permission prompt or tap while disabled.
pub fn start(on_event: impl Fn(Event) + Send + 'static) {
    WORKER.get_or_init(|| {
        let executable = std::env::current_exe().ok();
        std::thread::spawn(move || {
            loop {
                let available = executable.as_deref().is_some_and(std::path::Path::is_file);
                APP_AVAILABLE.store(available, Ordering::Relaxed);
                if !ENABLED.load(Ordering::Relaxed) {
                    publish(Status::Off, &on_event);
                    std::thread::park();
                } else if !available {
                    publish(Status::AppUnavailable, &on_event);
                    std::thread::park_timeout(RETRY);
                } else if !trusted() {
                    publish(Status::NeedsAccessibility, &on_event);
                    std::thread::park_timeout(RETRY);
                } else if !run_tap(executable.as_deref().unwrap(), &on_event) {
                    publish(Status::TapUnavailable, &on_event);
                    std::thread::park_timeout(RETRY);
                }
            }
        })
        .thread()
        .clone()
    });
}

fn publish(status: Status, on_event: &dyn Fn(Event)) {
    if STATUS.swap(status as u8, Ordering::Relaxed) != status as u8 {
        on_event(Event::StatusChanged);
    }
}

fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Only an explicit opt-in or the permission button calls this; checking status never prompts.
pub fn request_permission() {
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options =
        CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
    unsafe {
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
    }
}

type AxRef = *const c_void;
type EventRef = *mut c_void;
type TapCallback = unsafe extern "C" fn(*const c_void, u32, EventRef, *mut c_void) -> EventRef;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXUIElementCreateSystemWide() -> AxRef;
    fn AXUIElementGetTypeID() -> usize;
    fn AXUIElementGetPid(element: AxRef, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(element: AxRef, timeout: f32) -> i32;
    fn AXUIElementCopyElementAtPosition(element: AxRef, x: f32, y: f32, result: *mut AxRef) -> i32;
    fn AXUIElementCopyAttributeValue(
        element: AxRef,
        name: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn CGEventTapCreate(
        location: u32,
        placement: u32,
        options: u32,
        mask: u64,
        callback: TapCallback,
        context: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enabled: bool);
    fn CGEventGetFlags(event: EventRef) -> u64;
    fn CGEventGetLocation(event: EventRef) -> CGPoint;
    fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
}

struct TapContext<'a> {
    system: CFType,
    tap: CFMachPortRef,
    swallowing: bool,
    on_event: &'a dyn Fn(Event),
}

fn run_tap(executable: &std::path::Path, on_event: &dyn Fn(Event)) -> bool {
    let system = unsafe { AXUIElementCreateSystemWide() };
    if system.is_null() {
        return false;
    }
    let mut context = TapContext {
        system: unsafe { CFType::wrap_under_create_rule(system) },
        tap: ptr::null_mut(),
        swallowing: false,
        on_event,
    };
    // The core-graphics crate's tap wrapper treats None as "pass"; the raw callback is
    // needed here to return null and suppress a matched click.
    let tap = unsafe {
        CGEventTapCreate(
            1,
            0,
            0,
            (1 << DOWN) | (1 << UP) | (1 << DRAG),
            on_mouse,
            &mut context as *mut _ as *mut c_void,
        )
    };
    if tap.is_null() {
        return false;
    }
    let tap = unsafe { CFMachPort::wrap_under_create_rule(tap) };
    context.tap = tap.as_concrete_TypeRef();
    let Ok(source) = tap.create_runloop_source(0) else {
        unsafe {
            CFMachPortInvalidate(context.tap);
        }
        return false;
    };
    let runloop = CFRunLoop::get_current();
    unsafe {
        runloop.add_source(&source, kCFRunLoopCommonModes);
        CGEventTapEnable(context.tap, true);
    }
    publish(Status::Active, on_event);
    while ENABLED.load(Ordering::Relaxed) && trusted() {
        let available = executable.is_file();
        APP_AVAILABLE.store(available, Ordering::Relaxed);
        if !available {
            break;
        }
        unsafe {
            CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, RETRY, false);
        }
    }
    unsafe {
        CGEventTapEnable(context.tap, false);
        runloop.remove_source(&source, kCFRunLoopCommonModes);
        CFMachPortInvalidate(context.tap);
    }
    true
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Pass,
    Swallow,
    Open,
}

fn mouse_action(active: &mut bool, kind: u32, finder: bool) -> Action {
    match kind {
        DOWN => {
            *active = finder;
            if finder { Action::Open } else { Action::Pass }
        }
        UP => {
            if std::mem::take(active) {
                Action::Swallow
            } else {
                Action::Pass
            }
        }
        DRAG if *active => Action::Swallow,
        _ => Action::Pass,
    }
}

fn plain_click(enabled: bool, app_available: bool, flags: u64) -> bool {
    enabled && app_available && flags & MODIFIERS == 0
}

unsafe extern "C" fn on_mouse(
    _: *const c_void,
    kind: u32,
    event: EventRef,
    context: *mut c_void,
) -> EventRef {
    // The tap invokes this only on its owning run loop; context outlives the invalidated tap.
    let context = unsafe { &mut *(context as *mut TapContext<'_>) };
    if kind == 0xffff_fffe || kind == 0xffff_ffff {
        unsafe {
            CGEventTapEnable(context.tap, ENABLED.load(Ordering::Relaxed));
        }
        return event;
    }
    if event.is_null() {
        return event;
    }
    let finder = kind == DOWN
        && plain_click(
            ENABLED.load(Ordering::Relaxed),
            APP_AVAILABLE.load(Ordering::Relaxed),
            unsafe { CGEventGetFlags(event) },
        )
        && finder_at(&context.system, unsafe { CGEventGetLocation(event) });
    match mouse_action(&mut context.swallowing, kind, finder) {
        Action::Pass => event,
        Action::Swallow => ptr::null_mut(),
        Action::Open => {
            (context.on_event)(Event::Open);
            ptr::null_mut()
        }
    }
}

fn time_left(system: &CFType, deadline: Instant) -> bool {
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .as_secs_f32();
    remaining > 0.
        && unsafe { AXUIElementSetMessagingTimeout(system.as_CFTypeRef(), remaining.min(0.1)) == 0 }
}

fn attribute(system: &CFType, element: &CFType, name: &str, deadline: Instant) -> Option<CFType> {
    if !time_left(system, deadline) {
        return None;
    }
    let mut value = ptr::null();
    let error = unsafe {
        AXUIElementCopyAttributeValue(
            element.as_CFTypeRef(),
            CFString::new(name).as_concrete_TypeRef(),
            &mut value,
        )
    };
    owned_value(error, value)
}

fn owned_value(error: i32, value: CFTypeRef) -> Option<CFType> {
    if value.is_null() {
        return None;
    }
    let value = unsafe { CFType::wrap_under_create_rule(value) };
    (error == 0).then_some(value)
}

fn string_is(value: Option<CFType>, expected: &str) -> bool {
    value
        .and_then(|v| v.downcast_into::<CFString>())
        .is_some_and(|v| v == CFString::new(expected))
}

fn finder_url(value: Option<CFType>) -> bool {
    value
        .and_then(|v| v.downcast_into::<CFURL>())
        .and_then(|url| url.to_path())
        .is_some_and(|p| p == std::path::Path::new(FINDER))
}

fn element_at(system: &CFType, mut point: CGPoint, deadline: Instant) -> Option<CFType> {
    let mut retried = false;
    for _ in 0..3 {
        if !time_left(system, deadline) {
            break;
        }
        let mut element = ptr::null();
        let error = unsafe {
            AXUIElementCopyElementAtPosition(
                system.as_CFTypeRef(),
                point.x as f32,
                point.y as f32,
                &mut element,
            )
        };
        let element = owned_value(error, element);
        if error == 0 {
            return element;
        }
        if error == -25204 && !retried {
            // kAXErrorCannotComplete: a cold Dock may miss once.
            retried = true;
            continue;
        }
        if error == -25212 {
            // kAXErrorNoValue at the Dock's outer edge.
            let displays: Vec<_> = CGDisplay::active_displays()
                .unwrap_or_default()
                .into_iter()
                .map(|id| CGDisplay::new(id).bounds())
                .collect();
            let adjusted = screen_point(point, &displays);
            if adjusted.x != point.x || adjusted.y != point.y {
                point = adjusted;
                continue;
            }
        }
        break;
    }
    None
}

fn finder_at(system: &CFType, point: CGPoint) -> bool {
    let deadline = Instant::now() + AX_BUDGET;
    let Some(mut element) = element_at(system, point, deadline) else {
        return false;
    };
    let mut pid = 0;
    let mut executable = [0u8; 4096];
    if unsafe { AXUIElementGetPid(element.as_CFTypeRef(), &mut pid) } != 0
        || unsafe { proc_pidpath(pid, executable.as_mut_ptr().cast(), executable.len() as u32) }
            <= 0
        || CStr::from_bytes_until_nul(&executable)
            .ok()
            .map(CStr::to_bytes)
            != Some(DOCK)
    {
        return false;
    }
    // Recheck the live Dock item, rather than caching a rectangle that magnification changes.
    for _ in 0..6 {
        if string_is(
            attribute(system, &element, "AXRole", deadline),
            "AXDockItem",
        ) {
            return string_is(
                attribute(system, &element, "AXSubrole", deadline),
                "AXApplicationDockItem",
            ) && finder_url(attribute(system, &element, "AXURL", deadline))
                && Instant::now() < deadline;
        }
        let Some(parent) = attribute(system, &element, "AXParent", deadline) else {
            break;
        };
        if parent.type_of() != unsafe { AXUIElementGetTypeID() } {
            break;
        }
        element = parent;
    }
    false
}

fn screen_point(point: CGPoint, displays: &[CGRect]) -> CGPoint {
    let mut adjusted = point;
    for bounds in displays {
        let (x, y, w, h) = (
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        );
        if w <= 0. || h <= 0. || point.x < x || point.x > x + w || point.y < y || point.y > y + h {
            continue;
        }
        // shortcut: Dock AX stops a few points short of the edge; use live item geometry if that gap grows.
        adjusted = CGPoint::new(
            if point.x >= x + w - 1. {
                x + w - 8.
            } else {
                point.x
            },
            if point.y >= y + h - 1. {
                y + h - 8.
            } else {
                point.y
            },
        );
        if point.x < x + w && point.y < y + h {
            return adjusted;
        }
    }
    adjusted
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::url::kCFURLPOSIXPathStyle;

    #[test]
    fn only_plain_clicks_are_candidates() {
        assert!(plain_click(true, true, 0));
        assert!(plain_click(true, true, 0x10000)); // Caps Lock is not a click modifier.
        assert!(!plain_click(false, true, 0));
        assert!(!plain_click(true, false, 0)); // An uninstalled/moved app cannot swallow clicks.
        for flag in [0x20000, 0x40000, 0x80000, 0x100000] {
            assert!(!plain_click(true, true, flag));
        }
    }

    #[test]
    fn matched_mouse_sequence_is_consumed_and_unrelated_clicks_pass() {
        let mut active = false;
        assert_eq!(mouse_action(&mut active, DOWN, true), Action::Open);
        assert_eq!(mouse_action(&mut active, DRAG, false), Action::Swallow);
        assert_eq!(mouse_action(&mut active, UP, false), Action::Swallow);
        assert_eq!(mouse_action(&mut active, UP, false), Action::Pass);
        assert_eq!(mouse_action(&mut active, DOWN, false), Action::Pass);
        assert_eq!(mouse_action(&mut active, DRAG, false), Action::Pass);
        assert_eq!(mouse_action(&mut active, 3, true), Action::Pass); // Right-click.
        assert_eq!(mouse_action(&mut active, DOWN, true), Action::Open);
        assert_eq!(mouse_action(&mut active, DOWN, false), Action::Pass); // Reset a stale down.
        assert!(!active);
    }

    #[test]
    fn finder_identity_requires_a_file_url_to_the_system_application() {
        assert!(!finder_url(None));
        assert!(!finder_url(Some(CFString::new(FINDER).as_CFType())));
        for (path, expected) in [
            (FINDER, true),
            ("/Applications/Finder.app", false),
            ("/System/Library/CoreServices/Finder.app.evil", false),
        ] {
            let url = CFURL::from_file_system_path(CFString::new(path), kCFURLPOSIXPathStyle, true);
            assert_eq!(finder_url(Some(url.as_CFType())), expected);
        }
    }

    #[test]
    fn edge_probe_handles_multiple_displays_and_does_not_pull_in_offscreen_points() {
        let displays = [
            CGRect::new(
                &CGPoint::new(0., 0.),
                &core_graphics::geometry::CGSize::new(1920., 1080.),
            ),
            CGRect::new(
                &CGPoint::new(204., 1080.),
                &core_graphics::geometry::CGSize::new(1512., 982.),
            ),
            CGRect::new(
                &CGPoint::new(-1280., 0.),
                &core_graphics::geometry::CGSize::new(1280., 1024.),
            ),
        ];
        for (point, expected) in [
            ((521., 2062.), (521., 2054.)),
            ((1919.999, 500.), (1912., 500.)),
            ((521., 1080.), (521., 1080.)),
            ((-500., 1024.), (-500., 1016.)),
            ((-1280., 500.), (-1280., 500.)),
            ((521., 2063.), (521., 2063.)),
        ] {
            let actual = screen_point(CGPoint::new(point.0, point.1), &displays);
            assert_eq!((actual.x, actual.y), expected);
        }
    }
}
