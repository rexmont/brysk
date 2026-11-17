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

/// Decodes a picture file to BGRA, or asks Quick Look / the Finder for a picture of it.
pub fn get_image(path: &Path, size: u32, kind: Kind) -> anyhow::Result<Bgra> {
    if path.is_dir() {
        return icon_for(path, size);
    }
    // Thumbnails: Quick Look has providers for video, PDF, Office, HEIC/RAW and the rest, and
    // draws them the way the Finder does. Icons: the picture of the type is enough.
    if kind == Kind::Thumbnail
        && let Ok(t) = quick_look(path, size)
    {
        return Ok(t);
    }
    match image::open(path) {
        Ok(img) => {
            let thumb = img.thumbnail(size.max(1), size.max(1)).to_rgba8();
            Ok(rgba_to_bgra(&thumb))
        }
        // Not a picture this crate knows: the Finder's icon for the type (an app's own icon, a
        // document's), which is what Explorer shows for a file type too.
        Err(e) => match icon_for(path, size) {
            Ok(icon) => Ok(icon),
            Err(_) => Err(e.into()),
        },
    }
}

/// A Quick Look thumbnail: PDF pages, video frames, Office documents, HEIC/RAW pictures, and
/// anything with a Quick Look provider of its own.
fn quick_look(path: &Path, size: u32) -> anyhow::Result<Bgra> {
    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2_foundation::{NSString, NSURL};
    use objc2_quick_look_thumbnailing::{
        QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes, QLThumbnailGenerator, QLThumbnailRepresentation,
    };

    let (tx, rx) = std::sync::mpsc::channel::<Option<Vec<u8>>>();
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    // 2x: Retina. The size is in points, the picture comes back in pixels.
    let points = size.max(16) as f64;
    let request = unsafe {
        QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
            QLThumbnailGenerationRequest::alloc(),
            &url,
            objc2_foundation::NSSize::new(points, points),
            2.0,
            QLThumbnailGenerationRequestRepresentationTypes::All,
        )
    };
    let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
    let block = RcBlock::new(move |rep: *mut QLThumbnailRepresentation, _err: *mut objc2_foundation::NSError| {
        let Some(tx) = tx.lock().unwrap().take() else { return };
        if rep.is_null() {
            let _ = tx.send(None);
            return;
        }
        let rep = unsafe { &*rep };
        // NSImage is main-thread AppKit: its bytes are made there (the image itself is only
        // carried across as an owned pointer and taken back on the main thread).
        let img = unsafe { rep.NSImage() };
        let img_ptr = objc2::rc::Retained::into_raw(img) as usize;
        let (tx2, rx2) = std::sync::mpsc::channel::<Option<Vec<u8>>>();
        dispatch2::DispatchQueue::main().exec_async(move || {
            let img = unsafe { objc2::rc::Retained::from_raw(img_ptr as *mut objc2_app_kit::NSImage) };
            let data = img.and_then(|i| i.TIFFRepresentation()).map(|d| d.to_vec());
            let _ = tx2.send(data);
        });
        let out = rx2.recv_timeout(std::time::Duration::from_secs(5)).ok().flatten();
        let _ = tx.send(out);
    });
    unsafe {
        QLThumbnailGenerator::sharedGenerator().generateBestRepresentationForRequest_completionHandler(&request, &block);
    }
    let data = match rx.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(Some(d)) => d,
        Ok(None) => {
            log::debug!("quicklook: no representation for {}", path.display());
            anyhow::bail!("Quick Look tanıtımı yok")
        }
        Err(_) => {
            log::debug!("quicklook: no answer for {} within 15 s", path.display());
            anyhow::bail!("Quick Look yanıt vermedi")
        }
    };
    let img = image::load_from_memory(&data)?;
    let thumb = img.thumbnail(size.max(1), size.max(1)).to_rgba8();
    Ok(rgba_to_bgra(&thumb))
}

/// The icon the Finder draws for `path`, through `NSWorkspace` on the main thread.
fn icon_for(path: &Path, size: u32) -> anyhow::Result<Bgra> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{MainThreadMarker, NSString};

    let (tx, rx) = std::sync::mpsc::channel::<Option<Bgra>>();
    let p = path.to_path_buf();
    dispatch2::DispatchQueue::main().exec_async(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.send(None);
            return;
        };
        let ws = NSWorkspace::sharedWorkspace();
        let img = ws.iconForFile(&NSString::from_str(&p.to_string_lossy()));
        // The Finder hands 32×32; ask for the size the list needs before drawing it.
        let s = size.max(16) as f64;
        img.setSize(objc2_foundation::NSSize::new(s, s));
        let out = img
            .TIFFRepresentation()
            .and_then(|data| image::load_from_memory(&data.to_vec()).ok())
            .map(|img| rgba_to_bgra(&img.to_rgba8()));
        let _ = mtm;
        let _ = tx.send(out);
    });
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("ikon alınamadı"))
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
