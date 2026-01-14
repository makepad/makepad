// Audio stream is strictly a utility class to combine multiple input streams
// Uses an adaptive jitter buffer for smooth, low-latency audio

use {
    crate::{
        audio::*,
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
    min_buf: usize,
    max_buf: usize,
    stream_recv: Receiver<(u64, AudioBuffer)>,
}

unsafe impl Send for AudioStreamReceiver {}

pub struct AudioRoute {
    id: u64,
    // Fractional read position for smooth rate adjustment
    read_pos: f64,
    buffers: Vec<AudioBuffer>,
    // Total frames consumed from current first buffer
    frames_consumed: usize,
    // Adaptive rate: 1.0 = normal, >1.0 = faster (catching up), <1.0 = slower
    playback_rate: f64,
    // Smoothed buffer level for rate control
    smoothed_buffer_frames: f64,
}

impl AudioStreamSender {
    pub fn create_pair(min_buf:usize, max_buf: usize) -> (AudioStreamSender, AudioStreamReceiver) {
        let (stream_send, stream_recv) = channel::<(u64, AudioBuffer)>();
        (AudioStreamSender {
            stream_send,
        }, AudioStreamReceiver(Arc::new(Mutex::new(ReceiverInner {
            stream_recv,
            min_buf,
            max_buf,
            routes: Vec::new()
        }))))
    }
    
    pub fn send(&self, route_id: u64, buffer: AudioBuffer) -> Result<(), SendError<(u64, AudioBuffer) >> {
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
    
    /// Returns the channel count of pending buffers for a route, or None if no buffers
    pub fn channel_count(&self, route_num: usize) -> Option<usize> {
        let iself = self.0.lock().unwrap();
        iself.routes.get(route_num)
            .and_then(|route| route.buffers.first())
            .map(|buf| buf.channel_count())
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
                    read_pos: 0.0,
                    frames_consumed: 0,
                    playback_rate: 1.0,
                    smoothed_buffer_frames: 0.0,
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
                        read_pos: 0.0,
                        frames_consumed: 0,
                        playback_rate: 1.0,
                        smoothed_buffer_frames: 0.0,
                    });
                }
            }
        }
        self.try_recv_stream();
    }
    
    pub fn read_buffer(&mut self, route_num: usize, output: &mut AudioBuffer) -> usize {
        let mut iself = self.0.lock().unwrap();
        let min_buf = iself.min_buf;
        let max_buf = iself.max_buf;
        let route = if let Some(route) = iself.routes.get_mut(route_num) {
            route
        }
        else {
            return 0;
        };

        // Calculate total available frames
        let mut total_frames: usize = 0;
        for buf in route.buffers.iter() { 
            total_frames += buf.frame_count();
        }
        let available_frames = total_frames.saturating_sub(route.frames_consumed);
        
        let out_frame_count = output.frame_count();
        let out_channel_count = output.channel_count();
        
        // Target buffer level: midpoint between min and max
        let target_frames = out_frame_count * (min_buf + max_buf) / 2;
        
        // Smooth the buffer level measurement (exponential moving average)
        let alpha = 0.1; // Smoothing factor
        route.smoothed_buffer_frames = route.smoothed_buffer_frames * (1.0 - alpha) 
            + available_frames as f64 * alpha;
        
        // If we have absolutely nothing, output silence
        if available_frames == 0 || route.buffers.is_empty() {
            output.zero();
            return 0;
        }
        
        // Adaptive rate control based on buffer fullness
        // When buffer is fuller than target, speed up slightly
        // When buffer is emptier than target, slow down slightly
        let buffer_ratio = route.smoothed_buffer_frames / target_frames as f64;
        
        // Rate adjustment: gentle curve, clamped to reasonable range
        // At 2x target: rate = 1.02 (2% faster)
        // At 0.5x target: rate = 0.98 (2% slower)
        // Max adjustment: ±5%
        let rate_adjustment = (buffer_ratio - 1.0) * 0.02;
        let target_rate = (1.0 + rate_adjustment).clamp(0.95, 1.05);
        
        // Smooth rate changes to avoid artifacts
        let rate_alpha = 0.05;
        route.playback_rate = route.playback_rate * (1.0 - rate_alpha) + target_rate * rate_alpha;
        
        // If buffer is critically low, just output what we have at normal rate
        // to avoid pitch artifacts on near-empty buffer
        let effective_rate = if available_frames < out_frame_count {
            1.0
        } else {
            route.playback_rate
        };
        
        // Read with interpolation at the adjusted rate
        let mut out_frame = 0;
        while out_frame < out_frame_count {
            // Find current buffer and position
            let first_buf = match route.buffers.first() {
                Some(b) => b,
                None => break,
            };
            
            let local_pos = route.read_pos;
            let local_idx = local_pos as usize;
            let frac = (local_pos - local_idx as f64) as f32;
            
            // Check if we've exhausted this buffer
            if local_idx >= first_buf.frame_count() {
                // Move to next buffer
                route.buffers.remove(0);
                route.read_pos = 0.0;
                route.frames_consumed = 0;
                continue;
            }
            
            // Check if we need to stop (not enough data for interpolation)
            let next_idx = local_idx + 1;
            let have_next = next_idx < first_buf.frame_count() || route.buffers.len() > 1;
            
            if !have_next && local_idx >= first_buf.frame_count() - 1 {
                // At the very end with no next buffer, output what we have
                for chan in 0..out_channel_count {
                    let src_chan = chan.min(first_buf.channel_count() - 1);
                    output.channel_mut(chan)[out_frame] = first_buf.channel(src_chan)[local_idx];
                }
                out_frame += 1;
                route.read_pos += effective_rate;
                continue;
            }
            
            // Linear interpolation between samples
            for chan in 0..out_channel_count {
                let src_chan = chan.min(first_buf.channel_count() - 1);
                let sample0 = first_buf.channel(src_chan)[local_idx];
                let sample1 = if next_idx < first_buf.frame_count() {
                    first_buf.channel(src_chan)[next_idx]
                } else if let Some(next_buf) = route.buffers.get(1) {
                    // Interpolate across buffer boundary
                    next_buf.channel(chan.min(next_buf.channel_count() - 1))[0]
                } else {
                    sample0 // No next sample, hold value
                };
                
                output.channel_mut(chan)[out_frame] = sample0 + (sample1 - sample0) * frac;
            }
            
            out_frame += 1;
            route.read_pos += effective_rate;
        }
        
        // Update frames_consumed for accurate buffer level tracking
        route.frames_consumed = route.read_pos as usize;
        
        // Clean up fully consumed buffers
        while let Some(first) = route.buffers.first() {
            if route.read_pos >= first.frame_count() as f64 {
                let consumed = first.frame_count();
                route.buffers.remove(0);
                route.read_pos -= consumed as f64;
                route.frames_consumed = route.read_pos as usize;
            } else {
                break;
            }
        }
        
        out_frame
    }
}
