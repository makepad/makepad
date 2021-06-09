use {makepad_render::*, std::collections::HashMap};

pub struct TabBarLogic {
    tab_ids_by_area: HashMap<Area, TabId>,
    selected_tab_id: Option<TabId>,
}

impl TabBarLogic {
    pub fn new() -> TabBarLogic {
        TabBarLogic {
            tab_ids_by_area: HashMap::new(),
            selected_tab_id: None,
        }
    }

    pub fn begin(&mut self) {
        self.tab_ids_by_area.clear();
    }

    pub fn end(&mut self) {}

    pub fn begin_tab(&mut self, tab_id: TabId) -> TabInfo {
        TabInfo {
            is_selected: self
                .selected_tab_id
                .map_or(false, |selected_tab_id| selected_tab_id == tab_id),
        }
    }

    pub fn end_tab(&mut self) {}

    pub fn set_tab_area(&mut self, tab_id: TabId, area: Area) {
        self.tab_ids_by_area.insert(area, tab_id);
    }

    pub fn set_selected_tab_id(&mut self, tab_id: TabId) -> bool {
        if self.selected_tab_id == Some(tab_id) {
            return false;
        }
        self.selected_tab_id = Some(tab_id);
        true
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(Action),
    ) {
        for (area, tab_id) in &self.tab_ids_by_area {
            match event.hits(cx, *area, HitOpt::default()) {
                Event::FingerDown(_) => {
                    dispatch_action(Action::SetSelectedTabId(*tab_id));
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TabId(pub usize);

#[derive(Clone, Copy, Debug)]
pub struct TabInfo {
    pub is_selected: bool,
}

pub enum Action {
    SetSelectedTabId(TabId),
}
