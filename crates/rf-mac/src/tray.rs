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
    Quit,
}

#[derive(Clone)]
pub struct Labels {
    pub tooltip: String,
    pub open: String,
    pub new_window: String,
    pub settings: String,
    pub quit: String,
}

static HANDLER: Mutex<Option<Box<dyn Fn(TrayEvent) + Send + 'static>>> = Mutex::new(None);
static ITEM: AtomicUsize = AtomicUsize::new(0);

fn mascot_bitmap(pixels: u32) -> Option<resvg::tiny_skia::Pixmap> {
    let tree = resvg::usvg::Tree::from_str(include_str!("../assets/menubar-mascot.svg"), &resvg::usvg::Options::default()).ok()?;
    let mut bitmap = resvg::tiny_skia::Pixmap::new(pixels, pixels)?;
    let canvas = pixels as f32;
    let available = canvas * 14. / 18.;
    let scale = (available / tree.size().width()).min(available / tree.size().height());
    let x = (canvas - tree.size().width() * scale) / 2.;
    let y = (canvas - tree.size().height() * scale) / 2.;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(x, y);
    resvg::render(&tree, transform, &mut bitmap.as_mut());
    Some(bitmap)
}

fn mascot_image() -> Option<Retained<NSImage>> {
    let size = NSSize::new(18., 18.);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    for pixels in [18, 36] {
        let png = mascot_bitmap(pixels)?.encode_png().ok()?;
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
        menu.addItem(&item(&labels.quit, sel!(quit:), "q", &target, mtm));

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
        for pixels in [18, 36] {
            let bitmap = mascot_bitmap(pixels).unwrap();
            let alpha = |x: u32, y: u32| bitmap.data()[((y * pixels + x) * 4 + 3) as usize];
            assert!(bitmap.data().chunks_exact(4).any(|p| p[3] > 0));
            for n in 0..pixels {
                assert_eq!((alpha(n, 0), alpha(n, pixels - 1), alpha(0, n), alpha(pixels - 1, n)), (0, 0, 0, 0));
                for inset in 0..(2 * pixels / 18) {
                    assert_eq!((alpha(n, inset), alpha(n, pixels - 1 - inset)), (0, 0));
                }
            }
        }
    }

    #[test]
    fn mascot_image_preserves_logical_size_and_both_resolutions() {
        let image = mascot_image().unwrap();
        assert!(image.isTemplate());
        assert_eq!(image.size(), NSSize::new(18., 18.));
        let reps = image.representations();
        assert_eq!(reps.len(), 2);
        assert!(reps.iter().all(|rep| rep.size() == NSSize::new(18., 18.)));
    }
}
