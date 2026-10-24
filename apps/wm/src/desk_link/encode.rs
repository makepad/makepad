//! Desk Link's encoder thread: the capture's picture texture (registered as a DMA-BUF) goes to
//! `makepad_video::VideoStreamEncoder` with the fence of the frame that rendered it, and each
//! encoded picture to every watching viewer. The UI thread only queues jobs (never waits).

use makepad_desk_link as proto;
use makepad_widgets::makepad_platform::video_file::{GpuReady, StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use makepad_desk_link::server::Out;

/// The viewers a picture goes to: their queue, its backlog, whether they pause.
pub type Subscribers = Arc<Mutex<Vec<(Sender<Out>, Arc<AtomicUsize>, Arc<AtomicBool>)>>>;

/// The release fence of the frame that rendered a picture (Linux), or nothing to wait for.
pub enum Fence {
    #[cfg(target_os = "linux")]
    Linux(makepad_widgets::makepad_platform::foreign_dmabuf::ForeignDmabufFence),
    None,
}

pub enum Job {
    Picture { handle: u64, size: [u32; 2], fence: Fence, built_ns: u64, fps: u32, kbps: u32 },
    Keyframe,
    Bitrate(u32),
    /// The picture texture's registration `handle` is released (after the pictures queued
    /// before it were encoded).
    Release(u64),
    /// The GPU device was replaced (`Event::GpuReset`): the encoder is opened again on the new
    /// one with the next picture, which starts with a key frame.
    Reset,
}

pub struct Encoder {
    tx: Sender<Job>,
}

impl Encoder {
    /// The thread; `wake` tells the UI about a failure (`note`s carry the text).
    pub fn start(subscribers: Subscribers, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Encoder, String> {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("desk-link-encode".into())
            .spawn(move || run(rx, subscribers, wake))
            .map_err(|e| e.to_string())?;
        Ok(Encoder { tx })
    }

    pub fn send(&self, job: Job) {
        let _ = self.tx.send(job);
    }
}

/// The fence's sync_file once published (the frame was submitted), waiting briefly for it.
fn ready(fence: Fence) -> Result<GpuReady, String> {
    match fence {
        Fence::None => Ok(GpuReady::Done),
        #[cfg(target_os = "linux")]
        Fence::Linux(mut fence) => {
            let started = Instant::now();
            loop {
                match fence.poll() {
                    Some(Ok(Some(fd))) => return Ok(GpuReady::SyncFile(fd)),
                    Some(Ok(None)) => return Ok(GpuReady::Done),
                    Some(Err(e)) => return Err(format!("frame fence: {e}")),
                    None if started.elapsed() > Duration::from_secs(2) => return Err("the frame's fence was never published".into()),
                    None => std::thread::sleep(Duration::from_micros(200)),
                }
            }
        }
    }
}

fn run(rx: Receiver<Job>, subscribers: Subscribers, wake: Arc<dyn Fn() + Send + Sync>) {
    let mut encoder: Option<(VideoStreamEncoder, [u32; 2])> = None;
    let mut frame: u32 = 0;
    let mut key_wanted = false;
    // the last failure noted and when: the same one again is noted at most every 10 s
    let mut last_failure: Option<(String, Instant)> = None;
    let mut fail = |text: String| {
        if last_failure.as_ref().is_none_or(|(t, at)| *t != text || at.elapsed() > Duration::from_secs(10)) {
            super::note(&text);
            last_failure = Some((text, Instant::now()));
        }
    };
    while let Ok(job) = rx.recv() {
        match job {
            Job::Release(_handle) => {
                #[cfg(target_os = "linux")]
                makepad_hal::dmabuf::hal_dmabuf_release(_handle);
            }
            Job::Reset => {
                encoder = None;
                key_wanted = true;
            }
            Job::Keyframe => {
                key_wanted = true;
                if let Some((e, _)) = encoder.as_mut() {
                    e.request_keyframe();
                }
            }
            Job::Bitrate(kbps) => {
                if let Some((e, _)) = encoder.as_mut() {
                    e.set_bitrate_kbps(kbps);
                }
            }
            Job::Picture { handle, size, fence, built_ns, fps, kbps } => {
                if encoder.as_ref().is_none_or(|(_, s)| *s != size) {
                    let options = VideoStreamEncoderOptions { codec: StreamVideoCodec::H264, width: size[0], height: size[1], fps, bitrate_kbps: kbps, keyint: 0, low_latency: true };
                    match VideoStreamEncoder::new(options) {
                        Ok(e) => {
                            super::note(&format!("desk link stream: {}", e.description()));
                            encoder = Some((e, size));
                        }
                        Err(e) => {
                            fail(format!("desk link stream: {e:?}"));
                            encoder = None;
                            wake();
                            continue;
                        }
                    }
                }
                let ready = match ready(fence) {
                    Ok(r) => r,
                    Err(e) => {
                        fail(format!("desk link stream: {e}"));
                        continue;
                    }
                };
                let (enc, _) = encoder.as_mut().unwrap();
                if std::mem::take(&mut key_wanted) {
                    enc.request_keyframe();
                }
                match enc.push_shared_texture(handle, ready, frame as i64) {
                    Ok(packets) => {
                        let encoded_ns = makepad_hal::clock_ns();
                        for p in packets {
                            let bytes = proto::encode_video(frame, p.is_key, built_ns, encoded_ns, 0, &p.data);
                            for (out, backlog, paused) in subscribers.lock().unwrap().iter() {
                                if paused.load(Ordering::Acquire) {
                                    continue;
                                }
                                backlog.fetch_add(1, Ordering::AcqRel);
                                let _ = out.send(Out::Video(bytes.clone()));
                            }
                        }
                        frame = frame.wrapping_add(1);
                    }
                    Err(e) => {
                        // opened again with the next picture (a lost or replaced GPU device ends
                        // an encoder for good), which is a key frame
                        fail(format!("desk link stream: picture {frame}: {e:?}; the encoder is opened again"));
                        encoder = None;
                        key_wanted = true;
                        wake();
                    }
                }
            }
        }
    }
}
