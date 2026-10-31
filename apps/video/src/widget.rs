//! The video view: one clip, fitted whole to the window, with the library's
//! glass `PlayerControls` over it.
//!
//! The view is a makepad `VideoPlayer`: the platform's own player plays the
//! file (AVFoundation, GStreamer, Media Foundation, Android's MediaPlayer),
//! sound included, and the picture and the transport are the same ones every
//! makepad player shows. What this view adds is the app's keyboard (Left/
//! Right seek 5 s, Shift for 30 s, Up/Down volume, M mute, F picture only,
//! Escape/Q close) and the status line it reports to the shell.

use makepad_widgets::*;
use std::path::{Path, PathBuf};

/// Left/Right, and Shift+Left/Right.
const SEEK_STEP_SECS: f64 = 5.0;
const SEEK_STEP_BIG_SECS: f64 = 30.0;
/// One Up/Down press.
const VOLUME_STEP: f64 = 0.05;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MpVideoViewBase = #(MpVideoView::register_widget(vm))

    mod.widgets.MpVideoView = set_type_default() do mod.widgets.MpVideoViewBase{
        width: Fill
        height: Fill
        player := VideoPlayer{
            draw_bg.color: mod.theme.color_bg_app
            controls +: {
                top_row +: {
                    leading +: {
                        open_button := PlayerGlass{
                            width: Fit
                            height: 48
                            padding: Inset{left: 20 right: 20}
                            cursor: MouseCursor.Hand
                            grab_key_focus: false
                            draw_bg +: {corner_radius: 24.0}
                            PlayerClock{text: "Open"}
                        }
                    }
                }
            }
        }
    }
}

/// What the view tells the app shell about the clip, for the status line
/// and the window title.
#[derive(Clone, Debug, Default)]
pub enum MpVideoAction {
    Status {
        name: String,
        width: u32,
        height: u32,
        position_secs: f64,
        duration_secs: f64,
        playing: bool,
        ended: bool,
        volume_pct: i32,
        muted: bool,
        /// Chrome hidden (F): the shell hides its status row too.
        bare: bool,
        error: Option<String>,
    },
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct MpVideoView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    #[rust]
    path: Option<PathBuf>,
    /// Chrome hidden by F — picture only.
    #[rust]
    bare: bool,
    /// This process is a warm Quick Look viewer (`--preview`): Escape/Q
    /// hides the panel instead of ending the process. See `preview.rs`.
    #[rust]
    preview: bool,
    /// The last status sent, so the shell hears about changes only.
    #[rust]
    last_status: String,
}

impl Widget for MpVideoView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::KeyDown(ke) = event {
            self.handle_key(cx, ke);
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if self.view.view(cx, ids!(open_button)).finger_up(&actions).is_some_and(|fe| fe.is_over && !fe.cancelled) {
            pick_video(cx);
        }
        cx.extend_actions(actions);
        // The picker answers as a platform action, delivered to the whole
        // tree in an Actions event, not from inside this view.
        if let Event::Actions(actions) = event {
            if let Some(path) = actions.iter().find_map(|action| picked_video(action)) {
                self.open(cx, Path::new(&path));
            }
        }
        self.emit_status(cx);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}

impl MpVideoView {
    fn player(&self, cx: &Cx) -> VideoPlayerRef {
        self.view.video_player(cx, ids!(player))
    }

    /// Opens `path` and starts playing it.
    pub fn open(&mut self, cx: &mut Cx, path: &Path) {
        self.path = Some(path.to_path_buf());
        self.player(cx).open(cx, &path.to_string_lossy());
        self.emit_status(cx);
    }

    /// Quick Look v2: the panel hid, so stop playing and let the platform
    /// player go. An idle warm viewer costs nothing until the next `open`.
    pub fn unload(&mut self, cx: &mut Cx) {
        self.path = None;
        self.bare = false;
        let player = self.player(cx);
        player.stop(cx);
        player.set_controls_enabled(cx, true);
        // The volume and the mute are the listener's, not the clip's: they
        // survive an unload and apply to whatever plays next.
        self.emit_status(cx);
    }

    /// Start (or stop) muted. Set before [`open`](Self::open) for a run that
    /// must be silent; `M` and the volume keys still work from there.
    pub fn set_muted(&mut self, cx: &mut Cx, muted: bool) {
        self.player(cx).set_muted(cx, muted);
    }

    /// Tell the view it belongs to a warm Quick Look panel, where Escape/Q
    /// hides the panel rather than ending the process.
    pub fn set_preview(&mut self, preview: bool) {
        self.preview = preview;
    }

    /// Escape / Q: hide the panel when hosted as a Quick Look, quit
    /// otherwise.
    fn close(&mut self, cx: &mut Cx) {
        if crate::preview::hide_panel(cx, self.preview) {
            return;
        }
        cx.quit();
    }

    pub fn is_playing(&self, cx: &Cx) -> bool {
        self.player(cx).is_playing()
    }

    // Space is the controls' own (PlayerControls), like every makepad player.
    fn handle_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let step = if ke.modifiers.shift { SEEK_STEP_BIG_SECS } else { SEEK_STEP_SECS };
        let player = self.player(cx);
        match ke.key_code {
            KeyCode::Escape | KeyCode::KeyQ => self.close(cx),
            KeyCode::ArrowLeft => self.seek_by(cx, -step),
            KeyCode::ArrowRight => self.seek_by(cx, step),
            KeyCode::ArrowUp => player.set_volume(cx, player.volume() + VOLUME_STEP),
            KeyCode::ArrowDown => player.set_volume(cx, player.volume() - VOLUME_STEP),
            KeyCode::KeyM => player.set_muted(cx, !player.is_muted()),
            KeyCode::KeyF => {
                self.bare = !self.bare;
                player.set_controls_enabled(cx, !self.bare);
            }
            _ => {}
        }
    }

    fn seek_by(&mut self, cx: &mut Cx, delta: f64) {
        let player = self.player(cx);
        let here = player.video(cx).current_position_ms() as f64 / 1000.0;
        player.seek(cx, here + delta);
    }

    fn emit_status(&mut self, cx: &mut Cx) {
        let player = self.player(cx);
        let video = player.video(cx);
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let (width, height) = video.video_size();
        let position_secs = video.current_position_ms() as f64 / 1000.0;
        let status = MpVideoAction::Status {
            name,
            width: width as u32,
            height: height as u32,
            position_secs,
            duration_secs: video.total_duration_ms() as f64 / 1000.0,
            playing: player.is_playing(),
            ended: video.has_completed(),
            volume_pct: (player.volume() * 100.0).round() as i32,
            muted: player.is_muted(),
            bare: self.bare,
            error: video.last_error(),
        };
        // The status line prints whole seconds, so a report per decoded
        // frame would only repeat itself: send it when what it prints moves.
        let key = match &status {
            MpVideoAction::Status { name, width, height, duration_secs, playing, ended, volume_pct, muted, bare, error, .. } => format!(
                "{name}|{width}x{height}|{}|{duration_secs}|{playing}|{ended}|{volume_pct}|{muted}|{bare}|{error:?}",
                position_secs.floor()
            ),
            MpVideoAction::None => String::new(),
        };
        if key != self.last_status {
            self.last_status = key;
            cx.widget_action(self.widget_uid(), status);
        }
    }
}

/// Tags our file dialog, so its answer is not confused with another's.
const PICK_VIDEO: LiveId = live_id!(pick_video);

/// The Open chip: the platform's file picker (on Android the document
/// picker, which answers with a `content://` URI the player plays as is).
fn pick_video(cx: &mut Cx) {
    use makepad_widgets::makepad_platform::file_dialogs::FileDialog;
    let extensions = ["mp4", "mkv", "webm", "mov", "m4v", "avi", "ts", "mpg", "mpeg", "m3u8"];
    let dialog = FileDialog::new()
        .set_id(PICK_VIDEO)
        .set_title("Open Video".to_string())
        .add_filter("Video".to_string(), extensions.iter().map(|e| e.to_string()).collect())
        .add_filter("All Files".to_string(), vec!["*".to_string()]);
    cx.open_select_file_dialog(dialog);
}

fn picked_video(action: &Action) -> Option<String> {
    use makepad_widgets::makepad_platform::file_dialogs::FileDialogAction;
    let picked = action.downcast_ref::<FileDialogAction>()?;
    if picked.id() != PICK_VIDEO {
        return None;
    }
    Some(picked.path()?.to_string_lossy().into_owned())
}

/// `m:ss` under an hour, `h:mm:ss` over it — a stable width in a monospace
/// readout for as long as the clip's own length allows.
pub fn format_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".to_string();
    }
    let total = secs as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_reads_as_a_clock() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(9.4), "0:09");
        assert_eq!(format_time(65.0), "1:05");
        assert_eq!(format_time(600.0), "10:00");
        assert_eq!(format_time(3661.0), "1:01:01");
        // A container that reports no duration must not print garbage.
        assert_eq!(format_time(-1.0), "0:00");
        assert_eq!(format_time(f64::NAN), "0:00");
    }
}
