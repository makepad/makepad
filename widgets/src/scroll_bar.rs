use crate::animator::*;
use crate::event::ScrollPhase;
use crate::makepad_derive_widget::*;
use crate::makepad_draw::*;
use crate::scroll_motion::{
    estimate_release_velocity, push_sample, raw_os_momentum, Fling, ScrollSample,
    FLING_MIN_TOTAL_DELTA, FLING_MOMENTUM_SMOOTH_BELOW, PER_FRAME_TO_PER_SECOND,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    //use mod.animator.*
    set_type_default() do #(DrawScrollBar::script_shader(vm)){
        ..mod.draw.DrawQuad // splat in draw quad
    }

    mod.widgets.ScrollBarBase = #(ScrollBar::script_component(vm))
    mod.widgets.ScrollBar = set_type_default() do mod.widgets.ScrollBarBase{
        bar_size: 10.0
        bar_side_margin: 3.0
        min_handle_size: 30.0
        draw_bg +: {
            drag: instance(0.0)
            hover: instance(0.0)

            size: uniform(6.0)
            border_size: uniform(theme.beveling)
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
                        1.
                        self.rect_size.y * self.norm_scroll
                        self.size
                        self.rect_size.y * self.norm_handle
                        self.border_radius
                    )
                }
                else {
                    sdf.box(
                        self.rect_size.x * self.norm_scroll
                        1.
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
                return sdf.result
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
        }
    }

    mod.widgets.ScrollBarTabs = mod.widgets.ScrollBar {
        draw_bg +: {
            drag: instance(0.0)
            hover: instance(0.0)

            size: uniform(6.0)
            border_size: uniform(1.0)
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
                        1.,
                        self.rect_size.y * self.norm_scroll
                        self.size
                        self.rect_size.y * self.norm_handle
                        self.border_radius
                    )
                }
                else {
                    sdf.box(
                        self.rect_size.x * self.norm_scroll
                        1.
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

                return sdf.result
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
    Flick {
        /// The momentum-fling animation state (native iOS-style exponential decay,
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
    /// Deprecated: unused. The deceleration rate is the shared
    /// [`crate::scroll_motion::FLING_DECEL_RATE_PER_MS`] exponential model;
    /// kept only so existing DSL doesn't break.
    #[live(0.97)]
    flick_scroll_decay: f64,
    /// Whether to enable drag scrolling
    #[live(false)]
    drag_scrolling: bool,

    #[apply_default]
    animator: Animator,

    #[rust]
    next_frame: NextFrame,
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
            scroll_pos: self.scroll_pos,
            view_total: self.view_total,
            view_visible: self.view_visible,
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
            if cx.is_scrolling_allowed_within(&scroll_area) && scroll_area.rect(cx).contains(e.abs)
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
                    // deltas. We apply the user-driven and fast momentum deltas directly, then
                    // once the momentum slows past a threshold hand off to a gentler self-decaying
                    // tail (same model as PortalList; see `scroll_motion`). Phase-less events
                    // (`None`: wheels, X11, Windows) apply their delta directly.
                    match e.phase {
                        ScrollPhase::Momentum if raw_os_momentum() => {
                            // Diagnostic: apply the OS momentum delta directly, no smoothed tail.
                            // Claim it only if this bar applied it, so a pinned bar still chains.
                            let scroll_pos = self.get_scroll_pos();
                            if self.set_scroll_pos(cx, scroll_pos + scroll) {
                                mark_handled(e, self.axis);
                                return dispatch_action(cx, self.make_scroll_action());
                            }
                        }
                        ScrollPhase::Momentum => {
                            // Apply/hand off the momentum only if this bar owned the finger-driven
                            // gesture. A bar pinned at its scroll limit didn't own it, so it falls
                            // through and the momentum chains to the ancestor. macOS ends its
                            // momentum stream when the pad
                            // is touched, so following it also gives a native stop on the first
                            // touch.
                            if self.owns_gesture {
                                match &mut self.scroll_state {
                                    // The tail fling already owns the deceleration; it self-decays,
                                    // so ignore the OS momentum stream from here on.
                                    ScrollState::Flick { .. } => {}
                                    ScrollState::Stopped
                                        if scroll.abs() < FLING_MOMENTUM_SMOOTH_BELOW =>
                                    {
                                        // Deceleration tail: apply this delta directly (no dead
                                        // frame at the seam), then hand off to a self-decaying
                                        // fling seeded at the current speed. Clamp the seed against
                                        // a degenerate dt between coalesced events.
                                        let scroll_pos = self.get_scroll_pos();
                                        self.set_scroll_pos(cx, scroll_pos + scroll);
                                        let dt =
                                            (e.time - self.last_trackpad_time).max(1.0 / 240.0);
                                        let max_v =
                                            self.flick_scroll_maximum * PER_FRAME_TO_PER_SECOND;
                                        let velocity = (-scroll / dt).clamp(-max_v, max_v);
                                        self.scroll_state = ScrollState::Flick {
                                            fling: Fling::new_trackpad_tail(velocity),
                                            next_frame: cx.new_next_frame(),
                                        };
                                    }
                                    ScrollState::Stopped => {
                                        // Fast phase: apply the OS delta directly.
                                        self.last_trackpad_time = e.time;
                                        let scroll_pos = self.get_scroll_pos();
                                        self.set_scroll_pos(cx, scroll_pos + scroll);
                                    }
                                    _ => {}
                                }
                                mark_handled(e, self.axis);
                                return dispatch_action(cx, self.make_scroll_action());
                            }
                        }
                        // The stream finished (decayed or the user touched the pad). The tail
                        // fling self-decays to a stop; claim the event if it's ours.
                        ScrollPhase::MomentumEnded => {
                            self.owns_gesture = false;
                            if matches!(self.scroll_state, ScrollState::Flick { .. }) {
                                mark_handled(e, self.axis);
                                return;
                            }
                        }
                        ScrollPhase::None => {
                            self.owns_gesture = false;
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
                            self.scroll_state = ScrollState::Stopped;
                            let scroll_pos = self.get_scroll_pos();
                            self.owns_gesture = self.set_scroll_pos(cx, scroll_pos + scroll);
                            if self.owns_gesture {
                                mark_handled(e, self.axis);
                            }
                            return dispatch_action(cx, self.make_scroll_action());
                        }
                        ScrollPhase::Ended => {
                            // Fingers lifted. Apply the final delta (usually zero); the momentum
                            // fling starts on the first `Momentum` event, gated on `owns_gesture`
                            // set during the finger-driven phase above.
                            self.scroll_state = ScrollState::Stopped;
                            self.last_trackpad_time = e.time;
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

    /// Whether a momentum fling is currently animating this bar.
    pub fn is_flinging(&self) -> bool {
        matches!(self.scroll_state, ScrollState::Flick { .. })
    }

    /// Stop an in-progress momentum fling, the "press to catch the scroll" behavior. Returns
    /// whether a fling was actually stopped. The containing view calls this on any press in
    /// the content, independent of `drag_scrolling`, so kinetic scrolling always halts on a
    /// tap or click as it does on iOS, Android, and macOS.
    pub fn stop_fling(&mut self) -> bool {
        if self.is_flinging() {
            self.scroll_state = ScrollState::Stopped;
            // Release gesture ownership so a still-live OS momentum stream (e.g. a mouse click
            // caught a trackpad fling on a two-device setup) can't restart the fling.
            self.owns_gesture = false;
            true
        } else {
            false
        }
    }

    /// Handles touch-based drag scrolling
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
        if !cx.is_scrolling_allowed_within(&scroll_area) {
            self.scroll_state = ScrollState::Stopped;
            return;
        }

        // Check if scroll bar handle is not captured
        if self.is_area_captured(cx) {
            self.scroll_state = ScrollState::Stopped;
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
            }
            Hit::FingerMove(e) => match &mut self.scroll_state {
                ScrollState::Drag { samples } => {
                    let new_abs = match self.axis {
                        ScrollAxis::Horizontal => e.abs.x,
                        ScrollAxis::Vertical => e.abs.y,
                    };
                    let old_sample = *samples.last().unwrap();
                    push_sample(samples, new_abs, e.time);

                    let delta = new_abs - old_sample.abs;
                    let scroll_pos = self.get_scroll_pos();

                    if self.set_scroll_pos(cx, scroll_pos - delta) {
                        dispatch_action(cx, self.make_scroll_action());
                    }
                }
                _ => (),
            },
            Hit::FingerUp(fe) if fe.is_primary_hit() => match &mut self.scroll_state {
                ScrollState::Drag { samples } => {
                    // Estimate the release velocity (pixels/second) like a native
                    // VelocityTracker (see `scroll_motion`), then start the same momentum
                    // fling as PortalList — same model, same parameters — so drag flicks
                    // decelerate identically in every scrollable view.
                    let (release_velocity, total_delta) = estimate_release_velocity(samples);
                    let max_velocity = self.flick_scroll_maximum * PER_FRAME_TO_PER_SECOND;
                    let release_velocity = release_velocity.clamp(-max_velocity, max_velocity);
                    let min_velocity = self.flick_scroll_minimum * PER_FRAME_TO_PER_SECOND;
                    if total_delta.abs() > FLING_MIN_TOTAL_DELTA
                        && release_velocity.abs() > min_velocity
                    {
                        self.scroll_state = ScrollState::Flick {
                            fling: Fling::new(release_velocity),
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

        if self.visible {
            self.animator_handle_event(cx, event);
            if self.next_frame.is_event(event).is_some() {
                if self.move_towards_scroll_target(cx) {
                    self.next_frame = cx.new_next_frame();
                }
                return dispatch_action(cx, self.make_scroll_action());
            }

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
                }
                Hit::FingerHoverIn(_) => {
                    self.animator_play(cx, ids!(hover.on));
                }
                Hit::FingerHoverOut(_) => {
                    self.animator_play(cx, ids!(hover.off));
                }
                Hit::FingerUp(fe) if fe.is_primary_hit() => {
                    self.drag_point = None;
                    if fe.is_over && fe.device.has_hovers() {
                        self.animator_play(cx, ids!(hover.on));
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }
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
                Some((fling.step(ne.time), fling.is_active(min_velocity)))
            } else {
                None
            }
        } else {
            None
        };

        match step {
            None => {}
            Some((None, _)) => {
                // First fling frame: time baseline established, no movement yet.
                if let ScrollState::Flick { next_frame, .. } = &mut self.scroll_state {
                    *next_frame = cx.new_next_frame();
                }
            }
            Some((Some(displacement), active)) => {
                if active {
                    let scroll_pos = self.get_scroll_pos();
                    if self.set_scroll_pos(cx, scroll_pos - displacement) {
                        dispatch_action(cx, self.make_scroll_action());
                    }
                    if let ScrollState::Flick { next_frame, .. } = &mut self.scroll_state {
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
        self.axis = axis;

        match self.axis {
            ScrollAxis::Horizontal => {
                self.visible = view_total.x > view_rect.size.x + 0.1;
                self.scroll_size = if view_total.y > view_rect.size.y + 0.1 {
                    view_rect.size.x - self.bar_size
                } else {
                    view_rect.size.x
                } - self.bar_side_margin * 2.;
                self.view_total = view_total.x;
                self.view_visible = view_rect.size.x;
                self.scroll_pos = self
                    .scroll_pos
                    .min(self.view_total - self.view_visible)
                    .max(0.);

                if self.visible {
                    let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();
                    self.draw_bg.is_vertical = 0.0;
                    self.draw_bg.norm_scroll = norm_scroll as f32;
                    self.draw_bg.norm_handle = norm_handle as f32;
                    let scroll = cx.turtle().scroll();
                    self.draw_bg.draw_rel(
                        cx,
                        Rect {
                            pos: dvec2(self.bar_side_margin, view_rect.size.y - self.bar_size)
                                + scroll,
                            size: dvec2(self.scroll_size, self.bar_size),
                        },
                    );
                }
            }
            ScrollAxis::Vertical => {
                // compute if we need a horizontal one
                self.visible = view_total.y > view_rect.size.y + 0.1;
                self.scroll_size = if view_total.x > view_rect.size.x + 0.1 {
                    view_rect.size.y - self.bar_size
                } else {
                    view_rect.size.y
                } - self.bar_side_margin * 2.;
                self.view_total = view_total.y;
                self.view_visible = view_rect.size.y;
                self.scroll_pos = self
                    .scroll_pos
                    .min(self.view_total - self.view_visible)
                    .max(0.);

                if self.visible {
                    let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();
                    self.draw_bg.is_vertical = 1.0;
                    self.draw_bg.norm_scroll = norm_scroll as f32;
                    self.draw_bg.norm_handle = norm_handle as f32;
                    let scroll = cx.turtle().scroll();
                    self.draw_bg.draw_rel(
                        cx,
                        Rect {
                            pos: dvec2(view_rect.size.x - self.bar_size, self.bar_side_margin)
                                + scroll,
                            size: dvec2(self.bar_size, self.scroll_size),
                        },
                    );
                }
            }
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
