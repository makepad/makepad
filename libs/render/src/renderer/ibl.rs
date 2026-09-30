//! Image-based lighting for Splash materials (KERNELS.md §3.3.3). Opt-in:
//! `World::environment.ibl` names an environment, the renderer builds its
//! lane texture once per change (render-material's ibl.rs: the prefiltered
//! atlas, SH9 and a meta row) and binds it on the detail slot of the
//! materials compiled with IBL. Without it nothing is built or bound, and
//! every stock lane keeps its analytic sky reflection.
use super::*;
use makepad_render_material::ibl::{ibl_texture, EnvMap, EnvPreset};
use makepad_scene::{Environment, IblSource, TextureRef};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct IblState {
    /// Host-registered HDR environments, by the handle documents name.
    maps: HashMap<TextureRef, Arc<EnvMap>>,
    /// What `texture` was built from: (source, intensity bits, rotation bits).
    key: Option<(IblSource, u32, u32)>,
    texture: Option<Texture>,
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
}
