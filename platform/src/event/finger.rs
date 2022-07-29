use {
    crate::{
        makepad_error_log::*,
        makepad_math::*,
        makepad_live_id::{LiveId, FromLiveId},
        event::{
            event::{Event, Hit, DragHit}
        },
        window::WindowId,
        cx::Cx,
        draw_2d::turtle::{Margin},
        area::Area,
    },
};

pub const TAP_COUNT_TIME: f64 = 0.5;
pub const TAP_COUNT_DISTANCE: f32 = 10.0;

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct DigitId(pub LiveId);

#[derive(Default, Clone)]
pub struct CxDigit {
    digit_id: DigitId,
    pub captured: Area,
    pub down_abs_start: Vec2,
    pub down_rel_start: Vec2,
}

#[derive(Default, Clone)]
pub struct CxDigitTap {
    digit_id: DigitId,
    last_pos: Vec2,
    last_time: f64,
    count: u32
}

#[derive(Default, Clone)]
pub struct CxDigitHover {
    digit_id: DigitId,
    new_area: Area,
    area: Area,
}

#[derive(Default, Clone)]
pub struct CxFingers {
    capture_count: u32,
    digits: Vec<CxDigit>,
    taps: Vec<CxDigitTap>,
    hovers: Vec<CxDigitHover>,
}

impl CxFingers {
    pub (crate) fn alloc_digit(&mut self, digit_id: DigitId) {
        for cxdigit in self.digits.iter_mut() {
            if cxdigit.digit_id == digit_id {
                error!("Double alloc for digit");
                return
            }
        }
        self.digits.push(CxDigit {
            digit_id,
            ..Default::default()
        });
    }
    
    pub (crate) fn free_digit(&mut self, digit_id: DigitId) {
        if let Some(index) = self.digits.iter_mut().position( | v | v.digit_id == digit_id) {
            self.capture_count -= 1;
            self.digits.remove(index);
            return
        }
    }
    
    pub (crate) fn get_digit(&self, digit_id: DigitId) -> Option<&CxDigit> {
        self.digits.iter().find( | v | v.digit_id == digit_id)
    }
    
    pub (crate) fn get_digit_mut(&mut self, digit_id: DigitId) -> Option<&mut CxDigit> {
        self.digits.iter_mut().find( | v | v.digit_id == digit_id)
    }
    
    pub (crate) fn is_digit_allocated(&mut self, digit_id: DigitId) -> bool {
        self.digits.iter_mut().position( | v | v.digit_id == digit_id).is_some()
    }
    
    pub (crate) fn get_captured_area(&self, digit_id: DigitId) -> Area {
        if let Some(cxdigit) = self.digits.iter().find( | v | v.digit_id == digit_id) {
            cxdigit.captured
        }
        else {
            Area::Empty
        }
    }
    
    pub (crate) fn get_captured_area_digit(&self, area: Area) -> Option<DigitId> {
        if self.capture_count == 0 {
            return None
        }
        if let Some(digit) = self.digits.iter().find( | d | d.captured == area) {
            return Some(digit.digit_id)
        }
        None
    }
    
    pub (crate) fn update_area(&mut self, old_area: Area, new_area: Area) {
        for hover in &mut self.hovers {
            if hover.area == old_area {
                hover.area = new_area;
            }
        }
        if self.capture_count != 0 {
            for digit in &mut self.digits {
                if digit.captured == old_area {
                    digit.captured = new_area;
                    return
                }
            }
        }
    }
    
    pub (crate) fn new_hover_area(&mut self, digit_id: DigitId, new_area: Area) {
        for hover in &mut self.hovers {
            if hover.digit_id == digit_id {
                hover.new_area = new_area;
                return
            }
        }
        self.hovers.push(CxDigitHover {
            digit_id,
            area: Area::Empty,
            new_area: new_area,
        })
    }
    
    pub (crate) fn get_hover_area(&mut self, digit: DigitId) -> Area {
        for hover in &mut self.hovers {
            if hover.digit_id == digit {
                return hover.area
            }
        }
        Area::Empty
    }
    
    pub (crate) fn cycle_hover_area(&mut self, digit_id: DigitId) {
        if let Some(hover) = self.hovers.iter_mut().find( | v | v.digit_id == digit_id) {
            hover.area = hover.new_area;
            hover.new_area = Area::Empty;
        }
    }
    
    pub (crate) fn capture_digit(&mut self, digit_id: DigitId, area: Area)->bool{
        if let Some(cxdigit) = self.digits.iter_mut().find( | v | v.digit_id == digit_id) {
            self.capture_count += 1;
            cxdigit.captured = area;
            return true
        }
        false
    }
    
    pub (crate) fn release_digit(&mut self, digit_id: DigitId) {
        if let Some(cxdigit) = self.digits.iter_mut().find( | v | v.digit_id == digit_id) {
            self.capture_count -= 1;
            cxdigit.captured = Area::Empty;
        }
    }
    
    pub (crate) fn process_tap_count(&mut self, digit_id: DigitId, pos: Vec2, time: f64) -> u32 {
        self.taps.retain( | tap | (time - tap.last_time) < TAP_COUNT_TIME);
        
        if let Some(tap) = self.taps.iter_mut().find( | v | v.digit_id == digit_id) {
            if pos.distance(&tap.last_pos) < TAP_COUNT_DISTANCE {
                tap.count += 1;
            }
            else {
                tap.count = 1;
            }
            tap.last_pos = pos;
            tap.last_time = time;
            return tap.count
        }
        self.taps.push(CxDigitTap {
            digit_id,
            last_pos: pos,
            last_time: time,
            count: 1
        });
        1
    }
}

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

#[derive(Clone, Debug, PartialEq, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool
}

#[derive(Clone, Debug, PartialEq)]
pub enum FingerInputType {
    Mouse(usize),
    Touch(usize),
    XR(usize)
}

impl FingerInputType {
    pub fn is_touch(&self) -> bool {if let FingerInputType::Touch(_) = self {true}else {false}}
    pub fn is_mouse(&self) -> bool {if let FingerInputType::Mouse(_) = self {true}else {false}}
    pub fn is_xr(&self) -> bool {if let FingerInputType::XR(_) = self {true}else {false}}
    pub fn has_hovers(&self) -> bool {self.is_mouse() || self.is_xr()}
    pub fn get_down_mutex(&self) -> usize {
        match self {
            Self::Mouse(btn) => *btn,
            Self::Touch(_) => 0,
            Self::XR(_) => 0
        }
    }
    
}

impl Default for FingerInputType {
    fn default() -> Self {Self::Mouse(0)}
}

#[derive(Clone, Debug, PartialEq)]
pub struct FingerDownEvent {
    pub window_id: WindowId,
    pub abs: Vec2,
    pub digit_id: DigitId,
    pub tap_count: u32,
    pub handled: bool,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl FingerDownEvent {
    pub fn mod_control(&self) -> bool {self.modifiers.control}
    pub fn mod_alt(&self) -> bool {self.modifiers.alt}
    pub fn mod_shift(&self) -> bool {self.modifiers.shift}
    pub fn mod_logo(&self) -> bool {self.modifiers.logo}
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct FingerMoveEvent {
    pub window_id: WindowId,
    pub abs: Vec2,
    pub captured: Area,
    pub digit_id: DigitId,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct FingerUpEvent {
    pub window_id: WindowId,
    pub abs: Vec2,
    pub captured: Area,
    pub digit_id: DigitId,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct FingerHoverEvent {
    pub window_id: WindowId,
    pub abs: Vec2,
    pub digit_id: DigitId,
    pub hover_last: Area,
    pub handled: bool,
    pub input_type: FingerInputType,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug, PartialEq)]
pub struct FingerHoverHitEvent {
    pub rel: Vec2,
    pub rect: Rect,
    pub any_captured: Option<DigitId>,
    pub event: FingerHoverEvent
}

impl std::ops::Deref for FingerHoverHitEvent {
    type Target = FingerHoverEvent;
    fn deref(&self) -> &Self::Target {&self.event}
}

impl std::ops::DerefMut for FingerHoverHitEvent {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.event}
}

#[derive(Clone, Debug, PartialEq)]
pub struct FingerScrollEvent {
    pub window_id: WindowId,
    pub digit_id: DigitId,
    pub abs: Vec2,
    pub scroll: Vec2,
    pub input_type: FingerInputType,
    pub handled_x: bool,
    pub handled_y: bool,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug, PartialEq)]
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

impl HitOptions {
    pub fn margin(margin: Margin) -> Self {
        Self {
            use_multi_touch: false,
            margin: Some(margin)
        }
    }
    pub fn use_multi_touch() -> Self {
        Self {
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
        if !area.is_valid(cx) {
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
                if cx.keyboard.has_key_focus(area) {
                    return Hit::KeyDown(kd.clone())
                }
            },
            Event::KeyUp(ku) => {
                if cx.keyboard.has_key_focus(area) {
                    return Hit::KeyUp(ku.clone())
                }
            },
            Event::TextInput(ti) => {
                if cx.keyboard.has_key_focus(area) {
                    return Hit::TextInput(ti.clone())
                }
            },
            Event::TextCopy(tc) => {
                if cx.keyboard.has_key_focus(area) {
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
                if fe.hover_last == area {
                    let any_captured = cx.fingers.get_captured_area_digit(area);
                    if !fe.handled && rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        fe.handled = true;
                        cx.fingers.new_hover_area(fe.digit_id, area);
                        return Hit::FingerHoverOver(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_captured,
                            event: fe.clone()
                        })
                    }
                    else {
                        return Hit::FingerHoverOut(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_captured,
                            event: fe.clone()
                        })
                    }
                }
                else {
                    if !fe.handled && rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        let any_captured = cx.fingers.get_captured_area_digit(area);
                        cx.fingers.new_hover_area(fe.digit_id, area);
                        fe.handled = true;
                        return Hit::FingerHoverIn(FingerHoverHitEvent {
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            any_captured,
                            event: fe.clone()
                        })
                    }
                }
            },
            Event::FingerMove(fe) => {
                // check wether our digit is captured, otherwise don't send
                if let Some(digit) = cx.fingers.get_digit(fe.digit_id) {
                    if digit.captured == area {
                        let rect = area.get_rect(&cx);
                        return Hit::FingerMove(FingerMoveHitEvent {
                            abs_start: digit.down_abs_start,
                            rel: area.abs_to_rel(cx, fe.abs),
                            rel_start: digit.down_rel_start,
                            rect: rect,
                            is_over: rect_contains_with_margin(&rect, fe.abs, &options.margin),
                            deref_target: fe.clone()
                        })
                    }
                }
            },
            Event::FingerDown(fe) => {
                if !fe.handled {
                    let rect = area.get_rect(&cx);
                    if rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        // scan if any of the fingers already captured this area
                        if cx.fingers.capture_digit(fe.digit_id, area){
                            let rel = area.abs_to_rel(cx, fe.abs);
                            let digit = cx.fingers.get_digit_mut(fe.digit_id).unwrap();
                            digit.down_abs_start = fe.abs;
                            digit.down_rel_start = rel;
                            fe.handled = true;
                            return Hit::FingerDown(FingerDownHitEvent {
                                rel: rel,
                                rect: rect,
                                deref_target: fe.clone()
                            })
                        }
                    }
                }
            },
            Event::FingerUp(fe) => {
                if let Some(digit) = cx.fingers.get_digit(fe.digit_id) {
                    if digit.captured == area {
                        let abs_start = digit.down_abs_start;
                        let rel_start = digit.down_rel_start;
                        cx.fingers.release_digit(fe.digit_id);
                        let rect = area.get_rect(&cx);
                        return Hit::FingerUp(FingerUpHitEvent {
                            is_over: rect.contains(fe.abs),
                            abs_start,
                            rel_start,
                            rel: area.abs_to_rel(cx, fe.abs),
                            rect: rect,
                            deref_target: fe.clone()
                        })
                    }
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
