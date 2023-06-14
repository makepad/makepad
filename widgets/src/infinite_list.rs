
use {
    crate::{
        widget::*,
        makepad_derive_widget::*,
        makepad_draw::*,
        //scroll_bars::ScrollBars
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    
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

#[derive(Live)]
pub struct InfiniteList {
    #[rust] area: Area,
    #[live] walk: Walk,
    #[live] layout: Layout,
    #[rust] draw_state: DrawStateWrap<ListDrawState>,
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
    #[rust] entries: ComponentMap<(InfiniteListEntryId, LiveId), WidgetRef>,
}

impl LiveHook for InfiniteList {
    fn before_live_design(cx:&mut Cx){
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
        cx.begin_turtle(walk, self.layout);
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle();
        self.entries.retain_visible();
    }
    
    pub fn get_entry(&mut self, cx: &mut Cx2d, entry_id: InfiniteListEntryId, template: LiveId) -> Option<WidgetRef> {
        if let Some(ptr) = self.templates.get(&template) {
            let entry = self.entries.get_or_insert(cx, (entry_id, template), | cx | {
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
        //_dispatch_action: &mut dyn FnMut(&mut Cx, SwipeListAction),
    ) {
        //self.scroll_bars.handle_event_with(cx, event, &mut | _, _ | {});
        
        match event.hits(cx, self.area) {
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

impl Widget for InfiniteList {
    fn redraw(&mut self, _cx: &mut Cx) {
        
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let _uid = self.widget_uid();
        
        for entry in self.entries.values_mut() {
            entry.handle_widget_event_with(cx, event, dispatch_action);
        }
        /*
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });*/
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

