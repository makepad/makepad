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
    os::apple::{apple_sys::*, apple_util::cfstring_ref_to_string},
};

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
            return shared
                .devices
                .iter()
                .map(|device| {
                    (
                        device.info.clone(),
                        GameInputState::Gamepad(device.state.clone()),
                    )
                })
                .collect();
        }
        Vec::new()
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
        let values = [gamepad as *const _, joystick as *const _];
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
}

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

#[cfg(not(target_os = "macos"))]
struct AppleRawHidInput;

#[cfg(not(target_os = "macos"))]
impl AppleRawHidInput {
    fn new() -> Self {
        Self
    }

    fn snapshot(&self) -> Vec<(GameInputInfo, GameInputState)> {
        Vec::new()
    }
}

impl CxGameInputApi for Cx {
    fn game_input_state(&mut self, index: usize) -> Option<&GameInputState> {
        if let Some(game_input) = &self.os.apple_game_input {
            if index < game_input.states.len() {
                return Some(&game_input.states[index]);
            }
        }
        None
    }

    fn game_input_states(&mut self) -> &[GameInputState] {
        if let Some(game_input) = &self.os.apple_game_input {
            return &game_input.states;
        }
        &[]
    }

    fn game_input_state_mut(&mut self, index: usize) -> Option<&mut GameInputState> {
        if let Some(game_input) = &mut self.os.apple_game_input {
            if index < game_input.states.len() {
                return Some(&mut game_input.states[index]);
            }
        }
        None
    }

    fn game_input_states_mut(&mut self) -> &mut [GameInputState] {
        if let Some(game_input) = &mut self.os.apple_game_input {
            return &mut game_input.states;
        }
        &mut []
    }
}
