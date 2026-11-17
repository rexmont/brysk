//! The Properties window's Windows implementation (security, compatibility, PE facts) is not
//! ported yet; the macOS version will show the Finder's own Get Info facts. Until then the
//! menu entries do nothing rather than opening an empty window.

use std::path::PathBuf;

use gpui_kit::App;

use crate::image_cache::ImageCache;

pub fn open(_paths: Vec<PathBuf>, _images: gpui_kit::Entity<ImageCache>, _cx: &mut App) {}

pub fn open_with(_paths: Vec<PathBuf>, _images: gpui_kit::Entity<ImageCache>, _locked: bool, _cx: &mut App) {}
