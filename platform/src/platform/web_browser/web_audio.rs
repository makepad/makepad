use {
    crate::{
        audio::{
            AudioTime,
            AudioOutputBuffer
        },
    }
};

#[derive(Default)]
pub struct WebAudioOutputBuffer{
    pub channels: usize,
    pub data: Vec<f32>
}

#[repr(C)]
pub struct WebAudioOutputClosure{
    pub callback: Box<dyn FnMut(AudioTime, &mut dyn AudioOutputBuffer) + Send + 'static>,
    pub output_buffer: WebAudioOutputBuffer,
}

impl WebAudioOutputBuffer{
    fn assure_size(&mut self, frames:usize, channels:usize){
        if self.data.len() != frames * channels{
            self.data.resize(frames * channels, 0.0);
            self.zero();
        }
        self.channels = channels;
    }
}

impl AudioOutputBuffer for WebAudioOutputBuffer{
    fn frame_count(&self)->usize{self.data.len() / self.channels}
    fn channel_count(&self)->usize{self.channels}
    fn channel_mut(&mut self, channel: usize) -> &mut [f32]{
        let frame_count = self.frame_count();
        &mut self.data[frame_count*channel..frame_count*(channel+1)]    
    }
    
    fn zero(&mut self){
        for i in 0..self.data.len(){
            self.data[i] = 0.0;
        }    
    }
}

#[export_name = "wasm_audio_entrypoint"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_audio_entrypoint(closure_ptr: u32, frames:u32, channels:u32)->u32{
    let mut closure = Box::from_raw(closure_ptr as *mut WebAudioOutputClosure);
    let time = AudioTime{ sample_time: 0.0, host_time: 0, rate_scalar:0.0};
    let callback = &mut closure.callback;
    closure.output_buffer.assure_size(frames as usize, channels as usize);
    callback(time, &mut closure.output_buffer);
    let ptr = closure.output_buffer.data.as_ptr();
    Box::into_raw(closure);
    ptr as u32
}

