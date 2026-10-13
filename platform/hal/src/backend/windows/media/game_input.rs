//! Game controllers: XInput pads (slots 0..3, both rumble motors) and DirectInput driving wheels
//! (steering, pedals, buttons, and force feedback through three platform effects: constant
//! force, centring spring, damper).
//!
//! Moved from os/windows/windows_game_input.rs. One long-lived worker (`makepad-game-input`) owns
//! DirectInput and polls the devices about every 4 ms; it publishes the device list and states
//! and rings the session when something changed. A second long-lived thread watches for hot-plug
//! (XInput slot probing and DirectInput enumeration stall for hundreds of milliseconds while the
//! driver walks the USB tree) and only bumps a generation the poller reacts to. The app's force
//! targets (`game_op::FORCE`) are atomics the poller reads; a target older than
//! `FORCE_STALE_AFTER` is zero, and the effects run for a finite `FORCE_EFFECT_DURATION` that
//! each fresh target extends, so neither a silent app nor a stalled poll can hold a force.

use super::Signal;
use crate::media::game::*;
use crate::media::game_output;
use makepad_windows_sys::core::{BOOL, GUID};
use makepad_windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use makepad_windows_sys::Win32::Foundation::{HINSTANCE, HWND};
use makepad_windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use makepad_windows_sys::Win32::UI::Input::XboxController::*;
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a force target stays in force without a new one.
const FORCE_STALE_AFTER: Duration = Duration::from_millis(100);
/// The effects' own duration; each fresh target restarts it.
const FORCE_EFFECT_DURATION: Duration = Duration::from_millis(250);
/// How often a held target restarts the effects (well inside their duration).
const FORCE_REFRESH_EVERY: Duration = Duration::from_millis(100);
/// Full scale of a force command (DI_FFNOMINALMAX).
const FORCE_NOMINAL_MAX: i32 = 10_000;
/// The poll period of the worker.
const POLL: Duration = Duration::from_millis(4);
/// The poll period while no controller is attached.
const IDLE_POLL: Duration = Duration::from_millis(50);
/// The hot-plug watch period.
const DISCOVERY: Duration = Duration::from_secs(2);
/// dinput.h's DIDFT_OPTIONAL (an object the device may lack); the Win32 metadata does not carry it.
const DIDFT_OPTIONAL: u32 = 0x8000_0000;
/// DirectInput wheels get ids from here (pads are their XInput slot, 0..3).
const FIRST_WHEEL_ID: u64 = 128;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The latest force target of one device, written by the HAL's thread, read by the poller.
struct ForceTarget {
    constant: AtomicU32,
    spring: AtomicU32,
    damper: AtomicU32,
    /// ms since `epoch` of the last write plus one (0 = never)
    stamp_ms: AtomicU64,
}

/// What the poller publishes and the HAL's thread takes.
#[derive(Default)]
struct Published {
    devices: Vec<GameDevice>,
    states: Vec<GameState>,
    /// the device list changed since the last take
    list_changed: bool,
    /// indexes whose state changed since the last take
    changed: Vec<usize>,
}

struct Shared {
    epoch: Instant,
    published: Mutex<Published>,
    forces: Mutex<HashMap<u64, Arc<ForceTarget>>>,
    /// XInput slots that hold a pad (from the discovery thread)
    xinput_mask: AtomicU32,
    /// bumped when the set of attached DirectInput wheels changed
    di_generation: AtomicU32,
    /// pulse commands for the poller: (slot, left, right, seconds)
    pulses: Mutex<Vec<(u32, f32, f32, f32)>>,
    stop: AtomicBool,
    change: Signal,
}

/// The session's game input service.
pub struct GameInput {
    shared: Arc<Shared>,
    poller: Option<std::thread::JoinHandle<()>>,
    discovery: Option<std::thread::JoinHandle<()>>,
}

impl Drop for GameInput {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        // the poller stops its effects and rumble on the way out
        for t in [self.poller.take(), self.discovery.take()].into_iter().flatten() {
            let _ = t.join();
        }
    }
}

impl GameInput {
    /// Starts polling; the first device list follows (rings `change`).
    pub fn new(change: Signal) -> GameInput {
        let shared = Arc::new(Shared {
            epoch: Instant::now(),
            published: Mutex::new(Published { list_changed: true, ..Default::default() }),
            forces: Mutex::new(HashMap::new()),
            xinput_mask: AtomicU32::new(0),
            di_generation: AtomicU32::new(0),
            pulses: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            change,
        });
        shared.change.set();
        let s = shared.clone();
        let discovery = std::thread::Builder::new().name("makepad-game-input-discovery".into()).spawn(move || discover(&s)).map_err(|e| eprintln!("[hal-media] game input: discovery thread: {e}")).ok();
        let s = shared.clone();
        let poller = std::thread::Builder::new().name("makepad-game-input".into()).spawn(move || Poller::new(s).run()).map_err(|e| eprintln!("[hal-media] game input: poll thread: {e}")).ok();
        GameInput { shared, poller, discovery }
    }

    /// What changed since the last call: the device list (when it changed) and the states that
    /// changed (device id, state).
    pub fn take(&self) -> (Option<Vec<GameDevice>>, Vec<(u64, GameState)>) {
        let mut p = lock(&self.shared.published);
        let list = std::mem::take(&mut p.list_changed).then(|| p.devices.clone());
        let changed = std::mem::take(&mut p.changed);
        let states = changed.into_iter().filter_map(|i| Some((p.devices.get(i)?.id, p.states.get(i)?.clone()))).collect();
        (list, states)
    }

    /// The app's force target of a wheel. False when the device is not one the platform drives.
    pub fn force(&self, id: u64, constant: f32, spring: f32, damper: f32) -> bool {
        let forces = lock(&self.shared.forces);
        let Some(t) = forces.get(&id) else { return false };
        let unit = |v: f32, lo: f32| if v.is_finite() { v.clamp(lo, 1.0) } else { 0.0 };
        t.constant.store(unit(constant, -1.0).to_bits(), Ordering::Relaxed);
        t.spring.store(unit(spring, 0.0).to_bits(), Ordering::Relaxed);
        t.damper.store(unit(damper, 0.0).to_bits(), Ordering::Relaxed);
        t.stamp_ms.store(self.shared.epoch.elapsed().as_millis() as u64 + 1, Ordering::Release);
        true
    }

    /// A haptic sample on a pad (XInput vibration, stopped by the poller when its time is up).
    pub fn pulse(&self, id: u64, pulse: HapticPulse) -> bool {
        if id >= 4 || self.shared.xinput_mask.load(Ordering::Relaxed) & (1 << id) == 0 {
            return false;
        }
        lock(&self.shared.pulses).push((id as u32, pulse.left, pulse.right, pulse.duration_s));
        true
    }
}

/// A `IDirectInput8W` for the calling thread (DirectInput objects are not shared between threads).
unsafe fn create_direct_input() -> Option<IDirectInput8W> {
    let module = GetModuleHandleW(None).ok()?;
    let mut out: *mut std::ffi::c_void = std::ptr::null_mut();
    DirectInput8Create(HINSTANCE(module.0), DIRECTINPUT_VERSION, &IDirectInput8W::IID, &mut out, None).ok()?;
    IDirectInput8W::from_raw(out).ok()
}

/// Attached driving controllers: instance GUID, name, product GUID's first field
/// (`PID << 16 | VID`).
unsafe fn attached_wheels(di: &IDirectInput8W) -> Vec<(GUID, String, u32)> {
    unsafe extern "system" fn collect(instance: *mut DIDEVICEINSTANCEW, found: *mut std::ffi::c_void) -> BOOL {
        let found = &mut *(found as *mut Vec<(GUID, String, u32)>);
        let instance = &*instance;
        if instance.dwDevType & 0xff == DI8DEVTYPE_DRIVING {
            let name = String::from_utf16_lossy(&instance.tszInstanceName).trim_matches('\0').to_string();
            found.push((instance.guidInstance, name, instance.guidProduct.data1));
        }
        BOOL(DIENUM_CONTINUE as i32)
    }
    let mut found: Vec<(GUID, String, u32)> = Vec::new();
    let _ = di.EnumDevices(DI8DEVCLASS_GAMECTRL, Some(collect), &mut found as *mut _ as *mut _, DIEDFL_ATTACHEDONLY);
    found.sort_unstable_by_key(|f| f.0.to_u128());
    found
}

/// The hot-plug watch: XInput slots that answer, and a generation bump when the set of
/// DirectInput wheels changed.
fn discover(s: &Shared) {
    let di = unsafe { create_direct_input() };
    let mut last: Option<Vec<u128>> = None;
    while !s.stop.load(Ordering::Acquire) {
        let mut mask = 0u32;
        for slot in 0..4u32 {
            let mut state = XINPUT_STATE::default();
            if unsafe { XInputGetState(slot, &mut state) } == 0 {
                mask |= 1 << slot;
            }
        }
        s.xinput_mask.store(mask, Ordering::Relaxed);
        if let Some(di) = &di {
            let set: Vec<u128> = unsafe { attached_wheels(di) }.into_iter().map(|w| w.0.to_u128()).collect();
            if last.as_ref() != Some(&set) {
                last = Some(set);
                s.di_generation.fetch_add(1, Ordering::Release);
            }
        }
        // sleep in short steps so a closing session does not wait out the period
        let until = Instant::now() + DISCOVERY;
        while Instant::now() < until && !s.stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// `c_dfDIJoystick2` (DIJOYSTATE2), built here rather than linked from dinput8.lib's data export.
struct JoyFormat {
    format: DIDATAFORMAT,
    _objects: Vec<DIOBJECTDATAFORMAT>,
}

impl JoyFormat {
    fn new() -> JoyFormat {
        let mut objects = Vec::new();
        let axes = [(&GUID_XAxis, 0), (&GUID_YAxis, 4), (&GUID_ZAxis, 8), (&GUID_RxAxis, 12), (&GUID_RyAxis, 16), (&GUID_RzAxis, 20), (&GUID_Slider, 24), (&GUID_Slider, 28)];
        for (guid, offset) in axes {
            objects.push(DIOBJECTDATAFORMAT { pguid: guid, dwOfs: offset, dwType: DIDFT_AXIS | DIDFT_OPTIONAL | DIDFT_ANYINSTANCE, dwFlags: 0 });
        }
        for i in 0..4 {
            objects.push(DIOBJECTDATAFORMAT { pguid: &GUID_POV, dwOfs: 32 + i * 4, dwType: DIDFT_POV | DIDFT_OPTIONAL | DIDFT_ANYINSTANCE, dwFlags: 0 });
        }
        for i in 0..128 {
            objects.push(DIOBJECTDATAFORMAT { pguid: std::ptr::null(), dwOfs: 48 + i, dwType: DIDFT_BUTTON | DIDFT_OPTIONAL | DIDFT_ANYINSTANCE, dwFlags: 0 });
        }
        let format = DIDATAFORMAT {
            dwSize: size_of::<DIDATAFORMAT>() as u32,
            dwObjSize: size_of::<DIOBJECTDATAFORMAT>() as u32,
            dwFlags: DIDF_ABSAXIS,
            dwDataSize: size_of::<DIJOYSTATE2>() as u32,
            dwNumObjs: objects.len() as u32,
            rgodf: objects.as_mut_ptr(),
        };
        JoyFormat { format, _objects: objects }
    }
}

/// The foreground window when it belongs to this process: DirectInput ties exclusive access
/// (which force feedback needs) to a top-level window of the process.
fn own_foreground_window() -> Option<HWND> {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut process = 0u32;
        (!hwnd.0.is_null() && GetWindowThreadProcessId(hwnd, Some(&mut process)) != 0 && process == std::process::id()).then_some(hwnd)
    }
}

fn condition(k: i32) -> DICONDITION {
    DICONDITION { lOffset: 0, lPositiveCoefficient: k, lNegativeCoefficient: k, dwPositiveSaturation: FORCE_NOMINAL_MAX as u32, dwNegativeSaturation: FORCE_NOMINAL_MAX as u32, lDeadBand: 0 }
}

/// One effect on the X axis with a finite duration, downloaded but not started.
unsafe fn create_effect(device: &IDirectInputDevice8W, kind: &GUID, params: *mut std::ffi::c_void, size: usize) -> Option<IDirectInputEffect> {
    let mut axes: [u32; 1] = [0];
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
    device.CreateEffect(kind, &mut eff, &mut out, None).ok()?;
    out
}

/// Only an effect's type-specific parameters (no restart).
unsafe fn set_type_params(effect: &IDirectInputEffect, params: *mut std::ffi::c_void, size: usize) {
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
    let _ = effect.SetParameters(&mut eff, DIEP_TYPESPECIFICPARAMS);
}

/// From the app's target and its age to effect updates: changed parameters only, never a
/// restart on a stale target. `Some` = set that parameter, `restart` = run another duration.
#[derive(Default)]
struct ForceApplier {
    sent: Option<[i32; 3]>,
    last_restart: Option<Instant>,
}

#[derive(Debug, Default, PartialEq)]
struct ForceCommands {
    constant: Option<i32>,
    spring: Option<i32>,
    damper: Option<i32>,
    restart: bool,
}

impl ForceApplier {
    fn step(&mut self, target: [f32; 3], age: Option<Duration>, now: Instant) -> ForceCommands {
        let fresh = age.is_some_and(|a| a <= FORCE_STALE_AFTER);
        let scale = |v: f32| (v.clamp(-1.0, 1.0) * FORCE_NOMINAL_MAX as f32).round() as i32;
        let want = if fresh { [scale(target[0]), scale(target[1].max(0.0)), scale(target[2].max(0.0))] } else { [0; 3] };
        let prev = self.sent;
        let changed = |i: usize| (prev.map(|p| p[i]) != Some(want[i])).then_some(want[i]);
        let mut out = ForceCommands { constant: changed(0), spring: changed(1), damper: changed(2), restart: false };
        let active = want != [0; 3];
        if fresh && active {
            let due = self.last_restart.is_none_or(|t| now.duration_since(t) >= FORCE_REFRESH_EVERY);
            let resumed = prev.is_none_or(|p| p == [0; 3]);
            if due || resumed {
                out.restart = true;
                self.last_restart = Some(now);
            }
        } else if !active {
            self.last_restart = None;
        }
        self.sent = Some(want);
        out
    }
}

/// One open DirectInput wheel.
struct Wheel {
    id: u64,
    guid: u128,
    device: IDirectInputDevice8W,
    /// constant, spring, damper: present when exclusive access allowed making them
    effects: [Option<IDirectInputEffect>; 3],
    force: Arc<ForceTarget>,
    applier: ForceApplier,
}

impl Wheel {
    fn has_force(&self) -> bool {
        self.effects.iter().any(Option::is_some)
    }

    unsafe fn apply_force(&mut self, epoch: Instant, now: Instant) {
        if !self.has_force() {
            return;
        }
        let f = |a: &AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
        let stamp = self.force.stamp_ms.load(Ordering::Acquire);
        let target = [f(&self.force.constant), f(&self.force.spring), f(&self.force.damper)];
        let age = (stamp != 0).then(|| Duration::from_millis((epoch.elapsed().as_millis() as u64 + 1).saturating_sub(stamp)));
        let commands = self.applier.step(target, age, now);
        if let (Some(e), Some(magnitude)) = (&self.effects[0], commands.constant) {
            let mut params = DICONSTANTFORCE { lMagnitude: magnitude };
            set_type_params(e, &mut params as *mut _ as *mut _, size_of::<DICONSTANTFORCE>());
        }
        for (slot, value) in [(1, commands.spring), (2, commands.damper)] {
            if let (Some(e), Some(k)) = (&self.effects[slot], value) {
                let mut params = condition(k);
                set_type_params(e, &mut params as *mut _ as *mut _, size_of::<DICONDITION>());
            }
        }
        if commands.restart {
            for e in self.effects.iter().flatten() {
                let _ = e.Start(1, 0);
            }
        }
    }
}

impl Drop for Wheel {
    fn drop(&mut self) {
        unsafe {
            for e in self.effects.iter().flatten() {
                let _ = e.Stop();
            }
            let _ = self.device.Unacquire();
        }
    }
}

struct Poller {
    s: Arc<Shared>,
    di: Option<IDirectInput8W>,
    format: JoyFormat,
    /// XInput slots believed to hold a pad (only these are polled here)
    pads: [bool; 4],
    wheels: Vec<Wheel>,
    next_wheel: u64,
    di_generation_seen: u32,
    rumble_until: [Option<Instant>; 4],
    /// the list as published, in order: pads then wheels
    devices: Vec<GameDevice>,
    states: Vec<GameState>,
}

fn pad_state(g: &XINPUT_GAMEPAD) -> GamepadState {
    let b = |flag: XINPUT_GAMEPAD_BUTTON_FLAGS| if g.wButtons.0 & flag.0 != 0 { 1.0 } else { 0.0 };
    let axis = |v: i16| v as f32 / 32768.0;
    GamepadState {
        a: b(XINPUT_GAMEPAD_A),
        b: b(XINPUT_GAMEPAD_B),
        x: b(XINPUT_GAMEPAD_X),
        y: b(XINPUT_GAMEPAD_Y),
        left_shoulder: b(XINPUT_GAMEPAD_LEFT_SHOULDER),
        right_shoulder: b(XINPUT_GAMEPAD_RIGHT_SHOULDER),
        left_trigger: g.bLeftTrigger as f32 / 255.0,
        right_trigger: g.bRightTrigger as f32 / 255.0,
        select: b(XINPUT_GAMEPAD_BACK),
        start: b(XINPUT_GAMEPAD_START),
        home: 0.0,
        left_thumb: b(XINPUT_GAMEPAD_LEFT_THUMB),
        right_thumb: b(XINPUT_GAMEPAD_RIGHT_THUMB),
        dpad_up: b(XINPUT_GAMEPAD_DPAD_UP),
        dpad_down: b(XINPUT_GAMEPAD_DPAD_DOWN),
        dpad_left: b(XINPUT_GAMEPAD_DPAD_LEFT),
        dpad_right: b(XINPUT_GAMEPAD_DPAD_RIGHT),
        left_stick: [axis(g.sThumbLX), axis(g.sThumbLY)],
        right_stick: [axis(g.sThumbRX), axis(g.sThumbRY)],
    }
}

/// A wheel's axes: steering -1..1, pedals 0..1 (the common layout: Y throttle, Rz brake, slider
/// clutch, all inverted), the first 32 buttons.
fn wheel_state(j: &DIJOYSTATE2) -> WheelState {
    let axis = |v: i32| (v as f32 - 32768.0) / 32768.0;
    let pedal = |v: i32| (65535 - v) as f32 / 65535.0;
    let mut buttons = 0u32;
    for (i, b) in j.rgbButtons.iter().take(32).enumerate() {
        if b & 0x80 != 0 {
            buttons |= 1 << i;
        }
    }
    WheelState { steering: axis(j.lX), throttle: pedal(j.lY), brake: pedal(j.lRz), clutch: pedal(j.rglSlider[0]), steer_force: 0.0, buttons }
}

impl Poller {
    fn new(s: Arc<Shared>) -> Poller {
        Poller {
            di: unsafe { create_direct_input() },
            s,
            format: JoyFormat::new(),
            pads: [false; 4],
            wheels: Vec::new(),
            next_wheel: FIRST_WHEEL_ID,
            di_generation_seen: 0,
            rumble_until: [None; 4],
            devices: Vec::new(),
            states: Vec::new(),
        }
    }

    fn run(mut self) {
        while !self.s.stop.load(Ordering::Acquire) {
            self.poll();
            // nothing attached: only hot-plug (and a pulse that has nowhere to go) can change that
            let idle = self.wheels.is_empty() && !self.pads.iter().any(|p| *p) && self.s.xinput_mask.load(Ordering::Relaxed) == 0;
            std::thread::sleep(if idle { IDLE_POLL } else { POLL });
        }
        // stop what still runs: effects (Wheel::drop) and rumble
        self.wheels.clear();
        for slot in 0..4 {
            if self.rumble_until[slot].is_some() {
                unsafe { XInputSetState(slot as u32, &XINPUT_VIBRATION { wLeftMotorSpeed: 0, wRightMotorSpeed: 0 }) };
            }
        }
    }

    fn poll(&mut self) {
        let now = Instant::now();
        let mut list_changed = false;
        let mut changed = Vec::new();
        // pulses the HAL's thread queued
        for (slot, left, right, secs) in std::mem::take(&mut *lock(&self.s.pulses)) {
            let motor = |v: f32| (v.clamp(0.0, 1.0) * 65535.0) as u16;
            if unsafe { XInputSetState(slot, &XINPUT_VIBRATION { wLeftMotorSpeed: motor(left), wRightMotorSpeed: motor(right) }) } == 0 {
                self.rumble_until[slot as usize] = Some(now + Duration::from_secs_f32(secs.clamp(0.01, 0.25)));
            }
        }
        for slot in 0..4 {
            if self.rumble_until[slot].is_some_and(|until| now >= until) {
                self.rumble_until[slot] = None;
                unsafe { XInputSetState(slot as u32, &XINPUT_VIBRATION { wLeftMotorSpeed: 0, wRightMotorSpeed: 0 }) };
            }
        }
        // XInput: adopt what discovery found; a slot polled here that stops answering is gone
        let mask = self.s.xinput_mask.load(Ordering::Relaxed);
        for slot in 0..4u64 {
            if mask & (1 << slot) != 0 {
                self.pads[slot as usize] = true;
            }
            if !self.pads[slot as usize] {
                continue;
            }
            let mut x = XINPUT_STATE::default();
            let ok = unsafe { XInputGetState(slot as u32, &mut x) } == 0;
            let index = self.devices.iter().position(|d| d.id == slot);
            if !ok {
                self.pads[slot as usize] = false;
                if let Some(i) = index {
                    self.devices.remove(i);
                    self.states.remove(i);
                    list_changed = true;
                }
                continue;
            }
            let state = GameState::Gamepad(pad_state(&x.Gamepad));
            match index {
                Some(i) => {
                    if self.states[i] != state {
                        self.states[i] = state;
                        changed.push(i);
                    }
                }
                None => {
                    // pads first, in slot order
                    let at = self.devices.iter().position(|d| d.id > slot).unwrap_or(self.devices.len());
                    let haptics = HapticCaps { handles: true, separate_handles: true };
                    self.devices.insert(at, GameDevice { id: slot, name: format!("Xbox Controller {}", slot + 1), announce: true, haptics, vendor: 0, product: 0, output: game_output::NONE });
                    self.states.insert(at, state);
                    list_changed = true;
                }
            }
        }
        // DirectInput wheels: (re)enumerate only when discovery saw the set change
        let generation = self.s.di_generation.load(Ordering::Acquire);
        if generation != self.di_generation_seen {
            self.di_generation_seen = generation;
            list_changed |= self.enumerate_wheels();
        }
        let epoch = self.s.epoch;
        for wheel in &mut self.wheels {
            unsafe {
                wheel.apply_force(epoch, now);
                let _ = wheel.device.Poll();
                let mut j = DIJOYSTATE2::default();
                if wheel.device.GetDeviceState(size_of::<DIJOYSTATE2>() as u32, &mut j as *mut _ as *mut _).is_err() {
                    // lost focus or input: take it again for the next poll
                    let _ = wheel.device.Acquire();
                    continue;
                }
                let state = GameState::Wheel(wheel_state(&j));
                if let Some(i) = self.devices.iter().position(|d| d.id == wheel.id) {
                    if self.states[i] != state {
                        self.states[i] = state;
                        changed.push(i);
                    }
                }
            }
        }
        if list_changed || !changed.is_empty() {
            let mut p = lock(&self.s.published);
            p.devices = self.devices.clone();
            p.states = self.states.clone();
            if list_changed {
                p.list_changed = true;
                // after a list change every state is news (indexes moved)
                p.changed = (0..self.states.len()).collect();
            } else {
                for i in changed {
                    if !p.changed.contains(&i) {
                        p.changed.push(i);
                    }
                }
            }
            drop(p);
            self.s.change.set();
        }
    }

    /// Opens new wheels, drops gone ones. True when the list changed.
    fn enumerate_wheels(&mut self) -> bool {
        let Some(di) = self.di.clone() else { return false };
        let found = unsafe { attached_wheels(&di) };
        let mut changed = false;
        // gone
        let before = self.wheels.len();
        let gone: Vec<u64> = self.wheels.iter().filter(|w| !found.iter().any(|f| f.0.to_u128() == w.guid)).map(|w| w.id).collect();
        self.wheels.retain(|w| !gone.contains(&w.id));
        for id in &gone {
            if let Some(i) = self.devices.iter().position(|d| d.id == *id) {
                self.devices.remove(i);
                self.states.remove(i);
            }
            lock(&self.s.forces).remove(id);
        }
        changed |= self.wheels.len() != before;
        // new
        for (guid, name, product) in found {
            if self.wheels.iter().any(|w| w.guid == guid.to_u128()) {
                continue;
            }
            let Some(wheel) = (unsafe { self.open_wheel(&di, &guid) }) else { continue };
            let output = if wheel.has_force() { game_output::FORCE } else { game_output::NONE };
            lock(&self.s.forces).insert(wheel.id, wheel.force.clone());
            self.devices.push(GameDevice { id: wheel.id, name, announce: true, haptics: HapticCaps::default(), vendor: product & 0xffff, product: product >> 16, output });
            self.states.push(GameState::Wheel(WheelState::default()));
            self.wheels.push(wheel);
            changed = true;
        }
        changed
    }

    unsafe fn open_wheel(&mut self, di: &IDirectInput8W, guid: &GUID) -> Option<Wheel> {
        let mut device: Option<IDirectInputDevice8W> = None;
        di.CreateDevice(guid, &mut device, None).ok()?;
        let device = device?;
        device.SetDataFormat(&mut self.format.format).ok()?;
        // force feedback needs exclusive access, tied to a top-level window of this process; the
        // foreground window at hot-plug can be another app's: then input only, no effects
        let hwnd = own_foreground_window();
        let level = match hwnd {
            Some(_) => DISCL_BACKGROUND | DISCL_EXCLUSIVE,
            None => DISCL_BACKGROUND | DISCL_NONEXCLUSIVE,
        };
        device.SetCooperativeLevel(hwnd.unwrap_or(HWND(std::ptr::null_mut())), level).ok()?;
        let _ = device.Acquire();
        let mut effects = [None, None, None];
        if hwnd.is_some() {
            let mut constant = DICONSTANTFORCE { lMagnitude: 0 };
            effects[0] = create_effect(&device, &GUID_ConstantForce, &mut constant as *mut _ as *mut _, size_of::<DICONSTANTFORCE>());
            let mut spring = condition(0);
            effects[1] = create_effect(&device, &GUID_Spring, &mut spring as *mut _ as *mut _, size_of::<DICONDITION>());
            let mut damper = condition(0);
            effects[2] = create_effect(&device, &GUID_Damper, &mut damper as *mut _ as *mut _, size_of::<DICONDITION>());
        }
        let id = self.next_wheel;
        self.next_wheel += 1;
        let force = Arc::new(ForceTarget { constant: AtomicU32::new(0), spring: AtomicU32::new(0), damper: AtomicU32::new(0), stamp_ms: AtomicU64::new(0) });
        Some(Wheel { id, guid: guid.to_u128(), device, effects, force, applier: ForceApplier::default() })
    }
}
