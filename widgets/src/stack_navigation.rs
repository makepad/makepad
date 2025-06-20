use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    label::*,
    button::*,
    view::*,
    WidgetMatchEvent,
    WindowAction,
};

live_design!{
    link widgets;
    use link::widgets::*;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    pub StackNavigationViewBase = {{StackNavigationView}} {}
    pub StackNavigationBase = {{StackNavigation}} {}
    
    // StackView DSL begin
    
    HEADER_HEIGHT = 80.0
    
    pub StackViewHeader = <View> {
        width: Fill, height: (HEADER_HEIGHT),
        padding: {bottom: 10., top: 50.}
        show_bg: true
        draw_bg: {
            color: (THEME_COLOR_APP_CAPTION_BAR)
        }
        
        content = <View> {
            width: Fill, height: Fit,
            flow: Overlay,
            
            title_container = <View> {
                width: Fill, height: Fit,
                align: {x: 0.5, y: 0.5}
                
                title = <H4> {
                    width: Fit, height: Fit,
                    margin: 0,
                    text: "Stack View Title"
                }
            }
            
            button_container = <View> {
                left_button = <Button> {
                    width: Fit, height: 68,
                    icon_walk: {width: 10, height: 68}
                    draw_bg: {
                        fn pixel(self) -> vec4 {
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                            return sdf.result
                        }
                    }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/back.svg"),
                        color: (THEME_COLOR_LABEL_INNER);
                        brightness: 0.8;
                    }
                }
            }
        }
    }
    
    pub StackNavigationView = <StackNavigationViewBase> {
        visible: false
        width: Fill, height: Fill,
        flow: Overlay
        
        show_bg: true
        draw_bg: {
            color: (THEME_COLOR_WHITE)
        }
        
        // Empty slot to place a generic full-screen background
        background = <View> {
            width: Fill, height: Fill,
            visible: false
        }
        
        body = <View> {
            width: Fill, height: Fill,
            flow: Down,
            
            // THEME_SPACE between body and header can be adjusted overriding this margin
            margin: {top: (HEADER_HEIGHT)},
        }
        
        header = <StackViewHeader> {}
        
        offset: 4000.0
        
        animator: {
            slide = {
                default: hide,
                hide = {
                    redraw: true
                    ease: ExpDecay {d1: 0.80, d2: 0.97}
                    from: {all: Forward {duration: 5.0}}
                    // Large enough number to cover several screens,
                    // but we need a way to parametrize it
                    apply: {offset: 4000.0}
                }
                
                show = {
                    redraw: true
                    ease: ExpDecay {d1: 0.82, d2: 0.95}
                    from: {all: Forward {duration: 0.5}}
                    apply: {offset: 0.0}
                }
            }
        }
    }
    
    pub StackNavigation = <StackNavigationBase> {
        width: Fill, height: Fill
        flow: Overlay
        
        root_view = <View> {}
    }
    
}

#[derive(Clone, DefaultNone, Eq, Hash, PartialEq, Debug)]
pub enum StackNavigationAction {
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
    #[default] Inactive,
    Active,
}

/// Actions that are delivered to an incoming or outgoing "active" widget/view
/// within a stack navigation container.
#[derive(Clone, DefaultNone, Eq, Hash, PartialEq, Debug)]
pub enum StackNavigationTransitionAction {
    None,
    ShowBegin,
    ShowDone,
    HideBegin,
    HideEnd,
}

#[derive(Live, LiveHook, Widget)]
pub struct StackNavigationView {
    #[deref]
    view: View,

    #[live]
    offset: f64,

    #[rust(10000.0)]
    offset_to_hide: f64,

    #[animator]
    animator: Animator,

    #[rust]
    state: StackNavigationViewState,
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

    fn draw_walk(&mut self, cx:&mut Cx2d, scope:&mut Scope, walk:Walk) -> DrawStep{
        self.view.draw_walk(
            cx,
            scope,
            walk.with_abs_pos(DVec2 {
                x: self.offset,
                y: 0.,
            }),
        )
    }
}

impl StackNavigationView {
    fn hide_stack_view(&mut self, cx: &mut Cx) {
        self.animator_play(cx, id!(slide.hide));
        cx.widget_action(
            self.widget_uid(),
            &HeapLiveIdPath::default(),
            StackNavigationTransitionAction::HideBegin,
        );
    }

    fn handle_stack_view_closure_request(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // Hide the active stack view if:
        // * the back navigation button/gesture occurred,
        // * the left_button was clicked,
        // * the "back" button on the mouse was clicked.
        // TODO: in the future, handle a swipe right gesture on touchscreen, or two-finger swipe on trackpad
        if matches!(self.state, StackNavigationViewState::Active) {
            if event.back_pressed()
                || matches!(event, Event::Actions(actions) if self.button(id!(left_button)).clicked(&actions))
                || matches!(event, Event::MouseUp(mouse) if mouse.button.is_back())
            {
                cx.widget_action(
                    self.widget_uid(),
                    &HeapLiveIdPath::default(),
                    StackNavigationAction::Pop,
                );
            }
        }
    }

    fn finish_closure_animation_if_done(&mut self, cx: &mut Cx) {
        if self.state == StackNavigationViewState::Active
            && self.animator.animator_in_state(cx, id!(slide.hide))
        {
            if self.offset > self.offset_to_hide {
                self.apply_over(cx, live! { visible: false });

                cx.widget_action(
                    self.widget_uid(),
                    &HeapLiveIdPath::default(),
                    StackNavigationTransitionAction::HideEnd,
                );

                self.animator_cut(cx, id!(slide.hide));
                self.state = StackNavigationViewState::Inactive;
            }
        }
    }

    fn trigger_action_post_opening_if_done(&mut self, cx: &mut Cx) {
        if self.state == StackNavigationViewState::Inactive &&
            self.animator.animator_in_state(cx, id!(slide.show))
        {
            const OPENING_OFFSET_THRESHOLD: f64 = 0.5;
            if self.offset < OPENING_OFFSET_THRESHOLD {
                cx.widget_action(
                    self.widget_uid(),
                    &HeapLiveIdPath::default(),
                    StackNavigationTransitionAction::ShowDone,
                );
                self.state = StackNavigationViewState::Active;
            }
        }
    }
}

impl StackNavigationViewRef {
    pub fn show(&self, cx: &mut Cx, root_width: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.apply_over(cx, live! {offset: (root_width), visible: true});
            inner.offset_to_hide = root_width;
            inner.animator_play(cx, id!(slide.show));
        }
    }

    pub fn is_showing(&self, cx: &mut Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.animator.animator_in_state(cx, id!(slide.show))
                || inner.animator.is_track_animating(cx, id!(slide))
        } else {
            false
        }
    }

    pub fn is_animating(&self, cx: &mut Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.animator.is_track_animating(cx, id!(slide))
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

    // Remove all instances of a view from the stack (useful for preventing cycles)
    fn remove_all(&mut self, view_id: LiveId) {
        self.stack.retain(|entry| entry.view_id != view_id);
    }

    // Get all view IDs in the stack (for debugging/inspection)
    fn view_ids(&self) -> Vec<LiveId> {
        self.stack.iter().map(|entry| entry.view_id).collect()
    }
}

#[derive(Live, LiveRegisterWidget, WidgetRef)]
pub struct StackNavigation {
    #[deref]
    view: View,

    #[rust]
    screen_width: f64,

    #[rust]
    navigation_stack: NavigationStack,
}

impl LiveHook for StackNavigation {
    fn after_apply_from(&mut self, cx: &mut Cx, apply: &mut Apply) {
        if apply.from.is_new_from_doc() {
            self.navigation_stack = NavigationStack::default();
        } else {
            // Make sure current stack view is visible when code reloads
            if let Some(current_entry) = self.navigation_stack.current() {
                let stack_view_ref = self.stack_navigation_view(&[current_entry.view_id]);
                stack_view_ref.apply_over(cx, live! {visible: true, offset: 0.0});
            }
        }
    }
}

impl Widget for StackNavigation {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // If the event requires visibility, only forward it to the visible views.
        // If the event does not require visibility, forward it to all views,
        // ensuring that we don't forward it to the root view twice.
        let mut visible_views = self.get_visible_views(cx);
        if !event.requires_visibility() {
            let root_view = self.view.widget(id!(root_view));
            if !visible_views.contains(&root_view) {
                visible_views.insert(0, root_view);
            }
        }
        for widget_ref in visible_views {
            widget_ref.handle_event(cx, event, scope);
        }

        // Leaving this to the final step, so that the active stack view can handle the event first.
        // It is relevant when the active stack view is animating out and wants to handle
        // the StackNavigationTransitionAction::HideEnd action.
        self.widget_match_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep  {
        for widget_ref in self.get_visible_views(cx.cx).iter() {
            widget_ref.draw_walk(cx, scope, walk) ?;
        }
        DrawStep::done()
    }
}

impl WidgetNode for StackNavigation {
    fn walk(&mut self, cx:&mut Cx) -> Walk{
        self.view.walk(cx)
    }
    fn area(&self)->Area{self.view.area()}
    
    fn redraw(&mut self, cx: &mut Cx) {
        for widget_ref in self.get_visible_views(cx).iter() {
            widget_ref.redraw(cx);
        }
    }

    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.view.find_widgets(path, cached, results);
    }
    
    fn uid_to_widget(&self, uid:WidgetUid)->WidgetRef{
        self.view.uid_to_widget(uid)
    }
    
}

impl WidgetMatchEvent for StackNavigation {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        for action in actions {
            // If the window is resized, we need to record the new screen width to
            // fit the transition animation for the new dimensions.
            if let WindowAction::WindowGeomChange(ce) = action.as_widget_action().cast() {
                self.screen_width = ce.new_geom.inner_size.x * ce.new_geom.dpi_factor;
                if let Some(current_entry) = self.navigation_stack.current() {
                    let stack_view_ref = self.stack_navigation_view(&[current_entry.view_id]);
                    stack_view_ref.set_offset_to_hide(self.screen_width);
                }
            }

            // Handle navigation actions
            match action.as_widget_action().cast() {
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

            // If the active stack view is already hidden, we need to update the navigation stack.
            if let StackNavigationTransitionAction::HideEnd = action.as_widget_action().cast() {
                // The current view has finished hiding, so we can remove it from the stack
                self.navigation_stack.pop();
            }
        }
    }
}


impl StackNavigation {
    fn push_view(&mut self, view_id: LiveId, cx: &mut Cx) {
        // Prevent cycles by removing any existing instances of this view from the stack
        self.navigation_stack.remove_all(view_id);
        
        // Add the new view to the stack
        self.navigation_stack.push(view_id);

        let stack_view_ref = self.stack_navigation_view(&[view_id]);
        stack_view_ref.show(cx, self.screen_width);

        // Send a `Show` action to the view being shown so it can be aware of the transition.
        cx.widget_action(
            stack_view_ref.widget_uid(),
            &HeapLiveIdPath::default(),
            StackNavigationTransitionAction::ShowBegin,
        );

        self.redraw(cx);
    }

    fn pop_view(&mut self, cx: &mut Cx) {
        if let Some(current_entry) = self.navigation_stack.current() {
            let current_view_ref = self.stack_navigation_view(&[current_entry.view_id]);
            current_view_ref.hide(cx);
        }
        self.redraw(cx);
    }

    fn pop_to_root(&mut self, cx: &mut Cx) {
        if let Some(current_entry) = self.navigation_stack.current() {
            let stack_view_ref = self.stack_navigation_view(&[current_entry.view_id]);
            stack_view_ref.hide(cx);
            // Clear the entire stack to go back to root
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
    fn get_visible_views(&mut self, cx: &mut Cx) -> Vec<WidgetRef> {
        match self.navigation_stack.current() {
            None => {
                // No views in stack, show root view
                vec![self.view.widget(id!(root_view))]
            },
            Some(current_entry) => {
                let current_view_ref = self.stack_navigation_view(&[current_entry.view_id]);
                let mut views = vec![];

                // If current view is showing and animating, we need to show the previous view behind it
                if current_view_ref.is_showing(cx) && current_view_ref.is_animating(cx) {
                    if let Some(previous_entry) = self.navigation_stack.previous() {
                        // Show the previous stack view
                        let previous_view_ref = self.stack_navigation_view(&[previous_entry.view_id]);
                        views.push(previous_view_ref.0.clone());
                    } else {
                        // Show the root view if there's no previous stack view
                        views.push(self.view.widget(id!(root_view)));
                    }
                }

                // Always add the current view
                views.push(current_view_ref.0.clone());
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
    /// ```rust
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
    /// ```rust
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
    /// ```rust
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

    /// Set the title of a specific view in the navigation stack
    /// 
    /// # Arguments
    /// * `view_id` - The LiveId of the view whose title to set
    /// * `title` - The new title text
    pub fn set_title(&self, cx: &mut Cx, view_id: LiveId, title: &str) {
        if let Some(inner) = self.borrow_mut() {
            let stack_view_ref = inner.stack_navigation_view(&[view_id]);
            stack_view_ref.label(id!(title)).set_text(cx, title);
        }
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
