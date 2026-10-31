//! Activity-owned haptics, permission prompts and location subscriptions.
use super::{exception, file_dialog::Dialogs, java, lock, method, string, void, Engine, Message, Output};
use crate::{backend::exec::OsCmd, media::{location_kind, location_op, permission_kind, permission_op, permission_status}};
use makepad_jni_sys::{registration as reg, *};
use std::{collections::{HashMap, VecDeque}, ffi::c_void, sync::{Mutex, OnceLock}, thread::Thread};

pub(super) enum Command { Haptic(u32), Permission(u32, u32, u32), Location(u32), Dialog(OsCmd) }
#[derive(Clone)]
pub(super) struct Fix {
    pub kind: u32, pub lon: f64, pub lat: f64, pub accuracy: f64, pub time: f64,
    pub alt: f32, pub speed: f32, pub heading: f32, pub text: String,
}
impl Fix {
    fn error(kind: u32, text: String) -> Self {
        Self { kind, lon: 0., lat: 0., accuracy: 0., time: 0., alt: f32::NAN, speed: f32::NAN, heading: f32::NAN, text }
    }
}
struct Subscriber { output: Output, thread: Thread }
#[derive(Default)]
struct Locations {
    outputs: HashMap<u32, Subscriber>,
    granted: bool,
    permission_pending: bool,
    enabled: bool,
    busy: bool,
}
fn locations() -> &'static Mutex<Locations> {
    static LOCATIONS: OnceLock<Mutex<Locations>> = OnceLock::new();
    LOCATIONS.get_or_init(Default::default)
}
struct Permission { output: Output, req: u32, kind: u32, name: &'static str, thread: Thread, code: i32, internal_location: bool }
#[derive(Default)]
struct Permissions { active: Option<Permission>, waiting: VecDeque<Permission>, serial: u32 }
fn permissions() -> &'static Mutex<Permissions> {
    static PERMISSIONS: OnceLock<Mutex<Permissions>> = OnceLock::new();
    PERMISSIONS.get_or_init(Default::default)
}
// Permissions and Activity results are separate Android namespaces. One live
// prompt, with all callers queued, avoids Android dropping simultaneous prompts.
const PERMISSION_BASE: i32 = 0x4000;
const PERMISSION_CODES: u32 = 0x3900;
fn permission_name(kind: u32) -> Option<&'static str> {
    match kind {
        permission_kind::AUDIO_INPUT => Some("android.permission.RECORD_AUDIO"),
        permission_kind::CAMERA => Some("android.permission.CAMERA"),
        permission_kind::HEADSET_CAMERA => Some("horizonos.permission.HEADSET_CAMERA"),
        permission_kind::SCENE_ACCESS => Some("com.oculus.permission.USE_SCENE"),
        permission_kind::LOCATION => Some("android.permission.ACCESS_FINE_LOCATION"),
        _ => None,
    }
}
fn status(java: i32) -> u32 {
    // Java: 0 undetermined, 1 granted. HAL: 0 granted, 1 undetermined.
    match java { 1 => permission_status::GRANTED, 2 => permission_status::DENIED_CAN_RETRY, 3 => permission_status::DENIED_PERMANENT, _ => permission_status::NOT_DETERMINED }
}
pub(super) fn camera_granted() -> bool {
    check("android.permission.CAMERA").is_ok_and(|s| s == permission_status::GRANTED)
        || check("horizonos.permission.HEADSET_CAMERA").is_ok_and(|s| s == permission_status::GRANTED)
}
fn check(name: &str) -> Result<u32, String> {
    java(|env, activity| unsafe {
        let name = string(env, name)?;
        let method = method(env, activity, c"checkPermission", c"(Ljava/lang/String;)I")?;
        let value = ((**env).CallIntMethodA.unwrap())(env, activity, method, [jvalue { l: name }].as_ptr());
        exception(env)?;
        Ok(status(value))
    })
}
pub(super) struct Services { output: Output, dialogs: Dialogs }
impl Engine for Services {
    type Command = Command;
    fn new(output: Output) -> Self { Self { dialogs: Dialogs::new(output.clone()), output } }
    fn command(&mut self, command: Command) {
        let result = match command {
            Command::Haptic(kind) => java(|env, activity| unsafe { void(env, activity, c"performHaptic", c"(I)V", &[jvalue { i: kind as i32 }]) }),
            Command::Dialog(command) => { self.dialogs.open(command); Ok(()) }
            Command::Permission(op, kind, req) => {
                let Some(name) = permission_name(kind) else {
                    self.output.push(Message::Permission(req, kind, permission_status::DENIED_PERMANENT)); return;
                };
                if op == permission_op::CHECK {
                    match check(name) {
                        Ok(status) => self.output.push(Message::Permission(req, kind, status)),
                        Err(e) => { eprintln!("[hal-android-permission] {e}"); self.output.push(Message::Permission(req, kind, permission_status::NOT_DETERMINED)); }
                    }
                } else if op == permission_op::REQUEST {
                    lock(permissions()).waiting.push_back(Permission { output: self.output.clone(), req, kind, name, thread: std::thread::current(), code: 0, internal_location: false });
                    start_permission();
                } else { eprintln!("[hal-android-permission] unknown operation {op}"); }
                Ok(())
            }
            Command::Location(op) if op == location_op::START => {
                lock(locations()).outputs.insert(self.output.sid, Subscriber { output: self.output.clone(), thread: std::thread::current() });
                let name = permission_name(permission_kind::LOCATION).unwrap();
                match check(name) {
                    Ok(permission_status::GRANTED) => { lock(locations()).granted = true; sync_location() }
                    Ok(_) => {
                        let prompt = {
                            let mut state = lock(locations());
                            let prompt = !state.permission_pending;
                            state.permission_pending = true;
                            prompt
                        };
                        if prompt {
                            lock(permissions()).waiting.push_back(Permission { output: self.output.clone(), req: 0, kind: permission_kind::LOCATION, name, thread: std::thread::current(), code: 0, internal_location: true });
                            start_permission();
                        }
                        Ok(())
                    }
                    Err(e) => { broadcast(Fix::error(location_kind::UNAVAILABLE, e.clone())); Err(e) }
                }
            }
            Command::Location(op) if op == location_op::STOP => stop_location(self.output.sid),
            Command::Location(op) => Err(format!("unknown location operation {op}")),
        };
        if let Err(e) = result { eprintln!("[hal-android-services] {e}"); }
    }
    fn step(&mut self) { self.dialogs.step(); start_permission(); if let Err(e) = sync_location() { eprintln!("[hal-android-location] {e}"); } }
    fn active(&self) -> bool { self.dialogs.active() }
}
impl Drop for Services {
    fn drop(&mut self) {
        let _ = stop_location(self.output.sid);
        let replacement = lock(locations()).outputs.values().next().map(|s| s.thread.clone());
        let mut p = lock(permissions());
        p.waiting.retain(|p| p.output.sid != self.output.sid || (p.internal_location && replacement.is_some()));
        if let Some(thread) = replacement {
            for permission in &mut p.waiting {
                if permission.internal_location && permission.output.sid == self.output.sid { permission.thread = thread.clone(); }
            }
        }
        let location_pending = p.active.as_ref().is_some_and(|p| p.internal_location) || p.waiting.iter().any(|p| p.internal_location);
        drop(p);
        lock(locations()).permission_pending = location_pending;
        // Keep an in-flight prompt until Java answers; its code cannot be reused yet.
    }
}
fn stop_location(sid: u32) -> Result<(), String> {
    lock(locations()).outputs.remove(&sid);
    sync_location()
}
fn sync_location() -> Result<(), String> {
    let mut enable = {
        let mut state = lock(locations());
        let enable = !state.outputs.is_empty() && state.granted;
        if state.busy || state.enabled == enable { return Ok(()); }
        state.busy = true;
        state.enabled = enable;
        enable
    };
    // Only one worker posts listener changes. The lock guards field updates,
    // never a JNI call; concurrent START/STOP changes are applied in order.
    loop {
        let result = java(|env, activity| unsafe {
            if enable {
                // These are the old Android StartLocationUpdates defaults.
                void(env, activity, c"startLocationUpdates", c"(JF)V", &[jvalue { j: 1000 }, jvalue { f: 3. }])
            } else { void(env, activity, c"stopLocationUpdates", c"()V", &[]) }
        });
        let mut state = lock(locations());
        if let Err(e) = result {
            state.busy = false; state.enabled = false; state.granted = false;
            drop(state);
            broadcast(Fix::error(location_kind::UNAVAILABLE, e.clone()));
            return Err(e);
        }
        let desired = !state.outputs.is_empty() && state.granted;
        if state.enabled == desired { state.busy = false; return Ok(()); }
        state.enabled = desired;
        enable = desired;
    }
}
fn start_permission() {
    let (name, code) = {
        let mut p = lock(permissions());
        if p.active.is_some() { return; }
        let Some(mut next) = p.waiting.pop_front() else { return; };
        next.code = PERMISSION_BASE + (p.serial % PERMISSION_CODES) as i32;
        p.serial = p.serial.wrapping_add(1);
        let name = next.name;
        let code = next.code;
        p.active = Some(next);
        (name, code)
    };
    let result = java(|env, activity| unsafe {
        let name = string(env, name)?;
        void(env, activity, c"requestPermission", c"(Ljava/lang/String;I)V", &[jvalue { l: name }, jvalue { i: code }])
    });
    if let Err(e) = result { eprintln!("[hal-android-permission] {e}"); finish_permission(name, code, permission_status::NOT_DETERMINED); }
}
fn finish_permission(name: &str, code: i32, status: u32) {
    let (active, next) = {
        let mut p = lock(permissions());
        if !p.active.as_ref().is_some_and(|p| p.name == name && p.code == code) { return; }
        (p.active.take(), p.waiting.front().map(|p| p.thread.clone()))
    };
    if let Some(p) = active {
        if p.kind == permission_kind::LOCATION {
            let (wanted, threads) = {
                let mut state = lock(locations());
                if p.internal_location { state.permission_pending = false; }
                state.granted = status == permission_status::GRANTED;
                (!state.outputs.is_empty(), state.outputs.values().map(|s| s.thread.clone()).collect::<Vec<_>>())
            };
            if wanted && status != permission_status::GRANTED { broadcast(Fix::error(location_kind::DENIED, String::new())); }
            for thread in threads { thread.unpark(); }
        }
        if !p.internal_location { p.output.push(Message::Permission(p.req, p.kind, status)); }
    }
    if let Some(thread) = next { thread.unpark(); }
}
fn broadcast(fix: Fix) {
    let outputs: Vec<_> = lock(locations()).outputs.values().map(|s| s.output.clone()).collect();
    for output in outputs { output.push(Message::Location(fix.clone())); }
}
unsafe extern "system" fn permission_result(env: *mut JNIEnv, _: jclass, permission: jstring, request: jint, java_status: jint) {
    let status = match java_status { 0..=3 => status(java_status), _ => permission_status::DENIED_PERMANENT };
    finish_permission(&reg::string(env, permission), request, status);
}
unsafe extern "system" fn location_update(_: *mut JNIEnv, _: jclass, lon: jdouble, lat: jdouble, accuracy: jfloat, has_alt: jboolean, alt: jdouble, has_speed: jboolean, speed: jfloat, has_heading: jboolean, heading: jfloat, time: jlong) {
    broadcast(Fix { kind: location_kind::FIX, lon, lat, accuracy: accuracy as f64, time: time as f64 / 1000., alt: if has_alt != 0 { alt as f32 } else { f32::NAN }, speed: if has_speed != 0 { speed } else { f32::NAN }, heading: if has_heading != 0 { heading } else { f32::NAN }, text: String::new() });
}
unsafe extern "system" fn location_error(env: *mut JNIEnv, _: jclass, code: jint, message: jstring) {
    broadcast(Fix::error(if code == 1 { location_kind::DENIED } else { location_kind::UNAVAILABLE }, reg::string(env, message)));
    // Java did not install a listener on failure. A subsequent START must retry.
    let mut state = lock(locations());
    state.enabled = false;
    state.granted = false;
}
pub(super) fn native_methods() -> Vec<(&'static str, &'static str, *mut c_void)> {
    vec![
        ("onPermissionResult", "(Ljava/lang/String;II)V", permission_result as *mut c_void),
        ("onLocationUpdate", "(DDFZDZFZFJ)V", location_update as *mut c_void),
        ("onLocationError", "(ILjava/lang/String;)V", location_error as *mut c_void),
    ]
}
