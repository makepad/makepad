//! Replay: a capture re-executed in a private HAL of the backend kind it was recorded on
//! (offscreen windows, its own device and session, nothing shared with the app). Every resource
//! is created under a fresh id with its captured contents, every pipeline from its captured blob,
//! and the frame is submitted cut after any draw: passes before the cut whole, the cut's pass up
//! to it, nothing after. The window's passes draw into a texture of the window's pixel size, so
//! any target can be read back after any draw.
//!
//! The same frame replays identically (the replay is deterministic); what a replay cannot show
//! is said: depth texture contents are not uploaded (the frame's own clears give them back),
//! surfaces lent by other sessions are their captured image.

use super::*;
use crate::buffers::{CmdWriter, EventStore};
use crate::events::HalEvent;
use crate::Hal;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Where a replay stops: passes before `pass` whole, then the first `draws` draws of `pass`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {
    pub pass: usize,
    pub draws: usize,
}

impl Cut {
    /// The cut right after draw `d` (a global draw index).
    pub fn after(c: &Capture, d: usize) -> Cut {
        let pass = c.draws[d].pass as usize;
        Cut { pass, draws: d + 1 - c.passes[pass].draws.off as usize }
    }
    /// The cut right before draw `d`.
    pub fn before(c: &Capture, d: usize) -> Cut {
        let pass = c.draws[d].pass as usize;
        Cut { pass, draws: d - c.passes[pass].draws.off as usize }
    }
    /// The whole frame.
    pub fn all(c: &Capture) -> Cut {
        match c.passes.len() {
            0 => Cut { pass: 0, draws: 0 },
            n => Cut { pass: n - 1, draws: c.passes[n - 1].draws.len as usize },
        }
    }
}

/// Pixels read back, as RGBA8 for viewing (`native`: the texels as stored, for formats that are
/// not 8-bit colour).
#[derive(Clone, Debug, Default)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub format: u32,
}

impl Image {
    pub fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        self.rgba.get(i..i + 4).map_or([0; 4], |p| [p[0], p[1], p[2], p[3]])
    }
}

/// How a replayed frame is changed.
#[derive(Clone, Copy, Debug, Default)]
pub struct Opts {
    /// draw this one draw `times` times in place (GPU timing of one draw)
    pub repeat: Option<(usize, u32)>,
    /// leave this draw out
    pub skip: Option<usize>,
    /// draw this draw once more right after itself, as its triangles' edges (the mesh overlay)
    pub wire: Option<usize>,
}

pub struct Replayer {
    hal: Arc<Hal>,
    sid: u32,
    store: EventStore,
    tex: HashMap<Id, Id>,
    buf: HashMap<Id, Id>,
    pipe: HashMap<Id, Id>,
    /// the texture the window's passes draw into
    rt: Id,
    frame: u64,
    req: u32,
    pub backend: u32,
    /// pipelines that failed to compile in the replay (id, log)
    pub failed: Vec<(Id, String)>,
    /// GPU time of the newest replayed frame (0: the backend reports none)
    pub last_gpu_ns: u64,
}

fn backend_name(b: u32) -> &'static str {
    match b {
        backend_id::METAL => "Metal",
        backend_id::VULKAN => "Vulkan",
        backend_id::D3D11 => "Direct3D 11",
        backend_id::GL => "GLES",
        backend_id::HEADLESS => "headless",
        _ => "unknown",
    }
}

impl Replayer {
    /// A private HAL with every resource of the capture created and every pipeline compiled.
    pub fn new(c: &Capture) -> Result<Replayer, String> {
        let hal = Hal::new();
        let info = hal.open(&OpenReq { backend: c.backend, flags: open_flags::OFFSCREEN_WINDOWS, dpi_hint: c.dpi.max(1.0), ..Default::default() });
        if info.backend != c.backend || info.dialect != c.dialect {
            return Err(format!(
                "this capture was recorded on {} and replays only there (this machine opened {})",
                backend_name(c.backend),
                backend_name(info.backend)
            ));
        }
        let sid = info.session;
        let biggest = c.textures.iter().map(|t| format::level_bytes(t.format, t.w, t.h) as usize).max().unwrap_or(0);
        let window = format::level_bytes(format::BGRA8, c.size[0], c.size[1]) as usize;
        let store = EventStore::new(sid, 256, biggest.max(window) + (1 << 20));
        let mut r = Replayer { hal, sid, store, tex: HashMap::new(), buf: HashMap::new(), pipe: HashMap::new(), rt: Id::NONE, frame: 1, req: 1, backend: info.backend, failed: Vec::new(), last_gpu_ns: 0 };
        let mut w = CmdWriter::new(sid);
        let mut next = 1u32;
        let mut fresh = || {
            let id = Id::new(next, 1);
            next += 1;
            id
        };
        for t in &c.textures {
            let id = fresh();
            r.tex.insert(t.id, id);
            w.texture_create(id, t.usage | usage::SAMPLED, t.format, t.w, t.h, [t.depth.min(u16::MAX as u32) as u16, t.mips.min(u16::MAX as u32) as u16], Attr::default());
            let Some(bytes) = t.contents.as_ref().filter(|_| !format::is_depth(t.format)) else { continue };
            let mut at = 0usize;
            for level in 0..t.mips.max(1) {
                let (lw, lh) = ((t.w >> level).max(1), (t.h >> level).max(1));
                let n = format::level_bytes(t.format, lw, lh) as usize;
                for layer in 0..t.depth.max(1) {
                    let Some(part) = bytes.get(at..at + n) else { break };
                    w.texture_update(id, level as u16, [0, 0, lw, lh], layer, part);
                    at += n;
                }
            }
        }
        if c.passes.iter().any(|p| p.is_window()) {
            r.rt = fresh();
            w.texture_create(r.rt, usage::RENDER_TARGET | usage::SAMPLED, format::BGRA8, c.size[0].max(1), c.size[1].max(1), [1, 1], Attr::default());
        }
        let mut next_buf = 1u32;
        for b in &c.buffers {
            let id = Id::new(next_buf, 1);
            next_buf += 1;
            r.buf.insert(b.id, id);
            w.buffer_create(id, b.kind, b.size, Attr::default());
            if let Some(bytes) = &b.contents {
                w.buffer_update(id, 0, &bytes[..bytes.len().min(b.size as usize)]);
            }
        }
        let mut next_pipe = 1u32;
        for p in &c.pipelines {
            let id = Id::new(next_pipe, 1);
            next_pipe += 1;
            r.pipe.insert(p.id, id);
            w.pipeline_create(id, p.dialect, Attr::default(), &p.blob);
        }
        r.hal.submit(&w.buf());
        // every pipeline answers once: ready or failed
        let mut waiting: Vec<Id> = r.pipe.values().copied().collect();
        let mut failed = Vec::new();
        let back: HashMap<Id, Id> = r.pipe.iter().map(|(a, b)| (*b, *a)).collect();
        r.until(Duration::from_secs(60), |e| {
            match e {
                HalEvent::PipelineReady { pipeline } => waiting.retain(|p| *p != pipeline),
                HalEvent::PipelineFailed { pipeline, log } => {
                    waiting.retain(|p| *p != pipeline);
                    failed.push((back.get(&pipeline).copied().unwrap_or(pipeline), String::from_utf8_lossy(log).into_owned()));
                }
                _ => {}
            }
            waiting.is_empty()
        })
        .map_err(|_| format!("{} pipelines did not compile within 60 s", waiting.len()))?;
        r.failed = failed;
        let bad = r.hal.with_world(|w| w.session(r.sid).map_or(0, |s| s.stats.bad_args));
        if bad != 0 {
            return Err(format!("the replay HAL refused {bad} of the capture's commands"));
        }
        Ok(r)
    }

    /// Pumps until `accept` says so; Err on the deadline.
    fn until(&mut self, timeout: Duration, mut accept: impl FnMut(HalEvent<'_>) -> bool) -> Result<(), ()> {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(());
            }
            let mut buf = self.store.buf();
            let n = self.hal.pump(20_000_000, &mut buf);
            let len = buf.arena_len;
            let mut grow = None;
            for (_, e) in self.store.reader(n, len) {
                if let HalEvent::PayloadSize { bytes } = e {
                    grow = Some(bytes as usize);
                    continue;
                }
                if accept(e) {
                    return Ok(());
                }
            }
            if let Some(bytes) = grow {
                self.store = EventStore::new(self.sid, 256, bytes + (1 << 20));
            }
        }
    }

    fn map_tex(&self, id: Id) -> Id {
        if id.is_none() {
            return id;
        }
        self.tex.get(&id).copied().unwrap_or(Id::NONE)
    }

    fn map_buf(&self, id: Id) -> Id {
        if id.is_none() {
            return id;
        }
        self.buf.get(&id).copied().unwrap_or(Id::NONE)
    }

    /// The replay id of the colour target draw pass `pass` renders (attachment 0).
    pub fn target_of(&self, c: &Capture, pass: usize) -> Id {
        let p = &c.passes[pass];
        if p.is_window() {
            self.rt
        } else {
            self.map_tex(p.color[0])
        }
    }

    /// Replays the frame up to `cut`; waits for it to finish. A draw the backend left out (a
    /// pipeline variant still compiling for its target) makes the frame run again.
    pub fn run(&mut self, c: &Capture, cut: Cut, opts: Opts) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let before = self.hal.with_world(|w| w.session(self.sid).map(|s| s.stats)).unwrap_or_default();
            let frame = self.frame;
            self.frame += 1;
            let mut w = CmdWriter::new(self.sid);
            w.frame_begin(frame, FrameMeta::default());
            let mut expected = 0u64;
            for (pi, p) in c.passes.iter().enumerate().take(cut.pass + 1) {
                let pass = Pass {
                    color: if p.is_window() { [self.rt, Id::NONE, Id::NONE, Id::NONE] } else { p.color.map(|i| self.map_tex(i)) },
                    depth: self.map_tex(p.depth),
                    load: p.load,
                    clear: p.clear,
                    clear_depth: p.clear_depth,
                    layer: p.layer,
                    viewport: p.viewport,
                    dpi: p.dpi,
                    uniforms: Slice::from_bytes(&p.uniforms),
                    draws: Range::default(),
                    name: 0,
                    owner: 0,
                };
                w.pass(pass);
                let n = if pi == cut.pass { cut.draws.min(p.draws.len as usize) } else { p.draws.len as usize };
                for di in p.draws.off as usize..p.draws.off as usize + n {
                    if opts.skip == Some(di) {
                        continue;
                    }
                    let times = match opts.repeat {
                        Some((d, t)) if d == di => t.max(1),
                        _ => 1,
                    };
                    let d = &c.draws[di];
                    let Some(pipeline) = self.pipe.get(&d.pipeline).copied() else { continue };
                    let inst_ranges = w.put_pairs(&d.inst_ranges);
                    let tex_ids: Vec<Id> = d.tex.iter().map(|t| self.map_tex(*t)).collect();
                    let tex = w.put_ids(&tex_ids);
                    let draw = Draw {
                        pipeline,
                        vb: self.map_buf(d.vb),
                        ib: self.map_buf(d.ib),
                        index: d.index,
                        inst: Inst {
                            kind: d.inst_kind,
                            id: if d.inst_kind == Inst::BUFFER { self.map_buf(d.inst_id) } else { Id::NONE },
                            off: d.inst_off,
                            count: d.inst_count,
                            data: if d.inst_kind == Inst::INLINE { Slice::from_bytes(&d.inst_data) } else { Slice::EMPTY },
                        },
                        inst_ranges,
                        uniforms: [Slice::from_bytes(&d.uniforms[0]), Slice::from_bytes(&d.uniforms[1]), Slice::from_bytes(&d.uniforms[2]), Slice::from_bytes(&d.uniforms[3])],
                        ubufs: d.ubufs.map(|b| self.map_buf(b)),
                        tex,
                        pick_tex: Range::default(),
                        scissor: d.scissor,
                        zbias: d.zbias,
                        pick: 0,
                        flags: d.flags,
                        owner: 0,
                        list: d.list,
                        item: d.item,
                        list_name: 0,
                        bounds: d.bounds,
                    };
                    for _ in 0..times {
                        w.draw(draw);
                        expected += 1;
                    }
                    if opts.wire == Some(di) {
                        w.draw(Draw { flags: (draw.flags | Draw::WIREFRAME) & !Draw::CULL, ..draw });
                        expected += 1;
                    }
                }
            }
            let target = if self.rt.is_none() { self.target_of(c, cut.pass) } else { self.rt };
            w.frame_end(target, target::TEXTURE);
            self.hal.submit(&w.buf());
            let (mut gpu, mut partial) = (0, false);
            self.until(Duration::from_secs(30), |e| match e {
                HalEvent::FramePartial { frame: f, .. } if f == frame => {
                    partial = true;
                    false
                }
                HalEvent::FrameDone { frame: done, gpu_ns, .. } if done == frame => {
                    gpu = gpu_ns;
                    true
                }
                _ => false,
            })
            .map_err(|_| format!("replayed frame {frame} did not finish within 30 s"))?;
            self.last_gpu_ns = gpu;
            let after = self.hal.with_world(|w| w.session(self.sid).map(|s| s.stats)).unwrap_or_default();
            if after.bad_args != before.bad_args {
                return Err(format!("the replay HAL refused {} commands of the frame", after.bad_args - before.bad_args));
            }
            let skipped = after.skipped_draws - before.skipped_draws;
            if (skipped == 0 && !partial) || expected == 0 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("{skipped} draws stayed left out{} (their pipelines never became drawable)", if partial { ", the frame partial" } else { "" }));
            }
        }
    }

    /// Reads `rect` (x, y, w, h; `None`: all) of a replay texture after the last [`run`].
    pub fn read(&mut self, c: &Capture, id: Id, rect: Option<[u32; 4]>) -> Result<Image, String> {
        let (w, h, fmt) = if id == self.rt {
            (c.size[0].max(1), c.size[1].max(1), format::BGRA8)
        } else {
            let orig = self.tex.iter().find(|(_, v)| **v == id).map(|(k, _)| *k).ok_or("no such texture in the replay")?;
            let t = c.texture(orig).ok_or("no such texture in the capture")?;
            (t.w, t.h, t.format)
        };
        let rect = rect.unwrap_or([0, 0, w, h]);
        let req = self.req;
        self.req += 1;
        let mut w2 = CmdWriter::new(self.sid);
        w2.texture_readback(id, req, rect);
        self.hal.submit(&w2.buf());
        let mut out = Err("no answer".to_string());
        self.until(Duration::from_secs(30), |e| match e {
            HalEvent::Readback { req: got, status, rect: r, pixels } if got == req => {
                out = match status {
                    readback::OK => Ok(Image { w: r[2], h: r[3], rgba: pixels.to_vec(), format: fmt }),
                    readback::NATIVE => Ok(Image { w: r[2], h: r[3], rgba: to_rgba8(fmt, pixels), format: fmt }),
                    s => Err(format!("readback status {s}")),
                };
                true
            }
            _ => false,
        })
        .map_err(|_| "readback did not answer within 30 s".to_string())?;
        out
    }
}

/// Texels of a format that is not 8-bit colour, as RGBA8 for viewing: floats clamped to 0..1,
/// one- and two-channel formats in red / green, ids as distinct colours.
pub fn to_rgba8(fmt: u32, px: &[u8]) -> Vec<u8> {
    fn half(b: u16) -> f32 {
        let (s, e, m) = ((b >> 15) as u32, ((b >> 10) & 31) as u32, (b & 1023) as u32);
        let v = match e {
            0 => (m as f32) * (2.0f32).powi(-24),
            31 => if m == 0 { f32::INFINITY } else { f32::NAN },
            _ => f32::from_bits(((e + 112) << 23) | (m << 13)),
        };
        if s == 1 { -v } else { v }
    }
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let u16s = |b: &[u8], i: usize| u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
    let f32s = |b: &[u8], i: usize| f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
    let mut out = Vec::new();
    match fmt {
        format::R8 => px.iter().for_each(|v| out.extend_from_slice(&[*v, *v, *v, 255])),
        format::RG8 => px.chunks_exact(2).for_each(|p| out.extend_from_slice(&[p[0], p[1], 0, 255])),
        format::RGBA16F => px.chunks_exact(8).for_each(|p| out.extend_from_slice(&[q(half(u16s(p, 0))), q(half(u16s(p, 1))), q(half(u16s(p, 2))), q(half(u16s(p, 3)))])),
        format::RG16F => px.chunks_exact(4).for_each(|p| out.extend_from_slice(&[q(half(u16s(p, 0))), q(half(u16s(p, 1))), 0, 255])),
        format::RGBA32F => px.chunks_exact(16).for_each(|p| out.extend_from_slice(&[q(f32s(p, 0)), q(f32s(p, 1)), q(f32s(p, 2)), q(f32s(p, 3))])),
        format::R32F => px.chunks_exact(4).for_each(|p| {
            let v = q(f32s(p, 0));
            out.extend_from_slice(&[v, v, v, 255])
        }),
        format::R32UINT => px.chunks_exact(4).for_each(|p| {
            let id = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            let h = id.wrapping_mul(2654435761);
            out.extend_from_slice(&if id == 0 { [0, 0, 0, 255] } else { [(h >> 24) as u8, (h >> 16) as u8, (h >> 8) as u8, 255] })
        }),
        _ => out.extend(px.iter().copied()),
    }
    out
}
