//! The video view: one clip, letterboxed to the window, with a slim
//! transport bar that gets out of the way.
//!
//! The picture is the VJ's NV12 path (apps/vj/src/nv12_view.rs) drawn
//! straight to the screen instead of into an offscreen slot texture: the
//! decoder's own Y and UV planes go to an R8 and an RG8 texture (two
//! memcpys) and the pixel shader does the BT.709 conversion. The VJ needs a
//! child pass because its output feeds a mixer; a player only needs the
//! picture on screen, so the quad is drawn with `draw_abs` into the fitted
//! rect and the custom pass-space vertex shader is not needed here.
//!
//! Everything in the transport bar is drawn by this widget rather than laid
//! out as children, for the same reason image positions its `Image`
//! itself: the bar overlays the picture, auto-hides, and has one scrub area
//! whose geometry the seek maths needs in hand anyway.

use crate::player::VideoPlayer;
use crate::theme::Palette;
use makepad_widgets::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Height of the transport bar.
const BAR_H: f64 = 34.0;
/// The bar hides this long after the last mouse movement, while playing.
const BAR_LINGER: Duration = Duration::from_secs(2);
/// Two clicks closer together than this (and near each other) are a
/// double-click. The platform gives us no tap count, so we count them.
const DOUBLE_CLICK_SECS: f64 = 0.35;
const DOUBLE_CLICK_SLOP: f64 = 6.0;
/// Left/Right, and Shift+Left/Right.
const SEEK_STEP_SECS: f64 = 5.0;
const SEEK_STEP_BIG_SECS: f64 = 30.0;
/// One Up/Down press.
const VOLUME_STEP: f32 = 0.05;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawNv12::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_y: texture_2d(float)
        tex_uv: texture_2d(float)

        // Biplanar NV12, BT.709 LIMITED range — the same arithmetic the
        // VJ's present pass and the CPU converter in makepad-video speak.
        pixel: fn() {
            let yv = self.tex_y.sample(self.pos).x
            let uv = self.tex_uv.sample(self.pos).xy
            let y = (yv * 255.0 - 16.0) / 219.0
            let u = (uv.x * 255.0 - 128.0) / 224.0
            let v = (uv.y * 255.0 - 128.0) / 224.0
            let r = y + 1.5748 * v
            let g = y - 0.1873 * u - 0.4681 * v
            let b = y + 1.8556 * u
            return vec4(clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0), 1.0)
        }
    }

    mod.widgets.MpVideoViewBase = #(MpVideoView::register_widget(vm))

    mod.widgets.MpVideoView = set_type_default() do mod.widgets.MpVideoViewBase{
        width: Fill
        height: Fill

        draw_flat: mod.draw.DrawColor{}
        play_icon: Icon{
            width: Fill
            height: Fill
            align: Align{x: 0.5 y: 0.5}
            icon_walk: Walk{width: 15 height: 15}
            draw_icon +: {
                svg: crate_resource("self:resources/icons/play.svg")
                color: mod.mpv.fg
            }
        }
        pause_icon: Icon{
            width: Fill
            height: Fill
            align: Align{x: 0.5 y: 0.5}
            icon_walk: Walk{width: 15 height: 15}
            draw_icon +: {
                svg: crate_resource("self:resources/icons/pause.svg")
                color: mod.mpv.fg
            }
        }
        time_label: Label{
            width: Fill
            height: Fill
            padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
            align: Align{x: 0.0 y: 0.5}
            text: "0:00 / 0:00"
            draw_text +: {
                color: mod.mpv.fg
                text_style: theme.font_code{font_size: 9.0}
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNv12 {
    #[deref]
    pub draw_super: DrawQuad,
}

/// What the view tells the app shell about the clip, for the status line
/// and the window title.
#[derive(Clone, Debug, Default)]
pub enum MpVideoAction {
    Status {
        name: String,
        width: u32,
        height: u32,
        fps: f64,
        position_secs: f64,
        duration_secs: f64,
        playing: bool,
        ended: bool,
        volume_pct: i32,
        muted: bool,
        /// Chrome hidden (double-click): the shell hides its status row too.
        bare: bool,
        error: Option<String>,
    },
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct MpVideoView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_video: DrawNv12,
    /// Every flat fill in the bar: background, scrub track, played portion.
    /// One instance per `draw_abs`, so a single shader paints all three.
    #[live]
    draw_flat: DrawColor,
    #[live]
    play_icon: WidgetRef,
    #[live]
    pause_icon: WidgetRef,
    #[live]
    time_label: WidgetRef,

    #[rust]
    area: Area,
    #[rust]
    rect: Rect,

    #[rust]
    player: Option<VideoPlayer>,
    #[rust]
    path: Option<PathBuf>,
    #[rust]
    error: Option<String>,
    /// The picture's Y (R8, w x h) and UV (RG8, w/2 x h/2) plane textures,
    /// recreated only on a resolution change.
    #[rust]
    planes: Option<VideoPlanes>,
    #[rust]
    size: (u32, u32),
    /// Drives the frame pump; re-armed for as long as the clip plays.
    #[rust]
    pump: NextFrame,

    /// Playback reached the end: the picture holds and Space replays.
    #[rust]
    ended: bool,
    /// Chrome hidden by a double-click — picture only.
    #[rust]
    bare: bool,
    #[rust(1.0f32)]
    volume: f32,
    #[rust]
    muted: bool,

    #[rust]
    scrubbing: bool,
    /// The newest scrub target while the decoder is still mid-seek.
    #[rust]
    pending_scrub: Option<f64>,
    #[rust]
    last_move: Option<Instant>,
    #[rust]
    last_click: Option<Click>,
    /// A seek is outstanding and its target frame has not reached the
    /// screen yet; the pump keeps running until it does (or the deadline
    /// passes, so a seek the decoder cannot serve never spins forever).
    #[rust]
    awaiting_frame: Option<Instant>,

    /// This process is a warm Quick Look viewer (`--preview`): Escape/Q
    /// hides the panel instead of ending the process. See `preview.rs`.
    #[rust]
    preview: bool,
}

/// The two plane textures for one video resolution.
#[derive(Clone)]
pub struct VideoPlanes {
    y: Texture,
    uv: Texture,
}

/// Where and when the last mouse-down landed, for double-click detection.
#[derive(Clone, Copy)]
pub struct Click {
    at: Vec2d,
    time: f64,
}

impl Widget for MpVideoView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.pump.is_event(event).is_some() {
            self.pump_frame(cx);
        }
        match event {
            Event::KeyDown(ke) => self.handle_key(cx, ke),
            Event::MouseDown(me) if me.button.is_primary() && self.rect.contains(me.abs) => {
                self.handle_mouse_down(cx, me);
            }
            Event::MouseMove(me) => {
                self.last_move = Some(Instant::now());
                if self.scrubbing {
                    let fraction = self.scrub_fraction(me.abs);
                    self.scrub_to(cx, fraction);
                }
                self.arm_pump(cx);
                cx.redraw_all();
            }
            Event::MouseUp(_) => {
                if self.scrubbing {
                    self.scrubbing = false;
                    self.emit_status(cx);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let rect = cx.turtle().rect();
        self.rect = rect;

        if rect.size.x > 0.0 && rect.size.y > 0.0 {
            self.draw_picture(cx, rect);
            if self.bar_visible() {
                self.draw_bar(cx, scope, rect);
            }
        }

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl MpVideoView {
    // -- opening ------------------------------------------------------------

    /// Opens `path` and starts playing it.
    pub fn open(&mut self, cx: &mut Cx, path: &Path) {
        crate::player::stop_audio();
        self.player = None;
        self.planes = None;
        self.size = (0, 0);
        self.ended = false;
        self.error = None;
        self.pending_scrub = None;
        self.path = Some(path.to_path_buf());
        match VideoPlayer::new(&path.to_string_lossy()) {
            Ok(player) => {
                self.player = Some(player);
                crate::player::set_volume(self.volume);
                crate::player::set_user_muted(self.muted);
            }
            Err(error) => {
                log!("video: cannot open {}: {}", path.display(), error);
                self.error = Some(error);
            }
        }
        self.last_move = Some(Instant::now());
        self.arm_pump(cx);
        self.emit_status(cx);
        cx.redraw_all();
    }

    /// Quick Look v2: the panel hid, so stop playing and give everything
    /// back — the soundtrack, the decode thread (dropping `VideoPlayer`
    /// signals it and lets it die detached), and the two plane textures.
    /// The picture goes blank and the frame pump parks, so an idle warm
    /// viewer costs nothing until the next `open`.
    pub fn unload(&mut self, cx: &mut Cx) {
        crate::player::stop_audio();
        self.player = None;
        self.planes = None;
        self.size = (0, 0);
        self.path = None;
        self.error = None;
        self.ended = false;
        self.bare = false;
        self.scrubbing = false;
        self.pending_scrub = None;
        self.awaiting_frame = None;
        self.last_move = None;
        self.last_click = None;
        // The volume and the mute are the listener's, not the clip's: they
        // survive an unload and apply to whatever plays next.
        self.emit_status(cx);
        cx.redraw_all();
    }

    /// Start (or stop) muted. Set before [`open`](Self::open) for a run that
    /// must be silent; `M` and the volume keys still work from there.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        crate::player::set_user_muted(muted);
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

    pub fn is_playing(&self) -> bool {
        self.player.as_ref().is_some_and(|p| !p.is_paused())
    }

    // -- the frame pump -----------------------------------------------------

    /// Arms the next-frame chain unless it would spin for nothing (no clip,
    /// or a paused clip with the bar already settled).
    fn arm_pump(&mut self, cx: &mut Cx) {
        if self.player.is_some() {
            self.pump = cx.new_next_frame();
        }
    }

    fn pump_frame(&mut self, cx: &mut Cx) {
        let Some(player) = &mut self.player else { return };
        if let Some(frame) = player.take_due_frame() {
            self.awaiting_frame = None;
            self.upload_frame(cx, frame);
        }
        // A scrub that arrived while the decoder was mid-seek: flush the
        // newest one the moment the previous lands. This is what makes
        // dragging the knob live instead of a flood of stale requests.
        if let Some(fraction) = self.pending_scrub {
            if self.player.as_ref().is_some_and(|p| !p.seek_pending()) {
                self.pending_scrub = None;
                self.seek_fraction(fraction);
            }
        }
        let Some(player) = &mut self.player else { return };
        if player.at_end() && !player.is_paused() && !player.seek_pending() {
            // Play-once: hold the last picture, show the bar, and let Space
            // replay. The decode thread stays parked, so that replay is an
            // in-place seek rather than a reopen.
            player.pause();
            self.ended = true;
            self.emit_status(cx);
            cx.redraw_all();
            return;
        }
        self.emit_status(cx);
        cx.redraw_all();
        // Keep pumping while the clip runs (which is also what fades the
        // transport bar out), and while a seek is still in flight so its
        // target frame reaches the screen. A paused, settled clip parks the
        // chain: nothing on screen changes until the next input.
        if self.awaiting_frame.is_some_and(|deadline| Instant::now() >= deadline) {
            self.awaiting_frame = None;
        }
        let seeking = self.pending_scrub.is_some()
            || self.awaiting_frame.is_some()
            || self.player.as_ref().is_some_and(|p| p.seek_pending());
        if self.is_playing() || seeking {
            self.pump = cx.new_next_frame();
        }
    }

    fn upload_frame(&mut self, cx: &mut Cx, frame: crate::player::Frame) {
        let (w, h) = (frame.width as usize, frame.height as usize);
        if w == 0 || h == 0 || frame.nv12.len() < w * h * 3 / 2 {
            return;
        }
        if self.size != (frame.width, frame.height) || self.planes.is_none() {
            self.planes = Some(VideoPlanes {
                y: Texture::new_with_format(
                    cx,
                    TextureFormat::VecRu8 {
                        width: w,
                        height: h,
                        data: Some(vec![0; w * h]),
                        unpack_row_length: None,
                        updated: TextureUpdated::Full,
                    },
                ),
                uv: Texture::new_with_format(
                    cx,
                    TextureFormat::VecRGu8 {
                        width: w / 2,
                        height: h / 2,
                        data: Some(vec![0; (w / 2) * (h / 2) * 2]),
                        unpack_row_length: None,
                        updated: TextureUpdated::Full,
                    },
                ),
            });
            self.size = (frame.width, frame.height);
        }
        let Some(planes) = &self.planes else { return };
        // Two contiguous memcpys: the Y plane is the first w*h bytes, and
        // the interleaved UV plane maps 1:1 onto an RG8 texture at half
        // resolution. No software YUV loop — the shader does that.
        let mut buf = planes.y.take_vec_u8(cx);
        buf.clear();
        buf.extend_from_slice(&frame.nv12[..w * h]);
        planes.y.put_back_vec_u8(cx, buf, None);
        let mut buf = planes.uv.take_vec_u8(cx);
        buf.clear();
        buf.extend_from_slice(&frame.nv12[w * h..w * h + (w / 2) * (h / 2) * 2]);
        planes.uv.put_back_vec_u8(cx, buf, None);
    }

    // -- drawing ------------------------------------------------------------

    fn draw_picture(&mut self, cx: &mut Cx2d, rect: Rect) {
        let Some(planes) = self.planes.clone() else { return };
        let (vw, vh) = (self.size.0 as f64, self.size.1 as f64);
        if vw <= 0.0 || vh <= 0.0 {
            return;
        }
        self.draw_video.draw_vars.set_texture(0, &planes.y);
        self.draw_video.draw_vars.set_texture(1, &planes.uv);
        self.draw_video.draw_abs(cx, letterbox(rect, vw, vh));
    }

    fn draw_bar(&mut self, cx: &mut Cx2d, scope: &mut Scope, rect: Rect) {
        let palette = Palette::shared();
        let layout = BarLayout::of(rect);

        self.draw_flat.color = palette.bar_vec4();
        self.draw_flat.draw_abs(cx, layout.bar);

        // The scrub bar: the whole span in dark_foreground, the played
        // portion painted over it in the accent. Flat, square, no gradient.
        self.draw_flat.color = palette.dim_vec4();
        self.draw_flat.draw_abs(cx, layout.track);
        let fraction = self.position_fraction();
        if fraction > 0.0 && layout.track.size.x > 0.0 {
            self.draw_flat.color = palette.accent_vec4();
            self.draw_flat.draw_abs(
                cx,
                Rect {
                    pos: layout.track.pos,
                    size: dvec2(layout.track.size.x * fraction, layout.track.size.y),
                },
            );
        }

        let icon = if self.is_playing() {
            self.pause_icon.clone()
        } else {
            self.play_icon.clone()
        };
        icon.draw_walk_all(cx, scope, Walk::abs_rect(layout.button));

        // The readout's text is set by the pump, never here: mutating a
        // widget mid-draw asks for a redraw from inside a draw.
        self.time_label
            .clone()
            .draw_walk_all(cx, scope, Walk::abs_rect(layout.time));
    }

    /// Visible while paused, while scrubbing, and for [`BAR_LINGER`] after
    /// the last mouse movement. Never in bare (double-clicked) mode.
    fn bar_visible(&self) -> bool {
        if self.bare || self.player.is_none() {
            return false;
        }
        if self.scrubbing || !self.is_playing() {
            return true;
        }
        self.last_move
            .is_some_and(|at| at.elapsed() < BAR_LINGER)
    }

    fn position_fraction(&self) -> f64 {
        let Some(player) = &self.player else { return 0.0 };
        let duration = player.duration_secs();
        if duration <= 0.0 {
            return 0.0;
        }
        (player.position_secs() / duration).clamp(0.0, 1.0)
    }

    fn time_text(&self) -> String {
        let Some(player) = &self.player else {
            return "0:00 / 0:00".to_string();
        };
        format!(
            "{} / {}",
            format_time(player.position_secs()),
            format_time(player.duration_secs())
        )
    }

    // -- input --------------------------------------------------------------

    fn handle_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let step = if ke.modifiers.shift {
            SEEK_STEP_BIG_SECS
        } else {
            SEEK_STEP_SECS
        };
        match ke.key_code {
            KeyCode::Escape | KeyCode::KeyQ => self.close(cx),
            KeyCode::Space => self.toggle_play(cx),
            KeyCode::ArrowLeft => self.seek_by(cx, -step),
            KeyCode::ArrowRight => self.seek_by(cx, step),
            KeyCode::ArrowUp => self.set_volume(cx, self.volume + VOLUME_STEP),
            KeyCode::ArrowDown => self.set_volume(cx, self.volume - VOLUME_STEP),
            KeyCode::KeyM => {
                self.muted = !self.muted;
                crate::player::set_user_muted(self.muted);
                self.wake(cx);
            }
            KeyCode::KeyF => {
                self.bare = !self.bare;
                self.wake(cx);
            }
            _ => {}
        }
    }

    fn handle_mouse_down(&mut self, cx: &mut Cx, me: &MouseDownEvent) {
        let double = self.last_click.is_some_and(|last| {
            me.time - last.time < DOUBLE_CLICK_SECS
                && (me.abs - last.at).length() < DOUBLE_CLICK_SLOP
        });
        self.last_click = Some(Click { at: me.abs, time: me.time });
        self.last_move = Some(Instant::now());

        if double {
            // A double-click anywhere but on the transport toggles
            // fullscreen-in-window: picture only, no chrome.
            self.last_click = None;
            let on_bar = self.bar_visible() && BarLayout::of(self.rect).bar.contains(me.abs);
            if !on_bar {
                self.bare = !self.bare;
                self.wake(cx);
                return;
            }
        }

        if self.bar_visible() {
            let layout = BarLayout::of(self.rect);
            if layout.button.contains(me.abs) {
                self.toggle_play(cx);
                return;
            }
            if layout.scrub_hit.contains(me.abs) {
                self.scrubbing = true;
                let fraction = self.scrub_fraction(me.abs);
                self.scrub_to(cx, fraction);
                return;
            }
            if layout.bar.contains(me.abs) {
                return;
            }
        }
        self.wake(cx);
    }

    fn scrub_fraction(&self, abs: Vec2d) -> f64 {
        let track = BarLayout::of(self.rect).track;
        if track.size.x <= 0.0 {
            return 0.0;
        }
        ((abs.x - track.pos.x) / track.size.x).clamp(0.0, 1.0)
    }

    /// Jump to `fraction`, coalescing while the decoder is mid-seek.
    fn scrub_to(&mut self, cx: &mut Cx, fraction: f64) {
        let Some(player) = &mut self.player else { return };
        if player.duration_secs() <= 0.0 {
            return;
        }
        if player.seek_pending() {
            self.pending_scrub = Some(fraction);
        } else {
            self.pending_scrub = None;
            self.seek_fraction(fraction);
        }
        self.wake(cx);
    }

    fn seek_fraction(&mut self, fraction: f64) {
        let Some(player) = &mut self.player else { return };
        let duration = player.duration_secs();
        if duration <= 0.0 {
            return;
        }
        player.seek(fraction.clamp(0.0, 1.0) * duration);
        self.ended = false;
        self.expect_frame();
    }

    fn seek_by(&mut self, cx: &mut Cx, delta: f64) {
        let Some(player) = &mut self.player else { return };
        let target = player.position_secs() + delta;
        player.seek(target);
        self.ended = false;
        self.pending_scrub = None;
        self.expect_frame();
        self.wake(cx);
    }

    /// A seek was just asked for: keep the pump alive until its frame is on
    /// screen, so a scrub while paused actually shows what it landed on.
    fn expect_frame(&mut self) {
        self.awaiting_frame = Some(Instant::now() + Duration::from_secs(2));
    }

    fn toggle_play(&mut self, cx: &mut Cx) {
        if self.ended {
            // Replay: an in-place seek on the still-parked decode thread.
            self.ended = false;
            if let Some(player) = &mut self.player {
                player.seek(0.0);
                player.resume();
            }
            self.expect_frame();
            self.wake(cx);
            return;
        }
        if let Some(player) = &mut self.player {
            if player.is_paused() {
                player.resume();
            } else {
                player.pause();
            }
        }
        self.wake(cx);
    }

    fn set_volume(&mut self, cx: &mut Cx, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        // Reaching for the volume is an unmute: the knob is what the
        // listener just told us they want to hear.
        self.muted = false;
        crate::player::set_volume(self.volume);
        crate::player::set_user_muted(false);
        self.wake(cx);
    }

    /// Re-arm the pump, refresh the status line, redraw.
    fn wake(&mut self, cx: &mut Cx) {
        self.last_move = Some(Instant::now());
        self.arm_pump(cx);
        self.emit_status(cx);
        cx.redraw_all();
    }

    fn emit_status(&mut self, cx: &mut Cx) {
        let time_text = self.time_text();
        self.time_label.clone().set_text(cx, &time_text);
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let (width, height, fps, position_secs, duration_secs) = match &self.player {
            Some(player) => (
                player.width,
                player.height,
                player.fps,
                player.position_secs(),
                player.duration_secs(),
            ),
            None => (0, 0, 0.0, 0.0, 0.0),
        };
        cx.widget_action(
            self.uid,
            MpVideoAction::Status {
                name,
                width,
                height,
                fps,
                position_secs,
                duration_secs,
                playing: self.is_playing(),
                ended: self.ended,
                volume_pct: (self.volume * 100.0).round() as i32,
                muted: self.muted,
                bare: self.bare,
                error: self.error.clone(),
            },
        );
    }
}

/// The transport bar's geometry, derived from the view rect. One place, so
/// drawing and hit testing can never disagree.
pub struct BarLayout {
    pub bar: Rect,
    pub button: Rect,
    pub track: Rect,
    /// The whole clickable span around the 4px track.
    pub scrub_hit: Rect,
    pub time: Rect,
}

impl BarLayout {
    pub fn of(rect: Rect) -> Self {
        const TIME_W: f64 = 112.0;
        const GAP: f64 = 10.0;
        let bar = Rect {
            pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y - BAR_H),
            size: dvec2(rect.size.x, BAR_H),
        };
        let button = Rect {
            pos: bar.pos,
            size: dvec2(BAR_H, BAR_H),
        };
        let x0 = bar.pos.x + BAR_H + GAP;
        let x1 = (bar.pos.x + bar.size.x - TIME_W).max(x0);
        let track = Rect {
            pos: dvec2(x0, bar.pos.y + BAR_H * 0.5 - 2.0),
            size: dvec2(x1 - x0, 4.0),
        };
        Self {
            bar,
            button,
            track,
            scrub_hit: Rect {
                pos: dvec2(x0, bar.pos.y),
                size: dvec2(x1 - x0, BAR_H),
            },
            time: Rect {
                pos: dvec2(x1 + GAP, bar.pos.y),
                size: dvec2((TIME_W - GAP).max(0.0), BAR_H),
            },
            }
    }
}

/// The largest `vw x vh`-shaped rect that fits inside `container`, centered:
/// the letterbox (or pillarbox) every player owes its picture.
pub fn letterbox(container: Rect, vw: f64, vh: f64) -> Rect {
    if vw <= 0.0 || vh <= 0.0 {
        return container;
    }
    let scale = (container.size.x / vw).min(container.size.y / vh);
    let size = dvec2(vw * scale, vh * scale);
    Rect {
        pos: container.pos + (container.size - size) * 0.5,
        size,
    }
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
    fn letterbox_fits_and_centers_both_ways() {
        let window = Rect { pos: dvec2(0.0, 0.0), size: dvec2(1000.0, 500.0) };
        // Wider than the window: pillars top and bottom.
        let wide = letterbox(window, 1000.0, 250.0);
        assert_eq!(wide.size, dvec2(1000.0, 250.0));
        assert_eq!(wide.pos, dvec2(0.0, 125.0));
        // Taller than the window: bars left and right.
        let tall = letterbox(window, 250.0, 500.0);
        assert_eq!(tall.size, dvec2(250.0, 500.0));
        assert_eq!(tall.pos, dvec2(375.0, 0.0));
        // The aspect ratio survives in both cases.
        assert!((wide.size.x / wide.size.y - 4.0).abs() < 1e-9);
        assert!((tall.size.x / tall.size.y - 0.5).abs() < 1e-9);
        // A degenerate size falls back to the container rather than NaN.
        assert_eq!(letterbox(window, 0.0, 0.0), window);
    }

    #[test]
    fn letterbox_offsets_by_the_container_origin() {
        let panel = Rect { pos: dvec2(40.0, 20.0), size: dvec2(200.0, 200.0) };
        let fit = letterbox(panel, 100.0, 50.0);
        assert_eq!(fit.size, dvec2(200.0, 100.0));
        assert_eq!(fit.pos, dvec2(40.0, 70.0));
    }

    #[test]
    fn bar_layout_agrees_with_itself() {
        let rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(900.0, 700.0) };
        let l = BarLayout::of(rect);
        assert_eq!(l.bar.size.y, BAR_H);
        assert_eq!(l.bar.pos.y, 700.0 - BAR_H);
        // The button sits at the left edge of the bar, and clicking its
        // middle hits it.
        assert!(l.button.contains(dvec2(BAR_H * 0.5, 700.0 - BAR_H * 0.5)));
        // The track never overlaps the button, and the scrub hit area
        // spans the bar's full height above the 4px track.
        assert!(l.track.pos.x >= l.button.pos.x + l.button.size.x);
        assert!(l.scrub_hit.contains(dvec2(l.track.pos.x + 1.0, l.bar.pos.y + 2.0)));
        assert!(!l.scrub_hit.contains(dvec2(l.button.pos.x + 1.0, l.bar.pos.y + 2.0)));
        // The time readout follows the track without overlapping it.
        assert!(l.time.pos.x >= l.track.pos.x + l.track.size.x);
        assert!(l.time.pos.x + l.time.size.x <= rect.size.x);
    }

    #[test]
    fn bar_layout_degrades_on_a_narrow_window() {
        // Narrower than button + gap + time readout: nothing goes negative
        // and nothing escapes the bar.
        let rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(90.0, 200.0) };
        let l = BarLayout::of(rect);
        assert!(l.track.size.x >= 0.0);
        assert!(l.scrub_hit.size.x >= 0.0);
        assert_eq!(l.bar.size.x, 90.0);
    }

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
