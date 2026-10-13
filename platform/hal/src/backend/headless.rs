//! The headless backend: executes the CmdBuf against the session tables and the ledger, with no
//! OS and no GPU. Every command is accepted and validated; resources are accounted (bytes by
//! format and mip chain), frames are walked (ids touched, stale ids and draws without a live
//! pipeline counted) and answered with `FrameDone`, pipelines are ready immediately.
//!
//! Pixels: there is no raster here. `TextureReadback` and `Snapshot` answer `Readback` with
//! status `readback::NO_RASTER` and an empty payload (`readback::STALE` for a dead id).

use super::exec::{self, Gpu, PipeStatus};
use super::Backend;
use crate::buffers::{CmdView, PipelineBlob};
use crate::ledger::Kind;
use crate::session::*;
use crate::types::*;

pub use super::exec::texture_bytes;

#[derive(Default)]
pub struct Headless;

impl Backend for Headless {
    fn session_closed(&mut self, world: &mut World, sid: u32) {
        // the objects are table entries: drop them and their ledger rows (lent windows go back
        // to the host in World::close)
        exec::free_session(world, sid);
    }

    fn host_gpu(&mut self) -> Option<&mut dyn Gpu> {
        Some(self)
    }

    fn info(&self) -> HalInfo {
        HalInfo {
            session: 0,
            backend: backend_id::HEADLESS,
            dialect: dialect::NONE,
            flags: 0,
            formats: (1u64 << format::COUNT) - 1,
            msaa: 1,
            dpi: 1.0,
            max_texture: MAX_TEXTURE,
            max_window: [MAX_TEXTURE, MAX_TEXTURE],
            max_buffer: 0,
            working_set: 0,
        }
    }

    fn submit(&mut self, world: &mut World, sid: u32, view: &CmdView) {
        exec::submit(self, world, sid, view);
    }

    fn step(&mut self, world: &mut World, sid: u32) {
        exec::step_timers(world, sid);
    }

    fn next_due(&self, world: &World, sid: u32) -> Option<u64> {
        exec::next_timer(world, sid)
    }
}

/// The largest texture dimension the headless device takes.
const MAX_TEXTURE: u32 = 16384;

impl Gpu for Headless {
    /// As a device does: a texture past the largest dimension it reports (`max_texture`) gets
    /// no storage.
    fn texture_create(&mut self, _s: &mut Session, _id: Id, t: &mut TextureObj) -> Result<(), String> {
        if t.w > MAX_TEXTURE || t.h > MAX_TEXTURE {
            return Err(format!("{}x{} is past the device's {MAX_TEXTURE} texels", t.w, t.h));
        }
        Ok(())
    }
    fn texture_update(&mut self, _s: &mut Session, _id: Id, _t: &TextureObj, _mip: u16, _rect: [u32; 4], _layer: u32, _data: &[u8]) {}
    fn texture_destroy(&mut self, _sid: u32, _id: Id) {}
    fn texture_readback(&mut self, s: &mut Session, _id: Id, req: u32, rect: [u32; 4]) {
        s.events.readback(req, readback::NO_RASTER, rect, &[]);
    }
    fn texture_share(&mut self, s: &mut Session, id: Id) {
        // no shareable surface: handle 0
        s.events.texture_shared(id, 0);
    }
    fn texture_import(&mut self, _s: &mut Session, _id: Id, _handle: u64, _kind: u32) -> Option<TextureObj> {
        Some(TextureObj { format: format::RGBA8, w: 0, h: 0, depth: 1, mips: 1, usage: usage::SAMPLED, bytes: 0, imported: true, uploaded: 0, lent: None })
    }
    fn buffer_create(&mut self, _s: &mut Session, _id: Id, _b: &mut BufferObj) -> Result<(), String> {
        Ok(())
    }
    fn buffer_update(&mut self, _s: &mut Session, _id: Id, _b: &mut BufferObj, _off: u64, _data: &[u8]) {}
    fn buffer_destroy(&mut self, _sid: u32, _id: Id) {}
    fn buffer_evictable(&mut self, _sid: u32, _id: Id) {}
    fn pipeline_create(&mut self, _s: &mut Session, _id: Id, _dialect: u32, _blob: &PipelineBlob, _attr: Attr) -> PipeStatus {
        PipeStatus::Ready
    }
    fn pipeline_destroy(&mut self, _sid: u32, _id: Id) -> bool {
        false
    }
    fn frame(&mut self, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, now: u64) -> Option<exec::SubmittedFrame> {
        let window = kind & !target::FLAGS == target::WINDOW && kind & target::PICK == 0
            && view.frames.get(packet as usize).is_some_and(|p| {
                let start = p.passes.off as usize;
                start.checked_add(p.passes.len as usize).and_then(|end| view.passes.get(start..end))
                    .is_some_and(|passes| passes.iter().any(|pass| pass.color.iter().all(|id| id.is_none())))
            });
        frame(s, sid, target, kind, packet, view, now).map(|deps| exec::SubmittedFrame {
            deps, window,
        })
    }
    fn pick(&mut self, s: &mut Session, _window: Id, req: u32, _pos: [f32; 2]) {
        s.events.pick_result(req, &[]);
    }
    fn snapshot(&mut self, s: &mut Session, window: Id, req: u32) {
        let g = s.windows.get(s.id, window).map_or([0.0; 4], |e| e.val.geom);
        s.events.readback(req, readback::NO_RASTER, [0, 0, g[2] as u32, g[3] as u32], &[]);
    }
    fn window_closed(&mut self, _sid: u32, _id: Id) {}
    fn window_overlay(&mut self, _s: &mut Session, _id: Id, _rect: Option<[f32; 4]>, _now: u64) {}
    fn completes_inline(&self) -> bool {
        true
    }
}

/// Walks the packet: counts what it renders and marks the handles it uses (the frame is done as
/// it returns: `completes_inline`).
fn frame(s: &mut Session, sid: u32, _target: Id, _kind: u32, packet: u32, view: &CmdView, now: u64) -> Option<Vec<LentRef>> {
    let Some(p) = view.frames.get(packet as usize) else {
        s.stats.bad_args += 1;
        return None;
    };
    let slice = |r: Range, len: usize| {
        let a = r.off as usize;
        a.checked_add(r.len as usize).filter(|e| *e <= len).map(|e| a..e)
    };
    let (Some(pr), Some(_)) = (slice(p.passes, view.passes.len()), slice(p.draws, view.draws.len())) else {
        s.stats.bad_args += 1;
        return None;
    };
    let frame = p.frame;
    s.frame = frame;
    s.font_frames.submitted(frame);
    // the surfaces of other sessions this frame samples (a parent compositing its children)
    let mut deps: Vec<LentRef> = Vec::new();
    s.stats.max_frame_ns = s.stats.max_frame_ns.max(p.meta.build_end_ns.saturating_sub(p.meta.build_start_ns));
    for pass in &view.passes[pr] {
        s.stats.passes += 1;
        for id in pass.color.iter().chain(std::iter::once(&pass.depth)).filter(|i| !i.is_none()) {
            touch(s, sid, Kind::Texture, *id, frame, now);
        }
        let Some(dr) = slice(pass.draws, view.draws.len()) else {
            s.stats.bad_args += 1;
            continue;
        };
        for d in &view.draws[dr] {
            s.stats.draws += 1;
            // a surface lent by a session that is gone (or relaunched) is never sampled
            let mut dead_lent = false;
            for id in crate::buffers::read_ids(view.arena, d.tex).chain(crate::buffers::read_ids(view.arena, d.pick_tex)) {
                match exec::lent_state(s, sid, id) {
                    exec::LentState::Own => {}
                    exec::LentState::Lent(l) => {
                        if !deps.contains(&l) {
                            deps.push(l);
                        }
                    }
                    exec::LentState::Dead => dead_lent = true,
                }
            }
            if dead_lent {
                s.stats.skipped_draws += 1;
                s.stats.stale_lent_draws += 1;
                s.stale();
                continue;
            }
            if s.pipelines.get(sid, d.pipeline).is_err() {
                s.stats.skipped_draws += 1;
                s.stale();
                continue;
            }
            touch(s, sid, Kind::Pipeline, d.pipeline, frame, now);
            for id in [d.vb, d.ib].into_iter().chain(d.ubufs).chain(std::iter::once(d.inst.id)).filter(|i| !i.is_none()) {
                touch(s, sid, Kind::Buffer, id, frame, now);
            }
            for id in crate::buffers::read_ids(view.arena, d.tex).chain(crate::buffers::read_ids(view.arena, d.pick_tex)).filter(|i| !i.is_none()) {
                touch(s, sid, Kind::Texture, id, frame, now);
            }
            if d.inst_ranges.len > 0 {
                let n = crate::buffers::read_pairs(view.arena, d.inst_ranges).count();
                if n != d.inst_ranges.len as usize {
                    s.stats.bad_args += 1;
                }
            }
        }
    }
    s.stats.frames += 1;
    s.font_frames.done(frame);
    Some(deps)
}

fn touch(s: &mut Session, sid: u32, kind: Kind, id: Id, frame: u64, now: u64) {
    let ok = match kind {
        Kind::Texture => s.textures.get(sid, id).is_ok(),
        Kind::Buffer => s.buffers.get(sid, id).is_ok(),
        Kind::Pipeline => s.pipelines.get(sid, id).is_ok(),
        Kind::Window => false,
    };
    if ok {
        s.ledger.touch(kind, id.index, id.gen, frame, now);
    } else {
        s.stale();
    }
}
