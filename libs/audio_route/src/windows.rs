//! WASAPI process loopback (Windows 10 2004, build 19041, and later). One
//! route captures the target's process tree through a loopback client made
//! by `ActivateAudioInterfaceAsync` (`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`,
//! include the target's tree), runs the processor on the render thread, and
//! plays the result through a shared-mode render client on the chosen
//! output device.
//!
//! Two threads per route: the render thread owns the render client, the
//! captures and the processor, and is clocked by the output device's
//! events; the watch thread finds the target processes (a relaunched
//! player is followed), activates their captures and hands them over.
//!
//! The player's direct path is not silenced: [`crate::Mute`] is not honoured
//! here, every route is a copy. Muting the player's audio session
//! (`ISimpleAudioVolume::SetMute`) also silences its process loopback
//! (measured: the capture reads exact zeros while the session is muted),
//! and the session volume scales the capture linearly, so neither can
//! stand in for macOS's tap mute.
//!
//! COM is bound here by hand, like the Core Audio calls in `macos.rs`: the
//! handful of interfaces this needs are small vtables, and the crate stays
//! free of dependencies.

use crate::{AudioProcess, Error, FrameInfo, Mute, OutputDevice, Processor, Result, RouteConfig, Source};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

type Hresult = i32;
type Handle = *mut c_void;
type Bool = i32;

// ---- GUIDs ----

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const fn guid(value: u128) -> Guid {
    Guid {
        data1: (value >> 96) as u32,
        data2: (value >> 80) as u16,
        data3: (value >> 64) as u16,
        data4: (value as u64).to_be_bytes(),
    }
}

const IID_IUNKNOWN: Guid = guid(0x00000000_0000_0000_c000_000000000046);
const IID_IAGILE_OBJECT: Guid = guid(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90);
const CLSID_MM_DEVICE_ENUMERATOR: Guid = guid(0xbcde0395_e52f_467c_8e3d_c4579291692e);
const IID_IMM_DEVICE_ENUMERATOR: Guid = guid(0xa95664d2_9614_4f35_a746_de8db63617e6);
const IID_IAUDIO_CLIENT: Guid = guid(0x1cb9ad4c_dbfa_4c32_b178_c2f568a703b2);
const IID_IAUDIO_CAPTURE_CLIENT: Guid = guid(0xc8adbd64_e71e_48a0_a4de_185c395cd317);
const IID_IAUDIO_RENDER_CLIENT: Guid = guid(0xf294acfc_3146_4483_a7bf_addca7c260e2);
const IID_IAUDIO_SESSION_MANAGER2: Guid = guid(0x77aa99a0_1bd6_484f_8bc7_2c654c9a9b6f);
const IID_IAUDIO_SESSION_CONTROL2: Guid = guid(0xbfb7ff88_7239_4fc9_8fa2_07c950be9c6d);
const IID_ISIMPLE_AUDIO_VOLUME: Guid = guid(0x87ce5498_68d6_44e5_9215_6da47ef883d8);
const IID_IAUDIO_SESSION_EVENTS: Guid = guid(0x24918acc_64b3_37c1_8ca9_74a66e9957a8);
/// The event context of every session volume change this crate makes, so
/// its own changes are told apart from the person's.
const OUR_CONTEXT: Guid = guid(0x6d616b65_7061_6400_a0d1_6f2d726f7574);
const IID_IACTIVATE_COMPLETION_HANDLER: Guid = guid(0x41d949ab_9862_444a_80f6_c261334da5eb);
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: Guid = guid(0x00000003_0000_0010_8000_00aa00389b71);
const PKEY_DEVICE_FRIENDLY_NAME: PropertyKey = PropertyKey { fmtid: guid(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };

// ---- constants ----

const S_OK: Hresult = 0;
const E_NOINTERFACE: Hresult = 0x8000_4002u32 as i32;
const E_POINTER: Hresult = 0x8000_4003u32 as i32;
const RPC_E_CHANGED_MODE: Hresult = 0x8001_0106u32 as i32;
const COINIT_MULTITHREADED: u32 = 0;
const CLSCTX_ALL: u32 = 0x17;
const STGM_READ: u32 = 0;
const E_RENDER: i32 = 0;
const E_CAPTURE: i32 = 1;
const E_CONSOLE: i32 = 0;
const DEVICE_STATE_ACTIVE: u32 = 1;
const AUDCLNT_SHAREMODE_SHARED: u32 = 0;
const AUDCLNT_STREAMFLAGS_LOOPBACK: u32 = 0x0002_0000;
const AUDCLNT_STREAMFLAGS_EVENTCALLBACK: u32 = 0x0004_0000;
const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;
const AUDIO_SESSION_STATE_ACTIVE: u32 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
const VT_LPWSTR: u16 = 31;
const VT_BLOB: u16 = 65;
const AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK: u32 = 1;
const PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE: u32 = 0;
const WAIT_OBJECT_0: u32 = 0;
const TH32CS_SNAPPROCESS: u32 = 0x2;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const SYNCHRONIZE: u32 = 0x0010_0000;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

/// The render client's buffer (100 ns units): 20 ms.
const RENDER_BUFFER: i64 = 200_000;
/// The capture client's buffer: 100 ms, read every render period.
const CAPTURE_BUFFER: i64 = 1_000_000;
/// How long an activation may take before it counts as failed.
const ACTIVATE_TIMEOUT_MS: u32 = 5_000;
/// How often the watch thread looks for the player and its sessions.
const WATCH_EVERY: Duration = Duration::from_millis(500);
/// Captures the render thread can hold without allocating.
const MAX_CAPTURES: usize = 8;

// ---- structs ----

#[repr(C)]
#[derive(Clone, Copy)]
struct PropertyKey {
    fmtid: Guid,
    pid: u32,
}

/// PROPVARIANT, 24 bytes on 64-bit Windows: the type, three reserved
/// words, then the value (a pointer for `VT_LPWSTR`; size then pointer for
/// `VT_BLOB`).
#[repr(C)]
struct PropVariant {
    vt: u16,
    _reserved: [u16; 3],
    data: [usize; 2],
}

#[repr(C, packed(1))]
#[derive(Clone, Copy)]
struct WaveFormatEx {
    format_tag: u16,
    channels: u16,
    samples_per_sec: u32,
    avg_bytes_per_sec: u32,
    block_align: u16,
    bits_per_sample: u16,
    cb_size: u16,
}

#[repr(C, packed(1))]
#[derive(Clone, Copy)]
struct WaveFormatExtensible {
    format: WaveFormatEx,
    valid_bits_per_sample: u16,
    channel_mask: u32,
    sub_format: Guid,
}

#[repr(C)]
struct ActivationParams {
    activation_type: u32,
    target_process_id: u32,
    process_loopback_mode: u32,
}

#[repr(C)]
struct ProcessEntry {
    size: u32,
    _usage: u32,
    process_id: u32,
    _default_heap_id: usize,
    _module_id: u32,
    _threads: u32,
    parent_process_id: u32,
    _pri_class_base: i32,
    _flags: u32,
    exe_file: [u16; 260],
}

// ---- COM vtables (only the slots this file calls are typed) ----

type QueryInterface = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult;
type AddRefRelease = unsafe extern "system" fn(*mut c_void) -> u32;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: QueryInterface,
    add_ref: AddRefRelease,
    release: AddRefRelease,
}

#[repr(C)]
struct IMMDeviceEnumeratorVtbl {
    base: IUnknownVtbl,
    enum_audio_endpoints: unsafe extern "system" fn(*mut c_void, i32, u32, *mut *mut c_void) -> Hresult,
    get_default_audio_endpoint: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> Hresult,
    get_device: unsafe extern "system" fn(*mut c_void, *const u16, *mut *mut c_void) -> Hresult,
}

#[repr(C)]
struct IMMDeviceCollectionVtbl {
    base: IUnknownVtbl,
    get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
    item: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> Hresult,
}

#[repr(C)]
struct IMMDeviceVtbl {
    base: IUnknownVtbl,
    activate: unsafe extern "system" fn(*mut c_void, *const Guid, u32, *const PropVariant, *mut *mut c_void) -> Hresult,
    open_property_store: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> Hresult,
    get_id: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
}

#[repr(C)]
struct IPropertyStoreVtbl {
    base: IUnknownVtbl,
    _get_count: usize,
    _get_at: usize,
    get_value: unsafe extern "system" fn(*mut c_void, *const PropertyKey, *mut PropVariant) -> Hresult,
}

#[repr(C)]
struct IAudioClientVtbl {
    base: IUnknownVtbl,
    initialize: unsafe extern "system" fn(*mut c_void, u32, u32, i64, i64, *const c_void, *const Guid) -> Hresult,
    get_buffer_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
    _get_stream_latency: usize,
    get_current_padding: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
    _is_format_supported: usize,
    get_mix_format: unsafe extern "system" fn(*mut c_void, *mut *mut WaveFormatEx) -> Hresult,
    _get_device_period: usize,
    start: unsafe extern "system" fn(*mut c_void) -> Hresult,
    stop: unsafe extern "system" fn(*mut c_void) -> Hresult,
    reset: unsafe extern "system" fn(*mut c_void) -> Hresult,
    set_event_handle: unsafe extern "system" fn(*mut c_void, Handle) -> Hresult,
    get_service: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
}

#[repr(C)]
struct IAudioCaptureClientVtbl {
    base: IUnknownVtbl,
    get_buffer: unsafe extern "system" fn(*mut c_void, *mut *mut u8, *mut u32, *mut u32, *mut u64, *mut u64) -> Hresult,
    release_buffer: unsafe extern "system" fn(*mut c_void, u32) -> Hresult,
    get_next_packet_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
}

#[repr(C)]
struct IAudioRenderClientVtbl {
    base: IUnknownVtbl,
    get_buffer: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u8) -> Hresult,
    release_buffer: unsafe extern "system" fn(*mut c_void, u32, u32) -> Hresult,
}

#[repr(C)]
struct IAudioSessionManager2Vtbl {
    base: IUnknownVtbl,
    _get_audio_session_control: usize,
    _get_simple_audio_volume: usize,
    get_session_enumerator: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
}

#[repr(C)]
struct IAudioSessionEnumeratorVtbl {
    base: IUnknownVtbl,
    get_count: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    get_session: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> Hresult,
}

#[repr(C)]
struct IAudioSessionControl2Vtbl {
    base: IUnknownVtbl,
    get_state: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
    _names_and_grouping: [usize; 6],
    register_audio_session_notification: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Hresult,
    unregister_audio_session_notification: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Hresult,
    get_session_identifier: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
    _get_session_instance_identifier: usize,
    get_process_id: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
}

#[repr(C)]
struct ISimpleAudioVolumeVtbl {
    base: IUnknownVtbl,
    set_master_volume: unsafe extern "system" fn(*mut c_void, f32, *const Guid) -> Hresult,
    get_master_volume: unsafe extern "system" fn(*mut c_void, *mut f32) -> Hresult,
}

#[repr(C)]
struct IActivateAudioInterfaceAsyncOperationVtbl {
    base: IUnknownVtbl,
    get_activate_result: unsafe extern "system" fn(*mut c_void, *mut Hresult, *mut *mut c_void) -> Hresult,
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> Hresult;
    fn CoUninitialize();
    fn CoCreateInstance(clsid: *const Guid, outer: *mut c_void, context: u32, iid: *const Guid, out: *mut *mut c_void) -> Hresult;
    fn CoTaskMemFree(pointer: *mut c_void);
    fn PropVariantClear(value: *mut PropVariant) -> Hresult;
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateEventW(attributes: *mut c_void, manual_reset: Bool, initial_state: Bool, name: *const u16) -> Handle;
    fn SetEvent(event: Handle) -> Bool;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn WaitForMultipleObjects(count: u32, handles: *const Handle, wait_all: Bool, milliseconds: u32) -> u32;
    fn CloseHandle(handle: Handle) -> Bool;
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> Bool;
    fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry) -> Bool;
    fn OpenProcess(access: u32, inherit: Bool, process_id: u32) -> Handle;
    fn QueryPerformanceCounter(count: *mut i64) -> Bool;
    fn QueryPerformanceFrequency(frequency: *mut i64) -> Bool;
}

// Linked by name at load time, so no import library is needed for them.
#[link(name = "kernel32", kind = "raw-dylib")]
extern "system" {
    fn GetPackageFamilyName(process: Handle, length: *mut u32, name: *mut u16) -> i32;
}

#[link(name = "mmdevapi", kind = "raw-dylib")]
extern "system" {
    fn ActivateAudioInterfaceAsync(
        device_interface_path: *const u16,
        riid: *const Guid,
        activation_params: *const PropVariant,
        completion_handler: *mut c_void,
        operation: *mut *mut c_void,
    ) -> Hresult;
}

#[link(name = "avrt", kind = "raw-dylib")]
extern "system" {
    fn AvSetMmThreadCharacteristicsW(task: *const u16, index: *mut u32) -> Handle;
    fn AvRevertMmThreadCharacteristics(handle: Handle) -> Bool;
}

// ---- small owners ----

/// One reference to a COM object, released on drop.
struct Com(*mut c_void);

// SAFETY: every interface held here is free-threaded (MTA / agile); the
// render thread takes over captures activated on the watch thread.
unsafe impl Send for Com {}

impl Com {
    fn from_out(pointer: *mut c_void, hr: Hresult, step: &'static str) -> Result<Com> {
        if hr < 0 {
            return Err(Error::System { status: hr, step });
        }
        if pointer.is_null() {
            return Err(Error::System { status: E_POINTER, step });
        }
        Ok(Com(pointer))
    }

    /// The object's vtable, typed as `T`.
    unsafe fn vtbl<T>(&self) -> &T {
        &**(self.0 as *mut *const T)
    }

    fn query(&self, iid: &Guid, step: &'static str) -> Result<Com> {
        let mut out = ptr::null_mut();
        let hr = unsafe { (self.vtbl::<IUnknownVtbl>().query_interface)(self.0, iid, &mut out) };
        Com::from_out(out, hr, step)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { (self.vtbl::<IUnknownVtbl>().release)(self.0) };
        }
    }
}

/// A kernel handle, closed on drop.
struct OwnedHandle(Handle);

unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    fn event(manual_reset: bool) -> Result<OwnedHandle> {
        let handle = unsafe { CreateEventW(ptr::null_mut(), manual_reset as Bool, 0, ptr::null()) };
        if handle.is_null() {
            return Err(Error::System { status: -1, step: "create event" });
        }
        Ok(OwnedHandle(handle))
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// COM on the calling thread for the length of a call. A thread that
/// already chose apartment-threaded keeps it; the calls here work in both.
struct ComScope(bool);

impl ComScope {
    fn enter() -> ComScope {
        let hr = unsafe { CoInitializeEx(ptr::null_mut(), COINIT_MULTITHREADED) };
        ComScope(hr >= 0 && hr != RPC_E_CHANGED_MODE)
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(text: *const u16) -> String {
    if text.is_null() {
        return String::new();
    }
    let mut len = 0;
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) })
}

/// A string COM allocated for us, read and freed.
fn take_co_string(text: *mut u16) -> String {
    let string = from_wide(text);
    if !text.is_null() {
        unsafe { CoTaskMemFree(text as *mut c_void) };
    }
    string
}

fn check(hr: Hresult, step: &'static str) -> Result<()> {
    if hr < 0 { Err(Error::System { status: hr, step }) } else { Ok(()) }
}

/// Now, in the 100 ns units of the performance counter that capture
/// packets are stamped in.
fn now_hns() -> u64 {
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut frequency = 0i64;
        unsafe { QueryPerformanceFrequency(&mut frequency) };
        frequency.max(1)
    });
    let mut count = 0i64;
    unsafe { QueryPerformanceCounter(&mut count) };
    (count as u128 * 10_000_000 / frequency as u128) as u64
}

// ---- devices ----

fn enumerator() -> Result<Com> {
    let mut out = ptr::null_mut();
    let hr = unsafe { CoCreateInstance(&CLSID_MM_DEVICE_ENUMERATOR, ptr::null_mut(), CLSCTX_ALL, &IID_IMM_DEVICE_ENUMERATOR, &mut out) };
    Com::from_out(out, hr, "device enumerator")
}

fn endpoints(enumerator: &Com, flow: i32) -> Result<Vec<Com>> {
    let mut collection = ptr::null_mut();
    let hr = unsafe { (enumerator.vtbl::<IMMDeviceEnumeratorVtbl>().enum_audio_endpoints)(enumerator.0, flow, DEVICE_STATE_ACTIVE, &mut collection) };
    let collection = Com::from_out(collection, hr, "list devices")?;
    let vtbl = unsafe { collection.vtbl::<IMMDeviceCollectionVtbl>() };
    let mut count = 0u32;
    check(unsafe { (vtbl.get_count)(collection.0, &mut count) }, "count devices")?;
    let mut devices = Vec::with_capacity(count as usize);
    for index in 0..count {
        let mut device = ptr::null_mut();
        let hr = unsafe { (vtbl.item)(collection.0, index, &mut device) };
        if let Ok(device) = Com::from_out(device, hr, "device") {
            devices.push(device);
        }
    }
    Ok(devices)
}

fn default_render_device(enumerator: &Com) -> Result<Com> {
    let mut device = ptr::null_mut();
    let hr = unsafe { (enumerator.vtbl::<IMMDeviceEnumeratorVtbl>().get_default_audio_endpoint)(enumerator.0, E_RENDER, E_CONSOLE, &mut device) };
    Com::from_out(device, hr, "default output").map_err(|_| Error::NotFound)
}

fn device_by_id(enumerator: &Com, uid: &str) -> Result<Com> {
    let id = wide(uid);
    let mut device = ptr::null_mut();
    let hr = unsafe { (enumerator.vtbl::<IMMDeviceEnumeratorVtbl>().get_device)(enumerator.0, id.as_ptr(), &mut device) };
    Com::from_out(device, hr, "output device").map_err(|_| Error::NotFound)
}

fn device_id(device: &Com) -> Result<String> {
    let mut id = ptr::null_mut();
    check(unsafe { (device.vtbl::<IMMDeviceVtbl>().get_id)(device.0, &mut id) }, "device id")?;
    Ok(take_co_string(id))
}

fn device_name(device: &Com) -> String {
    let mut store = ptr::null_mut();
    let hr = unsafe { (device.vtbl::<IMMDeviceVtbl>().open_property_store)(device.0, STGM_READ, &mut store) };
    let Ok(store) = Com::from_out(store, hr, "property store") else { return String::new() };
    let mut value = PropVariant { vt: 0, _reserved: [0; 3], data: [0; 2] };
    let hr = unsafe { (store.vtbl::<IPropertyStoreVtbl>().get_value)(store.0, &PKEY_DEVICE_FRIENDLY_NAME, &mut value) };
    let name = if hr >= 0 && value.vt == VT_LPWSTR { from_wide(value.data[0] as *const u16) } else { String::new() };
    unsafe { PropVariantClear(&mut value) };
    name
}

fn activate(device: &Com, iid: &Guid, step: &'static str) -> Result<Com> {
    let mut out = ptr::null_mut();
    let hr = unsafe { (device.vtbl::<IMMDeviceVtbl>().activate)(device.0, iid, CLSCTX_ALL, ptr::null(), &mut out) };
    Com::from_out(out, hr, step)
}

/// A device's shared-mode mix format: channels, rate, channel mask.
struct MixFormat {
    channels: u16,
    sample_rate: u32,
    channel_mask: u32,
}

fn mix_format(client: &Com) -> Result<MixFormat> {
    let mut format = ptr::null_mut();
    check(unsafe { (client.vtbl::<IAudioClientVtbl>().get_mix_format)(client.0, &mut format) }, "mix format")?;
    if format.is_null() {
        return Err(Error::System { status: E_POINTER, step: "mix format" });
    }
    let base = unsafe { ptr::read_unaligned(format) };
    let channel_mask = if base.format_tag == WAVE_FORMAT_EXTENSIBLE && base.cb_size >= 22 {
        unsafe { ptr::read_unaligned(format as *const WaveFormatExtensible) }.channel_mask
    } else {
        0
    };
    unsafe { CoTaskMemFree(format as *mut c_void) };
    Ok(MixFormat { channels: base.channels.max(1), sample_rate: base.samples_per_sec, channel_mask })
}

fn device_info(device: &Com) -> Result<OutputDevice> {
    let uid = device_id(device)?;
    let name = device_name(device);
    let (channels, sample_rate) = match activate(device, &IID_IAUDIO_CLIENT, "audio client").and_then(|client| mix_format(&client)) {
        Ok(format) => (format.channels, format.sample_rate as f64),
        Err(_) => (0, 0.0),
    };
    Ok(OutputDevice { uid, name, channels, sample_rate })
}

pub fn outputs() -> Result<Vec<OutputDevice>> {
    let _com = ComScope::enter();
    let enumerator = enumerator()?;
    let mut devices: Vec<OutputDevice> = endpoints(&enumerator, E_RENDER)?.iter().filter_map(|device| device_info(device).ok()).collect();
    devices.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(devices)
}

pub fn default_output() -> Result<OutputDevice> {
    let _com = ComScope::enter();
    let enumerator = enumerator()?;
    device_info(&default_render_device(&enumerator)?)
}

/// Windows asks nobody before one app records another's output.
pub fn permission() -> Option<crate::Permission> {
    None
}

// ---- processes ----

#[derive(Clone)]
struct ProcessInfo {
    pid: u32,
    parent: u32,
    exe: String,
}

fn snapshot() -> Vec<ProcessInfo> {
    let snapshot = OwnedHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
    if snapshot.0 == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<ProcessEntry>() as u32;
    let mut list = Vec::new();
    let mut ok = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    while ok != 0 {
        let len = entry.exe_file.iter().position(|c| *c == 0).unwrap_or(entry.exe_file.len());
        list.push(ProcessInfo { pid: entry.process_id, parent: entry.parent_process_id, exe: String::from_utf16_lossy(&entry.exe_file[..len]) });
        ok = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    list
}

/// The package family of a packaged (Store) process, e.g.
/// `SpotifyAB.SpotifyMusic_zpdnekdrzrea0`; empty for a desktop one.
fn package_family(pid: u32) -> String {
    let process = OwnedHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) });
    if process.0.is_null() {
        return String::new();
    }
    let mut buffer = [0u16; 256];
    let mut length = buffer.len() as u32;
    if unsafe { GetPackageFamilyName(process.0, &mut length, buffer.as_mut_ptr()) } != 0 {
        return String::new();
    }
    from_wide(buffer.as_ptr())
}

/// Applications known under their macOS bundle id, as Windows names them:
/// the executable (the same for the Store package and the desktop
/// installer). A host names its player once, by bundle id, on every
/// platform.
const KNOWN_APPS: &[(&str, &str)] = &[("com.spotify.client", "Spotify.exe")];

/// Whether a process is the application `wanted` names: a known bundle id
/// ([`KNOWN_APPS`]) by its executable; otherwise the executable
/// (`Spotify.exe`, or without `.exe`) or the Store package family (whole,
/// or the name before the publisher hash). A known app is matched by its
/// executable alone: a package also runs system hosts (RuntimeBroker,
/// backgroundTaskHost) under its family. `family` is looked up only when
/// needed.
fn matches(wanted: &str, exe: &str, family: &mut dyn FnMut() -> String) -> bool {
    let stem = |name: &str| -> String { name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name).to_ascii_lowercase() };
    if let Some((_, known)) = KNOWN_APPS.iter().find(|(bundle, _)| bundle.eq_ignore_ascii_case(wanted)) {
        return stem(exe) == stem(known);
    }
    if stem(exe) == stem(wanted) {
        return true;
    }
    let family = family();
    !family.is_empty() && (family.eq_ignore_ascii_case(wanted) || family.split('_').next().is_some_and(|name| name.eq_ignore_ascii_case(wanted)))
}

/// The processes the sources name, as the roots of their trees: a match
/// whose parent also matches is covered by that parent's tree.
fn resolve_roots(sources: &[Source], processes: &[ProcessInfo]) -> Vec<u32> {
    let mut chosen: Vec<u32> = Vec::new();
    for source in sources {
        match source {
            Source::Pid(pid) => {
                if processes.iter().any(|process| process.pid == *pid) {
                    chosen.push(*pid);
                }
            }
            Source::BundleId(bundle) => {
                for process in processes {
                    if matches(bundle, &process.exe, &mut || package_family(process.pid)) {
                        chosen.push(process.pid);
                    }
                }
            }
        }
    }
    chosen.sort_unstable();
    chosen.dedup();
    let roots: Vec<u32> = chosen
        .iter()
        .copied()
        .filter(|pid| {
            let parent = processes.iter().find(|process| process.pid == *pid).map_or(0, |process| process.parent);
            parent == *pid || !chosen.contains(&parent)
        })
        .collect();
    roots
}

/// One audio session: its process, and whether it is playing.
struct Session {
    pid: u32,
    active: bool,
}

fn sessions(enumerator: &Com, flow: i32) -> Vec<Session> {
    let mut list = Vec::new();
    let Ok(devices) = endpoints(enumerator, flow) else { return list };
    for device in devices {
        let Ok(manager) = activate(&device, &IID_IAUDIO_SESSION_MANAGER2, "session manager") else { continue };
        let mut sessions = ptr::null_mut();
        let hr = unsafe { (manager.vtbl::<IAudioSessionManager2Vtbl>().get_session_enumerator)(manager.0, &mut sessions) };
        let Ok(sessions) = Com::from_out(sessions, hr, "sessions") else { continue };
        let vtbl = unsafe { sessions.vtbl::<IAudioSessionEnumeratorVtbl>() };
        let mut count = 0i32;
        if unsafe { (vtbl.get_count)(sessions.0, &mut count) } < 0 {
            continue;
        }
        for index in 0..count {
            let mut control = ptr::null_mut();
            let hr = unsafe { (vtbl.get_session)(sessions.0, index, &mut control) };
            let Ok(control) = Com::from_out(control, hr, "session") else { continue };
            let Ok(control) = control.query(&IID_IAUDIO_SESSION_CONTROL2, "session control") else { continue };
            let c = unsafe { control.vtbl::<IAudioSessionControl2Vtbl>() };
            let mut pid = 0u32;
            // AUDCLNT_S_NO_SINGLE_PROCESS still names the first process.
            if unsafe { (c.get_process_id)(control.0, &mut pid) } < 0 || pid == 0 {
                continue;
            }
            let mut state = 0u32;
            unsafe { (c.get_state)(control.0, &mut state) };
            list.push(Session { pid, active: state == AUDIO_SESSION_STATE_ACTIVE });
        }
    }
    list
}

pub fn processes() -> Result<Vec<AudioProcess>> {
    let _com = ComScope::enter();
    let enumerator = enumerator()?;
    let all = snapshot();
    let mut processes: Vec<AudioProcess> = Vec::new();
    for (flow, output) in [(E_RENDER, true), (E_CAPTURE, false)] {
        for session in sessions(&enumerator, flow) {
            let index = match processes.iter().position(|process| process.pid == session.pid) {
                Some(index) => index,
                None => {
                    let exe = all.iter().find(|process| process.pid == session.pid).map(|process| process.exe.clone()).unwrap_or_default();
                    let name = exe.strip_suffix(".exe").unwrap_or(&exe).to_string();
                    processes.push(AudioProcess {
                        pid: session.pid,
                        name: if name.is_empty() { format!("pid {}", session.pid) } else { name },
                        bundle_id: exe,
                        output_running: false,
                        input_running: false,
                    });
                    processes.len() - 1
                }
            };
            if output {
                processes[index].output_running |= session.active;
            } else {
                processes[index].input_running |= session.active;
            }
        }
    }
    processes.sort_by(|a, b| b.output_running.cmp(&a.output_running).then_with(|| a.name.cmp(&b.name)).then(a.pid.cmp(&b.pid)));
    Ok(processes)
}

// ---- the process loopback capture ----

/// The `IActivateAudioInterfaceCompletionHandler` activation calls back:
/// it signals `event`. Agile, as the activation requires.
#[repr(C)]
struct Handler {
    vtbl: *const HandlerVtbl,
    refs: AtomicU32,
    event: OwnedHandle,
}

#[repr(C)]
struct HandlerVtbl {
    base: IUnknownVtbl,
    activate_completed: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Hresult,
}

static HANDLER_VTBL: HandlerVtbl = HandlerVtbl {
    base: IUnknownVtbl { query_interface: handler_query, add_ref: handler_add_ref, release: handler_release },
    activate_completed: handler_completed,
};

unsafe extern "system" fn handler_query(this: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    let iid = &*iid;
    if *iid == IID_IUNKNOWN || *iid == IID_IACTIVATE_COMPLETION_HANDLER || *iid == IID_IAGILE_OBJECT {
        handler_add_ref(this);
        *out = this;
        S_OK
    } else {
        *out = ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn handler_add_ref(this: *mut c_void) -> u32 {
    (*(this as *mut Handler)).refs.fetch_add(1, Ordering::AcqRel) + 1
}

unsafe extern "system" fn handler_release(this: *mut c_void) -> u32 {
    let left = (*(this as *mut Handler)).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        drop(Box::from_raw(this as *mut Handler));
    }
    left
}

unsafe extern "system" fn handler_completed(this: *mut c_void, _operation: *mut c_void) -> Hresult {
    SetEvent((*(this as *mut Handler)).event.0);
    S_OK
}

/// A loopback client of `pid`'s process tree, not yet initialized.
fn activate_loopback(pid: u32) -> Result<Com> {
    let params = ActivationParams {
        activation_type: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        target_process_id: pid,
        process_loopback_mode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    };
    let blob = PropVariant {
        vt: VT_BLOB,
        _reserved: [0; 3],
        data: [std::mem::size_of::<ActivationParams>(), &params as *const ActivationParams as usize],
    };
    let handler = Box::into_raw(Box::new(Handler { vtbl: &HANDLER_VTBL, refs: AtomicU32::new(1), event: OwnedHandle::event(true)? }));
    // Our reference, released when this returns.
    let handler_ref = Com(handler as *mut c_void);
    let path = wide("VAD\\Process_Loopback");
    let mut operation = ptr::null_mut();
    let hr = unsafe { ActivateAudioInterfaceAsync(path.as_ptr(), &IID_IAUDIO_CLIENT, &blob, handler as *mut c_void, &mut operation) };
    let operation = Com::from_out(operation, hr, "activate process loopback")?;
    let event = unsafe { (*handler).event.0 };
    if unsafe { WaitForSingleObject(event, ACTIVATE_TIMEOUT_MS) } != WAIT_OBJECT_0 {
        return Err(Error::System { status: -1, step: "activate process loopback (timed out)" });
    }
    drop(handler_ref);
    let mut result = 0;
    let mut unknown = ptr::null_mut();
    check(unsafe { (operation.vtbl::<IActivateAudioInterfaceAsyncOperationVtbl>().get_activate_result)(operation.0, &mut result, &mut unknown) }, "activation result")?;
    if !unknown.is_null() && result < 0 {
        drop(Com(unknown));
    }
    let unknown = Com::from_out(unknown, result, "activate process loopback")?;
    unknown.query(&IID_IAUDIO_CLIENT, "loopback client")
}

// ---- the gain timeline ----

/// When a session volume change shows in the capture, from the call
/// (100 ns units): measured on Windows 11 26200, 28 to 50 ms after it, as
/// a step within one 10 ms packet. A packet ending before `from + EARLY`
/// was mixed at the old volume, one starting after `until + LATE` at the
/// new one; in between the packet's own level tells.
const EARLY_HNS: u64 = 100_000;
const LATE_HNS: u64 = 800_000;
/// A change this large (either way) is found in the capture's level.
const BIG_STEP: f32 = 16.0;
const TIMELINE_SLOTS: usize = 32;

/// One change of the tapped sessions' volume, relative to the level the
/// person set (1 = as they set it).
struct GainChange {
    /// The call's start and end, 100 ns units.
    from: AtomicU64,
    until: AtomicU64,
    before: AtomicU32,
    gain: AtomicU32,
}

/// Which gain each captured packet was mixed at, so the capture is
/// brought back to the level the person set packet by packet. Written by
/// the watch thread only, read by the render thread without locking.
struct Timeline {
    slots: [GainChange; TIMELINE_SLOTS],
    count: AtomicUsize,
}

impl Timeline {
    fn new() -> Timeline {
        Timeline {
            slots: std::array::from_fn(|_| GainChange { from: AtomicU64::new(0), until: AtomicU64::new(0), before: AtomicU32::new(1f32.to_bits()), gain: AtomicU32::new(1f32.to_bits()) }),
            count: AtomicUsize::new(0),
        }
    }

    fn current(&self) -> f32 {
        let n = self.count.load(Ordering::Acquire);
        if n == 0 { 1.0 } else { f32::from_bits(self.slots[(n - 1) % TIMELINE_SLOTS].gain.load(Ordering::Relaxed)) }
    }

    fn push(&self, from: u64, until: u64, gain: f32) {
        let before = self.current();
        let n = self.count.load(Ordering::Relaxed);
        let slot = &self.slots[n % TIMELINE_SLOTS];
        slot.from.store(from, Ordering::Relaxed);
        slot.until.store(until, Ordering::Relaxed);
        slot.before.store(before.to_bits(), Ordering::Relaxed);
        slot.gain.store(gain.to_bits(), Ordering::Relaxed);
        self.count.store(n + 1, Ordering::Release);
    }

    /// The change `index` (0-based): its call, and the gains before and
    /// after it.
    fn change(&self, index: usize) -> (u64, u64, f32, f32) {
        let slot = &self.slots[index % TIMELINE_SLOTS];
        (
            slot.from.load(Ordering::Relaxed),
            slot.until.load(Ordering::Relaxed),
            f32::from_bits(slot.before.load(Ordering::Relaxed)),
            f32::from_bits(slot.gain.load(Ordering::Relaxed)),
        )
    }
}

/// Which gain each packet of one capture was mixed at.
#[derive(Default)]
struct Landing {
    /// Changes (by count) seen landed in this capture's level.
    landed: usize,
    /// The previous packet's peak, as captured.
    last_raw: f32,
}

impl Landing {
    /// The gain a packet stamped `start..end`, peaking at `raw`, was mixed
    /// at. A large change that may have landed in it is looked for in its
    /// level against the packet before; a small one takes the larger gain,
    /// so a packet is never boosted past the level it was captured at.
    fn gain(&mut self, timeline: &Timeline, start: u64, end: u64, raw: f32) -> f32 {
        let n = timeline.count.load(Ordering::Acquire);
        let kept = n.min(TIMELINE_SLOTS);
        let mut gain = if n == 0 { 1.0 } else { timeline.change(n - kept).2 };
        for index in (n - kept..n).rev() {
            let (from, until, before, after) = timeline.change(index);
            if from + EARLY_HNS > end {
                continue;
            }
            gain = if start > until + LATE_HNS || self.landed > index {
                after
            } else {
                let ratio = after / before.max(f32::MIN_POSITIVE);
                let big = !(1.0 / BIG_STEP..=BIG_STEP).contains(&ratio);
                if big && raw > 1e-12 && self.last_raw > 1e-12 {
                    let jump = raw / self.last_raw;
                    let landed = if ratio < 1.0 { jump < ratio.sqrt() } else { jump > ratio.sqrt() };
                    if landed {
                        self.landed = index + 1;
                        after
                    } else {
                        before
                    }
                } else {
                    after.max(before)
                }
            };
            break;
        }
        self.last_raw = raw;
        gain
    }
}

// ---- the capture ----

/// A running capture of one process tree, in stereo float at the route's
/// rate, with the frames read but not yet played, at the level the person
/// set (the ducking undone).
struct Capture {
    pid: u32,
    client: Com,
    capture: Com,
    /// Signalled by the client; nobody waits on it (the render thread reads
    /// every period), but event mode needs one.
    _event: OwnedHandle,
    fifo: Fifo,
    sample_rate: u32,
    landing: Landing,
    guard: Guard,
}

/// The last line against a wrong gain: a brought-back packet far louder
/// than full scale, or 36 dB over the level just before (and loud), is
/// passed as captured, never boosted. Three such packets in a row are the
/// music, and pass.
#[derive(Default)]
struct Guard {
    envelope: f32,
    suspicious: u32,
}

impl Guard {
    /// Whether a packet brought back to `peak` may be played so.
    fn allows(&mut self, peak: f32) -> bool {
        let jump = peak > 0.1 && peak > self.envelope * 64.0 && self.suspicious < 3;
        let allowed = peak <= BLAST_GUARD && !jump;
        self.suspicious = if jump { self.suspicious + 1 } else { 0 };
        if allowed {
            self.envelope = peak.max(self.envelope * 0.97);
        }
        allowed
    }

    /// A packet played as captured: the level to compare with.
    fn observe(&mut self, peak: f32) {
        self.suspicious = 0;
        self.envelope = peak.max(self.envelope * 0.97);
    }
}

impl Capture {
    fn open(pid: u32, sample_rate: u32, prime_frames: usize) -> Result<Capture> {
        let client = activate_loopback(pid)?;
        let format = WaveFormatEx {
            format_tag: WAVE_FORMAT_IEEE_FLOAT,
            channels: 2,
            samples_per_sec: sample_rate,
            avg_bytes_per_sec: sample_rate * 8,
            block_align: 8,
            bits_per_sample: 32,
            cb_size: 0,
        };
        let vtbl = unsafe { client.vtbl::<IAudioClientVtbl>() };
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        check(
            unsafe { (vtbl.initialize)(client.0, AUDCLNT_SHAREMODE_SHARED, flags, CAPTURE_BUFFER, 0, &format as *const WaveFormatEx as *const c_void, ptr::null()) },
            "initialize loopback",
        )?;
        let event = OwnedHandle::event(false)?;
        check(unsafe { (vtbl.set_event_handle)(client.0, event.0) }, "loopback event")?;
        let mut capture = ptr::null_mut();
        let hr = unsafe { (vtbl.get_service)(client.0, &IID_IAUDIO_CAPTURE_CLIENT, &mut capture) };
        let capture = Com::from_out(capture, hr, "capture client")?;
        check(unsafe { (vtbl.start)(client.0) }, "start loopback")?;
        // Room for the capture buffer plus the priming margin.
        let capacity = (sample_rate as usize / 5) + prime_frames * 4;
        Ok(Capture { pid, client, capture, _event: event, fifo: Fifo::new(capacity, prime_frames), sample_rate, landing: Landing::default(), guard: Guard::default() })
    }

    /// Move every packet the client holds into the fifo, each brought back
    /// by the gain it was mixed at. An error means the capture is gone.
    fn pull(&mut self, shared: &Shared) -> std::result::Result<(), Hresult> {
        let (timeline, guarded) = (&shared.timeline, &shared.guarded);
        let vtbl = unsafe { self.capture.vtbl::<IAudioCaptureClientVtbl>() };
        loop {
            let mut next = 0u32;
            let hr = unsafe { (vtbl.get_next_packet_size)(self.capture.0, &mut next) };
            if hr < 0 {
                return Err(hr);
            }
            if next == 0 {
                return Ok(());
            }
            let mut data = ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            let mut stamp = 0u64;
            let hr = unsafe { (vtbl.get_buffer)(self.capture.0, &mut data, &mut frames, &mut flags, ptr::null_mut(), &mut stamp) };
            if hr < 0 {
                return Err(hr);
            }
            if frames == 0 {
                return Ok(());
            }
            if flags & AUDCLNT_BUFFERFLAGS_SILENT != 0 || data.is_null() {
                self.fifo.push_silence(frames as usize);
                self.landing.last_raw = 0.0;
            } else {
                let samples = unsafe { std::slice::from_raw_parts(data as *const f32, frames as usize * 2) };
                // Unstamped, the packet counts as older than it is: an older
                // stamp can only pick the larger gain of a change.
                let now = now_hns();
                shared.stamp_lag.store(if stamp == 0 { 0 } else { now.saturating_sub(stamp) }, Ordering::Relaxed);
                let start = if stamp == 0 { now.saturating_sub(1_000_000) } else { stamp };
                let end = start + frames as u64 * 10_000_000 / self.sample_rate.max(1) as u64;
                let raw = samples.iter().fold(0f32, |peak, sample| peak.max(sample.abs()));
                let gain = self.landing.gain(timeline, start, end, raw);
                let mut scale = if gain > 0.0 { 1.0 / gain } else { 1.0 };
                if scale > 1.0 && !self.guard.allows(raw * scale) {
                    scale = 1.0;
                    guarded.fetch_add(1, Ordering::Relaxed);
                } else if scale <= 1.0 {
                    self.guard.observe(raw * scale);
                }
                self.fifo.push_scaled(samples, scale);
            }
            let hr = unsafe { (vtbl.release_buffer)(self.capture.0, frames) };
            if hr < 0 {
                return Err(hr);
            }
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe { (self.client.vtbl::<IAudioClientVtbl>().stop)(self.client.0) };
    }
}

/// Stereo frames between a capture and the render clock. Playback starts
/// once `prime` frames are in (a margin against the two clocks' packet
/// timing), stops again when it runs dry, and a backlog past the margin
/// (the capture's clock running ahead) is dropped from the oldest end.
struct Fifo {
    buffer: Vec<f32>,
    /// Frames the buffer holds.
    capacity: usize,
    read: usize,
    len: usize,
    prime: usize,
    primed: bool,
}

impl Fifo {
    fn new(capacity: usize, prime: usize) -> Fifo {
        let capacity = capacity.max(prime * 4).max(1);
        Fifo { buffer: vec![0.0; capacity * 2], capacity, read: 0, len: 0, prime, primed: false }
    }

    fn push_frame(&mut self, left: f32, right: f32) {
        if self.len == self.capacity {
            self.read = (self.read + 1) % self.capacity;
            self.len -= 1;
        }
        let at = (self.read + self.len) % self.capacity;
        self.buffer[at * 2] = left;
        self.buffer[at * 2 + 1] = right;
        self.len += 1;
    }

    fn push_scaled(&mut self, samples: &[f32], scale: f32) {
        for [left, right] in samples.as_chunks::<2>().0 {
            self.push_frame(*left * scale, *right * scale);
        }
    }

    fn push_silence(&mut self, frames: usize) {
        for _ in 0..frames {
            self.push_frame(0.0, 0.0);
        }
    }

    /// Add up to `frames` frames into `block`, as they are; `n` taken.
    fn take_into(&mut self, block: &mut [f32], frames: usize) -> usize {
        let take = frames.min(self.len);
        for frame in 0..take {
            let at = (self.read + frame) % self.capacity;
            block[frame * 2] += self.buffer[at * 2];
            block[frame * 2 + 1] += self.buffer[at * 2 + 1];
        }
        self.read = (self.read + take) % self.capacity;
        self.len -= take;
        take
    }

    /// Add `frames` frames into `block` on the render clock: primed first,
    /// a backlog cut, and unprimed again when it runs dry.
    fn mix_into(&mut self, block: &mut [f32], frames: usize) {
        if !self.primed {
            if self.len < self.prime {
                return;
            }
            self.primed = true;
        }
        let backlog = self.prime * 3;
        if self.len > backlog + frames {
            let drop = self.len - backlog - frames;
            self.read = (self.read + drop) % self.capacity;
            self.len -= drop;
        }
        if self.take_into(block, frames) < frames {
            self.primed = false;
        }
    }
}

// ---- the tapped sessions' volume ----

/// The player's audio session ducked to this while processing: 2^-13
/// (-78.3 dB). A power of two, so bringing the capture back (x 8192) only
/// moves the float exponent: nothing is lost on the round trip.
const DUCK: f32 = 1.0 / 8192.0;
/// Past this, a brought-back sample is a mistake, not sound.
const BLAST_GUARD: f32 = 4.0;
/// How long a volume step of the crossfade lasts: past the time a change
/// takes to show in the capture, so one is in doubt at a time.
const STEP: Duration = Duration::from_millis(100);
/// How fast our output's share follows the crossfade: a full swing in 10 ms.
const FADE_SECS: f32 = 0.010;
const RECORD_FILE: &str = "audio-route-ducked.txt";

/// The session gains of the crossfade from the person's level down to
/// [`DUCK`]: even steps in amplitude to a quarter (the two paths trade
/// places, the small steps' doubt costing our share a little), then one
/// step, found in the capture's level, to the bottom.
fn duck_steps() -> Vec<f32> {
    vec![1.0, 0.75, 0.5, 0.25, DUCK]
}

/// Session volume changes: a change that is not ours (the person moved
/// the player's slider in the volume mixer) silences our output at once
/// and wakes the watch thread, which gives the player its level back and
/// ducks again from there.
#[repr(C)]
struct Events {
    vtbl: *const EventsVtbl,
    refs: AtomicU32,
    shared: Arc<Shared>,
    /// The volume this crate last set on the session.
    expected: AtomicU32,
}

#[repr(C)]
struct EventsVtbl {
    base: IUnknownVtbl,
    on_display_name_changed: unsafe extern "system" fn(*mut c_void, *const u16, *const Guid) -> Hresult,
    on_icon_path_changed: unsafe extern "system" fn(*mut c_void, *const u16, *const Guid) -> Hresult,
    on_simple_volume_changed: unsafe extern "system" fn(*mut c_void, f32, Bool, *const Guid) -> Hresult,
    on_channel_volume_changed: unsafe extern "system" fn(*mut c_void, u32, *const f32, u32, *const Guid) -> Hresult,
    on_grouping_param_changed: unsafe extern "system" fn(*mut c_void, *const Guid, *const Guid) -> Hresult,
    on_state_changed: unsafe extern "system" fn(*mut c_void, u32) -> Hresult,
    on_session_disconnected: unsafe extern "system" fn(*mut c_void, u32) -> Hresult,
}

static EVENTS_VTBL: EventsVtbl = EventsVtbl {
    base: IUnknownVtbl { query_interface: events_query, add_ref: events_add_ref, release: events_release },
    on_display_name_changed: events_ignore_text,
    on_icon_path_changed: events_ignore_text,
    on_simple_volume_changed: events_volume,
    on_channel_volume_changed: events_ignore_channels,
    on_grouping_param_changed: events_ignore_grouping,
    on_state_changed: events_ignore_u32,
    on_session_disconnected: events_ignore_u32,
};

unsafe extern "system" fn events_query(this: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    let iid = &*iid;
    if *iid == IID_IUNKNOWN || *iid == IID_IAUDIO_SESSION_EVENTS || *iid == IID_IAGILE_OBJECT {
        events_add_ref(this);
        *out = this;
        S_OK
    } else {
        *out = ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn events_add_ref(this: *mut c_void) -> u32 {
    (*(this as *mut Events)).refs.fetch_add(1, Ordering::AcqRel) + 1
}

unsafe extern "system" fn events_release(this: *mut c_void) -> u32 {
    let left = (*(this as *mut Events)).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        drop(Box::from_raw(this as *mut Events));
    }
    left
}

unsafe extern "system" fn events_volume(this: *mut c_void, volume: f32, _mute: Bool, context: *const Guid) -> Hresult {
    let events = &*(this as *mut Events);
    if !context.is_null() && *context == OUR_CONTEXT {
        return S_OK;
    }
    // A mute toggle keeps the volume: the capture follows it by itself.
    let expected = f32::from_bits(events.expected.load(Ordering::Relaxed));
    if (volume - expected).abs() <= expected * 1e-4 {
        return S_OK;
    }
    events.shared.fade_target.store(0f32.to_bits(), Ordering::Relaxed);
    events.shared.external.store(true, Ordering::Release);
    SetEvent(events.shared.wake.0);
    S_OK
}

unsafe extern "system" fn events_ignore_text(_: *mut c_void, _: *const u16, _: *const Guid) -> Hresult {
    S_OK
}

unsafe extern "system" fn events_ignore_channels(_: *mut c_void, _: u32, _: *const f32, _: u32, _: *const Guid) -> Hresult {
    S_OK
}

unsafe extern "system" fn events_ignore_grouping(_: *mut c_void, _: *const Guid, _: *const Guid) -> Hresult {
    S_OK
}

unsafe extern "system" fn events_ignore_u32(_: *mut c_void, _: u32) -> Hresult {
    S_OK
}

/// One of the player's sessions while it is ducked (or on its way).
struct Ducked {
    identifier: String,
    control: Com,
    volume: Com,
    events: *mut Events,
    /// The level the person set.
    original: f32,
}

unsafe impl Send for Ducked {}

impl Ducked {
    fn set(&self, level: f32) -> Hresult {
        unsafe { (*self.events).expected.store(level.to_bits(), Ordering::Relaxed) };
        unsafe { (self.volume.vtbl::<ISimpleAudioVolumeVtbl>().set_master_volume)(self.volume.0, level, &OUR_CONTEXT) }
    }

    fn get(&self) -> Option<f32> {
        let mut level = 0f32;
        (unsafe { (self.volume.vtbl::<ISimpleAudioVolumeVtbl>().get_master_volume)(self.volume.0, &mut level) } >= 0).then_some(level)
    }
}

impl Drop for Ducked {
    fn drop(&mut self) {
        unsafe {
            (self.control.vtbl::<IAudioSessionControl2Vtbl>().unregister_audio_session_notification)(self.control.0, self.events as *mut c_void);
            events_release(self.events as *mut c_void);
        }
    }
}

/// The render sessions of `pids`: identifier (stable across the player's
/// relaunch) and control.
fn sessions_of(enumerator: &Com, pids: &[u32]) -> Vec<(String, Com)> {
    let mut found = Vec::new();
    let Ok(devices) = endpoints(enumerator, E_RENDER) else { return found };
    for device in devices {
        let Ok(manager) = activate(&device, &IID_IAUDIO_SESSION_MANAGER2, "session manager") else { continue };
        let mut list = ptr::null_mut();
        let hr = unsafe { (manager.vtbl::<IAudioSessionManager2Vtbl>().get_session_enumerator)(manager.0, &mut list) };
        let Ok(list) = Com::from_out(list, hr, "sessions") else { continue };
        let vtbl = unsafe { list.vtbl::<IAudioSessionEnumeratorVtbl>() };
        let mut count = 0i32;
        if unsafe { (vtbl.get_count)(list.0, &mut count) } < 0 {
            continue;
        }
        for index in 0..count {
            let mut control = ptr::null_mut();
            let hr = unsafe { (vtbl.get_session)(list.0, index, &mut control) };
            let Ok(control) = Com::from_out(control, hr, "session") else { continue };
            let Ok(control) = control.query(&IID_IAUDIO_SESSION_CONTROL2, "session control") else { continue };
            let c = unsafe { control.vtbl::<IAudioSessionControl2Vtbl>() };
            let mut pid = 0u32;
            if unsafe { (c.get_process_id)(control.0, &mut pid) } < 0 || !pids.contains(&pid) {
                continue;
            }
            let mut identifier = ptr::null_mut();
            unsafe { (c.get_session_identifier)(control.0, &mut identifier) };
            found.push((take_co_string(identifier), control));
        }
    }
    found
}

/// Every render session, by identifier, with its volume control.
fn all_session_volumes(enumerator: &Com) -> Vec<(String, Com)> {
    let mut found = Vec::new();
    let Ok(devices) = endpoints(enumerator, E_RENDER) else { return found };
    for device in devices {
        let Ok(manager) = activate(&device, &IID_IAUDIO_SESSION_MANAGER2, "session manager") else { continue };
        let mut list = ptr::null_mut();
        let hr = unsafe { (manager.vtbl::<IAudioSessionManager2Vtbl>().get_session_enumerator)(manager.0, &mut list) };
        let Ok(list) = Com::from_out(list, hr, "sessions") else { continue };
        let vtbl = unsafe { list.vtbl::<IAudioSessionEnumeratorVtbl>() };
        let mut count = 0i32;
        unsafe { (vtbl.get_count)(list.0, &mut count) };
        for index in 0..count {
            let mut control = ptr::null_mut();
            let hr = unsafe { (vtbl.get_session)(list.0, index, &mut control) };
            let Ok(control) = Com::from_out(control, hr, "session") else { continue };
            let Ok(control) = control.query(&IID_IAUDIO_SESSION_CONTROL2, "session control") else { continue };
            let mut identifier = ptr::null_mut();
            unsafe { (control.vtbl::<IAudioSessionControl2Vtbl>().get_session_identifier)(control.0, &mut identifier) };
            let Ok(volume) = control.query(&IID_ISIMPLE_AUDIO_VOLUME, "session volume") else { continue };
            found.push((take_co_string(identifier), volume));
        }
    }
    found
}

/// The levels the person set on sessions ducked now, kept on disk so a
/// crash cannot leave the player near silent: `identifier \t level`.
fn read_records(dir: &Path) -> Vec<(String, f32)> {
    let Ok(text) = std::fs::read_to_string(dir.join(RECORD_FILE)) else { return Vec::new() };
    text.lines()
        .filter_map(|line| {
            let (identifier, level) = line.rsplit_once('\t')?;
            Some((identifier.to_string(), level.trim().parse().ok()?))
        })
        .collect()
}

fn write_records(dir: &Option<PathBuf>, ducked: &[Ducked]) {
    let Some(dir) = dir else { return };
    let path = dir.join(RECORD_FILE);
    if ducked.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let text: String = ducked.iter().map(|d| format!("{}\t{}\n", d.identifier, d.original)).collect();
    let _ = std::fs::create_dir_all(dir);
    // Written whole and renamed into place: a crash mid-write leaves the
    // previous record.
    let temp = dir.join(format!("{RECORD_FILE}.tmp"));
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// Give back the levels a route that did not close (the app crashed or was
/// killed while processing) left ducked, as recorded in `state_dir`. A
/// session the person has since turned up is left as they set it.
pub fn restore_after_crash(state_dir: &Path) {
    let records = read_records(state_dir);
    if records.is_empty() {
        let _ = std::fs::remove_file(state_dir.join(RECORD_FILE));
        return;
    }
    let _com = ComScope::enter();
    let steps = duck_steps();
    if let Ok(enumerator) = enumerator() {
        for (identifier, volume) in all_session_volumes(&enumerator) {
            let Some((_, original)) = records.iter().find(|(id, _)| *id == identifier) else { continue };
            restore_if_ducked(&volume, *original, &steps);
        }
    }
    let _ = std::fs::remove_file(state_dir.join(RECORD_FILE));
}

/// Set a session back to `original` when it sits at one of the levels the
/// crossfade sets (exactly `original` times a step): a level the person
/// chose is left alone.
fn restore_if_ducked(volume: &Com, original: f32, steps: &[f32]) {
    let v = unsafe { volume.vtbl::<ISimpleAudioVolumeVtbl>() };
    let mut level = 0f32;
    if unsafe { (v.get_master_volume)(volume.0, &mut level) } < 0 {
        return;
    }
    if steps.iter().skip(1).any(|step| level == original * step) {
        unsafe { (v.set_master_volume)(volume.0, original, &OUR_CONTEXT) };
    }
}

// ---- the route ----

struct ProcSlot {
    processor: Box<dyn Processor>,
}

/// Where the route stands: monitoring, crossfading, or processing.
const PHASE_MONITOR: u32 = 0;
const PHASE_FADING: u32 = 1;
const PHASE_PROCESSING: u32 = 2;

struct Shared {
    processor: AtomicPtr<ProcSlot>,
    epoch: AtomicU64,
    /// Captures running now, for [`Route::describe`].
    captures: AtomicUsize,
    /// Blocks where a capture ran dry mid-block.
    underruns: AtomicU64,
    /// Packets passed without bringing back (see [`BLAST_GUARD`]).
    guarded: AtomicU64,
    /// How old the last captured packet's stamp was when read, 100 ns
    /// (0: the client does not stamp packets).
    stamp_lag: AtomicU64,
    timeline: Timeline,
    /// Processing asked for ([`Route::set_processing`]).
    want_processing: AtomicBool,
    /// The render stream runs (the watch thread decides).
    render_on: AtomicBool,
    /// Our output's share of the crossfade, f32 bits.
    fade_target: AtomicU32,
    /// A session volume change that is not ours arrived.
    external: AtomicBool,
    phase: AtomicU32,
    /// The ducked level read back differed from the level asked for.
    mismatch: AtomicBool,
    wake: OwnedHandle,
}

enum Command {
    Add(Capture),
    Remove(u32),
}

pub struct Route {
    shared: Arc<Shared>,
    stop: Arc<OwnedHandle>,
    render: Option<JoinHandle<()>>,
    watch: Option<JoinHandle<()>>,
    retired: Mutex<Vec<(u64, *mut ProcSlot)>>,
    sample_rate: f64,
    device: String,
    out_channels: u16,
    targets: Arc<Mutex<Vec<u32>>>,
}

unsafe impl Send for Route {}
unsafe impl Sync for Route {}

impl Drop for Route {
    fn drop(&mut self) {
        // Crossfade back to the player's own path before the output stops.
        let _ = self.set_processing(false);
        let deadline = std::time::Instant::now() + Duration::from_millis(1500);
        while self.shared.phase.load(Ordering::Acquire) != PHASE_MONITOR && std::time::Instant::now() < deadline {
            unsafe { WaitForSingleObject(self.stop.0, 10) };
        }
        unsafe { SetEvent(self.stop.0) };
        unsafe { SetEvent(self.shared.wake.0) };
        if let Some(watch) = self.watch.take() {
            let _ = watch.join();
        }
        if let Some(render) = self.render.take() {
            let _ = render.join();
        }
        drop_slot(self.shared.processor.swap(ptr::null_mut(), Ordering::AcqRel));
        if let Ok(mut retired) = self.retired.lock() {
            for (_, slot) in retired.drain(..) {
                drop_slot(slot);
            }
        }
    }
}

impl Route {
    pub fn open(config: RouteConfig, processor: Box<dyn Processor>) -> Result<Self> {
        if windows_build() < 19041 {
            return Err(Error::Unsupported);
        }
        if config.sources.is_empty() {
            return Err(Error::NotFound);
        }
        if let Some(dir) = &config.state_dir {
            restore_after_crash(dir);
        }
        // Nothing to tap: the host tries again later, like on macOS.
        if resolve_roots(&config.sources, &snapshot()).is_empty() {
            return Err(Error::NotFound);
        }
        let shared = Arc::new(Shared {
            processor: AtomicPtr::new(Box::into_raw(Box::new(ProcSlot { processor }))),
            epoch: AtomicU64::new(0),
            captures: AtomicUsize::new(0),
            underruns: AtomicU64::new(0),
            guarded: AtomicU64::new(0),
            stamp_lag: AtomicU64::new(0),
            timeline: Timeline::new(),
            want_processing: AtomicBool::new(config.processing),
            render_on: AtomicBool::new(false),
            fade_target: AtomicU32::new(0f32.to_bits()),
            external: AtomicBool::new(false),
            phase: AtomicU32::new(PHASE_MONITOR),
            mismatch: AtomicBool::new(false),
            wake: OwnedHandle::event(false)?,
        });
        let stop = Arc::new(OwnedHandle::event(true)?);
        let (commands, command_rx) = mpsc::sync_channel::<Command>(MAX_CAPTURES);
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, usize, u16, String)>>();
        let render = {
            let (shared, stop, output) = (Arc::clone(&shared), Arc::clone(&stop), config.output.clone());
            std::thread::Builder::new()
                .name("audio-route-render".into())
                .spawn(move || render_thread(output, shared, stop, command_rx, ready_tx))
                .map_err(|_| Error::System { status: -1, step: "start render thread" })?
        };
        let (sample_rate, prime, out_channels, device) = match ready_rx.recv() {
            Ok(Ok(ready)) => ready,
            Ok(Err(error)) => {
                let _ = render.join();
                drop_slot(shared.processor.swap(ptr::null_mut(), Ordering::AcqRel));
                return Err(error);
            }
            Err(_) => {
                let _ = render.join();
                drop_slot(shared.processor.swap(ptr::null_mut(), Ordering::AcqRel));
                return Err(Error::System { status: -1, step: "render thread" });
            }
        };
        let targets = Arc::new(Mutex::new(Vec::new()));
        let (first_tx, first_rx) = mpsc::channel::<Result<()>>();
        let watch = {
            let watch = Watch {
                sources: config.sources.clone(),
                duck: config.mute != Mute::HearOriginal,
                state_dir: config.state_dir.clone(),
                sample_rate,
                prime,
                shared: Arc::clone(&shared),
                stop: Arc::clone(&stop),
                commands,
                targets: Arc::clone(&targets),
            };
            std::thread::Builder::new()
                .name("audio-route-watch".into())
                .spawn(move || watch.run(first_tx))
                .map_err(|_| Error::System { status: -1, step: "start watch thread" })
        };
        let mut route = Route {
            shared,
            stop,
            render: Some(render),
            watch: None,
            retired: Mutex::new(Vec::new()),
            sample_rate: sample_rate as f64,
            device,
            out_channels,
            targets,
        };
        route.watch = Some(watch?);
        match first_rx.recv() {
            Ok(Ok(())) => Ok(route),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(Error::System { status: -1, step: "watch thread" }),
        }
    }

    /// Monitoring (`false`): the player plays by itself, untouched, and the
    /// processor only sees a copy; nothing is played here. Processing
    /// (`true`): the player's session is ducked to 2^-13, the capture is
    /// brought back by 2^13, and the processor's output is played. The
    /// switch is a crossfade of about 0.3 s.
    pub fn set_processing(&self, processing: bool) -> Result<()> {
        self.shared.want_processing.store(processing, Ordering::Release);
        unsafe { SetEvent(self.shared.wake.0) };
        Ok(())
    }

    /// The mode asked for last ([`Route::set_processing`]); the crossfade
    /// to it may still be under way.
    pub fn processing(&self) -> bool {
        self.shared.want_processing.load(Ordering::Acquire)
    }

    pub fn set_processor(&self, processor: Box<dyn Processor>) {
        let slot = Box::into_raw(Box::new(ProcSlot { processor }));
        let previous = self.shared.processor.swap(slot, Ordering::AcqRel);
        // Read after the swap, as on macOS: only a block already under way
        // can still hold `previous`, and it bumps the epoch when it ends.
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        if let Ok(mut retired) = self.retired.lock() {
            retired.push((epoch, previous));
        }
        self.reclaim();
    }

    /// Drops processors replaced by [`set_processor`] once the render
    /// thread has finished the block that might still be using them.
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
        self.sample_rate
    }

    /// The processor's width: the capture is stereo.
    pub fn channels(&self) -> u16 {
        2
    }

    pub fn describe(&self) -> String {
        let targets = self.targets.lock().map(|t| t.iter().map(|pid| pid.to_string()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let phase = match self.shared.phase.load(Ordering::Relaxed) {
            PHASE_MONITOR => "monitoring (the player plays by itself)",
            PHASE_FADING => "crossfading",
            _ => "processing (the player's session at 2^-13, brought back x 8192)",
        };
        format!(
            "process loopback of pid [{targets}] (their trees), stereo float {:.0} Hz, {} capture(s), packet stamps {:.1} ms old; {phase}; session gain {:.6}, our share {:.2}{}; L/R -> output ch 1/2 of {} on '{}'; {} underrun block(s), {} guarded packet(s)",
            self.sample_rate,
            self.shared.captures.load(Ordering::Relaxed),
            self.shared.stamp_lag.load(Ordering::Relaxed) as f64 / 10_000.0,
            self.shared.timeline.current(),
            f32::from_bits(self.shared.fade_target.load(Ordering::Relaxed)),
            if self.shared.mismatch.load(Ordering::Relaxed) { "; DUCKED LEVEL READ BACK DIFFERENT (calibrated from the read-back)" } else { "" },
            self.out_channels,
            self.device,
            self.shared.underruns.load(Ordering::Relaxed),
            self.shared.guarded.load(Ordering::Relaxed)
        )
    }
}

/// Opens the render client, reports `(rate, prime frames, output channels,
/// device name)` or the error, then runs until `stop`: while the render
/// stream is off (monitoring) every captured frame goes through the
/// processor and nothing is played; while it is on, the output device's
/// clock pulls blocks through the processor and our share of the crossfade
/// of them is played.
fn render_thread(output: Option<String>, shared: Arc<Shared>, stop: Arc<OwnedHandle>, commands: Receiver<Command>, ready: mpsc::Sender<Result<(u32, usize, u16, String)>>) {
    let _com = ComScope::enter();
    let opened = (|| -> Result<(Com, Com, OwnedHandle, MixFormat, u32, String)> {
        let enumerator = enumerator()?;
        let device = match &output {
            Some(uid) => device_by_id(&enumerator, uid)?,
            None => default_render_device(&enumerator)?,
        };
        let name = device_name(&device);
        let client = activate(&device, &IID_IAUDIO_CLIENT, "render client")?;
        let mix = mix_format(&client)?;
        let format = WaveFormatExtensible {
            format: WaveFormatEx {
                format_tag: WAVE_FORMAT_EXTENSIBLE,
                channels: mix.channels,
                samples_per_sec: mix.sample_rate,
                avg_bytes_per_sec: mix.sample_rate * 4 * mix.channels as u32,
                block_align: 4 * mix.channels,
                bits_per_sample: 32,
                cb_size: 22,
            },
            valid_bits_per_sample: 32,
            channel_mask: mix.channel_mask,
            sub_format: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        };
        let vtbl = unsafe { client.vtbl::<IAudioClientVtbl>() };
        let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        check(
            unsafe { (vtbl.initialize)(client.0, AUDCLNT_SHAREMODE_SHARED, flags, RENDER_BUFFER, 0, &format as *const WaveFormatExtensible as *const c_void, ptr::null()) },
            "initialize output",
        )?;
        let event = OwnedHandle::event(false)?;
        check(unsafe { (vtbl.set_event_handle)(client.0, event.0) }, "output event")?;
        let mut frames = 0u32;
        check(unsafe { (vtbl.get_buffer_size)(client.0, &mut frames) }, "output buffer size")?;
        let mut render = ptr::null_mut();
        let hr = unsafe { (vtbl.get_service)(client.0, &IID_IAUDIO_RENDER_CLIENT, &mut render) };
        let render = Com::from_out(render, hr, "render client")?;
        Ok((client, render, event, mix, frames, name))
    })();
    let (client, render, event, mix, buffer_frames, name) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let buffer_frames = buffer_frames as usize;
    // The capture primes with one output buffer: 20 ms.
    let prime = buffer_frames;
    let _ = ready.send(Ok((mix.sample_rate, prime, mix.channels, name)));
    drop(ready);

    let task_name = wide("Pro Audio");
    let mut task_index = 0u32;
    let task = unsafe { AvSetMmThreadCharacteristicsW(task_name.as_ptr(), &mut task_index) };

    let mut captures: Vec<Capture> = Vec::with_capacity(MAX_CAPTURES);
    let mut block = vec![0.0f32; buffer_frames * 2];
    let out_channels = mix.channels as usize;
    let vtbl = unsafe { client.vtbl::<IAudioClientVtbl>() };
    let r = unsafe { render.vtbl::<IAudioRenderClientVtbl>() };
    let handles = [stop.0, event.0];
    let mut rendering = false;
    let mut fade = 0f32;
    let fade_step = 1.0 / (FADE_SECS * mix.sample_rate as f32).max(1.0);
    loop {
        let woke = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, if rendering { 200 } else { 10 }) };
        if woke == WAIT_OBJECT_0 {
            break;
        }
        loop {
            match commands.try_recv() {
                Ok(Command::Add(capture)) => {
                    if captures.len() < MAX_CAPTURES {
                        captures.push(capture);
                    }
                }
                Ok(Command::Remove(pid)) => captures.retain(|capture| capture.pid != pid),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        // A capture that errors is gone (its process exited): let it go.
        captures.retain_mut(|capture| capture.pull(&shared).is_ok());
        shared.captures.store(captures.len(), Ordering::Relaxed);
        let want = shared.render_on.load(Ordering::Acquire);
        if want != rendering {
            if want {
                // Start on a buffer of silence, the captures priming again.
                let mut data = ptr::null_mut();
                if unsafe { (r.get_buffer)(render.0, buffer_frames as u32, &mut data) } >= 0 {
                    unsafe { (r.release_buffer)(render.0, buffer_frames as u32, AUDCLNT_BUFFERFLAGS_SILENT) };
                }
                for capture in captures.iter_mut() {
                    capture.fifo.primed = false;
                }
                fade = 0.0;
                if unsafe { (vtbl.start)(client.0) } < 0 {
                    break;
                }
            } else {
                unsafe { (vtbl.stop)(client.0) };
                unsafe { (vtbl.reset)(client.0) };
            }
            rendering = want;
        }
        if !rendering {
            // Monitoring: everything captured goes through the processor
            // (the pictures and meters), and nothing is played.
            loop {
                let frames = captures.iter().map(|capture| capture.fifo.len).max().unwrap_or(0).min(buffer_frames);
                if frames == 0 {
                    break;
                }
                let block = &mut block[..frames * 2];
                block.fill(0.0);
                for capture in captures.iter_mut() {
                    capture.fifo.take_into(block, frames);
                }
                run_processor(&shared, block, frames, mix.sample_rate);
            }
            shared.epoch.fetch_add(1, Ordering::Release);
            continue;
        }
        let mut padding = 0u32;
        if unsafe { (vtbl.get_current_padding)(client.0, &mut padding) } < 0 {
            // The device went away; the host notices the default output
            // changing and opens a new route.
            break;
        }
        let frames = buffer_frames.saturating_sub(padding as usize);
        if frames == 0 {
            continue;
        }
        let block = &mut block[..frames * 2];
        block.fill(0.0);
        for capture in captures.iter_mut() {
            let was = capture.fifo.primed;
            capture.fifo.mix_into(block, frames);
            if was && !capture.fifo.primed {
                shared.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        run_processor(&shared, block, frames, mix.sample_rate);
        // Our share of the crossfade, gliding; and never past full scale.
        let target = f32::from_bits(shared.fade_target.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        for frame in block.as_chunks_mut::<2>().0 {
            fade = if fade < target { (fade + fade_step).min(target) } else { (fade - fade_step).max(target) };
            frame[0] = (frame[0] * fade).clamp(-1.0, 1.0);
            frame[1] = (frame[1] * fade).clamp(-1.0, 1.0);
        }
        let mut data = ptr::null_mut();
        if unsafe { (r.get_buffer)(render.0, frames as u32, &mut data) } >= 0 && !data.is_null() {
            let out = unsafe { std::slice::from_raw_parts_mut(data as *mut f32, frames * out_channels) };
            write_out(out, out_channels, block, frames);
            unsafe { (r.release_buffer)(render.0, frames as u32, 0) };
        }
        shared.epoch.fetch_add(1, Ordering::Release);
    }
    captures.clear();
    shared.captures.store(0, Ordering::Relaxed);
    unsafe { (vtbl.stop)(client.0) };
    if !task.is_null() {
        unsafe { AvRevertMmThreadCharacteristics(task) };
    }
}

fn run_processor(shared: &Shared, block: &mut [f32], frames: usize, sample_rate: u32) {
    let info = FrameInfo { sample_rate: sample_rate as f64, channels: 2, frames, host_time: now_hns() };
    let slot = shared.processor.load(Ordering::Acquire);
    if !slot.is_null() {
        unsafe { (*slot).processor.process(block, &info) };
    }
}

/// The processed pair onto output channels 1 and 2; a mono output hears
/// both sides mixed, and every other channel is silent.
fn write_out(out: &mut [f32], channels: usize, block: &[f32], frames: usize) {
    for frame in 0..frames {
        let (left, right) = (block[frame * 2], block[frame * 2 + 1]);
        let row = &mut out[frame * channels..(frame + 1) * channels];
        if channels == 1 {
            row[0] = 0.5 * (left + right);
            continue;
        }
        row.fill(0.0);
        row[0] = left;
        row[1] = right;
    }
}

/// `roots` and every process under them.
fn tree_of(roots: &[u32], processes: &[ProcessInfo]) -> Vec<u32> {
    let mut tree: Vec<u32> = roots.to_vec();
    let mut grew = true;
    while grew {
        grew = false;
        for process in processes {
            if !tree.contains(&process.pid) && process.parent != process.pid && tree.contains(&process.parent) {
                tree.push(process.pid);
                grew = true;
            }
        }
    }
    tree
}

/// The watch thread: finds the target processes, activates a capture of
/// each and hands it to the render thread, follows the player across
/// relaunch, and runs the crossfade between monitoring and processing on
/// the player's session volume.
struct Watch {
    sources: Vec<Source>,
    /// Processing ducks the player (`Mute::HearOriginal` plays a copy).
    duck: bool,
    state_dir: Option<PathBuf>,
    sample_rate: u32,
    prime: usize,
    shared: Arc<Shared>,
    stop: Arc<OwnedHandle>,
    commands: SyncSender<Command>,
    targets: Arc<Mutex<Vec<u32>>>,
}

impl Watch {
    fn run(self, first: mpsc::Sender<Result<()>>) {
        let _com = ComScope::enter();
        let Ok(enumerator) = enumerator() else {
            let _ = first.send(Err(Error::System { status: -1, step: "device enumerator" }));
            return;
        };
        let steps = duck_steps();
        // Running captures: pid and a handle that signals when it exits.
        let mut running: Vec<(u32, OwnedHandle)> = Vec::new();
        let mut ducked: Vec<Ducked> = Vec::new();
        // The person's levels by session, for the route's life: a relaunched
        // player's session comes back at the ducked level.
        let mut known: Vec<(String, f32)> = Vec::new();
        // The crossfade's position in `steps` (0 = the person's level).
        let mut at = 0usize;
        let mut first = Some(first);
        let mut last_error: Option<Error> = None;
        let mut last_scan: Option<std::time::Instant> = None;
        let handles = [self.stop.0, self.shared.wake.0];
        loop {
            // Targets, every half second.
            if last_scan.is_none_or(|t| t.elapsed() >= WATCH_EVERY) {
                last_scan = Some(std::time::Instant::now());
                let processes = snapshot();
                let before = running.len();
                running.retain(|(pid, handle)| {
                    let exited = handle.0.is_null() || unsafe { WaitForSingleObject(handle.0, 0) } == WAIT_OBJECT_0;
                    if exited {
                        let _ = self.commands.try_send(Command::Remove(*pid));
                    }
                    !exited
                });
                let mut changed = running.len() != before;
                for root in resolve_roots(&self.sources, &processes) {
                    if running.iter().any(|(pid, _)| *pid == root) || running.len() >= MAX_CAPTURES {
                        continue;
                    }
                    match Capture::open(root, self.sample_rate, self.prime) {
                        Ok(capture) => {
                            let handle = OwnedHandle(unsafe { OpenProcess(SYNCHRONIZE, 0, root) });
                            if self.commands.try_send(Command::Add(capture)).is_ok() {
                                running.push((root, handle));
                                changed = true;
                            }
                        }
                        Err(error) => last_error = Some(error),
                    }
                }
                if let Ok(mut t) = self.targets.lock() {
                    *t = running.iter().map(|(pid, _)| *pid).collect();
                }
                if let Some(first) = first.take() {
                    let outcome = if running.is_empty() { Err(last_error.take().unwrap_or(Error::NotFound)) } else { Ok(()) };
                    let failed = outcome.is_err();
                    let _ = first.send(outcome);
                    if failed {
                        return;
                    }
                }
                // The player relaunched, or opened a session we did not duck
                // (another output): give every level back and start over.
                let roots: Vec<u32> = running.iter().map(|(pid, _)| *pid).collect();
                let stray = at > 0 && self.duck && {
                    let pids = tree_of(&roots, &processes);
                    sessions_of(&enumerator, &pids).iter().any(|(identifier, _)| !ducked.iter().any(|d| d.identifier == *identifier))
                };
                if at > 0 && (changed || stray) {
                    self.give_back(&mut ducked, &mut at);
                }
                // A relaunched player's session can come back at a level this
                // route set on its last one: back to the person's.
                if changed && self.duck {
                    let pids = tree_of(&roots, &processes);
                    for (identifier, control) in sessions_of(&enumerator, &pids) {
                        let Some((_, original)) = known.iter().find(|(id, _)| *id == identifier) else { continue };
                        if ducked.iter().any(|d| d.identifier == identifier) {
                            continue;
                        }
                        let Ok(volume) = control.query(&IID_ISIMPLE_AUDIO_VOLUME, "session volume") else { continue };
                        restore_if_ducked(&volume, *original, &steps);
                    }
                }
            }
            // The person moved the player's slider: their level wins, at once.
            if self.shared.external.swap(false, Ordering::AcqRel) && at > 0 {
                for d in ducked.iter_mut() {
                    if let Some(level) = d.get() {
                        let expected = f32::from_bits(unsafe { (*d.events).expected.load(Ordering::Relaxed) });
                        if (level - expected).abs() > expected * 1e-4 {
                            d.original = level;
                            if let Some(k) = known.iter_mut().find(|(id, _)| *id == d.identifier) {
                                k.1 = level;
                            }
                        }
                    }
                }
                let now = now_hns();
                // The change landed before its event: packets from a while
                // back count at the person's level.
                self.shared.timeline.push(now.saturating_sub(3_000_000), now, 1.0);
                self.give_back(&mut ducked, &mut at);
            }
            let want = self.shared.want_processing.load(Ordering::Acquire) && !running.is_empty();
            let last = steps.len() - 1;
            if want && at < last {
                if at == 0 {
                    self.shared.render_on.store(true, Ordering::Release);
                    if self.duck {
                        let roots: Vec<u32> = running.iter().map(|(pid, _)| *pid).collect();
                        ducked = self.take_sessions(&enumerator, &tree_of(&roots, &snapshot()), &mut known);
                        write_records(&self.state_dir, &ducked);
                    }
                }
                at += 1;
                self.step(&ducked, steps[at], at == last);
            } else if !want && at > 0 {
                at -= 1;
                self.step(&ducked, steps[at], false);
                if at == 0 {
                    ducked.clear();
                    write_records(&self.state_dir, &ducked);
                }
            } else if at == 0 && self.shared.render_on.load(Ordering::Relaxed) && f32::from_bits(self.shared.fade_target.load(Ordering::Relaxed)) == 0.0 {
                // Our share has glided out: the render stream stops.
                self.shared.render_on.store(false, Ordering::Release);
            }
            self.shared.phase.store(
                match at {
                    0 if !self.shared.render_on.load(Ordering::Relaxed) => PHASE_MONITOR,
                    a if a == last => PHASE_PROCESSING,
                    _ => PHASE_FADING,
                },
                Ordering::Release,
            );
            let fading = (want && at < last) || (!want && at > 0) || (at == 0 && self.shared.render_on.load(Ordering::Relaxed));
            let wait = if fading { STEP } else { WATCH_EVERY };
            if unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, wait.as_millis() as u32) } == WAIT_OBJECT_0 {
                break;
            }
        }
        // Closing: every level back as the person set it.
        for d in ducked.iter() {
            d.set(d.original);
        }
        ducked.clear();
        write_records(&self.state_dir, &ducked);
    }

    /// The player's sessions, ducked from here on, each with the level the
    /// person set: the level now, unless a relaunched session came back at
    /// a level this route (or a crashed one) ducked it to.
    fn take_sessions(&self, enumerator: &Com, pids: &[u32], known: &mut Vec<(String, f32)>) -> Vec<Ducked> {
        let recorded = self.state_dir.as_deref().map(read_records).unwrap_or_default();
        let mut ducked = Vec::new();
        for (identifier, control) in sessions_of(enumerator, pids) {
            let Ok(volume) = control.query(&IID_ISIMPLE_AUDIO_VOLUME, "session volume") else { continue };
            let mut level = 0f32;
            if unsafe { (volume.vtbl::<ISimpleAudioVolumeVtbl>().get_master_volume)(volume.0, &mut level) } < 0 {
                continue;
            }
            let remembered = known.iter().chain(recorded.iter()).find(|(id, _)| *id == identifier).map(|(_, level)| *level);
            let original = match remembered {
                Some(original) if level < original => original,
                _ => level,
            };
            match known.iter_mut().find(|(id, _)| *id == identifier) {
                Some(k) => k.1 = original,
                None => known.push((identifier.clone(), original)),
            }
            let events = Box::into_raw(Box::new(Events { vtbl: &EVENTS_VTBL, refs: AtomicU32::new(1), shared: Arc::clone(&self.shared), expected: AtomicU32::new(level.to_bits()) }));
            let hr = unsafe { (control.vtbl::<IAudioSessionControl2Vtbl>().register_audio_session_notification)(control.0, events as *mut c_void) };
            if hr < 0 {
                unsafe { events_release(events as *mut c_void) };
                continue;
            }
            ducked.push(Ducked { identifier, control, volume, events, original });
        }
        ducked
    }

    /// One step of the crossfade: the sessions to `gain` of the person's
    /// level, our share to the rest. At the bottom the level is read back
    /// and a difference is flagged and used.
    fn step(&self, ducked: &[Ducked], gain: f32, bottom: bool) {
        let from = now_hns();
        let mut applied = gain;
        if self.duck {
            for d in ducked {
                d.set(d.original * gain);
            }
            if bottom {
                if let Some(d) = ducked.first() {
                    if let Some(read) = d.get() {
                        let expected = d.original * gain;
                        if read != expected && d.original > 0.0 {
                            self.shared.mismatch.store(true, Ordering::Relaxed);
                            applied = read / d.original;
                        }
                    }
                }
            }
        } else {
            applied = 1.0;
        }
        let until = now_hns();
        if self.duck {
            self.shared.timeline.push(from, until, applied);
        }
        let share = if bottom { 1.0 } else { 1.0 - gain };
        self.shared.fade_target.store(share.to_bits(), Ordering::Relaxed);
    }

    /// Every ducked session back at once (our share off first), for a
    /// relaunch, a stray session or the person's own slider; processing,
    /// if still wanted, crossfades in again from there.
    fn give_back(&self, ducked: &mut Vec<Ducked>, at: &mut usize) {
        self.shared.fade_target.store(0f32.to_bits(), Ordering::Relaxed);
        let from = now_hns();
        for d in ducked.iter() {
            d.set(d.original);
        }
        if self.duck {
            self.shared.timeline.push(from, now_hns(), 1.0);
        }
        ducked.clear();
        write_records(&self.state_dir, ducked);
        *at = 0;
    }
}

/// The running Windows build number (process loopback needs 19041).
fn windows_build() -> u32 {
    #[repr(C)]
    struct VersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service_pack: [u16; 128],
    }
    #[link(name = "ntdll", kind = "raw-dylib")]
    extern "system" {
        fn RtlGetVersion(info: *mut VersionInfo) -> i32;
    }
    let mut info = VersionInfo { size: std::mem::size_of::<VersionInfo>() as u32, major: 0, minor: 0, build: 0, platform: 0, service_pack: [0; 128] };
    // SAFETY: RtlGetVersion fills the struct whose size it is given; unlike
    // GetVersionEx it reports the real version without a manifest.
    if unsafe { RtlGetVersion(&mut info) } != 0 {
        return u32::MAX;
    }
    info.build
}

fn drop_slot(slot: *mut ProcSlot) {
    if !slot.is_null() {
        unsafe { drop(Box::from_raw(slot)) };
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_players_match_by_bundle_exe_or_package() {
        let mut none = || String::new();
        assert!(matches("com.spotify.client", "Spotify.exe", &mut none));
        assert!(matches("Spotify.exe", "spotify.exe", &mut none));
        assert!(matches("spotify", "Spotify.exe", &mut none));
        assert!(!matches("com.spotify.client", "SpotifyWebHelper2.exe", &mut none));
        let mut store = || "SpotifyAB.SpotifyMusic_zpdnekdrzrea0".to_string();
        // The package's system hosts are not the player.
        assert!(!matches("com.spotify.client", "RuntimeBroker.exe", &mut store));
        assert!(matches("SpotifyAB.SpotifyMusic_zpdnekdrzrea0", "Other.exe", &mut store));
        assert!(matches("SpotifyAB.SpotifyMusic", "Other.exe", &mut store));
    }

    #[test]
    fn a_tree_is_captured_from_its_root() {
        let p = |pid, parent, exe: &str| ProcessInfo { pid, parent, exe: exe.into() };
        let processes = vec![p(10, 1, "explorer.exe"), p(20, 10, "Spotify.exe"), p(21, 20, "Spotify.exe"), p(22, 20, "Spotify.exe"), p(30, 10, "Other.exe")];
        assert_eq!(resolve_roots(&[Source::BundleId("com.spotify.client".into())], &processes), vec![20]);
        assert_eq!(resolve_roots(&[Source::Pid(30)], &processes), vec![30]);
    }

    #[test]
    fn the_fifo_primes_plays_and_drops_a_backlog() {
        let mut fifo = Fifo::new(64, 4);
        let mut block = vec![0.0; 8];
        fifo.push_scaled(&[1.0, 1.0, 2.0, 2.0], 1.0);
        fifo.mix_into(&mut block, 4);
        assert!(block.iter().all(|s| *s == 0.0), "not primed yet");
        fifo.push_scaled(&[3.0, 3.0, 4.0, 4.0], 1.0);
        fifo.mix_into(&mut block, 2);
        assert_eq!(&block[..4], &[1.0, 1.0, 2.0, 2.0]);
        for i in 0..40 {
            fifo.push_scaled(&[i as f32, i as f32], 1.0);
        }
        let mut block = vec![0.0; 8];
        fifo.mix_into(&mut block, 4);
        // Backlog cut to 3 x prime + the block: the oldest were dropped.
        assert_eq!(fifo.len, 12);
        assert_eq!(block[6], 27.0);
    }

    /// A packet is brought back by the gain it was mixed at: a small
    /// change in doubt takes the larger gain, a large one is found in the
    /// packet's level.
    #[test]
    fn packets_are_brought_back_by_their_gain() {
        let timeline = Timeline::new();
        let mut landing = Landing::default();
        assert_eq!(landing.gain(&timeline, 0, 100_000, 0.5), 1.0);
        timeline.push(1_000_000, 1_000_100, 0.5);
        assert_eq!(landing.gain(&timeline, 900_000, 1_050_000, 0.5), 1.0, "ends before it can land");
        assert_eq!(landing.gain(&timeline, 1_300_000, 1_400_000, 0.25), 1.0, "in doubt: the larger");
        assert_eq!(landing.gain(&timeline, 2_000_000, 2_100_000, 0.25), 0.5, "surely landed");
        timeline.push(3_000_000, 3_000_100, DUCK);
        assert_eq!(landing.gain(&timeline, 3_300_000, 3_400_000, 0.25), 0.5, "the level has not dropped");
        assert_eq!(landing.gain(&timeline, 3_400_000, 3_500_000, 0.25 * DUCK * 2.0), DUCK, "it dropped");
        assert_eq!(landing.gain(&timeline, 3_500_000, 3_600_000, 0.4 * DUCK), DUCK, "landed stays landed");
        assert_eq!(1.0 / DUCK, 8192.0);
        let mut guard = Guard::default();
        guard.observe(0.2);
        assert!(guard.allows(0.3));
        assert!(!guard.allows(8192.0 * 0.3), "past full scale");
        let mut guard = Guard { envelope: 0.001, suspicious: 0 };
        assert!(!guard.allows(0.5), "36 dB over the level before");
        assert!(!guard.allows(0.5));
        assert!(!guard.allows(0.5));
        assert!(guard.allows(0.5), "three in a row are the music");
    }
}
