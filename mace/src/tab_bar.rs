use {
    crate::{generational::{Id, Arena}, tab::{self, Tab}},
    makepad_render::*,
    makepad_widget::*,
};

pub struct TabBar {
    view: ScrollView,
    tabs: Arena<Tab>,
    tab_ids: Vec<TabId>,
    selected_tab_id: Option<TabId>,
    tab_height: f32,
}

impl TabBar {
    pub fn new(cx: &mut Cx) -> TabBar {
        TabBar {
            view: ScrollView::new_standard_hv(cx),
            tabs: Arena::new(),
            tab_ids: Vec::new(),
            selected_tab_id: None,
            tab_height: 0.0,
        }
    }

    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.view.begin_view(cx, self.layout())?;
        self.apply_style(cx);
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
        self.tab_height = live_float!(cx, crate::tab::tab_height);
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
        if !self.tabs.contains(tab_id) {
            self.tabs.insert(tab_id, Tab::new(cx));
        }
        &mut self.tabs[tab_id]
    }

    pub fn forget_tab(&mut self, tab_id: TabId) {
        self.tabs.remove(tab_id);
    }

    pub fn selected_tab_id(&self) -> Option<TabId> {
        self.selected_tab_id
    }

    pub fn set_selected_tab_id(&mut self, cx: &mut Cx, tab_id: Option<TabId>) {
        if self.selected_tab_id == tab_id {
            return;
        }
        if let Some(tab_id) = self.selected_tab_id {
            let tab = &mut self.tabs[tab_id];
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
        dispatch_action: &mut dyn FnMut(Action),
    ) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
        for tab_id in &self.tab_ids {
            let tab = &mut self.tabs[*tab_id];
            tab.handle_event(cx, event, &mut |action| match action {
                tab::Action::WasPressed => {
                    dispatch_action(Action::TabWasPressed(*tab_id));
                }
            });
        }
    }
}

pub type TabId = Id;

pub enum Action {
    TabWasPressed(TabId),
}

#[derive(Clone, DrawQuad)]
#[repr(C)]
struct DrawTab {
    #[default_shader(self::draw_tab_shader)]
    base: DrawColor,
    border_width: f32,
    border_color: Vec4,
}
