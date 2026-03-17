use {
    makepad_platform::{
        CameraColorMatrix, CameraFrameLayout, CameraFramePlaneRef, CameraFrameRef,
        MediaVideoEncoder, VideoEncodeError,
    },
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant},
    },
};

pub struct DummyVideoEncoder {
    running: Arc<AtomicBool>,
    worker: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl DummyVideoEncoder {
    pub fn with_inner(
        inner: Box<dyn MediaVideoEncoder>,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
    ) -> Option<Self> {
        if width == 0 || height == 0 || fps_num == 0 || fps_den == 0 {
            return None;
        }
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = running.clone();
        let worker = thread::Builder::new()
            .name("dummy-video-encoder".to_string())
            .spawn(move || run_dummy_loop(inner, worker_running, width as usize, height as usize, fps_num, fps_den))
            .ok()?;
        Some(Self {
            running,
            worker: std::sync::Mutex::new(Some(worker)),
        })
    }

    fn stop_worker(&self) {
        if self.running.swap(false, Ordering::SeqCst) {
            if let Some(worker) = self.worker.lock().unwrap().take() {
                let _ = worker.join();
            }
        }
    }
}

fn run_dummy_loop(
    inner: Box<dyn MediaVideoEncoder>,
    running: Arc<AtomicBool>,
    width: usize,
    height: usize,
    fps_num: u32,
    fps_den: u32,
) {
    let frame_duration = Duration::from_secs_f64(fps_den as f64 / fps_num as f64);
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let mut frame_index = 0u64;
    let start = Instant::now();

    while running.load(Ordering::Relaxed) {
        let mut y = vec![0u8; width * height];
        for row in 0..height {
            let row_off = row * width;
            for col in 0..width {
                y[row_off + col] = ((col + (frame_index as usize * 3)) % 256) as u8;
            }
        }
        let u = vec![64u8.wrapping_add((frame_index as usize % 64) as u8); cw * ch];
        let v = vec![192u8.wrapping_sub((frame_index as usize % 64) as u8); cw * ch];
        let frame = CameraFrameRef {
            timestamp_ns: start.elapsed().as_nanos() as u64,
            width,
            height,
            layout: CameraFrameLayout::I420,
            matrix: CameraColorMatrix::BT601,
            plane_count: 3,
            planes: [
                CameraFramePlaneRef { bytes: &y, row_stride: width, pixel_stride: 1 },
                CameraFramePlaneRef { bytes: &u, row_stride: cw, pixel_stride: 1 },
                CameraFramePlaneRef { bytes: &v, row_stride: cw, pixel_stride: 1 },
            ],
        };
        inner.push_frame(frame);
        frame_index += 1;
        thread::sleep(frame_duration);
    }
    inner.stop();
}

impl MediaVideoEncoder for DummyVideoEncoder {
    fn push_frame(&self, _frame: CameraFrameRef<'_>) {}

    fn request_keyframe(&self) -> Result<(), VideoEncodeError> {
        Ok(())
    }

    fn stop(&self) {
        self.stop_worker();
    }
}

impl Drop for DummyVideoEncoder {
    fn drop(&mut self) {
        self.stop_worker();
    }
}
