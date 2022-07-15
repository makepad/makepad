#[cfg(target_os = "macos")]
pub use crate::platform::apple::audio_unit::{
    AudioFactory,
    AudioDevice,
    AudioDeviceClone,
};

#[derive(Copy, Clone)]
pub enum AudioDeviceType {
    DefaultOutput,
    MusicDevice,
    Effect
}

#[derive(Copy, Clone)]
pub struct AudioTime {
    pub sample_time: f64,
    pub host_time: u64,
    pub rate_scalar: f64,
}

pub trait AudioOutputBuffer{
    fn frame_count(&self)->usize;
    fn channel_count(&self)->usize;
    fn channel_mut(&mut self, channel: usize) -> &mut [f32];
    fn zero(&mut self);
    fn copy_from_buffer(&mut self, buffer:&AudioBuffer);
}

#[derive(Clone, Default)]
pub struct AudioBuffer {
    pub data: Vec<f32>,
    pub frame_count: usize,
    pub channel_count: usize
}

impl AudioBuffer {
    pub fn frame_count(&self)->usize{self.frame_count}
    pub fn channel_count(&self)->usize{self.channel_count}
    
    pub fn resize_like(&mut self, like:&AudioBuffer)->&mut Self{
        self.resize(like.frame_count(), like.channel_count());
        self
    }

    pub fn resize_like_output(&mut self, like:&mut impl AudioOutputBuffer)->&mut Self{
        self.resize(like.frame_count(), like.channel_count());
        self
    }    
    pub fn resize(&mut self, frame_count: usize, channel_count: usize) {
        self.frame_count = frame_count;
        self.channel_count = channel_count;
        self.data.resize(frame_count * channel_count as usize, 0.0);
    }

    pub fn channel_mut(&mut self, channel: usize) -> &mut [f32] {
        &mut self.data[channel * self.frame_count..(channel+1) * self.frame_count]
    }

    pub fn channel(&self, channel: usize) -> &[f32] {
        &self.data[channel * self.frame_count..(channel+1) * self.frame_count]
    }

    pub fn zero(&mut self) {
        for i in 0..self.data.len() {
            self.data[i] = 0.0;
        }
    }
}
