#![allow(dead_code)]
use {
    std::sync::atomic::{AtomicU32, Ordering},
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
        makepad_live_id::*,
        thread::Signal,
        audio::*,
    }
};

struct PulseAudioDesc {
    name: String,
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

struct PulseOutputStream {
    device_id: AudioDeviceId,
    stream: *mut pa_stream,
}

struct PulseOutputStruct {
    device_id: AudioDeviceId,
    output_fn: Arc<Mutex<Option<AudioOutputFn> > >,
    write_byte_count: usize,
    clear_on_read: bool,
    ready_state: AtomicU32,
    audio_buffer: AudioBuffer,
}

impl PulseOutputStream {
    unsafe fn new(device_id: AudioDeviceId, name: &str, index: usize, pulse: &PulseAudioAccess) -> PulseOutputStream {
        
        pa_threaded_mainloop_lock(pulse.main_loop);
        let sample_spec = pa_sample_spec {
            format: PA_SAMPLE_FLOAT32LE,
            rate: 48000,
            channels: 2
        };

        let stream = pa_stream_new(pulse.context, "makepad output stream\0".as_ptr(), &sample_spec, std::ptr::null());
        if stream == std::ptr::null_mut() {
            panic!("pa_stream_new failed"); 
        }
        
        let output_fn_raw = Box::into_raw(Box::new(PulseOutputStruct {
            device_id,
            clear_on_read: true,
            output_fn: pulse.audio_output_cb[index].clone(),
            write_byte_count: 0,
            ready_state: AtomicU32::new(0), 
            audio_buffer: AudioBuffer::default()
        }));
        pa_stream_set_state_callback(stream, Some(Self::playback_stream_state_callback), output_fn_raw as *mut _);

        let buffer_attr = pa_buffer_attr {
            maxlength: (8 * pulse.buffer_frames) as u32,
            tlength: (8 * pulse.buffer_frames) as u32,
            prebuf: 0,
            minreq:std::u32::MAX,
            fragsize: std::u32::MAX,
        }; 
        let flags = PA_STREAM_ADJUST_LATENCY|PA_STREAM_START_CORKED|PA_STREAM_START_UNMUTED;

        pa_stream_connect_playback(
            stream,
            format!("{}\0", name).as_ptr(),
            &buffer_attr,
            flags,
            std::ptr::null(),
            std::ptr::null_mut()
        );
        
        pa_threaded_mainloop_unlock(pulse.main_loop);
        
        loop {
            let ready_state = (*output_fn_raw).ready_state.load(Ordering::Relaxed);
            if ready_state == 1 {
                break; 
            }
            if ready_state == 2 {
                panic!("STREAM CANNOT BE STARTED");
            }
            pa_threaded_mainloop_wait(pulse.main_loop);
        }
        
        (*output_fn_raw).write_byte_count = pa_stream_writable_size(stream);
        
        pa_stream_set_write_callback(stream, Some(Self::playback_stream_write_callback), output_fn_raw as *mut _);
        
        let op = pa_stream_cork(stream, 0, None, std::ptr::null_mut());
        if op == std::ptr::null_mut() {
            panic!("pa_stream_cork failed"); 
        }
        pa_operation_unref(op);
        
        Self {
            device_id,
            stream
        }
    }
    
    pub unsafe fn terminate(self, pulse: &PulseAudioAccess) {
        pa_threaded_mainloop_lock(pulse.main_loop);
        
        pa_stream_set_write_callback(self.stream, None, std::ptr::null_mut());
        pa_stream_set_state_callback(self.stream, None, std::ptr::null_mut());
        pa_stream_disconnect(self.stream);
        pa_stream_unref(self.stream);
        pa_threaded_mainloop_unlock(pulse.main_loop);
    }
    
    unsafe extern "C" fn playback_stream_write_callback (
        stream: *mut pa_stream,
        _nbytes: usize,
        output: *mut c_void
    ) {
        let output = &mut*(output as *mut PulseOutputStruct);
        let mut write_ptr = std::ptr::null_mut();
        let mut write_byte_count = output.write_byte_count;
        if pa_stream_begin_write(stream, &mut write_ptr, &mut write_byte_count) != 0 {
            panic!("pa_stream_begin_write");
        }
        if write_byte_count == output.write_byte_count{
            let mut output_fn = (*output).output_fn.lock().unwrap();
            output.audio_buffer.resize(output.write_byte_count / 8, 2);
            if let Some(output_fn) = &mut *output_fn {
                output_fn(AudioInfo{
                    device_id: output.device_id,
                    time: None
                }, &mut output.audio_buffer);
                // lets copy it to interleaved format
                let interleaved = std::slice::from_raw_parts_mut(write_ptr as *mut f32, output.write_byte_count / 4);
                let data = &output.audio_buffer.data;
                let frame_count = output.audio_buffer.frame_count();

                for i in 0..frame_count{
                    interleaved[i * 2] = data[i];
                    interleaved[i * 2 + 1] = data[i + frame_count];
                } 
            }
        }
        else{
            println!("Pulse audio buffer size unexpected");
        }
        let flags = if output.clear_on_read{
            output.clear_on_read = false;
            PA_SEEK_RELATIVE_ON_READ
        } 
        else{
            PA_SEEK_RELATIVE
        };
        
        if pa_stream_write(stream, write_ptr, write_byte_count, None, 0, flags) != 0 {
            panic!("pa_stream_write");
        }
    }
    
    unsafe extern "C" fn playback_stream_state_callback (
        stream: *mut pa_stream,
        output: *mut c_void
    ) {
        let output = output as *mut PulseOutputStruct;
        let state = pa_stream_get_state(stream);
        match state {
            PA_STREAM_UNCONNECTED => (),
            PA_STREAM_CREATING => (),
            PA_STREAM_READY => {
                (*output).ready_state.store(1, Ordering::Relaxed)
            },
            PA_STREAM_FAILED => {
                (*output).ready_state.store(2, Ordering::Relaxed)
            },
            PA_STREAM_TERMINATED => {
                let _ = Box::from_raw(output);
            },
            _ => panic!()
        }
    }
}

pub struct PulseAudioAccess {
    pub audio_input_cb: [Arc<Mutex<Option<AudioInputFn> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<AudioOutputFn> > >; MAX_AUDIO_DEVICE_INDEX],
    
    buffer_frames: usize,
    
    audio_outputs: Vec<PulseOutputStream>,
    //audio_inputs: Arc<Mutex<Vec<AlsaAudioDeviceRef >> >,
    device_query: Option<PulseDeviceQuery>,
    
    device_descs: Vec<PulseAudioDesc>,
    change_signal: Signal,
    context_state: ContextState,
    main_loop: *mut pa_threaded_mainloop,
    main_loop_api: *mut pa_mainloop_api,
    context: *mut pa_context,
    self_ptr: *const Mutex<PulseAudioAccess>,
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
    source_list: Vec<PulseDeviceDesc>,
    default_sink: Option<String>,
    default_source: Option<String>,
}

impl PulseAudioAccess {
    pub fn new(change_signal: Signal, alsa_audio: &AlsaAudioAccess) -> Arc<Mutex<Self >> {
        unsafe {
            let main_loop = pa_threaded_mainloop_new();
            let main_loop_api = pa_threaded_mainloop_get_api(main_loop);
            let prop_list = pa_proplist_new();
            let context = pa_context_new_with_proplist(main_loop_api, "makepad\0".as_ptr(), prop_list);
            
            let pulse = Arc::new(Mutex::new(
                PulseAudioAccess {
                    buffer_frames: 256,
                    audio_outputs: Vec::new(),
                    change_signal,
                    device_query: None,
                    context: context.clone(),
                    main_loop,
                    main_loop_api,
                    context_state: ContextState::Connecting,
                    audio_input_cb: alsa_audio.audio_input_cb.clone(),
                    audio_output_cb: alsa_audio.audio_output_cb.clone(),
                    device_descs: Default::default(),
                    self_ptr: std::ptr::null()
                }
            ));
            let self_ptr = Arc::into_raw(pulse.clone());
            (*self_ptr).lock().unwrap().self_ptr = self_ptr;
            
            pa_context_set_state_callback(context, Some(Self::context_state_callback), self_ptr as *mut _);
            if pa_context_connect(context, std::ptr::null(), 0, std::ptr::null()) != 0 {
                panic!("Pulse audio pa_context_connect failed");
            };
            if pa_threaded_mainloop_start(main_loop) != 0 {
                panic!("Pulse audio pa_threaded_mainloop_start failed");
            }
            
            pa_threaded_mainloop_lock(main_loop);
            loop {
                if let ContextState::Connecting = pulse.lock().unwrap().context_state {}
                else {
                    break;
                }
                pa_threaded_mainloop_wait(main_loop);
            }
            
            pa_threaded_mainloop_unlock(main_loop);
            pulse
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
        pulse: *mut c_void
    ) {
        let pulse: &Mutex<PulseAudioAccess> = &*(pulse as *const _);
        let pulse = pulse.lock().unwrap();
        pulse.change_signal.set();
        pa_threaded_mainloop_signal(pulse.main_loop, 0);
    }
    
    unsafe extern "C" fn context_state_callback (
        c: *mut pa_context,
        pulse: *mut c_void
    ) {
        let pulse: &Mutex<PulseAudioAccess> = &*(pulse as *mut _);
        let state = pa_context_get_state(c);
        
        match state {
            PA_CONTEXT_READY => {
                pulse.lock().unwrap().context_state = ContextState::Ready;
                let main_loop = pulse.lock().unwrap().main_loop;
                pa_threaded_mainloop_signal(main_loop, 0);
            }
            PA_CONTEXT_FAILED | PA_CONTEXT_TERMINATED => {
                pulse.lock().unwrap().context_state = ContextState::Failed;
                let main_loop = pulse.lock().unwrap().main_loop;
                pa_threaded_mainloop_signal(main_loop, 0);
            },
            _ => (),
        }
    }
    
    unsafe extern "C" fn sink_info_callback(
        _ctx: *mut pa_context,
        info: *const pa_sink_info,
        eol: c_int,
        query: *mut c_void,
    ) {
        let query: &mut PulseDeviceQuery = &mut *(query as *mut _);
        if eol>0 {
            pa_threaded_mainloop_signal(query.main_loop.unwrap(), 0);
            return
        }
        query.sink_list.push(PulseDeviceDesc {
            name: CStr::from_ptr((*info).name).to_str().unwrap().to_string(),
            description: CStr::from_ptr((*info).description).to_str().unwrap().to_string(),
            _index: (*info).index
        })
    }
    
    unsafe extern "C" fn source_info_callback(
        _ctx: *mut pa_context,
        info: *const pa_source_info,
        eol: c_int,
        query: *mut c_void,
    ) {
        let query: &mut PulseDeviceQuery = &mut *(query as *mut _);
        if eol>0 {
            pa_threaded_mainloop_signal(query.main_loop.unwrap(), 0);
            return
        }
        query.source_list.push(PulseDeviceDesc {
            name: CStr::from_ptr((*info).name).to_str().unwrap().to_string(),
            description: CStr::from_ptr((*info).description).to_str().unwrap().to_string(),
            _index: (*info).index
        })
    }
    
    unsafe extern "C" fn server_info_callback(
        _ctx: *mut pa_context,
        info: *const pa_server_info,
        query: *mut c_void,
    ) {
        let query: &mut PulseDeviceQuery = &mut *(query as *mut _);
        query.default_sink = Some(CStr::from_ptr((*info).default_sink_name).to_str().unwrap().to_string());
        query.default_source = Some(CStr::from_ptr((*info).default_source_name).to_str().unwrap().to_string());
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
            // lets add some input/output devices
            let mut out = Vec::new();
            let mut device_descs = Vec::new();
            for source in query.source_list {
                out.push(AudioDeviceDesc {
                    device_id: LiveId::from_str_unchecked(&source.name).into(),
                    device_type: AudioDeviceType::Input,
                    is_default: Some(&source.name) == query.default_source.as_ref(),
                    channels: 2,
                    name: format!("[Pulse Audio] {}", source.description)
                });
                device_descs.push(PulseAudioDesc {
                    name: source.name.clone(),
                    desc: out.last().cloned().unwrap()
                });
            }
            for sink in query.sink_list {
                out.push(AudioDeviceDesc {
                    device_id: LiveId::from_str_unchecked(&sink.name).into(),
                    device_type: AudioDeviceType::Output,
                    is_default: Some(&sink.name) == query.default_sink.as_ref(),
                    channels: 2,
                    name: format!("[Pulse Audio] {}", sink.description)
                });
                device_descs.push(PulseAudioDesc {
                    name: sink.name.clone(),
                    desc: out.last().cloned().unwrap()
                });
            }
            self.device_descs = device_descs;
            out
        }
        
    }
    
    pub fn use_audio_inputs(&mut self, _devices: &[AudioDeviceId]) {
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
        let new = {
            let mut i = 0;
            while i < self.audio_outputs.len() {
                if !devices.contains(&self.audio_outputs[i].device_id) {
                    let item = self.audio_outputs.remove(i);
                    unsafe {item.terminate(self)};
                }
                else {
                    i += 1;
                }
            }
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if self.audio_outputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    if let Some(v) = self.device_descs.iter().find( | v | v.desc.device_id == *device_id) {
                        new.push((index, *device_id, &v.name))
                    }
                }
            }
            new
            
        };
        for (index, device_id, name) in new {
            let new_output = unsafe {PulseOutputStream::new(device_id, name, index, self)};
            self.audio_outputs.push(new_output);
        }
    }
}
