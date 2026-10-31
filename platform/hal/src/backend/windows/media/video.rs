//! Video and audio-file playback through Media Foundation, with decoded frames landing in the
//! HAL textures the app named (`VideoPrepare`).
//!
//! Moved from os/windows/{windows_mf_source_reader, windows_video_playback,
//! windows_video_player, windows_media_engine_notify}.rs. Each player has one long-lived MTA
//! worker (`makepad-mf-player`) that owns every Media Foundation object of the player; the HAL's
//! thread only posts commands and takes what the worker left in the player's mailbox (rung by
//! the session's `Signal`). Two decode paths, both on the HAL's own D3D11 device through an
//! `IMFDXGIDeviceManager`:
//!
//! - progressive files: an `IMFSourceReader` decodes to NV12 DXGI surfaces (hardware MFTs, DXVA),
//!   paced against a WASAPI audio master clock (the reader's float PCM) or the wall clock;
//! - network streams, WMV/ASF, audio files, and whatever the reader cannot open or decode:
//!   `IMFMediaEngine`, whose frames are transferred into NV12 (or BGRA) targets.
//!
//! A reader that fails after preparing hands over to the engine at the same position. A player
//! whose engine fails answers `VideoStatus` ERROR (the app's own decoder takes over). On the
//! HAL's thread a frame is copied into the player's present textures: the Y plane and the UV
//! plane each from its own NV12 texture (sampling both planes of one NV12 resource in one draw
//! removes the device on some GPUs), ping-ponged so the copy never writes what the frame being
//! drawn samples.

use super::{Signal, Sink};
use crate::backend::windows::d3d11::device::{hr, Device};
use crate::backend::windows::d3d11::resources::Texture;
use crate::media::src_kind;
use crate::session::Session;
use crate::types::{format, Id};
use makepad_windows_sys::core::{IUnknown, BSTR, GUID, PCWSTR};
use makepad_windows_sys::Win32::Foundation::RECT;
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::Common::*;
use makepad_windows_sys::Win32::Media::Audio::*;
use makepad_windows_sys::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use makepad_windows_sys::Win32::Media::MediaFoundation::*;
use makepad_windows_sys::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use makepad_windows_sys::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use makepad_windows_sys::Win32::System::Com::*;
use makepad_windows_sys::Win32::System::Variant::VARENUM;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The worker's tick period while it prepares or plays.
const TICK: Duration = Duration::from_millis(4);

pub enum Source {
    Network(String),
    File(String),
    Memory(Vec<u8>),
}

impl Source {
    pub fn new(kind: u16, bytes: &[u8]) -> Source {
        match kind {
            src_kind::NETWORK => Source::Network(String::from_utf8_lossy(bytes).into_owned()),
            src_kind::FILE => Source::File(String::from_utf8_lossy(bytes).into_owned()),
            _ => Source::Memory(bytes.to_vec()),
        }
    }
}

/// The textures the app named for the player.
#[derive(Clone, Copy)]
pub struct Targets {
    pub bgra: Id,
    pub y: Id,
    pub u: Id,
    pub v: Id,
}

pub struct Prepared {
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub has_audio: bool,
}

/// A decoded frame waiting for the HAL's thread: an NV12 (or BGRA) texture of the HAL's device
/// and the array slice the frame is in. `_keep` holds the decoder surface (the reader's sample)
/// until the frame was copied.
pub struct Frame {
    texture: ID3D11Texture2D,
    slice: u32,
    /// the top-left of the picture in the surface (a coded frame can be larger than the picture)
    origin: (u32, u32),
    width: u32,
    height: u32,
    nv12: bool,
    pub position_ms: u64,
    /// `YuvColorMatrix` as f32: 0 BT.709, 1 BT.601, 2 BT.2020
    pub matrix: f32,
    pub full_range: bool,
    _keep: Option<Keep>,
}

impl Frame {
    pub fn is_nv12(&self) -> bool {
        self.nv12
    }
}

/// A COM object that only moves from the worker to the HAL's thread to be dropped there.
struct Keep(#[allow(dead_code)] IMFSample);
// SAFETY: the sample is free-threaded (MF samples are); it is never used after the move
unsafe impl Send for Keep {}

pub enum Note {
    Prepared(Prepared),
    Frame(Frame),
    Completed,
    Error(String),
}

/// What the worker leaves for the HAL's thread.
pub struct Mailbox {
    notes: Mutex<VecDeque<Note>>,
    signal: Signal,
}

impl Mailbox {
    fn post(&self, note: Note) {
        let mut q = lock(&self.notes);
        // only the newest frame matters: an older one waiting is dropped (and its surface freed)
        if matches!(note, Note::Frame(_)) {
            q.retain(|n| !matches!(n, Note::Frame(_)));
        }
        q.push_back(note);
        drop(q);
        self.signal.set();
    }
}

enum Cmd {
    Play,
    Pause,
    Seek(u64),
    Mute(bool),
    Volume(f64),
    Rate(f64),
    /// a MediaEngine notification (from the engine's callback thread)
    Notify(u32),
    Destroy,
}

// ------------------------------------------------------------------ the HAL's side

/// One player, as the media module of the HAL's thread holds it.
pub struct Player {
    pub targets: Targets,
    tx: Sender<Cmd>,
    mailbox: Arc<Mailbox>,
    present: Present,
    pub position_ms: u64,
    /// frames installed so far (diagnostics)
    pub frames_shown: u64,
}

impl Drop for Player {
    fn drop(&mut self) {
        // the worker shuts its engine down and ends; nothing here waits for it
        let _ = self.tx.send(Cmd::Destroy);
    }
}

impl Player {
    pub fn new(device: &ID3D11Device, targets: Targets, source: Source, video_id: u64, autoplay: bool, looping: bool, audio_only: bool, signal: Signal) -> Player {
        let (tx, rx) = channel();
        let mailbox = Arc::new(Mailbox { notes: Mutex::new(VecDeque::new()), signal });
        let (worker_mailbox, worker_tx, device) = (mailbox.clone(), tx.clone(), device.clone());
        let spawned = std::thread::Builder::new().name("makepad-mf-player".into()).spawn(move || {
            let worker = Worker::new(device, source, video_id, autoplay, looping, audio_only, worker_mailbox, worker_tx);
            worker.run(rx);
        });
        if let Err(e) = spawned {
            mailbox.post(Note::Error(format!("Media Foundation player thread: {e}")));
        }
        Player { targets, tx, mailbox, present: Present::default(), position_ms: 0, frames_shown: 0 }
    }

    pub fn play(&self) {
        let _ = self.tx.send(Cmd::Play);
    }
    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }
    pub fn seek(&self, ms: u64) {
        let _ = self.tx.send(Cmd::Seek(ms));
    }
    pub fn mute(&self, muted: bool) {
        let _ = self.tx.send(Cmd::Mute(muted));
    }
    pub fn volume(&self, v: f64) {
        let _ = self.tx.send(Cmd::Volume(v));
    }
    pub fn rate(&self, r: f64) {
        let _ = self.tx.send(Cmd::Rate(r));
    }

    /// What the worker posted since the last call.
    pub fn take(&self) -> Vec<Note> {
        lock(&self.mailbox.notes).drain(..).collect()
    }

    /// Copies the frame into the present textures and installs them as the storage of the
    /// player's textures. False when the frame could not be shown.
    pub fn present(&mut self, sink: &mut Sink, s: &mut Session, frame: &Frame) -> bool {
        self.position_ms = frame.position_ms;
        let result = if frame.nv12 { self.present.nv12(sink.dev, frame) } else { self.present.bgra(sink.dev, frame) };
        match result {
            Ok(Planes::Nv12(y, uv)) => sink.adopt(s, self.targets.y, y) && sink.adopt(s, self.targets.u, uv),
            Ok(Planes::Bgra(t)) => sink.adopt(s, self.targets.bgra, t),
            Err(e) => {
                eprintln!("[hal-media] video: frame present: {e}");
                false
            }
        }
    }
}

enum Planes {
    Nv12(Arc<Texture>, Arc<Texture>),
    Bgra(Arc<Texture>),
}

/// The present textures of one player: per ping-pong side, the Y and UV holders (NV12, each
/// sampled through one plane view) or one BGRA texture.
#[derive(Default)]
struct Present {
    size: (u32, u32),
    nv12: bool,
    sides: [Option<(Arc<Texture>, Option<Arc<Texture>>)>; 2],
    next: usize,
}

impl Present {
    fn side(&mut self, dev: &Arc<Device>, w: u32, h: u32, nv12: bool) -> Result<(Arc<Texture>, Option<Arc<Texture>>), String> {
        if self.size != (w, h) || self.nv12 != nv12 {
            *self = Present { size: (w, h), nv12, ..Default::default() };
        }
        let i = self.next;
        self.next = 1 - self.next;
        if let Some(side) = &self.sides[i] {
            return Ok(side.clone());
        }
        let side = if nv12 {
            let y = Texture::wrap(dev.clone(), new_texture(dev, w, h, DXGI_FORMAT_NV12)?, format::R8, w, h);
            let uv = Texture::wrap(dev.clone(), new_texture(dev, w, h, DXGI_FORMAT_NV12)?, format::RG8, w / 2, h / 2);
            (y, Some(uv))
        } else {
            (Texture::wrap(dev.clone(), new_texture(dev, w, h, DXGI_FORMAT_B8G8R8A8_UNORM)?, format::BGRA8, w, h), None)
        };
        self.sides[i] = Some(side.clone());
        Ok(side)
    }

    fn nv12(&mut self, dev: &Arc<Device>, frame: &Frame) -> Result<Planes, String> {
        let src = frame_desc(&frame.texture);
        if src.Format != DXGI_FORMAT_NV12 {
            return Err(format!("expected an NV12 surface, got format {}", src.Format.0));
        }
        if frame.slice >= src.ArraySize.max(1) {
            return Err(format!("array slice {} of {}", frame.slice, src.ArraySize));
        }
        // NV12 regions are whole 2x2 blocks
        let (x0, y0) = (frame.origin.0 & !1, frame.origin.1 & !1);
        let w = frame.width.min(src.Width.saturating_sub(x0)) & !1;
        let h = frame.height.min(src.Height.saturating_sub(y0)) & !1;
        if w == 0 || h == 0 {
            return Err("empty frame".into());
        }
        let (y, uv) = self.side(dev, w, h, true)?;
        let uv = uv.ok_or("NV12 side without its UV holder")?;
        let src_box = D3D11_BOX { left: x0, top: y0, front: 0, right: x0 + w, bottom: y0 + h, back: 1 };
        unsafe {
            let src_res = frame.texture.as_ID3D11Resource();
            dev.ctx.CopySubresourceRegion(y.res.as_ID3D11Resource(), 0, 0, 0, 0, src_res, frame.slice, Some(&src_box));
            dev.ctx.CopySubresourceRegion(uv.res.as_ID3D11Resource(), 0, 0, 0, 0, src_res, frame.slice, Some(&src_box));
        }
        Ok(Planes::Nv12(y, uv))
    }

    fn bgra(&mut self, dev: &Arc<Device>, frame: &Frame) -> Result<Planes, String> {
        let src = frame_desc(&frame.texture);
        let (w, h) = (frame.width.min(src.Width), frame.height.min(src.Height));
        if w == 0 || h == 0 {
            return Err("empty frame".into());
        }
        let (t, _) = self.side(dev, w, h, false)?;
        let src_box = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
        unsafe { dev.ctx.CopySubresourceRegion(t.res.as_ID3D11Resource(), 0, 0, 0, 0, frame.texture.as_ID3D11Resource(), frame.slice, Some(&src_box)) };
        Ok(Planes::Bgra(t))
    }
}

fn frame_desc(t: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut d = D3D11_TEXTURE2D_DESC::default();
    unsafe { t.GetDesc(&mut d) };
    d
}

/// A sampled default-usage texture of `fmt` (NV12 present holders, BGRA present, MediaEngine
/// transfer targets add RENDER_TARGET with `target`).
fn new_texture(dev: &Device, w: u32, h: u32, fmt: DXGI_FORMAT) -> Result<ID3D11Texture2D, String> {
    new_texture_on(&dev.device, w, h, fmt, false)
}

fn new_texture_on(device: &ID3D11Device, w: u32, h: u32, fmt: DXGI_FORMAT, target: bool) -> Result<ID3D11Texture2D, String> {
    let mut bind = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    if target {
        bind |= D3D11_BIND_RENDER_TARGET.0 as u32;
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut out = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut out)) }.map_err(|e| hr(&format!("CreateTexture2D {w}x{h} format {}", fmt.0), e))?;
    out.ok_or_else(|| "CreateTexture2D returned nothing".into())
}

// ------------------------------------------------------------------ sources

/// A path as a `file://` URL Media Foundation opens (UTF-16 kept as is, ASCII specials escaped).
pub fn path_to_file_url(path: &str) -> String {
    if path.starts_with("file://") {
        return path.to_string();
    }
    let normalized = path.replace('\\', "/");
    let mut out = String::with_capacity(normalized.len() + 8);
    let mut rest = normalized.as_str();
    if let Some(stripped) = rest.strip_prefix('/') {
        out.push('/');
        rest = stripped;
    }
    let mut chars = rest.chars();
    let tail: String = match (chars.next(), chars.next()) {
        (Some(a), Some(':')) if a.is_ascii_alphabetic() => {
            out.push(a);
            out.push(':');
            chars.collect()
        }
        _ => rest.to_string(),
    };
    for ch in tail.chars() {
        match ch {
            '/' | '-' | '_' | '.' | '~' | '(' | ')' | '!' | '*' | '\'' | ':' => out.push(ch),
            'A'..='Z' | 'a'..='z' | '0'..='9' => out.push(ch),
            ' ' => out.push_str("%20"),
            _ if ch.is_ascii() => out.push_str(&format!("%{:02X}", ch as u32)),
            _ => out.push(ch),
        }
    }
    if out.starts_with('/') {
        format!("file://{out}")
    } else {
        format!("file:///{out}")
    }
}

/// The container of in-memory bytes, from their magic, for the temp file's extension.
pub fn container_extension(data: &[u8]) -> &'static str {
    if data.len() < 12 {
        return "mp4";
    }
    if data.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return "webm";
    }
    if data.starts_with(b"OggS") {
        return "ogg";
    }
    if data.starts_with(b"RIFF") {
        return if &data[8..12] == b"AVI " { "avi" } else { "wav" };
    }
    if data.starts_with(b"fLaC") {
        return "flac";
    }
    if data.starts_with(b"ID3") || (data[0] == 0xff && (data[1] & 0xe0) == 0xe0) {
        return "mp3";
    }
    if &data[4..8] == b"ftyp" {
        return if &data[8..12] == b"qt  " { "mov" } else { "mp4" };
    }
    if data.starts_with(&[0x30, 0x26, 0xb2, 0x75]) {
        return "wmv";
    }
    "mp4"
}

/// WMV/ASF (VC-1 / WMA) rarely give DXGI NV12 through the source reader: the engine plays them.
fn prefers_engine(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let ext = lower.rsplit(['/', '\\', '?']).next().and_then(|n| n.rsplit_once('.')).map_or("", |(_, e)| e);
    matches!(ext, "wmv" | "asf" | "wma")
}

// ------------------------------------------------------------------ the worker

enum Engine {
    None,
    Reader(Box<ReaderSession>),
    Media(Box<EngineSession>),
}

struct Worker {
    device: ID3D11Device,
    url: String,
    temp_file: Option<PathBuf>,
    network: bool,
    audio_only: bool,
    looping: bool,
    want_play: bool,
    muted: bool,
    volume: f64,
    rate: f64,
    /// a seek asked before the engine prepared (or a reader handover's resume point)
    pending_seek: Option<u64>,
    prepared_sent: bool,
    engine: Engine,
    mailbox: Arc<Mailbox>,
    tx: Sender<Cmd>,
    mf_started: bool,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.engine = Engine::None;
        if let Some(path) = self.temp_file.take() {
            let _ = std::fs::remove_file(path);
        }
        if self.mf_started {
            unsafe {
                let _ = MFShutdown();
            }
        }
    }
}

impl Worker {
    #[allow(clippy::too_many_arguments)]
    fn new(device: ID3D11Device, source: Source, video_id: u64, autoplay: bool, looping: bool, audio_only: bool, mailbox: Arc<Mailbox>, tx: Sender<Cmd>) -> Worker {
        let mut temp_file = None;
        let (url, network) = match source {
            Source::Network(url) => (url, true),
            Source::File(path) => (path_to_file_url(&path), false),
            Source::Memory(bytes) => {
                let path = std::env::temp_dir().join(format!("makepad_video_{}_{video_id:x}.{}", std::process::id(), container_extension(&bytes)));
                match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        let url = path_to_file_url(&path.to_string_lossy());
                        temp_file = Some(path);
                        (url, false)
                    }
                    Err(e) => {
                        mailbox.post(Note::Error(format!("staging the in-memory video failed: {e}")));
                        (String::new(), false)
                    }
                }
            }
        };
        Worker { device, url, temp_file, network, audio_only, looping, want_play: autoplay, muted: false, volume: 1.0, rate: 1.0, pending_seek: None, prepared_sent: false, engine: Engine::None, mailbox, tx, mf_started: false }
    }

    fn run(self, rx: Receiver<Cmd>) {
        // declared before the worker, so released after it (the worker's Drop shuts Media
        // Foundation down and releases its COM objects first)
        let _com = super::Mta::enter();
        let mut this = self;
        this.serve(rx);
    }

    fn serve(&mut self, rx: Receiver<Cmd>) {
        if self.url.is_empty() {
            return;
        }
        unsafe {
            if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                self.mailbox.post(Note::Error(format!("MFStartup: {e}")));
                return;
            }
        }
        self.mf_started = true;
        self.open();
        loop {
            let busy = match &self.engine {
                Engine::None => false,
                Engine::Reader(r) => !r.fatal && (!self.prepared_sent || (self.want_play && !r.video_eos)),
                Engine::Media(m) => m.prepared && self.want_play,
            };
            let cmd = if busy {
                match rx.recv_timeout(TICK) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => break,
                }
            };
            if let Some(cmd) = cmd {
                if matches!(cmd, Cmd::Destroy) {
                    break;
                }
                self.command(cmd);
                // a burst of commands is taken before the next tick
                continue;
            }
            self.tick();
        }
    }

    /// The source reader for progressive files, the engine for the rest (or when forced with
    /// `MAKEPAD_MF_ENGINE`).
    fn open(&mut self) {
        let force_engine = std::env::var_os("MAKEPAD_MF_ENGINE").is_some();
        if !self.network && !self.audio_only && !force_engine && !prefers_engine(&self.url) {
            match ReaderSession::open(&self.device, &self.url) {
                Ok(r) => {
                    let mut r = Box::new(r);
                    r.apply(self.muted, self.volume);
                    self.engine = Engine::Reader(r);
                    return;
                }
                Err(e) => eprintln!("[hal-media] video: source reader unavailable ({e}); using the media engine"),
            }
        }
        self.open_engine();
    }

    fn open_engine(&mut self) {
        let bgra = std::env::var_os("MAKEPAD_MF_BGRA").is_some();
        match EngineSession::open(&self.device, &self.url, self.looping, !bgra, self.tx.clone()) {
            Ok(e) => self.engine = Engine::Media(Box::new(e)),
            Err(e) => {
                self.engine = Engine::None;
                self.mailbox.post(Note::Error(e));
            }
        }
    }

    /// A reader that failed: the engine takes over at `resume_ms` (or reports the failure when
    /// it never prepared and the engine cannot either).
    fn reader_failed(&mut self, error: String, resume_ms: u64) {
        eprintln!("[hal-media] video: source reader failed ({error}); handing over to the media engine");
        if resume_ms > 0 {
            self.pending_seek = Some(resume_ms);
        }
        self.open_engine();
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Play => self.want_play = true,
            Cmd::Pause => self.want_play = false,
            Cmd::Seek(ms) => {
                if !self.prepared_sent {
                    self.pending_seek = Some(ms);
                }
            }
            Cmd::Mute(m) => self.muted = m,
            Cmd::Volume(v) => self.volume = v.clamp(0.0, 1.0),
            Cmd::Rate(r) => self.rate = if r.is_finite() && r > 0.0 { r.clamp(0.05, 8.0) } else { 1.0 },
            Cmd::Notify(_) | Cmd::Destroy => {}
        }
        let (want_play, muted, volume, rate) = (self.want_play, self.muted, self.volume, self.rate);
        match (&mut self.engine, cmd) {
            (Engine::Reader(r), Cmd::Play) => r.play(),
            (Engine::Reader(r), Cmd::Pause) => r.pause(),
            (Engine::Reader(r), Cmd::Seek(ms)) if self.prepared_sent => r.seek(ms),
            (Engine::Reader(r), Cmd::Mute(_) | Cmd::Volume(_)) => r.apply(muted, volume),
            (Engine::Reader(r), Cmd::Rate(_)) => r.set_rate(rate),
            (Engine::Media(m), Cmd::Notify(event)) => {
                let notes = m.notify(event, want_play);
                for note in notes {
                    self.note(note);
                }
            }
            (Engine::Media(m), Cmd::Play) if m.prepared => {
                m.engine_play();
            }
            (Engine::Media(m), Cmd::Pause) => m.engine_pause(),
            (Engine::Media(m), Cmd::Seek(ms)) if m.prepared => m.seek(ms),
            (Engine::Media(m), Cmd::Mute(_) | Cmd::Volume(_) | Cmd::Rate(_)) if m.prepared => m.apply(muted, volume, rate),
            _ => {}
        }
    }

    /// A note from an engine: prepare also applies what the app asked for meanwhile.
    fn note(&mut self, note: Note) {
        match note {
            Note::Prepared(p) => {
                if let Engine::Media(m) = &mut self.engine {
                    m.apply(self.muted, self.volume, self.rate);
                    if let Some(ms) = self.pending_seek.take() {
                        m.seek(ms);
                    }
                    if self.want_play {
                        m.engine_play();
                    }
                }
                // the app knows the video once; a handover after a reader failure prepares again
                if !self.prepared_sent {
                    self.prepared_sent = true;
                    self.mailbox.post(Note::Prepared(p));
                }
            }
            Note::Completed => {
                self.want_play = false;
                self.mailbox.post(Note::Completed);
            }
            Note::Error(e) => {
                self.want_play = false;
                self.mailbox.post(Note::Error(e));
            }
            frame @ Note::Frame(_) => self.mailbox.post(frame),
        }
    }

    fn tick(&mut self) {
        let Engine::Reader(r) = &mut self.engine else {
            if let Engine::Media(m) = &mut self.engine {
                if let Some(note) = m.tick() {
                    self.note(note);
                }
            }
            return;
        };
        if !self.prepared_sent {
            match r.prime() {
                Ok(true) => {
                    let prepared = Prepared { width: r.width, height: r.height, duration_ms: r.duration_ms, has_audio: r.audio.is_some() };
                    eprintln!("[hal-media] video: source reader NV12 ready ({}x{}, {}, {} ms)", r.width, r.height, if r.audio.is_some() { "A/V" } else { "video only" }, r.duration_ms);
                    r.set_rate(self.rate);
                    if let Some(ms) = self.pending_seek.take() {
                        r.seek(ms);
                    }
                    if self.want_play {
                        r.play();
                    }
                    self.prepared_sent = true;
                    self.mailbox.post(Note::Prepared(prepared));
                }
                Ok(false) => {}
                Err(e) => self.reader_failed(e, 0),
            }
            return;
        }
        if !self.want_play {
            return;
        }
        match r.tick(self.looping) {
            ReaderTick::Idle => {}
            ReaderTick::Frame(f) => self.mailbox.post(Note::Frame(f)),
            ReaderTick::Eos => {
                self.want_play = false;
                self.mailbox.post(Note::Completed);
            }
            ReaderTick::Failed(e) => {
                let resume = r.position_ms;
                self.reader_failed(e, resume);
            }
        }
    }
}

// ------------------------------------------------------------------ source reader

fn propvariant_i8(value: i64) -> PROPVARIANT {
    // VT_I8
    PROPVARIANT { Anonymous: PROPVARIANT_0 { Anonymous: PROPVARIANT_0_0 { vt: VARENUM(20), wReserved1: 0, wReserved2: 0, wReserved3: 0, Anonymous: PROPVARIANT_0_0_0 { hVal: value } } } }
}

/// MF_PD_DURATION (VT_UI8, 100 ns units; VT_I8 accepted too).
fn propvariant_hns(var: &PROPVARIANT) -> i64 {
    unsafe {
        let inner = &var.Anonymous.Anonymous;
        match inner.vt.0 {
            20 => inner.Anonymous.hVal,
            21 => inner.Anonymous.uhVal as i64,
            _ => 0,
        }
    }
}

fn decoder_bind_flags() -> u32 {
    (D3D11_BIND_DECODER.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32
}

fn nv12_type(native: Option<&IMFMediaType>) -> Result<IMFMediaType, String> {
    unsafe {
        let mt = MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| format!("MAJOR_TYPE: {e}"))?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| format!("SUBTYPE NV12: {e}"))?;
        let _ = mt.SetUINT32(&MF_SA_D3D11_BINDFLAGS, decoder_bind_flags());
        // geometry copied from the stream, so hardware decoders accept the type
        if let Some(native) = native {
            for key in [&MF_MT_FRAME_SIZE, &MF_MT_FRAME_RATE, &MF_MT_PIXEL_ASPECT_RATIO] {
                if let Ok(v) = native.GetUINT64(key) {
                    let _ = mt.SetUINT64(key, v);
                }
            }
            if let Ok(v) = native.GetUINT32(&MF_MT_INTERLACE_MODE) {
                let _ = mt.SetUINT32(&MF_MT_INTERLACE_MODE, v);
            }
        }
        Ok(mt)
    }
}

fn configure_nv12(reader: &IMFSourceReader, stream: u32) -> Result<(), String> {
    let native = unsafe { reader.GetNativeMediaType(stream, 0) }.map_err(|e| format!("GetNativeMediaType: {e}"))?;
    let mut last = String::from("no NV12 type");
    for mt in [nv12_type(Some(&native)), nv12_type(None)] {
        match mt.and_then(|mt| unsafe { reader.SetCurrentMediaType(stream, None, &mt) }.map_err(|e| format!("SetCurrentMediaType(NV12): {e}"))) {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Float PCM out of the audio stream, mono or stereo (wider layouts downmixed by MF).
fn configure_float_audio(reader: &IMFSourceReader, stream: u32) -> Result<(u32, u32), String> {
    unsafe {
        let native = reader.GetNativeMediaType(stream, 0).map_err(|e| format!("audio GetNativeMediaType: {e}"))?;
        let channels = native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2).clamp(1, 2);
        let rate = native.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(48_000).max(8_000);
        let mt = MFCreateMediaType().map_err(|e| format!("audio MFCreateMediaType: {e}"))?;
        let block = channels * 4;
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(|e| format!("audio MAJOR_TYPE: {e}"))?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float).map_err(|e| format!("audio Float: {e}"))?;
        mt.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels).map_err(|e| format!("audio channels: {e}"))?;
        mt.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).map_err(|e| format!("audio rate: {e}"))?;
        mt.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32).map_err(|e| format!("audio bits: {e}"))?;
        mt.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block).map_err(|e| format!("audio block: {e}"))?;
        mt.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, block * rate).map_err(|e| format!("audio bytes/s: {e}"))?;
        reader.SetCurrentMediaType(stream, None, &mt).map_err(|e| format!("audio SetCurrentMediaType: {e}"))?;
        Ok((channels, rate))
    }
}

/// `MF_MT_MINIMUM_DISPLAY_APERTURE` (an `MFVideoArea`: two MFOffsets of fract u16 + value i16,
/// then the size as two i32) as x, y, width, height.
fn aperture(mt: &IMFMediaType) -> Option<(u32, u32, u32, u32)> {
    let mut blob = [0u8; 16];
    let mut size = 0u32;
    unsafe { mt.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut blob, Some(&mut size)) }.ok()?;
    if size < 16 {
        return None;
    }
    let x = i16::from_le_bytes([blob[2], blob[3]]).max(0) as u32;
    let y = i16::from_le_bytes([blob[6], blob[7]]).max(0) as u32;
    let w = i32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]).max(0) as u32;
    let h = i32::from_le_bytes([blob[12], blob[13], blob[14], blob[15]]).max(0) as u32;
    Some((x, y, w, h))
}

/// The colour of the stream: matrix (as `YuvColorMatrix` f32) and full range. Missing
/// attributes: HD and up is BT.709, SD BT.601, limited range.
fn yuv_color(mt: &IMFMediaType, height: u32) -> Option<(f32, bool)> {
    let matrix = unsafe { mt.GetUINT32(&MF_MT_YUV_MATRIX) }.ok()?;
    let matrix = match matrix {
        m if m == MFVideoTransferMatrix_BT601.0 as u32 => 1.0,
        m if m == MFVideoTransferMatrix_BT709.0 as u32 => 0.0,
        m if m == MFVideoTransferMatrix_BT2020_10.0 as u32 || m == MFVideoTransferMatrix_BT2020_12.0 as u32 => 2.0,
        _ if height >= 720 => 0.0,
        _ => 1.0,
    };
    let full = matches!(unsafe { mt.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE) }, Ok(v) if v == MFNominalRange_0_255.0 as u32);
    Some((matrix, full))
}

/// The reader's audio: a shared WASAPI stream on the default output, fed from a PCM backlog;
/// the media frames it consumed are the master clock.
struct ReaderAudio {
    client: IAudioClient,
    render: IAudioRenderClient,
    channels: usize,
    sample_rate: u32,
    pending: VecDeque<f32>,
    /// media frames handed to the device since the last clock reset
    consumed: u64,
    muted: bool,
    volume: f32,
    started: bool,
    /// resampling scratch (rate != 1), kept across writes
    scratch: Vec<f32>,
}

impl ReaderAudio {
    fn open_default(channels: u32, sample_rate: u32) -> Result<ReaderAudio, String> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL, &IMMDeviceEnumerator::IID).and_then(|raw| IMMDeviceEnumerator::from_raw(raw)).map_err(|e| format!("MMDeviceEnumerator: {e}"))?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|e| format!("GetDefaultAudioEndpoint: {e}"))?;
            let client: IAudioClient = device.Activate(&IAudioClient::IID, CLSCTX_ALL, None).and_then(|raw| IAudioClient::from_raw(raw)).map_err(|e| format!("Activate IAudioClient: {e}"))?;
            let channels = channels.clamp(1, 2);
            let format = WAVEFORMATEXTENSIBLE {
                Format: WAVEFORMATEX {
                    wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
                    nChannels: channels as u16,
                    nSamplesPerSec: sample_rate,
                    nAvgBytesPerSec: sample_rate * channels * 4,
                    nBlockAlign: (channels * 4) as u16,
                    wBitsPerSample: 32,
                    cbSize: 22,
                },
                Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
                dwChannelMask: if channels == 1 { 0x4 } else { 0x3 },
                SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
            };
            // about 100 ms of shared buffer, converted toward the engine's mix format
            client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, 1_000_000, 0, &format as *const _ as *const WAVEFORMATEX, None)
                .map_err(|e| format!("IAudioClient::Initialize: {e}"))?;
            let render: IAudioRenderClient = client.GetService(&IAudioRenderClient::IID).and_then(|raw| IAudioRenderClient::from_raw(raw)).map_err(|e| format!("IAudioRenderClient: {e}"))?;
            Ok(ReaderAudio { client, render, channels: channels as usize, sample_rate, pending: VecDeque::new(), consumed: 0, muted: false, volume: 1.0, started: false, scratch: Vec::new() })
        }
    }

    fn set_playing(&mut self, playing: bool) {
        unsafe {
            if playing {
                if !self.started {
                    let _ = self.client.Start();
                    self.started = true;
                }
            } else if self.started {
                // silence what is still queued in the device buffer, then stop
                let padding = self.client.GetCurrentPadding().unwrap_or(0);
                if let Ok(size) = self.client.GetBufferSize() {
                    let avail = size.saturating_sub(padding);
                    if avail > 0 {
                        if let Ok(ptr) = self.render.GetBuffer(avail) {
                            std::slice::from_raw_parts_mut(ptr as *mut f32, avail as usize * self.channels).fill(0.0);
                            let _ = self.render.ReleaseBuffer(avail, 0);
                        }
                    }
                }
                let _ = self.client.Stop();
                self.started = false;
                self.pending.clear();
                self.consumed = self.consumed.saturating_sub(padding as u64);
            }
        }
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.consumed = 0;
    }

    /// Media seconds of audio heard (handed over minus what the device still holds).
    fn clock_secs(&self) -> f64 {
        let padding = if self.started { unsafe { self.client.GetCurrentPadding() }.unwrap_or(0) as f64 } else { 0.0 };
        (self.consumed as f64 - padding).max(0.0) / self.sample_rate.max(1) as f64
    }

    fn push(&mut self, samples: &[f32]) {
        self.pending.extend(samples.iter().copied());
        // a backlog past about a second (after a seek or stall) is dropped from the front
        let cap = self.channels * self.sample_rate.max(1) as usize;
        let excess = self.pending.len().saturating_sub(cap);
        self.pending.drain(..excess);
    }

    /// Fills what the device has room for, resampled by `rate` (nearest frame).
    fn write(&mut self, rate: f64) {
        if !self.started {
            return;
        }
        unsafe {
            let (Ok(padding), Ok(size)) = (self.client.GetCurrentPadding(), self.client.GetBufferSize()) else { return };
            let avail = size.saturating_sub(padding) as usize;
            if avail == 0 {
                return;
            }
            let rate = rate.clamp(0.05, 8.0);
            let ch = self.channels;
            let take = (((avail as f64) * rate).ceil() as usize).min(self.pending.len() / ch);
            if take == 0 {
                return;
            }
            self.scratch.clear();
            self.scratch.extend(self.pending.drain(..take * ch));
            let out_frames = if (0.99..=1.01).contains(&rate) { take } else { ((take as f64) / rate).round() as usize }.clamp(1, avail);
            let Ok(ptr) = self.render.GetBuffer(out_frames as u32) else { return };
            let out = std::slice::from_raw_parts_mut(ptr as *mut f32, out_frames * ch);
            let gain = if self.muted { 0.0 } else { self.volume.clamp(0.0, 1.0) };
            for f in 0..out_frames {
                let src = if take <= 1 { 0 } else { ((f as f64) * (take - 1) as f64 / (out_frames.saturating_sub(1)).max(1) as f64).round() as usize }.min(take - 1);
                for c in 0..ch {
                    out[f * ch + c] = self.scratch[src * ch + c] * gain;
                }
            }
            let _ = self.render.ReleaseBuffer(out_frames as u32, 0);
            self.consumed += take as u64;
        }
    }
}

enum ReaderTick {
    Idle,
    Frame(Frame),
    Eos,
    Failed(String),
}

enum ReadVideo {
    Empty,
    Eos,
    Sample(IMFSample, i64),
}

struct ReaderSession {
    reader: IMFSourceReader,
    _manager: IMFDXGIDeviceManager,
    /// the picture inside the decoded surface (`MF_MT_MINIMUM_DISPLAY_APERTURE`)
    origin: (u32, u32),
    width: u32,
    height: u32,
    duration_ms: u64,
    matrix: f32,
    full_range: bool,
    audio: Option<ReaderAudio>,
    rate: f64,
    playing: bool,
    wall_start: Option<Instant>,
    pts_origin: i64,
    /// after a start or seek the clock starts at the first decoded frame
    await_anchor: bool,
    /// accurate seek: frames and audio before this (100 ns) are dropped
    seek_target: Option<i64>,
    pending_video: Option<(IMFSample, i64)>,
    video_eos: bool,
    fatal: bool,
    position_ms: u64,
}

const VIDEO_STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const AUDIO_STREAM: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;

/// A DXGI device manager over the HAL's device (decoders and the engine render on it).
fn device_manager(device: &ID3D11Device) -> Result<IMFDXGIDeviceManager, String> {
    let mut token = 0u32;
    let mut manager = None;
    unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager) }.map_err(|e| format!("MFCreateDXGIDeviceManager: {e}"))?;
    let manager = manager.ok_or("MFCreateDXGIDeviceManager returned nothing")?;
    unsafe { manager.ResetDevice(device.as_IUnknown(), token) }.map_err(|e| format!("IMFDXGIDeviceManager::ResetDevice: {e}"))?;
    Ok(manager)
}

impl ReaderSession {
    fn open(device: &ID3D11Device, url: &str) -> Result<ReaderSession, String> {
        let manager = device_manager(device)?;
        // hardware decoders first; when NV12 negotiation fails there, DXVA on the software decoder
        let (reader, audio_format) = match Self::open_reader(url, &manager, true) {
            Ok(r) => r,
            Err(hw) => Self::open_reader(url, &manager, false).map_err(|e| format!("source reader (hardware: {hw}; DXVA only: {e})"))?,
        };
        let audio = match audio_format {
            Some((channels, rate)) => match ReaderAudio::open_default(channels, rate) {
                Ok(a) => Some(a),
                Err(e) => {
                    eprintln!("[hal-media] video: no audio output for the reader ({e}); playing video only");
                    let _ = unsafe { reader.SetStreamSelection(AUDIO_STREAM, false) };
                    None
                }
            },
            None => None,
        };
        let mt = unsafe { reader.GetCurrentMediaType(VIDEO_STREAM) }.map_err(|e| format!("GetCurrentMediaType: {e}"))?;
        let size = unsafe { mt.GetUINT64(&MF_MT_FRAME_SIZE) }.map_err(|e| format!("MF_MT_FRAME_SIZE: {e}"))?;
        let (width, height) = ((size >> 32) as u32, (size & 0xffff_ffff) as u32);
        if width == 0 || height == 0 {
            return Err(format!("frame size {width}x{height}"));
        }
        // the coded size can exceed the picture (16-aligned macroblocks): show the aperture
        let aperture = aperture(&mt).or_else(|| unsafe { reader.GetNativeMediaType(VIDEO_STREAM, 0) }.ok().and_then(|n| aperture(&n)));
        let (origin, width, height) = match aperture {
            Some((x, y, w, h)) if x + w <= width && y + h <= height && w > 0 && h > 0 => ((x, y), w, h),
            _ => ((0, 0), width, height),
        };
        // the converted type may lack the colour attributes: the stream's own type has them
        let (matrix, full_range) = yuv_color(&mt, height)
            .or_else(|| unsafe { reader.GetNativeMediaType(VIDEO_STREAM, 0) }.ok().and_then(|n| yuv_color(&n, height)))
            .unwrap_or((if height >= 720 { 0.0 } else { 1.0 }, false));
        let duration_ms = match unsafe { reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION) } {
            Ok(var) => (propvariant_hns(&var).max(0) / 10_000) as u64,
            Err(_) => 0,
        };
        Ok(ReaderSession {
            reader,
            _manager: manager,
            origin,
            width,
            height,
            duration_ms,
            matrix,
            full_range,
            audio,
            rate: 1.0,
            playing: false,
            wall_start: None,
            pts_origin: 0,
            await_anchor: true,
            seek_target: None,
            pending_video: None,
            video_eos: false,
            fatal: false,
            position_ms: 0,
        })
    }

    fn open_reader(url: &str, manager: &IMFDXGIDeviceManager, hardware: bool) -> Result<(IMFSourceReader, Option<(u32, u32)>), String> {
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 5).map_err(|e| format!("MFCreateAttributes: {e}"))?;
            let attrs = attrs.ok_or("MFCreateAttributes returned nothing")?;
            attrs.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, manager.as_IUnknown()).map_err(|e| format!("D3D_MANAGER: {e}"))?;
            if hardware {
                attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(|e| format!("HARDWARE_TRANSFORMS: {e}"))?;
            }
            attrs.SetUINT32(&MF_SOURCE_READER_DISABLE_DXVA, 0).map_err(|e| format!("DISABLE_DXVA: {e}"))?;
            attrs.SetUINT32(&MF_SOURCE_READER_D3D11_BIND_FLAGS, decoder_bind_flags()).map_err(|e| format!("D3D11_BIND_FLAGS: {e}"))?;
            let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
            let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), Some(&attrs)).map_err(|e| format!("MFCreateSourceReaderFromURL: {e}"))?;
            reader.SetStreamSelection(VIDEO_STREAM, true).map_err(|e| format!("video stream: {e}"))?;
            configure_nv12(&reader, VIDEO_STREAM)?;
            let audio = match configure_float_audio(&reader, AUDIO_STREAM) {
                Ok(fmt) => {
                    let _ = reader.SetStreamSelection(AUDIO_STREAM, true);
                    Some(fmt)
                }
                Err(e) => {
                    if crate::backend::windows::win32::log_enabled() {
                        eprintln!("[hal-media] video: the reader plays no audio ({e})");
                    }
                    let _ = reader.SetStreamSelection(AUDIO_STREAM, false);
                    None
                }
            };
            Ok((reader, audio))
        }
    }

    fn apply(&mut self, muted: bool, volume: f64) {
        if let Some(a) = &mut self.audio {
            a.muted = muted;
            a.volume = volume as f32;
        }
    }

    fn play(&mut self) {
        self.playing = true;
        self.video_eos = false;
        // resume from a frozen clock: the wall restarts, the media origin stays
        if self.wall_start.is_none() && !self.await_anchor {
            self.wall_start = Some(Instant::now());
        }
        if let Some(a) = &mut self.audio {
            a.set_playing(true);
        }
    }

    fn pause(&mut self) {
        self.playing = false;
        if let Some(start) = self.wall_start.take() {
            self.pts_origin += (start.elapsed().as_secs_f64() * self.rate.max(0.05) * 1e7) as i64;
        }
        if let Some(a) = &mut self.audio {
            a.set_playing(false);
        }
    }

    fn set_rate(&mut self, rate: f64) {
        // the audio clock counts media frames (no re-anchor); the wall clock needs one
        if self.audio.is_none() {
            if let Some(start) = self.wall_start {
                self.pts_origin += (start.elapsed().as_secs_f64() * self.rate.max(0.05) * 1e7) as i64;
                self.wall_start = Some(Instant::now());
            }
        }
        self.rate = rate;
    }

    fn seek(&mut self, ms: u64) {
        let hns = (ms as i64).saturating_mul(10_000);
        let time_format = GUID::zeroed();
        match unsafe { self.reader.SetCurrentPosition(&time_format, &propvariant_i8(hns)) } {
            Ok(()) => {
                let _ = unsafe { self.reader.Flush(MF_SOURCE_READER_ALL_STREAMS.0 as u32) };
                self.pending_video = None;
                self.video_eos = false;
                if let Some(a) = &mut self.audio {
                    a.clear();
                }
                // the clock waits for the first frame after the seek (decode latency is not
                // something to catch up on)
                self.wall_start = None;
                self.pts_origin = hns;
                self.seek_target = Some(hns);
                self.await_anchor = true;
            }
            Err(e) => eprintln!("[hal-media] video: source reader seek: {e}"),
        }
    }

    fn read_video(&mut self) -> Result<ReadVideo, String> {
        let (mut flags, mut ts, mut sample) = (0u32, 0i64, None::<IMFSample>);
        unsafe { self.reader.ReadSample(VIDEO_STREAM, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)) }.map_err(|e| format!("ReadSample: {e}"))?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Ok(ReadVideo::Eos);
        }
        match sample {
            Some(s) if flags & MF_SOURCE_READERF_STREAMTICK.0 as u32 == 0 => Ok(ReadVideo::Sample(s, ts)),
            _ => Ok(ReadVideo::Empty),
        }
    }

    /// Decodes until the first frame is held (a container the DXGI path cannot decode fails
    /// here, before the app is told the video is prepared).
    fn prime(&mut self) -> Result<bool, String> {
        if self.pending_video.is_some() {
            return Ok(true);
        }
        let read = match self.read_video() {
            Err(e) if self.audio.is_some() => {
                // audio negotiation can poison some demuxers (classic ASF): retry video only
                eprintln!("[hal-media] video: first read failed ({e}); retrying without audio");
                let _ = unsafe { self.reader.SetStreamSelection(AUDIO_STREAM, false) };
                let _ = unsafe { self.reader.Flush(MF_SOURCE_READER_ALL_STREAMS.0 as u32) };
                if let Some(mut a) = self.audio.take() {
                    a.set_playing(false);
                }
                self.read_video()
            }
            other => other,
        };
        match read? {
            ReadVideo::Sample(s, ts) => {
                self.pending_video = Some((s, ts));
                Ok(true)
            }
            ReadVideo::Empty => Ok(false),
            ReadVideo::Eos => Err("end of stream before the first frame".into()),
        }
    }

    fn drain_audio(&mut self) {
        let drop_before = self.seek_target.unwrap_or(self.pts_origin);
        for _ in 0..8 {
            let Some(a) = &self.audio else { return };
            if a.pending.len() > a.channels * (a.sample_rate as usize / 5) {
                return;
            }
            let (mut flags, mut ts, mut sample) = (0u32, 0i64, None::<IMFSample>);
            if unsafe { self.reader.ReadSample(AUDIO_STREAM, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)) }.is_err() || flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return;
            }
            let Some(sample) = sample else { continue };
            if ts < drop_before {
                continue;
            }
            unsafe {
                let Ok(buffer) = sample.GetBufferByIndex(0) else { continue };
                let (mut ptr, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
                if buffer.Lock(&mut ptr, None, Some(&mut len)).is_err() {
                    continue;
                }
                if !ptr.is_null() && len >= 4 {
                    let floats = std::slice::from_raw_parts(ptr as *const f32, len as usize / 4);
                    if let Some(a) = &mut self.audio {
                        a.push(floats);
                    }
                }
                let _ = buffer.Unlock();
            }
        }
    }

    fn clock_secs(&self) -> Option<f64> {
        if let Some(a) = &self.audio {
            if a.started && !self.await_anchor {
                return Some(a.clock_secs());
            }
        }
        self.wall_start.map(|s| s.elapsed().as_secs_f64() * self.rate.max(0.05))
    }

    /// One tick of playback: at most one frame presented, late frames dropped to catch the
    /// clock, frames before an accurate-seek target discarded.
    fn tick(&mut self, looping: bool) -> ReaderTick {
        if self.fatal || !self.playing || self.video_eos {
            return ReaderTick::Idle;
        }
        // audio waits for the clock's anchor, or it plays ahead of the picture after a seek
        if !self.await_anchor {
            self.drain_audio();
            let rate = self.rate;
            if let Some(a) = &mut self.audio {
                a.write(rate);
            }
        }
        for _ in 0..8 {
            if self.pending_video.is_none() {
                match self.read_video() {
                    Ok(ReadVideo::Empty) => return ReaderTick::Idle,
                    Ok(ReadVideo::Eos) => {
                        if looping {
                            self.seek(0);
                            self.play();
                            return ReaderTick::Idle;
                        }
                        self.video_eos = true;
                        self.pause();
                        return ReaderTick::Eos;
                    }
                    Ok(ReadVideo::Sample(s, ts)) => self.pending_video = Some((s, ts)),
                    Err(e) => {
                        self.fatal = true;
                        return ReaderTick::Failed(e);
                    }
                }
            }
            let Some(pts) = self.pending_video.as_ref().map(|p| p.1) else { return ReaderTick::Idle };
            if let Some(target) = self.seek_target {
                if pts < target {
                    self.pending_video = None;
                    // keep the demuxer's audio from piling up behind the dropped frames
                    self.drain_audio();
                    if let Some(a) = &mut self.audio {
                        a.clear();
                    }
                    continue;
                }
                self.seek_target = None;
            }
            if !self.await_anchor {
                if let Some(clock) = self.clock_secs() {
                    let ahead = (pts - self.pts_origin) as f64 / 1e7 - clock;
                    if ahead > 0.035 {
                        return ReaderTick::Idle;
                    }
                    if ahead < -0.080 {
                        self.pending_video = None;
                        continue;
                    }
                }
            }
            let Some((sample, pts)) = self.pending_video.take() else { return ReaderTick::Idle };
            if self.await_anchor {
                self.pts_origin = pts;
                self.wall_start = Some(Instant::now());
                if let Some(a) = &mut self.audio {
                    a.consumed = 0;
                }
                self.await_anchor = false;
            }
            self.position_ms = (pts.max(0) / 10_000) as u64;
            return match self.frame_of(sample) {
                Ok(f) => ReaderTick::Frame(f),
                Err(e) => {
                    self.fatal = true;
                    ReaderTick::Failed(e)
                }
            };
        }
        ReaderTick::Idle
    }

    /// The decoder surface (and array slice) a sample holds.
    fn frame_of(&self, sample: IMFSample) -> Result<Frame, String> {
        unsafe {
            let buffer = sample.GetBufferByIndex(0).map_err(|e| format!("GetBufferByIndex: {e}"))?;
            let dxgi = IMFDXGIBuffer::query(buffer.as_raw()).map_err(|e| format!("the decoder gave no DXGI surface ({e})"))?;
            let raw = dxgi.GetResource(&ID3D11Texture2D::IID).map_err(|e| format!("IMFDXGIBuffer::GetResource: {e}"))?;
            let texture = ID3D11Texture2D::from_raw(raw).map_err(|e| format!("ID3D11Texture2D: {e}"))?;
            let slice = dxgi.GetSubresourceIndex().unwrap_or(0);
            Ok(Frame { texture, slice, origin: self.origin, width: self.width, height: self.height, nv12: true, position_ms: self.position_ms, matrix: self.matrix, full_range: self.full_range, _keep: Some(Keep(sample)) })
        }
    }
}

impl Drop for ReaderSession {
    fn drop(&mut self) {
        if let Some(a) = &mut self.audio {
            a.set_playing(false);
        }
    }
}

// ------------------------------------------------------------------ media engine

/// The engine's notifications go to the player's worker as commands.
struct Notify {
    tx: Mutex<Sender<Cmd>>,
}

impl IMFMediaEngineNotifyImpl for Notify {
    fn EventNotify(&self, event: u32, _param1: usize, _param2: u32) -> Result<(), makepad_windows_sys::core::HRESULT> {
        let _ = lock(&self.tx).send(Cmd::Notify(event));
        Ok(())
    }
}

struct EngineSession {
    engine: IMFMediaEngine,
    _manager: IMFDXGIDeviceManager,
    device: ID3D11Device,
    url: String,
    looping: bool,
    nv12: bool,
    /// transfer targets, three so a transfer never overwrites one the HAL's thread copies from
    targets: [Option<ID3D11Texture2D>; 3],
    write: usize,
    width: u32,
    height: u32,
    prepared: bool,
    last_pts: Option<i64>,
    tx: Sender<Cmd>,
}

impl Drop for EngineSession {
    fn drop(&mut self) {
        unsafe {
            let _ = self.engine.Shutdown();
        }
    }
}

impl EngineSession {
    fn open(device: &ID3D11Device, url: &str, looping: bool, nv12: bool, tx: Sender<Cmd>) -> Result<EngineSession, String> {
        let manager = device_manager(device)?;
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 4).map_err(|e| format!("MFCreateAttributes: {e}"))?;
            let attrs = attrs.ok_or("MFCreateAttributes returned nothing")?;
            let notify = IMFMediaEngineNotify::implement(Box::new(Notify { tx: Mutex::new(tx.clone()) }));
            attrs.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, notify.as_IUnknown()).map_err(|e| format!("CALLBACK: {e}"))?;
            attrs.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, manager.as_IUnknown()).map_err(|e| format!("DXGI_MANAGER: {e}"))?;
            let format = if nv12 { DXGI_FORMAT_NV12 } else { DXGI_FORMAT_B8G8R8A8_UNORM };
            let _ = attrs.SetUINT32(&MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, format.0 as u32);
            let factory: IMFMediaEngineClassFactory = CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER, &IMFMediaEngineClassFactory::IID)
                .and_then(|raw| IMFMediaEngineClassFactory::from_raw(raw))
                .map_err(|e| format!("MFMediaEngineClassFactory: {e} (is the Media Feature Pack installed?)"))?;
            let engine = factory.CreateInstance(0, &attrs).map_err(|e| format!("IMFMediaEngine: {e}"))?;
            let _ = engine.SetLoop(looping);
            let bstr = BSTR::from_str(url).map_err(|e| format!("source URL: {e}"))?;
            engine.SetSource(bstr.0).map_err(|e| format!("SetSource: {e}"))?;
            Ok(EngineSession { engine, _manager: manager, device: device.clone(), url: url.to_string(), looping, nv12, targets: [None, None, None], write: 0, width: 0, height: 0, prepared: false, last_pts: None, tx })
        }
    }

    fn engine_play(&mut self) {
        let _ = unsafe { self.engine.Play() };
    }

    fn engine_pause(&mut self) {
        let _ = unsafe { self.engine.Pause() };
    }

    fn seek(&mut self, ms: u64) {
        self.last_pts = None;
        let _ = unsafe { self.engine.SetCurrentTime(ms as f64 / 1000.0) };
    }

    fn apply(&mut self, muted: bool, volume: f64, rate: f64) {
        unsafe {
            let _ = self.engine.SetMuted(muted);
            let _ = self.engine.SetVolume(volume);
            let _ = self.engine.SetPlaybackRate(rate);
        }
    }

    fn error_text(&self) -> String {
        match unsafe { self.engine.GetError() } {
            Ok(err) => {
                let code = unsafe { err.GetErrorCode() };
                let label = match code {
                    1 => "aborted",
                    2 => "network",
                    3 => "decode",
                    4 => "source not supported",
                    5 => "encrypted",
                    _ => "unknown",
                };
                format!("media engine error: {label} (code {code})")
            }
            Err(e) => format!("media engine error ({e})"),
        }
    }

    /// A notification of the engine: what it means for the app.
    fn notify(&mut self, event: u32, _want_play: bool) -> Vec<Note> {
        let mut notes = Vec::new();
        if event == MF_MEDIA_ENGINE_EVENT_CANPLAY.0 as u32 && !self.prepared {
            let (mut w, mut h) = (0u32, 0u32);
            let has_video = unsafe { self.engine.GetNativeVideoSize(Some(&mut w), Some(&mut h)) }.is_ok() && w > 0 && h > 0;
            let has_audio = unsafe { (self.engine.vtable().HasAudio)(self.engine.as_raw()) }.as_bool();
            if has_video || has_audio {
                self.prepared = true;
                self.width = w;
                self.height = h;
                let duration = unsafe { self.engine.GetDuration() };
                let duration_ms = if duration.is_finite() && duration > 0.0 { (duration * 1000.0) as u64 } else { 0 };
                notes.push(Note::Prepared(Prepared { width: w, height: h, duration_ms, has_audio }));
            }
        } else if event == MF_MEDIA_ENGINE_EVENT_ENDED.0 as u32 && !self.looping {
            let _ = unsafe { self.engine.Pause() };
            notes.push(Note::Completed);
        } else if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
            let text = self.error_text();
            let _ = unsafe { self.engine.Pause() };
            notes.push(Note::Error(text));
        } else if event == MF_MEDIA_ENGINE_EVENT_FORMATCHANGE.0 as u32 {
            let (mut w, mut h) = (0u32, 0u32);
            if unsafe { self.engine.GetNativeVideoSize(Some(&mut w), Some(&mut h)) }.is_ok() && w > 0 && h > 0 && (w, h) != (self.width, self.height) {
                self.width = w;
                self.height = h;
                self.targets = [None, None, None];
                self.write = 0;
            }
        }
        notes
    }

    /// A new picture of the engine, transferred into the next target.
    fn tick(&mut self) -> Option<Note> {
        if !self.prepared || self.width == 0 || self.height == 0 || unsafe { self.engine.IsPaused() }.as_bool() {
            return None;
        }
        let pts = unsafe { self.engine.OnVideoStreamTick() }.ok()?;
        if self.last_pts == Some(pts) {
            return None;
        }
        self.last_pts = Some(pts);
        let i = self.write;
        if self.targets[i].is_none() {
            let fmt = if self.nv12 { DXGI_FORMAT_NV12 } else { DXGI_FORMAT_B8G8R8A8_UNORM };
            // some drivers refuse RENDER_TARGET on NV12: sampled only then
            let made = new_texture_on(&self.device, self.width, self.height, fmt, true).or_else(|_| new_texture_on(&self.device, self.width, self.height, fmt, false));
            match made {
                Ok(t) => self.targets[i] = Some(t),
                Err(e) => return Some(Note::Error(format!("media engine transfer target: {e}"))),
            }
        }
        let target = self.targets[i].clone()?;
        let dst = RECT { left: 0, top: 0, right: self.width as i32, bottom: self.height as i32 };
        let border = MFARGB { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbAlpha: 0 };
        let unknown = unsafe { IUnknown::query(target.as_raw()) }.ok()?;
        if let Err(e) = unsafe { self.engine.TransferVideoFrame(&unknown, None, &dst, Some(&border)) } {
            if self.nv12 {
                // NV12 transfer is unusable on this machine: the engine again, in BGRA
                eprintln!("[hal-media] video: NV12 TransferVideoFrame failed ({e}); reopening the engine with BGRA output");
                let position = unsafe { self.engine.GetCurrentTime() };
                match EngineSession::open(&self.device, &self.url, self.looping, false, self.tx.clone()) {
                    Ok(mut bgra) => {
                        std::mem::swap(self, &mut bgra);
                        // it prepares again (the app is not told twice); the position carries over
                        if position.is_finite() && position > 0.0 {
                            let _ = unsafe { self.engine.SetCurrentTime(position) };
                        }
                        let _ = unsafe { self.engine.Play() };
                    }
                    Err(e) => return Some(Note::Error(e)),
                }
            }
            return None;
        }
        self.write = (i + 1) % self.targets.len();
        let secs = unsafe { self.engine.GetCurrentTime() };
        let position_ms = if secs.is_finite() && secs >= 0.0 { (secs * 1000.0) as u64 } else { 0 };
        Some(Note::Frame(Frame { texture: target, slice: 0, origin: (0, 0), width: self.width, height: self.height, nv12: self.nv12, position_ms, matrix: if self.height >= 720 { 0.0 } else { 1.0 }, full_range: false, _keep: None }))
    }
}

/// The MIME types the engine says it plays (`can_play_type`).
pub fn can_play_type(mime: &str) -> &'static str {
    let base = mime.split(';').next().unwrap_or("").trim();
    match base {
        "video/mp4" | "video/x-m4v" | "audio/mp4" | "audio/x-m4a" | "audio/mpeg" | "audio/wav" | "audio/x-wav" => "probably",
        _ if base.starts_with("video/") || base.starts_with("audio/") => "maybe",
        _ => "",
    }
}
