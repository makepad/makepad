
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
            Win32::Media::Audio::{
                //WAVEFORMATEX,  
                MMDeviceEnumerator, 
                IMMDeviceEnumerator,
                DEVICE_STATE_ACTIVE,
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
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

#[repr(C)]
#[allow(non_snake_case)]
struct WAVEFORMATEX{
    wFormatTag: u16,
    nChannels: u16,
    nSamplesPerSec: u32,
    nAvgBytesPerSec: u32,
    nBlockAlign: u16,
    wBitsPerSample: u16,
    cbSize: u16,
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
            
            let wave_format = client.GetMixFormat().unwrap();
            let wave_format_api = wave_format as *mut WAVEFORMATEX;
            //let wave_format_api = WAVEFORMATEXAPI(std::slice::from_raw_parts(wave_format as *const u8, std::mem::size_of::<WAVEFORMATEX>()));
            println!("Channels: {}", (*wave_format_api).wFormatTag);
            //(*wave_format_api).wFormatTag = 3;
            //(*wave_format_api).cbSize = std::mem::size_of::<WAVEFORMATEX>() as u16;
            
            //println!("SSEC: {}", wave_format_api.nSamplesPerSec());
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                0, // buffer duration
                0, // periodicity
                wave_format,
                None
            ).unwrap();
            
            let event = CreateEventA(None, FALSE, FALSE, None).unwrap();
            
            client.SetEventHandle(event).unwrap();
            
            client.Start().unwrap();
            
            let render_client = client.GetService().unwrap();
            Self {
                render_client,
                event,
                client,
                _device:device
            }
        }
    }
    
    pub fn wait_for_buffer(&mut self) -> Result<WasapiAudioOutputBuffer, ()> {
        unsafe {
            loop{
                if WaitForSingleObject(self.event, 2000) != WAIT_OBJECT_0 {
                    return Err(())
                };
                let buffer_size = self.client.GetBufferSize().unwrap();
                let padding = self.client.GetCurrentPadding().unwrap();
                let req_size =  buffer_size - padding;
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
            self.render_client.ReleaseBuffer((output_buffer.frame_count * output_buffer.channel_count) as u32, 0).unwrap();
        }
    }
}
