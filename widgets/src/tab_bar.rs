use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        scroll_bars::ScrollBars,
        tab::{TabAction, Tab},
    },
};

live_design!{
    TabBarBase = {{TabBar}} {}
}

#[derive(Live, Widget)]
pub struct TabBar {
    
    #[redraw] #[live] scroll_bars: ScrollBars,
    #[live] draw_drag: DrawColor,

    #[live] draw_fill: DrawColor,
    #[walk] walk: Walk,
    
    #[rust] draw_state: DrawStateWrap<()>,
    
    #[rust] view_area: Area,

    #[rust] tab_order: Vec<LiveId>,
    
    #[rust] is_dragged: bool,
    
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
    #[rust] tabs: ComponentMap<LiveId, (Tab, LiveId)>,
    
    #[rust] selected_tab: Option<usize>,
    
    #[rust] selected_tab_id: Option<LiveId>,
    #[rust] next_selected_tab_id: Option<LiveId>,
}

impl LiveHook for TabBar {
    /*fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, live_id!(tab).as_field()) {
            for (tab, templl) in self.tabs.values_mut() {
                tab.apply(cx, apply, index, nodes);
            }
        }
        self.view_area.redraw(cx);
    }*/
    
    // hook the apply flow to collect our templates and apply to instanced childnodes
    fn apply_value_instance(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        if nodes[index].is_instance_prop() {
            if let Some(live_ptr) = apply.from.to_live_ptr(cx, index){
                let id = nodes[index].id;
                self.templates.insert(id, live_ptr);
                for (_, (node, templ_id)) in self.tabs.iter_mut() {
                    if *templ_id == id {
                        node.apply(cx, apply, index, nodes);
                    }
                }
            }
        }
        else {
            cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
        }
        nodes.skip_node(index)
    }
}

impl Widget for TabBar{
    
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope
    ){
        let uid = self.widget_uid();
        if self.scroll_bars.handle_event(cx, event, scope).len()>0{
            self.view_area.redraw(cx);
        };
                
        if let Some(tab_id) = self.next_selected_tab_id.take() {
            cx.widget_action(uid, &scope.path, TabBarAction::TabWasPressed(tab_id));
        }
        for (tab_id, (tab,_)) in self.tabs.iter_mut() {
            tab.handle_event_with(cx, event, &mut | cx, action | match action {
                TabAction::WasPressed => {
                    cx.widget_action(uid, &scope.path, TabBarAction::TabWasPressed(*tab_id));
                }
                TabAction::CloseWasPressed => {
                    cx.widget_action(uid, &scope.path, TabBarAction::TabCloseWasPressed(*tab_id));
                }
                TabAction::ShouldTabStartDrag=>{
                    cx.widget_action(uid, &scope.path, TabBarAction::ShouldTabStartDrag(*tab_id));
                }
                TabAction::ShouldTabStopDrag=>{
                }/*
                TabAction::DragHit(hit)=>{
                    dispatch_action(cx, TabBarAction::DragHitTab(hit, *tab_id));
                }*/
            });
        }
        /*
        match event.drag_hits(cx, self.scroll_bars.area()) {
            DragHit::NoHit=>(),
            hit=>dispatch_action(cx, TabBarAction::DragHitTabBar(hit))
        }*/
        /*
        match event.drag_hits(cx, self.scroll_view.area()) {
            DragHit::Drag(f) => match f.state {
                DragState::In => {
                    self.is_dragged = true;
                    self.redraw(cx);
                    f.action.set(DragAction::Copy);
                }
                DragState::Out => {
                    self.is_dragged = false;
                    self.redraw(cx);
                }
                DragState::Over => match event {
                    Event::Drag(event) => {
                        event.action.set(DragAction::Copy);
                    }
                    _ => panic!(),
                },
            },
            DragHit::Drop(f) => {
                self.is_dragged = false;
                self.redraw(cx);
                dispatch_action(cx, TabBarAction::ReceivedDraggedItem(f.dragged_item.clone()))
            }
            _ => {}
        }*/
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            return DrawStep::make_step()
        }
        if let Some(()) = self.draw_state.get() {
            self.draw_state.end();
        }
        DrawStep::done()
    }
}


impl TabBar {
    pub fn begin(&mut self, cx: &mut Cx2d, selected_tab: Option<usize>, walk:Walk) {
        self.selected_tab = selected_tab;
        //if selected_tab.is_some(){
        //    self.selected_tab_id = None
        // }
        self.scroll_bars.begin(cx, walk, Layout::flow_right());
        self.tab_order.clear();
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        if self.is_dragged {
            self.draw_drag.draw_walk(
                cx,
                Walk {
                    width: Size::Fill,
                    height: Size::Fill,
                    ..Walk::default()
                },
            );
        }
        self.tabs.retain_visible();
        self.draw_fill.draw_walk(cx, Walk::size(Size::Fill, Size::Fill));
        self.scroll_bars.end(cx);
    }
    
    pub fn draw_tab(&mut self, cx: &mut Cx2d, tab_id: LiveId, name: &str, template:LiveId) {
        if let Some(selected_tab) = self.selected_tab {
            let tab_order_len = self.tab_order.len();
            let tab = self.get_or_create_tab(cx, tab_id, template);
            if tab_order_len == selected_tab {
                tab.set_is_selected(cx, true, Animate::No);
            }
            else {
                tab.set_is_selected(cx, false, Animate::No);
            }
            tab.draw(cx, name);
            if tab_order_len == selected_tab {
                self.selected_tab_id = Some(tab_id);
            }
            self.tab_order.push(tab_id);
        }
        else {
            self.tab_order.push(tab_id);
            let tab = self.get_or_create_tab(cx, tab_id, template);
            tab.draw(cx, name);
        }
    }
    
    fn get_or_create_tab(&mut self, cx: &mut Cx, tab_id: LiveId, template:LiveId) -> &mut Tab {
        let ptr = self.templates.get(&template).cloned();
        let (tab,_) = self.tabs.get_or_insert(cx, tab_id, | cx | {
            (Tab::new_from_ptr(cx, ptr),template)
        });
        tab
    }
    
    pub fn selected_tab_id(&self) -> Option<LiveId> {
        self.selected_tab_id
    }
    
    pub fn set_selected_tab_id(&mut self, cx: &mut Cx, tab_id: Option<LiveId>, animate: Animate) {
        if self.selected_tab_id == tab_id {
            return;
        }
        if let Some(tab_id) = self.selected_tab_id {
            let (tab,_) = &mut self.tabs[tab_id];
            tab.set_is_selected(cx, false, animate);
        }
        self.selected_tab_id = tab_id;
        if let Some(tab_id) = self.selected_tab_id {
            let (tab,_) = &mut self.tabs[tab_id];
            tab.set_is_selected(cx, true, animate);
        }
        self.view_area.redraw(cx);
    }
    
    
    pub fn set_next_selected_tab(&mut self, cx: &mut Cx, tab_id: LiveId, animate: Animate) {
        if let Some(index) = self.tab_order.iter().position( | id | *id == tab_id) {
            if self.selected_tab_id != Some(tab_id) {
                self.next_selected_tab_id = self.selected_tab_id;
            }
            else if index >0 {
                self.next_selected_tab_id = Some(self.tab_order[index - 1]);
                self.set_selected_tab_id(cx, self.next_selected_tab_id, animate);
            }
            else if index + 1 < self.tab_order.len() {
                self.next_selected_tab_id = Some(self.tab_order[index + 1]);
                self.set_selected_tab_id(cx, self.next_selected_tab_id, animate);
            }
            else {
                self.set_selected_tab_id(cx, None, animate);
            }
            cx.new_next_frame();
        }
        
    }
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.view_area.redraw(cx)
    }
    
    pub fn is_over_tab(&self, cx:&Cx, abs:DVec2)->Option<(LiveId,Rect)>{
        for (tab_id, (tab,_)) in self.tabs.iter() {
            let rect = tab.area().rect(cx);
            if rect.contains(abs){
                return Some((*tab_id, rect))
            }
        }
        None
    }
    
    pub fn is_over_tab_bar(&self, cx:&Cx, abs:DVec2)->Option<Rect>{
        let rect = self.scroll_bars.area().rect(cx);
        if rect.contains(abs){
            return Some(rect)
        }
        None
    }
    

}

#[derive(Clone, Debug, DefaultNone)]
pub enum TabBarAction {
    TabWasPressed(LiveId),
    ShouldTabStartDrag(LiveId),
    TabCloseWasPressed(LiveId),
    None
    //DragHitTab(DragHit, LiveId),
    //DragHitTabBar(DragHit)
}
