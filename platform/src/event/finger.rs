use {
    std::cell::{Cell},
    std::rc::Rc,
    std::ops::Deref,
    crate::{
        makepad_live_tokenizer::{LiveErrorOrigin, live_error_origin},
        makepad_live_compiler::{
            LiveValue,
            LivePropType,
            LiveType,
            LiveTypeField,
            LiveFieldKind,
            LiveNode,
            LiveId,
            LiveModuleId,
            LiveTypeInfo,
            LiveNodeSlice
        },
        live_traits::{LiveNew, LiveHook, LiveApplyValue, LiveApply, ApplyFrom},
        makepad_derive_live::*,
        makepad_error_log::*,
        makepad_math::*,
        makepad_live_id::{FromLiveId},
        event::{
            event::{Event, Hit, DragHit}
        },
        window::WindowId,
        cx::Cx,
        area::Area,
    },
};


#[derive(Clone, Copy, Default, Debug, Live)]
#[live_ignore]
pub struct Margin {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32
}


impl LiveHook for Margin {
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        match &nodes[index].value {
            LiveValue::Float(v) => {
                *self = Self {left: *v as f32, top: *v as f32, right: *v as f32, bottom: *v as f32};
                Some(index + 1)
            }
            LiveValue::Int(v) => {
                *self = Self {left: *v as f32, top: *v as f32, right: *v as f32, bottom: *v as f32};
                Some(index + 1)
            }
            _ => None
        }
    }
}

impl Margin {
    pub fn left_top(&self) -> Vec2 {
        vec2(self.left, self.top)
    }
    pub fn right_bottom(&self) -> Vec2 {
        vec2(self.right, self.bottom)
    }
    pub fn size(&self) -> Vec2 {
        vec2(self.left + self.right, self.top + self.bottom)
    }
    pub fn width(&self) -> f32 {
        self.left + self.right
    }
    pub fn height(&self) -> f32 {
        self.top + self.bottom
    }
}

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
            if self.capture_count > 0 {
                self.capture_count -= 1;
            }
            self.digits.remove(index);
            return
        }
    }
    
    pub (crate) fn get_digit_index(&self, digit_id: DigitId) -> usize {
        if let Some(index) = self.digits.iter().position( | v | v.digit_id == digit_id) {
            return index
        }
        0
    }
    
    pub (crate) fn get_digit_count(&self) -> usize {
        self.digits.len()
    }
    
    
    pub (crate) fn get_digit(&self, digit_id: DigitId) -> Option<&CxDigit> {
        self.digits.iter().find( | v | v.digit_id == digit_id)
    }
    
    pub (crate) fn get_digit_mut(&mut self, digit_id: DigitId) -> Option<&mut CxDigit> {
        self.digits.iter_mut().find( | v | v.digit_id == digit_id)
    }
    
    pub (crate) fn is_digit_allocated(&self, digit_id: DigitId) -> bool {
        self.digits.iter().position( | v | v.digit_id == digit_id).is_some()
    }
    
    pub (crate) fn get_captured_area(&self, digit_id: DigitId) -> Area {
        if let Some(cxdigit) = self.digits.iter().find( | v | v.digit_id == digit_id) {
            cxdigit.captured
        }
        else {
            Area::Empty
        }
    }
    
    pub (crate) fn get_digit_for_captured_area(&self, area: Area) -> Option<DigitId> {
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
    
    pub (crate) fn get_hover_area(&self, digit: DigitId) -> Area {
        for hover in &self.hovers {
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
    
    pub (crate) fn capture_digit(&mut self, digit_id: DigitId, area: Area) -> bool {
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
    pub (crate) fn get_tap_count(&self, digit_id: DigitId) -> u32 {
        if let Some(tap) = self.taps.iter().find( | v | v.digit_id == digit_id) {
            tap.count
        }
        else {
            0
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

#[derive(Clone, Debug, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool
}

#[derive(Clone, Debug)]
pub enum DigitDevice {
    Mouse(usize),
    Touch(u64),
    XR(usize)
}

impl DigitDevice {
    pub fn is_touch(&self) -> bool {if let DigitDevice::Touch(_) = self {true}else {false}}
    pub fn is_mouse(&self) -> bool {if let DigitDevice::Mouse(_) = self {true}else {false}}
    pub fn is_xr(&self) -> bool {if let DigitDevice::XR(_) = self {true}else {false}}
    pub fn has_hovers(&self) -> bool {self.is_mouse() || self.is_xr()}
    pub fn mouse_button(&self) -> Option<usize> {if let DigitDevice::Mouse(button) = self {Some(*button)}else {None}}
    pub fn touch_uid(&self) -> Option<u64> {if let DigitDevice::Touch(uid) = self {Some(*uid)}else {None}}
    pub fn xr_input(&self) -> Option<usize> {if let DigitDevice::XR(input) = self {Some(*input)}else {None}}
}

#[derive(Clone, Debug)]
pub struct DigitInfo {
    pub id: DigitId,
    pub index: usize,
    pub count: usize,
    pub device: DigitDevice,
}

impl Deref for DigitInfo {
    type Target = DigitDevice;
    fn deref(&self) -> &Self::Target {
        &self.device
    }
}

#[derive(Clone, Debug)]
pub struct FingerDownEvent {
    pub window_id: WindowId,
    pub abs: Vec2,
    pub digit: DigitInfo,
    pub tap_count: u32,
    pub handled: Cell<bool>,
    pub modifiers: KeyModifiers,
    pub time: f64
}

impl FingerDownEvent {
    pub fn mod_control(&self) -> bool {self.modifiers.control}
    pub fn mod_alt(&self) -> bool {self.modifiers.alt}
    pub fn mod_shift(&self) -> bool {self.modifiers.shift}
    pub fn mod_logo(&self) -> bool {self.modifiers.logo}
}

#[derive(Clone, Debug)]
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


#[derive(Clone, Debug)]
pub struct FingerMoveEvent {
    pub window_id: WindowId,
    pub abs: Vec2,
    pub handled: Cell<bool>,
    pub hover_last: Area,
    //pub captured: Area,
    pub digit: DigitInfo,
    pub tap_count: u32,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug)]
pub struct FingerMoveHitEvent {
    pub abs_start: Vec2,
    pub rect: Rect,
    pub is_over: bool,
    pub deref_target: FingerMoveEvent,
}

#[derive(Clone, Debug)]
pub struct FingerSweepEvent {
    pub abs: Vec2,
    pub abs_start: Vec2,
    pub rect: Rect,

    pub window_id: WindowId,

    pub digit: DigitInfo,
    pub tap_count: u32,
    pub modifiers: KeyModifiers,
    pub time: f64,

    pub is_up: bool,
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


#[derive(Clone, Debug)]
pub struct FingerUpEvent {
    pub window_id: WindowId,
    pub abs: Vec2,
    pub captured: Area,
    pub digit: DigitInfo,
    pub tap_count: u32,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug)]
pub struct FingerUpHitEvent {
    pub abs_start: Vec2,
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

#[derive(Clone, Debug)]
pub struct FingerHoverEvent {
    pub window_id: WindowId,
    pub abs: Vec2,
    pub digit_id: DigitId,
    pub hover_last: Area,
    pub handled: Cell<bool>,
    pub device: DigitDevice,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug)]
pub struct FingerHoverHitEvent {
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

#[derive(Clone, Debug)]
pub struct FingerScrollEvent {
    pub window_id: WindowId,
    pub digit_id: DigitId,
    pub abs: Vec2,
    pub scroll: Vec2,
    pub device: DigitDevice,
    pub handled_x: Cell<bool>,
    pub handled_y: Cell<bool>,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug)]
pub struct FingerScrollHitEvent {
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


#[derive(Clone, Debug)]
pub struct DragEvent {
    pub handled: Cell<bool>,
    pub abs: Vec2,
    pub state: DragState,
    pub action: Rc<Cell<DragAction >>,
}

#[derive(Clone, Debug)]
pub struct DropEvent {
    pub handled: Cell<bool>,
    pub abs: Vec2,
    pub dragged_item: DraggedItem,
}

#[derive(Debug, PartialEq)]
pub struct DragHitEvent<'a> {
    pub abs: Vec2,
    pub rect: Rect,
    pub state: DragState,
    pub action: &'a Cell<DragAction>,
}

#[derive(Debug, PartialEq)]
pub struct DropHitEvent<'a> {
    pub abs: Vec2,
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


#[derive(Clone, Debug, Default)]
pub struct HitOptions {
    pub use_multi_touch: bool,
    pub margin: Option<Margin>,
    pub sweep_area: Area
}

impl HitOptions {
    pub fn with_sweep_area(sweep_area:Area)->Self{
        Self{
            use_multi_touch: false,
            sweep_area,
            margin: None            
        }
    }
    pub fn margin(margin: Margin) -> Self {
        Self {
            use_multi_touch: false,
            sweep_area: Area::Empty,
            margin: Some(margin)
        }
    }
    pub fn use_multi_touch() -> Self {
        Self {
            use_multi_touch: true,
            sweep_area: Area::Empty,
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
    
    pub fn hits(&self, cx: &mut Cx, area: Area) -> Hit {
        self.hits_with_options(cx, area, HitOptions::default())
    }
    
    pub fn hits_with_options(&self, cx: &mut Cx, area: Area, options: HitOptions) -> Hit {
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
                    return Hit::TextCopy(tc.clone());
                }
            },
            Event::FingerScroll(fe) => {
                let rect = area.get_rect(&cx);
                if rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                    //fe.handled = true;
                    return Hit::FingerScroll(FingerScrollHitEvent {
                        rect: rect,
                        event: fe.clone()
                    })
                }
            },
            Event::FingerHover(fe) => {
                let rect = area.get_rect(&cx);
                if fe.hover_last == area {
                    let any_captured = cx.fingers.get_digit_for_captured_area(area);
                    if !fe.handled.get() && rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        fe.handled.set(true);
                        cx.fingers.new_hover_area(fe.digit_id, area);
                        return Hit::FingerHoverOver(FingerHoverHitEvent {
                            rect: rect,
                            any_captured,
                            event: fe.clone()
                        })
                    }
                    else {
                        return Hit::FingerHoverOut(FingerHoverHitEvent {
                            rect: rect,
                            any_captured,
                            event: fe.clone()
                        })
                    }
                }
                else {
                    if !fe.handled.get() && rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        let any_captured = cx.fingers.get_digit_for_captured_area(area);
                        cx.fingers.new_hover_area(fe.digit_id, area);
                        fe.handled.set(true);
                        return Hit::FingerHoverIn(FingerHoverHitEvent {
                            rect: rect,
                            any_captured,
                            event: fe.clone()
                        })
                    }
                }
            },
            Event::FingerMove(fe) => {// ok so we dont get hovers
                // check wether our digit is captured, otherwise don't send
                if let Some(digit) = cx.fingers.get_digit(fe.digit.id) {
                    if digit.captured == options.sweep_area{
                        let abs_start = digit.down_abs_start;
                        let rect = area.get_rect(&cx);
                        if fe.hover_last == area {
                            if !fe.handled.get() && rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                                fe.handled.set(true);
                                cx.fingers.new_hover_area(fe.digit.id, area);
                                return Hit::FingerSweep(FingerSweepEvent {
                                    abs_start,
                                    rect: rect,
                                    window_id: fe.window_id,
                                    abs: fe.abs,
                                    digit: fe.digit.clone(),
                                    tap_count: fe.tap_count,
                                    modifiers: fe.modifiers.clone(),
                                    time: fe.time,
                                    is_up: false
                                })
                            }
                            else {
                                return Hit::FingerSweepOut(FingerSweepEvent {
                                    abs_start,
                                    rect: rect,
                                    window_id: fe.window_id,
                                    abs: fe.abs,
                                    digit: fe.digit.clone(),
                                    tap_count: fe.tap_count,
                                    modifiers: fe.modifiers.clone(),
                                    time: fe.time,
                                    is_up: false
                                })
                            }
                        }
                        else {
                            if !fe.handled.get() && rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                                cx.fingers.new_hover_area(fe.digit.id, area);
                                fe.handled.set(true);
                                return Hit::FingerSweepIn(FingerSweepEvent {
                                    abs_start,
                                    rect: rect,
                                    window_id: fe.window_id,
                                    abs: fe.abs,
                                    digit: fe.digit.clone(),
                                    tap_count: fe.tap_count,
                                    modifiers: fe.modifiers.clone(),
                                    time: fe.time,
                                    is_up: false
                                })
                            }
                        }
                    }
                    else if digit.captured == area {
                        let rect = area.get_rect(&cx);
                        return Hit::FingerMove(FingerMoveHitEvent {
                            abs_start: digit.down_abs_start,
                            rect: rect,
                            is_over: rect_contains_with_margin(&rect, fe.abs, &options.margin),
                            deref_target: fe.clone()
                        })
                    }
                }
            },
            Event::FingerDown(fe) => {
                if !fe.handled.get() {
                    let rect = area.get_rect(&cx);
                    if rect_contains_with_margin(&rect, fe.abs, &options.margin) {
                        // if we have a parent area, capture that one
                        if !options.sweep_area.is_empty(){
                            if cx.fingers.capture_digit(fe.digit.id, options.sweep_area) {
                                
                                cx.fingers.new_hover_area(fe.digit.id, area);
                                let digit = cx.fingers.get_digit_mut(fe.digit.id).unwrap();
                                digit.down_abs_start = fe.abs;
                                digit.down_rel_start = vec2(0.0,0.0);
                                fe.handled.set(true);
                                
                                return Hit::FingerSweepIn(FingerSweepEvent {
                                    abs_start: fe.abs,
                                    rect: rect,
                                    window_id: fe.window_id,
                                    abs: fe.abs,
                                    digit: fe.digit.clone(),
                                    tap_count: fe.tap_count,
                                    modifiers: fe.modifiers.clone(),
                                    time: fe.time,
                                    is_up: false
                                })
                            }
                        }
                        else{
                            if cx.fingers.capture_digit(fe.digit.id, area) {
                                let rel = area.abs_to_rel(cx, fe.abs);
                                let digit = cx.fingers.get_digit_mut(fe.digit.id).unwrap();
                                digit.down_abs_start = fe.abs;
                                digit.down_rel_start = rel;
                                fe.handled.set(true);
                                return Hit::FingerDown(FingerDownHitEvent {
                                    rel: rel,
                                    rect: rect,
                                    deref_target: fe.clone()
                                })
                            }
                        };
                    }
                }
            },
            Event::FingerUp(fe) => {
                if let Some(digit) = cx.fingers.get_digit(fe.digit.id) {
                    if digit.captured == options.sweep_area{
                        let abs_start = digit.down_abs_start;
                        let rect = area.get_rect(&cx);
                        if rect.contains(fe.abs){
                            return Hit::FingerSweepOut(FingerSweepEvent {
                                abs_start,
                                rect: rect,
                                window_id: fe.window_id,
                                abs: fe.abs,
                                digit: fe.digit.clone(),
                                tap_count: fe.tap_count,
                                modifiers: fe.modifiers.clone(),
                                time: fe.time,
                                is_up: true
                            })
                        }
                    }
                    else if digit.captured == area {
                        let abs_start = digit.down_abs_start;
                        cx.fingers.release_digit(fe.digit.id);
                        let rect = area.get_rect(&cx);
                        return Hit::FingerUp(FingerUpHitEvent {
                            is_over: rect.contains(fe.abs),
                            abs_start,
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
    
    pub fn drag_hits(&self, cx: &mut Cx, area: Area) -> DragHit {
        self.drag_hits_with_options(cx, area, HitOptions::default())
    }
    
    pub fn drag_hits_with_options(&self, cx: &mut Cx, area: Area, options: HitOptions) -> DragHit {
        match self {
            Event::Drag(event) => {
                let rect = area.get_rect(cx);
                if area == cx.finger_drag.drag_area {
                    if !event.handled.get() && rect_contains_with_margin(&rect, event.abs, &options.margin) {
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
                    if !event.handled.get() && rect_contains_with_margin(&rect, event.abs, &options.margin) {
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
                let rect = area.get_rect(cx);
                if !event.handled.get() && rect_contains_with_margin(&rect, event.abs, &options.margin) {
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
