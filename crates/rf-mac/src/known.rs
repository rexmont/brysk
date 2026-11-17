//! The user's known folders on macOS: the same list Explorer's sidebar pins, resolved through
//! `$HOME` (Finder's own folders), which is what `NSFileManager` would return.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnownFolder {
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Music,
    Videos,
    Profile,
}

impl KnownFolder {
    pub const PINNED_DEFAULTS: [KnownFolder; 6] = [
        KnownFolder::Desktop,
        KnownFolder::Downloads,
        KnownFolder::Documents,
        KnownFolder::Pictures,
        KnownFolder::Music,
        KnownFolder::Videos,
    ];

    pub fn title(self) -> &'static str {
        match self {
            KnownFolder::Desktop => "Masaüstü",
            KnownFolder::Downloads => "İndirilenler",
            KnownFolder::Documents => "Belgeler",
            KnownFolder::Pictures => "Resimler",
            KnownFolder::Music => "Müzik",
            KnownFolder::Videos => "Videolar",
            KnownFolder::Profile => "Kullanıcı",
        }
    }

    pub fn path(self) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        Some(match self {
            KnownFolder::Desktop => home.join("Desktop"),
            KnownFolder::Downloads => home.join("Downloads"),
            KnownFolder::Documents => home.join("Documents"),
            KnownFolder::Pictures => home.join("Pictures"),
            KnownFolder::Music => home.join("Music"),
            KnownFolder::Videos => home.join("Movies"),
            KnownFolder::Profile => home,
        })
        .filter(|p| p.is_dir())
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
}