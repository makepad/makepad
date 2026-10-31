//! Materials: what an edit's or a stroke's `material` slot stands for.
//!
//! Bricks store one u8 slot per sample; an object (or a layer) owns a table
//! of up to 256 [`Material`]s. The look lives in the table, not in the
//! bricks: moving a metal, rough, emissive or colour slider rewrites one
//! table entry and re-bakes nothing. A slot is set where an edit or stroke
//! writes it (see `ops` and `stroke`).
//!
//! Rendering reads the table through the trilinear footprint: the 8 corner
//! samples' entries are mixed with the trilinear weights, so colours and
//! surface values blend across a seam over one cell instead of stepping
//! ([`blend`]); the flags (`unlit`) come from the nearest sample. The slicer
//! assigns filaments by the nearest sample's slot (`Layer::material`).

use crate::math::*;

/// One material slot (linear RGB, values 0..1 unless noted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    pub color: V3,
    pub metal: f32,
    pub rough: f32,
    /// emitted light, in multiples of the colour (0 = none; may exceed 1)
    pub emissive: f32,
    /// a tint mixed over the colour by `colorize_amount`
    pub colorize: V3,
    pub colorize_amount: f32,
    /// opacity (1 = solid)
    pub fade: f32,
    /// shade without lighting (flat colour + emission)
    pub unlit: bool,
}

impl Default for Material {
    fn default() -> Material {
        Material {
            color: [0.8, 0.8, 0.8],
            metal: 0.0,
            rough: 0.5,
            emissive: 0.0,
            colorize: [1.0, 1.0, 1.0],
            colorize_amount: 0.0,
            fade: 1.0,
            unlit: false,
        }
    }
}

impl Material {
    /// The colour after the tint.
    pub fn albedo(&self) -> V3 {
        mix3(self.color, self.colorize, self.colorize_amount.clamp(0.0, 1.0))
    }

    /// The table texel layout the engine uploads (2 RGBA f32 texels per
    /// slot): [albedo.rgb, fade] [metal, rough, emissive, unlit 0/1].
    pub fn texels(&self) -> [[f32; 4]; 2] {
        let a = self.albedo();
        [[a[0], a[1], a[2], self.fade], [self.metal, self.rough, self.emissive, if self.unlit { 1.0 } else { 0.0 }]]
    }
}

/// An object's material slots (slot 0 is the default material).
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialTable {
    pub slots: Vec<Material>,
}

impl Default for MaterialTable {
    fn default() -> MaterialTable {
        MaterialTable { slots: vec![Material::default()] }
    }
}

impl MaterialTable {
    pub fn get(&self, slot: u8) -> Material {
        self.slots.get(slot as usize).copied().unwrap_or_default()
    }

    /// Sets a slot (growing the table with defaults).
    pub fn set(&mut self, slot: u8, m: Material) {
        let i = slot as usize;
        if self.slots.len() <= i {
            self.slots.resize(i + 1, Material::default());
        }
        self.slots[i] = m;
    }

    /// The upload: 2 texels per slot, 256 slots max.
    pub fn texels(&self) -> Vec<[f32; 4]> {
        self.slots.iter().flat_map(|m| m.texels()).collect()
    }
}

/// The shaded surface values at a point: the 8 corner samples' slots mixed
/// with the trilinear weights `t` (x fastest: corner i = (i&1, i>>1&1, i>>2)),
/// `unlit` from the nearest corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub albedo: V3,
    pub fade: f32,
    pub metal: f32,
    pub rough: f32,
    pub emissive: f32,
    pub unlit: bool,
}

pub fn blend(table: &MaterialTable, corners: [u8; 8], t: V3) -> Surface {
    let mut s = Surface { albedo: [0.0; 3], fade: 0.0, metal: 0.0, rough: 0.0, emissive: 0.0, unlit: false };
    for (i, slot) in corners.iter().enumerate() {
        let w = [if i & 1 == 1 { t[0] } else { 1.0 - t[0] }, if i & 2 == 2 { t[1] } else { 1.0 - t[1] }, if i & 4 == 4 { t[2] } else { 1.0 - t[2] }];
        let w = w[0] * w[1] * w[2];
        let m = table.get(*slot);
        s.albedo = add(s.albedo, scale(m.albedo(), w));
        s.fade += m.fade * w;
        s.metal += m.metal * w;
        s.rough += m.rough * w;
        s.emissive += m.emissive * w;
    }
    let nearest = (t[0] >= 0.5) as usize | ((t[1] >= 0.5) as usize) << 1 | ((t[2] >= 0.5) as usize) << 2;
    s.unlit = table.get(corners[nearest]).unlit;
    s
}
