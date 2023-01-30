#![allow(dead_code)]
use {
    std::sync::{Arc, Mutex},
    std::ffi::CStr,
    std::os::raw::{
        c_void,
        c_int,
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

enum ContextState {
    Connecting,
    Ready,
    Failed
}

pub struct PulseAudioAccess {
    pub audio_input_cb: [Arc<Mutex<Option<Box<dyn FnMut(AudioInfo, AudioBuffer) -> AudioBuffer + Send + 'static >> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<Box<dyn FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static >> > >; MAX_AUDIO_DEVICE_INDEX],
    
    //audio_outputs: Arc<Mutex<Vec<AlsaAudioDeviceRef >> >,
    //audio_inputs: Arc<Mutex<Vec<AlsaAudioDeviceRef >> >,
    device_query: Option<PulseDeviceQuery>,
    
    device_descs: Vec<PulseAudioDesc>,
    change_signal: Signal,
    context_state: ContextState,
    main_loop: *mut pa_threaded_mainloop,
    main_loop_api: *mut pa_mainloop_api,
    context: *mut pa_context,
}

struct PulseDeviceDesc {
    name: String,
    description: String,
    _index: u32,
}

#[derive(Default)]
struct PulseDeviceQuery {
    main_loop: Option<*mut pa_threaded_mainloop>,
    sink_list: Vec<PulseDeviceDesc>,
    input_list: Vec<PulseDeviceDesc>,
    default_sink: Option<String>,
    default_input: Option<String>,
}

impl PulseAudioAccess {
    pub fn new(change_signal: Signal, alsa_audio: &AlsaAudioAccess) -> Arc<Mutex<Self >> {
        unsafe {
            let main_loop = pa_threaded_mainloop_new();
            let main_loop_api = pa_threaded_mainloop_get_api(main_loop);
            let prop_list = pa_proplist_new();
            let context = pa_context_new_with_proplist(main_loop_api, "makepad\0".as_ptr(), prop_list);
            
            let pself = Arc::new(Mutex::new(
                PulseAudioAccess {
                    change_signal,
                    device_query: None,
                    context: context.clone(),
                    main_loop,
                    main_loop_api,
                    context_state: ContextState::Connecting,
                    audio_input_cb: alsa_audio.audio_input_cb.clone(),
                    audio_output_cb: alsa_audio.audio_output_cb.clone(),
                    device_descs: Default::default(),
                    
                }
            ));
            let self_ptr = Arc::into_raw(pself.clone());
            
            pa_context_set_state_callback(context, Some(Self::context_state_callback), self_ptr as *mut _);
            
            if pa_context_connect(context, std::ptr::null(), 0, std::ptr::null()) != 0 {
                panic!("Pulse audio pa_context_connect failed");
            };
            
            if pa_threaded_mainloop_start(main_loop) != 0 {
                panic!("Pulse audio pa_threaded_mainloop_start failed");
            }
            
            pa_threaded_mainloop_lock(main_loop);
             
            loop  {
                if let ContextState::Connecting = pself.lock().unwrap().context_state{}
                else{
                    break;
                }
                pa_threaded_mainloop_wait(main_loop);
            }
             
            pa_threaded_mainloop_unlock(main_loop);
            println!("DONE"); 
            pself
        }
    }
    /*
    pub fn destroy(&self){
        pa_threaded_mainloop_stop(self.main_loop);
        pa_context_unref(self.context);
        pa_context_disconnect(self.context);
        pa_threaded_mainloop_free(self.main_loop);
    }*/
    
    unsafe extern "C" fn subscribe_callback (
        _c: *mut pa_context,
        _event_bits: pa_subscription_event_type_t,
        _index: u32,
        pself: *mut c_void
    ) {
        let pself: &Mutex<PulseAudioAccess> = &*(pself as *const _);
        let pself = pself.lock().unwrap();
        pself.change_signal.set();
        pa_threaded_mainloop_signal(pself.main_loop, 0);
    }
    
    unsafe extern "C" fn context_state_callback (
        c: *mut pa_context,
        pself: *mut c_void
    ) {
        let pself: &Mutex<PulseAudioAccess> = &*(pself as *mut _);
        let state =  pa_context_get_state(c);
        
        match state{
            PA_CONTEXT_READY => {
                pself.lock().unwrap().context_state = ContextState::Ready;
                let main_loop = pself.lock().unwrap().main_loop;
                pa_threaded_mainloop_signal(main_loop, 0);
            }
            PA_CONTEXT_FAILED | PA_CONTEXT_TERMINATED => {
                pself.lock().unwrap().context_state = ContextState::Failed;
                let main_loop = pself.lock().unwrap().main_loop;
                pa_threaded_mainloop_signal(main_loop, 0);
            },
            _ => (),
        }
    }
    
    unsafe extern "C" fn sink_info_callback(
        _ctx: *mut pa_context,
        info: *const pa_sink_info,
        eol: c_int,
        pself: *mut c_void,
    ) {
        let pself: &mut PulseDeviceQuery = &mut *(pself as *mut _);
        if eol>0 {
            pa_threaded_mainloop_signal(pself.main_loop.unwrap(), 0);
            return
        }
        pself.sink_list.push(PulseDeviceDesc {
            name: CStr::from_ptr((*info).name).to_str().unwrap().to_string(),
            description: CStr::from_ptr((*info).description).to_str().unwrap().to_string(),
            _index: (*info).index
        })
    }
    
    unsafe extern "C" fn source_info_callback(
        _ctx: *mut pa_context,
        info: *const pa_source_info,
        eol: c_int,
        pself: *mut c_void,
    ) {
        let pself: &mut PulseDeviceQuery = &mut *(pself as *mut _);
        if eol>0 {
            pa_threaded_mainloop_signal(pself.main_loop.unwrap(), 0);
            return
        }
        pself.input_list.push(PulseDeviceDesc {
            name: CStr::from_ptr((*info).name).to_str().unwrap().to_string(),
            description: CStr::from_ptr((*info).description).to_str().unwrap().to_string(),
            _index: (*info).index
        })
    }
    
    unsafe extern "C" fn server_info_callback(
        _ctx: *mut pa_context,
        _info: *const pa_server_info,
        _pself: *mut c_void,
    ) {
        //let pself: &mut PulseDeviceQuery = &mut *(pself as *mut _);
        // this should give us the defaults
    }
    
    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceDesc> {
        // ok lets enumerate pulse audio
        unsafe {
            let mut query = PulseDeviceQuery::default();
            query.main_loop = Some(self.main_loop);
            let sink_op = pa_context_get_sink_info_list(self.context, Some(Self::sink_info_callback), &mut query as *mut _ as *mut _);
            let source_op = pa_context_get_source_info_list(self.context, Some(Self::source_info_callback), &mut query as *mut _ as *mut _);
            let server_op = pa_context_get_server_info(self.context, Some(Self::server_info_callback), &mut query as *mut _ as *mut _);
            while pa_operation_get_state(sink_op) == PA_OPERATION_RUNNING ||
            pa_operation_get_state(source_op) == PA_OPERATION_RUNNING ||
            pa_operation_get_state(server_op) == PA_OPERATION_RUNNING {
                pa_threaded_mainloop_wait(self.main_loop);
            }
            pa_operation_unref(sink_op);
            pa_operation_unref(source_op);
            pa_operation_unref(server_op);
            // we should have the devices + default device now
            println!("GOT HERE");
        }
        
        let mut out = Vec::new();
        for dev in &self.device_descs {
            out.push(dev.desc.clone());
        }
        out
    }
    
    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        /*let new = {
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
            
        }*/
    }
    
    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        /*let new = {
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
        }*/
    }
}
