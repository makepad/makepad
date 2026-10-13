use crate::tween::{
    prop, tag, ClockPolicy, EventKind, LagSmoothing, PropKey, PropTo, Tag, TargetId, TweenEvent,
    TweenHost, TweenOpts,
};
use crate::{
    animator::*, button::*, label::*, makepad_derive_widget::*, makepad_draw::*, view::*,
    widget::*, widget_match_event::WidgetMatchEvent, widget_tree::CxWidgetExt,
    event::{BackGestureEdge, BackGestureState, DigitDevice, FingerCancelEvent, TouchPoint, TouchState, TouchUpdateEvent},
    scroll_motion::{critically_damped_spring, estimate_release_velocity, push_sample, FrameClock, ScrollSample},
};
use std::collections::HashMap;

/// The slide's clock: raw frame deltas with no lag clamp (the Animator it
/// replaces measured `nf.time - start_time`), and a first frame of 0.
const SLIDE_CLOCK: ClockPolicy = ClockPolicy {
    lag: Some(LagSmoothing::OFF),
    first_dt: 0.0,
    follow_ticker: true,
};
/// The one target of the slide host: this view.
const SLIDE_SELF: TargetId = TargetId(0);
/// The slid property, `slide` (a fraction of the view's width: 0 shown, 1 hidden).
const SLIDE_PROP: PropKey = prop(live_id!(slide));
/// Tags of the two slides, read back from their events.
const SLIDE_SHOW: Tag = tag(live_id!(slide_show));
const SLIDE_HIDE: Tag = tag(live_id!(slide_hide));

/// A slide whose start waits for the script VM (see
/// `StackNavigationView::start_slide`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum SlideStart {
    Show,
    Hide,
}

/// How far (in the OS's points, so UI zoom doesn't shrink it) a finger must move before we decide
/// whether it's a swipe back. That's iOS's pan threshold, and at least both platforms' long-press tolerance.
const SWIPE_BACK_SLOP: f64 = 10.0;

/// A swipe back must move at least this many times more sideways than vertically.
const SWIPE_BACK_DIRECTION_RATIO: f64 = 2.0;

/// We project where a released view would coast to over this long (in seconds)
/// and go back if that's past halfway, like iOS and react-navigation do.
const RELEASE_PROJECTION_TIME: f64 = 0.3;

/// The fastest release velocity we honor, in layout px per second.
const MAX_RELEASE_VELOCITY: f64 = 8000.0;

/// A finger that sat still this long (in seconds) before lifting gets no velocity.
const STALE_RELEASE_TIME: f64 = 0.04;

/// The decay rate of the critically damped spring that finishes a released view:
/// sqrt(1000 / 3), react-navigation's iOS stack spring, which rests after about 0.45 s.
const RELEASE_SPRING_RATE: f64 = 18.257_418_583_505_537;

/// A committed view springs toward a bit past the edge, so it leaves the screen at speed.
const COMMIT_OVERSHOOT: f64 = 0.05;

/// A canceled view counts as back at rest once it's this close (in layout px).
const CANCEL_LANDING_DISTANCE: f64 = 0.5;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.StackNavigationViewBase = #(StackNavigationView::register_widget(vm))
    mod.widgets.StackNavigationBase = #(StackNavigation::register_widget(vm))

    // StackView DSL
    let HEADER_HEIGHT = 80.0

    mod.widgets.StackViewHeader = View{
        width: Fill height: (HEADER_HEIGHT)
        padding: Inset{bottom: 10. top: 50.}
        show_bg: true
        draw_bg.color: theme.color_app_caption_bar

        content := View{
            width: Fill height: Fit
            flow: Overlay

            title_container := View{
                width: Fill height: Fit
                align: Align{x: 0.5 y: 0.5}

                title := H4{
                    width: Fit height: Fit
                    margin: 0
                    text: "Stack View Title"
                }
            }

            button_container := View{
                height: Fit width: Fit
                left_button := ButtonFlatterIcon{
                    width: 68 height: 68
                    icon_walk: Walk{
                        height: 10 width: Fit
                    }
                    draw_icon +: {
                        color: theme.color_label_inner
                        svg: crate_resource("makepad_widgets:resources/icons/back.svg")
                    }
                }
            }
        }
    }

    mod.widgets.StackNavigationView = mod.widgets.StackNavigationViewBase{
        visible: false
        width: Fill height: Fill
        flow: Overlay

        show_bg: true
        draw_bg +: {
            color: instance(theme.color_bg_app)
            pixel: fn() {
                return Pal.premul(self.color)
            }
        }

        // Empty view to place a generic background behind the content.
        background := View{
            width: Fill height: Fill
            visible: false
        }

        body := View{
            width: Fill height: Fill
            flow: Down

            // Space between body and header can be adjusted overriding this margin
            margin: Inset{top: (HEADER_HEIGHT)}
        }

        header := mod.widgets.StackViewHeader{}

        slide: 1.0

        animator: Animator{
            slide: {
                default: @hide
                hide: AnimatorState{
                    redraw: true
                    ease: Ease.Emphasized
                    from: {all: Play.Forward{duration: 0.45}}
                    apply: {slide: 1.0}
                }

                show: AnimatorState{
                    redraw: true
                    ease: Ease.Emphasized
                    from: {all: Play.Forward{duration: 0.45}}
                    apply: {slide: 0.0}
                }
            }
        }
    }

    mod.widgets.StackNavigation = mod.widgets.StackNavigationBase{
        width: Fill height: Fill
        flow: Overlay

        root_view := View{}
    }
}

#[derive(Clone, Default, Debug)]
pub enum StackNavigationAction {
    #[default]
    None,
    /// Push a new view onto the navigation stack
    Push(LiveId),
    /// Pop the current view from the navigation stack
    Pop,
    /// Pop all views and return to the root view
    PopToRoot,
}

#[derive(Clone, Default, Eq, Hash, PartialEq, Debug)]
pub enum StackNavigationViewState {
    #[default]
    Inactive,
    Active,
}

/// Actions that are delivered to an incoming or outgoing "active" widget/view
/// within a stack navigation container.
#[derive(Clone, Default, Debug)]
pub enum StackNavigationTransitionAction {
    #[default]
    None,
    ShowBegin,
    ShowDone,
    HideBegin,
    HideEnd(WidgetUid), // Include the parent navigation's UID
    ViewReleased(LiveId),
    /// A swipe or the system back gesture started dragging the current view away, revealing
    /// the root view. To reveal another view, populate it and call
    /// [`StackNavigationRef::set_interactive_pop_view()`] right away.
    InteractivePopStarted,
    /// The user let go past the threshold, so the pop will finish. Record it in your history now,
    /// or call [`StackNavigationRef::cancel_interactive_pop()`] to slide the view back instead.
    InteractivePopCommitted,
    /// The current view slid back and the revealed view was released.
    InteractivePopCancelled,
}

#[derive(Script, Widget, Animator)]
pub struct StackNavigationView {
    #[source]
    source: ScriptObjectRef,

    #[deref]
    view: View,

    /// How far this view has slid out to the right, as a fraction of its width:
    /// 0 when it's fully shown and 1 when it's fully hidden. A back gesture can also slide it left.
    #[live]
    slide: f64,

    /// Configures the slide (its `slide` group: durations, eases, offsets,
    /// including apps' overrides); the `slide` host below plays it.
    #[apply_default]
    animator: Animator,

    /// Plays the slide into `slide` and reports its ends as events.
    #[rust(TweenHost::with_clock(SLIDE_CLOCK)
        .inspect_named("StackNavigation slide")
        .inspect_tags(&["slide_show", "slide_hide"]))]
    slide_host: TweenHost,

    /// The slide host's drained events (reused every frame).
    #[rust]
    slide_events: Vec<TweenEvent>,

    /// A slide requested while the script VM was held (inside an apply walk):
    /// started on the next event, as the Animator replays deferred plays.
    #[rust]
    slide_deferred: Option<SlideStart>,

    /// The slide to start on the next frame, so that a slow first draw of the view
    /// it reveals happens before the slide's clock starts, rather than making it jump.
    #[rust]
    pending_slide: Option<SlideStart>,

    #[rust]
    next_frame: NextFrame,

    /// The state of the stack view.
    #[rust]
    state: StackNavigationViewState,

    /// Held from the start of opening until this view is hidden. Acquiring it
    /// before the animation keeps popups opened during the slide in front.
    #[rust]
    cancel_scope: Option<CancelScope>,

    /// The UID of the parent navigation.
    #[rust]
    parent_navigation_uid: Option<WidgetUid>,

    #[rust]
    is_moved_by_gesture: bool,

    /// Runtime override for the header title text, set by `set_title`.
    ///
    /// Stored here (rather than relying solely on `Label.text` persisting
    /// across re-applies) because in some configurations — notably nested
    /// inside an `AdaptiveView` whose variant gets re-instantiated on
    /// `WindowGeomChange` — the inner `title` `Label` widget can be
    /// recreated via `Apply::New` on rotation, which bypasses
    /// `ArcStringMut::script_apply`'s `ScriptReapply` early-return. The
    /// override is re-asserted in `on_after_apply` after every apply walk
    /// so the title sticks regardless of whether the label was re-applied
    /// or recreated. Storage cost is one `Rc<String>` clone per stack
    /// view that has had a title set — refcount bump, no byte copy.
    #[rust]
    runtime_title: Option<ArcStringMut>,
}

impl ScriptHook for StackNavigationView {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // After every apply walk, re-install the runtime title (if any) into
        // the inner header title label. This is the bulletproof path —
        // independent of whether the label was re-applied (`Apply::Reload` /
        // `Apply::ScriptReapply`) or freshly recreated (`Apply::New`, e.g.
        // when an enclosing `AdaptiveView` re-instantiates its variant on
        // `WindowGeomChange`). Either way, the runtime title wins.
        //
        // We skip:
        //   * `Apply::Eval`     — temporary objects, not real applies.
        //   * `Apply::Animate`  — animator-driven re-apply; widget structure
        //                         unchanged, no template values were reset.
        //   * `Apply::Default(N)` — the recursive call from
        //                         `#[apply_default] animator`. The outer
        //                         apply (New/Reload/ScriptReapply) already
        //                         fired this hook; we don't need to redo
        //                         the title write at every animator-group
        //                         recursion level.
        if apply.is_eval() || apply.is_animate() || apply.as_default().is_some() {
            return;
        }
        if let Some(title) = self.runtime_title.as_ref() {
            // Borrow only what we need from `runtime_title` — `as_ref()`
            // returns `&str`, which lives as long as the `&self` borrow.
            // `write_title_to_header_label` only takes `&self`, so the
            // outstanding borrow on `self.runtime_title` is fine.
            let title = title.as_ref();
            self.write_title_to_header_label(vm.cx_mut(), title);
        }
    }
}

impl StackNavigationView {
    /// Set the runtime title override for this stack view.
    ///
    /// The title persists across every kind of apply walk (LiveEdit,
    /// `request_script_reapply`, AdaptiveView variant swap, etc.) because
    /// it's re-asserted in `on_after_apply`.
    pub(crate) fn set_runtime_title(&mut self, cx: &mut Cx, title: &str) {
        let mut owned = ArcStringMut::default();
        owned.set(title);
        self.runtime_title = Some(owned);
        // Apply immediately so the change is visible without waiting for the
        // next apply walk.
        self.write_title_to_header_label(cx, title);
    }

    /// Single source of truth for the descendant path that resolves the
    /// header title `Label`. Both `set_runtime_title` (initial write) and
    /// `on_after_apply` (re-assertion after every apply walk) go through
    /// this so the path lives in exactly one place; restructuring the
    /// `StackViewHeader` DSL only requires updating this one site.
    fn write_title_to_header_label(&self, cx: &mut Cx, title: &str) {
        self.view
            .label(cx, ids!(header.content.title_container.title))
            .set_text(cx, title);
    }
}

impl Widget for StackNavigationView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The animator first: it flushes deferred cuts, which the slide host
        // then overrides while it plays.
        if let Some(start) = self.pending_slide {
            if self.next_frame.is_event(event).is_some() {
                self.pending_slide = None;
                self.start_slide(cx, start);
            }
        }
        if self.animator_handle_event(cx, event).must_redraw() {
            self.view.redraw(cx);
        }
        if let Some(start) = self.slide_deferred.take() {
            self.start_slide(cx, start);
        }
        self.step_slide(cx, event);
        self.view.handle_event(cx, event, scope);

        self.handle_stack_view_closure_request(cx, event, scope);
        self.finish_slide_if_done(cx);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.slide_host.draw_check(cx);
        let parent_rect = cx.peek_walk_turtle(walk);
        let abs_pos = Vec2d {
            x: parent_rect.pos.x + self.slide * parent_rect.size.x,
            y: parent_rect.pos.y,
        };

        self.view.draw_walk(cx, scope, walk.with_abs_pos(abs_pos))
    }
}

impl StackNavigationView {
    fn hide_stack_view(&mut self, cx: &mut Cx) {
        self.begin_hide(cx);
        self.schedule_slide(cx, SlideStart::Hide);
    }

    fn begin_hide(&mut self, cx: &mut Cx) {
        self.dismiss_transient_children(cx);
        cx.widget_action(
            self.widget_uid(),
            StackNavigationTransitionAction::HideBegin,
        );
    }

    fn dismiss_transient_children(&mut self, cx: &mut Cx) {
        self.view.handle_event(cx,
            &Event::Actions(vec![Box::new(crate::modal::ModalAction::Dismissed)]),
            &mut Scope::empty());
    }

    fn handle_stack_view_closure_request(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _scope: &mut Scope,
    ) {
        // Hide the active stack view if:
        // * the back navigation button/gesture occurred,
        // * the left_button was clicked,
        // * the "back" button on the mouse was clicked.
        if self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s))
            && self.is_animating()
        {
            // Navigation cannot change mid-transition, but Back must not fall
            // through to the outgoing view or the operating system.
            event.back_pressed();
            return;
        }
        if matches!(self.state, StackNavigationViewState::Active) {
            // Ownership decides, so a modal or pane opened over this view keeps the press
            // and this view stays put. Checked before `back_pressed()`, which consumes.
            let owns_back = self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s));
            if (owns_back && (event.back_pressed()
                    || matches!(event, Event::MouseUp(mouse) if mouse.button.is_back())))
                // The left button is an explicit click on this view's own header, not a
                // gesture something in front of it could have a better claim to.
                || matches!(event, Event::Actions(actions) if self.button(cx, ids!(left_button)).clicked(&actions))
            {
                cx.widget_action(self.widget_uid(), StackNavigationAction::Pop);
            }
        }
    }

    /// Starts the slide into the `slide` group's `show` or `hide` state on
    /// the host, with the duration, ease and offsets the Animator holds for
    /// it. The Animator is cut to the destination at once (so `in_state`
    /// and a re-apply see where the slide goes) and the host carries
    /// `offset` there. Cuts need the script VM: while an apply walk holds it
    /// the start waits for the next event, as `Animator::defer_play` does.
    fn start_slide(&mut self, cx: &mut Cx, start: SlideStart) {
        if cx.is_script_vm_held() {
            self.slide_deferred = Some(start);
            cx.new_next_frame();
            return;
        }
        let (from, to, motion, tag) = match start {
            SlideStart::Show => {
                // Each cut applies its state now: read the offsets back.
                self.animator_cut(cx, ids!(slide.show));
                let show_to = self.slide;
                self.animator_cut(cx, ids!(slide.hide));
                // The show starts from the hide offset (4000, or the app's
                // override), as the Animator's play from the hide state did.
                let hide_to = self.slide;
                let motion = self
                    .animator
                    .state_motion(live_id!(slide), live_id!(show), Some(live_id!(hide)))
                    .unwrap_or(StateMotion {
                        secs: 0.45,
                        ease: Ease::Emphasized,
                        redraw: true,
                    });
                self.animator_cut(cx, ids!(slide.show));
                (hide_to, show_to, motion, SLIDE_SHOW)
            }
            SlideStart::Hide => {
                // From where the view is (the cut applies the hide offset).
                let from = self.slide;
                self.animator_cut(cx, ids!(slide.hide));
                let hide_to = self.slide;
                let motion = self
                    .animator
                    .state_motion(live_id!(slide), live_id!(hide), Some(live_id!(show)))
                    .unwrap_or(StateMotion {
                        secs: 0.45,
                        ease: Ease::Emphasized,
                        redraw: true,
                    });
                (from, hide_to, motion, SLIDE_HIDE)
            }
        };
        self.stop_slide();
        // The first frame of the slide moves 0, as the Animator's first
        // frame (its start time) did, even when a slide was cut short.
        self.slide_host.clock.stop();
        self.slide = from;
        self.slide_host.seed(SLIDE_SELF, SLIDE_PROP, from);
        let opts = TweenOpts::new()
            .duration(motion.secs)
            .ease((&motion.ease).into())
            .tag(tag)
            .on_update()
            .on_complete();
        self.slide_host.to(
            cx,
            SLIDE_SELF.into(),
            &[PropTo::to_f64(SLIDE_PROP, to)],
            opts,
        );
        if !self.slide_host.is_active() {
            // Over as it was built (a duration of 0, or reduced motion):
            // land now. The next event may be any event, not the host's
            // frame, and it drains the Complete that reports the end
            // against this offset.
            self.slide = self.slide_host.f64(SLIDE_SELF, SLIDE_PROP, to);
        }
        self.view.redraw(cx);
    }

    /// Ends any slide at once, dropping what it queued (external completion:
    /// `show_at_rest` / `hide_immediately`, and apps that synthesize
    /// `ShowDone` / `HideEnd`). `offset` stays where the caller puts it.
    fn stop_slide(&mut self) {
        self.slide_deferred = None;
        self.pending_slide = None;
        // Straight on the engine: the host's kill arms a frame to report the
        // kill, and there is nothing to report (the events go just below).
        self.slide_host.engine.kill_all();
        self.slide_host.clear_changes();
        self.slide_host.engine.clear_events();
        self.slide_events.clear();
    }

    /// Steps the slide host and writes its value into `offset` whenever it
    /// plays (or landed this frame), so a re-apply or a deferred cut cannot
    /// snap the view mid-slide. Drains the slide's events.
    fn step_slide(&mut self, cx: &mut Cx, event: &Event) {
        let act = self.slide_host.handle_event(cx, event);
        if self.slide_host.is_active() || act.settled() {
            let v = self.slide_host.f64(SLIDE_SELF, SLIDE_PROP, self.slide);
            if v != self.slide {
                self.slide = v;
                self.view.redraw(cx);
            }
        }
        self.slide_host.clear_changes();
        self.slide_host.swap_events(&mut self.slide_events);
    }

    /// Reports the ends of the slide from its frames: ShowDone when the incoming
    /// view has arrived, HideEnd when the outgoing view has slid out.
    fn finish_slide_if_done(&mut self, cx: &mut Cx) {
        for i in 0..self.slide_events.len() {
            let e = self.slide_events[i];
            if !matches!(e.kind, EventKind::Complete) {
                continue;
            }
            if e.tag == SLIDE_SHOW && self.state == StackNavigationViewState::Inactive {
                cx.widget_action(self.widget_uid(), StackNavigationTransitionAction::ShowDone);
                self.set_nav_state(cx, StackNavigationViewState::Active);
                return;
            }
            if e.tag == SLIDE_HIDE && self.state == StackNavigationViewState::Active {
                self.finish_hide(cx);
                return;
            }
        }
    }

    fn finish_hide(&mut self, cx: &mut Cx) {
        self.view.visible = false;
        self.redraw(cx);
        // Release the scope even for standalone views with no parent to notify.
        self.stop_slide();
        self.animator_cut(cx, ids!(slide.hide));
        self.set_nav_state(cx, StackNavigationViewState::Inactive);

        // Dispatch HideEnd with the parent navigation's UID
        let hide_end_action = if let Some(parent_uid) = self.parent_navigation_uid {
            StackNavigationTransitionAction::HideEnd(parent_uid)
        } else {
            error!(
                "No parent navigation UID found for stack view {:?}",
                self.widget_uid()
            );
            return;
        };

        cx.widget_action(self.widget_uid(), hide_end_action);
    }

    /// Finish a state transition without replacing a scope already acquired by
    /// `show`: doing so would overtake children opened during the animation.
    fn set_nav_state(&mut self, cx: &mut Cx, state: StackNavigationViewState) {
        match state {
            StackNavigationViewState::Active => {
                if self.cancel_scope.is_none() {
                    self.cancel_scope = Some(self.begin_cancel_scope_for(cx, CancelScopeKind::Back));
                }
            }
            StackNavigationViewState::Inactive => {
                if let Some(scope) = self.cancel_scope.take() {
                    cx.end_cancel_scope(scope);
                }
            }
        }
        self.state = state;
    }

    fn schedule_slide(&mut self, cx: &mut Cx, start: SlideStart) {
        self.is_moved_by_gesture = false;
        self.pending_slide = Some(start);
        self.next_frame = cx.new_next_frame();
    }

    fn is_animating(&self) -> bool {
        self.is_moved_by_gesture
            || self.pending_slide.is_some()
            || self.slide_host.is_active()
    }
}

impl StackNavigationViewRef {
    pub fn show(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.view.visible = true;
            inner.slide = 1.0;
            inner.set_nav_state(cx, StackNavigationViewState::Inactive);
            inner.cancel_scope = Some(inner.begin_cancel_scope_for(cx, CancelScopeKind::Back));

            // Always slide in from fully hidden, wherever an earlier slide left off.
            inner.schedule_slide(cx, SlideStart::Show);
            inner.redraw(cx);
        }
    }

    pub fn is_showing(&self, cx: &mut Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.animator.in_state(cx, ids!(slide.show)) || inner.is_animating()
        } else {
            false
        }
    }

    pub fn is_animating(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.is_animating()
        } else {
            false
        }
    }

    pub fn hide(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.hide_stack_view(cx);
        }
    }

    pub fn show_at_rest(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.stop_slide();
            inner.view.visible = true;
            inner.slide = 0.0;
            inner.pending_slide = None;
            inner.is_moved_by_gesture = false;
            inner.set_nav_state(cx, StackNavigationViewState::Active);
            inner.animator_cut(cx, ids!(slide.show));
            inner.redraw(cx);
        }
    }

    pub fn hide_immediately(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.stop_slide();
            if inner.view.visible {
                inner.dismiss_transient_children(cx);
            }
            inner.slide = 1.0;
            inner.pending_slide = None;
            inner.is_moved_by_gesture = false;
            inner.view.visible = false;
            inner.set_nav_state(cx, StackNavigationViewState::Inactive);
            inner.animator_cut(cx, ids!(slide.hide));
            inner.redraw(cx);
        }
    }

    pub fn set_parent_navigation_uid(&self, parent_uid: WidgetUid) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.parent_navigation_uid = Some(parent_uid);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum StackNavigationTransition {
    Push {
        incoming: LiveId,
        outgoing: Option<LiveId>,
    },
    Pop {
        incoming: Option<LiveId>,
        outgoing: LiveId,
    },
}

/// A finger on the current view that may still turn into a swipe back.
#[derive(Clone, Copy, Debug)]
struct SwipeBackCandidate {
    uid: u64,
    start_abs: Vec2d,
}

/// What drags the current view away during an interactive pop.
#[derive(Clone, Copy, Debug, PartialEq)]
enum InteractivePopDriver {
    /// The finger with this touch uid, which was at `origin_x` when the view was at rest.
    Touch { uid: u64, origin_x: f64 },
    /// The system back gesture (Android's predictive back), whose progress the view follows.
    BackGesture,
}

#[derive(Clone, Debug)]
enum InteractivePopPhase {
    Dragging {
        driver: InteractivePopDriver,
        /// How far the gesture has moved the view over time, for its release velocity.
        samples: Vec<ScrollSample>,
    },
    /// The gesture let go, so a spring carries the view off-screen (`is_commit`) or back to rest.
    Releasing {
        is_commit: bool,
        start_progress: f64,
        /// In widths per second, toward going back.
        start_velocity: f64,
        clock: FrameClock,
        next_frame: NextFrame,
    },
}

/// A pop whose outgoing view is moved by a gesture rather than by its slide animation.
#[derive(Clone, Debug)]
struct InteractivePop {
    phase: InteractivePopPhase,
    /// How far the outgoing view has slid out, as a fraction of its width.
    progress: f64,
    /// 1 if the view slides out to the right, or -1 to the left.
    direction: f64,
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct StackNavigation {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    /// Optional templates that can be instantiated dynamically and pushed onto
    /// the stack at runtime.
    #[live]
    stack_templates: ScriptObjectRef,

    /// Whether swiping a finger right pops the current view.
    /// The system back gesture always can.
    #[live(true)]
    enable_swipe_back: bool,

    #[rust]
    current_view: Option<LiveId>,

    #[rust]
    transition: Option<StackNavigationTransition>,

    #[rust]
    swipe_back_candidate: Option<SwipeBackCandidate>,

    #[rust]
    interactive_pop: Option<InteractivePop>,

    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,

    #[rust]
    dynamic_views: ComponentMap<LiveId, WidgetRef>,

    #[rust]
    dynamic_view_templates: HashMap<LiveId, LiveId>,

    #[rust]
    next_dynamic_view_index: u64,
}

impl ScriptHook for StackNavigation {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_new() {
            self.current_view = None;
            self.transition = None;
            self.swipe_back_candidate = None;
            self.interactive_pop = None;
            self.dynamic_views.clear();
            self.dynamic_view_templates.clear();
            self.next_dynamic_view_index = 0;
        }
        if apply.is_reload() {
            self.templates.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_eval() {
            return;
        }

        self.collect_stack_templates(vm);

        if apply.is_reload() {
            self.reapply_dynamic_views(vm, apply, scope);
            self.restore_current_visual_state(vm.cx_mut());
        }
    }
}

impl Widget for StackNavigation {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.step_interactive_pop_release(cx, event);

        let dragging_driver = self.interactive_pop.as_ref().and_then(|pop| match pop.phase {
            InteractivePopPhase::Dragging { driver, .. } => Some(driver),
            InteractivePopPhase::Releasing { .. } => None,
        });
        match (event, dragging_driver) {
            (Event::TouchUpdate(e), Some(driver)) => {
                // Nothing in the stack gets touch input while a gesture drags the view, but the views
                // still see a finger end, as a cancel, so nothing in them stays pressed.
                for touch in e.touches.iter().filter(|touch| touch.state == TouchState::Stop) {
                    let cancel_event = cancel_touch(cx, e, touch);
                    self.forward_event_to_views(cx, &cancel_event, scope);
                }
                if let InteractivePopDriver::Touch { uid, origin_x } = driver {
                    match e.touches.iter().find(|touch| touch.uid == uid) {
                        Some(touch) if matches!(touch.state, TouchState::Move) => {
                            self.drag_interactive_pop(cx, touch.abs.x - origin_x, touch.time);
                        }
                        // iOS re-sends a finger that didn't move with the time it last moved.
                        Some(touch) if matches!(touch.state, TouchState::Stable) => {}
                        Some(touch) if matches!(touch.state, TouchState::Stop) => {
                            // A finger that sat still before lifting doesn't fling the view.
                            let has_paused = matches!(&self.interactive_pop,
                                Some(InteractivePop { phase: InteractivePopPhase::Dragging { samples, .. }, .. })
                                    if samples.last().is_some_and(|last| touch.time - last.time > STALE_RELEASE_TIME)
                            );
                            self.drag_interactive_pop(cx, touch.abs.x - origin_x, touch.time);
                            let velocity = if has_paused { 0.0 } else { self.get_interactive_pop_release_velocity(cx) };
                            let progress = self.interactive_pop.as_ref().map_or(0.0, |pop| pop.progress);
                            let is_commit = progress + RELEASE_PROJECTION_TIME * velocity >= 0.5;
                            self.release_interactive_pop(cx, is_commit, velocity);
                        }
                        // We missed its end.
                        _ => self.release_interactive_pop(cx, false, 0.0),
                    }
                }
            }
            // The OS took the touch away.
            (Event::FingerCancel(e), Some(InteractivePopDriver::Touch { uid, .. }))
                if e.digit_id == live_id_num!(touch, uid).into() && cx.fingers.press_taken_away(e.digit_id) =>
            {
                self.forward_event_to_views(cx, event, scope);
                self.release_interactive_pop(cx, false, 0.0);
            }
            (Event::LongPress(_), Some(_)) => {}
            (Event::TouchUpdate(e), None) => 'touch: {
                // Only a single finger on a view that Back would go to can swipe it away, so nothing that
                // covers it (a modal, or a popover holding the pointer) gets left behind.
                if let (true, Some(current), [touch @ TouchPoint { state: TouchState::Start, .. }]) = (
                    self.enable_swipe_back && self.transition.is_none() && cx.sweep_lock_area().is_none(),
                    self.current_view,
                    e.touches.as_slice(),
                ) {
                    if self.is_back_owned_by_current_view(cx, Cx::owns_back_swipe)
                        && self.stack_view_ref(cx, current).area().rect(cx).contains(touch.abs)
                    {
                        self.swipe_back_candidate = Some(SwipeBackCandidate { uid: touch.uid, start_abs: touch.abs });
                    }
                }
                // The views see each move first, so a widget that gives this finger's drag its own meaning
                // (a scroller claiming it, or a popover its press opened) keeps it before we'd take it.
                self.forward_event_to_views(cx, event, scope);
                let Some(candidate) = self.swipe_back_candidate.take() else { break 'touch };
                let [touch] = e.touches.as_slice() else { break 'touch };
                if touch.uid != candidate.uid
                    || cx.fingers.is_touch_drag_claimed(touch.uid)
                    || cx.sweep_lock_area().is_some()
                {
                    break 'touch;
                }
                let delta = touch.abs - candidate.start_abs;
                match touch.state {
                    TouchState::Start => self.swipe_back_candidate = Some(candidate),
                    TouchState::Move | TouchState::Stable
                        if delta.length() < cx.windows[e.window_id].native_points_to_layout(SWIPE_BACK_SLOP) =>
                    {
                        self.swipe_back_candidate = Some(candidate);
                    }
                    // A finger that swipes right far enough takes over the current view.
                    TouchState::Move | TouchState::Stable => {
                        let (true, Some(current)) = (
                            delta.x >= SWIPE_BACK_DIRECTION_RATIO * delta.y.abs() && self.transition.is_none(),
                            self.current_view,
                        ) else { break 'touch };
                        // The view's widgets see this touch get canceled, so none of them act on it.
                        let cancel_event = cancel_touch(cx, e, touch);
                        self.stack_view_ref(cx, current).handle_event(cx, &cancel_event, scope);
                        // The pop can end while this finger is still down, so the rest of it must hit nothing.
                        cx.release_touch_captures(touch.uid);
                        // This hides the soft keyboard of a text input in the view.
                        cx.set_key_focus(Area::Empty);
                        let driver = InteractivePopDriver::Touch { uid: touch.uid, origin_x: touch.abs.x };
                        self.begin_interactive_pop(cx, driver, 1.0, touch.time);
                    }
                    _ => {}
                }
            }
            (Event::LongPress(_), None) => {
                self.swipe_back_candidate = None;
                self.forward_event_to_views(cx, event, scope);
            }
            (Event::BackGesture(e), _) => match e.state {
                // A gesture without a touch is a back button press, which keeps its usual slide.
                BackGestureState::Start if e.abs.is_some() => {
                    // The view follows the finger, so a swipe from the right edge slides it left.
                    let direction = if e.edge == BackGestureEdge::Right { -1.0 } else { 1.0 };
                    // A swipe that the system took over for its back gesture continues under its control.
                    let can_take_over = self.interactive_pop.as_ref().map_or(self.transition.is_none(), |pop|
                        pop.direction == direction && matches!(pop.phase,
                            InteractivePopPhase::Dragging { driver: InteractivePopDriver::Touch { .. }, .. }
                            | InteractivePopPhase::Releasing { is_commit: false, .. }
                        )
                    );
                    // A popover holding the pointer gets the Back instead.
                    if !can_take_over || !self.is_back_owned_by_current_view(cx, Cx::owns_cancel) || cx.sweep_lock_area().is_some() {
                        self.forward_event_to_views(cx, event, scope);
                    } else if let Some(pop) = &mut self.interactive_pop {
                        pop.phase = InteractivePopPhase::Dragging {
                            driver: InteractivePopDriver::BackGesture,
                            samples: Vec::new(),
                        };
                    } else {
                        self.begin_interactive_pop(cx, InteractivePopDriver::BackGesture, direction, e.time);
                    }
                }
                // The system smooths the gesture's progress, and eases it back to 0
                // when the user lets go without going back.
                BackGestureState::Move if dragging_driver == Some(InteractivePopDriver::BackGesture) => {
                    let distance = e.progress * self.get_interactive_pop_width(cx);
                    self.drag_interactive_pop(cx, distance, e.time);
                }
                BackGestureState::Cancel if dragging_driver == Some(InteractivePopDriver::BackGesture) => {
                    let velocity = self.get_interactive_pop_release_velocity(cx);
                    self.release_interactive_pop(cx, false, velocity);
                }
                _ => self.forward_event_to_views(cx, event, scope),
            },
            // Completing the system back gesture finishes its pop,
            // unless something in front of the view took Back meanwhile.
            (Event::BackPressed { .. }, Some(InteractivePopDriver::BackGesture)) => {
                let owns_back = self.is_back_owned_by_current_view(cx, Cx::owns_cancel);
                let velocity = self.get_interactive_pop_release_velocity(cx);
                if owns_back && event.back_pressed() {
                    self.release_interactive_pop(cx, true, velocity);
                } else {
                    self.release_interactive_pop(cx, false, velocity);
                    self.forward_event_to_views(cx, event, scope);
                }
            }
            // Back while the view slides back into place goes back after all. Without predictive back,
            // that's how the system's back gesture ends a swipe it took from us.
            (Event::BackPressed { .. }, None) if matches!(&self.interactive_pop,
                Some(InteractivePop { phase: InteractivePopPhase::Releasing { is_commit: false, .. }, .. })
            ) => {
                if self.is_back_owned_by_current_view(cx, Cx::owns_cancel) && event.back_pressed() {
                    self.release_interactive_pop(cx, true, 0.0);
                } else {
                    self.forward_event_to_views(cx, event, scope);
                }
            }
            _ => self.forward_event_to_views(cx, event, scope),
        }

        // Leaving this to the final step, so that the active stack view can handle the event first.
        // It is relevant when the active stack view is animating out and wants to handle
        // the StackNavigationTransitionAction::HideEnd action.
        self.widget_match_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        for (_id, widget_ref) in self.get_visible_views(cx.cx).iter() {
            widget_ref.draw_walk(cx, scope, walk)?;
        }
        DrawStep::done()
    }
}

impl WidgetNode for StackNavigation {
    fn widget_uid(&self) -> WidgetUid {
        self.view.widget_uid()
    }
    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.view.walk(cx)
    }
    fn area(&self) -> Area {
        self.view.area()
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        self.view.children(visit);
        for (id, widget_ref) in self.dynamic_views.iter() {
            visit(*id, widget_ref.clone());
        }
    }

    fn cancel_children_impl(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) -> bool {
        if !self.view.visible {
            return false;
        }
        let active = match self.transition {
            Some(StackNavigationTransition::Push { incoming, .. }) => incoming,
            Some(StackNavigationTransition::Pop { outgoing, .. }) => outgoing,
            None => self.current_view.unwrap_or(id!(root_view)),
        };
        if let Some(widget) = self.dynamic_views.get(&active) {
            visit(active, widget.clone());
        } else if let Some((_, widget)) = self.view.children.iter().find(|(id, _)| *id == active) {
            visit(active, widget.clone());
        }
        true
    }

    fn redraw(&mut self, cx: &mut Cx) {
        for (_id, widget_ref) in self.get_visible_views(cx).iter() {
            widget_ref.redraw(cx);
        }
    }
}

impl WidgetMatchEvent for StackNavigation {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        for action in actions {
            if let Some(widget_action) = action.as_widget_action() {
                if !cx.widget_tree().widget(widget_action.widget_uid).is_empty() {
                    match widget_action.cast() {
                        StackNavigationAction::Push(view_id) => {
                            self.push_view(view_id, cx);
                        }
                        StackNavigationAction::Pop
                            if self.view_id_for_widget_uid(cx, widget_action.widget_uid).is_some() =>
                        {
                            self.pop_to_root(cx);
                        }
                        StackNavigationAction::PopToRoot => {
                            self.pop_to_root(cx);
                        }
                        _ => {}
                    }

                    if let StackNavigationTransitionAction::HideEnd(target_parent_uid) =
                        widget_action.cast()
                    {
                        if target_parent_uid == self.widget_uid() {
                            self.finish_pop_transition(cx, widget_action.widget_uid);
                        }
                    }

                    if let StackNavigationTransitionAction::ShowDone = widget_action.cast() {
                        self.finish_push_transition(cx, widget_action.widget_uid);
                    }
                }
            }
        }
    }
}

impl StackNavigation {
    fn collect_stack_templates(&mut self, vm: &mut ScriptVm) {
        if self.stack_templates.is_zero() {
            return;
        }

        let obj = self.stack_templates.as_object();
        vm.vec_with(obj, |vm, vec| {
            for kv in vec {
                let Some(id) = kv.key.as_id() else {
                    continue;
                };
                let Some(template_obj) = kv.value.as_object() else {
                    warning!("StackNavigation template {id} is not an object, skipping");
                    continue;
                };
                self.templates
                    .insert(id, vm.new_object_ref(template_obj));
            }
        });
    }

    fn reapply_dynamic_views(&mut self, vm: &mut ScriptVm, apply: &Apply, scope: &mut Scope) {
        for (view_id, widget_ref) in self.dynamic_views.iter_mut() {
            let Some(template_id) = self.dynamic_view_templates.get(view_id) else {
                continue;
            };
            let Some(template_ref) = self.templates.get(template_id) else {
                continue;
            };
            let template_value: ScriptValue = template_ref.as_object().into();
            widget_ref.script_apply(vm, apply, scope, template_value);
        }
    }

    fn next_dynamic_view_id(&mut self) -> LiveId {
        let view_id = LiveId::from_str(&format!(
            "stack_navigation_dynamic_view_{}_{}",
            self.widget_uid().0,
            self.next_dynamic_view_index,
        ));
        self.next_dynamic_view_index += 1;
        view_id
    }

    fn transition_contains_view(&self, view_id: LiveId) -> bool {
        match self.transition {
            Some(StackNavigationTransition::Push { incoming, outgoing }) => {
                incoming == view_id || outgoing == Some(view_id)
            }
            Some(StackNavigationTransition::Pop { incoming, outgoing }) => {
                incoming == Some(view_id) || outgoing == view_id
            }
            None => false,
        }
    }

    fn view_is_reserved(&self, view_id: LiveId) -> bool {
        self.current_view == Some(view_id) || self.transition_contains_view(view_id)
    }

    fn visible_view_ids(&self) -> Vec<LiveId> {
        match self.transition {
            Some(StackNavigationTransition::Push { incoming, outgoing }) => {
                let mut view_ids = Vec::with_capacity(2);
                if let Some(outgoing) = outgoing {
                    view_ids.push(outgoing);
                }
                view_ids.push(incoming);
                view_ids
            }
            Some(StackNavigationTransition::Pop { incoming, outgoing }) => {
                let mut view_ids = Vec::with_capacity(2);
                if let Some(incoming) = incoming {
                    view_ids.push(incoming);
                }
                view_ids.push(outgoing);
                view_ids
            }
            None => self.current_view.into_iter().collect(),
        }
    }

    fn free_dynamic_view_id(&self) -> Option<LiveId> {
        self.dynamic_views
            .iter()
            .find_map(|(view_id, _)| (!self.view_is_reserved(*view_id)).then_some(*view_id))
    }

    fn create_view_from_template(
        &mut self,
        cx: &mut Cx,
        template_id: LiveId,
    ) -> Option<(LiveId, WidgetRef)> {
        let Some(template_ref) = self.templates.get(&template_id) else {
            warning!(
                "StackNavigation template not found: {template_id}. \
                Did you add it to `stack_templates`?"
            );
            return None;
        };

        let template_value: ScriptValue = template_ref.as_object().into();
        if let Some(view_id) = self.free_dynamic_view_id() {
            if self.dynamic_view_templates.get(&view_id) == Some(&template_id) {
                let widget_ref = self.dynamic_views.get(&view_id)?.clone();
                return Some((view_id, widget_ref));
            }

            let widget_ref = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
            if widget_ref.is_empty() {
                warning!("StackNavigation template {template_id} created an empty widget");
                return None;
            }
            self.dynamic_view_templates.insert(view_id, template_id);
            self.dynamic_views.insert(view_id, widget_ref.clone());
            cx.widget_tree_insert_child_deep(self.widget_uid(), view_id, widget_ref.clone());
            cx.widget_tree_mark_dirty(self.widget_uid());
            return Some((view_id, widget_ref));
        }

        if self.dynamic_views.len() >= 2 {
            warning!(
                "StackNavigation cannot create a view for template {template_id}: \
                both dynamic views are currently in use"
            );
            return None;
        }

        let widget_ref = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
        if widget_ref.is_empty() {
            warning!("StackNavigation template {template_id} created an empty widget");
            return None;
        }

        let view_id = self.next_dynamic_view_id();
        self.dynamic_view_templates.insert(view_id, template_id);
        self.dynamic_views.insert(view_id, widget_ref.clone());
        cx.widget_tree_insert_child_deep(self.widget_uid(), view_id, widget_ref.clone());
        cx.widget_tree_mark_dirty(self.widget_uid());
        Some((view_id, widget_ref))
    }

    fn stack_view_ref(&mut self, cx: &mut Cx, view_id: LiveId) -> StackNavigationViewRef {
        if let Some(widget_ref) = self.dynamic_views.get(&view_id) {
            return widget_ref.clone().as_stack_navigation_view();
        }
        self.stack_navigation_view(cx, &[view_id])
    }

    fn view_id_for_widget_uid(&mut self, cx: &mut Cx, widget_uid: WidgetUid) -> Option<LiveId> {
        for view_id in self.visible_view_ids() {
            if self.stack_view_ref(cx, view_id).widget_uid() == widget_uid {
                return Some(view_id);
            }
        }
        None
    }

    fn release_view(&mut self, cx: &mut Cx, view_id: LiveId) {
        let stack_view_ref = self.stack_view_ref(cx, view_id);
        stack_view_ref.hide_immediately(cx);
        cx.widget_action(
            self.widget_uid(),
            StackNavigationTransitionAction::ViewReleased(view_id),
        );
    }

    fn restore_current_visual_state(&mut self, cx: &mut Cx) {
        // A gesture outlives the re-apply, so we put both of its views back where it had them.
        if let (true, Some(StackNavigationTransition::Pop { incoming, .. })) = (self.interactive_pop.is_some(), self.transition) {
            if let Some(incoming) = incoming {
                self.stack_view_ref(cx, incoming).show_at_rest(cx);
            }
            self.set_outgoing_slide(cx);
            return;
        }
        let transition = self.transition.take();
        self.current_view = match transition {
            Some(StackNavigationTransition::Push { incoming, outgoing }) => {
                if let Some(outgoing) = outgoing {
                    self.release_view(cx, outgoing);
                }
                Some(incoming)
            }
            Some(StackNavigationTransition::Pop { incoming, outgoing }) => {
                self.release_view(cx, outgoing);
                incoming
            }
            None => self.current_view,
        };

        let active = self.current_view;
        let dynamic_view_ids: Vec<LiveId> = self.dynamic_views.iter().map(|(id, _)| *id).collect();
        for view_id in dynamic_view_ids {
            let stack_view_ref = self.stack_view_ref(cx, view_id);
            if Some(view_id) == active {
                stack_view_ref.show_at_rest(cx);
            } else {
                stack_view_ref.hide_immediately(cx);
            }
        }
        if let Some(view_id) = active {
            if !self.dynamic_views.contains_key(&view_id) {
                self.stack_view_ref(cx, view_id).show_at_rest(cx);
            }
        }
    }

    fn push_view(&mut self, view_id: LiveId, cx: &mut Cx) {
        if self.transition.is_some() || self.current_view == Some(view_id) {
            return;
        }

        let outgoing = self.current_view;
        self.transition = Some(StackNavigationTransition::Push {
            incoming: view_id,
            outgoing,
        });

        let stack_view_ref = self.stack_view_ref(cx, view_id);
        stack_view_ref.set_parent_navigation_uid(self.widget_uid());
        stack_view_ref.show(cx);

        cx.widget_action(
            stack_view_ref.widget_uid(),
            StackNavigationTransitionAction::ShowBegin,
        );

        self.redraw(cx);
    }

    fn pop_to_view(&mut self, cx: &mut Cx, view_id: Option<LiveId>) {
        if self.transition.is_some() || self.current_view == view_id {
            return;
        }
        let Some(outgoing) = self.current_view else {
            if let Some(view_id) = view_id {
                self.push_view(view_id, cx);
            }
            return;
        };

        if let Some(incoming) = view_id {
            self.show_view_beneath(cx, incoming);
        }

        self.transition = Some(StackNavigationTransition::Pop {
            incoming: view_id,
            outgoing,
        });
        let outgoing_view_ref = self.stack_view_ref(cx, outgoing);
        outgoing_view_ref.hide(cx);
        self.redraw(cx);
    }

    fn show_view_beneath(&mut self, cx: &mut Cx, view_id: LiveId) {
        let view_ref = self.stack_view_ref(cx, view_id);
        view_ref.set_parent_navigation_uid(self.widget_uid());
        view_ref.show_at_rest(cx);
    }

    fn pop_to_root(&mut self, cx: &mut Cx) {
        self.pop_to_view(cx, None);
    }

    fn finish_push_transition(&mut self, cx: &mut Cx, widget_uid: WidgetUid) {
        let Some(view_id) = self.view_id_for_widget_uid(cx, widget_uid) else {
            return;
        };
        let Some(StackNavigationTransition::Push { incoming, outgoing }) = self.transition else {
            return;
        };
        if incoming != view_id {
            return;
        }

        self.transition = None;
        self.current_view = Some(incoming);
        self.stack_view_ref(cx, incoming).show_at_rest(cx);
        if let Some(outgoing) = outgoing {
            self.release_view(cx, outgoing);
        }
        self.redraw(cx);
    }

    fn finish_pop_transition(&mut self, cx: &mut Cx, widget_uid: WidgetUid) {
        let Some(view_id) = self.view_id_for_widget_uid(cx, widget_uid) else {
            warning!(
                "StackNavigation received HideEnd from unknown view {:?}",
                widget_uid
            );
            return;
        };
        let Some(StackNavigationTransition::Pop { incoming, outgoing }) = self.transition else {
            return;
        };
        if outgoing != view_id {
            return;
        }

        self.transition = None;
        self.current_view = incoming;
        self.release_view(cx, outgoing);
        if let Some(incoming) = incoming {
            self.stack_view_ref(cx, incoming).show_at_rest(cx);
        }
        self.redraw(cx);
    }

    fn get_visible_views(&mut self, cx: &mut Cx) -> Vec<(LiveId, WidgetRef)> {
        match self.transition {
            Some(StackNavigationTransition::Push { incoming, outgoing }) => {
                let mut views = Vec::with_capacity(2);
                if let Some(outgoing) = outgoing {
                    views.push((outgoing, self.stack_view_ref(cx, outgoing).0.clone()));
                } else {
                    views.push((live_id!(root_view), self.view.widget(cx, ids!(root_view))));
                }
                views.push((incoming, self.stack_view_ref(cx, incoming).0.clone()));
                views
            }
            Some(StackNavigationTransition::Pop { incoming, outgoing }) => {
                let mut views = Vec::with_capacity(2);
                if let Some(incoming) = incoming {
                    views.push((incoming, self.stack_view_ref(cx, incoming).0.clone()));
                } else {
                    views.push((live_id!(root_view), self.view.widget(cx, ids!(root_view))));
                }
                views.push((outgoing, self.stack_view_ref(cx, outgoing).0.clone()));
                views
            }
            None => match self.current_view {
                Some(current) => vec![(current, self.stack_view_ref(cx, current).0.clone())],
                None => vec![(live_id!(root_view), self.view.widget(cx, ids!(root_view)))],
            },
        }
    }

    fn forward_event_to_views(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if event.requires_visibility() {
            // Visibility-gated input (e.g., MouseDown): only forward to the
            // currently-visible stack views, so inactive views don't receive
            // input they shouldn't.
            for (_id, widget_ref) in self.get_visible_views(cx) {
                widget_ref.handle_event(cx, event, scope);
            }
        } else {
            // App-wide event (e.g., `Event::Actions`): delegate to the inner
            // `View`, which propagates to every child — including inactive
            // stack views. This lets them react to global state changes
            // (e.g., preference updates) that are broadcast once and have no
            // other way of reaching views that aren't currently on the stack.
            // `View::handle_event` still gates children whose own `visible`
            // flag is false, but only for events that require visibility,
            // which this branch doesn't cover.
            self.view.handle_event(cx, event, scope);
            for (_id, widget_ref) in self.dynamic_views.iter_mut() {
                widget_ref.handle_event(cx, event, scope);
            }
        }
    }

    /// Starts popping the current view under a gesture's control at `time`,
    /// revealing the root view until the app picks another one.
    fn begin_interactive_pop(&mut self, cx: &mut Cx, driver: InteractivePopDriver, direction: f64, time: f64) {
        let Some(outgoing) = self.current_view else { return };
        self.swipe_back_candidate = None;
        self.transition = Some(StackNavigationTransition::Pop { incoming: None, outgoing });
        let mut samples = Vec::new();
        push_sample(&mut samples, 0.0, time);
        self.interactive_pop = Some(InteractivePop {
            phase: InteractivePopPhase::Dragging { driver, samples },
            progress: 0.0,
            direction,
        });
        self.set_outgoing_slide(cx);
        // The view beneath hasn't been drawn since it got covered.
        self.redraw(cx);
        cx.widget_action(self.widget_uid(), StackNavigationTransitionAction::InteractivePopStarted);
    }

    /// Moves the dragged view to where the gesture has taken it at `time`,
    /// `distance` layout px from rest.
    fn drag_interactive_pop(&mut self, cx: &mut Cx, distance: f64, time: f64) {
        let width = self.get_interactive_pop_width(cx);
        let Some(InteractivePop {
            phase: InteractivePopPhase::Dragging { samples, .. },
            progress,
            ..
        }) = &mut self.interactive_pop else { return };
        push_sample(samples, distance, time);
        *progress = (distance / width).clamp(0.0, 1.0);
        self.set_outgoing_slide(cx);
    }

    /// How fast the gesture was moving the view, in widths per second toward going back.
    fn get_interactive_pop_release_velocity(&mut self, cx: &mut Cx) -> f64 {
        let width = self.get_interactive_pop_width(cx);
        let Some(InteractivePop { phase: InteractivePopPhase::Dragging { samples, .. }, .. }) = &self.interactive_pop else {
            return 0.0;
        };
        let (velocity, _) = estimate_release_velocity(samples);
        velocity.clamp(-MAX_RELEASE_VELOCITY, MAX_RELEASE_VELOCITY) / width
    }

    /// Lets go of the view, which a spring carries off-screen if `is_commit` or else back to rest.
    /// `velocity` is in widths per second toward going back.
    fn release_interactive_pop(&mut self, cx: &mut Cx, is_commit: bool, velocity: f64) {
        let Some(pop) = &mut self.interactive_pop else { return };
        pop.phase = InteractivePopPhase::Releasing {
            is_commit,
            start_progress: pop.progress,
            // We only keep the part of the velocity that heads where the view is going.
            start_velocity: if is_commit { velocity.max(0.0) } else { velocity.min(0.0) },
            clock: FrameClock::default(),
            next_frame: cx.new_next_frame(),
        };
        if is_commit {
            cx.widget_action(self.widget_uid(), StackNavigationTransitionAction::InteractivePopCommitted);
        }
    }

    fn step_interactive_pop_release(&mut self, cx: &mut Cx, event: &Event) {
        let Some(InteractivePop { phase: InteractivePopPhase::Releasing { next_frame, .. }, .. }) = &self.interactive_pop else { return };
        let Some(frame) = next_frame.is_event(event) else { return };
        let width = self.get_interactive_pop_width(cx);
        let Some(pop) = &mut self.interactive_pop else { return };
        let InteractivePopPhase::Releasing { is_commit, start_progress, start_velocity, clock, next_frame } = &mut pop.phase else { return };
        let is_commit = *is_commit;
        let target = if is_commit { 1.0 + COMMIT_OVERSHOOT } else { 0.0 };
        let elapsed = clock.advance(frame.time);
        let progress = target + critically_damped_spring(*start_progress - target, *start_velocity, RELEASE_SPRING_RATE, elapsed);
        let has_landed = if is_commit {
            progress >= 1.0
        } else {
            progress * width <= CANCEL_LANDING_DISTANCE
        };
        if has_landed {
            self.end_interactive_pop(cx, is_commit);
        } else {
            *next_frame = cx.new_next_frame();
            pop.progress = progress;
            self.set_outgoing_slide(cx);
        }
    }

    /// Ends an interactive pop: a commit hides the outgoing view, which then finishes popping
    /// like a timed pop does, and a cancel puts it back at rest and releases the revealed view.
    fn end_interactive_pop(&mut self, cx: &mut Cx, is_commit: bool) {
        self.interactive_pop = None;
        let Some(StackNavigationTransition::Pop { incoming, outgoing }) = self.transition else { return };
        let outgoing_ref = self.stack_view_ref(cx, outgoing);
        if is_commit {
            if let Some(mut outgoing_view) = outgoing_ref.borrow_mut() {
                outgoing_view.is_moved_by_gesture = false;
                outgoing_view.animator_cut(cx, ids!(slide.hide));
                outgoing_view.begin_hide(cx);
                outgoing_view.finish_hide(cx);
            }
        } else {
            self.transition = None;
            if let Some(incoming) = incoming {
                self.release_view(cx, incoming);
            }
            outgoing_ref.show_at_rest(cx);
            cx.widget_action(self.widget_uid(), StackNavigationTransitionAction::InteractivePopCancelled);
        }
        self.redraw(cx);
    }

    fn set_outgoing_slide(&mut self, cx: &mut Cx) {
        let (Some(pop), Some(StackNavigationTransition::Pop { outgoing, .. })) = (&self.interactive_pop, self.transition) else { return };
        let slide = pop.direction * pop.progress.clamp(0.0, 1.0);
        let outgoing_ref = self.stack_view_ref(cx, outgoing);
        let Some(mut outgoing_view) = outgoing_ref.borrow_mut() else { return };
        outgoing_view.view.visible = true;
        outgoing_view.is_moved_by_gesture = true;
        outgoing_view.stop_slide();
        outgoing_view.slide = slide;
        outgoing_view.view.redraw(cx);
    }

    /// The width of the view an interactive pop moves, which its progress is a fraction of.
    fn get_interactive_pop_width(&mut self, cx: &mut Cx) -> f64 {
        let Some(StackNavigationTransition::Pop { outgoing, .. }) = self.transition else { return 1.0 };
        self.stack_view_ref(cx, outgoing).area().rect(cx).size.x.max(1.0)
    }

    /// Whether the current view's Back scope passes `owns`, e.g., `Cx::owns_cancel`.
    fn is_back_owned_by_current_view(&mut self, cx: &mut Cx, owns: fn(&Cx, &CancelScope) -> bool) -> bool {
        let Some(current) = self.current_view else { return false };
        self.stack_view_ref(cx, current).borrow().is_some_and(|view|
            view.cancel_scope.as_ref().is_some_and(|scope| owns(cx, scope))
        )
    }
}

impl StackNavigationRef {
    /// Create a new stack view from a template in `stack_templates`.
    ///
    /// The returned view is registered in the widget tree but is not pushed yet,
    /// which lets callers configure child widgets before starting the push
    /// animation.
    pub fn create_view_from_template(
        &self,
        cx: &mut Cx,
        template_id: LiveId,
    ) -> Option<(LiveId, WidgetRef)> {
        let mut inner = self.borrow_mut()?;
        inner.create_view_from_template(cx, template_id)
    }

    /// Push a view as the next visible stack view.
    ///
    /// StackNavigation only keeps the current visual view and the active
    /// transition view. Callers that need screen history should store that
    /// history separately and call `pop_to_view()` with the screen to reveal.
    ///
    /// # Arguments
    /// * `view_id` - The LiveId of the view to push onto the stack
    ///
    /// # Example
    /// ```ignore
    /// navigation.push(cx, live_id!(settings_view));
    /// ```
    pub fn push(&self, cx: &mut Cx, view_id: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.push_view(view_id, cx);
        }
    }

    pub fn pop(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.pop_to_root(cx);
        }
    }

    /// Pop the current visual view, revealing an already-configured view behind it.
    pub fn pop_to_view(&self, cx: &mut Cx, view_id: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.pop_to_view(cx, Some(view_id));
        }
    }

    /// Pop all views and return to the root view
    ///
    /// This clears the entire navigation stack and returns to the root view.
    /// The current view will slide out with an animation.
    ///
    /// # Example
    /// ```ignore
    /// navigation.pop_to_root(cx);
    /// ```
    pub fn pop_to_root(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.pop_to_root(cx);
        }
    }

    pub fn view_by_id(&self, cx: &mut Cx, view_id: LiveId) -> WidgetRef {
        let Some(mut inner) = self.borrow_mut() else {
            return WidgetRef::empty();
        };
        inner.stack_view_ref(cx, view_id).0.clone()
    }

    pub fn is_transitioning(&self) -> bool {
        self.borrow()
            .as_ref()
            .is_some_and(|inner| inner.transition.is_some())
    }

    /// Whether a swipe or back gesture is moving the current view and hasn't committed its pop yet.
    /// An app that handles Back itself should leave Back to the stack then, since a back gesture ends with one.
    pub fn has_pending_interactive_pop(&self) -> bool {
        self.borrow().as_ref().is_some_and(|inner| matches!(
            inner.interactive_pop,
            Some(InteractivePop {
                phase: InteractivePopPhase::Dragging { .. } | InteractivePopPhase::Releasing { is_commit: false, .. },
                ..
            })
        ))
    }

    /// Reveals the given view beneath the one that a swipe or back gesture is dragging away,
    /// or the root view if `None`. Like with `pop_to_view()`, the view must already be populated.
    pub fn set_interactive_pop_view(&self, cx: &mut Cx, view_id: Option<LiveId>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        let (Some(_), Some(StackNavigationTransition::Pop { incoming, outgoing })) = (&inner.interactive_pop, inner.transition) else { return };
        if view_id == incoming || view_id == Some(outgoing) {
            return;
        }
        if let Some(incoming) = incoming {
            inner.release_view(cx, incoming);
        }
        if let Some(view_id) = view_id {
            inner.show_view_beneath(cx, view_id);
        }
        inner.transition = Some(StackNavigationTransition::Pop { incoming: view_id, outgoing });
        inner.redraw(cx);
    }

    /// Slides the view that a swipe or back gesture is dragging away back into place,
    /// even if the user already let go of it past the threshold.
    pub fn cancel_interactive_pop(&self, cx: &mut Cx) {
        let Some(mut inner) = self.borrow_mut() else { return };
        let Some(pop) = &inner.interactive_pop else { return };
        let is_unmoved = matches!(pop.phase, InteractivePopPhase::Dragging { .. }) && pop.progress == 0.0;
        let is_sliding_back = matches!(pop.phase, InteractivePopPhase::Releasing { is_commit: false, .. });
        // A pop the app refuses before it moves ends right away, so the next gesture asks the app again.
        if is_unmoved {
            inner.end_interactive_pop(cx, false);
        } else if !is_sliding_back {
            inner.release_interactive_pop(cx, false, 0.0);
        }
    }

    /// Ends a swipe or back gesture's pop right away: a committed pop finishes and any other is canceled.
    pub fn end_interactive_pop_without_animation(&self, cx: &mut Cx) {
        let Some(mut inner) = self.borrow_mut() else { return };
        let Some(pop) = inner.interactive_pop.take() else { return };
        match (pop.phase, inner.transition) {
            (InteractivePopPhase::Releasing { is_commit: true, .. }, Some(StackNavigationTransition::Pop { outgoing, .. })) => {
                let outgoing_uid = inner.stack_view_ref(cx, outgoing).widget_uid();
                inner.finish_pop_transition(cx, outgoing_uid);
            }
            _ => inner.end_interactive_pop(cx, false),
        }
    }

    pub fn handle_stack_view_actions(&self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            match action.as_widget_action().cast() {
                StackNavigationAction::Push(view_id) => {
                    self.push(cx, view_id);
                    break;
                }
                StackNavigationAction::Pop => {
                    self.pop(cx);
                    break;
                }
                StackNavigationAction::PopToRoot => {
                    self.pop_to_root(cx);
                    break;
                }
                _ => {}
            }
        }
    }

    /// Set the title of a specific view in the navigation stack.
    ///
    /// The runtime title is stored on the `StackNavigationView` itself and
    /// re-asserted in `on_after_apply` after every kind of re-apply walk
    /// (LiveEdit, `request_script_reapply`, AdaptiveView variant swap on
    /// `WindowGeomChange`, etc.). Callers therefore set it once at push
    /// time and never need to refresh it.
    ///
    /// # Arguments
    /// * `view_id` - The LiveId of the view whose title to set
    /// * `title` - The new title text
    pub fn set_title(&self, cx: &mut Cx, view_id: LiveId, title: &str) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        let stack_view_ref = inner.stack_view_ref(cx, view_id);
        let Some(mut stack_view) = stack_view_ref.borrow_mut() else {
            return;
        };
        stack_view.set_runtime_title(cx, title);
    }

    /// Get the current visual depth.
    pub fn depth(&self) -> usize {
        if let Some(inner) = self.borrow() {
            usize::from(inner.current_view.is_some())
        } else {
            0
        }
    }

    /// Check if there is a current visual view that can be popped to root.
    pub fn can_pop(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.current_view.is_some()
        } else {
            false
        }
    }

    /// Get the current view ID at the top of the stack
    ///
    /// Returns None if the root view is currently showing.
    ///
    /// # Returns
    /// The LiveId of the current view, or None if at root
    pub fn current_view(&self) -> Option<LiveId> {
        if let Some(inner) = self.borrow() {
            inner.current_view
        } else {
            None
        }
    }

    /// Get the view ID that is showing, or that will be once an in-progress
    /// transition finishes.
    ///
    /// Unlike [`Self::current_view`], which only advances once a transition
    /// completes, this reports the destination as soon as it is known. Use it to
    /// address the view that the app considers current, e.g. to update its title.
    ///
    /// Returns None if the root view is (or is becoming) the visible one.
    pub fn destination_view(&self) -> Option<LiveId> {
        let inner = self.borrow()?;
        match inner.transition {
            Some(StackNavigationTransition::Push { incoming, .. }) => Some(incoming),
            // A view that a gesture is dragging away stays the destination until the user commits.
            Some(StackNavigationTransition::Pop { incoming, outgoing }) => match &inner.interactive_pop {
                Some(InteractivePop { phase: InteractivePopPhase::Releasing { is_commit: true, .. }, .. })
                | None => incoming,
                Some(_) => Some(outgoing),
            },
            None => inner.current_view,
        }
    }

    /// Get the view IDs that are currently visible or transitioning.
    pub fn stack_view_ids(&self) -> Vec<LiveId> {
        if let Some(inner) = self.borrow() {
            inner.visible_view_ids()
        } else {
            vec![]
        }
    }

    /// Get every dynamically-created stack view ID retained by this StackNavigation.
    pub fn dynamic_stack_view_ids(&self) -> Vec<LiveId> {
        if let Some(inner) = self.borrow() {
            inner.dynamic_views.iter().map(|(id, _)| *id).collect()
        } else {
            vec![]
        }
    }

    // Legacy methods for backward compatibility
    #[deprecated(note = "Use push() instead")]
    pub fn show_stack_view_by_id(&self, view_id: LiveId, cx: &mut Cx) {
        self.push(cx, view_id);
    }
}

/// Cancels `touch` for every widget that captured it and returns the `FingerCancel` that tells them,
/// so widgets that saw it start let go of it without acting on it.
fn cancel_touch(cx: &mut Cx, e: &TouchUpdateEvent, touch: &TouchPoint) -> Event {
    let digit_id = live_id_num!(touch, touch.uid).into();
    cx.fingers.cancel_digit(digit_id);
    Event::FingerCancel(FingerCancelEvent {
        window_id: e.window_id,
        digit_id,
        device: DigitDevice::Touch { uid: touch.uid },
        abs: touch.abs,
        time: e.time,
        modifiers: e.modifiers,
    })
}
