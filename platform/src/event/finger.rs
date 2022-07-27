use {
    //makepad_microserde::*,
    crate::{
        makepad_math::*,
        event::{
            event::{Event, Hit, DragHit}
        },
        cx::Cx,
        draw_2d::turtle::{Margin},
        area::Area,
    },
};

pub const NUM_FINGERS: usize = 10;

#[derive(Default, Clone)]
pub struct CxDigit {
    captured: Area,
    tap_count: (Vec2, f64, u32),
    down_abs_start: Vec2,
    down_rel_start: Vec2,
}

#[derive(Default, Clone)]
pub struct CxFingers {
    next_over_last: Area,
    over_last: Area,
    captured_mask: u32,
    digits: [CxDigit;NUM_FINGERS]
}

impl CxFingers{
    
    pub(crate) fn any_digits_captured_area(&self, area:Area)->bool{
        for digit in &self.digits {
            if digit.captured == area {
                return true
            }
        }
        false
    }
    
    pub(crate) fn update_area(&mut self, old_area:Area, new_area:Area){
        if self.over_last == old_area{
            self.over_last = new_area;
        }
        if self.captured_mask != 0{
            for digit in &mut self.digits {
                if digit.captured == old_area {
                    digit.captured = new_area
                }
            }
        }
    }
    
    pub(crate) fn cycle_over_last(&mut self){
        self.over_last = self.next_over_last;
        self.next_over_last = Area::Empty;
    }
    
    pub(crate) fn capture_digit(&mut self, digit:usize, area:Area){
        self.captured_mask |= 1<<digit;
        self.digits[digit].captured = area;
    }
    
    pub(crate) fn release_digit(&mut self, digit:usize, ){
        self.captured_mask &= !(1<<digit);
        self.digits[digit].captured = Area::Empty;
    }

    pub (crate) fn process_tap_count(&mut self, digit: usize, pos: Vec2, time: f64) -> u32 {
        if digit >= self.digits.len() {
            return 0
        };
        let (last_pos, last_time, count) = self.digits[digit].tap_count;
        
        if (time - last_time) < 0.5 && pos.distance(&last_pos) < 10. {
            self.digits[digit].tap_count = (pos, time, count + 1);
            count + 1
        }
        else {
            self.digits[digit].tap_count = (pos, time, 1);
            1
        }
    }
}

#[derive(Default)]
pub struct CxFingerDrag{
    drag_area: Area,
    next_drag_area: Area,
}

impl CxFingerDrag{
    pub(crate) fn cycle_drag(&mut self){
        self.drag_area = self.next_drag_area;
        self.next_drag_area = Area::Empty;
    }

    pub(crate) fn update_area(&mut self, old_area:Area, new_area:Area){
        if self.drag_area == old_area {
            self.drag_area = new_area;
        }
    }
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
/*
pub enum HitTouch {
    Single,
    Multi
}*/


// Status


#[derive(Clone, Debug, Default)]
pub struct HitOptions {
    pub use_multi_touch: bool,
    pub margin: Option<Margin>,
}

impl HitOptions{
    pub fn margin(margin:Margin)->Self{
        Self{
            use_multi_touch: false,
            margin: Some(margin)
        }
    }
    pub fn use_multi_touch()->Self{
        Self{
            use_multi_touch: true,
            margin: None
        }
    }
}

fn rect_contains_with_margin(rect: &Rect, pos: Vec2, margin: &Option<Margin>) -> bool {
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
    
    pub fn hits(&mut self, cx: &mut Cx, area: Area) -> Hit {
        self.hits_with_options(cx, area, HitOptions::default())
    }
    
    pub fn hits_with_options(&mut self, cx: &mut Cx, area: Area, options: HitOptions) -> Hit {
        if !area.is_valid(cx){
            return Hit::Nothing
        }
        match self {
            Event::KeyFocus(kf) => {
                if area == kf.prev {
                    return Hit::KeyFocusLost(kf.clone())
                }
                else if area == kf.focus {
                    return Hit::KeyFocus(kf.clone())
                }
            },
            Event::KeyDown(kd) => {
                if cx.keyboard.has_key_focus(area){
                    return Hit::KeyDown(kd.clone())
                }
            },
            Event::KeyUp(ku) => {
                if cx.keyboard.has_key_focus(area){
                    return Hit::KeyUp(ku.clone())
                }
            },
            Event::TextInput(ti) => {
                if cx.keyboard.has_key_focus(area){
                    return Hit::TextInput(ti.clone())
                }
            },
            Event::TextCopy(tc) => {
                if cx.keyboard.has_key_focus(area){
                    return Hit::TextCopy(tc);
                }
            },
            Event::FingerScroll(fe) => {
                let rect = area.get_rect(&cx);
                if rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                    //fe.handled = true;
                    return Hit::FingerScroll(FingerScrollHitEvent {
                        rel: fe.abs - rect.pos,
                        rect: rect,
                        event: fe.clone()
                    })
                }
            },
            Event::FingerHover(fe) => {
                let rect = area.get_rect(&cx);
                if  cx.fingers.over_last == area {
                    let any_down = cx.fingers.any_digits_captured_area(area);
                    if !fe.handled && rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        fe.handled = true;
                        cx.fingers.next_over_last = area;
                        return Hit::FingerHoverOver(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            event: fe.clone()
                        })
                    }
                    else {
                        //self.was_over_last_call = false;
                        return Hit::FingerHoverOut(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            event: fe.clone()
                        })
                    }
                }
                else {
                    if !fe.handled && rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        let any_down = cx.fingers.any_digits_captured_area(area);
                        cx.fingers.next_over_last = area;
                        fe.handled = true;
                        //self.was_over_last_call = true;
                        return Hit::FingerHoverIn(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_down: any_down,
                            event: fe.clone()
                        })
                    }
                }
            },
            Event::FingerMove(fe) => {
                // check wether our digit is captured, otherwise don't send
                if cx.fingers.digits[fe.digit].captured == area  {
                    let abs_start = cx.fingers.digits[fe.digit].down_abs_start;
                    let rel_start = cx.fingers.digits[fe.digit].down_rel_start;
                    let rect = area.get_rect(&cx);
                    return Hit::FingerMove(FingerMoveHitEvent {
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
                        if !options.use_multi_touch && cx.fingers.any_digits_captured_area(area){
                            return Hit::Nothing;
                        }
                        cx.fingers.capture_digit(fe.digit, area);
                        let rel = area.abs_to_rel(cx, fe.abs);
                        cx.fingers.digits[fe.digit].down_abs_start = fe.abs;
                        cx.fingers.digits[fe.digit].down_rel_start = rel;
                        fe.handled = true;
                        return Hit::FingerDown(FingerDownHitEvent {
                            rel: rel,
                            rect: rect,
                            deref_target: fe.clone()
                        })
                    }
                }
            },
            Event::FingerUp(fe) => {
                if cx.fingers.digits[fe.digit].captured == area {
                    cx.fingers.release_digit(fe.digit);
                    let abs_start = cx.fingers.digits[fe.digit].down_abs_start;
                    let rel_start = cx.fingers.digits[fe.digit].down_rel_start;
                    let rect = area.get_rect(&cx);
                    return Hit::FingerUp(FingerUpHitEvent {
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
        Hit::Nothing
    }
    
    pub fn drag_hits(&mut self, cx: &mut Cx, area: Area) -> DragHit {
        self.drag_hits_with_options(cx, area, HitOptions::default())
    }
    
    pub fn drag_hits_with_options(&mut self, cx: &mut Cx, area: Area, options: HitOptions) -> DragHit {
        match self {
            Event::FingerDrag(event) => {
                let rect = area.get_rect(cx);
                if area == cx.finger_drag.drag_area {
                    if !event.handled && rect_contains_with_margin(&rect, event.abs, &options.margin) {
                        cx.finger_drag.next_drag_area = area;
                        event.handled = true;
                        DragHit::FingerDrag(FingerDragHitEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            abs: event.abs,
                            state: event.state.clone(),
                            action: &mut event.action
                        })
                    } else {
                        DragHit::FingerDrag(FingerDragHitEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            state: DragState::Out,
                            abs: event.abs,
                            action: &mut event.action
                        })
                    }
                } else {
                    if !event.handled && rect_contains_with_margin(&rect, event.abs, &options.margin) {
                        cx.finger_drag.next_drag_area = area;
                        event.handled = true;
                        DragHit::FingerDrag(FingerDragHitEvent {
                            rel: area.abs_to_rel(cx, event.abs),
                            rect,
                            state: DragState::In,
                            abs: event.abs,
                            action: &mut event.action
                        })
                    } else {
                        DragHit::NoHit
                    }
                }
            }
            Event::FingerDrop(event) => {
                let rect = area.get_rect(cx);
                if !event.handled && rect_contains_with_margin(&rect, event.abs, &options.margin) {
                    cx.finger_drag.next_drag_area = Area::default();
                    event.handled = true;
                    DragHit::FingerDrop(FingerDropHitEvent {
                        rel: area.abs_to_rel(cx, event.abs),
                        rect,
                        abs: event.abs,
                        dragged_item: &mut event.dragged_item
                    })
                } else {
                    DragHit::NoHit
                }
            }
            _ => DragHit::NoHit,
        }
    }
    
}
