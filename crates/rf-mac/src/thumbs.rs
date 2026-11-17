//! Icons and thumbnails, the macOS half of `rf-win::thumbs`.
//!
//! Images are decoded and scaled here (the same `image` crate the preview uses). Finder icons
//! come from `NSWorkspace` and document thumbnails (PDF pages, video frames, Office, HEIC) from
//! Quick Look's `QLThumbnailGenerator`.
//!
//! Nothing in this module may run on the UI thread: every picture is made on a pool worker
//! (`get_image` may block for a slow file provider). In particular the AppKit and Quick Look
//! results are turned into pixels on the worker that asked for them: `NSWorkspace` is documented
//! as safe from any thread, an `NSImage` is created and used on one thread only, and the pixels
//! go through a Core Graphics bitmap context (no `TIFFRepresentation` and no main-queue hop).

use std::ffi::c_void;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use core_graphics::base::{kCGBitmapByteOrder32Little, kCGImageAlphaPremultipliedFirst};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};

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
///
/// May block (disk, Quick Look, AppKit): call it on a worker, never on the UI thread.
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

/// The largest size a picture of `iw × ih` takes inside `max_side × max_side` without being
/// blown up past its own size (the rule `image::DynamicImage::thumbnail` follows). Pure.
fn fit_size(iw: u32, ih: u32, max_side: u32) -> (u32, u32) {
    if iw == 0 || ih == 0 {
        return (1, 1);
    }
    let side = f64::from(max_side.max(1));
    let k = (side / f64::from(iw)).min(side / f64::from(ih)).min(1.0);
    (((f64::from(iw) * k).round() as u32).max(1), ((f64::from(ih) * k).round() as u32).max(1))
}

/// Core Graphics hands bitmap pixels premultiplied; the rest of the app (and the `image` crate
/// paths here) use straight alpha. Also the byte order is BGRA at this point. Pure.
fn straighten_bgra(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if a != 255 {
            for c in &mut px[..3] {
                *c = (((u32::from(*c) * 255) + a / 2) / a).min(255) as u8;
            }
        }
    }
}

// The Core Graphics functions used below. They are plain C: a bitmap context and its drawing
// are usable from any thread (unlike AppKit's TIFF encoding).
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        bitmap_info: u32,
    ) -> *mut c_void;
    fn CGBitmapContextGetData(context: *mut c_void) -> *mut c_void;
    fn CGContextClearRect(context: *mut c_void, rect: CGRect);
    fn CGContextDrawImage(context: *mut c_void, rect: CGRect, image: *mut c_void);
    fn CGContextRelease(context: *mut c_void);
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGImageGetWidth(image: *mut c_void) -> usize;
    fn CGImageGetHeight(image: *mut c_void) -> usize;
}

/// Draws a `CGImage` into an exact `width × height` BGRA picture. Runs entirely on the calling
/// thread: a Core Graphics bitmap context is safe off the main thread, unlike AppKit's TIFF
/// encoding. The result has the same top-down row order `Bgra` uses everywhere else.
fn draw_cg_image(ptr: *mut c_void, width: u32, height: u32) -> Option<Bgra> {
    if ptr.is_null() || width == 0 || height == 0 {
        return None;
    }
    // (premultipliedFirst | 32Little = B, G, R, A in memory, which is what `Bgra` wants; the
    // alpha is made straight below)
    unsafe {
        let space = CGColorSpaceCreateDeviceRGB();
        if space.is_null() {
            return None;
        }
        let ctx = CGBitmapContextCreate(
            std::ptr::null_mut(),
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            space,
            kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little,
        );
        CGColorSpaceRelease(space);
        if ctx.is_null() {
            return None;
        }
        let rect = CGRect::new(&CGPoint::new(0., 0.), &CGSize::new(f64::from(width), f64::from(height)));
        // (a fresh bitmap context has no defined background on every OS version)
        CGContextClearRect(ctx, rect);
        CGContextDrawImage(ctx, rect, ptr);
        let mut data = {
            let base = CGBitmapContextGetData(ctx) as *const u8;
            if base.is_null() {
                CGContextRelease(ctx);
                return None;
            }
            std::slice::from_raw_parts(base, width as usize * height as usize * 4).to_vec()
        };
        CGContextRelease(ctx);
        straighten_bgra(&mut data);
        Some(Bgra { width, height, data })
    }
}

/// A Quick Look / Finder picture, at most `max_side` on its longer side (never upscaled).
pub(crate) fn cg_image_to_bgra(ptr: *mut c_void, max_side: u32) -> Option<Bgra> {
    if ptr.is_null() {
        return None;
    }
    let (iw, ih) = unsafe { (CGImageGetWidth(ptr), CGImageGetHeight(ptr)) };
    let (w, h) = fit_size(iw as u32, ih as u32, max_side);
    draw_cg_image(ptr, w, h)
}

/// A `CGImage` at an exact pixel size (PDF pages, whose layout needs the exact box).
pub(crate) fn cg_image_to_bgra_exact(ptr: *mut c_void, width: u32, height: u32) -> Option<Bgra> {
    draw_cg_image(ptr, width.max(1), height.max(1))
}

/// A Quick Look thumbnail: PDF pages, video frames, Office documents, HEIC/RAW pictures, and
/// anything with a Quick Look provider of its own.
///
/// The completion handler runs on a Quick Look worker; the pixels are taken from its `CGImage`
/// there. Nothing is sent to the main queue (which would make the UI thread do the work, and a
/// busy UI thread would then time the request out).
fn quick_look(path: &Path, size: u32) -> anyhow::Result<Bgra> {
    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2_foundation::{NSString, NSURL};
    use objc2_quick_look_thumbnailing::{
        QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes, QLThumbnailGenerator, QLThumbnailRepresentation,
    };

    let (tx, rx) = std::sync::mpsc::channel::<Option<Bgra>>();
    // 2x: Retina. The size is in points, the picture comes back in pixels.
    let points = size.max(16) as f64;
    let request = objc2::rc::autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        unsafe {
            QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
                QLThumbnailGenerationRequest::alloc(),
                &url,
                objc2_foundation::NSSize::new(points, points),
                2.0,
                QLThumbnailGenerationRequestRepresentationTypes::All,
            )
        }
    });
    let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
    let block = RcBlock::new(move |rep: *mut QLThumbnailRepresentation, _err: *mut objc2_foundation::NSError| {
        let Some(tx) = tx.lock().unwrap().take() else { return };
        let out = if rep.is_null() {
            None
        } else {
            let rep = unsafe { &*rep };
            // (Core Graphics only: this callback is on a Quick Look worker, not on the main thread.)
            let cg = unsafe { rep.CGImage() };
            cg_image_to_bgra(Retained::as_ptr(&cg) as *mut c_void, size)
        };
        let _ = tx.send(out);
    });
    unsafe {
        QLThumbnailGenerator::sharedGenerator().generateBestRepresentationForRequest_completionHandler(&request, &block);
    }
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Some(b)) => Ok(b),
        Ok(None) => {
            log::debug!("quicklook: no representation for {}", path.display());
            anyhow::bail!("Quick Look tanıtımı yok")
        }
        Err(_) => {
            log::debug!("quicklook: no answer for {} within 15 s", path.display());
            anyhow::bail!("Quick Look yanıt vermedi")
        }
    }
}

/// The icon the Finder draws for `path`, through `NSWorkspace`. This runs on the calling worker:
/// `NSWorkspace` is documented as safe from any thread, and the `NSImage` it returns is created
/// and consumed on this one (no main-queue round trip, no TIFF encoding on the UI thread).
fn icon_for(path: &Path, size: u32) -> anyhow::Result<Bgra> {
    use objc2::rc::Retained;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSPoint, NSRect, NSSize};

    // (an autorelease pool for this worker thread: the AppKit objects here have no main run loop)
    objc2::rc::autoreleasepool(|_| {
        let ws = NSWorkspace::sharedWorkspace();
        let img = ws.iconForFile(&NSString::from_str(&path.to_string_lossy()));
        // The Finder hands 32×32; ask for the size the list needs before drawing it.
        let s = size.max(16) as f64;
        img.setSize(NSSize::new(s, s));
        let mut rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(s, s));
        if let Some(cg) = unsafe { img.CGImageForProposedRect_context_hints(&mut rect, None, None) }
            && let Some(b) = cg_image_to_bgra(Retained::as_ptr(&cg) as *mut c_void, size)
        {
            return Ok(b);
        }
        // Last resort: the image's TIFF, decoded here (still off the UI thread). TIFF carries every
        // representation, so this is the slow way; the bitmap above is the normal one.
        let data = img.TIFFRepresentation().ok_or_else(|| anyhow::anyhow!("ikon alınamadı"))?;
        let img = image::load_from_memory(&data.to_vec())?;
        let thumb = img.thumbnail(size.max(1), size.max(1)).to_rgba8();
        Ok(rgba_to_bgra(&thumb))
    })
}
// ------------------------------------------------------------------ the worker pool

use std::path::PathBuf;

/// One picture to make.
pub struct Request {
    pub key: u64,
    pub path: PathBuf,
    pub size: u32,
    pub kind: Kind,
    /// The picture the user is looking at right now (the preview pane's).
    pub urgent: bool,
    queued: Instant,
    /// The pool generation this request was made in. A request from an older generation (the
    /// folder changed) is dropped when it comes back, so it can never fill a newer entry.
    token: u64,
}

impl Request {
    pub fn new(key: u64, path: PathBuf, size: u32, kind: Kind) -> Self {
        Self { key, path, size, kind, urgent: false, queued: Instant::now(), token: 0 }
    }
}

/// What came of a request.
pub struct Response {
    pub key: u64,
    pub image: Option<Arc<Bgra>>,
    /// Request was discarded (backlog or navigation) — not a failure.
    pub dropped: bool,
    pub waited: Duration,
    pub took: Duration,
    pub urgent: bool,
    /// The pool generation the request belonged to (`ThumbPool::generation`); a response whose
    /// token is not the one an entry is waiting for belongs to a picture nobody wants any more.
    pub token: u64,
}

/// Icon/thumbnail workers; the callback runs on a worker thread, never on the UI thread.
pub struct ThumbPool {
    workers: usize,
    on_done: Arc<dyn Fn(Response) + Send + Sync>,
    pending: Arc<AtomicUsize>,
    /// Bumped by `clear` (a navigation): answers of the old generation are discarded, and no
    /// worker waits on Quick Look / the Finder for a picture nobody will see.
    generation: Arc<AtomicU64>,
}

impl ThumbPool {
    pub fn new(workers: usize, on_done: impl Fn(Response) + Send + Sync + 'static) -> Self {
        keep_gdiplus_alive();
        Self {
            workers: workers.max(1),
            on_done: Arc::new(on_done),
            pending: Arc::new(AtomicUsize::new(0)),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The current generation; requests are stamped with it.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Queues a picture; the backlog is bounded the same way the Windows pool bounds it.
    /// Returns the generation token the answer will carry.
    pub fn request(&self, mut req: Request) -> u64 {
        let token = self.generation();
        req.token = token;
        if self.pending.fetch_add(1, Ordering::Relaxed) >= self.workers * 32 {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            (self.on_done)(Response {
                key: req.key,
                image: None,
                dropped: true,
                waited: req.queued.elapsed(),
                took: Duration::ZERO,
                urgent: req.urgent,
                token,
            });
            return token;
        }
        let on_done = self.on_done.clone();
        let pending = self.pending.clone();
        let generation = self.generation.clone();
        let _ = std::thread::Builder::new().name("rf-thumb".into()).spawn(move || {
            let started = Instant::now();
            // (a folder change while this was queued: nothing to make)
            let image = if generation.load(Ordering::SeqCst) == token { get_image(&req.path, req.size, req.kind).ok().map(Arc::new) } else { None };
            let dropped = generation.load(Ordering::SeqCst) != token;
            let response = Response {
                key: req.key,
                image: if dropped { None } else { image },
                dropped,
                waited: started - req.queued,
                took: started.elapsed(),
                urgent: req.urgent,
                token,
            };
            pending.fetch_sub(1, Ordering::Relaxed);
            on_done(response);
        });
        token
    }

    /// The folder changed: everything still queued or running belongs to the old one.
    pub fn clear(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, fit_size, get_image, straighten_bgra};

    /// A picture keeps its proportions inside the box and is never enlarged.
    #[test]
    fn a_picture_fits_the_box_without_being_blown_up() {
        assert_eq!(fit_size(1000, 500, 256), (256, 128));
        assert_eq!(fit_size(500, 1000, 256), (128, 256));
        assert_eq!(fit_size(10, 10, 256), (10, 10), "a small picture keeps its size");
        assert_eq!(fit_size(0, 0, 256), (1, 1), "a broken picture still has a pixel");
    }

    /// Core Graphics gives premultiplied BGRA; every other path here gives straight alpha.
    #[test]
    fn premultiplied_pixels_are_made_straight() {
        let mut px = [128u8, 64, 32, 128]; // b, g, r, a (premultiplied)
        straighten_bgra(&mut px);
        assert_eq!(px, [255, 128, 64, 128]);
        let mut clear = [9u8, 9, 9, 0];
        straighten_bgra(&mut clear);
        assert_eq!(clear, [0, 0, 0, 0], "a transparent pixel has no colour");
        let mut opaque = [1u8, 2, 3, 255];
        straighten_bgra(&mut opaque);
        assert_eq!(opaque, [1, 2, 3, 255], "an opaque pixel is left alone");
    }

    /// The icon path runs on the calling worker (no main queue): an icon comes back here.
    #[test]
    fn an_icon_is_made_off_the_main_thread() {
        let path = std::path::Path::new("/System/Library/CoreServices/Finder.app");
        if !path.exists() {
            return;
        }
        // (`cargo test` runs this on a test thread, not on the main thread.)
        let icon = get_image(path, 32, Kind::Icon).unwrap();
        assert!(icon.width > 0 && icon.height > 0, "the icon has pixels");
        assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
    }
}