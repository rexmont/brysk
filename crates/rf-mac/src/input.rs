//! Mouse and modifier-key state, for the places the interface asks Windows directly today.
//!
//! Quartz' event source answers both without the AppKit main thread (a global session state
//! and the last mouse location), so the context menu and ops code keeps working outside a
//! window update.

use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

/// The mouse pointer in screen points (top-left origin, as GPUI uses).
pub fn cursor_pos() -> (i32, i32) {
    CGEvent::new(CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok().unwrap_or_else(|| CGEventSource::new(CGEventSourceStateID::HIDSystemState).unwrap()))
        .map(|e| {
            let p = e.location();
            (p.x.round() as i32, p.y.round() as i32)
        })
        .unwrap_or((0, 0))
}

// Hardware key state through Quartz: the virtual key codes of the left-hand modifiers.
unsafe extern "C" {
    fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
}

/// 0 = `kCGEventSourceStateCombinedSessionState`.
const COMBINED: i32 = 0;
const KEY_SHIFT: u16 = 56;
const KEY_CONTROL: u16 = 59;
const KEY_COMMAND: u16 = 55;

/// The Shift key is down.
pub fn shift_down() -> bool {
    unsafe { CGEventSourceKeyState(COMBINED, KEY_SHIFT) }
}

/// The Control key is down.
pub fn ctrl_down() -> bool {
    unsafe { CGEventSourceKeyState(COMBINED, KEY_CONTROL) }
}

/// The Option key is the macOS file-drag copy modifier.
pub fn alt_down() -> bool {
    unsafe { CGEventSourceKeyState(COMBINED, 58) || CGEventSourceKeyState(COMBINED, 61) }
}

/// The Command key is down.
pub fn cmd_down() -> bool {
    unsafe { CGEventSourceKeyState(COMBINED, KEY_COMMAND) }
}

/// The system's double-click interval in milliseconds.
pub fn double_click_ms() -> u64 {
    // NSEvent.doubleClickInterval needs AppKit; the default is what almost every Mac uses.
    500
}