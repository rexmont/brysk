//! The small part of `gpui_windows` (the Windows fork's extension crate) that `rf-app` calls
//! directly, for macOS: there is no hybrid GPU/CPU renderer to switch, no drop-effect hint and
//! no Win32 foreground-window tricks, so these are constants and no-ops.
//!
//! Keeping the same paths means `rf-app` needs no `cfg` around those call sites.

use std::sync::atomic::{AtomicBool, Ordering};

/// Windows keeps the app's windows "active" while a flyout is open; macOS menus and panels
/// handle this themselves.
pub static KEEP_ACTIVE_LOOK: AtomicBool = AtomicBool::new(false);

/// Windows' smooth mouse-wheel scrolling preference, read by the list.
pub static SMOOTH_SCROLL: AtomicBool = AtomicBool::new(true);

pub mod dnd_hint {
    /// No drop effect.
    pub const NONE: u32 = 0;
    /// Copy drop effect.
    pub const COPY: u32 = 1;
    /// Move drop effect.
    pub const MOVE: u32 = 2;

    /// Sets the cursor's drop effect for the next drag-over (Windows only; no-op here).
    pub fn set(_effect: u32) {}
}

pub mod hybrid {
    use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

    /// Which renderer the user asked for.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum RendererMode {
        /// Pick by measurement.
        Auto,
        /// Always the GPU.
        Gpu,
        /// Always the CPU.
        Cpu,
    }

    static MODE: AtomicU8 = AtomicU8::new(0);
    static SWITCHES: AtomicU64 = AtomicU64::new(0);

    /// Starts the "first frame on the GPU" measurement (Windows only; no-op here).
    pub fn set_fast_start(_on: bool) {}

    /// How long the GPU device took to come up, if it has been measured.
    pub fn gpu_device_ms() -> Option<u32> {
        None
    }

    /// How many times the renderer changed device.
    pub fn switch_count() -> u64 {
        SWITCHES.load(Ordering::Relaxed)
    }

    /// The renderer the user asked for.
    pub fn mode() -> RendererMode {
        match MODE.load(Ordering::Relaxed) {
            1 => RendererMode::Gpu,
            2 => RendererMode::Cpu,
            _ => RendererMode::Auto,
        }
    }

    /// Sets the renderer (the choice is persisted by the settings; macOS always draws on Metal).
    pub fn set_mode(mode: RendererMode) {
        MODE.store(
            match mode {
                RendererMode::Auto => 0,
                RendererMode::Gpu => 1,
                RendererMode::Cpu => 2,
            },
            Ordering::Relaxed,
        );
    }

    /// What draws the window, as the status bar badge shows it.
    pub fn current_backend_label() -> &'static str {
        "GPU"
    }

    /// The renderer's live counters (Windows: GPU/CPU load from the hybrid engine).
    #[derive(Debug, Clone, Copy, Default)]
    pub struct HybridSnapshot {
        pub gpu_percent: Option<f32>,
        pub cpu_percent: Option<f32>,
        pub last_frame_ms: f32,
    }

    /// No hybrid renderer on macOS: the counters are unknown.
    pub fn load_snapshot() -> HybridSnapshot {
        HybridSnapshot::default()
    }
}