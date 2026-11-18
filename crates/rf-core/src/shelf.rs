//! The shelf: a staging list of items collected from different folders, to copy/move/delete
//! them together later (Files' "Shelf pane"). Ordered, unique (case-insensitive on Windows).

use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct Shelf {
    items: Vec<PathBuf>,
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

impl Shelf {
    pub fn items(&self) -> &[PathBuf] {
        &self.items
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn contains(&self, p: &Path) -> bool {
        let k = key(p);
        self.items.iter().any(|i| key(i) == k)
    }
    /// Adds paths not already on the shelf; returns how many were added.
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> usize {
        // One pass over the keys already there, not one lower-cased copy per item per path: a
        // shelf with ten thousand items used to make fifty million small strings while a big
        // selection was added.
        let mut seen: std::collections::HashSet<String> = self.items.iter().map(|i| key(i)).collect();
        let mut added = 0;
        for p in paths {
            if seen.insert(key(&p)) {
                self.items.push(p);
                added += 1;
            }
        }
        added
    }
    pub fn remove(&mut self, p: &Path) {
        let k = key(p);
        self.items.retain(|i| key(i) != k);
        self.shrink_if_sparse();
    }
    /// Drops items that no longer exist (moved/deleted elsewhere). Blocking (stats each item).
    pub fn prune_missing(&mut self) {
        self.items.retain(|p| p.exists());
        self.shrink_if_sparse();
    }
    /// Gives the backing buffer back after many items left (a shelf emptied after a big
    /// selection kept room for every path until the window closed).
    fn shrink_if_sparse(&mut self) {
        if self.items.capacity() > self.items.len() * 2 + 16 {
            self.items.shrink_to_fit();
        }
    }
    pub fn clear(&mut self) {
        self.items = Vec::new();
    }
    /// After a move: replaces moved items with their new locations.
    pub fn relocate(&mut self, from: &Path, to: &Path) {
        let k = key(from);
        for i in &mut self.items {
            if key(i) == k {
                *i = to.to_path_buf();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_ordered() {
        let mut s = Shelf::default();
        assert_eq!(s.add([PathBuf::from(r"C:\a.txt"), PathBuf::from(r"C:\B")]), 2);
        assert_eq!(s.add([PathBuf::from(r"c:\A.TXT"), PathBuf::from(r"C:\c")]), 1);
        assert_eq!(s.items(), &[PathBuf::from(r"C:\a.txt"), PathBuf::from(r"C:\B"), PathBuf::from(r"C:\c")]);
        s.remove(Path::new(r"c:\b"));
        assert_eq!(s.len(), 2);
        s.relocate(Path::new(r"C:\c"), Path::new(r"D:\c"));
        assert!(s.contains(Path::new(r"d:\C")));
    }

    #[test]
    fn a_big_shelf_takes_duplicates_once_and_gives_its_room_back() {
        let mut s = Shelf::default();
        let paths: Vec<PathBuf> = (0..1000).map(|i| PathBuf::from(format!("/tmp/{i}"))).collect();
        assert_eq!(s.add(paths), 1000);
        // the same thousand again, spelled with another case on the way in: none of them twice
        assert_eq!(s.add((0..1000).map(|i| PathBuf::from(format!("/TMP/{i}")))), 0);
        assert_eq!(s.len(), 1000);
        assert!(s.items.capacity() >= 1000);
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.items.capacity(), 0, "the buffer went with the items");
    }
}
