//! What a tray app does around its tray, shared by the apps that live there (the AI Hub, Desk
//! Host, Desk Link): the dropdown under the icon ([`TrayDropdown`]) and, on macOS, "Open at
//! login" as a per-user LaunchAgent ([`LoginItem`]).

use makepad_widgets::*;
use std::path::PathBuf;

/// A tray click this soon after the dropdown lost the focus is the click that closed it.
const REOPEN_GUARD: std::time::Duration = std::time::Duration::from_millis(350);

/// The tray's dropdown: a `window.dropdown: true` window shown under the icon on the primary
/// click (`show_at_tray`), as tall as its content up to a part of the screen, closed by a click
/// elsewhere (focus loss), Esc or another click on the icon.
pub struct TrayDropdown {
    pub tray: LiveId,
    pub width: f64,
    /// The height cap as a part of the screen.
    pub max: f64,
    /// `<name> selftest: dropdown ...` lines under `MAKEPAD_TRAY_SELFTEST`.
    pub name: &'static str,
    open: bool,
    /// the size it was last shown at (points)
    size: Vec2d,
    /// the visible area of the icon's screen at the last click
    screen: Rect,
    /// when a click elsewhere closed it: the click on the icon that did is not a new open
    dismissed: Option<std::time::Instant>,
}

/// What [`TrayDropdown::handle_event`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropdownChange {
    Opened,
    Closed,
}

/// A line for a tray selftest (`MAKEPAD_TRAY_SELFTEST`): `<name> selftest: <what>`.
pub fn selftest_note(name: &str, what: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("MAKEPAD_TRAY_SELFTEST").is_some()) {
        log!("{name} selftest: {what}");
    }
}

impl TrayDropdown {
    pub fn new(name: &'static str, tray: LiveId, width: f64) -> TrayDropdown {
        TrayDropdown { tray, width, max: 0.8, name, open: false, size: Vec2d::default(), screen: Rect::default(), dismissed: None }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The size for `content` points of content (0: not known yet, the cap).
    fn size_for(&self, content: f64) -> Vec2d {
        let screen_h = if self.screen.size.y > 0.0 { self.screen.size.y } else { 900.0 };
        let max = (screen_h * self.max).floor();
        dvec2(self.width, if content > 1.0 { content.ceil().min(max) } else { max })
    }

    pub fn open(&mut self, cx: &mut Cx, window: &WindowRef, content: f64) {
        let size = self.size_for(content);
        self.open = true;
        self.size = size;
        window.show_at_tray(cx, self.tray, size);
        selftest_note(self.name, &format!("dropdown opened {}x{}", size.x, size.y));
    }

    pub fn close(&mut self, cx: &mut Cx, window: &WindowRef, why: &str) {
        if std::mem::replace(&mut self.open, false) {
            selftest_note(self.name, &format!("dropdown closed ({why})"));
            window.set_visible(cx, false);
        }
    }

    /// The content grew or shrank: the open dropdown follows (call it after a draw).
    pub fn fit(&mut self, cx: &mut Cx, window: &WindowRef, content: f64) {
        if !self.open {
            return;
        }
        let size = self.size_for(content);
        if (size.y - self.size.y).abs() >= 1.0 {
            selftest_note(self.name, &format!("dropdown fitted to its content: {}x{}", size.x, size.y));
            self.size = size;
            window.show_at_tray(cx, self.tray, size);
        }
    }

    /// The tray's click (toggles; the click that just closed it does not open it again), the
    /// window's focus loss and Esc. Only where the desktop gives the tray click to the app
    /// (`Cx::has_tray_dropdown`); elsewhere the app handles `TrayClick` itself.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, window: &WindowRef, content: f64) -> Option<DropdownChange> {
        match event {
            Event::TrayClick(e) if e.tray == self.tray && cx.has_tray_dropdown() => {
                if e.screen.size.y > 0.0 {
                    self.screen = e.screen;
                }
                if self.open {
                    self.close(cx, window, "tray click");
                    Some(DropdownChange::Closed)
                } else if self.dismissed.take().is_some_and(|at| at.elapsed() < REOPEN_GUARD) {
                    None
                } else {
                    self.open(cx, window, content);
                    Some(DropdownChange::Opened)
                }
            }
            Event::PopupDismissed(e) if Some(e.window_id) == window.window_id() => {
                if self.open {
                    self.dismissed = Some(std::time::Instant::now());
                }
                self.close(cx, window, "focus lost");
                Some(DropdownChange::Closed)
            }
            Event::KeyDown(k) if k.key_code == KeyCode::Escape && self.open => {
                self.close(cx, window, "esc");
                Some(DropdownChange::Closed)
            }
            _ => None,
        }
    }
}

/// "Open at login" (macOS): a per-user LaunchAgent, `~/Library/LaunchAgents/<label>.plist`
/// (RunAtLoad, ProgramArguments = `program`), written and removed by the app itself. No admin
/// rights, no `launchctl` (launchd reads the folder at the next login, so switching it on never
/// starts a second copy), no privacy prompt. The switch shows on when the plist exists, names
/// `program` and runs it at load; switching on rewrites a plist that names something else.
/// `MAKEPAD_LAUNCH_AGENTS_DIR` replaces `~/Library/LaunchAgents` (tests).
pub struct LoginItem {
    pub label: &'static str,
    /// What launchd starts (an installed bundle's executable).
    pub program: PathBuf,
    /// More plist keys, as `<key>..</key><value/>` lines (KeepAlive, a log path).
    pub extra: String,
}

/// This bundle's executable, when the app runs from an installed `.app` (macOS).
pub fn bundle_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let in_bundle = exe.parent()?.file_name()? == "MacOS" && exe.parent()?.parent()?.file_name()? == "Contents" && exe.parent()?.parent()?.parent()?.extension()? == "app";
    (cfg!(target_os = "macos") && in_bundle).then_some(exe)
}

/// The pid record of an app installed as a bundle (`<settings dir>/app.pid`): written while it runs
/// from the `.app`, so its installer (tools/<app>/install-mac.sh) stops the installed copy by
/// that pid only before replacing it. Nothing is recorded for a `cargo run` or `makepad run` copy.
pub struct AppPid(pub PathBuf);

impl AppPid {
    pub fn write(&self) {
        if bundle_exe().is_some() {
            if let Some(dir) = self.0.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&self.0, format!("{}\n", std::process::id()));
        }
    }

    /// Removes the record when it is this process's.
    pub fn clear(&self) {
        if bundle_exe().is_some() && std::fs::read_to_string(&self.0).ok().and_then(|t| t.trim().parse::<u32>().ok()) == Some(std::process::id()) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

pub fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl LoginItem {
    fn path(&self) -> Option<PathBuf> {
        let dir = match std::env::var_os("MAKEPAD_LAUNCH_AGENTS_DIR").filter(|d| !d.is_empty()) {
            Some(d) => PathBuf::from(d),
            None => PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty())?).join("Library/LaunchAgents"),
        };
        Some(dir.join(format!("{}.plist", self.label)))
    }

    pub fn plist_text(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n    <key>Label</key>\n    <string>{}</string>\n    <key>ProgramArguments</key>\n    <array>\n        <string>{}</string>\n    </array>\n    <key>RunAtLoad</key>\n    <true/>\n    <key>LimitLoadToSessionType</key>\n    <string>Aqua</string>\n{}</dict>\n</plist>\n",
            self.label,
            xml(&self.program.to_string_lossy()),
            self.extra
        )
    }

    /// Whether it is on.
    pub fn is_on(&self) -> bool {
        let Some(path) = self.path() else { return false };
        let text = std::fs::read_to_string(path).unwrap_or_default();
        text.contains(&format!("<string>{}</string>", xml(&self.program.to_string_lossy()))) && text.contains("<key>RunAtLoad</key>")
    }

    pub fn set(&self, on: bool) -> Result<(), String> {
        let path = self.path().ok_or("no home folder")?;
        if on {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&path, self.plist_text()).map_err(|e| format!("{}: {e}", path.display()))
        } else {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plist_names_the_label_the_program_and_run_at_load() {
        let item = LoginItem { label: "nl.makepad.test", program: PathBuf::from("/Applications/A & B.app/Contents/MacOS/x"), extra: "    <key>KeepAlive</key>\n    <true/>\n".into() };
        let text = item.plist_text();
        assert!(text.contains("<string>nl.makepad.test</string>"));
        assert!(text.contains("<string>/Applications/A &amp; B.app/Contents/MacOS/x</string>"));
        assert!(text.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(text.contains("<key>KeepAlive</key>"));
    }
}
