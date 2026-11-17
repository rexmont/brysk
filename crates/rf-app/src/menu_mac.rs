//! The macOS main menu: the standard shape (app, File, Edit, View, Go, Window) over the
//! app's own actions. The shortcuts shown beside the items come from the keymap, the same
//! bindings the shortcuts use (Ctrl and Cmd variants both work).

use gpui_kit::*;

use crate::actions::*;
use crate::t;

/// Installs the menu bar (once, after the actions are bound).
pub fn install(cx: &mut App) {
    let app = Menu::new("Brysk").items([
        MenuItem::action(t!("Brysk Hakkında"), About),
        MenuItem::separator(),
        MenuItem::action(t!("Ayarlar…"), GoSettings),
        MenuItem::separator(),
        MenuItem::os_submenu("Services", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action(t!("Brysk'tan çık"), Quit),
    ]);
    let file = Menu::new(t!("Dosya")).items([
        MenuItem::action(t!("Yeni sekme"), NewTab),
        MenuItem::action(t!("Yeni pencere"), NewWindow),
        MenuItem::separator(),
        MenuItem::action(t!("Yeni klasör"), NewFolder),
        MenuItem::action(t!("Yeni metin belgesi"), NewTextFile),
        MenuItem::separator(),
        MenuItem::action(t!("Sekmeyi kapat"), CloseTab),
        MenuItem::action(t!("Diğer sekmeleri kapat"), CloseOtherTabs),
        MenuItem::action(t!("Kapatılan sekmeyi yeniden aç"), ReopenClosedTab),
        MenuItem::separator(),
        MenuItem::action(t!("Özellikler"), Properties),
    ]);
    let edit = Menu::new(t!("Düzen")).items([
        MenuItem::os_action(t!("Geri al"), Undo, OsAction::Undo),
        MenuItem::os_action(t!("Yinele"), Redo, OsAction::Redo),
        MenuItem::separator(),
        MenuItem::os_action(t!("Kes"), CutItems, OsAction::Cut),
        MenuItem::os_action(t!("Kopyala"), CopyItems, OsAction::Copy),
        MenuItem::os_action(t!("Yapıştır"), PasteItems, OsAction::Paste),
        MenuItem::action(t!("Seçili klasöre yapıştır"), PasteIntoSelected),
        MenuItem::separator(),
        MenuItem::os_action(t!("Tümünü seç"), SelectAll, OsAction::SelectAll),
        MenuItem::separator(),
        MenuItem::action(t!("Yeniden adlandır"), RenameItem),
        MenuItem::action(t!("Sil"), DeleteItems),
    ]);
    let view = Menu::new(t!("Görünüm")).items([
        MenuItem::action(t!("Ayrıntılar"), ViewDetails),
        MenuItem::action(t!("Simgeler"), ViewGrid),
        MenuItem::action(t!("Sütunlar"), ViewColumns),
        MenuItem::separator(),
        MenuItem::action(t!("Önizleme paneli"), TogglePreview),
        MenuItem::action(t!("Kenar çubuğu"), ToggleSidebar),
        MenuItem::action(t!("Gizli öğeleri göster/gizle"), ToggleHidden),
        MenuItem::separator(),
        MenuItem::action(t!("Yenile"), Refresh),
        MenuItem::action(t!("Tam ekran"), ToggleFullscreen),
    ]);
    let go = Menu::new(t!("Git")).items([
        MenuItem::action(t!("Geri"), GoBack),
        MenuItem::action(t!("İleri"), GoForward),
        MenuItem::action(t!("Üst klasör"), GoUp),
        MenuItem::action(t!("Ana sayfa"), GoHome),
        MenuItem::separator(),
        MenuItem::action(t!("Klasöre git…"), FocusAddress),
    ]);
    cx.set_menus([app, file, edit, view, go]);
}
