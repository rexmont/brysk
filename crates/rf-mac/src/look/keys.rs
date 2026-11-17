//! The keys DirectLook understands, the macOS half of `rf-win::look::keys`.
//!
//! The virtual-key numbers are Windows'; macOS key handling goes through GPUI's key events,
//! which the viewers already receive. The types are here so the shared code compiles.

pub const VK_RETURN: u32 = 0x0D;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_SPACE: u32 = 0x20;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;
pub const VK_F5: u32 = 0x74;
pub const VK_F11: u32 = 0x7A;

pub const HOLD_CLOSE_MS: u64 = 750;
pub const GUARD_MS: u64 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookKey {
    Space,
    Enter,
    Escape,
    Up,
    Down,
    Left,
    Right,
    F5,
    F11,
}

impl LookKey {
    pub fn from_vk(vk: u32) -> Option<LookKey> {
        Some(match vk {
            VK_SPACE => LookKey::Space,
            VK_RETURN => LookKey::Enter,
            VK_ESCAPE => LookKey::Escape,
            VK_UP => LookKey::Up,
            VK_DOWN => LookKey::Down,
            VK_LEFT => LookKey::Left,
            VK_RIGHT => LookKey::Right,
            VK_F5 => LookKey::F5,
            VK_F11 => LookKey::F11,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookAction {
    Toggle,
    RunAndClose,
    Close,
    Reload,
    Fullscreen,
    SelectionMoved,
}