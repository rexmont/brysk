//! Hand-over of a second launch to the running instance, the macOS half of `rf-win::handover`.
//!
//! The Windows version talks over a named pipe with a request gate (a quit waits while a
//! request is in flight). Here the same conversation runs over the Unix-domain socket of
//! `single_instance`: the first instance listens, the second sends its launch line and waits
//! for "ok" or "failed".

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::single_instance::{self, InstanceGuard, Role};

pub struct Gate(Mutex<bool>);

impl Gate {
    pub const fn new() -> Gate {
        Gate(Mutex::new(false))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, bool> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn try_leave(&self) -> bool {
        let mut s = self.lock();
        if *s {
            return true;
        }
        *s = true;
        true
    }

    pub fn leave_now(&self) {
        *self.lock() = true;
    }

    /// Sparkle cancelled an installation: this process can accept launches again.
    pub fn resume(&self) { *self.lock() = false; }

    pub fn is_leaving(&self) -> bool {
        *self.lock()
    }
}

pub static GATE: Gate = Gate::new();

#[cfg(test)]
mod gate_tests {
    #[test]
    fn cancelled_updates_allow_a_later_launch_again() {
        let gate = super::Gate::new();
        assert!(gate.try_leave());
        assert!(gate.is_leaving());
        gate.resume();
        assert!(!gate.is_leaving());
    }
}

fn trace_sink() -> Option<&'static PathBuf> {
    static SINK: OnceLock<Option<PathBuf>> = OnceLock::new();
    SINK.get_or_init(|| {
        let v = std::env::var_os("RUSTYFILES_TRACE_HANDOVER").filter(|v| !v.is_empty())?;
        Some(if v == "1" { std::env::temp_dir().join("Brysk-handover.log") } else { v.into() })
    })
    .as_ref()
}

/// One line in the hand-over log, when `RUSTYFILES_TRACE_HANDOVER` is set.
pub fn trace(text: &str) {
    use std::io::Write;
    let Some(path) = trace_sink() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{:?}] {text}", std::process::id());
    }
}

type Handler = Arc<dyn Fn(String, Ack) + Send + Sync>;

fn handler_slot() -> &'static Mutex<Option<Handler>> {
    static H: OnceLock<Mutex<Option<Handler>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(None))
}

/// The answer a second launch waits for.
pub struct Ack {
    stream: Option<std::os::unix::net::UnixStream>,
}

impl Ack {
    /// The request was acted on: tells the second launch, which then ends.
    pub fn done(mut self) -> bool {
        self.reply(b'1');
        false
    }

    /// The request could not be shown: the second launch shows the folder itself.
    pub fn refuse(mut self) -> bool {
        self.reply(b'0');
        false
    }

    fn reply(&mut self, b: u8) {
        use std::io::Write;
        if let Some(mut s) = self.stream.take() {
            let _ = s.write_all(&[b]);
            let _ = s.flush();
        }
    }
}

impl Drop for Ack {
    fn drop(&mut self) {
        self.reply(b'0');
    }
}

/// Sends `message` to the running instance and waits for its answer.
fn send_with_ack(app: &str, message: &str) -> bool {
    use std::io::{Read, Write};
    let Ok(mut s) = std::os::unix::net::UnixStream::connect(single_instance::socket_path(app)) else { return false };
    let bytes = message.as_bytes();
    if s.write_all(&(bytes.len() as u32).to_be_bytes()).is_err() || s.write_all(bytes).is_err() || s.flush().is_err() {
        return false;
    }
    let mut answer = [0u8; 1];
    matches!(s.read_exact(&mut answer), Ok(())) && answer[0] == b'1'
}

/// Second launches: hands the request to the running instance, or becomes it.
pub fn acquire(message: &str) -> Role {
    let app = single_instance::instance_name();
    if send_with_ack(&app, message) {
        trace("handover: forwarded to the running instance");
        return Role::Forwarded;
    }
    let path = single_instance::socket_path(&app);
    let _ = std::fs::remove_file(&path);
    match std::os::unix::net::UnixListener::bind(&path) {
        Ok(listener) => {
            let _ = std::thread::Builder::new().name("rf-handover".into()).spawn(move || {
                for conn in listener.incoming().flatten() {
                    let Some(msg) = single_instance::read_message(&conn) else { continue };
                    let handler = handler_slot().lock().unwrap().clone();
                    match handler {
                        Some(h) => h(msg, Ack { stream: Some(conn) }),
                        None => {
                            // not ready yet: let the second launch show the folder itself
                            drop(conn);
                        }
                    }
                }
            });
            Role::Primary(InstanceGuard::new(path))
        }
        Err(e) => {
            log::warn!("handover: cannot listen on {}: {e}", path.display());
            Role::Primary(InstanceGuard::new(path))
        }
    }
}

/// A second message to the running instance after [`acquire`] returned [`Role::Forwarded`].
pub fn send_followup(message: &str) {
    let app = single_instance::instance_name();
    let _ = single_instance::send_to(&app, message);
}

/// Where the first instance's launches arrive.
pub fn serve(on_message: impl Fn(String, Ack) + Send + Sync + 'static) {
    *handler_slot().lock().unwrap() = Some(Arc::new(on_message));
}
