
use {
    crate::{
        os::mswindows::win32_app::{FALSE},
        audio::{AudioOutputBuffer, AudioBuffer},
        windows_crate::{
            Win32::Foundation::{
                WAIT_OBJECT_0,
                HANDLE,
            },
            Win32::System::Com::{
                CoInitialize,
                CoCreateInstance,
                CLSCTX_ALL,
                STGM_READ,
                VT_LPWSTR
            },
            Win32::Media::KernelStreaming::{
                WAVE_FORMAT_EXTENSIBLE,
            },
            Win32::Media::Multimedia::{
                KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
                //WAVE_FORMAT_IEEE_FLOAT
            },
            Win32::Media::Audio::{
                WAVEFORMATEX,
                WAVEFORMATEXTENSIBLE,
                WAVEFORMATEXTENSIBLE_0,
                //WAVEFORMATEX,
                MMDeviceEnumerator,
                IMMDeviceEnumerator,
                DEVICE_STATE_ACTIVE,
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                eRender,
                eConsole,
                IAudioClient,
                IMMDevice,
                IAudioRenderClient,
            },
            Win32::System::Threading::{
                WaitForSingleObject,
                CreateEventA,
            },
            Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
            
        }
    }
};
/*
#[repr(C)]
#[allow(non_snake_case)]
struct WAVEFORMATEX {
    wFormatTag: u16,
    nChannels: u16,
    nSamplesPerSec: u32,
    nAvgBytesPerSec: u32,
    nBlockAlign: u16,
    wBitsPerSample: u16,
    cbSize: u16,
}

#[allow(non_snake_case)]
#[repr(C)]
union WAVEFORMATEXTENSIBLE_0 {
    wValidBitsPerSample: u16,
    wSamplesPerBlock: u16,
    wReserved: u16,
}

#[allow(non_snake_case)]
#[repr(C)]
struct WAVEFORMATEXTENSIBLE {
    Format: WAVEFORMATEX,
    Samples: WAVEFORMATEXTENSIBLE_0,
    dwChannelMask: u32,
    SubFormat: GUID
}*/
/*
impl fmt::Debug for WaveFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WaveFormat")
            .field("nAvgBytesPerSec", &{ self.wave_fmt.Format.nAvgBytesPerSec })
            .field("cbSize", &{ self.wave_fmt.Format.cbSize })
            .field("nBlockAlign", &{ self.wave_fmt.Format.nBlockAlign })
            .field("wBitsPerSample", &{ self.wave_fmt.Format.wBitsPerSample })
            .field("nSamplesPerSec", &{ self.wave_fmt.Format.nSamplesPerSec })
            .field("wFormatTag", &{ self.wave_fmt.Format.wFormatTag })
            .field("wValidBitsPerSample", &unsafe {
                self.wave_fmt.Samples.wValidBitsPerSample
            })
            .field("SubFormat", &{ self.wave_fmt.SubFormat })
            .field("nChannel", &{ self.wave_fmt.Format.nChannels })
            .field("dwChannelMask", &{ self.wave_fmt.dwChannelMask })
            .finish()
    }
}*/

fn new_float_waveformatextensible(storebits: usize, validbits: usize, samplerate: usize, channels: usize) -> WAVEFORMATEXTENSIBLE {
    let blockalign = channels * storebits / 8;
    let byterate = samplerate * blockalign;
    let wave_format = WAVEFORMATEX {
        cbSize: 22,
        nAvgBytesPerSec: byterate as u32,
        nBlockAlign: blockalign as u16,
        nChannels: channels as u16,
        nSamplesPerSec: samplerate as u32,
        wBitsPerSample: storebits as u16,
        wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
    };
    let sample = WAVEFORMATEXTENSIBLE_0 {
        wValidBitsPerSample: validbits as u16,
    };
    let subformat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT; //
    /*match sample_type {
        SampleType::Float => KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        SampleType::Int => KSDATAFORMAT_SUBTYPE_PCM,
    };*/
    let mask = match channels {
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

pub struct Wasapi {
    event: HANDLE,
    _device: IMMDevice,
    client: IAudioClient,
    render_client: IAudioRenderClient,
}

pub struct WasapiAudioOutputBuffer {
    frame_count: usize,
    channel_count: usize,
    buffer: *mut f32
}

impl WasapiAudioOutputBuffer {
    pub fn get_buffer_mut<'a>(&'a self) -> &mut [f32] {
        unsafe {
            std::slice::from_raw_parts_mut(self.buffer, self.frame_count * self.channel_count)
        }
    }
}

impl AudioOutputBuffer for WasapiAudioOutputBuffer {
    fn frame_count(&self) -> usize {self.frame_count}
    fn channel_count(&self) -> usize {self.channel_count}
    
    fn zero(&mut self) {
        let buffer = self.get_buffer_mut();
        for i in 0..buffer.len() {
            buffer[i] = 0.0;
        }
    }
    
    fn copy_from_buffer(&mut self, input_buffer: &AudioBuffer) {
        if self.channel_count() != input_buffer.channel_count {
            panic!("Output buffer channel_count != buffer channel_count {} {}", self.channel_count(), input_buffer.channel_count);
        }
        if self.frame_count() != input_buffer.frame_count {
            panic!("Output buffer frame_count != buffer frame_count ");
        }
        let output_buffer = self.get_buffer_mut();
        for i in 0..self.channel_count() {
            let input_channel = input_buffer.channel(i);
            for j in 0..self.frame_count() {
                output_buffer[j * self.channel_count() + i] = input_channel[j];
            }
        }
    }
}


impl Wasapi {
    pub fn new() -> Self {
        unsafe {
            CoInitialize(None).unwrap();
            
            let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            
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
            
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).unwrap();
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).unwrap();
            
            let mut def_period = 0i64;
            let mut min_period = 0i64;
            client.GetDevicePeriod(Some(&mut def_period), Some(&mut min_period)).unwrap();
            
            //let wave_format = client.GetMixFormat().unwrap();
            //let wave_format = WAVEFORMATEXTENSIBLE {
            //};
            let wave_format = new_float_waveformatextensible(32, 32, 44100, 2);
            
            //let wave_format_api = wave_format as *mut WAVEFORMATEXTENSIBLE;
            //let wave_format_api = WAVEFORMATEXAPI(std::slice::from_raw_parts(wave_format as *const u8, std::mem::size_of::<WAVEFORMATEX>()));
            //println!("wFormatTag: {:?}", (*wave_format_api).SubFormat);
            //println!("wFormatTag: {:?}", (*wave_format_api).samples.wValidBitsPerSample);
            //(*wave_format_api).wFormatTag = 3;
            //(*wave_format_api).cbSize = std::mem::size_of::<WAVEFORMATEX>() as u16;
            
            //println!("SSEC: {}", wave_format_api.nSamplesPerSec());
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
            
            let render_client = client.GetService().unwrap();
            
            client.Start().unwrap();
            
            Self {
                render_client,
                event,
                client,
                _device: device
            }
        }
    }
    
    pub fn wait_for_buffer(&mut self) -> Result<WasapiAudioOutputBuffer, ()> {
        unsafe {
            loop {
                if WaitForSingleObject(self.event, 2000) != WAIT_OBJECT_0 {
                    return Err(())
                };
                let padding = self.client.GetCurrentPadding().unwrap();
                let buffer_size = self.client.GetBufferSize().unwrap();
                let req_size = buffer_size - padding;
                if req_size > 0 {
                    let buffer = self.render_client.GetBuffer(req_size).unwrap();
                    return Ok(WasapiAudioOutputBuffer {
                        frame_count: (req_size / 2) as usize,
                        channel_count: 2, 
                        buffer: buffer as *mut _   
                    })
                }
            }
        }
    }
    
    pub fn release_buffer(&mut self, output_buffer: WasapiAudioOutputBuffer) {
        unsafe { 
            self.render_client.ReleaseBuffer(output_buffer.frame_count as u32, 0).unwrap();
        }
    }
}
