//! Desk Link's encoder thread: the capture's picture texture (registered as a DMA-BUF) goes to
//! `makepad_video::VideoStreamEncoder` with the fence of the frame that rendered it, and each
//! encoded picture to every watching viewer. The UI thread only queues jobs (never waits).
//!
//! A GPU with several encode engines (`VideoStreamEncoder::engines`) runs one stream per engine,
//! each on a worker thread of its own: as tiles (each picture split into bands, encoded at once:
//! the lowest latency, the default) or interleaved (the engines take whole pictures in turn:
//! the most pictures per second at one picture's latency).

use makepad_desk_link as proto;
use makepad_widgets::makepad_platform::video_file::{GpuReady, StreamTile, StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use makepad_desk_link::server::Out;
pub use super::settings::Split;

/// The viewers a picture goes to: their queue, its backlog, whether they pause.
pub type Subscribers = Arc<Mutex<Vec<(Sender<Out>, Arc<AtomicUsize>, Arc<AtomicBool>)>>>;

/// The release fence of the frame that rendered a picture (Linux), or nothing to wait for.
pub enum Fence {
    #[cfg(target_os = "linux")]
    Linux(makepad_widgets::makepad_platform::foreign_dmabuf::ForeignDmabufFence),
    None,
}

pub enum Job {
    Picture { handle: u64, size: [u32; 2], fence: Fence, built_ns: u64, fps: u32, kbps: u32, split: Split },
    Keyframe,
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

/// One engine's stream: its encoder, the part of each picture it takes, and its worker.
struct Stream {
    tx: Sender<Work>,
    /// First row and rows of its band (tiles), or the whole picture.
    y0: u32,
}

struct Work {
    handle: u64,
    ready: GpuReady,
    frame: u32,
    key: bool,
    built_ns: u64,
}

/// A worker's answer: (stream, frame, built, encoded, key, Annex-B or the failure).
type Done = (usize, u32, u64, u64, bool, Result<Vec<u8>, String>);

/// The streams for `size` with `split` on this machine's engines: each a worker thread owning
/// its encoder. Bands are whole macroblock rows except the last, so they cover the picture's rows
/// exactly once (pixel-exact seams).
fn open_streams(size: [u32; 2], fps: u32, kbps: u32, split: Split, done: &Sender<Done>) -> Result<Vec<Stream>, String> {
    let engines = VideoStreamEncoder::engines().max(1) as usize;
    let n = if split == Split::Off || size[1] < 64 { 1 } else { engines };
    let tiles = split == Split::Tiles && n > 1;
    let band = (size[1] / n as u32) / 16 * 16;
    let kbps = if kbps == 0 { 0 } else { (kbps / n as u32).max(1) };
    let mut streams = Vec::with_capacity(n);
    for i in 0..n {
        let (y0, rows) = if tiles { (band * i as u32, if i + 1 == n { size[1] - band * i as u32 } else { band }) } else { (0, size[1]) };
        // an interleaved stream carries every n-th picture
        let stream_fps = if tiles { fps } else { (fps / n as u32).max(1) };
        let options = VideoStreamEncoderOptions { codec: StreamVideoCodec::H264, width: size[0], height: rows, fps: stream_fps, bitrate_kbps: kbps, keyint: 0, low_latency: true };
        let tile = StreamTile { engine: i as u32, y0, source_height: size[1] };
        let mut encoder = VideoStreamEncoder::new_tile(options, tile).map_err(|e| format!("{e:?}"))?;
        super::note(&format!("desk link stream {}/{n}{}: {}", i + 1, if n == 1 { "" } else if tiles { " (tiles)" } else { " (interleaved)" }, encoder.description()));
        let (tx, rx) = channel::<Work>();
        let done = done.clone();
        std::thread::Builder::new()
            .name(format!("desk-link-encode-{i}"))
            .spawn(move || {
                while let Ok(w) = rx.recv() {
                    if w.key {
                        encoder.request_keyframe();
                    }
                    let out = encoder.push_shared_texture(w.handle, w.ready, w.frame as i64).map_err(|e| format!("{e:?}")).map(|packets| packets.into_iter().flat_map(|p| p.data).collect::<Vec<u8>>());
                    let key = w.key;
                    if done.send((i, w.frame, w.built_ns, makepad_hal::clock_ns(), key, out)).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        streams.push(Stream { tx, y0 });
    }
    Ok(streams)
}

fn run(rx: Receiver<Job>, subscribers: Subscribers, wake: Arc<dyn Fn() + Send + Sync>) {
    // the streams, for (size, split); dropping them ends their workers
    let mut streams: Option<(Vec<Stream>, [u32; 2], Split, bool)> = None;
    let (done_tx, done_rx) = channel::<Done>();
    let mut in_flight = 0usize;
    // the tiles of the picture being encoded: (frame, parts received, messages)
    let mut gathering: Vec<Option<Vec<u8>>> = Vec::new();
    let mut frame: u32 = 0;
    let mut key_wanted = true;
    // interleaved streams each need a key frame: the next this many pictures are key frames
    let mut keys_left = 0usize;
    let mut failed = false;
    // the last failure noted and when: the same one again is noted at most every 10 s
    let mut last_failure: Option<(String, Instant)> = None;
    let mut fail = |text: String| {
        if last_failure.as_ref().is_none_or(|(t, at)| *t != text || at.elapsed() > Duration::from_secs(10)) {
            super::note(&text);
            last_failure = Some((text, Instant::now()));
        }
    };
    let send = |bytes: Vec<u8>| {
        for (out, backlog, paused) in subscribers.lock().unwrap().iter() {
            if paused.load(Ordering::Acquire) {
                continue;
            }
            backlog.fetch_add(1, Ordering::AcqRel);
            let _ = out.send(Out::Video(bytes.clone()));
        }
    };
    // a worker's answer: a tile waits for its picture's other tiles, an interleaved picture goes
    let take = |d: Done, streams: &Option<(Vec<Stream>, [u32; 2], Split, bool)>, gathering: &mut Vec<Option<Vec<u8>>>, failed: &mut bool, fail: &mut dyn FnMut(String)| {
        let (i, frame, built_ns, encoded_ns, key, out) = d;
        let Some((list, _, _, tiles)) = streams else { return };
        let parts = if *tiles { list.len() } else { 1 };
        match out {
            Ok(data) => {
                let msg = proto::encode_video(frame, key, built_ns, encoded_ns, 0, (i as u8, parts as u8, list[i].y0), &data);
                if parts == 1 {
                    send(msg);
                } else {
                    gathering[i] = Some(msg);
                    if gathering.iter().all(Option::is_some) {
                        let all: Vec<u8> = gathering.iter_mut().flat_map(|m| m.take().unwrap()).collect();
                        send(all);
                    }
                }
            }
            Err(e) => {
                fail(format!("desk link stream: picture {frame}: {e}; the encoders are opened again"));
                *failed = true;
            }
        }
    };
    loop {
        let job = match rx.recv() {
            Ok(job) => job,
            Err(_) => return,
        };
        // the jobs other than pictures wait for the pictures before them
        if !matches!(job, Job::Picture { .. }) {
            while in_flight > 0 {
                let Ok(d) = done_rx.recv() else { return };
                in_flight -= 1;
                take(d, &streams, &mut gathering, &mut failed, &mut fail);
            }
        }
        match job {
            Job::Keyframe => key_wanted = true,
            Job::Release(_handle) => {
                #[cfg(target_os = "linux")]
                makepad_hal::dmabuf::hal_dmabuf_release(_handle);
            }
            Job::Reset => {
                streams = None;
                key_wanted = true;
            }
            Job::Picture { handle, size, fence, built_ns, fps, kbps, split } => {
                if std::mem::take(&mut failed) {
                    // a failed or device-replaced encoder is opened again (its next picture a key frame)
                    while in_flight > 0 {
                        let Ok(d) = done_rx.recv() else { return };
                        in_flight -= 1;
                        take(d, &streams, &mut gathering, &mut failed, &mut fail);
                    }
                    failed = false;
                    streams = None;
                    key_wanted = true;
                    wake();
                }
                if streams.as_ref().is_none_or(|(_, s, sp, _)| *s != size || *sp != split) {
                    while in_flight > 0 {
                        let Ok(d) = done_rx.recv() else { return };
                        in_flight -= 1;
                        take(d, &streams, &mut gathering, &mut failed, &mut fail);
                    }
                    streams = None;
                    match open_streams(size, fps, kbps, split, &done_tx) {
                        Ok(list) => {
                            let tiles = split == Split::Tiles && list.len() > 1;
                            gathering = vec![None; if tiles { list.len() } else { 0 }];
                            streams = Some((list, size, split, tiles));
                            key_wanted = true;
                        }
                        Err(e) => {
                            fail(format!("desk link stream: {e}"));
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
                let (list, _, _, tiles) = streams.as_ref().unwrap();
                let tiles = *tiles;
                if std::mem::take(&mut key_wanted) {
                    keys_left = if tiles { 1 } else { list.len() };
                }
                let key = keys_left > 0;
                keys_left = keys_left.saturating_sub(1);
                if tiles {
                    // every engine on its band of this picture at once
                    let mut readies = vec![ready];
                    for _ in 1..list.len() {
                        readies.push(clone_ready(&readies[0]));
                    }
                    for (s, ready) in list.iter().zip(readies) {
                        let _ = s.tx.send(Work { handle, ready, frame, key, built_ns });
                        in_flight += 1;
                    }
                    // a picture's tiles are complete before the next picture starts
                    while in_flight > 0 {
                        let Ok(d) = done_rx.recv() else { return };
                        in_flight -= 1;
                        take(d, &streams, &mut gathering, &mut failed, &mut fail);
                    }
                } else {
                    // the engines in turn; at most one picture in flight on each
                    let n = list.len();
                    while in_flight >= n {
                        let Ok(d) = done_rx.recv() else { return };
                        in_flight -= 1;
                        take(d, &streams, &mut gathering, &mut failed, &mut fail);
                    }
                    let _ = list[frame as usize % n].tx.send(Work { handle, ready, frame, key, built_ns });
                    in_flight += 1;
                    // answers that came meanwhile go out at once
                    while let Ok(d) = done_rx.try_recv() {
                        in_flight -= 1;
                        take(d, &streams, &mut gathering, &mut failed, &mut fail);
                    }
                }
                frame = frame.wrapping_add(1);
            }
        }
    }
}

/// The same readiness for another engine (the sync_file duplicated).
fn clone_ready(ready: &GpuReady) -> GpuReady {
    match ready {
        GpuReady::Done => GpuReady::Done,
        #[cfg(target_os = "linux")]
        GpuReady::SyncFile(fd) => fd.try_clone().map(GpuReady::SyncFile).unwrap_or(GpuReady::Done),
    }
}
