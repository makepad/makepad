//! Steam's close button hides the client. The WM owns the lifetime of the
//! dedicated Steam/Xwayland user services, including windows opened by games.

use super::Win;
use makepad_app_host::ClientId;
use makepad_widgets::makepad_platform::thread::{Lane, TaskHandle};
use makepad_widgets::{log, Cx};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Session {
    connections: HashSet<u32>,
    had_windows: bool,
    empty_since: Option<Instant>,
    stopping: Option<TaskHandle<Result<(), String>>>,
    reopen: Option<String>,
    last_error: Option<String>,
}

fn steam_app(id: &str) -> bool {
    id.eq_ignore_ascii_case("steam")
        || id.eq_ignore_ascii_case("wl.steam")
        || id.strip_prefix("steam_app_").is_some_and(|game| {
            !game.is_empty() && game.bytes().all(|c| c.is_ascii_digit())
        })
}

impl Session {
    pub(super) fn pending_launch(&self) -> Option<&str> { self.reopen.as_deref() }
    /// Only mapping/identity changes walk the window list, never pointer events or frames.
    pub(super) fn windows_changed(&mut self, wins: &HashMap<ClientId, Win>) {
        for win in wins.values().filter(|w| steam_app(&w.app_id)) {
            self.connections.insert(win.main.0);
        }
        // The bridge has one Wayland connection for all its X11 windows. Counting
        // that connection also covers games that do not set a Steam app id.
        let any = wins.values().any(|w| self.connections.contains(&w.main.0));
        if any {
            self.had_windows = true;
            self.empty_since = None;
        } else if self.had_windows && self.empty_since.is_none() && self.stopping.is_none() {
            self.empty_since = Some(Instant::now());
        }
    }

    pub(super) fn defer_launch(&mut self, app: &str) -> bool {
        if app == "wl.steam" && (self.empty_since.is_some() || self.stopping.is_some()) {
            // A new start must not race systemd's stop job and lose its processes.
            self.reopen = Some(app.to_owned());
            return true;
        }
        false
    }

    fn error(&mut self, error: String) {
        if self.last_error.as_ref() != Some(&error) {
            log!("wm: Steam shutdown pending: {error}");
            self.last_error = Some(error);
        }
    }

    pub(super) fn tick(&mut self, cx: &mut Cx, runtime: &Path) -> Option<String> {
        if let Some(result) = self.stopping.as_mut().and_then(TaskHandle::try_take) {
            self.stopping = None;
            match result {
                Ok(Ok(())) => {
                    log!("wm: Steam and its Xwayland bridge stopped");
                    self.connections.clear();
                    self.had_windows = false;
                    self.empty_since = None;
                    self.last_error = None;
                    return self.reopen.take();
                }
                Ok(Err(error)) => self.error(error),
                Err(error) => self.error(error.to_string()),
            }
        }
        if self.stopping.is_some() { return None; }
        let Some(since) = self.empty_since else {
            // A replacement window mapped while the close was settling.
            return self.reopen.take();
        };
        // A client can unmap and replace a window across adjacent event batches.
        // This is a quiet period, not a shutdown deadline; systemd owns process cleanup.
        if since.elapsed() < Duration::from_secs(1) { return None; }
        let runtime = runtime.to_owned();
        match cx.task_pool().submit(Lane::Heavy, move || stop_services(&runtime)) {
            Ok(task) => {
                log!("wm: last Steam/game window closed; stopping Steam and its Xwayland bridge");
                self.stopping = Some(task);
            }
            // Keep the request and retry on the next maintenance tick when the pool is full.
            Err(error) => self.error(error.to_string()),
        }
        None
    }
}

fn stop_services(runtime: &Path) -> Result<(), String> {
    let mut failures = Vec::new();
    // Let Steam finish while its display is still alive, then remove the bridge.
    // KillMode=control-group in both units also reaps Steam helpers and Xwayland.
    for unit in ["makepad-steam.service", "makepad-steam-xwayland.service"] {
        let result = Command::new("systemctl")
            .args(["--user", "stop", unit])
            .env("XDG_RUNTIME_DIR", runtime)
            .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/bus", runtime.display()))
            .stdin(Stdio::null())
            .output();
        match result {
            Ok(output) if output.status.success() => {}
            Ok(output) => failures.push(format!("{unit}: {} ({})", String::from_utf8_lossy(&output.stderr).trim(), output.status)),
            Err(error) => failures.push(format!("{unit}: {error}")),
        }
    }
    if failures.is_empty() { Ok(()) } else { Err(failures.join("; ")) }
}
