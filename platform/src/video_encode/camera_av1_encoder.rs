use crate::video::*;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};

struct SharedQueue {
    queue: Mutex<VecDeque<CameraFrameOwned>>,
    condvar: Condvar,
}

pub struct CameraAv1Encoder {
    running: Arc<AtomicBool>,
    queue: Arc<SharedQueue>,
    queue_capacity: usize,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl CameraAv1Encoder {
    pub fn start(config: CameraAv1EncoderConfig, output: CameraAv1OutputFn) -> Option<Self> {
        if config.width == 0 || config.height == 0 || config.fps_num == 0 {
            crate::error!("camera av1 encoder invalid config: {:?}", config);
            return None;
        }

        #[cfg(not(has_svt_av1))]
        {
            let _ = output;
            crate::error!("camera av1 encoder unavailable: SVT-AV1 not linked for this target");
            return None;
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
                .name("camera-av1-encoder".to_string())
                .spawn(move || {
                    worker_loop(config, output, running_clone, queue_clone);
                })
                .ok()?;

            Some(Self {
                running,
                queue,
                queue_capacity: config.queue_capacity.max(1),
                worker: Mutex::new(Some(worker)),
            })
        }
    }

    pub fn push_frame(&self, frame: CameraFrameRef<'_>) {
        if !self.running.load(Ordering::Relaxed) {
            return;
        }

        let mut owned = CameraFrameOwned::default();
        if !convert_to_i420(&mut owned, frame) {
            return;
        }

        let mut q = self.queue.queue.lock().unwrap();
        if q.len() >= self.queue_capacity {
            q.pop_front();
        }
        q.push_back(owned);
        self.queue.condvar.notify_one();
    }

    pub fn stop(&self) {
        if self.running.swap(false, Ordering::SeqCst) {
            self.queue.condvar.notify_all();
            if let Some(worker) = self.worker.lock().unwrap().take() {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for CameraAv1Encoder {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(has_svt_av1)]
fn worker_loop(
    config: CameraAv1EncoderConfig,
    mut output: CameraAv1OutputFn,
    running: Arc<AtomicBool>,
    queue: Arc<SharedQueue>,
) {
    use super::svt_av1_ffi::*;

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
            config.enc_mode,
        )
    };

    if enc.is_null() {
        crate::error!("camera av1 encoder init failed");
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
            if frame.width as u32 != config.width || frame.height as u32 != config.height {
                continue;
            }
            let y = &frame.planes[0].bytes;
            let u = &frame.planes[1].bytes;
            let v = &frame.planes[2].bytes;
            if y.is_empty() || u.is_empty() || v.is_empty() {
                continue;
            }

            let send_ret = unsafe {
                mp_svt_av1_encoder_send_i420(
                    enc.0,
                    y.as_ptr(),
                    frame.planes[0].row_stride as u32,
                    u.as_ptr(),
                    frame.planes[1].row_stride as u32,
                    v.as_ptr(),
                    frame.planes[2].row_stride as u32,
                    frame.timestamp_ns as i64,
                )
            };

            if send_ret != 0 {
                crate::error!("camera av1 encoder send frame failed: {}", send_ret);
                break;
            }

            drain_packets(enc.0, false, &mut output);
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
fn drain_packets(
    encoder: *mut std::ffi::c_void,
    pic_send_done: bool,
    output: &mut CameraAv1OutputFn,
) {
    use super::svt_av1_ffi::*;

    loop {
        let mut pkt = MpSvtAv1Packet {
            data: std::ptr::null_mut(),
            len: 0,
            pts: 0,
            flags: 0,
            pic_type: 0,
        };
        let ret = unsafe {
            mp_svt_av1_encoder_get_packet_copy(encoder, if pic_send_done { 1 } else { 0 }, &mut pkt)
        };
        if ret == 1 {
            break;
        }
        if ret != 0 {
            crate::error!("camera av1 encoder get packet failed: {}", ret);
            break;
        }
        if pkt.data.is_null() || pkt.len == 0 {
            unsafe { mp_svt_av1_packet_free(&mut pkt) };
            continue;
        }
        let bytes = unsafe { std::slice::from_raw_parts(pkt.data, pkt.len as usize) };
        output(EncodedAv1PacketRef {
            pts_ns: pkt.pts.max(0) as u64,
            is_key: pkt.pic_type == 3,
            is_eos: (pkt.flags & 0x1) != 0,
            data: bytes,
        });
        unsafe { mp_svt_av1_packet_free(&mut pkt) };
    }
}

fn copy_strided_plane(
    src: CameraFramePlaneRef<'_>,
    width: usize,
    height: usize,
    dst: &mut Vec<u8>,
) {
    dst.resize(width * height, 0);
    if src.bytes.is_empty() || width == 0 || height == 0 {
        return;
    }

    if src.pixel_stride == 1 && src.row_stride == width {
        let n = dst.len().min(src.bytes.len());
        dst[..n].copy_from_slice(&src.bytes[..n]);
        return;
    }

    for row in 0..height {
        let src_row = row.saturating_mul(src.row_stride);
        let dst_row = row * width;
        for col in 0..width {
            let src_idx = src_row + col.saturating_mul(src.pixel_stride.max(1));
            dst[dst_row + col] = src.bytes.get(src_idx).copied().unwrap_or(0);
        }
    }
}

fn convert_to_i420(dst: &mut CameraFrameOwned, src: CameraFrameRef<'_>) -> bool {
    let w = src.width;
    let h = src.height;
    if w == 0 || h == 0 {
        return false;
    }

    dst.timestamp_ns = src.timestamp_ns;
    dst.width = w;
    dst.height = h;
    dst.layout = CameraFrameLayout::I420;
    dst.matrix = src.matrix;
    dst.plane_count = 3;
    dst.planes[0].row_stride = w;
    dst.planes[0].pixel_stride = 1;
    dst.planes[1].row_stride = w.div_ceil(2);
    dst.planes[1].pixel_stride = 1;
    dst.planes[2].row_stride = w.div_ceil(2);
    dst.planes[2].pixel_stride = 1;

    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);

    match src.layout {
        CameraFrameLayout::I420 => {
            if src.plane_count < 3 {
                return false;
            }
            copy_strided_plane(src.planes[0], w, h, &mut dst.planes[0].bytes);
            copy_strided_plane(src.planes[1], cw, ch, &mut dst.planes[1].bytes);
            copy_strided_plane(src.planes[2], cw, ch, &mut dst.planes[2].bytes);
            true
        }
        CameraFrameLayout::NV12 => {
            if src.plane_count < 2 {
                return false;
            }
            copy_strided_plane(src.planes[0], w, h, &mut dst.planes[0].bytes);

            dst.planes[1].bytes.resize(cw * ch, 128);
            dst.planes[2].bytes.resize(cw * ch, 128);
            let uv = src.planes[1];
            for row in 0..ch {
                let src_row = row.saturating_mul(uv.row_stride);
                let dst_row = row * cw;
                for col in 0..cw {
                    let base = src_row + col.saturating_mul(uv.pixel_stride.max(2));
                    dst.planes[1].bytes[dst_row + col] = uv.bytes.get(base).copied().unwrap_or(128);
                    dst.planes[2].bytes[dst_row + col] =
                        uv.bytes.get(base + 1).copied().unwrap_or(128);
                }
            }
            true
        }
        CameraFrameLayout::YUY2 => {
            if src.plane_count < 1 {
                return false;
            }
            let packed = src.planes[0];
            dst.planes[0].bytes.resize(w * h, 16);
            dst.planes[1].bytes.resize(cw * ch, 128);
            dst.planes[2].bytes.resize(cw * ch, 128);

            let half_w = w.div_ceil(2);
            let mut u_full = vec![128u8; half_w * h];
            let mut v_full = vec![128u8; half_w * h];

            for row in 0..h {
                let src_row = row.saturating_mul(packed.row_stride);
                for pair in 0..half_w {
                    let base = src_row + pair * 4;
                    let y0 = packed.bytes.get(base).copied().unwrap_or(16);
                    let u = packed.bytes.get(base + 1).copied().unwrap_or(128);
                    let y1 = packed.bytes.get(base + 2).copied().unwrap_or(16);
                    let v = packed.bytes.get(base + 3).copied().unwrap_or(128);

                    let px0 = row * w + pair * 2;
                    if px0 < dst.planes[0].bytes.len() {
                        dst.planes[0].bytes[px0] = y0;
                    }
                    let px1 = px0 + 1;
                    if px1 < dst.planes[0].bytes.len() {
                        dst.planes[0].bytes[px1] = y1;
                    }

                    u_full[row * half_w + pair] = u;
                    v_full[row * half_w + pair] = v;
                }
            }

            for row in 0..ch {
                let r0 = row * 2;
                let r1 = (r0 + 1).min(h - 1);
                for col in 0..cw {
                    let u0 = u_full[r0 * half_w + col.min(half_w - 1)] as u16;
                    let u1 = u_full[r1 * half_w + col.min(half_w - 1)] as u16;
                    dst.planes[1].bytes[row * cw + col] = ((u0 + u1 + 1) / 2) as u8;

                    let v0 = v_full[r0 * half_w + col.min(half_w - 1)] as u16;
                    let v1 = v_full[r1 * half_w + col.min(half_w - 1)] as u16;
                    dst.planes[2].bytes[row * cw + col] = ((v0 + v1 + 1) / 2) as u8;
                }
            }
            true
        }
        _ => false,
    }
}
