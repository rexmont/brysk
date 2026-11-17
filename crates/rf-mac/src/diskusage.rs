//! "Show disk usage": a tree of what takes the space, the macOS half of `rf-win::diskusage`.
//!
//! The walk is a plain recursive `readdir` on a background thread (Windows uses a parallel
//! `FindFirstFileEx` walk). The finished tree keeps names, sizes and parent ids and answers the
//! treemap's queries without touching the disk again.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Which size ranks and measures the entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Metric {
    #[default]
    Logical,
    OnDisk,
}

/// What the walk has found so far, readable from any thread while it runs.
#[derive(Default)]
pub struct Progress {
    pub entries: AtomicU64,
    pub bytes: AtomicU64,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Flags {
    #[default]
    None,
    Dir,
}

struct Node {
    name: String,
    parent: u32,
    size: u64,
    alloc: u64,
    files: u32,
    flags: Flags,
    children: Vec<u32>,
}

/// The finished tree. Id 0 is the folder that was walked.
pub struct DiskTree {
    root: PathBuf,
    nodes: Vec<Node>,
    skipped: u32,
    cancelled: bool,
}

impl DiskTree {
    pub fn build(root: &Path, cancel: &AtomicBool, progress: &Progress) -> DiskTree {
        let mut t = DiskTree { root: root.to_path_buf(), nodes: vec![Node { name: root.to_string_lossy().into_owned(), ..Node::default() }], skipped: 0, cancelled: false };
        let mut stack = vec![(root.to_path_buf(), 0u32)];
        while let Some((dir, id)) = stack.pop() {
            if cancel.load(Ordering::Relaxed) {
                t.cancelled = true;
                break;
            }
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => {
                    t.skipped += 1;
                    continue;
                }
            };
            for e in rd.flatten() {
                if cancel.load(Ordering::Relaxed) {
                    t.cancelled = true;
                    break;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
                if md.file_type().is_symlink() {
                    continue;
                }
                let is_dir = md.is_dir();
                let size = if is_dir { 0 } else { md.len() };
                let alloc = md.blocks() as u64 * 512;
                let child = t.nodes.len() as u32;
                t.nodes.push(Node {
                    name,
                    parent: id,
                    size,
                    alloc,
                    files: (!is_dir) as u32,
                    flags: if is_dir { Flags::Dir } else { Flags::None },
                    children: Vec::new(),
                });
                t.nodes[id as usize].children.push(child);
                if is_dir {
                    stack.push((e.path(), child));
                }
                progress.entries.fetch_add(1, Ordering::Relaxed);
                progress.bytes.fetch_add(size, Ordering::Relaxed);
            }
        }
        // totals bottom-up, by id (children always have larger ids than their parent)
        for i in (0..t.nodes.len()).rev() {
            let (size, alloc, files, parent) = {
                let n = &t.nodes[i];
                (n.size, n.alloc, n.files, n.parent)
            };
            if i > 0 {
                t.nodes[parent as usize].size += size;
                t.nodes[parent as usize].alloc += alloc;
                t.nodes[parent as usize].files += files;
            }
        }
        t
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    pub fn cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn skipped(&self) -> u32 {
        self.skipped
    }

    pub fn memory(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Node>() + self.nodes.iter().map(|n| n.name.len() + n.children.capacity() * 4).sum::<usize>()
    }

    pub fn name(&self, i: u32) -> &str {
        &self.nodes[i as usize].name
    }

    pub fn is_dir(&self, i: u32) -> bool {
        self.nodes[i as usize].flags == Flags::Dir
    }

    pub fn parent(&self, i: u32) -> Option<u32> {
        (i != 0).then_some(self.nodes[i as usize].parent)
    }

    pub fn size(&self, i: u32) -> u64 {
        self.nodes[i as usize].size
    }

    pub fn allocated(&self, i: u32) -> u64 {
        self.nodes[i as usize].alloc
    }

    pub fn weight(&self, i: u32, m: Metric) -> u64 {
        match m {
            Metric::Logical => self.size(i),
            Metric::OnDisk => self.allocated(i),
        }
    }

    pub fn files(&self, i: u32) -> u32 {
        self.nodes[i as usize].files
    }

    pub fn children(&self, i: u32) -> std::ops::Range<u32> {
        match self.nodes[i as usize].children.split_first() {
            Some((first, _)) => *first..(*first + self.nodes[i as usize].children.len() as u32),
            None => 0..0,
        }
    }

    /// The id of `i`'s `nth` child.
    pub fn child_at(&self, i: u32, nth: u32) -> u32 {
        self.nodes[i as usize].children[nth as usize]
    }
}

impl Default for Node {
    fn default() -> Self {
        Node { name: String::new(), parent: 0, size: 0, alloc: 0, files: 0, flags: Flags::None, children: Vec::new() }
    }
}

/// A running scan.
pub struct DiskScan {
    cancel: Arc<AtomicBool>,
    progress: Arc<Progress>,
    result: Arc<Mutex<Option<DiskTree>>>,
}

impl DiskScan {
    pub fn start(root: PathBuf) -> DiskScan {
        let scan = DiskScan { cancel: Arc::default(), progress: Arc::default(), result: Arc::default() };
        let (cancel, progress, result) = (scan.cancel.clone(), scan.progress.clone(), scan.result.clone());
        let _ = std::thread::Builder::new().name("rf-diskusage".into()).spawn(move || {
            let t = DiskTree::build(&root, &cancel, &progress);
            *result.lock().unwrap() = Some(t);
        });
        scan
    }

    /// (entries, bytes) found so far.
    pub fn progress(&self) -> (u64, u64) {
        (self.progress.entries.load(Ordering::Relaxed), self.progress.bytes.load(Ordering::Relaxed))
    }

    /// The tree once the walk has finished (taken: later calls return `None`).
    pub fn take(&self) -> Option<DiskTree> {
        self.result.lock().unwrap().take()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for DiskScan {
    fn drop(&mut self) {
        self.cancel();
    }
}