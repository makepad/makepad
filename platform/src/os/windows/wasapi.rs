#![allow(dead_code)]
use {
    crate::{
        audio::*,
        makepad_live_id::*,
        thread::SignalToUI,
        windows::{
            core::Interface,
            core::PCWSTR,
            Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
            Win32::Foundation::PROPERTYKEY,
            Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
            Win32::Media::Audio::{
                eAll,
                eCapture,
                eConsole,
                eRender,
                EDataFlow,
                ERole,
                //IMMDevice,
                IAudioCaptureClient,
                IAudioClient,
                IAudioClient3,
                IAudioRenderClient,
                IMMDevice,
                IMMDeviceEnumerator,
                IMMNotificationClient,
                IMMNotificationClient_Impl,
                MMDeviceEnumerator,
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                //WAVEFORMATEX,
                DEVICE_STATE,
                DEVICE_STATE_ACTIVE,
                WAVEFORMATEX,
                WAVEFORMATEXTENSIBLE,
                WAVEFORMATEXTENSIBLE_0,
            },
            Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE,
            Win32::Media::Multimedia::{
                KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
                //WAVE_FORMAT_IEEE_FLOAT
            },
            Win32::System::Com::{
                CoCreateInstance,
                CoInitializeEx,
                CLSCTX_ALL,
                //STGM_READ,
                COINIT_APARTMENTTHREADED,
                STGM_READ,
            },
            Win32::System::Threading::{
                AvSetMmThreadCharacteristicsW, CreateEventA, SetEvent, WaitForSingleObject,
            },
        },
    },
    std::collections::HashSet,
    std::sync::atomic::{AtomicU64, Ordering},
    std::sync::{Arc, Mutex},
};

/// Whether an output asked for needs a thread of its own: not while a live
/// one is already open (or opening) for it, and not while it stands as
/// failed. A terminated entry is one on its way out and does not count --
/// a device asked for again while its old thread winds down gets a new
/// one, which shared mode allows.
///
/// Pure, so the rule that decides a spawn can be pinned: the list it reads
/// is published BEFORE the open (see `use_audio_outputs`), and reading it
/// wrongly is how one device ends up with two threads.
fn should_spawn(
    known: impl Iterator<Item = (AudioDeviceId, bool)>,
    failed: &HashSet<AudioDeviceId>,
    device_id: AudioDeviceId,
) -> bool {
    let live = known.into_iter().any(|(id, terminated)| id == device_id && !terminated);
    !live && !failed.contains(&device_id)
}

/// Every output thread is told apart from every other by this, not by its
/// device: two threads for one device can exist for a moment -- one
/// winding down, one opening -- and each must find and remove its OWN
/// entry, or the old one's exit takes the new one's entry with it and the
/// next scan opens the device a third time.
static NEXT_OUTPUT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// What an output thread leaves behind however it leaves: its entry gone
/// from the list and the app told. A guard rather than code at the end
/// of the thread, because a thread that dies on a device error -- a cable
/// pulled between two calls -- never reaches the end, and the entry it
/// left behind then refused the device its own return.
struct OutputLease {
    outputs: Arc<Mutex<Vec<WasapiBaseRef>>>,
    serial: u64,
    change_signal: SignalToUI,
}

impl Drop for OutputLease {
    fn drop(&mut self) {
        // Poisoned or not, the list is edited: this may be running because
        // another thread died holding it.
        let mut outputs = match self.outputs.lock() {
            Ok(outputs) => outputs,
            Err(poisoned) => poisoned.into_inner(),
        };
        outputs.retain(|v| v.serial != self.serial);
        drop(outputs);
        self.change_signal.set();
    }
}

/// Elevate the current thread to Pro Audio priority using Windows MMCSS
/// Returns the task handle for later cleanup, or None if failed
fn elevate_audio_thread_priority() -> Option<HANDLE> {
    unsafe {
        let mut task_index: u32 = 0;
        // "Pro Audio" gives the highest priority for audio processing
        let task_name: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
        let handle = AvSetMmThreadCharacteristicsW(PCWSTR(task_name.as_ptr()), &mut task_index);
        if handle.is_err() {
            println!("Warning: Failed to elevate audio thread priority");
            None
        } else {
            Some(handle.unwrap())
        }
    }
}

pub struct WasapiAccess {
    change_signal: SignalToUI,
    pub change_listener: IMMNotificationClient,
    pub audio_input_cb: [Arc<Mutex<Option<AudioInputFn>>>; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<AudioOutputFn>>>; MAX_AUDIO_DEVICE_INDEX],
    enumerator: IMMDeviceEnumerator,
    audio_inputs: Arc<Mutex<Vec<WasapiBaseRef>>>,
    audio_outputs: Arc<Mutex<Vec<WasapiBaseRef>>>,
    descs: Vec<AudioDeviceDesc>,
    failed_devices: Arc<Mutex<HashSet<AudioDeviceId>>>,
}

impl WasapiAccess {
    pub fn new(change_signal: SignalToUI) -> Arc<Mutex<Self>> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).unwrap();
            let change_listener: IMMNotificationClient = WasapiChangeListener {
                change_signal: change_signal.clone(),
            }
            .into();
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            enumerator
                .RegisterEndpointNotificationCallback(&change_listener)
                .unwrap();
            //let change_listener:IMMNotificationClient = WasapiChangeListener{}.into();
            change_signal.set();
            Arc::new(Mutex::new(WasapiAccess {
                change_signal,
                enumerator: enumerator,
                change_listener: change_listener,
                audio_input_cb: Default::default(),
                audio_output_cb: Default::default(),
                audio_inputs: Default::default(),
                audio_outputs: Default::default(),
                failed_devices: Default::default(),
                descs: Default::default(),
            }))
        }
    }

    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceDesc> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let mut out = Vec::new();
            Self::enumerate_devices(AudioDeviceType::Input, &enumerator, &mut out);
            Self::enumerate_devices(AudioDeviceType::Output, &enumerator, &mut out);
            // Also enumerate output devices as loopback inputs (for capturing system audio)
            Self::enumerate_loopback_devices(&enumerator, &mut out);
            self.descs = out;
        }
        // Match Linux/macOS: once open fails, mark the device so default_* skips it
        // and apps stop retrying forever via change_signal → use_audio_outputs.
        let failed = self.failed_devices.lock().unwrap();
        for d in &mut self.descs {
            d.has_failed = failed.contains(&d.device_id);
        }
        self.descs.clone()
    }

    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_inputs = self.audio_inputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_inputs.iter_mut().for_each(|v| {
                if !devices.contains(&v.device_id) {
                    v.signal_termination();
                }
            });
            // create the new ones
            let failed = self.failed_devices.lock().unwrap();
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_inputs
                    .iter()
                    .find(|v| v.device_id == *device_id)
                    .is_none()
                    && !failed.contains(device_id)
                {
                    let is_loopback = self.is_loopback_device(*device_id);
                    let channel_count = self
                        .descs
                        .iter()
                        .find(|d| d.device_id == *device_id)
                        .map(|d| d.channel_count)
                        .unwrap_or(2);
                    new.push((index, *device_id, is_loopback, channel_count))
                }
            }
            new
        };
        for (index, device_id, is_loopback, channel_count) in new {
            let audio_input_cb = self.audio_input_cb[index].clone();
            let audio_inputs = self.audio_inputs.clone();
            let failed_devices = self.failed_devices.clone();
            let change_signal = self.change_signal.clone();

            if is_loopback {
                // Use loopback capture for output devices
                std::thread::spawn(move || {
                    let _mmcss_handle = elevate_audio_thread_priority();
                    if let Ok(mut wasapi) = WasapiLoopback::new(device_id, channel_count) {
                        let sample_rate = wasapi.base.sample_rate;
                        audio_inputs.lock().unwrap().push(wasapi.get_ref());
                        while let Ok(buffer) = wasapi.wait_for_buffer() {
                            // Use try_lock to avoid blocking the audio thread
                            if let Ok(inputs) = audio_inputs.try_lock() {
                                if inputs
                                    .iter()
                                    .find(|v| v.device_id == device_id && v.is_terminated)
                                    .is_some()
                                {
                                    break;
                                }
                            }
                            // Use try_lock - if we can't get the lock, skip this buffer
                            if let Ok(mut cb_guard) = audio_input_cb.try_lock() {
                                if let Some(fbox) = &mut *cb_guard {
                                    fbox(
                                        AudioInfo {
                                            device_id,
                                            time: None,
                                            sample_rate,
                                        },
                                        &buffer,
                                    );
                                }
                            }
                            wasapi.release_buffer(buffer);
                        }
                        let mut audio_inputs = audio_inputs.lock().unwrap();
                        audio_inputs.retain(|v| v.device_id != device_id);
                    } else {
                        println!("Error opening wasapi loopback device");
                        failed_devices.lock().unwrap().insert(device_id);
                        change_signal.set();
                    }
                });
            } else {
                // Use regular input capture
                std::thread::spawn(move || {
                    let _mmcss_handle = elevate_audio_thread_priority();
                    if let Ok(mut wasapi) = WasapiInput::new(device_id, channel_count) {
                        let sample_rate = wasapi.base.sample_rate;
                        audio_inputs.lock().unwrap().push(wasapi.base.get_ref());
                        while let Ok(buffer) = wasapi.wait_for_buffer() {
                            // Use try_lock to avoid blocking the audio thread
                            if let Ok(inputs) = audio_inputs.try_lock() {
                                if inputs
                                    .iter()
                                    .find(|v| v.device_id == device_id && v.is_terminated)
                                    .is_some()
                                {
                                    break;
                                }
                            }
                            // Use try_lock - if we can't get the lock, skip this buffer
                            if let Ok(mut cb_guard) = audio_input_cb.try_lock() {
                                if let Some(fbox) = &mut *cb_guard {
                                    fbox(
                                        AudioInfo {
                                            device_id,
                                            time: None,
                                            sample_rate,
                                        },
                                        &buffer,
                                    );
                                }
                            }
                            wasapi.release_buffer(buffer);
                        }
                        let mut audio_inputs = audio_inputs.lock().unwrap();
                        audio_inputs.retain(|v| v.device_id != device_id);
                    } else {
                        println!("Error opening wasapi input device");
                        failed_devices.lock().unwrap().insert(device_id);
                        change_signal.set();
                    }
                });
            }
        }
    }

    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_outputs = self.audio_outputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_outputs.iter_mut().for_each(|v| {
                if !devices.contains(&v.device_id) {
                    v.signal_termination();
                }
            });
            // create the new ones
            let failed = self.failed_devices.lock().unwrap();
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                let known = audio_outputs.iter().map(|v| (v.device_id, v.is_terminated));
                if should_spawn(known, &failed, *device_id) {
                    let channel_count = self
                        .descs
                        .iter()
                        .find(|d| d.device_id == *device_id)
                        .map(|d| d.channel_count)
                        .unwrap_or(2);
                    // Published BEFORE the open, not after it: opening takes
                    // long enough for a second device scan to land -- the
                    // ordinary startup, where the endpoint watcher fires
                    // right after the first enumeration -- and the scan
                    // reads this same list to decide whether to spawn. An
                    // entry that only appeared after the open let that
                    // second scan open the device again, and two threads
                    // then rendered the program into one endpoint at twice
                    // the rate, garbled, for the rest of the set. It also
                    // means a terminate arriving mid-open is not lost.
                    let serial = NEXT_OUTPUT_SERIAL.fetch_add(1, Ordering::Relaxed);
                    audio_outputs.push(WasapiBaseRef {
                        device_id: *device_id,
                        is_terminated: false,
                        event: HANDLE(std::ptr::null_mut()),
                        serial,
                    });
                    new.push((index, *device_id, channel_count, serial))
                }
            }
            new
        };
        for (index, device_id, channel_count, serial) in new {
            let audio_output_cb = self.audio_output_cb[index].clone();
            let audio_outputs = self.audio_outputs.clone();
            let failed_devices = self.failed_devices.clone();
            let change_signal = self.change_signal.clone();

            std::thread::spawn(move || {
                let _mmcss_handle = elevate_audio_thread_priority();
                let opened = WasapiOutput::new(device_id, channel_count);
                // Whatever happens from here -- the loop ends, the open
                // fails, a call on a vanished device unwinds the thread --
                // the entry goes and the app is told. A thread that died on
                // a device error used to leave its entry behind, and that
                // ghost then refused the device its own return.
                let lease = OutputLease {
                    outputs: audio_outputs.clone(),
                    serial,
                    change_signal,
                };
                let Ok(mut wasapi) = opened else {
                    println!("Error opening wasapi output device");
                    failed_devices.lock().unwrap().insert(device_id);
                    return;
                };
                let sample_rate = wasapi.base.sample_rate;
                // The entry is already published; give it the event so a
                // terminate can wake this thread, and honour one that
                // arrived while the device was opening.
                let terminated_meanwhile = {
                    let mut outputs = audio_outputs.lock().unwrap();
                    match outputs.iter_mut().find(|v| v.serial == serial) {
                        Some(entry) => {
                            entry.event = wasapi.base.event;
                            entry.is_terminated
                        }
                        // Nothing waits for a thread nobody is tracking.
                        None => true,
                    }
                };
                while !terminated_meanwhile {
                    let Ok(mut buffer) = wasapi.wait_for_buffer() else { break };
                    // Use try_lock to avoid blocking the audio thread
                    if let Ok(outputs) = audio_outputs.try_lock() {
                        if outputs
                            .iter()
                            .find(|v| v.serial == serial && v.is_terminated)
                            .is_some()
                        {
                            break;
                        }
                    }
                    // Use try_lock - if we can't get the lock, output silence this frame
                    if let Ok(mut cb_guard) = audio_output_cb.try_lock() {
                        if let Some(fbox) = &mut *cb_guard {
                            fbox(
                                AudioInfo {
                                    device_id,
                                    time: None,
                                    sample_rate,
                                },
                                &mut buffer.audio_buffer,
                            );
                        }
                    }
                    // A device that went away between the wait and the
                    // release ends the loop, not the process.
                    if wasapi.release_buffer(buffer).is_err() {
                        break;
                    }
                }
                // The entry goes before the device does: a terminate from the
                // app side wakes this thread through the entry's event, and
                // must not find the handle closed.
                drop(lease);
            });
        }
    }

    unsafe fn get_device_descs(device: &IMMDevice) -> (String, String) {
        let dev_id = device.GetId().unwrap();
        let props = device.OpenPropertyStore(STGM_READ).unwrap();
        let value = props.GetValue(&PKEY_Device_FriendlyName).unwrap();
        let dev_name = if value.Anonymous.Anonymous.vt.0 == 31 {
            value
                .Anonymous
                .Anonymous
                .Anonymous
                .pwszVal
                .to_string()
                .unwrap_or_default()
        } else {
            String::new()
        };
        (dev_name, dev_id.to_string().unwrap())
    }

    /// Get the native channel count from the device's mix format
    unsafe fn get_device_channel_count(device: &IMMDevice) -> usize {
        if let Ok(client) = device.Activate::<IAudioClient>(CLSCTX_ALL, None) {
            if let Ok(mix_format) = client.GetMixFormat() {
                let channel_count = (*mix_format).nChannels as usize;
                // Free the format allocated by WASAPI
                crate::windows::Win32::System::Com::CoTaskMemFree(Some(
                    mix_format as *const _ as *const _,
                ));
                return channel_count;
            }
        }
        // Default to 2 channels if we can't query
        2
    }

    // add audio device enumeration for input and output
    unsafe fn enumerate_devices(
        device_type: AudioDeviceType,
        enumerator: &IMMDeviceEnumerator,
        out: &mut Vec<AudioDeviceDesc>,
    ) {
        let flow = match device_type {
            AudioDeviceType::Output => eRender,
            AudioDeviceType::Input => eCapture,
            AudioDeviceType::Loopback => eRender, // Loopback uses render devices
        };
        let def_device = enumerator.GetDefaultAudioEndpoint(flow, eConsole);
        if def_device.is_err() {
            return;
        }
        let def_device = def_device.unwrap();
        let (_, def_id) = Self::get_device_descs(&def_device);
        let col = enumerator
            .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
            .unwrap();
        let count = col.GetCount().unwrap();
        for i in 0..count {
            let device = col.Item(i).unwrap();
            let (dev_name, dev_id) = Self::get_device_descs(&device);
            let device_id = AudioDeviceId(LiveId::from_str(&dev_id));
            let channel_count = Self::get_device_channel_count(&device);
            out.push(AudioDeviceDesc {
                has_failed: false,
                device_id,
                device_type,
                is_default: def_id == dev_id,
                channel_count,
                name: dev_name,
            });
        }
    }

    // Enumerate output devices as loopback inputs for capturing system audio
    unsafe fn enumerate_loopback_devices(
        enumerator: &IMMDeviceEnumerator,
        out: &mut Vec<AudioDeviceDesc>,
    ) {
        let def_device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole);
        if def_device.is_err() {
            return;
        }
        let def_device = def_device.unwrap();
        let (_, def_id) = Self::get_device_descs(&def_device);
        let col = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .unwrap();
        let count = col.GetCount().unwrap();
        for i in 0..count {
            let device = col.Item(i).unwrap();
            let (dev_name, dev_id) = Self::get_device_descs(&device);
            // Create a distinct device_id for loopback by appending "_loopback" to the id
            let loopback_id = format!("{}_loopback", dev_id);
            let device_id = AudioDeviceId(LiveId::from_str(&loopback_id));
            let channel_count = Self::get_device_channel_count(&device);
            out.push(AudioDeviceDesc {
                has_failed: false,
                device_id,
                device_type: AudioDeviceType::Loopback,
                is_default: def_id == dev_id,
                channel_count,
                name: format!("{} (Loopback)", dev_name),
            });
        }
    }

    unsafe fn find_device_by_id(search_device_id: AudioDeviceId) -> Option<IMMDevice> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let col = enumerator
            .EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE)
            .unwrap();
        let count = col.GetCount().unwrap();
        for i in 0..count {
            let device = col.Item(i).unwrap();
            let (_, dev_id) = Self::get_device_descs(&device);
            let device_id = AudioDeviceId(LiveId::from_str(&dev_id));
            if device_id == search_device_id {
                return Some(device);
            }
        }
        None
    }

    // Find the output device for a loopback device id (strips the "_loopback" suffix)
    unsafe fn find_loopback_device_by_id(search_device_id: AudioDeviceId) -> Option<IMMDevice> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let col = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .unwrap();
        let count = col.GetCount().unwrap();
        for i in 0..count {
            let device = col.Item(i).unwrap();
            let (_, dev_id) = Self::get_device_descs(&device);
            // Create the loopback id to match against
            let loopback_id = format!("{}_loopback", dev_id);
            let device_id = AudioDeviceId(LiveId::from_str(&loopback_id));
            if device_id == search_device_id {
                return Some(device);
            }
        }
        None
    }

    // Check if a device_id is a loopback device
    pub fn is_loopback_device(&self, device_id: AudioDeviceId) -> bool {
        self.descs
            .iter()
            .any(|d| d.device_id == device_id && d.device_type == AudioDeviceType::Loopback)
    }

    fn new_float_waveformatextensible(
        samplerate: usize,
        channel_count: usize,
    ) -> WAVEFORMATEXTENSIBLE {
        let storebits = 32;
        let validbits = 32;
        let blockalign = channel_count * storebits / 8;
        let byterate = samplerate * blockalign;
        let wave_format = WAVEFORMATEX {
            cbSize: 22,
            nAvgBytesPerSec: byterate as u32,
            nBlockAlign: blockalign as u16,
            nChannels: channel_count as u16,
            nSamplesPerSec: samplerate as u32,
            wBitsPerSample: storebits as u16,
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
        };
        let sample = WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: validbits as u16,
        };
        let subformat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;

        let mask = match channel_count {
            ch if ch <= 18 => {
                // setting bit for each channel
                (1 << ch) - 1
            }
            _ => 0,
        };
        WAVEFORMATEXTENSIBLE {
            Format: wave_format,
            Samples: sample,
            SubFormat: subformat,
            dwChannelMask: mask,
        }
    }
}

struct WasapiBaseRef {
    device_id: AudioDeviceId,
    is_terminated: bool,
    /// The wake event of the thread this entry stands for, or null while
    /// that thread is still opening the device: an output's entry is
    /// published before its open, so the scan that decides whether to
    /// spawn can see it.
    event: HANDLE,
    /// Which thread this entry is (outputs only; inputs leave it zero).
    serial: u64,
}

unsafe impl Send for WasapiBaseRef {}
unsafe impl Sync for WasapiBaseRef {}

struct WasapiBase {
    device_id: AudioDeviceId,
    device: IMMDevice,
    frames: u32,
    event: HANDLE,
    client: IAudioClient,
    channel_count: usize,
    sample_rate: f64,
    audio_buffer: Option<AudioBuffer>,
}

impl WasapiBaseRef {
    pub fn signal_termination(&mut self) {
        self.is_terminated = true;
        // A thread still opening its device has no event yet; it reads the
        // flag the moment the open lands and leaves. A thread that has
        // already gone took its handle with it; the flag is enough.
        if !self.event.is_invalid() {
            unsafe {
                let _ = SetEvent(self.event);
            }
        }
    }
}

impl Drop for WasapiBase {
    /// The stream stopped and the wake event closed: an event handle was
    /// leaked per open, and the client was never told to stop.
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            if !self.event.is_invalid() {
                let _ = CloseHandle(self.event);
            }
        }
    }
}

impl WasapiBase {
    fn get_ref(&self) -> WasapiBaseRef {
        WasapiBaseRef {
            is_terminated: false,
            device_id: self.device_id,
            event: self.event,
            serial: 0,
        }
    }

    pub fn new(device_id: AudioDeviceId, channel_count: usize) -> Result<Self, ()> {
        unsafe {
            let channel_count = channel_count.min(2);
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

            let device = WasapiAccess::find_device_by_id(device_id).ok_or(())?;

            // Shared-mode streams must match (or autoconvert toward) the mix format.
            // Hardcoding 48000 fails on devices whose engine runs at 44100/96000/etc.
            let sample_rate = {
                let probe: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|_| ())?;
                let mix = probe.GetMixFormat().map_err(|_| ())?;
                let rate = (*mix).nSamplesPerSec as usize;
                crate::windows::Win32::System::Com::CoTaskMemFree(Some(
                    mix as *const _ as *const _,
                ));
                if rate == 0 {
                    48000
                } else {
                    rate
                }
            };
            let wave_format =
                WasapiAccess::new_float_waveformatextensible(sample_rate, channel_count);
            let wave_ptr = &wave_format as *const _
                as *const crate::windows::Win32::Media::Audio::WAVEFORMATEX;

            // Prefer IAudioClient3 low-latency shared stream when available.
            if let Ok(client3) = device.Activate::<IAudioClient3>(CLSCTX_ALL, None) {
                let mut default_period_frames = 0u32;
                let mut fundamental_period_frames = 0u32;
                let mut min_period_frames = 0u32;
                let mut max_period_frames = 0u32;
                if client3
                    .GetSharedModeEnginePeriod(
                        wave_ptr,
                        &mut default_period_frames,
                        &mut fundamental_period_frames,
                        &mut min_period_frames,
                        &mut max_period_frames,
                    )
                    .is_ok()
                    && client3
                        .InitializeSharedAudioStream(
                            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                            default_period_frames,
                            wave_ptr,
                            None,
                        )
                        .is_ok()
                {
                    let event = CreateEventA(None, false, false, None).map_err(|_| ())?;
                    client3.SetEventHandle(event).map_err(|_| ())?;
                    client3.Start().map_err(|_| ())?;
                    let client: IAudioClient = client3.cast().map_err(|_| ())?;
                    return Ok(Self {
                        device_id,
                        frames: default_period_frames.max(1),
                        device,
                        channel_count,
                        sample_rate: sample_rate as f64,
                        audio_buffer: Some(Default::default()),
                        event,
                        client,
                    });
                }
            }

            // Fallback: classic shared-mode client with PCM autoconvert.
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|_| ())?;
            let mut def_period = 0i64;
            let mut min_period = 0i64;
            client
                .GetDevicePeriod(Some(&mut def_period), Some(&mut min_period))
                .map_err(|_| ())?;
            if client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    def_period,
                    0,
                    wave_ptr,
                    None,
                )
                .is_err()
            {
                return Err(());
            }
            let event = CreateEventA(None, false, false, None).map_err(|_| ())?;
            client.SetEventHandle(event).map_err(|_| ())?;
            client.Start().map_err(|_| ())?;
            let frames =
                (((def_period as f64 / 10_000_000.0) * sample_rate as f64) as u32).max(1);
            Ok(Self {
                device_id,
                frames,
                device,
                channel_count,
                sample_rate: sample_rate as f64,
                audio_buffer: Some(Default::default()),
                event,
                client,
            })
        }
    }

    pub fn new_loopback(device_id: AudioDeviceId, channel_count: usize) -> Result<Self, ()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let channel_count = channel_count.min(2);
            // Find the output device that corresponds to this loopback device
            let device = WasapiAccess::find_loopback_device_by_id(device_id).ok_or(())?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|_| ())?;

            let mut def_period = 0i64;
            let mut min_period = 0i64;
            client
                .GetDevicePeriod(Some(&mut def_period), Some(&mut min_period))
                .map_err(|_| ())?;

            // Force at least 20ms buffer for loopback
            if def_period < 200_000 {
                def_period = 200_000;
            }

            let sample_rate = {
                let mix = client.GetMixFormat().map_err(|_| ())?;
                let rate = (*mix).nSamplesPerSec as usize;
                crate::windows::Win32::System::Com::CoTaskMemFree(Some(
                    mix as *const _ as *const _,
                ));
                if rate == 0 {
                    48000
                } else {
                    rate
                }
            };

            // Calculate frames from period (100-nanosecond units)
            let frames =
                (((def_period as f64 / 10_000_000.0) * sample_rate as f64) as u32).max(1);

            let wave_format =
                WasapiAccess::new_float_waveformatextensible(sample_rate, channel_count);

            // Use AUDCLNT_STREAMFLAGS_LOOPBACK to capture from the output device
            if client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_LOOPBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    def_period,
                    0, // hnsPeriodicity must be 0 for shared mode
                    &wave_format as *const _
                        as *const crate::windows::Win32::Media::Audio::WAVEFORMATEX,
                    None,
                )
                .is_err()
            {
                return Err(());
            }

            let event = CreateEventA(None, false, false, None).map_err(|_| ())?;
            client.SetEventHandle(event).map_err(|_| ())?;
            client.Start().map_err(|_| ())?;

            Ok(Self {
                device_id,
                frames,
                device,
                channel_count,
                sample_rate: sample_rate as f64,
                audio_buffer: Some(Default::default()),
                event,
                client,
            })
        }
    }
}

pub struct WasapiOutput {
    base: WasapiBase,
    render_client: IAudioRenderClient,
}

pub struct WasapiAudioOutputBuffer {
    frame_count: usize,
    channel_count: usize,
    device_buffer: *mut f32,
    pub audio_buffer: AudioBuffer,
}

impl WasapiOutput {
    pub fn new(device_id: AudioDeviceId, channel_count: usize) -> Result<Self, ()> {
        let base = WasapiBase::new(device_id, channel_count)?;
        let render_client = unsafe { base.client.GetService().map_err(|_| ())? };
        Ok(Self {
            render_client,
            base,
        })
    }

    pub fn wait_for_buffer(&mut self) -> Result<WasapiAudioOutputBuffer, ()> {
        unsafe {
            loop {
                if WaitForSingleObject(self.base.event, 2000) != WAIT_OBJECT_0 {
                    return Err(());
                };
                let Ok(padding) = self.base.client.GetCurrentPadding() else {
                    return Err(());
                };
                // Every call here can fail once the device has gone -- between
                // the padding and the buffer is exactly where a pulled cable
                // lands -- and a failure ends this thread, not the process.
                let buffer_size = self.base.client.GetBufferSize().map_err(|_| ())?;
                let req_size = buffer_size.saturating_sub(padding);
                if req_size > 0 {
                    let device_buffer = self.render_client.GetBuffer(req_size).map_err(|_| ())?;
                    let Some(mut audio_buffer) = self.base.audio_buffer.take() else {
                        return Err(());
                    };
                    let channel_count = self.base.channel_count;
                    // GetBuffer / GetCurrentPadding sizes are in frames, not samples.
                    let frame_count = req_size as usize;
                    audio_buffer.clear_final_size();
                    audio_buffer.resize(frame_count, channel_count);
                    audio_buffer.set_final_size();
                    return Ok(WasapiAudioOutputBuffer {
                        frame_count,
                        channel_count,
                        device_buffer: device_buffer as *mut f32,
                        audio_buffer,
                    });
                }
            }
        }
    }

    /// Hand the rendered frames to the device. Err when the device has
    /// gone; the audio buffer is kept either way, so the thread can leave
    /// cleanly rather than trip over a missing one on the way out.
    pub fn release_buffer(&mut self, output: WasapiAudioOutputBuffer) -> Result<(), ()> {
        unsafe {
            let device_buffer = std::slice::from_raw_parts_mut(
                output.device_buffer,
                output.frame_count * output.channel_count,
            );
            output.audio_buffer.copy_to_interleaved(device_buffer);
            let released = self.render_client.ReleaseBuffer(output.frame_count as u32, 0);
            self.base.audio_buffer = Some(output.audio_buffer);
            released.map_err(|_| ())
        }
    }
}

pub struct WasapiInput {
    base: WasapiBase,
    capture_client: IAudioCaptureClient,
}

pub struct WasapiAudioInputBuffer {
    pub audio_buffer: AudioBuffer,
}

impl WasapiInput {
    pub fn new(device_id: AudioDeviceId, channel_count: usize) -> Result<Self, ()> {
        let base = WasapiBase::new(device_id, channel_count)?;
        let capture_client = unsafe { base.client.GetService().map_err(|_| ())? };
        Ok(Self {
            capture_client,
            base,
        })
    }

    pub fn wait_for_buffer(&mut self) -> Result<AudioBuffer, ()> {
        unsafe {
            loop {
                if WaitForSingleObject(self.base.event, 2000) != WAIT_OBJECT_0 {
                    println!("Wait for object error");
                    return Err(());
                };
                let mut pdata: *mut u8 = 0 as *mut _;
                let mut frame_count = 0u32;
                let mut dwflags = 0u32;

                if self
                    .capture_client
                    .GetBuffer(&mut pdata, &mut frame_count, &mut dwflags, None, None)
                    .is_err()
                {
                    return Err(());
                }

                if frame_count == 0 {
                    continue;
                }

                let device_buffer = std::slice::from_raw_parts_mut(
                    pdata as *mut f32,
                    frame_count as usize * self.base.channel_count,
                );
                let Some(mut audio_buffer) = self.base.audio_buffer.take() else {
                    return Err(());
                };
                audio_buffer.copy_from_interleaved(self.base.channel_count, device_buffer);
                // A device that went away between the two calls: the buffer
                // goes back and the thread leaves, rather than the process.
                if self.capture_client.ReleaseBuffer(frame_count).is_err() {
                    self.base.audio_buffer = Some(audio_buffer);
                    return Err(());
                }
                return Ok(audio_buffer);
            }
        }
    }

    pub fn release_buffer(&mut self, buffer: AudioBuffer) {
        self.base.audio_buffer = Some(buffer);
    }
}

// Loopback capture - captures audio from output devices (speakers)
pub struct WasapiLoopback {
    base: WasapiBase,
    capture_client: IAudioCaptureClient,
}

impl WasapiLoopback {
    pub fn new(device_id: AudioDeviceId, channel_count: usize) -> Result<Self, ()> {
        let base = WasapiBase::new_loopback(device_id, channel_count)?;
        let capture_client = unsafe { base.client.GetService().map_err(|_| ())? };
        Ok(Self {
            capture_client,
            base,
        })
    }

    fn get_ref(&self) -> WasapiBaseRef {
        self.base.get_ref()
    }

    pub fn wait_for_buffer(&mut self) -> Result<AudioBuffer, ()> {
        unsafe {
            loop {
                if WaitForSingleObject(self.base.event, 2000) != WAIT_OBJECT_0 {
                    println!("Loopback: Wait for object error");
                    return Err(());
                };
                let mut pdata: *mut u8 = 0 as *mut _;
                let mut frame_count = 0u32;
                let mut dwflags = 0u32;

                if self
                    .capture_client
                    .GetBuffer(&mut pdata, &mut frame_count, &mut dwflags, None, None)
                    .is_err()
                {
                    return Err(());
                }

                if frame_count == 0 {
                    continue;
                }

                let device_buffer = std::slice::from_raw_parts_mut(
                    pdata as *mut f32,
                    frame_count as usize * self.base.channel_count,
                );
                let Some(mut audio_buffer) = self.base.audio_buffer.take() else {
                    return Err(());
                };
                audio_buffer.copy_from_interleaved(self.base.channel_count, device_buffer);
                // A device that went away between the two calls: the buffer
                // goes back and the thread leaves, rather than the process.
                if self.capture_client.ReleaseBuffer(frame_count).is_err() {
                    self.base.audio_buffer = Some(audio_buffer);
                    return Err(());
                }
                return Ok(audio_buffer);
            }
        }
    }

    pub fn release_buffer(&mut self, buffer: AudioBuffer) {
        self.base.audio_buffer = Some(buffer);
    }
}

pub(crate) struct WasapiChangeListener {
    change_signal: SignalToUI,
}

crate::implement_com! {
    for_struct: WasapiChangeListener,
    identity: IMMNotificationClient,
    wrapper_struct: WasapiChangeListener_Impl,
    interface_count: 1,
    interfaces: {
        0: IMMNotificationClient
    }
}

impl IMMNotificationClient_Impl for WasapiChangeListener_Impl {
    fn OnDeviceStateChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _dwnewstate: DEVICE_STATE,
    ) -> crate::windows::core::Result<()> {
        self.change_signal.set();
        Ok(())
    }
    fn OnDeviceAdded(&self, _pwstrdeviceid: &PCWSTR) -> crate::windows::core::Result<()> {
        self.change_signal.set();
        Ok(())
    }
    fn OnDeviceRemoved(&self, _pwstrdeviceid: &PCWSTR) -> crate::windows::core::Result<()> {
        self.change_signal.set();
        Ok(())
    }
    fn OnDefaultDeviceChanged(
        &self,
        _flow: EDataFlow,
        _role: ERole,
        _pwstrdefaultdeviceid: &crate::windows::core::PCWSTR,
    ) -> crate::windows::core::Result<()> {
        self.change_signal.set();
        Ok(())
    }
    fn OnPropertyValueChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> crate::windows::core::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_live_id::LiveId;

    fn id(n: u64) -> AudioDeviceId {
        AudioDeviceId(LiveId(n))
    }

    fn entry(device: u64, serial: u64) -> WasapiBaseRef {
        WasapiBaseRef {
            device_id: id(device),
            is_terminated: false,
            event: HANDLE(std::ptr::null_mut()),
            serial,
        }
    }

    /// A thread that dies mid-buffer -- the process unwinding it -- still
    /// takes its own entry out and nobody else's, so the device can come
    /// back and its neighbour goes on.
    #[test]
    fn a_thread_that_dies_takes_its_entry_with_it_and_nobody_elses() {
        let outputs = Arc::new(Mutex::new(vec![entry(1, 7), entry(2, 8)]));
        let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _lease = OutputLease {
                outputs: outputs.clone(),
                serial: 7,
                change_signal: SignalToUI::new(),
            };
            panic!("the device went away between two calls");
        }));
        assert!(died.is_err());
        let left: Vec<u64> = outputs.lock().unwrap().iter().map(|v| v.serial).collect();
        assert_eq!(left, vec![8], "its own entry gone, its neighbour's kept");
        // And the scan would open the device again now.
        let known: Vec<_> = outputs.lock().unwrap().iter().map(|v| (v.device_id, v.is_terminated)).collect();
        assert!(should_spawn(known.into_iter(), &HashSet::new(), id(1)));
    }

    #[test]
    fn a_device_already_open_or_opening_is_not_opened_again() {
        let failed = HashSet::new();
        let known = [(id(1), false), (id(2), true)];
        assert!(!should_spawn(known.iter().copied(), &failed, id(1)), "live: no second thread");
        assert!(should_spawn(known.iter().copied(), &failed, id(3)), "unknown: opened");
        assert!(should_spawn(known.iter().copied(), &failed, id(2)), "winding down: opened again");
        assert!(should_spawn(std::iter::empty(), &failed, id(1)), "nothing known: opened");
    }

    #[test]
    fn a_device_that_failed_is_not_retried_until_cleared() {
        let mut failed = HashSet::new();
        failed.insert(id(1));
        assert!(!should_spawn(std::iter::empty(), &failed, id(1)));
        failed.clear();
        assert!(should_spawn(std::iter::empty(), &failed, id(1)));
    }

    /// On this machine, against a real output: asked for twice back to
    /// back -- inside the open's window -- the device is opened once, and
    /// asked for by nobody it goes away.
    /// `cargo test -p makepad-platform --lib wasapi::tests -- --ignored`
    #[test]
    #[ignore]
    fn an_output_asked_for_twice_while_it_opens_is_opened_once() {
        let access = WasapiAccess::new(SignalToUI::new());
        let device = {
            let mut access = access.lock().unwrap();
            let descs = access.get_updated_descs();
            let device = descs
                .iter()
                .find(|d| d.is_default && d.device_type.is_output())
                .or_else(|| descs.iter().find(|d| d.device_type.is_output()))
                .map(|d| d.device_id);
            let Some(device) = device else {
                eprintln!("no output device on this machine; nothing to prove");
                return;
            };
            access.descs = descs;
            access.use_audio_outputs(&[device]);
            access.use_audio_outputs(&[device]);
            device
        };
        std::thread::sleep(std::time::Duration::from_millis(1500));
        {
            let access = access.lock().unwrap();
            let outputs = access.audio_outputs.lock().unwrap();
            let mine: Vec<_> = outputs.iter().filter(|v| v.device_id == device).collect();
            assert_eq!(mine.len(), 1, "one entry, one thread");
            assert!(!mine[0].event.is_invalid(), "and its thread has opened the device");
        }
        access.lock().unwrap().use_audio_outputs(&[]);
        std::thread::sleep(std::time::Duration::from_millis(800));
        let gone = access.lock().unwrap().audio_outputs.lock().unwrap().iter().all(|v| v.device_id != device);
        assert!(gone, "terminated, and its entry went with it");
    }
}
