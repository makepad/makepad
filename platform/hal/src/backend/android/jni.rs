//! Host-native entry points. The host calls register_natives from its JNI_OnLoad; no JNI
//! symbol is exported here, so app-side services can register their remaining methods.
use super::{shell::{lock, AndroidParams, Shared}, state::{self, Pointer}, surface::Window};
use crate::{backend::exec::OsCmd, buffers::Wire, clock_ns, input, lifecycle};
use makepad_jni_sys::{registration as reg, *};
use std::{ffi::{c_long, c_void, CStr}, sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, Mutex}};

static ACTIVITY: Mutex<usize> = Mutex::new(0);
static VM: AtomicUsize = AtomicUsize::new(0);
static CHOREOGRAPHER: AtomicUsize = AtomicUsize::new(0);
static FRAME_POSTED: AtomicBool = AtomicBool::new(false);

#[link(name = "android")]
extern "C" {
    fn AChoreographer_getInstance() -> *mut c_void;
    fn AChoreographer_postFrameCallback(choreographer: *mut c_void, callback: unsafe extern "C" fn(c_long, *mut c_void), data: *mut c_void);
}

pub(super) fn request_frame() {
    // an XR session paces frames itself (xrWaitFrame)
    if super::xr::active() { return; }
    let p = CHOREOGRAPHER.load(Ordering::Acquire);
    let shared = Shared::global();
    let visible = lock(&shared.registry).window.is_some();
    let wanted = { let s = lock(&shared.state); visible && s.active && !s.pending.is_empty() };
    if p != 0 && wanted && !FRAME_POSTED.swap(true, Ordering::AcqRel) {
        unsafe { AChoreographer_postFrameCallback(p as _, frame_callback, std::ptr::null_mut()); }
    }
}
unsafe extern "C" fn frame_callback(time: c_long, _: *mut c_void) {
    FRAME_POSTED.store(false, Ordering::Release);
    Shared::global().frame(time as u64);
    request_frame();
}

unsafe fn method(env: *mut JNIEnv, object: jobject, name: &CStr, sig: &CStr) -> jmethodID {
    let cls = ((**env).GetObjectClass.unwrap())(env, object);
    let id = ((**env).GetMethodID.unwrap())(env, cls, name.as_ptr(), sig.as_ptr());
    ((**env).DeleteLocalRef.unwrap())(env, cls);
    id
}
unsafe fn int(env: *mut JNIEnv, object: jobject, name: &CStr, sig: &CStr, args: &[jvalue]) -> i32 {
    let id = method(env, object, name, sig);
    if id.is_null() { return 0; }
    ((**env).CallIntMethodA.unwrap())(env, object, id, args.as_ptr())
}
unsafe fn float(env: *mut JNIEnv, object: jobject, name: &CStr, sig: &CStr, args: &[jvalue]) -> f32 {
    let id = method(env, object, name, sig);
    if id.is_null() { return 0.; }
    ((**env).CallFloatMethodA.unwrap())(env, object, id, args.as_ptr())
}
unsafe fn void(env: *mut JNIEnv, object: jobject, name: &CStr, sig: &CStr, args: &[jvalue]) -> bool {
    let id = method(env, object, name, sig);
    if id.is_null() { return false; }
    ((**env).CallVoidMethodA.unwrap())(env, object, id, args.as_ptr());
    ((**env).ExceptionCheck.unwrap())(env) == 0
}

unsafe extern "system" fn create(env: *mut JNIEnv, _: jclass, activity: jobject) {
    let mut vm = std::ptr::null_mut();
    ((**env).GetJavaVM.unwrap())(env, &mut vm);
    VM.store(vm as usize, Ordering::Release);
    let new = ((**env).NewGlobalRef.unwrap())(env, activity);
    if new.is_null() { return; }
    let old = std::mem::replace(&mut *lock(&ACTIVITY), new as usize);
    if old != 0 { ((**env).DeleteGlobalRef.unwrap())(env, old as _); }
    emit_lifecycle(lifecycle::FOREGROUND);
}
fn emit_lifecycle(kind: u32) { Shared::global().emit(clock_ns(), |q, _| q.lifecycle(kind)); }
unsafe extern "system" fn start(_: *mut JNIEnv, _: jclass) { emit_lifecycle(lifecycle::FOREGROUND); }
unsafe extern "system" fn resume(_: *mut JNIEnv, _: jclass) {
    lock(&Shared::global().state).active = true;
    emit_lifecycle(lifecycle::RESUME); request_frame();
}
unsafe extern "system" fn pause(_: *mut JNIEnv, _: jclass) {
    // an XR session keeps drawing while the Activity pauses (its state decides)
    if !super::xr::active() { lock(&Shared::global().state).active = false; }
    emit_lifecycle(lifecycle::PAUSE);
}
unsafe extern "system" fn stop(_: *mut JNIEnv, _: jclass) {
    if !super::xr::active() { lock(&Shared::global().state).active = false; }
    emit_lifecycle(lifecycle::BACKGROUND);
    crate::background_hook();
}
unsafe extern "system" fn destroy(env: *mut JNIEnv, _: jclass) {
    lock(&Shared::global().state).active = false;
    surface_destroyed(env, std::ptr::null_mut(), std::ptr::null_mut());
    let old = std::mem::take(&mut *lock(&ACTIVITY));
    if old != 0 { ((**env).DeleteGlobalRef.unwrap())(env, old as _); }
    emit_lifecycle(lifecycle::DESTROYED);
}
unsafe extern "system" fn focus(_: *mut JNIEnv, _: jclass, on: jboolean) {
    // the XR session's state is the focus of an immersive app
    if super::xr::active() { return; }
    lock(&Shared::global().state).focused = on != 0;
    Shared::global().emit(clock_ns(), |q, id| { q.focus(id, on != 0); q.lifecycle(if on != 0 { lifecycle::FOCUS_GAINED } else { lifecycle::FOCUS_LOST }); });
}
unsafe extern "system" fn params(env: *mut JNIEnv, _: jclass, cache: jstring, data: jstring, density: jfloat, emulator: jboolean, version: jstring, build: jstring, kernel: jstring, night: jboolean) {
    let params = AndroidParams { cache_path: reg::string(env, cache), data_path: reg::string(env, data), density: if density.is_finite() && density > 0. { density } else { 1. }, emulator: emulator != 0, android_version: reg::string(env, version), build_number: reg::string(env, build), kernel_version: reg::string(env, kernel), night_mode: night != 0, rotation: 0 };
    lock(&Shared::global().state).params = params;
    Shared::global().display();
}
unsafe extern "system" fn night_mode(_: *mut JNIEnv, _: jclass, night: jboolean) { lock(&Shared::global().state).params.night_mode = night != 0; }
unsafe extern "system" fn display_rotation(_: *mut JNIEnv, _: jclass, rotation: jint) { lock(&Shared::global().state).params.rotation = rotation.rem_euclid(4) as u32; }
unsafe extern "system" fn choreographer(_: *mut JNIEnv, _: jclass, refresh: jfloat, _: jint) {
    CHOREOGRAPHER.store(AChoreographer_getInstance() as usize, Ordering::Release);
    if refresh.is_finite() && refresh > 0. { lock(&Shared::global().state).refresh = refresh; }
    Shared::global().display(); request_frame();
}
unsafe fn set_surface(env: *mut JNIEnv, surface: jobject, size: Option<[u32; 2]>) {
    // an immersive app shows its window in the XR session, never on the Activity's surface
    if super::xr::active() { return; }
    let Some(window) = Window::from_java(env, surface) else {
        eprintln!("[hal-android] ANativeWindow_fromSurface failed");
        surface_destroyed(env, std::ptr::null_mut(), surface);
        return;
    };
    let dimensions = size.unwrap_or_else(|| window.size());
    let shared = Shared::global();
    let old = {
        let mut r = lock(&shared.registry);
        if size.is_some() { r.state.changed(window.0, dimensions); } else { r.state.created(window.0, dimensions); }
        r.window.replace(window)
    };
    drop(old);
    emit_lifecycle(lifecycle::SURFACE_AVAILABLE); shared.display(); request_frame();
}
unsafe extern "system" fn surface_created(env: *mut JNIEnv, _: jclass, surface: jobject) { set_surface(env, surface, None); }
unsafe extern "system" fn surface_changed(env: *mut JNIEnv, _: jclass, surface: jobject, w: jint, h: jint) { set_surface(env, surface, Some([w.max(0) as u32, h.max(0) as u32])); }
unsafe extern "system" fn surface_destroyed(_: *mut JNIEnv, _: jclass, _: jobject) {
    if super::xr::active() { return; }
    let shared = Shared::global();
    let old = { let mut r = lock(&shared.registry); r.state.destroyed(); r.window.take() };
    drop(old);
    emit_lifecycle(lifecycle::SURFACE_LOST); shared.display();
}

unsafe fn motion(env: *mut JNIEnv, event: jobject, history: Option<i32>, time: u64) {
    let action = if history.is_some() { 2 } else { int(env, event, c"getActionMasked", c"()I", &[]) };
    let index = int(env, event, c"getActionIndex", c"()I", &[]) as usize;
    let count = int(env, event, c"getPointerCount", c"()I", &[]).max(0);
    let mut pointers = Vec::with_capacity(count as usize);
    for i in 0..count {
        let args = [jvalue { i }, jvalue { i: history.unwrap_or(0) }];
        let id = int(env, event, c"getPointerId", c"(I)I", &args[..1]) as u32;
        let (x, y, pressure) = if history.is_some() {
            (float(env, event, c"getHistoricalX", c"(II)F", &args), float(env, event, c"getHistoricalY", c"(II)F", &args), float(env, event, c"getHistoricalPressure", c"(II)F", &args))
        } else {
            (float(env, event, c"getX", c"(I)F", &args[..1]), float(env, event, c"getY", c"(I)F", &args[..1]), float(env, event, c"getPressure", c"(I)F", &args[..1]))
        };
        pointers.push(Pointer { id, x, y, pressure });
    }
    if ((**env).ExceptionCheck.unwrap())(env) != 0 { return; }
    let shared = Shared::global();
    let sample = state::sample(action, index, &pointers, shared.dpi(), time, history.is_some());
    if sample.points.is_empty() { return; }
    let mut bytes = Vec::with_capacity(sample.points.len() * crate::TouchPoint::SIZE);
    for p in &sample.points { p.write(&mut bytes); }
    shared.emit(sample.time_ns, |q, id| {
        if sample.cancelled { q.touch_cancel(id, sample.points.len() as u32, &bytes); }
        else { q.touch(id, sample.points.len() as u32, &bytes); }
    });
}
unsafe extern "system" fn touch(env: *mut JNIEnv, _: jclass, event: jobject) {
    let id = method(env, event, c"getEventTime", c"()J");
    if id.is_null() { return; }
    let time = ((**env).CallLongMethodA.unwrap())(env, event, id, std::ptr::null());
    motion(env, event, None, time as u64 * 1_000_000);
}
unsafe extern "system" fn touch_nanos(env: *mut JNIEnv, _: jclass, event: jobject, time: jlong) { motion(env, event, None, time as u64); }
unsafe extern "system" fn touch_history(env: *mut JNIEnv, _: jclass, event: jobject, history: jint, time: jlong) { motion(env, event, Some(history), time as u64); }
unsafe extern "system" fn long_click(_: *mut JNIEnv, _: jclass, x: jfloat, y: jfloat, pointer: jint, time: jlong) {
    let dpi = Shared::global().dpi() as f64;
    Shared::global().emit(time as u64 * 1_000_000, |q, id| q.long_press(id, x as f64 / dpi, y as f64 / dpi, pointer as u32));
}
fn mods(meta: i32) -> u32 {
    u32::from(meta & 0x1 != 0) * input::MOD_SHIFT | u32::from(meta & 0x1000 != 0) * input::MOD_CONTROL |
    u32::from(meta & 0x2 != 0) * input::MOD_ALT | u32::from(meta & 0x10000 != 0) * input::MOD_LOGO
}
fn key(code: i32) -> u32 {
    let key = makepad_key_code::android::android_to_makepad_key_code(code as u32);
    key.to_index()
}
unsafe extern "system" fn key_down(_: *mut JNIEnv, _: jclass, code: jint, meta: jint, repeat: jboolean) { Shared::global().emit(clock_ns(), |q, id| q.key_down(id, key(code), repeat != 0, mods(meta))); }
unsafe extern "system" fn key_up(_: *mut JNIEnv, _: jclass, code: jint, meta: jint) { Shared::global().emit(clock_ns(), |q, id| q.key_up(id, key(code), mods(meta))); }
unsafe extern "system" fn back(_: *mut JNIEnv, _: jclass) { Shared::global().emit(clock_ns(), |q, id| q.back_pressed(id)); }
/// `x`, `y` are surface pixels (NaN for the back button), `time` the uptime in milliseconds.
unsafe extern "system" fn back_gesture(_: *mut JNIEnv, _: jclass, state: jint, x: jfloat, y: jfloat, progress: jfloat, edge: jint, time: jlong) {
    let dpi = Shared::global().dpi() as f64;
    Shared::global().emit(time as u64 * 1_000_000, |q, id| q.back_gesture(id, state as u32, x as f64 / dpi, y as f64 / dpi, progress as f64, edge as u32));
}
/// The app did not take a back press: the activity does the system default.
pub(super) fn back_unhandled() -> bool {
    with_activity(|env, activity| unsafe {
        let ok = void(env, activity, c"onBackUnhandled", c"()V", &[]);
        if ((**env).ExceptionCheck.unwrap())(env) != 0 { ((**env).ExceptionDescribe.unwrap())(env); ((**env).ExceptionClear.unwrap())(env); }
        ok
    }).unwrap_or(false)
}
unsafe extern "system" fn character(_: *mut JNIEnv, _: jclass, code: jint) {
    if let Some(c) = char::from_u32(code as u32) { let mut bytes = [0; 4]; let text = c.encode_utf8(&mut bytes); Shared::global().emit(clock_ns(), |q, id| q.text(id, input::text::TYPED, text.as_bytes())); }
}
unsafe extern "system" fn resize_ime(_: *mut JNIEnv, _: jclass, height: jint, open: jboolean) {
    if open == 0 { *lock(&LAST_IME) = None; }
    lock(&Shared::global().state).ime = Some((height.max(0), open != 0));
    let height = height.max(0) as f64 / Shared::global().dpi() as f64;
    Shared::global().emit(clock_ns(), |q, id| q.ime_resize(id, height, open != 0));
}
unsafe extern "system" fn physical_keyboard(_: *mut JNIEnv, _: jclass, connected: jboolean) {
    lock(&Shared::global().state).physical_keyboard = Some(connected != 0);
    Shared::global().emit(clock_ns(), |q, id| q.physical_keyboard(id, connected != 0));
}
unsafe extern "system" fn insets(_: *mut JNIEnv, _: jclass, top: jfloat, right: jfloat, bottom: jfloat, left: jfloat) {
    lock(&Shared::global().state).insets = [top, right, bottom, left]; Shared::global().display();
}
unsafe extern "system" fn ime_state(env: *mut JNIEnv, _: jclass, text: jstring, ss: jint, se: jint, cs: jint, ce: jint) {
    let text = reg::string(env, text);
    Shared::global().emit(clock_ns(), |q, id| q.ime_state(id, ss as u32, se as u32, cs as u32, ce as u32, text.as_bytes()));
}
unsafe extern "system" fn ime_action(_: *mut JNIEnv, _: jclass, action: jint) { Shared::global().emit(clock_ns(), |q, id| q.ime_action(id, action as u32)); }
unsafe extern "system" fn selection_handle(_: *mut JNIEnv, _: jclass, handle: jint, phase: jint, x: jfloat, y: jfloat, time: jlong) {
    let dpi = Shared::global().dpi() as f64;
    Shared::global().emit(time as u64 * 1_000_000, |q, id| q.selection_handle(id, handle as u32, phase as u32, x as f64 / dpi, y as f64 / dpi));
}

/// The config of the soft keyboard last asked for, while it is up: a focused text input re-sends
/// its show on every draw, and each one reaches Android's IME tracker (a show/cancel loop).
static LAST_IME: Mutex<Option<u32>> = Mutex::new(None);

/// The JavaVM and the global reference to the Activity (an XR loader keeps both); `None` before
/// the Activity exists.
pub(super) fn vm_and_activity() -> Option<(*mut c_void, *mut c_void)> {
    let vm = VM.load(Ordering::Acquire);
    let activity = *lock(&ACTIVITY);
    (vm != 0 && activity != 0).then_some((vm as *mut c_void, activity as *mut c_void))
}

type JavaJob = Box<dyn FnOnce(*mut JNIEnv) + Send>;

std::thread_local! {
    /// This thread's companion: an ordinary thread attached to the VM that makes its Java calls.
    static COMPANION: std::cell::OnceCell<Option<std::sync::mpsc::Sender<JavaJob>>> = const { std::cell::OnceCell::new() };
}

/// Runs `f` with a JNI environment: on this thread when the VM knows it, else on the thread's
/// companion (made at its first call, gone with the thread). A native thread is never attached
/// here: the JIT runs programs on stacks of its own, and attaching a thread while it runs on one
/// aborts in ART's stack check. The companion keeps the caller's blocking (a socket read waits on
/// its own companion, not on other threads' calls). `None`: no VM yet.
pub(super) fn on_java<R>(f: impl FnOnce(*mut JNIEnv) -> R) -> Option<R> {
    unsafe {
        let vm = VM.load(Ordering::Acquire) as *mut JavaVM;
        if vm.is_null() { return None; }
        let mut env: *mut JNIEnv = std::ptr::null_mut();
        if ((**vm).GetEnv.unwrap())(vm, &mut env as *mut _ as _, JNI_VERSION_1_6) == 0 {
            return Some(f(env));
        }
        let tx = COMPANION.with(|c| c.get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<JavaJob>();
            let vm = vm as usize;
            std::thread::Builder::new().name("java-companion".into()).spawn(move || {
                let vm = vm as *mut JavaVM;
                let mut env: *mut JNIEnv = std::ptr::null_mut();
                if ((**vm).AttachCurrentThread.unwrap())(vm, &mut env, std::ptr::null_mut()) != 0 { return; }
                for job in rx { job(env); }
                ((**vm).DetachCurrentThread.unwrap())(vm);
            }).ok().map(|_| tx)
        }).clone())?;
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel::<R>(1);
        let job: Box<dyn FnOnce(*mut JNIEnv) + '_> = Box::new(move |env| { let _ = done_tx.send(f(env)); });
        // SAFETY: the caller waits below until the job ran (or was dropped unrun), so what it
        // borrows outlives it, and nothing it holds is used on two threads at once
        let job: JavaJob = std::mem::transmute(job);
        tx.send(job).ok()?;
        done_rx.recv().ok()
    }
}

/// Runs `f` with a JNI environment ([`on_java`]) and the Activity; `None` before the Activity exists.
pub(super) fn with_activity<R>(f: impl FnOnce(*mut JNIEnv, jobject) -> R) -> Option<R> {
    on_java(|env| unsafe {
        let global = *lock(&ACTIVITY);
        let out = if global == 0 { None } else {
            let activity = ((**env).NewLocalRef.unwrap())(env, global as _);
            let r = f(env, activity);
            ((**env).DeleteLocalRef.unwrap())(env, activity);
            Some(r)
        };
        out
    }).flatten()
}

pub(super) fn ime_command(cmd: OsCmd) -> bool {
    match &cmd {
        OsCmd::ImeShow { config, .. } => {
            let mut last = lock(&LAST_IME);
            if *last == Some(*config) { return true; }
            *last = Some(*config);
        }
        OsCmd::ImeHide { .. } => *lock(&LAST_IME) = None,
        _ => {}
    }
    on_java(|env| unsafe {
        let activity = {
            let global = lock(&ACTIVITY);
            if *global == 0 { std::ptr::null_mut() } else { ((**env).NewLocalRef.unwrap())(env, *global as _) }
        };
        let ok = if activity.is_null() { false } else { match cmd {
            OsCmd::ImeShow { config, .. } => {
                let ok = void(env, activity, c"configureKeyboard", c"(IIIIZZ)V", &[jvalue { i: if config & 8 != 0 { 8 } else { 0 } }, jvalue { i: 0 }, jvalue { i: 0 }, jvalue { i: if config & 4 != 0 { 5 } else { 0 } }, jvalue { z: (config & 1 != 0) as u8 }, jvalue { z: (config & 2 != 0) as u8 }]);
                ok && void(env, activity, c"showKeyboard", c"(Z)V", &[jvalue { z: (config & 8 == 0) as u8 }])
            }
            OsCmd::ImeHide { .. } => void(env, activity, c"showKeyboard", c"(Z)V", &[jvalue { z: 0 }]),
            OsCmd::ImeSync { selection, composition, text, .. } => {
                // Cx commands use character offsets; Android's editor uses UTF-16 units.
                let offset = |n: u32| if n == u32::MAX { -1 } else { text.chars().take(n as usize).map(char::len_utf16).sum::<usize>() as i32 };
                let selection = selection.map(offset);
                let composition = if composition[0] == composition[1] { [-1; 2] } else { composition.map(offset) };
                let utf16: Vec<u16> = text.encode_utf16().collect();
                let s = ((**env).NewString.unwrap())(env, utf16.as_ptr(), utf16.len() as i32);
                let ok = !s.is_null() && void(env, activity, c"updateImeTextState", c"(Ljava/lang/String;IIII)V", &[jvalue { l: s }, jvalue { i: selection[0] as i32 }, jvalue { i: selection[1] as i32 }, jvalue { i: composition[0] as i32 }, jvalue { i: composition[1] as i32 }]);
                ((**env).DeleteLocalRef.unwrap())(env, s); ok
            }
            _ => false,
        }};
        if ((**env).ExceptionCheck.unwrap())(env) != 0 { ((**env).ExceptionDescribe.unwrap())(env); ((**env).ExceptionClear.unwrap())(env); }
        if !activity.is_null() { ((**env).DeleteLocalRef.unwrap())(env, activity); }
        if !ok { eprintln!("[hal-android] IME command failed"); }
        ok
    }).unwrap_or(false)
}

/// Registers the shell's, and the media services' natives in one strict pass. The
/// returned names are the Java natives no HAL module owns (app-side services); they are never
/// filled with no-op functions.
/// # Safety
/// Called on a JVM-attached thread with the live MakepadNative class. The host owns JNI_OnLoad.
pub unsafe fn register_natives(env: *mut JNIEnv, class: jclass) -> Result<Vec<String>, String> {
    // the VM from the start: Java calls work before the Activity exists (on_java)
    let mut vm = std::ptr::null_mut();
    ((**env).GetJavaVM.unwrap())(env, &mut vm);
    VM.store(vm as usize, Ordering::Release);
    // fleet crypto and device stats (FleetTls): optional, an APK without the class only lacks them
    if let Err(e) = super::fleet::init(env) {
        eprintln!("[hal-android] fleet: {e}");
    }
    let natives = reg::reflect(env, class)?;
    let mut table = native_methods();
    table.extend(super::media::native_methods());
    table.extend(super::native_layer::native_methods());
    reg::register(env, class, &natives, |name, sig| {
        let Some((_, expected, ptr)) = table.iter().find(|(n, _, _)| *n == name) else { return Ok(None); };
        if sig != *expected { return Err(format!("MakepadNative.{name}: expected {expected}, reflected {sig}")); }
        Ok(Some(*ptr))
    })
}

/// Names and ABI checks, never a second source of Java's reflected registration descriptors.
pub fn native_methods() -> Vec<(&'static str, &'static str, *mut c_void)> {
    vec![
        ("activityOnCreate", "(Ljava/lang/Object;)V", create as *mut c_void),
        ("activityOnStart", "()V", start as *mut c_void),
        ("activityOnResume", "()V", resume as *mut c_void),
        ("activityOnPause", "()V", pause as *mut c_void),
        ("activityOnStop", "()V", stop as *mut c_void),
        ("activityOnDestroy", "()V", destroy as *mut c_void),
        ("activityOnWindowFocusChanged", "(Z)V", focus as *mut c_void),
        ("onAndroidParams", "(Ljava/lang/String;Ljava/lang/String;FZLjava/lang/String;Ljava/lang/String;Ljava/lang/String;Z)V", params as *mut c_void),
        ("onNightModeChanged", "(Z)V", night_mode as *mut c_void),
        ("onDisplayRotation", "(I)V", display_rotation as *mut c_void),
        ("initChoreographer", "(FI)V", choreographer as *mut c_void),
        ("onBackPressed", "()V", back as *mut c_void),
        ("onBackGesture", "(IFFFIJ)V", back_gesture as *mut c_void),
        ("surfaceOnSurfaceCreated", "(Landroid/view/Surface;)V", surface_created as *mut c_void),
        ("surfaceOnSurfaceChanged", "(Landroid/view/Surface;II)V", surface_changed as *mut c_void),
        ("surfaceOnSurfaceDestroyed", "(Landroid/view/Surface;)V", surface_destroyed as *mut c_void),
        ("surfaceOnTouch", "(Landroid/view/MotionEvent;)V", touch as *mut c_void),
        ("surfaceOnTouchNanos", "(Landroid/view/MotionEvent;J)V", touch_nanos as *mut c_void),
        ("surfaceOnTouchHistory", "(Landroid/view/MotionEvent;IJ)V", touch_history as *mut c_void),
        ("surfaceOnLongClick", "(FFIJ)V", long_click as *mut c_void),
        ("surfaceOnKeyDown", "(IIZ)V", key_down as *mut c_void),
        ("surfaceOnKeyUp", "(II)V", key_up as *mut c_void),
        ("surfaceOnCharacter", "(I)V", character as *mut c_void),
        ("surfaceOnResizeTextIME", "(IZ)V", resize_ime as *mut c_void),
        ("surfaceOnPhysicalKeyboardChanged", "(Z)V", physical_keyboard as *mut c_void),
        ("surfaceOnSafeAreaInsets", "(FFFF)V", insets as *mut c_void),
        ("onImeTextStateChanged", "(Ljava/lang/String;IIII)V", ime_state as *mut c_void),
        ("onImeEditorAction", "(I)V", ime_action as *mut c_void),
        ("onSelectionHandleDrag", "(IIFFJ)V", selection_handle as *mut c_void),
    ]
}
