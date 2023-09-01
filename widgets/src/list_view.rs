
use {
    crate::{
        widget::*,
        makepad_derive_widget::*,
        makepad_draw::*,
        scroll_bar::{ScrollBar, ScrollBarAction}
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    import makepad_widgets::view::View;
    ListView = {{ListView}} {
        width: Fill
        height: Fill
        capture_overload: true
        flow: Down
        Entry = <View> {}
    }
}

enum DragState {
    None,
    // Swipe gesture recorded by a very short amount of time, determining the flick scroll direction and speed
    SwipeDrag {last_abs: f64, delta: f64, initial_time: f64},
    // If it is a longer tap, it is considered a normal drag
    NormalDrag {last_abs: f64, delta: f64}
}

enum ScrollState {
    Stopped,
    Flick {delta: f64, next_frame: NextFrame}
}

#[derive(Live)]
pub struct ListView {
    #[rust] area: Area,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    
    #[rust] range_start: u64,
    #[rust(u64::MAX)] range_end: u64,
    #[rust(10u64)] view_window: u64,
    #[live(0.1)] flick_scroll_minimum: f64,
    #[live(0.98)] flick_scroll_decay: f64,
    #[live(0.2)] swipe_drag_duration: f64,
    #[rust] first_id: u64,
    #[rust] first_scroll: f64,
    #[rust(Vec2Index::X)] vec_index: Vec2Index,
    #[live] scroll_bar: ScrollBar,
    #[live] capture_overload: bool,
    #[rust] draw_state: DrawStateWrap<ListDrawState>,
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
    #[rust] items: ComponentMap<(u64, LiveId), WidgetRef>,
    #[rust(DragState::None)] drag_state: DragState,
    #[rust(ScrollState::Stopped)] scroll_state: ScrollState
}

impl LiveHook for ListView {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, ListView)
    }
    
    fn before_apply(&mut self, _cx: &mut Cx, from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc {..} = from {
            self.templates.clear();
        }
    }
    
    // hook the apply flow to collect our templates and apply to instanced childnodes
    fn apply_value_instance(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let id = nodes[index].id;
        match from {
            ApplyFrom::NewFromDoc {file_id} | ApplyFrom::UpdateFromDoc {file_id} => {
                if nodes[index].origin.has_prop_type(LivePropType::Instance) {
                    let live_ptr = cx.live_registry.borrow().file_id_index_to_live_ptr(file_id, index);
                    self.templates.insert(id, live_ptr);
                    // lets apply this thing over all our childnodes with that template
                    for ((_, templ_id), node) in self.items.iter_mut() {
                        if *templ_id == id {
                            node.apply(cx, from, index, nodes);
                        }
                    }
                }
                else {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                }
            }
            _ => ()
        }
        nodes.skip_node(index)
    }
    
    fn after_apply(&mut self, _cx: &mut Cx, _from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if let Flow::Down = self.layout.flow {
            self.vec_index = Vec2Index::Y
        }
        else {
            self.vec_index = Vec2Index::X
        }
    }
}

impl ListView {
    
    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, self.layout);
    }
    
    fn end(&mut self, cx: &mut Cx2d) {
        let rect = cx.turtle().rect();
        let total_views = (self.range_end - self.range_start) as f64 / self.view_window as f64;
        self.scroll_bar.draw_scroll_bar(cx, Axis::Vertical, rect, dvec2(100.0, rect.size.y * total_views));
        self.items.retain_visible();
        cx.end_turtle_with_area(&mut self.area);
    }
    
    pub fn next_visible_item(&mut self, cx: &mut Cx2d) -> Option<u64> {
        let vi = self.vec_index;
        match self.draw_state.get() {
            Some(ListDrawState::Begin) => {
                let viewport = cx.turtle().padded_rect();
                self.draw_state.set(ListDrawState::Down {
                    index: self.first_id,
                    scroll: self.first_scroll,
                    viewport,
                });
                
                cx.begin_turtle(Walk {
                    abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y) + DVec2::from_index_pair(vi, self.first_scroll, 0.0)),
                    margin: Default::default(),
                    width: Size::Fill,
                    height: Size::Fit
                }, Layout::flow_down());
                return Some(self.first_id)
            }
            Some(ListDrawState::Down {index, scroll, viewport}) => {
                let did_draw = cx.turtle_has_align_items();
                let rect = cx.end_turtle();
                
                if did_draw && rect.pos.index(vi) + rect.size.index(vi) < viewport.pos.index(vi) && index + 1 < self.range_end {
                    self.first_id = index + 1;
                    self.first_scroll = (rect.pos.index(vi) + rect.size.index(vi)) - viewport.pos.index(vi);
                    if self.first_id + 1 == self.range_end && self.first_scroll < 0.0 {
                        self.first_scroll = 0.0;
                    }
                }
                
                if !did_draw
                    || rect.pos.index(vi) + rect.size.index(vi) > viewport.pos.index(vi) + viewport.size.index(vi)
                /* || index + 1 == self.range_end*/ {
                    if self.first_id > self.range_start && self.first_scroll > 0.0 {
                        self.draw_state.set(ListDrawState::Up {
                            index: self.first_id - 1,
                            scroll: self.first_scroll,
                            viewport
                        });
                        cx.begin_turtle(Walk {
                            abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y)),
                            margin: Default::default(),
                            width: Size::Fill,
                            height: Size::Fit
                        }, Layout::flow_down());
                        return Some(self.first_id - 1);
                    }
                    else {
                        self.draw_state.set(ListDrawState::End);
                        return None
                    }
                }
                
                let scroll = scroll + rect.size.index(vi);
                
                self.draw_state.set(ListDrawState::Down {
                    index: index + 1,
                    scroll,
                    viewport
                });
                cx.begin_turtle(Walk {
                    abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y) + DVec2::from_index_pair(vi, scroll, 0.0)),
                    margin: Default::default(),
                    width: Size::Fill,
                    height: Size::Fit
                }, Layout::flow_down());
                return Some(index + 1)
            }
            Some(ListDrawState::Up {index, scroll, viewport}) => {
                let did_draw = cx.turtle_has_align_items();
                let used = cx.turtle().used();
                let shift = DVec2::from_index_pair(vi, scroll - used.index(vi), 0.0);
                cx.turtle_mut().set_shift(shift);
                
                let rect = cx.end_turtle();
                if !did_draw || rect.pos.index(vi) + rect.size.index(vi) + shift.index(vi) < viewport.pos.index(vi) {
                    self.draw_state.set(ListDrawState::End);
                    if self.first_id == self.range_start && self.first_scroll < 0.0 {
                        self.first_scroll = 0.0;
                    }
                    return None
                }
                self.first_id = index;
                self.first_scroll = scroll - used.index(vi);
                
                
                
                if index == self.range_start {
                    self.draw_state.set(ListDrawState::End);
                    return None
                }
                
                self.draw_state.set(ListDrawState::Up {
                    index: self.first_id - 1,
                    scroll: self.first_scroll,
                    viewport
                });
                
                cx.begin_turtle(Walk {
                    abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y)),
                    margin: Default::default(),
                    width: Size::Fill,
                    height: Size::Fit
                }, Layout::flow_down());
                
                return Some(self.first_id - 1);
            }
            _ => {
                return None
            }
        }
    }
    
    pub fn item(&mut self, cx: &mut Cx, entry_id: u64, template: LiveId) -> Option<WidgetRef> {
        if let Some(ptr) = self.templates.get(&template) {
            let entry = self.items.get_or_insert(cx, (entry_id, template), | cx | {
                WidgetRef::new_from_ptr(cx, Some(*ptr))
            });
            return Some(entry.clone())
        }
        None
    }
    
    pub fn set_item_range(&mut self, range_start: u64, range_end: u64, view_window: u64) {
        self.range_start = range_start;
        self.range_end = range_end;
        self.view_window = view_window;
    }
    
    fn delta_top_scroll(&mut self, cx: &mut Cx, delta: f64) {
        self.first_scroll += delta;
        if self.first_id == self.range_start && self.first_scroll > 0.0 {
            self.first_scroll = 0.0;
        }
        if self.first_id + 1 == self.range_end && self.first_scroll < 0.0 {
            self.first_scroll = 0.0;
        }
        let scroll_pos = ((self.first_id - self.range_start) as f64 / (self.range_end - self.range_start - self.view_window) as f64) * self.scroll_bar.get_scroll_view_total();
        // move the scrollbar to the right 'top' position
        self.scroll_bar.set_scroll_pos_no_action(cx, scroll_pos);
    }
}

#[derive(Clone)]
enum ListDrawState {
    Begin,
    Down {index: u64, scroll: f64, viewport: Rect},
    Up {index: u64, scroll: f64, viewport: Rect},
    End
}

#[derive(Clone, WidgetAction)]
pub enum InfiniteListAction {
    Scroll,
    None
}

impl Widget for ListView {
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        
        let mut scroll_to = None;
        self.scroll_bar.handle_event_with(cx, event, &mut | _cx, action | {
            // snap the scrollbar to a top-index with scroll_pos 0
            if let ScrollBarAction::Scroll {scroll_pos, ..} = action {
                scroll_to = Some(scroll_pos)
            }
        });
        if let Some(scroll_to) = scroll_to {
            // reverse compute the top_id
            let scroll_to = ((scroll_to / self.scroll_bar.get_scroll_view_visible()) * self.view_window as f64) as u64;
            self.first_id = scroll_to;
            self.first_scroll = 0.0;
            dispatch_action(cx, WidgetActionItem::new(InfiniteListAction::Scroll.into(), uid));
            self.area.redraw(cx);
        }
        
        for item in self.items.values_mut() {
            let item_uid = item.widget_uid();
            item.handle_widget_event_with(cx, event, &mut | cx, action | {
                dispatch_action(cx, action.with_container(uid).with_item(item_uid))
            });
        }
        
        if let ScrollState::Flick {delta, next_frame} = &mut self.scroll_state {
            if let Some(_) = next_frame.is_event(event) {
                *delta = *delta * self.flick_scroll_decay;
                if delta.abs()>self.flick_scroll_minimum {
                    *next_frame = cx.new_next_frame();
                    let delta = *delta;
                    self.delta_top_scroll(cx, delta);
                    dispatch_action(cx, InfiniteListAction::Scroll.into_action(uid));
                    self.area.redraw(cx);
                } else {
                    self.scroll_state = ScrollState::Stopped;
                }
            }
        }
        let vi = self.vec_index;
        match event.hits_with_capture_overload(cx, self.area, self.capture_overload) {
            Hit::FingerScroll(e) => {
                self.scroll_state = ScrollState::Stopped;
                self.delta_top_scroll(cx, -e.scroll.index(vi));
                dispatch_action(cx, InfiniteListAction::Scroll.into_action(uid));
                self.area.redraw(cx);
            },
            Hit::FingerDown(e) => {
                cx.set_key_focus(self.area);
                // ok so fingerdown eh.
                self.drag_state = DragState::SwipeDrag {
                    last_abs: e.abs.index(vi),
                    delta: 0.0,
                    initial_time: e.time
                };
            }
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::ArrowDown => {
                    self.first_id += 1;
                    if self.first_id >= self.range_end.max(1) {
                        self.first_id = self.range_end - 1;
                    }
                    self.first_scroll = 0.0;
                    self.area.redraw(cx);
                },
                KeyCode::ArrowUp => {
                    if self.first_id > 0 {
                        self.first_id -= 1;
                        if self.first_id < self.range_start {
                            self.first_id = self.range_start;
                        }
                        self.first_scroll = 0.0;
                        self.area.redraw(cx);
                    }
                },
                _ => ()
            }
            Hit::FingerMove(e) => {
                cx.set_cursor(MouseCursor::Default);
                
                // ok we kinda have to set the scroll pos to our abs position
                match &mut self.drag_state {
                    DragState::SwipeDrag {last_abs, delta, initial_time} => {
                        let new_delta = e.abs.index(vi) - *last_abs;
                        if e.time - *initial_time < self.swipe_drag_duration {
                            *delta = new_delta;
                            *last_abs = e.abs.index(vi);
                        }
                        else {
                            // After a short span of time, the flick motion is considered a normal drag
                            self.scroll_state = ScrollState::Stopped;
                            self.drag_state = DragState::NormalDrag {
                                last_abs: e.abs.index(vi),
                                delta: new_delta
                            };
                        }
                        self.delta_top_scroll(cx, new_delta);
                        dispatch_action(cx, InfiniteListAction::Scroll.into_action(uid));
                        self.area.redraw(cx);
                    },
                    DragState::NormalDrag {last_abs, delta} => {
                        let new_delta = e.abs.index(vi) - *last_abs;
                        *delta = new_delta;
                        *last_abs = e.abs.index(vi);
                        self.delta_top_scroll(cx, new_delta);
                        dispatch_action(cx, InfiniteListAction::Scroll.into_action(uid));
                        self.area.redraw(cx);
                    },
                    DragState::None => {}
                }
            }
            Hit::FingerUp(_) => {
                if let DragState::SwipeDrag {delta, ..} = &mut self.drag_state {
                    if delta.abs()>self.flick_scroll_minimum {
                        self.scroll_state = ScrollState::Flick {
                            delta: *delta,
                            next_frame: cx.new_next_frame()
                        };
                    }
                }
                self.drag_state = DragState::None;
                // ok so. lets check our gap from 'drag'
                // here we kinda have to take our last delta and animate it
            }
            Hit::KeyFocus(_) => {
            }
            Hit::KeyFocusLost(_) => {
            }
            _ => ()
        }
    }
    
    fn walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, ListDrawState::Begin) {
            self.begin(cx, walk);
            return WidgetDraw::hook_above()
        }
        // ok so if we are
        if let Some(_) = self.draw_state.get() {
            self.end(cx);
            self.draw_state.end();
        }
        WidgetDraw::done()
    }
}

#[derive(Clone, Default, PartialEq, WidgetRef)]
pub struct ListViewRef(WidgetRef);

impl ListViewRef {
    pub fn set_first_id_and_scroll(&self, id:u64, s:f64){
        if let Some(mut inner) = self.borrow_mut() {
            inner.first_id = id;
            inner.first_scroll =s;
        }
    }

    pub fn set_first_id(&self, id:u64){
        if let Some(mut inner) = self.borrow_mut() {
            inner.first_id = id;
        }
    }
    
    pub fn first_id(&self)->u64{
         if let Some(inner) = self.borrow() {
             inner.first_id 
        }
        else{
            0
        }
    }
    
    pub fn item(&self, cx: &mut Cx, entry_id: u64, template: LiveId) -> Option<WidgetRef> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.item(cx, entry_id, template)
        }
        else {
            None
        }
    }
    
    pub fn items_with_actions(&self, actions: &WidgetActions) -> Vec<(u64, WidgetRef)> {
        let mut set = Vec::new();
        self.items_with_actions_vec(actions, &mut set);
        set
    }
    
    fn items_with_actions_vec(&self, actions: &WidgetActions, set: &mut Vec<(u64, WidgetRef)>) {
        let uid = self.widget_uid();
        for action in actions {
            if action.container_uid == uid {
                if let Some(inner) = self.borrow() {
                    for ((item_id, _), item) in inner.items.iter() {
                        if item.widget_uid() == action.item_uid {
                            set.push((*item_id, item.clone()))
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Default, WidgetSet)]
pub struct ListViewSet(WidgetSet);

impl ListViewSet {
    pub fn set_first_id(&self, id:u64){
        for list in self.iter() {
            list.set_first_id(id)
        }
    }
    
    
    pub fn items_with_actions(&self, actions: &WidgetActions) -> Vec<(u64, WidgetRef)> {
        let mut set = Vec::new();
        for list in self.iter() {
            list.items_with_actions_vec(actions, &mut set)
        }
        set
    }
}
