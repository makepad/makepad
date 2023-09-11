#![allow(dead_code)]
use {
    std::collections::HashSet,
    std::sync::{Arc, Mutex},
    std::sync::atomic::{AtomicBool, Ordering},
    std::os::raw::{c_int, c_void},
    self::super::{
        aaudio_sys::*,
       // android_jni::*,
    },
    crate::{
        makepad_error_log::*,
        makepad_live_id::*,
        thread::Signal,
        audio::*,
    }
}; 

struct AndroidAudioInput {
    device_id: AudioDeviceId,
    stream: *mut AAudioStream,
    input_stream_ptr: *mut AndroidAudioInputStream,
    is_in_error_state: Arc<AtomicBool>,
}

struct AndroidAudioOutput {
    device_id: AudioDeviceId,
    stream: *mut AAudioStream,
    output_stream_ptr: *mut AndroidAudioOutputStream,
    is_in_error_state: Arc<AtomicBool>,
}

struct AndroidAudioStreamData {
    device_id: AudioDeviceId,
    is_in_error_state: Arc<AtomicBool>,
    change_signal: Signal,
    audio_buffer: AudioBuffer,
    actual_channel_count: usize,
    channel_count: usize
}

struct AndroidAudioInputStream {
    data: AndroidAudioStreamData,
    input_fn: Arc<Mutex<Option<AudioInputFn >> >,
}

struct AndroidAudioOutputStream{
    data: AndroidAudioStreamData,
    output_fn: Arc<Mutex<Option<AudioOutputFn >> >,
}

#[derive(Clone)]
struct AndroidAudioDeviceDesc {
    aaudio_id: i32,
    desc: AudioDeviceDesc,
}

pub struct AndroidAudioAccess {
    change_signal: Signal,
    pub audio_input_cb: [Arc<Mutex<Option<AudioInputFn> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<AudioOutputFn> > >; MAX_AUDIO_DEVICE_INDEX],
    audio_inputs: Vec<AndroidAudioInput >,
    audio_outputs: Vec<AndroidAudioOutput >,
    device_descs: Vec<AndroidAudioDeviceDesc>,
    failed_devices: HashSet<AudioDeviceId>,
}
#[derive(Debug)]
pub struct AndroidAudioError(String);

impl AndroidAudioError {
    pub fn from(prefix: &str, err: c_int) -> Result<i32, Self> {
        if err < 0 {
            let err_str = match err {
                AAUDIO_ERROR_BASE => "AAUDIO_ERROR_BASE",
                AAUDIO_ERROR_DISCONNECTED => "AAUDIO_ERROR_DISCONNECTED",
                AAUDIO_ERROR_ILLEGAL_ARGUMENT => "AAUDIO_ERROR_ILLEGAL_ARGUMENT",
                AAUDIO_ERROR_INTERNAL => "AAUDIO_ERROR_INTERNAL",
                AAUDIO_ERROR_INVALID_STATE => "AAUDIO_ERROR_INVALID_STATE",
                AAUDIO_ERROR_INVALID_HANDLE => "AAUDIO_ERROR_INVALID_HANDLE",
                AAUDIO_ERROR_UNIMPLEMENTED => "AAUDIO_ERROR_UNIMPLEMENTED",
                AAUDIO_ERROR_UNAVAILABLE => "AAUDIO_ERROR_UNAVAILABLE",
                AAUDIO_ERROR_NO_FREE_HANDLES => "AAUDIO_ERROR_NO_FREE_HANDLES",
                AAUDIO_ERROR_NO_MEMORY => "AAUDIO_ERROR_NO_MEMORY",
                AAUDIO_ERROR_NULL => "AAUDIO_ERROR_NULL",
                AAUDIO_ERROR_TIMEOUT => "AAUDIO_ERROR_TIMEOUT",
                AAUDIO_ERROR_WOULD_BLOCK => "AAUDIO_ERROR_WOULD_BLOCK",
                AAUDIO_ERROR_INVALID_FORMAT => "AAUDIO_ERROR_INVALID_FORMAT",
                AAUDIO_ERROR_OUT_OF_RANGE => "AAUDIO_ERROR_OUT_OF_RANGE",
                AAUDIO_ERROR_NO_SERVICE => "AAUDIO_ERROR_NO_SERVICE => :",
                AAUDIO_ERROR_INVALID_RATE => "AAUDIO_ERROR_INVALID_RATE => :",
                _ => "Unknown error"
            };
            Err(AndroidAudioError(format!("AAudio error {} - {}", prefix, err_str)))
        }
        else {
            Ok(err)
        }
    }
}

macro_rules!aaudio_error {
    ( $ call: expr) => {
        AndroidAudioError::from(stringify!( $ call), $ call)
    }
}

impl AndroidAudioStreamData {
    fn new(desc: &AndroidAudioDeviceDesc, change_signal: Signal) -> Self {
        AndroidAudioStreamData {
            actual_channel_count: desc.desc.channel_count,
            device_id: desc.desc.device_id,
            change_signal,
            audio_buffer: Default::default(),
            channel_count: desc.desc.channel_count,
            is_in_error_state: Arc::new(AtomicBool::new(false)),
        }
    }
    
    unsafe fn set_error_state(&self) {
        self.is_in_error_state.store(true, Ordering::SeqCst);
        self.change_signal.set();
    }
    
    unsafe fn verify_channel_count(&mut self, stream: *mut AAudioStream) {
        self.actual_channel_count = AAudioStream_getChannelCount(stream) as usize;
        if self.actual_channel_count != self.channel_count {
            log!("Android audio device channel count does not match, todo add handling here");
        }
    }
    
    unsafe fn setup_builder(desc: &AndroidAudioDeviceDesc) -> Result<*mut AAudioStreamBuilder,
    AndroidAudioError> {
        let mut builder: *mut AAudioStreamBuilder = std::ptr::null_mut();
        
        aaudio_error!(AAudio_createStreamBuilder(&mut builder)) ?;
        
        if desc.aaudio_id != 0 {
            AAudioStreamBuilder_setDeviceId(builder, desc.aaudio_id);
        }
        
        AAudioStreamBuilder_setDirection(builder, AAUDIO_DIRECTION_INPUT);
        AAudioStreamBuilder_setSharingMode(builder, AAUDIO_SHARING_MODE_SHARED);
        AAudioStreamBuilder_setSampleRate(builder, 48000);
        AAudioStreamBuilder_setChannelCount(builder, desc.desc.channel_count as i32);
        AAudioStreamBuilder_setFormat(builder, AAUDIO_FORMAT_PCM_FLOAT);
        AAudioStreamBuilder_setBufferCapacityInFrames(builder, 256);
        AAudioStreamBuilder_setPerformanceMode(builder, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);
        Ok(builder)
    }
    
    unsafe fn stop_stream(stream: *mut AAudioStream) {
        let input_state = AAUDIO_STREAM_STATE_STOPPING;
        let mut next_state = AAUDIO_STREAM_STATE_UNINITIALIZED;
        AAudioStream_requestStop(stream);
        AAudioStream_waitForStateChange(stream, input_state, &mut next_state, 2_000_000);
        AAudioStream_close(stream);
    }
}

impl AndroidAudioOutput {
    unsafe fn new(desc: &AndroidAudioDeviceDesc, change_signal: Signal, output_fn: Arc<Mutex<Option<AudioOutputFn >> >) -> Result<Self,
    AndroidAudioError> {
        let builder = AndroidAudioStreamData::setup_builder(desc) ?;
        AAudioStreamBuilder_setDirection(builder, AAUDIO_DIRECTION_OUTPUT);
        let output_stream_ptr = Box::into_raw(Box::new(AndroidAudioOutputStream {
            output_fn,
            data: AndroidAudioStreamData::new(desc, change_signal)
        }));
        AAudioStreamBuilder_setDataCallback(builder, Some(Self::aaudio_stream_data_callback), output_stream_ptr as *mut c_void);
        AAudioStreamBuilder_setErrorCallback(builder, Some(Self::aaudio_stream_error_callback), output_stream_ptr as *mut c_void);
        
        let mut stream: *mut AAudioStream = std::ptr::null_mut();
        aaudio_error!(AAudioStreamBuilder_openStream(builder, &mut stream)) ?;
        (*output_stream_ptr).data.verify_channel_count(stream);
        
        AAudioStreamBuilder_delete(builder);
        AAudioStream_requestStart(stream);
        Ok(Self {
            is_in_error_state: (*output_stream_ptr).data.is_in_error_state.clone(),
            stream,
            output_stream_ptr,
            device_id: desc.desc.device_id
        })
    }
    
    unsafe fn terminate(self) {
        AndroidAudioStreamData::stop_stream(self.stream);
        let _ = Box::from_raw(self.output_stream_ptr);
    }
    
    unsafe extern "C" fn aaudio_stream_error_callback(
        _stream: *mut AAudioStream,
        user_data: *mut c_void,
        _error: aaudio_result_t,
    ) {
        let wrap = &mut *(user_data as *mut AndroidAudioOutputStream);
        wrap.data.set_error_state();
    }
    
    unsafe extern "C" fn aaudio_stream_data_callback(
        _stream: *mut AAudioStream,
        user_data: *mut c_void,
        audio_data: *mut c_void,
        frame_count: i32
    ) -> aaudio_data_callback_result_t {
        let output_stream = &mut *(user_data as *mut AndroidAudioOutputStream);
        let mut output_fn = output_stream.output_fn.lock().unwrap();
        let data = &mut output_stream.data;
        if let Some(output_fn) = &mut *output_fn {
            data.audio_buffer.resize(frame_count as usize, data.channel_count);
            output_fn(AudioInfo {
                device_id: data.device_id,
                time: None
            }, &mut data.audio_buffer);
            let output = std::slice::from_raw_parts_mut(audio_data as *mut f32, frame_count as usize * data.actual_channel_count);
            if data.channel_count != data.actual_channel_count {
                // we have a problem
            }
            else {
                data.audio_buffer.copy_to_interleaved(output);
            }
        }
        AAUDIO_CALLBACK_RESULT_CONTINUE
    }
}


impl AndroidAudioInput {
    unsafe fn new(desc: &AndroidAudioDeviceDesc, change_signal: Signal, input_fn: Arc<Mutex<Option<AudioInputFn >> >) -> Result<Self,
    AndroidAudioError> {
        let builder = AndroidAudioStreamData::setup_builder(&desc) ?;
        
        let input_stream_ptr = Box::into_raw(Box::new(AndroidAudioInputStream {
            input_fn,
            data: AndroidAudioStreamData::new(desc, change_signal)
        }));
        
        AAudioStreamBuilder_setDataCallback(builder, Some(Self::aaudio_stream_data_callback), input_stream_ptr as *mut c_void);
        AAudioStreamBuilder_setErrorCallback(builder, Some(Self::aaudio_stream_error_callback), input_stream_ptr as *mut c_void);
        
        let mut stream: *mut AAudioStream = std::ptr::null_mut();
        aaudio_error!(AAudioStreamBuilder_openStream(builder, &mut stream)) ?;
        (*input_stream_ptr).data.verify_channel_count(stream);
        AAudioStreamBuilder_delete(builder);
        AAudioStream_requestStart(stream);
        
        Ok(Self {
            is_in_error_state: (*input_stream_ptr).data.is_in_error_state.clone(),
            stream,
            input_stream_ptr,
            device_id: desc.desc.device_id
        })
    }
    
    unsafe fn terminate(self) {
        AndroidAudioStreamData::stop_stream(self.stream);
        let _ = Box::from_raw(self.input_stream_ptr);
    }
    
    unsafe extern "C" fn aaudio_stream_error_callback(
        _stream: *mut AAudioStream,
        user_data: *mut c_void,
        _error: aaudio_result_t,
    ) {
        let input_stream_ptr = &mut *(user_data as *mut AndroidAudioInputStream);
        input_stream_ptr.data.set_error_state();
    }
    
    unsafe extern "C" fn aaudio_stream_data_callback(
        _stream: *mut AAudioStream,
        user_data: *mut c_void,
        audio_data: *mut c_void,
        frame_count: i32
    ) -> aaudio_data_callback_result_t {
        let input_stream = &mut *(user_data as *mut AndroidAudioInputStream);
        let mut input_fn = input_stream.input_fn.lock().unwrap();
        let data = &mut input_stream.data;
        if let Some(input_fn) = &mut *input_fn {
            let input_data = std::slice::from_raw_parts(audio_data as *mut f32, frame_count as usize * data.actual_channel_count);
            data.audio_buffer.resize(frame_count as usize, data.channel_count);
            data.audio_buffer.copy_from_interleaved(data.channel_count, &input_data);
            input_fn(AudioInfo {
                device_id: data.device_id,
                time: None
            }, &data.audio_buffer);
        }
        AAUDIO_CALLBACK_RESULT_CONTINUE
    }
}

impl AndroidAudioAccess {
    pub fn new(change_signal: Signal) -> Arc<Mutex<Self >> {
        change_signal.set();
        // alright Soooo. lets just enumerate the damn audio devices.
        
        Arc::new(Mutex::new(
            AndroidAudioAccess {
                change_signal,
                audio_input_cb: Default::default(),
                audio_output_cb: Default::default(),
                audio_inputs: Default::default(),
                audio_outputs: Default::default(),
                device_descs: Default::default(),
                failed_devices: Default::default(),
            }
        ))
    }
    
    fn java_ret_to_device(input: &str, device_type: AudioDeviceType) -> Option<AndroidAudioDeviceDesc> {
        fn type_id_to_string(ty: u32) -> &'static str {
            match ty {
                AAUDIO_TYPE_UNKNOWN => "Unknown",
                AAUDIO_TYPE_BUILTIN_EARPIECE => "Built-in Earpiece",
                AAUDIO_TYPE_BUILTIN_SPEAKER => "Built-in Speaker",
                AAUDIO_TYPE_WIRED_HEADSET => "Wired Headset",
                AAUDIO_TYPE_WIRED_HEADPHONES => "Wired Headphones",
                AAUDIO_TYPE_LINE_ANALOG => "Line Analog",
                AAUDIO_TYPE_LINE_DIGITAL => "Line Digital",
                AAUDIO_TYPE_BLUETOOTH_SCO => "Bluetooth SCO",
                AAUDIO_TYPE_BLUETOOTH_A2DP => "Bluetooth A2DP",
                AAUDIO_TYPE_HDMI => "HDMI",
                AAUDIO_TYPE_HDMI_ARC => "HDMI ARC",
                AAUDIO_TYPE_USB_DEVICE => "USB Device",
                AAUDIO_TYPE_USB_ACCESSORY => "USB Accessory",
                AAUDIO_TYPE_DOCK => "Dock",
                AAUDIO_TYPE_FM => "FM",
                AAUDIO_TYPE_BUILTIN_MIC => "Built-in Mic",
                AAUDIO_TYPE_FM_TUNER => "FM Tuner",
                AAUDIO_TYPE_TV_TUNER => "TV Tuner",
                AAUDIO_TYPE_TELEPHONY => "Telephony",
                AAUDIO_TYPE_AUX_LINE => "Aux Line",
                AAUDIO_TYPE_IP => "IP",
                AAUDIO_TYPE_BUS => "Bus",
                AAUDIO_TYPE_USB_HEADSET => "Usb Headset",
                AAUDIO_TYPE_HEARING_AID => "Hearing Aid",
                AAUDIO_TYPE_BUILTIN_SPEAKER_SAFE => "Built-in Speaker Safe",
                AAUDIO_TYPE_REMOTE_SUBMIX => "Remote Submix",
                AAUDIO_TYPE_BLE_HEADSET => "BLE Headset",
                AAUDIO_TYPE_BLE_SPEAKER => "BLE Speaker",
                AAUDIO_TYPE_ECHO_REFERENCE => "Echo Reference",
                AAUDIO_TYPE_HDMI_EARC => "HDMI EARC",
                AAUDIO_TYPE_BLE_BROADCAST => "BLE Broadcast",
                _ => "Unknown"
            }
        }
        let parts: Vec<&str> = input.split("$$").collect();
        if parts.len() != 4 {
            return None;
        }
        let aaudio_id: i32 = parts[0].parse().unwrap();
        let ty: u32 = parts[1].parse().unwrap();
        let channel_count: usize = parts[2].parse().unwrap();
        let name = format!("{} {} {} channels", parts[3], type_id_to_string(ty), channel_count);
        Some(AndroidAudioDeviceDesc {
            aaudio_id,
            desc: AudioDeviceDesc {
                device_id: LiveId::from_str(&name).into(),
                device_type,
                is_default: false,
                has_failed: false,
                channel_count,
                name
            }
        })
    }
    
    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceDesc> {
        /*
        let inputs = to_java.get_audio_devices(0);
        let outputs = to_java.get_audio_devices(1);
        self.device_descs.clear();
        
        let name = format!("Default input 2 channels");
        self.device_descs.push(AndroidAudioDeviceDesc {
            aaudio_id: 0,
            desc: AudioDeviceDesc {
                device_id: LiveId::from_str(&name).into(),
                device_type: AudioDeviceType::Input,
                is_default: true,
                has_failed: false,
                channel_count: 2,
                name
            }
        });
        let name = format!("Default output 2 channels");
        self.device_descs.push(AndroidAudioDeviceDesc {
            aaudio_id: 0,
            desc: AudioDeviceDesc {
                device_id: LiveId::from_str(&name).into(),
                device_type: AudioDeviceType::Output,
                is_default: true,
                has_failed: false,
                channel_count: 2,
                name
            }
        });
        for input in &inputs {
            if let Some(mut device) = Self::java_ret_to_device(input, AudioDeviceType::Input) {
                device.desc.has_failed = self.failed_devices.contains(&device.desc.device_id);
                self.device_descs.push(device);
            }
        }
        for output in &outputs {
            if let Some(mut device) = Self::java_ret_to_device(output, AudioDeviceType::Output) {
                device.desc.has_failed = self.failed_devices.contains(&device.desc.device_id);
                self.device_descs.push(device);
            }
        }
        let mut descs = Vec::new();
        for device in &self.device_descs {
            descs.push(device.desc.clone())
        }
        descs
        */
        Vec::new()
    }
    
    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut i = 0;
            while i < self.audio_inputs.len() {
                if self.audio_inputs[i].is_in_error_state.swap(false, Ordering::SeqCst)
                    || !devices.contains(&self.audio_inputs[i].device_id) {
                    unsafe {self.audio_inputs.remove(i).terminate()};
                }
                else {
                    i += 1;
                }
            }
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if self.audio_inputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    if let Some(device_desc) = self.device_descs.iter().find( | v | v.desc.device_id == *device_id) {
                        new.push((index, device_desc.clone()))
                    }
                }
            }
            new
        };
        for (index, device_desc) in new {
            let input_cb = self.audio_input_cb[index].clone();
            match unsafe {AndroidAudioInput::new(&device_desc, self.change_signal.clone(), input_cb)} {
                Ok(new_input) => {
                    self.audio_inputs.push(new_input);
                }
                Err(e) => {
                    log!("AAaudio error {}", e.0);
                    self.failed_devices.insert(device_desc.desc.device_id);
                    self.change_signal.set();
                }
            }
        }
    }
    
    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            // lets shut down the ones we dont use
            let mut i = 0;
            while i < self.audio_outputs.len() {
                if self.audio_outputs[i].is_in_error_state.swap(false, Ordering::SeqCst)
                    || !devices.contains(&self.audio_outputs[i].device_id) {
                    unsafe{self.audio_outputs.remove(i).terminate()};
                }
                else {
                    i += 1;
                }
            }
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if self.audio_outputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    if let Some(device_desc) = self.device_descs.iter().find( | v | v.desc.device_id == *device_id) {
                        new.push((index, device_desc.clone()))
                    }
                }
            }
            new
            
        };
        for (index, device_desc) in new {
            let output_cb = self.audio_output_cb[index].clone();
            match unsafe {AndroidAudioOutput::new(&device_desc, self.change_signal.clone(), output_cb)} {
                Ok(new_output) => {
                    self.audio_outputs.push(new_output);
                }
                Err(e) => {
                    log!("AAaudio error {}", e.0);
                    self.failed_devices.insert(device_desc.desc.device_id);
                    self.change_signal.set();
                }
            }
        }
    }
}

