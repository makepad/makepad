//! Scrubbing a [`Snapshot`]: a decode thread of its own (its own hardware decoders, the live
//! ones untouched) shows any picture of the history and keeps the pictures around the shown one
//! decoded, so a step back is as quick as a step forward.
//!
//! A picture decodes from its key run on (or on from the decoders' last picture, when that is
//! before it and nearer). Decoded pictures stay in a cache bounded in bytes; the ones farthest
//! from the shown picture go first (a picture after it counts three times as far: forward is a
//! cheap decode from where the decoders are, back is a decode from the key run). While the
//! user looks, the thread fills the cache around the shown picture: a little ahead, mostly
//! behind.

use crate::conn::Part;
use crate::history::Snapshot;
use makepad_network::SignalToUI;
use makepad_widgets::makepad_platform::video_file::{DecodedPicture, VideoColor, VideoStreamDecoder};
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};

/// The decoded pictures kept around the shown one (bytes of picture memory).
pub const CACHE_BYTES: usize = 2 << 30;

/// One decoded part (a tile, or the whole picture).
pub struct DecPart {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub gpu: Option<DecodedPicture>,
    pub nv12: Vec<u8>,
    pub color: VideoColor,
}

pub struct Decoded {
    pub index: usize,
    pub width: u32,
    pub height: u32,
    pub parts: Vec<DecPart>,
}

impl Decoded {
    /// The parts as the view takes them (GPU pictures shared, CPU NV12 copied).
    pub fn view_parts(&self) -> Vec<Part> {
        self.parts.iter().map(|p| Part { x0: p.x0, y0: p.y0, width: p.width, height: p.height, nv12: p.nv12.clone(), gpu: p.gpu.clone(), color: p.color }).collect()
    }

    fn bytes(&self) -> usize {
        self.parts
            .iter()
            .map(|p| {
                let px = p.width as usize * p.height as usize;
                match &p.gpu {
                    Some(g) if g.full_chroma => px * 3,
                    Some(_) => px * 3 / 2,
                    None => p.nv12.len(),
                }
            })
            .sum()
    }
}

/// Thumbnails kept (key pictures, decoded whole).
const THUMBS: usize = 16;

#[derive(Default)]
struct State {
    /// The pictures around the target are decoded and kept (false: a preview of a live history,
    /// only thumbnails).
    fill: bool,
    target: usize,
    /// The user steps back (the last move went to an earlier picture): fill behind first.
    backward: bool,
    stop: bool,
    cache: BTreeMap<usize, Arc<Decoded>>,
    cache_bytes: usize,
    /// One picture's size in the cache (from the first decoded one).
    picture_bytes: usize,
    /// The target's decode, for the UI to take.
    ready: Option<Arc<Decoded>>,
    /// The picture the UI shows (taken from `ready` or from the cache at `seek`).
    shown: Option<usize>,
    /// Pictures that did not decode: why.
    failed: BTreeMap<usize, String>,
    /// What the thread is doing, for the status line.
    work: String,
    /// The thumbnail asked for (a key picture: one decode from nothing), the ones decoded, and
    /// the one for the UI to take.
    thumb_want: Option<usize>,
    thumbs: BTreeMap<usize, Arc<Decoded>>,
    thumb_ready: Option<Arc<Decoded>>,
}

impl State {
    fn distance(&self, i: usize) -> usize {
        if i > self.target {
            (i - self.target) * 3
        } else {
            self.target - i
        }
    }

    fn capacity(&self) -> usize {
        (CACHE_BYTES / self.picture_bytes.max(1)).max(4)
    }

    /// Keeps `d` if the cache has room once farther pictures are dropped.
    fn insert(&mut self, d: Arc<Decoded>) {
        let bytes = d.bytes();
        if self.picture_bytes == 0 {
            self.picture_bytes = bytes;
        }
        let mine = self.distance(d.index);
        while self.cache_bytes + bytes > CACHE_BYTES {
            let first = self.cache.keys().next().copied();
            let last = self.cache.keys().next_back().copied();
            let far = match (first, last) {
                (Some(a), Some(b)) => if self.distance(a) >= self.distance(b) { a } else { b },
                _ => break,
            };
            if self.distance(far) <= mine {
                return;
            }
            let gone = self.cache.remove(&far).unwrap();
            self.cache_bytes -= gone.bytes();
        }
        self.cache_bytes += bytes;
        if let Some(old) = self.cache.insert(d.index, d) {
            self.cache_bytes -= old.bytes();
        }
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

pub struct Scrub {
    pub snap: Arc<Snapshot>,
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Scrub {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().stop = true;
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Scrub {
    pub fn start(snap: Arc<Snapshot>, at: usize, signal: SignalToUI) -> Scrub {
        Self::spawn(snap, State { target: at, fill: true, ..State::default() }, signal)
    }

    /// Thumbnails only (the scrubber over a live history: nothing is decoded until a moment
    /// is hovered).
    pub fn preview(snap: Arc<Snapshot>, signal: SignalToUI) -> Scrub {
        let at = snap.len().saturating_sub(1);
        Self::spawn(snap, State { target: at, fill: false, ..State::default() }, signal)
    }

    fn spawn(snap: Arc<Snapshot>, state: State, signal: SignalToUI) -> Scrub {
        let shared = Arc::new(Shared { state: Mutex::new(state), wake: Condvar::new() });
        let (s, sh) = (snap.clone(), shared.clone());
        let thread = std::thread::Builder::new().name("desklink-scrub".into()).spawn(move || Engine::new(s, sh, signal).run()).ok();
        Scrub { snap, shared, thread }
    }

    /// Show picture `i`: its decode at once when it is cached (the step is instant), else later
    /// through [`Self::take_ready`].
    pub fn seek(&self, i: usize) -> Option<Arc<Decoded>> {
        let i = i.min(self.snap.len().saturating_sub(1));
        let mut s = self.shared.state.lock().unwrap();
        if i != s.target {
            s.backward = i < s.target;
        }
        s.target = i;
        let hit = s.cache.get(&i).cloned();
        if hit.is_some() {
            s.shown = Some(i);
            s.ready = None;
        }
        drop(s);
        self.shared.wake.notify_all();
        hit
    }

    /// A thumbnail of picture `i`: the picture itself when it is decoded already, else the key
    /// picture its run starts from (one decode), later through [`Self::take_thumb`].
    pub fn want_thumb(&self, i: usize) {
        let Some(p) = self.snap.pictures.get(i) else { return };
        let mut s = self.shared.state.lock().unwrap();
        if let Some(d) = s.cache.get(&i).cloned() {
            s.thumb_ready = Some(d);
            s.thumb_want = None;
            return;
        }
        let Some(key) = p.sync else { return };
        if let Some(d) = s.thumbs.get(&key).cloned() {
            s.thumb_ready = Some(d);
            s.thumb_want = None;
            return;
        }
        s.thumb_want = Some(key);
        drop(s);
        self.shared.wake.notify_all();
    }

    pub fn take_thumb(&self) -> Option<Arc<Decoded>> {
        self.shared.state.lock().unwrap().thumb_ready.take()
    }

    pub fn take_ready(&self) -> Option<Arc<Decoded>> {
        let mut s = self.shared.state.lock().unwrap();
        let d = s.ready.take()?;
        if d.index != s.target {
            return None;
        }
        s.shown = Some(d.index);
        Some(d)
    }

    /// The cache around the shown picture (`first..=last` decoded without a hole, counted from
    /// the target), what the thread does, and why the target failed (if it did).
    pub fn status(&self) -> (usize, usize, String, Option<String>) {
        let s = self.shared.state.lock().unwrap();
        let t = s.target;
        let mut lo = t;
        while lo > 0 && s.cache.contains_key(&(lo - 1)) {
            lo -= 1;
        }
        let mut hi = t;
        while s.cache.contains_key(&(hi + 1)) {
            hi += 1;
        }
        let (behind, ahead) = if s.cache.contains_key(&t) { (t - lo, hi - t) } else { (0, 0) };
        (behind, ahead, s.work.clone(), s.failed.get(&t).cloned())
    }
}

struct Engine {
    snap: Arc<Snapshot>,
    shared: Arc<Shared>,
    signal: SignalToUI,
    /// Per stream: its decoder and the codec it decodes (`pref::*`).
    decoders: Vec<Option<(VideoStreamDecoder, u8)>>,
    /// The last picture decoded (the decoders' state is after it).
    at: Option<usize>,
}

enum Job {
    /// Decode up to `upto`, keeping the pictures from `keep_from` on.
    Decode { upto: usize, keep_from: usize },
    /// A key picture's thumbnail.
    Thumb(usize),
    Idle,
}

impl Engine {
    fn new(snap: Arc<Snapshot>, shared: Arc<Shared>, signal: SignalToUI) -> Engine {
        Engine { snap, shared, signal, decoders: Vec::new(), at: None }
    }

    fn plan(&self, s: &mut State) -> Job {
        let n = self.snap.len();
        if n == 0 {
            return Job::Idle;
        }
        let t = s.target.min(n - 1);
        let cap = s.capacity();
        let back = cap * 3 / 4;
        let ahead = (cap / 4).max(2);
        let decodes = |i: usize| self.snap.pictures[i].sync.is_some() && !s.failed.contains_key(&i);
        let thumb = s.thumb_want.filter(|k| *k < n && !s.thumbs.contains_key(k) && decodes(*k));
        if !s.fill {
            s.thumb_want = thumb;
            return thumb.map_or(Job::Idle, Job::Thumb);
        }
        if !s.cache.contains_key(&t) {
            if !decodes(t) {
                return Job::Idle;
            }
            if s.shown != Some(t) && s.ready.as_ref().is_none_or(|r| r.index != t) {
                return Job::Decode { upto: t, keep_from: t.saturating_sub(back) };
            }
        } else if s.shown != Some(t) && s.ready.as_ref().is_none_or(|r| r.index != t) {
            s.ready = s.cache.get(&t).cloned();
            self.signal.set();
        }
        // a hovered moment's thumbnail before filling the cache
        if let Some(k) = thumb {
            return Job::Thumb(k);
        }
        // the way the user moves first; ahead is cheap from where the decoders are, behind is a
        // decode from the key run
        let end = (t + ahead).min(n - 1);
        let start = t.saturating_sub(back);
        let fwd = (t + 1..=end).find(|i| !s.cache.contains_key(i) && decodes(*i)).map(|f| Job::Decode { upto: f.max(end.min(f + 8)), keep_from: f });
        let bwd = (start..t).rev().find(|i| !s.cache.contains_key(i) && decodes(*i)).map(|b| Job::Decode { upto: b, keep_from: start });
        let (first, second) = if s.backward { (bwd, fwd) } else { (fwd, bwd) };
        first.or(second).unwrap_or(Job::Idle)
    }

    fn run(mut self) {
        loop {
            let job = {
                let mut s = self.shared.state.lock().unwrap();
                loop {
                    if s.stop {
                        return;
                    }
                    match self.plan(&mut s) {
                        Job::Idle => {
                            s.work.clear();
                            s = self.shared.wake.wait(s).unwrap();
                        }
                        job => break job,
                    }
                }
            };
            match job {
                Job::Decode { upto, keep_from } => self.decode(upto, keep_from),
                Job::Thumb(k) => self.thumb(k),
                Job::Idle => {}
            }
        }
    }

    /// Decodes up to picture `upto` from the decoders' picture or the key run (whichever is
    /// nearer), keeping `keep_from..=upto`. Stops early when the target moves elsewhere.
    fn decode(&mut self, upto: usize, keep_from: usize) {
        let Some(sync) = self.snap.pictures[upto].sync else { return };
        let from = match self.at {
            Some(a) if a < upto && a + 1 >= sync => a + 1,
            Some(a) if a < upto && upto - a < upto - sync => a + 1,
            _ => {
                self.decoders.clear();
                sync
            }
        };
        for i in from..=upto {
            {
                let mut s = self.shared.state.lock().unwrap();
                if s.stop {
                    return;
                }
                let t = s.target;
                // the user went elsewhere: this work is done if its pictures are no longer wanted
                let wanted = (t >= keep_from || t >= i) && t <= upto + (s.capacity() / 4).max(2);
                if t != upto && !wanted {
                    s.work.clear();
                    return;
                }
                if s.cache.contains_key(&t) && t == upto {
                    return;
                }
                s.work = format!("decoding {} of {} from frame {}", i - from + 1, upto - from + 1, self.snap.pictures[from].frame);
            }
            let decoded = self.decode_one(i);
            self.at = Some(i);
            let mut s = self.shared.state.lock().unwrap();
            match decoded {
                Ok(Some((width, height, parts))) => {
                    if i >= keep_from || i == s.target {
                        let d = Arc::new(Decoded { index: i, width, height, parts });
                        if i == s.target {
                            s.ready = Some(d.clone());
                            self.signal.set();
                        }
                        s.insert(d);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    // the pictures after it up to the next key run depend on it: none of them decodes
                    let sync = self.snap.pictures[i].sync;
                    for j in (i..self.snap.len()).take_while(|j| self.snap.pictures[*j].sync == sync) {
                        s.failed.insert(j, e.clone());
                    }
                    self.signal.set();
                    // the decoders' state is lost: the next picture starts from its key run
                    self.decoders.clear();
                    self.at = None;
                    return;
                }
            }
        }
    }

    /// Key picture `k` decoded on its own (the decoders start over there).
    fn thumb(&mut self, k: usize) {
        self.decoders.clear();
        self.at = None;
        let decoded = self.decode_one(k);
        let mut s = self.shared.state.lock().unwrap();
        s.thumb_want = None;
        match decoded {
            Ok(Some((width, height, parts))) => {
                self.at = Some(k);
                let d = Arc::new(Decoded { index: k, width, height, parts });
                while s.thumbs.len() >= THUMBS {
                    let far = *s.thumbs.keys().max_by_key(|j| j.abs_diff(k)).unwrap();
                    s.thumbs.remove(&far);
                }
                s.thumbs.insert(k, d.clone());
                s.thumb_ready = Some(d);
                self.signal.set();
            }
            Ok(None) => {}
            Err(e) => {
                s.failed.insert(k, e);
                self.decoders.clear();
            }
        }
    }

    /// Picture `i` through the decoders: its parts once every part decoded (None: a tile
    /// missing, nothing to show).
    fn decode_one(&mut self, i: usize) -> Result<Option<(u32, u32, Vec<DecPart>)>, String> {
        let snap = self.snap.clone();
        let mut parts = Vec::new();
        let packets = snap.packets_of(i);
        for p in packets {
            let s = p.stream as usize;
            if self.decoders.len() <= s {
                self.decoders.resize_with(s + 1, || None);
            }
            // a key frame in another codec (the desktop switched) gets a decoder of its own
            if self.decoders[s].as_ref().is_none_or(|d| d.1 != p.codec) {
                if !p.key {
                    return Err(format!("stream {s} has no key frame before frame {}", p.frame));
                }
                self.decoders[s] = Some((VideoStreamDecoder::new_gpu(p.video_codec()).map_err(|e| format!("decoder: {e:?}"))?, p.codec));
            }
            let frames = self.decoders[s].as_mut().unwrap().0.push_packet(&p.data, p.frame as i64).map_err(|e| format!("frame {} stream {s}: {e:?}", p.frame))?;
            for f in frames {
                parts.push(DecPart { x0: p.x0, y0: p.y0, width: f.width, height: f.height, gpu: f.gpu, nv12: f.nv12, color: f.color });
            }
        }
        if parts.len() < packets.len() || parts.is_empty() {
            return Ok(None);
        }
        let width = parts.iter().map(|p| p.x0 + p.width).max().unwrap_or(0);
        let height = parts.iter().map(|p| p.y0 + p.height).max().unwrap_or(0);
        Ok(Some((width, height, parts)))
    }
}
