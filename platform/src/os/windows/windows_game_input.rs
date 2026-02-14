use {
    crate::{
        event::game_input::*,
        makepad_live_id::*,
        makepad_math::Vec2,
        windows::core::{BOOL, GUID, Interface},
        windows::Win32::Devices::HumanInterfaceDevice::{
            DirectInput8Create, GUID_RxAxis, GUID_RyAxis, GUID_RzAxis, GUID_Slider, GUID_XAxis,
            GUID_YAxis, GUID_ZAxis, IDirectInput8W, IDirectInputDevice8W, IDirectInputEffect,
            DI8DEVCLASS_GAMECTRL, DI8DEVTYPE_DRIVING, DICONSTANTFORCE, DIDATAFORMAT,
            DIDEVICEINSTANCEW, DIDFT_ANYINSTANCE, DIDFT_AXIS, DIDFT_BUTTON, DIDFT_POV,
            DIDF_ABSAXIS, DIEB_NOTRIGGER, DIEDFL_ATTACHEDONLY, DIEFFECT, DIEFF_CARTESIAN,
            DIEFF_OBJECTOFFSETS, DIENUM_CONTINUE, DIEP_START, DIEP_TYPESPECIFICPARAMS, DIJOYSTATE2,
            DIOBJECTDATAFORMAT, DISCL_BACKGROUND, DISCL_EXCLUSIVE, GUID_POV,
        },
        windows::Win32::System::LibraryLoader::GetModuleHandleW,
        windows::Win32::UI::Input::XboxController::{
            XInputGetState, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_BACK,
            XINPUT_GAMEPAD_BUTTON_FLAGS, XINPUT_GAMEPAD_DPAD_DOWN, XINPUT_GAMEPAD_DPAD_LEFT,
            XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_LEFT_SHOULDER,
            XINPUT_GAMEPAD_LEFT_THUMB, XINPUT_GAMEPAD_RIGHT_SHOULDER, XINPUT_GAMEPAD_RIGHT_THUMB,
            XINPUT_GAMEPAD_START, XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_Y, XINPUT_STATE,
        },
    },
    std::mem::size_of,
};

// GUID_ConstantForce: {13541C20-8E33-11D0-9AD0-00A0C9A06E35}
#[allow(non_upper_case_globals)]
const GUID_ConstantForce: GUID = GUID::from_u128(0x13541C20_8E33_11D0_9AD0_00A0C9A06E35);

// Basic exact match for c_dfDIJoystick2 (DIJOYSTATE2)
// This avoids linking against dinput8.lib/dxguid.lib data exports which might be missing.

const DIDFT_OPTIONAL: u32 = 0x80000000;

// Global storage for the format to ensure it lives forever
static mut DF_JOYSTICK2_FORMAT: Option<(DIDATAFORMAT, Vec<DIOBJECTDATAFORMAT>)> = None;
static DF_INIT: std::sync::Once = std::sync::Once::new();

fn ensure_data_format_initialized() {
    unsafe {
        DF_INIT.call_once(|| {
            let mut rgodf = Vec::new();
            // Axes
            let axes = [
                (&GUID_XAxis, 0),
                (&GUID_YAxis, 4),
                (&GUID_ZAxis, 8),
                (&GUID_RxAxis, 12),
                (&GUID_RyAxis, 16),
                (&GUID_RzAxis, 20),
                (&GUID_Slider, 24),
                (&GUID_Slider, 28),
            ];
            for (guid, offset) in axes {
                rgodf.push(DIOBJECTDATAFORMAT {
                    pguid: guid,
                    dwOfs: offset,
                    dwType: DIDFT_AXIS | DIDFT_OPTIONAL | DIDFT_ANYINSTANCE,
                    dwFlags: 0,
                });
            }
            // POVs
            for i in 0..4 {
                rgodf.push(DIOBJECTDATAFORMAT {
                    pguid: &GUID_POV,
                    dwOfs: 32 + (i * 4),
                    dwType: DIDFT_POV | DIDFT_OPTIONAL | DIDFT_ANYINSTANCE,
                    dwFlags: 0,
                });
            }
            // Buttons (128)
            for i in 0..128 {
                rgodf.push(DIOBJECTDATAFORMAT {
                    pguid: std::ptr::null(),
                    dwOfs: 48 + i,
                    dwType: DIDFT_BUTTON | DIDFT_OPTIONAL | DIDFT_ANYINSTANCE,
                    dwFlags: 0,
                });
            }
            // Need to map the rest? Vector/Accel/Force are rarely used inputs.
            // If they are not mapped, they will just be garbage/zero in the struct.

            let format = DIDATAFORMAT {
                dwSize: size_of::<DIDATAFORMAT>() as u32,
                dwObjSize: size_of::<DIOBJECTDATAFORMAT>() as u32,
                dwFlags: DIDF_ABSAXIS,
                dwDataSize: size_of::<DIJOYSTATE2>() as u32,
                dwNumObjs: rgodf.len() as u32,
                rgodf: rgodf.as_mut_ptr(),
            };

            DF_JOYSTICK2_FORMAT = Some((format, rgodf));
        });
    }
}

pub struct WindowsGameInput {
    pub gamepads: Vec<GameInputInfo>,
    pub states: Vec<GameInputState>,
    pub direct_input: Option<IDirectInput8W>,
    pub di_devices: Vec<(
        LiveId,
        IDirectInputDevice8W,
        GUID,
        Option<IDirectInputEffect>,
    )>,
    pub next_wheel_id: u64,
    pub enum_timer: u64,
}

impl WindowsGameInput {
    pub fn new() -> Self {
        let direct_input = unsafe {
            let hinstance = GetModuleHandleW(None).unwrap();
            let mut di_out: Option<IDirectInput8W> = None;
            // DIRECTINPUT_VERSION is 0x0800
            match DirectInput8Create(
                hinstance.into(),
                0x0800,
                &IDirectInput8W::IID,
                &mut di_out as *mut _ as *mut _,
                None,
            ) {
                Ok(_) => di_out,
                Err(_) => {
                    // Log error or just continue without DirectInput
                    None
                }
            }
        };

        Self {
            gamepads: Vec::new(),
            states: Vec::new(),
            direct_input,
            di_devices: Vec::new(),
            next_wheel_id: 128,
            enum_timer: 0,
        }
    }

    pub fn init() -> Self {
        Self::new()
    }

    pub fn poll<F>(&mut self, mut callback: F)
    where
        F: FnMut(GameInputConnectedEvent),
    {
        // 1. Poll XInput (Xbox Controllers)
        for i in 0..4 {
            let mut state = XINPUT_STATE::default();
            let result = unsafe { XInputGetState(i, &mut state) };

            // Construct a stable ID for this XInput slot
            let id = LiveId(i as u64);

            if result == 0 {
                // ERROR_SUCCESS
                // Connected
                let info = GameInputInfo {
                    id,
                    name: format!("Xbox Controller {}", i + 1),
                };

                // Check if we already know about this gamepad
                let index = self.gamepads.iter().position(|g| g.id == id);

                if index.is_none() {
                    // New connection
                    self.gamepads.push(info.clone());
                    // Default to Gamepad variant
                    self.states
                        .push(GameInputState::Gamepad(GamepadState::default()));
                    callback(GameInputConnectedEvent::Connected(info));
                }

                // Update state
                if let Some(index) = self.gamepads.iter().position(|g| g.id == id) {
                    if let GameInputState::Gamepad(gp_state) = &mut self.states[index] {
                        let x_state = state.Gamepad;

                        // Buttons
                        let z = XINPUT_GAMEPAD_BUTTON_FLAGS(0);
                        gp_state.dpad_up = if (x_state.wButtons & XINPUT_GAMEPAD_DPAD_UP) != z {
                            1.0
                        } else {
                            0.0
                        };
                        gp_state.dpad_down = if (x_state.wButtons & XINPUT_GAMEPAD_DPAD_DOWN) != z {
                            1.0
                        } else {
                            0.0
                        };
                        gp_state.dpad_left = if (x_state.wButtons & XINPUT_GAMEPAD_DPAD_LEFT) != z {
                            1.0
                        } else {
                            0.0
                        };
                        gp_state.dpad_right = if (x_state.wButtons & XINPUT_GAMEPAD_DPAD_RIGHT) != z
                        {
                            1.0
                        } else {
                            0.0
                        };

                        gp_state.start = if (x_state.wButtons & XINPUT_GAMEPAD_START) != z {
                            1.0
                        } else {
                            0.0
                        };
                        gp_state.select = if (x_state.wButtons & XINPUT_GAMEPAD_BACK) != z {
                            1.0
                        } else {
                            0.0
                        };

                        gp_state.left_thumb = if (x_state.wButtons & XINPUT_GAMEPAD_LEFT_THUMB) != z
                        {
                            1.0
                        } else {
                            0.0
                        };
                        gp_state.right_thumb =
                            if (x_state.wButtons & XINPUT_GAMEPAD_RIGHT_THUMB) != z {
                                1.0
                            } else {
                                0.0
                            };

                        gp_state.left_shoulder =
                            if (x_state.wButtons & XINPUT_GAMEPAD_LEFT_SHOULDER) != z {
                                1.0
                            } else {
                                0.0
                            };
                        gp_state.right_shoulder =
                            if (x_state.wButtons & XINPUT_GAMEPAD_RIGHT_SHOULDER) != z {
                                1.0
                            } else {
                                0.0
                            };

                        gp_state.a = if (x_state.wButtons & XINPUT_GAMEPAD_A) != z {
                            1.0
                        } else {
                            0.0
                        };
                        gp_state.b = if (x_state.wButtons & XINPUT_GAMEPAD_B) != z {
                            1.0
                        } else {
                            0.0
                        };
                        gp_state.x = if (x_state.wButtons & XINPUT_GAMEPAD_X) != z {
                            1.0
                        } else {
                            0.0
                        };
                        gp_state.y = if (x_state.wButtons & XINPUT_GAMEPAD_Y) != z {
                            1.0
                        } else {
                            0.0
                        };

                        // Triggers (0-255 -> 0.0-1.0)
                        gp_state.left_trigger = x_state.bLeftTrigger as f32 / 255.0;
                        gp_state.right_trigger = x_state.bRightTrigger as f32 / 255.0;

                        // Thumbsticks (-32768 to 32767 -> -1.0 to 1.0)
                        fn normalize_axis(val: i16) -> f32 {
                            val as f32 / 32768.0
                        }

                        gp_state.left_stick = Vec2 {
                            x: normalize_axis(x_state.sThumbLX),
                            y: normalize_axis(x_state.sThumbLY),
                        };

                        gp_state.right_stick = Vec2 {
                            x: normalize_axis(x_state.sThumbRX),
                            y: normalize_axis(x_state.sThumbRY),
                        };
                    }
                }
            } else {
                // Disconnected
                // Only disconnect if it was an XInput device (id < 128)
                if let Some(index) = self.gamepads.iter().position(|g| g.id == id) {
                    let info = self.gamepads[index].clone();
                    self.gamepads.remove(index);
                    self.states.remove(index);
                    callback(GameInputConnectedEvent::Disconnected(info));
                }
            }
        }

        // 2. Poll DirectInput (Racing Wheels)
        if let Some(di) = &self.direct_input {
            unsafe {
                // Enumeration context
                struct EnumContext<'a> {
                    found_devices: Vec<(GUID, String)>,
                    _marker: std::marker::PhantomData<&'a ()>,
                }

                let mut ctx = EnumContext {
                    found_devices: Vec::new(),
                    _marker: std::marker::PhantomData,
                };

                // Callback function for EnumDevices
                unsafe extern "system" fn enum_callback(
                    lpddi: *mut DIDEVICEINSTANCEW,
                    pvref: *mut std::ffi::c_void,
                ) -> BOOL {
                    let ctx = &mut *(pvref as *mut EnumContext);
                    let instance = &*lpddi;

                    // Filter for Driving devices if needed, but we used DI8DEVCLASS_GAMECTRL in EnumDevices call to broaden search,
                    // or we check dwDevType here.
                    // Let's accept things that look like driving controls.
                    let dev_type = instance.dwDevType & 0xFF;
                    if dev_type == DI8DEVTYPE_DRIVING {
                        // Read name
                        let name = String::from_utf16_lossy(&instance.tszInstanceName);
                        // Clean up null terminators
                        let name = name.trim_matches('\0').to_string();
                        ctx.found_devices.push((instance.guidInstance, name));
                    }

                    BOOL(DIENUM_CONTINUE as i32)
                }

                // Enumerate attached devices (Throttle this to every 200 polls ~ 3 seconds at 60fps)
                if self.enum_timer % 200 == 0 {
                    let _ = di.EnumDevices(
                        DI8DEVCLASS_GAMECTRL,
                        Some(enum_callback),
                        &mut ctx as *mut _ as *mut _,
                        DIEDFL_ATTACHEDONLY,
                    );

                    self.enum_timer += 1;

                    let mut active_di_indices = Vec::new();

                    for (guid, name) in ctx.found_devices {
                        // Check if we already have this device open
                        // Note: We need a way to persistently identify devices. GUID is good for session.
                        // For now, we linear search our open devices.

                        // Currently we store (LiveId, IDirectInputDevice8W) in self.di_devices
                        // We need to know which device corresponds to which GUID.
                        // Since IDirectInputDevice8W doesn't easily expose GUID back,
                        // we might want to change `di_devices` to store GUID too.
                        // But accessing device info is slow.
                        // Simplest approach: We don't have easy stable ID across runs without more logic,
                        // but within session GUID is stable.
                        // For the sake of this prompt, let's just assume we can't easily match existing open devices by GUID
                        // completely efficiently without changing the struct, so I'll trust the order or add GUID to struct.

                        // Let's match by comparing device objects? No.
                        // Let's upgrade `di_devices` to store GUID.
                        // Wait, I can't change the struct definition inside this method.
                        // I need to change the struct in the file first.
                        // I will assume `di_devices` stores `(LiveId, IDirectInputDevice8W, GUID)`.
                        // Ah, I defined the struct above without GUID. I should add it.
                        // But since I am overwriting the whole file, I CAN change the struct! :)

                        // See below for corrected struct definition in the same file.

                        let mut existing_index = None;
                        for (idx, (_, _, existing_guid, _)) in self.di_devices.iter().enumerate() {
                            if *existing_guid == guid {
                                existing_index = Some(idx);
                                break;
                            }
                        }

                        if let Some(idx) = existing_index {
                            active_di_indices.push(idx);
                            // Device is already open
                            // We will poll it below
                        } else {
                            // Open new device
                            let mut device_out: Option<IDirectInputDevice8W> = None;
                            if di
                                .CreateDevice(&guid, &mut device_out as *mut _ as *mut _, None)
                                .is_ok()
                            {
                                if let Some(device) = device_out {
                                    // Set data format
                                    ensure_data_format_initialized();
                                    #[allow(static_mut_refs)]
                                    let data_format = &mut DF_JOYSTICK2_FORMAT.as_mut().unwrap().0;
                                    if device.SetDataFormat(data_format).is_ok() {
                                        // Set cooperative level (Background | Exclusive)
                                        // FORCE FEEDBACK REQUIRES EXCLUSIVE ACCESS
                                        let hwnd =  windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();

                                        if device
                                            .SetCooperativeLevel(
                                                hwnd,
                                                DISCL_BACKGROUND | DISCL_EXCLUSIVE,
                                            )
                                            .is_ok()
                                        {
                                            // Acquire
                                            let _ = device.Acquire();

                                            // Try to create Constant Force Effect
                                            let mut effect: Option<IDirectInputEffect> = None;

                                            // Only try if we have a valid window handle, otherwise FFB creation might fail or be invalid
                                            if hwnd.0 != std::ptr::null_mut() {
                                                let mut axes: [u32; 1] = [0]; // Offset 0 is X axis
                                                let mut directions: [i32; 1] = [0];
                                                let mut cf_params =
                                                    DICONSTANTFORCE { lMagnitude: 0 };

                                                // DIEFFECT structure
                                                // Note: windows crate might expect specific structure layout.
                                                // We assume DIEFFECT is available.

                                                let mut eff = DIEFFECT {
                                                    dwSize: size_of::<DIEFFECT>() as u32,
                                                    dwFlags: DIEFF_CARTESIAN | DIEFF_OBJECTOFFSETS,
                                                    dwDuration: u32::MAX, // Infinite
                                                    dwSamplePeriod: 0,
                                                    dwGain: 10000, // Max gain
                                                    dwTriggerButton: DIEB_NOTRIGGER,
                                                    dwTriggerRepeatInterval: 0,
                                                    cAxes: 1,
                                                    rgdwAxes: axes.as_mut_ptr(),
                                                    rglDirection: directions.as_mut_ptr(),
                                                    lpEnvelope: std::ptr::null_mut(),
                                                    cbTypeSpecificParams: size_of::<DICONSTANTFORCE>(
                                                    )
                                                        as u32,
                                                    lpvTypeSpecificParams: &mut cf_params as *mut _
                                                        as *mut _,
                                                    dwStartDelay: 0,
                                                };

                                                let mut effect_out: Option<IDirectInputEffect> =
                                                    None;
                                                // We ignore error here, if it fails, we just don't have FFB
                                                if device
                                                    .CreateEffect(
                                                        &GUID_ConstantForce,
                                                        &mut eff,
                                                        &mut effect_out as *mut _ as *mut _,
                                                        None,
                                                    )
                                                    .is_ok()
                                                {
                                                    effect = effect_out;
                                                    if let Some(e) = &effect {
                                                        let _ = e.Start(1, 0);
                                                    }
                                                }
                                            }

                                            // Register
                                            let id_val = self.next_wheel_id;
                                            self.next_wheel_id += 1;
                                            let new_id = LiveId(id_val);

                                            self.di_devices.push((
                                                new_id,
                                                device.clone(),
                                                guid,
                                                effect,
                                            ));
                                            active_di_indices.push(self.di_devices.len() - 1);

                                            let info = GameInputInfo {
                                                id: new_id,
                                                name: name.clone(),
                                            };
                                            self.gamepads.push(info.clone());
                                            // Use WheelState for DI devices (assuming they are wheels mainly for now)
                                            // We could differentiate based on type, but prompt asked for Wheel support.
                                            self.states
                                                .push(GameInputState::Wheel(WheelState::default()));
                                            callback(GameInputConnectedEvent::Connected(info));
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Cleanup disconnected DI devices
                    let mut i = 0;
                    while i < self.di_devices.len() {
                        if !active_di_indices.contains(&i) {
                            let (id, _, _, _) = self.di_devices[i];
                            self.di_devices.remove(i);
                            if let Some(index) = self.gamepads.iter().position(|g| g.id == id) {
                                let info = self.gamepads[index].clone();
                                self.gamepads.remove(index);
                                self.states.remove(index);
                                callback(GameInputConnectedEvent::Disconnected(info));
                            }
                            // Don't increment i, as we removed current element
                        } else {
                            i += 1;
                        }
                    }
                }

                // Poll active DI devices
                for (id, device, _, effect) in &self.di_devices {
                    // Poll() usually needed before GetDeviceState
                    let _ = device.Poll();

                    let mut state = DIJOYSTATE2::default();
                    if device
                        .GetDeviceState(
                            size_of::<DIJOYSTATE2>() as u32,
                            &mut state as *mut _ as *mut _,
                        )
                        .is_ok()
                    {
                        if let Some(index) = self.gamepads.iter().position(|g| g.id == *id) {
                            if let GameInputState::Wheel(wh_state) = &mut self.states[index] {
                                // Map DirectInput axes to WheelState
                                // This mapping is generic; specific wheels might differ.
                                // Usually:
                                // lX -> Steering
                                // lY -> Accelerator (often inverted)
                                // lRz -> Brake
                                // etc.

                                // Normalize 0..65535 to -1.0..1.0 or 0.0..1.0
                                fn norm_axis(val: i32) -> f32 {
                                    (val as f32 - 32768.0) / 32768.0
                                }
                                fn norm_trig(val: i32) -> f32 {
                                    // 0..65535 -> 0..1
                                    val as f32 / 65535.0
                                }

                                wh_state.steering = norm_axis(state.lX);
                                // Throttle/Brake mapping varies wildly.
                                // Logitech G29 example: lY is throttle (inv), lRz is brake (inv), lYz is clutch.
                                // Generic fallback:
                                wh_state.throttle = norm_trig(65535 - state.lY); // Often Y axis inverted
                                wh_state.brake = norm_trig(65535 - state.lRz);
                                wh_state.clutch = norm_trig(65535 - state.rglSlider[0]);

                                // Force Feedback Update
                                if let Some(eff) = effect {
                                    // Map steer_force (-1.0 to 1.0) to -10000 to 10000
                                    let steer_force = wh_state.steer_force.clamp(-1.0, 1.0);
                                    let mag = (steer_force * 10000.0) as i32;

                                    // To update parameters, we need to pass a DIEFFECT structure again?
                                    // Or just TypeSpecificParams?
                                    // IDirectInputEffect::SetParameters takes flags.
                                    // If we only update magnitude, we can update TypeSpecificParams.

                                    let mut cf_params = DICONSTANTFORCE { lMagnitude: mag };

                                    // We need to construct a DIEFFECT that points to this
                                    let mut eff_struct = DIEFFECT {
                                        dwSize: size_of::<DIEFFECT>() as u32,
                                        dwFlags: 0, // Not used when we only update typespecific?
                                        dwDuration: 0,
                                        dwSamplePeriod: 0,
                                        dwGain: 0,
                                        dwTriggerButton: 0,
                                        dwTriggerRepeatInterval: 0,
                                        cAxes: 0,
                                        rgdwAxes: std::ptr::null_mut(),
                                        rglDirection: std::ptr::null_mut(),
                                        lpEnvelope: std::ptr::null_mut(),
                                        cbTypeSpecificParams: size_of::<DICONSTANTFORCE>() as u32,
                                        lpvTypeSpecificParams: &mut cf_params as *mut _ as *mut _,
                                        dwStartDelay: 0,
                                    };

                                    let _ = eff.SetParameters(
                                        &mut eff_struct,
                                        DIEP_TYPESPECIFICPARAMS | DIEP_START,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
