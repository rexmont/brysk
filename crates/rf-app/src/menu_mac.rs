//! The macOS main menu: the standard shape (app, File, Edit, View, Go, Window) over the
//! app's own actions. The shortcuts shown beside the items come from the keymap, the same
//! bindings the shortcuts use (Ctrl and Cmd variants both work).

use gpui_kit::*;

use crate::actions::*;

/// Installs the menu bar (once, after the actions are bound).
pub fn install(cx: &mut App) {
    let app = Menu::new("Brysk").items([
        MenuItem::action("About Brysk", About),
        MenuItem::separator(),
        MenuItem::action("Settings…", GoSettings),
        MenuItem::separator(),
        MenuItem::os_submenu("Services", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action("Quit Brysk", Quit),
    ]);
    let file = Menu::new("File").items([
        MenuItem::action("New Tab", NewTab),
        MenuItem::action("New Window", NewWindow),
        MenuItem::separator(),
        MenuItem::action("New Folder", NewFolder),
        MenuItem::action("New Text File", NewTextFile),
        MenuItem::separator(),
        MenuItem::action("Close Tab", CloseTab),
        MenuItem::action("Close Other Tabs", CloseOtherTabs),
        MenuItem::action("Reopen Closed Tab", ReopenClosedTab),
        MenuItem::separator(),
        MenuItem::action("Get Info", Properties),
    ]);
    let edit = Menu::new("Edit").items([
        MenuItem::os_action("Undo", Undo, OsAction::Undo),
        MenuItem::os_action("Redo", Redo, OsAction::Redo),
        MenuItem::separator(),
        MenuItem::os_action("Cut", CutItems, OsAction::Cut),
        MenuItem::os_action("Copy", CopyItems, OsAction::Copy),
        MenuItem::os_action("Paste", PasteItems, OsAction::Paste),
        MenuItem::action("Paste Into Folder", PasteIntoSelected),
        MenuItem::separator(),
        MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        MenuItem::separator(),
        MenuItem::action("Rename", RenameItem),
        MenuItem::action("Delete", DeleteItems),
    ]);
    let view = Menu::new("View").items([
        MenuItem::action("Details", ViewDetails),
        MenuItem::action("Icons", ViewGrid),
        MenuItem::action("Columns", ViewColumns),
        MenuItem::separator(),
        MenuItem::action("Preview Pane", TogglePreview),
        MenuItem::action("Sidebar", ToggleSidebar),
        MenuItem::action("Hidden Items", ToggleHidden),
        MenuItem::separator(),
        MenuItem::action("Refresh", Refresh),
        MenuItem::action("Enter Full Screen", ToggleFullscreen),
    ]);
    let go = Menu::new("Go").items([
        MenuItem::action("Back", GoBack),
        MenuItem::action("Forward", GoForward),
        MenuItem::action("Enclosing Folder", GoUp),
        MenuItem::action("Home", GoHome),
        MenuItem::separator(),
        MenuItem::action("Go to Folder…", FocusAddress),
    ]);
    cx.set_menus([app, file, edit, view, go]);
}
