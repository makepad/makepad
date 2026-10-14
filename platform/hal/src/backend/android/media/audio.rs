//! AAudio streams owned by the audio control worker. Only planar/interleaved conversion and
//! the registered HAL entry run on AAudio's realtime thread; all storage is allocated before
//! requestStart. A callback larger than the scratch quantum is split, never truncated.
use crate::{
    backend::android::jhelp,
    media::{name_hash, AudioBlock, AudioDeviceInfo, AudioFn},
};
use makepad_jni_sys::{jvalue, registration as reg};
use std::{
    cell::UnsafeCell,
    collections::HashSet,
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub const AUDIO_BLOCK_FRAMES: usize = 1024;

#[derive(Clone, Copy)]
pub struct Entry {
    pub function: AudioFn,
    pub context: usize,
}

#[derive(Default)]
pub struct Slot {
    busy: AtomicBool,
    changing: AtomicBool,
    entry: UnsafeCell<Option<Entry>>,
}
// The atomic admission gate protects the entry; the realtime thread never waits.
unsafe impl Send for Slot {}
unsafe impl Sync for Slot {}
impl Slot {
    pub fn replace(&self, entry: Option<Entry>) {
        self.changing.store(true, Ordering::SeqCst);
        // AudioEntry explicitly requires quiescence before hal_submit returns: after this
        // finite admitted block, the client may destroy the old context immediately.
        while self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            std::hint::spin_loop();
        }
        unsafe {
            *self.entry.get() = entry;
        }
        self.busy.store(false, Ordering::SeqCst);
        self.changing.store(false, Ordering::SeqCst);
    }
    fn call(&self, block: &mut AudioBlock) {
        if self.changing.load(Ordering::SeqCst)
            || self
                .busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return;
        }
        if let Some(entry) = unsafe { *self.entry.get() } {
            (entry.function)(entry.context as *mut c_void, block);
        }
        self.busy.store(false, Ordering::SeqCst);
    }
}

#[derive(Clone, Default)]
pub struct Entries([Vec<Arc<Slot>>; 2]);
impl Entries {
    pub fn slot(&mut self, direction: usize, index: usize) -> Arc<Slot> {
        let slots = &mut self.0[direction];
        slots.resize_with(slots.len().max(index + 1), || Arc::new(Slot::default()));
        slots[index].clone()
    }
    pub fn clear(&self) {
        for slot in self.0.iter().flatten() {
            slot.replace(None);
        }
    }
}

// These are the NDK entry points used by this transport, deliberately independent of the
// former platform Android module. AAudio is part of the existing API-26 minimum.
#[link(name = "aaudio")]
extern "C" {
    fn AAudio_createStreamBuilder(builder: *mut *mut c_void) -> i32;
    fn AAudioStreamBuilder_delete(builder: *mut c_void) -> i32;
    fn AAudioStreamBuilder_setDeviceId(builder: *mut c_void, device: i32);
    fn AAudioStreamBuilder_setDirection(builder: *mut c_void, direction: i32);
    fn AAudioStreamBuilder_setSharingMode(builder: *mut c_void, mode: i32);
    fn AAudioStreamBuilder_setSampleRate(builder: *mut c_void, rate: i32);
    fn AAudioStreamBuilder_setChannelCount(builder: *mut c_void, channels: i32);
    fn AAudioStreamBuilder_setFormat(builder: *mut c_void, format: i32);
    fn AAudioStreamBuilder_setBufferCapacityInFrames(builder: *mut c_void, frames: i32);
    fn AAudioStreamBuilder_setPerformanceMode(builder: *mut c_void, mode: i32);
    fn AAudioStreamBuilder_setDataCallback(
        builder: *mut c_void,
        callback: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, i32) -> i32,
        context: *mut c_void,
    );
    fn AAudioStreamBuilder_setErrorCallback(
        builder: *mut c_void,
        callback: unsafe extern "C" fn(*mut c_void, *mut c_void, i32),
        context: *mut c_void,
    );
    fn AAudioStreamBuilder_openStream(builder: *mut c_void, stream: *mut *mut c_void) -> i32;
    fn AAudioStream_getChannelCount(stream: *mut c_void) -> i32;
    fn AAudioStream_getSampleRate(stream: *mut c_void) -> i32;
    fn AAudioStream_getFormat(stream: *mut c_void) -> i32;
    fn AAudioStream_requestStart(stream: *mut c_void) -> i32;
    fn AAudioStream_requestStop(stream: *mut c_void) -> i32;
    fn AAudioStream_close(stream: *mut c_void) -> i32;
    fn AAudioStream_getXRunCount(stream: *mut c_void) -> i32;
    fn AAudioStream_getFramesPerBurst(stream: *mut c_void) -> i32;
    fn AAudioStream_getBufferCapacityInFrames(stream: *mut c_void) -> i32;
    fn AAudioStream_getBufferSizeInFrames(stream: *mut c_void) -> i32;
    fn AAudioStream_setBufferSizeInFrames(stream: *mut c_void, frames: i32) -> i32;
}

fn result(operation: &str, code: i32) -> Result<(), String> {
    if code < 0 {
        Err(format!("AAudio {operation}: {code}"))
    } else {
        Ok(())
    }
}

struct Builder(*mut c_void);
impl Drop for Builder {
    fn drop(&mut self) {
        unsafe {
            AAudioStreamBuilder_delete(self.0);
        }
    }
}
struct Scratch {
    samples: Vec<f32>,
    planes: Vec<*mut f32>,
}
struct Callback {
    device: u64,
    input: bool,
    channels: usize,
    rate: f64,
    slot: Arc<Slot>,
    error: AtomicI32,
    // Only AAudio's serialized data callback touches the scratch. The error callback
    // accesses only error, and the worker reads it atomically before closing the stream.
    scratch: UnsafeCell<Scratch>,
}
struct Stream {
    pointer: *mut c_void,
    callback: Box<Callback>,
    index: usize,
    /// Underruns (output) or overruns (input) seen so far.
    xruns: i32,
}

/// The most frames a stream may buffer: what the buffer grows to after
/// underruns (~170 ms at 48 kHz). A stream starts at two bursts, AAudio's
/// low-latency size, and grows by a burst at each underrun (the latency
/// tuning AAudio's own guide asks for): a phone whose app stalls the
/// callback now and then (a JIT compile, a busy core) settles at a size
/// that never runs dry, and a quiet one keeps its low latency.
const BUFFER_CAPACITY_FRAMES: i32 = 8192;

impl Stream {
    /// Grow the buffer after new xruns; logs each change.
    unsafe fn tune(&mut self) {
        let xruns = AAudioStream_getXRunCount(self.pointer);
        if xruns <= self.xruns {
            return;
        }
        let new = xruns - self.xruns;
        self.xruns = xruns;
        let burst = AAudioStream_getFramesPerBurst(self.pointer).max(1);
        let size = AAudioStream_getBufferSizeInFrames(self.pointer);
        let cap = AAudioStream_getBufferCapacityInFrames(self.pointer);
        let grown = if size + burst <= cap { AAudioStream_setBufferSizeInFrames(self.pointer, size + burst) } else { size };
        eprintln!(
            "[hal-aaudio] device {} {}: {new} new {} ({xruns} in all), buffer {size} -> {grown} frames (burst {burst}, capacity {cap})",
            self.callback.device,
            if self.callback.input { "input" } else { "output" },
            if self.callback.input { "overruns" } else { "underruns" },
        );
    }
}
impl Stream {
    unsafe fn open(device: &Device, index: usize, slot: Arc<Slot>) -> Result<Self, String> {
        let mut builder = std::ptr::null_mut();
        result("create builder", AAudio_createStreamBuilder(&mut builder))?;
        if builder.is_null() {
            return Err("AAudio returned a null builder".into());
        }
        let builder = Builder(builder);
        let input = device.desc.kind == 0;
        if device.native_id != 0 {
            AAudioStreamBuilder_setDeviceId(builder.0, device.native_id);
        }
        AAudioStreamBuilder_setDirection(builder.0, if input { 1 } else { 0 });
        AAudioStreamBuilder_setSharingMode(builder.0, 1); // SHARED
        AAudioStreamBuilder_setSampleRate(builder.0, 48_000);
        AAudioStreamBuilder_setChannelCount(builder.0, device.desc.channels as i32);
        AAudioStreamBuilder_setFormat(builder.0, 2); // PCM_FLOAT
        AAudioStreamBuilder_setBufferCapacityInFrames(builder.0, BUFFER_CAPACITY_FRAMES);
        AAudioStreamBuilder_setPerformanceMode(builder.0, 12); // LOW_LATENCY
        let mut callback = Box::new(Callback {
            device: device.desc.id,
            input,
            channels: 0,
            rate: 0.0,
            slot,
            error: AtomicI32::new(0),
            scratch: UnsafeCell::new(Scratch {
                samples: Vec::new(),
                planes: Vec::new(),
            }),
        });
        let context = (&mut *callback as *mut Callback).cast();
        AAudioStreamBuilder_setDataCallback(builder.0, data_callback, context);
        AAudioStreamBuilder_setErrorCallback(builder.0, error_callback, context);
        let mut pointer = std::ptr::null_mut();
        let opened = AAudioStreamBuilder_openStream(builder.0, &mut pointer);
        if pointer.is_null() {
            result("open", opened)?;
            return Err("AAudio returned a null stream".into());
        }
        let mut stream = Self {
            pointer,
            callback,
            index,
            xruns: 0,
        };
        result("open", opened)?;
        let channels = AAudioStream_getChannelCount(pointer);
        let rate = AAudioStream_getSampleRate(pointer);
        if channels <= 0 || rate <= 0 || AAudioStream_getFormat(pointer) != 2 {
            return Err("AAudio did not negotiate valid float PCM".into());
        }
        stream.callback.channels = channels as usize;
        stream.callback.rate = rate as f64;
        let scratch = stream.callback.scratch.get_mut();
        scratch.samples = vec![0.0; channels as usize * AUDIO_BLOCK_FRAMES];
        scratch.planes = (0..channels as usize)
            .map(|c| scratch.samples.as_mut_ptr().add(c * AUDIO_BLOCK_FRAMES))
            .collect();
        // Low latency to begin with: two bursts (see BUFFER_CAPACITY_FRAMES).
        let burst = AAudioStream_getFramesPerBurst(pointer);
        if burst > 0 {
            AAudioStream_setBufferSizeInFrames(pointer, 2 * burst);
        }
        result("start", AAudioStream_requestStart(pointer))?;
        Ok(stream)
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            // NDK stop/close may wait for the callback. This owner only drops on the worker;
            // close completes before the callback context and planar storage are destroyed.
            AAudioStream_requestStop(self.pointer);
            let code = AAudioStream_close(self.pointer);
            if code < 0 {
                eprintln!("[hal-aaudio] close: {code}");
            }
        }
    }
}
unsafe extern "C" fn error_callback(_: *mut c_void, context: *mut c_void, error: i32) {
    let callback = &*context.cast::<Callback>();
    callback.error.store(error, Ordering::Release);
}
unsafe extern "C" fn data_callback(
    _: *mut c_void,
    context: *mut c_void,
    data: *mut c_void,
    frames: i32,
) -> i32 {
    let callback = &*context.cast::<Callback>();
    if frames <= 0 {
        return 0;
    }
    if data.is_null() || callback.channels == 0 {
        return 1;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let scratch = &mut *callback.scratch.get();
        let interleaved =
            std::slice::from_raw_parts_mut(data.cast::<f32>(), frames as usize * callback.channels);
        for start in (0..frames as usize).step_by(AUDIO_BLOCK_FRAMES) {
            let count = (frames as usize - start).min(AUDIO_BLOCK_FRAMES);
            for channel in 0..callback.channels {
                let plane = &mut scratch.samples
                    [channel * AUDIO_BLOCK_FRAMES..channel * AUDIO_BLOCK_FRAMES + count];
                if callback.input {
                    for (i, sample) in plane.iter_mut().enumerate() {
                        *sample = interleaved[(start + i) * callback.channels + channel];
                    }
                } else {
                    plane.fill(0.0);
                }
            }
            let mut block = AudioBlock {
                device: callback.device,
                sample_rate: callback.rate,
                sample_time: 0.0,
                host_time: 0,
                rate_scalar: 1.0,
                has_time: 0,
                frames: count as u32,
                channels: callback.channels as u32,
                planes: scratch.planes.as_mut_ptr(),
            };
            callback.slot.call(&mut block);
            if !callback.input {
                for channel in 0..callback.channels {
                    for i in 0..count {
                        interleaved[(start + i) * callback.channels + channel] =
                            scratch.samples[channel * AUDIO_BLOCK_FRAMES + i];
                    }
                }
            }
        }
    }));
    if result.is_err() {
        if !callback.input {
            std::slice::from_raw_parts_mut(data.cast::<f32>(), frames as usize * callback.channels)
                .fill(0.0);
        }
        callback.error.store(-896, Ordering::Release); // INTERNAL; reported by control worker
        1 // STOP
    } else {
        0
    } // CONTINUE
}

#[derive(Clone, PartialEq)]
struct Device {
    native_id: i32,
    desc: AudioDeviceInfo,
}

fn device_type(kind: u32) -> &'static str {
    match kind {
        1 => "Built-in Earpiece",
        2 => "Built-in Speaker",
        3 => "Wired Headset",
        4 => "Wired Headphones",
        5 => "Line Analog",
        6 => "Line Digital",
        7 => "Bluetooth SCO",
        8 => "Bluetooth A2DP",
        9 => "HDMI",
        10 => "HDMI ARC",
        11 => "USB Device",
        12 => "USB Accessory",
        13 => "Dock",
        14 => "FM",
        15 => "Built-in Mic",
        16 => "FM Tuner",
        17 => "TV Tuner",
        18 => "Telephony",
        19 => "Aux Line",
        20 => "IP",
        21 => "Bus",
        22 => "Usb Headset",
        23 => "Hearing Aid",
        24 => "Built-in Speaker Safe",
        25 => "Remote Submix",
        26 => "BLE Headset",
        27 => "BLE Speaker",
        28 => "Echo Reference",
        29 => "HDMI EARC",
        30 => "BLE Broadcast",
        _ => "Unknown",
    }
}
fn enumerate() -> Result<Vec<Device>, String> {
    jhelp::activity(|env, activity| unsafe {
        let method = jhelp::method(env, activity, c"getAudioDevices", c"(J)[Ljava/lang/String;")?;
        let mut devices = Vec::new();
        for (kind, name) in [
            (0, "Default input 2 channels"),
            (1, "Default output 2 channels"),
        ] {
            devices.push(Device {
                native_id: 0,
                desc: AudioDeviceInfo {
                    id: name_hash(name),
                    kind,
                    is_default: true,
                    has_failed: false,
                    channels: 2,
                    name: name.into(),
                },
            });
        }
        for kind in 0..2 {
            let array = ((**env).CallObjectMethodA.unwrap())(
                env,
                activity,
                method,
                [jvalue { j: kind as i64 }].as_ptr(),
            );
            jhelp::check(env)?;
            if array.is_null() {
                continue;
            }
            let len = ((**env).GetArrayLength.unwrap())(env, array);
            for i in 0..len {
                let item = ((**env).GetObjectArrayElement.unwrap())(env, array, i);
                jhelp::check(env)?;
                let value = reg::string(env, item);
                ((**env).DeleteLocalRef.unwrap())(env, item);
                jhelp::check(env)?;
                let parts: Vec<_> = value.splitn(4, "$$").collect();
                if parts.len() != 4 {
                    continue;
                }
                let (Ok(native_id), Ok(ty), Ok(channels)) = (
                    parts[0].parse::<i32>(),
                    parts[1].parse::<u32>(),
                    parts[2].parse::<u32>(),
                ) else {
                    continue;
                };
                if channels == 0 || channels > i32::MAX as u32 {
                    continue;
                }
                let name = format!("{} {} {} channels", parts[3], device_type(ty), channels);
                devices.push(Device {
                    native_id,
                    desc: AudioDeviceInfo {
                        id: name_hash(&name),
                        kind,
                        is_default: false,
                        has_failed: false,
                        channels,
                        name,
                    },
                });
            }
            ((**env).DeleteLocalRef.unwrap())(env, array);
        }
        Ok(devices)
    })
}

pub struct Audio {
    devices: Vec<Device>,
    streams: [Vec<Stream>; 2],
    wanted: [Vec<u64>; 2],
    entries: Entries,
    failed: HashSet<(usize, u64)>,
    reconnect: HashSet<(usize, u64)>,
    enumerated: Instant,
    published: Option<Vec<AudioDeviceInfo>>,
    dirty: bool,
}
impl Audio {
    pub fn active(&self) -> bool {
        self.streams.iter().any(|streams| !streams.is_empty())
    }
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            streams: Default::default(),
            wanted: Default::default(),
            entries: Entries::default(),
            failed: HashSet::new(),
            reconnect: HashSet::new(),
            enumerated: Instant::now(),
            published: None,
            dirty: true,
        }
    }
    pub fn configure(&mut self, direction: Option<usize>, devices: Vec<u64>, entries: Entries) {
        self.entries = entries;
        if self.devices.is_empty() {
            self.refresh();
        }
        if let Some(direction) = direction {
            self.wanted[direction] = devices;
            self.failed.retain(|(dir, _)| *dir != direction);
            self.dirty = true;
        }
        self.apply();
    }
    fn refresh(&mut self) {
        self.enumerated = Instant::now();
        match enumerate() {
            Ok(devices) => {
                if devices != self.devices {
                    self.failed.clear();
                    self.dirty = true;
                }
                // A device id can persist while its Android routing id changes.
                for (dir, streams) in self.streams.iter_mut().enumerate() {
                    streams.retain(|stream| {
                        self.devices
                            .iter()
                            .find(|d| {
                                d.desc.kind as usize == dir && d.desc.id == stream.callback.device
                            })
                            .is_some_and(|old| devices.contains(old))
                    });
                }
                self.devices = devices;
            }
            Err(error) => eprintln!("[hal-aaudio] enumerate: {error}"),
        }
    }
    fn apply(&mut self) {
        for direction in 0..2 {
            self.streams[direction].retain(|stream| {
                self.wanted[direction].get(stream.index) == Some(&stream.callback.device)
            });
            for (index, id) in self.wanted[direction].iter().copied().enumerate() {
                if self.streams[direction].iter().any(|s| s.index == index)
                    || self.failed.contains(&(direction, id))
                {
                    continue;
                }
                let Some(device) = self
                    .devices
                    .iter()
                    .find(|d| d.desc.kind as usize == direction && d.desc.id == id)
                else {
                    continue;
                };
                let slot = self.entries.slot(direction, index);
                match unsafe { Stream::open(device, index, slot) } {
                    Ok(stream) => {
                        self.streams[direction].push(stream);
                    }
                    Err(error) => {
                        eprintln!("[hal-aaudio] {}: {error}", device.desc.name);
                        self.failed.insert((direction, id));
                        self.dirty = true;
                    }
                }
            }
        }
    }
    pub fn poll(&mut self) -> Option<Vec<AudioDeviceInfo>> {
        for stream in self.streams.iter_mut().flatten() {
            unsafe { stream.tune() };
        }
        for (direction, streams) in self.streams.iter_mut().enumerate() {
            streams.retain(|stream| {
                let error = stream.callback.error.load(Ordering::Acquire);
                if error == 0 {
                    return true;
                }
                eprintln!(
                    "[hal-aaudio] device {} disconnected/failed: {error}",
                    stream.callback.device
                );
                self.failed.insert((direction, stream.callback.device));
                if error == -899 {
                    // AAudio DISCONNECTED also occurs when a default route changes but
                    // the Java device list has not changed yet. Reopen on the next scan.
                    self.reconnect.insert((direction, stream.callback.device));
                }
                self.dirty = true;
                false
            });
        }
        if self.enumerated.elapsed() >= Duration::from_secs(1) {
            self.refresh();
            for device in self.reconnect.drain() {
                self.failed.remove(&device);
                self.dirty = true;
            }
            self.apply();
        }
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        let devices: Vec<_> = self
            .devices
            .iter()
            .map(|d| {
                let mut desc = d.desc.clone();
                desc.has_failed = self.failed.contains(&(desc.kind as usize, desc.id));
                desc
            })
            .collect();
        if self.published.as_ref() == Some(&devices) {
            return None;
        }
        self.published = Some(devices.clone());
        Some(devices)
    }
}
