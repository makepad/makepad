use {
    crate::{list_logic::ItemId, splitter::Splitter, tab_bar::TabBar},
    makepad_render::*,
    std::collections::HashMap,
};

pub struct Dock {
    containers_by_container_id: HashMap<ContainerId, Container>,
    container_id_stack: Vec<ContainerId>,
}

impl Dock {
    pub fn new(_cx: &mut Cx) -> Dock {
        Dock {
            containers_by_container_id: HashMap::new(),
            container_id_stack: Vec::new(),
        }
    }

    pub fn begin_splitter(&mut self, cx: &mut Cx, container_id: ContainerId) {
        let splitter = self
            .containers_by_container_id
            .entry(container_id)
            .or_insert_with(|| Container::Splitter(Splitter::new(cx)))
            .as_splitter_mut();
        splitter.begin(cx);
        self.container_id_stack.push(container_id);
    }

    pub fn middle_splitter(&mut self, cx: &mut Cx) {
        let container_id = self.container_id_stack.last().unwrap();
        let splitter = self
            .containers_by_container_id
            .get_mut(&container_id)
            .unwrap()
            .as_splitter_mut();
        splitter.middle(cx);
    }

    pub fn end_splitter(&mut self, cx: &mut Cx) {
        let container_id = self.container_id_stack.pop().unwrap();
        let splitter = self
            .containers_by_container_id
            .get_mut(&container_id)
            .unwrap()
            .as_splitter_mut();
        splitter.end(cx);
    }

    pub fn begin_tab_bar(&mut self, cx: &mut Cx, container_id: ContainerId) -> Result<(), ()> {
        let tab_bar = self
            .containers_by_container_id
            .entry(container_id)
            .or_insert_with(|| Container::TabBar(TabBar::new(cx)))
            .as_tab_bar_mut();
        match tab_bar.begin(cx) {
            Ok(()) => {
                self.container_id_stack.push(container_id);
                Ok(())
            }
            Err(()) => Err(()),
        }
    }

    pub fn end_tab_bar(&mut self, cx: &mut Cx) {
        let container_id = self.container_id_stack.pop().unwrap();
        let tab_bar = self
            .containers_by_container_id
            .get_mut(&container_id)
            .unwrap()
            .as_tab_bar_mut();
        tab_bar.end(cx);
    }

    pub fn tab(&mut self, cx: &mut Cx, tab_id: ItemId, name: &str) {
        let container_id = self.container_id_stack.last().unwrap();
        let tab_bar = self
            .containers_by_container_id
            .get_mut(&container_id)
            .unwrap()
            .as_tab_bar_mut();
        tab_bar.tab(cx, tab_id, name);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ContainerId(pub usize);

pub enum Container {
    Splitter(Splitter),
    TabBar(TabBar),
}

impl Container {
    fn as_splitter_mut(&mut self) -> &mut Splitter {
        match self {
            Container::Splitter(splitter) => splitter,
            _ => panic!(),
        }
    }

    fn as_tab_bar_mut(&mut self) -> &mut TabBar {
        match self {
            Container::TabBar(tab_bar) => tab_bar,
            _ => panic!(),
        }
    }
}
