//! The mp* apps' API to the window manager.
//!
//! mpwm hosts apps as tiles over the studio protocol; an app never spawns
//! other apps or windows itself while hosted — it ASKS the compositor,
//! which knows the app registry, the file associations and how to float a
//! Quick-Look popup over the desk. The channel is `AppToStudio::Custom`
//! (app → WM) and `StudioToApp::Custom` (WM → app), both carrying one JSON
//! object: `{"mpwm": <WmRequest>}` upward, `{"mpwm": <WmEvent>}` downward.
//!
//! Standalone (not hosted) the requests fall back: a preview/open spawns
//! the associated app as its own window, title/cwd are no-ops.
//!
//! ```ignore
//! // mpfiles, on Space:
//! mp_wm_api::preview(cx, &path);
//! ```

use makepad_widgets::makepad_micro_serde::*;
use makepad_widgets::makepad_platform::studio::AppToStudio;
use makepad_widgets::*;
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

/// What an app can ask the window manager.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum WmRequest {
    /// Quick Look: open `path` in a floating preview popup over the desk.
    /// `app` picks the viewer; `None` = the WM's association for the file.
    Preview { app: Option<String>, path: String },
    /// Open `path` in its associated app (or `app`) as a normal tiled window.
    Open { app: Option<String>, path: String },
    /// Launch an app from the registry by id (e.g. "terminal", "browser").
    Launch { app: String, args: Vec<String> },
    /// The app's window title changed (the WM shows it in the bar).
    Title { title: String },
    /// The app's working directory changed (new terminals open here).
    Cwd { path: String },
    /// Show a desktop notification.
    Notify { title: String, body: String },
    /// Ask the WM to close this window (the app finished; previews on Esc).
    Close,
    /// From the preview REQUESTER: hide the Quick Look panel. The WM hides
    /// the float but keeps the viewer process warm for the next Preview.
    PreviewClose,
    /// Ask the WM to float / tile / fullscreen this window.
    SetFloating { floating: bool },
    SetFullscreen { fullscreen: bool },
}

/// What the window manager tells an app.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum WmEvent {
    /// The app is hosted; here is the current theme's splash path.
    Hosted { theme_splash: String },
    /// The WM wants the app to shut down gracefully.
    CloseRequested,
    /// Focus moved onto / off this window.
    Focus { focused: bool },
    /// This warm-pool instance was ADOPTED into a real tile: wake up.
    /// A warm instance is spawned with `MPWM_WARM_START=1` and must idle
    /// until this arrives — no samplers, no timers beyond a heartbeat, no
    /// background refresh (a cached task manager must not burn CPU).
    Adopted,
    /// Quick Look retarget: this WARM preview viewer must now show `path`
    /// (same viewer type as it was spawned for). The viewer swaps content
    /// in place — no respawn, no focus change.
    PreviewFile { path: String },
    /// The panel was hidden: the warm viewer UNLOADS what it was showing
    /// (drop decoders/textures/file handles, stop playback, blank state)
    /// and idles at near-zero cost until the next `PreviewFile`.
    PreviewUnload,
    /// To the app that REQUESTED a preview: the panel's true state, so the
    /// requester never tracks it blindly. While shown, that app keeps key
    /// focus and re-sends `Preview` on every selection change (dialing);
    /// its Space/Esc close the panel (`PreviewClose`).
    PreviewShown { path: String },
    PreviewHidden,
}

// The wire envelope: `{"mpwm": ...}`. (The derives don't bound generics,
// so one concrete envelope per direction.)
#[derive(SerJson, DeJson)]
struct RequestEnvelope {
    mpwm: WmRequest,
}

#[derive(SerJson, DeJson)]
struct EventEnvelope {
    mpwm: WmEvent,
}

impl WmRequest {
    pub fn to_json(&self) -> String {
        RequestEnvelope { mpwm: self.clone() }.serialize_json()
    }

    /// Parse a Custom message; `None` when it is not a WM request.
    pub fn parse(json: &str) -> Option<WmRequest> {
        if !json.contains("\"mpwm\"") {
            return None;
        }
        RequestEnvelope::deserialize_json(json)
            .ok()
            .map(|e| e.mpwm)
    }
}

impl WmEvent {
    pub fn to_json(&self) -> String {
        EventEnvelope { mpwm: self.clone() }.serialize_json()
    }

    pub fn parse(json: &str) -> Option<WmEvent> {
        if !json.contains("\"mpwm\"") {
            return None;
        }
        EventEnvelope::deserialize_json(json)
            .ok()
            .map(|e| e.mpwm)
    }
}

/// True when this process is hosted as an mpwm tile (or a Studio run view).
pub fn hosted(cx: &Cx) -> bool {
    cx.in_makepad_studio()
}

/// True when this process was spawned as a DORMANT warm-pool instance:
/// heavy periodic work (samplers, refresh timers) must wait for
/// [`WmEvent::Adopted`]. Checked once — adoption never re-reads the env.
pub fn warm_start() -> bool {
    std::env::var("MPWM_WARM_START").is_ok()
}

/// Send a request to the window manager. Returns false when not hosted
/// (nothing was sent; the caller may fall back).
pub fn send(cx: &Cx, req: &WmRequest) -> bool {
    if !hosted(cx) {
        return false;
    }
    Cx::send_studio_message(AppToStudio::Custom(req.to_json()));
    true
}

/// Quick Look `path`. Hosted: the WM floats the associated viewer over the
/// desk. Standalone: spawn the viewer's binary with `--preview` next to
/// this executable (best effort; false when nothing could be started).
pub fn preview(cx: &Cx, path: &Path) -> bool {
    let req = WmRequest::Preview {
        app: None,
        path: path.to_string_lossy().to_string(),
    };
    if send(cx, &req) {
        return true;
    }
    spawn_sibling(viewer_for(path), &["--preview", &path.to_string_lossy()])
}

/// Open `path` in its associated app: hosted = a new tile; standalone = a
/// sibling process with its own window.
pub fn open(cx: &Cx, path: &Path) -> bool {
    let req = WmRequest::Open {
        app: None,
        path: path.to_string_lossy().to_string(),
    };
    if send(cx, &req) {
        return true;
    }
    spawn_sibling(viewer_for(path), &[&path.to_string_lossy()])
}

/// Tell the WM the window title (no-op standalone: the app sets its own).
pub fn set_title(cx: &Cx, title: &str) {
    send(
        cx,
        &WmRequest::Title {
            title: title.to_string(),
        },
    );
}

/// Tell the WM the working directory (terminals; no-op standalone).
pub fn set_cwd(cx: &Cx, path: &Path) {
    send(
        cx,
        &WmRequest::Cwd {
            path: path.to_string_lossy().to_string(),
        },
    );
}

/// The file associations, shared by the WM and the file browser:
/// extension → viewer app id (a registry id, also the binary name).
pub fn viewer_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "qoi" | "ico" | "svg" => "mpimage",
        "mp4" | "mov" | "m4v" | "webm" | "mkv" | "avi" => "mpvideo",
        "csv" | "tsv" => "mpsheets",
        "pdf" => "mppdf",
        "html" | "htm" | "url" | "webloc" => "mpbrowser",
        _ => "mpterm",
    }
}

/// Spawn a sibling binary of the running executable, detached.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_sibling(bin: &str, args: &[&str]) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    let mut path: PathBuf = dir.join(bin);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    if !path.exists() {
        return false;
    }
    std::process::Command::new(path)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(target_arch = "wasm32")]
fn spawn_sibling(_bin: &str, _args: &[&str]) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip() {
        let req = WmRequest::Preview {
            app: Some("mpimage".into()),
            path: "/a/b c.png".into(),
        };
        let json = req.to_json();
        assert!(json.starts_with("{\"mpwm\":"), "{json}");
        assert_eq!(WmRequest::parse(&json), Some(req));
        let open = WmRequest::Open {
            app: None,
            path: "/x\"y.mov".into(),
        };
        assert_eq!(WmRequest::parse(&open.to_json()), Some(open));
        assert_eq!(WmRequest::parse(r#"{"mpterm_title":"x"}"#), None);
        assert_eq!(WmRequest::parse("not json"), None);
    }

    #[test]
    fn preview_protocol_round_trip() {
        let retarget = WmEvent::PreviewFile {
            path: "/a/b.png".into(),
        };
        assert_eq!(WmEvent::parse(&retarget.to_json()), Some(retarget));
        let shown = WmEvent::PreviewShown {
            path: "/a/b.png".into(),
        };
        assert_eq!(WmEvent::parse(&shown.to_json()), Some(shown));
        assert_eq!(
            WmEvent::parse(&WmEvent::PreviewHidden.to_json()),
            Some(WmEvent::PreviewHidden)
        );
        assert_eq!(
            WmEvent::parse(&WmEvent::PreviewUnload.to_json()),
            Some(WmEvent::PreviewUnload)
        );
        assert_eq!(
            WmRequest::parse(&WmRequest::PreviewClose.to_json()),
            Some(WmRequest::PreviewClose)
        );
    }

    #[test]
    fn event_round_trip() {
        let ev = WmEvent::Hosted {
            theme_splash: "/t/theme.splash".into(),
        };
        assert_eq!(WmEvent::parse(&ev.to_json()), Some(ev));
        assert_eq!(WmEvent::parse(&WmEvent::CloseRequested.to_json()), Some(WmEvent::CloseRequested));
    }

    #[test]
    fn associations() {
        assert_eq!(viewer_for(Path::new("a/B.PNG")), "mpimage");
        assert_eq!(viewer_for(Path::new("clip.mov")), "mpvideo");
        assert_eq!(viewer_for(Path::new("data.csv")), "mpsheets");
        assert_eq!(viewer_for(Path::new("paper.PDF")), "mppdf");
        assert_eq!(viewer_for(Path::new("README.md")), "mpterm");
        assert_eq!(viewer_for(Path::new("noext")), "mpterm");
    }
}
