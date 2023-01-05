
// ok so. an audio stream. its an object with a 'sender'
use {
    crate::{
        makepad_platform::audio::*,
    },
    std::sync::mpsc::{
        channel,
        Sender,
        Receiver,
        SendError
    }
};

#[derive(Clone)]
pub struct AudioStreamSender {
    stream_send: Sender<(u64, AudioBuffer)>,
}
unsafe impl Send for AudioStreamSender {}

pub struct AudioStreamReceiver {
    buffers: Vec<(u64, Vec<AudioBuffer>)>,
    start_offset: usize,
    stream_recv: Receiver<(u64, AudioBuffer)>,
}

unsafe impl Send for AudioStreamReceiver {}

impl AudioStreamSender {
    pub fn create_pair() -> (AudioStreamSender, AudioStreamReceiver) {
        let (stream_send, stream_recv) = channel::<(u64, AudioBuffer)>();
        (AudioStreamSender {
            stream_send,
        }, AudioStreamReceiver {
            start_offset: 0,
            stream_recv,
            buffers: Vec::new()
        })
    }
    
    pub fn write_buffer(&self, route_id:u64, buffer: AudioBuffer) -> Result<(), SendError<(u64, AudioBuffer) >> {
        self.stream_send.send((route_id, buffer))
    }
}

impl AudioStreamReceiver {
    pub fn num_routes(&self) -> usize {
        self.buffers.len()
    }
    
    pub fn route_id(&self, route_num: usize) -> u64 {
        self.buffers[route_num].0
    }
    
    pub fn try_recv_stream(&mut self) {
        while let Ok((channel_id, buf)) = self.stream_recv.try_recv() {
            if let Some((_, buffers)) = self.buffers.iter_mut().find( | v | v.0 == channel_id) {
                buffers.push(buf);
            }
            else {
                self.buffers.push((channel_id, vec![buf]));
            }
        }
    }
    
    pub fn recv_stream(&mut self) {
        if let Ok((channel_id, buf)) = self.stream_recv.recv() {
            if let Some((_, buffers)) = self.buffers.iter_mut().find( | v | v.0 == channel_id) {
                buffers.push(buf);
            }
            else {
                self.buffers.push((channel_id, vec![buf]));
            }
        }
        self.try_recv_stream();
    }
    
    pub fn read_buffer(&mut self, route_num: usize, output: &mut AudioBuffer, min_multiple: usize, max_multiple: usize) -> usize {
        
        let buffers = if let Some((_, vec)) = self.buffers.get_mut(route_num) {
            vec
        }
        else {
            return 0;
        };
        
        // ok if we dont have enough data in our stack for output, just output nothing
        let mut total = 0;
        for buf in buffers.iter() {
            total += buf.frame_count();
        }
        
        // check if we have enough buffer
        if total - self.start_offset < output.frame_count() * min_multiple {
            return 0
        }
        
        // if we have too much buffer throw it out
        while total > output.frame_count() * max_multiple {
            let input = buffers.remove(0);
            total -= input.frame_count();
        }
        
        // ok so we need to eat from the start of the buffer vec until output is filled
        let mut frames_read = 0;
        let out_channel_count = output.channel_count();
        let out_frame_count = output.frame_count();
        while let Some(input) = buffers.first() {
            // ok so. we can copy buffer from start_offset
            let mut start_offset = self.start_offset;
            let start_frames_read = frames_read;
            for chan in 0..out_channel_count {
                frames_read = start_frames_read;
                let inp = input.channel(chan.min(input.channel_count() - 1));
                let out = output.channel_mut(chan);
                // alright so we write into the output buffer
                for i in self.start_offset..inp.len() {
                    if frames_read >= out_frame_count {
                        start_offset = i;
                        break;
                    }
                    out[frames_read] = inp[i];
                    frames_read += 1;
                }
            }
            // only consumed a part of the buffer
            if start_offset != self.start_offset {
                self.start_offset = start_offset;
                return frames_read
            }
            else { // consumed entire buffer
                self.start_offset = 0;
                buffers.remove(0);
            }
        }
        frames_read
    }
}
