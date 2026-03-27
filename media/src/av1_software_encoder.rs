#[cfg(has_svt_av1)]
use std::collections::VecDeque;
#[cfg(has_svt_av1)]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueuePolicy {
    LatestWins,
}

#[derive(Clone, Copy, Debug)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub target_bitrate: u32,
    pub keyint: i32,
    pub codec_mode: i32,
    pub queue_policy: QueuePolicy,
    pub queue_capacity: usize,
}

#[derive(Clone, Debug, Default)]
pub struct I420Frame {
    pub timestamp_ns: u64,
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub y_stride: u32,
    pub u_stride: u32,
    pub v_stride: u32,
}

#[derive(Clone, Debug, Default)]
pub struct EncodedPacket {
    pub pts_ns: u64,
    pub is_key: bool,
    pub is_eos: bool,
    pub data: Vec<u8>,
}

pub type OutputFn = Box<dyn FnMut(EncodedPacket) + Send + 'static>;

#[cfg(has_svt_av1)]
struct SharedQueue {
    queue: Mutex<VecDeque<I420Frame>>,
    condvar: Condvar,
}

pub struct Av1SoftwareEncoder {
    #[cfg(has_svt_av1)]
    running: Arc<AtomicBool>,
    #[cfg(has_svt_av1)]
    queue: Arc<SharedQueue>,
    #[cfg(has_svt_av1)]
    queue_policy: QueuePolicy,
    #[cfg(has_svt_av1)]
    queue_capacity: usize,
    #[cfg(has_svt_av1)]
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Av1SoftwareEncoder {
    pub fn start(config: EncoderConfig, output: OutputFn) -> Option<Self> {
        #[cfg(not(has_svt_av1))]
        {
            let _ = config;
            let _ = output;
            None
        }

        #[cfg(has_svt_av1)]
        {
            let running = Arc::new(AtomicBool::new(true));
            let queue = Arc::new(SharedQueue {
                queue: Mutex::new(VecDeque::new()),
                condvar: Condvar::new(),
            });

            let running_clone = running.clone();
            let queue_clone = queue.clone();
            let worker = std::thread::Builder::new()
                .name("video-encoder-av1".to_string())
                .spawn(move || {
                    worker_loop(config, output, running_clone, queue_clone);
                })
                .ok()?;

            Some(Self {
                running,
                queue,
                queue_policy: config.queue_policy,
                queue_capacity: config.queue_capacity.max(1),
                worker: Mutex::new(Some(worker)),
            })
        }
    }

    pub fn push_i420(&self, frame: I420Frame) {
        #[cfg(has_svt_av1)]
        {
            if !self.running.load(Ordering::Relaxed) {
                return;
            }

            let mut q = self.queue.queue.lock().unwrap();
            match self.queue_policy {
                QueuePolicy::LatestWins => {
                    if q.len() >= self.queue_capacity {
                        q.pop_front();
                    }
                }
            }
            q.push_back(frame);
            self.queue.condvar.notify_one();
        }

        #[cfg(not(has_svt_av1))]
        {
            let _ = frame;
        }
    }

    pub fn stop(&self) {
        #[cfg(has_svt_av1)]
        {
            if self.running.swap(false, Ordering::SeqCst) {
                self.queue.condvar.notify_all();
                if let Some(worker) = self.worker.lock().unwrap().take() {
                    let _ = worker.join();
                }
            }
        }
    }
}

impl Drop for Av1SoftwareEncoder {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(has_svt_av1)]
fn worker_loop(
    config: EncoderConfig,
    mut output: OutputFn,
    running: Arc<AtomicBool>,
    queue: Arc<SharedQueue>,
) {
    use crate::svt_av1_ffi::*;

    struct EncoderPtr(*mut std::ffi::c_void);
    unsafe impl Send for EncoderPtr {}

    let enc = unsafe {
        mp_svt_av1_encoder_create(
            config.width,
            config.height,
            config.fps_num,
            config.fps_den,
            config.target_bitrate,
            config.keyint,
            config.codec_mode,
        )
    };

    if enc.is_null() {
        return;
    }

    let enc = EncoderPtr(enc);

    loop {
        let frame = {
            let mut guard = queue.queue.lock().unwrap();
            while running.load(Ordering::Relaxed) && guard.is_empty() {
                guard = queue.condvar.wait(guard).unwrap();
            }
            guard.pop_front()
        };

        if let Some(frame) = frame {
            if frame.width != config.width || frame.height != config.height {
                continue;
            }
            if frame.y.is_empty() || frame.u.is_empty() || frame.v.is_empty() {
                continue;
            }

            let send_ret = unsafe {
                mp_svt_av1_encoder_send_i420(
                    enc.0,
                    frame.y.as_ptr(),
                    frame.y_stride,
                    frame.u.as_ptr(),
                    frame.u_stride,
                    frame.v.as_ptr(),
                    frame.v_stride,
                    frame.height,
                    frame.timestamp_ns as i64,
                )
            };

            if send_ret == 0 {
                drain_packets(enc.0, false, &mut output);
            }
            continue;
        }

        if !running.load(Ordering::Relaxed) {
            let _ = unsafe { mp_svt_av1_encoder_send_eos(enc.0) };
            drain_packets(enc.0, true, &mut output);
            break;
        }
    }

    unsafe {
        mp_svt_av1_encoder_destroy(enc.0);
    }
}

#[cfg(has_svt_av1)]
fn drain_packets(encoder: *mut std::ffi::c_void, pic_send_done: bool, output: &mut OutputFn) {
    use crate::svt_av1_ffi::*;

    loop {
        let mut pkt = MpSvtAv1Packet {
            data: std::ptr::null_mut(),
            len: 0,
            pts: 0,
            flags: 0,
            pic_type: 0,
            out_buffer: std::ptr::null_mut(),
        };
        let ret = unsafe {
            mp_svt_av1_encoder_get_packet_copy(encoder, if pic_send_done { 1 } else { 0 }, &mut pkt)
        };
        if ret == 1 {
            break;
        }
        if ret != 0 {
            break;
        }
        if pkt.data.is_null() || pkt.len == 0 {
            unsafe { mp_svt_av1_packet_free(&mut pkt) };
            continue;
        }

        let bytes = unsafe { std::slice::from_raw_parts(pkt.data, pkt.len as usize) };
        output(EncodedPacket {
            pts_ns: pkt.pts.max(0) as u64,
            is_key: pkt.pic_type == 3,
            is_eos: (pkt.flags & 0x1) != 0,
            data: bytes.to_vec(),
        });
        unsafe { mp_svt_av1_packet_free(&mut pkt) };
    }
}
