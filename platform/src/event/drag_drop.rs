use {
    std::cell::{Cell},
    std::rc::Rc,
    crate::{
        makepad_math::*,
        event::{
            finger::{HitOptions,Margin},
            event::{Event, DragHit}
        },
        cx::Cx,
        area::Area,
    },
};


#[derive(Clone, Debug)]
pub struct DragEvent {
    pub handled: Cell<bool>,
    pub abs: DVec2,
    pub state: DragState,
    pub action: Rc<Cell<DragAction >>,
}

#[derive(Clone, Debug)]
pub struct DropEvent {
    pub handled: Cell<bool>,
    pub abs: DVec2,
    pub dragged_item: DraggedItem,
}

#[derive(Debug, PartialEq)]
pub struct DragHitEvent<'a> {
    pub abs: DVec2,
    pub rect: Rect,
    pub state: DragState,
    pub action: &'a Cell<DragAction>,
}

#[derive(Debug, PartialEq)]
pub struct DropHitEvent<'a> {
    pub abs: DVec2,
    pub rect: Rect,
    pub dragged_item: &'a DraggedItem,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragState {
    In,
    Over,
    Out,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragAction {
    None,
    Copy,
    Link,
    Move,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DraggedItem {
    pub file_urls: Vec<String>
}
/*
pub enum HitTouch {
    Single,
    Multi
}*/


// Status


#[derive(Default)]
pub struct CxFingerDrag {
    drag_area: Area,
    next_drag_area: Area,
}

impl CxFingerDrag {
    #[allow(dead_code)]
    pub (crate) fn cycle_drag(&mut self) {
        self.drag_area = self.next_drag_area;
        self.next_drag_area = Area::Empty;
    }
    
    pub (crate) fn update_area(&mut self, old_area: Area, new_area: Area) {
        if self.drag_area == old_area {
            self.drag_area = new_area;
        }
    }
}

impl Event {
    
    pub fn drag_hits(&self, cx: &mut Cx, area: Area) -> DragHit {
        self.drag_hits_with_options(cx, area, HitOptions::default())
    }
    
    pub fn drag_hits_with_options(&self, cx: &mut Cx, area: Area, options: HitOptions) -> DragHit {
        match self {
            Event::Drag(event) => {
                let rect = area.get_clipped_rect(cx);
                if area == cx.finger_drag.drag_area {
                    if !event.handled.get() && Margin::rect_contains_with_margin(&rect, event.abs, &options.margin) {
                        cx.finger_drag.next_drag_area = area;
                        event.handled.set(true);
                        DragHit::Drag(DragHitEvent {
                            rect,
                            abs: event.abs,
                            state: event.state.clone(),
                            action: &event.action
                        })
                    } else {
                        DragHit::Drag(DragHitEvent {
                            rect,
                            state: DragState::Out,
                            abs: event.abs,
                            action: &event.action
                        })
                    }
                } else {
                    if !event.handled.get() && Margin::rect_contains_with_margin(&rect, event.abs, &options.margin) {
                        cx.finger_drag.next_drag_area = area;
                        event.handled.set(true);
                        DragHit::Drag(DragHitEvent {
                            rect,
                            state: DragState::In,
                            abs: event.abs,
                            action: &event.action
                        })
                    } else {
                        DragHit::NoHit
                    }
                }
            }
            Event::Drop(event) => {
                let rect = area.get_clipped_rect(cx);
                if !event.handled.get() && Margin::rect_contains_with_margin(&rect, event.abs, &options.margin) {
                    cx.finger_drag.next_drag_area = Area::default();
                    event.handled.set(true);
                    DragHit::Drop(DropHitEvent {
                        rect,
                        abs: event.abs,
                        dragged_item: &event.dragged_item
                    })
                } else {
                    DragHit::NoHit
                }
            }
            _ => DragHit::NoHit,
        }
    }
    
}
