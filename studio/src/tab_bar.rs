use {
    crate::{
        id::GenId,
        id_map::GenIdMap,
        tab::{self, Tab},
    },
    makepad_render::*,
    makepad_widget::*,
};

live_register!{
    use makepad_render::shader_std::*;
    
    TabBar: {{TabBar}} {
        view: {
            show_v: true
            show_h: true
            view: {
                debug_id: tab_bar_view
                layout: {
                    walk: {
                        width: Width::Filled
                        height: Height::Fixed(24.0)
                    }
                }
            }
        }
    }
}

#[derive(LiveComponent, LiveApply, LiveCast)]
pub struct TabBar {
    #[live] view: ScrollView,
    #[live] drag: DrawColor,
    #[rust] is_dragged: bool,
    #[rust] tabs_by_tab_id: GenIdMap<TabId, Tab>,
    #[rust] tab_ids: Vec<TabId>,
    #[rust] selected_tab_id: Option<TabId>,
    //    #[rust] tab_height: f32,
}

impl TabBar {
    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.view.begin_view(cx) ?;
        self.tab_ids.clear();
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx) {
        if self.is_dragged {
            self.drag.draw_quad_walk(
                cx,
                Walk {
                    width: Width::Filled,
                    height: Height::Filled,
                    ..Walk::default()
                },
            );
        }
        self.view.end_view(cx);
    }
    
    pub fn tab(&mut self, cx: &mut Cx, tab_id: TabId, name: &str) {
        let tab = self.get_or_create_tab(cx, tab_id);
        tab.draw(cx, name);
        self.tab_ids.push(tab_id);
    }
    
    pub fn get_or_create_tab(&mut self, cx: &mut Cx, tab_id: TabId) -> &mut Tab {
        if !self.tabs_by_tab_id.contains(tab_id) {
            self.tabs_by_tab_id.insert(tab_id, Tab::new(cx));
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
        self.view.redraw_view(cx);
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw_view(cx)
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, Action),
    ) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
        for tab_id in &self.tab_ids {
            let tab = &mut self.tabs_by_tab_id[*tab_id];
            tab.handle_event(cx, event, &mut | cx, action | match action {
                tab::Action::WasPressed => {
                    dispatch_action(cx, Action::TabWasPressed(*tab_id));
                }
                tab::Action::ButtonWasPressed => {
                    dispatch_action(cx, Action::TabButtonWasPressed(*tab_id));
                }
                tab::Action::ReceivedDraggedItem(item) => {
                    dispatch_action(cx, Action::TabReceivedDraggedItem(*tab_id, item));
                }
            });
        }
        match event.drag_hits(cx, self.view.area(), HitOpt::default()) {
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
                dispatch_action(cx, Action::ReceivedDraggedItem(event.dragged_item))
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

pub enum Action {
    ReceivedDraggedItem(DraggedItem),
    TabWasPressed(TabId),
    TabButtonWasPressed(TabId),
    TabReceivedDraggedItem(TabId, DraggedItem),
}
