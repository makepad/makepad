use {
    crate::{
        area::Area,
        cx::Cx,
        file_dialogs::VirtualFile,
        event::{
            event::{DragHit, Event},
            finger::{HitOptions, Inset},
            KeyModifiers,
        },
        makepad_live_id::*,
        makepad_math::*,
        thread::lock_from_ui,
    },
    std::sync::Arc,
    std::sync::Mutex,
};

#[derive(Clone, Debug)]
pub struct DragEvent {
    pub modifiers: KeyModifiers,
    pub handled: Arc<Mutex<bool>>,
    pub abs: Vec2d,
    pub items: Arc<Vec<DragItem>>,
    pub response: Arc<Mutex<DragResponse>>,
}

#[derive(Clone, Debug)]
pub struct DropEvent {
    pub modifiers: KeyModifiers,
    pub handled: Arc<Mutex<bool>>,
    pub abs: Vec2d,
    pub items: Arc<Vec<DragItem>>,
}

#[derive(Clone, Debug)]
pub struct DragHitEvent {
    pub modifiers: KeyModifiers,
    pub abs: Vec2d,
    pub rect: Rect,
    pub state: DragState,
    pub items: Arc<Vec<DragItem>>,
    pub response: Arc<Mutex<DragResponse>>,
}

#[derive(Clone, Debug)]
pub struct DropHitEvent {
    pub modifiers: KeyModifiers,
    pub abs: Vec2d,
    pub rect: Rect,
    pub items: Arc<Vec<DragItem>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
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
    FilePath {
        path: String,
        internal_id: Option<LiveId>,
    },
    String {
        value: String,
        internal_id: Option<LiveId>,
    },
    /// A browser-selected file whose bytes live only in the application.
    /// Web hover events use empty values as count/type placeholders; the
    /// values on [`Event::Drop`] always contain the complete file bytes.
    VirtualFile(VirtualFile),
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
    #[cfg(any(target_arch = "wasm32", target_os = "linux", test))]
    internal_drag_items: Option<Arc<Vec<DragItem>>>,
    #[cfg(any(target_arch = "wasm32", target_os = "linux", test))]
    internal_drag_moved: bool,
}

#[cfg(any(target_arch = "wasm32", target_os = "linux", test))]
pub(crate) enum InternalDragEvent {
    Drag(DragEvent),
    Drop(DropEvent),
}

impl CxDragDrop {
    #[cfg(any(target_arch = "wasm32", target_os = "linux", test))]
    pub(crate) fn start_internal_drag(&mut self, items: Vec<DragItem>) {
        assert!(self.internal_drag_items.is_none(), "start drag twice");
        self.internal_drag_items = Some(Arc::new(items));
        self.internal_drag_moved = false;
    }

    #[cfg(any(target_arch = "wasm32", target_os = "linux", test))]
    pub(crate) fn internal_drag_event(&mut self, event: &Event) -> Option<InternalDragEvent> {
        match event {
            Event::MouseMove(event) => {
                let items = self.internal_drag_items.as_ref()?.clone();
                self.internal_drag_moved = true;
                Some(InternalDragEvent::Drag(DragEvent {
                    modifiers: event.modifiers,
                    handled: Arc::new(Mutex::new(false)),
                    abs: event.abs,
                    items,
                    response: Arc::new(Mutex::new(DragResponse::None)),
                }))
            }
            Event::MouseUp(event) if event.button.is_primary() => {
                let items = self.internal_drag_items.take()?;
                if !std::mem::take(&mut self.internal_drag_moved) {
                    return None;
                }
                Some(InternalDragEvent::Drop(DropEvent {
                    modifiers: event.modifiers,
                    handled: Arc::new(Mutex::new(false)),
                    abs: event.abs,
                    items,
                }))
            }
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn cycle_drag(&mut self) {
        self.drag_area = self.next_drag_area;
        self.next_drag_area = Area::Empty;
    }

    pub(crate) fn update_area(&mut self, old_area: Area, new_area: Area) {
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
                let rect = area.clipped_rect(cx);
                if area == cx.drag_drop.drag_area {
                    if !*lock_from_ui(&event.handled)
                        && Inset::rect_contains_with_inset(event.abs, &rect, &options.margin)
                    {
                        //log!("drag_hist_with_options: Drag, in drag area, event handled and rect ({:?}) contains ({},{}) with margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                        cx.drag_drop.next_drag_area = area;
                        *lock_from_ui(&event.handled) = true;
                        DragHit::Drag(DragHitEvent {
                            rect,
                            modifiers: event.modifiers,
                            abs: event.abs,
                            items: event.items.clone(),
                            state: DragState::Over,
                            response: event.response.clone(),
                        })
                    } else {
                        //log!("drag_hist_with_options: Drag, in drag area, event not handled or rect ({:?}) doesn't contain ({},{}) with margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                        DragHit::Drag(DragHitEvent {
                            rect,
                            modifiers: event.modifiers,
                            state: DragState::Out,
                            items: event.items.clone(),
                            abs: event.abs,
                            response: event.response.clone(),
                        })
                    }
                } else {
                    if !*lock_from_ui(&event.handled)
                        && Inset::rect_contains_with_inset(event.abs, &rect, &options.margin)
                    {
                        //log!("drag_hits_with_options: Drag, not in drag_area, event not handled and rect ({:?}) contains ({},{}) with margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                        cx.drag_drop.next_drag_area = area;
                        *lock_from_ui(&event.handled) = true;
                        DragHit::Drag(DragHitEvent {
                            modifiers: event.modifiers,
                            rect,
                            state: DragState::In,
                            items: event.items.clone(),
                            abs: event.abs,
                            response: event.response.clone(),
                        })
                    } else {
                        //log!("drag_hits_with_options: Drag, not in drag_area, event handled or rect ({:?}) doesn't contain ({},{}) with margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                        DragHit::NoHit
                    }
                }
            }
            Event::Drop(event) => {
                let rect = area.clipped_rect(cx);
                if !*lock_from_ui(&event.handled)
                    && Inset::rect_contains_with_inset(event.abs, &rect, &options.margin)
                {
                    //log!("drag_hits_with_options: Drop, event not handled and rect {:?} contains ({},{}) in margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                    cx.drag_drop.next_drag_area = Area::default();
                    *lock_from_ui(&event.handled) = true;
                    DragHit::Drop(DropHitEvent {
                        modifiers: event.modifiers,
                        rect,
                        abs: event.abs,
                        items: event.items.clone(),
                    })
                } else {
                    //log!("drag_hits_with_options: Drop, event handled or rect {:?} doesn't contain ({},{}) in margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                    DragHit::NoHit
                }
            }
            _ => DragHit::NoHit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::{MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent},
        window::WindowId,
    };
    use std::{cell::Cell, cell::RefCell, rc::Rc};

    #[test]
    fn internal_drag_replaces_moves_and_release_with_drag_drop_end() {
        let sequence = Rc::new(RefCell::new(Vec::new()));
        let seen = sequence.clone();
        let item = DragItem::String {
            value: "effect".to_string(),
            internal_id: Some(LiveId(7)),
        };
        let dragged_item = item.clone();
        let mut cx = Cx::new(Box::new(move |cx, event| {
            seen.borrow_mut().push(event.name());
            if matches!(event, Event::MouseDown(_)) {
                cx.start_dragging(vec![dragged_item.clone()]);
            }
        }));
        let window_id = WindowId(0, 0);

        cx.call_event_handler(&Event::MouseDown(MouseDownEvent {
            abs: dvec2(10.0, 20.0),
            button: MouseButton::PRIMARY,
            window_id,
            modifiers: Default::default(),
            handled: Cell::new(Area::Empty),
            time: 1.0,
        }));
        for (time, abs) in [(2.0, dvec2(20.0, 30.0)), (3.0, dvec2(30.0, 40.0))] {
            cx.call_event_handler(&Event::MouseMove(MouseMoveEvent {
                abs,
                lock_delta: Default::default(),
                window_id,
                modifiers: Default::default(),
                time,
                handled: Cell::new(Area::Empty),
            }));
        }
        cx.call_event_handler(&Event::MouseUp(MouseUpEvent {
            abs: dvec2(40.0, 50.0),
            button: MouseButton::PRIMARY,
            window_id,
            modifiers: Default::default(),
            time: 4.0,
        }));

        assert_eq!(
            sequence.borrow().as_slice(),
            ["MouseDown", "Drag", "Drag", "Drop", "DragEnd"]
        );
    }

    #[test]
    fn internal_drag_without_movement_remains_a_click() {
        let sequence = Rc::new(RefCell::new(Vec::new()));
        let seen = sequence.clone();
        let mut cx = Cx::new(Box::new(move |cx, event| {
            seen.borrow_mut().push(event.name());
            if matches!(event, Event::MouseDown(_)) {
                cx.start_dragging(vec![DragItem::String {
                    value: "effect".to_string(),
                    internal_id: Some(LiveId(7)),
                }]);
            }
        }));
        let window_id = WindowId(0, 0);

        cx.call_event_handler(&Event::MouseDown(MouseDownEvent {
            abs: dvec2(10.0, 20.0),
            button: MouseButton::PRIMARY,
            window_id,
            modifiers: Default::default(),
            handled: Cell::new(Area::Empty),
            time: 1.0,
        }));
        cx.call_event_handler(&Event::MouseUp(MouseUpEvent {
            abs: dvec2(10.0, 20.0),
            button: MouseButton::PRIMARY,
            window_id,
            modifiers: Default::default(),
            time: 2.0,
        }));

        assert_eq!(sequence.borrow().as_slice(), ["MouseDown", "MouseUp"]);
    }
}
