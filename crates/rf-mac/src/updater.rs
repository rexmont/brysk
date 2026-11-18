//! Sparkle's standard updater, loaded from the signed app bundle. Main thread only.
use std::{cell::RefCell, ffi::CString, sync::Mutex};

use block2::{Block, RcBlock};
use objc2::{
    MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool},
};
use objc2_foundation::{MainThreadMarker, NSError, NSObject, NSObjectProtocol, NSString};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Available(String),
    UpToDate,
    Failed(String),
    SessionFinished,
    InstallRequested,
}

static HANDLER: Mutex<Option<Box<dyn Fn(Event) + Send>>> = Mutex::new(None);
thread_local! {
    static UPDATER: RefCell<Option<(Retained<AnyObject>, Retained<Delegate>)>> = const { RefCell::new(None) };
    static INSTALL: RefCell<Option<RcBlock<dyn Fn()>>> = const { RefCell::new(None) };
}

fn emit(event: Event) {
    if let Some(handler) = HANDLER.lock().unwrap().as_ref() {
        handler(event);
    }
}

fn version(item: &AnyObject) -> String {
    // SAFETY: SUAppcastItem's documented string property, only on Sparkle's main-thread callbacks.
    let version: Retained<NSString> = unsafe { msg_send![item, displayVersionString] };
    version.to_string()
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "BryskUpdaterDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    impl Delegate {
        #[unsafe(method(supportsGentleScheduledUpdateReminders))]
        fn gentle(&self) -> bool { true }

        #[unsafe(method(standardUserDriverShouldHandleShowingScheduledUpdate:andInImmediateFocus:))]
        fn should_show(&self, _item: &AnyObject, _immediate: bool) -> bool { false }

        #[unsafe(method(standardUserDriverWillHandleShowingUpdate:forUpdate:state:))]
        fn will_show(&self, _handled: bool, item: &AnyObject, _state: &AnyObject) {
            emit(Event::Available(version(item)));
        }

        #[unsafe(method(updater:didFindValidUpdate:))]
        fn found(&self, _updater: &AnyObject, item: &AnyObject) {
            emit(Event::Available(version(item)));
        }

        #[unsafe(method(updaterDidNotFindUpdate:error:))]
        fn not_found(&self, _updater: &AnyObject, _error: &NSError) { emit(Event::UpToDate); }

        #[unsafe(method(updater:didAbortWithError:))]
        fn failed(&self, _updater: &AnyObject, error: &NSError) {
            // No update and cancelled authorization are normal outcomes, not failures.
            if !matches!(error.code(), 1001 | 4007 | 4008) {
                emit(Event::Failed(error.localizedDescription().to_string()));
            }
        }

        #[unsafe(method(standardUserDriverWillFinishUpdateSession))]
        fn finished(&self) { emit(Event::SessionFinished); }

        #[unsafe(method(updater:shouldPostponeRelaunchForUpdate:untilInvokingBlock:))]
        fn postpone(&self, _updater: &AnyObject, _item: &AnyObject, install: &Block<dyn Fn()>) -> bool {
            INSTALL.with(|pending| *pending.borrow_mut() = Some(install.copy()));
            emit(Event::InstallRequested);
            true
        }
    }
);

/// Returns false for an unpackaged Cargo binary; it cannot replace an app bundle.
pub fn start(handler: impl Fn(Event) + Send + 'static) -> Result<bool, String> {
    let mtm = MainThreadMarker::new().ok_or("Updater must start on the main thread")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let contents = exe
        .parent()
        .and_then(|p| p.parent())
        .ok_or("Missing app bundle")?;
    if contents.file_name().is_none_or(|name| name != "Contents") {
        return Ok(false);
    }
    let library = contents.join("Frameworks/Sparkle.framework/Sparkle");
    let library =
        CString::new(library.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
    // Keep the framework loaded for the process lifetime: Objective-C classes refer into it.
    unsafe {
        if libc::dlopen(library.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL).is_null() {
            return Err("The bundled update framework could not be loaded.".into());
        }
    }
    let class = AnyClass::get(c"SPUStandardUpdaterController").ok_or("Missing Sparkle updater")?;
    let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(mtm), init] };
    *HANDLER.lock().unwrap() = Some(Box::new(handler));
    // SAFETY: signatures are from the pinned Sparkle framework's public headers.
    unsafe {
        let allocated: objc2::rc::Allocated<AnyObject> = msg_send![class, alloc];
        let controller: Retained<AnyObject> = msg_send![allocated, initWithStartingUpdater: false,
            updaterDelegate: &*delegate, userDriverDelegate: &*delegate];
        let updater: Retained<AnyObject> = msg_send![&*controller, updater];
        let mut error: *mut NSError = std::ptr::null_mut();
        let started: Bool = msg_send![&*updater, startUpdater: &mut error];
        if !started.as_bool() {
            return Err(error
                .as_ref()
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or("Updates could not start.".into()));
        }
        UPDATER.with(|slot| *slot.borrow_mut() = Some((controller, delegate)));
    }
    Ok(true)
}

pub fn check() -> bool {
    UPDATER.with(|slot| {
        if let Some((controller, _)) = slot.borrow().as_ref() {
            // Also brings an existing update/download window back into focus.
            unsafe {
                let _: () =
                    msg_send![&**controller, checkForUpdates: std::ptr::null::<AnyObject>()];
            }
            return true;
        }
        false
    })
}

pub fn automatic_checks() -> bool {
    UPDATER.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|(controller, _)| unsafe {
                let updater: Retained<AnyObject> = msg_send![&**controller, updater];
                let on: Bool = msg_send![&*updater, automaticallyChecksForUpdates];
                on.as_bool()
            })
    })
}

pub fn set_automatic_checks(on: bool) {
    UPDATER.with(|slot| {
        if let Some((controller, _)) = slot.borrow().as_ref() {
            unsafe {
                let updater: Retained<AnyObject> = msg_send![&**controller, updater];
                let _: () = msg_send![&*updater, setAutomaticallyChecksForUpdates: on];
            }
        }
    });
}

/// Called after Brysk has saved the session and all active file operations have finished.
pub fn install() {
    let block = INSTALL.with(|pending| pending.borrow_mut().take());
    if let Some(block) = block {
        block.call(());
    }
}
