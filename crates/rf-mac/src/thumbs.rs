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

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use parking_lot::Mutex;

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

pub fn has_unique_icon(ext: &str) -> bool {
    matches!(ext, "alias" | "dmg" | "pkg" | "mpkg" | "xip" | "icns")
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
    get_image_while(path, size, kind, &|| false)
}

/// [`get_image`] that gives up when `cancel` says the folder changed: a worker stops waiting for
/// Quick Look instead of holding the queue (and a decode) for a picture nobody will see.
fn get_image_while(path: &Path, size: u32, kind: Kind, cancel: &dyn Fn() -> bool) -> anyhow::Result<Bgra> {
    if path.is_dir() {
        if cancel() {
            anyhow::bail!("iptal edildi");
        }
        return icon_for(path, size);
    }
    // Thumbnails: Quick Look has providers for video, PDF, Office, HEIC/RAW and the rest, and
    // draws them the way the Finder does. Icons: the picture of the type is enough.
    if kind == Kind::Thumbnail
        && !cancel()
        && let Ok(t) = quick_look(path, size, cancel)
    {
        return Ok(t);
    }
    // (a full-size decode is the expensive fallback: a navigation between the two steps must not
    // start it)
    if cancel() {
        anyhow::bail!("iptal edildi");
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
fn quick_look(path: &Path, size: u32, cancel: &dyn Fn() -> bool) -> anyhow::Result<Bgra> {
    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2_foundation::{NSString, NSURL};
    use objc2_quick_look_thumbnailing::{
        QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes, QLThumbnailGenerator, QLThumbnailRepresentation,
    };

    let (tx, rx) = std::sync::mpsc::channel::<Option<Bgra>>();
    // The picture we keep is `size` pixels on its longer side: asking Quick Look for a larger
    // representation and downscaling it here would only cost time and memory in quicklookd.
    let points = size.max(16) as f64;
    let request = objc2::rc::autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        unsafe {
            QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
                QLThumbnailGenerationRequest::alloc(),
                &url,
                objc2_foundation::NSSize::new(points, points),
                1.0,
                QLThumbnailGenerationRequestRepresentationTypes::All,
            )
        }
    });
    let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
    // (set when the worker gives up: the callback then skips the bitmap draw instead of making
    // pixels nobody will take)
    let cancelled = std::sync::Arc::new(AtomicBool::new(false));
    let cancelled_in_block = cancelled.clone();
    let block = RcBlock::new(move |rep: *mut QLThumbnailRepresentation, _err: *mut objc2_foundation::NSError| {
        let Some(tx) = tx.lock().unwrap().take() else { return };
        if cancelled_in_block.load(Ordering::Relaxed) {
            return;
        }
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
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Some(b)) => return Ok(b),
            Ok(None) => {
                log::debug!("quicklook: no representation for {}", path.display());
                anyhow::bail!("Quick Look tanıtımı yok")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if cancel() {
                    cancelled.store(true, Ordering::Relaxed);
                    anyhow::bail!("iptal edildi")
                }
                if Instant::now() >= deadline {
                    cancelled.store(true, Ordering::Relaxed);
                    log::debug!("quicklook: no answer for {} within 15 s", path.display());
                    anyhow::bail!("Quick Look yanıt vermedi")
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                cancelled.store(true, Ordering::Relaxed);
                anyhow::bail!("Quick Look yanıt vermedi")
            }
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
    /// Set when the cache no longer wants this picture (its slot was evicted): a worker waiting
    /// on Quick Look stops without making pixels.
    cancelled: Arc<AtomicBool>,
}

impl Request {
    pub fn new(key: u64, path: PathBuf, size: u32, kind: Kind) -> Self {
        Self { key, path, size, kind, urgent: false, queued: Instant::now(), token: 0, cancelled: Arc::new(AtomicBool::new(false)) }
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

/// One queued or running picture: how many calls wait for it, and the flag that stops it when
/// the cache evicts its slot.
struct Job {
    waiters: usize,
    cancelled: Arc<AtomicBool>,
}

/// (key, generation token) → its job.
type InFlight = HashMap<(u64, u64), Job>;

/// Icon/thumbnail workers; the callback runs on a worker thread, never on the UI thread.
///
/// A fixed set of workers and two queues (the preview's urgent one is taken first): a folder of
/// tiles queues behind at most `workers * 32` pictures instead of starting an OS thread per
/// request, which is what made a big folder spike in both CPU and memory (each thread decodes a
/// whole picture at once). The same picture asked for twice is made once; both callers get an
/// answer.
pub struct ThumbPool {
    workers: usize,
    on_done: Arc<dyn Fn(Response) + Send + Sync>,
    pending: Arc<AtomicUsize>,
    /// Bumped by `clear` (a navigation): answers of the old generation are discarded, and no
    /// worker waits on Quick Look / the Finder for a picture nobody will see.
    generation: Arc<AtomicU64>,
    urgent_tx: Sender<Request>,
    urgent_rx: Receiver<Request>,
    normal_tx: Sender<Request>,
    normal_rx: Receiver<Request>,
    inflight: Arc<Mutex<InFlight>>,
    /// The generation, the queues and `inflight` change together: a `clear` must not take a
    /// request that was made after it.
    state: Arc<Mutex<()>>,
}

impl ThumbPool {
    pub fn new(workers: usize, on_done: impl Fn(Response) + Send + Sync + 'static) -> Self {
        keep_gdiplus_alive();
        let (urgent_tx, urgent_rx) = crossbeam_channel::unbounded();
        let (normal_tx, normal_rx) = crossbeam_channel::unbounded();
        let on_done: Arc<dyn Fn(Response) + Send + Sync> = Arc::new(on_done);
        let pending = Arc::new(AtomicUsize::new(0));
        let generation = Arc::new(AtomicU64::new(0));
        let inflight: Arc<Mutex<InFlight>> = Arc::new(Mutex::new(HashMap::new()));
        let workers = workers.max(1);
        for _ in 0..workers {
            let urgent = urgent_rx.clone();
            let normal = normal_rx.clone();
            let on_done = on_done.clone();
            let pending = pending.clone();
            let generation = generation.clone();
            let inflight = inflight.clone();
            let _ = std::thread::Builder::new().name("rf-thumb".into()).spawn(move || {
                worker(urgent, normal, on_done, pending, generation, inflight)
            });
        }
        Self {
            workers,
            on_done,
            pending,
            generation,
            urgent_tx,
            urgent_rx,
            normal_tx,
            normal_rx,
            inflight,
            state: Arc::new(Mutex::new(())),
        }
    }

    /// The current generation; requests are stamped with it.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Queues a picture; the backlog is bounded the same way the Windows pool bounds it.
    /// Returns the generation token the answer will carry.
    pub fn request(&self, mut req: Request) -> u64 {
        let guard = self.state.lock();
        // (read under the same lock as `clear`: a request made after a folder change belongs to
        // the new generation, one made before it is drained by the clear)
        let token = self.generation.load(Ordering::SeqCst);
        req.token = token;
        {
            let mut inflight = self.inflight.lock();
            // (the same picture is already queued or being made: one answer per caller, one decode)
            if let Some(job) = inflight.get_mut(&(req.key, token)) {
                job.waiters += 1;
                return token;
            }
            if self.pending.load(Ordering::Relaxed) >= self.workers * 32 {
                drop(inflight);
                drop(guard);
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
            inflight.insert((req.key, token), Job { waiters: 1, cancelled: req.cancelled.clone() });
            self.pending.fetch_add(1, Ordering::Relaxed);
        }
        let sent = if req.urgent { self.urgent_tx.send(req) } else { self.normal_tx.send(req) };
        if let Err(crossbeam_channel::SendError(req)) = sent {
            // (the workers are gone: the process is ending; nobody waits for an answer)
            self.inflight.lock().remove(&(req.key, token));
            self.pending.fetch_sub(1, Ordering::Relaxed);
        }
        token
    }

    /// The cache dropped this picture's slot (an eviction): no need to make it any more. The
    /// answer is marked dropped, so a slot that comes back asks again.
    pub fn cancel(&self, key: u64, token: u64) {
        if let Some(job) = self.inflight.lock().get(&(key, token)) {
            job.cancelled.store(true, Ordering::Relaxed);
        }
    }

    /// The folder changed: everything still queued or running belongs to the old one.
    pub fn clear(&self) {
        let _guard = self.state.lock();
        self.generation.fetch_add(1, Ordering::SeqCst);
        // Answer the backlog now instead of making each worker walk through it: the cache waits
        // for one answer per ask, and the folder that asked is already gone.
        while let Ok(req) = self.urgent_rx.try_recv() {
            finish(&self.inflight, &self.pending, &*self.on_done, &req, None, true, req.queued.elapsed(), Duration::ZERO);
        }
        while let Ok(req) = self.normal_rx.try_recv() {
            finish(&self.inflight, &self.pending, &*self.on_done, &req, None, true, req.queued.elapsed(), Duration::ZERO);
        }
    }
}

/// A closing window drops its cache: stop the workers waiting on pictures nobody will see
/// (`clear` bumps the generation, and `quick_look` checks it every 100 ms).
impl Drop for ThumbPool {
    fn drop(&mut self) {
        self.clear();
    }
}

/// The next picture for a worker: the preview's first, then the list's.
fn next_request(urgent: &Receiver<Request>, normal: &Receiver<Request>) -> Option<Request> {
    match urgent.try_recv() {
        Ok(r) => return Some(r),
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => return None,
    }
    match normal.try_recv() {
        Ok(r) => return Some(r),
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => return None,
    }
    crossbeam_channel::select_biased! {
        recv(urgent) -> msg => match msg { Ok(r) => Some(r), Err(_) => None },
        recv(normal) -> msg => match msg { Ok(r) => Some(r), Err(_) => None },
    }
}

fn worker(
    urgent: Receiver<Request>,
    normal: Receiver<Request>,
    on_done: Arc<dyn Fn(Response) + Send + Sync>,
    pending: Arc<AtomicUsize>,
    generation: Arc<AtomicU64>,
    inflight: Arc<Mutex<InFlight>>,
) {
    while let Some(req) = next_request(&urgent, &normal) {
        let started = Instant::now();
        // (a folder change while this was queued, or while Quick Look is thinking: nothing to make)
        let cancelled = {
            let generation = generation.clone();
            let token = req.token;
            let flag = req.cancelled.clone();
            move || flag.load(Ordering::Relaxed) || generation.load(Ordering::SeqCst) != token
        };
        let image = if cancelled() {
            None
        } else {
            get_image_while(&req.path, req.size, req.kind, &cancelled).ok().map(Arc::new)
        };
        let dropped = cancelled();
        finish(&inflight, &pending, &*on_done, &req, if dropped { None } else { image }, dropped, started - req.queued, started.elapsed());
    }
}

/// Answers one job: every caller that asked for this picture gets a response (and the shared
/// picture is cloned, not decoded again).
fn finish(
    inflight: &Mutex<InFlight>,
    pending: &AtomicUsize,
    on_done: &(dyn Fn(Response) + Send + Sync),
    req: &Request,
    image: Option<Arc<Bgra>>,
    dropped: bool,
    waited: Duration,
    took: Duration,
) {
    let job = inflight.lock().remove(&(req.key, req.token));
    let waiters = job.as_ref().map_or(1, |j| j.waiters).max(1);
    pending.fetch_sub(1, Ordering::Relaxed);
    for _ in 0..waiters {
        on_done(Response {
            key: req.key,
            image: image.clone(),
            dropped,
            waited,
            took,
            urgent: req.urgent,
            token: req.token,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, fit_size, get_image, get_image_while, straighten_bgra};

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

    use super::{Request, ThumbPool};
    use std::path::PathBuf;
    use std::time::Duration;

    /// A FIFO whose read side blocks the worker in `image::open` until the test writes to it: a
    /// deterministic "a picture is being made" state without depending on how fast a decode is.
    fn blocked_picture(name: &str) -> PathBuf {
        use std::os::unix::ffi::OsStrExt;
        let p = std::env::temp_dir().join(format!("rf-thumb-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        p
    }

    fn release(p: &std::path::Path) {
        // (the worker already has the read side open and is waiting for a writer)
        let mut w = std::fs::OpenOptions::new().write(true).open(p).expect("open the writer side");
        use std::io::Write;
        let _ = w.write_all(&[]);
    }

    fn missing(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rf-thumb-no-such-file-{}-{name}.png", std::process::id()))
    }

    /// The same picture asked for twice is made once, and both callers get an answer: the pool
    /// keeps one job per (key, generation) and counts its waiters.
    #[test]
    fn a_duplicate_request_is_made_once_and_each_caller_is_answered() {
        let path = blocked_picture("dedupe");
        let (tx, rx) = std::sync::mpsc::channel();
        let pool = ThumbPool::new(1, move |r| {
            let _ = tx.send(r);
        });
        let t1 = pool.request(Request::new(7, path.clone(), 64, Kind::Icon));
        let t2 = pool.request(Request::new(7, path.clone(), 64, Kind::Icon));
        assert_eq!(t1, t2, "both asks belong to the same generation");
        std::thread::sleep(Duration::from_millis(50));
        release(&path);
        let r1 = rx.recv_timeout(Duration::from_secs(5)).expect("the first caller's answer");
        let r2 = rx.recv_timeout(Duration::from_secs(5)).expect("the duplicate caller's answer");
        assert_eq!((r1.key, r2.key, r1.token, r2.token), (7, 7, t1, t2));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "one job answers twice, not three times");
        let _ = std::fs::remove_file(&path);
    }

    /// A navigation bumps the generation and answers everything that was queued right away (the
    /// cache waits for one answer per ask); no request is lost, and none is made afterwards.
    #[test]
    fn clearing_a_backlog_answers_every_ask() {
        let (tx, rx) = std::sync::mpsc::channel();
        let pool = ThumbPool::new(1, move |r| {
            let _ = tx.send(r);
        });
        let n = 50;
        for i in 0..n {
            pool.request(Request::new(i as u64, missing(&format!("clear-{i}")), 32, Kind::Icon));
        }
        let old = pool.generation();
        pool.clear();
        assert!(pool.generation() > old);
        let mut got = 0;
        while got < n {
            rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| panic!("only {got} of {n} answers came"));
            got += 1;
        }
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "no answer for a request that was never made");
    }

    /// The preview's picture goes before the list's: with one worker busy, the urgent ask is
    /// taken first when it comes free.
    #[test]
    fn the_urgent_ask_is_taken_before_the_queued_tiles() {
        let blocker = blocked_picture("urgent");
        let (tx, rx) = std::sync::mpsc::channel();
        let pool = ThumbPool::new(1, move |r| {
            let _ = tx.send(r);
        });
        pool.request(Request::new(1, blocker.clone(), 32, Kind::Icon));
        std::thread::sleep(Duration::from_millis(50));
        pool.request(Request::new(2, missing("normal-1"), 32, Kind::Icon));
        pool.request(Request::new(3, missing("normal-2"), 32, Kind::Icon));
        let mut urgent = Request::new(99, missing("urgent"), 384, Kind::Icon);
        urgent.urgent = true;
        pool.request(urgent);
        release(&blocker);
        let first = rx.recv_timeout(Duration::from_secs(5)).expect("the blocker's answer");
        assert_eq!(first.key, 1);
        let next = rx.recv_timeout(Duration::from_secs(5)).expect("the urgent answer");
        assert_eq!((next.key, next.urgent), (99, true), "the pane's picture is not queued behind the list");
        let _ = std::fs::remove_file(&blocker);
    }

    /// A picture whose cache slot was evicted is answered as dropped, so the slot that comes back
    /// asks again instead of showing a stale failure.
    #[test]
    fn an_evicted_request_is_answered_as_dropped() {
        let path = blocked_picture("evict");
        let (tx, rx) = std::sync::mpsc::channel();
        let pool = ThumbPool::new(1, move |r| {
            let _ = tx.send(r);
        });
        let token = pool.request(Request::new(5, path.clone(), 64, Kind::Icon));
        std::thread::sleep(Duration::from_millis(50));
        pool.cancel(5, token);
        release(&path);
        let r = rx.recv_timeout(Duration::from_secs(5)).expect("an answer");
        assert_eq!(r.key, 5);
        assert!(r.dropped, "the cache was told this ask no longer matters");
        let _ = std::fs::remove_file(&path);
    }

    /// A stale ask stops before the expensive fallback decode: the worker is free for the folder
    /// the user is in now (the picture would be thrown away anyway).
    #[test]
    fn a_cancelled_picture_is_not_decoded() {
        // (the FIFO would block forever in `image::open` if the cancel check did not stop first)
        let path = blocked_picture("cancel");
        let started = std::time::Instant::now();
        assert!(get_image_while(&path, 32, Kind::Icon, &|| true).is_err());
        assert!(started.elapsed() < Duration::from_secs(2), "the cancelled ask returned at once");
        let _ = std::fs::remove_file(&path);
    }
}