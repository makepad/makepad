use {
    crate::{list_logic::ItemId, splitter::Splitter, tab_bar::TabBar},
    makepad_render::*,
    std::collections::HashMap,
};

pub struct Dock {
    panel_id_stack: Vec<PanelId>,
    panels_by_panel_id: HashMap<PanelId, Panel>,
}

impl Dock {
    pub fn new(_cx: &mut Cx) -> Dock {
        Dock {
            panel_id_stack: Vec::new(),
            panels_by_panel_id: HashMap::new(),
        }
    }

    pub fn begin_split_panel(&mut self, cx: &mut Cx, panel_id: PanelId) {
        self.panel_id_stack.push(panel_id);
        let panel = self
            .panels_by_panel_id
            .entry(panel_id)
            .or_insert_with(|| Panel::Split(Splitter::new(cx)))
            .as_split_panel_mut();
        panel.begin(cx);
    }

    pub fn middle_split_panel(&mut self, cx: &mut Cx) {
        let panel_id = self.panel_id_stack.last().unwrap();
        let panel = self.panels_by_panel_id.get_mut(&panel_id).unwrap().as_split_panel_mut();
        panel.middle(cx);
    }

    pub fn end_split_panel(&mut self, cx: &mut Cx) {
        let panel_id = self.panel_id_stack.pop().unwrap();
        let panel = self.panels_by_panel_id.get_mut(&panel_id).unwrap().as_split_panel_mut();
        panel.end(cx);
    }

    pub fn begin_tab_panel(&mut self, cx: &mut Cx, panel_id: PanelId) -> Result<(), ()> {
        self.panel_id_stack.push(panel_id);
        let panel = self
            .panels_by_panel_id
            .entry(panel_id)
            .or_insert_with(|| Panel::Tab(TabBar::new(cx)))
            .as_tab_panel_mut();
        panel.begin(cx)
    }

    pub fn end_tab_panel(&mut self, cx: &mut Cx) {
        let panel_id = self.panel_id_stack.pop().unwrap();
        let panel = self.panels_by_panel_id.get_mut(&panel_id).unwrap().as_tab_panel_mut();
        panel.end(cx);
    }

    pub fn tab(&mut self, cx: &mut Cx, tab_id: ItemId, name: &str) {
        let panel_id = self.panel_id_stack.last().unwrap();
        let panel = self.panels_by_panel_id.get_mut(&panel_id).unwrap().as_tab_panel_mut();
        panel.tab(cx, tab_id, name);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PanelId(pub usize);

enum Panel {
    Split(Splitter),
    Tab(TabBar),
}

impl Panel {
    fn as_split_panel_mut(&mut self) -> &mut Splitter {
        match self {
            Panel::Split(panel) => panel,
            _ => panic!(),
        }
    }

    fn as_tab_panel_mut(&mut self) -> &mut TabBar {
        match self {
            Panel::Tab(panel) => panel,
            _ => panic!(),
        }
    }
}
