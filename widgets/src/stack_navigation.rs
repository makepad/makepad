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
    NavigateTo(LiveId)
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
                self.hide_stack_view(cx);
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
}

#[derive(Default)]
enum ActiveStackView {
    #[default]
    None,
    Active(LiveId),
}

#[derive(Live, LiveRegisterWidget, WidgetRef)]
pub struct StackNavigation {
    #[deref]
    view: View,

    #[rust]
    screen_width: f64,

    #[rust]
    active_stack_view: ActiveStackView,
}

impl LiveHook for StackNavigation {
    fn after_apply_from(&mut self, cx: &mut Cx, apply: &mut Apply) {
        if apply.from.is_new_from_doc() {
            self.active_stack_view = ActiveStackView::None;
        } else {
            if let ActiveStackView::Active(stack_view_id) = self.active_stack_view {
                // Make sure current stack view is visible when code reloads
                let stack_view_ref = self.stack_navigation_view(&[stack_view_id]);
                stack_view_ref.apply_over(cx, live! {visible: true, offset: 0.0});
            }
        }
    }
}

impl Widget for StackNavigation {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        for widget_ref in self.get_active_views(cx).iter() {
            widget_ref.handle_event(cx, event, scope);
        }

        // Leaving this to the final step, so that the active stack view can handle the event first.
        // It is relevant when the active stack view is animating out and wants to handle
        // the StackNavigationTransitionAction::HideEnd action.
        self.widget_match_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep  {
        for widget_ref in self.get_active_views(cx.cx).iter() {
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
        for widget_ref in self.get_active_views(cx).iter() {
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
    fn handle_actions(&mut self, _cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        for action in actions {
            // If the window is resized, we need to record the new screen width to
            // fit the transition animation for the new dimensions.
            if let WindowAction::WindowGeomChange(ce) = action.as_widget_action().cast() {
                self.screen_width = ce.new_geom.inner_size.x * ce.new_geom.dpi_factor;
                if let ActiveStackView::Active(stack_view_id) = self.active_stack_view {
                    let stack_view_ref = self.stack_navigation_view(&[stack_view_id]);
                    stack_view_ref.set_offset_to_hide(self.screen_width);
                }
            }

            // If the active stack view is already hidden, we need to reset the active stack view.
            if let StackNavigationTransitionAction::HideEnd = action.as_widget_action().cast() {
                self.active_stack_view = ActiveStackView::None;
            }
        }
    }
}


impl StackNavigation {
    pub fn show_stack_view_by_id(&mut self, stack_view_id: LiveId, cx: &mut Cx) {
        if let ActiveStackView::None = self.active_stack_view {
            let stack_view_ref = self.stack_navigation_view(&[stack_view_id]);
            stack_view_ref.show(cx, self.screen_width);
            self.active_stack_view = ActiveStackView::Active(stack_view_id);

            // Send a `Show` action to the view being shown so it can be aware of the transition.
            cx.widget_action(
                stack_view_ref.widget_uid(),
                &HeapLiveIdPath::default(),
                StackNavigationTransitionAction::ShowBegin,
            );

            self.redraw(cx);
        }
    }

    fn get_active_views(&mut self, cx: &mut Cx) -> Vec<WidgetRef> {
        match self.active_stack_view {
            ActiveStackView::None => {
                vec![self.view.widget(id!(root_view))]
            },
            ActiveStackView::Active(stack_view_id) => {
                let stack_view_ref = self.stack_navigation_view(&[stack_view_id]);
                let mut views = vec![];

                if stack_view_ref.is_showing(cx) {
                    if stack_view_ref.is_animating(cx) {
                        views.push(self.view.widget(id!(root_view)));
                    }
                }

                views.push(stack_view_ref.0.clone());
                views
            }
        }
    }
}

impl StackNavigationRef {
    pub fn show_stack_view_by_id(&self, stack_view_id: LiveId, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_stack_view_by_id(stack_view_id, cx);
        }
    }

    pub fn handle_stack_view_actions(&self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let StackNavigationAction::NavigateTo(stack_view_id) = action.as_widget_action().cast() {
                self.show_stack_view_by_id(stack_view_id, cx);
                break;
            }
        }
    }

    pub fn set_title(&self, cx:&mut Cx, stack_view_id: LiveId, title: &str) {
        if let Some(inner) = self.borrow_mut() {
            let stack_view_ref = inner.stack_navigation_view(&[stack_view_id]);
            stack_view_ref.label(id!(title)).set_text(cx, title);
        }
    }
}
