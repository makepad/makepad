//! Game controllers: GameController-framework pads (with Core Haptics rumble) and raw-HID
//! wheels, joysticks and Xbox pads read by IOKit on a thread of their own. The module polls
//! them while the session asked for game input (`GameOut` ENABLE) and tells the session what
//! changed: the ordered device list (`GameDevices`) and a device's state (`GameInput`).
//!
//! Moved from os/apple/apple_game_input.rs. Wheel force feedback is the raw output report the
//! app writes (`GameOut` REPORT); controller rumble is a haptic pulse (`GameOut` PULSE).
//!
//! iOS and tvOS have the GameController pads and their haptics; IOKit HID (wheels, joysticks,
//! raw Xbox pads) is macOS only.

use super::Signal;
use crate::media::game::*;
use crate::media::game_output;
use makepad_apple_sys::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Controllers the GameController framework announced and the poll has not taken in yet.
#[derive(Default)]
pub struct Connections {
    pub connected: Vec<(u64, String)>,
    pub disconnected: Vec<u64>,
}

pub struct GameInput {
    gc_gamepads: Vec<GameInfo>,
    controllers: Vec<ObjcId>,
    gc_states: Vec<GameState>,
    haptics: Vec<Option<AppleControllerHaptics>>,
    #[cfg(target_os = "macos")]
    raw_hid: AppleRawHidInput,
    connections: Arc<Mutex<Connections>>,
    signal: Signal,
    /// the combined list the session last heard and the states it last heard
    pub infos: Vec<GameInfo>,
    pub states: Vec<GameState>,
    announced: Vec<GameInfo>,
}

// SAFETY: the controller objects are retained and used under the Hal's state lock only
unsafe impl Send for GameInput {}

impl GameInput {
    /// Starts watching controllers; `signal` rings when one connects or disconnects.
    pub fn new(signal: Signal) -> GameInput {
        // an initial poll finds the controllers that are already connected
        signal.set();
        let connections = Arc::new(Mutex::new(Connections::default()));
        unsafe {
            let gc_controller_class = class!(GCController);
            let sel_monitor = Sel::register("setShouldMonitorBackgroundEvents:");
            if msg_send![gc_controller_class, respondsToSelector: sel_monitor] {
                let () = msg_send![gc_controller_class, setShouldMonitorBackgroundEvents: YES];
            }

            let center: ObjcId = msg_send![class!(NSNotificationCenter), defaultCenter];
            let (list, ring) = (connections.clone(), signal.clone());
            let block = objc_block!(move |note: ObjcId| {
                let controller: ObjcId = msg_send![note, object];
                let vendor_name: ObjcId = msg_send![controller, vendorName];
                let name = nsstring_to_string(vendor_name);
                list.lock().unwrap_or_else(|e| e.into_inner()).connected.push((controller as u64, name));
                ring.set();
            });
            let () = msg_send![center, addObserverForName: GCControllerDidConnectNotification object: nil queue: nil usingBlock: &block];

            let (list, ring) = (connections.clone(), signal.clone());
            let block = objc_block!(move |note: ObjcId| {
                let controller: ObjcId = msg_send![note, object];
                list.lock().unwrap_or_else(|e| e.into_inner()).disconnected.push(controller as u64);
                ring.set();
            });
            let () = msg_send![center, addObserverForName: GCControllerDidDisconnectNotification object: nil queue: nil usingBlock: &block];

            let raw_controllers = Self::raw_connected_controllers(gc_controller_class);
            let discovery_sel = Sel::register("startWirelessControllerDiscoveryWithCompletionHandler:");
            if raw_controllers.is_empty() && msg_send![gc_controller_class, respondsToSelector: discovery_sel] {
                let block = objc_block!(move || {});
                let () = msg_send![gc_controller_class, startWirelessControllerDiscoveryWithCompletionHandler: &block];
            }
        }
        GameInput {
            gc_gamepads: Vec::new(),
            controllers: Vec::new(),
            gc_states: Vec::new(),
            haptics: Vec::new(),
            #[cfg(target_os = "macos")]
            raw_hid: AppleRawHidInput::new(signal.clone()),
            connections,
            signal,
            infos: Vec::new(),
            states: Vec::new(),
            announced: Vec::new(),
        }
    }

    unsafe fn controller_name(controller: ObjcId) -> String {
        let vendor_name: ObjcId = msg_send![controller, vendorName];
        if vendor_name == nil {
            "<nil>".to_string()
        } else {
            nsstring_to_string(vendor_name)
        }
    }

    unsafe fn raw_connected_controllers(gc_controller_class: &Class) -> Vec<(ObjcId, GameInfo)> {
        let controllers: ObjcId = msg_send![gc_controller_class, controllers];
        let count: usize = msg_send![controllers, count];
        let mut result = Vec::with_capacity(count);
        for index in 0..count {
            let controller: ObjcId = msg_send![controllers, objectAtIndex: index];
            let name = Self::controller_name(controller);
            result.push((controller, GameInfo { id: controller as u64, name, vendor: 0, product: 0 }));
        }
        result
    }

    fn sync_connected_controllers(&mut self) {
        unsafe {
            let gc_controller_class = class!(GCController);
            let raw_controllers = Self::raw_connected_controllers(gc_controller_class);
            for (_, info) in raw_controllers.iter() {
                self.on_connected(info);
            }
        }
    }

    fn on_connected(&mut self, info: &GameInfo) {
        if self.gc_gamepads.iter().any(|gamepad| gamepad.id == info.id) {
            return;
        }
        let ptr = info.id as ObjcId;
        unsafe {
            let _: ObjcId = msg_send![ptr, retain];
        }
        unsafe {
            // the pad rings the session whenever a value changes: no polling timer
            let extended: ObjcId = msg_send![ptr, extendedGamepad];
            if extended != nil {
                let ring = self.signal.clone();
                let handler = objc_block!(move |_gamepad: ObjcId, _element: ObjcId| {
                    ring.set();
                });
                let () = msg_send![extended, setValueChangedHandler: &handler];
            }
        }
        self.gc_gamepads.push(info.clone());
        self.controllers.push(ptr);
        self.gc_states.push(GameState::Gamepad(GamepadState::default()));
        self.haptics.push(unsafe { AppleControllerHaptics::new(ptr) });
    }

    fn on_disconnected(&mut self, id: u64) {
        if let Some(index) = self.gc_gamepads.iter().position(|g| g.id == id) {
            let ptr = self.controllers[index];
            self.gc_gamepads.remove(index);
            self.controllers.remove(index);
            self.gc_states.remove(index);
            self.haptics.remove(index);
            unsafe {
                let _: () = msg_send![ptr, release];
            }
        }
    }

    fn refresh_combined_states(&mut self) {
        self.infos.clear();
        self.infos.extend(self.gc_gamepads.iter().cloned());

        self.states.clear();
        self.states.extend(self.gc_states.iter().cloned());

        #[cfg(target_os = "macos")]
        for (info, state) in self.raw_hid.snapshot() {
            if self.infos.iter().any(|gamepad| gamepad.id == info.id) {
                continue;
            }
            self.infos.push(info);
            self.states.push(state);
        }
    }

    /// The device list entries for `GameDevices`, in order: the GameController pads (announced to
    /// the app as connections) first, then the raw-HID devices.
    pub fn device_list(&self) -> Vec<GameDevice> {
        self.infos
            .iter()
            .enumerate()
            .map(|(i, info)| {
                let gc = self.gc_gamepads.iter().position(|g| g.id == info.id);
                let haptics = gc.and_then(|g| self.haptics[g].as_ref()).map(AppleControllerHaptics::capabilities).unwrap_or_default();
                let _ = i;
                GameDevice { id: info.id, name: info.name.clone(), announce: gc.is_some(), haptics, vendor: info.vendor, product: info.product, output: if info.vendor != 0 { game_output::REPORTS } else { game_output::NONE } }
            })
            .collect()
    }

    /// One poll: takes in the connections, reads the pads. Returns whether the device list
    /// changed and the indexes whose state changed.
    pub fn step(&mut self) -> (bool, Vec<usize>) {
        let (connected, disconnected) = {
            let mut c = self.connections.lock().unwrap_or_else(|e| e.into_inner());
            (std::mem::take(&mut c.connected), std::mem::take(&mut c.disconnected))
        };
        for (id, name) in connected {
            self.on_connected(&GameInfo { id, name, vendor: 0, product: 0 });
        }
        for id in disconnected {
            self.on_disconnected(id);
        }
        let before = std::mem::take(&mut self.states);
        self.poll();
        let list_changed = self.infos != self.announced;
        if list_changed {
            self.announced = self.infos.clone();
        }
        let changed = (0..self.states.len()).filter(|i| list_changed || before.get(*i) != self.states.get(*i)).collect();
        (list_changed, changed)
    }

    /// One raw output report to a raw-HID device (a wheel's force feedback). False when the
    /// device is gone or the write failed.
    pub fn send_report(&self, id: u64, report_id: u8, data: &[u8]) -> bool {
        #[cfg(target_os = "macos")]
        return self.raw_hid.send_report(id, report_id, data);
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (id, report_id, data);
            false
        }
    }

    /// A haptic pulse on a pad. False for a pad without haptics.
    pub fn pulse(&mut self, id: u64, pulse: HapticPulse) -> bool {
        match self.gc_gamepads.iter().position(|info| info.id == id) {
            Some(index) => match &mut self.haptics[index] {
                Some(haptics) => unsafe { haptics.pulse(pulse) },
                None => false,
            },
            None => false,
        }
    }

    fn poll(&mut self) {
        self.sync_connected_controllers();
        for (i, controller) in self.controllers.iter().enumerate() {
            unsafe {
                let extended_gamepad: ObjcId = msg_send![*controller, extendedGamepad];
                if extended_gamepad != nil {
                    if let GameState::Gamepad(state) = &mut self.gc_states[i] {
                        let get_val = |btn: ObjcId| -> f32 {
                            if btn != nil {
                                let val: f32 = msg_send![btn, value];
                                if val == 0.0 {
                                    let pressed: BOOL = msg_send![btn, isPressed];
                                    if pressed == YES {
                                        return 1.0;
                                    }
                                }
                                val
                            } else {
                                0.0
                            }
                        };

                        let get_axis = |input: ObjcId| -> f32 {
                            if input != nil {
                                let val: f32 = msg_send![input, value];
                                val
                            } else {
                                0.0
                            }
                        };

                        state.a = get_val(msg_send![extended_gamepad, buttonA]);
                        state.b = get_val(msg_send![extended_gamepad, buttonB]);
                        state.x = get_val(msg_send![extended_gamepad, buttonX]);
                        state.y = get_val(msg_send![extended_gamepad, buttonY]);

                        state.left_shoulder = get_val(msg_send![extended_gamepad, leftShoulder]);
                        state.right_shoulder = get_val(msg_send![extended_gamepad, rightShoulder]);

                        state.left_trigger = get_val(msg_send![extended_gamepad, leftTrigger]);
                        state.right_trigger = get_val(msg_send![extended_gamepad, rightTrigger]);

                        state.select = get_val(msg_send![extended_gamepad, buttonOptions]);
                        state.start = get_val(msg_send![extended_gamepad, buttonMenu]);
                        let home = if msg_send![extended_gamepad, respondsToSelector: sel!(buttonHome)]
                        {
                            msg_send![extended_gamepad, buttonHome]
                        } else {
                            nil
                        };
                        state.home = get_val(home);

                        state.left_thumb =
                            get_val(msg_send![extended_gamepad, leftThumbstickButton]);
                        state.right_thumb =
                            get_val(msg_send![extended_gamepad, rightThumbstickButton]);

                        let dpad: ObjcId = msg_send![extended_gamepad, dpad];
                        if dpad != nil {
                            state.dpad_up = get_axis(msg_send![dpad, up]);
                            state.dpad_down = get_axis(msg_send![dpad, down]);
                            state.dpad_left = get_axis(msg_send![dpad, left]);
                            state.dpad_right = get_axis(msg_send![dpad, right]);
                        }

                        let left_stick: ObjcId = msg_send![extended_gamepad, leftThumbstick];
                        if left_stick != nil {
                            state.left_stick = [get_axis(msg_send![left_stick, xAxis]), get_axis(msg_send![left_stick, yAxis])];
                        }

                        let right_stick: ObjcId = msg_send![extended_gamepad, rightThumbstick];
                        if right_stick != nil {
                            state.right_stick = [get_axis(msg_send![right_stick, xAxis]), get_axis(msg_send![right_stick, yAxis])];
                        }
                    }
                }
            }
        }
        self.refresh_combined_states();
    }
}
#[link(name = "GameController", kind = "framework")]
unsafe extern "C" {
    static GCHapticsLocalityDefault: ObjcId;
    static GCHapticsLocalityLeftHandle: ObjcId;
    static GCHapticsLocalityRightHandle: ObjcId;
}
#[link(name = "CoreHaptics", kind = "framework")]
unsafe extern "C" {
    static CHHapticEventTypeHapticContinuous: ObjcId;
    static CHHapticEventParameterIDHapticIntensity: ObjcId;
    static CHHapticEventParameterIDHapticSharpness: ObjcId;
}

/// One platform-owned Core Haptics engine. Engines start asynchronously;
/// the UI can keep polling/publishing without waiting for Bluetooth or the
/// haptic server. Short retained players are reaped on the next sample.
struct AppleHapticChannel {
    engine: ObjcId,
    ready: Arc<AtomicBool>,
    players: Vec<(ObjcId, std::time::Instant)>,
}
impl AppleHapticChannel {
    unsafe fn new(device_haptics: ObjcId, locality: ObjcId) -> Option<Self> {
        let engine: ObjcId = msg_send![device_haptics, createEngineWithLocality: locality];
        if engine == nil {
            return None;
        }
        let _: ObjcId = msg_send![engine, retain];
        let () = msg_send![engine, setPlaysHapticsOnly: YES];
        let ready = Arc::new(AtomicBool::new(false));
        let ready_cb = Arc::clone(&ready);
        let completion = objc_block!(move |error: ObjcId| {
            ready_cb.store(error == nil, Ordering::Release);
        });
        let () = msg_send![engine, startWithCompletionHandler: &completion];
        Some(Self { engine, ready, players: Vec::new() })
    }

    unsafe fn pulse(&mut self, intensity: f32, sharpness: f32, duration_s: f32) -> bool {
        let now = std::time::Instant::now();
        self.players.retain(|(player, until)| {
            if *until > now {
                true
            } else {
                let _: () = msg_send![*player, release];
                false
            }
        });
        if !self.ready.load(Ordering::Acquire) || intensity <= 0.001 {
            return self.ready.load(Ordering::Acquire);
        }

        let intensity_param: ObjcId = msg_send![class!(CHHapticEventParameter), alloc];
        let intensity_param: ObjcId = msg_send![
            intensity_param,
            initWithParameterID: CHHapticEventParameterIDHapticIntensity
            value: intensity.clamp(0.0, 1.0)
        ];
        let sharpness_param: ObjcId = msg_send![class!(CHHapticEventParameter), alloc];
        let sharpness_param: ObjcId = msg_send![
            sharpness_param,
            initWithParameterID: CHHapticEventParameterIDHapticSharpness
            value: sharpness.clamp(0.0, 1.0)
        ];
        let parameters: ObjcId = msg_send![class!(NSMutableArray), arrayWithCapacity: 2usize];
        let () = msg_send![parameters, addObject: intensity_param];
        let () = msg_send![parameters, addObject: sharpness_param];

        let event: ObjcId = msg_send![class!(CHHapticEvent), alloc];
        let duration = duration_s.clamp(0.01, 0.25) as f64;
        let event: ObjcId = msg_send![
            event,
            initWithEventType: CHHapticEventTypeHapticContinuous
            parameters: parameters
            relativeTime: 0.0f64
            duration: duration
        ];
        let events: ObjcId = msg_send![class!(NSArray), arrayWithObject: event];
        let empty: ObjcId = msg_send![class!(NSArray), array];
        let mut error: ObjcId = nil;
        let pattern: ObjcId = msg_send![class!(CHHapticPattern), alloc];
        let pattern: ObjcId = msg_send![
            pattern,
            initWithEvents: events
            parameters: empty
            error: &mut error
        ];
        let mut ok = pattern != nil && error == nil;
        let player: ObjcId = if ok {
            msg_send![self.engine, createPlayerWithPattern: pattern error: &mut error]
        } else {
            nil
        };
        if player != nil && error == nil {
            let _: ObjcId = msg_send![player, retain];
            let started: BOOL = msg_send![player, startAtTime: 0.0f64 error: &mut error];
            ok = started == YES && error == nil;
            if ok {
                self.players.push((player, now + std::time::Duration::from_secs_f64(duration + 0.05)));
            } else {
                let _: () = msg_send![player, release];
            }
        } else {
            ok = false;
        }
        let _: () = msg_send![intensity_param, release];
        let _: () = msg_send![sharpness_param, release];
        let _: () = msg_send![event, release];
        if pattern != nil {
            let _: () = msg_send![pattern, release];
        }
        ok
    }
}
impl Drop for AppleHapticChannel {
    fn drop(&mut self) {
        unsafe {
            for (player, _) in self.players.drain(..) {
                let mut error: ObjcId = nil;
                let _: BOOL = msg_send![player, stopAtTime: 0.0f64 error: &mut error];
                let _: () = msg_send![player, release];
            }
            let () = msg_send![self.engine, stopWithCompletionHandler: nil];
            let _: () = msg_send![self.engine, release];
        }
    }
}

/// Per-controller left/right handle channels. A controller with only the
/// default locality uses one channel and receives the stronger side.
struct AppleControllerHaptics {
    left: AppleHapticChannel,
    right: Option<AppleHapticChannel>,
}
impl AppleControllerHaptics {
    unsafe fn new(controller: ObjcId) -> Option<Self> {
        let selector = Sel::register("haptics");
        // the reply's type is the call's own (msg_send! returns any R): `!` gives the operand
        // no expected type, so it is named here (rustc E0282, Relax E0283 without it)
        let responds: bool = msg_send![controller, respondsToSelector: selector];
        if !responds {
            return None;
        }
        let haptics: ObjcId = msg_send![controller, haptics];
        if haptics == nil {
            return None;
        }
        let supported: ObjcId = msg_send![haptics, supportedLocalities];
        let has_left: BOOL = msg_send![supported, containsObject: GCHapticsLocalityLeftHandle];
        let has_right: BOOL = msg_send![supported, containsObject: GCHapticsLocalityRightHandle];
        if has_left == YES && has_right == YES {
            Some(Self {
                left: AppleHapticChannel::new(haptics, GCHapticsLocalityLeftHandle)?,
                right: AppleHapticChannel::new(haptics, GCHapticsLocalityRightHandle),
            })
        } else {
            Some(Self {
                left: AppleHapticChannel::new(haptics, GCHapticsLocalityDefault)?,
                right: None,
            })
        }
    }

    fn capabilities(&self) -> HapticCaps {
        HapticCaps { handles: true, separate_handles: self.right.is_some() }
    }

    unsafe fn pulse(&mut self, pulse: HapticPulse) -> bool {
        if let Some(right) = &mut self.right {
            let left_ok = self.left.pulse(pulse.left, pulse.sharpness, pulse.duration_s);
            let right_ok = right.pulse(pulse.right, pulse.sharpness, pulse.duration_s);
            left_ok && right_ok
        } else {
            self.left.pulse(pulse.left.max(pulse.right), pulse.sharpness, pulse.duration_s)
        }
    }
}

#[cfg(target_os = "macos")]
use raw_hid::AppleRawHidInput;

/// Raw HID through IOKit (macOS): racing wheels, joysticks and Xbox pads the GameController
/// framework does not cover, read on a thread of their own.
#[cfg(target_os = "macos")]
mod raw_hid {
    use super::super::util::cfstring_ref_to_string;
    use super::super::Signal;
    use crate::media::game::*;
    use makepad_apple_sys::*;
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};

    const APPLE_RAW_HID_XBOX_VENDOR_ID: u32 = 0x045e;
    /// Racing-wheel vendors the raw HID path claims: their reports are parsed
    /// by HID usage (steering = X, pedals = the next axes, buttons = the button
    /// page) and the device is handed out for output reports — the way force
    /// feedback is driven, which the Game Controller framework does not
    /// expose. Logitech, Thrustmaster, Fanatec, Moza, Simucube, Asetek. A
    /// device from one of these vendors that is not a known wheel (a Logitech
    /// Extreme 3D, a Thrustmaster HOTAS) is a JOYSTICK, as is any other
    /// joystick-class device (primary usage Joystick or Multi-axis).
    const APPLE_RAW_HID_WHEEL_VENDOR_IDS: [u32; 6] = [0x046d, 0x044f, 0x0eb7, 0x346e, 0x16d0, 0x2433];

    /// Is this vendor/product a racing wheel? Fanatec, Moza, Simucube and
    /// Asetek make nothing else; Logitech and Thrustmaster need their wheel
    /// product tables (the same ids the FFB protocols key on).
    fn apple_raw_hid_is_wheel(vendor_id: u32, product_id: u32) -> bool {
        match vendor_id {
            0x0eb7 | 0x346e | 0x16d0 | 0x2433 => true,
            // Logitech: WingMan FFG, MOMO, DFP, G25, DFGT, G27, MOMO2, G29,
            // G920, G923 (PS), G923 (Xbox), Driving Force, Formula Force EX.
            0x046d => matches!(
                product_id,
                0xc293 | 0xc295 | 0xc298 | 0xc299 | 0xc29a | 0xc29b | 0xca03 | 0xc24f | 0xc262 | 0xc266 | 0xc267 | 0xc294 | 0xc29c
            ),
            // Thrustmaster: T150, T300RS (3 modes), T500RS, TX, T248, TS-PC, T-GT, TMX, T80.
            0x044f => matches!(
                product_id,
                0xb677 | 0xb66e | 0xb66f | 0xb65e | 0xb65d | 0xb669 | 0xb696 | 0xb689 | 0xb684 | 0xb67f | 0xb66d
            ),
            _ => false,
        }
    }
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum AppleRawHidKind {
        Wheel,
        Joystick,
    }
    const HID_USAGE_GENERIC_DESKTOP_MULTI_AXIS: i32 = 0x08;
    const HID_PAGE_GENERIC_DESKTOP: u32 = 0x01;
    const HID_USAGE_PAGE_BUTTON: u32 = 0x09;
    const HID_USAGE_X: u32 = 0x30;
    const HID_USAGE_Y: u32 = 0x31;
    const HID_USAGE_Z: u32 = 0x32;
    const HID_USAGE_RX: u32 = 0x33;
    const HID_USAGE_RY: u32 = 0x34;
    const HID_USAGE_RZ: u32 = 0x35;
    const HID_USAGE_SLIDER: u32 = 0x36;
    const HID_USAGE_HAT: u32 = 0x39;
    const HID_USAGE_PAGE_GENERIC_DESKTOP: i32 = 0x01;
    const HID_USAGE_GENERIC_DESKTOP_JOYSTICK: i32 = 0x04;
    const HID_USAGE_GENERIC_DESKTOP_GAMEPAD: i32 = 0x05;
    const XBOX_ONE_REPORT_BUTTONS: u8 = 0x20;
    const XBOX_ONE_REPORT_HOME: u8 = 0x07;
    const XBOX_ONE_TRIGGER_MAX: f32 = 1023.0;
    pub(super) struct AppleRawHidInput {
        shared: Arc<Mutex<AppleRawHidShared>>,
        run_loop: Arc<Mutex<usize>>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }
    impl AppleRawHidInput {
        pub(super) fn new(signal: Signal) -> Self {
            let shared = Arc::new(Mutex::new(AppleRawHidShared { signal: Some(signal), ..Default::default() }));
            let run_loop = Arc::new(Mutex::new(0usize));
            let stop = Arc::new(AtomicBool::new(false));

            let thread_shared = Arc::clone(&shared);
            let thread_run_loop = Arc::clone(&run_loop);
            let thread_stop = Arc::clone(&stop);
            let thread = thread::spawn(move || {
                Self::thread_main(thread_shared, thread_run_loop, thread_stop);
            });

            Self {
                shared,
                run_loop,
                stop,
                thread: Some(thread),
            }
        }

        pub(super) fn snapshot(&self) -> Vec<(GameInfo, GameState)> {
            if let Ok(shared) = self.shared.lock() {
                let mut out: Vec<(GameInfo, GameState)> = shared
                    .devices
                    .iter()
                    .map(|device| {
                        (
                            device.info.clone(),
                            GameState::Gamepad(device.state.clone()),
                        )
                    })
                    .collect();
                out.extend(shared.wheels.iter().map(|wheel| {
                    let state = match wheel.kind {
                        AppleRawHidKind::Wheel => GameState::Wheel(wheel.state.clone()),
                        AppleRawHidKind::Joystick => GameState::Joystick(wheel.stick.clone()),
                    };
                    (wheel.info.clone(), state)
                }));
                return out;
            }
            Vec::new()
        }

        /// One output report to a raw-HID device (wheels only: a pad's rumble goes through the Game
        /// Controller framework, not here). Holds the shared table for the duration of one SetReport,
        /// so a device unplugged mid-write fails the write instead of dangling.
        pub(super) fn send_report(&self, id: u64, report_id: u8, data: &[u8]) -> bool {
            let Ok(shared) = self.shared.lock() else { return false };
            let Some(wheel) = shared.wheels.iter().find(|w| w.info.id == id) else {
                return false;
            };
            unsafe { IOHIDDeviceSetReport(wheel.device, kIOHIDReportTypeOutput, report_id as CFIndex, data.as_ptr(), data.len() as CFIndex) == 0 }
        }

        fn stop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Ok(run_loop) = self.run_loop.lock() {
                if *run_loop != 0 {
                    unsafe {
                        CFRunLoopStop(*run_loop as CFRunLoopRef);
                    }
                }
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }

        fn thread_main(
            shared: Arc<Mutex<AppleRawHidShared>>,
            run_loop_slot: Arc<Mutex<usize>>,
            stop: Arc<AtomicBool>,
        ) {
            unsafe {
                let run_loop = CFRunLoopGetCurrent();
                if let Ok(mut slot) = run_loop_slot.lock() {
                    *slot = run_loop as usize;
                }

                let manager = IOHIDManagerCreate(ptr::null(), 0);
                if manager.is_null() {
                    return;
                }

                let matching = Self::create_matching_multiple();
                IOHIDManagerSetDeviceMatchingMultiple(manager, matching);

                let callback_context = Box::new(AppleRawHidCallbackContext {
                    shared: Arc::clone(&shared),
                });
                let callback_context_ptr = Box::into_raw(callback_context);

                IOHIDManagerRegisterDeviceMatchingCallback(
                    manager,
                    Some(raw_hid_device_matching_callback),
                    callback_context_ptr as *mut _,
                );
                IOHIDManagerRegisterDeviceRemovalCallback(
                    manager,
                    Some(raw_hid_device_removal_callback),
                    callback_context_ptr as *mut _,
                );
                IOHIDManagerScheduleWithRunLoop(manager, run_loop, kCFRunLoopDefaultMode);

                let open_result = IOHIDManagerOpen(manager, 0);
                let _ = open_result;

                Self::enumerate_existing_devices(manager, callback_context_ptr as *mut _);

                if !stop.load(Ordering::Relaxed) {
                    CFRunLoopRun();
                }

                IOHIDManagerUnscheduleFromRunLoop(manager, run_loop, kCFRunLoopDefaultMode);
                let _ = IOHIDManagerClose(manager, 0);
                CFRelease(manager as *const _);
                drop(Box::from_raw(callback_context_ptr));
            }
        }

        unsafe fn create_matching_multiple() -> CFArrayRef {
            let gamepad = Self::create_usage_matching_dict(
                HID_USAGE_PAGE_GENERIC_DESKTOP,
                HID_USAGE_GENERIC_DESKTOP_GAMEPAD,
            );
            let joystick = Self::create_usage_matching_dict(
                HID_USAGE_PAGE_GENERIC_DESKTOP,
                HID_USAGE_GENERIC_DESKTOP_JOYSTICK,
            );
            let multi_axis = Self::create_usage_matching_dict(
                HID_USAGE_PAGE_GENERIC_DESKTOP,
                HID_USAGE_GENERIC_DESKTOP_MULTI_AXIS,
            );
            let values = [gamepad as *const _, joystick as *const _, multi_axis as *const _];
            CFArrayCreate(
                ptr::null(),
                values.as_ptr(),
                values.len() as isize,
                ptr::null(),
            )
        }

        unsafe fn create_usage_matching_dict(usage_page: i32, usage: i32) -> CFDictionaryRef {
            let usage_page_key = Self::cf_string("DeviceUsagePage");
            let usage_key = Self::cf_string("DeviceUsage");
            let usage_page_value = CFNumberCreate(
                ptr::null(),
                kCFNumberSInt32Type,
                &usage_page as *const _ as *const _,
            );
            let usage_value = CFNumberCreate(
                ptr::null(),
                kCFNumberSInt32Type,
                &usage as *const _ as *const _,
            );
            let keys = [usage_page_key as *const _, usage_key as *const _];
            let values = [usage_page_value as *const _, usage_value as *const _];
            CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                keys.len() as isize,
                ptr::null(),
                ptr::null(),
            )
        }

        unsafe fn enumerate_existing_devices(manager: IOHIDManagerRef, context: *mut std::ffi::c_void) {
            let devices = IOHIDManagerCopyDevices(manager);
            if devices.is_null() {
                return;
            }
            let count = CFSetGetCount(devices);
            if count > 0 {
                let mut values = vec![ptr::null(); count as usize];
                CFSetGetValues(devices, values.as_mut_ptr());
                for value in values {
                    if !value.is_null() {
                        Self::register_device(context, value as IOHIDDeviceRef);
                    }
                }
            }
            CFRelease(devices as *const _);
        }

        unsafe fn register_device(context: *mut std::ffi::c_void, device: IOHIDDeviceRef) {
            let callback_context = &*(context as *const AppleRawHidCallbackContext);
            let vendor_id = Self::device_u32_property(device, "VendorID");
            let product_id = Self::device_u32_property(device, "ProductID");
            let primary_usage = Self::device_u32_property(device, "PrimaryUsage") as i32;
            let joystick_class = primary_usage == HID_USAGE_GENERIC_DESKTOP_JOYSTICK
                || primary_usage == HID_USAGE_GENERIC_DESKTOP_MULTI_AXIS;
            if apple_raw_hid_is_wheel(vendor_id, product_id) {
                Self::register_axes_device(context, device, vendor_id, product_id, AppleRawHidKind::Wheel);
                return;
            }
            if APPLE_RAW_HID_WHEEL_VENDOR_IDS.contains(&vendor_id) || joystick_class {
                if vendor_id == APPLE_RAW_HID_XBOX_VENDOR_ID {
                    // An Xbox pad is a pad whatever it claims.
                } else {
                    Self::register_axes_device(context, device, vendor_id, product_id, AppleRawHidKind::Joystick);
                    return;
                }
            }
            if vendor_id != APPLE_RAW_HID_XBOX_VENDOR_ID {
                return;
            }

            let product_id = Self::device_u32_property(device, "ProductID");
            let location_id = Self::device_u32_property(device, "LocationID");
            let report_size = Self::device_u32_property(device, "MaxInputReportSize").max(32);
            let name = Self::device_string_property(device, "Product");
            let info = GameInfo {
                id: (if location_id != 0 {
                    ((vendor_id as u64) << 48) | ((product_id as u64) << 32) | location_id as u64
                } else {
                    device as u64
                }),
                name: if name.is_empty() {
                    format!("Xbox Controller {:04x}:{:04x}", vendor_id, product_id)
                } else {
                    format!("{name} {:04x}:{:04x}", vendor_id, product_id)
                },
                vendor: vendor_id,
                product: product_id,
            };

            let mut shared = match callback_context.shared.lock() {
                Ok(shared) => shared,
                Err(_) => return,
            };
            if shared.devices.iter().any(|entry| entry.device == device) {
                return;
            }

            let _ = CFRetain(device as *const _);
            let _ = IOHIDDeviceOpen(device, 0);

            shared.devices.push(AppleRawHidDevice {
                device,
                info: info.clone(),
                state: GamepadState::default(),
                report_buffer: vec![0u8; report_size as usize].into_boxed_slice(),
            });

            let index = shared.devices.len() - 1;
            let entry = &mut shared.devices[index];
            IOHIDDeviceRegisterInputReportCallback(
                device,
                entry.report_buffer.as_mut_ptr(),
                entry.report_buffer.len() as isize,
                Some(raw_hid_report_callback),
                context,
            );
        }

        unsafe fn remove_device(context: *mut std::ffi::c_void, device: IOHIDDeviceRef) {
            let callback_context = &*(context as *const AppleRawHidCallbackContext);
            let mut shared = match callback_context.shared.lock() {
                Ok(shared) => shared,
                Err(_) => return,
            };
            if let Some(index) = shared
                .devices
                .iter()
                .position(|entry| entry.device == device)
            {
                let entry = shared.devices.remove(index);
                let _ = IOHIDDeviceClose(entry.device, 0);
                CFRelease(entry.device as *const _);
            }
            if let Some(index) = shared.wheels.iter().position(|entry| entry.device == device) {
                let entry = shared.wheels.remove(index);
                let _ = IOHIDDeviceClose(entry.device, 0);
                CFRelease(entry.device as *const _);
            }
        }

        /// A racing wheel: parsed by HID USAGE rather than a hand-decoded report,
        /// because every brand lays its report out differently while all of
        /// them declare X as the wheel and the pedals as the next generic-desktop
        /// axes. Pedals rest at their logical MAXIMUM on Logitech (255 =
        /// released) and at the minimum on others; the first value seen from an
        /// untouched pedal decides, per axis (`rest_high`).
        unsafe fn register_axes_device(
            context: *mut std::ffi::c_void,
            device: IOHIDDeviceRef,
            vendor_id: u32,
            product_id: u32,
            kind: AppleRawHidKind,
        ) {
            let callback_context = &*(context as *const AppleRawHidCallbackContext);
            let location_id = Self::device_u32_property(device, "LocationID");
            let name = Self::device_string_property(device, "Product");
            let info = GameInfo {
                id: (if location_id != 0 {
                    ((vendor_id as u64) << 48) | ((product_id as u64) << 32) | location_id as u64
                } else {
                    device as u64
                }),
                name: if name.is_empty() {
                    format!("{:?} {:04x}:{:04x}", kind, vendor_id, product_id)
                } else {
                    format!("{name} {:04x}:{:04x}", vendor_id, product_id)
                },
                vendor: vendor_id,
                product: product_id,
            };
            let mut shared = match callback_context.shared.lock() {
                Ok(shared) => shared,
                Err(_) => return,
            };
            if shared.wheels.iter().any(|entry| entry.device == device) {
                return;
            }
            let _ = CFRetain(device as *const _);
            let _ = IOHIDDeviceOpen(device, 0);
            shared.wheels.push(AppleRawHidWheel {
                device,
                info,
                kind,
                state: WheelState::default(),
                stick: JoystickState::default(),
                pedal_rest: [None; 3],
            });
            IOHIDDeviceRegisterInputValueCallback(device, Some(raw_hid_value_callback), context);
        }

        unsafe fn handle_value(context: *mut std::ffi::c_void, sender: *mut std::ffi::c_void, value: IOHIDValueRef) {
            let callback_context = &*(context as *const AppleRawHidCallbackContext);
            let device = sender as IOHIDDeviceRef;
            let element = IOHIDValueGetElement(value);
            if element.is_null() {
                return;
            }
            let page = IOHIDElementGetUsagePage(element);
            let usage = IOHIDElementGetUsage(element);
            let raw = IOHIDValueGetIntegerValue(value) as i64;
            let lo = IOHIDElementGetLogicalMin(element) as i64;
            let hi = IOHIDElementGetLogicalMax(element) as i64;
            let mut shared = match callback_context.shared.lock() {
                Ok(shared) => shared,
                Err(_) => return,
            };
            let Some(wheel) = shared.wheels.iter_mut().find(|entry| entry.device == device) else {
                return;
            };
            Self::apply_wheel_value(wheel, page, usage, raw, lo, hi);
            if let Some(signal) = &shared.signal {
                signal.set();
            }
        }

        /// Pure: one HID element value into the wheel's state. Steering is X
        /// mapped to -1..1 over its logical range (a 900° wheel reports 0..65535
        /// on Logitech, 16 bit either way). Pedals are the next three
        /// generic-desktop axes in usage order; each becomes 0..1 pressed with
        /// its resting end learned from the first sample.
        fn apply_wheel_value(wheel: &mut AppleRawHidWheel, page: u32, usage: u32, raw: i64, lo: i64, hi: i64) {
            let span = (hi - lo).max(1) as f32;
            let unit = ((raw - lo) as f32 / span).clamp(0.0, 1.0);
            if wheel.kind == AppleRawHidKind::Joystick {
                return Self::apply_stick_value(wheel, page, usage, raw, lo, hi, unit);
            }
            match page {
                HID_PAGE_GENERIC_DESKTOP => match usage {
                    HID_USAGE_X => wheel.state.steering = unit * 2.0 - 1.0,
                    HID_USAGE_Y | HID_USAGE_Z | HID_USAGE_RZ | HID_USAGE_RX | HID_USAGE_RY | HID_USAGE_SLIDER => {
                        let slot = match usage {
                            HID_USAGE_Y => 0,
                            HID_USAGE_Z => 1,
                            HID_USAGE_RZ => 2,
                            // Wheels that put pedals on Rx/Ry/Slider (Thrustmaster)
                            // still land throttle, brake, clutch in usage order.
                            HID_USAGE_RX => 0,
                            HID_USAGE_RY => 1,
                            _ => 2,
                        };
                        let rest_high = *wheel.pedal_rest[slot].get_or_insert(unit > 0.5);
                        let pressed = if rest_high { 1.0 - unit } else { unit };
                        match slot {
                            0 => wheel.state.throttle = pressed,
                            1 => wheel.state.brake = pressed,
                            _ => wheel.state.clutch = pressed,
                        }
                    }
                    HID_USAGE_HAT => {
                        // A hat is a direction 0..7 clockwise from up, `hi + 1`
                        // (usually 8) when released; stash it in the top nibble
                        // of the button mask so the app can read a d-pad.
                        let dir = if raw < lo || raw > hi { 0xf } else { (raw - lo) as u32 & 0xf };
                        wheel.state.buttons = (wheel.state.buttons & 0x0fff_ffff) | (dir << 28);
                    }
                    _ => {}
                },
                HID_USAGE_PAGE_BUTTON => {
                    if (1..=28).contains(&usage) {
                        let bit = 1u32 << (usage - 1);
                        if raw != 0 {
                            wheel.state.buttons |= bit;
                        } else {
                            wheel.state.buttons &= !bit;
                        }
                    }
                }
                _ => {}
            }
        }

        /// A flight stick by usage: X/Y the stick, Rz the twist, Slider (or Z
        /// when there is no slider) the throttle lever, the hat, the buttons.
        fn apply_stick_value(wheel: &mut AppleRawHidWheel, page: u32, usage: u32, raw: i64, lo: i64, hi: i64, unit: f32) {
            let signed = unit * 2.0 - 1.0;
            match page {
                HID_PAGE_GENERIC_DESKTOP => match usage {
                    HID_USAGE_X => wheel.stick.x = signed,
                    HID_USAGE_Y => wheel.stick.y = signed,
                    HID_USAGE_RZ => wheel.stick.twist = signed,
                    // A throttle lever rests at its high end on most sticks
                    // (pulled back = max); report it 0..1 pushed forward.
                    HID_USAGE_SLIDER => wheel.stick.throttle = 1.0 - unit,
                    HID_USAGE_Z => {
                        if wheel.stick.throttle == 0.0 || wheel.pedal_rest[0].is_none() {
                            wheel.pedal_rest[0] = Some(true);
                            wheel.stick.throttle = 1.0 - unit;
                        }
                    }
                    HID_USAGE_HAT => {
                        wheel.stick.hat = if raw < lo || raw > hi { 0xf } else { ((raw - lo) as u32 & 0xf) as u8 };
                    }
                    _ => {}
                },
                HID_USAGE_PAGE_BUTTON => {
                    if (1..=32).contains(&usage) {
                        let bit = 1u32 << (usage - 1);
                        if raw != 0 {
                            wheel.stick.buttons |= bit;
                        } else {
                            wheel.stick.buttons &= !bit;
                        }
                    }
                }
                _ => {}
            }
        }

            unsafe fn handle_report(
            context: *mut std::ffi::c_void,
            sender: *mut std::ffi::c_void,
            report_id: u32,
            report: &[u8],
        ) {
            let callback_context = &*(context as *const AppleRawHidCallbackContext);
            let device = sender as IOHIDDeviceRef;
            let mut shared = match callback_context.shared.lock() {
                Ok(shared) => shared,
                Err(_) => return,
            };
            let Some(entry) = shared
                .devices
                .iter_mut()
                .find(|entry| entry.device == device)
            else {
                return;
            };

            let report_kind = report.first().copied().unwrap_or(report_id as u8);
            match report_kind {
                XBOX_ONE_REPORT_BUTTONS => Self::parse_xbox_one_buttons(&mut entry.state, report),
                XBOX_ONE_REPORT_HOME => Self::parse_xbox_one_home(&mut entry.state, report),
                _ => {}
            }
            if let Some(signal) = &shared.signal {
                signal.set();
            }
        }

        fn parse_xbox_one_buttons(state: &mut GamepadState, report: &[u8]) {
            if report.len() < 18 {
                return;
            }
            state.start = Self::button(report[4] & 0x04);
            state.select = Self::button(report[4] & 0x08);
            state.a = Self::button(report[4] & 0x10);
            state.b = Self::button(report[4] & 0x20);
            state.x = Self::button(report[4] & 0x40);
            state.y = Self::button(report[4] & 0x80);

            state.dpad_up = Self::button(report[5] & 0x01);
            state.dpad_down = Self::button(report[5] & 0x02);
            state.dpad_left = Self::button(report[5] & 0x04);
            state.dpad_right = Self::button(report[5] & 0x08);

            state.left_shoulder = Self::button(report[5] & 0x10);
            state.right_shoulder = Self::button(report[5] & 0x20);
            state.left_thumb = Self::button(report[5] & 0x40);
            state.right_thumb = Self::button(report[5] & 0x80);

            state.left_trigger = Self::normalize_trigger(u16::from_le_bytes([report[6], report[7]]));
            state.right_trigger = Self::normalize_trigger(u16::from_le_bytes([report[8], report[9]]));

            state.left_stick = [Self::normalize_stick(i16::from_le_bytes([report[10], report[11]])), Self::normalize_stick(!i16::from_le_bytes([report[12], report[13]]))];
            state.right_stick = [Self::normalize_stick(i16::from_le_bytes([report[14], report[15]])), Self::normalize_stick(!i16::from_le_bytes([report[16], report[17]]))];
        }

        fn parse_xbox_one_home(state: &mut GamepadState, report: &[u8]) {
            if report.len() >= 5 {
                state.home = Self::button(report[4] & 0x01);
            }
        }

        fn button(flag: u8) -> f32 {
            if flag != 0 {
                1.0
            } else {
                0.0
            }
        }

        fn normalize_trigger(raw: u16) -> f32 {
            (raw as f32 / XBOX_ONE_TRIGGER_MAX).clamp(0.0, 1.0)
        }

        fn normalize_stick(raw: i16) -> f32 {
            (raw as f32 / 32768.0).clamp(-1.0, 1.0)
        }

        unsafe fn device_string_property(device: IOHIDDeviceRef, key: &str) -> String {
            let key_ref = Self::cf_string(key);
            let value = IOHIDDeviceGetProperty(device, key_ref);
            if value.is_null() {
                return String::new();
            }
            cfstring_ref_to_string(value as CFStringRef)
        }

        unsafe fn device_u32_property(device: IOHIDDeviceRef, key: &str) -> u32 {
            let key_ref = Self::cf_string(key);
            let value = IOHIDDeviceGetProperty(device, key_ref);
            if value.is_null() {
                return 0;
            }
            let mut out = 0i32;
            if CFNumberGetValue(
                value as CFNumberRef,
                kCFNumberSInt32Type,
                &mut out as *mut _ as *mut _,
            ) == 0
            {
                return 0;
            }
            out.max(0) as u32
        }

        unsafe fn cf_string(value: &str) -> CFStringRef {
            CFStringCreateWithBytes(
                ptr::null(),
                value.as_ptr(),
                value.len() as isize,
                kCFStringEncodingUTF8,
                0,
            )
        }
    }
    impl Drop for AppleRawHidInput {
        fn drop(&mut self) {
            self.stop();
        }
    }
    #[derive(Default)]
    struct AppleRawHidShared {
        devices: Vec<AppleRawHidDevice>,
        wheels: Vec<AppleRawHidWheel>,
        /// rings the session when a device reports a value
        signal: Option<Signal>,
    }
    struct AppleRawHidWheel {
        device: IOHIDDeviceRef,
        info: GameInfo,
        kind: AppleRawHidKind,
        state: WheelState,
        stick: JoystickState,
        /// Per pedal (throttle, brake, clutch): does it rest at the HIGH end of
        /// its range? Learned from the first sample, which arrives untouched.
        pedal_rest: [Option<bool>; 3],
    }
    unsafe impl Send for AppleRawHidWheel {}
    struct AppleRawHidDevice {
        device: IOHIDDeviceRef,
        info: GameInfo,
        state: GamepadState,
        report_buffer: Box<[u8]>,
    }
    unsafe impl Send for AppleRawHidDevice {}
    struct AppleRawHidCallbackContext {
        shared: Arc<Mutex<AppleRawHidShared>>,
    }
    unsafe extern "C" fn raw_hid_device_matching_callback(
        context: *mut std::ffi::c_void,
        result: IOReturn,
        _sender: *mut std::ffi::c_void,
        device: IOHIDDeviceRef,
    ) {
        if result == 0 {
            AppleRawHidInput::register_device(context, device);
        }
    }
    unsafe extern "C" fn raw_hid_device_removal_callback(
        context: *mut std::ffi::c_void,
        result: IOReturn,
        _sender: *mut std::ffi::c_void,
        device: IOHIDDeviceRef,
    ) {
        if result == 0 {
            AppleRawHidInput::remove_device(context, device);
        }
    }
    unsafe extern "C" fn raw_hid_value_callback(
        context: *mut std::ffi::c_void,
        result: IOReturn,
        sender: *mut std::ffi::c_void,
        value: IOHIDValueRef,
    ) {
        if result != 0 || value.is_null() {
            return;
        }
        AppleRawHidInput::handle_value(context, sender, value);
    }
    unsafe extern "C" fn raw_hid_report_callback(
        context: *mut std::ffi::c_void,
        result: IOReturn,
        sender: *mut std::ffi::c_void,
        _report_type: IOHIDReportType,
        report_id: u32,
        report: *mut u8,
        report_length: isize,
    ) {
        if result != 0 || report.is_null() || report_length <= 0 {
            return;
        }
        let bytes = std::slice::from_raw_parts(report, report_length as usize);
        AppleRawHidInput::handle_report(context, sender, report_id, bytes);
    }
}
