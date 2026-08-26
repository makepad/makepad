//! Lane B. The ambient-occlusion bake: off the UI thread, cached per model.
//!
//! `libs/render`'s realtime rig gives us the sun (cascaded shadow maps,
//! `GpuLightmapMode::Realtime`) but not the contact darkening that makes an
//! architectural model read as built rather than as a stack of primitives —
//! that is what the AO bake is for, and it is the one part of the lighting a
//! BIM model can carry in its own vertex stream.
//!
//! * `makepad_render::ao::AoSampler` is the SAME sampler `bake_vertex_ao`
//!   and the atlas baker use, so nothing here invents its own occlusion.
//! * It runs on worker threads over plain `Vec<Vec3f>` (no `Cx`, no GPU
//!   handle ever crosses a thread) and reports through `Cx::post_action`.
//! * The answer is filed under a content hash of the geometry in
//!   the per-user cache directory (`FAB_CACHE_DIR` overrides), so opening
//!   the same building twice bakes once.

use crate::api::*;
use makepad_render::ao::AoSampler;
use makepad_widgets::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Matches `ao.rs`'s own budget ladder — a dense building gets fewer rays
/// per vertex, which is the only reason a 700k-triangle villa bakes in
/// seconds instead of minutes.
fn ray_budget(vertices: usize) -> usize {
    if vertices > 8_000 {
        3
    } else if vertices > 4_000 {
        6
    } else {
        12
    }
}

fn cache_dir() -> PathBuf {
    // A per-user cache, never a path relative to whatever the current
    // directory happens to be (tests run from the crate directory and used
    // to leave `apps/fab/local/` behind). `FAB_CACHE_DIR` overrides.
    if let Ok(dir) = std::env::var("FAB_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        let base = if cfg!(target_os = "macos") { "Library/Caches" } else { ".cache" };
        return PathBuf::from(home).join(base).join("makepad-fab");
    }
    std::env::temp_dir().join("makepad-fab-cache")
}

fn cache_path(hash: u64) -> PathBuf {
    cache_dir().join(format!("ao-{hash:016x}.f32"))
}

fn read_cache(hash: u64, expect: usize) -> Option<Vec<f32>> {
    let bytes = std::fs::read(cache_path(hash)).ok()?;
    if bytes.len() != expect * 4 {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

fn write_cache(hash: u64, ao: &[f32]) {
    let _ = std::fs::create_dir_all(cache_dir());
    let mut out = Vec::with_capacity(ao.len() * 4);
    for v in ao {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let _ = std::fs::write(cache_path(hash), out);
}

/// Posted by the worker when the occlusion is ready. Lane B's `apply` hook
/// hands it to whichever viewports are showing that scene.
#[derive(Debug)]
pub struct AoBaked {
    pub hash: u64,
    pub ao: super::AoValues,
}

/// One bake in flight. A newer scene bumps `token` and the older result is
/// dropped on arrival rather than half-applied.
#[derive(Default)]
pub struct AoBake {
    running: Option<u64>,
    token: Arc<AtomicU64>,
}

impl AoBake {
    /// Start (or reuse) a bake for `hash`. Returns the cached occlusion
    /// immediately when there is one — the common case after the first open.
    pub fn start(
        &mut self,
        cx: &mut Cx,
        hash: u64,
        scene: Arc<Scene>,
    ) -> Option<super::AoValues> {
        if self.running == Some(hash) {
            return None;
        }
        let merged = super::pack::merge(&scene);
        let n = merged.positions.len();
        if n == 0 {
            return None;
        }
        if let Some(ao) = read_cache(hash, n) {
            self.running = Some(hash);
            return Some(Arc::new(ao));
        }
        self.running = Some(hash);
        let token = self.token.clone();
        let seq = token.fetch_add(1, Ordering::SeqCst) + 1;
        cx.action(ShellAction::StatusMessage(format!(
            "Baking ambient occlusion — {} vertices…",
            n
        )));
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let bounds = scene.bounds;
            let (min, max) = if aabb_is_empty(&bounds) {
                (vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
            } else {
                (bounds.min, bounds.max)
            };
            let rays = ray_budget(n);
            let sampler = AoSampler::new(&merged.positions, &merged.indices, min, max, rays);
            let threads = std::thread::available_parallelism()
                .map(|p| p.get().min(12).max(1))
                .unwrap_or(4);
            let chunk = (n + threads - 1) / threads;
            let mut ao = vec![1.0f32; n];
            std::thread::scope(|s| {
                let sampler = &sampler;
                let merged = &merged;
                for (t, out) in ao.chunks_mut(chunk).enumerate() {
                    let base = t * chunk;
                    s.spawn(move || {
                        for (i, slot) in out.iter_mut().enumerate() {
                            let v = base + i;
                            *slot = sampler.at(
                                &merged.positions,
                                &merged.indices,
                                merged.positions[v],
                                merged.normals[v],
                            );
                        }
                    });
                }
            });
            if token.load(Ordering::SeqCst) != seq {
                return;
            }
            write_cache(hash, &ao);
            let ms = t0.elapsed().as_secs_f32();
            Cx::post_action(ShellAction::StatusMessage(format!(
                "Ambient occlusion baked in {ms:.1} s"
            )));
            Cx::post_action(AoBaked {
                hash,
                ao: Arc::new(ao),
            });
        });
        None
    }

    /// Drop anything in flight (a new model arrived).
    pub fn cancel(&mut self) {
        self.token.fetch_add(1, Ordering::SeqCst);
        self.running = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_budget_follows_the_libs_render_ladder() {
        assert_eq!(ray_budget(100), 12);
        assert_eq!(ray_budget(5_000), 6);
        assert_eq!(ray_budget(500_000), 3);
    }

    #[test]
    fn a_baked_house_is_darker_in_its_corners_than_on_its_roof() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let merged = super::super::pack::merge(&scene);
        let b = scene.bounds;
        let sampler = AoSampler::new(&merged.positions, &merged.indices, b.min, b.max, 12);
        let mut top = 0.0f32;
        let mut low = 1.0f32;
        for i in 0..merged.positions.len() {
            let v = sampler.at(
                &merged.positions,
                &merged.indices,
                merged.positions[i],
                merged.normals[i],
            );
            top = top.max(v);
            low = low.min(v);
            assert!((0.0..=1.0).contains(&v), "occlusion out of range: {v}");
        }
        assert!(top > low, "a house with rooms must have some occlusion");
        assert!(top > 0.9, "an unobstructed roof vertex must come out lit");
    }
}
