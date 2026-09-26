use crate::animator::*;
use crate::event::{ScrollPhase, TAP_COUNT_DISTANCE};
use crate::makepad_derive_widget::*;
use crate::makepad_draw::*;
use crate::scroll_motion::{
    drag_origin, estimate_release_velocity, press_settles_finger_scroll, push_sample, rubber_band_bounce,
    touch_drag_slop, touch_fling_limits,
    soften_bounce_velocity, stretch_displayed, stretch_raw, Fling, FrameClock, ScrollSample,
    CATCH_PRESS_WINDOW, COAST_STREAM_TIMEOUT, FLING_BOOST_MAX_DWELL, FLING_DECEL_RATE_PER_MS,
    FLING_MIN_TOTAL_DELTA, MOMENTUM_CUT_TOUCH_WINDOW, PER_FRAME_TO_PER_SECOND,
    RUBBER_BAND_STRETCH_STIFFNESS, RUBBER_BAND_TOUCH_RANGE,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    //use mod.animator.*
    set_type_default() do #(DrawScrollBar::script_shader(vm)){
        ..mod.draw.DrawQuad // splat in draw quad
    }

    mod.widgets.ScrollBarBase = #(ScrollBar::script_component(vm))
    /** The scrollbar: a rounded handle sized and placed by the scroll fraction. */
    mod.widgets.ScrollBar = set_type_default() do mod.widgets.ScrollBarBase{
        /** track thickness reserved in the layout, in pixels 2..24 step 1 */
        bar_size: 10.0
        /** gap between handle and container edge in pixels 0..12 step 0.5 */
        bar_side_margin: 3.0
        /** shortest the handle may shrink, in pixels 8..120 step 1 */
        min_handle_size: 30.0

        // The kinetics. A list carrying a bar shows these as its own
        // `scroll_bar.` rows, which is the one place they are reachable.
        /** the slowest release that still throws the view 0.05..2 step 0.05 */
        flick_scroll_minimum: 0.2
        /** the fastest a hard flick may throw it 20..600 step 10 */
        flick_scroll_maximum: 240.0
        /** how fast a flick runs down, per millisecond 0.98..0.9999 step 0.0005 */
        fling_decel: #(crate::scroll_motion::FLING_DECEL_RATE_PER_MS)
        /** rubber-band past the start edge 0..1 step 1 */
        bounce_at_start: true
        /** rubber-band past the end edge 0..1 step 1 */
        bounce_at_end: true
        /** DEAD: the fling speed comes from the measured release now */
        flick_scroll_scaling: 0.005
        /** DEAD: the run-down rate is fling_decel now */
        flick_scroll_decay: 0.97
        /** fade the bar out once scrolling stops, like macOS overlay scrollers */
        auto_hide: true
        /** how long the bar stays after the last scroll, in seconds 0..10 step 0.1 */
        hide_delay: 1.5
        /** The handle material: one rounded bar along the scroll axis. */
        draw_bg +: {
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** how visible the bar is 0..1 step 0.01 */
            opacity: instance(0.0)

            /** drawn handle thickness in pixels 1..20 step 0.5 */
            size: uniform(6.0)
            /** handle border thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)
            /** handle corner rounding radius 0..10 step 0.5 */
            border_radius: uniform(1.5)

            color: uniform(theme.color_outset)
            color_hover: uniform(theme.color_outset_hover)
            color_drag:  uniform(theme.color_outset_drag)

            border_color: uniform(theme.color_u_hidden)
            border_color_hover: uniform(theme.color_u_hidden)
            border_color_drag: uniform(theme.color_u_hidden)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                if self.is_vertical > 0.5 {
                    sdf.box(
                        /** handle edge inset 0..6 step 0.5 */ 1.
                        self.rect_size.y * self.norm_scroll
                        self.size
                        self.rect_size.y * self.norm_handle
                        self.border_radius
                    )
                }
                else {
                    sdf.box(
                        self.rect_size.x * self.norm_scroll
                        /** handle edge inset 0..6 step 0.5 */ 1.
                        self.rect_size.x * self.norm_handle
                        self.size
                        self.border_radius
                    )
                }

                sdf.fill_keep(mix(
                    self.color
                    mix(
                        self.color_hover
                        self.color_drag
                        self.drag
                    )
                    self.hover
                ))

                sdf.stroke(mix(
                    self.border_color
                    mix(
                        self.border_color_hover
                        self.border_color_drag
                        self.drag
                    )
                    self.hover
                ) self.border_size)
                return sdf.result * self.opacity
            }
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Play.Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {drag: 0 hover: 0}
                    }
                }
                on: AnimatorState{
                    cursor: MouseCursor.Default
                    from: {
                        all: Play.Forward {duration: 0.1}
                        drag: Play.Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {
                            drag: 0
                            hover: snap(1)
                        }
                    }
                }
                drag: AnimatorState{
                    cursor: MouseCursor.Default
                    from: {all: Play.Snap}
                    apply: {
                        draw_bg: {
                            drag: 1
                            hover: 1
                        }
                    }
                }
            }
            show: {
                default: @off
                off: AnimatorState{
                    from: {all: Play.Forward {duration: 0.3}}
                    apply: {draw_bg: {opacity: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Play.Snap}
                    apply: {draw_bg: {opacity: 1.0}}
                }
            }
        }
    }

    /** The tab-bar scrollbar: the strip scrolls, but there's no handle to grab. */
    mod.widgets.ScrollBarTabs = mod.widgets.ScrollBar {
        show_handle: false
        draw_bg +: {
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** how visible the bar is 0..1 step 0.01 */
            opacity: instance(0.0)

            /** drawn handle thickness in pixels 1..20 step 0.5 */
            size: uniform(6.0)
            /** handle border thickness in pixels 0..4 step 0.5 */
            border_size: uniform(1.0)
            /** handle corner rounding radius 0..10 step 0.5 */
            border_radius: uniform(1.5)

            color: uniform(theme.color_u_hidden)
            color_hover: uniform(theme.color_outset_hover)
            color_drag: uniform(theme.color_outset_drag)

            border_color: uniform(theme.color_u_hidden)
            border_color_hover: uniform(theme.color_u_hidden)
            border_color_drag: uniform(theme.color_u_hidden)

            pixel: fn() -> vec4 {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                if self.is_vertical > 0.5 {
                    sdf.box(
                        /** handle edge inset 0..6 step 0.5 */ 1.,
                        self.rect_size.y * self.norm_scroll
                        self.size
                        self.rect_size.y * self.norm_handle
                        self.border_radius
                    )
                }
                else {
                    sdf.box(
                        self.rect_size.x * self.norm_scroll
                        /** handle edge inset 0..6 step 0.5 */ 1.
                        self.rect_size.x * self.norm_handle
                        self.size
                        self.border_radius
                    )
                }

                sdf.fill_keep(mix(
                    self.color
                    mix(
                        self.color_hover
                        self.color_drag
                        self.drag
                    ),
                    self.hover
                ))

                sdf.stroke(mix(
                    self.border_color,
                    mix(
                        self.border_color_hover,
                        self.border_color_drag,
                        self.drag
                    ),
                    self.hover
                ) self.border_size)

                return sdf.result * self.opacity
            }
        }
    }


}

#[derive(Copy, Clone, Debug, Script, ScriptHook)]
pub enum ScrollAxis {
    #[pick]
    Horizontal,
    Vertical,
}

/// The scrolling state
enum ScrollState {
    Stopped,
    Drag {
        samples: Vec<ScrollSample>,
    },
    /// The rubber-band bounce at a scroll limit (see `scroll_motion`).
    /// `x0`/`v0` are signed: negative stretches before the start, positive past the end.
    /// `v0` is softened against the overscroll headroom on the bounce's first frame.
    Bounce {
        next_frame: NextFrame,
        x0: f64,
        v0: f64,
        /// Jitter-smoothed animation time, started on the first bounce frame.
        clock: FrameClock,
        /// Whether a finger on the screen caused this bounce (touch/mouse drag or the
        /// flick it released), which springs back with the stronger iOS curve, rather
        /// than trackpad momentum (Chrome's stiffer macOS curve).
        touch: bool,
    },
    Flick {
        /// The momentum-fling animation state (the platform's native momentum curve,
        /// frame-rate-independent). Shared with PortalList via [`crate::scroll_motion`]
        /// so every scrollable widget decelerates identically.
        fling: Fling,
        next_frame: NextFrame,
    },
}

#[derive(Script, ScriptHook, Animator)]
pub struct ScrollBar {
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_bg: DrawScrollBar,
    /// Whether the handle is drawn and can be grabbed. With it off the view still
    /// scrolls by wheel, trackpad and API, there's just nothing to click.
    #[live(true)]
    show_handle: bool,
    #[live]
    pub bar_size: f64,
    #[live]
    pub min_handle_size: f64, //minimum size of the handle in pixels
    #[live]
    bar_side_margin: f64,
    #[live(ScrollAxis::Horizontal)]
    pub axis: ScrollAxis,

    #[live]
    use_vertical_finger_scroll: bool,
    #[live]
    smoothing: Option<f64>,

    /// The minimum release speed for a fling, in per-frame pixels at a nominal 60fps
    /// (×60 → px/s). Below this a finger lift is a stop, not a flick; an active fling
    /// also stops once it decays below this speed. Same default as PortalList.
    #[live(0.2)]
    flick_scroll_minimum: f64,
    /// The maximum fling speed, in per-frame pixels at a nominal 60fps (×60 → px/s).
    /// 240 → 14,400 px/s. Same default as PortalList; raise for faster flicks.
    #[live(240.0)]
    flick_scroll_maximum: f64,
    /// Deprecated: unused. The fling speed now comes directly from the tracked release
    /// velocity (see [`crate::scroll_motion`]); kept only so existing DSL doesn't break.
    #[live(0.005)]
    flick_scroll_scaling: f64,
    /// Deprecated: unused. The deceleration rate is now `fling_decel`; kept so existing DSL
    /// doesn't break.
    #[live(0.97)]
    flick_scroll_decay: f64,
    /// Per-ms velocity decay of a touch-drag flick; lower stops sooner (see `scroll_motion`).
    #[live(FLING_DECEL_RATE_PER_MS)]
    fling_decel: f64,
    /// Whether to enable drag scrolling
    #[live(false)]
    drag_scrolling: bool,
    /// Whether the bar fades out once scrolling stops and the pointer leaves it.
    #[live(true)]
    auto_hide: bool,
    /// How long the bar stays shown after the last scroll, in seconds.
    #[live(1.5)]
    hide_delay: f64,

    #[apply_default]
    animator: Animator,

    #[rust]
    next_frame: NextFrame,
    #[rust]
    hide_timer: Timer,
    /// Whether the pointer is over the bar, which keeps it from hiding.
    #[rust]
    pointer_over: bool,
    /// The scroll position and viewport size when the bar last appeared; moving far enough from them shows it again.
    #[rust]
    last_shown: Option<(f64, f64)>,
    #[rust(false)]
    visible: bool,
    #[rust]
    view_total: f64, // the total view area
    #[rust]
    view_visible: f64, // the visible view area
    #[rust]
    scroll_size: f64, // the size of the scrollbar
    #[rust]
    scroll_pos: f64, // scrolling position non normalised

    #[rust]
    scroll_target: f64,
    #[rust]
    scroll_delta: f64,
    #[rust]
    drag_point: Option<f64>, // the point in pixels where we are dragging
    #[rust(ScrollState::Stopped)]
    scroll_state: ScrollState,

    /// Whether this bar applied the current trackpad gesture's most recent finger-driven
    /// delta. It decides who owns the momentum that follows: a bar pinned at its scroll limit
    /// leaves the delta unapplied, so ownership (and the fling) chains to an ancestor.
    #[rust]
    owns_gesture: bool,
    /// Wall-clock time of the previous trackpad scroll event, used to seed the deceleration
    /// tail's velocity (`delta / dt`) at the moment it hands off from direct OS application.
    #[rust]
    last_trackpad_time: f64,
    /// Whether content rubber-bands past the start (top/left) scroll limit.
    #[live(true)]
    bounce_at_start: bool,
    /// Whether content rubber-bands past the end (bottom/right) scroll limit.
    #[live(true)]
    bounce_at_end: bool,
    /// Current rubber-band overscroll in pixels, added to the dispatched scroll
    /// position: negative stretches before the start, positive past the end.
    #[rust]
    overscroll: f64,
    /// When a trackpad touch stopped live scroll motion. The press belonging to that
    /// same tap is consumed as the stop rather than delivered as a click.
    #[rust]
    touch_caught_motion_at: Option<f64>,
    /// The `(velocity, time)` of a fling the user just caught with a press. A quick
    /// same-direction re-flick adds this speed back (fling boost), so repeated
    /// flicks build up speed; consumed by the press's release either way.
    #[rust]
    caught_fling: Option<(f64, f64)>,
    /// This press's drag scroll owns the finger (`Cx::claim_finger_gesture`):
    /// until it does, the drag only tracks the finger, it does not scroll.
    #[rust]
    drag_claimed: bool,
    /// Where along the axis this press landed: the claim scrolls by all
    /// the travel since, so no finger travel is lost to the slop.
    #[rust]
    drag_origin: f64,
    /// When the OS momentum stream ended while still live (i.e. was cut by a touch
    /// rather than fading out at rest). The cut and its touch can be delivered in
    /// either order, so the touch handler reads this to see the motion it stopped.
    #[rust]
    momentum_cut_at: Option<f64>,
    /// Wall-clock time of the last finger-driven scroll delta, so presses during
    /// active scrolling count as stops rather than clicks. `None` until a finger has
    /// actually scrolled this bar — see [`press_settles_finger_scroll`].
    #[rust]
    last_finger_scroll_time: Option<f64>,
    /// True during the fast phase of a trackpad coast, when OS momentum deltas are applied
    /// directly and `scroll_state` stays `Stopped`. This flag is then the only sign that the
    /// view is still moving. The stream can stop reaching the view without a final event
    /// (the pointer or window routing can change mid-coast), so read it via `is_coasting`,
    /// which also requires a recent momentum event.
    #[rust]
    coasting: bool,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawScrollBar {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    is_vertical: f32,
    #[live]
    norm_handle: f32,
    #[live]
    norm_scroll: f32,
}

#[derive(Clone, PartialEq, Debug)]
pub enum ScrollBarAction {
    None,
    Scroll {
        scroll_pos: f64,
        view_total: f64,
        view_visible: f64,
    },
    ScrollDone,
}

impl ScrollBar {
    /*
    pub fn with_bar_size(self, bar_size: f32) -> Self {Self {bar_size, ..self}}
    pub fn with_smoothing(self, s: f32) -> Self {Self {smoothing: Some(s), ..self}}
    pub fn with_use_vertical_finger_scroll(self, use_vertical_finger_scroll: bool) -> Self {Self {use_vertical_finger_scroll, ..self}}
    */
    // reads back normalized scroll position info
    pub fn get_normalized_scroll_pos(&self) -> (f64, f64) {
        // computed handle size normalized
        let vy = self.view_visible / self.view_total;
        if !self.visible {
            return (0.0, 0.0);
        }
        let norm_handle = vy.max(self.min_handle_size / self.scroll_size);
        let norm_scroll = (1. - norm_handle) * ((self.scroll_pos / self.view_total) / (1. - vy));
        return (norm_scroll, norm_handle);
    }

    // sets the scroll pos from finger position
    pub fn set_scroll_pos_from_finger(&mut self, finger: f64) -> bool {
        let vy = self.view_visible / self.view_total;
        let norm_handle = vy.max(self.min_handle_size / self.scroll_size);

        let new_scroll_pos = ((self.view_total * (1. - vy) * (finger / self.scroll_size))
            / (1. - norm_handle))
            .max(0.)
            .min(self.view_total - self.view_visible);
        // lets snap new_scroll_pos
        let changed = self.scroll_pos != new_scroll_pos;
        self.scroll_pos = new_scroll_pos;
        self.scroll_target = new_scroll_pos;
        changed
    }

    // writes the norm_scroll value into the shader.. why did we do this again
    // doesnt seem to be needed. also apply eval is broken
    pub fn update_shader_scroll_pos(&mut self, _cx: &mut Cx) {
        //let (norm_scroll, _) = self.get_normalized_scroll_pos();
        //script_apply_eval!(cx, self.draw_bg, {
        //    norm_scroll:#(norm_scroll)
        //});
    }

    // turns scroll_pos into an event on this.event
    pub fn make_scroll_action(&mut self) -> ScrollBarAction {
        ScrollBarAction::Scroll {
            scroll_pos: self.scroll_pos + self.overscroll,
            view_total: self.view_total,
            view_visible: self.view_visible,
        }
    }

    /// Whether this axis has any content to scroll (beyond layout rounding error).
    /// The rubber band only engages on a scrollable axis: a view whose content fits
    /// entirely — e.g. the unused direction of a two-axis scroll view — must not
    /// stretch or bounce.
    fn scrollable(&self) -> bool {
        self.view_total - self.view_visible > 0.5
    }

    /// Reveals an auto-hiding bar and starts its hide countdown afresh.
    fn show(&mut self, cx: &mut Cx) {
        if !self.auto_hide {
            return;
        }
        if !self.animator_in_state(cx, ids!(show.on)) {
            self.animator_play(cx, ids!(show.on));
        }
        self.restart_hide_timer(cx);
    }

    /// Restarts the hide countdown, unless the pointer is holding the bar shown.
    fn restart_hide_timer(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.hide_timer);
        self.hide_timer = Timer::default();
        if self.auto_hide && !self.pointer_over && self.drag_point.is_none() {
            self.hide_timer = cx.start_timeout(self.hide_delay);
        }
    }

    pub fn move_towards_scroll_target(&mut self, cx: &mut Cx) -> bool {
        if self.smoothing.is_none() {
            return false;
        }
        if (self.scroll_target - self.scroll_pos).abs() < 0.01 {
            return false;
        }
        if self.scroll_pos > self.scroll_target {
            // go back
            self.scroll_pos =
                self.scroll_pos + (self.smoothing.unwrap() * self.scroll_delta).min(-1.);
            if self.scroll_pos <= self.scroll_target {
                // hit the target
                self.scroll_pos = self.scroll_target;
                self.update_shader_scroll_pos(cx);
                return false;
            }
        } else {
            // go forward
            self.scroll_pos =
                self.scroll_pos + (self.smoothing.unwrap() * self.scroll_delta).max(1.);
            if self.scroll_pos > self.scroll_target {
                // hit the target
                self.scroll_pos = self.scroll_target;
                self.update_shader_scroll_pos(cx);
                return false;
            }
        }
        self.update_shader_scroll_pos(cx);
        true
    }

    pub fn get_scroll_pos(&self) -> f64 {
        return self.scroll_pos;
    }

    pub fn set_scroll_pos_no_action(&mut self, cx: &mut Cx, scroll_pos: f64) -> bool {
        let scroll_pos = scroll_pos.min(self.view_total - self.view_visible).max(0.);
        if self.scroll_pos != scroll_pos {
            self.scroll_pos = scroll_pos;
            self.scroll_target = scroll_pos;
            self.update_shader_scroll_pos(cx);
            return true;
        };
        return false;
    }
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, scroll_pos: f64) -> bool {
        let scroll_pos = scroll_pos.min(self.view_total - self.view_visible).max(0.);
        if self.scroll_pos != scroll_pos {
            self.scroll_pos = scroll_pos;
            self.scroll_target = scroll_pos;
            self.update_shader_scroll_pos(cx);
            self.next_frame = cx.new_next_frame();
            return true;
        };
        return false;
    }

    pub fn set_scroll_pos_no_clip(&mut self, cx: &mut Cx, scroll_pos: f64) -> bool {
        if self.scroll_pos != scroll_pos {
            self.scroll_pos = scroll_pos;
            self.scroll_target = scroll_pos;
            self.update_shader_scroll_pos(cx);
            self.next_frame = cx.new_next_frame();
            return true;
        };
        return false;
    }

    pub fn get_scroll_target(&mut self) -> f64 {
        return self.scroll_target;
    }

    pub fn set_scroll_view_total(&mut self, _cx: &mut Cx, view_total: f64) {
        self.view_total = view_total;
    }

    pub fn get_scroll_view_total(&self) -> f64 {
        return self.view_total;
    }

    pub fn get_scroll_view_visible(&self) -> f64 {
        return self.view_visible;
    }

    pub fn set_scroll_target(&mut self, cx: &mut Cx, scroll_pos_target: f64) -> bool {
        // clamp scroll_pos to

        let new_target = scroll_pos_target
            .min(self.view_total - self.view_visible)
            .max(0.);
        if self.scroll_target != new_target {
            self.scroll_target = new_target;
            self.scroll_delta = new_target - self.scroll_pos;
            self.next_frame = cx.new_next_frame();
            return true;
        };
        return false;
    }

    pub fn scroll_into_view(&mut self, cx: &mut Cx, pos: f64, size: f64, smooth: bool) {
        if pos < self.scroll_pos {
            // scroll up
            let scroll_to = pos;
            if !smooth || self.smoothing.is_none() {
                self.set_scroll_pos(cx, scroll_to);
            } else {
                self.set_scroll_target(cx, scroll_to);
            }
        } else if pos + size > self.scroll_pos + self.view_visible {
            // scroll down
            let scroll_to = (pos + size) - self.view_visible;
            if pos + size > self.view_total {
                // resize _view_total if need be
                self.view_total = pos + size;
            }
            if !smooth || self.smoothing.is_none() {
                self.set_scroll_pos(cx, scroll_to);
            } else {
                self.set_scroll_target(cx, scroll_to);
            }
        }
    }

    pub fn handle_scroll_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scroll_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, ScrollBarAction),
    ) {
        if let Event::Scroll(e) = event {
            // The CLIPPED rect: a scroll view partially hidden by an ancestor
            // clip must not accept wheels over its invisible band — whatever
            // is really drawn there owns them (the portallist_hit example's
            // scroll-gate test pins this).
            if cx.is_scrolling_allowed_within(&scroll_area)
                && scroll_area.clipped_rect(cx).contains(e.abs)
            {
                if !match self.axis {
                    ScrollAxis::Horizontal => e.handled_x.get(),
                    ScrollAxis::Vertical => e.handled_y.get(),
                } {
                    let scroll = match self.axis {
                        ScrollAxis::Horizontal => {
                            if self.use_vertical_finger_scroll {
                                // Accept both horizontal and vertical scroll input,
                                // so trackpad horizontal scrolling and mouse wheel
                                // vertical scrolling both work.
                                e.scroll.x + e.scroll.y
                            } else {
                                e.scroll.x
                            }
                        }
                        ScrollAxis::Vertical => e.scroll.y,
                    };
                    let mark_handled = |e: &crate::event::ScrollEvent, axis: ScrollAxis| match axis
                    {
                        ScrollAxis::Horizontal => e.handled_x.set(true),
                        ScrollAxis::Vertical => e.handled_y.set(true),
                    };
                    // On macOS, trackpad scrolling is a gesture with phases: user-driven deltas
                    // (`Began`/`Changed`), fingers lifted (`Ended`), then the OS's own `Momentum`
                    // deltas. Both are applied exactly as delivered, so the feel and the
                    // deceleration are the native ones, and the OS ending its stream on a touch
                    // gives the native instant stop. Phase-less events (`None`: wheels, X11,
                    // Windows) apply their delta directly.
                    match e.phase {
                        ScrollPhase::Momentum => {
                            // Apply the momentum only if this bar owned the finger-driven
                            // gesture. A bar pinned at its scroll limit didn't own it, so it
                            // falls through and the momentum chains to the ancestor.
                            if self.owns_gesture
                                && matches!(self.scroll_state, ScrollState::Stopped)
                                && scroll != 0.0
                            {
                                let scroll_pos = self.get_scroll_pos();
                                if self.set_scroll_pos(cx, scroll_pos + scroll) {
                                    self.coasting = true;
                                    self.last_trackpad_time = e.time;
                                } else {
                                    // Pinned at a limit: drop the rest of the stream so
                                    // presses on visibly stationary content aren't treated
                                    // as catches. The remaining momentum feeds the
                                    // rubber-band bounce when that edge has it enabled.
                                    self.owns_gesture = false;
                                    self.coasting = false;
                                    let bounces = if scroll > 0.0 {
                                        self.bounce_at_end
                                    } else {
                                        self.bounce_at_start
                                    };
                                    if bounces && self.scrollable() {
                                        let dt = (e.time - self.last_trackpad_time)
                                            .max(1.0 / 240.0);
                                        let max_v = self.flick_scroll_maximum
                                            * PER_FRAME_TO_PER_SECOND;
                                        let v0 = (scroll / dt).clamp(-max_v, max_v);
                                        self.scroll_state = ScrollState::Bounce {
                                            next_frame: cx.new_next_frame(),
                                            x0: 0.0,
                                            v0,
                                            clock: FrameClock::default(),
                                            touch: false,
                                        };
                                    }
                                }
                                mark_handled(e, self.axis);
                                return dispatch_action(cx, self.make_scroll_action());
                            }
                        }
                        ScrollPhase::MomentumEnded => {
                            // A stream that ends while still coasting was cut by a touch;
                            // one that faded out at rest (or was dropped at a limit) just
                            // ends. Record the cut for the touch handler, which may be
                            // delivered after this event.
                            if self.is_coasting(e.time) {
                                self.momentum_cut_at = Some(e.time);
                            }
                            self.owns_gesture = false;
                            self.coasting = false;
                        }
                        ScrollPhase::Touched => {
                            // A finger contacted the trackpad: instantly stop any kinetic
                            // scrolling, like the native catch. Not consumed, so every bar
                            // in the chain stops; a drag in progress is left alone. If the
                            // touch stopped real motion, its own press (delivered separately
                            // at finger lift) is the second half of the catch, not a click.
                            let was_moving = self.is_motion_live(e.time)
                                || self
                                    .momentum_cut_at
                                    .is_some_and(|at| e.time - at < MOMENTUM_CUT_TOUCH_WINDOW);
                            self.owns_gesture = false;
                            self.coasting = false;
                            self.momentum_cut_at = None;
                            if matches!(self.scroll_state, ScrollState::Flick { .. }) {
                                self.scroll_state = ScrollState::Stopped;
                            }
                            if was_moving {
                                self.touch_caught_motion_at = Some(e.time);
                            }
                        }
                        ScrollPhase::None => {
                            self.owns_gesture = false;
                            self.coasting = false;
                            self.overscroll = 0.0;
                            // A wheel scroll takes over from a running bounce; the
                            // spring must stop too, or its next frame would rewrite
                            // the overscroll just cleared above.
                            if matches!(self.scroll_state, ScrollState::Bounce { .. }) {
                                self.scroll_state = ScrollState::Stopped;
                            }
                            if !self.smoothing.is_none() && e.is_mouse {
                                let scroll_pos_target = self.get_scroll_target();
                                if self.set_scroll_target(cx, scroll_pos_target + scroll) {
                                    mark_handled(e, self.axis);
                                };
                                self.move_towards_scroll_target(cx); // take the first step now
                                return dispatch_action(cx, self.make_scroll_action());
                            } else {
                                let scroll_pos = self.get_scroll_pos();
                                if self.set_scroll_pos(cx, scroll_pos + scroll) {
                                    mark_handled(e, self.axis);
                                }
                                return dispatch_action(cx, self.make_scroll_action());
                            }
                        }
                        ScrollPhase::Began | ScrollPhase::Changed => {
                            // Fingers on the pad apply directly and stop any active fling, so
                            // putting fingers back on the pad catches the scroll. `owns_gesture`
                            // tracks whether this bar actually moved on the latest finger-driven
                            // delta, which decides who owns the momentum that follows.
                            self.coasting = false;
                            self.scroll_state = ScrollState::Stopped;
                            if scroll != 0.0 {
                                self.last_finger_scroll_time = Some(e.time);
                            }
                            let mut scroll = scroll;
                            // A stretched rubber band unwinds first, symmetrically.
                            if self.overscroll != 0.0 && scroll * self.overscroll < 0.0 {
                                let reduce = scroll / RUBBER_BAND_STRETCH_STIFFNESS;
                                if reduce.abs() <= self.overscroll.abs() {
                                    self.overscroll += reduce;
                                    scroll = 0.0;
                                } else {
                                    scroll = (reduce + self.overscroll) * RUBBER_BAND_STRETCH_STIFFNESS;
                                    self.overscroll = 0.0;
                                }
                                mark_handled(e, self.axis);
                            }
                            let scroll_pos = self.get_scroll_pos();
                            self.owns_gesture = self.set_scroll_pos(cx, scroll_pos + scroll);
                            if !self.owns_gesture && scroll != 0.0 {
                                // Pinned at a limit: the delta stretches the rubber band
                                // instead, displayed as the raw overscroll divided by the
                                // stiffness.
                                let bounces = if scroll > 0.0 {
                                    self.bounce_at_end
                                } else {
                                    self.bounce_at_start
                                };
                                if bounces && self.scrollable() {
                                    self.overscroll += scroll / RUBBER_BAND_STRETCH_STIFFNESS;
                                    mark_handled(e, self.axis);
                                }
                            }
                            if self.owns_gesture {
                                mark_handled(e, self.axis);
                            }
                            return dispatch_action(cx, self.make_scroll_action());
                        }
                        ScrollPhase::Ended => {
                            // Fingers lifted. Apply the final delta (usually zero); the momentum
                            // fling starts on the first `Momentum` event, gated on `owns_gesture`
                            // set during the finger-driven phase above. A stretched rubber band
                            // springs back from here.
                            self.coasting = false;
                            self.scroll_state = ScrollState::Stopped;
                            self.last_trackpad_time = e.time;
                            if self.overscroll != 0.0 {
                                self.owns_gesture = false;
                                self.scroll_state = ScrollState::Bounce {
                                    next_frame: cx.new_next_frame(),
                                    x0: self.overscroll,
                                    v0: 0.0,
                                    clock: FrameClock::default(),
                                    touch: false,
                                };
                            }
                            let scroll_pos = self.get_scroll_pos();
                            if self.set_scroll_pos(cx, scroll_pos + scroll) || self.owns_gesture {
                                mark_handled(e, self.axis);
                            }
                            return dispatch_action(cx, self.make_scroll_action());
                        }
                    }
                }
            }
        }

        self.handle_touch_based_drag(cx, event, scroll_area, dispatch_action);
    }

    pub fn is_area_captured(&self, cx: &Cx) -> bool {
        cx.fingers.is_area_captured(self.draw_bg.area())
    }

    /// This bar's handle area. A host names it, along with its own content
    /// area, as one of "mine" when asking [`CxFingers::is_mouse_held_outside`]
    /// whether some other control holds the pointer: a press on a bar of its
    /// own is the host's own press, not an outside one.
    pub fn area(&self) -> Area {
        self.draw_bg.area()
    }

    /// Whether a momentum fling is currently animating this bar.
    pub fn is_flinging(&self) -> bool {
        matches!(self.scroll_state, ScrollState::Flick { .. })
    }

    /// Whether the fast phase of a trackpad coast is still moving the content at `time`.
    pub fn is_coasting(&self, time: f64) -> bool {
        self.coasting && time - self.last_trackpad_time < COAST_STREAM_TIMEOUT
    }

    /// Whether anything is moving this bar's content at `time`: a fling, the bounce,
    /// a live coast, or active finger-driven scrolling.
    pub fn is_motion_live(&self, time: f64) -> bool {
        matches!(
            self.scroll_state,
            ScrollState::Flick { .. } | ScrollState::Bounce { .. }
        ) || self.is_coasting(time)
            || press_settles_finger_scroll(self.last_finger_scroll_time, time)
    }

    /// Whether a press at `time` belongs to a touch that just stopped live motion.
    /// Consumes the marker, so at most one press is treated as the stop.
    pub fn take_press_catch(&mut self, time: f64) -> bool {
        let caught = self
            .touch_caught_motion_at
            .is_some_and(|at| time - at < CATCH_PRESS_WINDOW);
        self.touch_caught_motion_at = None;
        caught
    }

    /// Stop an in-progress momentum fling, the "press to catch the scroll" behavior. Returns
    /// whether a fling was actually stopped. The containing view calls this on any press in
    /// the content, independent of `drag_scrolling`, so kinetic scrolling always halts on a
    /// tap or click as it does on iOS, Android, and macOS.
    pub fn stop_fling(&mut self, time: f64) -> bool {
        if let ScrollState::Flick { fling, .. } = &self.scroll_state {
            // Remember the caught speed briefly: a quick same-direction re-flick
            // adds it back (fling boost), so repeated flicks build up speed the
            // way Chrome and native scrollers allow.
            self.caught_fling = Some((fling.velocity, time));
        }
        if self.is_flinging() || self.coasting {
            self.scroll_state = ScrollState::Stopped;
            self.coasting = false;
            // Release gesture ownership so a still-live OS momentum stream (e.g. a mouse click
            // caught a trackpad fling on a two-device setup) can't restart the fling.
            self.owns_gesture = false;
            true
        } else {
            false
        }
    }

    /// Handles touch-based drag scrolling
    /// The drag lost the finger (another scroller owns it) or was taken
    /// away: no fling, no boost; a stretch springs back from rest.
    fn stand_down(&mut self, cx: &mut Cx) {
        self.caught_fling = None;
        self.drag_claimed = false;
        self.scroll_state = if self.overscroll != 0.0 {
            ScrollState::Bounce {
                next_frame: cx.new_next_frame(),
                x0: self.overscroll,
                v0: 0.0,
                clock: FrameClock::default(),
                touch: true,
            }
        } else {
            ScrollState::Stopped
        };
    }

    fn handle_touch_based_drag(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scroll_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, ScrollBarAction),
    ) {
        if !self.drag_scrolling {
            return;
        }

        // Don't start or continue a touch-based drag scroll if scrolling is blocked.
        // These force-stops must also let go of any stretched rubber band: leaving
        // `overscroll` set with the spring stopped would freeze the stretch on
        // screen with nothing left to pull it back.
        if !cx.is_scrolling_allowed_within(&scroll_area) {
            self.scroll_state = ScrollState::Stopped;
            if self.overscroll != 0.0 {
                self.overscroll = 0.0;
                dispatch_action(cx, self.make_scroll_action());
            }
            return;
        }

        // Check if scroll bar handle is not captured
        if self.is_area_captured(cx) {
            self.scroll_state = ScrollState::Stopped;
            if self.overscroll != 0.0 {
                self.overscroll = 0.0;
                dispatch_action(cx, self.make_scroll_action());
            }
            return;
        }

        // THE POINTER-CAPTURE RULE. Dragging the content is a gesture that
        // starts from a press on whatever happens to be under the pointer, so
        // it stands down the whole time the MOUSE is held by anything that is
        // not this box — at the press, and again on every move while the drag
        // is still running.
        //
        // `hits` below already refuses a press another widget marked handled,
        // but that flag lives on the event and is not the whole answer. A
        // widget that ALREADY holds the mouse is handed its `FingerDown`
        // straight off the capture list without marking the event at all
        // (`hits`' "if we already captured it just return it immediately"
        // path), so every press delivered while a control is being dragged
        // arrives here looking like bare background. A `capture_overload`
        // co-capture, a capture promoted away from a child, and a capture
        // taken after this drag began are invisible to the flag as well. So
        // ask the capture list itself.
        //
        // Only the mouse locks. `is_mouse_held_outside` ignores touch
        // captures, so a TOUCH drag that begins on a control may still scroll
        // the box under it; that asymmetry is the rule, not an oversight.
        //
        // A fling or a bounce is deliberately NOT stopped here: it is not this
        // pointer's gesture, and a press on some unrelated control must not
        // freeze a spring in mid-flight.
        // ...and that asymmetry has to be enforced here, not merely stated:
        // this guard sits in front of `hits`, so applied to every event it
        // would drop TOUCH moves and ups as well and kill finger scrolling
        // whenever a mouse happened to be held anywhere. A touch event is let
        // through to the gesture below; only a mouse-driven one stands down.
        // A cancellation is terminal cleanup of this box's own press: it is
        // never an unrelated mouse gesture, so no held mouse stops it.
        let touch_event = matches!(event, Event::TouchUpdate(_) | Event::FingerCancel(_));
        if !touch_event && cx.fingers.is_mouse_held_outside(&[scroll_area]) {
            if matches!(self.scroll_state, ScrollState::Drag { .. }) {
                self.scroll_state = ScrollState::Stopped;
                if self.overscroll != 0.0 {
                    self.overscroll = 0.0;
                    dispatch_action(cx, self.make_scroll_action());
                }
            }
            return;
        }

        match event.hits(cx, scroll_area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                let abs = match self.axis {
                    ScrollAxis::Horizontal => fe.abs.x,
                    ScrollAxis::Vertical => fe.abs.y,
                };
                self.scroll_state = ScrollState::Drag {
                    samples: vec![ScrollSample { abs, time: fe.time }],
                };
                self.drag_claimed = false;
                self.drag_origin = abs;
            }
            Hit::FingerMove(e) => match &mut self.scroll_state {
                ScrollState::Drag { samples } => {
                    let new_abs = match self.axis {
                        ScrollAxis::Horizontal => e.abs.x,
                        ScrollAxis::Vertical => e.abs.y,
                    };
                    // The drag scrolls only once it owns the finger: it
                    // claims when the finger travels along this axis past the
                    // tap slop and at least as far as across it (a sideways
                    // carousel inside a vertical list leaves the list's drag
                    // alone). Until then it only tracks the finger. A finger
                    // another scroller already owns is not this view's — it
                    // stands down, settling from rest.
                    let mut from = samples.last().unwrap().abs;
                    if !self.drag_claimed {
                        let travel = e.abs - e.abs_start;
                        let (along, across) = match self.axis {
                            ScrollAxis::Horizontal => (travel.x.abs(), travel.y.abs()),
                            ScrollAxis::Vertical => (travel.y.abs(), travel.x.abs()),
                        };
                        // Touch only: a mouse drag scrolls from its first move,
                        // as it always has.
                        let slop = touch_drag_slop(TAP_COUNT_DISTANCE, e.device.is_touch());
                        if e.device.is_touch() && (along < slop || along < across) {
                            push_sample(samples, new_abs, e.time);
                            return;
                        }
                        if !cx.claim_finger_gesture(e.digit_id, scroll_area) {
                            self.stand_down(cx);
                            return;
                        }
                        self.drag_claimed = true;
                        from = drag_origin(self.drag_origin, new_abs, slop, e.device.is_touch());
                    }
                    push_sample(samples, new_abs, e.time);

                    let mut delta = new_abs - from;
                    let extent = self.view_visible;
                    let mut changed = false;

                    // A stretched rubber band unwinds along the same curve it
                    // stretched by, so the content tracks the finger exactly;
                    // only travel beyond the unwind scrolls the content again.
                    if self.overscroll != 0.0 && delta != 0.0 {
                        let raw = stretch_raw(self.overscroll, extent, true) - delta;
                        if raw == 0.0 || raw.signum() == self.overscroll.signum() {
                            self.overscroll = stretch_displayed(raw, extent, true);
                            delta = 0.0;
                        } else {
                            self.overscroll = 0.0;
                            delta = -raw;
                        }
                        changed = true;
                    }

                    if delta != 0.0 {
                        let scroll_pos = self.get_scroll_pos();
                        let max_scroll = (self.view_total - self.view_visible).max(0.0);
                        let target = scroll_pos - delta;
                        let clamped = target.clamp(0.0, max_scroll);
                        if self.set_scroll_pos(cx, clamped) {
                            changed = true;
                        }
                        // Finger travel past a scroll limit stretches the rubber
                        // band there (the loose iOS curve; see `scroll_motion`),
                        // when that edge has the bounce enabled.
                        let leftover = target - clamped;
                        let bounces = if leftover > 0.0 {
                            self.bounce_at_end
                        } else {
                            self.bounce_at_start
                        };
                        if leftover != 0.0 && bounces && self.scrollable() {
                            self.overscroll = stretch_displayed(leftover, extent, true);
                            changed = true;
                        }
                    }

                    if changed {
                        dispatch_action(cx, self.make_scroll_action());
                    }
                }
                _ => (),
            },
            Hit::FingerUp(fe) if fe.is_primary_hit() => match &mut self.scroll_state {
                ScrollState::Drag { samples } => {
                    // The press's release settles the fate of any fling it caught:
                    // only a qualifying same-direction flick below adds it back.
                    let caught_fling = self.caught_fling.take();
                    // Estimate the release velocity (pixels/second) like a native
                    // VelocityTracker (see `scroll_motion`), then start the same momentum
                    // fling as PortalList — same model, same parameters — so drag flicks
                    // decelerate identically in every scrollable view. A press taken
                    // away (`cancelled`) releases at rest: no fling, no boost, and a
                    // stretch springs back from rest.
                    // A drag that never owned the finger never scrolled: it
                    // releases at rest too.
                    let at_rest = fe.cancelled || !self.drag_claimed;
                    self.drag_claimed = false;
                    let (release_velocity, total_delta) = if at_rest {
                        (0.0, 0.0)
                    } else {
                        estimate_release_velocity(samples)
                    };
                    let caught_fling = caught_fling.filter(|_| !at_rest);
                    let (min_velocity, max_velocity) = touch_fling_limits(
                        self.flick_scroll_minimum * PER_FRAME_TO_PER_SECOND,
                        self.flick_scroll_maximum * PER_FRAME_TO_PER_SECOND,
                        fe.device.is_touch(),
                    );
                    let release_velocity = release_velocity.clamp(-max_velocity, max_velocity);
                    if self.overscroll != 0.0 {
                        // Lifted while stretched past an edge: spring back, with the
                        // lift velocity carried into the bounce (positive further
                        // into the overscroll, matching the overscroll's sign).
                        self.scroll_state = ScrollState::Bounce {
                            next_frame: cx.new_next_frame(),
                            x0: self.overscroll,
                            v0: -release_velocity,
                            clock: FrameClock::default(),
                            touch: true,
                        };
                    } else if total_delta.abs() > FLING_MIN_TOTAL_DELTA
                        && release_velocity.abs() > min_velocity
                    {
                        // Fling boost: a quick same-direction re-flick adds the
                        // speed of the fling this press caught.
                        let release_velocity = match caught_fling {
                            Some((caught_velocity, caught_at))
                                if caught_velocity * release_velocity > 0.0
                                    && fe.time - caught_at < FLING_BOOST_MAX_DWELL =>
                            {
                                (release_velocity + caught_velocity)
                                    .clamp(-max_velocity, max_velocity)
                            }
                            _ => release_velocity,
                        };
                        self.scroll_state = ScrollState::Flick {
                            fling: Fling::new(release_velocity, self.fling_decel),
                            next_frame: cx.new_next_frame(),
                        };
                    } else {
                        self.scroll_state = ScrollState::Stopped;
                    }
                }
                _ => (),
            },
            _ => (),
        }
    }

    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, ScrollBarAction),
    ) {
        self.handle_flick(cx, event, dispatch_action);
        self.handle_bounce(cx, event, dispatch_action);

        if self.hide_timer.is_event(event).is_some() {
            self.hide_timer = Timer::default();
            if !self.pointer_over && self.drag_point.is_none() {
                self.animator_play(cx, ids!(show.off));
            }
        }
        if let Event::ClearHover = event {
            if self.pointer_over {
                self.pointer_over = false;
                self.restart_hide_timer(cx);
            }
        }
        self.animator_handle_event(cx, event);

        if self.visible {
            if self.next_frame.is_event(event).is_some() {
                if self.move_towards_scroll_target(cx) {
                    self.next_frame = cx.new_next_frame();
                }
                return dispatch_action(cx, self.make_scroll_action());
            }

            if !self.show_handle {
                return;
            }
            // A faded bar still owns its strip: a press there shows it and
            // scrolls or drags as always. The layouts in this tree leave the
            // strip to their bar (ItemGrid's scroll_bar_band, View's
            // pointer capture), so a bar that let the press through here
            // would leave a strip along the edge that answers nothing.

            match event.hits(cx, self.draw_bg.area()) {
                Hit::FingerDown(fe) if fe.is_primary_hit() => {
                    self.animator_play(cx, ids!(hover.drag));
                    let rel = fe.abs - fe.rect.pos;
                    let rel = match self.axis {
                        ScrollAxis::Horizontal => rel.x,
                        ScrollAxis::Vertical => rel.y,
                    };
                    let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();
                    let bar_start = norm_scroll * self.scroll_size;
                    let bar_size = norm_handle * self.scroll_size;
                    if rel < bar_start || rel > bar_start + bar_size {
                        // clicked outside
                        self.drag_point = Some(bar_size * 0.5);
                        if self.set_scroll_pos_from_finger(rel - self.drag_point.unwrap()) {
                            dispatch_action(cx, self.make_scroll_action());
                        }
                    } else {
                        // clicked on
                        self.drag_point = Some(rel - bar_start); // store the drag delta
                    }
                    self.show(cx);
                }
                Hit::FingerHoverIn(_) => {
                    self.pointer_over = true;
                    self.show(cx);
                    self.animator_play(cx, ids!(hover.on));
                }
                Hit::FingerHoverOut(_) => {
                    self.pointer_over = false;
                    self.animator_play(cx, ids!(hover.off));
                    self.restart_hide_timer(cx);
                }
                Hit::FingerUp(fe) if fe.is_primary_hit() => {
                    self.drag_point = None;
                    // Touch has no hover, so the bar can hide once a touch ends.
                    self.pointer_over = fe.is_over && fe.device.has_hovers();
                    if self.pointer_over {
                        self.animator_play(cx, ids!(hover.on));
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }
                    self.restart_hide_timer(cx);
                    return;
                }
                Hit::FingerMove(fe) => {
                    let rel = fe.abs - fe.rect.pos;
                    // helper called by event code to scroll from a finger
                    if self.drag_point.is_none() {
                        // state should never occur.
                        //println!("Invalid state in scrollbar, fingerMove whilst drag_point is none")
                    } else {
                        match self.axis {
                            ScrollAxis::Horizontal => {
                                if self.set_scroll_pos_from_finger(rel.x - self.drag_point.unwrap())
                                {
                                    dispatch_action(cx, self.make_scroll_action());
                                }
                            }
                            ScrollAxis::Vertical => {
                                if self.set_scroll_pos_from_finger(rel.y - self.drag_point.unwrap())
                                {
                                    dispatch_action(cx, self.make_scroll_action());
                                }
                            }
                        }
                    }
                }
                _ => (),
            };
        }
    }

    /// Animates the rubber-band bounce back to the scroll limit (see `scroll_motion`;
    /// the curve depends on the input that caused the bounce), driving `overscroll`
    /// and dispatching scroll updates.
    fn handle_bounce(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, ScrollBarAction),
    ) {
        let step = if let ScrollState::Bounce { next_frame, x0, v0, clock, touch } =
            &mut self.scroll_state
        {
            if let Some(ne) = next_frame.is_event(event) {
                // The bounce may never travel farther than the drag stretch can
                // reach, however fast the fling that hit the edge was: the seed
                // velocity is softened against the headroom (huge flicks compress,
                // gentle ones keep their feel), and the clamp below is a backstop.
                let max_overscroll = self.view_visible * RUBBER_BAND_TOUCH_RANGE;
                if clock.not_started() {
                    *v0 = soften_bounce_velocity(*v0, *x0, max_overscroll, *touch);
                }
                let t = clock.advance(ne.time);
                let (x, past_peak) = rubber_band_bounce(*x0, *v0, t, *touch);
                let x = x.clamp(-max_overscroll, max_overscroll);
                // A lift velocity opposing the stretch can swing the spring through
                // the edge; the content is back at rest there, so settle.
                let crossed = x * *x0 < 0.0;
                let settled = crossed
                    || (x.abs() <= 0.5 && (past_peak || (v0.abs() <= 1.0 && x0.abs() <= 0.5)));
                if !settled {
                    *next_frame = cx.new_next_frame();
                }
                Some((x, settled))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((x, settled)) = step {
            if settled {
                self.overscroll = 0.0;
                self.scroll_state = ScrollState::Stopped;
            } else {
                self.overscroll = x;
            }
            dispatch_action(cx, self.make_scroll_action());
        }
    }

    fn handle_flick(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, ScrollBarAction),
    ) {
        // The scroll animation lives in `scroll_motion::Fling`, shared with PortalList. A
        // both the touch-drag flick and the trackpad deceleration tail self-decay.
        let min_velocity = self.flick_scroll_minimum * PER_FRAME_TO_PER_SECOND;
        let step = if let ScrollState::Flick { fling, next_frame } = &mut self.scroll_state {
            if let Some(ne) = next_frame.is_event(event) {
                Some((fling.step(ne.time), fling.is_active(min_velocity), fling.velocity))
            } else {
                None
            }
        } else {
            None
        };

        match step {
            None => {}
            Some((None, ..)) => {
                // First fling frame: time baseline established, no movement yet.
                if let ScrollState::Flick { next_frame, .. } = &mut self.scroll_state {
                    *next_frame = cx.new_next_frame();
                }
            }
            Some((Some(displacement), active, velocity)) => {
                if active {
                    let scroll_pos = self.get_scroll_pos();
                    if self.set_scroll_pos(cx, scroll_pos - displacement) {
                        dispatch_action(cx, self.make_scroll_action());
                        if let ScrollState::Flick { next_frame, .. } = &mut self.scroll_state {
                            *next_frame = cx.new_next_frame();
                        }
                    } else if displacement != 0.0 {
                        // Reached a scroll limit: hand the remaining momentum to the
                        // rubber-band bounce when that edge has it enabled. Otherwise
                        // stop the fling now — letting it run down while pinned would
                        // keep eating presses as catch attempts on visibly stationary
                        // content. The overscroll rate is the scroll-position rate:
                        // positive past the end, negative before the start.
                        let v0 = -velocity;
                        let bounces = if v0 > 0.0 {
                            self.bounce_at_end
                        } else {
                            self.bounce_at_start
                        };
                        if bounces && v0 != 0.0 && self.scrollable() {
                            self.scroll_state = ScrollState::Bounce {
                                next_frame: cx.new_next_frame(),
                                x0: self.overscroll,
                                v0,
                                clock: FrameClock::default(),
                                touch: true,
                            };
                        } else {
                            self.scroll_state = ScrollState::Stopped;
                        }
                        self.owns_gesture = false;
                    } else if let ScrollState::Flick { next_frame, .. } = &mut self.scroll_state {
                        *next_frame = cx.new_next_frame();
                    }
                } else {
                    self.scroll_state = ScrollState::Stopped;
                    self.owns_gesture = false;
                }
            }
        }
    }

    pub fn draw_scroll_bar(
        &mut self,
        cx: &mut Cx2d,
        axis: ScrollAxis,
        view_rect: Rect,
        view_total: Vec2d,
    ) -> f64 {
        let track = Rect { pos: Vec2d::default(), size: view_rect.size };
        self.draw_scroll_bar_along(cx, axis, track, view_rect.size, view_total)
    }

    /// Like [`Self::draw_scroll_bar`], but runs the bar along the far edge of `track`
    /// (relative to the turtle's origin) instead of the edge of the visible area.
    pub fn draw_scroll_bar_along(
        &mut self,
        cx: &mut Cx2d,
        axis: ScrollAxis,
        track: Rect,
        view_visible: Vec2d,
        view_total: Vec2d,
    ) -> f64 {
        self.axis = axis;

        let vertical = matches!(axis, ScrollAxis::Vertical);
        let (total, viewport, track_len, cross_overflows) = if vertical {
            (view_total.y, view_visible.y, track.size.y, view_total.x > view_visible.x + 0.1)
        } else {
            (view_total.x, view_visible.x, track.size.x, view_total.y > view_visible.y + 0.1)
        };
        self.visible = total > viewport + 0.1;
        // The other axis' bar, when it shows, takes the corner.
        self.scroll_size =
            if cross_overflows { track_len - self.bar_size } else { track_len } - self.bar_side_margin * 2.;
        self.view_total = total;
        self.view_visible = viewport;
        self.scroll_pos = self.scroll_pos.min(total - viewport).max(0.);

        if self.visible && self.show_handle {
            // Drop a stale hover, since we miss the hover-out if the pointer left while the bar wasn't drawn.
            if self.pointer_over && !cx.fingers.is_area_hovered(self.draw_bg.area()) {
                self.pointer_over = false;
                self.restart_hide_timer(cx);
            }
            match self.last_shown {
                Some((pos, vis)) if (pos - self.scroll_pos).abs() > 0.5 || (vis - viewport).abs() > 0.5 => {
                    self.last_shown = Some((self.scroll_pos, viewport));
                    self.show(cx);
                }
                Some(_) => {}
                None => self.last_shown = Some((self.scroll_pos, viewport)),
            }
            if !self.auto_hide && !self.animator_in_state(cx, ids!(show.on)) {
                self.animator_cut(cx, ids!(show.on));
            }

            let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();
            self.draw_bg.is_vertical = if vertical { 1.0 } else { 0.0 };
            self.draw_bg.norm_scroll = norm_scroll as f32;
            self.draw_bg.norm_handle = norm_handle as f32;
            let scroll = cx.turtle().scroll();
            let rect = if vertical {
                Rect {
                    pos: track.pos + dvec2(track.size.x - self.bar_size, self.bar_side_margin) + scroll,
                    size: dvec2(self.bar_size, self.scroll_size),
                }
            } else {
                Rect {
                    pos: track.pos + dvec2(self.bar_side_margin, track.size.y - self.bar_size) + scroll,
                    size: dvec2(self.scroll_size, self.bar_size),
                }
            };
            self.draw_bg.draw_rel(cx, rect);
        }

        // see if we need to clamp
        let clamped_pos = self
            .scroll_pos
            .min(self.view_total - self.view_visible)
            .max(0.);
        if clamped_pos != self.scroll_pos {
            self.scroll_pos = clamped_pos;
            self.scroll_target = clamped_pos;
            // ok so this means we 'scrolled' this can give a problem for virtual viewport widgets
            self.next_frame = cx.new_next_frame();
        }

        self.scroll_pos
    }
}

/// The pointer-capture rule, as it applies to a box that drag-scrolls its
/// content: while another control holds the MOUSE, the box's own gestures
/// stand down; a TOUCH held elsewhere changes nothing.
#[cfg(test)]
mod pointer_capture_tests {
    use super::*;
    use crate::button::ButtonAction;
    use crate::event::{ScrollEvent, TouchPoint, TouchState, TouchUpdateEvent};
    use crate::makepad_draw::cx_draw::CxDraw;
    use crate::view::View;
    use crate::widget::*;
    use std::cell::Cell;

    const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };
    const WINDOW: WindowId = WindowId(1, 1);

    /// A 300x200 box holding 1000 points of content, so it really scrolls,
    /// with a button under it to hold the pointer with.
    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    page := ScrollYView{
                        width: 300.
                        height: 200.
                        flow: Down
                        inner := Button{width: 100. height: 40. text: "inner"}
                        tall := View{width: Fill height: 1000.}
                    }
                    grab := Button{width: 100. height: 40. text: "grab"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            Target {
                pass: DrawPass::new(cx),
                draw_list: DrawList2d::new(cx),
            }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn press(abs: Vec2d) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn mouse_move(abs: Vec2d) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: Vec2d::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time: 0.02,
            handled: Cell::new(Area::Empty),
        })
    }

    fn release(abs: Vec2d) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time: 0.04,
        })
    }

    fn touch_press(abs: Vec2d) -> Event {
        Event::TouchUpdate(TouchUpdateEvent {
            time: 0.0,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            touches: vec![TouchPoint {
                state: TouchState::Start,
                abs,
                time: 0.0,
                uid: 7,
                rotation_angle: 0.0,
                force: 1.0,
                radius: dvec2(1.0, 1.0),
                handled: Cell::new(Area::Empty),
                sweep_lock: Cell::new(Area::Empty),
            }],
        })
    }

    /// One finger-driven trackpad delta over `abs`. It leaves the box counting
    /// as moving for the next `FINGER_SCROLL_SETTLE_WINDOW`, which is what arms
    /// the press-to-catch below without an animation frame in sight.
    fn trackpad_scroll(abs: Vec2d, dy: f64) -> Event {
        Event::Scroll(ScrollEvent {
            window_id: WINDOW,
            scroll: dvec2(0.0, dy),
            abs,
            modifiers: KeyModifiers::default(),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: false,
            time: 0.0,
            phase: ScrollPhase::Changed,
        })
    }

    fn send(cx: &mut Cx, root: &WidgetRef, event: Event) {
        cx.capture_actions(|cx| root.handle_event(cx, &event, &mut Scope::empty()));
    }

    /// Whether `button` heard a press out of this event.
    fn pressed(cx: &mut Cx, root: &WidgetRef, event: Event, button: &WidgetRef) -> bool {
        let actions = cx.capture_actions(|cx| root.handle_event(cx, &event, &mut Scope::empty()));
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .any(|action| {
                action.widget_uid == button.widget_uid()
                    && matches!(action.cast::<ButtonAction>(), ButtonAction::Pressed(_))
            })
    }

    /// Press, move, release - the whole drag through the widget tree.
    fn drag(cx: &mut Cx, root: &WidgetRef, from: Vec2d, to: Vec2d) {
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(cx, root, press(from));
        send(cx, root, mouse_move(to));
        send(cx, root, release(to));
        cx.fingers.first_mouse_button = None;
    }

    /// Press something and DON'T let go: the pointer is now locked to it, the
    /// way it is locked to a slider's thumb being dragged.
    fn hold(cx: &mut Cx, root: &WidgetRef, at: Vec2d) {
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(cx, root, press(at));
    }

    fn middle(cx: &Cx, widget: &WidgetRef) -> Vec2d {
        let rect = widget.area().rect(cx);
        assert!(rect.size.x > 0.0 && rect.size.y > 0.0, "not drawn");
        rect.pos + rect.size * 0.5
    }

    fn scroll_y(widget: &WidgetRef) -> f64 {
        widget
            .borrow::<View>()
            .unwrap()
            .scroll_extent()
            .unwrap()
            .pos
            .y
    }

    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef, WidgetRef) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = scene(cx);
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let page = root.widget(cx, ids!(page));
        let grab = root.widget(cx, ids!(grab));
        assert_eq!(scroll_y(&page), 0.0, "starts at the top");
        (root, page, grab)
    }

    /// The control: with nothing else holding the pointer, a drag upward over
    /// the content scrolls the box. Without this the two tests below would
    /// pass on a box that never scrolled at all.
    #[test]
    fn a_drag_on_bare_content_scrolls_the_box() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, page, _grab) = start(&mut cx);
        drag(&mut cx, &root, dvec2(50.0, 150.0), dvec2(50.0, 90.0));
        assert_eq!(
            scroll_y(&page),
            60.0,
            "60 points of finger travel moved the content 60 points"
        );
    }

    /// The operator's bug: a press the pointer is already locked to must not
    /// also drag-scroll a box around it.
    ///
    /// A widget that already holds the mouse is handed its `FingerDown`
    /// straight off the capture list, and that path marks nothing on the
    /// event - so the press reaches the box looking exactly like a press on
    /// bare background. Asking the capture list is the only way to tell.
    #[test]
    fn a_press_another_control_holds_never_drag_scrolls() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, page, grab) = start(&mut cx);
        let on_grab = middle(&cx, &grab);
        hold(&mut cx, &root, on_grab);
        assert!(
            cx.fingers.is_mouse_held_outside(&[page.area()]),
            "the button holds the pointer"
        );
        // The same drag as above, with the pointer locked to the button.
        send(&mut cx, &root, press(dvec2(50.0, 150.0)));
        send(&mut cx, &root, mouse_move(dvec2(50.0, 90.0)));
        assert_eq!(
            scroll_y(&page),
            0.0,
            "the box stayed put while another control held the pointer"
        );
        // The same press and move on an unheld pointer scrolls it 60 points -
        // that is `a_drag_on_bare_content_scrolls_the_box` above. It cannot be
        // asserted here as well: there is no event loop in this harness to end
        // the button's capture on the release, so the pointer stays locked for
        // the rest of this `Cx`.
        cx.fingers.first_mouse_button = None;
    }

    /// Only the mouse locks. A finger held on a control does not stop a drag
    /// of the content - that half of the rule is deliberate, and asking
    /// "is anything captured" instead of "is the MOUSE held" would break it.
    #[test]
    fn a_touch_held_on_a_control_does_not_stand_the_drag_down() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, page, grab) = start(&mut cx);
        let on_grab = middle(&cx, &grab);
        send(&mut cx, &root, touch_press(on_grab));
        assert!(
            cx.fingers.any_areas_captured(),
            "the finger is on the button"
        );
        assert!(
            !cx.fingers.is_mouse_held_outside(&[page.area()]),
            "but the mouse is not held by anything"
        );
        drag(&mut cx, &root, dvec2(50.0, 150.0), dvec2(50.0, 90.0));
        assert_eq!(scroll_y(&page), 60.0, "so the drag still scrolls");
    }

    /// `catch_fling_on_press` is the box's other press-driven gesture: a press
    /// on moving content is spent stopping it, and is not passed on. That is
    /// right for a reader reaching for the brake, and wrong for a pointer that
    /// is already locked to a control - that press was never the box's to
    /// spend. Whether the widget under the pointer should hear a press while
    /// the pointer is held elsewhere is `hits`' half of the rule; what is
    /// asserted here is only that the scroll box stopped taking it.
    #[test]
    fn a_press_on_a_locked_pointer_is_not_eaten_as_a_fling_catch() {
        let on_content = dvec2(50.0, 150.0);

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _page, _grab) = start(&mut cx);
        let inner = root.widget(&cx, ids!(inner));
        let on_inner = middle(&cx, &inner);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        send(&mut cx, &root, trackpad_scroll(on_content, 10.0));
        assert!(
            !pressed(&mut cx, &root, press(on_inner), &inner),
            "the press that stops moving content is spent on stopping it"
        );

        // The same press, with the pointer already locked to the button
        // outside the box.
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, page, grab) = start(&mut cx);
        let inner = root.widget(&cx, ids!(inner));
        let on_inner = middle(&cx, &inner);
        let on_grab = middle(&cx, &grab);
        hold(&mut cx, &root, on_grab);
        assert!(cx.fingers.is_mouse_held_outside(&[page.area()]));
        send(&mut cx, &root, trackpad_scroll(on_content, 10.0));
        assert!(
            pressed(&mut cx, &root, press(on_inner), &inner),
            "the box did not consume a press belonging to a locked pointer"
        );
        cx.fingers.first_mouse_button = None;
    }
}
