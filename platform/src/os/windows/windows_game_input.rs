use {
    crate::{
        event::game_input::*,
        makepad_live_id::*,
        makepad_math::Vec2,
        windows::core::{BOOL, GUID},
        windows::Win32::Devices::HumanInterfaceDevice::{
            DirectInput8Create, GUID_RxAxis, GUID_RyAxis, GUID_RzAxis, GUID_Slider, GUID_XAxis,
            GUID_YAxis, GUID_ZAxis, IDirectInput8W, IDirectInputDevice8W, IDirectInputEffect,
            DI8DEVCLASS_GAMECTRL, DI8DEVTYPE_DRIVING, DICONSTANTFORCE, DIDATAFORMAT,
            DIDEVICEINSTANCEW, DIDFT_ANYINSTANCE, DIDFT_AXIS, DIDFT_BUTTON, DIDFT_POV,
            DIDF_ABSAXIS, DIEB_NOTRIGGER, DIEDFL_ATTACHEDONLY, DIEFFECT, DIEFF_CARTESIAN,
            DIEFF_OBJECTOFFSETS, DIENUM_CONTINUE, DIEP_TYPESPECIFICPARAMS, DIJOYSTATE2,
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
    std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};

// GUID_ConstantForce / GUID_Spring / GUID_Damper: {13541C2x-8E33-11D0-9AD0-00A0C9A06E35}
#[allow(non_upper_case_globals)]
const GUID_ConstantForce: GUID = GUID::from_u128(0x13541C20_8E33_11D0_9AD0_00A0C9A06E35);
#[allow(non_upper_case_globals)]
const GUID_Spring: GUID = GUID::from_u128(0x13541C27_8E33_11D0_9AD0_00A0C9A06E35);
#[allow(non_upper_case_globals)]
const GUID_Damper: GUID = GUID::from_u128(0x13541C28_8E33_11D0_9AD0_00A0C9A06E35);

/// `DICONDITION` (the vendored bindings do not carry it): one axis of a
/// spring or damper.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DiCondition {
    offset: i32,
    positive_coefficient: i32,
    negative_coefficient: i32,
    positive_saturation: u32,
    negative_saturation: u32,
    dead_band: i32,
}

/// One open DirectInput wheel. Its force feedback is three effects (constant
/// force, spring, damper) that the app drives through `force`, applied here
/// once per poll by `applier`.
pub struct DiDevice {
    pub id: LiveId,
    device: IDirectInputDevice8W,
    guid: GUID,
    vendor_id: u32,
    product_id: u32,
    /// Constant, spring, damper — present only when exclusive access
    /// allowed creating them.
    effects: [Option<IDirectInputEffect>; 3],
    force: GameInputForce,
    applier: ForceApplier,
}

impl DiDevice {
    fn has_force(&self) -> bool {
        self.effects.iter().any(Option::is_some)
    }

    /// The app's latest target → the effects: changed parameters only, and
    /// a restart that extends their finite duration while the target is
    /// fresh. A stale target is zeroed and never restarted, so the effects
    /// run out on their own.
    unsafe fn apply_force(&mut self, now: std::time::Instant) {
        if !self.has_force() {
            return;
        }
        let (target, age) = self.force.latest();
        let commands = self.applier.step(target, age, now);
        if let (Some(e), Some(magnitude)) = (&self.effects[0], commands.constant) {
            let mut params = DICONSTANTFORCE { lMagnitude: magnitude };
            let _ = set_type_params(e, &mut params as *mut _ as *mut _, size_of::<DICONSTANTFORCE>());
        }
        for (slot, value) in [(1, commands.spring), (2, commands.damper)] {
            if let (Some(e), Some(k)) = (&self.effects[slot], value) {
                let mut params = condition(k);
                let _ = set_type_params(e, &mut params as *mut _ as *mut _, size_of::<DiCondition>());
            }
        }
        if commands.restart {
            for e in self.effects.iter().flatten() {
                let _ = e.Start(1, 0);
            }
        }
    }
}

fn condition(k: i32) -> DiCondition {
    DiCondition {
        positive_coefficient: k,
        negative_coefficient: k,
        positive_saturation: FORCE_NOMINAL_MAX as u32,
        negative_saturation: FORCE_NOMINAL_MAX as u32,
        ..Default::default()
    }
}

/// Update only an effect's type-specific parameters (no restart).
unsafe fn set_type_params(effect: &IDirectInputEffect, params: *mut std::ffi::c_void, size: usize) -> Result<(),windows::core::HRESULT> {
    let mut eff = DIEFFECT {
        dwSize: size_of::<DIEFFECT>() as u32,
        dwFlags: 0,
        dwDuration: 0,
        dwSamplePeriod: 0,
        dwGain: 0,
        dwTriggerButton: 0,
        dwTriggerRepeatInterval: 0,
        cAxes: 0,
        rgdwAxes: std::ptr::null_mut(),
        rglDirection: std::ptr::null_mut(),
        lpEnvelope: std::ptr::null_mut(),
        cbTypeSpecificParams: size as u32,
        lpvTypeSpecificParams: params,
        dwStartDelay: 0,
    };
    effect.SetParameters(&mut eff, DIEP_TYPESPECIFICPARAMS)
}

/// One effect on the X axis with a finite duration (`FORCE_EFFECT_DURATION`,
/// renewed by each restart), downloaded but not started.
unsafe fn create_effect(
    device: &IDirectInputDevice8W,
    kind: &GUID,
    params: *mut std::ffi::c_void,
    size: usize,
) -> Option<IDirectInputEffect> {
    let mut axes: [u32; 1] = [0]; // offset 0 = the X axis
    let mut directions: [i32; 1] = [0];
    let mut eff = DIEFFECT {
        dwSize: size_of::<DIEFFECT>() as u32,
        dwFlags: DIEFF_CARTESIAN | DIEFF_OBJECTOFFSETS,
        dwDuration: FORCE_EFFECT_DURATION.as_micros() as u32,
        dwSamplePeriod: 0,
        dwGain: FORCE_NOMINAL_MAX as u32,
        dwTriggerButton: DIEB_NOTRIGGER,
        dwTriggerRepeatInterval: 0,
        cAxes: 1,
        rgdwAxes: axes.as_mut_ptr(),
        rglDirection: directions.as_mut_ptr(),
        lpEnvelope: std::ptr::null_mut(),
        cbTypeSpecificParams: size as u32,
        lpvTypeSpecificParams: params,
        dwStartDelay: 0,
    };
    let mut out: Option<IDirectInputEffect> = None;
    device.CreateEffect(kind, &mut eff, &mut out as *mut _ as *mut _, None).ok()?;
    out
}

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
    /// Open DirectInput wheels.
    pub di_devices: Vec<DiDevice>,
    pub next_wheel_id: u64,
    pub enum_timer: u64,
    /// Which XInput slots (0..4) we believe hold a controller. Only these are polled from the
    /// event loop; probing an empty slot must never happen on the UI thread (see `discovered`).
    pub xinput_connected: [bool; 4],
    /// Device discovery results published by a background thread. Both XInput slot probing and
    /// DirectInput `EnumDevices` stall for hundreds of milliseconds while the driver walks the
    /// USB tree, and doing either from the event loop froze the whole UI — a playing video
    /// visibly stopped for ~300ms every time discovery came around. The event loop now only
    /// reads these fields and never enumerates on its own.
    discovery: Arc<GameInputDiscovery>,
    /// Last `discovery.di_generation` this side has enumerated for.
    di_generation_seen: u32,
    /// XInput rumble per slot: when the running vibration should stop.
    rumble_until: [Option<std::time::Instant>; 4],
}

/// Shared with the discovery thread.
#[derive(Default)]
struct GameInputDiscovery {
    /// Bitmask of XInput slots that answered.
    xinput_mask: AtomicU32,
    /// Bumped whenever the set of attached DirectInput controllers changes, which is the only
    /// time the (expensive) UI-side enumeration is worth doing.
    di_generation: AtomicU32,
}

/// A `IDirectInput8W` for the calling thread. DirectInput objects are not shared between
/// threads, so the discovery thread makes its own.
unsafe fn create_direct_input() -> Option<IDirectInput8W> {
    let hinstance = GetModuleHandleW(None).ok()?;
    let mut di_out: Option<IDirectInput8W> = None;
    // DIRECTINPUT_VERSION is 0x0800
    DirectInput8Create(
        windows::Win32::Foundation::HINSTANCE(hinstance.0),
        0x0800,
        &IDirectInput8W::IID,
        &mut di_out as *mut _ as *mut _,
        None,
    )
    .ok()?;
    di_out
}

/// Instance GUIDs of the attached driving controllers, sorted so the set can be compared.
unsafe fn attached_driving_guids(di: &IDirectInput8W) -> Vec<u128> {
    unsafe extern "system" fn collect(
        lpddi: *mut DIDEVICEINSTANCEW,
        pvref: *mut std::ffi::c_void,
    ) -> BOOL {
        let found = &mut *(pvref as *mut Vec<u128>);
        let instance = &*lpddi;
        if instance.dwDevType & 0xFF == DI8DEVTYPE_DRIVING {
            found.push(instance.guidInstance.to_u128());
        }
        BOOL(DIENUM_CONTINUE as i32)
    }
    let mut found: Vec<u128> = Vec::new();
    let _ = di.EnumDevices(
        DI8DEVCLASS_GAMECTRL,
        Some(collect),
        &mut found as *mut _ as *mut _,
        DIEDFL_ATTACHEDONLY,
    );
    found.sort_unstable();
    found
}

const DISCL_NONEXCLUSIVE: u32 = 0x2;

/// `XINPUT_VIBRATION`: left = the low-frequency motor, right = the high.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XinputVibration {
    left: u16,
    right: u16,
}

unsafe fn xinput_set_state(slot: u32, vibration: &XinputVibration) -> u32 {
    #[link(name = "xinput1_4", kind = "raw-dylib")] extern "system" { fn XInputSetState(dwuserindex: u32, pvibration: *const XinputVibration) -> u32; }
    unsafe { XInputSetState(slot, vibration) }
}

/// The foreground window when it belongs to this process, else null.
fn own_foreground_window() -> windows::Win32::Foundation::HWND {
    #[link(name = "user32", kind = "raw-dylib")] extern "system" { fn GetWindowThreadProcessId(hwnd: windows::Win32::Foundation::HWND, process: *mut u32) -> u32; }
    unsafe {
        let hwnd = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
        let mut process = 0u32;
        if !hwnd.0.is_null() && GetWindowThreadProcessId(hwnd, &mut process) != 0 && process == std::process::id() {
            hwnd
        } else {
            windows::Win32::Foundation::HWND(std::ptr::null_mut())
        }
    }
}

impl WindowsGameInput {
    /// The output handle of a DirectInput wheel: vendor/product always, the
    /// platform force channel when its effects exist. XInput pads have none.
    pub fn output(&self, id: LiveId) -> Option<GameInputOutput> {
        let d = self.di_devices.iter().find(|d| d.id == id)?;
        Some(if d.has_force() {
            GameInputOutput::with_force(id, d.vendor_id, d.product_id, d.force.clone())
        } else {
            GameInputOutput::new(id, d.vendor_id, d.product_id, Arc::new(|_, _| false))
        })
    }

    /// XInput pads are ids 0..3 (their slot); both motors are addressable.
    pub fn haptic_capabilities(&self, id: LiveId) -> GamepadHapticCapabilities {
        let is_pad = id.0 < 4 && self.xinput_connected[id.0 as usize];
        GamepadHapticCapabilities { handles: is_pad, separate_handles: is_pad }
    }

    /// One haptic sample as XInput vibration, stopped by `poll` when its
    /// duration is up.
    pub fn haptic_pulse(&mut self, id: LiveId, pulse: GamepadHapticPulse) -> bool {
        if id.0 >= 4 || !self.xinput_connected[id.0 as usize] {
            return false;
        }
        let motor = |v: f32| (v.clamp(0.0, 1.0) * 65535.0) as u16;
        let vibration = XinputVibration { left: motor(pulse.left), right: motor(pulse.right) };
        let ok = unsafe { xinput_set_state(id.0 as u32, &vibration) } == 0;
        if ok {
            let secs = pulse.duration_s.clamp(0.01, 0.25);
            self.rumble_until[id.0 as usize] = Some(std::time::Instant::now() + std::time::Duration::from_secs_f32(secs));
        }
        ok
    }

    pub fn new() -> Self {
        let direct_input = unsafe { create_direct_input() };

        Self {
            gamepads: Vec::new(),
            states: Vec::new(),
            direct_input,
            di_devices: Vec::new(),
            next_wheel_id: 128,
            enum_timer: 0,
            xinput_connected: [false; 4],
            discovery: Self::spawn_discovery(),
            di_generation_seen: 0,
            rumble_until: [None; 4],
        }
    }

    /// Poll for attached controllers on a background thread. Publishes the XInput slot mask and
    /// bumps a generation counter when the DirectInput device set changes, so the event loop can
    /// do its enumeration only in response to real hot-plug rather than on a timer.
    fn spawn_discovery() -> Arc<GameInputDiscovery> {
        let discovery = Arc::new(GameInputDiscovery::default());
        let out = discovery.clone();
        std::thread::Builder::new()
            .name("makepad-game-input-discovery".into())
            .spawn(move || {
                let di = unsafe { create_direct_input() };
                let mut last_di_set = None;
                loop {
                    let mut mask = 0u32;
                    for slot in 0..4u32 {
                        let mut state = XINPUT_STATE::default();
                        if unsafe { XInputGetState(slot, &mut state) } == 0 {
                            mask |= 1 << slot;
                        }
                    }
                    out.xinput_mask.store(mask, Ordering::Relaxed);

                    if let Some(di) = &di {
                        let set = unsafe { attached_driving_guids(di) };
                        if last_di_set.as_ref() != Some(&set) {
                            last_di_set = Some(set);
                            out.di_generation.fetch_add(1, Ordering::Release);
                        }
                    }

                    if Arc::strong_count(&out) == 1 {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            })
            .ok();
        discovery
    }

    pub fn init() -> Self {
        Self::new()
    }

    pub fn poll<F>(&mut self, mut callback: F)
    where
        F: FnMut(GameInputConnectedEvent),
    {
        self.enum_timer = self.enum_timer.wrapping_add(1);
        // Adopt whatever the discovery thread found. Polling a slot it reports as occupied is
        // cheap; polling one it reports as empty is not, and must never happen here.
        let discovered = self.discovery.xinput_mask.load(Ordering::Relaxed);
        for i in 0..4 {
            if discovered & (1 << i) != 0 {
                self.xinput_connected[i] = true;
            }
        }
        // A haptic sample is short; XInput vibration runs until told
        // otherwise, so stop what has run its time.
        let now = std::time::Instant::now();
        for slot in 0..4 {
            if self.rumble_until[slot].is_some_and(|until| now >= until) {
                self.rumble_until[slot] = None;
                unsafe { xinput_set_state(slot as u32, &XinputVibration::default()) };
            }
        }
        // 1. Poll XInput (Xbox Controllers)
        for i in 0..4 {
            if !self.xinput_connected[i as usize] {
                continue;
            }
            let mut state = XINPUT_STATE::default();
            let result = unsafe { XInputGetState(i, &mut state) };
            self.xinput_connected[i as usize] = result == 0;

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
                    found_devices: Vec<(GUID, String, u32)>,
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
                        // The product GUID's first field is PID << 16 | VID.
                        ctx.found_devices.push((instance.guidInstance, name, instance.guidProduct.data1));
                    }

                    BOOL(DIENUM_CONTINUE as i32)
                }

                // `EnumDevices` blocks for a few hundred milliseconds while the driver walks the
                // USB tree, so it must not run on a timer here: doing it periodically froze the
                // UI (and any playing video) for ~300ms every time it came around. The discovery
                // thread watches for hot-plug and bumps the generation; only then is a stall both
                // necessary and unnoticeable.
                let di_generation = self.discovery.di_generation.load(Ordering::Acquire);
                if di_generation != self.di_generation_seen {
                    self.di_generation_seen = di_generation;
                    let _ = di.EnumDevices(
                        DI8DEVCLASS_GAMECTRL,
                        Some(enum_callback),
                        &mut ctx as *mut _ as *mut _,
                        DIEDFL_ATTACHEDONLY,
                    );

                    let mut active_di_indices = Vec::new();

                    for (guid, name, product) in ctx.found_devices {
                        // Already open? Instance GUIDs are stable for the session.
                        let mut existing_index = None;
                        for (idx, existing) in self.di_devices.iter().enumerate() {
                            if existing.guid == guid {
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
                                        // Force feedback needs exclusive access, which
                                        // DirectInput ties to a top-level window. Only
                                        // this process's own window may hold it: the
                                        // foreground window at hot-plug can be another
                                        // app's. Otherwise input only (non-exclusive,
                                        // no window needed) and no effect.
                                        let hwnd = own_foreground_window();
                                        let level = if hwnd.0.is_null() {
                                            DISCL_BACKGROUND | DISCL_NONEXCLUSIVE
                                        } else {
                                            DISCL_BACKGROUND | DISCL_EXCLUSIVE
                                        };
                                        if device.SetCooperativeLevel(hwnd, level).is_ok() {
                                            // Acquire
                                            let _ = device.Acquire();

                                            // Force feedback: constant, spring, damper,
                                            // downloaded idle; the app's force channel
                                            // starts them (see `DiDevice::apply_force`).
                                            let mut effects = [None, None, None];
                                            if !hwnd.0.is_null() {
                                                let mut constant = DICONSTANTFORCE { lMagnitude: 0 };
                                                effects[0] = create_effect(&device, &GUID_ConstantForce, &mut constant as *mut _ as *mut _, size_of::<DICONSTANTFORCE>());
                                                let mut spring = condition(0);
                                                effects[1] = create_effect(&device, &GUID_Spring, &mut spring as *mut _ as *mut _, size_of::<DiCondition>());
                                                let mut damper = condition(0);
                                                effects[2] = create_effect(&device, &GUID_Damper, &mut damper as *mut _ as *mut _, size_of::<DiCondition>());
                                            }

                                            // Register
                                            let id_val = self.next_wheel_id;
                                            self.next_wheel_id += 1;
                                            let new_id = LiveId(id_val);

                                            self.di_devices.push(DiDevice {
                                                id: new_id,
                                                device: device.clone(),
                                                guid,
                                                vendor_id: product & 0xffff,
                                                product_id: product >> 16,
                                                effects,
                                                force: GameInputForce::new(),
                                                applier: ForceApplier::default(),
                                            });
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
                            let id = self.di_devices[i].id;
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
                let now = std::time::Instant::now();
                for dev in &mut self.di_devices {
                    dev.apply_force(now);
                    let (id, device) = (&dev.id, &dev.device);
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
                            }
                        }
                    }
                }
            }
        }
    }
}
