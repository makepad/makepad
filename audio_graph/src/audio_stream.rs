
// ok so. an audio stream. its an object with a 'sender'
use {
    crate::{
        makepad_platform::audio::*,
    },
    std::sync::{Arc, Mutex},
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

#[derive(Clone)]
pub struct AudioStreamReceiver(Arc<Mutex<ReceiverInner>>);

pub struct ReceiverInner {
    pub routes: Vec<AudioRoute>,
    stream_recv: Receiver<(u64, AudioBuffer)>,
}

unsafe impl Send for AudioStreamReceiver {}

pub struct AudioRoute {
    id: u64,
    start_offset: usize,
    buffers: Vec<AudioBuffer>
}

impl AudioStreamSender {
    pub fn create_pair() -> (AudioStreamSender, AudioStreamReceiver) {
        let (stream_send, stream_recv) = channel::<(u64, AudioBuffer)>();
        (AudioStreamSender {
            stream_send,
        }, AudioStreamReceiver(Arc::new(Mutex::new(ReceiverInner {
            stream_recv,
            routes: Vec::new()
        }))))
    }
    
    pub fn write_buffer(&self, route_id: u64, buffer: AudioBuffer) -> Result<(), SendError<(u64, AudioBuffer) >> {
        self.stream_send.send((route_id, buffer))
    }
}

impl AudioStreamReceiver {
    pub fn num_routes(&self) -> usize {
        let iself = self.0.lock().unwrap();
        iself.routes.len()
    }
    
    pub fn route_id(&self, route_num: usize) -> u64 {
        let iself = self.0.lock().unwrap();
        iself.routes[route_num].id
    }

    pub fn try_recv_stream(&mut self) {
        let mut iself = self.0.lock().unwrap();
        while let Ok((route_id, buf)) = iself.stream_recv.try_recv() {
            if let Some(route) = iself.routes.iter_mut().find( | v | v.id == route_id) {
                route.buffers.push(buf);
            }
            else {
                iself.routes.push(AudioRoute {
                    id: route_id,
                    buffers: vec![buf],
                    start_offset: 0
                });
            }
        }
    }
    
    pub fn recv_stream(&mut self) {
        {
            let mut iself = self.0.lock().unwrap();
            if let Ok((route_id, buf)) = iself.stream_recv.recv() {
                if let Some(route) = iself.routes.iter_mut().find( | v | v.id == route_id) {
                    route.buffers.push(buf);
                }
                else {
                    iself.routes.push(AudioRoute {
                        id: route_id,
                        buffers: vec![buf],
                        start_offset: 0
                    });
                }
            }
        }
        self.try_recv_stream();
    }
    
    pub fn read_buffer(&mut self, route_num: usize, output: &mut AudioBuffer, min_buf: usize) -> usize {
        let mut iself = self.0.lock().unwrap();
        let route = if let Some(route) = iself.routes.get_mut(route_num) {
            route
        }
        else {
            return 0;
        };
        
        // ok if we dont have enough data in our stack for output, just output nothing
        let mut total = 0;
        for buf in route.buffers.iter() {
            total += buf.frame_count();
        }

        // check if we have enough buffer
        if total - route.start_offset < output.frame_count() * min_buf {
            return 0
        }
        
        // ok so we need to eat from the start of the buffer vec until output is filled
        let mut frames_read = 0;
        let out_channel_count = output.channel_count();
        let out_frame_count = output.frame_count();
        while let Some(input) = route.buffers.first() {
            // ok so. we can copy buffer from start_offset
            let mut start_offset = None;
            let start_frames_read = frames_read;
            for chan in 0..out_channel_count {
                frames_read = start_frames_read;
                let inp = input.channel(chan.min(input.channel_count() - 1));
                let out = output.channel_mut(chan);
                // alright so we write into the output buffer
                for i in route.start_offset..inp.len() {
                    if frames_read >= out_frame_count {
                        start_offset = Some(i);
                        break;
                    }
                    out[frames_read] = inp[i];
                    frames_read += 1;
                }
            }
            // only consumed a part of the buffer
            if let Some(start_offset) = start_offset {
                route.start_offset = start_offset;
                break
            }
            else { // consumed entire buffer
                route.start_offset = 0;
                route.buffers.remove(0);
            }
        }
        
        frames_read
    }
}
