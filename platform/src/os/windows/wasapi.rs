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
            Win32::Foundation::{HANDLE, WAIT_OBJECT_0},
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
            Win32::Foundation::PROPERTYKEY,
        },
    },
    std::collections::HashSet,
    std::sync::{Arc, Mutex},
};

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
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_inputs
                    .iter()
                    .find(|v| v.device_id == *device_id)
                    .is_none()
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
                                            sample_rate: 48000.0,
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
                                            sample_rate: 48000.0,
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
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_outputs
                    .iter()
                    .find(|v| v.device_id == *device_id)
                    .is_none()
                {
                    let channel_count = self
                        .descs
                        .iter()
                        .find(|d| d.device_id == *device_id)
                        .map(|d| d.channel_count)
                        .unwrap_or(2);
                    new.push((index, *device_id, channel_count))
                }
            }
            new
        };
        for (index, device_id, channel_count) in new {
            let audio_output_cb = self.audio_output_cb[index].clone();
            let audio_outputs = self.audio_outputs.clone();
            let failed_devices = self.failed_devices.clone();
            let change_signal = self.change_signal.clone();

            std::thread::spawn(move || {
                let _mmcss_handle = elevate_audio_thread_priority();
                if let Ok(mut wasapi) = WasapiOutput::new(device_id, channel_count) {
                    audio_outputs.lock().unwrap().push(wasapi.base.get_ref());
                    while let Ok(mut buffer) = wasapi.wait_for_buffer() {
                        // Use try_lock to avoid blocking the audio thread
                        if let Ok(outputs) = audio_outputs.try_lock() {
                            if outputs
                                .iter()
                                .find(|v| v.device_id == device_id && v.is_terminated)
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
                                        sample_rate: 48000.0,
                                    },
                                    &mut buffer.audio_buffer,
                                );
                            }
                        }
                        wasapi.release_buffer(buffer);
                    }
                    let mut audio_outputs = audio_outputs.lock().unwrap();
                    audio_outputs.retain(|v| v.device_id != device_id);
                    change_signal.set();
                } else {
                    println!("Error opening wasapi output device");
                    failed_devices.lock().unwrap().insert(device_id);
                    change_signal.set();
                }
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
    event: HANDLE,
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
    audio_buffer: Option<AudioBuffer>,
}

impl WasapiBaseRef {
    pub fn signal_termination(&mut self) {
        self.is_terminated = true;
        unsafe { SetEvent(self.event).unwrap() };
    }
}

impl WasapiBase {
    fn get_ref(&self) -> WasapiBaseRef {
        WasapiBaseRef {
            is_terminated: false,
            device_id: self.device_id,
            event: self.event,
        }
    }

    pub fn new(device_id: AudioDeviceId, channel_count: usize) -> Result<Self, ()> {
        unsafe {
            let channel_count = channel_count.min(2);
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).unwrap();

            let device = WasapiAccess::find_device_by_id(device_id).unwrap();
            let client3: IAudioClient3 = if let Ok(client) = device.Activate(CLSCTX_ALL, None) {
                client
            } else {
                return Err(());
            };

            let wave_format = WasapiAccess::new_float_waveformatextensible(48000, channel_count);

            let mut default_period_frames = 0u32;
            let mut fundamental_period_frames = 0u32;
            let mut min_period_frames = 0u32;
            let mut max_period_frames = 0u32;
            if client3
                .GetSharedModeEnginePeriod(
                    &wave_format as *const _
                        as *const crate::windows::Win32::Media::Audio::WAVEFORMATEX,
                    &mut default_period_frames,
                    &mut fundamental_period_frames,
                    &mut min_period_frames,
                    &mut max_period_frames,
                )
                .is_err()
            {
                return Err(());
            }
            if client3
                .InitializeSharedAudioStream(
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                    default_period_frames,
                    &wave_format as *const _
                        as *const crate::windows::Win32::Media::Audio::WAVEFORMATEX,
                    None,
                )
                .is_err()
            {
                return Err(());
            }

            let event = CreateEventA(None, false, false, None).unwrap();
            client3.SetEventHandle(event).unwrap();
            client3.Start().unwrap();

            // Cast IAudioClient3 to IAudioClient for storage
            let client: IAudioClient = client3.cast().unwrap();

            Ok(Self {
                device_id,
                frames: default_period_frames,
                device,
                channel_count,
                audio_buffer: Some(Default::default()),
                event,
                client,
            })
        }
    }

    pub fn new_loopback(device_id: AudioDeviceId, channel_count: usize) -> Result<Self, ()> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).unwrap();
            let channel_count = channel_count.min(2);
            // Find the output device that corresponds to this loopback device
            let device = WasapiAccess::find_loopback_device_by_id(device_id).ok_or(())?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|_| ())?;

            let mut def_period = 0i64;
            let mut min_period = 0i64;
            client
                .GetDevicePeriod(Some(&mut def_period), Some(&mut min_period))
                .unwrap();

            // Force at least 20ms buffer for loopback
            if def_period < 200_000 {
                def_period = 200_000;
            }

            // Calculate frames from period (100-nanosecond units to frames at 48kHz)
            let frames = ((def_period as f64 / 10_000_000.0) * 48000.0) as u32;

            let wave_format = WasapiAccess::new_float_waveformatextensible(48000, channel_count);

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

            let event = CreateEventA(None, false, false, None).unwrap();
            client.SetEventHandle(event).unwrap();
            client.Start().unwrap();

            Ok(Self {
                device_id,
                frames,
                device,
                channel_count,
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
        let render_client = unsafe { base.client.GetService().unwrap() };
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
                let padding = self.base.client.GetCurrentPadding();
                if padding.is_err() {
                    return Err(());
                }
                let padding = padding.unwrap();
                let buffer_size = self.base.client.GetBufferSize().unwrap();
                let req_size = buffer_size - padding;
                if req_size > 0 {
                    let device_buffer = self.render_client.GetBuffer(req_size).unwrap();
                    let mut audio_buffer = self.base.audio_buffer.take().unwrap();
                    let channel_count = self.base.channel_count;
                    let frame_count = (req_size / channel_count as u32) as usize;
                    audio_buffer.clear_final_size();
                    audio_buffer.resize(frame_count, channel_count);
                    audio_buffer.set_final_size();
                    if (frame_count as u32) < self.base.frames {
                        println!(
                            "Wasapi glitch detected, resettting output device {}<{}",
                            frame_count, self.base.frames
                        );
                        return Err(());
                    }
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

    pub fn release_buffer(&mut self, output: WasapiAudioOutputBuffer) {
        unsafe {
            let device_buffer = std::slice::from_raw_parts_mut(
                output.device_buffer,
                output.frame_count * output.channel_count,
            );
            output.audio_buffer.copy_to_interleaved(device_buffer);
            self.render_client
                .ReleaseBuffer(output.frame_count as u32, 0)
                .unwrap();
            self.base.audio_buffer = Some(output.audio_buffer);
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
        let capture_client = unsafe { base.client.GetService().unwrap() };
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
                let mut audio_buffer = self.base.audio_buffer.take().unwrap();
                audio_buffer.copy_from_interleaved(self.base.channel_count, device_buffer);

                self.capture_client.ReleaseBuffer(frame_count).unwrap();

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
        let capture_client = unsafe { base.client.GetService().unwrap() };
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
                let mut audio_buffer = self.base.audio_buffer.take().unwrap();
                audio_buffer.copy_from_interleaved(self.base.channel_count, device_buffer);

                self.capture_client.ReleaseBuffer(frame_count).unwrap();

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
