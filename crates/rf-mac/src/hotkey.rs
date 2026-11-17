//! The system-wide quick-search shortcut, the macOS half of `rf-win::hotkey`.
//!
//! Carbon's `RegisterEventHotKey` is the supported way for an app to own a global key
//! combination; it needs no Accessibility permission (unlike an event tap) and delivers the
//! press to the application's Carbon event target, which the AppKit run loop processes.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyError {
    /// Not a shortcut we can register (no modifier, unknown key).
    Invalid,
    /// Another app (or the system) has it.
    Taken,
}

// ---- Carbon -----------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOTKEY_PRESSED: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type EventHandlerProc = extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, *mut std::ffi::c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut std::ffi::c_void;
    fn InstallEventHandler(
        target: *mut std::ffi::c_void,
        handler: EventHandlerProc,
        num_types: u64,
        event_types: *const std::ffi::c_void,
        user_data: *mut std::ffi::c_void,
        out_ref: *mut *mut std::ffi::c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        hotkey_id: EventHotKeyID,
        target: *mut std::ffi::c_void,
        options: u32,
        out_ref: *mut *mut std::ffi::c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey: *mut std::ffi::c_void) -> i32;
}

const SIGNATURE: u32 = u32::from_be_bytes(*b"RFSH");
const OPTION_KEY: u32 = 0x0800;
const CONTROL_KEY: u32 = 0x1000;
const SHIFT_KEY: u32 = 0x0200;
const CMD_KEY: u32 = 0x0100;

static CALLBACK: Mutex<Option<Box<dyn Fn() + Send + 'static>>> = Mutex::new(None);
static HOTKEY: Mutex<Option<usize>> = Mutex::new(None);
static PRESSES: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_event(_handler: *mut std::ffi::c_void, _event: *mut std::ffi::c_void, _user: *mut std::ffi::c_void) -> i32 {
    PRESSES.fetch_add(1, Ordering::Relaxed);
    if let Some(f) = CALLBACK.lock().unwrap().as_ref() {
        f();
    }
    0 // noErr
}

fn key_code(name: &str) -> Option<u32> {
    // Carbon virtual key codes (the layout-independent ones).
    Some(match name {
        "space" => 49,
        "return" | "enter" => 36,
        "tab" => 48,
        "escape" | "esc" => 53,
        "a" => 0,
        "b" => 11,
        "c" => 8,
        "d" => 2,
        "e" => 14,
        "f" => 3,
        "g" => 5,
        "h" => 4,
        "i" => 34,
        "j" => 38,
        "k" => 40,
        "l" => 37,
        "m" => 46,
        "n" => 45,
        "o" => 31,
        "p" => 35,
        "q" => 12,
        "r" => 15,
        "s" => 1,
        "t" => 17,
        "u" => 32,
        "v" => 9,
        "w" => 13,
        "x" => 7,
        "y" => 16,
        "z" => 6,
        "0" => 29,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "5" => 23,
        "6" => 22,
        "7" => 26,
        "8" => 28,
        "9" => 25,
        _ => {
            if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
                // F1..F12 are 122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111
                const F: [u32; 12] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111];
                return (1..=12).contains(&n).then(|| F[(n - 1) as usize]);
            }
            return None;
        }
    })
}

fn with_main<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    // Already on the main thread (the usual case at start-up): run it here. Dispatching to the
    // main queue and waiting would wait on ourselves until the timeout.
    if objc2_foundation::MainThreadMarker::new().is_some() {
        return Some(job());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    dispatch2::DispatchQueue::main().exec_async(move || {
        let _ = tx.send(job());
    });
    rx.recv_timeout(std::time::Duration::from_secs(5)).ok()
}

fn unregister() -> Option<usize> {
    let old = HOTKEY.lock().unwrap().take();
    if let Some(hk) = old {
        unsafe {
            let _ = UnregisterEventHotKey(hk as *mut _);
        }
    }
    old
}

/// Installs `f` as the handler of the registered shortcut.
pub fn set_handler(f: impl Fn() + Send + 'static) {
    *CALLBACK.lock().unwrap() = Some(Box::new(f));
}

/// Makes `keys` (GPUI syntax: "alt-space", "ctrl-shift-k") the global shortcut, or drops it.
pub fn set(keys: Option<&str>) -> Result<(), HotkeyError> {
    unregister();
    let Some(keys) = keys else { return Ok(()) };
    let mut mods = 0u32;
    let mut code = None;
    for part in keys.to_ascii_lowercase().split(['-', '+']) {
        match part {
            "alt" | "option" | "opt" => mods |= OPTION_KEY,
            "ctrl" | "control" => mods |= CONTROL_KEY,
            "shift" => mods |= SHIFT_KEY,
            "cmd" | "meta" | "super" => mods |= CMD_KEY,
            other => code = key_code(other).or(code),
        }
    }
    let code = code.ok_or(HotkeyError::Invalid)?;
    if mods == 0 {
        return Err(HotkeyError::Invalid);
    }
    let installed = with_main(move || unsafe {
        // one handler for the application's key events, installed once per process
        static HANDLER: Mutex<bool> = Mutex::new(false);
        let target = GetApplicationEventTarget();
        if !*HANDLER.lock().unwrap() {
            let spec = EventTypeSpec { event_class: K_EVENT_CLASS_KEYBOARD, event_kind: K_EVENT_HOTKEY_PRESSED };
            let mut out = std::ptr::null_mut();
            if InstallEventHandler(target, on_event, 1, &spec as *const _ as *const _, std::ptr::null_mut(), &mut out) != 0 {
                return None;
            }
            *HANDLER.lock().unwrap() = true;
        }
        let mut hk = std::ptr::null_mut();
        let id = EventHotKeyID { signature: SIGNATURE, id: 1 };
        (RegisterEventHotKey(code, mods, id, target, 0, &mut hk) == 0).then_some(hk as usize)
    })
    .flatten();
    match installed {
        Some(hk) => {
            *HOTKEY.lock().unwrap() = Some(hk);
            Ok(())
        }
        None => Err(HotkeyError::Taken),
    }
}

/// How many times the shortcut fired (a cheap check for tests and the log).
pub fn presses() -> u32 {
    PRESSES.load(Ordering::Relaxed)
}
