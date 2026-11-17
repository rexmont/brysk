//! Installer and auto-update, macOS half: macOS builds are distributed as a `.app` (Developer
//! ID, notarised) and updated by downloading a new one; the Velopack updater is Windows-only.
//! The same names keep the About/Settings/Status Center code unchanged.

pub const APP_ID: &str = "rustyfiles";
pub const REPO: &str = "https://github.com/gelenmedya/rustyfiles";
pub const DONATE: &str = "https://www.buymeacoffee.com/ozerguzel";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// A build run from `target/` (or anywhere not installed): no updates.
    NotInstalled,
    Idle,
    Checking,
    UpToDate,
    Downloading(String, u8),
    Ready(String),
    Failed(String),
}

pub fn startup() {}

pub fn state() -> State {
    State::NotInstalled
}

pub fn ready_since() -> Option<std::time::Instant> {
    None
}

pub fn is_important(_version: &str) -> bool {
    false
}

pub fn set_beta_updates(_on: bool) {}

pub fn is_beta(_version: &str) -> bool {
    false
}

pub fn version_full() -> String {
    format!("{} (macOS)", env!("CARGO_PKG_VERSION"))
}

pub fn check() {}

pub async fn redraw_while_busy(_cx: &mut gpui_kit::AsyncApp) {}

pub fn spawn_checks(_cx: &mut gpui_kit::App, _changed: impl Fn(&mut gpui_kit::App) + 'static) {}

pub fn restart_to_update() {}

pub fn count_start(_cx: &mut gpui_kit::App) {}

pub fn star_due(_s: &crate::settings::Settings) -> bool {
    false
}

pub fn donate_due(_s: &crate::settings::Settings) -> bool {
    false
}

pub fn donate_answered(s: &mut crate::settings::Settings, _donated: bool) {
    s.donate_never = true;
}