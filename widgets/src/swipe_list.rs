
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
    
    SwipeListEntry = {{SwipeListEntry}} {
        layout: {
            flow: Overlay
        }
        walk: {
            height: 60,
            width: Fill
        }
    }
    
    SwipeList = {{SwipeList}} {
        walk: {
            margin: {top: 3, right: 10, bottom: 3, left: 10},
            width: Fill
            height: 400
        }
        layout: {flow: Down, padding: 10, spacing: 2}
        Entry = <SwipeListEntry> {}
    }
}

#[derive(Live, LiveHook)]
pub struct SwipeListEntry {
    #[animator] animator: Animator,

    #[live] walk: Walk,
    #[live] left_drawer: WidgetRef,
    #[live] center: WidgetRef,
    #[live] right_drawer: WidgetRef,
    #[live] layout: Layout,
    #[rust] draw_state: DrawStateWrap<EntryDrawState>,
}
#[derive(Clone)]
enum EntryDrawState {
    LeftDrawer,
    RightDrawer,
    Center,
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct SwipeListEntryRef(WidgetRef);

#[derive(Clone, Default, WidgetSet)]
pub struct SwipeListEntrySet(WidgetSet);

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct SwipeListEntryId(pub LiveId);

impl Widget for SwipeListEntry {
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, EntryDrawState::LeftDrawer) {
            cx.begin_turtle(walk, self.layout);
        }
        if let Some(EntryDrawState::LeftDrawer) = self.draw_state.get() {
            self.left_drawer.draw_widget(cx) ?;
            self.draw_state.set(EntryDrawState::RightDrawer);
        }
        if let Some(EntryDrawState::RightDrawer) = self.draw_state.get() {
            self.right_drawer.draw_widget(cx) ?;
            self.draw_state.set(EntryDrawState::Center);
        }
        if let Some(EntryDrawState::Center) = self.draw_state.get() {
            self.center.draw_widget(cx) ?;
            cx.end_turtle();
            self.draw_state.end();
        }
        WidgetDraw::done()
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        self.center.handle_widget_event_with(cx, event, dispatch_action);
        self.left_drawer.handle_widget_event_with(cx, event, dispatch_action);
        self.right_drawer.handle_widget_event_with(cx, event, dispatch_action);
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.left_drawer.redraw(cx);
        self.center.redraw(cx);
        self.right_drawer.redraw(cx);
    }
    
    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.left_drawer.find_widgets(path, cached, results);
        self.right_drawer.find_widgets(path, cached, results);
        self.center.find_widgets(path, cached, results);
    }
}

#[derive(Live)]
pub struct SwipeList {
    #[rust] area: Area,
    #[live] walk: Walk,
    #[live] layout: Layout,
    #[live] scroll_bars: ScrollBars,
    #[rust] draw_state: DrawStateWrap<ListDrawState>,
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
    #[rust] entries: ComponentMap<(SwipeListEntryId, LiveId), SwipeListEntry>,
}

impl LiveHook for SwipeList {
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, SwipeList)
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

#[derive(Clone, WidgetAction)]
pub enum SwipeListAction {
    None
}

impl SwipeListEntry {
    
    pub fn handle_event_with(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _sweep_area: Area,
        _dispatch_action: &mut dyn FnMut(&mut Cx, SwipeListAction),
    ) {
        /*
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_button.area().redraw(cx);
        }
        match event.hits_with_options(
            cx,
            self.draw_button.area(),
            HitOptions::new().with_sweep_area(sweep_area)
        ) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerDown(_) => {
                if self.state.animator_in_state(cx, id!(active.on)) {
                    self.animator_play(cx, id!(active.off));
                    dispatch_action(cx, SequencerAction::Change);
                }
                else {
                    self.animator_play(cx, id!(active.on));
                    dispatch_action(cx, SequencerAction::Change);
                    
                }
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerUp(se) => {
                if !se.is_sweep && se.is_over && se.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                }
            }
            _ => {}
        }*/
    }
}


impl SwipeList {
    
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.scroll_bars.begin(cx, walk, self.layout);
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        self.scroll_bars.end(cx);
        self.entries.retain_visible();
    }
    
    pub fn get_entry(&mut self, cx: &mut Cx2d, entry_id: SwipeListEntryId, template: LiveId) -> Option<&mut SwipeListEntry> {
        if let Some(ptr) = self.templates.get(&template) {
            let entry = self.entries.get_or_insert(cx, (entry_id, template), | cx | {
                SwipeListEntry::new_from_ptr(cx, Some(*ptr))
            });
            return Some(entry)
        }
        None
    }
    
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, SwipeListAction),
    ) {
        self.scroll_bars.handle_event_with(cx, event, &mut | _, _ | {});
        
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

impl Widget for SwipeList {
    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
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
pub struct SwipeListRef(WidgetRef);

impl SwipeListRef {
    pub fn items_with_actions(&self, _actions: &WidgetActions) -> SwipeListEntrySet {
        // find items with container set to our uid
        // and return those
        Default::default()
    }
}

#[derive(Clone, Default, WidgetSet)]
pub struct SwipeListSet(WidgetSet);

