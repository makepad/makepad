use {
    crate::{
        id::Id,
        id_map::IdMap,
        tab::{self, Tab},
    },
    makepad_render::*,
    makepad_widget::*,
};

pub struct TabBar {
    view: ScrollView,
    tabs_by_tab_id: IdMap<TabId, Tab>,
    tab_ids: Vec<TabId>,
    selected_tab_id: Option<TabId>,
    tab_height: f32,
}

impl TabBar {
    pub fn new(cx: &mut Cx) -> TabBar {
        TabBar {
            view: ScrollView::new_standard_hv(cx),
            tabs_by_tab_id: IdMap::new(),
            tab_ids: Vec::new(),
            selected_tab_id: None,
            tab_height: 0.0,
        }
    }

    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.apply_style(cx);
        self.view.begin_view(cx, self.layout())?;
        self.tab_ids.clear();
        Ok(())
    }

    pub fn end(&mut self, cx: &mut Cx) {
        self.view.end_view(cx);
    }

    pub fn tab(&mut self, cx: &mut Cx, tab_id: TabId, name: &str) {
        let tab = self.get_or_create_tab(cx, tab_id);
        tab.draw(cx, name);
        self.tab_ids.push(tab_id);
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.tab_height = live_float!(cx, crate::tab::height);
    }

    fn layout(&self) -> Layout {
        Layout {
            walk: Walk {
                width: Width::Fill,
                height: Height::Fix(self.tab_height),
                ..Walk::default()
            },
            ..Layout::default()
        }
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
            tab.handle_event(cx, event, &mut |cx, action| match action {
                tab::Action::WasPressed => {
                    dispatch_action(cx, Action::TabWasPressed(*tab_id));
                }
                tab::Action::ButtonWasPressed => {
                    dispatch_action(cx, Action::TabButtonWasPressed(*tab_id));
                }
                tab::Action::ReceivedDragItem(item) => {
                    dispatch_action(cx, Action::TabReceivedDragItem(*tab_id, item));
                }
            });
        }
        match event.drag_hits(cx, self.view.area(), HitOpt::default()) {
            Event::FingerDrag(_) => {
                match event {
                    Event::FingerDrag(event) => {
                        event.action = DragAction::Copy;
                    }
                    _ => panic!(),
                }
            }
            Event::FingerDrop(event) => {
                dispatch_action(cx, Action::ReceivedDragItem(event.drag_item))
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TabId(pub Id);

impl AsRef<Id> for TabId {
    fn as_ref(&self) -> &Id {
        &self.0
    }
}

pub enum Action {
    ReceivedDragItem(DragItem),
    TabWasPressed(TabId),
    TabButtonWasPressed(TabId),
    TabReceivedDragItem(TabId, DragItem),
}

#[derive(Clone, DrawQuad)]
#[repr(C)]
struct DrawTab {
    #[default_shader(self::draw_tab_shader)]
    base: DrawColor,
    border_width: f32,
    border_color: Vec4,
}
