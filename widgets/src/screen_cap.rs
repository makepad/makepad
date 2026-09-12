//! ScreenCap — CTRL+F10 records the window to an mp4, picture and sound.
//!
//! One widget, hardcoded into [`crate::window::Window`] the way the tweaker
//! and the nav control are, so every Makepad app can record itself without
//! wiring anything up. Ctrl+F10 starts at up to 60fps; Ctrl+Shift+F10 starts
//! at 120fps. Either shortcut stops the current recording. While it records,
//! a red dot sits in the top-right corner of the window (and therefore in
//! the file — the indicator is drawn into the same pass the recorder reads
//! back).
//!
//! The key sits beside the AI's plain F10 and the tweaker's Shift+F10 on
//! purpose: one assistant key, one designer key, one recorder key.
//!
//! Both halves come off platform seams added for this:
//!
//! - picture: `makepad_platform::screen_capture` — a standing sink on the
//!   window's presented frames. Same readback the `--remote` `/g` grab uses,
//!   but continuous and without the PNG encode.
//! - sound: `makepad_platform::audio_output_tap` — a fork of the buffers the
//!   app hands its output device, taken right after the app fills them. This
//!   is the app's OWN audio, not a system loopback: no screen-recording
//!   permission, no other app's sound in the file.
//!
//! The mp4 is written by `makepad_platform::video_file::VideoFileEncoder`
//! (VideoToolbox / Media Foundation / GStreamer), H.264 + AAC, on a worker
//! thread — neither the GPU completion thread nor the realtime audio thread
//! ever touches the encoder. Files land in `local/screencap/`, one per
//! recording, named for when it was taken.
//!
//! Video is constant-rate — 60fps by default, with `max_fps` or
//! `MAKEPAD_SCREENCAP_FPS` able to lower that rate. Ctrl+Shift+F10 explicitly
//! opts into 120fps (Studio feedback sessions reserve that shortcut for
//! selecting feedback). The frame index is taken from the wall clock, and
//! the audio position derived from that index —
//! so an encoder that falls behind leaves a gap in both tracks rather than
//! letting sound drift away from picture. A window that presents nothing is
//! kept ticking by a pass repaint, which costs a re-present of the existing
//! draw lists and no widget redraw.
//!
//! Key events are not scoped to a window in Makepad, so in a multi-window app
//! Ctrl+F10 starts one recording per window, each into its own file. That is
//! the honest reading of "record the window" when there is more than one.

use crate::makepad_draw::audio::AudioBuffer;
use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use makepad_platform::audio_output_tap::{add_audio_output_tap, remove_audio_output_tap};
use makepad_platform::screen_capture::{
    add_screen_capture, remove_screen_capture, ScreenCaptureOptions,
};
use makepad_platform::script::timer::script_local_utc_offset_secs;
use makepad_platform::video_file::{
    PcmAudioTrackOptions, VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
};
use makepad_platform::thread::{CancellationToken, TaskHandle, ThreadOptions};

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.draw.KeyCode

    set_type_default() do #(DrawRecDot::script_shader(vm)){
        ..mod.draw.DrawColor
        color: #xff3b30
        // 1.0 while recording, 0.0 while the encoder finalizes the file.
        // Deliberately NOT animated: the dot is redrawn only when that state
        // changes, so a recording costs the app no extra widget redraws.
        armed: 1.0

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let c = self.rect_size * 0.5
            let r = min(self.rect_size.x, self.rect_size.y) * 0.5 - 1.5
            sdf.circle(c.x, c.y, r)
            // Recording: a filled dot. Finalizing: the same circle as a ring.
            sdf.fill_keep(mix(vec4(0.0, 0.0, 0.0, 0.0), self.color, self.armed))
            // A dark rim keeps the dot readable over light UI.
            sdf.stroke(mix(self.color, vec4(0.0, 0.0, 0.0, 0.45), self.armed), 1.5)
            return sdf.result
        }
    }

    mod.widgets.ScreenCapBase = #(ScreenCap::register_widget(vm))

    mod.widgets.ScreenCap = set_type_default() do mod.widgets.ScreenCapBase{
        width: Fill
        height: Fill
        hotkey: KeyCode.F10
        hotkey_shift: false
        hotkey_ctrl: true
        dot_size: 13.0
        dot_margin: 12.0
        max_fps: 60.0
    }
}

/// Frames per second written to the file, and the ceiling on how often the
/// window is read back. Normal recording is capped at 60; holding Shift
/// with the default Ctrl+F10 shortcut opts into 120 for that session only.
const DEFAULT_FPS: u32 = 60;
const HIGH_FPS: u32 = 120;
const MANAGED_MAX_FPS: u32 = 15;
const MANAGED_MAX_INSPECTION_PIXELS: u64 = 16_777_216;
static MANAGED_RECORDINGS: AtomicUsize = AtomicUsize::new(0);
static CAPTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// Used only when the app never opened an output device, so the tap never
/// reported a rate — the track is then silence at a plausible rate.
const FALLBACK_AUDIO_RATE: u32 = 48_000;
/// How long to let the audio device announce its sample rate before the
/// encoder is created with the fallback.
const AUDIO_RATE_GRACE: Duration = Duration::from_millis(300);
/// Tap backlog ceiling. Reaching it means the encoder thread is wedged; drop
/// the oldest rather than grow without bound behind a realtime callback.
const AUDIO_BACKLOG_SECONDS: usize = 4;

/// The requested normal recording rate. The entry points cap this at 60
/// (15 for managed evidence); the high-rate shortcut selects 120 directly.
fn capture_fps(max_fps: f64) -> u32 {
    static ENV: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    let env = *ENV.get_or_init(|| {
        std::env::var("MAKEPAD_SCREENCAP_FPS")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
    });
    let fps = env.unwrap_or(max_fps);
    if fps >= 1.0 {
        (fps.round() as u32).clamp(1, 240)
    } else {
        DEFAULT_FPS
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRecDot {
    #[deref]
    draw_super: DrawColor,
    #[live]
    pub armed: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ScreenCap {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_dot: DrawRecDot,
    /// The key that starts and stops recording. Seen before key focus, so it
    /// works while a text input has the caret.
    #[live(KeyCode::F10)]
    hotkey: KeyCode,
    /// Whether the normal hotkey needs Shift held. With the default Ctrl
    /// binding, adding Shift selects the high frame rate instead.
    #[live(false)]
    hotkey_shift: bool,
    /// Whether the hotkey needs Ctrl held. Ctrl+F10 by default, so the
    /// recorder sits beside the AI's bare F10 without stealing it.
    #[live(true)]
    hotkey_ctrl: bool,
    #[live(13.0)]
    dot_size: f64,
    #[live(12.0)]
    dot_margin: f64,
    #[live(60.0)]
    max_fps: f64,
    /// Only a Studio-managed recording (an absolute directory handed over by
    /// the flow) sets this. Every other recording, in every app of a repo,
    /// lands in that repo's `local/screencap/` (see `repo_screencap_dir`).
    #[live]
    output_dir: String,
    #[rust]
    next_frame: NextFrame,
    /// Set by [`crate::window::Window`] so the sink follows one window in a
    /// multi-window app. `None` records whichever window presents.
    #[rust]
    window_id: Option<usize>,
    #[rust]
    session: Option<Session>,
    /// Set on every frame tick while busy; `Window` drains it and repaints
    /// its pass, which is what makes an otherwise still app keep presenting
    /// frames for the recorder to read back.
    #[rust]
    repaint_requested: bool,
    /// Set when the indicator's state changed. A repaint re-presents the draw
    /// lists as they stand; only a redraw runs `draw_indicator` again, so the
    /// dot appearing and disappearing needs this separate, rare signal.
    #[rust]
    redraw_requested: bool,
    #[rust]
    managed: bool,
    #[rust]
    managed_checked: bool,
}

#[derive(Clone, Debug, Default)]
pub enum ScreenCapAction {
    Started(PathBuf),
    Finished(PathBuf),
    Failed(String),
    #[default]
    None,
}

impl ScreenCap {
    pub fn is_managed(&self) -> bool { self.managed }

    pub fn managed_recordings_pending() -> bool {
        MANAGED_RECORDINGS.load(Ordering::Acquire) != 0
    }

    /// Studio supplies an owned per-run directory. Environment lookup and
    /// filename construction do no filesystem I/O; the encoder owns writes.
    pub fn start_managed_once(&mut self, cx: &mut Cx) {
        if self.managed_checked || self.window_id.is_none() { return; }
        self.managed_checked = true;
        let Some(directory) = std::env::var_os("MAKEPAD_STUDIO_RECORDING_DIR") else { return; };
        let directory = PathBuf::from(directory);
        if !directory.is_absolute() || directory.as_os_str().len() > 4096 {
            cx.widget_action(self.uid, ScreenCapAction::Failed("Studio recording directory must be absolute and at most 4096 bytes".into()));
            return;
        }
        let Some(directory) = directory.to_str() else {
            cx.widget_action(self.uid, ScreenCapAction::Failed("Studio recording directory must be UTF-8".into()));
            return;
        };
        self.managed = true;
        self.output_dir = directory.into();
        self.start(cx);
    }

    pub fn is_recording(&self) -> bool {
        self.session.as_ref().map(|s| !s.stopping).unwrap_or(false)
    }

    /// True while recording OR while the encoder is finalizing — either way
    /// the indicator is on screen and the window must keep presenting.
    pub fn is_busy(&self) -> bool {
        self.session.is_some()
    }

    /// Bind the recorder to one window. Called by `Window` before it forwards
    /// events; a standalone `ScreenCap` leaves it unset and takes any window.
    pub fn set_window_id(&mut self, window_id: usize) {
        self.window_id = Some(window_id);
    }

    /// True once per frame tick while a recording is running or finalizing.
    /// The owner must answer it by repainting the window's pass: redrawing
    /// this widget alone would not make the window present a new frame.
    pub fn take_repaint_request(&mut self) -> bool {
        std::mem::take(&mut self.repaint_requested)
    }

    /// True when the indicator needs to be drawn again — recording started,
    /// stopped, or finished finalizing. The owner must answer it with a real
    /// redraw of its content; a pass repaint would just re-present the frame
    /// that has no dot in it.
    pub fn take_redraw_request(&mut self) -> bool {
        std::mem::take(&mut self.redraw_requested)
    }

    pub fn toggle(&mut self, cx: &mut Cx) {
        self.toggle_at_fps(cx, capture_fps(self.max_fps).min(DEFAULT_FPS));
    }

    fn toggle_at_fps(&mut self, cx: &mut Cx, fps: u32) {
        // Studio owns the lifetime of automatic evidence. The manual shortcut
        // remains unchanged for ordinary app launches without the managed env.
        if self.managed { return; }
        if self.is_recording() {
            self.stop(cx);
        } else if self.session.is_none() {
            self.start_at_fps(cx, fps);
        }
        // While a file is still finalizing, either shortcut is a no-op;
        // never race a second encoder onto the same directory.
    }

    pub fn start(&mut self, cx: &mut Cx) {
        self.start_at_fps(cx, capture_fps(self.max_fps).min(DEFAULT_FPS));
    }

    fn start_at_fps(&mut self, cx: &mut Cx, fps: u32) {
        if self.session.is_some() {
            return;
        }
        let dir = if self.managed && !self.output_dir.is_empty() {
            PathBuf::from(&self.output_dir)
        } else {
            repo_screencap_dir()
        };
        let path = capture_path(&dir, self.window_id);
        let fps = if self.managed { fps.min(MANAGED_MAX_FPS) } else { fps };
        let session = Session::start(cx, path.clone(), self.window_id, fps, self.managed);
        log!("ScreenCap: recording to {}", path.display());
        self.session = Some(session);
        self.next_frame = cx.new_next_frame();
        self.redraw_requested = true;
        cx.widget_action(self.uid, ScreenCapAction::Started(path));
    }

    /// Stop capturing and let the encoder finalize in the background. The
    /// indicator stays up (as a ring) until the file is closed; the result is
    /// reported from `handle_event`.
    pub fn stop(&mut self, cx: &mut Cx) {
        if let Some(session) = &mut self.session {
            session.begin_stop();
            self.next_frame = cx.new_next_frame();
            self.redraw_requested = true;
        }
    }

    /// Reap a finished encoder thread. Returns the action to emit, if any.
    fn poll_finish(&mut self) -> Option<ScreenCapAction> {
        let session = self.session.as_mut()?;
        if !session.is_finished() {
            return None;
        }
        let result = session.try_finish()?;
        self.session = None;
        self.redraw_requested = true;
        Some(match result {
            Ok(path) => {
                log!("ScreenCap: wrote {}", path.display());
                ScreenCapAction::Finished(path)
            }
            Err(err) => {
                error!("ScreenCap: {}", err);
                ScreenCapAction::Failed(err)
            }
        })
    }

    /// Draw the indicator pinned to the top-right of `rect`. Split out so
    /// `Window` can draw it last, over everything, without giving it a slot
    /// in the layout.
    pub fn draw_indicator(&mut self, cx: &mut Cx2d, rect: Rect) {
        if !self.is_busy() || self.managed {
            return;
        }
        let size = self.dot_size.max(4.0);
        if rect.size.x < size * 2.0 || rect.size.y < size * 2.0 {
            return;
        }
        let m = self.dot_margin;
        self.draw_dot.armed = if self.is_recording() { 1.0 } else { 0.0 };
        let dot_rect = Rect {
            pos: dvec2(rect.pos.x + rect.size.x - m - size, rect.pos.y + m),
            size: dvec2(size, size),
        };
        self.draw_dot.draw_abs(cx, dot_rect);
    }
}

impl Widget for ScreenCap {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::KeyDown(ke) = event {
            if ke.key_code == self.hotkey
                && ke.modifiers.control == self.hotkey_ctrl
                && !ke.is_repeat
            {
                if ke.modifiers.shift == self.hotkey_shift {
                    self.toggle(cx);
                } else if !self.hotkey_shift
                    && ke.modifiers.shift
                    && ke.modifiers.control
                    && !ke.modifiers.alt
                    && !ke.modifiers.logo
                    && !cx.global::<crate::ai_slot::AiSlotRequests>().feedback_enabled
                {
                    self.toggle_at_fps(cx, HIGH_FPS);
                }
            }
        }
        if self.next_frame.is_event(event).is_some() {
            if let Some(action) = self.poll_finish() {
                cx.widget_action(self.uid, action);
            }
            if self.is_busy() {
                // Keep the window presenting: a still app presents nothing,
                // and a recorder with no frames is an empty file.
                self.next_frame = cx.new_next_frame();
                self.repaint_requested = true;
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_indicator(cx, rect);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// Recording session
// ---------------------------------------------------------------------------

struct CapturedFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Default)]
struct FrameSlot {
    /// Only the NEWEST presented frame is kept. The encoder samples on its own
    /// recording clock, so queueing every 120Hz present would just buy latency and
    /// tens of megabytes of backlog.
    pending: Option<CapturedFrame>,
    /// Buffer handed back by the encoder thread, reused by the capture
    /// callback so a steady-state recording allocates nothing per frame.
    spare: Vec<u8>,
    dropped: u64,
    wake_generation: u64,
}

#[derive(Default)]
struct AudioQueue {
    /// 0 until the first tapped buffer says what the device runs at.
    rate: u32,
    /// Interleaved stereo, the shape the encoder's AAC input wants.
    samples: VecDeque<i16>,
    /// Samples thrown away because the encoder could not keep up.
    dropped: u64,
}

struct Session {
    slot: Arc<(Mutex<FrameSlot>, Condvar)>,
    stop: Arc<AtomicBool>,
    /// Cleared by the encoder thread on exit, so the UI can poll for the
    /// finalize without blocking on a join.
    running: Arc<AtomicBool>,
    join: Option<TaskHandle<Result<PathBuf, String>>>,
    stopping: bool,
}

impl Session {
    fn start(cx: &Cx, path: PathBuf, window_id: Option<usize>, fps: u32, managed: bool) -> Self {
        let slot = Arc::new((Mutex::new(FrameSlot::default()), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let audio = Arc::new(Mutex::new(AudioQueue::default()));
        let running = Arc::new(AtomicBool::new(true));

        let thread_slot = slot.clone();
        let thread_audio = audio.clone();
        let thread_stop = stop.clone();
        let thread_running = running.clone();
        let pending = ManagedRecording::new(managed);
        let join = cx
            .thread_spawner()
            .spawn_worker(
                ThreadOptions { name: Some("makepad-screencap".into()), ..Default::default() },
                move || {
                    let _pending = pending;
                    let capture_slot = thread_slot.clone();
                    let capture_stop = thread_stop.clone();
                    let capture_id = add_screen_capture(
            ScreenCaptureOptions {
                window_id,
                max_fps: fps as f64,
            },
            move |frame| {
                if capture_stop.load(Ordering::Acquire) {
                    return;
                }
                let (lock, cvar) = &*capture_slot;
                let Ok(mut slot) = lock.try_lock() else {
                    return;
                };
                let mut buf = match slot.pending.take() {
                    Some(old) => {
                        slot.dropped += 1;
                        old.rgba
                    }
                    None => std::mem::take(&mut slot.spare),
                };
                buf.clear();
                buf.extend_from_slice(frame.rgba);
                slot.pending = Some(CapturedFrame {
                    width: frame.width,
                    height: frame.height,
                    rgba: buf,
                });
                slot.wake_generation = slot.wake_generation.wrapping_add(1);
                drop(slot);
                cvar.notify_one();
            },
        );

        let tap_audio = thread_audio.clone();
        let tap_id = add_audio_output_tap(move |info, buffer| {
            let Ok(mut queue) = tap_audio.try_lock() else {
                return;
            };
            if queue.rate == 0 {
                queue.rate = info.sample_rate.round().max(1.0) as u32;
            }
            let cap = queue.rate as usize * 2 * AUDIO_BACKLOG_SECONDS;
            append_interleaved_i16(&mut queue.samples, buffer);
            if queue.samples.len() > cap {
                let excess = queue.samples.len() - cap;
                queue.samples.drain(..excess);
                queue.dropped += excess as u64;
            }
        });

                    let attachments = CaptureAttachments { capture_id, tap_id };
                    let mut info = RecordingInfo::default();
                    let result = (|| {
                        let parent = path.parent().ok_or("Capture has no output directory")?;
                        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                        // Reserve only this generated name; never overwrite a
                        // previous run if a filesystem/clock collision occurs.
                        std::fs::OpenOptions::new().write(true).create_new(true).open(&path)
                            .map_err(|error| error.to_string())?;
                        encode_loop(&path, thread_slot, thread_audio, thread_stop, fps, managed, window_id, &mut info)
                    })();
                    drop(attachments);
                    if managed {
                        if let Err(error) = write_managed_result(&path, window_id, fps, &info, false, Some(&result)) {
                            error!("ScreenCap: capture evidence could not be saved: {}", error);
                        }
                    }
                    thread_running.store(false, Ordering::Release);
                    result.map(|_| path)
                },
            )
            .ok();
        if join.is_none() {
            running.store(false, Ordering::Release);
        }

        Self {
            slot,
            stop,
            running,
            join,
            stopping: false,
        }
    }

    /// Detach from the live streams and tell the encoder to finalize. The
    /// file is not closed yet — `is_finished` reports that.
    fn begin_stop(&mut self) {
        if self.stopping {
            return;
        }
        self.stopping = true;
        self.stop.store(true, Ordering::Release);
        self.slot.1.notify_all();
    }

    fn is_finished(&self) -> bool {
        !self.running.load(Ordering::Acquire) || self.join.as_ref().is_some_and(TaskHandle::is_finished)
    }

    /// Reap the encoder's result — never a blocking join, which
    /// `TaskHandle` refuses from the UI thread. `is_finished` already told
    /// the caller the worker set `running` false, so `try_take` normally
    /// answers at once; `None` here just means the completion has not
    /// posted yet and the caller polls again next frame.
    fn try_finish(&mut self) -> Option<Result<PathBuf, String>> {
        match self.join.take() {
            Some(mut handle) => match handle.try_take() {
                Some(result) => Some(match result {
                    Ok(outcome) => outcome,
                    Err(task_error) => Err(format!("encoder thread panicked: {task_error}")),
                }),
                None => {
                    self.join = Some(handle);
                    None
                }
            },
            None => Some(Err("encoder thread could not be started".to_string())),
        }
    }
}

struct CaptureAttachments { capture_id: u64, tap_id: u64 }
impl Drop for CaptureAttachments {
    fn drop(&mut self) {
        // Registry locks belong to the encoder worker, never the window/UI.
        remove_screen_capture(self.capture_id);
        remove_audio_output_tap(self.tap_id);
    }
}

struct ManagedRecording(bool);
impl ManagedRecording {
    fn new(managed: bool) -> Self {
        if managed { MANAGED_RECORDINGS.fetch_add(1, Ordering::AcqRel); }
        Self(managed)
    }
}
impl Drop for ManagedRecording {
    fn drop(&mut self) {
        if self.0 { MANAGED_RECORDINGS.fetch_sub(1, Ordering::AcqRel); }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A window closed mid-recording still gets a playable file: detach,
        // signal, and let the encoder finalize on its own thread.
        self.begin_stop();
    }
}

/// Planar f32 (makepad's `AudioBuffer` layout) to interleaved stereo i16.
/// Mono is duplicated; anything wider than stereo keeps its first two
/// channels, which is what the AAC track carries.
fn append_interleaved_i16(out: &mut VecDeque<i16>, buffer: &AudioBuffer) {
    let frames = buffer.frame_count();
    let channels = buffer.channel_count();
    if frames == 0 || channels == 0 {
        return;
    }
    let left = buffer.channel(0);
    let right = if channels > 1 {
        buffer.channel(1)
    } else {
        left
    };
    let frames = frames.min(left.len()).min(right.len());
    out.reserve(frames * 2);
    for i in 0..frames {
        out.push_back(to_i16(left[i]));
        out.push_back(to_i16(right[i]));
    }
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// `<repo>/local/screencap`, where `<repo>` is the nearest ancestor of the
/// working directory that has a `.git` entry or a `local/` directory. Apps
/// are launched from their crate directories as often as from the repo root;
/// the recordings must not scatter with them. Falls back to the working
/// directory itself when no repo is found.
pub fn repo_screencap_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut dir = cwd.clone();
    for _ in 0..8 {
        if dir.join(".git").exists() || dir.join("local").is_dir() {
            return dir.join("local").join("screencap");
        }
        if !dir.pop() {
            break;
        }
    }
    cwd.join("local").join("screencap")
}

fn capture_path(dir: &Path, window: Option<usize>) -> PathBuf {
    let app = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "app".into());
    let stamp = local_timestamp();
    let sequence = CAPTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    dir.join(format!("screencap-{app}-{stamp}-p{}-w{}-{nanos}-{sequence}.mp4", std::process::id(), window.unwrap_or(0)))
}

/// `YYYYmmdd-HHMMSS`, so the files sort by when they were taken. Local time
/// when the host has told the platform its zone offset
/// (`set_script_local_utc_offset_secs`), UTC otherwise — the same convention
/// every other timestamp the platform formats follows.
fn local_timestamp() -> String {
    let now = Cx::time_now().max(0.0) as i64;
    let (y, mo, d, h, mi, s) = civil_from_unix(now.saturating_add(script_local_utc_offset_secs()));
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// Epoch seconds to (year, month, day, hour, minute, second), via Howard
/// Hinnant's civil_from_days.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (
        if m <= 2 { y + 1 } else { y },
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// The encoder thread. Owns the file for its whole life; nothing else here
/// ever blocks on it.
fn encode_loop(
    path: &Path,
    slot: Arc<(Mutex<FrameSlot>, Condvar)>,
    audio: Arc<Mutex<AudioQueue>>,
    stop: Arc<AtomicBool>,
    fps: u32,
    save_final_frame: bool,
    window_id: Option<usize>,
    info: &mut RecordingInfo,
) -> Result<(), String> {
    let fps = fps.max(1);
    let wait = CancellationToken::new();
    // Wait for the window's first presented frame: it fixes the resolution
    // for the whole file (an mp4 track cannot change size mid-stream).
    let Some(first) = take_frame(&slot, &stop, &wait, None) else {
        return Err("stopped before the window presented a frame".to_string());
    };
    let width = (first.width & !1).max(2);
    let height = (first.height & !1).max(2);
    info.width = width;
    info.height = height;
    info.original_width = first.width;
    info.original_height = first.height;

    // Let the audio device announce its rate before the AAC track is created.
    let grace_until = Cx::monotonic_now() + AUDIO_RATE_GRACE.as_secs_f64();
    let audio_rate = loop {
        let rate = audio.lock().map(|q| q.rate).unwrap_or(0);
        if rate != 0 {
            break rate;
        }
        if Cx::monotonic_now() >= grace_until || stopped(&stop) {
            break FALLBACK_AUDIO_RATE;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if !wait_for_capture_wake(&slot, &stop) {
            break FALLBACK_AUDIO_RATE;
        }
        #[cfg(target_arch = "wasm32")]
        let _ = wait.wait_until((Cx::monotonic_now() + 0.010).min(grace_until));
    };

    let options = VideoFileEncoderOptions {
        codec: VideoFileCodec::H264,
        width,
        height,
        fps_num: fps,
        fps_den: 1,
        video_bitrate_bps: bitrate_for(width, height, fps),
        audio: Some(PcmAudioTrackOptions {
            sample_rate: audio_rate,
            channels: 2,
            aac_bitrate_bps: 128_000,
        }),
        keyframe_only: false,
    };
    log!("ScreenCap: {}x{} at {}fps, H.264 target {:.1}Mbps, app audio AAC 128kbps",
        width, height, fps, options.video_bitrate_bps as f64 / 1_000_000.0);
    let path_str = path.to_string_lossy().to_string();
    let mut encoder =
        VideoFileEncoder::new(&path_str, options).map_err(|err| format!("{path_str}: {err}"))?;

    let mut canvas = vec![0u8; width as usize * height as usize * 4];
    blit_into(&mut canvas, width, height, &first.rgba, first.width, first.height);
    // Retain only the latest original drawable for pixel-accurate inspection;
    // the fixed MP4 canvas may have padding or scaling after a window resize.
    let mut latest_frame = first;

    let start = Cx::monotonic_now();
    let mut frame_index: u64 = 0;
    let mut audio_pushed: u64 = 0;
    let mut encode_error: Option<String> = None;
    let mut encoded: u64 = 0;
    let mut push_time = 0.0;
    let mut last_preview = 0.0;

    loop {
        // Video frame `frame_index` covers [n/fps, (n+1)/fps).
        let pts_100ns = (frame_index as u128 * 10_000_000u128 / fps as u128) as i64;
        let t0 = Cx::monotonic_now();
        let push = encoder.push_frame_rgba8(&canvas, Some(pts_100ns));
        push_time += Cx::monotonic_now() - t0;
        encoded += 1;
        if let Err(err) = push {
            encode_error = Some(format!("video frame {frame_index}: {err}"));
            break;
        }
        let audio_target = ((frame_index + 1) as u128 * audio_rate as u128 / fps as u128) as u64;
        if let Err(err) = push_audio_through(&mut encoder, &audio, audio_target, &mut audio_pushed) {
            encode_error = Some(err);
            break;
        }
        info.frames = encoded;
        info.elapsed_ms = ((Cx::monotonic_now() - start).max(0.0) * 1000.0) as u64;
        if save_final_frame && (!info.preview_written || Cx::monotonic_now() - last_preview >= 1.0) {
            let published = (|| {
                write_live_preview(path, width, height, &canvas)?;
                info.preview_written = true;
                info.current_written = write_inspection_frame(path, "current.png", &latest_frame)?;
                info.current_width = latest_frame.width;
                info.current_height = latest_frame.height;
                info.frame_sequence = encoded;
                write_managed_result(path, window_id, fps, info, true, None)
            })();
            if let Err(error) = published {
                // Finalize an otherwise playable MP4 even if its evidence
                // sidecar or image could not be published.
                encode_error = Some(format!("publishing recording evidence: {error}"));
                break;
            }
            last_preview = Cx::monotonic_now();
        }

        // Sleep to the next tick, then take whatever the window presented
        // meanwhile. Nothing new = the screen did not change; the frame is
        // repeated so the file keeps real time.
        let deadline = start + tick_duration(frame_index + 1, fps).as_secs_f64();
        // Take-frame returns immediately when a newer presentation is queued.
        // Pace independently so a fast display cannot exceed the MP4 rate.
        let _ = wait.wait_until(deadline);
        let next = take_frame(&slot, &stop, &wait, Some(deadline));
        if let Some(frame) = next {
            info.original_width = frame.width;
            info.original_height = frame.height;
            blit_into(&mut canvas, width, height, &frame.rgba, frame.width, frame.height);
            let previous = std::mem::replace(&mut latest_frame, frame);
            recycle(&slot, previous.rgba);
        } else if stopped(&stop) {
            break;
        }

        // Wall clock decides the next index, so an encoder that fell behind
        // leaves a gap instead of stretching the recording.
        let elapsed = Cx::monotonic_now() - start;
        let wanted = (elapsed * fps as f64).round() as u64;
        frame_index = wanted.max(frame_index + 1);
    }

    // The one line that says whether the requested rate was actually held.
    // `encode ms/frame` over the frame budget (1000/fps) is the ceiling; when
    // it exceeds the budget the wall clock leaves gaps and `gaps` counts them.
    let wall = (Cx::monotonic_now() - start).max(1e-6);
    let dropped = slot.0.lock().map(|s| s.dropped).unwrap_or(0);
    let audio_dropped = audio.lock().map(|q| q.dropped).unwrap_or(0);
    log!(
        "ScreenCap: {}x{} asked {}fps, held {:.1}fps over {:.1}s          ({} encoded, {} gaps, {} presents coalesced, {} audio samples dropped);          encode {:.2}ms/frame of {:.2}ms budget",
        width,
        height,
        fps,
        encoded as f64 / wall,
        wall,
        encoded,
        frame_index.saturating_sub(encoded.saturating_sub(1)),
        dropped,
        audio_dropped,
        push_time * 1000.0 / encoded.max(1) as f64,
        1000.0 / fps as f64,
    );

    encoder
        .finish()
        .map_err(|err| format!("finalizing {path_str}: {err}"))?;
    if save_final_frame {
        info.final_written = write_inspection_frame(path, "png", &latest_frame)?;
        info.current_written = info.final_written;
        info.current_width = latest_frame.width;
        info.current_height = latest_frame.height;
        info.frame_sequence = encoded;
    }
    match encode_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[derive(Default)]
struct RecordingInfo {
    width: u32,
    height: u32,
    original_width: u32,
    original_height: u32,
    frames: u64,
    elapsed_ms: u64,
    preview_written: bool,
    current_written: bool,
    current_width: u32,
    current_height: u32,
    frame_sequence: u64,
    final_written: bool,
}

fn write_inspection_frame(path: &Path, extension: &str, frame: &CapturedFrame) -> Result<bool, String> {
    let pixels = u64::from(frame.width) * u64::from(frame.height);
    if pixels == 0 || pixels > MANAGED_MAX_INSPECTION_PIXELS {
        return Ok(false);
    }
    let png = Cx::encode_rgba_as_png(frame.width, frame.height, &frame.rgba)?;
    let temporary = path.with_extension(format!("{extension}.tmp"));
    std::fs::write(&temporary, png).map_err(|error| error.to_string())?;
    std::fs::rename(temporary, path.with_extension(extension)).map_err(|error| error.to_string())?;
    Ok(true)
}

fn write_live_preview(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let scale = (640.0 / width.max(1) as f64).min(480.0 / height.max(1) as f64).min(1.0);
    let preview_width = (width as f64 * scale).round().max(1.0) as u32;
    let preview_height = (height as f64 * scale).round().max(1.0) as u32;
    let mut preview = vec![0; preview_width as usize * preview_height as usize * 4];
    blit_into(&mut preview, preview_width, preview_height, rgba, width, height);
    let png = Cx::encode_rgba_as_png(preview_width, preview_height, &preview)?;
    let temporary = path.with_extension("preview.png.tmp");
    std::fs::write(&temporary, png).map_err(|error| error.to_string())?;
    std::fs::rename(temporary, path.with_extension("preview.png")).map_err(|error| error.to_string())
}

fn write_managed_result(path: &Path, window: Option<usize>, fps: u32, info: &RecordingInfo, active: bool, result: Option<&Result<(), String>>) -> Result<(), String> {
    use crate::makepad_micro_serde::SerJson;
    let identity = |name| std::env::var(name).unwrap_or_default().serialize_json();
    let video = path.to_string_lossy().to_string().serialize_json();
    let final_frame = if info.final_written { path.with_extension("png").to_string_lossy().to_string().serialize_json() } else { "null".into() };
    let preview = if info.final_written { final_frame.clone() } else if info.preview_written { path.with_extension("preview.png").to_string_lossy().to_string().serialize_json() } else { "null".into() };
    let current_frame = if info.final_written { final_frame.clone() } else if info.current_written { path.with_extension("current.png").to_string_lossy().to_string().serialize_json() } else { "null".into() };
    let inspection_error = if info.frames != 0 && !info.current_written {
        "Original drawable exceeds the 16-megapixel inspection limit".to_owned().serialize_json()
    } else { "null".into() };
    let error = result.and_then(|result| result.as_ref().err()).map(|error| error.serialize_json()).unwrap_or_else(|| "null".into());
    let complete = result.is_some_and(Result::is_ok);
    let body = format!("{{\"kind\":\"studio_recording\",\"flow\":{},\"artifact\":{},\"run\":{},\"commit\":{},\"pid\":{},\"window\":{},\"fps\":{fps},\"width\":{},\"height\":{},\"original_width\":{},\"original_height\":{},\"frames\":{},\"elapsed_ms\":{},\"current_width\":{},\"current_height\":{},\"frame_sequence\":{},\"active\":{active},\"complete\":{complete},\"video\":{video},\"preview\":{preview},\"current_frame\":{current_frame},\"final_frame\":{final_frame},\"inspection_error\":{inspection_error},\"error\":{error}}}",
        identity("MAKEPAD_STUDIO_FLOW_ID"), identity("MAKEPAD_STUDIO_ARTIFACT_ID"), identity("MAKEPAD_STUDIO_RUN_ID"), identity("MAKEPAD_STUDIO_REVISION"), std::process::id(), window.unwrap_or(0),
        info.width, info.height, info.original_width, info.original_height, info.frames, info.elapsed_ms, info.current_width, info.current_height, info.frame_sequence);
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, body).map_err(|error| error.to_string())?;
    std::fs::rename(temporary, path.with_extension("json")).map_err(|error| error.to_string())
}

fn tick_duration(index: u64, fps: u32) -> Duration {
    Duration::from_nanos(index * 1_000_000_000 / fps as u64)
}

/// A quarter bit per pixel per frame preserves fine text and map outlines
/// during motion. Scale with both native resolution and capture rate;
/// the previous 40 Mbps ceiling starved large high-refresh drawables.
fn bitrate_for(width: u32, height: u32, fps: u32) -> u32 {
    let pixels = width as u64 * height as u64;
    let bps = pixels * fps as u64 / 4;
    bps.clamp(8_000_000, 160_000_000) as u32
}

fn stopped(stop: &AtomicBool) -> bool {
    stop.load(Ordering::Acquire)
}

fn recycle(slot: &Arc<(Mutex<FrameSlot>, Condvar)>, buffer: Vec<u8>) {
    if let Ok(mut slot) = slot.0.lock() {
        if slot.spare.capacity() < buffer.capacity() {
            slot.spare = buffer;
        }
    }
}

/// Block the encoder worker until the UI's next presented frame or stop.
#[cfg(not(target_arch = "wasm32"))]
fn wait_for_capture_wake(
    slot: &Arc<(Mutex<FrameSlot>, Condvar)>,
    stop: &AtomicBool,
) -> bool {
    let (lock, cvar) = &**slot;
    let Ok(guard) = lock.lock() else {
        return false;
    };
    if stopped(stop) {
        return false;
    }
    let generation = guard.wake_generation;
    cvar.wait_while(guard, |slot| {
        !stopped(stop) && slot.wake_generation == generation
    })
    .map(|_| !stopped(stop))
    .unwrap_or(false)
}

/// The next presented frame, or `None` at `deadline` / on stop. `deadline`
/// of `None` waits until a frame or stop without using a std timed wait.
fn take_frame(
    slot: &Arc<(Mutex<FrameSlot>, Condvar)>,
    stop: &AtomicBool,
    wait: &CancellationToken,
    deadline: Option<f64>,
) -> Option<CapturedFrame> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = wait;
        let (lock, cvar) = &**slot;
        let mut guard = lock.lock().ok()?;
        loop {
            if let Some(frame) = guard.pending.take() {
                return Some(frame);
            }
            if stopped(stop) {
                return None;
            }
            if let Some(deadline) = deadline {
                if Cx::monotonic_now() >= deadline {
                    return None;
                }
            }
            let generation = guard.wake_generation;
            guard = cvar
                .wait_while(guard, |slot| {
                    !stopped(stop) && slot.wake_generation == generation
                })
                .ok()?;
        }
    }

    #[cfg(target_arch = "wasm32")]
    loop {
        {
            let mut guard = slot.0.lock().ok()?;
            if let Some(frame) = guard.pending.take() {
                return Some(frame);
            }
            if stopped(stop) {
                return None;
            }
        }
        match deadline {
            Some(deadline) => {
                let now = Cx::monotonic_now();
                if now >= deadline {
                    return None;
                }
                let _ = wait.wait_until((now + 0.005).min(deadline));
            }
            None => {
                let _ = wait.wait_until(Cx::monotonic_now() + 0.005);
            }
        }
    }
}

/// Feed the AAC track up to `target` frames of audio, padding with silence
/// when the tap has not produced enough (a muted or closed output device).
/// Audio position is derived from the same frame clock as the video, so the
/// two tracks cannot drift apart.
fn push_audio_through(
    encoder: &mut VideoFileEncoder,
    audio: &Arc<Mutex<AudioQueue>>,
    target: u64,
    pushed: &mut u64,
) -> Result<(), String> {
    if target <= *pushed {
        return Ok(());
    }
    let want = (target - *pushed) as usize;
    let mut block: Vec<i16> = Vec::with_capacity(want * 2);
    if let Ok(mut queue) = audio.lock() {
        let have = (queue.samples.len() / 2).min(want);
        block.extend(queue.samples.drain(..have * 2));
    }
    // Silence for whatever the device did not supply, so the track stays
    // exactly as long as the picture.
    block.resize(want * 2, 0);
    encoder
        .push_audio_i16(&block)
        .map_err(|err| format!("audio at frame {target}: {err}"))?;
    *pushed = target;
    Ok(())
}

/// Center every frame on the recording's fixed canvas. Keep native pixels
/// when they fit; proportionally shrink larger frames instead of cropping.
/// Portrait and landscape changes therefore leave opaque black padding, with
/// no stretch, clipped content, or pixels left from the previous dimensions.
/// Runs only on the encoder worker.
fn blit_into(
    canvas: &mut [u8], width: u32, height: u32,
    src: &[u8], src_width: u32, src_height: u32,
) {
    if src_width == width && src_height == height && src.len() >= canvas.len() {
        canvas.copy_from_slice(&src[..canvas.len()]);
        return;
    }
    for pixel in canvas.chunks_exact_mut(4) { pixel.copy_from_slice(&[0, 0, 0, 255]); }
    if width == 0 || height == 0 || src_width == 0 || src_height == 0
        || src.len() < src_width as usize * src_height as usize * 4
        || canvas.len() < width as usize * height as usize * 4 { return; }
    let scale = (width as f64 / src_width as f64)
        .min(height as f64 / src_height as f64).min(1.0);
    let fit_w = ((src_width as f64 * scale).round() as usize).clamp(1, width as usize);
    let fit_h = ((src_height as f64 * scale).round() as usize).clamp(1, height as usize);
    let left = (width as usize - fit_w) / 2;
    let top = (height as usize - fit_h) / 2;
    let dst_stride = width as usize * 4;
    let src_stride = src_width as usize * 4;
    if fit_w == src_width as usize && fit_h == src_height as usize {
        // Desktop -> phone normally needs only a centered row copy.
        for y in 0..fit_h {
            let d = (top + y) * dst_stride + left * 4;
            canvas[d..d + fit_w * 4].copy_from_slice(&src[y * src_stride..y * src_stride + fit_w * 4]);
        }
        return;
    }
    // Bilinear sampling avoids jagged type when a resized window is larger
    // than the fixed movie. The center-to-center mapping preserves aspect.
    for y in 0..fit_h {
        let sy = ((y as f64 + 0.5) * src_height as f64 / fit_h as f64 - 0.5)
            .clamp(0.0, (src_height - 1) as f64);
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(src_height as usize - 1);
        let fy = sy - y0 as f64;
        for x in 0..fit_w {
            let sx = ((x as f64 + 0.5) * src_width as f64 / fit_w as f64 - 0.5)
                .clamp(0.0, (src_width - 1) as f64);
            let x0 = sx.floor() as usize;
            let x1 = (x0 + 1).min(src_width as usize - 1);
            let fx = sx - x0 as f64;
            let d = (top + y) * dst_stride + (left + x) * 4;
            for c in 0..4 {
                let a = src[y0 * src_stride + x0 * 4 + c] as f64;
                let b = src[y0 * src_stride + x1 * 4 + c] as f64;
                let c0 = src[y1 * src_stride + x0 * 4 + c] as f64;
                let d0 = src[y1 * src_stride + x1 * 4 + c] as f64;
                canvas[d + c] = ((a + (b - a) * fx) * (1.0 - fy) + (c0 + (d0 - c0) * fx) * fy).round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_target_tracks_the_video_clock_exactly() {
        // 30fps at 48kHz: 1600 samples per frame, no rounding drift over a
        // minute of recording.
        let fps = 30u128;
        let rate = 48_000u128;
        let last = ((60 * 30) as u128 * rate / fps) as u64;
        assert_eq!(last, 48_000 * 60);
    }

    #[test]
    fn audio_target_is_exact_for_44100() {
        // 44100 does not divide by 30; the running target must still land on
        // the second, which per-frame rounding would not.
        let fps = 30u128;
        let rate = 44_100u128;
        let one_second = (30u128 * rate / fps) as u64;
        assert_eq!(one_second, 44_100);
    }

    #[test]
    fn blit_centers_portrait_then_landscape_without_stale_pixels() {
        let black = [0, 0, 0, 255];
        let white = [255, 255, 255, 255];
        let mut canvas = vec![9; 10 * 6 * 4];
        blit_into(&mut canvas, 10, 6, &white.repeat(4 * 6), 4, 6);
        for y in 0..6 { for x in 0..10 {
            assert_eq!(&canvas[(y * 10 + x) * 4..(y * 10 + x + 1) * 4],
                if (3..7).contains(&x) { &white } else { &black });
        }}
        blit_into(&mut canvas, 10, 6, &white.repeat(6 * 4), 6, 4);
        for y in 0..6 { for x in 0..10 {
            assert_eq!(&canvas[(y * 10 + x) * 4..(y * 10 + x + 1) * 4],
                if (2..8).contains(&x) && (1..5).contains(&y) { &white } else { &black });
        }}
    }

    #[test]
    fn blit_fits_a_larger_window_without_cropping_its_far_edge() {
        let mut source = vec![0; 8 * 4 * 4];
        for y in 0..4 { for x in 0..8 {
            let pixel = if x < 4 { [255, 0, 0, 255] } else { [0, 0, 255, 255] };
            source[(y * 8 + x) * 4..(y * 8 + x + 1) * 4].copy_from_slice(&pixel);
        }}
        let mut canvas = vec![9; 4 * 4 * 4];
        blit_into(&mut canvas, 4, 4, &source, 8, 4);
        assert_eq!(&canvas[..16], &[0, 0, 0, 255].repeat(4));
        assert_eq!(&canvas[16..20], &[255, 0, 0, 255]);
        assert_eq!(&canvas[28..32], &[0, 0, 255, 255]);
        assert_eq!(&canvas[48..], &[0, 0, 0, 255].repeat(4));
    }

    #[test]
    fn bitrate_stays_in_band() {
        assert_eq!(bitrate_for(64, 64, 30), 8_000_000);
        assert_eq!(bitrate_for(7680, 4320, 60), 160_000_000);
        assert_eq!(bitrate_for(2048, 1536, 30), 2048 * 1536 * 30 / 4);
    }

    #[test]
    fn timestamps_round_trip_a_known_instant() {
        // 2026-08-30T12:34:56Z
        assert_eq!(civil_from_unix(1_788_093_296), (2026, 8, 30, 12, 34, 56));
        // Leap day, and the year boundary just after it.
        assert_eq!(civil_from_unix(1_709_208_000), (2024, 2, 29, 12, 0, 0));
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
    }
}
