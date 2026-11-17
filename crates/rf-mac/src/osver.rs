//! Which macOS this is, the macOS half of `rf-win::osver`.
//!
//! The Windows build number gates Mica and system backdrops. macOS has no Mica: GPUI's macOS
//! layer only gives a window a material for `Blurred` (an `NSVisualEffectView`), and treats
//! every other non-opaque request as a transparent window with nothing behind it. So the answer
//! here is "no system backdrops": the palette paints the window itself with its plain colour
//! (`settings::solid_backdrop`), which is the same fallback Windows 10 uses. A macOS vibrancy
//! material can become a separate appearance choice later.

/// The first macOS with everything the theme uses (`NSVisualEffectView`, Metal).
pub const SYSTEM_BACKDROP_BUILD: u32 = 11;

pub fn build() -> u32 {
    0
}

/// macOS has no Mica/Acrylic windows; see the module note.
pub fn system_backdrops() -> bool {
    false
}

/// [`system_backdrops`] for a build number: false on every macOS.
pub fn has_system_backdrops(_build: u32) -> bool {
    false
}