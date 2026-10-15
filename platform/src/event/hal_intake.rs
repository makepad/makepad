//! Event intake from the HAL: `RawEvent` -> `Event`, with the finger, tap count, hover cycle,
//! capture and dpi-override bookkeeping every backend used to repeat.
//!
//! This is the one copy (HAL.md section 1): macos.rs `cocoa_event_callback`, the X11 and Windows
//! callbacks and the iOS touch path each did the same work on their own event types. The HAL
//! delivers window-local points, modifier bits and key indices (`makepad_hal::input`); what the
//! apps see is unchanged.
//!
//! What the HAL's event table cannot carry yet is dropped with a one-time warning naming the gap:
//! scroll gesture phases, external drag items (the string table only runs Cx -> HAL), window
//! drag queries, and a paste source.

use crate::{
    cx::Cx,
    cx_api::CxOsApi,
    event::*,
    makepad_live_id::*,
    makepad_math::*,
    thread::SignalToUI,
    window::{CxWindowPool, WindowId},
};
use makepad_hal::{buffers::{read_domains, read_drag_items, read_items, read_strs}, domain_state, input, window_state, HalEvent, Id};
use std::{cell::Cell, cell::RefCell, rc::Rc, sync::{Arc, Mutex}};

pub(crate) fn window_id(id: Id) -> WindowId {
    WindowId(id.index as usize, id.gen as u64)
}

pub(crate) fn hal_window_id(id: WindowId) -> Id {
    Id::new(id.0 as u32, id.1 as u32)
}

pub(crate) fn key_modifiers(bits: u32) -> KeyModifiers {
    KeyModifiers {
        shift: bits & input::MOD_SHIFT != 0,
        control: bits & input::MOD_CONTROL != 0,
        alt: bits & input::MOD_ALT != 0,
        logo: bits & input::MOD_LOGO != 0,
    }
}

fn pinch_phase(phase: u32) -> PinchPhase {
    match phase {
        input::pinch::BEGIN => PinchPhase::Begin,
        input::pinch::END => PinchPhase::End,
        _ => PinchPhase::Update,
    }
}

fn scroll_phase(phase: u32) -> ScrollPhase {
    match phase {
        input::scroll::BEGAN => ScrollPhase::Began,
        input::scroll::TOUCHED => ScrollPhase::Touched,
        input::scroll::CHANGED => ScrollPhase::Changed,
        input::scroll::ENDED => ScrollPhase::Ended,
        input::scroll::MOMENTUM => ScrollPhase::Momentum,
        input::scroll::MOMENTUM_ENDED => ScrollPhase::MomentumEnded,
        _ => ScrollPhase::None,
    }
}

/// The dragged items of an event payload (strings ride in the payload).
fn drag_items(payload: &[u8], count: u32) -> Vec<DragItem> {
    read_drag_items(payload, count)
        .into_iter()
        .map(|(kind, value)| match kind {
            input::drag_item::FILE => DragItem::FilePath { path: value, internal_id: None },
            _ => DragItem::String { value, internal_id: None },
        })
        .collect()
}

fn domain_status(state: u32) -> DomainStatus {
    match state {
        domain_state::RUNNING => DomainStatus::Running,
        domain_state::CRASHED => DomainStatus::Crashed,
        domain_state::EXITED => DomainStatus::Exited,
        _ => DomainStatus::Compiling,
    }
}

fn touch_state(phase: u32) -> TouchState {
    match phase {
        input::touch::START => TouchState::Start,
        input::touch::STOP => TouchState::Stop,
        input::touch::STABLE => TouchState::Stable,
        _ => TouchState::Move,
    }
}

impl Cx {
    /// Feeds one HAL event to Cx. Returns false when the app is done (the last window closed).
    pub(crate) fn hal_intake(&mut self, time_ns: u64, event: HalEvent<'_>) -> bool {
        let time = self.hal.secs(time_ns);
        if matches!(
            event,
            HalEvent::MouseDown { .. } | HalEvent::MouseMove { .. } | HalEvent::MouseUp { .. } | HalEvent::Scroll { .. } | HalEvent::Pinch { .. } | HalEvent::Touch { .. } | HalEvent::KeyDown { .. } | HalEvent::KeyUp { .. } | HalEvent::Text { .. } | HalEvent::Ime { .. }
        ) && self.hal.input_ns == 0
        {
            // the oldest input a frame consumes is its cause (FrameMeta::input_ns)
            self.hal.input_ns = time_ns;
        }
        if makepad_hal::profile::on() {
            use makepad_hal::profile::{input, Input};
            match event {
                HalEvent::MouseMove { .. } => input(Input::Move),
                HalEvent::MouseDown { .. } => input(Input::ButtonDown),
                HalEvent::MouseUp { .. } => input(Input::ButtonUp),
                HalEvent::Scroll { .. } => input(Input::Scroll),
                _ => {}
            }
        }
        match event {
            // ---- pointer
            HalEvent::MouseDown { window, x, y, button, mods } => {
                let window_id = window_id(window);
                if !self.window_accepts_input(window_id) {
                    return true;
                }
                self.activate_window_on_pointer_down(window_id);
                let mut abs = dvec2(x, y);
                self.dpi_override_scale(&mut abs, window_id);
                self.fingers.process_tap_count(abs, time);
                let button = MouseButton::from_bits_retain(button);
                self.fingers.mouse_down(button, window_id);
                self.call_event_handler(&Event::MouseDown(MouseDownEvent {
                    abs,
                    button,
                    window_id,
                    modifiers: key_modifiers(mods),
                    handled: Cell::new(Default::default()),
                    time,
                }));
                self.update_pointer_capture_pacing();
            }
            HalEvent::MouseMove { window, x, y, dx, dy, mods } => {
                let window_id = window_id(window);
                if !self.window_accepts_input(window_id) {
                    return true;
                }
                let mut abs = dvec2(x, y);
                self.dpi_override_scale(&mut abs, window_id);
                let modifiers = key_modifiers(mods);
                self.call_event_handler(&Event::MouseMove(MouseMoveEvent {
                    abs,
                    lock_delta: dvec2(dx, dy),
                    window_id,
                    modifiers,
                    time,
                    handled: Cell::new(Default::default()),
                }));
                // the in-window drag protocol: a drag the app started follows the pointer
                if let Some(items) = self.hal.internal_drag_items.as_ref() {
                    self.call_event_handler(&Event::Drag(DragEvent {
                        modifiers,
                        handled: Arc::new(Mutex::new(false)),
                        abs,
                        items: items.clone(),
                        response: Arc::new(Mutex::new(DragResponse::None)),
                    }));
                    self.drag_drop.cycle_drag();
                }
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            HalEvent::MouseCancel { window, x, y, button, mods } => {
                let window_id = window_id(window);
                if self.windows.is_valid(window_id) {
                    self.dispatch_hosted_mouse_cancel(&makepad_studio_protocol::RemoteMouseUp {
                        time, x, y, button_raw_bits: button,
                        modifiers: makepad_studio_protocol::RemoteKeyModifiers::from_key_modifiers(&key_modifiers(mods)),
                    }, window_id, dvec2(0.0, 0.0));
                    self.update_pointer_capture_pacing();
                }
            }
            HalEvent::MouseUp { window, x, y, button, mods } => {
                let window_id = window_id(window);
                if !self.window_accepts_input(window_id) {
                    return true;
                }
                let mut abs = dvec2(x, y);
                self.dpi_override_scale(&mut abs, window_id);
                let button = MouseButton::from_bits_retain(button);
                let modifiers = key_modifiers(mods);
                self.call_event_handler(&Event::MouseUp(MouseUpEvent { abs, button, window_id, modifiers, time }));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.update_pointer_capture_pacing();
                if button == MouseButton::PRIMARY {
                    if let Some(items) = self.hal.internal_drag_items.take() {
                        self.call_event_handler(&Event::Drop(DropEvent { modifiers, handled: Arc::new(Mutex::new(false)), abs, items }));
                        self.drag_drop.cycle_drag();
                        self.call_event_handler(&Event::DragEnd);
                        self.drag_drop.cycle_drag();
                    }
                }
            }
            HalEvent::MouseLeave { window, x, y } => {
                let window_id = window_id(window);
                if !self.window_accepts_input(window_id) {
                    return true;
                }
                let mut abs = dvec2(x, y);
                self.dpi_override_scale(&mut abs, window_id);
                self.call_event_handler(&Event::MouseLeave(MouseLeaveEvent {
                    abs,
                    window_id,
                    modifiers: Default::default(),
                    time,
                    handled: Cell::new(Default::default()),
                }));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            HalEvent::Scroll { window, x, y, dx, dy, mods, precise, phase } => {
                let window_id = window_id(window);
                if !self.window_accepts_input(window_id) {
                    return true;
                }
                let mut abs = dvec2(x, y);
                self.dpi_override_scale(&mut abs, window_id);
                self.call_event_handler(&Event::Scroll(ScrollEvent {
                    window_id,
                    scroll: dvec2(dx, dy),
                    abs,
                    modifiers: key_modifiers(mods),
                    handled_x: Cell::new(false),
                    handled_y: Cell::new(false),
                    is_mouse: !precise,
                    time,
                    phase: scroll_phase(phase),
                }));
            }
            HalEvent::Pinch { window, x, y, scale, phase } => {
                let window_id = window_id(window);
                if !self.window_accepts_input(window_id) {
                    return true;
                }
                let mut abs = dvec2(x, y);
                self.dpi_override_scale(&mut abs, window_id);
                self.call_event_handler(&Event::Pinch(PinchEvent { window_id, abs, scale, phase: pinch_phase(phase), modifiers: Default::default(), time }));
            }
            HalEvent::Touch { window, count, touches } => {
                let window_id = window_id(window);
                if !self.window_accepts_input(window_id) {
                    return true;
                }
                let touches: Vec<TouchPoint> = read_items::<makepad_hal::TouchPoint>(touches, count)
                    .map(|t| {
                        let mut abs = dvec2(t.x as f64, t.y as f64);
                        self.dpi_override_scale(&mut abs, window_id);
                        TouchPoint {
                            state: touch_state(t.phase),
                            abs,
                            time,
                            uid: t.id as u64,
                            rotation_angle: 0.0,
                            force: t.force as f64,
                            radius: dvec2(0.0, 0.0),
                            handled: Cell::new(Default::default()),
                            sweep_lock: Cell::new(Default::default()),
                        }
                    })
                    .collect();
                self.touch_update(TouchUpdateEvent { time, window_id, modifiers: Default::default(), touches });
            }

            HalEvent::TouchCancel { window, count, touches } => {
                // The OS took the pointers away; they were not lifted. Each is cancelled for every
                // capture (`FingerCancel`, never a release; an internal drag ends with no drop).
                let window_id = window_id(window);
                if !self.window_accepts_input(window_id) {
                    return true;
                }
                for t in read_items::<makepad_hal::TouchPoint>(touches, count) {
                    let mut abs = dvec2(t.x as f64, t.y as f64);
                    self.dpi_override_scale(&mut abs, window_id);
                    let uid = t.id as u64;
                    let digit_id: DigitId = live_id_num!(touch, uid).into();
                    self.fingers.cancel_digit(digit_id);
                    self.call_event_handler(&Event::FingerCancel(FingerCancelEvent {
                        window_id,
                        digit_id,
                        device: DigitDevice::Touch { uid },
                        abs,
                        time,
                        modifiers: Default::default(),
                    }));
                }
                if self.hal.internal_drag_items.take().is_some() {
                    self.call_event_handler(&Event::DragEnd);
                    self.drag_drop.cycle_drag();
                }
            }
            HalEvent::BackGesture { window, state, x, y, progress, edge } => {
                let window_id = window_id(window);
                let abs = (!x.is_nan() && !y.is_nan()).then(|| {
                    let mut abs = dvec2(x, y);
                    self.dpi_override_scale(&mut abs, window_id);
                    abs
                });
                let state = match state {
                    input::back_state::START => BackGestureState::Start,
                    input::back_state::MOVE => BackGestureState::Move,
                    _ => BackGestureState::Cancel,
                };
                let edge = match edge {
                    input::back_edge::LEFT => BackGestureEdge::Left,
                    input::back_edge::RIGHT => BackGestureEdge::Right,
                    _ => BackGestureEdge::None,
                };
                self.call_event_handler(&Event::BackGesture(BackGestureEvent { state, abs, progress, edge, time }));
            }
            HalEvent::BackPressed { .. } => {
                let event = Event::BackPressed { handled: Cell::new(false) };
                self.call_event_handler(&event);
                let Event::BackPressed { handled } = event else { unreachable!() };
                if !handled.get() {
                    self.hal.frames.w.back_unhandled();
                }
            }
            HalEvent::LongPress { window, x, y, pointer } => {
                let window_id = window_id(window);
                let mut abs = dvec2(x, y);
                self.dpi_override_scale(&mut abs, window_id);
                self.call_event_handler(&Event::LongPress(LongPressEvent { window_id, abs, uid: pointer as u64, time }));
            }
            HalEvent::SelectionHandle { window, handle, phase, x, y } => {
                let window_id = window_id(window);
                let handle = match handle {
                    0 => SelectionHandleKind::Start,
                    1 => SelectionHandleKind::End,
                    _ => return true,
                };
                let phase = match phase {
                    0 => SelectionHandlePhase::Begin,
                    1 => SelectionHandlePhase::Move,
                    2 => SelectionHandlePhase::End,
                    _ => return true,
                };
                let mut abs = dvec2(x, y);
                self.dpi_override_scale(&mut abs, window_id);
                self.call_event_handler(&Event::SelectionHandleDrag(SelectionHandleDragEvent { handle, phase, abs, time }));
            }

            // ---- keyboard and text
            HalEvent::KeyDown { window: _, key, repeat, mods } => {
                let e = KeyEvent { key_code: KeyCode::from_index(key), is_repeat: repeat, modifiers: key_modifiers(mods), time };
                // the copy / cut chords ask the focused widget for its text; the answer is the clipboard
                if e.modifiers.logo || e.modifiers.control {
                    match e.key_code {
                        KeyCode::KeyC => self.hal_clipboard_chord(true),
                        KeyCode::KeyX => self.hal_clipboard_chord(false),
                        _ => {}
                    }
                }
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e));
            }
            HalEvent::KeyUp { window: _, key, mods } => {
                let e = KeyEvent { key_code: KeyCode::from_index(key), is_repeat: false, modifiers: key_modifiers(mods), time };
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e));
            }
            HalEvent::Text { window: _, kind, text } => {
                let input = String::from_utf8_lossy(text).into_owned();
                let was_paste = kind == input::text::PASTE;
                self.call_event_handler(&Event::TextInput(TextInputEvent { input, replace_last: false, was_paste, ..Default::default() }));
            }
            HalEvent::PayloadSize { bytes } => {
                self.hal.event_arena_min = self.hal.event_arena_min.max(bytes as usize);
            }
            #[cfg(target_os = "linux")]
            HalEvent::DirectDisplays { window, generation, inventory } => self.hal_display_inventory(window, generation, inventory),
            #[cfg(target_os = "linux")]
            HalEvent::DirectFrameTiming { window, generation, output, last_present_ns, refresh_interval } => {
                self.hal_display_timing(window, generation, output, last_present_ns, refresh_interval);
            }
            #[cfg(target_os = "linux")]
            HalEvent::DirectDisplayResult { req, command, ok, message } => self.hal_display_result(req, command, ok, message),
            #[cfg(target_os = "linux")]
            HalEvent::DirectInput { window, mouse_speed, touchpad_speed, natural_scroll, tap_to_click } => {
                if self.hal.direct_display.window == Some(window) {
                    crate::linux_input::report(mouse_speed, touchpad_speed, natural_scroll, tap_to_click);
                }
            }
            #[cfg(not(target_os = "linux"))]
            HalEvent::DirectDisplays { .. } | HalEvent::DirectFrameTiming { .. }
                | HalEvent::DirectDisplayResult { .. } | HalEvent::DirectInput { .. } => {}
            HalEvent::Clipboard { req, supported, content } => {
                use crate::clipboard_read::{ClipboardContent, ClipboardEvent, ClipboardFile, ClipboardImage};
                let content = match makepad_hal::clipboard::ClipboardData::read(content) {
                    Some(data) if supported => ClipboardContent {
                        supported: true,
                        text: (!data.text.is_empty()).then_some(data.text),
                        image: (!data.png.is_empty()).then_some(ClipboardImage::Png(Arc::new(data.png))),
                        files: data.files.into_iter().map(|(url, path)| ClipboardFile { url, path }).collect(),
                    },
                    _ => ClipboardContent::default(),
                };
                self.call_event_handler(&Event::Clipboard(ClipboardEvent { request: req, content: Arc::new(content) }));
            }
            HalEvent::ImeState { window: _, sel_start, sel_end, comp_start, comp_end, text } => {
                // the editor's full state; offsets are UTF-16 code units, u32::MAX is no composition
                let text = String::from_utf8_lossy(text).into_owned();
                let offset = |at: u32| CharOffset::from_utf16_index(&text, at as usize);
                let composition = (comp_start != u32::MAX && comp_end != u32::MAX).then(|| offset(comp_start)..offset(comp_end));
                let selection = offset(sel_start)..offset(sel_end);
                self.call_event_handler(&Event::TextInput(TextInputEvent {
                    full_state_sync: Some(FullTextState { text, selection, composition }),
                    ..Default::default()
                }));
            }
            HalEvent::ImeAction { window: _, action } => {
                let action = ImeAction::from_android_action_code(action as i32);
                self.call_event_handler(&Event::ImeAction(ImeActionEvent { action }));
            }
            HalEvent::ImeResize { window, height, open } => {
                let window_id = window_id(window);
                if !self.windows.is_valid(window_id) {
                    return true;
                }
                let height = self.windows[window_id].native_points_to_layout(height);
                if open {
                    if self.hal.ime_visible && (height - self.hal.ime_height).abs() < 0.5 {
                        return true;
                    }
                    self.hal.ime_visible = true;
                    self.hal.ime_height = height;
                    self.call_event_handler(&Event::VirtualKeyboard(VirtualKeyboardEvent::DidShow { height, time }));
                } else {
                    if !self.hal.ime_visible {
                        return true;
                    }
                    self.hal.ime_visible = false;
                    self.hal.ime_height = 0.0;
                    // With a physical keyboard the soft one hides while the focused TextInput
                    // stays active; dismissing it would stop hardware-key composition.
                    if !self.keyboard.has_physical_keyboard() {
                        self.text_ime_was_dismissed();
                    }
                    self.call_event_handler(&Event::VirtualKeyboard(VirtualKeyboardEvent::DidHide { time }));
                }
            }
            HalEvent::ImeAnimate { window, height, open, duration, curve } => {
                // the soft keyboard starts to move; its settled state follows as `ImeResize`
                let window_id = window_id(window);
                if !self.windows.is_valid(window_id) {
                    return true;
                }
                let height = self.windows[window_id].native_points_to_layout(height);
                // UIKit's standard timing curves as cubic beziers
                let ease = match curve {
                    input::ime_curve::EASE_IN_OUT => Ease::Bezier { cp0: 0.42, cp1: 0.0, cp2: 0.58, cp3: 1.0 },
                    input::ime_curve::EASE_IN => Ease::Bezier { cp0: 0.42, cp1: 0.0, cp2: 1.0, cp3: 1.0 },
                    input::ime_curve::EASE_OUT => Ease::Bezier { cp0: 0.0, cp1: 0.0, cp2: 0.58, cp3: 1.0 },
                    _ => Ease::Linear,
                };
                if open {
                    self.call_event_handler(&Event::VirtualKeyboard(VirtualKeyboardEvent::WillShow { time, height, ease, duration }));
                } else {
                    // the keyboard goes away (its hide key, a hardware keyboard): the focused field
                    // must not raise it again on its next draw; with a hardware keyboard the field
                    // stays first responder and keeps composing
                    if !self.keyboard.has_physical_keyboard() {
                        self.text_ime_was_dismissed();
                    }
                    self.call_event_handler(&Event::VirtualKeyboard(VirtualKeyboardEvent::WillHide { time, height: 0.0, ease, duration }));
                }
            }
            HalEvent::PhysicalKeyboard { window: _, connected } => self.update_physical_keyboard_state(connected),
            HalEvent::SafeArea { window, top, right, bottom, left } => {
                // kept: a window created after this event (the Activity reports its insets before
                // the app's window exists) takes them with its first geometry
                let native = SafeAreaInsets { top, right, bottom, left };
                self.hal.safe_area = native;
                let window_id = window_id(window);
                if !self.windows.is_valid(window_id) {
                    return true;
                }
                let w = &mut self.windows[window_id];
                let old_geom = w.window_geom.clone();
                w.window_geom.safe_area_insets = w.native_safe_area_insets_to_layout(native);
                let new_geom = w.window_geom.clone();
                if old_geom != new_geom {
                    self.call_event_handler(&Event::WindowGeomChange(WindowGeomChangeEvent { window_id, new_geom, old_geom }));
                    self.redraw_all();
                }
            }
            HalEvent::Ime { window: _, kind, start, end, text } => {
                let input = String::from_utf8_lossy(text).into_owned();
                // a composition preview replaces the previous preview; a commit replaces it when
                // it ends a composition or names a replaced range
                let (input, replace_last) = match kind {
                    input::ime::START | input::ime::UPDATE => {
                        self.hal.composing = true;
                        (input, true)
                    }
                    input::ime::CANCEL => {
                        self.hal.composing = false;
                        (String::new(), true)
                    }
                    _ => {
                        self.hal.composing = false;
                        (input, end != start)
                    }
                };
                self.call_event_handler(&Event::TextInput(TextInputEvent { input, replace_last, was_paste: false, ..Default::default() }));
            }

            // ---- windows
            HalEvent::Focus { window, focused } => {
                let window_id = window_id(window);
                if !self.windows.is_valid(window_id) {
                    return true;
                }
                if focused {
                    self.hal.focused = Some(window_id);
                    // repaint all window passes: a window can come back without its frame
                    for id in self.windows.id_iter().collect::<Vec<_>>() {
                        if let Some(main_pass_id) = self.windows[id].main_pass_id {
                            self.repaint_pass(main_pass_id);
                        }
                    }
                    self.call_event_handler(&Event::WindowGotFocus(window_id));
                } else {
                    self.call_event_handler(&Event::WindowLostFocus(window_id));
                }
            }
            HalEvent::Geom { window, x, y, w, h, dpi, state } => self.hal_window_geom(window_id(window), dvec2(x, y), dvec2(w, h), dpi as f64, state),
            HalEvent::Display { window, dpi, refresh: _ } => {
                let window_id = window_id(window);
                if self.windows.is_valid(window_id) {
                    let geom = self.windows[window_id].window_geom.clone();
                    let state = if geom.is_fullscreen { window_state::FULLSCREEN } else { window_state::RESTORE };
                    // the layout geometry back to what the OS reported (`native_window_geom_to_layout` inverse)
                    let scale = geom.dpi_factor / self.windows[window_id].native_dpi_factor();
                    self.hal_window_geom(window_id, geom.position, geom.inner_size * scale, dpi as f64, state);
                }
            }
            HalEvent::Chrome { window, x, y, w, h, topmost } => {
                #[cfg(target_os = "linux")]
                if w > 0.0 && h > 0.0 {
                    if let crate::cx::OsType::LinuxWindow(params) = &mut self.os_type { params.custom_window_chrome = true; }
                }
                // the window buttons' rect and the window level, ahead of the `Geom` that carries them on
                let window_id = window_id(window);
                if self.windows.is_valid(window_id) {
                    // Do not mix native points into the current layout geometry: it is the
                    // old side of the next change, and repeated Geom events must scale once.
                    self.hal.windows.entry(window_id.0).or_default().chrome = Some((Rect { pos: dvec2(x, y), size: dvec2(w, h) }, topmost));
                    #[cfg(target_os = "linux")]
                    {
                        let window = &mut self.windows[window_id];
                        window.uses_client_side_decorations = w > 0.0 && h > 0.0;
                        window.draws_chrome_buttons = window.uses_client_side_decorations;
                    }
                }
            }
            HalEvent::CloseRequested { window } => {
                let window_id = window_id(window);
                if !self.windows.is_valid(window_id) {
                    return true;
                }
                let accept_close = Rc::new(Cell::new(true));
                self.call_event_handler(&Event::WindowCloseRequested(WindowCloseRequestedEvent { window_id, accept_close: accept_close.clone() }));
                if accept_close.get() {
                    crate::remote::note_window_close_requested(window_id.id());
                    // the user dismissed it: the app does not close its own window this way
                    self.windows[window_id].is_created = false;
                    self.hal.frames.w.window_close(window);
                }
            }
            HalEvent::Closed { window } => {
                let window_id = window_id(window);
                if !self.windows.is_valid(window_id) {
                    return true;
                }
                let user_closed = crate::remote::take_window_close_requested(window_id.id()) || self.windows[window_id].is_created;
                let title = self.windows[window_id].create_title.clone();
                self.call_event_handler(&Event::WindowClosed(WindowClosedEvent { window_id }));
                self.windows[window_id].is_created = false;
                if user_closed {
                    crate::remote::note_user_closed_window(window_id.id(), &title);
                }
                self.hal.windows.remove(&window_id.0);
                if self.hal_hosted_extra_window(window_id) {
                    Self::send_hosted_window(crate::ime::HostedWindow::new(window_id.id(), crate::ime::hosted_window_op::CLOSED));
                }
                if let Some(hosted) = self.hal.hosted.as_mut() {
                    hosted.window_closed(window_id);
                }
                self.hal.frames.window_closed(hal_window_id(window_id));
                if !self.hal.windows.values().any(|w| w.confirmed) {
                    if user_closed {
                        crate::remote::note_user_closed_last_window();
                    }
                    self.call_event_handler(&Event::Shutdown);
                    return false;
                }
            }
            HalEvent::PopupDismissed { window } => {
                self.call_event_handler(&Event::PopupDismissed(PopupDismissedEvent { window_id: window_id(window), reason: PopupDismissReason::FocusLost }));
            }
            HalEvent::DragQuery { window, x, y } => {
                if let Some(answer) = self.hal_drag_query(window, x, y) {
                    self.hal.frames.w.drag_reply(window, input::reply::QUERY, answer);
                }
            }
            HalEvent::AppMessage { window, message } => {
                self.dispatch_studio_msg(makepad_studio_protocol::StudioToApp::Custom(String::from_utf8_lossy(message).into_owned()),
                    window_id(window), dvec2(0.0, 0.0));
            }
            HalEvent::ChildMessage { child, message } => {
                self.call_event_handler(&Event::ChildMessage { child, message: String::from_utf8_lossy(message).into_owned() });
            }
            HalEvent::ParentDragQuery { window, request, x, y } => {
                if let Some(answer) = self.hal_drag_query(window, x, y) {
                    self.hal.frames.w.child_drag_reply(window, request, answer);
                }
            }
            HalEvent::DragOver { window, x, y, phase: _, count, items } => {
                let window_id = window_id(window);
                if !self.window_accepts_input(window_id) {
                    return true;
                }
                let mut abs = dvec2(x, y);
                self.dpi_override_scale(&mut abs, window_id);
                let response = Arc::new(Mutex::new(DragResponse::None));
                self.call_event_handler(&Event::Drag(DragEvent {
                    modifiers: Default::default(),
                    handled: Arc::new(Mutex::new(false)),
                    abs,
                    items: Arc::new(drag_items(items, count)),
                    response: response.clone(),
                }));
                self.drag_drop.cycle_drag();
                let answer = match *response.lock().unwrap() {
                    DragResponse::None => input::drag_response::NONE,
                    DragResponse::Copy => input::drag_response::COPY,
                    DragResponse::Link => input::drag_response::LINK,
                    DragResponse::Move => input::drag_response::MOVE,
                };
                self.hal.frames.w.drag_reply(window, input::reply::OVER, answer);
            }
            HalEvent::DragDrop { window, x, y, dropped, count, items } => {
                let window_id = window_id(window);
                if !self.window_accepts_input(window_id) {
                    return true;
                }
                if dropped {
                    let mut abs = dvec2(x, y);
                    self.dpi_override_scale(&mut abs, window_id);
                    self.call_event_handler(&Event::Drop(DropEvent {
                        modifiers: Default::default(),
                        handled: Arc::new(Mutex::new(false)),
                        abs,
                        items: Arc::new(drag_items(items, count)),
                    }));
                    self.drag_drop.cycle_drag();
                } else {
                    // a drag that ended without a drop: the buttons are released all the same
                    self.call_event_handler(&Event::MouseUp(MouseUpEvent {
                        abs: dvec2(-100000.0, -100000.0),
                        button: MouseButton::PRIMARY,
                        window_id: CxWindowPool::id_zero(),
                        modifiers: Default::default(),
                        time,
                    }));
                    self.fingers.mouse_up(MouseButton::PRIMARY);
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                }
                self.call_event_handler(&Event::DragEnd);
                self.drag_drop.cycle_drag();
            }
            HalEvent::ResizeLoop { .. } => {} // the main thread tracks the resize, the sessions run on
            HalEvent::MenuCmd { cmd } => self.call_event_handler(&Event::MacosMenuCommand(LiveId(cmd))),

            // ---- app
            // (every backend paints its windows once after the app started)
            HalEvent::Startup {} => {
                if self.hal.hosted.is_some() {
                    Self::send_studio_message(crate::studio::AppToStudio::BeforeStartup);
                    self.set_physical_keyboard_state(true);
                }
                self.call_event_handler(&Event::Startup);
                self.redraw_all();
                if self.hal.hosted.is_some() {
                    use crate::studio::AppToStudio;
                    Self::send_studio_message(AppToStudio::AfterStartup);
                    for json in crate::ime::connect_announcements() {
                        Self::send_studio_message(AppToStudio::Custom(json));
                    }
                }
            }
            HalEvent::Lifecycle { kind } => match kind {
                makepad_hal::lifecycle::FOREGROUND => self.call_event_handler(&Event::Foreground),
                makepad_hal::lifecycle::BACKGROUND => self.call_event_handler(&Event::Background),
                makepad_hal::lifecycle::PAUSE => self.call_event_handler(&Event::Pause),
                makepad_hal::lifecycle::RESUME => self.call_event_handler(&Event::Resume),
                // the window's focus has its own event; the surface is the backend's (it rebuilds
                // the swapchain and reports the geometry), Cx only paints into the new one
                makepad_hal::lifecycle::FOCUS_GAINED | makepad_hal::lifecycle::FOCUS_LOST | makepad_hal::lifecycle::SURFACE_LOST => {}
                makepad_hal::lifecycle::SURFACE_AVAILABLE => self.hal_repaint_all(),
                _ => {
                    self.request_quit(QuitReason::App);
                    if !self.hal_handle_platform_ops() {
                        self.call_event_handler(&Event::Shutdown);
                        return false;
                    }
                }
            },
            HalEvent::Timer { timer } => {
                let e = TimerEvent { time: Some(time), timer_id: timer as u64 };
                self.handle_script_timer(&e);
                self.call_event_handler(&Event::Timer(e));
            }
            HalEvent::Wake {} => self.hal.wake = true,
            HalEvent::AppOpen { count, items } => {
                // the OS asks the app to open files and URLs
                let items: Vec<AppOpenItem> = read_drag_items(items, count)
                    .into_iter()
                    .map(|(kind, value)| match kind {
                        input::drag_item::FILE => AppOpenItem::Path(value.into()),
                        _ => AppOpenItem::Url(value),
                    })
                    .collect();
                if !items.is_empty() {
                    crate::app_open::post(items, AppOpenSource::System);
                }
            }

            // ---- child windows and domains (this app is a parent)
            HalEvent::ChildWindow { window, child, texture, w, h, dpi, title } => {
                self.call_event_handler(&Event::ChildWindow(ChildWindowEvent {
                    child,
                    window: crate::os::hal::lent_id(window),
                    texture: crate::os::hal::lent_id(texture),
                    width: w,
                    height: h,
                    dpi: dpi as f64,
                    title: String::from_utf8_lossy(title).into_owned(),
                }));
                self.hal_repaint_all();
            }
            HalEvent::ChildChrome { window, child, own } => {
                self.call_event_handler(&Event::ChildWindowAction(ChildWindowActionEvent {
                    child, window: LentId { index: window.index, gen: window.gen }, action: ChildWindowAction::OwnChrome(own),
                }));
            }
            HalEvent::ChildDragAnswer { window, child, request, answer } => {
                let response = match answer {
                    input::drag_query::CAPTION => WindowDragQueryResponse::Caption,
                    input::drag_query::CLIENT => WindowDragQueryResponse::Client,
                    input::drag_query::SYS_MENU => WindowDragQueryResponse::SysMenu,
                    _ => WindowDragQueryResponse::NoAnswer,
                };
                self.call_event_handler(&Event::ChildWindowAction(ChildWindowActionEvent {
                    child, window: LentId { index: window.index, gen: window.gen }, action: ChildWindowAction::DragAnswer { request, response },
                }));
            }
            HalEvent::ChildWindowState { window, child, state } => {
                self.call_event_handler(&Event::ChildWindowAction(ChildWindowActionEvent {
                    child, window: LentId { index: window.index, gen: window.gen }, action: ChildWindowAction::State(state),
                }));
            }
            HalEvent::ChildFrame { window, child, texture, frame } => {
                self.call_event_handler(&Event::ChildFrame(ChildFrameEvent {
                    child,
                    window: crate::os::hal::lent_id(window),
                    texture: crate::os::hal::lent_id(texture),
                    frame,
                }));
            }
            HalEvent::ChildWindowClosed { window, child } => {
                self.call_event_handler(&Event::ChildWindowClosed(ChildWindowClosedEvent { child, window: crate::os::hal::lent_id(window) }));
                self.hal_repaint_all();
            }
            HalEvent::DomainState { domain, state, phase, pct, req, text } => {
                let strs = read_strs(text, 2);
                self.call_event_handler(&Event::DomainState(DomainStateEvent {
                    domain,
                    status: domain_status(state),
                    phase,
                    percent: pct,
                    request: req,
                    app: strs.first().cloned().unwrap_or_default(),
                    message: strs.get(1).cloned().unwrap_or_default(),
                }));
            }
            // ---- the font service
            HalEvent::FontOpened { .. } | HalEvent::AtlasInfo { .. } | HalEvent::GlyphsReady { .. } | HalEvent::AtlasEvicted { .. } => self.hal_font_event(event),
            HalEvent::DomainListed { req, count, list } => {
                let domains = read_domains(list, count)
                    .into_iter()
                    .map(|d| DomainEntry { domain: d.domain, status: domain_status(d.state), phase: d.phase, percent: d.pct, app: d.app })
                    .collect();
                self.call_event_handler(&Event::DomainList(DomainListEvent { request: req, domains }));
            }

            // ---- GPU
            HalEvent::GpuBudget { epoch, device_bytes, used_bytes } => {
                if self.gpu_admission.update(epoch, device_bytes, used_bytes) {
                    self.hal_repaint_all();
                }
            }
            HalEvent::FrameReady { window, next_ns: _ } => {
                let window_id = window_id(window);
                if self.windows.is_valid(window_id) {
                    self.hal.beat = true;
                    if let Some(w) = self.hal.windows.get_mut(&window_id.0) {
                        w.paced = true;
                        w.credit = true;
                    }
                    makepad_hal::profile::credit();
                }
            }
            HalEvent::FrameDone { window, frame, gpu_ns, present_ns: _, gpu_start, gpu_end } => {
                if let Some((group, name, closes)) = self.hal.frames.timings.remove(&frame) {
                    if gpu_ns > 0 {
                        crate::gpu_frame_timer::record_pass(group, &name, gpu_start, gpu_end);
                    }
                    if closes { crate::gpu_frame_timer::finish_frame(group); }
                }
                self.hal.frames.frame_done(window, frame);
                // a draw item's backing is reusable once the frame that last read it is done
                self.textures.1.serials.complete(frame);
                let pool = self.task_pool();
                // a freed draw item's retained buffer goes with it
                let frames = &mut self.hal.frames;
                self.draw_lists.retire_free_items_with_ids(&pool, frame, |list, item, _| frames.inst_destroy((list.index(), item)));
                self.poll_texture_lifetimes();
                self.hal.frames_done += 1;
                match self.hal.hosted.as_mut().map(|h| h.completed(window, frame)) {
                    Some(crate::os::hal::HostedDone::Publish(draw)) => Self::send_studio_message(crate::studio::AppToStudio::DrawCompleteAndFlip(draw)),
                    // a hosted frame the GPU dropped: nothing to publish, paint it again
                    #[cfg(target_os = "linux")]
                    Some(crate::os::hal::HostedDone::Dropped) => self.hal_repaint_all(),
                    Some(crate::os::hal::HostedDone::Unknown) | None => {}
                }
                // (the first frame the HAL finished is the first one on the glass; once per app)
                crate::startup_trace_first_present();
            }
            HalEvent::PipelineReady { pipeline } => {
                // a draw built before the pipeline was ready was skipped by the HAL: paint again
                if self.hal.frames.pipeline_ready(pipeline) {
                    self.hal_repaint_all();
                }
            }
            HalEvent::PipelineFailed { pipeline, log } => {
                if let Some((shader, waited)) = self.hal.frames.pipeline_failed(pipeline) {
                    let name = self.draw_shaders[shader].debug_id;
                    let error = format!("hal: pipeline of shader {:?} failed: {}", name, String::from_utf8_lossy(log));
                    crate::shader_error::note(error.clone());
                    crate::error!("{}", error);
                    // a pass that kept its picture for this pipeline paints again
                    if waited { self.hal_repaint_all(); }
                }
            }
            HalEvent::BufferEvicted { buffer } => {
                if let Some((list, item)) = self.hal.frames.buffer_evicted(buffer) {
                    if list < self.draw_lists.0.pool.len() && !self.draw_lists.is_slot_freed(list) {
                        let items = &mut self.draw_lists.0.pool[list].item.draw_items;
                        if item < items.len() {
                            let it = items.binding_mut(item);
                            it.retained_gpu_evicted = true;
                            if let Some(call) = it.kind.draw_call_mut() {
                                call.instance_dirty = true;
                            }
                        }
                    }
                    self.hal_repaint_all();
                }
            }
            HalEvent::TextureEvicted { texture } => {
                // the HAL gave back the storage of a transient target nothing used for a while:
                // the allocation is released here too, and the producers paint it when it is
                // sampled again (the builder notices)
                // The pool slot may have been freed and handed to a new texture
                // after the HAL sent this: only a live generation is released.
                if let Some(id) = self.hal.frames.texture_pool_id(texture) {
                    if self.textures.0.is_live_generation(id.0, id.generation()) {
                        self.release_texture_allocation(id);
                    }
                }
            }
            HalEvent::DeviceLost { generation } => {
                // Nothing is drawn until DeviceReset: the event loop waits on the HAL's events
                // (no frames are built or asked of a host meanwhile), so this is said once.
                if !std::mem::replace(&mut self.hal.device_lost, true) {
                    crate::error!("hal: GPU device {generation} was lost; waiting for the HAL to replace it");
                }
                // frames sent to the lost device are never published to a host; its shared
                // images are imported again after the reset
                if let Some(hosted) = self.hal.hosted.as_mut() {
                    hosted.device_lost();
                }
            }
            HalEvent::DeviceReset { generation } => self.hal_device_reset(generation),
            HalEvent::GpuPressure { wanted_mb, free_mb, budget_mb, reserve_mb, failed } => {
                self.call_event_handler(&Event::GpuPressure(GpuPressureEvent { wanted_mb, free_mb, budget_mb, reserve_mb, failed }));
            }
            HalEvent::TextureShared { texture, handle } => {
                self.hal.frames.texture_shared(texture, handle);
                self.redraw_all();
            }
            #[cfg(not(target_os = "android"))]
            HalEvent::GlShare { display, config, context } => self.hal.gl.share = Some([display, config, context]),
            #[cfg(target_os = "android")]
            HalEvent::GlShare { .. } => {}
            HalEvent::TextureImported { texture, ok } => {
                if let Some(draw) = self.hal.hosted.as_mut().and_then(|hosted| hosted.imported(texture, ok)) {
                    Self::send_studio_message(crate::studio::AppToStudio::DrawCompleteAndFlip(draw));
                }
            }
            HalEvent::Readback { req, status, rect, pixels } => {
                self.hal_readback(req, status, rect, pixels);
            }
            // Cx asks for full pick images through TextureReadback; point picks belong to the host.
            HalEvent::PickResult { .. } => {},

            // ---- results
            HalEvent::DialogResult { req, count, paths } => {
                use crate::file_dialogs::FileDialogAction as A;
                use crate::os::DialogKind as K;
                let Some(dialog) = self.hal.dialogs.remove(&req) else { return true };
                let mut paths: Vec<std::path::PathBuf> = makepad_hal::buffers::read_strs(paths, count).into_iter().map(Into::into).collect();
                let id = dialog.id;
                let action = match (dialog.kind, paths.is_empty()) {
                    (K::SelectFile, true) => A::FileCancelled { id },
                    (K::SelectFile, false) => A::FileSelected { id, paths },
                    (K::SaveFile, true) => A::SaveFileCancelled { id },
                    (K::SaveFile, false) => A::SaveFileSelected { id, path: paths.swap_remove(0) },
                    (K::SelectFolder | K::SaveFolder, true) => A::FolderCancelled,
                    (K::SelectFolder | K::SaveFolder, false) => A::FolderSelected(paths.swap_remove(0)),
                };
                Cx::post_action(action);
            }
            HalEvent::Query { req, bytes } => self.hal_answer_query(req, bytes),

            // ---- media
            e @ (HalEvent::VideoPrepared { .. } | HalEvent::VideoFrame { .. } | HalEvent::VideoStatus { .. } | HalEvent::VideoRanges { .. } | HalEvent::AudioDevices { .. } | HalEvent::MidiPorts { .. } | HalEvent::MidiData { .. } | HalEvent::GameDevices { .. } | HalEvent::GameInput { .. } | HalEvent::Location { .. } | HalEvent::Permission { .. }) => self.hal_media_event(e),
            e @ (HalEvent::VideoInputs { .. } | HalEvent::CameraFrame { .. }) => self.hal_camera_event(e),
            e @ HalEvent::ScreenSurface { .. } => self.hal_media_event(e),
        }
        true
    }

    fn window_accepts_input(&self, window_id: WindowId) -> bool {
        self.windows.is_valid(window_id) && self.windows[window_id].is_created
    }

    /// Every pass paints again (a pipeline became ready, a retained buffer was given back).
    /// `DeviceReset`: the HAL made a new GPU device and kept every id; what the old device held
    /// is uploaded again and every target painted again. Frames the old device dropped are done.
    pub(crate) fn hal_device_reset(&mut self, generation: u32) {
        crate::log!("hal: GPU device {generation} is up; uploading and painting again");
        self.hal.device_lost = false;
        let serial = self.frame_submission_serial();
        self.textures.1.serials.complete(serial);
        let pool = self.task_pool();
        let frames = &mut self.hal.frames;
        self.draw_lists.retire_free_items_with_ids(&pool, serial, |list, item, _| frames.inst_destroy((list.index(), item)));
        self.poll_texture_lifetimes();
        let textures = &self.textures;
        let reupload = |slot: usize, gen: u64| {
            textures.0.is_live_generation(slot, gen) && textures[crate::texture::TextureId::from_pool_slot(slot, gen)].format.is_vec()
        };
        let items = self.hal.frames.device_reset(reupload);
        for (list, item) in items {
            if list < self.draw_lists.0.pool.len() && !self.draw_lists.is_slot_freed(list) {
                let items = &mut self.draw_lists.0.pool[list].item.draw_items;
                if item < items.len() {
                    let it = items.binding_mut(item);
                    it.retained_gpu_evicted = true;
                    if let Some(call) = it.kind.draw_call_mut() {
                        call.instance_dirty = true;
                    }
                }
            }
        }
        self.hal_repaint_all();
        // what only the GPU held is the app's to redo (`Event::GpuReset`)
        self.call_event_handler(&Event::GpuReset);
        self.redraw_all();
    }

    pub(crate) fn hal_repaint_all(&mut self) {
        for id in self.passes.id_iter().collect::<Vec<_>>() {
            if self.passes[id].main_draw_list_id.is_some() {
                self.passes[id].paint_dirty = true;
            }
        }
    }

    fn hal_drag_query(&mut self, window: Id, x: f64, y: f64) -> Option<u32> {
        let window_id = window_id(window);
        if !self.window_accepts_input(window_id) { return None; }
        let mut abs = dvec2(x, y);
        self.dpi_override_scale(&mut abs, window_id);
        let response = Rc::new(Cell::new(WindowDragQueryResponse::NoAnswer));
        self.call_event_handler(&Event::WindowDragQuery(WindowDragQueryEvent { window_id, abs, response: response.clone() }));
        Some(match response.get() {
            WindowDragQueryResponse::NoAnswer => input::drag_query::NO_ANSWER,
            WindowDragQueryResponse::Client => input::drag_query::CLIENT,
            WindowDragQueryResponse::Caption => input::drag_query::CAPTION,
            WindowDragQueryResponse::SysMenu => input::drag_query::SYS_MENU,
        })
    }

    /// The window's geometry as the HAL reports it: the same bookkeeping macOS did when Cocoa
    /// told it (dpi override, layout size, redraw on a size or dpi change).
    pub(crate) fn hal_window_geom(&mut self, window_id: WindowId, position: Vec2d, inner_size: Vec2d, dpi: f64, state: u32) {
        if !self.windows.is_valid(window_id) {
            return;
        }
        let hal_window = self.hal.windows.entry(window_id.0).or_default();
        let first = !hal_window.confirmed;
        hal_window.confirmed = true;
        let old_geom = self.windows[window_id].window_geom.clone();
        let mut new_geom = old_geom.clone();
        if let Some((buttons, topmost)) = hal_window.chrome {
            new_geom.window_chrome_buttons = buttons;
            new_geom.is_topmost = topmost;
        }
        new_geom.position = position;
        new_geom.inner_size = inner_size;
        new_geom.outer_size = inner_size;
        new_geom.dpi_factor = dpi;
        // A hosted window is fullscreen when its host window manager says so (`HostedFullscreen`).
        new_geom.is_fullscreen = state == window_state::FULLSCREEN || self.windows[window_id].hosted_fullscreen;
        let safe = self.hal.safe_area;
        let window = &mut self.windows[window_id];
        window.os_dpi_factor = Some(dpi);
        #[cfg(target_os = "linux")]
        { window.wayland_is_fullscreen = new_geom.is_fullscreen; }
        let mut new_geom = window.native_window_geom_to_layout(new_geom);
        new_geom.safe_area_insets = window.native_safe_area_insets_to_layout(safe);
        window.window_geom = new_geom.clone();
        if first || old_geom.dpi_factor != new_geom.dpi_factor || old_geom.inner_size != new_geom.inner_size {
            if let Some(main_pass_id) = self.windows[window_id].main_pass_id {
                self.redraw_pass_and_child_passes(main_pass_id);
            }
        }
        self.call_event_handler(&Event::WindowGeomChange(WindowGeomChangeEvent { window_id, old_geom, new_geom }));
    }

    /// Copy or cut: asks the focused widget for its text and puts the answer on the clipboard.
    /// A copy or cut of nothing (no selection) leaves the clipboard as it is.
    pub(crate) fn hal_clipboard_chord(&mut self, copy: bool) {
        let response = Rc::new(RefCell::new(None));
        let e = TextClipboardEvent { response: response.clone() };
        self.call_event_handler(&if copy { Event::TextCopy(e) } else { Event::TextCut(e) });
        let text = response.borrow().clone();
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            self.hal.frames.w.clip_set(text.as_bytes());
        }
    }

    /// A touch update (`TouchUpdate` carries every touch, a lifted one as `Stop`: the widgets turn
    /// it into the finger up). A cancel is `HalEvent::TouchCancel`, never a `Stop`.
    fn touch_update(&mut self, e: TouchUpdateEvent) {
        if e.touches.is_empty() {
            return;
        }
        self.fingers.process_touch_update_start(e.time, &e.touches);
        let e = Event::TouchUpdate(e);
        self.call_event_handler(&e);
        let Event::TouchUpdate(e) = e else { return };
        // an internal drag rides the touch: a lifted finger drops it, a moving one drags it
        if self.hal.internal_drag_items.is_some() {
            if let Some(touch) = e.touches.iter().find(|t| t.state == TouchState::Stop) {
                if let Some(items) = self.hal.internal_drag_items.take() {
                    self.call_event_handler(&Event::Drop(DropEvent {
                        modifiers: e.modifiers,
                        handled: Arc::new(Mutex::new(false)),
                        abs: touch.abs,
                        items,
                    }));
                    self.drag_drop.cycle_drag();
                    self.call_event_handler(&Event::DragEnd);
                    self.drag_drop.cycle_drag();
                }
            } else if let Some(touch) = e.touches.iter().find(|t| t.state == TouchState::Move) {
                if let Some(items) = self.hal.internal_drag_items.as_ref() {
                    self.call_event_handler(&Event::Drag(DragEvent {
                        modifiers: e.modifiers,
                        handled: Arc::new(Mutex::new(false)),
                        abs: touch.abs,
                        items: items.clone(),
                        response: Arc::new(Mutex::new(DragResponse::None)),
                    }));
                    self.drag_drop.cycle_drag();
                }
            }
        }
        self.fingers.process_touch_update_end(&e.touches);
    }

    /// Signals, actions and script tasks the loop services on a wake and on every beat (the
    /// work macOS did on its timer-0 beat).
    pub(crate) fn hal_service_signals(&mut self) -> bool {
        let internal_signal = SignalToUI::check_and_clear_internal_signal();
        let ui_signal = SignalToUI::check_and_clear_ui_signal();
        let mut did = false;
        if internal_signal || ui_signal {
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            self.handle_termination_signal();
            self.handle_script_signals();
            did = true;
        }
        if ui_signal {
            self.call_event_handler(&Event::Signal);
        }
        if SignalToUI::check_and_clear_action_signal() {
            self.handle_action_receiver();
            did = true;
        }
        self.poll_control_channel();
        self.poll_texture_readbacks();
        self.handle_actions();
        did
    }

    /// The time the app clock reads, for events the loop makes itself.
    pub(crate) fn hal_now(&self) -> f64 {
        self.seconds_since_app_start()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_bits_map_to_key_modifiers() {
        let m = key_modifiers(input::MOD_SHIFT | input::MOD_LOGO);
        assert!(m.shift && m.logo && !m.control && !m.alt);
        assert_eq!(key_modifiers(0), KeyModifiers::default());
    }

    #[test]
    fn window_ids_round_trip_through_hal_ids() {
        let w = WindowId(7, 3);
        assert_eq!(window_id(hal_window_id(w)), w);
    }

    #[test]
    fn key_indices_round_trip() {
        for k in [KeyCode::Escape, KeyCode::KeyA, KeyCode::ArrowRight, KeyCode::Unknown] {
            assert_eq!(KeyCode::from_index(k.to_index()), k);
        }
        assert_eq!(KeyCode::from_index(100_000), KeyCode::Unknown);
    }

    #[test]
    fn scroll_phases_and_drag_items_decode() {
        assert_eq!(scroll_phase(input::scroll::MOMENTUM_ENDED), ScrollPhase::MomentumEnded);
        assert_eq!(scroll_phase(0), ScrollPhase::None);
        let payload = makepad_hal::buffers::pack_drag_items(&[(input::drag_item::FILE, "/a/b"), (input::drag_item::STRING, "x")]);
        let items = drag_items(&payload, 2);
        assert_eq!(items[0], DragItem::FilePath { path: "/a/b".into(), internal_id: None });
        assert_eq!(items[1], DragItem::String { value: "x".into(), internal_id: None });
    }

    #[test]
    fn touch_and_pinch_phases() {
        assert_eq!(touch_state(input::touch::STOP), TouchState::Stop);
        assert_eq!(touch_state(99), TouchState::Move);
        assert_eq!(pinch_phase(input::pinch::BEGIN), PinchPhase::Begin);
        assert_eq!(pinch_phase(7), PinchPhase::Update);
    }
}
