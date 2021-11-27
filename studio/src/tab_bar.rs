use {
    crate::{
        id::GenId,
        id_map::GenIdMap,
        tab::{TabAction, Tab},
    },
    makepad_render::*,
    makepad_widget::*,
};

live_register!{
    use makepad_render::shader_std::*;
    use crate::tab::Tab;
    
    TabBar: {{TabBar}} {
        tab: Tab {}
        draw_drag:{
            draw_depth:10
            color:#ff00ffff
        }
        scroll_view: {
            show_v: false
            show_h: true
            scroll_h:{
                bar_size:8
                use_vertical_finger_scroll:true
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

#[derive(LiveComponent, LiveApply, LiveTraitCast)]
pub struct TabBar {
    #[live] scroll_view: ScrollView,
    #[live] draw_drag: DrawColor,
    #[live] tab: Option<LivePtr>,
    #[rust] is_dragged: bool,
    #[rust] tabs_by_tab_id: GenIdMap<TabId, Tab>,
    #[rust] tab_ids: Vec<TabId>,
    #[rust] selected_tab_id: Option<TabId>,
}

impl TabBar {
    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.scroll_view.begin(cx) ?;
        self.tab_ids.clear();
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx) {
        if self.is_dragged {
            self.draw_drag.draw_quad_walk(
                cx,
                Walk {
                    width: Width::Filled,
                    height: Height::Filled,
                    ..Walk::default()
                },
            );
        }
        self.scroll_view.end(cx);
    }
    
    pub fn tab(&mut self, cx: &mut Cx, tab_id: TabId, name: &str) {
        let tab = self.get_or_create_tab(cx, tab_id);
        tab.draw(cx, name);
        self.tab_ids.push(tab_id);
    }
    
    pub fn get_or_create_tab(&mut self, cx: &mut Cx, tab_id: TabId) -> &mut Tab {
        if !self.tabs_by_tab_id.contains(tab_id) {
            self.tabs_by_tab_id.insert(tab_id, Tab::new_from_ptr(cx, self.tab.unwrap()));
        }
        &mut self.tabs_by_tab_id[tab_id]
    }
    
    pub fn forget_tab(&mut self, tab_id: TabId) {
        self.tabs_by_tab_id.remove(tab_id);
    }
    
    pub fn selected_tab_id(&self) -> Option<TabId> {
        self.selected_tab_id
    }
    
    pub fn set_selected_tab_id(&mut self, cx: &mut Cx, tab_id: Option<TabId>) {
        if self.selected_tab_id == tab_id {
            return;
        }
        if let Some(tab_id) = self.selected_tab_id {
            let tab = &mut self.tabs_by_tab_id[tab_id];
            tab.set_is_selected(false);
        }
        self.selected_tab_id = tab_id;
        if let Some(tab_id) = self.selected_tab_id {
            let tab = self.get_or_create_tab(cx, tab_id);
            tab.set_is_selected(true);
        }
        self.scroll_view.redraw(cx);
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
        for tab_id in &self.tab_ids {
            let tab = &mut self.tabs_by_tab_id[*tab_id];
            tab.handle_event(cx, event, &mut | cx, action | match action {
                TabAction::WasPressed => {
                    dispatch_action(cx, TabBarAction::TabWasPressed(*tab_id));
                }
                TabAction::ButtonWasPressed => {
                    dispatch_action(cx, TabBarAction::TabButtonWasPressed(*tab_id));
                }
                TabAction::ReceivedDraggedItem(item) => {
                    dispatch_action(cx, TabBarAction::TabReceivedDraggedItem(*tab_id, item));
                }
            });
        }
        match event.drag_hits(cx, self.scroll_view.area(), HitOpt::default()) {
            Event::FingerDrag(drag_event) => match drag_event.state {
                DragState::In => {
                    self.is_dragged = true;
                    self.redraw(cx);
                    match event {
                        Event::FingerDrag(event) => {
                            event.action = DragAction::Copy;
                        }
                        _ => panic!(),
                    }
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
            Event::FingerDrop(event) => {
                self.is_dragged = false;
                self.redraw(cx);
                dispatch_action(cx, TabBarAction::ReceivedDraggedItem(event.dragged_item))
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TabId(pub GenId);

impl AsRef<GenId> for TabId {
    fn as_ref(&self) -> &GenId {
        &self.0
    }
}

pub enum TabBarAction {
    ReceivedDraggedItem(DraggedItem),
    TabWasPressed(TabId),
    TabButtonWasPressed(TabId),
    TabReceivedDraggedItem(TabId, DraggedItem),
}
