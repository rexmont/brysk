//! The system accent colour and the light/dark appearance, the macOS half of `rf-win::accent`.
//!
//! macOS gives one accent colour for the whole system (System Settings → Appearance); the
//! seven shades are derived from it exactly as Windows' palette does, so the theme code is
//! unchanged.

use std::sync::OnceLock;

/// Shades of the accent, as 0xRRGGBB: `[light3, light2, light1, base, dark1, dark2, dark3]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentPalette {
    pub shades: [u32; 7],
}

impl AccentPalette {
    pub fn light2(&self) -> u32 {
        self.shades[1]
    }
    pub fn dark1(&self) -> u32 {
        self.shades[4]
    }
    pub fn base(&self) -> u32 {
        self.shades[3]
    }
}

fn defaults_string(domain: &str, key: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/defaults").args(["read", domain, key]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// macOS' accent choices (`AppleAccentColor`); the default (no value) is the blue.
fn system_color() -> u32 {
    static C: OnceLock<u32> = OnceLock::new();
    *C.get_or_init(|| match defaults_string("-g", "AppleAccentColor").as_deref() {
        Some("-1") => 0x8E8E93, // graphite
        Some("0") => 0xFF3B30,  // red
        Some("1") => 0xFF9500,  // orange
        Some("2") => 0xFFCC00,  // yellow
        Some("3") => 0x34C759,  // green
        Some("5") => 0xAF52DE,  // purple
        Some("6") => 0xFF2D55,  // pink
        _ => 0x007AFF,          // blue (4 and the default)
    })
}

/// macOS has no per-user shade list; build one from the chosen colour.
pub fn system_palette() -> Option<AccentPalette> {
    Some(from_color(system_color()))
}

static OVERRIDE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(NO_OVERRIDE);
const NO_OVERRIDE: u32 = u32::MAX;

pub fn set_override(color: Option<u32>) {
    OVERRIDE.store(color.map_or(NO_OVERRIDE, |c| c & 0xFF_FFFF), std::sync::atomic::Ordering::Relaxed);
}

pub fn override_color() -> Option<u32> {
    let c = OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    (c != NO_OVERRIDE).then_some(c)
}

pub fn palette() -> Option<AccentPalette> {
    match override_color() {
        Some(c) => Some(from_color(c)),
        None => system_palette(),
    }
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xFF) as f32, ((b >> shift) & 0xFF) as f32);
        ((x + (y - x) * t).round() as u32).min(255) << shift
    };
    ch(16) | ch(8) | ch(0)
}

pub fn from_color(base: u32) -> AccentPalette {
    let base = base & 0xFF_FFFF;
    AccentPalette {
        shades: [mix(base, 0xFF_FFFF, 0.54), mix(base, 0xFF_FFFF, 0.36), mix(base, 0xFF_FFFF, 0.18), base, mix(base, 0, 0.15), mix(base, 0, 0.30), mix(base, 0, 0.45)],
    }
}

pub fn parse_hex(text: &str) -> Option<u32> {
    let t = text.trim().trim_start_matches('#');
    if !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match t.len() {
        6 => u32::from_str_radix(t, 16).ok(),
        3 => {
            let d: Vec<u32> = t.chars().filter_map(|c| c.to_digit(16)).collect();
            Some((d[0] * 17) << 16 | (d[1] * 17) << 8 | d[2] * 17)
        }
        _ => None,
    }
}

pub fn to_hex(color: u32) -> String {
    format!("#{:06X}", color & 0xFF_FFFF)
}

pub const DEFAULT_ACCENT: u32 = 0x0078D4;

pub fn color() -> u32 {
    palette().map_or(DEFAULT_ACCENT, |p| p.base())
}

pub fn dark_from_value(light_theme: Option<u32>) -> bool {
    light_theme == Some(0)
}

/// Whether the system is in the dark appearance (`AppleInterfaceStyle` is `Dark`).
pub fn system_is_dark() -> bool {
    static DARK: OnceLock<bool> = OnceLock::new();
    *DARK.get_or_init(|| defaults_string("-g", "AppleInterfaceStyle").as_deref() == Some("Dark"))
}

/// The app theme follows the system's, like the rest of macOS.
pub fn apps_use_dark() -> bool {
    system_is_dark()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_is_read_as_the_user_writes_it() {
        assert_eq!(parse_hex("#1A2B3C"), Some(0x1A2B3C));
        assert_eq!(parse_hex("  1a2b3c "), Some(0x1A2B3C));
        assert_eq!(parse_hex("#abc"), Some(0xAABBCC));
        assert_eq!(parse_hex("turuncu"), None);
        assert_eq!(to_hex(0x1A2B3C), "#1A2B3C");
    }

    #[test]
    fn a_palette_made_from_one_colour_is_ordered() {
        let p = from_color(0xF7630C);
        assert_eq!(p.base(), 0xF7630C);
        let light = |c: u32| ((c >> 16) & 0xFF) + ((c >> 8) & 0xFF) + (c & 0xFF);
        for w in p.shades.windows(2) {
            assert!(light(w[0]) > light(w[1]));
        }
    }
}