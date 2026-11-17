//! Rich preview handlers, the macOS half of `rf-win::preview_host`.
//!
//! On Windows these host the shell's preview handlers and Media Foundation in the preview pane.
//! macOS shows previews with Quick Look (QuickLookThumbnailing/QuickLookUI) and plays media with
//! AVFoundation; both arrive with the DirectLook milestone. Until then every function says
//! "no handler", so the built-in Rust viewers (images, text, archives, video through
//! `rf-player`) are what the preview pane uses.

use std::path::Path;

use crate::thumbs::Bgra;
use crate::RECT;

/// A preview handler's class id (Windows concept); none on macOS.
pub type HandlerId = u32;

pub fn handler_for(_path: &Path) -> Option<HandlerId> {
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaEvent {
    Opened,
    Played,
    Paused,
    Ended,
    Failed,
}

pub fn set_media_events(_f: impl Fn(MediaEvent) + Send + 'static) {}

pub fn is_playable(_path: &Path) -> bool {
    false
}

pub fn mf_decodes(_codec: &str) -> bool {
    false
}

pub fn media_play(_owner: isize, _path: &Path, _rect: Option<RECT>) {}

pub fn media_pause() {}

pub fn media_toggle() -> Option<bool> {
    None
}

pub fn media_seek(_fraction: f64) {}

#[derive(Debug, Clone, Copy, Default)]
pub struct MediaState {
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
}

pub fn media_state() -> Option<MediaState> {
    None
}

pub fn media_reposition(_rect: RECT) {}

#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub background: u32,
    pub text: u32,
}

pub fn show(_owner: isize, _path: &Path, _rect: RECT, _colors: &Colors) -> bool {
    false
}

pub fn hide() {}

pub fn reposition(_rect: RECT) {}

/// The Windows preview-host pixel type leaks into one signature; keep the name.
pub type Image = Bgra;