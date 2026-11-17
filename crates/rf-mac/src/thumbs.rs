//! Icons and thumbnails, the macOS half of `rf-win::thumbs`.
//!
//! Images are decoded and scaled here (the same `image` crate the preview uses). Finder icons
//! and Quick Look thumbnails for documents come with the QuickLookThumbnailing milestone; until
//! then a document without a picture of its own gets the generic icon the UI already has.

use std::path::Path;

/// Straight-alpha BGRA, top-down rows.
#[derive(Clone, Debug)]
pub struct Bgra {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Icon,
    /// Real thumbnail if there is one; otherwise falls back to the icon.
    Thumbnail,
}

/// Windows keeps GDI+ alive for the whole process; macOS has nothing to start.
pub fn keep_gdiplus_alive() {}

/// A folder's own icon (a custom `.VolumeIcon.icns` or an `Icon\r` file) is shown by the UI
/// from the attributes; the macOS icon is a file inside the folder, so there is nothing here.
pub fn folder_has_own_icon(_attrs: u32) -> bool {
    false
}

pub fn has_unique_icon(_ext: &str) -> bool {
    false
}

fn rgba_to_bgra(img: &image::RgbaImage) -> Bgra {
    let (w, h) = img.dimensions();
    let mut data = img.as_raw().clone();
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Bgra { width: w, height: h, data }
}

/// Decodes the picture `path` (an image file) to BGRA, at most `size` pixels on its longer
/// side. `Icon` asks for the file type's picture: without Quick Look, only images have one.
pub fn get_image(path: &Path, size: u32, kind: Kind) -> anyhow::Result<Bgra> {
    let img = image::open(path)?;
    let thumb = img.thumbnail(size.max(1), size.max(1)).to_rgba8();
    match kind {
        Kind::Thumbnail | Kind::Icon => Ok(rgba_to_bgra(&thumb)),
    }
}
// ------------------------------------------------------------------ the worker pool

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One picture to make.
pub struct Request {
    pub key: u64,
    pub path: PathBuf,
    pub size: u32,
    pub kind: Kind,
    /// The picture the user is looking at right now (the preview pane's).
    pub urgent: bool,
    queued: std::time::Instant,
}

impl Request {
    pub fn new(key: u64, path: PathBuf, size: u32, kind: Kind) -> Self {
        Self { key, path, size, kind, urgent: false, queued: std::time::Instant::now() }
    }
}

/// What came of a request.
pub struct Response {
    pub key: u64,
    pub image: Option<Arc<Bgra>>,
    /// Request was discarded (backlog) — not a failure.
    pub dropped: bool,
    pub waited: std::time::Duration,
    pub took: std::time::Duration,
    pub urgent: bool,
}

/// Icon/thumbnail workers; the callback runs on a worker thread.
pub struct ThumbPool {
    workers: usize,
    on_done: Arc<dyn Fn(Response) + Send + Sync>,
    pending: Arc<AtomicUsize>,
}

impl ThumbPool {
    pub fn new(workers: usize, on_done: impl Fn(Response) + Send + Sync + 'static) -> Self {
        keep_gdiplus_alive();
        Self { workers: workers.max(1), on_done: Arc::new(on_done), pending: Arc::new(AtomicUsize::new(0)) }
    }

    /// Queues a picture; the backlog is bounded the same way the Windows pool bounds it.
    pub fn request(&self, req: Request) {
        if self.pending.fetch_add(1, Ordering::Relaxed) >= self.workers * 32 {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            (self.on_done)(Response { key: req.key, image: None, dropped: true, waited: req.queued.elapsed(), took: std::time::Duration::ZERO, urgent: req.urgent });
            return;
        }
        let on_done = self.on_done.clone();
        let pending = self.pending.clone();
        let _ = std::thread::Builder::new().name("rf-thumb".into()).spawn(move || {
            let started = std::time::Instant::now();
            let image = get_image(&req.path, req.size, req.kind).ok().map(Arc::new);
            let response = Response { key: req.key, image, dropped: false, waited: started - req.queued, took: started.elapsed(), urgent: req.urgent };
            pending.fetch_sub(1, Ordering::Relaxed);
            on_done(response);
        });
    }

    /// Nothing to cancel: each request is its own short job.
    pub fn clear(&self) {}
}
