//! Desk Link's encoder thread: the capture's picture texture (its encode surface) goes to
//! `makepad_video::VideoStreamEncoder` with the completion of the frame that rendered it, and each
//! encoded picture to every watching viewer. The UI thread only queues jobs (never waits).
//!
//! A GPU with several encode engines (`EncoderCaps::engines`) runs one stream per engine,
//! each on a worker thread of its own: as tiles (each picture split into bands, encoded at once:
//! the lowest latency, the default) or interleaved (the engines take whole pictures in turn:
//! the most pictures per second at one picture's latency). A picture larger than one stream
//! takes (`VideoStreamEncoder::max_size`, e.g. 4096 wide for H.264 on some GPUs) is cut into columns
//! as well (`TilePlan`); the tiles of a picture spread over all engines. Tiles of two pictures
//! may be in flight at once (each engine encodes its tiles in order), so a picture's encode
//! overlaps the next one's copy and submit.

use makepad_desk_link as proto;
use makepad_desk_link::tiles::{TileDone, TileEncoders, TileSink, TileSpec, TileWork};
use makepad_widgets::makepad_platform::video_file::{GpuCompletion, StreamTile, SurfaceHandle, StreamTuning, StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use makepad_desk_link::server::{Feed, Out};
pub use super::capture::{InFlight, TilePlan};
pub use super::settings::Split;

/// The viewers a picture goes to: their queue and their feed (backlog, pause, whether the
/// picture goes to them: `Feed::admit`).
pub type Subscribers = Arc<Mutex<Vec<(Sender<Out>, Arc<Feed>)>>>;

pub enum Job {
    /// `refine`: the last picture again at a near-lossless QP (the desktop held still). `held`:
    /// its texture is not drawn over until every engine's read of it is done (dropped then).
    /// `hevc`: H.265 (else H.264); `sharp`: the encoder's best pictures per bit (else its
    /// fastest).
    Picture { surface: SurfaceHandle, held: InFlight, size: [u32; 2], ready: GpuCompletion, built_ns: u64, fps: u32, kbps: u32, split: Split, full_chroma: bool, refine: bool, plan: TilePlan, hevc: bool, sharp: bool },
    Keyframe,
    /// The GPU device was replaced (`Event::GpuReset`): the encoder is opened again on the new
    /// one with the next picture, which starts with a key frame.
    Reset,
}

pub struct Encoder {
    tx: Sender<Job>,
}

/// The encoder's side of the per-second timeline (read and reset by the WM each second):
/// pictures whose every part is encoded, refinements among them, the slowest picture's frame
/// submit -> bitstream (µs).
#[derive(Default)]
pub struct EncodeStats {
    pub pictures: AtomicU64,
    pub refined: AtomicU64,
    pub encode_us_max: AtomicU64,
    /// The slowest tile's encode (the push into the encoder until its bitstream, µs) of every
    /// picture, summed, and the pictures: the adaptive rate's encoder-bound test.
    pub tile_us_sum: AtomicU64,
    pub tile_pictures: AtomicU64,
    /// The last picture sent out (its frame number): a viewer's `Stats` frame against it is how
    /// far that viewer is behind.
    pub last_frame: std::sync::atomic::AtomicU32,
}

impl Encoder {
    /// The thread; `wake` tells the UI about a failure (`note`s carry the text).
    pub fn start(subscribers: Subscribers, wake: Arc<dyn Fn() + Send + Sync>, stats: Arc<EncodeStats>) -> Result<Encoder, String> {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("desk-link-encode".into())
            .spawn(move || run(rx, subscribers, wake, stats))
            .map_err(|e| e.to_string())?;
        Ok(Encoder { tx })
    }

    pub fn send(&self, job: Job) {
        let _ = self.tx.send(job);
    }
}

/// One engine's stream: the part of each picture it takes (its worker is the tile fan-out's,
/// `makepad_desk_link::tiles`).
struct Stream {
    /// Its share of the picture's area: of the whole stream's rate.
    share: f64,
    /// Its tile's first column and row (tiles), or 0, 0 (whole pictures).
    x0: u32,
    y0: u32,
}

/// The streams of a shape: their parts and their workers (one tile each).
struct Streams {
    list: Vec<Stream>,
    workers: TileEncoders<(u32, u64)>,
}

/// What the streams are opened for: a picture of another shape opens them again.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shape {
    split: Split,
    full_chroma: bool,
    fps: u32,
    plan: TilePlan,
    hevc: bool,
    sharp: bool,
}

/// The tile messages of the pictures in flight, oldest first, with the slowest tile's encode (µs).
type Gathering = std::collections::VecDeque<(u32, Vec<Option<Vec<u8>>>, u64)>;

/// A worker's answer: (stream, frame, built, encoded, key, the encode's own time (µs), Annex-B
/// or the failure).
type Done = (usize, u32, u64, u64, bool, u64, Result<Vec<u8>, String>);

/// `total` kbit/s for a stream of `share` of the picture.
fn share_kbps(total: u32, share: f64) -> u32 {
    if total == 0 { 0 } else { ((total as f64 * share) as u32).max(1) }
}

/// A picture whose bitstream is no bigger than this per stream changed nothing visible (a redraw
/// of the same desktop): the desktop is still.
const STILL_BYTES: usize = 4096;
/// Still this long: the next picture is a refinement (near lossless).
const REFINE_AFTER: Duration = Duration::from_millis(120);

/// The streams for `size` with `split` on this machine's engines: each a worker thread owning
/// its encoder. Bands are whole macroblock rows except the last, so they cover the picture's rows
/// exactly once (pixel-exact seams).
fn open_streams(size: [u32; 2], fps: u32, kbps: u32, shape: Shape, done: &Sender<Done>) -> Result<(Streams, bool), String> {
    let Shape { split, full_chroma, plan, hevc, sharp, .. } = shape;
    let engines = VideoStreamEncoder::caps().engines.max(1);
    // tiles: one stream per tile, the engines in turn; whole pictures interleaved over the
    // engines, or one stream
    let tiles = plan.tiles() > 1;
    let n = if tiles { plan.tiles() } else if split == Split::Interleave && size[1] >= 64 { engines } else { 1 } as usize;
    let cols = TilePlan::columns(size[0], plan.cols);
    let rows = TilePlan::bands(size[1], plan.rows);
    let mut list = Vec::with_capacity(n);
    let mut specs = Vec::with_capacity(n);
    for i in 0..n {
        let ((x0, w), (y0, h)) = if tiles { (cols[i % cols.len()], rows[i / cols.len()]) } else { ((0, size[0]), (0, size[1])) };
        // an interleaved stream carries every n-th picture
        let stream_fps = if tiles { fps } else { (fps / n as u32).max(1) };
        // the whole stream's rate by area: a tile its own share, interleaved streams each the
        // whole rate at their share of the pictures
        let share = if tiles { (w as f64 * h as f64) / (size[0] as f64 * size[1] as f64).max(1.0) } else { 1.0 / n as f64 };
        let codec = if hevc { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
        let options = VideoStreamEncoderOptions { codec, width: w, height: h, fps: stream_fps, bitrate_kbps: share_kbps(kbps, share), keyint: 0, low_latency: true, full_chroma };
        let tile = StreamTile { engine: i as u32 % engines, x0, y0, source_height: size[1] };
        specs.push(TileSpec { options, tile, tuning: StreamTuning { sharp, ..StreamTuning::default() } });
        list.push(Stream { share, x0, y0 });
    }
    let done = done.clone();
    let sink: TileSink<(u32, u64)> = Arc::new(move |d: TileDone<(u32, u64)>| {
        let took_us = d.push().as_micros() as u64;
        let out = d.packets.map(|packets| packets.into_iter().flat_map(|p| p.data).collect::<Vec<u8>>());
        let _ = done.send((d.tile, d.tag.0, d.tag.1, d.encoded_ns, d.key, took_us, out));
    });
    let workers = TileEncoders::open("desk-link-encode", specs, sink)?;
    for i in 0..n {
        let (tile, [w, h]) = workers.tile(i);
        let where_ = if n == 1 { String::new() } else if tiles { format!(" (tile {},{} {w}x{h} of {}x{}, engine {})", tile.x0, tile.y0, plan.cols, plan.rows, tile.engine) } else { " (interleaved)".into() };
        super::note(&format!("desk link stream {}/{n}{where_}: {}", i + 1, workers.description(i)));
    }
    Ok((Streams { list, workers }, tiles))
}

fn run(rx: Receiver<Job>, subscribers: Subscribers, wake: Arc<dyn Fn() + Send + Sync>, stats: Arc<EncodeStats>) {
    // the streams, for (size, split); dropping them ends their workers
    let mut streams: Option<(Streams, [u32; 2], Shape, bool)> = None;
    let (done_tx, done_rx) = channel::<Done>();
    let mut in_flight = 0usize;
    // the tiles of the pictures being encoded (at most two): each picture's frame and parts
    let mut gathering: Gathering = std::collections::VecDeque::new();
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
    // a picture goes to every viewer that follows the stream (one behind while others keep up
    // skips pictures until it starts again at a key frame)
    let send = |frame: u32, key: bool, bytes: Vec<u8>| {
        for (out, feed) in subscribers.lock().unwrap().iter() {
            if !feed.admit(frame, key) {
                continue;
            }
            feed.backlog.fetch_add(1, Ordering::AcqRel);
            let _ = out.send(Out::Video(bytes.clone()));
        }
    };
    // a worker's answer: a tile waits for its picture's other tiles, an interleaved picture goes
    // the last picture that changed something, and since which picture the still desktop is
    // encoded near lossless (it stays so until a picture changes something)
    let last_change = std::cell::Cell::new(Instant::now());
    let refining: std::cell::Cell<Option<u32>> = std::cell::Cell::new(None);
    let take = |d: Done, streams: &Option<(Streams, [u32; 2], Shape, bool)>, gathering: &mut Gathering, failed: &mut bool, fail: &mut dyn FnMut(String)| {
        let (i, frame, built_ns, encoded_ns, key, took_us, out) = d;
        let Some((Streams { list, .. }, _, shape, tiles)) = streams else { return };
        let codec = if shape.hevc { proto::pref::HEVC } else { proto::pref::H264 };
        let parts = if *tiles { list.len() } else { 1 };
        match out {
            Ok(data) => {
                // a big picture changed something; the first near-lossless one is the refinement
                // itself (big too)
                if data.len() > STILL_BYTES && !key && refining.get().is_none_or(|since| frame > since) {
                    last_change.set(Instant::now());
                    refining.set(None);
                }
                let msg = proto::encode_video(frame, key, built_ns, encoded_ns, 0, (i as u8, parts as u8, list[i].x0, list[i].y0), codec, &data);
                stats.encode_us_max.fetch_max(encoded_ns.saturating_sub(built_ns) / 1000, Ordering::Relaxed);
                let complete = |stats: &EncodeStats, took_us: u64| {
                    stats.pictures.fetch_add(1, Ordering::Relaxed);
                    stats.tile_us_sum.fetch_add(took_us, Ordering::Relaxed);
                    stats.tile_pictures.fetch_add(1, Ordering::Relaxed);
                    stats.last_frame.store(frame, Ordering::Relaxed);
                    if refining.get().is_some() {
                        stats.refined.fetch_add(1, Ordering::Relaxed);
                    }
                };
                if parts == 1 {
                    send(frame, key, msg);
                    complete(&stats, took_us);
                } else {
                    // each engine encodes in order: the oldest picture completes first
                    let Some((_, slot, slowest)) = gathering.iter_mut().find(|(f, _, _)| *f == frame) else { return };
                    slot[i] = Some(msg);
                    *slowest = (*slowest).max(took_us);
                    if gathering.front().is_some_and(|(_, parts, _)| parts.iter().all(Option::is_some)) {
                        let (_, parts, slowest) = gathering.pop_front().unwrap();
                        let all: Vec<u8> = parts.into_iter().flat_map(Option::unwrap).collect();
                        send(frame, key, all);
                        complete(&stats, slowest);
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
            Job::Reset => {
                streams = None;
                key_wanted = true;
            }
            Job::Picture { surface, held, size, ready, built_ns, fps, kbps, split, full_chroma, refine, plan, hevc, sharp } => {
                let shape = Shape { split, full_chroma, fps, plan, hevc, sharp };
                let held: Arc<dyn Send + Sync> = Arc::new(held);
                // still (no picture, or pictures that change nothing) for REFINE_AFTER: near-lossless
                // pictures from this one on, until one changes something
                if refining.get().is_none() && (refine || last_change.get().elapsed() >= REFINE_AFTER) {
                    refining.set(Some(frame));
                }
                let refine = refining.get().is_some();
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
                if streams.as_ref().is_none_or(|(_, s, sp, _)| *s != size || *sp != shape) {
                    while in_flight > 0 {
                        let Ok(d) = done_rx.recv() else { return };
                        in_flight -= 1;
                        take(d, &streams, &mut gathering, &mut failed, &mut fail);
                    }
                    streams = None;
                    match open_streams(size, fps, kbps, shape, &done_tx) {
                        Ok((list, tiles)) => {
                            gathering.clear();
                            streams = Some((list, size, shape, tiles));
                            key_wanted = true;
                        }
                        Err(e) => {
                            fail(format!("desk link stream: {e}"));
                            wake();
                            continue;
                        }
                    }
                }
                let (Streams { list, workers }, _, _, tiles) = streams.as_ref().unwrap();
                let tiles = *tiles;
                if std::mem::take(&mut key_wanted) {
                    keys_left = if tiles { 1 } else { list.len() };
                }
                let key = keys_left > 0;
                keys_left = keys_left.saturating_sub(1);
                if tiles {
                    // at most two pictures in flight: the oldest completes before a third starts
                    let n = list.len();
                    while in_flight > n {
                        let Ok(d) = done_rx.recv() else { return };
                        in_flight -= 1;
                        take(d, &streams, &mut gathering, &mut failed, &mut fail);
                    }
                    // every engine on its tiles of this picture at once
                    gathering.push_back((frame, vec![None; n], 0));
                    for (i, s) in list.iter().enumerate() {
                        workers.push(i, TileWork { surface: surface.clone(), held: held.clone(), ready: ready.clone(), kbps: share_kbps(kbps, s.share), key, refine, pts_100ns: frame as i64, tag: (frame, built_ns) });
                        in_flight += 1;
                    }
                    while let Ok(d) = done_rx.try_recv() {
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
                    let i = frame as usize % n;
                    workers.push(i, TileWork { surface, held, ready, kbps: share_kbps(kbps, list[i].share), key, refine, pts_100ns: frame as i64, tag: (frame, built_ns) });
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
