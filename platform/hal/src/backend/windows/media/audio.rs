//! WASAPI devices and streams: the device list (render, capture, and every render device again
//! as a loopback capture of system audio), default-device and hot-plug notifications, and one
//! stream thread per open device that calls the entry the app registered for its slot
//! (`AudioEntry`) with one planar block (`media::AudioBlock`).
//!
//! Moved from os/windows/wasapi.rs. Enumeration and the notification client live on one
//! long-lived MTA worker (`makepad-wasapi`), so neither the HAL's thread nor a COM callback ever
//! asks a device anything; the worker publishes the list and rings the session. Stream threads
//! run at MMCSS "Pro Audio" priority, prefer an IAudioClient3 low-latency shared stream and fall
//! back to the classic shared client with PCM conversion. A stream thread holds its slot's lock
//! only while the entry runs, so replacing an entry (`AudioEntry`) returns once the old one is no
//! longer running; nothing else on that thread blocks on a lock another thread holds long.

use super::Signal;
use crate::media::{device_kind, name_hash, AudioBlock, AudioDeviceInfo, AudioFn};
use makepad_windows_sys::core::{PCWSTR, PWSTR};
use makepad_windows_sys::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use makepad_windows_sys::Win32::Foundation::{CloseHandle, HANDLE, PROPERTYKEY, WAIT_OBJECT_0};
use makepad_windows_sys::Win32::Media::Audio::*;
use makepad_windows_sys::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use makepad_windows_sys::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use makepad_windows_sys::Win32::System::Com::*;
use makepad_windows_sys::Win32::System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, SetEvent, WaitForSingleObject};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

/// Slots per direction (the index of `AudioEntry`).
pub const MAX_AUDIO_DEVICE_INDEX: usize = 32;

/// A registered entry.
#[derive(Clone, Copy)]
pub struct Entry {
    pub f: AudioFn,
    pub ctx: usize,
}

/// The entry of one device slot. Locked while the entry runs.
pub type Slot = Arc<Mutex<Option<Entry>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A device as the worker listed it.
#[derive(Clone)]
struct Desc {
    info: AudioDeviceInfo,
    /// the endpoint id string (a loopback desc carries its render endpoint's id)
    endpoint: String,
}

/// One stream thread as the open set knows it. An output's entry is published before its
/// device opens (a second scan that lands during the open must see it), so the event is null
/// until the open finished.
struct StreamRef {
    device: u64,
    serial: u64,
    terminated: bool,
    event: HANDLE,
}

// SAFETY: the event handle is a kernel object usable from any thread; it is closed only by the
// stream thread that made it, after it removed this entry (see `StreamLease`)
unsafe impl Send for StreamRef {}

impl StreamRef {
    fn terminate(&mut self) {
        self.terminated = true;
        if !self.event.is_invalid() {
            unsafe {
                let _ = SetEvent(self.event);
            }
        }
    }
}

static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// State the worker, the stream threads and the HAL's thread share. Every lock is held for a few
/// field updates only.
struct Shared {
    descs: Mutex<Vec<Desc>>,
    failed: Mutex<HashSet<u64>>,
    inputs: Mutex<Vec<StreamRef>>,
    outputs: Mutex<Vec<StreamRef>>,
    change: Signal,
}

/// What a stream thread leaves behind however it leaves (the loop ends, the open fails, a call
/// on a vanished device unwinds it): its entry gone from its list and the app told.
struct StreamLease {
    shared: Arc<Shared>,
    input: bool,
    serial: u64,
}

impl Drop for StreamLease {
    fn drop(&mut self) {
        let list = if self.input { &self.shared.inputs } else { &self.shared.outputs };
        lock(list).retain(|v| v.serial != self.serial);
        self.shared.change.set();
    }
}

/// Whether a device asked for needs a thread of its own: not while a live one is open (or
/// opening) for it, not while it stands as failed. A terminated entry is on its way out and does
/// not count -- shared mode allows the new thread while the old one winds down.
fn should_spawn(known: &[StreamRef], failed: &HashSet<u64>, device: u64) -> bool {
    !known.iter().any(|v| v.device == device && !v.terminated) && !failed.contains(&device)
}

enum Cmd {
    Enumerate,
    Shutdown,
}

/// The session's WASAPI service.
pub struct WasapiAccess {
    pub input_cb: [Slot; MAX_AUDIO_DEVICE_INDEX],
    pub output_cb: [Slot; MAX_AUDIO_DEVICE_INDEX],
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
}

impl Drop for WasapiAccess {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        for list in [&self.shared.inputs, &self.shared.outputs] {
            lock(list).iter_mut().for_each(StreamRef::terminate);
        }
    }
}

/// The notification client: any endpoint change asks the worker for a new list.
struct Listener {
    tx: Mutex<Sender<Cmd>>,
}

impl Listener {
    fn changed(&self) {
        let _ = lock(&self.tx).send(Cmd::Enumerate);
    }
}

impl IMMNotificationClientImpl for Listener {
    fn OnDeviceStateChanged(&self, _id: PCWSTR, _state: DEVICE_STATE) -> Result<(), makepad_windows_sys::core::HRESULT> {
        self.changed();
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: PCWSTR) -> Result<(), makepad_windows_sys::core::HRESULT> {
        self.changed();
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: PCWSTR) -> Result<(), makepad_windows_sys::core::HRESULT> {
        self.changed();
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _id: PCWSTR) -> Result<(), makepad_windows_sys::core::HRESULT> {
        self.changed();
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: PCWSTR, _key: PROPERTYKEY) -> Result<(), makepad_windows_sys::core::HRESULT> {
        Ok(())
    }
}

impl WasapiAccess {
    /// Starts the worker; the first list follows (rings `change`).
    pub fn new(change: Signal) -> WasapiAccess {
        let shared = Arc::new(Shared { descs: Mutex::new(Vec::new()), failed: Mutex::new(HashSet::new()), inputs: Mutex::new(Vec::new()), outputs: Mutex::new(Vec::new()), change });
        let (tx, rx) = channel::<Cmd>();
        let worker_tx = tx.clone();
        let worker_shared = shared.clone();
        let spawned = std::thread::Builder::new().name("makepad-wasapi".into()).spawn(move || unsafe {
            let _com = super::Mta::enter();
            let enumerator = new_enumerator();
            let listener = IMMNotificationClient::implement(Box::new(Listener { tx: Mutex::new(worker_tx) }));
            if let Some(e) = &enumerator {
                if let Err(err) = e.RegisterEndpointNotificationCallback(&listener) {
                    eprintln!("[hal-media] audio: endpoint notifications unavailable: {err}");
                }
            }
            loop {
                let list = enumerator.as_ref().map(|e| list_devices(e)).unwrap_or_default();
                *lock(&worker_shared.descs) = list;
                worker_shared.change.set();
                match rx.recv() {
                    // a burst of notifications (a device with several roles) is one scan
                    Ok(Cmd::Enumerate) => while let Ok(Cmd::Enumerate) = rx.try_recv() {},
                    Ok(Cmd::Shutdown) | Err(_) => break,
                }
            }
            if let Some(e) = &enumerator {
                let _ = e.UnregisterEndpointNotificationCallback(&listener);
            }
            drop(listener);
            drop(enumerator);
        });
        if let Err(e) = spawned {
            eprintln!("[hal-media] audio: worker thread: {e}");
        }
        WasapiAccess { input_cb: Default::default(), output_cb: Default::default(), shared, tx }
    }

    /// The devices as the worker last listed them, with the ones that failed to open marked.
    pub fn descs(&self) -> Vec<AudioDeviceInfo> {
        let failed = lock(&self.shared.failed).clone();
        lock(&self.shared.descs)
            .iter()
            .map(|d| AudioDeviceInfo { has_failed: failed.contains(&d.info.id), ..d.info.clone() })
            .collect()
    }

    /// A request longer than there are slots is cut to them (the app asks for every loopback
    /// device it can see; a machine with more endpoints than slots is not unusual).
    fn within_slots(devices: &[u64]) -> &[u64] {
        let fits = devices.len().min(MAX_AUDIO_DEVICE_INDEX);
        if fits < devices.len() {
            eprintln!("[hal-media] audio: {} devices asked for, {} slots to serve them with", devices.len(), fits);
        }
        &devices[..fits]
    }

    /// The inputs (captures and loopbacks) to run, slot = position in `devices`.
    pub fn use_inputs(&self, devices: &[u64]) {
        self.use_devices(true, devices);
    }

    pub fn use_outputs(&self, devices: &[u64]) {
        self.use_devices(false, devices);
    }

    fn use_devices(&self, input: bool, devices: &[u64]) {
        let devices = Self::within_slots(devices);
        let list = if input { &self.shared.inputs } else { &self.shared.outputs };
        let new = {
            let mut open = lock(list);
            open.iter_mut().filter(|v| !devices.contains(&v.device)).for_each(StreamRef::terminate);
            let failed = lock(&self.shared.failed);
            let descs = lock(&self.shared.descs);
            let mut new = Vec::new();
            for (index, &device) in devices.iter().enumerate() {
                if !should_spawn(&open, &failed, device) {
                    continue;
                }
                let Some(desc) = descs.iter().find(|d| d.info.id == device) else {
                    eprintln!("[hal-media] audio: device {device:x} is not in the list");
                    continue;
                };
                let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
                // published before the open: a scan landing during the open must not open it again
                open.push(StreamRef { device, serial, terminated: false, event: HANDLE(std::ptr::null_mut()) });
                new.push((index, desc.clone(), serial));
            }
            new
        };
        for (index, desc, serial) in new {
            let slot = if input { self.input_cb[index].clone() } else { self.output_cb[index].clone() };
            let shared = self.shared.clone();
            let spawned = std::thread::Builder::new().name("makepad-wasapi-stream".into()).spawn(move || {
                let lease = StreamLease { shared: shared.clone(), input, serial };
                run_stream(&shared, &desc, input, serial, &slot);
                drop(lease);
            });
            if let Err(e) = spawned {
                eprintln!("[hal-media] audio: stream thread: {e}");
                lock(list).retain(|v| v.serial != serial);
            }
        }
    }
}

unsafe fn new_enumerator() -> Option<IMMDeviceEnumerator> {
    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL, &IMMDeviceEnumerator::IID).and_then(|raw| IMMDeviceEnumerator::from_raw(raw)).ok()
}

unsafe fn pwstr_string(p: PWSTR) -> Option<String> {
    if p.0.is_null() {
        return None;
    }
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

/// A device's endpoint id and friendly name, or nothing: every call is a question to a device
/// that may have gone between the list and the question.
unsafe fn device_names(device: &IMMDevice) -> Option<(String, String)> {
    let id = pwstr_string(device.GetId().ok()?)?;
    let props = device.OpenPropertyStore(STGM_READ).ok()?;
    let value = props.GetValue(&PKEY_Device_FriendlyName).ok()?;
    // VT_LPWSTR
    let name = if value.Anonymous.Anonymous.vt.0 == 31 { value.Anonymous.Anonymous.Anonymous.pwszVal.to_string().unwrap_or_default() } else { String::new() };
    Some((id, name))
}

/// The channel count of the device's mix format (2 when it cannot be asked).
unsafe fn mix_channels(device: &IMMDevice) -> u32 {
    let Ok(client) = device.Activate(&IAudioClient::IID, CLSCTX_ALL, None).and_then(|raw| IAudioClient::from_raw(raw)) else { return 2 };
    let Ok(mix) = client.GetMixFormat() else { return 2 };
    let channels = (*mix).nChannels as u32;
    CoTaskMemFree(Some(mix as *const _ as *const _));
    channels
}

unsafe fn list_flow(enumerator: &IMMDeviceEnumerator, flow: EDataFlow, kind: u32, out: &mut Vec<Desc>) {
    let default = enumerator.GetDefaultAudioEndpoint(flow, eConsole).ok().and_then(|d| device_names(&d)).map(|(id, _)| id);
    let Ok(col) = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else { return };
    let Ok(count) = col.GetCount() else { return };
    for i in 0..count {
        let Ok(device) = col.Item(i) else { continue };
        let Some((endpoint, name)) = device_names(&device) else { continue };
        let channels = mix_channels(&device);
        let is_default = default.as_deref() == Some(endpoint.as_str());
        let (id, name) = if kind == device_kind::LOOPBACK { (name_hash(&format!("{endpoint}_loopback")), format!("{name} (Loopback)")) } else { (name_hash(&endpoint), name) };
        out.push(Desc { info: AudioDeviceInfo { id, kind, is_default, has_failed: false, channels, name }, endpoint });
    }
}

/// Capture devices, render devices, then every render device as a loopback capture.
unsafe fn list_devices(enumerator: &IMMDeviceEnumerator) -> Vec<Desc> {
    let mut out = Vec::new();
    list_flow(enumerator, eCapture, device_kind::INPUT, &mut out);
    list_flow(enumerator, eRender, device_kind::OUTPUT, &mut out);
    list_flow(enumerator, eRender, device_kind::LOOPBACK, &mut out);
    out
}

unsafe fn find_endpoint(enumerator: &IMMDeviceEnumerator, endpoint: &str) -> Option<IMMDevice> {
    let col = enumerator.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE).ok()?;
    for i in 0..col.GetCount().ok()? {
        let Ok(device) = col.Item(i) else { continue };
        if device_names(&device).is_some_and(|(id, _)| id == endpoint) {
            return Some(device);
        }
    }
    None
}

fn float_format(sample_rate: u32, channels: u32) -> WAVEFORMATEXTENSIBLE {
    let block = channels * 4;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            cbSize: 22,
            nAvgBytesPerSec: sample_rate * block,
            nBlockAlign: block as u16,
            nChannels: channels as u16,
            nSamplesPerSec: sample_rate,
            wBitsPerSample: 32,
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
        SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        dwChannelMask: if channels <= 18 { (1 << channels) - 1 } else { 0 },
    }
}

/// An open, started WASAPI stream.
struct Stream {
    client: IAudioClient,
    event: HANDLE,
    channels: usize,
    sample_rate: f64,
    buffer_frames: u32,
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            if !self.event.is_invalid() {
                let _ = CloseHandle(self.event);
            }
        }
    }
}

impl Stream {
    /// Opens `device` as a shared float stream in its mix rate: low latency through
    /// IAudioClient3 when the engine offers it, else the classic client with PCM conversion.
    /// A loopback capture opens the render endpoint with the loopback flag (20 ms at least).
    unsafe fn open(device: &IMMDevice, channels: u32, loopback: bool) -> Result<Stream, String> {
        let channels = channels.clamp(1, 2);
        let probe: IAudioClient = device.Activate(&IAudioClient::IID, CLSCTX_ALL, None).and_then(|raw| IAudioClient::from_raw(raw)).map_err(|e| format!("Activate: {e}"))?;
        let mix = probe.GetMixFormat().map_err(|e| format!("GetMixFormat: {e}"))?;
        let rate = (*mix).nSamplesPerSec;
        CoTaskMemFree(Some(mix as *const _ as *const _));
        let rate = if rate == 0 { 48000 } else { rate };
        drop(probe);
        let format = float_format(rate, channels);
        let wave = &format as *const _ as *const WAVEFORMATEX;
        let event = CreateEventW(None, false, false, None).map_err(|e| format!("CreateEvent: {e}"))?;
        let started = |client: IAudioClient| -> Result<Stream, String> {
            client.SetEventHandle(event).map_err(|e| format!("SetEventHandle: {e}"))?;
            let buffer_frames = client.GetBufferSize().map_err(|e| format!("GetBufferSize: {e}"))?;
            client.Start().map_err(|e| format!("Start: {e}"))?;
            Ok(Stream { client, event, channels: channels as usize, sample_rate: rate as f64, buffer_frames })
        };
        if !loopback {
            if let Ok(client3) = device.Activate(&IAudioClient3::IID, CLSCTX_ALL, None).and_then(|raw| IAudioClient3::from_raw(raw)) {
                let (mut default, mut fundamental, mut min, mut max) = (0u32, 0u32, 0u32, 0u32);
                if client3.GetSharedModeEnginePeriod(wave, &mut default, &mut fundamental, &mut min, &mut max).is_ok()
                    && client3.InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, default, wave, None).is_ok()
                {
                    if let Ok(client) = IAudioClient::query(client3.as_raw()) {
                        return started(client).inspect_err(|_| {
                            let _ = CloseHandle(event);
                        });
                    }
                }
            }
        }
        let client: IAudioClient = match device.Activate(&IAudioClient::IID, CLSCTX_ALL, None).and_then(|raw| IAudioClient::from_raw(raw)) {
            Ok(c) => c,
            Err(e) => {
                let _ = CloseHandle(event);
                return Err(format!("Activate: {e}"));
            }
        };
        let (mut period, mut min_period) = (0i64, 0i64);
        let _ = client.GetDevicePeriod(Some(&mut period), Some(&mut min_period));
        let mut flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        if loopback {
            flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
            period = period.max(200_000);
        }
        if let Err(e) = client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, period, 0, wave, None) {
            let _ = CloseHandle(event);
            return Err(format!("Initialize: {e}"));
        }
        started(client).inspect_err(|_| {
            let _ = CloseHandle(event);
        })
    }

    /// Waits for the device's next period; false when it did not come (a device that went).
    unsafe fn wait(&self) -> bool {
        WaitForSingleObject(self.event, 2000) == WAIT_OBJECT_0
    }
}

/// Planar scratch for one stream, sized for the device buffer up front so the realtime loop
/// does not allocate.
struct Planar {
    data: Vec<f32>,
    frames: usize,
    channels: usize,
}

impl Planar {
    fn new(frames: usize, channels: usize) -> Planar {
        Planar { data: vec![0.0; frames.max(1) * channels], frames: frames.max(1), channels }
    }
    /// `frames` planar frames (grows only when a device hands more than its buffer size).
    fn fit(&mut self, frames: usize) {
        if frames > self.frames {
            self.data.resize(frames * self.channels, 0.0);
        }
        self.frames = frames.max(1).min(self.data.len() / self.channels.max(1));
    }
    fn call(&mut self, slot: &Slot, device: u64, sample_rate: f64, zero: bool) {
        let frames = self.frames;
        if zero {
            self.data[..frames * self.channels].fill(0.0);
        }
        let mut planes = [std::ptr::null_mut::<f32>(); 2];
        for (c, p) in planes.iter_mut().enumerate().take(self.channels) {
            *p = self.data[c * frames..].as_mut_ptr();
        }
        let guard = lock(slot);
        if let Some(entry) = *guard {
            let mut block = AudioBlock { device, sample_rate, sample_time: 0.0, host_time: 0, rate_scalar: 0.0, has_time: 0, frames: frames as u32, channels: self.channels as u32, planes: planes.as_mut_ptr() };
            (entry.f)(entry.ctx as *mut std::ffi::c_void, &mut block);
        }
    }
}

/// MMCSS "Pro Audio" for the calling thread while this lives.
struct Mmcss(Option<makepad_windows_sys::Win32::System::Threading::AVRT_TASK_HANDLE>);

impl Mmcss {
    fn raise() -> Mmcss {
        let name: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
        let mut index = 0u32;
        let handle = unsafe { AvSetMmThreadCharacteristicsW(PCWSTR(name.as_ptr()), &mut index) };
        if handle.is_err() {
            eprintln!("[hal-media] audio: could not raise the stream thread's priority (MMCSS)");
        }
        Mmcss(handle.ok())
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        if let Some(h) = self.0.take() {
            unsafe {
                let _ = AvRevertMmThreadCharacteristics(h);
            }
        }
    }
}

/// One stream thread: open, publish the wake event, run blocks until terminated or the device
/// goes. The lease (held by the caller) removes the entry and tells the app however this ends.
fn run_stream(shared: &Arc<Shared>, desc: &Desc, input: bool, serial: u64, slot: &Slot) {
    let _com = super::Mta::enter();
    let _mmcss = Mmcss::raise();
    let device = desc.info.id;
    let loopback = desc.info.kind == device_kind::LOOPBACK;
    let list = if input { &shared.inputs } else { &shared.outputs };
    let opened = unsafe { new_enumerator().and_then(|e| find_endpoint(&e, &desc.endpoint)).ok_or_else(|| "endpoint gone".to_string()).and_then(|d| Stream::open(&d, desc.info.channels, loopback)) };
    let stream = match opened {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[hal-media] audio: could not open {} \"{}\": {e}", if input { "input" } else { "output" }, desc.info.name);
            lock(&shared.failed).insert(device);
            return;
        }
    };
    // hand the entry its event (a terminate can wake this thread now) and honour a terminate
    // that arrived while the device opened
    let terminated_meanwhile = match lock(list).iter_mut().find(|v| v.serial == serial) {
        Some(entry) => {
            entry.event = stream.event;
            entry.terminated
        }
        None => true,
    };
    let terminated = || lock(list).iter().any(|v| v.serial == serial && v.terminated);
    if terminated_meanwhile {
        return;
    }
    let ended = if input { capture_loop(&stream, device, slot, &terminated) } else { render_loop(&stream, device, slot, &terminated) };
    if ended && !terminated() {
        eprintln!("[hal-media] audio: {} \"{}\" stopped", if input { "input" } else { "output" }, desc.info.name);
    }
    // the entry forgets the event before the stream closes it: a terminate never signals a
    // closed handle
    if let Some(entry) = lock(list).iter_mut().find(|v| v.serial == serial) {
        entry.event = HANDLE(std::ptr::null_mut());
    }
    drop(stream);
}

/// Fills the device buffer each period. True when the device went (not a terminate).
fn render_loop(stream: &Stream, device: u64, slot: &Slot, terminated: &dyn Fn() -> bool) -> bool {
    let Ok(render) = (unsafe { stream.client.GetService(&IAudioRenderClient::IID).and_then(|raw| IAudioRenderClient::from_raw(raw)) }) else { return true };
    let mut planar = Planar::new(stream.buffer_frames as usize, stream.channels);
    let ch = stream.channels;
    loop {
        if unsafe { !stream.wait() } {
            return true;
        }
        if terminated() {
            return false;
        }
        unsafe {
            let Ok(padding) = stream.client.GetCurrentPadding() else { return true };
            let frames = stream.buffer_frames.saturating_sub(padding) as usize;
            if frames == 0 {
                continue;
            }
            let Ok(ptr) = render.GetBuffer(frames as u32) else { return true };
            planar.fit(frames);
            planar.call(slot, device, stream.sample_rate, true);
            let out = std::slice::from_raw_parts_mut(ptr as *mut f32, frames * ch);
            for (f, frame) in out.chunks_exact_mut(ch).enumerate() {
                for (c, s) in frame.iter_mut().enumerate() {
                    *s = planar.data[c * frames + f];
                }
            }
            if render.ReleaseBuffer(frames as u32, 0).is_err() {
                return true;
            }
        }
    }
}

/// Hands each captured packet to the entry. True when the device went (not a terminate).
fn capture_loop(stream: &Stream, device: u64, slot: &Slot, terminated: &dyn Fn() -> bool) -> bool {
    let Ok(capture) = (unsafe { stream.client.GetService(&IAudioCaptureClient::IID).and_then(|raw| IAudioCaptureClient::from_raw(raw)) }) else { return true };
    let mut planar = Planar::new(stream.buffer_frames as usize, stream.channels);
    let ch = stream.channels;
    loop {
        if unsafe { !stream.wait() } {
            return true;
        }
        if terminated() {
            return false;
        }
        // drain every packet the period left
        loop {
            let (mut data, mut frames, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
            if unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.is_err() {
                return true;
            }
            if frames == 0 {
                break;
            }
            let n = frames as usize;
            planar.fit(n);
            // AUDCLNT_BUFFERFLAGS_SILENT: the packet is silence whatever the buffer holds
            if flags & 2 != 0 || data.is_null() {
                planar.data[..n * ch].fill(0.0);
            } else {
                let src = unsafe { std::slice::from_raw_parts(data as *const f32, n * ch) };
                for (f, frame) in src.chunks_exact(ch).enumerate() {
                    for (c, s) in frame.iter().enumerate() {
                        planar.data[c * n + f] = *s;
                    }
                }
            }
            if unsafe { capture.ReleaseBuffer(frames) }.is_err() {
                return true;
            }
            planar.call(slot, device, stream.sample_rate, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(device: u64, serial: u64, terminated: bool) -> StreamRef {
        StreamRef { device, serial, terminated, event: HANDLE(std::ptr::null_mut()) }
    }

    #[test]
    fn a_device_already_open_or_opening_is_not_opened_again() {
        let failed = HashSet::new();
        let known = [entry(1, 1, false), entry(2, 2, true)];
        assert!(!should_spawn(&known, &failed, 1), "live: no second thread");
        assert!(should_spawn(&known, &failed, 3), "unknown: opened");
        assert!(should_spawn(&known, &failed, 2), "winding down: opened again");
    }

    #[test]
    fn a_device_that_failed_is_not_retried() {
        let mut failed = HashSet::new();
        failed.insert(1);
        assert!(!should_spawn(&[], &failed, 1));
    }

    #[test]
    fn a_request_longer_than_the_slots_is_cut_to_them() {
        let many: Vec<u64> = (0..40).collect();
        assert_eq!(WasapiAccess::within_slots(&many).len(), MAX_AUDIO_DEVICE_INDEX);
        assert_eq!(WasapiAccess::within_slots(&many[..3]).len(), 3);
    }
}
