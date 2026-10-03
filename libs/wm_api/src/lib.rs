//! The Makepad apps' API to the window manager.
//!
//! wm hosts apps as tiles over the studio protocol; an app never spawns
//! other apps or windows itself while hosted — it ASKS the compositor,
//! which knows the app registry, the file associations and how to float a
//! Quick-Look popup over the desk. The channel is `AppToStudio::Custom`
//! (app → WM) and `StudioToApp::Custom` (WM → app), both carrying one JSON
//! object: `{"wm": <WmRequest>}` upward, `{"wm": <WmEvent>}` downward.
//!
//! Standalone (not hosted) the requests fall back: a preview/open spawns
//! the associated app as its own window, title/cwd are no-ops.
//! [`screens`]/[`set_fullscreen_span`] fall back too -- on Linux, not
//! OHOS (direct or windowed; only the direct backend actually ever
//! reports screens, so a span only ever resolves there) a span is
//! resolved against [`screens`] and recorded in-process, no WM involved;
//! see [`span_rect_for_window`] for the rect an app lays its content into.
//!
//! ```ignore
//! // files, on Space:
//! makepad_wm_api::preview(cx, &path);
//! ```

use makepad_widgets_core::makepad_micro_serde::*;
use makepad_widgets_core::makepad_platform::studio::AppToStudio;
use makepad_widgets_core::*;
use std::path::{Path, PathBuf};

mod span;
pub use span::{
    primary_or_first_index, resolve_standalone_current_span, resolve_standalone_span,
    screens_in_window, span_rect, ScreenSpan, SpanError, WmScreen,
};

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
    /// Ask the WM to make this window fullscreen across `span`'s screens
    /// (`None` = leave fullscreen). `SetFullscreen { fullscreen }` is the
    /// same as `Current` / `None`. The WM answers with a fresh
    /// [`WmEvent::Screens`] whose `span` says what it actually did.
    SetFullscreenSpan { span: Option<ScreenSpan> },
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
    /// A warm instance is spawned with `MAKEPAD_WM_WARM_START=1` and must idle
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
    /// The live screens, left to right, in THIS window's coordinates (a
    /// screen's rect minus the window's own origin), and the names of the
    /// screens this window currently spans fullscreen (`None` when it
    /// spans none). The rects are desk-clipped, like the window's own
    /// framebuffer (e.g. narrower while the AI pane reserves part of the
    /// desk), not the physical screens. Sent when the window is first
    /// shown or adopted, and again whenever the screens or the window's
    /// position change, in no guaranteed order relative to a concurrent
    /// `WindowGeomChange`/swapchain resize -- lay out from whichever
    /// arrives last. Read it back with [`screens`].
    Screens {
        screens: Vec<WmScreen>,
        span: Option<Vec<String>>,
    },
}

// The wire envelope: `{"wm": ...}`. (The derives don't bound generics,
// so one concrete envelope per direction.)
#[derive(SerJson, DeJson)]
struct RequestEnvelope {
    wm: WmRequest,
}

#[derive(SerJson, DeJson)]
struct EventEnvelope {
    wm: WmEvent,
}

impl WmRequest {
    pub fn to_json(&self) -> String {
        RequestEnvelope { wm: self.clone() }.serialize_json()
    }

    /// Parse a Custom message; `None` when it is not a WM request.
    pub fn parse(json: &str) -> Option<WmRequest> {
        if !json.contains("\"wm\"") {
            return None;
        }
        RequestEnvelope::deserialize_json(json)
            .ok()
            .map(|e| e.wm)
    }
}

impl WmEvent {
    pub fn to_json(&self) -> String {
        EventEnvelope { wm: self.clone() }.serialize_json()
    }

    /// Parse a Custom message; `None` when it is not a WM event. A
    /// [`WmEvent::Screens`] is also remembered for [`screens`], so an app
    /// that parses its Custom messages (as every hosted app must to see
    /// WM events at all) keeps that answer current without extra calls.
    pub fn parse(json: &str) -> Option<WmEvent> {
        if !json.contains("\"wm\"") {
            return None;
        }
        let ev = EventEnvelope::deserialize_json(json).ok().map(|e| e.wm)?;
        if let WmEvent::Screens { screens, span } = &ev {
            *LAST_SCREENS.lock().unwrap_or_else(|p| p.into_inner()) = Some(LastScreens {
                screens: screens.clone(),
                span: span.clone(),
            });
        }
        Some(ev)
    }
}

/// The last [`WmEvent::Screens`] this process parsed (hosted only).
#[derive(Clone, Debug, Default)]
struct LastScreens {
    screens: Vec<WmScreen>,
    span: Option<Vec<String>>,
}

static LAST_SCREENS: std::sync::Mutex<Option<LastScreens>> = std::sync::Mutex::new(None);

/// The live screens, left to right, in this window's coordinates.
///
/// Hosted: the last [`WmEvent::Screens`] the WM sent (empty until one
/// arrives, and always empty under a host that never sends one, such as
/// Studio's run view). Standalone: the platform's own screen list
/// (`makepad_platform::screens()`), named by connector on Linux and
/// `"screen0"`, `"screen1"`, ... elsewhere; on the Linux direct backend
/// these are already in the (desktop-sized) window's coordinates.
pub fn screens(cx: &Cx) -> Vec<WmScreen> {
    if hosted(cx) {
        return LAST_SCREENS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|l| l.screens.clone())
            .unwrap_or_default();
    }
    standalone_screens(
        &makepad_widgets_core::makepad_platform::screens(),
        &makepad_widgets_core::makepad_platform::linux_screen_names(),
    )
}

/// The names of the screens this window currently spans fullscreen, left
/// to right, or `None` when it spans none. Hosted: the last
/// [`WmEvent::Screens`]'s `span`. Standalone: the span last recorded by
/// [`set_fullscreen_span`] (Linux, not OHOS, only), re-resolved against
/// the live screens (dropped, like the WM drops it, if a name no longer
/// resolves -- e.g. after a hotplug); `None` on every other standalone
/// backend, matching that function's fallback there.
pub fn current_span(cx: &Cx) -> Option<Vec<String>> {
    if hosted(cx) {
        return LAST_SCREENS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .and_then(|l| l.span.clone());
    }
    standalone_current_span()
}

/// The union rect of the screens this window currently spans, in this
/// window's coordinates (the same space [`screens`] reports) -- what the
/// app should lay its content into. `None` when it spans no screens, or
/// [`current_span`]'s names no longer resolve against [`screens`].
pub fn span_rect_for_window(cx: &Cx) -> Option<(f64, f64, f64, f64)> {
    let names = current_span(cx)?;
    let screens = screens(cx);
    span_rect(&screens, &ScreenSpan::Screens(names), None)
        .ok()
        .map(|(_, rect)| rect)
}

/// Platform screens as [`WmScreen`]s: `geoms[i]` named `names[i]`, or
/// `"screen<i>"` where no connector name is known (non-Linux platforms
/// have none).
pub fn standalone_screens(
    geoms: &[makepad_widgets_core::makepad_platform::ScreenGeom],
    names: &[String],
) -> Vec<WmScreen> {
    geoms
        .iter()
        .enumerate()
        .map(|(i, g)| WmScreen {
            name: names
                .get(i)
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or_else(|| format!("screen{i}")),
            x: g.bounds.pos.x,
            y: g.bounds.pos.y,
            w: g.bounds.size.x,
            h: g.bounds.size.y,
            primary: g.is_primary,
        })
        .collect()
}

/// Ask the WM to make this window fullscreen across `span` (`None` =
/// leave fullscreen). Hosted, this is asynchronous: a `true` return means
/// only that the request was sent, not that it was honoured -- the
/// outcome arrives later as a [`WmEvent::Screens`] and is read back with
/// [`current_span`]; under Studio's run view (hosted, but no WM to
/// answer) it still returns `true` though no [`WmEvent::Screens`] ever
/// follows.
///
/// Standalone on Linux, not OHOS (direct or windowed; there is no WM to
/// answer, and on the direct backend its one window already covers the
/// whole wide desktop): resolves `span` against the live screens and
/// records it directly, same-process -- [`span_rect_for_window`] then
/// gives the union rect to lay content into; no composition change.
/// Returns false (and the recorded span is kept) when `span` does not
/// resolve (unknown name, not adjacent, or -- `Current` with no
/// primary/first screen -- no live screens; windowed Linux never has any,
/// so it always lands here).
///
/// Standalone on every other backend (macOS, Windows, wasm,
/// android/ohos): not yet implemented; returns false (nothing recorded,
/// nothing asked) regardless of `span`.
pub fn set_fullscreen_span(cx: &Cx, span: Option<ScreenSpan>) -> bool {
    if hosted(cx) {
        return send(cx, &WmRequest::SetFullscreenSpan { span });
    }
    standalone_set_fullscreen_span(span)
}

/// The span recorded by [`set_fullscreen_span`] for a standalone Linux
/// direct app: there is no WM to hold it, so this process does.
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
static STANDALONE_SPAN: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
fn standalone_set_fullscreen_span(span: Option<ScreenSpan>) -> bool {
    let screens = standalone_screens(
        &makepad_widgets_core::makepad_platform::screens(),
        &makepad_widgets_core::makepad_platform::linux_screen_names(),
    );
    let current = primary_or_first_index(&screens);
    let mut slot = STANDALONE_SPAN.lock().unwrap_or_else(|p| p.into_inner());
    let (recorded, ok) = resolve_standalone_span(&screens, current, &slot, span);
    if !ok {
        log!("wm_api: standalone span could not be honoured against the live screens; the current one is kept");
    }
    *slot = recorded;
    ok
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
fn standalone_current_span() -> Option<Vec<String>> {
    let recorded = STANDALONE_SPAN.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let screens = standalone_screens(
        &makepad_widgets_core::makepad_platform::screens(),
        &makepad_widgets_core::makepad_platform::linux_screen_names(),
    );
    resolve_standalone_current_span(&screens, &recorded)
}

#[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
fn standalone_set_fullscreen_span(_span: Option<ScreenSpan>) -> bool {
    false
}

#[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
fn standalone_current_span() -> Option<Vec<String>> {
    None
}

/// True when this process is hosted as an wm tile (or a Studio run view).
pub fn hosted(cx: &Cx) -> bool {
    cx.in_makepad_studio()
}

/// True when this process was spawned as a DORMANT warm-pool instance:
/// heavy periodic work (samplers, refresh timers) must wait for
/// [`WmEvent::Adopted`]. Checked once — adoption never re-reads the env.
pub fn warm_start() -> bool {
    std::env::var("MAKEPAD_WM_WARM_START").is_ok()
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
/// extension → viewer app id (a registry id; `spawn_sibling` finds its binary).
pub fn viewer_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "qoi" | "ico" | "svg" => "image",
        "mp4" | "mov" | "m4v" | "webm" | "mkv" | "avi" => "video",
        "csv" | "tsv" => "sheets",
        "pdf" => "pdf",
        "html" | "htm" | "url" | "webloc" => "browser",
        _ => "terminal",
    }
}

/// The app `id`'s binary (the registry's `makepad-app-<id>`) beside the
/// running executable, when it is there: where a standalone app finds the
/// viewers and the terminal it opens.
#[cfg(not(target_arch = "wasm32"))]
pub fn sibling_app(id: &str) -> Option<PathBuf> {
    let entry = makepad_app_registry::find(id)?;
    let exe = std::env::current_exe().ok()?;
    let mut path = exe.parent()?.join(entry.bin);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path.exists().then_some(path)
}

#[cfg(target_arch = "wasm32")]
pub fn sibling_app(_id: &str) -> Option<PathBuf> {
    None
}

/// Spawn the app `id`'s sibling binary ([`sibling_app`]), detached.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_sibling(id: &str, args: &[&str]) -> bool {
    let Some(path) = sibling_app(id) else {
        return false;
    };
    let mut command = std::process::Command::new(path);
    // Windows: a sibling linked as a console program (a plain `cargo
    // build`) would open a console window of its own; its output goes
    // nowhere anyway.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(target_arch = "wasm32")]
fn spawn_sibling(_id: &str, _args: &[&str]) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip() {
        let req = WmRequest::Preview {
            app: Some("image".into()),
            path: "/a/b c.png".into(),
        };
        let json = req.to_json();
        assert!(json.starts_with("{\"wm\":"), "{json}");
        assert_eq!(WmRequest::parse(&json), Some(req));
        let open = WmRequest::Open {
            app: None,
            path: "/x\"y.mov".into(),
        };
        assert_eq!(WmRequest::parse(&open.to_json()), Some(open));
        assert_eq!(WmRequest::parse(r#"{"terminal_title":"x"}"#), None);
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

    fn screen(name: &str, x: f64, w: f64, primary: bool) -> WmScreen {
        WmScreen { name: name.into(), x, y: 0.0, w, h: 1080.0, primary }
    }

    #[test]
    fn span_request_round_trip() {
        for span in [
            None,
            Some(ScreenSpan::Current),
            Some(ScreenSpan::All),
            Some(ScreenSpan::Screens(vec!["DP-1".into(), "HDMI-A-1".into()])),
        ] {
            let req = WmRequest::SetFullscreenSpan { span };
            let json = req.to_json();
            assert!(json.starts_with("{\"wm\":"), "{json}");
            assert_eq!(WmRequest::parse(&json), Some(req));
        }
        let old = WmRequest::SetFullscreen { fullscreen: true };
        assert_eq!(WmRequest::parse(&old.to_json()), Some(old));
    }

    #[test]
    fn screens_event_round_trip() {
        let ev = WmEvent::Screens {
            screens: vec![screen("DP-1", -12.5, 1920.0, true), screen("HDMI-A-1", 1907.5, 2560.0, false)],
            span: Some(vec!["DP-1".into()]),
        };
        assert_eq!(WmEvent::parse(&ev.to_json()), Some(ev));
        let none = WmEvent::Screens { screens: Vec::new(), span: None };
        assert_eq!(WmEvent::parse(&none.to_json()), Some(none));
        // A Screens event is not a request, nor the raw tiled-fullscreen note.
        assert_eq!(WmRequest::parse(&WmEvent::Screens { screens: Vec::new(), span: None }.to_json()), None);
        assert!(WmEvent::parse(r#"{"wm_fullscreen":true}"#).is_none());
    }

    #[test]
    fn standalone_screens_names_or_synthesizes() {
        let geom = |x: f64, w: f64, primary: bool| ScreenGeom {
            bounds: Rect { pos: dvec2(x, 0.0), size: dvec2(w, 1080.0) },
            work_area: Rect { pos: dvec2(x, 0.0), size: dvec2(w, 1040.0) },
            is_primary: primary,
        };
        let geoms = [geom(0.0, 1920.0, true), geom(1920.0, 2560.0, false)];
        // Linux: connector names, index-aligned.
        let named = standalone_screens(&geoms, &["DP-1".into(), "HDMI-A-1".into()]);
        assert_eq!(named, vec![screen("DP-1", 0.0, 1920.0, true), screen("HDMI-A-1", 1920.0, 2560.0, false)]);
        // Elsewhere: no names, so screen0.. by position in the list.
        let synth = standalone_screens(&geoms, &[]);
        assert_eq!(synth, vec![screen("screen0", 0.0, 1920.0, true), screen("screen1", 1920.0, 2560.0, false)]);
        // A short or blank name list is filled in, never misaligned.
        let partial = standalone_screens(&geoms, &["".into()]);
        assert_eq!(partial[0].name, "screen0");
        assert_eq!(partial[1].name, "screen1");
        assert!(standalone_screens(&[], &["DP-1".into()]).is_empty());
    }

    #[test]
    fn associations() {
        assert_eq!(viewer_for(Path::new("a/B.PNG")), "image");
        assert_eq!(viewer_for(Path::new("clip.mov")), "video");
        assert_eq!(viewer_for(Path::new("data.csv")), "sheets");
        assert_eq!(viewer_for(Path::new("paper.PDF")), "pdf");
        assert_eq!(viewer_for(Path::new("README.md")), "terminal");
        assert_eq!(viewer_for(Path::new("noext")), "terminal");
    }

    #[test]
    fn every_viewer_is_a_registry_app() {
        for name in ["a.png", "a.mp4", "a.csv", "a.pdf", "a.html", "a.txt"] {
            let id = viewer_for(Path::new(name));
            let entry = makepad_app_registry::find(id).expect(id);
            assert_eq!(entry.bin, format!("makepad-app-{id}"));
        }
    }
}
