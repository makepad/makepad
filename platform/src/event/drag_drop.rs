use {
    std::cell::{Cell},
    std::rc::Rc,
    crate::{
        makepad_live_id::*,
        makepad_math::*,
        event::{
            KeyModifiers,
            finger::{HitOptions, Margin},
            event::{Event, DragHit}
        },
        cx::Cx,
        area::Area,
    },
};


#[derive(Clone, Debug)]
pub struct DragEvent {
    pub modifiers: KeyModifiers,
    pub handled: Cell<bool>,
    pub abs: DVec2,
    pub items: Rc<Vec<DragItem >>,
    pub response: Rc<Cell<DragResponse >>,
}

#[derive(Clone, Debug)]
pub struct DropEvent {
    pub modifiers: KeyModifiers,
    pub handled: Cell<bool>,
    pub abs: DVec2,
    pub items: Rc<Vec<DragItem >>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DragHitEvent {
    pub modifiers: KeyModifiers,
    pub abs: DVec2,
    pub rect: Rect,
    pub state: DragState,
    pub items: Rc<Vec<DragItem >>,
    pub response: Rc<Cell<DragResponse >>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DropHitEvent {
    pub modifiers: KeyModifiers,
    pub abs: DVec2,
    pub rect: Rect,
    pub items: Rc<Vec<DragItem >>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragState {
    In,
    Over,
    Out,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragResponse {
    None,
    Copy,
    Link,
    Move,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragItem {
    FilePath {path: String, internal_id: Option<LiveId>},
    String {value: String, internal_id: Option<LiveId>}
}

/*
pub enum HitTouch {
    Single,
    Multi
}*/


// Status


#[derive(Default)]
pub struct CxDragDrop {
    drag_area: Area,
    next_drag_area: Area,
}

impl CxDragDrop {
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
                if area == cx.drag_drop.drag_area {
                    if !event.handled.get() && Margin::rect_contains_with_margin(&rect, event.abs, &options.margin) {
                        cx.drag_drop.next_drag_area = area;
                        event.handled.set(true);
                        DragHit::Drag(DragHitEvent {
                            rect,
                            modifiers: event.modifiers,
                            abs: event.abs,
                            items: event.items.clone(),
                            state: DragState::Over,
                            response: event.response.clone()
                        })
                    } else {
                        DragHit::Drag(DragHitEvent {
                            rect,
                            modifiers: event.modifiers,
                            state: DragState::Out,
                            items: event.items.clone(),
                            abs: event.abs,
                            response: event.response.clone()
                        })
                    }
                } else {
                    if !event.handled.get() && Margin::rect_contains_with_margin(&rect, event.abs, &options.margin) {
                        cx.drag_drop.next_drag_area = area;
                        event.handled.set(true);
                        DragHit::Drag(DragHitEvent {
                            modifiers: event.modifiers,
                            rect,
                            state: DragState::In,
                            items: event.items.clone(),
                            abs: event.abs,
                            response: event.response.clone()
                        })
                    } else {
                        DragHit::NoHit
                    }
                }
            }
            Event::Drop(event) => {
                let rect = area.get_clipped_rect(cx);
                if !event.handled.get() && Margin::rect_contains_with_margin(&rect, event.abs, &options.margin) {
                    cx.drag_drop.next_drag_area = Area::default();
                    event.handled.set(true);
                    DragHit::Drop(DropHitEvent {
                        modifiers: event.modifiers,
                        rect,
                        abs: event.abs,
                        items: event.items.clone()
                    })
                } else {
                    DragHit::NoHit
                }
            }
            _ => DragHit::NoHit,
        }
    }
    
}
