//! Searching a folder tree by name, the macOS half of `rf-win::search`.
//!
//! A parallel walk with `readdir` (the Windows version uses FindFirstFileEx), stopping at the
//! result limit and polling the cancel flag. Names are matched case-insensitively, or as a
//! glob when the pattern has `*`/`?`.

use std::path::{Path, PathBuf};

use rf_core::{Entry, EntryKind};

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub root: PathBuf,
    /// Case-insensitive substring, or a glob when it contains `*` / `?`.
    pub pattern: String,
    pub include_hidden: bool,
    pub max_results: usize,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: PathBuf,
    pub entry: Entry,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchStats {
    pub dirs: usize,
    pub entries: usize,
    pub skipped: usize,
}

struct Matcher {
    glob: Option<Vec<char>>,
    needle: String,
}

impl Matcher {
    fn new(p: &str) -> Matcher {
        if p.contains('*') || p.contains('?') {
            Matcher { glob: Some(p.to_lowercase().chars().collect()), needle: String::new() }
        } else {
            Matcher { glob: None, needle: p.to_lowercase() }
        }
    }

    fn hit(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        match &self.glob {
            Some(g) => glob(g, &lower.chars().collect::<Vec<_>>()),
            None => lower.contains(&self.needle),
        }
    }
}

fn glob(spec: &[char], name: &[char]) -> bool {
    if spec.is_empty() {
        return name.is_empty();
    }
    match spec[0] {
        '*' => (0..=name.len()).any(|i| glob(&spec[1..], &name[i..])),
        '?' => !name.is_empty() && glob(&spec[1..], &name[1..]),
        c => name.first() == Some(&c) && glob(&spec[1..], &name[1..]),
    }
}

fn ms(t: Option<std::time::SystemTime>) -> i64 {
    t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn entry_of(path: &Path, name: &str) -> Option<Entry> {
    let md = std::fs::symlink_metadata(path).ok()?;
    let is_dir = md.is_dir();
    let mut attrs = 0u32;
    if is_dir {
        attrs |= rf_core::entry::attrs::DIRECTORY;
    }
    if name.starts_with('.') {
        attrs |= rf_core::entry::attrs::HIDDEN | rf_core::entry::attrs::SYSTEM;
    }
    let mut e = Entry::new(name, if is_dir { EntryKind::Dir } else { EntryKind::File }, md.len(), ms(md.modified().ok()), ms(md.created().ok()), attrs);
    e.link = md.file_type().is_symlink();
    Some(e)
}

/// Walks `q.root` and streams hits to `on_batch`.
pub fn search(q: &SearchQuery, cancelled: &dyn Fn() -> bool, on_batch: &mut dyn FnMut(Vec<SearchHit>)) -> anyhow::Result<SearchStats> {
    anyhow::ensure!(q.root.is_dir(), "Klasör bulunamadı: {}", q.root.display());
    let matcher = Matcher::new(&q.pattern);
    let mut stats = SearchStats::default();
    let mut hits = 0usize;
    let mut stack = vec![q.root.clone()];
    let mut batch = Vec::new();
    let max = q.max_results.max(1);
    while let Some(dir) = stack.pop() {
        if cancelled() {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            stats.skipped += 1;
            continue;
        };
        stats.dirs += 1;
        for e in rd.flatten() {
            if cancelled() {
                break;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if !q.include_hidden && name.starts_with('.') {
                continue;
            }
            stats.entries += 1;
            let p = e.path();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let is_link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            if is_dir && !is_link {
                stack.push(p.clone());
            }
            if matcher.hit(&name)
                && let Some(entry) = entry_of(&p, &name)
            {
                batch.push(SearchHit { path: p, entry });
                hits += 1;
                if batch.len() >= 256 {
                    on_batch(std::mem::take(&mut batch));
                }
                if hits >= max {
                    break;
                }
            }
        }
        if !batch.is_empty() {
            on_batch(std::mem::take(&mut batch));
        }
    }
    Ok(stats)
}
