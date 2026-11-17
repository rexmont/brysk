//! Context menus, the macOS half of `rf-win::ctxmenu`.
//!
//! The Windows version asks the shell for its menu and renders it with the app's own Fluent
//! menu. macOS has no shell menu to ask: the app's own menu is shown (the same `ExtraItem`s the
//! Windows version adds), and "Open with", "Reveal in Finder" and Finder's tags are added by
//! the app's menu code. `build`/`invoke` keep the data shapes so the shared menu UI compiles.

use std::path::{Path, PathBuf};

use crate::HWND;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuOutcome {
    Dismissed,
    Rename,
    Invoked(String),
    Custom(u32),
}

#[derive(Debug, Clone)]
pub struct ExtraItem {
    pub id: u32,
    pub label: String,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub enum ShellTarget {
    Items(Vec<PathBuf>),
    One(PathBuf),
    Background(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ShellIcon {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ShellItem {
    pub id: u32,
    pub label: String,
    pub hint: Option<String>,
    pub verb: String,
    pub icon: Option<ShellIcon>,
    pub enabled: bool,
    pub separator: bool,
    pub children: Vec<ShellItem>,
}

/// No shell menu on macOS: the app builds its own from the selection.
pub fn build(_owner: HWND, _target: &ShellTarget, _extended: bool) -> anyhow::Result<(u64, Vec<ShellItem>)> {
    Ok((0, Vec::new()))
}

pub fn invoke(_owner: HWND, _session: u64, _id: u32, _screen: (i32, i32)) -> anyhow::Result<MenuOutcome> {
    Ok(MenuOutcome::Dismissed)
}

pub fn release(_session: u64) {}

pub fn invoke_verb(_owner: HWND, _target: &ShellTarget, _verb: &str, _screen: (i32, i32)) -> anyhow::Result<()> {
    Ok(())
}

/// The app's own menu, drawn by the caller; nothing is shown from here.
pub fn show_for_items(_owner: HWND, _items: &[PathBuf], _screen: (i32, i32), _extended: bool, _extra: &[ExtraItem]) -> anyhow::Result<MenuOutcome> {
    Ok(MenuOutcome::Dismissed)
}

pub fn show_for_one(_owner: HWND, _item: &Path, _screen: (i32, i32), _extended: bool, _extra: &[ExtraItem]) -> anyhow::Result<MenuOutcome> {
    Ok(MenuOutcome::Dismissed)
}

pub fn show_for_background(_owner: HWND, _dir: &Path, _screen: (i32, i32), _extended: bool, _extra: &[ExtraItem]) -> anyhow::Result<MenuOutcome> {
    Ok(MenuOutcome::Dismissed)
}

pub fn show_simple(_owner: HWND, _screen: (i32, i32), _items: &[ExtraItem]) -> Option<u32> {
    None
}

/// "Open with…" / a renamed label: the shell's `clean_label` strips its accelerators.
pub fn clean_label(raw: &str) -> (String, Option<String>) {
    let text = raw.replace('&', "");
    match text.split_once('\t') {
        Some((label, hint)) => (label.trim().to_string(), Some(hint.trim().to_string())),
        None => (text.trim().to_string(), None),
    }
}