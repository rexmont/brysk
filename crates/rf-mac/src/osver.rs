//! Which macOS this is, the macOS half of `rf-win::osver`.
//!
//! The Windows build number gates Mica and system backdrops; macOS 11+ has the visual-effect
//! views and Metal the theme expects, so the answer is always "yes" and the build is 0
//! (unknown, the same "assume yes" rule the Windows code uses).

/// The first macOS with everything the theme uses (`NSVisualEffectView`, Metal).
pub const SYSTEM_BACKDROP_BUILD: u32 = 11;

pub fn build() -> u32 {
    0
}

pub fn system_backdrops() -> bool {
    true
}

pub fn has_system_backdrops(build: u32) -> bool {
    build == 0 || build >= SYSTEM_BACKDROP_BUILD
}