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

live_design! {
    StackNavigationViewBase = {{StackNavigationView}} {}
    StackNavigationBase = {{StackNavigation}} {}
}

#[derive(Clone, DefaultNone, Eq, Hash, PartialEq, Debug)]
pub enum StackNavigationAction {
    None,
    NavigateTo(LiveId)
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
}

impl Widget for StackNavigationView {
    fn handle_event(&mut self, cx:&mut Cx, event:&Event, scope:&mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.view.redraw(cx);
            dbg!(self.offset);
        }

        self.view.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope);

        if self.animator.animator_in_state(cx, id!(slide.hide))
        {
            if self.offset > self.offset_to_hide {
                self.animator_cut(cx, id!(slide.hide));
            }
            if !self.animator.is_track_animating(cx, id!(slide)) {
                self.apply_over(cx, live! {visible: false});
            }
        }
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

impl WidgetMatchEvent for StackNavigationView {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        if self.button(id!(left_button)).clicked(&actions) {
            self.animator_play(cx, id!(slide.hide));
        }
    }
}

impl StackNavigationViewRef {
    pub fn show(&mut self, cx: &mut Cx, root_width: f64) {
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

    pub fn set_offset_to_hide(&mut self, offset_to_hide: f64) {
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
        self.widget_match_event(cx, event, scope);
        for widget_ref in self.get_active_views(cx).iter() {
            widget_ref.handle_event(cx, event, scope);
        }
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

    fn redraw(&mut self, cx: &mut Cx) {
        for widget_ref in self.get_active_views(cx).iter() {
            widget_ref.redraw(cx);
        }
    }
    
    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.view.find_widgets(path, cached, results);
    }
}

impl WidgetMatchEvent for StackNavigation {
    fn handle_actions(&mut self, _cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        for action in actions {
            if let WindowAction::WindowGeomChange(ce) = action.as_widget_action().cast() {
                self.screen_width = ce.new_geom.inner_size.x;
                if let ActiveStackView::Active(stack_view_id) = self.active_stack_view {
                    let mut stack_view_ref = self.stack_navigation_view(&[stack_view_id]);
                    stack_view_ref.set_offset_to_hide(self.screen_width);
                }
            }
        }
    }
}


impl StackNavigation {
    pub fn show_stack_view_by_id(&mut self, stack_view_id: LiveId, cx: &mut Cx) {
        if let ActiveStackView::None = self.active_stack_view {
            let mut stack_view_ref = self.stack_navigation_view(&[stack_view_id]);
            stack_view_ref.show(cx, self.screen_width);
            self.active_stack_view = ActiveStackView::Active(stack_view_id);
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
                    views.push(stack_view_ref.0.clone());
                    views
                } else {
                    self.active_stack_view = ActiveStackView::None;
                    vec![self.view.widget(id!(root_view))]
                }
            }
        }
    }
}

impl StackNavigationRef {
    pub fn show_stack_view_by_id(&mut self, stack_view_id: LiveId, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_stack_view_by_id(stack_view_id, cx);
        }
    }

    pub fn handle_stack_view_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let StackNavigationAction::NavigateTo(stack_view_id) = action.as_widget_action().cast() {
                self.show_stack_view_by_id(stack_view_id, cx);
                break;
            }
        }
    }

    pub fn set_title(&self, stack_view_id: LiveId, title: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            let stack_view_ref = inner.stack_navigation_view(&[stack_view_id]);
            stack_view_ref.label(id!(title)).set_text(title);
        }
    }
}
