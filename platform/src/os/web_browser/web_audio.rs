use {
    crate::{
        audio::{
            AudioTime,
            AudioOutputBuffer
        },
    }
};

#[repr(C)]
pub struct WebAudioOutputClosure{
    pub callback: Box<dyn FnMut(AudioTime, &mut AudioBuffer) + Send + 'static>,
    pub output_buffer: AudioBuffer,
}

#[export_name = "wasm_audio_entrypoint"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_audio_entrypoint(closure_ptr: u32, frames:u32, channels:u32)->u32{
    let mut closure = Box::from_raw(closure_ptr as *mut WebAudioOutputClosure);
    let time = AudioTime{ sample_time: 0.0, host_time: 0, rate_scalar:0.0};
    let callback = &mut closure.callback;
    closure.output_buffer.clear_final_size();
    closure.output_buffer.resize(frames as usize, channels as usize);
    closure.output_buffer.set_final_size();
    callback(time, &mut closure.output_buffer);
    let ptr = closure.output_buffer.data.as_ptr();
    Box::into_raw(closure);
    ptr as u32
}

