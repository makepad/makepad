//! The video face of a content preview: the clip's picture, a seek bar with
//! a real knob, and a transport row — play/pause, restart, loop, position.
//!
//! One widget for every place a video plays (the big viewer page, the
//! library rail's preview well), exactly like [`crate::audio_view`] is the
//! one audio face. The widget owns NO decoder and NO clock: the host owns
//! its player, pumps decoded frames in with [`VideoView::set_frame`], ticks
//! the playhead with [`VideoView::set_transport`], and carries out the
//! [`VideoAction`]s the transport reports. This widget knows where the
//! playhead is and what the buttons say — nothing else.

use makepad_widgets::*;

/// What the user did to the video transport. The host carries it out
/// against the player it owns.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum VideoAction {
    /// Play/pause pressed; the host decides which of the two it means (a
    /// clip at end-of-stream restarts).
    TogglePlay,
    /// Restart from the first frame.
    Restart,
    /// Move playback to this fraction of the clip (0..=1).
    Seek(f64),
    /// The loop toggle flipped; [`VideoView::looping`] has the new state.
    ToggleLoop,
    /// An IN/OUT trim handle was RELEASED at these fractions (start, end).
    /// Emitted once per drag, on the release — hosts re-fit beat-synced
    /// loops here, not per drag frame.
    TrimChanged(f64, f64),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.VideoViewBase = #(VideoView::register_widget(vm))
    mod.widgets.VideoView = set_type_default() do mod.widgets.VideoViewBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: 4

        lane := View{
            width: Fill
            height: Fill
            cursor: MouseCursor.Hand
            flow: Overlay
            align: Align{x: 0.5 y: 0.5}
            picture := Image{
                width: Fill
                height: Fill
                fit: ImageFit.Smallest
            }
        }
        // Reserved strip for `bar_below` hosts (a small monitor whose
        // picture must stay unobstructed); collapsed otherwise.
        bar_slot := View{
            width: Fill
            height: 12
            visible: false
        }
        // Seek bar: track + progress + knob, code-drawn over the lane's
        // bottom edge (a themed slider knob is invisible on a dark well;
        // these three rectangles never are).
        draw_track +: {
            color: #xffffff22
        }
        draw_progress +: {
            color: #xff5a4d99
        }
        draw_knob +: {
            // WHITE = where things are (the app's slider-handle language);
            // accent = what's on. The playhead follows the handles.
            color: #xffffff
        }
        draw_handle_in +: {
            color: #xf0b34d
            // [ — the in-point bracket, SDF so the AA is free.
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                sdf.box(1.0, 1.0, 2.5, h - 2.0, 1.0)
                sdf.fill(self.color)
                sdf.box(1.0, 1.0, w - 2.0, 2.5, 1.0)
                sdf.fill(self.color)
                sdf.box(1.0, h - 3.5, w - 2.0, 2.5, 1.0)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_handle_out +: {
            color: #xf0b34d
            // ] — mirrored.
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                sdf.box(w - 3.5, 1.0, 2.5, h - 2.0, 1.0)
                sdf.fill(self.color)
                sdf.box(1.0, 1.0, w - 2.0, 2.5, 1.0)
                sdf.fill(self.color)
                sdf.box(1.0, h - 3.5, w - 2.0, 2.5, 1.0)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_tail +: {
            color: #xffffff10
        }
        controls := View{
            width: Fill height: Fit
            flow: Right spacing: 6
            align: Align{y: 0.5}
            play := RoundedView{
                width: 34 height: 22
                cursor: MouseCursor.Hand
                align: Align{x: 0.5 y: 0.5}
                show_bg: true
                draw_bg +: {
                    color: #x1b1b1f
                    border_color: #xffffff14
                    border_size: 1.0
                    border_radius: 3.0
                }
                play_glyph := Label{
                    text: "⏸"
                    draw_text +: {
                        color: #xdfe6ec
                        text_style: theme.font_regular{font_size: 10}
                    }
                }
            }
            restart := RoundedView{
                width: 34 height: 22
                cursor: MouseCursor.Hand
                align: Align{x: 0.5 y: 0.5}
                show_bg: true
                draw_bg +: {
                    color: #x1b1b1f
                    border_color: #xffffff14
                    border_size: 1.0
                    border_radius: 3.0
                }
                restart_glyph := Label{
                    text: "↺"
                    draw_text +: {
                        color: #xdfe6ec
                        text_style: theme.font_regular{font_size: 10}
                    }
                }
            }
            position := Label{
                width: Fit
                draw_text +: {
                    color: #x8a939d
                    text_style: theme.font_regular{font_size: 8}
                }
            }
            spacer := View{width: Fill height: 1}
            loop_box := RoundedView{
                width: 52 height: 22
                cursor: MouseCursor.Hand
                align: Align{x: 0.5 y: 0.5}
                show_bg: true
                draw_bg +: {
                    color: #x1b1b1f
                    border_color: #xffffff14
                    border_size: 1.0
                    border_radius: 3.0
                }
                loop_label := Label{
                    text: "LOOP"
                    draw_text +: {
                        color: #xdfe6ec
                        text_style: theme.font_regular{font_size: 8}
                    }
                }
            }
        }
    }
}

/// Seek bar geometry, in layout points.
const TRACK_H: f64 = 4.0;
const KNOB_W: f64 = 8.0;
const KNOB_H: f64 = 14.0;
/// The bar floats this far above the lane's bottom edge.
const TRACK_LIFT: f64 = 10.0;

/// A playing clip's picture with a knobbed seek bar and a transport row.
#[derive(Script, ScriptHook, Widget)]
pub struct VideoView {
    #[deref]
    view: View,
    #[live]
    draw_track: DrawColor,
    #[live]
    draw_progress: DrawColor,
    #[live]
    draw_knob: DrawColor,
    /// Where the host says playback is, 0..=1.
    #[rust]
    fraction: f64,
    /// True while the user holds the knob: the host's own transport ticks
    /// must not fight the finger.
    #[rust]
    scrubbing: bool,
    #[rust]
    playing: bool,
    #[rust(true)]
    looping: bool,
    /// Draw the seek bar in its own strip BELOW the picture instead of
    /// floating over the picture's bottom edge — for small source
    /// monitors where the frame must stay unobstructed.
    #[live]
    bar_below: bool,
    /// Hide the built-in transport row (a host drawing its own tighter
    /// control line sets this off).
    #[live(true)]
    show_controls: bool,
    /// Show IN/OUT trim handles on the bar: two draggable notches that
    /// constrain the playing range. The tails outside [in, out] draw
    /// dimmed; releasing a handle emits [`VideoAction::TrimChanged`]. A
    /// plain viewer keeps this off — near-edge presses seek there.
    #[live]
    trim_handles: bool,
    #[live]
    draw_handle_in: DrawQuad,
    #[live]
    draw_handle_out: DrawQuad,
    #[live]
    draw_tail: DrawColor,
    /// The trim range, fractions of the clip. (0, 1) = untrimmed.
    #[rust((0.0f64, 1.0f64))]
    trim: (f64, f64),
    /// Which handle the finger holds: 0 = IN, 1 = OUT.
    #[rust]
    trim_drag: Option<usize>,
}

/// Two handles closer than this cannot be: the loop must keep a visible,
/// playable width.
const TRIM_MIN_GAP: f64 = 0.02;
/// Horizontal grab radius around a trim notch, layout points.
const TRIM_GRAB: f64 = 6.0;

impl VideoView {
    /// The decoded frame to show. The host pumps this once per presented
    /// frame; the texture handle is cheap to re-set.
    pub fn set_frame(&mut self, cx: &mut Cx, texture: Option<Texture>) {
        let image = self.view.image(cx, ids!(picture));
        image.set_texture(cx, texture);
        image.redraw(cx);
    }

    /// Playhead + button faces. Ignored while the finger holds the knob.
    pub fn set_transport(&mut self, cx: &mut Cx, fraction: f64, playing: bool, position: &str) {
        if !self.scrubbing {
            self.fraction = fraction.clamp(0.0, 1.0);
        }
        if playing != self.playing {
            self.playing = playing;
            self.view
                .label(cx, ids!(play_glyph))
                .set_text(cx, if playing { "⏸" } else { "▶" });
        }
        self.view.label(cx, ids!(position)).set_text(cx, position);
        self.view.redraw(cx);
    }

    pub fn is_scrubbing(&self) -> bool {
        self.scrubbing
    }

    /// The IN/OUT trim range as fractions.
    pub fn trim(&self) -> (f64, f64) {
        self.trim
    }

    /// True while a trim notch is held — hosts must not force `set_trim`
    /// against the finger.
    pub fn is_trim_dragging(&self) -> bool {
        self.trim_drag.is_some()
    }

    /// Set the trim range (host reset on cue, restore on reload).
    pub fn set_trim(&mut self, cx: &mut Cx, start: f64, end: f64) {
        let start = start.clamp(0.0, 1.0);
        let end = end.clamp(0.0, 1.0).max(start + TRIM_MIN_GAP).min(1.0);
        self.trim = (start.min(end - TRIM_MIN_GAP).max(0.0), end);
        self.view.redraw(cx);
    }

    pub fn looping(&self) -> bool {
        self.looping
    }

    pub fn set_looping(&mut self, cx: &mut Cx, looping: bool) {
        self.looping = looping;
        self.sync_loop_face(cx);
    }

    fn sync_loop_face(&mut self, cx: &mut Cx) {
        let color = if self.looping {
            Vec4f::from_u32(0xdfe6ecff)
        } else {
            Vec4f::from_u32(0x5a616aff)
        };
        let mut label = self.view.label(cx, ids!(loop_label));
        script_apply_eval!(cx, label, {
            draw_text +: { color: #(color) }
        });
        self.view.redraw(cx);
    }

    /// The band the picture occupies (letterboxed inside the lane); the
    /// seek bar and the drag math both measure against it. Falls back to
    /// the lane before the first frame.
    fn band(&self, cx: &mut Cx) -> Rect {
        let picture = self.view.image(cx, ids!(picture)).area().rect(cx);
        match picture.size.x > 1.0 {
            true => picture,
            false => self.view.view(cx, ids!(lane)).area().rect(cx),
        }
    }

    fn fraction_at(x: f64, band: Rect) -> Option<f64> {
        (band.size.x > 1.0).then(|| ((x - band.pos.x) / band.size.x).clamp(0.0, 1.0))
    }

    fn seek_to(&mut self, cx: &mut Cx, uid: WidgetUid, x: f64) {
        let band = self.bar_band(cx);
        let Some(fraction) = Self::fraction_at(x, band) else {
            return;
        };
        // Playback lives inside the trim range; so does the playhead.
        self.fraction = fraction.clamp(self.trim.0, self.trim.1);
        cx.widget_action(uid, VideoAction::Seek(self.fraction));
        self.view.redraw(cx);
    }

    /// The rect the seek bar occupies (strip in `bar_below`, picture band
    /// otherwise) — scrub x-mapping and the trim notches both measure
    /// against it.
    fn bar_band(&self, cx: &mut Cx) -> Rect {
        if self.bar_below {
            // Inset to the transport buttons' margin — the bar must not
            // run flush to the panel edge.
            let rect = self.view.view(cx, ids!(bar_slot)).area().rect(cx);
            Rect {
                pos: dvec2(rect.pos.x + 4.0, rect.pos.y),
                size: dvec2((rect.size.x - 8.0).max(1.0), rect.size.y),
            }
        } else {
            self.band(cx)
        }
    }

    /// Which trim notch (0 = IN, 1 = OUT) a press at `abs` grabs, if any.
    /// The notches win over the scrub surface — nearest one on overlap.
    fn trim_hit(&self, cx: &mut Cx, abs: DVec2) -> Option<usize> {
        if !self.trim_handles {
            return None;
        }
        let band = self.bar_band(cx);
        if band.size.x <= 1.0 {
            return None;
        }
        // Vertically: anywhere in the strip (bar_below) or within the
        // floating bar's lift zone.
        if !self.bar_below {
            let y = band.pos.y + band.size.y - TRACK_LIFT - TRACK_H * 0.5;
            if (abs.y - y).abs() > TRACK_LIFT + TRACK_H {
                return None;
            }
        } else if abs.y < band.pos.y || abs.y > band.pos.y + band.size.y {
            return None;
        }
        let x_in = band.pos.x + band.size.x * self.trim.0;
        let x_out = band.pos.x + band.size.x * self.trim.1;
        let (d_in, d_out) = ((abs.x - x_in).abs(), (abs.x - x_out).abs());
        if d_in <= TRIM_GRAB || d_out <= TRIM_GRAB {
            Some(if d_in <= d_out { 0 } else { 1 })
        } else {
            None
        }
    }

    fn drag_trim(&mut self, cx: &mut Cx, x: f64) {
        let Some(which) = self.trim_drag else { return };
        let band = self.bar_band(cx);
        let Some(f) = Self::fraction_at(x, band) else { return };
        if which == 0 {
            self.trim.0 = f.min(self.trim.1 - TRIM_MIN_GAP).max(0.0);
        } else {
            self.trim.1 = f.max(self.trim.0 + TRIM_MIN_GAP).min(1.0);
        }
        self.fraction = self.fraction.clamp(self.trim.0, self.trim.1);
        self.view.redraw(cx);
    }
}

impl Widget for VideoView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let slot = self.view.view(cx, ids!(bar_slot));
        if slot.visible() != self.bar_below {
            slot.set_visible(cx, self.bar_below);
        }
        let controls = self.view.view(cx, ids!(controls));
        if controls.visible() != self.show_controls {
            controls.set_visible(cx, self.show_controls);
        }
        while self.view.draw_walk(cx, scope, walk).is_step() {}
        // Track + progress + knob — over the picture's bottom edge, or in
        // the reserved strip below it (`bar_below`) — drawn last so they
        // ride every resize without a layout pass.
        // In bar_below mode the bar spans the PLAYER's full width (the
        // reserved strip), the normal-video-player shape; floating mode
        // hugs the letterboxed picture band.
        let band = self.bar_band(cx);
        if band.size.x > KNOB_W {
            let y = if self.bar_below {
                band.pos.y + (band.size.y - TRACK_H) * 0.5
            } else {
                band.pos.y + band.size.y - TRACK_LIFT - TRACK_H
            };
            let (t_in, t_out) = if self.trim_handles { self.trim } else { (0.0, 1.0) };
            let (x0, w) = (band.pos.x, band.size.x);
            // Trimmed-off tails draw DIM; the active range keeps the
            // normal track. Untrimmed, the tails are zero-width.
            if t_in > 0.0 {
                self.draw_tail.draw_abs(
                    cx,
                    Rect { pos: dvec2(x0, y), size: dvec2(w * t_in, TRACK_H) },
                );
            }
            if t_out < 1.0 {
                self.draw_tail.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x0 + w * t_out, y),
                        size: dvec2(w * (1.0 - t_out), TRACK_H),
                    },
                );
            }
            self.draw_track.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x0 + w * t_in, y),
                    size: dvec2(w * (t_out - t_in), TRACK_H),
                },
            );
            // Progress runs from IN to the playhead.
            let f = self.fraction.clamp(t_in, t_out);
            self.draw_progress.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x0 + w * t_in, y),
                    size: dvec2(w * (f - t_in), TRACK_H),
                },
            );
            let knob_x = x0 + (w - KNOB_W) * f;
            self.draw_knob.draw_abs(
                cx,
                Rect {
                    pos: dvec2(knob_x, y - (KNOB_H - TRACK_H) / 2.0),
                    size: dvec2(KNOB_W, KNOB_H),
                },
            );
            // The IN/OUT brackets — [ and ] on the range edges, thin
            // glyphs on a comfortably wide grab target.
            if self.trim_handles {
                let hy = y - (KNOB_H + 4.0 - TRACK_H) / 2.0;
                let hs = dvec2(7.0, KNOB_H + 4.0);
                self.draw_handle_in.draw_abs(
                    cx,
                    Rect { pos: dvec2(x0 + w * t_in - 2.0, hy), size: hs },
                );
                self.draw_handle_out.draw_abs(
                    cx,
                    Rect { pos: dvec2(x0 + w * t_out - 5.0, hy), size: hs },
                );
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let uid = self.widget_uid();
        // Press/drag/release anywhere on the lane scrubs — the lane holds
        // the finger capture (it carries the cursor), same as the audio
        // face; WHERE in the clip is measured against the picture band.
        let lane = self.view.view(cx, ids!(lane)).area();
        // One press decides ONCE what it is: a trim notch (they hit-test
        // first — the playhead drag stays distinct) or a scrub. The strip
        // and the lane are both surfaces for either.
        let surfaces = if self.bar_below {
            [Some(self.view.view(cx, ids!(bar_slot)).area()), Some(lane)]
        } else {
            [Some(lane), None]
        };
        for area in surfaces.into_iter().flatten() {
            match event.hits(cx, area) {
                Hit::FingerDown(fe) => {
                    if let Some(which) = self.trim_hit(cx, fe.abs) {
                        self.trim_drag = Some(which);
                        self.drag_trim(cx, fe.abs.x);
                    } else {
                        self.scrubbing = true;
                        self.seek_to(cx, uid, fe.abs.x);
                    }
                }
                Hit::FingerMove(fe) if self.trim_drag.is_some() => {
                    self.drag_trim(cx, fe.abs.x);
                }
                Hit::FingerMove(fe) if self.scrubbing => {
                    self.seek_to(cx, uid, fe.abs.x);
                }
                Hit::FingerUp(fe) => {
                    if self.trim_drag.is_some() {
                        self.drag_trim(cx, fe.abs.x);
                        self.trim_drag = None;
                        // ONE report per drag, on release — the host
                        // re-fits beat-synced loops here.
                        cx.widget_action(
                            uid,
                            VideoAction::TrimChanged(self.trim.0, self.trim.1),
                        );
                    } else if self.scrubbing {
                        self.seek_to(cx, uid, fe.abs.x);
                    }
                    self.scrubbing = false;
                }
                _ => {}
            }
        }
        let play_area = self.view.view(cx, ids!(play)).area();
        if let Hit::FingerDown(_) = event.hits(cx, play_area) {
            cx.widget_action(uid, VideoAction::TogglePlay);
        }
        let restart_area = self.view.view(cx, ids!(restart)).area();
        if let Hit::FingerDown(_) = event.hits(cx, restart_area) {
            cx.widget_action(uid, VideoAction::Restart);
        }
        let loop_area = self.view.view(cx, ids!(loop_box)).area();
        if let Hit::FingerDown(_) = event.hits(cx, loop_area) {
            self.looping = !self.looping;
            self.sync_loop_face(cx);
            cx.widget_action(uid, VideoAction::ToggleLoop);
        }
    }
}

impl VideoViewRef {
    pub fn set_frame(&self, cx: &mut Cx, texture: Option<Texture>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_frame(cx, texture);
        }
    }

    pub fn set_transport(&self, cx: &mut Cx, fraction: f64, playing: bool, position: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_transport(cx, fraction, playing, position);
        }
    }

    pub fn is_scrubbing(&self) -> bool {
        self.borrow().map(|inner| inner.is_scrubbing()).unwrap_or(false)
    }

    pub fn looping(&self) -> bool {
        self.borrow().map(|inner| inner.looping()).unwrap_or(true)
    }

    pub fn trim(&self) -> (f64, f64) {
        self.borrow().map(|inner| inner.trim()).unwrap_or((0.0, 1.0))
    }

    pub fn is_trim_dragging(&self) -> bool {
        self.borrow().map(|inner| inner.is_trim_dragging()).unwrap_or(false)
    }

    pub fn set_trim(&self, cx: &mut Cx, start: f64, end: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_trim(cx, start, end);
        }
    }

    pub fn action(&self, actions: &Actions) -> VideoAction {
        match actions.find_widget_action(self.widget_uid()) {
            Some(action) => action.cast(),
            None => VideoAction::None,
        }
    }
}
