use {
    
    crate::{
        makepad_draw::*,
        scroll_bars::ScrollBars,
        tab::{TabAction, Tab},
    },
};

live_design!{
    import crate::tab::Tab;
    import makepad_widgets::theme::*;
    
    TabBar= {{TabBar}} {
        tab: <Tab> {}
        draw_drag: {
            draw_depth: 10
            color: #c
        }
        draw_fill: {
            color: (COLOR_BG_HEADER)
        }
        walk: {
            width: Fill
            height: Fixed((DIM_TAB_HEIGHT))
        }
        scroll_bars: {
            show_scroll_x: true
            show_scroll_y: false
            scroll_bar_x: {
                draw_bar:{bar_width:3.0}
                bar_size: 4
                use_vertical_finger_scroll: true
            }
        }
    }
}

#[derive(Live)]
pub struct TabBar {
    
    #[live] scroll_bars: ScrollBars,
    #[live] draw_drag: DrawColor,

    #[live] draw_fill: DrawColor,
    #[walk] walk: Walk,
    #[live] tab: Option<LivePtr>,
    
    #[rust] view_area: Area,

    #[rust] tab_order: Vec<LiveId>,
    
    #[rust] is_dragged: bool,
    #[rust] tabs: ComponentMap<LiveId, Tab>,
    
    #[rust] selected_tab: Option<usize>,
    
    #[rust] selected_tab_id: Option<LiveId>,
    #[rust] next_selected_tab_id: Option<LiveId>,
}


impl LiveHook for TabBar {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, live_id!(tab).as_field()) {
            for tab in self.tabs.values_mut() {
                tab.apply(cx, from, index, nodes);
            }
        }
        self.view_area.redraw(cx);
    }
}

impl TabBar {
    pub fn begin(&mut self, cx: &mut Cx2d, selected_tab: Option<usize>) {
        self.selected_tab = selected_tab;
        //if selected_tab.is_some(){
        //    self.selected_tab_id = None
        // }
        self.scroll_bars.begin(cx, self.walk, Layout::flow_right());
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
    
    pub fn draw_tab(&mut self, cx: &mut Cx2d, tab_id: LiveId, name: &str) {
        if let Some(selected_tab) = self.selected_tab {
            let tab_order_len = self.tab_order.len();
            let tab = self.get_or_create_tab(cx, tab_id);
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
            let tab = self.get_or_create_tab(cx, tab_id);
            tab.draw(cx, name);
        }
    }
    
    fn get_or_create_tab(&mut self, cx: &mut Cx, tab_id: LiveId) -> &mut Tab {
        let tab = self.tab;
        self.tabs.get_or_insert(cx, tab_id, | cx | {
            Tab::new_from_ptr(cx, tab)
        })
    }
    
    pub fn selected_tab_id(&self) -> Option<LiveId> {
        self.selected_tab_id
    }
    
    pub fn set_selected_tab_id(&mut self, cx: &mut Cx, tab_id: Option<LiveId>, animate: Animate) {
        if self.selected_tab_id == tab_id {
            return;
        }
        if let Some(tab_id) = self.selected_tab_id {
            let tab = &mut self.tabs[tab_id];
            tab.set_is_selected(cx, false, animate);
        }
        self.selected_tab_id = tab_id;
        if let Some(tab_id) = self.selected_tab_id {
            let tab = self.get_or_create_tab(cx, tab_id);
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
        for (tab_id, tab) in self.tabs.iter() {
            let rect = tab.area().get_rect(cx);
            if rect.contains(abs){
                return Some((*tab_id, rect))
            }
        }
        None
    }
    
    pub fn is_over_tab_bar(&self, cx:&Cx, abs:DVec2)->Option<Rect>{
        let rect = self.scroll_bars.area().get_rect(cx);
        if rect.contains(abs){
            return Some(rect)
        }
        None
    }
    
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<TabBarAction> {
        let mut actions = Vec::new();
        self.handle_event_with(cx, event, &mut | _, a | actions.push(a));
        actions
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, TabBarAction),
    ) {
        let view_area = self.view_area;
        self.scroll_bars.handle_event_with(cx, event, &mut |cx,_|{
            view_area.redraw(cx);
        });
        
        if let Some(tab_id) = self.next_selected_tab_id.take() {
            dispatch_action(cx, TabBarAction::TabWasPressed(tab_id));
        }
        for (tab_id, tab) in self.tabs.iter_mut() {
            tab.handle_event_with(cx, event, &mut | cx, action | match action {
                TabAction::WasPressed => {
                    dispatch_action(cx, TabBarAction::TabWasPressed(*tab_id));
                }
                TabAction::CloseWasPressed => {
                    dispatch_action(cx, TabBarAction::TabCloseWasPressed(*tab_id));
                }
                TabAction::ShouldTabStartDrag=>{
                    dispatch_action(cx, TabBarAction::ShouldTabStartDrag(*tab_id));
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
}


pub enum TabBarAction {
    TabWasPressed(LiveId),
    ShouldTabStartDrag(LiveId),
    TabCloseWasPressed(LiveId),
    //DragHitTab(DragHit, LiveId),
    //DragHitTabBar(DragHit)
}
