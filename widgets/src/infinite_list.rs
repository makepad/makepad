
use {
    crate::{
        widget::*,
        makepad_derive_widget::*,
        makepad_draw::*,
        scroll_bars::ScrollBars
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
    #[rust] top_id: u64,
    #[rust] top_scroll: f64,
    #[rust] draw_phase: Option<DrawPhase>,
    //    #[live] scroll_bars: ScrollBars,
    #[rust] draw_state: DrawStateWrap<ListDrawState>,
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
    #[rust] entries: ComponentMap<(u64, LiveId), WidgetRef>,
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
                    for ((_, templ_id), node) in self.entries.iter_mut() {
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
    
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        // self.scroll_bars.begin(cx, walk, self.layout);
        cx.begin_turtle(walk, self.layout);
        // lets ask the turtle rect
        self.draw_phase = Some(DrawPhase::Begin)
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        // self.scroll_bars.end(cx);
        cx.end_turtle_with_area(&mut self.area);
        if self.draw_phase.is_some() {
            panic!("please call next_visible in a loop untill returns None");
        }
        // alright we ended.
    }
    
    
    // keep returning an item till the visual space is filled
    // we keep a movable turtle for that
    pub fn next_visible(&mut self, cx: &mut Cx2d) -> Option<u64> {
        // check our drawphase
        match self.draw_phase {
            Some(DrawPhase::Begin) => {
                // we're the first thing to draw. so that should be our 'first' item at scroll pos y
                let viewport = cx.turtle().rect();
                self.draw_phase = Some(DrawPhase::Down {
                    index: self.top_id,
                    scroll: self.top_scroll,
                    viewport,
                });
                // lets begin a turtle at 'top_scroll' with fill width and fit height
                cx.begin_turtle(Walk {
                    abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y + self.top_scroll)),
                    margin: Default::default(),
                    width: Size::Fill,
                    height: Size::Fit
                }, Layout::flow_down());
                return Some(self.top_id)
            }
            Some(DrawPhase::Down {index, scroll, viewport}) => {
                // lets end the turtle and check if we drew anything
                let did_draw = cx.turtle_has_align_items();
                let rect = cx.end_turtle();
                // check if we drew nothing, or if our turtle went > the end, ifso switch to the Up drawphase
                
                // ok we are seeking for a new first_id
                // if the item we just drew is above the viewport, we shift down
                if did_draw && rect.pos.y + rect.size.y < viewport.pos.y {
                    self.top_id = index + 1;
                    self.top_scroll = (rect.pos.y + rect.size.y) - viewport.pos.y;
                }
                if !did_draw || rect.pos.y + rect.size.y > viewport.pos.y + viewport.size.y {
                    if self.top_id > 0 && self.top_scroll > 0.0 {
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
                        // update our top_id / top_scroll values
                        
                        self.draw_phase = None;
                        return None
                    }
                }
                let scroll = scroll + rect.size.y;
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
                // set shift moves the item we drew at viewport 0,0 to the place we need it
                cx.turtle_mut().set_shift(dvec2(0.0, scroll - used.y));
                let rect = cx.end_turtle();
                 
                self.top_id = index;
                self.top_scroll = scroll - used.y;
                // check if we reached the end or not
                if !did_draw || rect.pos.y + rect.size.y < viewport.pos.y || index == 0 {
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
    
    pub fn get_entry(&mut self, cx: &mut Cx2d, entry_id: u64, template: &[LiveId; 1]) -> Option<WidgetRef> {
        if let Some(ptr) = self.templates.get(&template[0]) {
            let entry = self.entries.get_or_insert(cx, (entry_id, template[0]), | cx | {
                WidgetRef::new_from_ptr(cx, Some(*ptr))
            });
            return Some(entry.clone())
        }
        None
    }
    
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, InfiniteListAction),
    ) {
        //self.scroll_bars.handle_event_with(cx, event, &mut | _, _ | {});
        
        match event.hits(cx, self.area) {
            Hit::FingerScroll(s) => {
                self.top_scroll -= s.scroll.y;
                if self.top_id == 0 && self.top_scroll > 0.0 {
                    self.top_scroll = 0.0;
                }
                self.area.redraw(cx);
            },
            Hit::KeyFocus(_) => {
            }
            Hit::KeyFocusLost(_) => {
            }
            _ => ()
        }
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
    fn redraw(&mut self, _cx: &mut Cx) {
        
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        
        for entry in self.entries.values_mut() {
            entry.handle_widget_event_with(cx, event, dispatch_action);
        }
        
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
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
        // find items with container set to our uid
        // and return those
        Default::default()
    }
}

#[derive(Clone, Default, WidgetSet)]
pub struct InfiniteListSet(WidgetSet);

