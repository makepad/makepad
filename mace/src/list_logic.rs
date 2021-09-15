use {crate::id::Id, makepad_render::*, std::collections::HashMap};

#[derive(Default)]
pub struct ListLogic {
    item_ids_by_area: HashMap<Area, ItemId>,
    selected_item_id: Option<ItemId>,
}

impl ListLogic {
    pub fn new() -> ListLogic {
        ListLogic::default()
    }

    pub fn begin(&mut self) {
        self.item_ids_by_area.clear();
    }

    pub fn end(&mut self) {}

    pub fn begin_item(&mut self, item_id: ItemId) -> ItemInfo {
        ItemInfo {
            is_selected: self
                .selected_item_id
                .map_or(false, |selected_item_id| selected_item_id == item_id),
        }
    }

    pub fn end_item(&mut self) {}

    pub fn set_item_area(&mut self, item_id: ItemId, area: Area) {
        self.item_ids_by_area.insert(area, item_id);
    }

    pub fn selected_item_id(&self) -> Option<ItemId> {
        self.selected_item_id
    }

    pub fn set_selected_item_id(&mut self, item_id: Option<ItemId>) -> bool {
        if self.selected_item_id == item_id {
            return false;
        }
        self.selected_item_id = item_id;
        true
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(Action),
    ) {
        for (area, item_id) in &self.item_ids_by_area {
            match event.hits(cx, *area, HitOpt::default()) {
                Event::FingerDown(_) => {
                    dispatch_action(Action::ItemWasPressed(*item_id));
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ItemId(Id);

impl AsRef<Id> for ItemId {
    fn as_ref(&self) -> &Id {
        &self.0
    }
}

pub struct Item;

#[derive(Clone, Copy, Debug)]
pub struct ItemInfo {
    pub is_selected: bool,
}

pub enum Action {
    ItemWasPressed(ItemId),
}
