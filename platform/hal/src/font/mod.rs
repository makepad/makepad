//! The font service (HAL.md 13): the host owns the font files and the glyph atlases, every
//! session draws from them. Layout and shaping stay in the app; sessions open fonts, ask for
//! glyphs and sample the lent atlas textures with the same shaders as before.
//!
//! Commands: `FontOpen`, `GlyphRequest`, `GlyphRelease`. Events: `FontOpened`, `AtlasInfo`,
//! `GlyphsReady`, `AtlasEvicted` (`wire` has the records they carry).

mod atlas;
mod pool;
mod service;
pub mod wire;

pub use service::{FontService, FontStats, SessionFontStats};
pub use wire::{atlas as atlas_kind, font_status, glyph as glyph_kind, kind as request_kind, FontSource, GlyphEntry};

use crate::backend::exec::Gpu;
use crate::cmds::HalCmd;
use crate::session::World;
use std::sync::{atomic::{AtomicU64, Ordering}, Arc};

/// A session incarnation's GPU progress. Completion tickets keep it alive across close and
/// relaunch, when the session id can already refer to another incarnation.
#[derive(Default)]
pub(crate) struct FrameProgress {
    submitted: AtomicU64,
    completed: AtomicU64,
}

impl FrameProgress {
    pub(crate) fn submitted(&self, frame: u64) {
        self.submitted.fetch_max(frame, Ordering::Release);
    }

    pub(crate) fn done(&self, frame: u64) {
        self.completed.fetch_max(frame, Ordering::Release);
    }

    pub(crate) fn pending(&self) -> bool {
        self.completed.load(Ordering::Acquire) < self.submitted.load(Ordering::Acquire)
    }
}

/// The highest submitted frame of every session at eviction, not a moving target. All frames
/// on a device execute in submission order; a FrameDone covers earlier frames on that queue.
pub(super) struct RetireFence(Vec<(Arc<FrameProgress>, u64)>);

impl RetireFence {
    fn capture(world: &World, closed: &[Arc<FrameProgress>]) -> Arc<Self> {
        let mut frames = Vec::new();
        let mut add = |progress: &Arc<FrameProgress>| {
            let frame = progress.submitted.load(Ordering::Acquire);
            if progress.completed.load(Ordering::Acquire) < frame {
                frames.push((progress.clone(), frame));
            }
        };
        for sid in world.session_ids() {
            if let Some(session) = world.session(sid) {
                add(&session.font_frames);
            }
        }
        for progress in closed {
            add(progress);
        }
        Arc::new(Self(frames))
    }

    fn complete(&self) -> bool {
        self.0.iter().all(|(progress, frame)| progress.completed.load(Ordering::Acquire) >= *frame)
    }
}

fn with_service(world: &mut World, f: impl FnOnce(&mut FontService, &mut World)) {
    let mut service = match world.fonts.take() {
        Some(s) => s,
        None => Box::new(FontService::new(world.font_notify.clone())),
    };
    f(&mut service, world);
    world.fonts = Some(service);
}

/// Runs a font command of session `sid`.
pub(crate) fn command(gpu: &mut dyn Gpu, world: &mut World, sid: u32, c: HalCmd<'_>) {
    with_service(world, |service, world| service.command(gpu, world, sid, c));
}

/// Takes what the rasterising workers finished (cheap when there is none).
pub(crate) fn service(gpu: &mut dyn Gpu, world: &mut World) {
    if world.fonts.as_ref().is_some_and(|s| s.has_done()) {
        with_service(world, |service, world| service.service(gpu, world));
    }
}
