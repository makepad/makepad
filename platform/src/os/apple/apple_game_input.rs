use crate::{
    cx::Cx,
    event::game_input::*,
    game_input::CxGameInputApi,
    makepad_live_id::*,
    makepad_math::Vec2,
    makepad_objc_sys::{
        class, msg_send, objc_block,
        runtime::{nil, Class, ObjcId, Sel, BOOL, YES},
        sel, sel_impl,
    },
    os::apple::apple_sys::*,
};

#[cfg(target_os = "macos")]
use crate::os::apple::apple_util::cfstring_ref_to_string;
#[cfg(target_os = "macos")]
use std::{
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

pub struct AppleGameInput {
    pub gamepads: Vec<GameInputInfo>,
    pub controllers: Vec<ObjcId>,
    pub states: Vec<GameInputState>,
    gc_gamepads: Vec<GameInputInfo>,
    gc_states: Vec<GameInputState>,
    #[cfg(target_os = "macos")]
    raw_hid: AppleRawHidInput,
}

impl AppleGameInput {
    pub fn new() -> Self {
        Self {
            gamepads: Vec::new(),
            controllers: Vec::new(),
            states: Vec::new(),
            gc_gamepads: Vec::new(),
            gc_states: Vec::new(),
            #[cfg(target_os = "macos")]
            raw_hid: AppleRawHidInput::new(),
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

    unsafe fn raw_connected_controllers(
        gc_controller_class: &Class,
    ) -> Vec<(ObjcId, GameInputInfo)> {
        let controllers: ObjcId = msg_send![gc_controller_class, controllers];
        let count: usize = msg_send![controllers, count];
        let mut result = Vec::with_capacity(count);
        for index in 0..count {
            let controller: ObjcId = msg_send![controllers, objectAtIndex: index];
            let name = Self::controller_name(controller);
            let ptr = controller as u64;
            let id = LiveId(ptr);
            result.push((controller, GameInputInfo { id, name }));
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

    pub fn init<F>(callback: F) -> Self
    where
        F: Fn(GameInputConnectedEvent) + 'static + Clone,
    {
        unsafe {
            let gc_controller_class = class!(GCController);
            let sel_monitor = Sel::register("setShouldMonitorBackgroundEvents:");
            if msg_send![gc_controller_class, respondsToSelector: sel_monitor] {
                let () = msg_send![gc_controller_class, setShouldMonitorBackgroundEvents: YES];
            }

            let center: ObjcId = msg_send![class!(NSNotificationCenter), defaultCenter];
            let callback_clone = callback.clone();

            let block = objc_block!(move |note: ObjcId| {
                let controller: ObjcId = msg_send![note, object];
                let vendor_name: ObjcId = msg_send![controller, vendorName];
                let name = nsstring_to_string(vendor_name);

                let ptr = controller as u64;
                let id = LiveId(ptr);

                let info = GameInputInfo { id, name };
                callback_clone(GameInputConnectedEvent::Connected(info));
            });

            let () = msg_send![center, addObserverForName: GCControllerDidConnectNotification object: nil queue: nil usingBlock: &block];

            let callback_clone = callback.clone();
            let block = objc_block!(move |note: ObjcId| {
                let controller: ObjcId = msg_send![note, object];
                let vendor_name: ObjcId = msg_send![controller, vendorName];
                let name = nsstring_to_string(vendor_name);

                let ptr = controller as u64;
                let id = LiveId(ptr);

                let info = GameInputInfo { id, name };
                callback_clone(GameInputConnectedEvent::Disconnected(info));
            });
            let () = msg_send![center, addObserverForName: GCControllerDidDisconnectNotification object: nil queue: nil usingBlock: &block];

            let raw_controllers = Self::raw_connected_controllers(gc_controller_class);
            let discovery_sel =
                Sel::register("startWirelessControllerDiscoveryWithCompletionHandler:");
            if raw_controllers.is_empty()
                && msg_send![gc_controller_class, respondsToSelector: discovery_sel]
            {
                let block = objc_block!(move || {});
                let () = msg_send![
                    gc_controller_class,
                    startWirelessControllerDiscoveryWithCompletionHandler: &block
                ];
            }
            for (_, info) in raw_controllers {
                callback(GameInputConnectedEvent::Connected(info));
            }
        }

        Self::new()
    }

    pub fn on_connected(&mut self, info: &GameInputInfo) {
        if self.gc_gamepads.iter().any(|gamepad| gamepad.id == info.id) {
            return;
        }
        let ptr = info.id.0 as ObjcId;
        unsafe {
            let _: ObjcId = msg_send![ptr, retain];
        }
        self.gc_gamepads.push(info.clone());
        self.controllers.push(ptr);
        self.gc_states
            .push(GameInputState::Gamepad(GamepadState::default()));
    }

    pub fn on_disconnected(&mut self, info: &GameInputInfo) {
        if let Some(index) = self.gc_gamepads.iter().position(|g| g.id == info.id) {
            let ptr = self.controllers[index];
            self.gc_gamepads.remove(index);
            self.controllers.remove(index);
            self.gc_states.remove(index);
            unsafe {
                let _: () = msg_send![ptr, release];
            }
        }
    }

    fn refresh_combined_states(&mut self) {
        self.gamepads.clear();
        self.gamepads.extend(self.gc_gamepads.iter().cloned());

        self.states.clear();
        self.states.extend(self.gc_states.iter().cloned());

        #[cfg(target_os = "macos")]
        for (info, state) in self.raw_hid.snapshot() {
            if self.gamepads.iter().any(|gamepad| gamepad.id == info.id) {
                continue;
            }
            self.gamepads.push(info);
            self.states.push(state);
        }
    }

    pub fn poll(&mut self) {
        self.sync_connected_controllers();
        for (i, controller) in self.controllers.iter().enumerate() {
            unsafe {
                let extended_gamepad: ObjcId = msg_send![*controller, extendedGamepad];
                if extended_gamepad != nil {
                    if let GameInputState::Gamepad(state) = &mut self.gc_states[i] {
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
                            state.left_stick = Vec2 {
                                x: get_axis(msg_send![left_stick, xAxis]),
                                y: get_axis(msg_send![left_stick, yAxis]),
                            };
                        }

                        let right_stick: ObjcId = msg_send![extended_gamepad, rightThumbstick];
                        if right_stick != nil {
                            state.right_stick = Vec2 {
                                x: get_axis(msg_send![right_stick, xAxis]),
                                y: get_axis(msg_send![right_stick, yAxis]),
                            };
                        }
                    }
                }
            }
        }
        self.refresh_combined_states();
    }
}

#[cfg(target_os = "macos")]
const APPLE_RAW_HID_XBOX_VENDOR_ID: u32 = 0x045e;
/// Racing-wheel vendors the raw HID path claims: their reports are parsed
/// by HID usage (steering = X, pedals = the next axes, buttons = the button
/// page) and the device is handed out for output reports — the way force
/// feedback is driven, which the Game Controller framework does not
/// expose. Logitech, Thrustmaster, Fanatec, Moza, Simucube, Asetek. A
/// device from one of these vendors that is not a known wheel (a Logitech
/// Extreme 3D, a Thrustmaster HOTAS) is a JOYSTICK, as is any other
/// joystick-class device (primary usage Joystick or Multi-axis).
#[cfg(target_os = "macos")]
const APPLE_RAW_HID_WHEEL_VENDOR_IDS: [u32; 6] = [0x046d, 0x044f, 0x0eb7, 0x346e, 0x16d0, 0x2433];

/// Is this vendor/product a racing wheel? Fanatec, Moza, Simucube and
/// Asetek make nothing else; Logitech and Thrustmaster need their wheel
/// product tables (the same ids the FFB protocols key on).
#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AppleRawHidKind {
    Wheel,
    Joystick,
}
#[cfg(target_os = "macos")]
const HID_USAGE_GENERIC_DESKTOP_MULTI_AXIS: i32 = 0x08;
#[cfg(target_os = "macos")]
const HID_PAGE_GENERIC_DESKTOP: u32 = 0x01;
#[cfg(target_os = "macos")]
const HID_USAGE_PAGE_BUTTON: u32 = 0x09;
#[cfg(target_os = "macos")]
const HID_USAGE_X: u32 = 0x30;
#[cfg(target_os = "macos")]
const HID_USAGE_Y: u32 = 0x31;
#[cfg(target_os = "macos")]
const HID_USAGE_Z: u32 = 0x32;
#[cfg(target_os = "macos")]
const HID_USAGE_RX: u32 = 0x33;
#[cfg(target_os = "macos")]
const HID_USAGE_RY: u32 = 0x34;
#[cfg(target_os = "macos")]
const HID_USAGE_RZ: u32 = 0x35;
#[cfg(target_os = "macos")]
const HID_USAGE_SLIDER: u32 = 0x36;
#[cfg(target_os = "macos")]
const HID_USAGE_HAT: u32 = 0x39;
#[cfg(target_os = "macos")]
const HID_USAGE_PAGE_GENERIC_DESKTOP: i32 = 0x01;
#[cfg(target_os = "macos")]
const HID_USAGE_GENERIC_DESKTOP_JOYSTICK: i32 = 0x04;
#[cfg(target_os = "macos")]
const HID_USAGE_GENERIC_DESKTOP_GAMEPAD: i32 = 0x05;
#[cfg(target_os = "macos")]
const XBOX_ONE_REPORT_BUTTONS: u8 = 0x20;
#[cfg(target_os = "macos")]
const XBOX_ONE_REPORT_HOME: u8 = 0x07;
#[cfg(target_os = "macos")]
const XBOX_ONE_TRIGGER_MAX: f32 = 1023.0;

#[cfg(target_os = "macos")]
struct AppleRawHidInput {
    shared: Arc<Mutex<AppleRawHidShared>>,
    run_loop: Arc<Mutex<usize>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(target_os = "macos")]
impl AppleRawHidInput {
    fn new() -> Self {
        let shared = Arc::new(Mutex::new(AppleRawHidShared::default()));
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

    fn snapshot(&self) -> Vec<(GameInputInfo, GameInputState)> {
        if let Ok(shared) = self.shared.lock() {
            let mut out: Vec<(GameInputInfo, GameInputState)> = shared
                .devices
                .iter()
                .map(|device| {
                    (
                        device.info.clone(),
                        GameInputState::Gamepad(device.state.clone()),
                    )
                })
                .collect();
            out.extend(shared.wheels.iter().map(|wheel| {
                let state = match wheel.kind {
                    AppleRawHidKind::Wheel => GameInputState::Wheel(wheel.state.clone()),
                    AppleRawHidKind::Joystick => GameInputState::Joystick(wheel.stick.clone()),
                };
                (wheel.info.clone(), state)
            }));
            return out;
        }
        Vec::new()
    }

    /// An output-report handle for a raw-HID device (wheels only — a pad's
    /// rumble goes through the Game Controller framework, not here). The
    /// closure locks the shared table for the duration of one SetReport, so
    /// a device unplugged mid-write fails the write instead of dangling.
    fn output_handle(&self, id: LiveId) -> Option<GameInputOutput> {
        let shared = self.shared.lock().ok()?;
        let wheel = shared.wheels.iter().find(|w| w.info.id == id)?;
        let (vendor_id, product_id) = (wheel.vendor_id, wheel.product_id);
        drop(shared);
        let table = Arc::clone(&self.shared);
        let send = Arc::new(move |report_id: u8, data: &[u8]| -> bool {
            let Ok(shared) = table.lock() else { return false };
            let Some(wheel) = shared.wheels.iter().find(|w| w.info.id == id) else {
                return false;
            };
            unsafe {
                IOHIDDeviceSetReport(
                    wheel.device,
                    kIOHIDReportTypeOutput,
                    report_id as CFIndex,
                    data.as_ptr(),
                    data.len() as CFIndex,
                ) == 0
            }
        });
        Some(GameInputOutput::new(id, vendor_id, product_id, send))
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
        let info = GameInputInfo {
            id: LiveId(if location_id != 0 {
                ((vendor_id as u64) << 48) | ((product_id as u64) << 32) | location_id as u64
            } else {
                device as u64
            }),
            name: if name.is_empty() {
                format!("Xbox Controller {:04x}:{:04x}", vendor_id, product_id)
            } else {
                format!("{name} {:04x}:{:04x}", vendor_id, product_id)
            },
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
        let info = GameInputInfo {
            id: LiveId(if location_id != 0 {
                ((vendor_id as u64) << 48) | ((product_id as u64) << 32) | location_id as u64
            } else {
                device as u64
            }),
            name: if name.is_empty() {
                format!("{:?} {:04x}:{:04x}", kind, vendor_id, product_id)
            } else {
                format!("{name} {:04x}:{:04x}", vendor_id, product_id)
            },
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
            vendor_id,
            product_id,
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

    fn infos(&self) -> Vec<GameInputInfo> {
        self.snapshot().into_iter().map(|(info, _)| info).collect()
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

        state.left_stick = Vec2 {
            x: Self::normalize_stick(i16::from_le_bytes([report[10], report[11]])),
            y: Self::normalize_stick(!i16::from_le_bytes([report[12], report[13]])),
        };
        state.right_stick = Vec2 {
            x: Self::normalize_stick(i16::from_le_bytes([report[14], report[15]])),
            y: Self::normalize_stick(!i16::from_le_bytes([report[16], report[17]])),
        };
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

#[cfg(target_os = "macos")]
impl Drop for AppleRawHidInput {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct AppleRawHidShared {
    devices: Vec<AppleRawHidDevice>,
    wheels: Vec<AppleRawHidWheel>,
}

#[cfg(target_os = "macos")]
struct AppleRawHidWheel {
    device: IOHIDDeviceRef,
    info: GameInputInfo,
    vendor_id: u32,
    product_id: u32,
    kind: AppleRawHidKind,
    state: WheelState,
    stick: JoystickState,
    /// Per pedal (throttle, brake, clutch): does it rest at the HIGH end of
    /// its range? Learned from the first sample, which arrives untouched.
    pedal_rest: [Option<bool>; 3],
}

#[cfg(target_os = "macos")]
unsafe impl Send for AppleRawHidWheel {}

#[cfg(target_os = "macos")]
struct AppleRawHidDevice {
    device: IOHIDDeviceRef,
    info: GameInputInfo,
    state: GamepadState,
    report_buffer: Box<[u8]>,
}

#[cfg(target_os = "macos")]
unsafe impl Send for AppleRawHidDevice {}

#[cfg(target_os = "macos")]
struct AppleRawHidCallbackContext {
    shared: Arc<Mutex<AppleRawHidShared>>,
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

impl CxGameInputApi for Cx {
    fn game_input_state(&mut self, index: usize) -> Option<&GameInputState> {
        if self.in_makepad_studio {
            return self.game_input_remote.get(index);
        }
        if let Some(game_input) = &self.os.apple_game_input {
            if index < game_input.states.len() {
                return Some(&game_input.states[index]);
            }
        }
        None
    }

    fn game_input_states(&mut self) -> &[GameInputState] {
        // Hosted by Studio: this process has no window, so the OS never gave
        // it the controllers. Studio forwards them instead.
        if self.in_makepad_studio {
            return &self.game_input_remote;
        }
        if let Some(game_input) = &self.os.apple_game_input {
            return &game_input.states;
        }
        &[]
    }

    fn game_input_state_mut(&mut self, index: usize) -> Option<&mut GameInputState> {
        if self.in_makepad_studio {
            return self.game_input_remote.get_mut(index);
        }
        if let Some(game_input) = &mut self.os.apple_game_input {
            if index < game_input.states.len() {
                return Some(&mut game_input.states[index]);
            }
        }
        None
    }

    fn game_input_states_mut(&mut self) -> &mut [GameInputState] {
        if self.in_makepad_studio {
            return &mut self.game_input_remote;
        }
        if let Some(game_input) = &mut self.os.apple_game_input {
            return &mut game_input.states;
        }
        &mut []
    }

    fn game_input_infos(&mut self) -> Vec<GameInputInfo> {
        if let Some(game_input) = &self.os.apple_game_input {
            return game_input.gamepads.clone();
        }
        Vec::new()
    }

    fn game_input_output(&mut self, id: LiveId) -> Option<GameInputOutput> {
        #[cfg(target_os = "macos")]
        if let Some(game_input) = &self.os.apple_game_input {
            return game_input.raw_hid.output_handle(id);
        }
        None
    }
}
