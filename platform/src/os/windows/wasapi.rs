#![allow(dead_code)]
use {
    std::sync::{Arc, Mutex},
    std::collections::HashSet,
    crate::{
        implement_com,
        makepad_live_id::*,
        os::windows::win32_app::FALSE,
        audio::*,
        thread::SignalToUI,
        windows::{
            core::PCWSTR,
            Win32::Foundation::{
                WAIT_OBJECT_0,
                HANDLE,
            },
            Win32::System::Com::{
                STGM_READ,
                COINIT_APARTMENTTHREADED,
                CoInitializeEx,
                CoCreateInstance,
                CLSCTX_ALL,
                //STGM_READ,
            },
            Win32::System::Variant::VT_LPWSTR,
            Win32::UI::Shell::PropertiesSystem::PROPERTYKEY,
            Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE,
            Win32::Media::Multimedia::{
                KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
                //WAVE_FORMAT_IEEE_FLOAT
            },
            Win32::Media::Audio::{
                IMMNotificationClient_Impl,
                EDataFlow,
                ERole,
                WAVEFORMATEX,
                WAVEFORMATEXTENSIBLE,
                WAVEFORMATEXTENSIBLE_0,
                //WAVEFORMATEX,
                MMDeviceEnumerator,
                IMMDeviceEnumerator,
                IMMNotificationClient,
                IMMDevice,
                DEVICE_STATE_ACTIVE,
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                eRender,
                eCapture,
                eConsole,
                eAll,
                IAudioClient,
                //IMMDevice,
                IAudioCaptureClient,
                IAudioRenderClient,
            },
            Win32::System::Threading::{
                SetEvent,
                WaitForSingleObject,
                CreateEventA,
            },
            Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
            
        }
    }
};


pub struct WasapiAccess {
    change_signal: SignalToUI,
    pub change_listener: IMMNotificationClient,
    pub audio_input_cb: [Arc<Mutex<Option<AudioInputFn> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<AudioOutputFn> > >; MAX_AUDIO_DEVICE_INDEX],
    enumerator: IMMDeviceEnumerator,
    audio_inputs: Arc<Mutex<Vec<WasapiBaseRef >> >,
    audio_outputs: Arc<Mutex<Vec<WasapiBaseRef >> >,
    descs: Vec<AudioDeviceDesc>,
    failed_devices: Arc<Mutex<HashSet<AudioDeviceId>>>,
}

impl WasapiAccess {
    pub fn new(change_signal:SignalToUI) -> Arc<Mutex<Self >> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).unwrap();
            let change_listener: IMMNotificationClient = WasapiChangeListener {change_signal:change_signal.clone()}.into();
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            enumerator.RegisterEndpointNotificationCallback(&change_listener).unwrap();
            //let change_listener:IMMNotificationClient = WasapiChangeListener{}.into();
            change_signal.set();
            Arc::new(Mutex::new(
                WasapiAccess {
                    change_signal,
                    enumerator: enumerator,
                    change_listener: change_listener,
                    audio_input_cb: Default::default(),
                    audio_output_cb: Default::default(),
                    audio_inputs: Default::default(),
                    audio_outputs: Default::default(),
                    failed_devices: Default::default(),
                    descs: Default::default(),
                }
            ))
        }
    }
    
    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceDesc> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let mut out = Vec::new();
            Self::enumerate_devices(AudioDeviceType::Input, &enumerator, &mut out);
            Self::enumerate_devices(AudioDeviceType::Output, &enumerator, &mut out);
            self.descs = out;
        }
        self.descs.clone()
    }
    
    
    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_inputs = self.audio_inputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_inputs.iter_mut().for_each( | v | {
                if !devices.contains(&v.device_id) {
                    v.signal_termination();
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_inputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    new.push((index, *device_id))
                }
            }
            new
        };
        for (index, device_id) in new {
            let audio_input_cb = self.audio_input_cb[index].clone();
            let audio_inputs = self.audio_inputs.clone();
            let failed_devices = self.failed_devices.clone();
            let change_signal = self.change_signal.clone();
            std::thread::spawn(move || {
                if let Ok(mut wasapi) = WasapiInput::new(device_id, 2){
                    audio_inputs.lock().unwrap().push(wasapi.base.get_ref());
                    while let Ok(buffer) = wasapi.wait_for_buffer() {
                        if audio_inputs.lock().unwrap().iter().find( | v | v.device_id == device_id && v.is_terminated).is_some() {
                            break;
                        }
                        if let Some(fbox) = &mut *audio_input_cb.lock().unwrap() {
                            fbox(
                                AudioInfo {
                                    device_id,
                                    time: None
                                },
                                &buffer
                            );
                            wasapi.release_buffer(buffer);
                        }
                        else {
                            wasapi.release_buffer(buffer);
                        }
                    }
                    let mut audio_inputs = audio_inputs.lock().unwrap();
                    audio_inputs.retain( | v | v.device_id != device_id);
                }
                else{
                    println!("Error opening wasapi input device");
                    failed_devices.lock().unwrap().insert(device_id);
                    change_signal.set();
                }
            });
        }
    }
    
    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_outputs = self.audio_outputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_outputs.iter_mut().for_each( | v | {
                if !devices.contains(&v.device_id) {
                    v.signal_termination();
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_outputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    new.push((index, *device_id))
                }
            }
            new
            
        };
        for (index, device_id) in new {
            let audio_output_cb = self.audio_output_cb[index].clone();
            let audio_outputs = self.audio_outputs.clone();
            let failed_devices = self.failed_devices.clone();
            let change_signal = self.change_signal.clone();
            
            std::thread::spawn(move || {
                if let Ok(mut wasapi) = WasapiOutput::new(device_id, 2){
                    audio_outputs.lock().unwrap().push(wasapi.base.get_ref());
                    while let Ok(mut buffer) = wasapi.wait_for_buffer() {
                        if audio_outputs.lock().unwrap().iter().find( | v | v.device_id == device_id && v.is_terminated).is_some() {
                            break;
                        }
                        if let Some(fbox) = &mut *audio_output_cb.lock().unwrap() {
                            fbox(
                                AudioInfo {
                                    device_id,
                                    time: None,
                                },
                                &mut buffer.audio_buffer
                            );
                            wasapi.release_buffer(buffer);
                        }
                        else {
                            wasapi.release_buffer(buffer);
                        }
                    }
                    let mut audio_outputs = audio_outputs.lock().unwrap();
                    audio_outputs.retain( | v | v.device_id != device_id);
                }
                else{
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
        assert!(value.Anonymous.Anonymous.vt == VT_LPWSTR);
        let dev_name = value.Anonymous.Anonymous.Anonymous.pwszVal;
        (dev_name.to_string().unwrap(), dev_id.to_string().unwrap())
    }
    
    // add audio device enumeration for input and output
    unsafe fn enumerate_devices(device_type: AudioDeviceType, enumerator: &IMMDeviceEnumerator, out: &mut Vec<AudioDeviceDesc>) {
        let flow = match device_type {
            AudioDeviceType::Output => eRender,
            AudioDeviceType::Input => eCapture
        };
        let def_device = enumerator.GetDefaultAudioEndpoint(flow, eConsole);
        if def_device.is_err() {
            return
        }
        let def_device = def_device.unwrap();
        let (_, def_id) = Self::get_device_descs(&def_device);
        let col = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE).unwrap();
        let count = col.GetCount().unwrap();
        for i in 0..count {
            let device = col.Item(i).unwrap();
            let (dev_name, dev_id) = Self::get_device_descs(&device);
            let device_id = AudioDeviceId(LiveId::from_str(&dev_id));
            out.push(AudioDeviceDesc {
                has_failed: false,
                device_id,
                device_type,
                is_default: def_id == dev_id,
                channel_count: 2,
                name: dev_name
            });
        }
    }
    
    unsafe fn find_device_by_id(search_device_id: AudioDeviceId) -> Option<IMMDevice> {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let col = enumerator.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE).unwrap();
        let count = col.GetCount().unwrap();
        for i in 0..count {
            let device = col.Item(i).unwrap();
            let (_, dev_id) = Self::get_device_descs(&device);
            let device_id = AudioDeviceId(LiveId::from_str(&dev_id));
            if device_id == search_device_id {
                return Some(device)
            }
        }
        None
    }
    
    fn new_float_waveformatextensible(samplerate: usize, channel_count: usize) -> WAVEFORMATEXTENSIBLE {
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

struct WasapiBase {
    device_id: AudioDeviceId,
    device: IMMDevice,
    event: HANDLE,
    client: IAudioClient,
    channel_count: usize,
    audio_buffer: Option<AudioBuffer>
}


impl WasapiBaseRef {
    pub fn signal_termination(&mut self) {
        self.is_terminated = true;
        unsafe {SetEvent(self.event).unwrap()};
    }
}

impl WasapiBase {
    pub fn get_ref(&self) -> WasapiBaseRef {
        WasapiBaseRef {
            is_terminated: false,
            device_id: self.device_id,
            event: self.event
        }
    }
    
    pub fn new(device_id: AudioDeviceId, channel_count: usize) -> Result<Self,()> {
        unsafe {
            
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).unwrap();
            
            let device = WasapiAccess::find_device_by_id(device_id).unwrap();
            let client: IAudioClient = if let Ok(client) = device.Activate(CLSCTX_ALL, None){
                client
            }
            else{
                return Err(())
            };
            
            let mut def_period = 0i64;
            let mut min_period = 0i64;
            client.GetDevicePeriod(Some(&mut def_period), Some(&mut min_period)).unwrap();
            let wave_format = WasapiAccess::new_float_waveformatextensible(48000, channel_count);
            
            if client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                def_period,
                def_period,
                &wave_format as *const _ as *const crate::windows::Win32::Media::Audio::WAVEFORMATEX,
                None
            ).is_err(){
                return Err(())
            }
            
            let event = CreateEventA(None, FALSE, FALSE, None).unwrap();
            client.SetEventHandle(event).unwrap();
            client.Start().unwrap();
            
            Ok(Self {
                device_id,
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
    pub audio_buffer: AudioBuffer
}

impl WasapiOutput {
    
    pub fn new(device_id: AudioDeviceId, channel_count: usize) -> Result<Self,()> {
        let base = WasapiBase::new(device_id, channel_count)?;
        let render_client = unsafe {base.client.GetService().unwrap()};
        Ok(Self {
            render_client,
            base
        })
    }
    
    pub fn wait_for_buffer(&mut self) -> Result<WasapiAudioOutputBuffer, ()> {
        unsafe {
            loop {
                if WaitForSingleObject(self.base.event, 2000) != WAIT_OBJECT_0 {
                    return Err(())
                };
                let padding = self.base.client.GetCurrentPadding();
                if padding.is_err() {
                    return Err(())
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
                    return Ok(WasapiAudioOutputBuffer {
                        frame_count,
                        channel_count,
                        device_buffer: device_buffer as *mut f32,
                        audio_buffer
                    })
                }
            }
        }
    }
    
    pub fn release_buffer(&mut self, output: WasapiAudioOutputBuffer) {
        unsafe {
            let device_buffer = std::slice::from_raw_parts_mut(output.device_buffer, output.frame_count * output.channel_count);
            output.audio_buffer.copy_to_interleaved(device_buffer);
            self.render_client.ReleaseBuffer(output.frame_count as u32, 0).unwrap();
            self.base.audio_buffer = Some(output.audio_buffer);
        }
    }
}

pub struct WasapiInput {
    base: WasapiBase,
    capture_client: IAudioCaptureClient,
}

pub struct WasapiAudioInputBuffer {
    pub audio_buffer: AudioBuffer
}

impl WasapiInput {
    pub fn new(device_id: AudioDeviceId, channel_count: usize) -> Result<Self,()> {
        let base = WasapiBase::new(device_id, channel_count)?;
        let capture_client = unsafe {base.client.GetService().unwrap()};
        Ok(Self {
            capture_client,
            base
        })
    }
    
    pub fn wait_for_buffer(&mut self) -> Result<AudioBuffer, ()> {
        unsafe {
            loop {
                if WaitForSingleObject(self.base.event, 2000) != WAIT_OBJECT_0 {
                    println!("Wait for object error");
                    return Err(())
                };
                let mut pdata: *mut u8 = 0 as *mut _;
                let mut frame_count = 0u32;
                let mut dwflags = 0u32;
                
                if self.capture_client.GetBuffer(&mut pdata, &mut frame_count, &mut dwflags, None, None).is_err() {
                    return Err(())
                }
                
                if frame_count == 0 {
                    continue;
                }
                
                let device_buffer = std::slice::from_raw_parts_mut(pdata as *mut f32, frame_count as usize * self.base.channel_count);
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

struct WasapiChangeListener {
    change_signal:SignalToUI
}

implement_com!{
    for_struct: WasapiChangeListener,
    identity: IMMNotificationClient,
    wrapper_struct: WasapiChangeListener_Com,
    interface_count: 1,
    interfaces: {
        0: IMMNotificationClient
    }
}

impl IMMNotificationClient_Impl for WasapiChangeListener {
    fn OnDeviceStateChanged(&self, _pwstrdeviceid: &PCWSTR, _dwnewstate: u32) -> crate::windows::core::Result<()> {
        self.change_signal.set();
        Ok(())
    }
    fn OnDeviceAdded(&self, _pwstrdeviceid: &PCWSTR) -> crate::windows::core::Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _pwstrdeviceid: &PCWSTR) -> crate::windows::core::Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _pwstrdefaultdeviceid: &crate::windows::core::PCWSTR) -> crate::windows::core::Result<()> {
        self.change_signal.set();
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _pwstrdeviceid: &PCWSTR, _key: &PROPERTYKEY) -> crate::windows::core::Result<()> {
        Ok(())
    }
    
}
