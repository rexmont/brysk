//! The system-wide quick-search shortcut, the macOS half of `rf-win::hotkey`.
//!
//! Carbon's `RegisterEventHotKey` is the supported way for an app to own a global key
//! combination; it needs no Accessibility permission (unlike an event tap) and delivers the
//! press to the application's Carbon event target, which the AppKit run loop processes.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
const K_EVENT_HOTKEY_PRESSED: u32 = 5;
const K_EVENT_HOTKEY_RELEASED: u32 = 6;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type EventHandlerProc = extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, *mut std::ffi::c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetEventKind(event: *mut std::ffi::c_void) -> u32;
    fn GetApplicationEventTarget() -> *mut std::ffi::c_void;
    fn InstallEventHandler(
        target: *mut std::ffi::c_void,
        handler: EventHandlerProc,
        num_types: u32,
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
static DOWN: AtomicBool = AtomicBool::new(false);

fn first_press(down: &AtomicBool, pressed: bool) -> bool {
    let was_down = down.swap(pressed, Ordering::Relaxed);
    pressed && !was_down
}

extern "C" fn on_event(_handler: *mut std::ffi::c_void, event: *mut std::ffi::c_void, _user: *mut std::ffi::c_void) -> i32 {
    // Carbon can repeat pressed events while held. Toggle once, then wait for release.
    if !first_press(&DOWN, unsafe { GetEventKind(event) } == K_EVENT_HOTKEY_PRESSED) { return 0; }
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
    DOWN.store(false, Ordering::Relaxed);
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

fn parse(keys: &str) -> Result<(u32, u32), HotkeyError> {
    let mut mods = 0u32;
    let mut code = None;
    for part in keys.to_ascii_lowercase().split(['-', '+']) {
        match part {
            "alt" | "option" | "opt" => mods |= OPTION_KEY,
            "ctrl" | "control" => mods |= CONTROL_KEY,
            "shift" => mods |= SHIFT_KEY,
            "cmd" | "meta" | "super" | "win" => mods |= CMD_KEY,
            other => code = key_code(other).or(code),
        }
    }
    let code = code.ok_or(HotkeyError::Invalid)?;
    if mods == 0 {
        return Err(HotkeyError::Invalid);
    }
    Ok((code, mods))
}

/// Makes `keys` (GPUI syntax: "alt-space", "ctrl-shift-k") the global shortcut, or drops it.
pub fn set(keys: Option<&str>) -> Result<(), HotkeyError> {
    unregister();
    let Some(keys) = keys else { return Ok(()) };
    let (code, mods) = parse(keys)?;
    let installed = with_main(move || unsafe {
        // one handler for the application's key events, installed once per process
        static HANDLER: Mutex<bool> = Mutex::new(false);
        let target = GetApplicationEventTarget();
        if !*HANDLER.lock().unwrap() {
            let specs = [K_EVENT_HOTKEY_PRESSED, K_EVENT_HOTKEY_RELEASED].map(|event_kind| EventTypeSpec { event_class: K_EVENT_CLASS_KEYBOARD, event_kind });
            let mut out = std::ptr::null_mut();
            if InstallEventHandler(target, on_event, specs.len() as u32, specs.as_ptr().cast(), std::ptr::null_mut(), &mut out) != 0 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_shortcut_only_toggles_once_until_released() {
        let down = AtomicBool::new(false);
        assert!(first_press(&down, true));
        for _ in 0..10 { assert!(!first_press(&down, true)); }
        assert!(!first_press(&down, false));
        assert!(!first_press(&down, false));
        assert!(first_press(&down, true));
    }

    #[test]
    fn recorded_shortcuts_preserve_mac_modifiers() {
        assert_eq!(parse("alt-space"), Ok((49, OPTION_KEY)));
        assert_eq!(parse("ctrl-shift-k"), Ok((40, CONTROL_KEY | SHIFT_KEY)));
        assert_eq!(parse("win-shift-k"), Ok((40, CMD_KEY | SHIFT_KEY)));
        assert_eq!(parse("cmd-shift-k"), parse("win-shift-k"));
        assert_eq!(parse("space"), Err(HotkeyError::Invalid));
        assert_eq!(parse("alt"), Err(HotkeyError::Invalid));
    }
}
