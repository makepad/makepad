
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
    import makepad_widgets::frame::Frame;
    ListView = {{ListView}} {
        walk: {
            width: Fill
            height: Fill
        }
        layout: {flow: Down}
        Entry = <Frame> {}
    }
}

enum ScrollState {
    Stopped,
    Drag {last_abs: f64, delta:f64},
    Flick {delta: f64, next_frame: NextFrame}
}

#[derive(Live)]
pub struct ListView {
    #[rust] area: Area,
    #[live] walk: Walk,
    #[live] layout: Layout,
    
    #[rust] range_start: u64,
    #[rust(u64::MAX)] range_end: u64,
    #[rust(10u64)] view_window: u64,
    #[live(0.1)] flick_scroll_minimum: f64,
    #[live(0.98)] flick_scroll_decay: f64,
    #[rust] top_id: u64,
    #[rust] top_scroll: f64,
    #[live] scroll_bar: ScrollBar,
    //    #[live] scroll_bars: ScrollBars,
    #[rust] draw_state: DrawStateWrap<ListDrawState>,
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
    #[rust] items: ComponentMap<(u64, LiveId), WidgetRef>,
    #[rust(ScrollState::Stopped)] scroll_state: ScrollState
}

impl LiveHook for ListView {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, ListView)
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
}

impl ListView {
    
    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, self.layout);
       // self.draw_phase = Some(DrawPhase::Begin)
    }
    
    fn end(&mut self, cx: &mut Cx2d) {
        let rect = cx.turtle().rect();
        let total_views = (self.range_end - self.range_start) as f64 / self.view_window as f64;
        self.scroll_bar.draw_scroll_bar(cx, Axis::Vertical, rect, dvec2(100.0, rect.size.y * total_views));
        
        cx.end_turtle_with_area(&mut self.area);
    }
    
    pub fn next_visible_item(&mut self, cx: &mut Cx2d) -> Option<u64> {
        match self.draw_state.get() {
            Some(ListDrawState::Begin) => {
                let viewport = cx.turtle().rect();
                self.draw_state.set(ListDrawState::Down {
                    index: self.top_id,
                    scroll: self.top_scroll,
                    viewport,
                });
                
                cx.begin_turtle(Walk {
                    abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y + self.top_scroll)),
                    margin: Default::default(),
                    width: Size::Fill,
                    height: Size::Fit
                }, Layout::flow_down());
                return Some(self.top_id)
            }
            Some(ListDrawState::Down {index, scroll, viewport}) => {
                let did_draw = cx.turtle_has_align_items();
                let rect = cx.end_turtle();
                
                if did_draw && rect.pos.y + rect.size.y < viewport.pos.y && index + 1 < self.range_end {
                    self.top_id = index + 1;
                    self.top_scroll = (rect.pos.y + rect.size.y) - viewport.pos.y;
                    if self.top_id + 1 == self.range_end && self.top_scroll < 0.0 {
                        self.top_scroll = 0.0;
                    }
                }
                
                if !did_draw || rect.pos.y + rect.size.y > viewport.pos.y + viewport.size.y || index + 1 == self.range_end {
                    if self.top_id > self.range_start && self.top_scroll > 0.0 {
                        self.draw_state.set(ListDrawState::Up {
                            index: self.top_id - 1,
                            scroll: self.top_scroll,
                            viewport
                        });
                        cx.begin_turtle(Walk {
                            abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y)),
                            margin: Default::default(),
                            width: Size::Fill,
                            height: Size::Fit
                        }, Layout::flow_down());
                        return Some(self.top_id - 1);
                    }
                    else {
                        self.draw_state.set(ListDrawState::End);
                        return None
                    }
                }
                
                if index + 1 == self.range_end {
                    self.draw_state.set(ListDrawState::End);
                    return None
                }
                
                let mut scroll = scroll + rect.size.y;
                if self.top_id + 1 == self.range_end {
                    scroll = 0.0;
                }
                self.draw_state.set(ListDrawState::Down {
                    index: index + 1,
                    scroll,
                    viewport
                });
                cx.begin_turtle(Walk {
                    abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y + scroll)),
                    margin: Default::default(),
                    width: Size::Fill,
                    height: Size::Fit
                }, Layout::flow_down());
                return Some(index + 1)
            }
            Some(ListDrawState::Up {index, scroll, viewport}) => {
                let did_draw = cx.turtle_has_align_items();
                let used = cx.turtle().used();
                let shift = dvec2(0.0, scroll - used.y);
                cx.turtle_mut().set_shift(shift);
                let rect = cx.end_turtle();
                if !did_draw || rect.pos.y + rect.size.y + shift.y < viewport.pos.y {
                    self.draw_state.set(ListDrawState::End);
                    return None
                }
                self.top_id = index;
                self.top_scroll = scroll - used.y;
                if self.top_id + 1 == self.range_end && self.top_scroll < 0.0 {
                    self.top_scroll = 0.0;
                }
                if index == self.range_start {
                    self.draw_state.set(ListDrawState::End);
                    return None
                }
                self.draw_state.set(ListDrawState::Up {
                    index: self.top_id - 1,
                    scroll: self.top_scroll,
                    viewport
                });
                cx.begin_turtle(Walk {
                    abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y)),
                    margin: Default::default(),
                    width: Size::Fill,
                    height: Size::Fit
                }, Layout::flow_down());
                
                return Some(self.top_id - 1);
            }
            _ => {
                return None
            }
        }
    }
    
    pub fn get_item(&mut self, cx: &mut Cx2d, entry_id: u64, template: &[LiveId; 1]) -> Option<WidgetRef> {
        if let Some(ptr) = self.templates.get(&template[0]) {
            let entry = self.items.get_or_insert(cx, (entry_id, template[0]), | cx | {
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
        self.top_scroll += delta;
        if self.top_id == self.range_start && self.top_scroll > 0.0 {
            self.top_scroll = 0.0;
        }
        if self.top_id + 1 == self.range_end && self.top_scroll < 0.0 {
            self.top_scroll = 0.0;
        }
        let scroll_pos = ((self.top_id - self.range_start) as f64 / (self.range_end - self.range_start - self.view_window) as f64) * self.scroll_bar.get_scroll_view_total();
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
            self.top_id = scroll_to;
            self.top_scroll = 0.0;
            dispatch_action(cx, WidgetActionItem::new(InfiniteListAction::Scroll.into(), uid));
            self.area.redraw(cx);
        }
        
        for item in self.items.values_mut() {
            let item_uid = item.widget_uid();
            item.handle_widget_event_with(cx, event, &mut | cx, action | {
                dispatch_action(cx, action.with_container(uid).with_item(item_uid))
            });
        }
        
        if let ScrollState::Flick {delta, next_frame} = &mut self.scroll_state{
            if let Some(_) =  next_frame.is_event(event){
                *delta = *delta * self.flick_scroll_decay;
                if delta.abs()>self.flick_scroll_minimum {
                    *next_frame = cx.new_next_frame();
                    let delta = *delta;
                    self.delta_top_scroll(cx, delta);
                    dispatch_action(cx, InfiniteListAction::Scroll.into_action(uid));
                    self.area.redraw(cx);
                }
            }
        }

        match event.hits(cx, self.area) {
            Hit::FingerScroll(e) => {
                self.delta_top_scroll(cx, -e.scroll.y);
                dispatch_action(cx, InfiniteListAction::Scroll.into_action(uid));
                self.area.redraw(cx);
            },
            Hit::FingerDown(e) => {
                // ok so fingerdown eh.
                self.scroll_state = ScrollState::Drag {
                    last_abs: e.abs.y,
                    delta: 0.0
                };
            }
            Hit::FingerMove(e) => {
                // ok we kinda have to set the scroll pos to our abs position
                if let ScrollState::Drag {last_abs, delta} = &mut self.scroll_state {
                    let new_delta = e.abs.y - *last_abs;
                    *delta = new_delta;
                    *last_abs = e.abs.y;
                    self.delta_top_scroll(cx, new_delta);
                    dispatch_action(cx, InfiniteListAction::Scroll.into_action(uid));
                    self.area.redraw(cx);
                }
            }
            Hit::FingerUp(_) => {
                if let ScrollState::Drag {delta,..} = &mut self.scroll_state {
                    if delta.abs()>self.flick_scroll_minimum {
                        self.scroll_state = ScrollState::Flick {
                            delta: *delta,
                            next_frame: cx.new_next_frame()
                        };
                    }
                }
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
    
    fn get_walk(&self) -> Walk {self.walk}
    
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
    pub fn items_with_actions(&self, actions: &WidgetActions) -> Vec<(u64, WidgetRef)> {
        let mut set = Vec::new();
        for list in self.iter() {
            list.items_with_actions_vec(actions, &mut set)
        }
        set
    }
}
