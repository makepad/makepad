use crate::{
    animator::*, button::*, label::*, makepad_derive_widget::*, makepad_draw::*, view::*,
    widget::*, widget_match_event::WidgetMatchEvent, widget_tree::CxWidgetExt,
    window::WindowAction,
};

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
                        svg: crate_resource("self:resources/icons/back.svg")
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

        // Empty slot to place a generic full-screen background
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

        offset: 4000.0

        animator: Animator{
            slide: {
                default: @hide
                hide: AnimatorState{
                    redraw: true
                    ease: Ease.ExpDecay{d1: 0.80 d2: 0.97}
                    from: {all: Play.Forward{duration: 5.0}}
                    apply: {offset: 4000.0}
                }

                show: AnimatorState{
                    redraw: true
                    ease: Ease.ExpDecay{d1: 0.82 d2: 0.95}
                    from: {all: Play.Forward{duration: 0.5}}
                    apply: {offset: 0.0}
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
}

#[derive(Script, Widget, Animator)]
pub struct StackNavigationView {
    #[source]
    source: ScriptObjectRef,

    #[deref]
    view: View,

    /// The offset of the stack view from the left edge of the parent view.
    #[live]
    offset: f64,

    /// Whether the stack view should take over the entire screen.
    ///
    /// If false, the stack view will be constrained to the size of the parent view,
    /// and no animations will be played when navigating.
    #[live(true)]
    full_screen: bool,

    /// The offset of the stack view from the left edge of the parent view when it is fully hidden.
    #[rust(10000.0)]
    offset_to_hide: f64,

    #[apply_default]
    animator: Animator,

    /// The state of the stack view.
    #[rust]
    state: StackNavigationViewState,

    /// The UID of the parent navigation.
    #[rust]
    parent_navigation_uid: Option<WidgetUid>,

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
        if self.animator_handle_event(cx, event).must_redraw() {
            self.view.redraw(cx);
        }
        self.view.handle_event(cx, event, scope);

        self.handle_stack_view_closure_request(cx, event, scope);
        self.trigger_action_post_opening_if_done(cx);
        self.finish_closure_animation_if_done(cx);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let parent_rect = cx.peek_walk_turtle(walk);
        let abs_pos = if self.full_screen {
            // In full screen mode, position at the offset.
            // Use the larger of the safe area inset or the parent's y position
            // so the view respects both mobile safe areas and desktop title bars.
            let safe_top = cx.display_context.safe_area_insets.top;
            Vec2d {
                x: self.offset,
                y: safe_top.max(parent_rect.pos.y),
            }
        } else {
            // Non-fullscreen: ignore offset, position at parent.
            Vec2d {
                x: parent_rect.pos.x,
                y: parent_rect.pos.y,
            }
        };

        self.view.draw_walk(cx, scope, walk.with_abs_pos(abs_pos))
    }
}

impl StackNavigationView {
    fn hide_stack_view(&mut self, cx: &mut Cx) {
        if self.full_screen {
            self.animator_play(cx, ids!(slide.hide));
        } else {
            // Non-fullscreen: cut instantly (no animation).
            self.animator_cut(cx, ids!(slide.hide));
        }

        cx.widget_action(
            self.widget_uid(),
            StackNavigationTransitionAction::HideBegin,
        );
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
        if matches!(self.state, StackNavigationViewState::Active) {
            if event.back_pressed()
                || matches!(event, Event::Actions(actions) if self.button(cx, ids!(left_button)).clicked(&actions))
                || matches!(event, Event::MouseUp(mouse) if mouse.button.is_back())
            {
                cx.widget_action(self.widget_uid(), StackNavigationAction::Pop);
            }
        }
    }

    fn finish_closure_animation_if_done(&mut self, cx: &mut Cx) {
        if self.state == StackNavigationViewState::Active
            && self.animator.in_state(cx, ids!(slide.hide))
        {
            if self.offset > self.offset_to_hide {
                self.view.visible = false;
                self.redraw(cx);

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

                self.animator_cut(cx, ids!(slide.hide));
                self.state = StackNavigationViewState::Inactive;
            }
        }
    }

    fn trigger_action_post_opening_if_done(&mut self, cx: &mut Cx) {
        if self.state == StackNavigationViewState::Inactive
            && self.animator.in_state(cx, ids!(slide.show))
        {
            const OPENING_OFFSET_THRESHOLD: f64 = 0.5;
            // Non-fullscreen: consider fully opened immediately (offset ignored in draw_walk).
            if self.offset < OPENING_OFFSET_THRESHOLD || !self.full_screen {
                cx.widget_action(self.widget_uid(), StackNavigationTransitionAction::ShowDone);
                self.state = StackNavigationViewState::Active;
            }
        }
    }

    fn is_animating(&self) -> bool {
        self.animator.is_track_animating(live_id!(slide))
    }
}

impl StackNavigationViewRef {
    pub fn show(&self, cx: &mut Cx, root_width: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.view.visible = true;
            inner.offset_to_hide = root_width;
            inner.state = StackNavigationViewState::Inactive;

            // Force-reset the animator by cutting to show (offset=0) first,
            // then cutting to hide, then playing show. This ensures the animator
            // always sees a state change regardless of its current state.
            inner.animator_cut(cx, ids!(slide.show)); // force to show state
            inner.animator_cut(cx, ids!(slide.hide)); // then to hide state
            inner.offset = root_width; // set actual start offset
            inner.animator_play(cx, ids!(slide.show)); // now animate hide -> show
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

    pub fn set_offset_to_hide(&self, offset_to_hide: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.offset_to_hide = offset_to_hide;
        }
    }

    pub fn hide(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.hide_stack_view(cx);
        }
    }

    pub fn set_parent_navigation_uid(&self, parent_uid: WidgetUid) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.parent_navigation_uid = Some(parent_uid);
        }
    }
}

#[derive(Clone, Debug)]
struct StackEntry {
    view_id: LiveId,
}

#[derive(Default)]
struct NavigationStack {
    stack: Vec<StackEntry>,
}

impl NavigationStack {
    fn push(&mut self, view_id: LiveId) {
        self.stack.push(StackEntry { view_id });
    }

    fn pop(&mut self) -> Option<StackEntry> {
        self.stack.pop()
    }

    fn current(&self) -> Option<&StackEntry> {
        self.stack.last()
    }

    fn previous(&self) -> Option<&StackEntry> {
        if self.stack.len() >= 2 {
            self.stack.get(self.stack.len() - 2)
        } else {
            None
        }
    }

    fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    fn len(&self) -> usize {
        self.stack.len()
    }

    fn clear(&mut self) {
        self.stack.clear();
    }

    #[allow(dead_code)]
    fn remove_all(&mut self, view_id: LiveId) {
        self.stack.retain(|entry| entry.view_id != view_id);
    }

    fn view_ids(&self) -> Vec<LiveId> {
        self.stack.iter().map(|entry| entry.view_id).collect()
    }
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct StackNavigation {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    #[rust]
    screen_width: f64,

    #[rust]
    navigation_stack: NavigationStack,
}

impl ScriptHook for StackNavigation {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_new() {
            self.navigation_stack = NavigationStack::default();
        } else if apply.is_reload() {
            // Reload re-applies every DSL value across the whole tree, which
            // resets `visible: false` and `offset: 4000.0` (the StackNavigationView
            // defaults) on every stack view we've previously pushed — not just
            // the topmost. Restore visibility/offset for every entry in the
            // history so popping back to a view underneath the current one
            // reveals it instead of the parent's bare background.
            //
            // This matters on rotation specifically: a safe-area-inset change
            // calls `cx.request_live_edit()` (see window.rs), which fires this
            // reload pass. Before this loop existed, rotating with several
            // rooms pushed and then tapping back showed a blank screen because
            // the previous room view was still `visible = false`.
            let view_ids = self.navigation_stack.view_ids();
            for view_id in view_ids {
                let stack_view_ref = self.stack_navigation_view(_vm.cx_mut(), &[view_id]);
                if let Some(mut inner) = stack_view_ref.borrow_mut() {
                    inner.view.visible = true;
                    inner.offset = 0.0;
                };
            }
        }
    }
}

impl Widget for StackNavigation {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
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
            if let WindowAction::WindowGeomChange(ce) = action.as_widget_action().cast() {
                self.screen_width = ce.new_geom.inner_size.x * ce.new_geom.dpi_factor;
                // Refresh `offset_to_hide` on every stack view we know about,
                // not just the current one. Previously-pushed views hold the
                // screen width that was current when they were pushed, so
                // after a rotation the slide-out animation on those views
                // would terminate at the old (e.g. portrait) edge instead of
                // sliding fully off the new (e.g. landscape) edge.
                let view_ids = self.navigation_stack.view_ids();
                for view_id in view_ids {
                    let stack_view_ref = self.stack_navigation_view(cx, &[view_id]);
                    stack_view_ref.set_offset_to_hide(self.screen_width);
                }
            }

            if let Some(widget_action) = action.as_widget_action() {
                if !cx.widget_tree().widget(widget_action.widget_uid).is_empty() {
                    match widget_action.cast() {
                        StackNavigationAction::Push(view_id) => {
                            self.push_view(view_id, cx);
                        }
                        StackNavigationAction::Pop => {
                            self.pop_view(cx);
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
                            self.navigation_stack.pop();
                        }
                    }
                }
            }
        }
    }
}

impl StackNavigation {
    fn push_view(&mut self, view_id: LiveId, cx: &mut Cx) {
        self.navigation_stack.push(view_id);

        let stack_view_ref = self.stack_navigation_view(cx, &[view_id]);
        stack_view_ref.set_parent_navigation_uid(self.widget_uid());
        stack_view_ref.show(cx, self.screen_width);

        cx.widget_action(
            stack_view_ref.widget_uid(),
            StackNavigationTransitionAction::ShowBegin,
        );

        self.redraw(cx);
    }

    fn pop_view(&mut self, cx: &mut Cx) {
        if let Some(current_entry) = self.navigation_stack.current() {
            let current_view_ref = self.stack_navigation_view(cx, &[current_entry.view_id]);
            current_view_ref.hide(cx);
        }
        self.redraw(cx);
    }

    /// Ensures the given view is visible and at offset 0 with Active state.
    /// This is needed when a view that was reused at a deeper stack depth
    /// (and subsequently hidden when that depth was popped) becomes the
    /// current view again after further pops.
    /// Ensures the given view is visible and at offset 0 with Active state.
    fn pop_to_root(&mut self, cx: &mut Cx) {
        if let Some(current_entry) = self.navigation_stack.current() {
            let stack_view_ref = self.stack_navigation_view(cx, &[current_entry.view_id]);
            stack_view_ref.hide(cx);
            self.navigation_stack.clear();
        }
        self.redraw(cx);
    }

    /// Returns the views that are currently visible.
    ///
    /// This includes up to two views, in this order:
    /// 1. The previous view (root_view or previous stack view), if the current view is animating and partially showing,
    /// 2. The current stack view, if it exists and is partially or fully showing,
    ///   or if there is no current stack view at all (showing root_view).
    fn get_visible_views(&mut self, cx: &mut Cx) -> Vec<(LiveId, WidgetRef)> {
        match self.navigation_stack.current() {
            None => {
                // No views in stack, show root view
                vec![(live_id!(root_view), self.view.widget(cx, ids!(root_view)))]
            }
            Some(current_entry) => {
                let current_view_id = current_entry.view_id;
                let current_view_ref = self.stack_navigation_view(cx, &[current_view_id]);
                let mut views = vec![];

                // If current view is showing and animating, we need to show the previous view behind it
                if current_view_ref.is_showing(cx) && current_view_ref.is_animating() {
                    if let Some(previous_entry) = self.navigation_stack.previous() {
                        // Show the previous stack view
                        let previous_view_id = previous_entry.view_id;
                        let previous_view_ref = self.stack_navigation_view(cx, &[previous_view_id]);
                        views.push((previous_view_id, previous_view_ref.0.clone()));
                    } else {
                        // Show the root view if there's no previous stack view
                        views.push((live_id!(root_view), self.view.widget(cx, ids!(root_view))));
                    }
                }

                // Always add the current view
                views.push((current_view_id, current_view_ref.0.clone()));
                views
            }
        }
    }
}

impl StackNavigationRef {
    /// Push a new view onto the navigation stack
    ///
    /// This is the primary method for navigating to a new view.
    /// The view will slide in with an animation.
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

    /// Pop the current view from the navigation stack
    ///
    /// This removes the current view and returns to the previous view.
    /// If there's no previous view, it returns to the root view.
    /// The current view will slide out with an animation.
    ///
    /// # Example
    /// ```ignore
    /// navigation.pop(cx);
    /// ```
    pub fn pop(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.pop_view(cx);
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
        let Some(inner) = self.borrow_mut() else {
            return;
        };
        let stack_view_ref = inner.stack_navigation_view(cx, &[view_id]);
        let Some(mut stack_view) = stack_view_ref.borrow_mut() else {
            return;
        };
        stack_view.set_runtime_title(cx, title);
    }

    /// Get the current depth of the navigation stack
    ///
    /// Returns 0 if only the root view is showing, 1 if there's one view
    /// pushed onto the stack, etc.
    ///
    /// # Returns
    /// The number of views currently in the navigation stack
    pub fn depth(&self) -> usize {
        if let Some(inner) = self.borrow() {
            inner.navigation_stack.len()
        } else {
            0
        }
    }

    /// Check if navigation back is possible
    ///
    /// Returns true if there are views in the stack that can be popped.
    ///
    /// # Returns
    /// true if pop() would do something, false if already at root
    pub fn can_pop(&self) -> bool {
        if let Some(inner) = self.borrow() {
            !inner.navigation_stack.is_empty()
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
            inner.navigation_stack.current().map(|entry| entry.view_id)
        } else {
            None
        }
    }

    /// Get all view IDs in the current navigation stack
    ///
    /// Returns a vector of LiveIds representing the navigation history,
    /// with the first element being the oldest (bottom of stack) and
    /// the last element being the current view (top of stack).
    ///
    /// # Returns
    /// Vector of LiveIds in the navigation stack
    pub fn stack_view_ids(&self) -> Vec<LiveId> {
        if let Some(inner) = self.borrow() {
            inner.navigation_stack.view_ids()
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
