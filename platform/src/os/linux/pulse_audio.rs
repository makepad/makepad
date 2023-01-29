#![allow(dead_code)]
use {
    std::sync::{Arc, Mutex},
    //std::ffi::CStr,
    std::os::raw::{
        c_void,
       // c_int,
    },
    self::super::{
        alsa_audio::AlsaAudioAccess,
        pulse_sys::*,
    },
    crate::{
        thread::Signal,
        audio::*,
    }
};

struct PulseAudioDesc {
    _name: String,
    desc: AudioDeviceDesc,
}
/*
struct PulseAudioDevice {
}
*/

struct AlsaAudioDeviceRef {
    device_id: AudioDeviceId,
    _is_terminated: bool,
}

pub struct PulseAudioAccess {
    pub audio_input_cb: [Arc<Mutex<Option<Box<dyn FnMut(AudioInfo, AudioBuffer) -> AudioBuffer + Send + 'static >> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<Box<dyn FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static >> > >; MAX_AUDIO_DEVICE_INDEX],
    
    audio_outputs: Arc<Mutex<Vec<AlsaAudioDeviceRef >> >,
    audio_inputs: Arc<Mutex<Vec<AlsaAudioDeviceRef >> >,
    device_descs: Vec<PulseAudioDesc>,
    
    context_ready: bool,
    main_loop: *mut pa_threaded_mainloop,
    main_loop_api: *mut pa_mainloop_api,
}

struct PulseSinkDesc {
    name: String,
    description: String,
    _index: u32,
}

struct PulseInputDesc {
    name: String,
    description: String,
    _index: u32,
}

impl PulseAudioAccess {
    pub fn new(_change_signal: Signal, alsa_audio: &AlsaAudioAccess) -> Arc<Mutex<Self >> {
        unsafe{
            let main_loop = pa_threaded_mainloop_new();
            let main_loop_api = pa_threaded_mainloop_get_api(main_loop);
            
            let pself = Arc::new(Mutex::new(
                PulseAudioAccess {
                    main_loop,
                    main_loop_api,
                    context_ready: false,
                    audio_input_cb: alsa_audio.audio_input_cb.clone(),
                    audio_output_cb: alsa_audio.audio_output_cb.clone(),
                    device_descs: Default::default(),
                    audio_inputs: Default::default(),
                    audio_outputs: Default::default(),
                }
            ));
            
            let drop_self = pself.clone();
            let _ptr = Arc::into_raw(drop_self);
            
            pself
        }
    }
    
    unsafe extern "C" fn context_state_callback (
        c: *mut pa_context,
        pself: *mut c_void
    ) {
        let pself: &Mutex<PulseAudioAccess> = &*(pself as *mut _);
        let mut pself = pself.lock().unwrap();
        match pa_context_get_state(c) {
            PA_CONTEXT_READY => {
                pself.context_ready = true;
                pa_threaded_mainloop_signal(pself.main_loop, 0);
            }
            PA_CONTEXT_FAILED | PA_CONTEXT_TERMINATED => {
                //*state = State::Terminated
            }
            PA_CONTEXT_UNCONNECTED => (),
            PA_CONTEXT_CONNECTING => (),
            PA_CONTEXT_AUTHORIZING => (),
            PA_CONTEXT_SETTING_NAME => (),
            _ => (),
        }
    }
    
    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceDesc> {
        // ok lets enumerate pulse audio
        /*unsafe {
            let main_loop: *mut pa_mainloop = pa_mainloop_new();
            let api = pa_mainloop_get_api(main_loop);
            let ctx = pa_context_new(api, "makepad\0".as_ptr());
            pa_context_connect(ctx, std::ptr::null(), 0, std::ptr::null());
            
            enum State {
                Connecting,
                Terminated,
                Ready,
                WaitForSinks,
                WaitForInputs,
            }
            let mut state = State::Connecting;
            let mut sink_list: Vec<PulseSinkDesc> = Vec::new();
            let mut input_list: Vec<PulseInputDesc> = Vec::new();
            
            unsafe extern "C" fn sink_info_cb(
                _ctx: *mut pa_context,
                info: *const pa_sink_info,
                eol: c_int,
                userdata: *mut c_void,
            ) {
                if eol>0 {
                    return
                }
                let sink_list = &mut *(userdata as *mut Vec<PulseSinkDesc>);
                sink_list.push(PulseSinkDesc {
                    name: CStr::from_ptr((*info).name).to_str().unwrap().to_string(),
                    description: CStr::from_ptr((*info).description).to_str().unwrap().to_string(),
                    _index: (*info).index
                })
            }
            
            unsafe extern "C" fn input_info_cb(
                _ctx: *mut pa_context,
                info: *const pa_source_info,
                eol: c_int,
                userdata: *mut c_void,
            ) {
                if eol>0 {
                    return
                }
                let input_list = &mut *(userdata as *mut Vec<PulseInputDesc>);
                input_list.push(PulseInputDesc {
                    name: CStr::from_ptr((*info).name).to_str().unwrap().to_string(),
                    description: CStr::from_ptr((*info).description).to_str().unwrap().to_string(),
                    _index: (*info).index
                })
            }
            
            pa_context_set_state_callback(ctx, Some(state_cb), &mut state as *mut _ as *mut _);
            let mut operation = 0 as *mut _;
            loop {
                match &state {
                    State::Connecting => {
                        pa_mainloop_iterate(main_loop, 1, std::ptr::null_mut());
                        continue;
                    },
                    State::Terminated => {
                        println!("PULSE CONNECTION FAILED ");
                        pa_context_disconnect(ctx);
                        pa_context_unref(ctx);
                        pa_mainloop_free(main_loop);
                        return Default::default();
                    }
                    State::Ready => {
                        operation = pa_context_get_sink_info_list(ctx, Some(sink_info_cb), &mut sink_list as *mut _ as *mut _);
                        state = State::WaitForSinks
                    },
                    State::WaitForSinks => {
                        if pa_operation_get_state(operation) == PA_OPERATION_DONE {
                            pa_operation_unref(operation);
                            operation = pa_context_get_source_info_list(ctx, Some(input_info_cb), &mut input_list as *mut _ as *mut _);
                            state = State::WaitForInputs;
                        }
                    }
                    State::WaitForInputs => {
                        if pa_operation_get_state(operation) == PA_OPERATION_DONE {
                            pa_operation_unref(operation);
                        }
                        break;
                    }
                }
                pa_mainloop_iterate(main_loop, 1, std::ptr::null_mut());
            }
            
            for sink in &sink_list {
                println!("GOT SINK {} {}", sink.name, sink.description);
            }
            for input in &input_list {
                println!("GOT INPUT {} {}", input.name, input.description);
            }
        }
        */
        let mut out = Vec::new();
        for dev in &self.device_descs {
            out.push(dev.desc.clone());
        }
        out
    }
    
    
    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_inputs = self.audio_inputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_inputs.iter_mut().for_each( | v | {
                if !devices.contains(&v.device_id) {
                    //v.is_terminated = true;
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
        for (_index, _device_id) in new {
            
        }
    }
    
    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_outputs = self.audio_outputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_outputs.iter_mut().for_each( | v | {
                if !devices.contains(&v.device_id) {
                    // v.is_terminated = true;
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
        for (index, _device_id) in new {
            let _audio_output_cb = self.audio_output_cb[index].clone();
            let _audio_outputs = self.audio_outputs.clone();
        }
    }
}
