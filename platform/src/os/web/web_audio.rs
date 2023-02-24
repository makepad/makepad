use {
    std::sync::{Arc, Mutex},
    self::super::{
        web::CxOs,
        to_wasm::ToWasmAudioDeviceList,
        from_wasm::{FromWasmQueryAudioDevices, FromWasmStartAudioOutput, FromWasmStopAudioOutput}
    },
    crate::{
        makepad_live_id::*,
        thread::Signal,
        audio::*,
    }
};

#[repr(C)]
pub struct WebAudioOutputClosure {
    pub callback: Box<dyn FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static>,
    pub device_id: AudioDeviceId,
    pub output_buffer: AudioBuffer,
}

pub struct WebAudioDevice {
    web_device_id: String,
    desc: AudioDeviceDesc
}

pub struct WebAudioAccess {
    pub audio_input_cb: [Arc<Mutex<Option<AudioInputFn> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<AudioOutputFn> > >; MAX_AUDIO_DEVICE_INDEX],
    pub devices: Vec<WebAudioDevice>,
    change_signal: Signal,
    self_arc: *const Mutex<WebAudioAccess>,
    output_device_id: AudioDeviceId,
    output_buffer: Option<AudioBuffer>,
}

impl WebAudioAccess {
    pub fn new(os: &mut CxOs, change_signal: Signal) -> Arc<Mutex<Self >> {
        // ok lets request audio inputs and outputs
        os.from_wasm(FromWasmQueryAudioDevices {});
        let ret = Arc::new(Mutex::new(Self {
            audio_input_cb: Default::default(),
            audio_output_cb: Default::default(),
            devices: Default::default(),
            change_signal,
            self_arc: std::ptr::null(),
            output_buffer: Some(Default::default()),
            output_device_id: Default::default()
        }));
        let self_arc = ret.clone();
        ret.lock().unwrap().self_arc = Arc::into_raw(self_arc);
        ret
    }
    
    pub fn to_wasm_audio_device_list(&mut self, tw: ToWasmAudioDeviceList) {
        self.devices.clear();
        for device in tw.devices {
            let device_id = LiveId::from_str_unchecked(&format!("{} {} {}", device.web_device_id, device.label, device.is_output)).into();
            self.devices.push(WebAudioDevice {
                web_device_id: device.web_device_id,
                desc: AudioDeviceDesc {
                    device_id,
                    device_type: if device.is_output {AudioDeviceType::Output} else {AudioDeviceType::Input},
                    is_default: false,
                    has_failed: false,
                    channel_count: 2,
                    name: device.label
                }
            })
        }
        // lets pick some defaults
        // we need to find either 'default' or the first of either input or ouptput
        if let Some(device) = self.devices.iter_mut().find( | v | v.desc.device_type.is_input() && v.web_device_id == "default") {
            device.desc.is_default = true;
        }
        else if let Some(device) = self.devices.iter_mut().find( | v | v.desc.device_type.is_input()) {
            device.desc.is_default = true;
        }
        if let Some(device) = self.devices.iter_mut().find( | v | v.desc.device_type.is_output() && v.web_device_id == "default") {
            device.desc.is_default = true;
        }
        else if let Some(device) = self.devices.iter_mut().find( | v | v.desc.device_type.is_output()) {
            device.desc.is_default = true;
        }
        self.change_signal.set();
    }
    
    pub fn use_audio_inputs(&mut self, _os: &mut CxOs, _devices: &[AudioDeviceId]) {
        // TODO
        crate::log!("Web audio input todo!");
    }
    
    pub fn use_audio_outputs(&mut self, os: &mut CxOs, devices: &[AudioDeviceId]) {
        // alright we're going to use audio outputs.
        // we can however only use one so we'll use the first one
        // and then we'll send over the device we want to the other side
        if devices.len() == 0 {
            os.from_wasm(FromWasmStopAudioOutput {});
            return
        }
        if devices.len()>1 {
            crate::log!("Web only supports a single audio device");
        }
        let web_device_id = if let Some(device) = self.devices.iter().find( | v | v.desc.device_id == devices[0]) {
            device.web_device_id.clone()
        }
        else {
            "".to_string()
        };
        os.from_wasm(FromWasmStartAudioOutput {
            web_device_id,
            context_ptr: self.self_arc as u32
        });
    }
    
    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceDesc> {
        let mut desc = Vec::new();
        for device in &self.devices {
            desc.push(device.desc.clone())
        }
        desc
    }
}

#[export_name = "wasm_audio_output_entrypoint"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_audio_output_entrypoint(context_ptr: u32, frames: u32, channels: u32) -> u32 {
    let wa = context_ptr as *const Mutex<WebAudioAccess>;
    let (output_fn, mut output_buffer, device_id) = {
        let mut wa = (*wa).lock().unwrap();
        (wa.audio_output_cb[0].clone(), wa.output_buffer.take().unwrap(), wa.output_device_id)
    };
    
    output_buffer.clear_final_size();
    output_buffer.resize(frames as usize, channels as usize);
    output_buffer.set_final_size();
    let mut output_fn = output_fn.lock().unwrap();
    
    if let Some(output_fn) = &mut *output_fn {
        output_fn(AudioInfo {device_id, time: None}, &mut output_buffer);
    }
    let ptr = output_buffer.data.as_ptr();
    
    (*wa).lock().unwrap().output_buffer = Some(output_buffer);
    
    ptr as u32
}

