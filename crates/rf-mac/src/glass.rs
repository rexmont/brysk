//! HyperGlass, the macOS half of `rf-win::glass`.
//!
//! The Windows version captures the desktop and bends it behind the window (a GPU pass and a
//! screen-capture engine). macOS has the system's own material — an `NSVisualEffectView`
//! behind the window — and GPUI's blurred backdrop is the equivalent; the lens/refraction
//! engine is Windows-only. Everything here reports "off" so the theme falls back to its
//! ordinary backdrop.

/// What a window's glass is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlassState {
    Off,
    Starting,
    Running,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub opacity: f32,
    pub dy: f32,
    pub blur: f32,
}

impl Shadow {
    pub const SOFT: Shadow = Shadow { opacity: 0.26, dy: 4.0, blur: 22.0 };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassOptions {
    pub dark: bool,
    pub radius_px: Option<f32>,
    pub cut_corners: bool,
    pub shape: Option<[f32; 4]>,
    pub shadow: Option<Shadow>,
}

impl GlassOptions {
    pub const fn window(dark: bool) -> GlassOptions {
        GlassOptions { dark, radius_px: None, cut_corners: false, shape: None, shadow: None }
    }

    pub fn sanitized(self) -> GlassOptions {
        GlassOptions { radius_px: self.radius_px.filter(|r| r.is_finite()).map(|r| r.max(0.0)), ..self }
    }
}

/// How the engine keeps its own windows out of the picture it bends (Windows only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureMode {
    Off,
    Session,
    Affinity,
}

pub fn capture_mode() -> CaptureMode {
    CaptureMode::Off
}

pub const MAX_LENSES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookParams {
    pub frost: f32,
    pub tint: f32,
    pub refraction: f32,
    pub saturation: f32,
}

impl LookParams {
    pub const DEFAULT: LookParams = LookParams { frost: 52.0, tint: 50.0, refraction: 67.0, saturation: 62.0 };

    pub fn clamped(self) -> LookParams {
        let c = |v: f32, d: f32| if v.is_nan() { d } else { v.clamp(0.0, 100.0) };
        LookParams {
            frost: c(self.frost, Self::DEFAULT.frost),
            tint: c(self.tint, Self::DEFAULT.tint),
            refraction: c(self.refraction, Self::DEFAULT.refraction),
            saturation: c(self.saturation, Self::DEFAULT.saturation),
        }
    }
}

impl Default for LookParams {
    fn default() -> LookParams {
        LookParams::DEFAULT
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensSpec {
    pub key: u64,
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub radius: f32,
}

pub fn attach(_hwnd: isize, _dark: bool) -> bool {
    false
}

pub fn attach_with(_hwnd: isize, _opts: GlassOptions) -> bool {
    false
}

pub fn set_dark(_hwnd: isize, _dark: bool) {}

pub fn set_options(_hwnd: isize, _opts: GlassOptions) {}

pub fn detach(_hwnd: isize) {}

pub fn set_rim_cap(_hwnd: isize, _cap: Option<f32>) {}

pub fn backdrop_luma(_hwnd: isize) -> Option<f32> {
    None
}

pub fn set_lens(_hwnd: isize, _slot: u8, _lens: Option<LensSpec>, _animate: bool) {}

pub fn set_look(_params: LookParams) {}

pub fn set_freeze_when_behind(_on: bool) {}

pub fn set_freezable(_hwnd: isize, _on: bool) {}

pub fn pointer_over(_hwnd: isize) {}

pub fn state(_hwnd: isize) -> GlassState {
    GlassState::Off
}

pub fn any_active() -> bool {
    false
}

pub fn exclude_from_capture(_hwnd: isize, _on: bool) {}