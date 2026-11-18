//! Single instance and the small message pipe, the macOS half of `rf-win::single_instance`.
//!
//! Windows uses a named mutex plus a named pipe. macOS has no named objects; a Unix-domain
//! socket in the user's temporary directory gives the same contract: one instance binds it,
//! a second one connects and forwards its request, the first one's window handles it.

use std::path::PathBuf;

pub enum Role {
    /// This process is the first instance; the guard removes the socket when it ends.
    Primary(InstanceGuard),
    /// Another instance is running and received the message.
    Forwarded,
}

pub struct InstanceGuard {
    path: PathBuf,
}

impl InstanceGuard {
    pub fn new(path: PathBuf) -> Self {
        InstanceGuard { path }
    }
}
unsafe impl Send for InstanceGuard {}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Separate default channels for debug and release; `RUSTYFILES_INSTANCE` can override either.
pub fn instance_name() -> String {
    match std::env::var("RUSTYFILES_INSTANCE") {
        Ok(n) if !n.is_empty() => format!("Brysk-{n}"),
        _ if cfg!(debug_assertions) => "Brysk-Debug".to_string(),
        _ => "Brysk".to_string(),
    }
}

pub(crate) fn socket_path(app: &str) -> PathBuf {
    let uid = unsafe { libc::getuid() };
    std::env::temp_dir().join(format!("{app}-{uid}.sock"))
}

/// Sends one message to the instance that owns `app`'s socket. False when nobody listens.
pub fn send_to(app: &str, message: &str) -> bool {
    match std::os::unix::net::UnixStream::connect(socket_path(app)) {
        Ok(mut s) => {
            use std::io::Write;
            let bytes = message.as_bytes();
            let ok = s.write_all(&(bytes.len() as u32).to_be_bytes()).is_ok() && s.write_all(bytes).is_ok();
            let _ = s.flush();
            ok
        }
        Err(_) => false,
    }
}

/// Whether the live helper uses another bundle, or predates its executable on disk.
/// Called on the IPC worker, before sending to a helper that may have survived an app update.
pub fn peer_is_stale(app: &str, executable: &std::path::Path) -> Option<bool> {
    use std::os::fd::AsRawFd;
    let stream = std::os::unix::net::UnixStream::connect(socket_path(app)).ok()?;
    let mut pid = 0i32;
    let mut len = std::mem::size_of_val(&pid) as libc::socklen_t;
    if unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut i32).cast(), &mut len) } != 0 {
        return None;
    }
    process_is_stale(pid, executable)
}

pub(crate) fn process_is_stale(pid: i32, executable: &std::path::Path) -> Option<bool> {
    use std::os::unix::ffi::OsStrExt;
    let mut path = [0u8; 4096];
    if unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) } <= 0 { return None; }
    let path = std::ffi::CStr::from_bytes_until_nul(&path).ok()?;
    let running = std::path::Path::new(std::ffi::OsStr::from_bytes(path.to_bytes()));
    if running.canonicalize().ok()? != executable.canonicalize().ok()? { return Some(true); }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let len = std::mem::size_of_val(&info) as i32;
    if unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), len) } != len { return None; }
    let info = unsafe { info.assume_init() };
    let started = std::time::UNIX_EPOCH + std::time::Duration::new(info.pbi_start_tvsec, info.pbi_start_tvusec as u32 * 1000);
    Some(std::fs::metadata(executable).ok()?.modified().ok()? > started)
}

#[cfg(test)]
mod freshness_tests {
    #[cfg(debug_assertions)]
    #[test]
    fn debug_app_does_not_use_the_release_socket() {
        assert_ne!(super::socket_path(&super::instance_name()), super::socket_path("Brysk"));
    }

    #[test]
    fn finds_the_process_behind_a_live_helper_socket() {
        let app = format!("brysk-freshness-test-{}", std::process::id());
        let path = super::socket_path(&app);
        let _cleanup = super::InstanceGuard::new(path.clone());
        let _listener = std::os::unix::net::UnixListener::bind(path).unwrap();
        assert_eq!(super::peer_is_stale(&app, &std::env::current_exe().unwrap()), Some(false));
    }

    #[test]
    fn recognizes_its_current_executable_and_a_different_bundle() {
        let pid = std::process::id() as i32;
        assert_eq!(super::process_is_stale(pid, &std::env::current_exe().unwrap()), Some(false));
        assert_eq!(super::process_is_stale(pid, std::path::Path::new("/System/Library/CoreServices/Finder.app")), Some(true));
        assert_eq!(super::process_is_stale(-1, &std::env::current_exe().unwrap()), None);
    }
}

/// Receives messages meant for `app` (a socket of its own; used by the search pipe).
pub fn serve_app(app: &str, on_message: impl Fn(String) + Send + Sync + 'static) {
    let path = socket_path(app);
    let _ = std::fs::remove_file(&path);
    let listener = match std::os::unix::net::UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("single instance: cannot listen on {}: {e}", path.display());
            return;
        }
    };
    let on_message = std::sync::Arc::new(on_message);
    let _ = std::thread::Builder::new().name("rf-ipc".into()).spawn(move || {
        for conn in listener.incoming().flatten() {
            let on_message = on_message.clone();
            let _ = std::thread::Builder::new().name("rf-ipc-msg".into()).spawn(move || {
                if let Some(msg) = read_message(conn) {
                    on_message(msg);
                }
            });
        }
    });
}

pub(crate) fn read_message(mut s: impl std::io::Read) -> Option<String> {
    use std::io::Read;
    let mut len = [0u8; 4];
    s.read_exact(&mut len).ok()?;
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}
/// [`acquire`](crate::handover::acquire) for another application of the bundle (DirectLook,
/// the search pipe).
pub fn acquire_app(app: &str, message: &str) -> Role {
    if send_to(app, message) {
        return Role::Forwarded;
    }
    let path = socket_path(app);
    let _ = std::fs::remove_file(&path);
    if let Ok(listener) = std::os::unix::net::UnixListener::bind(&path) {
        let app = app.to_string();
        let _ = std::thread::Builder::new().name("rf-single".into()).spawn(move || {
            for conn in listener.incoming().flatten() {
                if let Some(msg) = read_message(&conn) {
                    // the app's own handler is registered with `serve_app`; if it is not yet,
                    // the message is dropped and the second launch goes on by itself
                    let _ = msg;
                }
            }
            let _ = app;
        });
    }
    Role::Primary(InstanceGuard::new(path))
}
