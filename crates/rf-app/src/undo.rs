//! Undo (Ctrl+Z) and redo (Ctrl+Y) for operations done in Brysk. The shell's own undo
//! stack is not scriptable, so we record what each operation did and run the inverse.

use crate::t;
use std::path::PathBuf;

use gpui_kit::*;
use parking_lot::Mutex;
use rf_win::fileops::Transfer;

#[derive(Debug, Clone)]
pub enum UndoOp {
    Renamed { from: PathBuf, to: PathBuf },
    Created { path: PathBuf },
    /// (source, result) pairs reported by the shell.
    Transferred { kind: Transfer, pairs: Vec<(PathBuf, PathBuf)> },
    /// (original path, path inside the Recycle Bin)
    Recycled { pairs: Vec<(PathBuf, PathBuf)> },
    /// "Flatten folder" on `dir`.
    Flattened { dir: PathBuf, rec: crate::ops::Flattened },
    /// One run of the folder organizer (all its moves, renames and recycled files).
    #[cfg(windows)]
    Organized { rec: crate::organizer::Record },
}

static STACK: Mutex<Vec<UndoOp>> = Mutex::new(Vec::new());
static REDO: Mutex<Vec<UndoOp>> = Mutex::new(Vec::new());
const MAX: usize = 50;
/// (source, target) path pairs kept across the whole stack. One operation is never split: an
/// operation larger than this stays undoable by itself, and older ones go instead. Fifty copies
/// of a 100k-file move used to keep ten million paths alive (hundreds of MB); a move of a folder
/// is still undone whole.
const MAX_PAIRS: usize = 20_000;

/// How many paths an operation keeps (the pairs it would move back, or the files it made).
fn pairs(op: &UndoOp) -> usize {
    match op {
        UndoOp::Renamed { .. } | UndoOp::Created { .. } => 1,
        UndoOp::Transferred { pairs, .. } | UndoOp::Recycled { pairs } => pairs.len(),
        UndoOp::Flattened { rec, .. } => rec.moves.len() + rec.removed.len(),
        #[cfg(windows)]
        UndoOp::Organized { rec } => rec.count(),
    }
}

/// Caps a stack: at most [`MAX`] operations and [`MAX_PAIRS`] paths, oldest first. The newest
/// operation is always kept whole (undo of it must work, even if it alone exceeds the budget).
fn trim(stack: &mut Vec<UndoOp>) {
    while stack.len() > MAX {
        stack.remove(0);
    }
    let mut total: usize = stack.iter().map(pairs).sum();
    while stack.len() > 1 && total > MAX_PAIRS {
        total -= pairs(&stack[0]);
        stack.remove(0);
    }
}

/// Records a new user operation (a new operation drops whatever could be redone).
pub fn push(op: UndoOp) {
    REDO.lock().clear();
    let mut s = STACK.lock();
    s.push(op);
    trim(&mut s);
}

/// Keeps an undone operation for Ctrl+Y, under the same caps.
fn push_redo(op: UndoOp) {
    let mut r = REDO.lock();
    r.push(op);
    trim(&mut r);
}

/// A short name for what an operation did, for a menu entry that says what it would undo ("Geri al: Silme").
fn label(op: &UndoOp) -> &'static str {
    match op {
        UndoOp::Renamed { .. } => t!("Yeniden adlandırma"),
        UndoOp::Created { .. } => t!("Oluşturma"),
        UndoOp::Transferred { kind: Transfer::Copy, .. } => t!("Kopyalama"),
        UndoOp::Transferred { kind: Transfer::Move, .. } => t!("Taşıma"),
        UndoOp::Recycled { .. } => t!("Silme"),
        UndoOp::Flattened { .. } => t!("Klasörü düzleştirme"),
        #[cfg(windows)]
        UndoOp::Organized { .. } => t!("Düzenleme"),
    }
}

/// What Ctrl+Z would undo now and what Ctrl+Y would redo (`None`: nothing).
pub fn next_labels() -> (Option<&'static str>, Option<&'static str>) {
    (STACK.lock().last().map(label), REDO.lock().last().map(label))
}

/// Reverses the most recent operation (in the background, through the Status Center).
pub fn undo(cx: &mut App) {
    let Some(op) = STACK.lock().pop() else { return };
    undo_op(op, cx);
}

/// Whether the most recent operation is a copy, move or recycle that made `path` (a copy or
/// move's new item, or the item's place in the Recycle Bin). The cards that offer "Geri al" ask
/// this: the button undoes the top of the stack, so it must be their own operation's record.
pub fn last_made(path: &std::path::Path) -> bool {
    top_made(&STACK.lock(), path)
}

fn top_made(stack: &[UndoOp], path: &std::path::Path) -> bool {
    match stack.last() {
        Some(UndoOp::Transferred { pairs, .. } | UndoOp::Recycled { pairs }) => pairs.iter().any(|(_, made)| made == path),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    // (not `use super::*`: the toolkit's glob brings an attribute named `test` that shadows the real one)
    use super::{MAX, MAX_PAIRS, PathBuf, Transfer, UndoOp, pairs, top_made, trim};

    fn made(n: usize) -> UndoOp {
        UndoOp::Transferred {
            kind: Transfer::Copy,
            pairs: (0..n).map(|i| (PathBuf::from(format!("/tmp/src/{i}")), PathBuf::from(format!("/tmp/dst/{i}")))).collect(),
        }
    }

    #[test]
    fn the_undo_button_of_a_card_needs_its_own_record_on_top_of_the_stack() {
        let made = PathBuf::from(r"C:\rf-test-card\made.bin");
        let copy = UndoOp::Transferred { kind: Transfer::Copy, pairs: vec![(PathBuf::from(r"C:\rf-test-card\src.bin"), made.clone())] };
        let bin = UndoOp::Recycled { pairs: vec![(PathBuf::from(r"C:\rf-test-card\gone.bin"), PathBuf::from(r"C:\$Recycle.Bin\S-1\$R123.bin"))] };
        assert!(!top_made(&[], &made), "an empty stack");
        assert!(top_made(&[copy.clone()], &made));
        assert!(!top_made(&[copy.clone()], std::path::Path::new(r"C:\rf-test-card\src.bin")), "the source is not what the copy made");
        assert!(top_made(std::slice::from_ref(&bin), std::path::Path::new(r"C:\$Recycle.Bin\S-1\$R123.bin")));
        // a newer operation on top: the button would undo that one instead
        assert!(!top_made(&[copy.clone(), UndoOp::Created { path: PathBuf::from(r"C:\rf-test-card\other") }], &made));
        assert!(top_made(&[bin, copy], &made));
    }

    #[test]
    fn the_stack_keeps_fifty_operations() {
        let mut s: Vec<UndoOp> = (0..MAX + 7).map(|i| UndoOp::Created { path: PathBuf::from(format!("/tmp/{i}")) }).collect();
        trim(&mut s);
        assert_eq!(s.len(), MAX);
        assert!(matches!(&s[0], UndoOp::Created { path } if path == std::path::Path::new("/tmp/7")), "the oldest went");
    }

    #[test]
    fn a_huge_operation_is_not_split_and_older_ones_make_room() {
        // ten copies of 5k files: only four fit the 20k-path budget
        let mut s: Vec<UndoOp> = (0..10).map(|_| made(5_000)).collect();
        trim(&mut s);
        assert_eq!(s.len(), MAX_PAIRS / 5_000);
        assert_eq!(s.iter().map(pairs).sum::<usize>(), MAX_PAIRS);
        assert_eq!(s.len(), 4, "the newest four stayed, whole");
        // a single operation bigger than the budget is still undoable
        let mut one = vec![made(MAX_PAIRS * 3)];
        trim(&mut one);
        assert_eq!(one.len(), 1);
        assert_eq!(pairs(&one[0]), MAX_PAIRS * 3);
    }

    #[test]
    fn small_operations_are_kept() {
        let mut s: Vec<UndoOp> = (0..MAX).map(|i| UndoOp::Renamed { from: PathBuf::from(format!("/tmp/a{i}")), to: PathBuf::from(format!("/tmp/b{i}")) }).collect();
        trim(&mut s);
        assert_eq!(s.len(), MAX, "50 single files never reach the path budget");
    }
}

/// Takes one folder organizer run off the undo stack (the "Geri al" button of its Status Center row
/// undoes it itself) and keeps it for Ctrl+Y. False when it is not on the stack: it ran by itself, or
/// 50 newer operations have pushed it off, or it was undone already.
pub fn forget_organized(token: u64) -> bool {
    // (one lock for the search and the removal: a push from another thread cannot slip in between)
    let op = {
        let mut s = STACK.lock();
        #[cfg(windows)]
        let at = s.iter().rposition(|op| matches!(op, UndoOp::Organized { rec } if rec.token == token));
        #[cfg(not(windows))]
        let at: Option<usize> = None;
        at.map(|i| s.remove(i))
    };
    match op {
        Some(op) => {
            push_redo(op);
            true
        }
        None => false,
    }
}

fn undo_op(op: UndoOp, cx: &mut App) {
    // creating an item can't be replayed (its contents may have changed), everything else can
    if !matches!(op, UndoOp::Created { .. }) {
        push_redo(op.clone());
    }
    match op {
        UndoOp::Renamed { from, to } => {
            let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            crate::status_center::start(cx, format!("{} {name}", t!("Geri alınıyor: yeniden adlandırma →")), move |_| {
                rf_win::fileops::rename(&to, &name)
            });
        }
        UndoOp::Created { path } => crate::status_center::delete_ex(cx, vec![path], false, false),
        UndoOp::Transferred { kind: Transfer::Copy, pairs } => {
            crate::status_center::delete_ex(cx, pairs.into_iter().map(|(_, dst)| dst).collect(), false, false)
        }
        UndoOp::Transferred { kind: Transfer::Move, pairs } => {
            #[cfg(target_os = "macos")]
            {
                crate::status_center::start(cx, t!("Geri alınıyor: taşıma").into(), move |_| {
                    for (original, moved) in pairs.into_iter().rev() {
                        rf_win::fastcopy::move_back(&moved, &original)?;
                    }
                    Ok(true)
                });
            }
            #[cfg(not(target_os = "macos"))]
            {
            // Move everything back, grouped by original folder.
            let mut groups: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
            for (src, dst) in pairs {
                if let Some(parent) = src.parent() {
                    groups.entry(parent.to_path_buf()).or_default().push(dst);
                }
            }
            for (dir, items) in groups {
                crate::status_center::transfer_ex(cx, Transfer::Move, items, dir, false);
            }
            }
        }
        UndoOp::Recycled { pairs } => {
            let n = pairs.len();
            crate::status_center::start(cx, format!("{} {}", t!("Geri dönüşüm kutusundan geri yükleniyor:"), crate::i18n::items(n)), move |_| {
                // Directly by path: the Finder's Trash cannot be listed without Full Disk
                // Access, but a known path can be moved back.
                rf_win::recycle::restore_pairs(&pairs)?;
                Ok(true)
            });
        }
        UndoOp::Flattened { dir, rec } => {
            let name = rf_core::history::display_name(&dir);
            crate::status_center::start(cx, format!("{} {name}", t!("Geri alınıyor: düzleştirme →")), move |_| crate::ops::unflatten(&rec));
        }
        #[cfg(windows)]
        UndoOp::Organized { rec } => crate::organizer::undo_run(rec, None, cx),
    }
}

/// Repeats the last undone operation.
pub fn redo(cx: &mut App) {
    let Some(op) = REDO.lock().pop() else { return };
    // it becomes undoable again
    {
        let mut s = STACK.lock();
        s.push(op.clone());
        trim(&mut s);
    }
    match op {
        UndoOp::Renamed { from, to } => {
            let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            crate::status_center::start(cx, format!("{} {name}", t!("Yineleniyor: yeniden adlandırma →")), move |_| {
                rf_win::fileops::rename(&from, &name)
            });
        }
        UndoOp::Created { .. } => {}
        UndoOp::Transferred { kind, pairs } => {
            let mut groups: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
            for (src, dst) in pairs {
                if let Some(parent) = dst.parent() {
                    groups.entry(parent.to_path_buf()).or_default().push(src);
                }
            }
            for (dir, items) in groups {
                crate::status_center::transfer_ex(cx, kind, items, dir, false);
            }
        }
        UndoOp::Recycled { pairs } => {
            crate::status_center::delete_ex(cx, pairs.into_iter().map(|(orig, _)| orig).collect(), false, false)
        }
        UndoOp::Flattened { dir, rec } => {
            let name = rf_core::history::display_name(&dir);
            crate::status_center::start(cx, format!("{} {name}", t!("Yineleniyor: düzleştirme →")), move |_| crate::ops::reflatten(&rec));
        }
        #[cfg(windows)]
        UndoOp::Organized { rec } => crate::organizer::redo_run(rec, cx),
    }
}
