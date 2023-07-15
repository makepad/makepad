
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
    InfiniteList = {{InfiniteList}} {
        walk: {
            width: Fill
            height: Fill
        }
        layout: {flow: Down}
        Entry = <Frame> {}
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct InfiniteListEntryId(pub LiveId);

enum DrawPhase {
    Begin,
    Down {index: u64, scroll: f64, viewport: Rect},
    Up {index: u64, scroll: f64, viewport: Rect},
}

#[derive(Live)]
pub struct InfiniteList {
    #[rust] area: Area,
    #[live] walk: Walk,
    #[live] layout: Layout,
    
    #[rust] range_start: u64,
    #[rust(u64::MAX)] range_end: u64,
    #[rust(10u64)] view_window: u64,
    
    #[rust] top_id: u64,
    #[rust] top_scroll: f64,
    #[rust] draw_phase: Option<DrawPhase>,
    #[live] scroll_bar: ScrollBar,
    //    #[live] scroll_bars: ScrollBars,
    #[rust] draw_state: DrawStateWrap<ListDrawState>,
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
    #[rust] items: ComponentMap<(u64, LiveId), WidgetRef>,
}

impl LiveHook for InfiniteList {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, InfiniteList)
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

impl InfiniteList {
    
    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, self.layout);
        self.draw_phase = Some(DrawPhase::Begin)
    }
    
    fn end(&mut self, cx: &mut Cx2d) {
        let rect = cx.turtle().rect();
        let total_views = (self.range_end - self.range_start) as f64 / self.view_window as f64;
        self.scroll_bar.draw_scroll_bar(cx, Axis::Vertical, rect, dvec2(100.0, rect.size.y * total_views));
        
        cx.end_turtle_with_area(&mut self.area);
        if self.draw_phase.is_some() {
            panic!("please call next_visible in a loop untill it returns None");
        }
    }
    
    pub fn next_visible_item(&mut self, cx: &mut Cx2d) -> Option<u64> {
        match self.draw_phase {
            Some(DrawPhase::Begin) => {
                let viewport = cx.turtle().rect();
                self.draw_phase = Some(DrawPhase::Down {
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
            Some(DrawPhase::Down {index, scroll, viewport}) => {
                let did_draw = cx.turtle_has_align_items();
                let rect = cx.end_turtle();
                
                if did_draw && rect.pos.y + rect.size.y < viewport.pos.y && index + 1 < self.range_end {
                    self.top_id = index + 1;
                    self.top_scroll = (rect.pos.y + rect.size.y) - viewport.pos.y;
                    if self.top_id + 1 == self.range_end && self.top_scroll < 0.0{
                        self.top_scroll = 0.0;
                    }
                }
                
                if !did_draw || rect.pos.y + rect.size.y > viewport.pos.y + viewport.size.y || index + 1 == self.range_end {
                    if self.top_id > self.range_start && self.top_scroll > 0.0 {
                        self.draw_phase = Some(DrawPhase::Up {
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
                        self.draw_phase = None;
                        return None
                    }
                }
                
                if index + 1 == self.range_end {
                    self.draw_phase = None;
                    return None
                }
                
                let mut scroll = scroll + rect.size.y;
                if self.top_id + 1 == self.range_end {
                    scroll = 0.0;
                }
                self.draw_phase = Some(DrawPhase::Down {
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
            Some(DrawPhase::Up {index, scroll, viewport}) => {
                let did_draw = cx.turtle_has_align_items();
                let used = cx.turtle().used();
                let shift = dvec2(0.0, scroll - used.y);
                cx.turtle_mut().set_shift(shift);
                let mut rect = cx.end_turtle();
                rect.pos += shift;
                if !did_draw || rect.pos.y + rect.size.y < viewport.pos.y {
                    self.draw_phase = None;
                    return None
                }
                self.top_id = index;
                self.top_scroll = scroll - used.y;
                if self.top_id + 1 == self.range_end && self.top_scroll < 0.0{
                    self.top_scroll = 0.0;
                }
                if index == self.range_start {
                    self.draw_phase = None;
                    return None
                }
                self.draw_phase = Some(DrawPhase::Up {
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
            None => {
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
}

#[derive(Clone)]
enum ListDrawState {
    Hook,
}

#[derive(Clone, WidgetAction)]
pub enum InfiniteListAction {
    Scroll,
    None
}

impl Widget for InfiniteList {
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        
        let mut scroll_to = None;
        self.scroll_bar.handle_event_with(cx, event, &mut | cx, action | {
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
            item.handle_widget_event_with(cx, event, dispatch_action);
        }
        
        match event.hits(cx, self.area) {
            Hit::FingerScroll(s) => {
                self.top_scroll -= s.scroll.y;
                if self.top_id == self.range_start && self.top_scroll > 0.0 {
                    self.top_scroll = 0.0;
                }
                if self.top_id + 1 == self.range_end && self.top_scroll < 0.0 {
                    self.top_scroll = 0.0;
                }
                log!("{} {}", self.top_id, self.top_scroll);
                let scroll_pos = ((self.top_id - self.range_start) as f64 / (self.range_end - self.range_start - self.view_window) as f64) * self.scroll_bar.get_scroll_view_total();
                // move the scrollbar to the right 'top' position
                self.scroll_bar.set_scroll_pos_no_action(cx, scroll_pos);
                
                dispatch_action(cx, WidgetActionItem::new(InfiniteListAction::Scroll.into(), uid));
                self.area.redraw(cx);
                
            },
            Hit::KeyFocus(_) => {
            }
            Hit::KeyFocusLost(_) => {
            }
            _ => ()
        }
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, ListDrawState::Hook) {
            self.begin(cx, walk);
            return WidgetDraw::hook_above()
        }
        if let Some(ListDrawState::Hook) = self.draw_state.get() {
            self.end(cx);
            self.draw_state.end();
        }
        WidgetDraw::done()
    }
}

#[derive(Clone, Default, PartialEq, WidgetRef)]
pub struct InfiniteListRef(WidgetRef);

impl InfiniteListRef {
    pub fn items_with_actions(&self, _actions: &WidgetActions) -> WidgetSet {
        Default::default()
    }
}

#[derive(Clone, Default, WidgetSet)]
pub struct InfiniteListSet(WidgetSet);

