#![allow(dead_code)]
use {
    std::sync::{Arc, Mutex},
    crate::{
        makepad_live_id::*,
        os::mswindows::win32_app::{FALSE},
        audio::{
            AudioBuffer,
            AudioDeviceId,
            AudioTime,
            AudioDeviceDesc,
            AudioDeviceType,
            MAX_AUDIO_DEVICE_INDEX
        },
        windows_crate::{
            core::{
                PCWSTR,
            },
            Win32::Foundation::{
                WAIT_OBJECT_0,
                HANDLE,
            },
            Win32::System::Com::{
                STGM_READ,
                CoInitialize,
                CoCreateInstance,
                CLSCTX_ALL,
                //STGM_READ,
                VT_LPWSTR
            },
            Win32::UI::Shell::PropertiesSystem::{
                PROPERTYKEY
            },
            Win32::Media::KernelStreaming::{
                WAVE_FORMAT_EXTENSIBLE,
            },
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
                WaitForSingleObject,
                CreateEventA,
            },
            Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
            
        }
    }
};


#[derive(Default)]
pub struct WasapiAccess {
    pub audio_input_cb: [Arc<Mutex<Option<Box<dyn FnMut(AudioDeviceId, AudioTime, AudioBuffer) -> AudioBuffer + Send + 'static >> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<Box<dyn FnMut(AudioDeviceId, AudioTime, &mut AudioBuffer) + Send + 'static >> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_inputs: Arc<Mutex<Vec<WasapiInput >> >,
    pub audio_outputs: Arc<Mutex<Vec<WasapiOutput >> >,
    pub descs: Vec<AudioDeviceDesc>,
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
    let def_device = enumerator.GetDefaultAudioEndpoint(flow, eConsole).unwrap();
    let (_, def_id) = get_device_descs(&def_device);
    let col = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE).unwrap();
    let count = col.GetCount().unwrap();
    for i in 0..count {
        let device = col.Item(i).unwrap();
        let (dev_name, dev_id) = get_device_descs(&device);
        let device_id = AudioDeviceId(LiveId::from_str_unchecked(&dev_id));
        out.push(AudioDeviceDesc {
            device_id,
            device_type,
            is_default: def_id == dev_id,
            channels: 2,
            name: dev_name
        });
    }
}

unsafe fn find_device_by_id(search_device_id: AudioDeviceId)->Option<IMMDevice>{
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
    let col = enumerator.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE).unwrap();
    let count = col.GetCount().unwrap();
    for i in 0..count {
        let device = col.Item(i).unwrap();
        let (_, dev_id) = get_device_descs(&device);
        let device_id = AudioDeviceId(LiveId::from_str_unchecked(&dev_id));
        if device_id == search_device_id{
            return Some(device)
        }
    }
    None
}
    
impl WasapiAccess {
    pub fn new() -> Self {
        unsafe {CoInitialize(None).unwrap();}
        WasapiAccess::default()
    }
    
    pub fn update_device_list(&mut self) {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let mut out = Vec::new();
            enumerate_devices(AudioDeviceType::Input, &enumerator, &mut out);
            enumerate_devices(AudioDeviceType::Output, &enumerator, &mut out);
            self.descs = out;
        }
    }
    
    pub fn get_descs(&self) -> Vec<AudioDeviceDesc> {
        self.descs.clone()
    }
    
    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_inputs = self.audio_inputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_inputs.retain_mut( | v | {
                if devices.contains(&v.base.device_id) {
                    true
                }
                else {
                    // stop wasapi
                    false
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_inputs.iter().find( | v | v.base.device_id == *device_id).is_none() {
                    new.push((index, *device_id))
                }
            }
            new
        };
        for (index, device_id) in new {
            let audio_input_cb = self.audio_input_cb[index].clone();
            
            std::thread::spawn(move || {
                let mut wasapi = WasapiInput::new(device_id, 2);
                let sample_time = 0f64;
                let host_time = 0u64;
                let rate_scalar = 44100f64;
                loop {
                    let buffer = wasapi.wait_for_buffer().unwrap();
                    
                    if let Some(fbox) = &mut *audio_input_cb.lock().unwrap(){
                        let ret_buffer = fbox(
                            device_id,
                            AudioTime {
                            sample_time,
                            host_time,
                            rate_scalar
                        }, buffer);
                        wasapi.release_buffer(ret_buffer);
                    }
                    else{
                        wasapi.release_buffer(buffer);
                    }
                }
            });
        }
    }
    
    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_outputs = self.audio_outputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_outputs.retain_mut( | v | {
                if devices.contains(&v.base.device_id) {
                    true
                }
                else {
                    // shut it down
                    false
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_outputs.iter().find( | v | v.base.device_id == *device_id).is_none() {
                    new.push((index, *device_id))
                }
            }
            new
            
        };
        for (index, device_id) in new {
            let audio_output_cb = self.audio_output_cb[index].clone();
            
            std::thread::spawn(move || {
                
                let mut wasapi = WasapiOutput::new(device_id, 2);
                let sample_time = 0f64;
                let host_time = 0u64;
                let rate_scalar = 44100f64;
                loop {
                    let mut buffer = wasapi.wait_for_buffer().unwrap();
                    
                    if let Some(fbox) = &mut *audio_output_cb.lock().unwrap(){
                        fbox(
                            device_id,
                            AudioTime {
                            sample_time,
                            host_time,
                            rate_scalar
                        }, &mut buffer.audio_buffer);
                        wasapi.release_buffer(buffer);
                    }
                    else{
                        wasapi.release_buffer(buffer);
                    }
                    
                }
            });
            
            // lets create an audio input
            /*
            std::thread::spawn(move || {
                let mut wasapi = WasapiOutput::new();
                let sample_time = 0f64;
                let host_time = 0u64;
                let rate_scalar = 44100f64;
                loop {
                    let mut buffer = wasapi.wait_for_buffer().unwrap();
                    let mut fbox = fbox.lock().unwrap();
                    fbox(AudioTime {
                        sample_time,
                        host_time,
                        rate_scalar
                    }, &mut buffer.audio_buffer);
                    wasapi.release_buffer(buffer);
                }
            });*/
        }
    }
    
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
    device_id: AudioDeviceId,
    device:IMMDevice,
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
    
    pub fn new(device_id:AudioDeviceId, channel_count: usize) -> Self {
        unsafe {
            
            CoInitialize(None).unwrap();
            
            let device = find_device_by_id(device_id).unwrap();
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
                device_id,
                device,
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
    
     pub fn new(device_id:AudioDeviceId, channel_count: usize) -> Self {
        let base = WasapiBase::new(device_id, channel_count);
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
     pub fn new(device_id:AudioDeviceId, channel_count: usize) -> Self {
        let base = WasapiBase::new(device_id, channel_count);
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

#[macro_export]
macro_rules!windows_com_interface {
    (
        $ base_struct: ident( $ iface_count: tt)
            $ impl_struct: ident( $ ( $ iface_index: tt: $ iface: ident), *)
    ) => {
        #[repr(C)]
        struct $ impl_struct {
            identity: *const crate::windows_crate::core::IUnknown_Vtbl,
            vtables:
            ( $ (*const < $ iface as crate::windows_crate::core::Vtable> ::Vtable), *, ()),
            this: $ base_struct,
            count: crate::windows_crate::core::WeakRefCount,
        }
        
        impl $ impl_struct {
            const VTABLES:
            ( $ (< $ iface as crate::windows_crate::core::Vtable> ::Vtable), *, ()) =
            ( $ (< $ iface as crate::windows_crate::core::Vtable> ::Vtable ::new ::<Self, $ base_struct, {-1 - $ iface_index}>()), *, ());
            
            const IDENTITY: crate::windows_crate::core::IUnknown_Vtbl = crate::windows_crate::core::IUnknown_Vtbl::new::<Self,
            0> ();
            
            fn new(this: $ base_struct) -> Self {
                Self {
                    identity: &Self::IDENTITY,
                    vtables: ( $ (&Self::VTABLES. $ iface_index), *, ()),
                    this,
                    count: crate::windows_crate::core::WeakRefCount ::new(),
                }
            }
        }
        
        impl crate::windows_crate::core::IUnknownImpl for $ impl_struct {
            type Impl = $ base_struct;
            
            fn get_impl(&self) -> &Self ::Impl {
                &self.this
            }
            
            unsafe fn QueryInterface(&self, iid: &crate::windows_crate::core::GUID, interface: *mut *const ::core ::ffi ::c_void) -> crate::windows_crate::core::HRESULT {
                *interface =
                if iid == &<crate::windows_crate::core::IUnknown as crate::windows_crate::core::Interface> ::IID {
                    &self.identity as *const _ as *const _
                }
                $ (else if < $ iface as crate::windows_crate::core::Vtable> ::Vtable::matches(iid) {
                    &self.vtables. $ iface_index as *const _ as *const _
                }) *
                else {
                    std::ptr::null_mut()
                };
                
                if!(*interface).is_null() {
                    self.count.add_ref();
                    return crate::windows_crate::core::HRESULT(0);
                }
                
                *interface = self.count.query(iid, &self.identity as *const _ as *mut _);
                
                if (*interface).is_null() {
                    crate::windows_crate::core::HRESULT(-2147467262i32)
                }
                else {
                    crate::windows_crate::core::HRESULT(0)
                }
            }
            
            fn AddRef(&self) -> u32 {self.count.add_ref()}
            
            unsafe fn Release(&self) -> u32 {
                let remaining = self.count.release();
                if remaining == 0 {
                    let _ = Box ::from_raw(self as *const Self as *mut Self);
                } remaining
            }
        }
        
        impl $ base_struct {
            unsafe fn cast<I: crate::windows_crate::core::Interface> (&self) -> crate::windows_crate::core::Result<I> {
                let boxed = (self as *const _ as *const *mut std::ffi::c_void).sub(1 + 2) as *mut $ impl_struct;
                let mut result = None;
                < $ impl_struct as crate::windows_crate::core::IUnknownImpl>::QueryInterface(&*boxed, &I::IID, &mut result as *mut _ as _).and_some(result)
            }
        }
        
        impl std::convert::From< $ base_struct> for crate::windows_crate::core::IUnknown {
            fn from(this: $ base_struct) -> Self {
                let this = $ impl_struct::new(this);
                let boxed = std::mem::ManuallyDrop::new(Box::new(this));
                unsafe {std::mem ::transmute(&boxed.identity)}
            }
        }
        
        $ (
            impl std::convert::From< $ base_struct> for $ iface {
                fn from(this: $ base_struct) -> Self {
                    let this = $ impl_struct::new(this);
                    let this = std::mem::ManuallyDrop::new(Box::new(this));
                    let vtable_ptr = &this.vtables. $ iface_index;
                    unsafe {std::mem ::transmute(vtable_ptr)}
                }
            }
            
            impl crate::windows_crate::core::AsImpl< $ base_struct> for $ iface {
                fn as_impl(&self) -> & $ base_struct {
                    let this = crate::windows_crate::core::Vtable::as_raw(self);
                    unsafe {
                        let this = (this as *mut *mut ::core ::ffi ::c_void).sub(1 + $ iface_index) as *mut $ impl_struct ::< >;
                        &(*this).this
                    }
                }
            }
        ) *
    }
}

struct WasapiEvent {
}

windows_com_interface!(
    WasapiEvent(1)
    WasapiEvent_Impl(
        0: IMMNotificationClient
    )
);

impl IMMNotificationClient_Impl for WasapiEvent {
    fn OnDeviceStateChanged(&self, _pwstrdeviceid: &PCWSTR, _dwnewstate: u32) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
    fn OnDeviceAdded(&self, _pwstrdeviceid: &PCWSTR) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _pwstrdeviceid: &PCWSTR) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _pwstrdefaultdeviceid: &crate::windows_crate::core::PCWSTR) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _pwstrdeviceid: &PCWSTR, _key: &PROPERTYKEY) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
    
}

/*
#[repr(C)] 
struct Thing_Impl {
    identity: *const IInspectable_Vtbl,
    vtables:
    (*const <IInterfaceA  as Vtable> ::Vtable, * const <IInterfaceB as Vtable> ::Vtable,),
    this: Thing,
    count: WeakRefCount,
} 

impl Thing_Impl{
    const VTABLES:
    
    (<IInterfaceA as Vtable>::Vtable, 
        <IInterfaceB as Vtable>::Vtable,) =
        
    (<IInterfaceA as Vtable> Vtable::new::<Self, Thing, -1>(), 
     <IInterfaceB as Vtable> Vtable::new::<Self, Thing, -2>(),
    );
    
    const IDENTITY: IInspectable_Vtbl = IInspectable_Vtbl::new::<Self, IInterfaceA, 0> ();
    
    fn new(this: Thing ::< >) -> Self{
        Self{
            identity: &Self::IDENTITY,
            vtables: (&Self::VTABLES.0, &Self::VTABLES.1,),
            this,
            count: WeakRefCount ::new(),
        }
    }
}

impl IUnknownImpl for Thing_Impl{
    type Impl = Thing;
    
    fn get_impl(&self) -> &Self ::Impl{
        &self.this
    } 
    
    unsafe fn QueryInterface(&self, iid: &GUID, interface: * mut *const ::core ::ffi ::c_void) -> HRESULT{
        *interface = 
        if iid == &<IUnknown as Interface> ::IID 
           || iid == &<IInspectable as Interface> ::IID
           || iid == &<IAgileObject as Interface> ::IID{
            &self.identity as *const _ as *const _
        } 
        else if <IInterfaceA as Vtable> Vtable::matches(iid) {
            &self.vtables.0 as *const _ as *const _
        } 
        else if <IInterfaceB as Vtable> Vtable::matches(iid) {
            &self.vtables.1 as *const _ as *const _
        }
        else {
            std::ptr::null_mut()
        };
        
        if! (*interface).is_null(){
            self.count.add_ref();
            return HRESULT(0);
        } 
        
        *interface = self.count.query(iid, &self.identity as *const _ as *mut _);
        
        if (*interface).is_null(){
            HRESULT(0x8000_4002)
        } 
        else {
            HRESULT(0)
        }
    } 
    
    fn AddRef(&self) -> u32 {self.count.add_ref()} 
    
    unsafe fn Release(&self) -> u32 {
        let remaining = self.count.release();
        if remaining == 0{
            let _ = ::std ::boxed ::Box ::
            from_raw(self as *const Self as *mut Self);
        } remaining
    }
}

 impl Thing{
     unsafe fn cast<I:Interface> (&self) -> Result<I>{
        let boxed = (self as *const _ as *const *mut std::ffi::c_void).sub(1 + 2) as *mut Thing_Impl;
        let mut result = None;
        <Thing_Impl as IUnknownImpl>::QueryInterface(&*boxed, &I::IID, &mut result as *mut _ as _).and_some(result)
    }
} 

impl std::convert::From<Thing> for IUnknown{
    fn from(this: Thing) -> Self{
        let this = Thing_Impl::new(this);
        let boxed = std::mem::ManuallyDrop::new(Box::new(this));
        unsafe{std::mem ::transmute(&boxed.identity)}
    }
} 

impl std::convert::From<Thing> for IInspectable{
    fn from(this: Thing) -> Self{
        let this = Thing_Impl::new(this);
        let boxed = std::mem::ManuallyDrop::new(Box::new(this));
        unsafe{std::mem::transmute(&boxed.identity)}
    }
} 

impl < > ::core ::convert ::From < Thing ::< >> for IInterfaceA < >{
    fn from(this: Thing ::< >) -> Self
    {
        let this = Thing_Impl ::< > ::new(this);
        let mut this = ::core ::
        mem ::ManuallyDrop ::new(::std ::boxed ::Box ::new(this));
        let
        vtable_ptr = &this.vtables.0;
        unsafe
        {::core ::mem ::transmute(vtable_ptr)}
    }
} 

impl < > ::windows ::core ::AsImpl < Thing ::< >> for IInterfaceA < >{
    fn as_impl(&self) -> &Thing ::< >
    {
        let this = ::windows ::core ::Vtable ::as_raw(self);
        unsafe
        {
            let this =
            (this as *mut *mut ::core ::ffi ::c_void).sub(1 + 0) as *mut
            Thing_Impl ::< >;
            &(*this).this
        }
    }
} 

impl < > ::core ::convert ::From < Thing ::< >> for IInterfaceB < >{
    fn from(this: Thing ::< >) -> Self
    {
        let this = Thing_Impl ::< > ::new(this);
        let mut this = ::core ::
        mem ::ManuallyDrop ::new(::std ::boxed ::Box ::new(this));
        let
        vtable_ptr = &this.vtables.1;
        unsafe
        {::core ::mem ::transmute(vtable_ptr)}
    }
} 

impl < > ::windows ::core ::AsImpl < Thing ::< >> for IInterfaceB < >{
    fn as_impl(&self) -> &Thing ::< >
    {
        let this = ::windows ::core ::Vtable ::as_raw(self);
        unsafe
        {
            let this =
            (this as *mut *mut ::core ::ffi ::c_void).sub(1 + 1) as *mut
            Thing_Impl ::< >;
            &(*this).this
        }
    }
} 
*/
