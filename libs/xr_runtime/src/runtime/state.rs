use crate::{abi::*, protocol::{self, ipc::Channel, Headset, Tracking}};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }
pub type Xr<T> = std::result::Result<T, Result>;
pub fn trace_errors() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MAKEPAD_XR_TRACE").is_some())
}
pub fn clock() -> Time {
    #[repr(C)] struct Timespec { sec: i64, ns: i64 }
    extern "C" { fn clock_gettime(clock: i32, time: *mut Timespec) -> i32; }
    let mut time = Timespec { sec: 0, ns: 0 };
    if unsafe { clock_gettime(1, &mut time) } == 0 { time.sec * 1_000_000_000 + time.ns } else { 0 }
}

pub struct Instance {
    pub name: String,
    pub extensions: HashSet<String>,
    pub paths: Vec<String>,
    pub events: VecDeque<Event>,
    pub headset: Option<Headset>,
    pub connection: Option<Arc<Mutex<Channel>>>,
    pub graphics_checked: bool,
    pub graphics_limits: Option<[u32; 2]>,
    pub vulkan: Option<(usize, usize)>,
    pub suggested: HashMap<String, Vec<(Handle, String)>>,
}
pub enum Event { State(Handle, i32, Time), Profile(Handle) }
pub struct Session {
    pub instance: Handle,
    pub connection: Arc<Mutex<Channel>>,
    pub cancel: std::os::unix::net::UnixStream,
    pub headset: Headset,
    pub gpu: Arc<super::gpu::Gpu>,
    pub exports: Mutex<[Option<super::gpu::Export>; 2]>,
    pub retired: Mutex<Vec<Arc<super::core::swapchains::Swapchain>>>,
    pub frame_done: Condvar,
    pub state: Mutex<SessionState>,
}
#[derive(Default)]
pub struct SessionState {
    pub running: bool,
    pub stopping: bool,
    pub lost: bool,
    /// Idle → Waited → Begun → Idle. Waiting itself releases this mutex.
    pub frame: u8,
    pub waiting: bool,
    pub tracking: Option<Tracking>,
    pub previous: Option<Tracking>,
    pub attached: Option<Vec<Handle>>,
    pub active_sets: Vec<(Handle, Handle)>,
    pub input: Option<Tracking>,
    pub previous_input: Option<Tracking>,
    pub changes: HashMap<(Handle, Handle), Time>,
    pub haptics: [Option<(f32, i64)>; 2],
}
pub struct Space { pub session: Handle, pub kind: SpaceKind, pub offset: Pose }
pub enum SpaceKind { Reference(i32), Action { action: Handle, path: Handle } }
pub struct ActionSet { pub instance: Handle, pub name: String, pub localized_name: String, pub priority: u32 }
pub struct Action { pub set: Handle, pub name: String, pub localized_name: String, pub kind: i32, pub paths: Vec<Handle>, pub bindings: Vec<String> }

#[derive(Default)]
pub struct Registry {
    next: Handle,
    pub instances: HashMap<Handle, Instance>,
    pub sessions: HashMap<Handle, Arc<Session>>,
    pub spaces: HashMap<Handle, Space>,
    pub sets: HashMap<Handle, ActionSet>,
    pub actions: HashMap<Handle, Action>,
    pub swapchains: HashMap<Handle, Arc<super::core::swapchains::Swapchain>>,
}
impl Registry {
    pub fn id(&mut self) -> Handle { self.next = self.next.checked_add(1).expect("OpenXR handle space exhausted"); self.next }
    pub fn instance(&self, id: Handle) -> Xr<&Instance> { self.instances.get(&id).ok_or(HANDLE_INVALID) }
    pub fn instance_mut(&mut self, id: Handle) -> Xr<&mut Instance> { self.instances.get_mut(&id).ok_or(HANDLE_INVALID) }
    pub fn session(&self, id: Handle) -> Xr<Arc<Session>> { self.sessions.get(&id).cloned().ok_or(HANDLE_INVALID) }
    pub fn path(&self, instance: Handle, path: Handle) -> Xr<&str> {
        self.instance(instance)?.paths.get(path.checked_sub(1).ok_or(PATH_INVALID)? as usize).map(String::as_str).ok_or(PATH_INVALID)
    }
}
pub fn registry() -> MutexGuard<'static, Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    lock(REGISTRY.get_or_init(|| Mutex::new(Registry::default())))
}
pub fn session(id: Handle) -> Xr<Arc<Session>> { registry().session(id) }
pub fn system(id: Handle, system: Handle) -> Xr<Headset> {
    let r = registry();
    let instance = r.instance(id)?;
    if system != 1 { return Err(SYSTEM_INVALID); }
    instance.headset.clone().ok_or(FORM_FACTOR_UNAVAILABLE)
}
pub fn event(session: Handle, state: i32) {
    let mut r = registry();
    if let Ok(s) = r.session(session) {
        if let Ok(i) = r.instance_mut(s.instance) { i.events.push_back(Event::State(session, state, clock())); }
    }
}
pub fn io_error(e: impl std::fmt::Display) -> Result { eprintln!("[makepad-openxr] {e}"); RUNTIME_FAILURE }
pub fn connect(instance: Handle) -> Xr<Headset> {
    let (name, existing) = { let r = registry(); let i = r.instance(instance)?; (i.name.clone(), i.connection.clone()) };
    let connection = match existing {
        Some(c) => c,
        None => Arc::new(Mutex::new(Channel::connect().map_err(|e| {
            eprintln!("[makepad-openxr] start XR Bridge and connect the headset: {e}"); FORM_FACTOR_UNAVAILABLE
        })?)),
    };
    let headset = {
        let mut c = lock(&connection);
        c.send(&protocol::Message::Hello { version: protocol::VERSION, app: name }, None).map_err(io_error)?;
        match c.recv().map_err(io_error)?.0 {
            protocol::Message::Headset(headset) => headset,
            protocol::Message::Error(e) => return Err(io_error(e)),
            _ => return Err(io_error("unexpected headset response")),
        }
    };
    let mut r = registry();
    let i = r.instance_mut(instance)?;
    i.connection = Some(connection);
    i.headset = headset.clone();
    headset.ok_or(FORM_FACTOR_UNAVAILABLE)
}

pub unsafe fn input<'a, T>(p: *const T, ty: i32) -> Xr<&'a T> {
    if p.is_null() || (*(p.cast::<Base>())).ty != ty { Err(VALIDATION) } else { Ok(&*p) }
}
pub unsafe fn output<'a, T>(p: *mut T, ty: i32) -> Xr<&'a mut T> {
    input(p, ty)?; Ok(&mut *p)
}
pub unsafe fn array<'a, T>(p: *const T, n: u32) -> Xr<&'a [T]> {
    if n == 0 { return Ok(&[]); }
    if p.is_null() || n as usize > isize::MAX as usize / std::mem::size_of::<T>().max(1) { return Err(VALIDATION); }
    Ok(std::slice::from_raw_parts(p, n as usize))
}
pub unsafe fn text(p: *const std::ffi::c_char, max: usize) -> Xr<String> {
    if p.is_null() { return Err(VALIDATION); }
    let mut len = 0;
    while len < max && *p.add(len) != 0 { len += 1; }
    if len == max { return Err(VALIDATION); }
    std::str::from_utf8(std::slice::from_raw_parts(p.cast(), len)).map(str::to_owned).map_err(|_| VALIDATION)
}
pub fn set_text<const N: usize>(out: &mut [std::ffi::c_char; N], value: &str) {
    out.fill(0);
    for (o, b) in out.iter_mut().take(N.saturating_sub(1)).zip(value.bytes()) { *o = b as std::ffi::c_char; }
}
pub unsafe fn enumerate<T: Copy>(values: &[T], capacity: u32, count: *mut u32, out: *mut T) -> Xr<()> {
    if count.is_null() { return Err(VALIDATION); }
    *count = u32::try_from(values.len()).map_err(|_| OUT_OF_MEMORY)?;
    if capacity == 0 { return Ok(()); }
    if (capacity as usize) < values.len() { return Err(SIZE_INSUFFICIENT); }
    if out.is_null() { return Err(VALIDATION); }
    std::ptr::copy_nonoverlapping(values.as_ptr(), out, values.len());
    Ok(())
}
pub fn valid_pose(p: Pose) -> bool {
    let n: f32 = p.orientation.iter().map(|v| v * v).sum();
    p.position.iter().chain(p.orientation.iter()).all(|v| v.is_finite()) && (n - 1.0).abs() < 0.01
}
pub fn quat(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[3]*b[0]+a[0]*b[3]+a[1]*b[2]-a[2]*b[1], a[3]*b[1]-a[0]*b[2]+a[1]*b[3]+a[2]*b[0],
     a[3]*b[2]+a[0]*b[1]-a[1]*b[0]+a[2]*b[3], a[3]*b[3]-a[0]*b[0]-a[1]*b[1]-a[2]*b[2]]
}
fn rotate(q: [f32; 4], p: [f32; 3]) -> [f32; 3] {
    let v = quat(quat(q, [p[0], p[1], p[2], 0.]), [-q[0], -q[1], -q[2], q[3]]); [v[0], v[1], v[2]]
}
pub fn compose(a: Pose, b: Pose) -> Pose {
    let p = rotate(a.orientation, b.position);
    Pose { orientation: quat(a.orientation, b.orientation), position: std::array::from_fn(|i| a.position[i] + p[i]) }
}
pub fn inverse(p: Pose) -> Pose {
    let q = [-p.orientation[0], -p.orientation[1], -p.orientation[2], p.orientation[3]];
    Pose { orientation: q, position: rotate(q, p.position.map(|v| -v)) }
}
pub fn pose(p: protocol::Pose) -> Pose { Pose { orientation: p.orientation, position: p.position } }
