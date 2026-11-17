//! Tags as a GPUI global, persisted to %APPDATA%\Brysk\tags.json.

use std::path::PathBuf;

use gpui_kit::*;
use rf_core::tags::TagStore;

pub struct Tags(pub TagStore);
impl Global for Tags {}

fn path() -> Option<PathBuf> {
    crate::settings::app_dir().map(|d| d.join("tags.json"))
}

pub fn load() -> Tags {
    Tags(path().map(|p| TagStore::load(&p)).unwrap_or_default())
}

pub fn update(cx: &mut App, f: impl FnOnce(&mut TagStore)) {
    cx.update_global::<Tags, _>(|t, _| f(&mut t.0));
    let snapshot = cx.global::<Tags>().0.clone();
    write_finder(&snapshot);
    save_snapshot(snapshot);
    cx.refresh_windows();
}

fn save_snapshot(snapshot: TagStore) {
    std::thread::spawn(move || {
        if let Some(p) = path() {
            if let Err(e) = snapshot.save(&p) {
                log::warn!("tags save: {e}");
            }
        }
    });
}

fn persist(cx: &App) {
    save_snapshot(cx.global::<Tags>().0.clone());
}

// ---------------------------------------------------------------- Finder tags (macOS)

/// The Finder's tag colours, by index (1 grey … 7 orange), with the app's palette colours.
#[cfg(target_os = "macos")]
const FINDER_COLORS: [(u8, u32); 7] = [
    (1, 0x8E8E93),
    (2, 0x30D158),
    (3, 0xBF5AF2),
    (4, 0x0A84FF),
    (5, 0xFFD60A),
    (6, 0xFF453A),
    (7, 0xFF9F0A),
];

#[cfg(target_os = "macos")]
fn finder_index(color: u32) -> u8 {
    let d = |a: u32, b: u32| {
        let (ar, ag, ab) = ((a >> 16) & 0xFF, (a >> 8) & 0xFF, a & 0xFF);
        let (br, bg, bb) = ((b >> 16) & 0xFF, (b >> 8) & 0xFF, b & 0xFF);
        (ar as i64 - br as i64).pow(2) + (ag as i64 - bg as i64).pow(2) + (ab as i64 - bb as i64).pow(2)
    };
    FINDER_COLORS.iter().min_by_key(|(_, c)| d(*c, color)).map(|(i, _)| *i).unwrap_or(6)
}

#[cfg(target_os = "macos")]
fn finder_color(index: u8) -> u32 {
    FINDER_COLORS.iter().find(|(i, _)| *i == index).map(|(_, c)| *c).unwrap_or(0xC7C7CC)
}

/// Writes every tagged item's tags to the Finder's `_kMDItemUserTags` attribute.
#[cfg(target_os = "macos")]
fn write_finder(store: &TagStore) {
    let store = store.clone();
    std::thread::spawn(move || {
        for item in store.items.values() {
            let tags: Vec<(String, u8)> = item
                .tags
                .iter()
                .filter_map(|id| store.def(*id).map(|d| (d.name.clone(), finder_index(d.color))))
                .collect();
            if let Err(e) = rf_win::tags::write(&item.path, &tags) {
                log::debug!("finder tags: {}: {e}", item.path.display());
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn write_finder(_store: &TagStore) {}

/// Merges the Finder tags of `paths` into the app's tags (so a tag given in Finder shows in
/// the tag menu and the tag views). Capped: a menu on a thousand files stays quick.
#[cfg(target_os = "macos")]
pub fn import_finder(cx: &mut App, paths: &[std::path::PathBuf]) {
    let mut changed = false;
    cx.update_global::<Tags, _>(|t, _| {
        for p in paths.iter().take(200) {
            for (name, index) in rf_win::tags::read(p) {
                let id = t
                    .0
                    .defs
                    .iter()
                    .find(|d| d.name.eq_ignore_ascii_case(&name))
                    .map(|d| d.id)
                    .unwrap_or_else(|| t.0.add(&name, finder_color(index)));
                if !t.0.has(p, id) {
                    t.0.toggle(std::slice::from_ref(p), id);
                    changed = true;
                }
            }
        }
    });
    if changed {
        persist(cx);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn import_finder(_cx: &mut App, _paths: &[std::path::PathBuf]) {}

pub fn color(c: u32) -> Hsla {
    rgb(c).into()
}

/// How a tag joins the result of a tag query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join {
    /// Items that have this tag too.
    And,
    /// Also the items that have this tag.
    Or,
    /// Without the items that have this tag.
    Not,
}

/// The tags of a query like `kırmızı AND iş`, `kırmızı OR mavi NOT bitti` (after `tag:`), in
/// order. Operators are AND / OR / NOT, VE / VEYA / DEĞİL (any case) or `&`, `|`, `!`; a tag
/// name may have spaces. The first tag joins with OR unless the query starts with NOT.
/// `text` is case-folded already.
pub fn parse_query(text: &str) -> Vec<(Join, String)> {
    let mut out: Vec<(Join, String)> = Vec::new();
    let mut join = Join::Or;
    let mut name = String::new();
    let flush = |join: Join, name: &mut String, out: &mut Vec<(Join, String)>| {
        if !name.is_empty() {
            out.push((join, std::mem::take(name)));
        }
    };
    for word in text.split_whitespace() {
        let op = match word {
            "and" | "ve" | "&" | "&&" => Some(Join::And),
            "or" | "veya" | "|" | "||" => Some(Join::Or),
            "not" | "değil" | "degil" | "!" => Some(Join::Not),
            _ => None,
        };
        match op {
            Some(op) => {
                flush(join, &mut name, &mut out);
                join = op;
            }
            None => {
                if !name.is_empty() {
                    name.push(' ');
                }
                name.push_str(word);
            }
        }
    }
    flush(join, &mut name, &mut out);
    out
}

/// The paths a tag query gives: the terms are applied left to right (no brackets). `with`
/// gives the paths of a tag by its (case-folded) name, empty for a name no tag has; `all` is
/// every tagged path (what a query starting with NOT takes from).
pub fn eval_query(terms: &[(Join, String)], with: &dyn Fn(&str) -> Vec<PathBuf>, all: &dyn Fn() -> Vec<PathBuf>) -> Vec<PathBuf> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<PathBuf> = BTreeSet::new();
    for (i, (join, name)) in terms.iter().enumerate() {
        let tagged: BTreeSet<PathBuf> = with(name).into_iter().collect();
        match join {
            Join::Or => set.extend(tagged),
            Join::And => set.retain(|p| tagged.contains(p)),
            Join::Not => {
                if i == 0 {
                    set = all().into_iter().collect();
                }
                set.retain(|p| !tagged.contains(p));
            }
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{Join, eval_query, parse_query};
    use std::path::PathBuf;

    #[test]
    fn tag_queries_with_and_or_not() {
        assert_eq!(parse_query("kırmızı"), vec![(Join::Or, "kırmızı".into())]);
        assert_eq!(parse_query("kırmızı and iş dosyası"), vec![(Join::Or, "kırmızı".into()), (Join::And, "iş dosyası".into())]);
        assert_eq!(parse_query("kırmızı veya mavi değil bitti"), vec![(Join::Or, "kırmızı".into()), (Join::Or, "mavi".into()), (Join::Not, "bitti".into())]);
        assert_eq!(parse_query("not bitti"), vec![(Join::Not, "bitti".into())]);
        assert_eq!(parse_query("  "), vec![]);

        let p = |s: &str| PathBuf::from(s);
        let with = |name: &str| -> Vec<PathBuf> {
            match name {
                "kırmızı" => vec![p("a"), p("b")],
                "mavi" => vec![p("b"), p("c")],
                "bitti" => vec![p("c")],
                _ => Vec::new(),
            }
        };
        let all = || vec![p("a"), p("b"), p("c")];
        let run = |q: &str| eval_query(&parse_query(q), &with, &all);
        assert_eq!(run("kırmızı"), vec![p("a"), p("b")]);
        assert_eq!(run("kırmızı and mavi"), vec![p("b")]);
        assert_eq!(run("kırmızı or mavi"), vec![p("a"), p("b"), p("c")]);
        assert_eq!(run("kırmızı or mavi not bitti"), vec![p("a"), p("b")]);
        assert_eq!(run("not kırmızı"), vec![p("c")]);
        // a name no tag has: nothing with AND, no change with OR / NOT
        assert!(run("kırmızı and yok").is_empty());
        assert_eq!(run("kırmızı or yok"), vec![p("a"), p("b")]);
    }
}
