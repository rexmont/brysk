//! Sparkle owns update scheduling, verification and installation; Brysk supplies a quiet badge.

use gpui_kit::App;
use rf_win::updater::Event;
use std::sync::Mutex;

pub const APP_ID: &str = "brysk";
pub const REPO: &str = "https://github.com/rexmont/brysk";
pub const DONATE: &str = "https://ko-fi.com/J3J3WWZKO";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// A Cargo binary without its app bundle cannot update itself.
    NotInstalled,
    Idle,
    Checking,
    UpToDate,
    Available(String),
    Installing,
    Downloading(String, u8),
    Ready(String),
    Failed(String),
}

pub fn startup() {}

static STATE: Mutex<State> = Mutex::new(State::NotInstalled);

pub fn state() -> State {
    STATE.lock().unwrap().clone()
}

pub fn ready_since() -> Option<std::time::Instant> {
    None
}

pub fn is_important(_version: &str) -> bool {
    false
}

pub fn set_beta_updates(_on: bool) {}

pub fn is_beta(_version: &str) -> bool {
    _version.split('+').next().unwrap_or("").contains('-')
}

pub fn version_full() -> String {
    format!("{} (macOS)", env!("CARGO_PKG_VERSION"))
}

/// Main thread: Sparkle supplies the check/download/relaunch windows.
pub fn check() {
    if matches!(state(), State::NotInstalled | State::Installing) {
        return;
    }
    if !matches!(state(), State::Available(_)) {
        *STATE.lock().unwrap() = State::Checking;
    }
    if !rf_win::updater::check() {
        *STATE.lock().unwrap() = State::Failed("The update framework is unavailable.".into());
    }
}

pub async fn redraw_while_busy(_cx: &mut gpui_kit::AsyncApp) {}

pub fn spawn_checks(cx: &mut App, changed: impl Fn(&mut App) + 'static) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    match rf_win::updater::start(move |event| {
        let _ = tx.unbounded_send(event);
    }) {
        Ok(true) => *STATE.lock().unwrap() = State::Idle,
        Ok(false) => return,
        Err(error) => {
            *STATE.lock().unwrap() = State::Failed(error);
            return;
        }
    }
    cx.spawn(async move |cx| {
        use futures::StreamExt;
        while let Some(event) = rx.next().await {
            cx.update(|cx| {
                if matches!(event, Event::Failed(_) | Event::Cancelled) && matches!(state(), State::Installing) {
                    rf_win::handover::GATE.resume();
                    rf_win::finder_replace::resume_after_update_failure();
                }
                let installing = event == Event::InstallRequested;
                let mut state = STATE.lock().unwrap();
                *state = after_event(state.clone(), event);
                drop(state);
                if installing {
                    install_when_idle(cx);
                }
                changed(cx);
            });
        }
    })
    .detach();
}

fn after_event(state: State, event: Event) -> State {
    match event {
        Event::Available(version) => State::Available(version),
        Event::UpToDate => State::UpToDate,
        Event::Failed(error) => State::Failed(error),
        Event::Cancelled => State::Idle,
        Event::InstallRequested => State::Installing,
        Event::SessionFinished if matches!(state, State::Available(_) | State::Checking) => {
            State::Idle
        }
        Event::SessionFinished => state,
    }
}

/// Observe operation completion rather than polling while a copy is running.
fn install_when_idle(cx: &mut App) {
    let ops = cx.global::<crate::status_center::OpsGlobal>().0.clone();
    if ops.read(cx).running() == 0 {
        prepare_install(cx);
        return;
    }
    let (tx, rx) = futures::channel::oneshot::channel();
    let mut tx = Some(tx);
    let subscription = cx.observe(&ops, move |ops, cx| {
        if ops.read(cx).running() == 0 {
            if let Some(tx) = tx.take() {
                let _ = tx.send(());
            }
        }
    });
    cx.spawn(async move |cx| {
        if rx.await.is_ok() {
            cx.update(prepare_install);
        }
        drop(subscription);
    })
    .detach();
}

fn prepare_install(cx: &mut App) {
    crate::workspace::save_all_sessions(cx);
    cx.global::<crate::settings::Settings>().clone().save();
    rf_win::handover::GATE.leave_now();
    // Once idle, close file-manager windows so no new operation can start during replacement.
    // Sparkle's native progress window remains open until the app relaunches.
    for window in cx.windows() {
        let _ = window.update(cx, |_, window, _| window.remove_window());
    }
    // Stop the nested helper before Sparkle replaces its containing bundle.
    cx.spawn(async move |cx| {
        cx.background_executor()
            .spawn(async {
                rf_win::finder_replace::stop_for_update();
                rf_win::single_instance::send_to(
                    &crate::look::ipc::app_name(),
                    &crate::look::ipc::Msg::Quit.encode(),
                );
                rf_win::indexer::stop();
            })
            .await;
        cx.update(|_| {
            rf_win::updater::install();
        });
    })
    .detach();
}

pub fn restart_to_update() {
    check();
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_sessions_clear_the_badge_but_keep_meaningful_results() {
        assert_eq!(after_event(State::Installing, Event::Cancelled), State::Idle);
        assert_eq!(
            after_event(State::Available("2".into()), Event::SessionFinished),
            State::Idle
        );
        assert_eq!(
            after_event(State::Checking, Event::UpToDate),
            State::UpToDate
        );
        assert_eq!(
            after_event(State::UpToDate, Event::SessionFinished),
            State::UpToDate
        );
        assert_eq!(
            after_event(State::Failed("offline".into()), Event::SessionFinished),
            State::Failed("offline".into())
        );
        assert_eq!(
            after_event(State::Idle, Event::InstallRequested),
            State::Installing
        );
    }
}
