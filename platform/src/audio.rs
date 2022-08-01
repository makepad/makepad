
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
    fn copy_from_buffer(&mut self, buffer:&AudioBuffer){
        if self.channel_count() != buffer.channel_count{
            panic!("Output buffer channel_count != buffer channel_count {} {}",self.channel_count(), buffer.channel_count );
        }
        if self.frame_count() != buffer.frame_count{
            panic!("Output buffer frame_count != buffer frame_count ");
        }
        for i in 0..self.channel_count(){
            let output = self.channel_mut(i);
            let input = buffer.channel(i);
            output.copy_from_slice(input);
        }        
    }
}

#[derive(Clone, Default)]
pub struct AudioBuffer {
    pub data: Vec<f32>,
    pub frame_count: usize,
    pub channel_count: usize
}

impl AudioBuffer {
    pub fn new_with_size(frame_count: usize, channel_count: usize)->Self{
        let mut ret = Self::default();
        ret.resize(frame_count, channel_count);
        ret
    }
    
    pub fn frame_count(&self)->usize{self.frame_count}
    pub fn channel_count(&self)->usize{self.channel_count}
    
    pub fn copy_from(&mut self, like:&AudioBuffer)->&mut Self{
        self.resize(like.frame_count(), like.channel_count());
        self.data.copy_from_slice(&like.data);
        self
    }
    
    pub fn resize_like(&mut self, like:&AudioBuffer)->&mut Self{
        self.resize(like.frame_count(), like.channel_count());
        self
    }

    pub fn resize_like_output(&mut self, like:&mut dyn AudioOutputBuffer)->&mut Self{
        self.resize(like.frame_count(), like.channel_count());
        self
    }
    
    pub fn resize(&mut self, frame_count: usize, channel_count: usize) {
        self.frame_count = frame_count;
        self.channel_count = channel_count;
        self.data.resize(frame_count * channel_count as usize, 0.0);
    }

    pub fn stereo_mut(&mut self) -> (&mut [f32],&mut [f32]) {
        if self.channel_count != 2{panic!()}
        self.data.split_at_mut(self.frame_count)
    }

    pub fn stereo(&self) -> (&[f32],&[f32]) {
        if self.channel_count != 2{panic!()}
        self.data.split_at(self.frame_count)
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
