//! Core Audio process taps. One private tap per route, played through a
//! private aggregate whose clock is the chosen output device.

use crate::{AudioProcess, Error, FrameInfo, Mute, OutputDevice, Processor, Result, RouteConfig, Source};
use makepad_objc_sys::rc::autoreleasepool;
use makepad_objc_sys::runtime::{ObjcId, BOOL, YES};
use makepad_objc_sys::{class, msg_send, sel, sel_impl};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::Mutex;

const SYSTEM: u32 = 1;
const GLOBAL: u32 = fourcc(*b"glob");
const OUTPUT: u32 = fourcc(*b"outp");
const INPUT: u32 = fourcc(*b"inpt");
/// The widest device stream tapped whole; a wider one falls back to a
/// stereo mixdown.
const MAX_TAP_CHANNELS: usize = 64;
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
    fn AudioObjectSetPropertyData(
        object: u32,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
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
    fn CFStringCreateWithCString(alloc: *const c_void, text: *const c_char, encoding: u32) -> *const c_void;
}

extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    // Keeps AppKit linked so NSRunningApplication is registered.
    fn NSApplicationLoad() -> BOOL;
}

struct ProcSlot {
    processor: Box<dyn Processor>,
}

/// Where the route's audio thread reads and writes, worked out when the
/// route opens.
///
/// The tap is the player's own output on the output device's stream that
/// carries the device's stereo pair, in that stream's width, rate and
/// channel order: no mixdown, so the level and the channels are exactly
/// what the player sends the device. The processor sees the pair as
/// stereo, and the processed pair goes back onto the same two output
/// channels; the stream's other channels pass through as tapped. Where a
/// stream tap is not available, a stereo mixdown is tapped and played on
/// the pair.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    /// Input buffers ahead of the tap's: the output device's own inputs.
    skip: usize,
    /// The tap's width.
    width: usize,
    /// The processor's width: 2, or 1 for a mono tap.
    channels: usize,
    /// Left and right: channels of the tap.
    tap_pair: [usize; 2],
    /// Left and right: channels of the output, counted across its buffers.
    out_pair: [usize; 2],
    /// A stream tap: the output channel its first channel lands on.
    raw_first: Option<usize>,
}

struct Shared {
    processor: AtomicPtr<ProcSlot>,
    epoch: AtomicU64,
    sample_rate: f64,
    channels: u16,
    layout: Layout,
    scratch_frames: usize,
    /// The tap's stream format as Core Audio reports it, for [`Route::describe`].
    tap_format: String,
    /// The last block's buffer layouts ([`pack_layout`]), input and output.
    in_layout: AtomicU64,
    out_layout: AtomicU64,
    /// Processing: the processor's output is played (faded in); monitoring,
    /// it is not (faded out) and the tap leaves the player unmuted.
    processing: AtomicBool,
}

struct Callback {
    shared: *const Shared,
    /// The tap's block, `layout.width` wide.
    scratch: Vec<f32>,
    /// The processor's block, `layout.channels` wide.
    block: Vec<f32>,
    /// Our output's level, gliding to 1 while processing and 0 while not.
    fade: f32,
}

pub struct Route {
    aggregate: u32,
    proc_id: *mut c_void,
    tap: u32,
    description: ObjcId,
    mute: Mute,
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

    /// Monitoring (`false`): the tap leaves the player unmuted and the
    /// processor's output is not played. Processing (`true`): the tap mutes
    /// the player per the route's [`Mute`] and the output is played. Our
    /// output glides in or out over 10 ms around the switch.
    pub fn set_processing(&self, processing: bool) -> Result<()> {
        if processing == self.processing() {
            return Ok(());
        }
        let behavior = if processing { mute_behavior(self.mute) } else { 0 };
        let () = unsafe { msg_send![self.description, setMuteBehavior: behavior] };
        let address = address(fourcc(*b"tdsc"), GLOBAL);
        let description = self.description;
        let status = unsafe {
            AudioObjectSetPropertyData(self.tap, &address, 0, ptr::null(), std::mem::size_of::<ObjcId>() as u32, &description as *const ObjcId as *const c_void)
        };
        check(status, "set tap mute")?;
        self.shared.processing.store(processing, Ordering::Release);
        Ok(())
    }

    pub fn processing(&self) -> bool {
        self.shared.processing.load(Ordering::Acquire)
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

    /// What the route's audio thread sees, for a diagnostic log: the tap's
    /// stream format, and the channel count of each buffer in the last
    /// block's input list (the output device's own inputs, then the tap)
    /// and output list.
    pub fn describe(&self) -> String {
        let layout = |packed: u64| {
            let count = (packed & 0xff) as usize;
            let widths: Vec<String> = (0..count.min(7)).map(|i| ((packed >> (8 + 8 * i)) & 0xff).to_string()).collect();
            format!("[{}]", widths.join(" "))
        };
        let l = &self.shared.layout;
        format!(
            "tap {} ({}), L/R = tap ch {}/{} -> output ch {}/{} (1-based){}; input buffers {}; output buffers {}",
            self.shared.tap_format,
            if l.raw_first.is_some() { "the device stream, no mixdown" } else { "stereo mixdown" },
            l.tap_pair[0] + 1,
            l.tap_pair[1] + 1,
            l.out_pair[0] + 1,
            l.out_pair[1] + 1,
            match l.raw_first {
                Some(first) => format!(", other stream channels pass through from output ch {}", first + 1),
                None => String::new(),
            },
            layout(self.shared.in_layout.load(Ordering::Relaxed)),
            layout(self.shared.out_layout.load(Ordering::Relaxed))
        )
    }
}

/// Whether macOS lets this app hear other apps ("Screen & System Audio
/// Recording", the TCC service `kTCCServiceAudioCapture`), asked without
/// prompting. `None` where the system does not say (the check is the
/// private `TCCAccessPreflight`, looked up at run time). Opening a route is
/// what asks the person, the first time.
pub fn permission() -> Option<crate::Permission> {
    type Preflight = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;
    static PREFLIGHT: std::sync::OnceLock<Option<Preflight>> = std::sync::OnceLock::new();
    let preflight = (*PREFLIGHT.get_or_init(|| {
        let path = CString::new("/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC").ok()?;
        let symbol = CString::new("TCCAccessPreflight").ok()?;
        let handle = unsafe { dlopen(path.as_ptr(), 1) };
        if handle.is_null() {
            return None;
        }
        let function = unsafe { dlsym(handle, symbol.as_ptr()) };
        (!function.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, Preflight>(function) })
    }))?;
    let service = CString::new("kTCCServiceAudioCapture").ok()?;
    let service = unsafe { CFStringCreateWithCString(ptr::null(), service.as_ptr(), 0x0800_0100) };
    if service.is_null() {
        return None;
    }
    let answer = unsafe { preflight(service, ptr::null()) };
    unsafe { CFRelease(service) };
    Some(match answer {
        0 => crate::Permission::Granted,
        1 => crate::Permission::Denied,
        _ => crate::Permission::Unknown,
    })
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
    let output_id = match &config.output {
        Some(uid) => device_for_uid(uid)?,
        None => default_output_device()?,
    };
    let output_uid = cfstring_property(output_id, fourcc(*b"uid "), GLOBAL, "output uid")?;
    let out_widths = stream_widths(output_id, OUTPUT);
    let out_total: usize = out_widths.iter().sum();
    let pair = preferred_pair(output_id, out_total);
    let (stream, first) = stream_of(&out_widths, pair[0]);
    // The player's output on the stream that carries the pair; a stereo
    // mixdown where that cannot be tapped.
    let mut streamed = stream.filter(|_| instances_respond(sel!(initWithProcesses:andDeviceUID:withStream:)));
    let mut tap = 0u32;
    let description = loop {
        let mute = if config.processing { config.mute } else { Mute::HearOriginal };
        let description = tap_description(&object_ids, &bundles, mute, streamed.map(|stream| (output_uid.as_str(), stream)))?;
        let status = unsafe { AudioHardwareCreateProcessTap(description, &mut tap) };
        if status == 0 {
            break description;
        }
        release(description);
        if streamed.take().is_none() {
            return Err(Error::System { status, step: "create tap" });
        }
    };
    let tap_uid = match cfstring_property(tap, fourcc(*b"tuid"), GLOBAL, "tap uid") {
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

    let width = (tap_channels(tap) as usize).clamp(1, MAX_TAP_CHANNELS);
    let layout = route_layout(width, streamed.map(|stream| out_widths[stream]), first, pair, out_total, stream_widths(output_id, INPUT).len());
    let channels = layout.channels as u16;
    let scratch_frames = buffer_cap(aggregate);
    let sample_rate = f64_property(aggregate, fourcc(*b"nsrt"), GLOBAL).unwrap_or(48_000.0);
    let shared = Box::new(Shared {
        processor: AtomicPtr::new(Box::into_raw(Box::new(ProcSlot { processor }))),
        epoch: AtomicU64::new(0),
        sample_rate,
        channels,
        layout,
        scratch_frames,
        tap_format: tap_format(tap),
        in_layout: AtomicU64::new(0),
        out_layout: AtomicU64::new(0),
        processing: AtomicBool::new(config.processing),
    });
    let callback = Box::new(Callback {
        shared: &*shared as *const Shared,
        scratch: vec![0.0; scratch_frames * layout.width],
        block: vec![0.0; scratch_frames * layout.channels],
        fade: if config.processing { 1.0 } else { 0.0 },
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
        mute: config.mute,
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
    let layout = shared.layout;
    let frames = output_frames(output);
    if frames == 0 || frames > shared.scratch_frames {
        shared.epoch.fetch_add(1, Ordering::Release);
        return 0;
    }
    shared.in_layout.store(pack_layout(input), Ordering::Relaxed);
    shared.out_layout.store(pack_layout(output), Ordering::Relaxed);
    let wide = &mut callback.scratch[..frames * layout.width];
    read_tap(buffers_of(input), layout.skip, layout.width, frames, wide);
    let block = &mut callback.block[..frames * layout.channels];
    take_pair(wide, &layout, frames, block);
    let info = FrameInfo {
        sample_rate: shared.sample_rate,
        channels: shared.channels,
        frames,
        host_time: if now.is_null() { 0 } else { (*now).host_time },
    };
    let slot = shared.processor.load(Ordering::Acquire);
    if !slot.is_null() {
        (*slot).processor.process(block, &info);
    }
    let target = if shared.processing.load(Ordering::Acquire) { 1.0 } else { 0.0 };
    if callback.fade != target || target == 0.0 {
        let step = 1.0 / (0.010 * shared.sample_rate as f32).max(1.0);
        for frame in 0..frames {
            let fade = &mut callback.fade;
            *fade = if *fade < target { (*fade + step).min(target) } else { (*fade - step).max(target) };
            for sample in &mut block[frame * layout.channels..(frame + 1) * layout.channels] {
                *sample *= *fade;
            }
            for sample in &mut wide[frame * layout.width..(frame + 1) * layout.width] {
                *sample *= *fade;
            }
        }
    }
    write_out(buffers_of_mut(output), &layout, frames, block, wide);
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

/// A tap of `object_ids` (and `bundles`): their output on one output
/// device's stream as it is (`stream`: the device's uid and the stream's
/// index), or mixed down to stereo.
fn tap_description(object_ids: &[u32], bundles: &[String], mute: Mute, stream: Option<(&str, usize)>) -> Result<ObjcId> {
    let numbers: Vec<ObjcId> = object_ids.iter().copied().map(ns_u32).collect();
    let processes = ns_array(&numbers);
    let allocated: ObjcId = unsafe { msg_send![class!(CATapDescription), alloc] };
    let description: ObjcId = match stream {
        Some((device, index)) => {
            let device = ns_string(device);
            let index = index as isize;
            let description: ObjcId = unsafe { msg_send![allocated, initWithProcesses: processes andDeviceUID: device withStream: index] };
            release(device);
            description
        }
        None => unsafe { msg_send![allocated, initStereoMixdownOfProcesses: processes] },
    };
    if description.is_null() {
        release(processes);
        return Err(Error::System { status: -1, step: "tap description" });
    }
    let behavior = mute_behavior(mute);
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

/// `CATapMuteBehavior`: unmuted, muted, muted while the tap is read.
fn mute_behavior(mute: Mute) -> isize {
    match mute {
        Mute::HearOriginal => 0,
        Mute::Replace => 1,
        Mute::ReplaceWhileRouted => 2,
    }
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

/// A buffer list's shape in one word: the count, then up to seven widths.
fn pack_layout(list: *const AudioBufferList) -> u64 {
    let buffers = buffers_of(list);
    let mut packed = buffers.len().min(255) as u64;
    for (i, buffer) in buffers.iter().take(7).enumerate() {
        packed |= (buffer.number_channels.min(255) as u64) << (8 + 8 * i);
    }
    packed
}

/// The tap's stream format, as text.
fn tap_format(tap: u32) -> String {
    let mut format = StreamFormat {
        _sample_rate: 0.0,
        _format_id: 0,
        _format_flags: 0,
        _bytes_per_packet: 0,
        _frames_per_packet: 0,
        _bytes_per_frame: 0,
        channels_per_frame: 0,
        _bits_per_channel: 0,
        _reserved: 0,
    };
    let mut size = std::mem::size_of::<StreamFormat>() as u32;
    let address = address(fourcc(*b"tfmt"), GLOBAL);
    let status = unsafe {
        AudioObjectGetPropertyData(tap, &address, 0, ptr::null(), &mut size, &mut format as *mut StreamFormat as *mut c_void)
    };
    if status != 0 {
        return format!("format unknown ({status})");
    }
    let id = format._format_id.to_be_bytes();
    format!(
        "{:.0} Hz, {} ch, '{}', flags {:#x} ({}float, {}interleaved), {} bit, {} bytes/frame",
        format._sample_rate,
        format.channels_per_frame,
        String::from_utf8_lossy(&id),
        format._format_flags,
        if format._format_flags & 1 != 0 { "" } else { "not " },
        if format._format_flags & (1 << 5) != 0 { "non-" } else { "" },
        format._bits_per_channel,
        format._bytes_per_frame
    )
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

/// Where the audio thread reads and writes; see [`Layout`]. `stream_width`
/// is the tapped stream's width when the tap is a device stream, `first`
/// the output channel that stream starts on, `pair` the device's stereo
/// pair, `skip` the output device's input streams.
fn route_layout(width: usize, stream_width: Option<usize>, first: usize, pair: [usize; 2], out_total: usize, skip: usize) -> Layout {
    let channels = if width >= 2 { 2 } else { 1 };
    let out_pair = if out_total <= 1 { [0, 0] } else { pair };
    match stream_width.filter(|stream_width| *stream_width == width) {
        Some(_) => {
            let in_stream = |channel: usize| channel.checked_sub(first).filter(|at| *at < width);
            let left = in_stream(pair[0]).unwrap_or(0);
            let right = in_stream(pair[1]).unwrap_or(left);
            Layout { skip, width, channels, tap_pair: [left, right], out_pair, raw_first: Some(first) }
        }
        None => Layout { skip, width, channels, tap_pair: [0, 1.min(width - 1)], out_pair, raw_first: None },
    }
}

/// Read the tap into interleaved `dst`, `channels` wide. The aggregate's
/// input list is the output device's own inputs (`skip` buffers), then the
/// tap: one buffer of the tap's width, or one buffer per channel. Every
/// channel is read, none is left silent, and a device input never stands
/// in for the tap.
fn read_tap(buffers: &[AudioBuffer], skip: usize, channels: usize, frames: usize, dst: &mut [f32]) {
    dst.fill(0.0);
    if channels == 0 || frames == 0 {
        return;
    }
    let buffers = if skip < buffers.len() { &buffers[skip..] } else { buffers };
    if let Some(buffer) = buffers.iter().rev().find(|buffer| !buffer.data.is_null() && buffer.number_channels as usize == channels) {
        copy_channels(buffer, 0, channels, frames, dst);
        return;
    }
    let mut start = buffers.len();
    let mut width = 0;
    while start > 0 && width < channels {
        start -= 1;
        if !buffers[start].data.is_null() {
            width += (buffers[start].number_channels as usize).max(1);
        }
    }
    let mut first = 0;
    for buffer in buffers[start..].iter().filter(|buffer| !buffer.data.is_null()) {
        copy_channels(buffer, first, channels, frames, dst);
        first += (buffer.number_channels as usize).max(1);
    }
}

/// Copy `buffer`'s channels into interleaved `dst` from channel `first` on,
/// as many as fit, sample for sample.
fn copy_channels(buffer: &AudioBuffer, first: usize, channels: usize, frames: usize, dst: &mut [f32]) {
    let ch = (buffer.number_channels as usize).max(1);
    let count = frames.min(buffer_frames(buffer));
    if count == 0 || first >= channels {
        return;
    }
    let src = buffer.data as *const f32;
    let take = ch.min(channels - first);
    for frame in 0..count {
        for channel in 0..take {
            dst[frame * channels + first + channel] = unsafe { *src.add(frame * ch + channel) };
        }
    }
}

/// The processor's block from the tap's: the pair, sample for sample.
fn take_pair(wide: &[f32], layout: &Layout, frames: usize, block: &mut [f32]) {
    for frame in 0..frames {
        let row = &wide[frame * layout.width..(frame + 1) * layout.width];
        if layout.channels == 1 {
            block[frame] = row[layout.tap_pair[0]];
        } else {
            block[frame * 2] = row[layout.tap_pair[0]];
            block[frame * 2 + 1] = row[layout.tap_pair[1]];
        }
    }
}

/// Play the processed pair on the output's pair (a mono output hears both
/// sides mixed); with a stream tap the stream's other channels pass through
/// as tapped, and every other output channel is silent.
fn write_out(buffers: &mut [AudioBuffer], layout: &Layout, frames: usize, block: &[f32], wide: &[f32]) {
    let total: usize = buffers.iter().filter(|buffer| !buffer.data.is_null()).map(|buffer| (buffer.number_channels as usize).max(1)).sum();
    let sample = |frame: usize, out: usize| -> f32 {
        let (left, right) = if layout.channels == 1 { (block[frame], block[frame]) } else { (block[frame * 2], block[frame * 2 + 1]) };
        if total == 1 {
            return 0.5 * (left + right);
        }
        if out == layout.out_pair[0] {
            return left;
        }
        if out == layout.out_pair[1] {
            return right;
        }
        match layout.raw_first.and_then(|first| out.checked_sub(first)).filter(|at| *at < layout.width && !layout.tap_pair.contains(at)) {
            Some(at) => wide[frame * layout.width + at],
            None => 0.0,
        }
    };
    let mut index = 0usize;
    for buffer in buffers.iter_mut().filter(|buffer| !buffer.data.is_null()) {
        let ch = (buffer.number_channels as usize).max(1);
        let count = frames.min(buffer_frames(buffer));
        let dst = buffer.data as *mut f32;
        for channel in 0..ch {
            for frame in 0..count {
                unsafe { *dst.add(frame * ch + channel) = sample(frame, index + channel) };
            }
        }
        index += ch;
    }
}

/// Each stream's width on `scope` of `device`, in order.
fn stream_widths(device: u32, scope: u32) -> Vec<usize> {
    let Ok(words) = property_words(device, fourcc(*b"slay"), scope, &[]) else { return Vec::new() };
    if words.is_empty() {
        return Vec::new();
    }
    let list = words.as_ptr() as *const AudioBufferList;
    buffers_of(list).iter().map(|buffer| buffer.number_channels as usize).collect()
}

/// The device's stereo pair (its "preferred channels for stereo"), 0-based;
/// the first two channels when it names none.
fn preferred_pair(device: u32, total: usize) -> [usize; 2] {
    let fallback = [0, 1.min(total.saturating_sub(1))];
    let Ok(words) = property_words(device, fourcc(*b"dch2"), OUTPUT, &[]) else { return fallback };
    let Some(word) = words.first() else { return fallback };
    let (left, right) = ((*word & 0xffff_ffff) as usize, (*word >> 32) as usize);
    if left == 0 || right == 0 || left > total || right > total {
        return fallback;
    }
    [left - 1, right - 1]
}

/// The stream holding output channel `channel`, and the channel its first
/// channel is.
fn stream_of(widths: &[usize], channel: usize) -> (Option<usize>, usize) {
    let mut first = 0;
    for (stream, width) in widths.iter().enumerate() {
        if channel < first + width {
            return (Some(stream), first);
        }
        first += width;
    }
    (None, 0)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(channels: u32, data: &mut [f32]) -> AudioBuffer {
        AudioBuffer { number_channels: channels, data_byte_size: (data.len() * 4) as u32, data: data.as_mut_ptr() as *mut c_void }
    }

    fn rms_db(samples: &[f32]) -> f32 {
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        20.0 * rms.log10()
    }

    const FRAMES: usize = 480;

    /// A -12 dBFS sine, left, and its inverse, right.
    fn stereo() -> (Vec<f32>, Vec<f32>) {
        let amp = 10f32.powf(-12.0 / 20.0);
        let left: Vec<f32> = (0..FRAMES).map(|i| amp * (std::f32::consts::TAU * 1_000.0 * i as f32 / 44_100.0).sin()).collect();
        let right = left.iter().map(|s| -s).collect();
        (left, right)
    }

    /// Run one block through the route's datapath at unity (read, pair,
    /// no processing, write) and return the output buffers' samples.
    fn through(layout: &Layout, input: &mut [AudioBuffer], outputs: &[u32]) -> Vec<Vec<f32>> {
        let mut wide = vec![0.0; FRAMES * layout.width];
        read_tap(input, layout.skip, layout.width, FRAMES, &mut wide);
        let mut block = vec![0.0; FRAMES * layout.channels];
        take_pair(&wide, layout, FRAMES, &mut block);
        let mut data: Vec<Vec<f32>> = outputs.iter().map(|ch| vec![9.0; FRAMES * *ch as usize]).collect();
        let mut out: Vec<AudioBuffer> = data.iter_mut().zip(outputs).map(|(d, ch)| buffer(*ch, d)).collect();
        write_out(&mut out, layout, FRAMES, &block, &wide);
        data
    }

    fn channel(data: &[f32], width: usize, at: usize) -> Vec<f32> {
        data.iter().skip(at).step_by(width).copied().collect()
    }

    /// The user's rig: an 8-channel interface whose own 8 inputs come first
    /// in the aggregate's input list, a tap of its 8-channel output stream
    /// with the player on channels 1/2, one 8-channel output buffer. The
    /// pair plays on output 1/2 sample for sample (-12 dBFS sine in, -12
    /// out), the device's inputs never leak in, and the stream's other
    /// channels pass through as tapped.
    #[test]
    fn a_stream_tap_plays_on_the_same_channels_at_unity() {
        let (left, right) = stereo();
        let mut tap = vec![0.0f32; FRAMES * 8];
        for frame in 0..FRAMES {
            tap[frame * 8] = left[frame];
            tap[frame * 8 + 1] = right[frame];
            tap[frame * 8 + 5] = 0.25;
        }
        let mut mic = vec![0.5f32; FRAMES * 8];
        let layout = route_layout(8, Some(8), 0, [0, 1], 8, 1);
        assert_eq!(layout, Layout { skip: 1, width: 8, channels: 2, tap_pair: [0, 1], out_pair: [0, 1], raw_first: Some(0) });
        let out = through(&layout, &mut [buffer(8, &mut mic), buffer(8, &mut tap)], &[8]);
        assert_eq!(channel(&out[0], 8, 0), left);
        assert_eq!(channel(&out[0], 8, 1), right);
        assert!(channel(&out[0], 8, 5).iter().all(|s| *s == 0.25), "the stream's other channels pass through");
        for silent in [2, 3, 4, 6, 7] {
            assert!(channel(&out[0], 8, silent).iter().all(|s| *s == 0.0), "output {silent} stays silent");
        }
        let db = rms_db(&channel(&out[0], 8, 0));
        assert!((db - (-12.0 - 3.01)).abs() < 0.05, "a -12 dBFS sine plays at {db:.2} dBFS RMS");
    }

    /// A device whose stereo pair is 3/4, on its second stream: the tap of
    /// that stream feeds the processor from its channels 1/2 and the pair
    /// plays on outputs 3/4 only.
    #[test]
    fn the_pair_follows_the_device() {
        let (left, right) = stereo();
        let (stream, first) = stream_of(&[2, 2], 2);
        assert_eq!((stream, first), (Some(1), 2));
        let layout = route_layout(2, Some(2), first, [2, 3], 4, 0);
        assert_eq!(layout.tap_pair, [0, 1]);
        let mut tap: Vec<f32> = left.iter().zip(&right).flat_map(|(l, r)| [*l, *r]).collect();
        let out = through(&layout, &mut [buffer(2, &mut tap)], &[2, 2]);
        assert!(out[0].iter().all(|s| *s == 0.0), "outputs 1/2 are not the pair");
        assert_eq!(channel(&out[1], 2, 0), left);
        assert_eq!(channel(&out[1], 2, 1), right);
    }

    /// The stereo mixdown fallback, from an interleaved tap and from a tap
    /// that comes one buffer per channel, plays on the pair at unity; no
    /// side dropped, nothing averaged, the device input ignored.
    #[test]
    fn a_mixdown_tap_reaches_the_pair_at_unity() {
        let (left, right) = stereo();
        let mut interleaved: Vec<f32> = left.iter().zip(&right).flat_map(|(l, r)| [*l, *r]).collect();
        let (mut l, mut r) = (left.clone(), right.clone());
        let mut mic = vec![0.5f32; FRAMES];
        let mut mic2 = mic.clone();
        let layout = route_layout(2, None, 0, [0, 1], 8, 1);
        assert_eq!(layout.raw_first, None);
        for input in [vec![buffer(1, &mut mic), buffer(2, &mut interleaved)], vec![buffer(1, &mut mic2), buffer(1, &mut l), buffer(1, &mut r)]] {
            let mut input = input;
            let out = through(&layout, &mut input, &[8]);
            assert_eq!(channel(&out[0], 8, 0), left);
            assert_eq!(channel(&out[0], 8, 1), right);
            assert!(channel(&out[0], 8, 2).iter().all(|s| *s == 0.0));
        }
    }

    /// A mono output hears both sides mixed.
    #[test]
    fn a_mono_output_hears_both_sides() {
        let (left, _) = stereo();
        let mut tap: Vec<f32> = left.iter().flat_map(|l| [*l, *l]).collect();
        let layout = route_layout(2, None, 0, [0, 0], 1, 0);
        let out = through(&layout, &mut [buffer(2, &mut tap)], &[1]);
        assert_eq!(out[0], left);
    }
}
