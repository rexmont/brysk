//! Mouse and modifier-key state, so `rf-app` asks the platform once instead of calling Win32.

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

/// The mouse pointer in physical pixels.
pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

fn down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

pub fn shift_down() -> bool {
    down(VK_SHIFT)
}

pub fn ctrl_down() -> bool {
    down(VK_CONTROL)
}

/// Windows has no Command key; Ctrl is the equivalent.
pub fn cmd_down() -> bool {
    ctrl_down()
}

pub fn double_click_ms() -> u64 {
    unsafe { GetDoubleClickTime() as u64 }
}