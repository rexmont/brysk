//! The menu-bar icon, the macOS half of `rf-win::tray`.
//!
//! `NSStatusItem` with a menu: Open, New Window, Settings, Quit. The menu's actions go through
//! a small Objective-C target object, which calls the handler the app registered. Everything
//! here is main-thread AppKit, dispatched from the caller's worker thread.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSFontWeightMedium, NSImage, NSImageScaling, NSImageSymbolConfiguration, NSImageSymbolScale, NSMenu, NSMenuItem, NSStatusBar, NSVariableStatusItemLength};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};

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
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str("folder"), None) {
                // Let AppKit render the vector at menu-bar size and the display's backing scale.
                let config = NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(18., unsafe { NSFontWeightMedium }, NSImageSymbolScale::Medium);
                button.setSymbolConfiguration(Some(&config));
                button.setImageScaling(NSImageScaling::ScaleNone);
                image.setTemplate(true);
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
