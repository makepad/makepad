use {
    crate::{
        list_logic::ItemId,
        splitter::{self, Splitter},
        tab_bar::{self, TabBar},
    },
    makepad_render::*,
    std::collections::HashMap,
};

pub struct Dock {
    panels_by_panel_id: HashMap<PanelId, Panel>,
    panel_id_stack: Vec<PanelId>,
}

impl Dock {
    pub fn new(_cx: &mut Cx) -> Dock {
        Dock {
            panels_by_panel_id: HashMap::new(),
            panel_id_stack: Vec::new(),
        }
    }

    pub fn begin_splitter(&mut self, cx: &mut Cx, panel_id: PanelId) -> Result<(), ()> {
        let splitter = self
            .panels_by_panel_id
            .entry(panel_id)
            .or_insert_with(|| Panel::Splitter(Splitter::new(cx)))
            .as_splitter_mut();
        splitter.begin(cx)?;
        self.panel_id_stack.push(panel_id);
        Ok(())
    }

    pub fn middle_splitter(&mut self, cx: &mut Cx) {
        let panel_id = self.panel_id_stack.last().unwrap();
        let splitter = self
            .panels_by_panel_id
            .get_mut(&panel_id)
            .unwrap()
            .as_splitter_mut();
        splitter.middle(cx);
    }

    pub fn end_splitter(&mut self, cx: &mut Cx) {
        let panel_id = self.panel_id_stack.pop().unwrap();
        let splitter = self
            .panels_by_panel_id
            .get_mut(&panel_id)
            .unwrap()
            .as_splitter_mut();
        splitter.end(cx);
    }

    pub fn begin_tab_bar(&mut self, cx: &mut Cx, panel_id: PanelId) -> Result<(), ()> {
        let tab_bar = self
            .panels_by_panel_id
            .entry(panel_id)
            .or_insert_with(|| Panel::TabBar(TabBar::new(cx)))
            .as_tab_bar_mut();
        tab_bar.begin(cx)?;
        self.panel_id_stack.push(panel_id);
        Ok(())
    }

    pub fn end_tab_bar(&mut self, cx: &mut Cx) {
        let panel_id = self.panel_id_stack.pop().unwrap();
        let tab_bar = self
            .panels_by_panel_id
            .get_mut(&panel_id)
            .unwrap()
            .as_tab_bar_mut();
        tab_bar.end(cx);
    }

    pub fn tab(&mut self, cx: &mut Cx, tab_id: ItemId, name: &str) {
        let panel_id = self.panel_id_stack.last().unwrap();
        let tab_bar = self
            .panels_by_panel_id
            .get_mut(&panel_id)
            .unwrap()
            .as_tab_bar_mut();
        tab_bar.tab(cx, tab_id, name);
    }

    pub fn splitter_mut(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut Splitter {
        self.panels_by_panel_id
            .entry(panel_id)
            .or_insert_with(|| Panel::Splitter(Splitter::new(cx)))
            .as_splitter_mut()
    }

    pub fn tab_bar_mut(&mut self, cx: &mut Cx, panel_id: PanelId) -> &mut TabBar {
        self.panels_by_panel_id
            .entry(panel_id)
            .or_insert_with(|| Panel::TabBar(TabBar::new(cx)))
            .as_tab_bar_mut()
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(Action),
    ) {
        for (_, panel) in &mut self.panels_by_panel_id {
            match panel {
                Panel::Splitter(splitter) => {
                    let mut actions = Vec::new();
                    splitter.handle_event(cx, event, &mut |action| actions.push(action));
                    for action in actions {
                        match action {
                            splitter::Action::Redraw => {
                                cx.redraw_child_area(Area::All);
                            }
                        }
                    }
                }
                Panel::TabBar(tab_bar) => {
                    let mut actions = Vec::new();
                    tab_bar.handle_event(cx, event, &mut |action| actions.push(action));
                    for action in actions {
                        match action {
                            tab_bar::Action::TabWasPressed(item_id) => {
                                dispatch_action(Action::TabWasPressed(item_id))
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PanelId(pub usize);

enum Panel {
    Splitter(Splitter),
    TabBar(TabBar),
}

impl Panel {
    fn as_splitter_mut(&mut self) -> &mut Splitter {
        match self {
            Panel::Splitter(splitter) => splitter,
            _ => panic!(),
        }
    }

    fn as_tab_bar_mut(&mut self) -> &mut TabBar {
        match self {
            Panel::TabBar(tab_bar) => tab_bar,
            _ => panic!(),
        }
    }
}

pub enum Action {
    TabWasPressed(ItemId),
}
