//! An opt-in, per-user SMAppService agent owns Finder clicks independently of Brysk's UI.
use objc2::{
    msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool},
};
use objc2_foundation::{NSBundle, NSError, NSString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        net::UnixDatagram,
    },
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    time::Duration,
};

const PLIST: &str = "com.rexmont.brysk.finder-agent.plist";
static ENABLED: AtomicBool = AtomicBool::new(false);
static SUSPENDED: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicU8 = AtomicU8::new(0);
static CONTROL: OnceLock<mpsc::Sender<Command>> = OnceLock::new();
static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
static ENABLED_FILE: OnceLock<PathBuf> = OnceLock::new();
static MARKER: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Status {
    Off,
    NeedsAccessibility,
    Active,
    TapUnavailable,
    AppUnavailable,
    NeedsBackgroundApproval,
    Starting,
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    pid: i32,
    status: Status,
}
enum Command {
    Refresh,
    Permission,
    Suspend(mpsc::Sender<()>),
}

pub fn status() -> Status {
    match STATUS.load(Ordering::Relaxed) {
        1 => Status::NeedsAccessibility,
        2 => Status::Active,
        3 => Status::TapUnavailable,
        4 => Status::AppUnavailable,
        5 => Status::NeedsBackgroundApproval,
        6 => Status::Starting,
        _ => Status::Off,
    }
}
pub fn set_enabled(enabled: bool) {
    if ENABLED.swap(enabled, Ordering::Relaxed) != enabled {
        if !enabled && let Some(directory) = DIRECTORY.get() {
            if let Some(marker) = ENABLED_FILE.get() {
                let _ = write_enabled(marker);
            }
            let _ = UnixDatagram::unbound()
                .and_then(|s| s.send_to(b"s", directory.join("control.sock")));
        }
        retry();
    }
}
pub fn retry() {
    if let Some(tx) = CONTROL.get() {
        let _ = tx.send(Command::Refresh);
    }
}
pub fn request_permission() {
    if let Some(tx) = CONTROL.get() {
        let _ = tx.send(Command::Permission);
    }
}

struct AgentPaths {
    bundle: PathBuf,
    helper: PathBuf,
    directory: PathBuf,
    marker: PathBuf,
}
impl AgentPaths {
    fn current() -> anyhow::Result<Self> {
        let executable = std::env::current_exe()?;
        let contents = executable
            .parent()
            .and_then(Path::parent)
            .filter(|p| p.file_name().is_some_and(|n| n == "Contents"))
            .ok_or_else(|| anyhow::anyhow!("Finder integration needs its app bundle"))?;
        let bundle = contents.parent().unwrap().to_path_buf();
        let info = NSBundle::bundleWithPath(&NSString::from_str(&bundle.to_string_lossy()))
            .ok_or_else(|| anyhow::anyhow!("Missing app bundle"))?;
        let identifier = info
            .bundleIdentifier()
            .ok_or_else(|| anyhow::anyhow!("Missing app identifier"))?
            .to_string();
        validate_agent_identity(
            &fs::read(contents.join("Library/LaunchAgents").join(PLIST))?,
            &identifier,
        )?;
        let directory = runtime_directory(&identifier);
        secure_directory(&directory)?;
        let hash = format!("{:x}", Sha256::digest(identifier.as_bytes()));
        let persistent = dirs::data_local_dir()
            .ok_or_else(|| anyhow::anyhow!("Missing application data directory"))?
            .join("Brysk/FinderAgents");
        fs::create_dir_all(&persistent)?;
        let persistent = persistent.join(&hash[..16]);
        secure_directory(&persistent)?;
        Ok(Self {
            helper: contents.join("MacOS/brysk-finder-agent"),
            bundle,
            directory,
            marker: persistent.join("enabled"),
        })
    }
    fn snapshot(&self) -> Option<Snapshot> {
        use std::io::Read;
        let mut bytes = Vec::new();
        fs::File::open(self.directory.join("status.json"))
            .ok()?
            .take(1025)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > 1024 {
            return None;
        }
        let state: Snapshot = serde_json::from_slice(&bytes).ok()?;
        (crate::single_instance::process_is_stale(state.pid, &self.helper) == Some(false))
            .then_some(state)
    }
    fn send(&self, command: u8) -> bool {
        UnixDatagram::unbound()
            .and_then(|s| s.send_to(&[command], self.directory.join("control.sock")))
            .is_ok()
    }
    fn publish(&self, state: Status) {
        if let Ok(data) = serde_json::to_vec(&Snapshot {
            pid: std::process::id() as i32,
            status: state,
        }) {
            let temporary = self.directory.join("status.tmp");
            if fs::write(&temporary, data).is_ok() {
                let _ = fs::rename(temporary, self.directory.join("status.json"));
            }
        }
    }
}

fn validate_agent_identity(bytes: &[u8], identifier: &str) -> anyhow::Result<()> {
    use objc2_foundation::{
        NSArray, NSData, NSDictionary, NSPropertyListMutabilityOptions, NSPropertyListSerialization,
    };
    let plist = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &NSData::with_bytes(bytes),
            NSPropertyListMutabilityOptions(0),
            std::ptr::null_mut(),
        )
    }
    .map_err(|e| anyhow::anyhow!("Invalid Finder helper plist: {e}"))?;
    let dictionary = plist
        .downcast::<NSDictionary>()
        .map_err(|_| anyhow::anyhow!("Finder helper plist is not a dictionary"))?;
    let label = dictionary
        .objectForKey(&NSString::from_str("Label"))
        .and_then(|v| v.downcast::<NSString>().ok());
    let association = dictionary
        .objectForKey(&NSString::from_str("AssociatedBundleIdentifiers"))
        .and_then(|v| v.downcast::<NSArray>().ok())
        .filter(|ids| ids.count() == 1)
        .and_then(|ids| ids.objectAtIndex(0).downcast::<NSString>().ok());
    // A renamed screenshot/QA bundle must never unregister the installed app's LaunchAgent.
    anyhow::ensure!(
        label.is_some_and(|v| v.to_string() == format!("{identifier}.finder-agent"))
            && association.is_some_and(|v| v.to_string() == identifier),
        "Finder helper identifiers do not match this app bundle"
    );
    Ok(())
}

fn runtime_directory(identifier: &str) -> PathBuf {
    let hash = format!("{:x}", Sha256::digest(identifier.as_bytes()));
    PathBuf::from(format!(
        "/tmp/brysk-finder-{}-{}",
        unsafe { libc::geteuid() },
        &hash[..16]
    ))
}
fn secure_directory(path: &Path) -> std::io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Unsafe Finder helper directory",
        ));
    }
    Ok(())
}

fn write_enabled(marker: &Path) -> std::io::Result<()> {
    let _guard = MARKER.lock().unwrap();
    let temporary = marker.with_extension("tmp");
    fs::write(
        &temporary,
        if ENABLED.load(Ordering::Relaxed) {
            b"1"
        } else {
            b"0"
        },
    )?;
    fs::rename(temporary, marker)
}

fn service_class() -> anyhow::Result<&'static AnyClass> {
    // Loading Apple's framework explicitly also supports the small non-AppKit agent executable.
    unsafe {
        anyhow::ensure!(
            !libc::dlopen(
                c"/System/Library/Frameworks/ServiceManagement.framework/ServiceManagement"
                    .as_ptr(),
                libc::RTLD_NOW | libc::RTLD_LOCAL
            )
            .is_null(),
            "Service Management is unavailable"
        );
    }
    AnyClass::get(c"SMAppService")
        .ok_or_else(|| anyhow::anyhow!("Service Management is unavailable"))
}
fn service() -> anyhow::Result<Retained<AnyObject>> {
    Ok(unsafe {
        msg_send![service_class()?, agentServiceWithPlistName: &*NSString::from_str(PLIST)]
    })
}
fn service_status(service: &AnyObject) -> isize {
    unsafe { msg_send![service, status] }
}
fn register(service: &AnyObject) -> anyhow::Result<()> {
    let mut error: *mut NSError = std::ptr::null_mut();
    let ok: Bool = unsafe { msg_send![service, registerAndReturnError: &mut error] };
    if ok.as_bool() || service_status(service) == 1 {
        return Ok(());
    }
    anyhow::bail!(
        "{}",
        unsafe { error.as_ref() }
            .map(|e| e.localizedDescription().to_string())
            .unwrap_or("Could not register Finder helper".into())
    )
}
fn unregister(service: &AnyObject) -> anyhow::Result<()> {
    if service_status(service) == 0 {
        return Ok(());
    }
    let (tx, rx) = mpsc::channel();
    let done = block2::RcBlock::new(move |error: *mut NSError| {
        let _ = tx.send(unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string()));
    });
    unsafe {
        let _: () = msg_send![service, unregisterWithCompletionHandler: &*done];
    }
    if let Some(error) = rx.recv_timeout(Duration::from_secs(10))? {
        anyhow::bail!("{error}");
    }
    Ok(())
}
pub fn open_background_settings() {
    dispatch2::DispatchQueue::main().exec_async(|| {
        if let Ok(class) = service_class() {
            unsafe {
                let _: () = msg_send![class, openSystemSettingsLoginItems];
            }
        }
    });
}

/// All registration and status I/O stays off the UI thread. Filesystem events redraw the status row.
pub fn start(changed: impl Fn() + Send + 'static) {
    if cfg!(debug_assertions) {
        return;
    }
    CONTROL.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let watcher_tx = tx.clone();
        std::thread::Builder::new()
            .name("finder-service".into())
            .spawn(move || {
                let publish = |state: Status| {
                    if STATUS.swap(state as u8, Ordering::Relaxed) != state as u8 {
                        changed();
                    }
                };
                let paths = match AgentPaths::current() {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Finder service: {e:#}");
                        publish(Status::AppUnavailable);
                        return;
                    }
                };
                let _ = DIRECTORY.set(paths.directory.clone());
                let _ = ENABLED_FILE.set(paths.marker.clone());
                let service = match service() {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("Finder service: {e:#}");
                        publish(Status::TapUnavailable);
                        return;
                    }
                };
                use notify::Watcher;
                let watcher =
                    notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                        if event.is_ok_and(|e| {
                            e.paths
                                .iter()
                                .any(|p| p.file_name().is_some_and(|n| n == "status.json"))
                        }) {
                            let _ = watcher_tx.send(Command::Refresh);
                        }
                    });
                let mut watcher = match watcher {
                    Ok(w) => w,
                    Err(e) => {
                        log::warn!("Finder status watch: {e}");
                        publish(Status::TapUnavailable);
                        return;
                    }
                };
                if watcher
                    .watch(&paths.directory, notify::RecursiveMode::NonRecursive)
                    .is_err()
                {
                    publish(Status::TapUnavailable);
                    return;
                }
                let mut permission = false;
                let mut registered_here = false;
                while let Ok(command) = rx.recv() {
                    if let Command::Suspend(done) = command {
                        SUSPENDED.store(true, Ordering::Relaxed);
                        if let Err(e) = unregister(&service) {
                            log::warn!("Finder helper shutdown: {e:#}");
                        }
                        let _ = done.send(());
                        continue;
                    }
                    if matches!(command, Command::Permission) {
                        permission = true;
                    }
                    if SUSPENDED.load(Ordering::Relaxed) {
                        continue;
                    }
                    let state = (|| -> anyhow::Result<Status> {
                        write_enabled(&paths.marker)?;
                        if !ENABLED.load(Ordering::Relaxed) {
                            permission = false;
                            unregister(&service)?;
                            return Ok(Status::Off);
                        }
                        anyhow::ensure!(
                            paths.helper.is_file(),
                            "The Finder helper is missing from this app bundle"
                        );
                        let snapshot = paths.snapshot();
                        if service_status(&service) == 1 && snapshot.is_none() && !registered_here {
                            unregister(&service)?;
                        }
                        if service_status(&service) != 1 {
                            registered_here = true;
                            register(&service)?;
                        }
                        if service_status(&service) == 2 {
                            return Ok(Status::NeedsBackgroundApproval);
                        }
                        if permission && snapshot.is_some() && paths.send(b'p') {
                            permission = false;
                        } else if snapshot.is_some() {
                            paths.send(b'r');
                        }
                        Ok(paths.snapshot().map_or(Status::Starting, |s| s.status))
                    })();
                    match state {
                        Ok(state) => publish(state),
                        Err(e) => {
                            log::warn!("Finder service: {e:#}");
                            publish(if service_status(&service) == 2 {
                                Status::NeedsBackgroundApproval
                            } else {
                                Status::TapUnavailable
                            });
                        }
                    }
                }
            })
            .expect("Finder service worker");
        let _ = tx.send(Command::Refresh);
        tx
    });
}

/// Call on the install worker before Sparkle replaces the executable. Settings retain the opt-in.
pub fn stop_for_update() {
    if let Some(tx) = CONTROL.get() {
        let (done, wait) = mpsc::channel();
        if tx.send(Command::Suspend(done)).is_ok() {
            let _ = wait.recv_timeout(Duration::from_secs(12));
        }
    }
}
pub fn resume_after_update_failure() {
    SUSPENDED.store(false, Ordering::Relaxed);
    retry();
}

pub fn run_agent() -> anyhow::Result<()> {
    let paths = std::sync::Arc::new(AgentPaths::current()?);
    use std::io::Read;
    let mut enabled = Vec::new();
    if fs::File::open(&paths.marker)
        .and_then(|f| f.take(2).read_to_end(&mut enabled))
        .is_err()
        || enabled != b"1"
    {
        return Ok(());
    }
    // Keep a flock for the lifetime of the agent; never unlink another running agent's socket.
    use std::os::fd::AsRawFd;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.directory.join("agent.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(());
    }
    let socket = paths.directory.join("control.sock");
    let _ = fs::remove_file(&socket);
    let control = UnixDatagram::bind(socket)?;
    let launch_pending = std::sync::Arc::new(AtomicBool::new(false));
    let launch_flag = launch_pending.clone();
    let bundle = paths.bundle.clone();
    let (launch, requests) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        while requests.recv().is_ok() {
            let bundle = bundle.clone();
            let launch_flag = launch_flag.clone();
            dispatch2::DispatchQueue::main().exec_async(move || open_brysk(&bundle, launch_flag));
        }
    });
    std::thread::spawn(move || {
        let mut command = [0u8; 8];
        while let Ok(size) = control.recv(&mut command) {
            if size == 1 {
                match command[0] {
                    b'p' => crate::finder_hook::request_permission(),
                    b'r' => crate::finder_hook::retry(),
                    b's' => std::process::exit(0),
                    _ => {}
                }
            }
        }
    });
    let target = paths.bundle.join("Contents/MacOS/brysk");
    crate::finder_hook::set_enabled(true);
    crate::finder_hook::start(
        target,
        move || {
            if launch_pending.swap(true, Ordering::Relaxed) {
                return true;
            }
            if launch.try_send(()).is_ok() {
                true
            } else {
                launch_pending.store(false, Ordering::Relaxed);
                false
            }
        },
        move |state| {
            let state = match state {
                crate::finder_hook::Status::Off => Status::Off,
                crate::finder_hook::Status::NeedsAccessibility => Status::NeedsAccessibility,
                crate::finder_hook::Status::Active => Status::Active,
                crate::finder_hook::Status::TapUnavailable => Status::TapUnavailable,
                crate::finder_hook::Status::AppUnavailable => Status::AppUnavailable,
            };
            paths.publish(state);
            if state == Status::AppUnavailable {
                std::process::exit(0);
            }
        },
    );
    use core_foundation::{
        base::TCFType,
        runloop::{
            CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, CFRunLoopSourceCreate,
            kCFRunLoopCommonModes,
        },
    };
    extern "C" fn idle(_: *const std::ffi::c_void) {}
    let mut context = CFRunLoopSourceContext {
        version: 0,
        info: std::ptr::null_mut(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: idle,
    };
    let source = unsafe {
        CFRunLoopSource::wrap_under_create_rule(CFRunLoopSourceCreate(
            std::ptr::null(),
            0,
            &mut context,
        ))
    };
    unsafe {
        CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
    }
    CFRunLoop::run_current();
    Ok(())
}

fn open_brysk(bundle: &Path, pending: std::sync::Arc<AtomicBool>) {
    use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
    use objc2_foundation::NSURL;
    let config = NSWorkspaceOpenConfiguration::configuration();
    config.setActivates(true);
    config.setCreatesNewApplicationInstance(false);
    let done = block2::RcBlock::new(move |_: *mut NSRunningApplication, error: *mut NSError| {
        pending.store(false, Ordering::Relaxed);
        if let Some(error) = unsafe { error.as_ref() } {
            log::warn!("Finder redirect launch failed: {error}");
            // A failed launch must not leave the user's Finder click swallowed.
            let _ = crate::shell::open(Path::new("/System/Library/CoreServices/Finder.app"));
        }
    });
    let url = NSURL::fileURLWithPath(&NSString::from_str(&bundle.to_string_lossy()));
    NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
        &url,
        &config,
        Some(&done),
    );
}

#[cfg(test)]
mod tests {
    use super::{runtime_directory, secure_directory};
    #[cfg(debug_assertions)]
    #[test]
    fn debug_app_never_initializes_the_finder_service() {
        super::start(|| panic!("debug must not register a Finder service"));
        assert!(super::CONTROL.get().is_none());
        assert!(super::DIRECTORY.get().is_none());
        assert!(super::ENABLED_FILE.get().is_none());
    }

    #[test]
    fn copied_app_cannot_control_another_bundles_finder_service() {
        let production =
            include_bytes!("../../../packaging/macos/com.rexmont.brysk.finder-agent.plist");
        super::validate_agent_identity(production, "com.rexmont.brysk").unwrap();
        assert!(super::validate_agent_identity(production, "com.rexmont.brysk.diskshot").is_err());
        let qa = std::str::from_utf8(production)
            .unwrap()
            .replace("com.rexmont.brysk", "com.rexmont.brysk.qa");
        super::validate_agent_identity(qa.as_bytes(), "com.rexmont.brysk.qa").unwrap();
        assert!(super::validate_agent_identity(qa.as_bytes(), "com.rexmont.brysk").is_err());
        let wrong_parent = qa.replace(
            "<string>com.rexmont.brysk.qa</string>",
            "<string>com.rexmont.brysk</string>",
        );
        assert!(
            super::validate_agent_identity(wrong_parent.as_bytes(), "com.rexmont.brysk.qa")
                .is_err()
        );
        for invalid in [
            b"not a plist".as_slice(),
            b"<plist><array/></plist>",
            b"<plist><dict/></plist>",
        ] {
            assert!(super::validate_agent_identity(invalid, "com.rexmont.brysk").is_err());
        }
    }

    #[test]
    fn status_rejects_unbounded_payloads_and_unrelated_processes() {
        let directory = runtime_directory(&format!("status-test-{}", std::process::id()));
        secure_directory(&directory).unwrap();
        let paths = super::AgentPaths {
            bundle: directory.clone(),
            helper: std::env::current_exe().unwrap(),
            marker: directory.join("enabled"),
            directory: directory.clone(),
        };
        std::fs::write(
            directory.join("status.json"),
            serde_json::to_vec(&super::Snapshot {
                pid: std::process::id() as i32,
                status: super::Status::Active,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(paths.snapshot().unwrap().status, super::Status::Active);
        std::fs::write(
            directory.join("status.json"),
            b"{\"pid\":-1,\"status\":\"Active\"}",
        )
        .unwrap();
        assert!(paths.snapshot().is_none());
        std::fs::write(directory.join("status.json"), vec![b' '; 1025]).unwrap();
        assert!(paths.snapshot().is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn control_paths_are_short_and_separate_qa_from_the_installed_app() {
        let main = runtime_directory("com.rexmont.brysk");
        assert_ne!(main, runtime_directory("com.rexmont.brysk.qa"));
        assert_eq!(main, runtime_directory("com.rexmont.brysk"));
        assert!(main.join("control.sock").as_os_str().len() < 104);
    }
    #[test]
    fn helper_directory_rejects_symlinks_and_shared_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let base =
            std::env::temp_dir().join(format!("brysk-agent-directory-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base).unwrap();
        let safe = base.join("safe");
        secure_directory(&safe).unwrap();
        let alias = base.join("alias");
        symlink(&safe, &alias).unwrap();
        assert!(secure_directory(&alias).is_err());
        std::fs::set_permissions(&safe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(secure_directory(&safe).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }
}
