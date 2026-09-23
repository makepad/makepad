//! Core Audio process taps. One private tap per route, played through a
//! private aggregate whose clock is the chosen output device.

use crate::{AudioProcess, Error, FrameInfo, Mute, OutputDevice, Processor, Result, RouteConfig, Source};
use makepad_objc_sys::rc::autoreleasepool;
use makepad_objc_sys::runtime::{ObjcId, BOOL, YES};
use makepad_objc_sys::{class, msg_send, sel, sel_impl};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::Mutex;

const SYSTEM: u32 = 1;
const GLOBAL: u32 = fourcc(*b"glob");
const OUTPUT: u32 = fourcc(*b"outp");
const MAX_CHANNELS: usize = 8;
const MAX_FRAMES: usize = 16_384;

const fn fourcc(bytes: [u8; 4]) -> u32 {
    ((bytes[0] as u32) << 24) | ((bytes[1] as u32) << 16) | ((bytes[2] as u32) << 8) | (bytes[3] as u32)
}

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    _pad: u32,
    buffers: [AudioBuffer; 1],
}

#[repr(C)]
struct AudioTimeStamp {
    _sample_time: f64,
    host_time: u64,
}

#[repr(C)]
struct StreamFormat {
    _sample_rate: f64,
    _format_id: u32,
    _format_flags: u32,
    _bytes_per_packet: u32,
    _frames_per_packet: u32,
    _bytes_per_frame: u32,
    channels_per_frame: u32,
    _bits_per_channel: u32,
    _reserved: u32,
}

#[repr(C)]
struct ValueRange {
    minimum: f64,
    maximum: f64,
}

type IoProc = unsafe extern "C" fn(
    u32,
    *const AudioTimeStamp,
    *const AudioBufferList,
    *const AudioTimeStamp,
    *mut AudioBufferList,
    *const AudioTimeStamp,
    *mut c_void,
) -> i32;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioHardwareCreateProcessTap(description: ObjcId, tap: *mut u32) -> i32;
    fn AudioHardwareDestroyProcessTap(tap: u32) -> i32;
    fn AudioHardwareCreateAggregateDevice(description: ObjcId, device: *mut u32) -> i32;
    fn AudioHardwareDestroyAggregateDevice(device: u32) -> i32;
    fn AudioDeviceCreateIOProcID(device: u32, proc_: IoProc, client: *mut c_void, out_proc: *mut *mut c_void) -> i32;
    fn AudioDeviceDestroyIOProcID(device: u32, proc_: *mut c_void) -> i32;
    fn AudioDeviceStart(device: u32, proc_: *mut c_void) -> i32;
    fn AudioDeviceStop(device: u32, proc_: *mut c_void) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringGetCString(string: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    // Keeps AppKit linked so NSRunningApplication is registered.
    fn NSApplicationLoad() -> BOOL;
}

struct ProcSlot {
    processor: Box<dyn Processor>,
}

struct Shared {
    processor: AtomicPtr<ProcSlot>,
    epoch: AtomicU64,
    sample_rate: f64,
    channels: u16,
    scratch_frames: usize,
}

struct Callback {
    shared: *const Shared,
    scratch: Vec<f32>,
}

pub struct Route {
    aggregate: u32,
    proc_id: *mut c_void,
    tap: u32,
    description: ObjcId,
    shared: Box<Shared>,
    /// The HAL calls back into this allocation. It stays here so the pointer
    /// handed to `AudioDeviceCreateIOProcID` remains valid.
    #[allow(dead_code)]
    callback: Box<Callback>,
    retired: Mutex<Vec<(u64, *mut ProcSlot)>>,
}

unsafe impl Send for Route {}
unsafe impl Sync for Route {}

impl Drop for Route {
    fn drop(&mut self) {
        unsafe {
            if self.aggregate != 0 && !self.proc_id.is_null() {
                AudioDeviceStop(self.aggregate, self.proc_id);
                AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
            }
            if self.aggregate != 0 {
                AudioHardwareDestroyAggregateDevice(self.aggregate);
            }
            if self.tap != 0 {
                AudioHardwareDestroyProcessTap(self.tap);
            }
            release(self.description);
        }
        self.proc_id = ptr::null_mut();
        self.aggregate = 0;
        self.tap = 0;
        self.description = ptr::null_mut();
        let current = self.shared.processor.swap(ptr::null_mut(), Ordering::AcqRel);
        drop_slot(current);
        if let Ok(mut retired) = self.retired.lock() {
            for (_, slot) in retired.drain(..) {
                drop_slot(slot);
            }
        }
    }
}

impl Route {
    pub fn open(config: RouteConfig, processor: Box<dyn Processor>) -> Result<Self> {
        let _ = unsafe { NSApplicationLoad() };
        autoreleasepool(|| open_route(config, processor))
    }

    pub fn set_processor(&self, processor: Box<dyn Processor>) {
        let slot = Box::into_raw(Box::new(ProcSlot { processor }));
        let previous = self.shared.processor.swap(slot, Ordering::AcqRel);
        // The epoch is read after the swap: only a block already under way
        // can still hold `previous`, and it bumps the epoch past this when it
        // ends. Read before the swap, a block starting in between could load
        // `previous` after an older block's bump let it go.
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        if let Ok(mut retired) = self.retired.lock() {
            retired.push((epoch, previous));
        }
        self.reclaim();
    }

    /// Drops processors replaced by [`set_processor`] once the audio thread
    /// has finished the block that might still be using them.
    pub fn reclaim(&self) {
        let seen = self.shared.epoch.load(Ordering::Acquire);
        let Ok(mut retired) = self.retired.lock() else { return };
        let mut i = 0;
        while i < retired.len() {
            if seen > retired[i].0 {
                let (_, slot) = retired.swap_remove(i);
                drop_slot(slot);
            } else {
                i += 1;
            }
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.shared.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.shared.channels
    }
}

pub fn processes() -> Result<Vec<AudioProcess>> {
    let _ = unsafe { NSApplicationLoad() };
    autoreleasepool(|| list_processes())
}

pub fn outputs() -> Result<Vec<OutputDevice>> {
    autoreleasepool(|| list_outputs())
}

pub fn default_output() -> Result<OutputDevice> {
    autoreleasepool(|| {
        let id = default_output_device()?;
        device_info(id)
    })
}

fn open_route(config: RouteConfig, processor: Box<dyn Processor>) -> Result<Route> {
    if config.sources.is_empty() {
        return Err(Error::NotFound);
    }
    let (object_ids, bundles) = resolve_sources(&config.sources)?;
    let description = tap_description(&object_ids, &bundles, config.mute)?;
    let mut tap = 0u32;
    let status = unsafe { AudioHardwareCreateProcessTap(description, &mut tap) };
    if let Err(error) = check(status, "create tap") {
        release(description);
        return Err(error);
    }
    let tap_uid = match cfstring_property(tap, fourcc(*b"tuid"), GLOBAL, "tap uid") {
        Ok(uid) => uid,
        Err(error) => {
            unsafe { AudioHardwareDestroyProcessTap(tap) };
            release(description);
            return Err(error);
        }
    };
    let output_id = match &config.output {
        Some(uid) => device_for_uid(uid),
        None => default_output_device(),
    };
    let output_id = match output_id {
        Ok(id) => id,
        Err(error) => {
            unsafe { AudioHardwareDestroyProcessTap(tap) };
            release(description);
            return Err(error);
        }
    };
    let output_uid = match cfstring_property(output_id, fourcc(*b"uid "), GLOBAL, "output uid") {
        Ok(uid) => uid,
        Err(error) => {
            unsafe { AudioHardwareDestroyProcessTap(tap) };
            release(description);
            return Err(error);
        }
    };
    let aggregate = match aggregate_device(&tap_uid, &output_uid) {
        Ok(id) => id,
        Err(error) => {
            unsafe { AudioHardwareDestroyProcessTap(tap) };
            release(description);
            return Err(error);
        }
    };

    let channels = tap_channels(tap).clamp(1, MAX_CHANNELS as u32) as u16;
    let scratch_frames = buffer_cap(aggregate);
    let sample_rate = f64_property(aggregate, fourcc(*b"nsrt"), GLOBAL).unwrap_or(48_000.0);
    let shared = Box::new(Shared {
        processor: AtomicPtr::new(Box::into_raw(Box::new(ProcSlot { processor }))),
        epoch: AtomicU64::new(0),
        sample_rate,
        channels,
        scratch_frames,
    });
    let callback = Box::new(Callback {
        shared: &*shared as *const Shared,
        scratch: vec![0.0; scratch_frames * channels as usize],
    });
    let client = &*callback as *const Callback as *mut c_void;
    let mut proc_id = ptr::null_mut();
    let status = unsafe { AudioDeviceCreateIOProcID(aggregate, io_proc, client, &mut proc_id) };
    if let Err(error) = check(status, "create io proc") {
        drop_slot(shared.processor.load(Ordering::Acquire));
        unsafe { AudioHardwareDestroyAggregateDevice(aggregate) };
        unsafe { AudioHardwareDestroyProcessTap(tap) };
        release(description);
        return Err(error);
    }
    let status = unsafe { AudioDeviceStart(aggregate, proc_id) };
    if let Err(error) = check(status, "start route") {
        unsafe { AudioDeviceDestroyIOProcID(aggregate, proc_id) };
        drop_slot(shared.processor.load(Ordering::Acquire));
        unsafe { AudioHardwareDestroyAggregateDevice(aggregate) };
        unsafe { AudioHardwareDestroyProcessTap(tap) };
        release(description);
        return Err(error);
    }
    Ok(Route {
        aggregate,
        proc_id,
        tap,
        description,
        shared,
        callback,
        retired: Mutex::new(Vec::new()),
    })
}

unsafe extern "C" fn io_proc(
    _device: u32,
    now: *const AudioTimeStamp,
    input: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    output: *mut AudioBufferList,
    _output_time: *const AudioTimeStamp,
    client: *mut c_void,
) -> i32 {
    if client.is_null() || output.is_null() {
        return 0;
    }
    let callback = &mut *(client as *mut Callback);
    let shared = &*callback.shared;
    let channels = shared.channels as usize;
    let frames = output_frames(output);
    if frames == 0 || channels == 0 || frames > shared.scratch_frames {
        shared.epoch.fetch_add(1, Ordering::Release);
        return 0;
    }
    let samples = frames * channels;
    let scratch = &mut callback.scratch[..samples];
    read_interleaved(input, channels, frames, scratch);
    let info = FrameInfo {
        sample_rate: shared.sample_rate,
        channels: shared.channels,
        frames,
        host_time: if now.is_null() { 0 } else { (*now).host_time },
    };
    let slot = shared.processor.load(Ordering::Acquire);
    if !slot.is_null() {
        (*slot).processor.process(scratch, &info);
    }
    write_interleaved(output, channels, frames, scratch);
    shared.epoch.fetch_add(1, Ordering::Release);
    0
}

fn resolve_sources(sources: &[Source]) -> Result<(Vec<u32>, Vec<String>)> {
    let alive = list_processes().unwrap_or_default();
    let mut ids = Vec::new();
    let mut bundles = Vec::new();
    for source in sources {
        match source {
            Source::Pid(pid) => {
                if let Some(object) = translate_pid(*pid) {
                    ids.push(object);
                }
                if let Some(process) = alive.iter().find(|process| process.pid == *pid) {
                    if !process.bundle_id.is_empty() {
                        bundles.push(process.bundle_id.clone());
                    }
                }
            }
            Source::BundleId(bundle) => {
                bundles.push(bundle.clone());
                for process in alive.iter().filter(|process| process.bundle_id == *bundle) {
                    if let Some(object) = translate_pid(process.pid) {
                        ids.push(object);
                    }
                }
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    bundles.sort();
    bundles.dedup();
    if ids.is_empty() && (bundles.is_empty() || !instances_respond(sel!(setBundleIDs:))) {
        return Err(Error::NotFound);
    }
    Ok((ids, bundles))
}

fn tap_description(object_ids: &[u32], bundles: &[String], mute: Mute) -> Result<ObjcId> {
    let numbers: Vec<ObjcId> = object_ids.iter().copied().map(ns_u32).collect();
    let processes = ns_array(&numbers);
    let allocated: ObjcId = unsafe { msg_send![class!(CATapDescription), alloc] };
    let description: ObjcId = unsafe { msg_send![allocated, initStereoMixdownOfProcesses: processes] };
    if description.is_null() {
        release(processes);
        return Err(Error::System { status: -1, step: "tap description" });
    }
    let behavior: isize = match mute {
        Mute::HearOriginal => 0,
        Mute::Replace => 1,
        Mute::ReplaceWhileRouted => 2,
    };
    let () = unsafe { msg_send![description, setMuteBehavior: behavior] };
    let () = unsafe { msg_send![description, setPrivate: YES] };
    let name = ns_string("audio-route");
    let () = unsafe { msg_send![description, setName: name] };
    release(name);
    if !bundles.is_empty() && instances_respond(sel!(setBundleIDs:)) {
        let owned: Vec<ObjcId> = bundles.iter().map(|bundle| ns_string(bundle)).collect();
        let array = ns_array(&owned);
        let () = unsafe { msg_send![description, setBundleIDs: array] };
        let () = unsafe { msg_send![description, setProcessRestoreEnabled: YES] };
        for item in owned {
            release(item);
        }
        release(array);
    }
    release(processes);
    Ok(description)
}

fn aggregate_device(tap_uid: &str, output_uid: &str) -> Result<u32> {
    let tap_uid_s = ns_string(tap_uid);
    let output_uid_s = ns_string(output_uid);
    let tap = ns_dictionary(&[("uid", tap_uid_s), ("drift", ns_i32(1))]);
    let sub = ns_dictionary(&[("uid", output_uid_s)]);
    release(tap_uid_s);
    release(output_uid_s);
    let taps = ns_array(&[tap]);
    let subs = ns_array(&[sub]);
    release(tap);
    release(sub);
    let uid = ns_string(&format!("audio-route.{tap_uid}"));
    let name = ns_string("audio-route");
    let master = ns_string(output_uid);
    let dict = ns_dictionary(&[
        ("name", name),
        ("uid", uid),
        ("private", ns_i32(1)),
        ("tapautostart", ns_i32(1)),
        ("master", master),
        ("taps", taps),
        ("subdevices", subs),
    ]);
    release(name);
    release(uid);
    release(master);
    release(taps);
    release(subs);
    let mut device = 0u32;
    let status = unsafe { AudioHardwareCreateAggregateDevice(dict, &mut device) };
    release(dict);
    check(status, "create aggregate")?;
    if device == 0 {
        return Err(Error::System { status: -1, step: "create aggregate" });
    }
    Ok(device)
}

fn list_processes() -> Result<Vec<AudioProcess>> {
    let ids = u32_list(SYSTEM, fourcc(*b"prs#"), GLOBAL, &[])?;
    let mut processes = Vec::with_capacity(ids.len());
    for object in ids {
        let Ok(pid) = u32_property(object, fourcc(*b"ppid"), GLOBAL) else { continue };
        let bundle_id = cfstring_property(object, fourcc(*b"pbid"), GLOBAL, "bundle id").unwrap_or_default();
        let mut name = process_name(pid as i32);
        if name.is_empty() {
            name = if bundle_id.is_empty() { format!("pid {pid}") } else { bundle_id.clone() };
        }
        processes.push(AudioProcess {
            pid,
            bundle_id,
            name,
            output_running: u32_property(object, fourcc(*b"piro"), GLOBAL).unwrap_or(0) != 0,
            input_running: u32_property(object, fourcc(*b"piri"), GLOBAL).unwrap_or(0) != 0,
        });
    }
    processes.sort_by(|a, b| {
        b.output_running.cmp(&a.output_running).then_with(|| a.name.cmp(&b.name)).then(a.pid.cmp(&b.pid))
    });
    Ok(processes)
}

fn list_outputs() -> Result<Vec<OutputDevice>> {
    let ids = u32_list(SYSTEM, fourcc(*b"dev#"), GLOBAL, &[])?;
    let mut devices = Vec::new();
    for id in ids {
        if output_channels(id) == 0 {
            continue;
        }
        if let Ok(info) = device_info(id) {
            devices.push(info);
        }
    }
    devices.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(devices)
}

fn device_info(id: u32) -> Result<OutputDevice> {
    Ok(OutputDevice {
        uid: cfstring_property(id, fourcc(*b"uid "), GLOBAL, "device uid")?,
        name: cfstring_property(id, fourcc(*b"lnam"), GLOBAL, "device name").unwrap_or_default(),
        channels: output_channels(id) as u16,
        sample_rate: f64_property(id, fourcc(*b"nsrt"), GLOBAL).unwrap_or(0.0),
    })
}

fn default_output_device() -> Result<u32> {
    let id = u32_property(SYSTEM, fourcc(*b"dOut"), GLOBAL).map_err(|_| Error::NotFound)?;
    if id == 0 { Err(Error::NotFound) } else { Ok(id) }
}

fn device_for_uid(uid: &str) -> Result<u32> {
    let ns = ns_string(uid);
    let cf = ns as *const c_void;
    let id = u32_property_qualified(SYSTEM, fourcc(*b"uidd"), GLOBAL, &cf as *const *const c_void as *const c_void, std::mem::size_of::<*const c_void>() as u32);
    release(ns);
    match id {
        Ok(id) if id != 0 => Ok(id),
        _ => Err(Error::NotFound),
    }
}

fn translate_pid(pid: u32) -> Option<u32> {
    let pid = pid as i32;
    u32_property_qualified(SYSTEM, fourcc(*b"id2p"), GLOBAL, &pid as *const i32 as *const c_void, std::mem::size_of::<i32>() as u32)
        .ok()
        .filter(|id| *id != 0)
}

fn tap_channels(tap: u32) -> u32 {
    let mut format = StreamFormat {
        _sample_rate: 0.0,
        _format_id: 0,
        _format_flags: 0,
        _bytes_per_packet: 0,
        _frames_per_packet: 0,
        _bytes_per_frame: 0,
        channels_per_frame: 2,
        _bits_per_channel: 0,
        _reserved: 0,
    };
    let mut size = std::mem::size_of::<StreamFormat>() as u32;
    let address = address(fourcc(*b"tfmt"), GLOBAL);
    let status = unsafe {
        AudioObjectGetPropertyData(tap, &address, 0, ptr::null(), &mut size, &mut format as *mut StreamFormat as *mut c_void)
    };
    if status != 0 || format.channels_per_frame == 0 { 2 } else { format.channels_per_frame }
}

fn buffer_cap(device: u32) -> usize {
    let frame = u32_property(device, fourcc(*b"fsiz"), GLOBAL).unwrap_or(512) as usize;
    let variable = u32_property(device, fourcc(*b"vfsz"), OUTPUT).unwrap_or(0) as usize;
    let mut range = ValueRange { minimum: 0.0, maximum: 0.0 };
    let mut size = std::mem::size_of::<ValueRange>() as u32;
    let address = address(fourcc(*b"fsz#"), GLOBAL);
    let status = unsafe {
        AudioObjectGetPropertyData(device, &address, 0, ptr::null(), &mut size, &mut range as *mut ValueRange as *mut c_void)
    };
    let max = if status == 0 { range.maximum as usize } else { 0 };
    frame.max(variable).max(max).clamp(256, MAX_FRAMES)
}

fn output_channels(device: u32) -> usize {
    stream_channels(device, OUTPUT)
}

fn stream_channels(device: u32, scope: u32) -> usize {
    let Ok(words) = property_words(device, fourcc(*b"slay"), scope, &[]) else { return 0 };
    if words.is_empty() {
        return 0;
    }
    let list = words.as_ptr() as *const AudioBufferList;
    buffers_of(list).iter().map(|buffer| buffer.number_channels as usize).sum()
}

fn output_frames(list: *const AudioBufferList) -> usize {
    buffers_of(list).iter().find_map(|buffer| {
        let frames = buffer_frames(buffer);
        if frames > 0 { Some(frames) } else { None }
    }).unwrap_or(0)
}

fn buffer_frames(buffer: &AudioBuffer) -> usize {
    let channels = (buffer.number_channels as usize).max(1);
    let bytes = buffer.data_byte_size as usize;
    if buffer.data.is_null() || bytes < 4 || bytes % 4 != 0 {
        return 0;
    }
    let samples = bytes / 4;
    if buffer.number_channels <= 1 { samples } else { samples / channels }
}

fn read_interleaved(list: *const AudioBufferList, channels: usize, frames: usize, dst: &mut [f32]) {
    dst.fill(0.0);
    if list.is_null() || channels == 0 || frames == 0 {
        return;
    }
    // The aggregate's input list is the output device's own inputs, then the
    // tap. The tap is the last buffer whose width matches the tap.
    let buffers = buffers_of(list);
    let Some(buffer) = buffers
        .iter()
        .rev()
        .find(|buffer| !buffer.data.is_null() && buffer.number_channels as usize == channels)
        .or_else(|| buffers.iter().rev().find(|buffer| !buffer.data.is_null()))
    else {
        return;
    };
    let ch = buffer.number_channels as usize;
    let count = frames.min(buffer_frames(buffer));
    if count == 0 || ch == 0 {
        return;
    }
    let src = buffer.data as *const f32;
    let take = ch.min(channels);
    for frame in 0..count {
        for channel in 0..take {
            dst[frame * channels + channel] = unsafe { *src.add(frame * ch + channel) };
        }
    }
}

fn write_interleaved(list: *mut AudioBufferList, src_channels: usize, frames: usize, src: &[f32]) {
    if list.is_null() || src_channels == 0 {
        return;
    }
    let buffers = buffers_of_mut(list);
    let total: usize = buffers.iter().filter(|buffer| !buffer.data.is_null()).map(|buffer| (buffer.number_channels as usize).max(1)).sum();
    if total == 0 {
        return;
    }
    let mut index = 0usize;
    for buffer in buffers {
        if buffer.data.is_null() {
            continue;
        }
        let ch = (buffer.number_channels as usize).max(1);
        let count = frames.min(buffer_frames(buffer));
        let dst = buffer.data as *mut f32;
        if total == 1 && src_channels > 1 {
            for frame in 0..count {
                let left = sample_at(src, src_channels, frame, 0);
                let right = sample_at(src, src_channels, frame, 1);
                unsafe { *dst.add(frame) = 0.5 * (left + right) };
            }
            return;
        }
        if ch == 1 {
            for frame in 0..count {
                unsafe { *dst.add(frame) = mapped_sample(src, src_channels, frame, index) };
            }
            index += 1;
        } else {
            for channel in 0..ch {
                for frame in 0..count {
                    unsafe { *dst.add(frame * ch + channel) = mapped_sample(src, src_channels, frame, index + channel) };
                }
            }
            index += ch;
        }
    }
}

fn mapped_sample(src: &[f32], src_channels: usize, frame: usize, out_index: usize) -> f32 {
    sample_at(src, src_channels, frame, out_index % src_channels)
}

fn sample_at(src: &[f32], channels: usize, frame: usize, channel: usize) -> f32 {
    let index = frame * channels + channel;
    if index < src.len() { src[index] } else { 0.0 }
}

fn buffers_of(list: *const AudioBufferList) -> &'static [AudioBuffer] {
    if list.is_null() {
        return &[];
    }
    let count = unsafe { (*list).number_buffers as usize };
    if count == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts((list as *const u8).add(8) as *const AudioBuffer, count) }
}

fn buffers_of_mut(list: *mut AudioBufferList) -> &'static mut [AudioBuffer] {
    if list.is_null() {
        return &mut [];
    }
    let count = unsafe { (*list).number_buffers as usize };
    if count == 0 {
        return &mut [];
    }
    unsafe { std::slice::from_raw_parts_mut((list as *mut u8).add(8) as *mut AudioBuffer, count) }
}

fn address(selector: u32, scope: u32) -> PropertyAddress {
    PropertyAddress { selector, scope, element: 0 }
}

fn u32_property(object: u32, selector: u32, scope: u32) -> Result<u32> {
    u32_property_qualified(object, selector, scope, ptr::null(), 0)
}

fn u32_property_qualified(object: u32, selector: u32, scope: u32, qualifier: *const c_void, qualifier_size: u32) -> Result<u32> {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let address = address(selector, scope);
    let status = unsafe { AudioObjectGetPropertyData(object, &address, qualifier_size, qualifier, &mut size, &mut value as *mut u32 as *mut c_void) };
    check(status, "property")?;
    Ok(value)
}

fn f64_property(object: u32, selector: u32, scope: u32) -> Result<f64> {
    let mut value = 0f64;
    let mut size = std::mem::size_of::<f64>() as u32;
    let address = address(selector, scope);
    let status = unsafe { AudioObjectGetPropertyData(object, &address, 0, ptr::null(), &mut size, &mut value as *mut f64 as *mut c_void) };
    check(status, "sample rate")?;
    Ok(value)
}

fn cfstring_property(object: u32, selector: u32, scope: u32, step: &'static str) -> Result<String> {
    let mut string: *const c_void = ptr::null();
    let mut size = std::mem::size_of::<*const c_void>() as u32;
    let address = address(selector, scope);
    let status = unsafe { AudioObjectGetPropertyData(object, &address, 0, ptr::null(), &mut size, &mut string as *mut *const c_void as *mut c_void) };
    check(status, step)?;
    Ok(cf_to_string(string))
}

fn u32_list(object: u32, selector: u32, scope: u32, qualifier: &[u8]) -> Result<Vec<u32>> {
    let address = address(selector, scope);
    let qptr = if qualifier.is_empty() { ptr::null() } else { qualifier.as_ptr() as *const c_void };
    let mut size = 0u32;
    let status = unsafe { AudioObjectGetPropertyDataSize(object, &address, qualifier.len() as u32, qptr, &mut size) };
    check(status, "property size")?;
    let count = (size as usize) / std::mem::size_of::<u32>();
    let mut data = vec![0u32; count];
    if count == 0 {
        return Ok(data);
    }
    let status = unsafe {
        AudioObjectGetPropertyData(object, &address, qualifier.len() as u32, qptr, &mut size, data.as_mut_ptr() as *mut c_void)
    };
    check(status, "property")?;
    let count = (size as usize) / std::mem::size_of::<u32>();
    data.truncate(count);
    Ok(data)
}

fn property_words(object: u32, selector: u32, scope: u32, qualifier: &[u8]) -> Result<Vec<u64>> {
    let address = address(selector, scope);
    let qptr = if qualifier.is_empty() { ptr::null() } else { qualifier.as_ptr() as *const c_void };
    let mut size = 0u32;
    let status = unsafe { AudioObjectGetPropertyDataSize(object, &address, qualifier.len() as u32, qptr, &mut size) };
    check(status, "property size")?;
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut data = vec![0u64; (size as usize).div_ceil(8)];
    let status = unsafe { AudioObjectGetPropertyData(object, &address, qualifier.len() as u32, qptr, &mut size, data.as_mut_ptr() as *mut c_void) };
    check(status, "property")?;
    let words = (size as usize).div_ceil(8).min(data.len());
    data.truncate(words);
    Ok(data)
}

fn cf_to_string(string: *const c_void) -> String {
    if string.is_null() {
        return String::new();
    }
    let mut buffer = [0u8; 1024];
    let ok = unsafe { CFStringGetCString(string, buffer.as_mut_ptr() as *mut c_char, buffer.len() as isize, 0x0800_0100) };
    let text = if ok {
        unsafe { CStr::from_ptr(buffer.as_ptr() as *const c_char) }.to_string_lossy().into_owned()
    } else {
        String::new()
    };
    unsafe { CFRelease(string) };
    text
}

fn process_name(pid: i32) -> String {
    let app: ObjcId = unsafe { msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid] };
    if app.is_null() {
        return String::new();
    }
    let name: ObjcId = unsafe { msg_send![app, localizedName] };
    ns_to_string(name)
}

fn ns_to_string(string: ObjcId) -> String {
    if string.is_null() {
        return String::new();
    }
    let utf8: *const c_char = unsafe { msg_send![string, UTF8String] };
    if utf8.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(utf8) }.to_string_lossy().into_owned()
}

fn ns_string(text: &str) -> ObjcId {
    let cstring = CString::new(text).unwrap_or_else(|_| CString::new("").unwrap());
    let string: ObjcId = unsafe { msg_send![class!(NSString), alloc] };
    unsafe { msg_send![string, initWithUTF8String: cstring.as_ptr()] }
}

fn ns_u32(value: u32) -> ObjcId {
    unsafe { msg_send![class!(NSNumber), numberWithUnsignedInt: value] }
}

fn ns_i32(value: i32) -> ObjcId {
    unsafe { msg_send![class!(NSNumber), numberWithInt: value] }
}

fn ns_array(items: &[ObjcId]) -> ObjcId {
    let array: ObjcId = unsafe { msg_send![class!(NSArray), alloc] };
    let ptr = if items.is_empty() { ptr::null() } else { items.as_ptr() };
    unsafe { msg_send![array, initWithObjects: ptr count: items.len()] }
}

fn ns_dictionary(pairs: &[(&str, ObjcId)]) -> ObjcId {
    let dict: ObjcId = unsafe { msg_send![class!(NSMutableDictionary), new] };
    for (key, value) in pairs {
        let ns_key = ns_string(key);
        let () = unsafe { msg_send![dict, setObject: *value forKey: ns_key] };
        release(ns_key);
    }
    dict
}

fn instances_respond(selector: makepad_objc_sys::runtime::Sel) -> bool {
    let responds: BOOL = unsafe { msg_send![class!(CATapDescription), instancesRespondToSelector: selector] };
    responds == YES
}

fn release(object: ObjcId) {
    if !object.is_null() {
        let () = unsafe { msg_send![object, release] };
    }
}

fn drop_slot(slot: *mut ProcSlot) {
    if !slot.is_null() {
        unsafe { drop(Box::from_raw(slot)) };
    }
}

fn check(status: i32, step: &'static str) -> Result<()> {
    if status == 0 { Ok(()) } else { Err(Error::System { status, step }) }
}
