//! The rustdecoder video window is a Windows (D3D11/HWND) feature; on macOS video plays through
//! the AVFoundation route in `video.rs` (milestone M3). This module keeps the shapes the viewer
//! uses and always says "cannot open", so the fallback route is taken.

use std::path::Path;

#[derive(Debug, Clone, Copy, Default)]
pub struct Settings {
    pub volume: f32,
    pub muted: bool,
    pub looping: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowState {
    pub position: f64,
    pub duration: f64,
    pub playing: bool,
    pub ended: bool,
    pub video_size: (u32, u32),
}

#[derive(Debug, Clone, Copy)]
pub enum WindowEvent {
    Click,
    DoubleClick,
    Wheel(i32),
    MouseMove,
    Leave,
    Closed,
}

#[derive(Debug, Clone, Copy)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    pub codec: &'static str,
}

/// The rustdecoder route is Windows-only (D3D11); never taken here.
pub fn enabled() -> bool {
    false
}

pub fn probe(_path: &Path) -> Option<Probe> {
    None
}

pub struct RdVideo;

impl RdVideo {
    pub fn open(_path: &Path, _owner: isize, _rect: [i32; 4], _s: &Settings) -> Result<RdVideo, String> {
        Err("rustdecoder's video window is Windows-only".into())
    }

    pub fn state(&self) -> WindowState {
        WindowState::default()
    }

    pub fn take_events(&self) -> Vec<WindowEvent> {
        Vec::new()
    }

    pub fn toggle(&self) {}
    pub fn seek_fraction(&self, _f: f64) {}
    pub fn set_volume(&self, _v: f32) {}
    pub fn set_looping(&self, _on: bool) {}
    pub fn set_muted(&self, _on: bool) {}
    pub fn reposition(&self, _rect: [i32; 4]) {}
}
