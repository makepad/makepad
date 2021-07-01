use {
    crate::{list_logic::ItemId, splitter::Splitter, tab_bar::TabBar},
    makepad_render::*,
    std::collections::HashMap,
};

pub struct Dock {
    container_id_stack: Vec<PanelId>,
    containers_by_container_id: HashMap<PanelId, Panel>,
}

impl Dock {
    pub fn new(_cx: &mut Cx) -> Dock {
        Dock {
            container_id_stack: Vec::new(),
            containers_by_container_id: HashMap::new(),
        }
    }

    pub fn begin_split_container(&mut self, cx: &mut Cx, container_id: PanelId) {
        self.container_id_stack.push(container_id);
        let container = self
            .containers_by_container_id
            .entry(container_id)
            .or_insert_with(|| Panel::Split(Splitter::new(cx)))
            .as_split_container_mut();
        container.begin(cx);
    }

    pub fn middle_split_container(&mut self, cx: &mut Cx) {
        let container_id = self.container_id_stack.last().unwrap();
        let container = self
            .containers_by_container_id
            .get_mut(&container_id)
            .unwrap()
            .as_split_container_mut();
        container.middle(cx);
    }

    pub fn end_split_container(&mut self, cx: &mut Cx) {
        let container_id = self.container_id_stack.pop().unwrap();
        let container = self
            .containers_by_container_id
            .get_mut(&container_id)
            .unwrap()
            .as_split_container_mut();
        container.end(cx);
    }

    pub fn begin_tab_container(&mut self, cx: &mut Cx, container_id: PanelId) -> Result<(), ()> {
        self.container_id_stack.push(container_id);
        let container = self
            .containers_by_container_id
            .entry(container_id)
            .or_insert_with(|| Panel::Tab(TabBar::new(cx)))
            .as_tab_container_mut();
        container.begin(cx)
    }

    pub fn end_tab_container(&mut self, cx: &mut Cx) {
        let container_id = self.container_id_stack.pop().unwrap();
        let container = self
            .containers_by_container_id
            .get_mut(&container_id)
            .unwrap()
            .as_tab_container_mut();
        container.end(cx);
    }

    pub fn tab(&mut self, cx: &mut Cx, tab_id: ItemId, name: &str) {
        let container_id = self.container_id_stack.last().unwrap();
        let container = self
            .containers_by_container_id
            .get_mut(&container_id)
            .unwrap()
            .as_tab_container_mut();
        container.tab(cx, tab_id, name);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PanelId(pub usize);

enum Panel {
    Split(Splitter),
    Tab(TabBar),
}

impl Panel {
    fn as_split_container_mut(&mut self) -> &mut Splitter {
        match self {
            Panel::Split(container) => container,
            _ => panic!(),
        }
    }

    fn as_tab_container_mut(&mut self) -> &mut TabBar {
        match self {
            Panel::Tab(container) => container,
            _ => panic!(),
        }
    }
}
