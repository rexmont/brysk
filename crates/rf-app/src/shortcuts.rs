//! The shortcut text shown beside menu rows, in the command palette and in tooltips.
//!
//! The strings are written once for the Explorer keymap (`Ctrl+…`, `Alt+…`). macOS also
//! installs Command/Option bindings (see `actions::bind`), and shows those in menus, so the
//! same text is rewritten to the symbols macOS uses.

/// `text` with its shortcut words in this platform's form.
pub fn hint(text: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        mac(text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        text.to_string()
    }
}

#[cfg(target_os = "macos")]
fn mac(text: &str) -> String {
    let mut s = text.to_string();
    // Keys whose macOS binding differs from the Explorer one (or which macOS itself owns).
    for (from, to) in [
        // Ctrl+Tab stays Control: Command+Tab belongs to the app switcher.
        ("Ctrl+Shift+Tab", "⌃⇧⇥"),
        ("Ctrl+Tab", "⌃⇥"),
        ("Ctrl+Space", "⌃Space"),
        // Hidden items are Command+Shift+Period on macOS (Command+H hides the app).
        ("Ctrl+H", "⇧⌘."),
        // Views are Command+1…5 (Command+Shift+3/4 are system screenshots).
        ("Ctrl+Shift+1", "⌘1"),
        ("Ctrl+Shift+2", "⌘2"),
        ("Ctrl+Shift+3", "⌘3"),
        ("Ctrl+Shift+4", "⌘4"),
        ("Ctrl+Shift+5", "⌘5"),
    ] {
        s = s.replace(from, to);
    }
    // Everything else follows the Command/Option bindings installed alongside the Ctrl ones.
    s = s.replace("Ctrl+", "⌘").replace("Alt+", "⌥").replace("Shift+", "⇧");
    s.replace("Del", "⌫")
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::mac;

    #[test]
    fn menu_hints_take_the_mac_keys() {
        assert_eq!(mac("Ctrl+N"), "⌘N");
        assert_eq!(mac("Ctrl+Shift+N"), "⌘⇧N");
        assert_eq!(mac("Shift+Del"), "⇧⌫");
        assert_eq!(mac("Alt+Enter"), "⌥Enter");
    }

    #[test]
    fn the_keys_macos_owns_keep_their_real_bindings() {
        assert_eq!(mac("Ctrl+Tab"), "⌃⇥");
        assert_eq!(mac("Ctrl+H"), "⇧⌘.");
        assert_eq!(mac("Ctrl+Shift+3"), "⌘3");
        assert_eq!(mac("Gizli öğeleri göster/gizle  (Ctrl+H)"), "Gizli öğeleri göster/gizle  (⇧⌘.)");
    }
}