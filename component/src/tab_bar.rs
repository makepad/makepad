use {
    crate::{
        component_map::ComponentMap,
        tab::{TabAction, Tab},
        scroll_view::ScrollView
    },
    makepad_render::*,
};

live_register!{
    use crate::tab::Tab;
    
    TabBar: {{TabBar}} {
        tab: Tab {}
        draw_drag: {
            draw_depth: 10
            color: #c
        }
        scroll_view: {
            v_show: false
            h_show: true
            h_scroll: {
                bar_size: 8
                use_vertical_finger_scroll: true
            }
            view: {
                debug_id: tab_bar_view
                layout: {
                    walk: {
                        width: Width::Filled
                        height: Height::Fixed(40.0)
                    }
                }
            }
        }
    }
}

#[derive(Live)]
pub struct TabBar {
    
    scroll_view: ScrollView,
    draw_drag: DrawColor,
    tab: Option<LivePtr>,
    
    #[rust] tab_order: Vec<TabId>,
    
    #[rust] is_dragged: bool,
    #[rust] tabs: ComponentMap<TabId, Tab>,

    #[rust] selected_tab_id: Option<TabId>,
    #[rust] next_selected_tab_id: Option<TabId>,
}


impl LiveHook for TabBar{
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, id!(tab)){
            for tab in self.tabs.values_mut() {
                tab.apply(cx, apply_from, index, nodes);
            }
        }
        self.scroll_view.redraw(cx);
    }
}

impl TabBar {
    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.scroll_view.begin(cx) ?;
        self.tab_order.clear();
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx) {
        if self.is_dragged {
            self.draw_drag.draw_walk(
                cx,
                Walk {
                    width: Width::Filled,
                    height: Height::Filled,
                    ..Walk::default()
                },
            );
        }
        self.tabs.retain_visible();
        self.scroll_view.end(cx);
    }
    
    pub fn draw_tab(&mut self, cx: &mut Cx, tab_id: TabId, name: &str) {
        self.tab_order.push(tab_id);
        let tab = self.get_or_create_tab(cx, tab_id);
        tab.draw(cx, name);
    }
    
    fn get_or_create_tab(&mut self, cx:&mut Cx, tab_id: TabId)->&mut Tab{
        self.tabs.get_or_insert_with_ptr(cx, tab_id, self.tab, |cx, ptr|{
             Tab::new_from_ptr(cx, ptr)
        })
    }
    
    pub fn selected_tab_id(&self) -> Option<TabId> {
        self.selected_tab_id
    }
    
    pub fn set_selected_tab_id(&mut self, cx: &mut Cx, tab_id: Option<TabId>, should_animate: bool) {
        if self.selected_tab_id == tab_id {
            return;
        }
        if let Some(tab_id) = self.selected_tab_id {
            let tab = &mut self.tabs[tab_id];
            tab.set_is_selected(cx, false, should_animate);
        }
        self.selected_tab_id = tab_id;
        if let Some(tab_id) = self.selected_tab_id {
            let tab = self.get_or_create_tab(cx, tab_id);
            tab.set_is_selected(cx, true, should_animate);
        }
        self.scroll_view.redraw(cx);
    }
    
    
    pub fn set_next_selected_tab(&mut self, cx: &mut Cx, tab_id: TabId, should_animate: bool) {
        if let Some(index) = self.tab_order.iter().position( | id | *id == tab_id) {
            if self.selected_tab_id != Some(tab_id) {
                self.next_selected_tab_id = self.selected_tab_id;
            }
            else if index >0 {
                self.next_selected_tab_id = Some(self.tab_order[index - 1]);
                self.set_selected_tab_id(cx, self.next_selected_tab_id, should_animate);
            }
            else if index + 1 < self.tab_order.len() {
                self.next_selected_tab_id = Some(self.tab_order[index + 1]);
                self.set_selected_tab_id(cx, self.next_selected_tab_id, should_animate);
            }
            cx.new_next_frame();
        }
    }
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_view.redraw(cx)
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, TabBarAction),
    ) {
        if self.scroll_view.handle_event(cx, event) {
            self.scroll_view.redraw(cx);
        }
        if let Some(tab_id) = self.next_selected_tab_id.take() {
            dispatch_action(cx, TabBarAction::TabWasPressed(tab_id));
        }
        for (tab_id, tab) in self.tabs.iter_mut() {
            tab.handle_event(cx, event, &mut | cx, action | match action {
                TabAction::WasPressed => {
                    dispatch_action(cx, TabBarAction::TabWasPressed(*tab_id));
                }
                TabAction::CloseWasPressed => {
                    dispatch_action(cx, TabBarAction::TabCloseWasPressed(*tab_id));
                }
                TabAction::ReceivedDraggedItem(item) => {
                    dispatch_action(cx, TabBarAction::TabReceivedDraggedItem(*tab_id, item));
                }
            });
        }
        match event.drag_hits(cx, self.scroll_view.area()) {
            DragEvent::FingerDrag(f) => match f.state {
                DragState::In => {
                    self.is_dragged = true;
                    self.redraw(cx);
                    *f.action = DragAction::Copy;
                }
                DragState::Out => {
                    self.is_dragged = false;
                    self.redraw(cx);
                }
                DragState::Over => match event {
                    Event::FingerDrag(event) => {
                        event.action = DragAction::Copy;
                    }
                    _ => panic!(),
                },
            },
            DragEvent::FingerDrop(f) => {
                self.is_dragged = false; 
                self.redraw(cx);
                dispatch_action(cx, TabBarAction::ReceivedDraggedItem(f.dragged_item.clone()))
            }
            _ => {}
        } 
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq)]
pub struct TabId(pub LiveId);
impl From<LiveId> for TabId{
    fn from(live_id:LiveId)->TabId{TabId(live_id)}
}

pub enum TabBarAction {
    ReceivedDraggedItem(DraggedItem),
    TabWasPressed(TabId),
    TabCloseWasPressed(TabId),
    TabReceivedDraggedItem(TabId, DraggedItem),
}
