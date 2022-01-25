use {
    //makepad_microserde::*,
    crate::{
        makepad_math::*,
        event::{
            event::{Event, HitEvent, TriggerHitEvent, DragEvent}
        },
        cx::Cx,
        draw_2d::turtle::{Margin},
        area::Area,
    },
};

pub const NUM_FINGERS: usize = 10;


#[derive(Default, Clone)]
pub struct CxPerFinger {
    pub captured: Area,
    pub tap_count: (Vec2, f64, u32),
    pub down_abs_start: Vec2,
    pub down_rel_start: Vec2,
    pub over_last: Area,
    pub _over_last: Area
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool
}

#[derive(Clone, Debug, PartialEq)]
pub enum FingerInputType {
    Mouse,
    Touch,
    XR
}

impl FingerInputType {
    pub fn is_touch(&self) -> bool {*self == FingerInputType::Touch}
    pub fn is_mouse(&self) -> bool {*self == FingerInputType::Mouse}
    pub fn is_xr(&self) -> bool {*self == FingerInputType::XR}
    pub fn has_hovers(&self) -> bool {*self == FingerInputType::Mouse || *self == FingerInputType::XR}
}

impl Default for FingerInputType {
    fn default() -> Self {Self::Mouse}
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerDownEvent {
    pub window_id: usize,
    pub abs: Vec2,
    pub digit: usize,
    pub tap_count: u32,
    pub handled: bool,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerDownHitEvent {
    pub rel: Vec2,
    pub rect: Rect,
    pub deref_target: FingerDownEvent
}

impl std::ops::Deref for FingerDownHitEvent {
    type Target = FingerDownEvent;
    fn deref(&self) -> &Self::Target {&self.deref_target}
}

impl std::ops::DerefMut for FingerDownHitEvent {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.deref_target}
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerMoveEvent {
    pub window_id: usize,
    pub abs: Vec2,
    pub digit: usize,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerMoveHitEvent {
    pub abs_start: Vec2,
    pub rel: Vec2,
    pub rel_start: Vec2,
    pub rect: Rect,
    pub is_over: bool,
    pub deref_target: FingerMoveEvent,
}

impl std::ops::Deref for FingerMoveHitEvent {
    type Target = FingerMoveEvent;
    fn deref(&self) -> &Self::Target {&self.deref_target}
}

impl std::ops::DerefMut for FingerMoveHitEvent {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.deref_target}
}


impl FingerMoveHitEvent {
    pub fn move_distance(&self) -> f32 {
        ((self.abs_start.x - self.abs.x).powf(2.) + (self.abs_start.y - self.abs.y).powf(2.)).sqrt()
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerUpEvent {
    pub window_id: usize,
    pub abs: Vec2,
    pub digit: usize,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerUpHitEvent {
    pub rel: Vec2,
    pub abs_start: Vec2,
    pub rel_start: Vec2,
    pub rect: Rect,
    pub is_over: bool,
    pub deref_target: FingerUpEvent
}

impl std::ops::Deref for FingerUpHitEvent {
    type Target = FingerUpEvent;
    fn deref(&self) -> &Self::Target {&self.deref_target}
}

impl std::ops::DerefMut for FingerUpHitEvent {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.deref_target}
}

#[derive(Clone, Debug, PartialEq)]
pub enum HoverState {
    In,
    Over,
    Out
}

impl Default for HoverState {
    fn default() -> HoverState {
        HoverState::Over
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerHoverEvent {
    pub window_id: usize,
    pub digit: usize,
    pub abs: Vec2,
    pub handled: bool,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerHoverHitEvent {
    pub rel: Vec2,
    pub rect: Rect,
    pub any_down: bool,
    pub hover_state: HoverState,
    pub event: FingerHoverEvent
}

impl std::ops::Deref for FingerHoverHitEvent {
    type Target = FingerHoverEvent;
    fn deref(&self) -> &Self::Target {&self.event}
}

impl std::ops::DerefMut for FingerHoverHitEvent {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.event}
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerScrollEvent {
    pub window_id: usize,
    pub digit: usize,
    pub abs: Vec2,
    pub scroll: Vec2,
    pub input_type: FingerInputType,
    pub handled_x: bool,
    pub handled_y: bool,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FingerScrollHitEvent {
    pub rel: Vec2,
    pub rect: Rect,
    pub event: FingerScrollEvent
}

impl std::ops::Deref for FingerScrollHitEvent {
    type Target = FingerScrollEvent;
    fn deref(&self) -> &Self::Target {&self.event}
}

impl std::ops::DerefMut for FingerScrollHitEvent {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.event}
}


#[derive(Clone, Debug, PartialEq)]
pub struct FingerDragEvent {
    pub handled: bool,
    pub abs: Vec2,
    pub state: DragState,
    pub action: DragAction,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FingerDropEvent {
    pub handled: bool,
    pub abs: Vec2,
    pub dragged_item: DraggedItem,
}

#[derive(Debug, PartialEq)]
pub struct FingerDragHitEvent<'a> {
    pub abs: Vec2,
    pub rel: Vec2,
    pub rect: Rect,
    pub state: DragState,
    pub action: &'a mut DragAction,
}

#[derive(Debug, PartialEq)]
pub struct FingerDropHitEvent<'a> {
    pub abs: Vec2,
    pub rel: Vec2,
    pub rect: Rect,
    pub dragged_item: &'a mut DraggedItem,
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

pub enum HitTouch {
    Single,
    Multi
}


// Status


#[derive(Clone, Debug, Default)]
pub struct HitOptions {
    pub use_multi_touch: bool,
    pub margin: Option<Margin>,
}

pub fn rect_contains_with_margin(rect: &Rect, pos: Vec2, margin: &Option<Margin>) -> bool {
    if let Some(margin) = margin {
        return
        pos.x >= rect.pos.x - margin.left
            && pos.x <= rect.pos.x + rect.size.x + margin.right
            && pos.y >= rect.pos.y - margin.top
            && pos.y <= rect.pos.y + rect.size.y + margin.bottom;
    }
    else {
        return rect.contains(pos);
    }
}

impl Event {
    
    
    pub fn hits(&mut self, cx: &mut Cx, area: Area) -> HitEvent {
        self.hits_with_options(cx, area, HitOptions::default())
    }
    
    pub fn hits_with_options(&mut self, cx: &mut Cx, area: Area, options: HitOptions) -> HitEvent {
        if !area.is_valid(cx){
            return HitEvent::None
        }
        match self {
            Event::Trigger(te)=>{
                if let Some(data) = te.triggers.get(&area){
                    return HitEvent::Trigger(TriggerHitEvent(data))
                }
            },
            Event::KeyFocus(kf) => {
                if area == kf.prev {
                    return HitEvent::KeyFocusLost(kf.clone())
                }
                else if area == kf.focus {
                    return HitEvent::KeyFocus(kf.clone())
                }
            },
            Event::KeyDown(kd) => {
                if area == cx.key_focus {
                    return HitEvent::KeyDown(kd.clone())
                }
            },
            Event::KeyUp(ku) => {
                if area == cx.key_focus {
                    return HitEvent::KeyUp(ku.clone())
                }
            },
            Event::TextInput(ti) => {
                if area == cx.key_focus {
                    return HitEvent::TextInput(ti.clone())
                }
            },
            Event::TextCopy(tc) => {
                if area == cx.key_focus {
                    return HitEvent::TextCopy(tc);
                }
            },
            Event::FingerScroll(fe) => {
                let rect = area.get_rect(&cx);
                if rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                    //fe.handled = true;
                    return HitEvent::FingerScroll(FingerScrollHitEvent {
                        rel: fe.abs - rect.pos,
                        rect: rect,
                        event: fe.clone()
                    })
                }
            },
            Event::FingerHover(fe) => {
                let rect = area.get_rect(&cx);
                
                if  cx.fingers[fe.digit]._over_last == area {
                    let mut any_down = false;
                    for finger in &cx.fingers {
                        if finger.captured == area {
                            any_down = true;
                            break;
                        }
                    }
                    if !fe.handled && rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        fe.handled = true;
                        //if let HoverState::Out = fe.hover_state {
                        //    cx.finger_over_last_area = Area::Empty;
                        //}
                        //else {
                        cx.fingers[fe.digit].over_last = area;
                        // }
                        return HitEvent::FingerHover(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            hover_state: HoverState::Over,
                            event: fe.clone()
                        })
                    }
                    else {
                        //self.was_over_last_call = false;
                        return HitEvent::FingerHover(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            hover_state: HoverState::Out,
                            event: fe.clone()
                        })
                    }
                }
                else {
                    if !fe.handled && rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        let mut any_down = false;
                        for finger in &cx.fingers {
                            if finger.captured == area {
                                any_down = true;
                                break;
                            }
                        }
                        cx.fingers[fe.digit].over_last = area;
                        fe.handled = true;
                        //self.was_over_last_call = true;
                        return HitEvent::FingerHover(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            hover_state: HoverState::In,
                            event: fe.clone()
                        })
                    }
                }
            },
            Event::FingerMove(fe) => {
                // check wether our digit is captured, otherwise don't send
                if cx.fingers[fe.digit].captured == area {
                    let abs_start = cx.fingers[fe.digit].down_abs_start;
                    let rel_start = cx.fingers[fe.digit].down_rel_start;
                    let rect = area.get_rect(&cx);
                    return HitEvent::FingerMove(FingerMoveHitEvent {
                        abs_start: abs_start,
                        rel: area.abs_to_rel(cx, fe.abs),
                        rel_start: rel_start,
                        rect: rect,
                        is_over: rect_contains_with_margin(&rect, fe.abs, &options.margin),
                        deref_target: fe.clone()
                    })
                }
            },
            Event::FingerDown(fe) => {
                if !fe.handled {
                    let rect = area.get_rect(&cx);
                    if rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        // scan if any of the fingers already captured this area
                        if !options.use_multi_touch {
                            for finger in &cx.fingers {
                                if finger.captured == area {
                                    return HitEvent::None;
                                }
                            }
                        }
                        cx.fingers[fe.digit].captured = area;
                        let rel = area.abs_to_rel(cx, fe.abs);
                        cx.fingers[fe.digit].down_abs_start = fe.abs;
                        cx.fingers[fe.digit].down_rel_start = rel;
                        fe.handled = true;
                        return HitEvent::FingerDown(FingerDownHitEvent {
                            rel: rel,
                            rect: rect,
                            deref_target: fe.clone()
                        })
                    }
                }
            },
            Event::FingerUp(fe) => {
                if cx.fingers[fe.digit].captured == area {
                    cx.fingers[fe.digit].captured = Area::Empty;
                    let abs_start = cx.fingers[fe.digit].down_abs_start;
                    let rel_start = cx.fingers[fe.digit].down_rel_start;
                    let rect = area.get_rect(&cx);
                    return HitEvent::FingerUp(FingerUpHitEvent {
                        is_over: rect.contains(fe.abs),
                        abs_start: abs_start,
                        rel_start: rel_start,
                        rel: area.abs_to_rel(cx, fe.abs),
                        rect: rect,
                        deref_target: fe.clone()
                    })
                }
            },
            _ => ()
        };
        HitEvent::None
    }
    
    pub fn drag_hits(&mut self, cx: &mut Cx, area: Area) -> DragEvent {
        self.drag_hits_with_options(cx, area, HitOptions::default())
    }
    
    pub fn drag_hits_with_options(&mut self, cx: &mut Cx, area: Area, options: HitOptions) -> DragEvent {
        match self {
            Event::FingerDrag(event) => {
                let rect = area.get_rect(cx);
                if area == cx.drag_area {
                    if !event.handled && rect_contains_with_margin(&rect, event.abs, &options.margin) {
                        cx.new_drag_area = area;
                        event.handled = true;
                        DragEvent::FingerDrag(FingerDragHitEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            abs: event.abs,
                            state: event.state.clone(),
                            action: &mut event.action
                        })
                    } else {
                        DragEvent::FingerDrag(FingerDragHitEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            state: DragState::Out,
                            abs: event.abs,
                            action: &mut event.action
                        })
                    }
                } else {
                    if !event.handled && rect_contains_with_margin(&rect, event.abs, &options.margin) {
                        cx.new_drag_area = area;
                        event.handled = true;
                        DragEvent::FingerDrag(FingerDragHitEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            state: DragState::In,
                            abs: event.abs,
                            action: &mut event.action
                        })
                    } else {
                        DragEvent::None
                    }
                }
            }
            Event::FingerDrop(event) => {
                let rect = area.get_rect(cx);
                if !event.handled && rect_contains_with_margin(&rect, event.abs, &options.margin) {
                    cx.new_drag_area = Area::default();
                    event.handled = true;
                    DragEvent::FingerDrop(FingerDropHitEvent {
                        rel: area.abs_to_rel(cx, event.abs),
                        rect,
                        abs: event.abs,
                        dragged_item: &mut event.dragged_item
                    })
                } else {
                    DragEvent::None
                }
            }
            _ => DragEvent::None,
        }
    }
    
}
