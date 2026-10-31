//! AMidi lives on the MIDI worker. Java devices cross threads only as global references;
//! ports retain their native device until closed. Android's input/output names are reversed
//! relative to the app: AMidiOutputPort receives, AMidiInputPort sends.
use crate::{
    backend::android::jhelp,
    media::{name_hash, MidiPortInfo},
};
use makepad_jni_sys::{registration as reg, *};
use std::{
    collections::{HashSet, VecDeque},
    ffi::{c_char, c_void},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

const JNI_OK: jint = 0;

struct Api {
    from_java: unsafe extern "C" fn(*mut JNIEnv, jobject, *mut *mut c_void) -> i32,
    release: unsafe extern "C" fn(*const c_void) -> i32,
    num_inputs: unsafe extern "C" fn(*const c_void) -> isize,
    num_outputs: unsafe extern "C" fn(*const c_void) -> isize,
    input_open: unsafe extern "C" fn(*const c_void, i32, *mut *mut c_void) -> i32,
    output_open: unsafe extern "C" fn(*const c_void, i32, *mut *mut c_void) -> i32,
    input_close: unsafe extern "C" fn(*const c_void),
    output_close: unsafe extern "C" fn(*const c_void),
    send: unsafe extern "C" fn(*const c_void, *const u8, usize) -> isize,
    receive: unsafe extern "C" fn(
        *const c_void,
        *mut i32,
        *mut u8,
        usize,
        *mut usize,
        *mut i64,
    ) -> isize,
}

impl Api {
    fn get() -> Result<&'static Self, String> {
        static API: OnceLock<Result<Api, String>> = OnceLock::new();
        // libamidi was introduced in API 29; do not make an API-26 app fail to load.
        API.get_or_init(|| unsafe {
            #[link(name = "dl")]
            extern "C" {
                fn dlopen(name: *const c_char, flags: i32) -> *mut c_void;
                fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
                fn dlclose(handle: *mut c_void) -> i32;
            }
            let library = dlopen(c"libamidi.so".as_ptr(), 2); // RTLD_NOW | RTLD_LOCAL
            if library.is_null() {
                return Err("AMidi requires Android API 29 or newer".into());
            }
            let result = (|| {
                macro_rules! symbol {
                    ($name:literal, $ty:ty) => {{
                        let address = dlsym(library, concat!($name, "\0").as_ptr().cast());
                        if address.is_null() {
                            return Err(format!("libamidi.so is missing {}", $name));
                        }
                        std::mem::transmute::<*mut c_void, $ty>(address)
                    }};
                }
                Ok(Self {
                    from_java: symbol!(
                        "AMidiDevice_fromJava",
                        unsafe extern "C" fn(*mut JNIEnv, jobject, *mut *mut c_void) -> i32
                    ),
                    release: symbol!(
                        "AMidiDevice_release",
                        unsafe extern "C" fn(*const c_void) -> i32
                    ),
                    num_inputs: symbol!(
                        "AMidiDevice_getNumInputPorts",
                        unsafe extern "C" fn(*const c_void) -> isize
                    ),
                    num_outputs: symbol!(
                        "AMidiDevice_getNumOutputPorts",
                        unsafe extern "C" fn(*const c_void) -> isize
                    ),
                    input_open: symbol!(
                        "AMidiInputPort_open",
                        unsafe extern "C" fn(*const c_void, i32, *mut *mut c_void) -> i32
                    ),
                    output_open: symbol!(
                        "AMidiOutputPort_open",
                        unsafe extern "C" fn(*const c_void, i32, *mut *mut c_void) -> i32
                    ),
                    input_close: symbol!(
                        "AMidiInputPort_close",
                        unsafe extern "C" fn(*const c_void)
                    ),
                    output_close: symbol!(
                        "AMidiOutputPort_close",
                        unsafe extern "C" fn(*const c_void)
                    ),
                    send: symbol!(
                        "AMidiInputPort_send",
                        unsafe extern "C" fn(*const c_void, *const u8, usize) -> isize
                    ),
                    receive: symbol!(
                        "AMidiOutputPort_receive",
                        unsafe extern "C" fn(
                            *const c_void,
                            *mut i32,
                            *mut u8,
                            usize,
                            *mut usize,
                            *mut i64,
                        ) -> isize
                    ),
                })
            })();
            if result.is_err() {
                dlclose(library);
            }
            // A successful library handle is retained with the process-wide function table.
            result
        })
        .as_ref()
        .map_err(Clone::clone)
    }
}

struct JavaDevice {
    vm: usize,
    object: usize,
    name: String,
    native: OnceLock<Result<NativeDevice, String>>,
}
impl JavaDevice {
    fn with_env<R>(&self, f: impl FnOnce(*mut JNIEnv, jobject) -> R) -> Option<R> {
        unsafe {
            let vm = self.vm as *mut JavaVM;
            let mut env = std::ptr::null_mut();
            let attached =
                ((**vm).GetEnv.unwrap())(vm, &mut env as *mut _ as _, JNI_VERSION_1_6) != JNI_OK;
            if attached
                && ((**vm).AttachCurrentThread.unwrap())(vm, &mut env, std::ptr::null_mut())
                    != JNI_OK
            {
                return None;
            }
            let out = f(env, self.object as jobject);
            if attached {
                ((**vm).DetachCurrentThread.unwrap())(vm);
            }
            Some(out)
        }
    }
}
impl Drop for JavaDevice {
    fn drop(&mut self) {
        let native = self.native.take();
        self.with_env(move |env, object| unsafe {
            // The native association must be released before MidiDevice.close and the
            // final global reference. Its lifetime includes all session port owners.
            drop(native);
            match jhelp::method(env, object, c"close", c"()V") {
                Ok(method) => {
                    ((**env).CallVoidMethodA.unwrap())(env, object, method, std::ptr::null());
                }
                Err(error) => eprintln!("[hal-midi] close Java device: {error}"),
            }
            let _ = jhelp::check(env);
            ((**env).DeleteGlobalRef.unwrap())(env, object);
        });
    }
}

struct Listener {
    id: u64,
    sender: Sender<Arc<JavaDevice>>,
    thread: std::thread::Thread,
}
fn listeners() -> &'static Mutex<Vec<Listener>> {
    static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

unsafe extern "system" fn device_opened(
    env: *mut JNIEnv,
    _: jclass,
    name: jstring,
    object: jobject,
) {
    if object.is_null() {
        return;
    }
    let name = reg::string(env, name);
    if jhelp::check(env).is_err() {
        return;
    }
    let mut vm = std::ptr::null_mut();
    if ((**env).GetJavaVM.unwrap())(env, &mut vm) != JNI_OK {
        return;
    }
    let object = ((**env).NewGlobalRef.unwrap())(env, object);
    if jhelp::check(env).is_err() || object.is_null() {
        return;
    }
    let device = Arc::new(JavaDevice {
        vm: vm as usize,
        object: object as usize,
        name,
        native: OnceLock::new(),
    });
    let senders: Vec<_> = lock(listeners())
        .iter()
        .map(|l| (l.sender.clone(), l.thread.clone()))
        .collect();
    for (sender, thread) in senders {
        if sender.send(device.clone()).is_ok() {
            thread.unpark();
        }
    }
}

pub fn native_methods() -> Vec<(&'static str, &'static str, *mut c_void)> {
    vec![(
        "onMidiDeviceOpened",
        "(Ljava/lang/String;Ljava/lang/Object;)V",
        device_opened as *mut c_void,
    )]
}

fn open_all(delay: i64) -> Result<(), String> {
    jhelp::activity(|env, activity| unsafe {
        let method = jhelp::method(env, activity, c"openAllMidiDevices", c"(J)V")?;
        ((**env).CallVoidMethodA.unwrap())(env, activity, method, [jvalue { j: delay }].as_ptr());
        Ok(())
    })
}

struct Device {
    api: &'static Api,
    pointer: *mut c_void,
    java: Arc<JavaDevice>,
}
struct NativeDevice {
    api: &'static Api,
    pointer: *mut c_void,
}
// The device is an immutable NDK identity. Each session opens and owns its own ports;
// release runs only after the last port/session drops it. Never call fromJava twice on
// the same Java object (the NDK rejects a second native association).
unsafe impl Send for NativeDevice {}
unsafe impl Sync for NativeDevice {}
impl Drop for NativeDevice {
    fn drop(&mut self) {
        unsafe {
            (self.api.release)(self.pointer);
        }
    }
}
struct Port {
    device: Arc<Device>,
    number: i32,
    desc: MidiPortInfo,
    pointer: *mut c_void,
    decoder: Decoder,
}
impl Port {
    fn close(&mut self) {
        if self.pointer.is_null() {
            return;
        }
        unsafe {
            if self.desc.output {
                (self.device.api.input_close)(self.pointer);
            } else {
                (self.device.api.output_close)(self.pointer);
            }
        }
        self.pointer = std::ptr::null_mut();
        self.decoder = Decoder::default();
    }
    fn open(&mut self) {
        if !self.pointer.is_null() {
            return;
        }
        let result = unsafe {
            if self.desc.output {
                (self.device.api.input_open)(self.device.pointer, self.number, &mut self.pointer)
            } else {
                (self.device.api.output_open)(self.device.pointer, self.number, &mut self.pointer)
            }
        };
        if result != 0 {
            eprintln!("[hal-midi] open {}: {result}", self.desc.name);
            self.close();
        }
    }
}
impl Drop for Port {
    fn drop(&mut self) {
        self.close();
    }
}

/// HAL MIDI carries short messages only. Keep running status/partial packets between reads;
/// SysEx has no representation in the current ABI and is skipped through its terminator.
#[derive(Default)]
struct Decoder {
    bytes: [u8; 3],
    used: usize,
    needed: usize,
    sysex: bool,
}
impl Decoder {
    fn read(&mut self, bytes: &[u8], port: u64, output: &mut Vec<(u64, u32)>) {
        for &byte in bytes {
            if byte >= 0xf8 {
                output.push((port, (byte as u32) << 16));
                continue;
            }
            if byte & 0x80 != 0 {
                self.used = 0;
                self.needed = 0;
                self.sysex = byte == 0xf0;
                if self.sysex || byte == 0xf7 {
                    continue;
                }
                self.bytes = [byte, 0, 0];
                self.used = 1;
                self.needed = message_len(byte);
            } else {
                if self.sysex || self.used == 0 {
                    continue;
                }
                self.bytes[self.used] = byte;
                self.used += 1;
            }
            if self.used == self.needed {
                let [a, b, c] = self.bytes;
                output.push((port, ((a as u32) << 16) | ((b as u32) << 8) | c as u32));
                self.bytes = [a, 0, 0];
                self.used = if a < 0xf0 { 1 } else { 0 };
            }
        }
    }
}
fn message_len(status: u8) -> usize {
    match status {
        0xc0..=0xdf | 0xf1 | 0xf3 => 2,
        0xf6..=0xff => 1,
        _ => 3,
    }
}
struct Pending {
    port: u64,
    bytes: [u8; 3],
    offset: usize,
}

pub struct Midi {
    listener: u64,
    incoming: Receiver<Arc<JavaDevice>>,
    ports: Vec<Port>,
    devices: Vec<Arc<Device>>,
    wanted: [Vec<u64>; 2],
    pending: VecDeque<Pending>,
    published: Option<Vec<MidiPortInfo>>,
    pressured: bool,
    reopen: bool,
    dirty: bool,
    enumerated: Instant,
}
impl Midi {
    pub fn new() -> Result<Self, String> {
        Api::get()?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let listener = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (sender, incoming) = mpsc::channel();
        lock(listeners()).push(Listener {
            id: listener,
            sender,
            thread: std::thread::current(),
        });
        let this = Self {
            listener,
            incoming,
            ports: Vec::new(),
            devices: Vec::new(),
            wanted: Default::default(),
            pending: VecDeque::new(),
            published: None,
            pressured: false,
            reopen: false,
            dirty: true,
            enumerated: Instant::now(),
        };
        open_all(0)?;
        Ok(this)
    }
    fn opened(&mut self, java: Arc<JavaDevice>) {
        if self.devices.iter().any(|d| d.java.name == java.name) {
            return;
        }
        let Ok(api) = Api::get() else {
            return;
        };
        // Only workers initialize this association; the JNI callback only queues the Arc.
        let native = java.native.get_or_init(|| {
            let mut pointer = std::ptr::null_mut();
            let result = java.with_env(|env, object| unsafe {
                let result = (api.from_java)(env, object, &mut pointer);
                jhelp::check(env).map(|_| result)
            });
            if !matches!(result, Some(Ok(0))) || pointer.is_null() {
                if !pointer.is_null() {
                    unsafe {
                        (api.release)(pointer);
                    }
                }
                return Err(format!(
                    "cannot acquire native device {}: {result:?}",
                    java.name
                ));
            }
            Ok(NativeDevice { api, pointer })
        });
        let pointer = match native {
            Ok(native) => native.pointer,
            Err(error) => {
                eprintln!("[hal-midi] {error}");
                return;
            }
        };
        let device = Arc::new(Device { api, pointer, java });
        let counts = unsafe {
            [
                (false, (api.num_outputs)(pointer)),
                (true, (api.num_inputs)(pointer)),
            ]
        };
        for (output, count) in counts {
            for number in 0..count.max(0) {
                let name = format!("{} port {}", device.java.name, number);
                self.ports.push(Port {
                    device: device.clone(),
                    number: number as i32,
                    desc: MidiPortInfo {
                        id: name_hash(&name),
                        output,
                        name,
                    },
                    pointer: std::ptr::null_mut(),
                    decoder: Decoder::default(),
                });
            }
        }
        self.devices.push(device);
        self.dirty = true;
        self.apply();
    }
    fn apply(&mut self) {
        for port in &mut self.ports {
            if self.wanted[port.desc.output as usize].contains(&port.desc.id) {
                port.open();
            } else {
                port.close();
            }
        }
    }
    pub fn use_ports(&mut self, output: bool, ports: Vec<u64>) {
        self.wanted[output as usize] = ports;
        self.apply();
    }
    pub fn reset(&mut self) {
        self.wanted = Default::default();
        self.pending.clear();
        self.ports.clear();
        self.devices.clear();
        self.published = None;
        self.dirty = true;
        self.reopen = true;
        self.enumerated = Instant::now() - Duration::from_secs(1);
    }
    pub fn send(&mut self, port: u64, data: u32) {
        let bytes = [(data >> 16) as u8, (data >> 8) as u8, data as u8];
        if port == 0 {
            for &port in &self.wanted[1] {
                self.pending.push_back(Pending {
                    port,
                    bytes,
                    offset: 0,
                });
            }
        } else {
            self.pending.push_back(Pending {
                port,
                bytes,
                offset: 0,
            });
        }
    }
    pub fn poll(&mut self) -> (Option<Vec<MidiPortInfo>>, Vec<(u64, u32)>) {
        while let Ok(device) = self.incoming.try_recv() {
            self.opened(device);
        }
        let mut messages = Vec::new();
        let mut disconnected = false;
        // One NDK packet per port per worker tick gives commands a turn even under a
        // continuous MIDI stream. 1024 holds AMidi's entire (up to 1015 byte) data packet.
        for port in self
            .ports
            .iter_mut()
            .filter(|p| !p.desc.output && !p.pointer.is_null())
        {
            let mut bytes = [0u8; 1024];
            let (mut opcode, mut count, mut timestamp) = (0, 0, 0);
            let result = unsafe {
                (port.device.api.receive)(
                    port.pointer,
                    &mut opcode,
                    bytes.as_mut_ptr(),
                    bytes.len(),
                    &mut count,
                    &mut timestamp,
                )
            };
            if result < 0 {
                eprintln!("[hal-midi] receive {}: {result}", port.desc.name);
                disconnected = true;
            } else if result > 0 && opcode == 1 {
                if count <= bytes.len() {
                    port.decoder
                        .read(&bytes[..count], port.desc.id, &mut messages);
                } else {
                    eprintln!("[hal-midi] AMidi returned an invalid byte count: {count}");
                }
            }
        }
        // Preserve order within a port, while a disconnected/busy port does not stall
        // unrelated outputs. Visit the existing queue once, retaining every unsent byte.
        let mut blocked = HashSet::new();
        for _ in 0..self.pending.len() {
            let mut pending = self.pending.pop_front().unwrap();
            if disconnected || blocked.contains(&pending.port) {
                self.pending.push_back(pending);
                continue;
            }
            let Some(port) = self
                .ports
                .iter()
                .find(|p| p.desc.output && p.desc.id == pending.port && !p.pointer.is_null())
            else {
                blocked.insert(pending.port);
                if !self.pressured {
                    eprintln!("[hal-midi] output unavailable; retaining queued messages");
                    self.pressured = true;
                }
                self.pending.push_back(pending);
                continue;
            };
            let len = message_len(pending.bytes[0]);
            let remaining = &pending.bytes[pending.offset..len];
            let result = unsafe {
                (port.device.api.send)(port.pointer, remaining.as_ptr(), remaining.len())
            };
            if result == -11 {
                if !self.pressured {
                    eprintln!("[hal-midi] output busy; retaining unsent bytes");
                    self.pressured = true;
                }
                blocked.insert(pending.port);
                self.pending.push_back(pending);
                continue;
            }
            if result < 0 {
                eprintln!(
                    "[hal-midi] send {}: {result}; retaining message for reconnect",
                    port.desc.name
                );
                disconnected = true;
                self.pending.push_back(pending);
                continue;
            }
            pending.offset += (result as usize).min(remaining.len());
            if pending.offset != len {
                if !self.pressured {
                    eprintln!("[hal-midi] output busy; retaining unsent bytes");
                    self.pressured = true;
                }
                blocked.insert(pending.port);
                self.pending.push_back(pending);
            }
        }
        if self.pending.is_empty() {
            self.pressured = false;
        }
        if disconnected {
            self.ports.clear();
            self.devices.clear();
            self.dirty = true;
            self.reopen = true;
            for pending in &mut self.pending {
                pending.offset = 0;
            }
            self.enumerated = Instant::now() - Duration::from_secs(1);
        }
        if self.reopen && self.enumerated.elapsed() >= Duration::from_secs(1) {
            self.enumerated = Instant::now();
            match open_all(1000) {
                Ok(()) => self.reopen = false,
                Err(error) => eprintln!("[hal-midi] enumerate: {error}"),
            }
        }
        if !self.dirty {
            return (None, messages);
        }
        self.dirty = false;
        let ports: Vec<_> = self.ports.iter().map(|p| p.desc.clone()).collect();
        let changed = if self.published.as_ref() == Some(&ports) {
            None
        } else {
            self.published = Some(ports.clone());
            Some(ports)
        };
        (changed, messages)
    }
    pub fn active(&self) -> bool {
        self.reopen
            || !self.pending.is_empty()
            || self
                .ports
                .iter()
                .any(|p| !p.desc.output && !p.pointer.is_null())
    }
}
impl Drop for Midi {
    fn drop(&mut self) {
        lock(listeners()).retain(|listener| listener.id != self.listener);
        // Close ports before releasing devices and their Java global references.
        self.ports.clear();
        self.devices.clear();
    }
}
