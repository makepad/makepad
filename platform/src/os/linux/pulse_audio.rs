#![allow(dead_code)]
use {
    std::collections::HashSet,
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

struct PulseInputStream {
    device_id: AudioDeviceId,
    stream: *mut pa_stream,
}

struct PulseInputStruct {
    device_id: AudioDeviceId,
    input_fn: Arc<Mutex<Option<AudioInputFn> > >,
    audio_buffer: AudioBuffer,
    ready_state: AtomicU32,
}

impl PulseInputStream {
    unsafe fn new(device_id: AudioDeviceId, name: &str, index: usize, pulse: &PulseAudioAccess) -> PulseInputStream {
        pa_threaded_mainloop_lock(pulse.main_loop);
        let sample_spec = pa_sample_spec {
            format: PA_SAMPLE_FLOAT32LE,
            rate: 48000,
            channels: 2
        };
        
        let stream = pa_stream_new(pulse.context, "makepad input stream\0".as_ptr(), &sample_spec, std::ptr::null());
        if stream == std::ptr::null_mut() {
            panic!("pa_stream_new failed");
        }
        let input_ptr = Box::into_raw(Box::new(PulseInputStruct {
            device_id,
            ready_state: AtomicU32::new(0),
            input_fn: pulse.audio_input_cb[index].clone(),
            audio_buffer: AudioBuffer::default()
        }));
        pa_stream_set_state_callback(stream, Some(Self::recording_stream_state_callback), input_ptr as *mut _);
        pa_stream_set_read_callback(stream, Some(Self::recording_stream_read_callback), input_ptr as *mut _);
        
        let buffer_attr = pa_buffer_attr {
            maxlength: std::u32::MAX,
            tlength: (8 * pulse.buffer_frames) as u32,
            prebuf: 0,
            minreq: std::u32::MAX,
            fragsize: std::u32::MAX,
        };
        let flags = PA_STREAM_ADJUST_LATENCY;
        
        pa_stream_connect_record(
            stream,
            format!("{}\0", name).as_ptr(),
            &buffer_attr,
            flags,
        );
        
        pa_threaded_mainloop_unlock(pulse.main_loop);
        
        loop {
            let ready_state = (*input_ptr).ready_state.load(Ordering::Relaxed);
            if ready_state == 1 {
                break;
            }
            if ready_state == 2 {
                panic!("STREAM CANNOT BE STARTED");
            }
            pa_threaded_mainloop_wait(pulse.main_loop);
        }
        
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
    
    unsafe extern "C" fn recording_stream_read_callback (
        stream: *mut pa_stream,
        _nbytes: usize,
        input_ptr: *mut c_void
    ) {
        let input = &mut*(input_ptr as *mut PulseInputStruct);
        let mut read_ptr: *mut f32 = std::ptr::null_mut();
        let mut byte_count = 0;
        if pa_stream_peek(stream, &mut read_ptr as *mut _ as *mut _, &mut byte_count) != 0{
            println!("pa_stream_peek failed");
            return
        }
        if byte_count == 0{
            return
        }
        let mut input_fn = (*input).input_fn.lock().unwrap();
        if let Some(input_fn) = &mut *input_fn {
            let interleaved = std::slice::from_raw_parts(read_ptr, byte_count / 4);
            input.audio_buffer.copy_from_interleaved(2, interleaved);
            input_fn(AudioInfo {
                device_id: input.device_id,
                time: None
            }, &input.audio_buffer);
        }        
        pa_stream_drop(stream);
    }
    
    unsafe extern "C" fn recording_stream_state_callback (
        stream: *mut pa_stream,
        output_ptr: *mut c_void
    ) {
        let input_ptr = output_ptr as *mut PulseOutputStruct;
        let state = pa_stream_get_state(stream);
        match state {
            PA_STREAM_UNCONNECTED => (),
            PA_STREAM_CREATING => (),
            PA_STREAM_READY => {
                (*input_ptr).ready_state.store(1, Ordering::Relaxed)
            },
            PA_STREAM_FAILED => {
                (*input_ptr).ready_state.store(2, Ordering::Relaxed)
            },
            PA_STREAM_TERMINATED => {
                let _ = Box::from_raw(output_ptr);
            },
            _ => panic!()
        }
    }
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
    unsafe fn new(device_id: AudioDeviceId, name: &str, index: usize, pulse: &PulseAudioAccess) -> Option<PulseOutputStream> {
        
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
        
        let output_ptr = Box::into_raw(Box::new(PulseOutputStruct {
            device_id,
            clear_on_read: true,
            output_fn: pulse.audio_output_cb[index].clone(),
            write_byte_count: 0,
            ready_state: AtomicU32::new(0),
            audio_buffer: AudioBuffer::default()
        }));
        pa_stream_set_state_callback(stream, Some(Self::playback_stream_state_callback), output_ptr as *mut _);
        
        let buffer_attr = pa_buffer_attr {
            maxlength: std::u32::MAX,
            tlength: (8 * pulse.buffer_frames) as u32,
            prebuf: 0,
            minreq: std::u32::MAX,
            fragsize: std::u32::MAX,
        };
        let flags = PA_STREAM_ADJUST_LATENCY | PA_STREAM_START_CORKED | PA_STREAM_START_UNMUTED;
        
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
            let ready_state = (*output_ptr).ready_state.load(Ordering::Relaxed);
            if ready_state == 1 {
                break;
            }
            if ready_state == 2 {
                // ok here we return None
                Self::terminate_stream(stream, pulse);
                return None
            }
            pa_threaded_mainloop_wait(pulse.main_loop);
        }
        
        (*output_ptr).write_byte_count = pa_stream_writable_size(stream);
        
        pa_stream_set_write_callback(stream, Some(Self::playback_stream_write_callback), output_ptr as *mut _);
        
        let op = pa_stream_cork(stream, 0, None, std::ptr::null_mut());
        if op == std::ptr::null_mut() {
            panic!("pa_stream_cork failed");
        }
        pa_operation_unref(op);
        
        Some(Self {
            device_id,
            stream
        })
    }

    pub fn terminate(&self, pulse: &PulseAudioAccess) {
        unsafe{Self::terminate_stream(self.stream, pulse)};
    }
    
    pub unsafe fn terminate_stream(stream:*mut pa_stream, pulse: &PulseAudioAccess) {
        pa_threaded_mainloop_lock(pulse.main_loop);
        pa_stream_set_write_callback(stream, None, std::ptr::null_mut());
        pa_stream_set_state_callback(stream, None, std::ptr::null_mut());
        pa_stream_disconnect(stream);
        pa_stream_unref(stream);
        pa_threaded_mainloop_unlock(pulse.main_loop);
    }
    
    unsafe extern "C" fn playback_stream_write_callback (
        stream: *mut pa_stream,
        _nbytes: usize,
        output_ptr: *mut c_void
    ) {
        let output = &mut*(output_ptr as *mut PulseOutputStruct);
        let mut write_ptr = std::ptr::null_mut();
        let mut write_byte_count = output.write_byte_count;
        if pa_stream_begin_write(stream, &mut write_ptr, &mut write_byte_count) != 0 {
            panic!("pa_stream_begin_write");
        }
        if write_byte_count == output.write_byte_count {
            let mut output_fn = (*output).output_fn.lock().unwrap();
            output.audio_buffer.resize(output.write_byte_count / 8, 2);
            if let Some(output_fn) = &mut *output_fn {
                output_fn(AudioInfo {
                    device_id: output.device_id,
                    time: None
                }, &mut output.audio_buffer);
                // lets copy it to interleaved format
                let interleaved = std::slice::from_raw_parts_mut(write_ptr as *mut f32, output.write_byte_count / 4);
                output.audio_buffer.copy_to_interleaved(interleaved);
            }
        }
        else {
            println!("Pulse audio buffer size unexpected");
        }
        let flags = if output.clear_on_read {
            output.clear_on_read = false;
            PA_SEEK_RELATIVE_ON_READ
        }
        else {
            PA_SEEK_RELATIVE
        };
        
        if pa_stream_write(stream, write_ptr, write_byte_count, None, 0, flags) != 0 {
            panic!("pa_stream_write");
        }
    }
    
    unsafe extern "C" fn playback_stream_state_callback (
        stream: *mut pa_stream,
        output_ptr: *mut c_void
    ) {
        let output_ptr = output_ptr as *mut PulseOutputStruct;
        let state = pa_stream_get_state(stream);
        match state {
            PA_STREAM_UNCONNECTED => (),
            PA_STREAM_CREATING => (),
            PA_STREAM_READY => {
                (*output_ptr).ready_state.store(1, Ordering::Relaxed)
            },
            PA_STREAM_FAILED => {
                (*output_ptr).ready_state.store(2, Ordering::Relaxed)
            },
            PA_STREAM_TERMINATED => {
                let _ = Box::from_raw(output_ptr);
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
    audio_inputs: Vec<PulseInputStream>,
    device_query: Option<PulseDeviceQuery>,
    
    device_descs: Vec<PulseAudioDesc>,
    change_signal: Signal,
    context_state: ContextState,
    main_loop: *mut pa_threaded_mainloop,
    main_loop_api: *mut pa_mainloop_api,
    context: *mut pa_context,
    self_ptr: *const Mutex<PulseAudioAccess>,
    
    failed_devices: HashSet<AudioDeviceId>,
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
                    audio_inputs: Vec::new(),
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
                    self_ptr: std::ptr::null(),
                    failed_devices: Default::default(),
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
        pulse_ptr: *mut c_void
    ) {
        let pulse: &Mutex<PulseAudioAccess> = &*(pulse_ptr as *const _);
        let pulse = pulse.lock().unwrap();
        pulse.change_signal.set();
        pa_threaded_mainloop_signal(pulse.main_loop, 0);
    }
    
    unsafe extern "C" fn context_state_callback (
        c: *mut pa_context,
        pulse_ptr: *mut c_void
    ) {
        let pulse: &Mutex<PulseAudioAccess> = &*(pulse_ptr as *mut _);
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
        query_ptr: *mut c_void,
    ) {
        let query: &mut PulseDeviceQuery = &mut *(query_ptr as *mut _);
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
        query_ptr: *mut c_void,
    ) {
        let query: &mut PulseDeviceQuery = &mut *(query_ptr as *mut _);
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
        query_ptr: *mut c_void,
    ) {
        let query: &mut PulseDeviceQuery = &mut *(query_ptr as *mut _);
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
                let device_id = LiveId::from_str(&source.name).into();
                out.push(AudioDeviceDesc {
                    has_failed: self.failed_devices.contains(&device_id),
                    device_id,
                    device_type: AudioDeviceType::Input,
                    is_default: Some(&source.name) == query.default_source.as_ref(),
                    channel_count: 2,
                    name: format!("[Pulse Audio] {}", source.description)
                });
                device_descs.push(PulseAudioDesc {
                    name: source.name.clone(),
                    desc: out.last().cloned().unwrap()
                });
            }
            for sink in query.sink_list {
                let device_id = LiveId::from_str(&sink.name).into();
                out.push(AudioDeviceDesc {
                    has_failed: self.failed_devices.contains(&device_id),
                    device_id,
                    device_type: AudioDeviceType::Output,
                    is_default: Some(&sink.name) == query.default_sink.as_ref(),
                    channel_count: 2,
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
    
    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut i = 0;
            while i < self.audio_inputs.len() {
                if !devices.contains(&self.audio_inputs[i].device_id) {
                    let item = self.audio_inputs.remove(i);
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
            let new_input = unsafe {PulseInputStream::new(device_id, name, index, self)};
            self.audio_inputs.push(new_input);
        }
    }
    
    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut i = 0;
            while i < self.audio_outputs.len() {
                if !devices.contains(&self.audio_outputs[i].device_id) {
                    let item = self.audio_outputs.remove(i);
                    item.terminate(self);
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
            if let Some(new_output) = unsafe {PulseOutputStream::new(device_id, name, index, self)}{
                self.audio_outputs.push(new_output);
            }
            else{
                self.failed_devices.insert(device_id);
                self.change_signal.set();
            }
        }
    }
}
