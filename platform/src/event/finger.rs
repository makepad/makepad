#![allow(unused)]
#![allow(dead_code)]
use {
    std::cell::{Cell},
    crate::{
        makepad_live_tokenizer::{LiveErrorOrigin, live_error_origin},
        makepad_live_compiler::{
            LivePropType,
            LiveType,
            LiveTypeField,
            LiveFieldKind,
            LiveNode,
            LiveId,
            LiveModuleId,
            LiveTypeInfo,
            LiveNodeSliceApi
        },
        live_traits::{LiveNew, LiveHook, LiveHookDeref, LiveApplyValue, LiveApply, ApplyFrom},
        makepad_derive_live::*,
        makepad_math::*,
        makepad_live_id::{FromLiveId, live_id, live_id_num},
        event::{
            event::{Event, Hit}
        },
        window::WindowId,
        cx::Cx,
        area::Area,
    },
};

// Mouse events


#[derive(Copy, Clone, Debug, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool
}

#[derive(Clone, Debug)]
pub struct MouseDownEvent {
    pub abs: DVec2,
    pub button: usize,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub handled: Cell<Area>,
    pub time: f64
}

#[derive(Clone, Debug)]
pub struct MouseMoveEvent {
    pub abs: DVec2,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub handled: Cell<Area>,
}

#[derive(Clone, Debug)]
pub struct MouseUpEvent {
    pub abs: DVec2,
    pub button: usize,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug)]
pub struct ScrollEvent {
    pub window_id: WindowId,
    pub scroll: DVec2,
    pub abs: DVec2,
    pub modifiers: KeyModifiers,
    pub handled_x: Cell<bool>,
    pub handled_y: Cell<bool>,
    pub is_mouse: bool,
    pub time: f64
}


// Touch events

#[derive(Clone, Debug)]
pub enum TouchState {
    Start,
    Stop,
    Move,
    Stable
}

#[derive(Clone, Debug)]
pub struct TouchPoint {
    pub state: TouchState,
    pub abs: DVec2,
    pub uid: u64,
    pub rotation_angle: f64,
    pub force: f64,
    pub radius: DVec2,
    pub handled: Cell<Area>,
    pub sweep_lock: Cell<Area>,
}

#[derive(Clone, Debug)]
pub struct TouchUpdateEvent {
    pub time: f64,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub touches: Vec<TouchPoint>,
}


// Finger API


#[derive(Clone, Copy, Default, Debug, Live)]
#[live_ignore]
pub struct Margin {
    #[live] pub left: f64,
    #[live] pub top: f64,
    #[live] pub right: f64,
    #[live] pub bottom: f64
}


impl LiveHook for Margin {
    fn skip_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        if let Some(v) = nodes[index].value.as_float() {
            *self = Self {left: v, top: v, right: v, bottom: v};
            Some(index + 1)
        }
        else {
            None
        }
    }
}

impl Margin {
    pub fn left_top(&self) -> DVec2 {
        dvec2(self.left, self.top)
    }
    pub fn right_bottom(&self) -> DVec2 {
        dvec2(self.right, self.bottom)
    }
    pub fn size(&self) -> DVec2 {
        dvec2(self.left + self.right, self.top + self.bottom)
    }
    pub fn width(&self) -> f64 {
        self.left + self.right
    }
    pub fn height(&self) -> f64 {
        self.top + self.bottom
    }
    
    pub fn rect_contains_with_margin(rect: &Rect, pos: DVec2, margin: &Option<Margin>) -> bool {
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
}

pub const TAP_COUNT_TIME: f64 = 0.5;
pub const TAP_COUNT_DISTANCE: f64 = 10.0;

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct DigitId(pub LiveId);

#[derive(Default, Clone)]
pub struct CxDigitCapture {
    digit_id: DigitId,
    pub area: Area,
    pub sweep_area: Area,
    pub switch_capture: Option<Area>,
    pub time: f64,
    pub abs_start: DVec2,
}

#[derive(Default, Clone)]
pub struct CxDigitTap {
    digit_id: DigitId,
    last_pos: DVec2,
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
    pub first_mouse_button: Option<usize>,
    captures: Vec<CxDigitCapture>,
    tap: CxDigitTap,
    hovers: Vec<CxDigitHover>,
    sweep_lock: Option<Area>,
}

impl CxFingers {
    
    pub (crate) fn get_captured_area(&self, digit_id: DigitId) -> Area {
        if let Some(cxdigit) = self.captures.iter().find( | v | v.digit_id == digit_id) {
            cxdigit.area
        }
        else {
            Area::Empty
        }
    }
    
    pub (crate) fn get_capture_time(&self, digit_id: DigitId) -> f64 {
        if let Some(cxdigit) = self.captures.iter().find( | v | v.digit_id == digit_id) {
            cxdigit.time
        }
        else {
            0.0
        }
    }
    
    pub (crate) fn get_digit_for_captured_area(&self, area: Area) -> Option<DigitId> {
        if let Some(digit) = self.captures.iter().find( | d | d.area == area) {
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
        for capture in &mut self.captures {
            if capture.area == old_area {
                capture.area = new_area;
            }
            if capture.sweep_area == old_area {
                capture.sweep_area = new_area;
            }
        }
        if self.sweep_lock == Some(old_area) {
            self.sweep_lock = Some(new_area);
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
    
    pub (crate) fn capture_digit(&mut self, digit_id: DigitId, area: Area, sweep_area: Area, time: f64, abs_start: DVec2) {
        if let Some(capture) = self.captures.iter_mut().find( | v | v.digit_id == digit_id) {
            capture.area = area;
            capture.time = time;
            capture.abs_start = abs_start;
        }
        else {
            self.captures.push(CxDigitCapture {
                sweep_area,
                digit_id,
                area,
                time,
                abs_start,
                switch_capture: None
            })
        }
    }
    
    pub (crate) fn get_digit_capture(&mut self, digit_id: DigitId) -> Option<&mut CxDigitCapture> {
        self.captures.iter_mut().find( | v | v.digit_id == digit_id)
    }
    
    pub (crate) fn release_digit(&mut self, digit_id: DigitId) {
        if let Some(index) = self.captures.iter_mut().position( | v | v.digit_id == digit_id) {
            self.captures.remove(index);
        }
    }
    
    pub (crate) fn remove_hover(&mut self, digit_id: DigitId) {
        if let Some(index) = self.hovers.iter_mut().position( | v | v.digit_id == digit_id) {
            self.hovers.remove(index);
        }
    }
    
    pub (crate) fn get_tap_count(&self) -> u32 {
        self.tap.count
    }
    
    pub (crate) fn process_tap_count(&mut self, pos: DVec2, time: f64) -> u32 {
        if (time - self.tap.last_time) < TAP_COUNT_TIME
            && pos.distance(&self.tap.last_pos) < TAP_COUNT_DISTANCE {
            self.tap.count += 1;
        }
        else {
            self.tap.count = 1;
        }
        self.tap.last_pos = pos;
        self.tap.last_time = time;
        return self.tap.count
    }
    
    pub (crate) fn process_touch_update_start(&mut self, time: f64, touches: &[TouchPoint]) {
        for touch in touches {
            if let TouchState::Start = touch.state {
                self.process_tap_count(touch.abs, time);
            }
        }
    }
    
    pub (crate) fn process_touch_update_end(&mut self, touches: &[TouchPoint]) {
        for touch in touches {
            let digit_id = live_id_num!(touch, touch.uid).into();
            match touch.state {
                TouchState::Stop => {
                    self.release_digit(digit_id);
                    self.remove_hover(digit_id);
                }
                TouchState::Start | TouchState::Move | TouchState::Stable => {
                    self.cycle_hover_area(digit_id);
                }
            }
        }
        self.switch_captures();
    }
    
    pub (crate) fn mouse_down(&mut self, button: usize) {
        if self.first_mouse_button.is_none() {
            self.first_mouse_button = Some(button);
        }
    }
    
    pub (crate) fn switch_captures(&mut self) {
        for capture in &mut self.captures {
            if let Some(area) = capture.switch_capture {
                capture.area = area;
                capture.switch_capture = None;
            }
        }
    }
    
    pub (crate) fn mouse_up(&mut self, button: usize) {
        if self.first_mouse_button == Some(button) {
            self.first_mouse_button = None;
            let digit_id = live_id!(mouse).into();
            self.release_digit(digit_id);
        }
    }
    
    pub (crate) fn test_sweep_lock(&mut self, sweep_area: Area) -> bool {
        if let Some(lock) = self.sweep_lock {
            if lock != sweep_area {
                return true
            }
        }
        false
    }
    
    pub fn sweep_lock(&mut self, area: Area) {
        if self.sweep_lock.is_none() {
            self.sweep_lock = Some(area);
        }
    }
    
    pub fn sweep_unlock(&mut self, area: Area) {
        if self.sweep_lock == Some(area) {
            self.sweep_lock = None;
        }
    }
    
}

#[derive(Clone, Debug)]
pub enum DigitDevice {
    Mouse {
        button: usize
    },
    Touch {
        uid: u64
    },
    XR {}
}

impl DigitDevice {
    pub fn is_touch(&self) -> bool {if let DigitDevice::Touch {..} = self {true}else {false}}
    pub fn is_mouse(&self) -> bool {if let DigitDevice::Mouse {..} = self {true}else {false}}
    pub fn is_xr(&self) -> bool {if let DigitDevice::XR {..} = self {true}else {false}}
    
    pub fn has_hovers(&self) -> bool {self.is_mouse() || self.is_xr()}
    
    pub fn mouse_button(&self) -> Option<usize> {if let DigitDevice::Mouse {button} = self {Some(*button)}else {None}}
    pub fn touch_uid(&self) -> Option<u64> {if let DigitDevice::Touch {uid} = self {Some(*uid)}else {None}}
    // pub fn xr_input(&self) -> Option<usize> {if let DigitDevice::XR(input) = self {Some(*input)}else {None}}
}

#[derive(Clone, Debug)]
pub struct FingerDownEvent {
    pub window_id: WindowId,
    pub abs: DVec2,
    
    pub digit_id: DigitId,
    pub device: DigitDevice,
    
    pub tap_count: u32,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub rect: Rect,
}

impl FingerDownEvent {
    pub fn mod_control(&self) -> bool {self.modifiers.control}
    pub fn mod_alt(&self) -> bool {self.modifiers.alt}
    pub fn mod_shift(&self) -> bool {self.modifiers.shift}
    pub fn mod_logo(&self) -> bool {self.modifiers.logo}
}

#[derive(Clone, Debug)]
pub struct FingerMoveEvent {
    pub window_id: WindowId,
    pub abs: DVec2,
    pub digit_id: DigitId,
    pub device: DigitDevice,
    
    pub tap_count: u32,
    pub modifiers: KeyModifiers,
    pub time: f64,
    
    pub abs_start: DVec2,
    pub rect: Rect,
    pub is_over: bool,
}

impl FingerMoveEvent {
    pub fn move_distance(&self) -> f64 {
        ((self.abs_start.x - self.abs.x).powf(2.) + (self.abs_start.y - self.abs.y).powf(2.)).sqrt()
    }
}

#[derive(Clone, Debug)]
pub struct FingerUpEvent {
    pub window_id: WindowId,
    pub abs: DVec2,
    pub capture_time: f64,
    
    pub digit_id: DigitId,
    pub device: DigitDevice,
    
    pub tap_count: u32,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub abs_start: DVec2,
    pub rect: Rect,
    pub is_over: bool,
    pub is_sweep: bool
}

impl FingerUpEvent {
    pub fn mod_shift(&self) -> bool {self.modifiers.shift}

    pub fn was_tap(&self) -> bool {
        self.time - self.capture_time < TAP_COUNT_TIME &&
        (self.abs_start - self.abs).length() < TAP_COUNT_DISTANCE
    }

    pub fn was_long_press(&self) -> bool {
        self.time - self.capture_time >= TAP_COUNT_TIME &&
        (self.abs_start - self.abs).length() < TAP_COUNT_DISTANCE
    }
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
    pub abs: DVec2,
    pub digit_id: DigitId,
    pub device: DigitDevice,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct FingerScrollEvent {
    pub window_id: WindowId,
    pub digit_id: DigitId,
    pub abs: DVec2,
    pub scroll: DVec2,
    pub device: DigitDevice,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub rect: Rect,
}

/*
pub enum HitTouch {
    Single,
    Multi
}*/


// Status


#[derive(Clone, Debug, Default)]
pub struct HitOptions {
    pub margin: Option<Margin>,
    pub sweep_area: Area
}

impl HitOptions {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn with_sweep_area(self, area: Area) -> Self {
        Self {
            sweep_area: area,
            ..self
        }
    }
    pub fn with_margin(self, margin: Margin) -> Self {
        Self {
            margin: Some(margin),
            ..self
        }
    }
}


impl Event {
    
    pub fn hits(&self, cx: &mut Cx, area: Area) -> Hit {
        self.hits_with_options(cx, area, HitOptions::default())
    }
    
    pub fn hits_with_sweep_area(&self, cx: &mut Cx, area: Area, sweep_area: Area) -> Hit {
        self.hits_with_options(cx, area, HitOptions::new().with_sweep_area(sweep_area))
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
            Event::TextCut(tc) => {
                if cx.keyboard.has_key_focus(area) {
                    return Hit::TextCut(tc.clone());
                }
            },
            Event::Scroll(e) => {
                let digit_id = live_id!(mouse).into();
                
                let rect = area.get_clipped_rect(&cx);
                if Margin::rect_contains_with_margin(&rect, e.abs, &options.margin) {
                    //fe.handled = true;
                    let device = DigitDevice::Mouse {
                        button: 0,
                    };
                    return Hit::FingerScroll(FingerScrollEvent {
                        abs: e.abs,
                        rect,
                        window_id: e.window_id,
                        digit_id,
                        device,
                        modifiers: e.modifiers.clone(),
                        time: e.time,
                        scroll: e.scroll
                    })
                }
            },
            Event::TouchUpdate(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing
                }
                for t in &e.touches {
                    let digit_id = live_id_num!(touch, t.uid).into();
                    let device = DigitDevice::Touch {
                        uid: t.uid,
                    };
                    
                    match t.state {
                        TouchState::Start => {
                            
                            if !t.handled.get().is_empty() {
                                continue;
                            }
                            
                            let rect = area.get_clipped_rect(&cx);
                            if !Margin::rect_contains_with_margin(&rect, t.abs, &options.margin) {
                                continue;
                            }
                            
                            cx.fingers.capture_digit(digit_id, area, options.sweep_area, e.time, t.abs);
                            
                            t.handled.set(area);
                            return Hit::FingerDown(FingerDownEvent {
                                window_id: e.window_id,
                                abs: t.abs,
                                digit_id,
                                device,
                                tap_count: cx.fingers.get_tap_count(),
                                modifiers: e.modifiers.clone(),
                                time: e.time,
                                rect,
                            })
                        }
                        TouchState::Stop => {
                            let tap_count = cx.fingers.get_tap_count();
                            let rect = area.get_clipped_rect(&cx);
                            if let Some(capture) = cx.fingers.get_digit_capture(digit_id) {
                                if capture.area == area {
                                    return Hit::FingerUp(FingerUpEvent {
                                        abs_start: capture.abs_start,
                                        rect: rect,
                                        window_id: e.window_id,
                                        abs: t.abs,
                                        digit_id,
                                        device,
                                        tap_count,
                                        capture_time: capture.time,
                                        modifiers: e.modifiers.clone(),
                                        time: e.time,
                                        is_over: rect.contains(t.abs),
                                        is_sweep: false,
                                    })
                                }
                            }
                        }
                        TouchState::Move => {
                            let tap_count = cx.fingers.get_tap_count();
                            //let hover_last = cx.fingers.get_hover_area(digit_id);
                            let rect = area.get_clipped_rect(&cx);
                            
                            if let Some(capture) = cx.fingers.get_digit_capture(digit_id) {
                                //let handled_area = t.handled.get();
                                if !options.sweep_area.is_empty() {
                                    if capture.switch_capture.is_none()
                                        && Margin::rect_contains_with_margin(&rect, t.abs, &options.margin) {
                                        if t.handled.get().is_empty() {
                                            t.handled.set(area);
                                            if capture.area == area {
                                                return Hit::FingerMove(FingerMoveEvent {
                                                    window_id: e.window_id,
                                                    abs: t.abs,
                                                    digit_id,
                                                    device,
                                                    tap_count,
                                                    modifiers: e.modifiers.clone(),
                                                    time: e.time,
                                                    abs_start: capture.abs_start,
                                                    rect,
                                                    is_over: true,
                                                })
                                            }
                                            else if capture.sweep_area == options.sweep_area { // take over the capture
                                                capture.switch_capture = Some(area);
                                                return Hit::FingerDown(FingerDownEvent {
                                                    window_id: e.window_id,
                                                    abs: t.abs,
                                                    digit_id,
                                                    device,
                                                    tap_count: cx.fingers.get_tap_count(),
                                                    modifiers: e.modifiers.clone(),
                                                    time: e.time,
                                                    rect: rect,
                                                })
                                            }
                                        }
                                    }
                                    else if capture.area == area { // we are not over the area
                                        if capture.switch_capture.is_none() {
                                            capture.switch_capture = Some(Area::Empty);
                                        }
                                        return Hit::FingerUp(FingerUpEvent {
                                            abs_start: capture.abs_start,
                                            rect: rect,
                                            window_id: e.window_id,
                                            abs: t.abs,
                                            digit_id,
                                            device,
                                            tap_count,
                                            capture_time: capture.time,
                                            modifiers: e.modifiers.clone(),
                                            time: e.time,
                                            is_sweep: true,
                                            is_over: false,
                                        });
                                    }
                                }
                                else if capture.area == area {
                                    return Hit::FingerMove(FingerMoveEvent {
                                        window_id: e.window_id,
                                        abs: t.abs,
                                        digit_id,
                                        device,
                                        tap_count,
                                        modifiers: e.modifiers.clone(),
                                        time: e.time,
                                        abs_start: capture.abs_start,
                                        rect,
                                        is_over: Margin::rect_contains_with_margin(&rect, t.abs, &options.margin),
                                    })
                                }
                            }
                        }
                        TouchState::Stable => {}
                    }
                }
            }
            Event::MouseMove(e) => { // ok so we dont get hovers
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing
                }
                
                let digit_id = live_id!(mouse).into();
                
                let tap_count = cx.fingers.get_tap_count();
                let hover_last = cx.fingers.get_hover_area(digit_id);
                let rect = area.get_clipped_rect(&cx);
                
                if let Some(button) = cx.fingers.first_mouse_button {
                    let device = DigitDevice::Mouse {
                        button,
                    };
                    
                    if let Some(capture) = cx.fingers.get_digit_capture(digit_id) {
                        //let handled_area = e.handled.get();
                        if !options.sweep_area.is_empty() {
                            if capture.switch_capture.is_none()
                                && Margin::rect_contains_with_margin(&rect, e.abs, &options.margin) {
                                if e.handled.get().is_empty() {
                                    e.handled.set(area);
                                    if capture.area == area {
                                        return Hit::FingerMove(FingerMoveEvent {
                                            window_id: e.window_id,
                                            abs: e.abs,
                                            digit_id,
                                            device,
                                            tap_count,
                                            modifiers: e.modifiers.clone(),
                                            time: e.time,
                                            abs_start: capture.abs_start,
                                            rect,
                                            is_over: true,
                                        })
                                    }
                                    else if capture.sweep_area == options.sweep_area { // take over the capture
                                        capture.switch_capture = Some(area);
                                        cx.fingers.new_hover_area(digit_id, area);
                                        return Hit::FingerDown(FingerDownEvent {
                                            window_id: e.window_id,
                                            abs: e.abs,
                                            digit_id,
                                            device,
                                            tap_count: cx.fingers.get_tap_count(),
                                            modifiers: e.modifiers.clone(),
                                            time: e.time,
                                            rect,
                                        })
                                    }
                                }
                            }
                            else if capture.area == area { // we are not over the area
                                if capture.switch_capture.is_none() {
                                    capture.switch_capture = Some(Area::Empty);
                                }
                                return Hit::FingerUp(FingerUpEvent {
                                    abs_start: capture.abs_start,
                                    rect,
                                    window_id: e.window_id,
                                    abs: e.abs,
                                    digit_id,
                                    device,
                                    tap_count,
                                    capture_time: capture.time,
                                    modifiers: e.modifiers.clone(),
                                    time: e.time,
                                    is_sweep: true,
                                    is_over: false,
                                });
                                
                            }
                        }
                        else if capture.area == area {
                            
                            let event= Hit::FingerMove(FingerMoveEvent {
                                window_id: e.window_id,
                                abs: e.abs,
                                digit_id,
                                device,
                                tap_count,
                                modifiers: e.modifiers.clone(),
                                time: e.time,
                                abs_start: capture.abs_start,
                                rect,
                                is_over: Margin::rect_contains_with_margin(&rect, e.abs, &options.margin),
                            });
                            cx.fingers.new_hover_area(digit_id, area);
                            return event
                        }
                    }
                }
                else {
                    let device = DigitDevice::Mouse {
                        button: 0,
                    };
                    
                    let handled_area = e.handled.get();
                    
                    let fhe = FingerHoverEvent {
                        window_id: e.window_id,
                        abs: e.abs,
                        digit_id,
                        device,
                        modifiers: e.modifiers.clone(),
                        time: e.time,
                        rect,
                    };
                    
                    if hover_last == area {
                        if handled_area.is_empty() && Margin::rect_contains_with_margin(&rect, e.abs, &options.margin) {
                            e.handled.set(area);
                            cx.fingers.new_hover_area(digit_id, area);
                            return Hit::FingerHoverOver(fhe)
                        }
                        else {
                            return Hit::FingerHoverOut(fhe)
                        }
                    }
                    else {
                        if handled_area.is_empty() && Margin::rect_contains_with_margin(&rect, e.abs, &options.margin) {
                            //let any_captured = cx.fingers.get_digit_for_captured_area(area);
                            cx.fingers.new_hover_area(digit_id, area);
                            e.handled.set(area);
                            return Hit::FingerHoverIn(fhe)
                        }
                    }
                }
            },
            Event::MouseDown(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing
                }
                
                let digit_id = live_id!(mouse).into();
                
                if !e.handled.get().is_empty() {
                    return Hit::Nothing
                }
                
                if cx.fingers.first_mouse_button != Some(e.button) {
                    return Hit::Nothing
                }
                
                let rect = area.get_clipped_rect(&cx);
                if !Margin::rect_contains_with_margin(&rect, e.abs, &options.margin) {
                    return Hit::Nothing
                }
                
                let device = DigitDevice::Mouse {
                    button: e.button,
                };
                
                if cx.fingers.get_digit_for_captured_area(area).is_some() {
                    return Hit::Nothing;
                }
                
                cx.fingers.capture_digit(digit_id, area, options.sweep_area, e.time, e.abs);
                e.handled.set(area);
                cx.fingers.new_hover_area(digit_id, area);
                return Hit::FingerDown(FingerDownEvent {
                    window_id: e.window_id,
                    abs: e.abs,
                    digit_id,
                    device,
                    tap_count: cx.fingers.get_tap_count(),
                    modifiers: e.modifiers.clone(),
                    time: e.time,
                    rect: rect,
                })
            },
            Event::MouseUp(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing
                }
                
                if cx.fingers.first_mouse_button != Some(e.button) {
                    return Hit::Nothing
                }
                
                let digit_id = live_id!(mouse).into();
                
                let device = DigitDevice::Mouse {
                    button: e.button,
                };
                let tap_count = cx.fingers.get_tap_count();
                let rect = area.get_clipped_rect(&cx);
                
                if let Some(capture) = cx.fingers.get_digit_capture(digit_id) {
                    if capture.area == area {
                        let is_over = rect.contains(e.abs);
                        let event =  Hit::FingerUp(FingerUpEvent {
                            abs_start: capture.abs_start,
                            rect: rect,
                            window_id: e.window_id,
                            abs: e.abs,
                            digit_id,
                            device,
                            tap_count,
                            capture_time: capture.time,
                            modifiers: e.modifiers.clone(),
                            time: e.time,
                            is_over,
                            is_sweep: false,
                        });
                        if is_over {
                            cx.fingers.new_hover_area(digit_id, area);
                        }
                        return event
                    }
                }
            },
            _ => ()
        };
        Hit::Nothing
    }
}
