//! Real-time VFX in the renderer: billboard particles expanded on the GPU
//! from burst records (particles.rs), soft against the scene's depth,
//! additive or alpha, HDR-emissive so they bloom, flipbook-textured from a
//! generated atlas, lit by the sun where they are smoke or dust; plus
//! device-local surface decals.
//!
//! Two ways to draw, chosen per host (fab and Stage need nothing):
//! * **In the scene pass** (the default): particles join the late
//!   transparent layer, hardware depth-tested, without the soft fade.
//! * **In their own pass** (`Renderer::set_vfx_pass(true)` + `run_vfx`
//!   after the scene pass): drawn over the finished scene target while
//!   sampling its depth, so every particle fades softly into what it
//!   touches. The sandbox opts in.

pub mod atlas;
pub mod shader;

pub use shader::{DrawSceneVfx, DrawSceneVfxDecal};

use crate::particles::{ParticleInstance, VfxDecalInstance};
use makepad_draw::*;
use std::sync::mpsc;

/// Everything the VFX lanes keep between frames.
#[derive(Default)]
pub(crate) struct VfxState {
    /// This frame's burst / explicit records (host-set, then consumed).
    pub records: Vec<ParticleInstance>,
    pub decals: Vec<VfxDecalInstance>,
    /// Evaluate particles on the CPU instead of in the vertex shader.
    pub cpu_sim: bool,
    /// The host draws particles in their own pass after the scene.
    pub pass_enabled: bool,
    /// Set by `draw_scene_full` when it left this frame's particles for the
    /// pass; `run_vfx` consumes it.
    pub pending: Option<VfxFrame>,
    pub draw_scene: Option<Box<DrawSceneVfx>>,
    pub draw_pass: Option<Box<DrawSceneVfx>>,
    pub draw_decal: Option<Box<DrawSceneVfxDecal>>,
    pub sheet: Option<Geometry>,
    pub single: Option<Geometry>,
    pub atlas: Option<Texture>,
    pub atlas_rx: Option<mpsc::Receiver<Vec<u32>>>,
    pub placeholder: Option<Texture>,
    pub pass: Option<(DrawPass, DrawList)>,
    pub gpu_ms: f64,
    gpu_log: (u32, f64),
    /// Scratch: (sort key, chunk) of this frame.
    pub chunks: Vec<(f32, ParticleInstance)>,
}

/// What `draw_scene_full` saw, for the separate pass.
#[derive(Clone, Copy)]
pub(crate) struct VfxFrame {
    pub scene: SceneState3D,
    pub stage: Mat4f,
    pub light: VfxLight,
    pub scale: f32,
}

/// The frame's light as the VFX shaders take it.
#[derive(Clone, Copy)]
pub(crate) struct VfxLight {
    pub dir: Vec3f,
    pub sun: Vec3f,
    pub sky: Vec3f,
    pub fog: Vec3f,
    pub fog_density: f32,
    pub lin: [f32; 4],
}

impl VfxState {
    /// The shared particle sheets: `n` quads (the burst sheet, or 1 for
    /// explicit particles) in the CubeVertex layout, `geom_id` = the quad's
    /// index in the sheet.
    pub fn sheet_geometry(&mut self, cx: &mut Cx, n: usize) -> GeometryId {
        let slot = if n == 1 { &mut self.single } else { &mut self.sheet };
        if let Some(g) = slot {
            return g.geometry_id();
        }
        let mut vertices: Vec<f32> = Vec::with_capacity(n * 4 * 12);
        let mut indices: Vec<u32> = Vec::with_capacity(n * 6);
        for i in 0..n {
            let base = (i * 4) as u32;
            for (qx, qy) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                // geom_pos(3), geom_id(1), geom_normal(3), geom_pad(1),
                // geom_uv(2), tail_pad(2) — the spark-sheet layout.
                vertices.extend_from_slice(&[qx, qy, 0.0, i as f32, 0.0, 0.0, 1.0, 0.0, qx + 0.5, 0.5 - qy, 0.0, 0.0]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let geometry = Geometry::new(cx);
        geometry.update(cx, indices, vertices);
        let id = geometry.geometry_id();
        *slot = Some(geometry);
        id
    }

    /// The flipbook atlas once it has been generated (kicked off on first
    /// use, on a worker thread), else a 1×1 placeholder and `false`.
    pub fn atlas_texture(&mut self, cx: &mut Cx) -> (Texture, bool) {
        if self.atlas.is_none() {
            if self.atlas_rx.is_none() {
                let (tx, rx) = mpsc::channel();
                let _ = std::thread::Builder::new().name("vfx-atlas".into()).spawn(move || {
                    let _ = tx.send(atlas::build_atlas_mips());
                });
                self.atlas_rx = Some(rx);
            }
            if let Some(texels) = self.atlas_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                self.atlas_rx = None;
                self.atlas = Some(Texture::new_with_format(
                    cx,
                    TextureFormat::VecMipBGRAu8_32 {
                        width: atlas::ATLAS_SIZE,
                        height: atlas::ATLAS_SIZE,
                        data: Some(texels),
                        max_level: Some(atlas::ATLAS_MIPS),
                        wrap: TextureWrap::ClampToEdge,
                        updated: TextureUpdated::Full,
                    },
                ));
            }
        }
        if let Some(t) = &self.atlas {
            return (t.clone(), true);
        }
        let placeholder = self.placeholder.get_or_insert_with(|| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 { width: 1, height: 1, data: Some(vec![0]), updated: TextureUpdated::Full },
            )
        });
        (placeholder.clone(), false)
    }

    pub fn note_gpu_ms(&mut self, ms: f64) {
        self.gpu_ms = ms;
        if std::env::var_os("SANDBOX_VFX_STATS").is_some() {
            self.gpu_log.0 += 1;
            self.gpu_log.1 += ms;
            if self.gpu_log.0 >= 120 {
                log!("vfx pass GPU: mean {:.3} ms over {} frames", self.gpu_log.1 / self.gpu_log.0 as f64, self.gpu_log.0);
                self.gpu_log = (0, 0.0);
            }
        }
    }
}

/// A conservative bounding radius of a burst record around its origin.
pub(crate) fn record_radius(r: &ParticleInstance) -> f32 {
    if r.is_explicit() {
        return r.size.x.max(r.size.y) + r.look.z * vec3f(r.dir.x, r.dir.y, r.dir.z).length();
    }
    let t = r.end();
    let k = r.size.w.max(0.001);
    // Flight under drag is bounded by v0/k; gravity adds ½gt² at most.
    let flight = (r.life.w / k).min(r.life.w * t) + 0.5 * r.size.z.abs() * t * t;
    let path = vec3f(r.path.x, r.path.y, r.path.z).length() * r.path.w
        + vec3f(r.inherit.x, r.inherit.y, r.inherit.z).length() * t;
    flight + path + r.extra.x + r.extra2.z + r.size.x.max(r.size.y) * 1.25
}
