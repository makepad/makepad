//! Image-based lighting for Splash materials (KERNELS.md §3.3.3). Opt-in:
//! `World::environment.ibl` names an environment, the renderer builds its
//! lane texture once per change (render-material's ibl.rs: the prefiltered
//! atlas, SH9 and a meta row) and binds it on the detail slot of the
//! materials compiled with IBL. Without it nothing is built or bound, and
//! every stock lane keeps its analytic sky reflection.
use super::*;
use makepad_render_material::ibl::{ibl_texture, EnvMap, EnvPreset};
use makepad_scene::{Background, Environment, IblSource, TextureRef};
use std::collections::HashMap;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.geom

    // The environment as the background (`Background::Environment`): a
    // fullscreen quad drawn first in the scene pass, looking up the view
    // ray in the IBL texture at the background's blur. The lookups are the
    // IBL material's own (render-material builtin.rs), on `detail_map`.
    mod.draw.DrawEnvBackground = mod.std.set_type_default() do #(DrawEnvBackground::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        detail_map: texture_2d(float)
        v_ndc: varying(vec2f)
        mat_ibl_meta: mod.draw.mat_ibl_meta
        mat_ibl_dir: mod.draw.mat_ibl_dir
        mat_ibl_level: mod.draw.mat_ibl_level
        mat_ibl_sky_env: mod.draw.mat_ibl_sky_env
        vertex: fn() {
            let p = self.geom.pos * 2.0 - vec2(1.0, 1.0)
            self.v_ndc = p
            self.vertex_pos = vec4(p.x, p.y, 0.9999, 1.0)
        }
        pixel: fn() {
            // The view ray through this pixel, in world space.
            let v = vec4(self.v_ndc.x * self.bg.z, self.v_ndc.y * self.bg.w, -1.0, 0.0)
            let d = normalize((self.draw_pass.camera_inv * v).xyz)
            return vec4(self.mat_ibl_sky_env(d, self.bg.x) * self.bg.y, 1.0)
        }
        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// The environment background's draw.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawEnvBackground {
    #[deref]
    pub draw_vars: DrawVars,
    /// x = blur (0..1), y = intensity over the IBL's, zw = the projection's
    /// inverse x and y scales (view ray from the NDC).
    #[live(vec4(0.0, 1.0, 1.0, 1.0))]
    pub bg: Vec4f,
}

#[derive(Default)]
pub(super) struct IblState {
    /// Host-registered HDR environments, by the handle documents name.
    maps: HashMap<TextureRef, Arc<EnvMap>>,
    /// What `texture` was built from: (source, intensity bits, rotation bits).
    key: Option<(IblSource, u32, u32)>,
    texture: Option<Texture>,
    background: Option<Box<DrawEnvBackground>>,
}

/// The built-in environments `IblSource::Procedural` indexes, in order.
pub const PROCEDURAL_ENVIRONMENTS: &[&str] = &["studio", "softbox", "sunset", "overcast", "night", "neon", "gradient"];

impl Renderer {
    /// Make an HDR environment available to `IblSource::Hdri(texture)`.
    /// Decoding (and its budget) is the host's: render-material's
    /// `ibl::load_hdr` confines it.
    pub fn register_environment(&mut self, texture: TextureRef, env: Arc<EnvMap>) {
        if self.ibl.key.is_some_and(|k| k.0 == IblSource::Hdri(texture)) {
            self.ibl.key = None;
        }
        self.ibl.maps.insert(texture, env);
    }

    /// The IBL lane texture for this frame, when the environment asks for one.
    pub fn ibl_texture(&self) -> Option<&Texture> {
        self.ibl.texture.as_ref()
    }

    /// Build (on change) or drop the lane texture for `env`.
    pub(super) fn resolve_ibl(&mut self, cx: &mut Cx, env: &Environment) {
        let Some(ibl) = env.ibl.filter(|i| i.intensity.is_finite() && i.rotation_deg.is_finite()) else {
            self.ibl.key = None;
            self.ibl.texture = None;
            return;
        };
        let key = (ibl.source, ibl.intensity.to_bits(), ibl.rotation_deg.to_bits());
        if self.ibl.key == Some(key) && self.ibl.texture.is_some() {
            return;
        }
        let map = match ibl.source {
            IblSource::Hdri(t) => self.ibl.maps.get(&t).cloned(),
            IblSource::Procedural(i) => PROCEDURAL_ENVIRONMENTS.get(i as usize)
                .and_then(|name| EnvPreset::by_name(name))
                .map(|p| Arc::new(EnvMap::procedural(&p, 256, 1.0, 0.0))),
        };
        let Some(map) = map else {
            self.ibl.key = None;
            self.ibl.texture = None;
            return;
        };
        let t = ibl_texture(&map, ibl.intensity, ibl.rotation_deg);
        self.ibl.texture = Some(Texture::new_with_format(cx, TextureFormat::VecRGBAf32 {
            width: t.width,
            height: t.height,
            data: Some(t.data),
            updated: TextureUpdated::Full,
        }));
        self.ibl.key = Some(key);
    }

    /// Draw the environment as the background when the world asks for it
    /// (first in the scene pass: everything after draws over it).
    pub(super) fn draw_environment_background(&mut self, cx: &mut Cx3d, env: &Environment, projection: &Mat4f) {
        let Background::Environment { blur, intensity } = env.background else { return };
        let Some(texture) = self.ibl.texture.clone() else { return };
        if self.ibl.background.is_none() {
            self.ibl.background = cx.cx.try_with_vm(|vm| {
                makepad_render_material::builtin::register(vm);
                let draw = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str("draw").into(), NoTrap).as_object();
                let have = draw.is_some_and(|d| {
                    let v = vm.bx.heap.value(d, LiveId::from_str("DrawEnvBackground").into(), NoTrap);
                    !v.is_nil() && !v.is_err()
                });
                if !have {
                    script_mod(vm);
                }
                Box::new(DrawEnvBackground::script_new_with_default(vm))
            });
        }
        let Some(d) = self.ibl.background.as_mut() else { return };
        let (px, py) = (projection.v[0], projection.v[5]);
        d.bg = vec4(blur.clamp(0.0, 1.0), intensity.max(0.0), 1.0 / if px.abs() > 1e-6 { px } else { 1.0 }, 1.0 / if py.abs() > 1e-6 { py } else { 1.0 });
        d.draw_vars.options.depth_write = false;
        d.draw_vars.set_texture(0, &texture);
        if d.draw_vars.can_instance() {
            cx.add_instance(&d.draw_vars);
        }
    }
}
