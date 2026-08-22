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
            color: #xff5a4d
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
}

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
        let band = self.band(cx);
        let Some(fraction) = Self::fraction_at(x, band) else {
            return;
        };
        self.fraction = fraction;
        cx.widget_action(uid, VideoAction::Seek(self.fraction));
        self.view.redraw(cx);
    }
}

impl Widget for VideoView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while self.view.draw_walk(cx, scope, walk).is_step() {}
        // Track + progress + knob over the picture's bottom edge, drawn
        // last so they ride every resize without a layout pass.
        let band = self.band(cx);
        if band.size.x > KNOB_W {
            let y = band.pos.y + band.size.y - TRACK_LIFT - TRACK_H;
            self.draw_track.draw_abs(
                cx,
                Rect {
                    pos: dvec2(band.pos.x, y),
                    size: dvec2(band.size.x, TRACK_H),
                },
            );
            self.draw_progress.draw_abs(
                cx,
                Rect {
                    pos: dvec2(band.pos.x, y),
                    size: dvec2(band.size.x * self.fraction, TRACK_H),
                },
            );
            let knob_x = band.pos.x + (band.size.x - KNOB_W) * self.fraction;
            self.draw_knob.draw_abs(
                cx,
                Rect {
                    pos: dvec2(knob_x, y - (KNOB_H - TRACK_H) / 2.0),
                    size: dvec2(KNOB_W, KNOB_H),
                },
            );
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
        match event.hits(cx, lane) {
            Hit::FingerDown(fe) => {
                self.scrubbing = true;
                self.seek_to(cx, uid, fe.abs.x);
            }
            Hit::FingerMove(fe) if self.scrubbing => {
                self.seek_to(cx, uid, fe.abs.x);
            }
            Hit::FingerUp(fe) => {
                if self.scrubbing {
                    self.seek_to(cx, uid, fe.abs.x);
                }
                self.scrubbing = false;
            }
            _ => {}
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

    pub fn action(&self, actions: &Actions) -> VideoAction {
        match actions.find_widget_action(self.widget_uid()) {
            Some(action) => action.cast(),
            None => VideoAction::None,
        }
    }
}
