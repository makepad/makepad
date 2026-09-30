//! This frame's material values, by id. Pure data: the programs they name
//! (compiled Splash hooks) belong to makepad-render-material, which turns a
//! `MaterialFrame` into a draw.
use makepad_math::*;
use crate::item::TextureRef;

/// A session-resident material handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaterialId(pub u64);

/// A compiled material program (a hook set and its derived variants), owned
/// by the material crate's registry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaterialProgramId(pub u64);

/// Metal/rough, the glTF model plus the render lane's extras.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PbrParams {
    pub base_color: Vec4f,
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: Vec3f,
    pub clearcoat: f32,
    pub flake: f32,
    pub rim: f32,
    pub base_map: Option<TextureRef>,
}

impl Default for PbrParams {
    fn default() -> Self {
        Self { base_color: vec4(1.0, 1.0, 1.0, 1.0), metallic: 0.0, roughness: 1.0,
            emissive: Vec3f::default(), clearcoat: 0.0, flake: 0.0, rim: 0.0, base_map: None }
    }
}

/// Colour (or texture) times intensity, in scene-referred HDR units: lines,
/// neon, cards, UI in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnlitParams {
    pub color: Vec4f,
    pub intensity: f32,
    pub map: Option<TextureRef>,
}

impl Default for UnlitParams {
    fn default() -> Self { Self { color: vec4(1.0, 1.0, 1.0, 1.0), intensity: 1.0, map: None } }
}

/// Which built-in kind a Splash material extends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BaseKind {
    #[default]
    Pbr,
    Unlit,
}

/// A Pbr or Unlit base plus compiled hook functions.
#[derive(Clone, Debug, PartialEq)]
pub struct SplashMaterial {
    pub base: BaseKind,
    pub program: MaterialProgramId,
    /// 16 keyable floats, `self.uniforms0..3` in the hooks.
    pub uniforms: [Vec4f; 4],
    pub textures: Vec<TextureRef>,
}

pub const MAX_MATERIAL_TEXTURES: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub enum MaterialKind {
    Pbr(PbrParams),
    Unlit(UnlitParams),
    Splash(SplashMaterial),
}

impl Default for MaterialKind {
    fn default() -> Self { MaterialKind::Pbr(PbrParams::default()) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Blend {
    #[default]
    Opaque,
    /// Alpha-tested: pixels under `cutoff` are cut, in the colour pass and
    /// in the shadow caster.
    Mask { cutoff: f32 },
    Over,
    Add,
    Multiply,
    Screen,
}

impl Blend {
    /// Whether the colour pass can cut pixels.
    pub fn can_discard(&self) -> bool { matches!(self, Blend::Mask { .. }) }
    pub fn is_opaque(&self) -> bool { matches!(self, Blend::Opaque | Blend::Mask { .. }) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    #[default]
    Front,
    Back,
    Double,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LightingModel {
    /// Clustered lights, cascaded shadows and IBL when the environment has it.
    #[default]
    Lit,
    Unlit,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaterialFrame {
    pub id: MaterialId,
    pub kind: MaterialKind,
    pub blend: Blend,
    pub side: Side,
    pub depth_write: bool,
    /// Written to the glow attachment when a Glow node reads it.
    pub glow: f32,
    pub lighting: LightingModel,
}

impl MaterialFrame {
    pub fn validate(&self) -> Result<(), &'static str> {
        let finite4 = |v: Vec4f| v.x.is_finite() && v.y.is_finite() && v.z.is_finite() && v.w.is_finite();
        if !self.glow.is_finite() || self.glow < 0.0 {
            return Err("glow must be finite and non-negative");
        }
        if let Blend::Mask { cutoff } = self.blend {
            if !(0.0..=1.0).contains(&cutoff) {
                return Err("mask cutoff must be in 0..1");
            }
        }
        match &self.kind {
            MaterialKind::Pbr(p) => {
                if !finite4(p.base_color) || !(0.0..=1.0).contains(&p.metallic) || !(0.0..=1.0).contains(&p.roughness) {
                    return Err("pbr needs a finite colour and metallic/roughness in 0..1");
                }
            }
            MaterialKind::Unlit(u) => {
                if !finite4(u.color) || !u.intensity.is_finite() || u.intensity < 0.0 {
                    return Err("unlit needs a finite colour and a non-negative intensity");
                }
            }
            MaterialKind::Splash(s) => {
                if s.uniforms.iter().any(|u| !finite4(*u)) {
                    return Err("material uniforms must be finite");
                }
                if s.textures.len() > MAX_MATERIAL_TEXTURES {
                    return Err("a material binds at most 4 textures");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_catches_nan_uniforms_texture_overflow_and_bad_cutoffs() {
        let mut m = MaterialFrame::default();
        assert!(m.validate().is_ok());
        m.blend = Blend::Mask { cutoff: 1.5 };
        assert!(m.validate().is_err());
        m.blend = Blend::Mask { cutoff: 0.5 };
        assert!(m.blend.can_discard() && m.blend.is_opaque());
        let mut s = SplashMaterial { base: BaseKind::Pbr, program: MaterialProgramId(1), uniforms: [Vec4f::default(); 4], textures: Vec::new() };
        s.uniforms[2].y = f32::NAN;
        m.kind = MaterialKind::Splash(s.clone());
        assert!(m.validate().is_err());
        s.uniforms[2].y = 0.0;
        s.textures = vec![TextureRef(1); 5];
        m.kind = MaterialKind::Splash(s);
        assert!(m.validate().is_err());
    }
}
