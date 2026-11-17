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

/// "Brysk", or "Brysk-NAME" with `RUSTYFILES_INSTANCE=NAME`.
pub fn instance_name() -> String {
    match std::env::var("RUSTYFILES_INSTANCE") {
        Ok(n) if !n.is_empty() => format!("Brysk-{n}"),
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
