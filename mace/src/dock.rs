use {
    crate::{
        id::{Id, IdMap},
        splitter::{self, Splitter},
        tab_bar::{self, TabBar, TabId},
    },
    makepad_render::*,
};

pub struct Dock {
    panels_by_panel_id: IdMap<PanelId, Panel>,
    panel_id_stack: Vec<PanelId>,
}

impl Dock {
    pub fn new(_cx: &mut Cx) -> Dock {
        Dock {
            panels_by_panel_id: IdMap::new(),
            panel_id_stack: Vec::new(),
        }
    }

    pub fn begin_splitter(&mut self, cx: &mut Cx, panel_id: PanelId) -> Result<(), ()> {
        let panel = self.get_or_create_split_panel(cx, panel_id);
        panel.splitter.begin(cx)?;
        self.panel_id_stack.push(panel_id);
        Ok(())
    }

    pub fn middle_splitter(&mut self, cx: &mut Cx) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_split_panel_mut();
        panel.splitter.middle(cx);
    }

    pub fn end_splitter(&mut self, cx: &mut Cx) {
        let panel_id = self.panel_id_stack.pop().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_split_panel_mut();
        panel.splitter.end(cx);
    }

    pub fn begin_tab_bar(&mut self, cx: &mut Cx, panel_id: PanelId) -> Result<(), ()> {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.begin(cx)?;
        self.panel_id_stack.push(panel_id);
        Ok(())
    }

    pub fn end_tab_bar(&mut self, cx: &mut Cx) {
        let panel_id = self.panel_id_stack.pop().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_tab_panel_mut();
        panel.tab_bar.end(cx);
    }

    pub fn tab(&mut self, cx: &mut Cx, tab_id: TabId, name: &str) {
        let panel_id = *self.panel_id_stack.last().unwrap();
        let panel = self.panels_by_panel_id[panel_id].as_tab_panel_mut();
        panel.tab_bar.tab(cx, tab_id, name);
    }

    fn get_or_create_split_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut SplitPanel {
        if !self.panels_by_panel_id.contains(panel_id) {
            self.panels_by_panel_id
                .insert(panel_id, Panel::Split(SplitPanel {
                    splitter: Splitter::new(cx)
                }));
        }
        self.panels_by_panel_id[panel_id].as_split_panel_mut()
    }

    fn get_or_create_tab_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut TabPanel {
        if !self.panels_by_panel_id.contains(panel_id) {
            self.panels_by_panel_id
                .insert(panel_id, Panel::Tab(TabPanel {
                    tab_bar: TabBar::new(cx)
                }));
        }
        self.panels_by_panel_id[panel_id].as_tab_panel_mut()
    }

    pub fn selected_tab_id(&mut self, cx: &mut Cx, panel_id: PanelId) -> Option<TabId> {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.selected_tab_id()
    }

    pub fn set_selected_tab_id(&mut self, cx: &mut Cx, panel_id: PanelId, tab_id: Option<TabId>) {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.set_selected_tab_id(cx, tab_id);
    }

    pub fn redraw_tab_bar(&mut self, cx: &mut Cx, panel_id: PanelId) {
        let panel = self.get_or_create_tab_panel(cx, panel_id);
        panel.tab_bar.redraw(cx);
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, Action),
    ) {
        for panel in &mut self.panels_by_panel_id {
            match panel {
                Panel::Split(panel) => {
                    panel.splitter.handle_event(cx, event, &mut |cx, action| match action {
                        splitter::Action::Redraw => {
                            cx.redraw_child_area(Area::All);
                        }
                    });
                }
                Panel::Tab(panel) => {
                    panel.tab_bar.handle_event(cx, event, &mut |cx, action| match action {
                        tab_bar::Action::TabWasPressed(tab_id) => {
                            dispatch_action(cx, Action::TabWasPressed(tab_id))
                        }
                        tab_bar::Action::TabButtonWasPressed(tab_id) => {
                            dispatch_action(cx, Action::TabButtonWasPressed(tab_id))
                        }
                    });
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PanelId(pub Id);

impl AsRef<Id> for PanelId {
    fn as_ref(&self) -> &Id {
        &self.0
    }
}

enum Panel {
    Split(SplitPanel),
    Tab(TabPanel),
}

impl Panel {
    fn as_split_panel_mut(&mut self) -> &mut SplitPanel {
        match self {
            Panel::Split(panel) => panel,
            _ => panic!(),
        }
    }

    fn as_tab_panel_mut(&mut self) -> &mut TabPanel {
        match self {
            Panel::Tab(panel) => panel,
            _ => panic!(),
        }
    }
}

struct SplitPanel {
    splitter: Splitter
}

struct TabPanel {
    tab_bar: TabBar
}

pub enum Action {
    TabWasPressed(TabId),
    TabButtonWasPressed(TabId),
}
