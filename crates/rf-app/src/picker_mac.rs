//! Another app's Open/Save dialog being answered by Brysk is a Windows DLL feature
//! (`rustyfiles_dialog.dll`). The types keep the window code compiling; nothing starts a pick.

use gpui_kit::*;

pub const PICK_PREFIX: &str = "\u{1}pick:";

pub struct PickSession {
    patterns: String,
    folders_only: bool,
    pub filter_ix: std::cell::Cell<u32>,
}

impl PickSession {
    pub fn patterns(&self) -> String {
        self.patterns.clone()
    }
    pub fn folders_only(&self) -> bool {
        self.folders_only
    }
}

pub fn begin(_pipe: String, _cx: &mut App) {}

pub(crate) fn picked(
    _ws: &mut crate::workspace::Workspace,
    _paths: Vec<std::path::PathBuf>,
    _window: &mut Window,
    _cx: &mut Context<crate::workspace::Workspace>,
) {
}

pub(crate) fn footer(
    _ws: &mut crate::workspace::Workspace,
    _s: &std::rc::Rc<PickSession>,
    _p: &crate::theme::Palette,
    _window: &mut Window,
    _cx: &mut Context<crate::workspace::Workspace>,
) -> AnyElement {
    div().into_any_element()
}

pub fn answer_cancel(_ws: &mut crate::workspace::Workspace) {}
