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
    /// Results found (the app shows this count).
    pub hits: usize,
}

struct Matcher {
    glob: Option<Vec<char>>,
    /// The glob's bytes when the whole pattern is ASCII (the common case: no `Vec<char>` per
    /// name walked).
    ascii_glob: Option<Vec<u8>>,
    needle: String,
}

/// Case folding for matching, with the Turkish letters as the plain ones a keyboard without
/// them types: "sarki" finds "Şarkı listesi.txt".
fn fold_into(s: &str, out: &mut String) {
    out.clear();
    out.reserve(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        if c == '\u{307}' {
            continue;
        }
        out.push(match c {
            'ş' => 's',
            'ğ' => 'g',
            'ü' | 'û' => 'u',
            'ö' => 'o',
            'ç' => 'c',
            'ı' | 'î' => 'i',
            'â' | 'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            c => c,
        });
    }
}

fn fold(s: &str) -> String {
    let mut out = String::new();
    fold_into(s, &mut out);
    out
}

impl Matcher {
    fn new(p: &str) -> Matcher {
        let p = fold(p.trim());
        if p.contains('*') || p.contains('?') {
            let glob: Vec<char> = p.chars().collect();
            let ascii_glob = glob.iter().all(|c| c.is_ascii()).then(|| glob.iter().map(|&c| c as u8).collect());
            Matcher { glob: Some(glob), ascii_glob, needle: String::new() }
        } else {
            Matcher { glob: None, ascii_glob: None, needle: p }
        }
    }

    /// Whether `name` matches. `folded` and `chars` are scratch buffers the walker reuses, so a
    /// folder of a hundred thousand names costs no allocation for the ones that don't match.
    fn hit(&self, name: &str, folded: &mut String, chars: &mut Vec<char>) -> bool {
        fold_into(name, folded);
        match &self.glob {
            Some(g) => {
                if let Some(ref spec) = self.ascii_glob
                    && folded.is_ascii()
                {
                    return glob_bytes(spec, folded.as_bytes());
                }
                chars.clear();
                chars.extend(folded.chars());
                glob(g, chars)
            }
            None => folded.contains(&self.needle),
        }
    }
}

/// `*` = any run (including empty), `?` = exactly one. Iterative with backtracking: a long name
/// against a pattern of many stars cannot recurse the stack away.
fn glob(spec: &[char], name: &[char]) -> bool {
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while ni < name.len() {
        if pi < spec.len() && spec[pi] == '*' {
            star = pi;
            mark = ni;
            pi += 1;
        } else if pi < spec.len() && (spec[pi] == '?' || spec[pi] == name[ni]) {
            pi += 1;
            ni += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < spec.len() && spec[pi] == '*' {
        pi += 1;
    }
    pi == spec.len()
}

/// [`glob`] on bytes: both sides are known ASCII, so one byte is one character.
fn glob_bytes(spec: &[u8], name: &[u8]) -> bool {
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while ni < name.len() {
        if pi < spec.len() && spec[pi] == b'*' {
            star = pi;
            mark = ni;
            pi += 1;
        } else if pi < spec.len() && (spec[pi] == b'?' || spec[pi] == name[ni]) {
            pi += 1;
            ni += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < spec.len() && spec[pi] == b'*' {
        pi += 1;
    }
    pi == spec.len()
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
    let mut folded = String::new();
    let mut chars = Vec::new();
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
            if is_dir && !is_link && !rf_core::is_package_name(&name) {
                stack.push(p.clone());
            }
            if matcher.hit(&name, &mut folded, &mut chars)
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
    stats.hits = hits;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::Matcher;

    fn hit(pattern: &str, name: &str) -> bool {
        Matcher::new(pattern).hit(name, &mut String::new(), &mut Vec::new())
    }

    #[test]
    fn names_match_what_was_typed_in_any_case_and_without_turkish_letters() {
        assert!(hit("rapor", "RAPOR 2026.docx"));
        assert!(hit("şarkı", "Şarkı listesi.txt"));
        assert!(hit("sarki", "Şarkı listesi.txt"));
        assert!(hit("ŞARKI", "şarkı listesi.txt"));
        assert!(hit("işlem", "İşlem raporu.txt"));
        assert!(!hit("xyz", "Rapor.txt"));
        assert!(hit("  rapor  ", "rapor.txt"), "the spaces around a query do not count");
    }

    #[test]
    fn a_glob_matches_the_whole_name_and_the_stars_do_not_cost_the_stack() {
        assert!(hit("*.psd", "Kapak.PSD"));
        assert!(hit("a?c", "abc"));
        assert!(!hit("a?c", "ac"));
        assert!(hit("*a*b*", "xxaxxbxx"));
        assert!(!hit("*a*b*", "xxbxxaxx"));
        // (a name that is nothing but stars' complement)
        assert!(hit("*", ""));
        let long = format!("{}b", "a".repeat(400));
        assert!(hit("*a*a*a*a*a*b", &long));
        // non-ASCII names keep character semantics for `?`
        assert!(hit("?本語.txt", "日本語.txt"));
        assert!(!hit("?語.txt", "日本語.txt"));
    }
}
