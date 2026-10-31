//! The material hook set: the Splash shader functions a material may supply,
//! where each runs, and which ones a host allows.
//!
//! Every hook is a plain shader function with explicit arguments and one
//! result, so it compiles on every backend the shader DSL targets. The base
//! lanes call each hook through a method whose stock body returns its input
//! unchanged (or the stock term, for `light`), so a lane without the hook
//! draws exactly as before.

/// One hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Hook {
    /// `vertex: fn(p: vec3, n: vec3, uv: vec2) -> vec3`: the model-space
    /// position to draw. Runs in the colour pass and in the shadow caster, so
    /// displaced geometry casts a displaced shadow.
    Vertex,
    /// `surface: fn(base: vec4) -> vec4`: the albedo (rgb, linear in the
    /// scene-referred lane) and alpha. Alpha under 0.5 cuts the pixel; under
    /// `Blend::Mask` the cutoff is the material's.
    Surface,
    /// `emission: fn(e: vec3) -> vec3`: emitted light, added after lighting.
    Emission,
    /// `normal: fn(n: vec3) -> vec3`: the world-space shading normal, after
    /// normal mapping.
    Normal,
    /// `metal_rough: fn(mr: vec2) -> vec2`: metallic and roughness.
    MetalRough,
    /// `light: fn(radiance: vec3, l: vec3, n: vec3, v: vec3, brdf: vec3) -> vec3`:
    /// one light's contribution, once per light (the sun with its cascaded
    /// shadow and every clustered light). `radiance` is the light arriving
    /// (colour, falloff and shadow), `l` the direction toward the light, `v`
    /// toward the eye, `brdf` the stock response per unit radiance (N.L,
    /// diffuse and specular). The stock body is `radiance * brdf`.
    Light,
    /// `lighting: fn(direct: vec3, ambient: vec3) -> vec3`: the lit colour from
    /// the summed direct light and the ambient (sky or IBL) term.
    Lighting,
    /// `finish: fn(c: vec4) -> vec4`: the final colour after lighting, before
    /// fog.
    Finish,
}

/// Where a hook runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Pixel,
}

pub struct HookSpec {
    pub hook: Hook,
    /// The key in a material spec (`Material{surface: fn(..) {..}}`).
    pub key: &'static str,
    /// The shader method the hook is installed as.
    pub method: &'static str,
    /// Declared parameter names, in order (their count is checked).
    pub params: &'static [&'static str],
    /// The signature, for guides and error messages.
    pub signature: &'static str,
    pub stage: Stage,
}

pub const HOOKS: &[HookSpec] = &[
    HookSpec { hook: Hook::Vertex, key: "vertex", method: "mat_vertex", params: &["p", "n", "uv"], signature: "fn(p: vec3, n: vec3, uv: vec2) -> vec3", stage: Stage::Vertex },
    HookSpec { hook: Hook::Surface, key: "surface", method: "surface", params: &["base"], signature: "fn(base: vec4) -> vec4", stage: Stage::Pixel },
    HookSpec { hook: Hook::Emission, key: "emission", method: "mat_emission", params: &["e"], signature: "fn(e: vec3) -> vec3", stage: Stage::Pixel },
    HookSpec { hook: Hook::Normal, key: "normal", method: "mat_normal", params: &["n"], signature: "fn(n: vec3) -> vec3", stage: Stage::Pixel },
    HookSpec { hook: Hook::MetalRough, key: "metal_rough", method: "mat_metal_rough", params: &["mr"], signature: "fn(mr: vec2) -> vec2", stage: Stage::Pixel },
    HookSpec { hook: Hook::Light, key: "light", method: "mat_light", params: &["radiance", "l", "n", "v", "brdf"], signature: "fn(radiance: vec3, l: vec3, n: vec3, v: vec3, brdf: vec3) -> vec3", stage: Stage::Pixel },
    HookSpec { hook: Hook::Lighting, key: "lighting", method: "mat_lighting", params: &["direct", "ambient"], signature: "fn(direct: vec3, ambient: vec3) -> vec3", stage: Stage::Pixel },
    HookSpec { hook: Hook::Finish, key: "finish", method: "mat_finish", params: &["c"], signature: "fn(c: vec4) -> vec4", stage: Stage::Pixel },
];

impl Hook {
    pub fn spec(self) -> &'static HookSpec {
        HOOKS.iter().find(|s| s.hook == self).expect("every hook has a spec")
    }
    pub fn from_key(key: &str) -> Option<Hook> {
        HOOKS.iter().find(|s| s.key == key).map(|s| s.hook)
    }
    pub fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// The hooks a host allows. A spec naming a hook outside the mask is
/// refused, not ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HookMask(pub u16);

impl HookMask {
    pub const NONE: Self = Self(0);
    /// Sandbox's `game.material`: the albedo hook only, until it opts in.
    pub const ALBEDO: Self = Self(1 << Hook::Surface as u16);
    pub const ALL: Self = Self((1 << HOOKS.len() as u16) - 1);

    pub fn of(hooks: &[Hook]) -> Self {
        Self(hooks.iter().fold(0, |m, h| m | h.bit()))
    }
    pub fn allows(self, hook: Hook) -> bool {
        self.0 & hook.bit() != 0
    }
    pub fn keys(self) -> Vec<&'static str> {
        HOOKS.iter().filter(|s| self.allows(s.hook)).map(|s| s.key).collect()
    }
}

/// Spec keys that are never hooks: the base shader's entry points and
/// stages stay engine-owned whatever the mask.
pub const ENGINE_OWNED: &[&str] = &["pixel", "fragment", "vertex_pos", "clip", "base_texel", "mat_compose", "sky_env"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hook_has_one_spec_and_masks_select_them() {
        for (i, s) in HOOKS.iter().enumerate() {
            assert_eq!(s.hook as usize, i, "HOOKS is in Hook order");
            assert_eq!(Hook::from_key(s.key), Some(s.hook));
            assert!(!ENGINE_OWNED.contains(&s.key));
        }
        assert!(HookMask::ALL.allows(Hook::Finish) && HookMask::ALL.allows(Hook::Vertex));
        assert!(HookMask::ALBEDO.allows(Hook::Surface) && !HookMask::ALBEDO.allows(Hook::Vertex));
        assert_eq!(HookMask::ALBEDO.keys(), vec!["surface"]);
        assert_eq!(HookMask::of(&[Hook::Light, Hook::Finish]).keys(), vec!["light", "finish"]);
        assert!(Hook::from_key("pixel").is_none());
    }
}
