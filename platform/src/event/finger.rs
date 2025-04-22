#![allow(unused)]
#![allow(dead_code)]
use {
    std::{cell::Cell, ops::Deref},
    crate::{
        makepad_micro_serde::*,
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
        live_traits::{LiveNew, LiveHook, LiveRegister, LiveHookDeref, LiveApplyValue, LiveApply,LiveApplyReset, Apply},
        makepad_derive_live::*,
        makepad_math::*,
        makepad_live_id::{FromLiveId, live_id, live_id_num},
        event::{
            event::{Event, Hit}
        },
        window::WindowId,
        cx::Cx,
        event::xr::XrHand,
        area::Area,
    },
};

// Mouse events


#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, Eq, PartialEq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool
}

impl KeyModifiers{
    /// Returns true if the primary key modifier is active (pressed).
    ///
    /// The primary modifier is Logo key (Command ⌘) on macOS
    /// and the Control key on all other platforms.
    pub fn is_primary(&self) -> bool {
        #[cfg(target_vendor = "apple")] {
            self.logo
        }
        #[cfg(not(target_vendor = "apple"))] {
            self.control
        }
    }

    fn any(&self)->bool{
        self.shift || self.control || self.alt || self.logo
    }
}


bitflags::bitflags! {
    /// A `u32` bit mask of all mouse buttons that were pressed
    /// during a given mouse event.
    ///
    /// This is a bit mask because it is possible for multiple buttons
    /// to be pressed simultaneously during a given input event.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
    #[doc(alias = "click")]
    pub struct MouseButton: u32 {
        /// The primary mouse button, typically the left-click button.
        #[doc(alias("left", "left-click"))]
        const PRIMARY =   1 << 0;
        /// The secondary mouse button, typically the right-click button.
        #[doc(alias("right", "right-click"))]
        const SECONDARY = 1 << 1;
        /// The middle mouse button, typically the scroll-wheel click button.
        #[doc(alias("scroll", "wheel"))]
        const MIDDLE =    1 << 2;
        /// The fourth mouse button, typically used for back navigation.
        const BACK =      1 << 3;
        /// The fifth mouse button, typically used for forward navigation.
        const FORWARD =   1 << 4;

        // Ensure that all bits are valid, such that no bits get truncated.
        const _ = !0;
    }
}
impl MouseButton {
    /// Returns true if the primary mouse button is pressed.
    pub fn is_primary(&self) -> bool {
        self.contains(MouseButton::PRIMARY)
    }
    /// Returns true if the secondary mouse button is pressed.
    pub fn is_secondary(&self) -> bool {
        self.contains(MouseButton::SECONDARY)
    }
    /// Returns true if the middle mouse button is pressed.
    pub fn is_middle(&self) -> bool {
        self.contains(MouseButton::MIDDLE)
    }
    /// Returns true if the back mouse button is pressed.
    pub fn is_back(&self) -> bool {
        self.contains(MouseButton::BACK)
    }
    /// Returns true if the forward mouse button is pressed.
    pub fn is_forward(&self) -> bool {
        self.contains(MouseButton::FORWARD)
    }
    /// Returns true if the `n`th button is pressed.
    ///
    /// The button values are:
    /// * n = 0: PRIMARY
    /// * n = 1: SECONDARY
    /// * n = 2: MIDDLE
    /// * n = 3: BACK
    /// * n = 4: FORWARD
    /// * n > 4: other/custom
    pub fn is_other_button(&self, n: u8) -> bool {
        self.bits() & (1 << n) != 0
    }
    /// Returns a `MouseButton` bit mask based on the raw button value: `1 << raw`.
    ///
    /// A raw button value is a number that represents a mouse button, like so:
    /// * 0: MouseButton::PRIMARY
    /// * 1: MouseButton::SECONDARY
    /// * 2: MouseButton::MIDDLE
    /// * 3: MouseButton::BACK
    /// * 4: MouseButton::FORWARD
    /// * etc.
    pub fn from_raw_button(raw: usize) -> MouseButton {
        MouseButton::from_bits_retain(1 << raw)
    }
}


#[derive(Clone, Debug)]
pub struct MouseDownEvent {
    pub abs: DVec2,
    pub button: MouseButton,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub handled: Cell<Area>,
    pub time: f64,
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
    pub button: MouseButton,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug)]
pub struct MouseLeaveEvent {
    pub abs: DVec2,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub handled: Cell<Area>,
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

#[derive(Clone, Debug)]
pub struct LongPressEvent {
    pub window_id: WindowId,
    pub abs: DVec2,
    pub uid: u64,
    pub time: f64,
}

// Touch events

#[derive(Clone, Copy, Debug)]
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
    pub time: f64,
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
impl LiveRegister for Margin{}

impl LiveHook for Margin {
    fn skip_apply(&mut self, _cx: &mut Cx, _apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> Option<usize> {
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
    
    pub fn rect_contains_with_margin(pos: DVec2, rect: &Rect, margin: &Option<Margin>) -> bool {
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

// TODO: query the platform for its long-press timeout.
//       See Android's ViewConfiguration.getLongPressTimeout().
pub const TAP_COUNT_TIME: f64 = 0.5;
pub const TAP_COUNT_DISTANCE: f64 = 5.0;

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct DigitId(pub LiveId);

#[derive(Default, Clone)]
pub struct CxDigitCapture {
    digit_id: DigitId,
    has_long_press_occurred: bool,
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
    pub first_mouse_button: Option<(MouseButton, WindowId)>,
    captures: Vec<CxDigitCapture>,
    tap: CxDigitTap,
    hovers: Vec<CxDigitHover>,
    sweep_lock: Option<Area>,
}

impl CxFingers {
    /*
    pub (crate) fn get_captured_area(&self, digit_id: DigitId) -> Area {
        if let Some(cxdigit) = self.captures.iter().find( | v | v.digit_id == digit_id) {
            cxdigit.area
        }
        else {
            Area::Empty
        }
    }*/
    /*
    pub (crate) fn get_capture_time(&self, digit_id: DigitId) -> f64 {
        if let Some(cxdigit) = self.captures.iter().find( | v | v.digit_id == digit_id) {
            cxdigit.time
        }
        else {
            0.0
        }
    }*/
    
    pub (crate) fn find_digit_for_captured_area(&self, area: Area) -> Option<DigitId> {
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
    
    pub (crate) fn find_hover_area(&self, digit: DigitId) -> Area {
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
        /*if let Some(capture) = self.captures.iter_mut().find( | v | v.digit_id == digit_id) {
            capture.sweep_area = sweep_area;
            capture.area = area;
            capture.time = time;
            capture.abs_start = abs_start;
        }
        else {*/
        self.captures.push(CxDigitCapture {
            sweep_area,
            digit_id,
            area,
            time,
            abs_start,
            has_long_press_occurred: false,
            switch_capture: None
        })
        /*}*/
    }
    
    pub (crate) fn uncapture_area(&mut self, area: Area){
        self.captures.retain(|v| v.area != area);
    }
    
    pub (crate) fn find_digit_capture(&mut self, digit_id: DigitId) -> Option<&mut CxDigitCapture> {
        self.captures.iter_mut().find( | v | v.digit_id == digit_id)
    }
    
    
    pub (crate) fn find_area_capture(&mut self, area: Area) -> Option<&mut CxDigitCapture> {
        self.captures.iter_mut().find( | v | v.area == area)
    }
    
    pub fn is_area_captured(&self, area: Area) -> bool {
        self.captures.iter().find( | v | v.area == area).is_some()
    }
    
    pub fn any_areas_captured(&self) -> bool {
        self.captures.len() > 0
    }
    
    pub (crate) fn release_digit(&mut self, digit_id: DigitId) {
        while let Some(index) = self.captures.iter_mut().position( | v | v.digit_id == digit_id) {
            self.captures.remove(index);
        }
    }
    
    pub (crate) fn remove_hover(&mut self, digit_id: DigitId) {
        while let Some(index) = self.hovers.iter_mut().position( | v | v.digit_id == digit_id) {
            self.hovers.remove(index);
        }
    }
    
    pub (crate) fn tap_count(&self) -> u32 {
        self.tap.count
    }
    
    pub (crate) fn process_tap_count(&mut self, pos: DVec2, time: f64) -> u32 {
        // TODO: query the platform for its multi-press / double-click timeout.
        //       e.g., see Android's ViewConfiguration.getMultiPressTimeout().
        if (time - self.tap.last_time) < TAP_COUNT_TIME
            && pos.distance(&self.tap.last_pos) < TAP_COUNT_DISTANCE
        {
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
    
    pub (crate) fn mouse_down(&mut self, button: MouseButton, window_id: WindowId) {
        if self.first_mouse_button.is_none() {
            self.first_mouse_button = Some((button, window_id));
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
    
    pub (crate) fn mouse_up(&mut self, button: MouseButton) {
        match self.first_mouse_button {
            Some((fmb, _)) if fmb == button => {
                self.first_mouse_button = None;
                let digit_id = live_id!(mouse).into();
                self.release_digit(digit_id);
            }
            _ => { }
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
        button: MouseButton,
    },
    Touch {
        uid: u64
    },
    XrHand{
        is_left: bool,
        index: usize
    },
    XrController{
    }
}

impl DigitDevice {
    /// Returns true if this device is a touch device.
    pub fn is_touch(&self) -> bool { matches!(self, Self::Touch {..}) }
    /// Returns true if this device is a mouse.
    pub fn is_mouse(&self) -> bool { matches!(self, Self::Mouse {..}) }
    /// Returns true if this device is an XR device.
    pub fn is_xr_hand(&self) -> bool { matches!(self, Self::XrHand {..}) }
    pub fn is_xr_controller(&self) -> bool { matches!(self, Self::XrController {..}) }
    /// Returns true if this device can hover: either a mouse or an XR device.
    pub fn has_hovers(&self) -> bool { matches!(self, Self::Mouse {..} | Self::XrController {..}| Self::XrHand {..}) }
    /// Returns the `MouseButton` if this device is a mouse; otherwise `None`.
    pub fn mouse_button(&self) -> Option<MouseButton> {
        if let Self::Mouse {button} = self {
            Some(*button)
        } else {
            None
        }
    }
    /// Returns the `uid` of the touch device if this device is a touch device; otherwise `None`.
    pub fn touch_uid(&self) -> Option<u64> {
        if let Self::Touch {uid} = self {
            Some(*uid)
        } else {
            None
        }
    }
    /// Returns true if this is a *primary* mouse button hit *or* any touch hit.
    pub fn is_primary_hit(&self) -> bool {
        match self {
            DigitDevice::Mouse { button } => button.is_primary(),
            DigitDevice::Touch {..} => true,
            DigitDevice::XrHand {..} => true,
            DigitDevice::XrController {..} => true,
        }
    }
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
impl Deref for FingerDownEvent {
    type Target = DigitDevice;
    fn deref(&self) -> &DigitDevice {
        &self.device
    }
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
impl Deref for FingerMoveEvent {
    type Target = DigitDevice;
    fn deref(&self) -> &DigitDevice {
        &self.device
    }
}
impl FingerMoveEvent {
    pub fn move_distance(&self) -> f64 {
        ((self.abs_start.x - self.abs.x).powf(2.) + (self.abs_start.y - self.abs.y).powf(2.)).sqrt()
    }
}

#[derive(Clone, Debug)]
pub struct FingerUpEvent {
    pub window_id: WindowId,
    /// The absolute position of this finger-up event.
    pub abs: DVec2,
    /// The absolute position of the original finger-down event.
    pub abs_start: DVec2,
    /// The time at which the original finger-down event occurred.
    pub capture_time: f64,
    /// The time at which this finger-up event occurred.
    pub time: f64,

    pub digit_id: DigitId,
    pub device: DigitDevice,
    /// Whether a platform-native long press has occurred between
    /// the original finger-down event and this finger-up event.
    pub has_long_press_occurred: bool,

    pub tap_count: u32,
    pub modifiers: KeyModifiers,
    pub rect: Rect,
    /// Whether this finger-up event (`abs`) occurred within the hits area.
    pub is_over: bool,
    pub is_sweep: bool,
}
impl Deref for FingerUpEvent {
    type Target = DigitDevice;
    fn deref(&self) -> &DigitDevice {
        &self.device
    }
}
impl FingerUpEvent {
    /// Returns `true` if this FingerUp event was a regular tap/click (not a long press).
    pub fn was_tap(&self) -> bool {
        if self.has_long_press_occurred {
            return false;
        }
        self.time - self.capture_time < TAP_COUNT_TIME
        && (self.abs_start - self.abs).length() < TAP_COUNT_DISTANCE
    }
}

#[derive(Clone, Debug)]
pub struct FingerLongPressEvent {
    pub window_id: WindowId,
    /// The absolute position of this long-press event.
    pub abs: DVec2,
    /// The time at which the original finger-down event occurred.
    pub capture_time: f64,
    /// The time at which this long-press event occurred.
    pub time: f64,
    
    pub digit_id: DigitId,
    pub device: DigitDevice,
    pub rect: Rect,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum HoverState {
    In,
    #[default]
    Over,
    Out
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
    pub sweep_area: Area,
    pub capture_overload: bool,
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
    pub fn with_capture_overload(self, capture_overload:bool) -> Self {
        Self {
            capture_overload,
            ..self
        }
    }
}

impl Event{
    pub fn unhandle(&self, cx:&mut Cx, area:&Area){
        match self{
            Event::TouchUpdate(e)=>{
                for t in &e.touches {
                    if let TouchState::Start = t.state{
                        if t.handled.get() == *area{
                            t.handled.set(Area::Empty);
                        }
                        cx.fingers.uncapture_area(*area);
                    }
                }
            }
            Event::MouseDown(fd)=>{
                if fd.handled.get() == *area{
                    fd.handled.set(Area::Empty);
                }
                cx.fingers.uncapture_area(*area);
            }
            _=>()
        }
    }
}

impl Event {
    
    pub fn hits(&self, cx: &mut Cx, area: Area) -> Hit {
        self.hits_with_options(cx, area, HitOptions::default())
    }

    pub fn hits_with_test<F>(&self, cx: &mut Cx, area: Area, hit_test:F) -> Hit 
    where F: Fn(DVec2, &Rect, &Option<Margin>)->bool{
        self.hits_with_options_and_test(cx, area,  HitOptions::new(), hit_test)
    }

    pub fn hits_with_sweep_area(&self, cx: &mut Cx, area: Area, sweep_area: Area) -> Hit {
        self.hits_with_options(cx, area, HitOptions::new().with_sweep_area(sweep_area))
    }
    
    pub fn hits_with_capture_overload(&self, cx: &mut Cx, area: Area, capture_overload: bool) -> Hit {
        self.hits_with_options(cx, area, HitOptions::new().with_capture_overload(capture_overload))
    }
    
    pub fn hits_with_options(&self, cx: &mut Cx, area: Area, options: HitOptions) -> Hit {
        self.hits_with_options_and_test(cx, area, options, |abs, rect, margin|{
            Margin::rect_contains_with_margin(abs, rect, margin)
        })
    }
    
    pub fn hits_with_options_and_test<F>(&self, cx: &mut Cx, area: Area, options: HitOptions, hit_test:F) -> Hit 
    where F: Fn(DVec2, &Rect, &Option<Margin>)->bool
    {
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
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    // log!("Skipping Scroll sweep_area: {:?}", options.sweep_area);
                    return Hit::Nothing
                }
                let digit_id = live_id!(mouse).into();
                
                let rect = area.clipped_rect(&cx);
                if hit_test(e.abs, &rect, &options.margin) {
                    let device = DigitDevice::Mouse {
                        button: MouseButton::PRIMARY,
                    };
                    return Hit::FingerScroll(FingerScrollEvent {
                        abs: e.abs,
                        rect,
                        window_id: e.window_id,
                        digit_id,
                        device,
                        modifiers: e.modifiers,
                        time: e.time,
                        scroll: e.scroll
                    })
                }
            },
            Event::TouchUpdate(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    // log!("Skipping TouchUpdate, sweep_area: {:?}", options.sweep_area);
                    return Hit::Nothing
                }
                for t in &e.touches {
                    let digit_id = live_id_num!(touch, t.uid).into();
                    let device = DigitDevice::Touch { uid: t.uid };

                    match t.state {
                        TouchState::Start => {
                            // someone did a second call on our area
                            if cx.fingers.find_digit_for_captured_area(area).is_some() {
                                let rect = area.clipped_rect(&cx);
                                return Hit::FingerDown(FingerDownEvent {
                                    window_id: e.window_id,
                                    abs: t.abs,
                                    digit_id,
                                    device,
                                    tap_count: cx.fingers.tap_count(),
                                    modifiers: e.modifiers,
                                    time: e.time,
                                    rect,
                                });
                            }
                            
                            if !options.capture_overload && !t.handled.get().is_empty() {
                                continue;
                            }
                            
                            if cx.fingers.find_area_capture(area).is_some(){
                                continue;
                            }
                            
                            let rect = area.clipped_rect(&cx);
                            if !hit_test(t.abs, &rect, &options.margin) {
                                continue;
                            }
                            
                            cx.fingers.capture_digit(digit_id, area, options.sweep_area, e.time, t.abs);
                            
                            t.handled.set(area);
                            return Hit::FingerDown(FingerDownEvent {
                                window_id: e.window_id,
                                abs: t.abs,
                                digit_id,
                                device,
                                tap_count: cx.fingers.tap_count(),
                                modifiers: e.modifiers,
                                time: e.time,
                                rect,
                            });
                        }
                        TouchState::Stop => {
                            let tap_count = cx.fingers.tap_count();
                            let rect = area.clipped_rect(&cx);
                            if let Some(capture) = cx.fingers.find_area_capture(area) {
                                return Hit::FingerUp(FingerUpEvent {
                                    abs_start: capture.abs_start,
                                    rect,
                                    window_id: e.window_id,
                                    abs: t.abs,
                                    digit_id,
                                    device,
                                    has_long_press_occurred: capture.has_long_press_occurred,
                                    tap_count,
                                    capture_time: capture.time,
                                    modifiers: e.modifiers,
                                    time: e.time,
                                    is_over: rect.contains(t.abs),
                                    is_sweep: false,
                                });
                            }
                        }
                        TouchState::Move => {
                            let tap_count = cx.fingers.tap_count();
                            //let hover_last = cx.fingers.get_hover_area(digit_id);
                            let rect = area.clipped_rect(&cx);
                            
                            //let handled_area = t.handled.get();
                            if !options.sweep_area.is_empty() {
                                if let Some(capture) = cx.fingers.find_digit_capture(digit_id) {
                                    if capture.switch_capture.is_none()
                                        && hit_test(t.abs, &rect, &options.margin)
                                    {
                                        if t.handled.get().is_empty() {
                                            t.handled.set(area);
                                            if capture.area == area {
                                                return Hit::FingerMove(FingerMoveEvent {
                                                    window_id: e.window_id,
                                                    abs: t.abs,
                                                    digit_id,
                                                    device,
                                                    tap_count,
                                                    modifiers: e.modifiers,
                                                    time: e.time,
                                                    abs_start: capture.abs_start,
                                                    rect,
                                                    is_over: true,
                                                });
                                            }
                                            else if capture.sweep_area == options.sweep_area { // take over the capture
                                                capture.switch_capture = Some(area);
                                                return Hit::FingerDown(FingerDownEvent {
                                                    window_id: e.window_id,
                                                    abs: t.abs,
                                                    digit_id,
                                                    device,
                                                    tap_count: cx.fingers.tap_count(),
                                                    modifiers: e.modifiers,
                                                    time: e.time,
                                                    rect,
                                                });
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
                                            abs: t.abs,
                                            digit_id,
                                            device,
                                            has_long_press_occurred: capture.has_long_press_occurred,
                                            tap_count,
                                            capture_time: capture.time,
                                            modifiers: e.modifiers,
                                            time: e.time,
                                            is_sweep: true,
                                            is_over: false,
                                        });
                                    }
                                }
                            }
                            else if let Some(capture) = cx.fingers.find_area_capture(area) {
                                return Hit::FingerMove(FingerMoveEvent {
                                    window_id: e.window_id,
                                    abs: t.abs,
                                    digit_id,
                                    device,
                                    tap_count,
                                    modifiers: e.modifiers,
                                    time: e.time,
                                    abs_start: capture.abs_start,
                                    rect,
                                    is_over: hit_test(t.abs, &rect, &options.margin),
                                })
                            }
                        }
                        TouchState::Stable => {}
                    }
                }
            }
            Event::MouseMove(e) => { // ok so we dont get hovers
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    // log!("Skipping MouseMove, sweep_area: {:?}", options.sweep_area);
                    return Hit::Nothing
                }
                
                let digit_id = live_id!(mouse).into();
                
                let tap_count = cx.fingers.tap_count();
                let hover_last = cx.fingers.find_hover_area(digit_id);
                let rect = area.clipped_rect(&cx);
                
                if let Some((button, _window_id)) = cx.fingers.first_mouse_button {
                    let device = DigitDevice::Mouse {
                        button,
                    };
                    //let handled_area = e.handled.get();
                    if !options.sweep_area.is_empty() {
                        if let Some(capture) = cx.fingers.find_digit_capture(digit_id) {
                            if capture.switch_capture.is_none()
                                && hit_test(e.abs, &rect, &options.margin) {
                                if e.handled.get().is_empty() {
                                    e.handled.set(area);
                                    if capture.area == area {
                                        return Hit::FingerMove(FingerMoveEvent {
                                            window_id: e.window_id,
                                            abs: e.abs,
                                            digit_id,
                                            device,
                                            tap_count,
                                            modifiers: e.modifiers,
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
                                            tap_count: cx.fingers.tap_count(),
                                            modifiers: e.modifiers,
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
                                    has_long_press_occurred: capture.has_long_press_occurred,
                                    tap_count,
                                    capture_time: capture.time,
                                    modifiers: e.modifiers,
                                    time: e.time,
                                    is_sweep: true,
                                    is_over: false,
                                });
                                
                            }
                        }
                    }
                    else if let Some(capture) = cx.fingers.find_area_capture(area) {
                        let event = Hit::FingerMove(FingerMoveEvent {
                            window_id: e.window_id,
                            abs: e.abs,
                            digit_id,
                            device,
                            tap_count,
                            modifiers: e.modifiers,
                            time: e.time,
                            abs_start: capture.abs_start,
                            rect,
                            is_over: hit_test(e.abs, &rect, &options.margin),
                        });
                        cx.fingers.new_hover_area(digit_id, area);
                        return event
                    }
                }
                else {
                    let device = DigitDevice::Mouse {
                        button: MouseButton::PRIMARY,
                    };
                    
                    let handled_area = e.handled.get();
                    
                    let fhe = FingerHoverEvent {
                        window_id: e.window_id,
                        abs: e.abs,
                        digit_id,
                        device,
                        modifiers: e.modifiers,
                        time: e.time,
                        rect,
                    };
                    
                    if hover_last == area {
                        if (handled_area.is_empty() || handled_area == area) && hit_test(e.abs, &rect, &options.margin) {
                            e.handled.set(area);
                            cx.fingers.new_hover_area(digit_id, area);
                            return Hit::FingerHoverOver(fhe)
                        }
                        else {
                            return Hit::FingerHoverOut(fhe)
                        }
                    }
                    else {
                        if (handled_area.is_empty() || handled_area == area) && hit_test(e.abs, &rect, &options.margin) {
                            //let any_captured = cx.fingers.get_digit_for_captured_area(area);
                            cx.fingers.new_hover_area(digit_id, area);
                            e.handled.set(area);
                            return Hit::FingerHoverIn(fhe)
                        }
                    }
                }
            },
            Event::MouseDown(e) => {
                                
                let digit_id = live_id!(mouse).into();
                                                
                let device = DigitDevice::Mouse {
                    button: e.button,
                };
                 
                // if we already captured it just return it immediately
                if cx.fingers.find_digit_for_captured_area(area).is_some() {
                    let rect = area.clipped_rect(&cx);
                    return Hit::FingerDown(FingerDownEvent {
                        window_id: e.window_id,
                        abs: e.abs,
                        digit_id,
                        device,
                        tap_count: cx.fingers.tap_count(),
                        modifiers: e.modifiers,
                        time: e.time,
                        rect,
                    })
                }
                
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    // log!("Skipping MouseDown, sweep_area: {:?}", options.sweep_area);
                    return Hit::Nothing
                }
                
                
                if !options.capture_overload && !e.handled.get().is_empty() {
                    return Hit::Nothing
                }
                
                if cx.fingers.first_mouse_button.is_some() && cx.fingers.first_mouse_button.unwrap().0 != e.button{
                    return Hit::Nothing
                }
                
                let rect = area.clipped_rect(&cx);
                if !hit_test(e.abs, &rect, &options.margin) {
                    return Hit::Nothing
                }
                
                
                if cx.fingers.find_digit_for_captured_area(area).is_some() {
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
                    tap_count: cx.fingers.tap_count(),
                    modifiers: e.modifiers,
                    time: e.time,
                    rect,
                })
            },
            Event::MouseUp(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    // log!("Skipping MouseUp, sweep_area: {:?}", options.sweep_area);
                    return Hit::Nothing
                }
                
                if cx.fingers.first_mouse_button.is_some() && cx.fingers.first_mouse_button.unwrap().0 != e.button {
                    return Hit::Nothing
                }
                
                let digit_id = live_id!(mouse).into();
                
                let device = DigitDevice::Mouse {
                    button: e.button,
                };
                let tap_count = cx.fingers.tap_count();
                let rect = area.clipped_rect(&cx);
                
                if let Some(capture) = cx.fingers.find_area_capture(area) {
                    let is_over = hit_test(e.abs, &rect, &options.margin);
                    let event = Hit::FingerUp(FingerUpEvent {
                        abs_start: capture.abs_start,
                        rect,
                        window_id: e.window_id,
                        abs: e.abs,
                        digit_id,
                        device,
                        has_long_press_occurred: capture.has_long_press_occurred,
                        tap_count,
                        capture_time: capture.time,
                        modifiers: e.modifiers,
                        time: e.time,
                        is_over,
                        is_sweep: false,
                    });
                    if is_over {
                        cx.fingers.new_hover_area(digit_id, area);
                    }
                    return event
                }
            },
            Event::MouseLeave(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    // log!("Skipping MouseLeave, sweep_area: {:?}", options.sweep_area);
                    return Hit::Nothing;
                }
                let device = DigitDevice::Mouse { button: MouseButton::empty() };
                let digit_id = live_id!(mouse).into();
                let rect = area.clipped_rect(&cx);
                let hover_last = cx.fingers.find_hover_area(digit_id);
                let handled_area = e.handled.get();
                
                let fhe = FingerHoverEvent {
                    window_id: e.window_id,
                    abs: e.abs,
                    digit_id,
                    device,
                    modifiers: e.modifiers,
                    time: e.time,
                    rect,
                };
                if hover_last == area {
                    return Hit::FingerHoverOut(fhe);
                }
            },
            Event::LongPress(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    log!("Skipping LongPress Hit, sweep_area: {:?}", options.sweep_area);
                    return Hit::Nothing
                }

                let rect = area.clipped_rect(&cx);
                if let Some(capture) = cx.fingers.find_area_capture(area) {
                    capture.has_long_press_occurred = true;
                    // No hit test is needed because we already did that in the previous
                    // FingerDown `capture` event that started the long press.
                    // Also, there is no need to include the starting position (`abs_start`)
                    // since it will always be identical to the `abs` position of the original capture.
                    let digit_id = live_id_num!(touch, e.uid).into();
                    let device = DigitDevice::Touch {
                        uid: e.uid,
                    };
                    return Hit::FingerLongPress(FingerLongPressEvent {
                        window_id: e.window_id,
                        abs: e.abs,
                        capture_time: capture.time,
                        time: e.time,
                        digit_id,
                        device,
                        rect,
                    });
                }
            },
            Event::DesignerPick(e) => {
               
                let rect = area.clipped_rect(&cx);
                if !hit_test(e.abs, &rect, &options.margin) {
                    return Hit::Nothing
                }
                // lets add our area to a handled vec?
                // but how will we communicate the widget?
                return Hit::DesignerPick(e.clone())
            },
            Event::XrLocal(e)=>{
                return e.hits_with_options_and_test(cx, area, options, hit_test)
            },
            _ => ()
        };
        Hit::Nothing
    }
}
