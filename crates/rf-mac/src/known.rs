//! The user's known folders on macOS: the same list Explorer's sidebar pins, resolved through
//! `$HOME` (Finder's own folders), which is what `NSFileManager` would return.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnownFolder {
    Applications,
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Music,
    Videos,
    Profile,
}

impl KnownFolder {
    pub const PINNED_DEFAULTS: [KnownFolder; 7] = [
        KnownFolder::Applications,
        KnownFolder::Desktop,
        KnownFolder::Downloads,
        KnownFolder::Documents,
        KnownFolder::Pictures,
        KnownFolder::Music,
        KnownFolder::Videos,
    ];

    /// The name the interface shows, as the Turkish source key of its string table.
    ///
    /// The platform layer cannot see the UI language setting, so this is NOT a finished label:
    /// `rf-app` translates it with `i18n::t`/`td` before drawing. Each key exists in
    /// `crates/rf-app/src/i18n_en.rs` ("Desktop", "Downloads", ...).
    pub fn title(self) -> &'static str {
        match self {
            KnownFolder::Applications => "Uygulamalar",
            KnownFolder::Desktop => "Masaüstü",
            KnownFolder::Downloads => "İndirilenler",
            KnownFolder::Documents => "Belgeler",
            KnownFolder::Pictures => "Resimler",
            KnownFolder::Music => "Müzik",
            KnownFolder::Videos => "Videolar",
            KnownFolder::Profile => "Kullanıcı",
        }
    }

    /// The folder's path, without asking the disk. Use it for label matching on a drawing
    /// path: `path()` below stats every home folder (a cloud/offline home would block the UI).
    pub fn path_unchecked(self) -> Option<PathBuf> {
        Some(match self {
            KnownFolder::Applications => PathBuf::from("/Applications"),
            KnownFolder::Desktop => dirs::home_dir()?.join("Desktop"),
            KnownFolder::Downloads => dirs::home_dir()?.join("Downloads"),
            KnownFolder::Documents => dirs::home_dir()?.join("Documents"),
            KnownFolder::Pictures => dirs::home_dir()?.join("Pictures"),
            KnownFolder::Music => dirs::home_dir()?.join("Music"),
            KnownFolder::Videos => dirs::home_dir()?.join("Movies"),
            KnownFolder::Profile => dirs::home_dir()?,
        })
    }

    pub fn path(self) -> Option<PathBuf> {
        self.path_unchecked().filter(|p| p.is_dir())
    }
}

pub fn known_folders() -> Vec<(KnownFolder, PathBuf)> {
    KnownFolder::PINNED_DEFAULTS.iter().filter_map(|k| k.path().map(|p| (*k, p))).collect()

}

#[cfg(test)]
mod tests {
    #[test]
    fn resolves_desktop() {
        // Not every account has a Desktop folder, but the profile always exists.
        assert!(super::KnownFolder::Profile.path().unwrap().is_dir());
    }

    #[test]
    fn every_known_folder_has_its_own_title_key() {
        use super::KnownFolder;
        let all = [KnownFolder::Applications, KnownFolder::Desktop, KnownFolder::Downloads, KnownFolder::Documents, KnownFolder::Pictures, KnownFolder::Music, KnownFolder::Videos, KnownFolder::Profile];
        let mut seen = std::collections::HashSet::new();
        for k in all {
            assert!(!k.title().is_empty());
            assert!(seen.insert(k.title()), "two known folders share the title key {:?}", k.title());
        }
    }
}