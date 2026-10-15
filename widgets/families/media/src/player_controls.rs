//! PlayerControls — the glass transport a player lays over its picture.
//!
//! A frosted, rounded bar along the bottom (play/pause, the elapsed time, a
//! scrubber with a round knob, the total and a skip button, plus a skip-back
//! button when the bar is wide enough) and a small frosted pill top-right
//! with the volume and a speaker that mutes. Both panels are
//! `GaussRoundedView`s, so they blur whatever the window drew under them —
//! the video — through the window's one shared pyramid; nothing here
//! samples a texture of its own.
//!
//! It plays nothing. A host tells it where playback is (`set_playing`,
//! `set_position`, `set_duration`, `set_volume`) and reads back what the
//! person asked for as [`PlayerControlsAction`]s. `VideoPlayer` is that host
//! for the `Video` widget; an app with its own decoder (apps/video) is
//! another.
//!
//! # Showing and hiding
//!
//! * Paused (or not started): always shown.
//! * Playing: a tap on the picture shows or hides them (a mouse click
//!   plays or pauses instead; hovering shows them), fading over 150 ms, and they hide on
//!   their own `hide_after` seconds after the last touch, click or mouse
//!   move over the picture.
//! * A double tap on the left or right side skips back or forward
//!   `skip_secs` and leaves the shown/hidden state as it was before the
//!   first tap of the pair.
//! * Space toggles play when the keyboard is not taken by something else.
//!
//! # Cost
//!
//! Hidden, the panels are not drawn, so the window does not capture or
//! blur anything for them. Shown over a paused picture nothing redraws, so
//! the cost is one capture when they appear. Shown over a playing picture
//! the window captures and blurs once per video frame — the price of glass
//! over moving content — for at most `hide_after` seconds.
use makepad_widgets_core::*;
use crate::playback_bar::{format_clock, PlaybackBar, PlaybackBarWidgetExt};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PlayerControlsBase = #(PlayerControls::register_widget(vm))

    // One frosted material for both panels: the video blurred well past
    // recognition, darkened enough that white text reads over a bright sky,
    // with a hairline edge.
    mod.widgets.PlayerGlass = GaussRoundedView{
        flow: Right
        align: Align{y: 0.5}
        draw_bg +: {
            corner_radius: 32.0
            blur_level: 3.0
            lensing_effect: 1.0
            lensing_strength: 2.0
            lensing_width: 8.0
            tint_color: #x16161a
            tint_alpha: 0.42
            surface_alpha: 1.0
            border_color: #xffffff
            border_alpha: 0.16
            border_width: 0.75
            rim_alpha: 0.25
            specular_strength: 0.0
            shadow_color: #x00000040
            fallback_color: #x2a2a30
        }
    }

    // A round touch target with a white glyph: 0 play, 1 pause, 2 skip
    // forward, 3 skip back, 4 speaker, 5 speaker muted. One shader for every
    // glyph, so the whole transport is one pipeline on the GPU.
    mod.widgets.PlayerGlyphFace = View{
        width: 48
        height: 48
        show_bg: true
        draw_bg +: {
            glyph: instance(0.0)
            fade: instance(1.0)
            ink: instance(#xffffff)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                let s = min(self.rect_size.x, self.rect_size.y) * 0.7
                if self.glyph < 0.5 {
                    sdf.move_to(c.x - s * 0.30, c.y - s * 0.42)
                    sdf.line_to(c.x + s * 0.46, c.y)
                    sdf.line_to(c.x - s * 0.30, c.y + s * 0.42)
                    sdf.close_path()
                    sdf.fill(self.ink)
                } else if self.glyph < 1.5 {
                    sdf.box(c.x - s * 0.34, c.y - s * 0.40, s * 0.24, s * 0.80, s * 0.06)
                    sdf.fill(self.ink)
                    sdf.box(c.x + s * 0.10, c.y - s * 0.40, s * 0.24, s * 0.80, s * 0.06)
                    sdf.fill(self.ink)
                } else if self.glyph < 3.5 {
                    // Two chevrons, pointing forward (2) or back (3).
                    let d = 1.0 - 2.0 * step(2.5, self.glyph)
                    let w = s * 0.24
                    sdf.move_to(c.x - d * (s * 0.36), c.y - s * 0.30)
                    sdf.line_to(c.x - d * (s * 0.36 - w), c.y)
                    sdf.line_to(c.x - d * (s * 0.36), c.y + s * 0.30)
                    sdf.move_to(c.x - d * (s * 0.02), c.y - s * 0.30)
                    sdf.line_to(c.x + d * w - d * (s * 0.02), c.y)
                    sdf.line_to(c.x - d * (s * 0.02), c.y + s * 0.30)
                    sdf.stroke(self.ink, max(1.5, s * 0.10))
                } else {
                    // The speaker: a box and a flared cone.
                    sdf.box(c.x - s * 0.52, c.y - s * 0.15, s * 0.20, s * 0.30, s * 0.04)
                    sdf.fill(self.ink)
                    sdf.move_to(c.x - s * 0.36, c.y - s * 0.15)
                    sdf.line_to(c.x - s * 0.08, c.y - s * 0.40)
                    sdf.line_to(c.x - s * 0.08, c.y + s * 0.40)
                    sdf.line_to(c.x - s * 0.36, c.y + s * 0.15)
                    sdf.close_path()
                    sdf.fill(self.ink)
                    if self.glyph < 4.5 {
                        // Two sound waves.
                        sdf.arc_round_caps(c.x - s * 0.02, c.y, s * 0.22, -2.54, -0.6, s * 0.09)
                        sdf.fill(self.ink)
                        sdf.arc_round_caps(c.x - s * 0.02, c.y, s * 0.44, -2.39, -0.75, s * 0.09)
                        sdf.fill(self.ink)
                    } else {
                        // Muted: a cross where the waves were.
                        sdf.move_to(c.x + s * 0.10, c.y - s * 0.18)
                        sdf.line_to(c.x + s * 0.46, c.y + s * 0.18)
                        sdf.move_to(c.x + s * 0.46, c.y - s * 0.18)
                        sdf.line_to(c.x + s * 0.10, c.y + s * 0.18)
                        sdf.stroke(self.ink, max(1.5, s * 0.09))
                    }
                }
                return sdf.result * self.fade
            }
        }
    }

    mod.widgets.PlayerGlyph = mod.widgets.PlayerGlyphFace{
        cursor: MouseCursor.Hand
        grab_key_focus: false
    }

    // A button with two faces, one shown at a time (play/pause,
    // speaker/muted): which one is a visibility flag, so the face is
    // right on every redraw whatever rebuilt the draw list.
    mod.widgets.PlayerToggle = View{
        width: 48
        height: 48
        flow: Overlay
        cursor: MouseCursor.Hand
        grab_key_focus: false
        off_face := mod.widgets.PlayerGlyphFace{}
        on_face := mod.widgets.PlayerGlyphFace{visible: false}
    }

    mod.widgets.PlayerClock = Label{
        width: Fit
        padding: 0
        draw_text +: {
            color: #xffffff
            text_style: theme.font_regular{font_size: 15}
        }
    }

    // The scrubber and the volume share one look: a pale track, a white
    // played part and a large white knob, no ring, no clock of its own.
    mod.widgets.PlayerTrack = PlaybackBarFlat{
        width: Fill
        height: 44
        margin: 0
        show_clock: false
        track_inset: 13.
        draw_bg +: {
            thickness: uniform(6.0)
            thickness_hover: uniform(8.0)
            knob_size: uniform(24.0)
            knob_size_hover: uniform(26.0)
            knob_ring: uniform(0.0)
            color_track: uniform(#xffffff45)
            color_elapsed: uniform(#xffffff)
            color_knob: uniform(#xffffff)
            color_knob_ring: uniform(#xffffff00)
            border_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        }
    }

    /** The glass transport over a player's picture: play/pause, elapsed, scrubber, total, skip, and a volume pill. */
    mod.widgets.PlayerControls = set_type_default() do mod.widgets.PlayerControlsBase{
        width: Fill
        height: Fill
        flow: Overlay
        /** seconds the controls stay up after the last interaction while playing 1..10 step 0.5 */
        hide_after: 3.0
        /** seconds a skip button or a double tap moves 1..60 step 1 */
        skip_secs: 10.0
        /** below this width (points) the bar drops its skip-back button 200..1000 step 10 */
        wide_from: 520.0

        // The double-tap answer: a glass chip on the side that was tapped.
        hint_row := View{
            width: Fill
            height: Fill
            flow: Right
            align: Align{y: 0.5}
            padding: Inset{left: 48 right: 48}
            visible: false
            hint_back := mod.widgets.PlayerGlass{
                width: 96 height: 64
                align: Align{x: 0.5 y: 0.5}
                draw_bg +: {corner_radius: 32.0}
                mod.widgets.PlayerGlyphFace{width: 40 height: 40 draw_bg +: {glyph: 3.0}}
                mod.widgets.PlayerClock{text: "10"}
            }
            View{width: Fill height: 1}
            hint_forward := mod.widgets.PlayerGlass{
                width: 96 height: 64
                align: Align{x: 0.5 y: 0.5}
                draw_bg +: {corner_radius: 32.0}
                mod.widgets.PlayerClock{text: "10"}
                mod.widgets.PlayerGlyphFace{width: 40 height: 40 draw_bg +: {glyph: 2.0}}
            }
        }

        top_row := View{
            width: Fill
            height: Fit
            align: Align{x: 1.0}
            padding: Inset{top: 16 right: 16 left: 16}
            // An app's own chips (an Open button, a title) go here; they
            // show and hide with the rest.
            leading := View{width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.5}}
            volume_pill := mod.widgets.PlayerGlass{
                width: 220
                height: 64
                padding: Inset{left: 14 right: 8}
                spacing: 4
                draw_bg +: {corner_radius: 32.0}
                volume := mod.widgets.PlayerTrack{duration: 1.0 position: 1.0 step_secs: 0.05}
                mute_button := mod.widgets.PlayerToggle{
                    off_face +: {draw_bg +: {glyph: 4.0}}
                    on_face +: {draw_bg +: {glyph: 5.0}}
                }
            }
        }

        bottom_row := View{
            width: Fill
            height: Fill
            align: Align{y: 1.0}
            padding: Inset{left: 16 right: 16 bottom: 16}
            bar := mod.widgets.PlayerGlass{
                width: Fill
                height: 72
                padding: Inset{left: 12 right: 12}
                spacing: 6
                draw_bg +: {corner_radius: 36.0}
                back_button := mod.widgets.PlayerGlyph{draw_bg +: {glyph: 3.0}}
                play_button := mod.widgets.PlayerToggle{
                    off_face +: {draw_bg +: {glyph: 0.0}}
                    on_face +: {draw_bg +: {glyph: 1.0}}
                }
                elapsed := mod.widgets.PlayerClock{text: "0:00" margin: Inset{left: 4}}
                scrub := mod.widgets.PlayerTrack{}
                total := mod.widgets.PlayerClock{text: "0:00"}
                skip_button := mod.widgets.PlayerGlyph{draw_bg +: {glyph: 2.0}}
            }
        }
    }
}

/// What the person asked a [`PlayerControls`] for. The host acts on these;
/// the controls already show the answer (the knob where the finger let go,
/// the new volume), and the host's next reports confirm it.
#[derive(Clone, Debug, Default)]
pub enum PlayerControlsAction {
    #[default]
    None,
    /// The play/pause button, a tap on the picture's centre while paused,
    /// or Space.
    TogglePlay,
    /// Go to this many seconds in — the scrubber on release, or its arrow
    /// keys.
    Seek(f64),
    /// Move by this many seconds (negative is back): the skip buttons and
    /// the double taps.
    SkipBy(f64),
    /// The volume slider, 0..1, on every move.
    Volume(f64),
    /// The speaker button.
    ToggleMute,
}

#[derive(Script, ScriptHook, Widget)]
pub struct PlayerControls {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    #[live(3.0)]
    hide_after: f64,
    #[live(10.0)]
    skip_secs: f64,
    #[live(520.0)]
    wide_from: f64,

    #[rust(true)]
    shown: bool,
    #[rust]
    playing: bool,
    #[rust]
    position: f64,
    #[rust]
    duration: f64,
    #[rust(1.0)]
    volume: f64,
    #[rust]
    muted: bool,
    /// A finger is on the scrubber: the elapsed clock shows its answer.
    #[rust]
    scrubbing: bool,
    #[rust]
    hide_timer: Timer,
    /// How far the controls are faded in, 0..1, and the clock moving it.
    #[rust(1.0)]
    fade: f64,
    #[rust]
    fade_frame: NextFrame,
    #[rust]
    fade_last: Option<f64>,
    /// A mouse click on a side waits this long for a second click (a skip)
    /// before it plays or pauses.
    #[rust]
    click_timer: Timer,
    #[rust]
    hint_timer: Timer,
    /// The last tap on the picture, for spotting a double tap.
    #[rust]
    last_tap: Option<(f64, Vec2d)>,
    /// Whether the controls were up before the first tap of a pair.
    #[rust]
    shown_before_tap: bool,
}

/// Two taps closer than this (and near each other) are a double tap.
const DOUBLE_TAP_SECS: f64 = 0.32;
const DOUBLE_TAP_SLOP: f64 = 48.0;
/// How long the controls take to fade in or out.
const FADE_SECS: f64 = 0.15;

impl Widget for PlayerControls {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The safe area moves with every rotation, so it is read per draw
        // rather than baked into the DSL padding.
        // Only the part of the safe area this widget actually reaches into
        // counts: a small player in a page is not under the status bar.
        let raw = cx.display_context.safe_area_insets;
        let rect = cx.turtle().rect();
        let width = rect.size.x;
        let win = cx.current_pass_size();
        let insets = SafeAreaInsets {
            top: (raw.top - rect.pos.y).max(0.0),
            left: (raw.left - rect.pos.x).max(0.0),
            right: (raw.right - (win.x - rect.pos.x - rect.size.x)).max(0.0),
            bottom: (raw.bottom - (win.y - rect.pos.y - rect.size.y)).max(0.0),
        };
        if let Some(mut top) = self.view.view(cx, ids!(top_row)).borrow_mut() {
            top.layout.padding = Inset {
                top: 16.0 + insets.top,
                right: 16.0 + insets.right,
                left: 16.0 + insets.left,
                bottom: 0.0,
            };
            top.visible = self.fade > 0.0;
        }
        if let Some(mut bottom) = self.view.view(cx, ids!(bottom_row)).borrow_mut() {
            bottom.layout.padding = Inset {
                top: 0.0,
                right: 16.0 + insets.right,
                left: 16.0 + insets.left,
                bottom: 16.0 + insets.bottom,
            };
            bottom.visible = self.fade > 0.0;
        }
        let wide = width <= 0.0 || width - insets.left - insets.right >= self.wide_from;
        if let Some(mut back) = self.view.view(cx, ids!(back_button)).borrow_mut() {
            back.visible = wide;
        }
        if self.fade > 0.0 {
            // Set on every draw: a style re-apply (a rotation) puts the
            // instances back to their defaults.
            let t = self.fade.clamp(0.0, 1.0);
            let alpha = (t * t * (3.0 - 2.0 * t)) as f32;
            for row in [ids!(top_row), ids!(bottom_row)] {
                let row = self.view.widget(cx, row);
                apply_fade(cx, &row, alpha);
            }
        }
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.hide_timer.is_event(event).is_some() {
            if self.playing && !self.scrubbing {
                self.set_shown(cx, false);
            }
        }
        if let Some(frame) = self.fade_frame.is_event(event) {
            let dt = self.fade_last.map_or(1.0 / 60.0, |last| (frame.time - last).clamp(0.0, 0.1));
            self.fade_last = Some(frame.time);
            let target = if self.shown { 1.0 } else { 0.0 };
            let step = dt / FADE_SECS;
            self.fade = if self.fade < target {
                (self.fade + step).min(target)
            } else {
                (self.fade - step).max(target)
            };
            if self.fade != target {
                self.fade_frame = cx.new_next_frame();
            } else {
                self.fade_last = None;
            }
            self.view.redraw(cx);
        }
        if self.click_timer.is_event(event).is_some() {
            self.emit(cx, PlayerControlsAction::TogglePlay);
            self.poke(cx);
        }
        if self.hint_timer.is_event(event).is_some() {
            self.view.view(cx, ids!(hint_row)).set_visible(cx, false);
        }

        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if !actions.is_empty() {
            self.handle_child_actions(cx, &actions);
            // An app's own chips in `leading` report to the app.
            cx.extend_actions(actions);
        }

        // The picture itself: everything the panels did not take.
        let area = self.view.area();
        match event.hits(cx, area) {
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                // The mouse over the picture is attention, like a touch.
                self.poke(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(area);
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() && fe.was_tap() => {
                self.on_tap(cx, fe.abs, fe.time, fe.rect, fe.device.is_mouse());
            }
            _ => {}
        }

        if let Event::KeyDown(ke) = event {
            let focus = cx.key_focus();
            if (focus.is_empty() || focus == area) && ke.key_code == KeyCode::Space {
                self.emit(cx, PlayerControlsAction::TogglePlay);
                self.poke(cx);
            }
        }
    }
}

impl PlayerControls {
    fn emit(&self, cx: &mut Cx, action: PlayerControlsAction) {
        cx.widget_action(self.widget_uid(), action);
    }

    fn handle_child_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let tapped = |view: ViewRef| view.finger_up(actions).is_some_and(|fe| fe.is_over && !fe.cancelled);
        if tapped(self.view.view(cx, ids!(play_button))) {
            self.emit(cx, PlayerControlsAction::TogglePlay);
            self.poke(cx);
        }
        if tapped(self.view.view(cx, ids!(skip_button))) {
            self.emit(cx, PlayerControlsAction::SkipBy(self.skip_secs));
            self.poke(cx);
        }
        if tapped(self.view.view(cx, ids!(back_button))) {
            self.emit(cx, PlayerControlsAction::SkipBy(-self.skip_secs));
            self.poke(cx);
        }
        if tapped(self.view.view(cx, ids!(mute_button))) {
            self.emit(cx, PlayerControlsAction::ToggleMute);
            self.poke(cx);
        }

        let scrub = self.view.playback_bar(cx, ids!(scrub));
        if scrub.grabbed(actions) {
            self.scrubbing = true;
            self.poke(cx);
        }
        if let Some(seconds) = scrub.scrubbed(actions) {
            self.set_elapsed_text(cx, seconds);
        }
        if let Some(seconds) = scrub.released(actions) {
            // Preview while the finger moves, seek where it lets go.
            self.scrubbing = false;
            self.emit(cx, PlayerControlsAction::Seek(seconds));
            self.poke(cx);
        } else if !self.scrubbing {
            // The arrow keys: one whole intent, no grab or release.
            if let Some(seconds) = scrub.seek(actions) {
                self.emit(cx, PlayerControlsAction::Seek(seconds));
                self.poke(cx);
            }
        }

        let volume = self.view.playback_bar(cx, ids!(volume));
        if let Some(value) = volume.scrubbed(actions) {
            self.volume = value.clamp(0.0, 1.0);
            self.emit(cx, PlayerControlsAction::Volume(self.volume));
            self.poke(cx);
        }
    }

    /// A tap on the picture: show or hide, or — the second of a quick pair
    /// on one side — skip.
    fn on_tap(&mut self, cx: &mut Cx, abs: Vec2d, time: f64, rect: Rect, mouse: bool) {
        // A tap on a panel's own glass, between its buttons, is not a tap
        // on the picture.
        if self.shown {
            for panel in [ids!(bar), ids!(volume_pill)] {
                if self.view.widget(cx, panel).area().rect(cx).contains(abs) {
                    self.poke(cx);
                    return;
                }
            }
        }
        let double = self
            .last_tap
            .is_some_and(|(t, at)| time - t < DOUBLE_TAP_SECS && (abs - at).length() < DOUBLE_TAP_SLOP);
        let third = rect.size.x / 3.0;
        let side = if abs.x < rect.pos.x + third {
            -1.0
        } else if abs.x > rect.pos.x + rect.size.x - third {
            1.0
        } else {
            0.0
        };
        if double && side != 0.0 {
            self.last_tap = None;
            // The first click of the pair was holding its play/pause back
            // for exactly this; it never happens.
            cx.stop_timer(self.click_timer);
            self.emit(cx, PlayerControlsAction::SkipBy(side * self.skip_secs));
            self.show_hint(cx, side > 0.0);
            // The pair is a skip, not two show/hide taps.
            let before = self.shown_before_tap;
            self.set_shown(cx, before);
            if before {
                self.arm_hide(cx);
            }
            return;
        }
        self.last_tap = Some((time, abs));
        self.shown_before_tap = self.shown;
        if mouse {
            // A pointer shows the controls by hovering, so a click is free
            // to do what a click on a desktop player does: play or pause.
            // In the middle at once; on a side only once it is clear no
            // second click (a skip) follows, so a double click skips and
            // nothing else.
            if side == 0.0 {
                self.emit(cx, PlayerControlsAction::TogglePlay);
                self.poke(cx);
            } else {
                cx.stop_timer(self.click_timer);
                self.click_timer = cx.start_timeout(DOUBLE_TAP_SECS);
            }
            return;
        }
        if !self.playing {
            // Paused, the controls stay; a tap in the middle plays.
            if side == 0.0 {
                self.emit(cx, PlayerControlsAction::TogglePlay);
            }
            self.set_shown(cx, true);
            return;
        }
        let show = !self.shown;
        self.set_shown(cx, show);
        if show {
            self.arm_hide(cx);
        }
    }

    fn show_hint(&mut self, cx: &mut Cx, forward: bool) {
        self.view.widget(cx, ids!(hint_back)).set_visible(cx, !forward);
        self.view.widget(cx, ids!(hint_forward)).set_visible(cx, forward);
        self.view.view(cx, ids!(hint_row)).set_visible(cx, true);
        cx.stop_timer(self.hint_timer);
        self.hint_timer = cx.start_timeout(0.6);
    }

    /// Any attention: show the controls and restart the hide countdown.
    fn poke(&mut self, cx: &mut Cx) {
        if !self.shown {
            self.set_shown(cx, true);
        }
        self.arm_hide(cx);
    }

    fn arm_hide(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.hide_timer);
        if self.playing {
            self.hide_timer = cx.start_timeout(self.hide_after);
        }
    }

    fn set_shown(&mut self, cx: &mut Cx, shown: bool) {
        let shown = shown || !self.playing;
        if self.shown != shown {
            self.shown = shown;
            self.fade_last = None;
            self.fade_frame = cx.new_next_frame();
            self.view.redraw(cx);
        }
    }

    fn set_elapsed_text(&mut self, cx: &mut Cx, seconds: f64) {
        self.view.label(cx, ids!(elapsed)).set_text(cx, &format_clock(seconds));
    }

    fn set_toggle(&mut self, cx: &mut Cx, button: &[LiveId], on: bool) {
        let button = self.view.view(cx, button);
        button.view(cx, ids!(off_face)).set_visible(cx, !on);
        button.view(cx, ids!(on_face)).set_visible(cx, on);
    }

    /// Whether playback runs. Pausing pins the controls up; playing starts
    /// the hide countdown.
    pub fn set_playing(&mut self, cx: &mut Cx, playing: bool) {
        if self.playing == playing {
            return;
        }
        self.playing = playing;
        self.set_toggle(cx, ids!(play_button), playing);
        if playing {
            self.arm_hide(cx);
        } else {
            cx.stop_timer(self.hide_timer);
            self.set_shown(cx, true);
        }
    }

    pub fn set_position(&mut self, cx: &mut Cx, seconds: f64) {
        if (self.position - seconds).abs() < 1e-3 {
            return;
        }
        self.position = seconds;
        self.view.playback_bar(cx, ids!(scrub)).set_position(cx, seconds);
        if !self.scrubbing {
            let shown = self.view.playback_bar(cx, ids!(scrub)).shown_seconds();
            self.set_elapsed_text(cx, shown);
        }
    }

    pub fn set_duration(&mut self, cx: &mut Cx, seconds: f64) {
        if (self.duration - seconds).abs() < 1e-3 {
            return;
        }
        self.duration = seconds;
        self.view.playback_bar(cx, ids!(scrub)).set_duration(cx, seconds);
        let text = if seconds > 0.0 { format_clock(seconds) } else { "--:--".to_string() };
        self.view.label(cx, ids!(total)).set_text(cx, &text);
    }

    /// The volume, 0..1, and whether it is muted.
    pub fn set_volume(&mut self, cx: &mut Cx, volume: f64, muted: bool) {
        let volume = volume.clamp(0.0, 1.0);
        if (self.volume - volume).abs() > 1e-4 {
            self.volume = volume;
            self.view.playback_bar(cx, ids!(volume)).set_position(cx, volume);
        }
        if self.muted != muted {
            self.muted = muted;
            self.set_toggle(cx, ids!(mute_button), muted);
        }
    }

    /// Show the controls as if touched (a host's own key handling, say).
    pub fn wake(&mut self, cx: &mut Cx) {
        self.poke(cx);
    }

    /// Whether the controls are up.
    pub fn is_shown(&self) -> bool {
        self.shown
    }
}

/// Fades a subtree of the controls: the glass, the glyphs, the clocks and
/// the bars, each through its own alpha (there is no layer opacity to set
/// once). Text keeps its colour and takes `alpha` as its alpha.
fn apply_fade(cx: &mut Cx, widget: &WidgetRef, alpha: f32) {
    if let Some(mut glass) = widget.borrow_mut::<GaussRoundedView>() {
        glass.draw_bg.draw_vars.set_dyn_instance(cx, id!(layer_opacity), &[alpha]);
    } else if let Some(mut view) = widget.borrow_mut::<View>() {
        view.draw_bg.draw_vars.set_dyn_instance(cx, id!(fade), &[alpha]);
    } else if let Some(mut label) = widget.borrow_mut::<Label>() {
        label.draw_text.color.w = alpha;
    } else if let Some(mut bar) = widget.borrow_mut::<PlaybackBar>() {
        bar.set_fade(cx, alpha);
    }
    widget.children(&mut |_, child| apply_fade(cx, &child, alpha));
}

impl PlayerControlsRef {
    pub fn set_playing(&self, cx: &mut Cx, playing: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_playing(cx, playing);
        }
    }

    pub fn set_position(&self, cx: &mut Cx, seconds: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_position(cx, seconds);
        }
    }

    pub fn set_duration(&self, cx: &mut Cx, seconds: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_duration(cx, seconds);
        }
    }

    pub fn set_volume(&self, cx: &mut Cx, volume: f64, muted: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_volume(cx, volume, muted);
        }
    }

    pub fn wake(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.wake(cx);
        }
    }

    pub fn is_shown(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.is_shown())
    }

    /// Every action these controls raised in `actions`, in order.
    pub fn actions(&self, actions: &Actions) -> Vec<PlayerControlsAction> {
        actions
            .filter_widget_actions_cast::<PlayerControlsAction>(self.widget_uid())
            .collect()
    }
}
