
use {
    crate::{
        os::mswindows::win32_app::{FALSE},
        audio::{AudioBuffer},
        windows_crate::{
            Win32::Foundation::{
                WAIT_OBJECT_0,
                HANDLE,
            },
            Win32::System::Com::{
                CoInitialize,
                CoCreateInstance,
                CLSCTX_ALL,
                //STGM_READ,
                //VT_LPWSTR
            },
            Win32::Media::KernelStreaming::{
                WAVE_FORMAT_EXTENSIBLE,
            },
            Win32::Media::Multimedia::{
                KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
                //WAVE_FORMAT_IEEE_FLOAT
            },
            Win32::Media::Audio::{
                EDataFlow,
                WAVEFORMATEX,
                WAVEFORMATEXTENSIBLE,
                WAVEFORMATEXTENSIBLE_0,
                //WAVEFORMATEX,
                MMDeviceEnumerator,
                IMMDeviceEnumerator,
                //DEVICE_STATE_ACTIVE,
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                eRender,
                eCapture,
                eConsole,
                IAudioClient,
                //IMMDevice,
                IAudioCaptureClient,
                IAudioRenderClient,
            },
            Win32::System::Threading::{
                WaitForSingleObject,
                CreateEventA,
            },
            //Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
            
        }
    }
};

pub struct OsAudioDevice {
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

struct WasapiBase {
    event: HANDLE,
    client: IAudioClient,
    channel_count: usize,
    audio_buffer: Option<AudioBuffer>
}


/*
    // add audio device enumeration for input and output
    let col = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).unwrap();
    let count = col.GetCount().unwrap();
    for i in 0..count {
        let endpoint = col.Item(i).unwrap();
        let dev_id = endpoint.GetId().unwrap();
        let props = endpoint.OpenPropertyStore(STGM_READ).unwrap();
        let value = props.GetValue(&PKEY_Device_FriendlyName).unwrap();
        assert!(value.Anonymous.Anonymous.vt == VT_LPWSTR);
        let dev_name = value.Anonymous.Anonymous.Anonymous.pwszVal;
        println!("{}, {}", dev_id.to_string().unwrap(), dev_name.to_string().unwrap());
        
    }
*/

impl WasapiBase {
    pub fn new_default_output() -> Self {
        Self::new_default(eRender, 2)
    }
    
    pub fn new_default_input() -> Self {
        Self::new_default(eCapture, 1)
    }
    
    pub fn new_default(flow: EDataFlow, channel_count: usize) -> Self {
        unsafe {
            
            CoInitialize(None).unwrap();
            
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            
            let device = enumerator.GetDefaultAudioEndpoint(flow, eConsole).unwrap();
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).unwrap();
            
            let mut def_period = 0i64;
            let mut min_period = 0i64;
            client.GetDevicePeriod(Some(&mut def_period), Some(&mut min_period)).unwrap();
            
            let wave_format = new_float_waveformatextensible(48000, channel_count);
            
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                def_period,
                def_period,
                &wave_format as *const _ as *const crate::windows_crate::Win32::Media::Audio::WAVEFORMATEX,
                None
            ).unwrap();
            
            let event = CreateEventA(None, FALSE, FALSE, None).unwrap();
            client.SetEventHandle(event).unwrap();
            client.Start().unwrap();
            
            Self {
                channel_count,
                audio_buffer: Some(Default::default()),
                event,
                client,
            }
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
    pub fn new() -> Self {
        let base = WasapiBase::new_default_output();
        let render_client = unsafe {base.client.GetService().unwrap()};
        Self {
            render_client,
            base
        }
    }
    
    pub fn wait_for_buffer(&mut self) -> Result<WasapiAudioOutputBuffer, ()> {
        unsafe {
            loop {
                if WaitForSingleObject(self.base.event, 2000) != WAIT_OBJECT_0 {
                    return Err(())
                };
                let padding = self.base.client.GetCurrentPadding().unwrap();
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
            for i in 0..output.channel_count {
                let input_channel = output.audio_buffer.channel(i);
                for j in 0..output.frame_count {
                    device_buffer[j * output.channel_count + i] = input_channel[j];
                }
            }
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
    pub fn new() -> Self {
        let base = WasapiBase::new_default_input();
        let capture_client = unsafe {base.client.GetService().unwrap()};
        Self {
            capture_client,
            base
        }
    }
    
    
    pub fn wait_for_buffer(&mut self) -> Result<AudioBuffer, ()> {
        unsafe {
            loop {
                if WaitForSingleObject(self.base.event, 2000) != WAIT_OBJECT_0 {
                    return Err(())
                };
                let mut pdata: *mut u8 = 0 as *mut _;
                let mut frame_count = 0u32;
                let mut dwflags = 0u32;
                self.capture_client.GetBuffer(&mut pdata, &mut frame_count, &mut dwflags, None, None).unwrap();
                
                if frame_count == 0 {
                    continue;
                }
                
                let device_buffer = std::slice::from_raw_parts_mut(pdata as *mut f32, frame_count as usize * self.base.channel_count);
                
                let mut audio_buffer = self.base.audio_buffer.take().unwrap();
                
                audio_buffer.resize(frame_count as usize, self.base.channel_count);
                
                for i in 0..self.base.channel_count {
                    let output_channel = audio_buffer.channel_mut(i);
                    for j in 0..frame_count as usize {
                        output_channel[j] = device_buffer[j * self.base.channel_count + i]
                    }
                }
                
                self.capture_client.ReleaseBuffer(frame_count).unwrap();
                
                return Ok(audio_buffer);
            }
        }
    }
    
    pub fn release_buffer(&mut self, buffer: AudioBuffer) {
        self.base.audio_buffer = Some(buffer);
    }
}
