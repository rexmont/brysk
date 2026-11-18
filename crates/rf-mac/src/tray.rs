//! The menu-bar icon, the macOS half of `rf-win::tray`.
//!
//! `NSStatusItem` with a menu: Open, New Window, Settings, Quit. The menu's actions go through
//! a small Objective-C target object, which calls the handler the app registered. Everything
//! here is main-thread AppKit, dispatched from the caller's worker thread.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSBitmapImageRep, NSImage, NSImageScaling, NSMenu, NSMenuItem, NSStatusBar, NSVariableStatusItemLength};
use objc2_foundation::{MainThreadMarker, NSData, NSObject, NSSize, NSString};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    Open,
    NewWindow,
    Settings,
    Close,
    Quit,
}

#[derive(Clone)]
pub struct Labels {
    pub tooltip: String,
    pub open: String,
    pub new_window: String,
    pub settings: String,
    pub close: String,
    pub quit: String,
}

static HANDLER: Mutex<Option<Box<dyn Fn(TrayEvent) + Send + 'static>>> = Mutex::new(None);
static ITEM: AtomicUsize = AtomicUsize::new(0);
const ICON_WIDTH: u32 = 24;
const ICON_HEIGHT: u32 = 18;

fn mascot_bitmap(backing_scale: u32) -> Option<resvg::tiny_skia::Pixmap> {
    let tree = resvg::usvg::Tree::from_str(include_str!("../assets/menubar-mascot.svg"), &resvg::usvg::Options::default()).ok()?;
    let mut bitmap = resvg::tiny_skia::Pixmap::new(ICON_WIDTH * backing_scale, ICON_HEIGHT * backing_scale)?;
    let (width, height, padding) = (bitmap.width() as f32, bitmap.height() as f32, backing_scale as f32);
    let bounds = tree.root().abs_layer_bounding_box();
    let scale = ((width - 2. * padding) / bounds.width()).min((height - 2. * padding) / bounds.height());
    let x = (width - bounds.width() * scale) / 2. - bounds.x() * scale;
    let y = (height - bounds.height() * scale) / 2. - bounds.y() * scale;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(x, y);
    resvg::render(&tree, transform, &mut bitmap.as_mut());
    Some(bitmap)
}

fn mascot_image() -> Option<Retained<NSImage>> {
    let size = NSSize::new(ICON_WIDTH as f64, ICON_HEIGHT as f64);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    for backing_scale in [1, 2] {
        let png = mascot_bitmap(backing_scale)?.encode_png().ok()?;
        let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &NSData::with_bytes(&png))?;
        rep.setSize(size);
        image.addRepresentation(&rep);
    }
    image.setTemplate(true);
    Some(image)
}

fn fire(event: TrayEvent) {
    let handler = HANDLER.lock().unwrap();
    if let Some(f) = handler.as_ref() {
        f(event);
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class is only made/used on the
    // main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "RFRustyfilesTrayTarget"]
    #[ivars = ()]
    struct TrayTarget;

    unsafe impl NSObjectProtocol for TrayTarget {}

    impl TrayTarget {
        #[unsafe(method(open:))]
        fn open(&self, _sender: &AnyObject) {
            fire(TrayEvent::Open);
        }

        #[unsafe(method(newWindow:))]
        fn new_window(&self, _sender: &AnyObject) {
            fire(TrayEvent::NewWindow);
        }

        #[unsafe(method(settings:))]
        fn settings(&self, _sender: &AnyObject) {
            fire(TrayEvent::Settings);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &AnyObject) {
            fire(TrayEvent::Quit);
        }

        #[unsafe(method(close:))]
        fn close(&self, _sender: &AnyObject) {
            fire(TrayEvent::Close);
        }
    }
);

impl TrayTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm);
        unsafe { msg_send![this, init] }
    }
}

use objc2::runtime::NSObjectProtocol;

fn item(title: &str, action: objc2::runtime::Sel, key: &str, target: &Retained<TrayTarget>, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(title), Some(action), &NSString::from_str(key)) };
    let _: () = unsafe { msg_send![&*item, setTarget: &**target] };
    item
}

/// Shows the menu-bar icon (once).
pub fn show(labels: Labels) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        let Some(mtm) = MainThreadMarker::new() else { return };
        if ITEM.load(Ordering::Relaxed) != 0 {
            return;
        }
        log::info!("tray: creating the menu-bar item");
        let target = TrayTarget::new(mtm);
        let menu = NSMenu::new(mtm);
        menu.setTitle(&NSString::from_str(&labels.tooltip));
        menu.addItem(&item(&labels.open, sel!(open:), "", &target, mtm));
        menu.addItem(&item(&labels.new_window, sel!(newWindow:), "", &target, mtm));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        if !labels.settings.is_empty() {
            menu.addItem(&item(&labels.settings, sel!(settings:), "", &target, mtm));
            menu.addItem(&NSMenuItem::separatorItem(mtm));
        }
        if !labels.close.is_empty() {
            menu.addItem(&item(&labels.close, sel!(close:), "q", &target, mtm));
        }
        menu.addItem(&item(&labels.quit, sel!(quit:), "", &target, mtm));

        let bar = NSStatusBar::systemStatusBar();
        let status = bar.statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = status.button(mtm) {
            if let Some(image) = mascot_image() {
                button.setImageScaling(NSImageScaling::ScaleNone);
                button.setImage(Some(&image));
            }
            button.setToolTip(Some(&NSString::from_str(&labels.tooltip)));
        }
        status.setMenu(Some(&menu));
        // The status bar keeps the item; the menu items' target is weak, so the target is kept
        // for the process lifetime (one small object).
        std::mem::forget(target);
        ITEM.store(Retained::into_raw(status) as usize, Ordering::Relaxed);
        log::info!("tray: menu-bar item shown");
    });
}

/// Takes the icon away.
pub fn hide() {
    dispatch2::DispatchQueue::main().exec_async(|| {
        let raw = ITEM.swap(0, Ordering::Relaxed);
        if raw == 0 {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else { return };
        let item = unsafe { Retained::from_raw(raw as *mut objc2_app_kit::NSStatusItem) };
        if let Some(item) = item {
            NSStatusBar::systemStatusBar().removeStatusItem(&item);
        }
        let _ = mtm;
    });
}

/// The handler for the menu's items (the newest caller wins).
pub fn set_handler(f: impl Fn(TrayEvent) + Send + 'static) {
    *HANDLER.lock().unwrap() = Some(Box::new(f));
}

pub fn notify(_title: &str, _body: &str) {
    // Notifications need UNUserNotificationCenter and the user's permission; later.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mascot_has_padding_at_standard_and_retina_resolutions() {
        for scale in [1, 2] {
            let bitmap = mascot_bitmap(scale).unwrap();
            let (width, height) = (bitmap.width(), bitmap.height());
            assert_eq!((width, height), (24 * scale, 18 * scale));
            let alpha = |x: u32, y: u32| bitmap.data()[((y * width + x) * 4 + 3) as usize];
            assert!(bitmap.data().chunks_exact(4).any(|p| p[3] > 0));
            for x in 0..width {
                for inset in 0..scale {
                    assert_eq!((alpha(x, inset), alpha(x, height - 1 - inset)), (0, 0));
                }
            }
            for y in 0..height {
                assert_eq!((alpha(0, y), alpha(width - 1, y)), (0, 0));
            }
            let occupied_columns = (0..width).filter(|&x| (0..height).any(|y| alpha(x, y) > 0)).count();
            assert!(occupied_columns >= (20 * scale) as usize, "mascot should fill the wider canvas");
        }
    }

    #[test]
    fn mascot_image_preserves_logical_size_and_both_resolutions() {
        let image = mascot_image().unwrap();
        assert!(image.isTemplate());
        assert_eq!(image.size(), NSSize::new(24., 18.));
        let reps = image.representations();
        assert_eq!(reps.len(), 2);
        assert!(reps.iter().all(|rep| rep.size() == NSSize::new(24., 18.)));
    }
}
