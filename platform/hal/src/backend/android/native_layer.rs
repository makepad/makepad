//! Android native views and SurfaceFlinger buffer leases. No callback enters the HAL lock.
use super::{jhelp, shell::{lock, Shared}};
use crate::{buffers::{read_rects, CmdView}, cmds::HalCmd, native_layer::{kind, mode, state}, session::Session, Id, Layer};
use crate::backend::vulkan::Image;
use makepad_jni_sys::{registration as reg, *};
use std::{collections::{HashMap, HashSet}, ffi::{c_char, c_void}, os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd}, sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex, OnceLock}};

type Pointer = *mut c_void;
#[repr(C)]
struct Rect { left: i32, top: i32, right: i32, bottom: i32 }

#[link(name = "android")]
extern "C" { fn android_get_device_api_level() -> i32; }
#[link(name = "dl")]
extern "C" { fn dlsym(handle: Pointer, name: *const c_char) -> Pointer; }
extern "C" { fn poll(fds: *mut PollFd, count: usize, timeout: i32) -> i32; }
extern "C" { fn __system_property_get(name: *const c_char, value: *mut c_char) -> i32; }
#[repr(C)]
struct PollFd { fd: i32, events: i16, revents: i16 }

pub(crate) fn api_level() -> i32 { unsafe { android_get_device_api_level() } }

// Match Java's Log.isLoggable gate; read once, so disabled tracing is just a cached branch.
fn trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        let mut value = [0u8; 92]; // Android PROP_VALUE_MAX, including the terminator.
        let len = unsafe { __system_property_get(c"log.tag.MakepadLayers".as_ptr(), value.as_mut_ptr().cast()) };
        len > 0 && matches!(&value[..len as usize], b"VERBOSE" | b"V")
    })
}

/// Every post-minSdk symbol is resolved at runtime; API 26 never loads an unavailable symbol.
struct Api {
    control: Option<unsafe extern "C" fn(*mut JNIEnv, jobject) -> Pointer>,
    transaction: Option<unsafe extern "C" fn(*mut JNIEnv, jobject) -> Pointer>,
    from_window: unsafe extern "C" fn(Pointer, *const c_char) -> Pointer,
    release: unsafe extern "C" fn(Pointer),
    create: unsafe extern "C" fn() -> Pointer,
    delete: unsafe extern "C" fn(Pointer),
    apply: unsafe extern "C" fn(Pointer),
    buffer: unsafe extern "C" fn(Pointer, Pointer, Pointer, i32),
    geometry: unsafe extern "C" fn(Pointer, Pointer, *const Rect, *const Rect, i32),
    visible: unsafe extern "C" fn(Pointer, Pointer, i8),
    z: unsafe extern "C" fn(Pointer, Pointer, i32),
    complete: unsafe extern "C" fn(Pointer, Pointer, unsafe extern "C" fn(Pointer, Pointer)),
    release_fence: unsafe extern "C" fn(Pointer, Pointer) -> i32,
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        if api_level() < 29 { return None; }
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                let p = dlsym(std::ptr::null_mut(), concat!($name, "\0").as_ptr().cast());
                if p.is_null() { return None; }
                std::mem::transmute::<Pointer, $ty>(p)
            }};
        }
        Some(Api {
            control: if api_level() >= 34 { Some(symbol!("ASurfaceControl_fromJava", unsafe extern "C" fn(*mut JNIEnv, jobject) -> Pointer)) } else { None },
            transaction: if api_level() >= 34 { Some(symbol!("ASurfaceTransaction_fromJava", unsafe extern "C" fn(*mut JNIEnv, jobject) -> Pointer)) } else { None },
            from_window: symbol!("ASurfaceControl_createFromWindow", unsafe extern "C" fn(Pointer, *const c_char) -> Pointer),
            release: symbol!("ASurfaceControl_release", unsafe extern "C" fn(Pointer)),
            create: symbol!("ASurfaceTransaction_create", unsafe extern "C" fn() -> Pointer),
            delete: symbol!("ASurfaceTransaction_delete", unsafe extern "C" fn(Pointer)),
            apply: symbol!("ASurfaceTransaction_apply", unsafe extern "C" fn(Pointer)),
            buffer: symbol!("ASurfaceTransaction_setBuffer", unsafe extern "C" fn(Pointer, Pointer, Pointer, i32)),
            geometry: symbol!("ASurfaceTransaction_setGeometry", unsafe extern "C" fn(Pointer, Pointer, *const Rect, *const Rect, i32)),
            visible: symbol!("ASurfaceTransaction_setVisibility", unsafe extern "C" fn(Pointer, Pointer, i8)),
            z: symbol!("ASurfaceTransaction_setZOrder", unsafe extern "C" fn(Pointer, Pointer, i32)),
            complete: symbol!("ASurfaceTransaction_setOnComplete", unsafe extern "C" fn(Pointer, Pointer, unsafe extern "C" fn(Pointer, Pointer))),
            release_fence: symbol!("ASurfaceTransactionStats_getPreviousReleaseFenceFd", unsafe extern "C" fn(Pointer, Pointer) -> i32),
        })
    }).as_ref()
}

pub(crate) fn compositing() -> bool { api().is_some() }

pub(crate) fn is_activity_window(s: &Session, target: Id) -> bool {
    let Ok(window) = s.windows.get(s.id, target) else { return false; };
    if window.val.embedded.is_some() || window.val.is_hosted() { return false; }
    let physical = window.val.alias_of.or((s.id == crate::session::HOST).then_some(target));
    let shared = Shared::global();
    let registry = lock(&shared.registry);
    physical.is_some() && physical == registry.host && registry.xr_window != Some((s.id, target))
}

pub(crate) struct Slot {
    pub image: Arc<Image>,
    release: Mutex<Option<OwnedFd>>,
}
impl Slot {
    pub fn new(image: Arc<Image>) -> Arc<Self> { Arc::new(Self { image, release: Mutex::new(None) }) }
    pub fn available(slot: &Arc<Self>) -> bool {
        if Arc::strong_count(slot) != 1 { return false; }
        let mut release = lock(&slot.release);
        if let Some(fd) = release.as_ref() {
            let mut pollfd = PollFd { fd: fd.as_raw_fd(), events: 1, revents: 0 };
            if unsafe { poll(&mut pollfd, 1, 0) } <= 0 || pollfd.revents & 1 == 0 { return false; }
        }
        *release = None;
        true
    }
}

struct Control { key: usize, pointer: usize, current: Mutex<Option<Arc<Slot>>>, _window: Option<super::surface::Window> }
impl Drop for Control { fn drop(&mut self) { if let Some(api) = api() { unsafe { (api.release)(self.pointer as Pointer); } } } }
fn controls() -> &'static Mutex<HashMap<usize, Arc<Control>>> {
    static CONTROLS: OnceLock<Mutex<HashMap<usize, Arc<Control>>>> = OnceLock::new();
    CONTROLS.get_or_init(Default::default)
}

pub(crate) struct Segment {
    pub index: usize,
    pub draws: crate::Range,
    pub rect: [i32; 4],
    pub slot: Arc<Slot>,
}
pub(crate) struct Frame {
    serial: u64,
    pub number: u64,
    pub session: u32,
    pub layers: Vec<(Layer, Vec<[f32; 4]>)>,
    pub segments: Vec<Segment>,
    pub composited: bool,
    pub shown: Option<Box<dyn FnOnce() + Send>>,
    fence: Option<OwnedFd>,
}
impl Frame {
    pub fn new(session: u32, layers: Vec<(Layer, Vec<[f32; 4]>)>, composited: bool) -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(1);
        Self { serial: SERIAL.fetch_add(1, Ordering::Relaxed), number: 0, session, layers, segments: Vec::new(), composited, shown: None, fence: None }
    }
    pub fn post(mut self, fence: Option<OwnedFd>) {
        static FIRST: std::sync::Once = std::sync::Once::new();
        FIRST.call_once(|| eprintln!("[hal-android-layer] posting {} segments, acquire fence {}", self.segments.len(), fence.is_some()));
        self.fence = fence;
        let sid = self.session;
        let serial = self.serial;
        let ids: Vec<jlong> = self.layers.iter().map(|(l, _)| packed(l.native)).collect();
        let mut geometry = Vec::<f32>::new();
        for (layer, occluders) in &self.layers {
            geometry.extend(layer.rect); geometry.extend(layer.clip); geometry.extend(layer.radius);
            geometry.extend([layer.opacity, layer.flags as f32, occluders.len() as f32]);
            for rect in occluders { geometry.extend(rect); }
        }
        let count = if self.composited { 5 } else { 0 };
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
        if trace_enabled() { eprintln!("[hal-android-layer] post frame={} serial={serial} ticket={ticket} composited={} segments={} fence={}", self.number, self.composited, self.segments.len(), self.fence.is_some()); }
        lock(frames()).insert(ticket, self);
        let result = jhelp::activity(|env, activity| unsafe {
            let ji = ((**env).NewLongArray.unwrap())(env, ids.len() as i32);
            let jg = ((**env).NewFloatArray.unwrap())(env, geometry.len() as i32);
            jhelp::check(env)?;
            ((**env).SetLongArrayRegion.unwrap())(env, ji, 0, ids.len() as i32, ids.as_ptr());
            ((**env).SetFloatArrayRegion.unwrap())(env, jg, 0, geometry.len() as i32, geometry.as_ptr());
            let method = jhelp::method(env, activity, c"applyNativeLayerFrame", c"(JJI[J[FI)V")?;
            ((**env).CallVoidMethodA.unwrap())(env, activity, method, [jvalue { j: ticket as i64 }, jvalue { j: serial as i64 }, jvalue { i: sid as i32 }, jvalue { l: ji }, jvalue { l: jg }, jvalue { i: count }].as_ptr());
            jhelp::check(env)
        });
        if let Err(e) = result { let removed = lock(frames()).remove(&ticket); drop(removed); eprintln!("[hal-android-layer] post: {e}"); }
    }
}
fn frames() -> &'static Mutex<HashMap<u64, Frame>> {
    static FRAMES: OnceLock<Mutex<HashMap<u64, Frame>>> = OnceLock::new();
    FRAMES.get_or_init(Default::default)
}

struct Completion { serial: u64, previous: Vec<(Arc<Control>, Arc<Slot>)>, shown: Option<Box<dyn FnOnce() + Send>> }
unsafe extern "C" fn completed(context: Pointer, stats: Pointer) {
    let mut completion = Box::from_raw(context.cast::<Completion>());
    if trace_enabled() { eprintln!("[hal-android-layer] complete serial={}", completion.serial); }
    if let Some(api) = api() {
        for (control, slot) in &completion.previous {
            let fd = (api.release_fence)(stats, control.pointer as Pointer);
            *lock(&slot.release) = (fd >= 0).then(|| OwnedFd::from_raw_fd(fd));
        }
    }
    if let Some(shown) = completion.shown.take() {
        static FIRST: std::sync::Once = std::sync::Once::new();
        FIRST.call_once(|| eprintln!("[hal-android-layer] first SurfaceFlinger transaction presented (API {})", api_level()));
        shown();
    }
}

unsafe fn java_controls(env: *mut JNIEnv, array: jobjectArray) -> Option<Vec<Option<Arc<Control>>>> {
    let api = api()?;
    let mut out = Vec::new();
    for i in 0..((**env).GetArrayLength.unwrap())(env, array) {
        let object = ((**env).GetObjectArrayElement.unwrap())(env, array, i);
        if object.is_null() { out.push(None); continue; }
        let (key, pointer, window) = if let Some(from_java) = api.control {
            let pointer = from_java(env, object) as usize;
            (pointer, pointer, None)
        } else {
            let window = super::surface::Window::from_java(env, object)?;
            let existing = lock(controls()).get(&window.0).cloned();
            if let Some(existing) = existing {
                ((**env).DeleteLocalRef.unwrap())(env, object); out.push(Some(existing)); continue;
            }
            let pointer = (api.from_window)(window.0 as Pointer, c"Makepad segment buffer".as_ptr()) as usize;
            (window.0, pointer, Some(window))
        };
        ((**env).DeleteLocalRef.unwrap())(env, object);
        if pointer == 0 { return None; }
        let owned = Arc::new(Control { key, pointer, current: Mutex::new(None), _window: window });
        // Drop a duplicate NDK reference outside the short registry lock.
        let control = lock(controls()).entry(key).or_insert_with(|| owned.clone()).clone();
        out.push(Some(control));
    }
    Some(out)
}

unsafe extern "system" fn commit(env: *mut JNIEnv, _: jclass, ticket: jlong, transaction: jobject, surfaces: jobjectArray, origin_x: jint, origin_y: jint) -> jboolean {
    if api().is_none() || surfaces.is_null() {
        let frame = lock(frames()).remove(&(ticket as u64));
        if let Some(mut frame) = frame { if let Some(shown) = frame.shown.take() { shown(); } }
        return 1;
    }
    let Some(api) = api() else { return 0; };
    let Some(controls) = java_controls(env, surfaces) else { return 0; };
    let owned = transaction.is_null();
    let tx = if owned { (api.create)() } else if let Some(from_java) = api.transaction { from_java(env, transaction) } else { return 0; };
    if tx.is_null() { return 0; }
    let Some(mut frame) = lock(frames()).remove(&(ticket as u64)) else { if owned { (api.delete)(tx); } return 1; };
    if frame.segments.iter().any(|s| controls.get(s.index).is_none_or(Option::is_none)) {
        lock(frames()).insert(ticket as u64, frame); if owned { (api.delete)(tx); } return 0;
    }
    // Duplicate all acquire fences before mutating anything, so failure is retryable atomically.
    let fences: Result<Vec<_>, _> = frame.segments.iter().map(|_| frame.fence.as_ref().map(OwnedFd::try_clone).transpose()).collect();
    let Ok(fences) = fences else { lock(frames()).insert(ticket as u64, frame); if owned { (api.delete)(tx); } return 0; };
    if trace_enabled() { eprintln!("[hal-android-layer] commit frame={} serial={} ticket={ticket} path={}", frame.number, frame.serial, if owned { "direct" } else { "java-draw" }); }
    let mut completion = Box::new(Completion { serial: frame.serial, previous: Vec::new(), shown: frame.shown.take() });
    let mut fences = fences.into_iter();
    for (i, control) in controls.into_iter().enumerate() {
        let Some(control) = control else { continue; };
        let ptr = control.pointer as Pointer;
        let segment = frame.segments.iter().find(|s| s.index == i);
        if let Some(segment) = segment {
            let [x, y, w, h] = segment.rect;
            let (x, y) = if i == 0 { (x, y) } else { (x + origin_x, y + origin_y) };
            let fd = fences.next().flatten().map_or(-1, IntoRawFd::into_raw_fd);
            let buffer = segment.slot.image.external.as_ref().unwrap().downcast_ref::<crate::backend::vulkan::NativeHandle>().unwrap().as_ptr();
            (api.buffer)(tx, ptr, buffer.cast(), fd);
            (api.geometry)(tx, ptr, &Rect { left: 0, top: 0, right: w, bottom: h }, &Rect { left: x, top: y, right: x+w, bottom: y+h }, 0);
            (api.z)(tx, ptr, if i == 0 { 1 } else { i as i32 });
            (api.visible)(tx, ptr, 1);
        } else {
            (api.buffer)(tx, ptr, std::ptr::null_mut(), -1);
            (api.visible)(tx, ptr, 0);
        }
        let old = std::mem::replace(&mut *lock(&control.current), segment.map(|s| s.slot.clone()));
        if let Some(old) = old { completion.previous.push((control, old)); }
    }
    (api.complete)(tx, Box::into_raw(completion).cast(), completed);
    if owned { (api.apply)(tx); (api.delete)(tx); }
    1
}

unsafe extern "system" fn discard(_: *mut JNIEnv, _: jclass, ticket: jlong) { let removed = lock(frames()).remove(&(ticket as u64)); drop(removed); }
unsafe extern "system" fn release_surfaces(env: *mut JNIEnv, _: jclass, surfaces: jobjectArray) {
    let Some(api) = api() else { return; };
    let Some(surfaces) = java_controls(env, surfaces) else { return; };
    let tx = (api.create)();
    let mut completion = Box::new(Completion { serial: 0, previous: Vec::new(), shown: None });
    for control in surfaces.into_iter().flatten() {
        (api.buffer)(tx, control.pointer as Pointer, std::ptr::null_mut(), -1);
        (api.visible)(tx, control.pointer as Pointer, 0);
        lock(controls()).remove(&control.key);
        let old = lock(&control.current).take();
        if let Some(old) = old { completion.previous.push((control, old)); }
    }
    (api.complete)(tx, Box::into_raw(completion).cast(), completed);
    (api.apply)(tx); (api.delete)(tx);
}

pub(crate) fn packed(id: Id) -> i64 { ((id.gen as u64) << 32 | id.index as u64) as i64 }
unsafe extern "system" fn web_state(env: *mut JNIEnv, _: jclass, session: jint, id: jlong, what: jint, a: jlong, b: jlong, text: jstring) {
    let id = Id::new(id as u32, ((id as u64) >> 32) as u32);
    let text = reg::string(env, text);
    Shared::global().emit_to(session as u32, Id::NONE, crate::clock_ns(), |q, _| q.native_layer_state(id, what as u32, a as u64, b as u64, text.as_bytes()));
}

pub(super) fn native_methods() -> Vec<(&'static str, &'static str, Pointer)> {
    vec![
        ("onNativeLayerState", "(IJIJJLjava/lang/String;)V", web_state as Pointer),
        ("commitNativeLayerFrame", "(JLjava/lang/Object;[Ljava/lang/Object;II)Z", commit as Pointer),
        ("discardNativeLayerFrame", "(J)V", discard as Pointer),
        ("releaseNativeLayerSurfaces", "([Ljava/lang/Object;)V", release_surfaces as Pointer),
    ]
}

#[derive(Default)]
pub(crate) struct Views { web: HashSet<(u32, Id)>, over_budget: HashSet<(u32, Id)>, worker: Option<crate::backend::vulkan::Worker> }
impl Views {
    fn enqueue(&mut self, job: impl FnOnce() + Send + 'static) -> Result<(), String> {
        if self.worker.is_none() { self.worker = Some(crate::backend::vulkan::Worker::new("makepad-native-views")?); }
        self.worker.as_ref().unwrap().push(job);
        Ok(())
    }
    pub fn hide(&mut self, sid: u32) {
        let frame = Frame::new(sid, Vec::new(), false);
        if let Err(e) = self.enqueue(move || frame.post(None)) { eprintln!("[hal-android-layer] hide: {e}"); }
    }
    pub fn command(&mut self, s: &mut Session, c: &HalCmd) -> bool {
        let (id, name, signature, operation, arg, data) = match *c {
            HalCmd::NativeLayerCreate { id, kind: kind::WEB, params } => {
                self.web.insert((s.id, id));
                (id, c"createNativeWebLayer", c"(IJ[B)V", None, 0, params)
            }
            HalCmd::NativeLayerOp { id, op, arg, data } if self.web.contains(&(s.id, id)) =>
                (id, c"nativeWebLayerOp", c"(IJIJ[B)V", Some(op), arg, data),
            HalCmd::NativeLayerDestroy { id } if self.web.remove(&(s.id, id)) => {
                self.over_budget.remove(&(s.id, id));
                let sid = s.id;
                if let Err(e) = self.enqueue(move || destroy(sid, id)) { eprintln!("[hal-android-layer] destroy: {e}"); }
                return true;
            }
            _ => return false,
        };
        let sid = s.id;
        let data = data.to_vec();
        let result = self.enqueue(move || {
        let result = jhelp::activity(|env, activity| unsafe {
            let bytes = ((**env).NewByteArray.unwrap())(env, data.len() as i32);
            jhelp::check(env)?;
            ((**env).SetByteArrayRegion.unwrap())(env, bytes, 0, data.len() as i32, data.as_ptr().cast());
            let method = jhelp::method(env, activity, name, signature)?;
            let mut args = vec![jvalue { i: sid as i32 }, jvalue { j: packed(id) }];
            if let Some(op) = operation { args.extend([jvalue { i: op as i32 }, jvalue { j: arg as i64 }]); }
            args.push(jvalue { l: bytes });
            ((**env).CallVoidMethodA.unwrap())(env, activity, method, args.as_ptr());
            jhelp::check(env)
        });
        if let Err(error) = result {
            Shared::global().emit_to(sid, Id::NONE, crate::clock_ns(), |q, _| q.native_layer_state(id, state::REALIZED, mode::UNSUPPORTED, 0, error.as_bytes()));
            eprintln!("[hal-android-layer] {error}");
        }
        });
        if let Err(error) = result {
            s.events.native_layer_state(id, state::REALIZED, mode::UNSUPPORTED, 0, error.as_bytes());
            eprintln!("[hal-android-layer] {error}");
        }
        true
    }
    pub fn geometry(&mut self, s: &mut Session, layers: &[Layer], view: &CmdView) -> Vec<(Layer, Vec<[f32; 4]>)> {
        let mut placed = Vec::new();
        for layer in layers.iter().filter(|l| l.kind == Layer::NATIVE && self.web.contains(&(s.id, l.native))) {
            if placed.len() == 4 {
                if self.over_budget.insert((s.id, layer.native)) {
                    s.events.native_layer_state(layer.native, state::REALIZED, mode::UNSUPPORTED, 0, b"composited native layer budget (4) exceeded");
                }
                continue;
            }
            if self.over_budget.remove(&(s.id, layer.native)) {
                s.events.native_layer_state(layer.native, state::REALIZED, mode::COMPOSITED, 0, b"");
            }
            placed.push((*layer, read_rects(view.arena, layer.occluders).collect()));
        }
        placed
    }
    pub fn close(&mut self, sid: u32) {
        let ids: Vec<_> = self.web.iter().filter(|(s, _)| *s == sid).copied().collect();
        for (_, id) in ids {
            self.web.remove(&(sid, id));
            self.over_budget.remove(&(sid, id));
            if let Err(e) = self.enqueue(move || destroy(sid, id)) { eprintln!("[hal-android-layer] destroy: {e}"); }
        }
    }
}
fn destroy(sid: u32, id: Id) {
    if let Err(e) = jhelp::activity(|env, activity| unsafe {
        let method = jhelp::method(env, activity, c"destroyNativeWebLayer", c"(IJ)V")?;
        ((**env).CallVoidMethodA.unwrap())(env, activity, method, [jvalue { i: sid as i32 }, jvalue { j: packed(id) }].as_ptr());
        jhelp::check(env)
    }) { eprintln!("[hal-android-layer] destroy: {e}"); }
}
