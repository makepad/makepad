use makepad_render::*;

pub struct TabBarLogic;

impl TabBarLogic {
    pub fn begin(&mut self) {}

    pub fn end(&mut self) {}

    pub fn begin_tab(&mut self, _tab_id: TabId) -> TabInfo {
        TabInfo { is_selected: false } // TODO
    }

    pub fn end_tab(&mut self) {}

    pub fn handle_event(&mut self, _cx: &mut Cx, _event: &mut Event) {}
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TabId(pub usize);

pub struct TabInfo {
    pub is_selected: bool,
}