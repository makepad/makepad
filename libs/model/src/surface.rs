//! Transactional source surfacing. Layers, masks and vertex colors are source
//! data; flattened PBR textures are derived on the document worker.
use crate::{
    canon::{Reader, Writer},
    mesh, Error, ImageEncoding, Limits, Material, Result, RgbaImage,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SurfaceChannel {
    BaseColor,
    MetallicRoughness,
    Normal,
    Occlusion,
    Emissive,
}
impl SurfaceChannel {
    pub fn name(self) -> &'static str {
        match self {
            Self::BaseColor => "base_color",
            Self::MetallicRoughness => "metallic_roughness",
            Self::Normal => "normal",
            Self::Occlusion => "occlusion",
            Self::Emissive => "emissive",
        }
    }
    pub fn encoding(self) -> ImageEncoding {
        match self {
            Self::BaseColor | Self::Emissive => ImageEncoding::Srgb,
            Self::Normal => ImageEncoding::Normal,
            _ => ImageEncoding::Linear,
        }
    }
    fn neutral(self) -> [u8; 4] {
        match self {
            Self::BaseColor => [255; 4],
            Self::MetallicRoughness => [255; 4],
            Self::Normal => [128, 128, 255, 255],
            Self::Occlusion => [255; 4],
            Self::Emissive => [0, 0, 0, 255],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceAlpha {
    Opaque,
    Mask,
    Blend,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceBlend {
    Over,
    Multiply,
    Add,
    Subtract,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternKind {
    Checker,
    Stripes,
    Gradient,
    Noise,
    Perlin,
    Fbm,
    Yarn,
}
impl PatternKind {
    pub fn name(self) -> &'static str { match self {
        Self::Checker => "checker", Self::Stripes => "stripes", Self::Gradient => "gradient",
        Self::Noise => "noise", Self::Perlin => "perlin", Self::Fbm => "fbm", Self::Yarn => "yarn",
    } }
}
#[derive(Clone, Debug, PartialEq)]
pub struct SurfacePattern {
    pub kind: PatternKind,
    pub color_a: [f64; 4],
    pub color_b: [f64; 4],
    pub scale: [f64; 2],
    pub seed: u64,
}
/// A retained recipe for a derived snapshot. Editing the height source does
/// not implicitly refresh the output; repeat surface_derive explicitly.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceDerivation {
    pub source_channel: SurfaceChannel,
    pub source_layer: Option<String>,
    pub strength: f64,
    pub roughness_min: f64,
    pub roughness_max: f64,
    pub metallic: f64,
    pub wrap: bool,
}
impl SurfaceDerivation {
    /// Derive pixels with the same UV-space height convention used by
    /// `surface_derive`. The caller supplies the composed or named source image;
    /// no material factors are applied here. Color sources decode to linear
    /// height, alpha supplies coverage, and both slopes point away from a rise.
    pub fn derive_image(
        &self,
        source: &RgbaImage,
        channel: SurfaceChannel,
        limits: &Limits,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<RgbaImage> {
        derivation::derive_image(source, channel, self, limits, ctx)
    }
    pub fn validate(&self, channel: SurfaceChannel, limits: &Limits) -> Result<()> {
        if !matches!(channel, SurfaceChannel::Normal | SurfaceChannel::MetallicRoughness)
            || self.source_channel == SurfaceChannel::Normal
            || self.source_layer.as_ref().is_some_and(|name| !name_valid(name, limits))
            || !self.strength.is_finite() || !(0.0..=16.0).contains(&self.strength)
            || !unit(self.roughness_min) || !unit(self.roughness_max)
            || self.roughness_min > self.roughness_max || !unit(self.metallic)
        { return Err(Error::Invalid("surface derivation")); }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceLayer {
    pub id: String,
    pub image: RgbaImage,
    pub mask: Option<Vec<u8>>,
    pub opacity: f64,
    pub blend: SurfaceBlend,
    pub visible: bool,
    pub pattern: Option<SurfacePattern>,
    pub derived: Option<SurfaceDerivation>,
    pub mips: Vec<RgbaImage>,
}
impl SurfaceLayer {
    pub fn new(id: impl Into<String>, image: RgbaImage) -> Self {
        Self {
            id: id.into(),
            image,
            mask: None,
            opacity: 1.,
            blend: SurfaceBlend::Over,
            visible: true,
            pattern: None,
            derived: None,
            mips: Vec::new(),
        }
    }
    pub fn memory_bytes(&self) -> usize {
        self.image
            .memory_bytes()
            .saturating_add(self.mask.as_ref().map_or(0, Vec::len))
            .saturating_add(self.mips.iter().map(RgbaImage::memory_bytes).sum::<usize>())
            .saturating_add(self.id.len())
            .saturating_add(self.derived.as_ref().and_then(|d| d.source_layer.as_ref()).map_or(0, String::len))
            .saturating_add(256)
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceMaterial {
    pub fur: Option<makepad_gltf::GlbFurMaterial>,
    pub base_color: [f64; 4],
    pub metallic: f64,
    pub roughness: f64,
    pub normal_scale: f64,
    pub occlusion_strength: f64,
    pub emissive: [f64; 3],
    pub emissive_strength: f64,
    pub alpha: SurfaceAlpha,
    pub alpha_cutoff: f64,
    pub double_sided: bool,
    pub channels: BTreeMap<SurfaceChannel, Vec<SurfaceLayer>>,
}
impl Default for SurfaceMaterial {
    fn default() -> Self {
        Self {
            fur: None,
            base_color: [1.; 4],
            metallic: 0.,
            roughness: 1.,
            normal_scale: 1.,
            occlusion_strength: 1.,
            emissive: [0.; 3],
            emissive_strength: 1.,
            alpha: SurfaceAlpha::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
            channels: BTreeMap::new(),
        }
    }
}
impl SurfaceMaterial {
    pub fn from_legacy(material: &Material, limits: &Limits) -> Result<Self> {
        let mut value = Self {
            base_color: [material.color[0], material.color[1], material.color[2], 1.],
            ..Self::default()
        };
        if !material.base_color_png.is_empty() {
            value.channels.insert(
                SurfaceChannel::BaseColor,
                vec![SurfaceLayer::new(
                    "legacy",
                    RgbaImage::from_png(&material.base_color_png, limits)?,
                )],
            );
        }
        Ok(value)
    }
    pub fn memory_bytes(&self) -> usize {
        self.channels
            .values()
            .flatten()
            .fold(256usize, |n, l| n.saturating_add(l.memory_bytes()))
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceState {
    pub materials: BTreeMap<u32, SurfaceMaterial>,
    pub vertex_colors: BTreeMap<(String, mesh::VertexId), [f64; 4]>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceOperation {
    Material {
        material: u32,
        value: SurfaceMaterial,
    },
    RemoveMaterial {
        material: u32,
    },
    Layer {
        material: u32,
        channel: SurfaceChannel,
        layer: SurfaceLayer,
    },
    RemoveLayer {
        material: u32,
        channel: SurfaceChannel,
        layer: String,
    },
    MoveLayer {
        material: u32,
        channel: SurfaceChannel,
        layer: String,
        index: usize,
    },
    Mask {
        material: u32,
        channel: SurfaceChannel,
        layer: String,
        mask: Option<Vec<u8>>,
    },
    Pattern {
        material: u32,
        channel: SurfaceChannel,
        layer: String,
        pattern: SurfacePattern,
    },
    Derive {
        material: u32,
        channel: SurfaceChannel,
        layer: String,
        recipe: SurfaceDerivation,
    },
    Stroke {
        material: u32,
        channel: SurfaceChannel,
        layer: String,
        points: Vec<[f64; 2]>,
        radius: f64,
        hardness: f64,
        opacity: f64,
        color: [f64; 4],
        mask: bool,
    },
    ProjectedStroke {
        object: String,
        material: u32,
        channel: SurfaceChannel,
        layer: String,
        origin: [f64; 3],
        direction: [f64; 3],
        radius: f64,
        depth: f64,
        hardness: f64,
        opacity: f64,
        color: [f64; 4],
        mask: bool,
    },
    VertexPaint {
        object: String,
        vertices: Vec<mesh::VertexId>,
        color: [f64; 4],
        opacity: f64,
    },
    Bake {
        source: String,
        target: String,
        material: u32,
        channel: SurfaceChannel,
        layer: String,
        width: u32,
        height: u32,
        max_distance: f64,
        ao_samples: u32,
        ao_distance: f64,
        dilation: u32,
    },
    Dilate {
        material: u32,
        channel: SurfaceChannel,
        layer: String,
        iterations: u32,
        preserve_alpha: bool,
    },
    Mips {
        material: u32,
        channel: SurfaceChannel,
        layer: String,
    },
}

#[path = "surface_codec.rs"]
mod codec;
pub(crate) use codec::{
    read_surface, read_surface_operation, write_surface, write_surface_operation,
};
#[path = "surface_geometry.rs"]
mod geometry;
#[path = "surface_parse.rs"]
mod parse;
#[path = "surface_pattern.rs"]
mod pattern_sampler;
#[path = "surface_derive.rs"]
mod derivation;

fn unit(v: f64) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}
fn color_valid(v: &[f64]) -> bool {
    v.iter().all(|x| unit(*x))
}
fn name_valid(v: &str, limits: &Limits) -> bool {
    !v.is_empty() && v.len() <= limits.max_name_bytes && !v.chars().any(char::is_control)
}
fn pattern_valid(p: &SurfacePattern) -> bool {
    color_valid(&p.color_a)
        && color_valid(&p.color_b)
        && p.scale
            .iter()
            .all(|x| x.is_finite() && *x > 0. && *x <= 4096.)
}
impl SurfaceMaterial {
    pub fn validate(&self, limits: &Limits, ctx: &mut mesh::Context<'_>) -> Result<()> {
        if self.fur.is_some_and(|fur| !fur.valid()) {
            return Err(Error::Invalid("fur material: length 0.001..0.05m, density 0.05..1, scale 20..1000, seed 0..65535"));
        }
        if !color_valid(&self.base_color)
            || !unit(self.metallic)
            || !unit(self.roughness)
            || !unit(self.occlusion_strength)
            || !color_valid(&self.emissive)
            || !unit(self.alpha_cutoff)
            || !self.normal_scale.is_finite()
            || !(0.0..=16.0).contains(&self.normal_scale)
            || !self.emissive_strength.is_finite()
            || !(0.0..=100000.0).contains(&self.emissive_strength)
        {
            return Err(Error::Invalid("surface material factors"));
        }
        for (channel, layers) in &self.channels {
            if layers.len() > 32 {
                return Err(Error::Budget("surface layers"));
            }
            let mut ids = std::collections::BTreeSet::new();
            for layer in layers {
                ctx.checkpoint(1)?;
                if !name_valid(&layer.id, limits) || !ids.insert(&layer.id) || !unit(layer.opacity)
                {
                    return Err(Error::Invalid("surface layer identity/opacity"));
                }
                layer.image.validate(limits)?;
                if layer
                    .mask
                    .as_ref()
                    .is_some_and(|m| m.len() != layer.image.pixels.len() / 4)
                {
                    return Err(Error::Invalid("surface mask dimensions"));
                }
                if layer.pattern.as_ref().is_some_and(|p| !pattern_valid(p)) {
                    return Err(Error::Invalid("surface pattern"));
                }
                if let Some(recipe) = &layer.derived { recipe.validate(*channel, limits)?; }
                let (mut w, mut h) = (layer.image.width, layer.image.height);
                for mip in &layer.mips {
                    ctx.checkpoint(1)?;
                    if w == 1 && h == 1 {
                        return Err(Error::Invalid("extra mip level"));
                    }
                    w = (w / 2).max(1);
                    h = (h / 2).max(1);
                    mip.validate(limits)?;
                    if (mip.width, mip.height) != (w, h) {
                        return Err(Error::Invalid("surface mip dimensions"));
                    }
                }
                if !layer.mips.is_empty() && (w, h) != (1, 1) {
                    return Err(Error::Invalid("incomplete mip chain"));
                }
            }
        }
        if self.memory_bytes() > limits.max_source_bytes {
            return Err(Error::Budget("surface source bytes"));
        }
        Ok(())
    }
    /// Flatten a retained stack in linear light. Returned pixels use the
    /// channel's declared transfer function; texture alpha remains straight.
    pub fn flatten(
        &self,
        channel: SurfaceChannel,
        limits: &Limits,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<Option<RgbaImage>> {
        self.validate(limits, ctx)?;
        let Some(layers) = self.channels.get(&channel) else {
            return Ok(None);
        };
        let visible: Vec<_> = layers
            .iter()
            .filter(|l| l.visible && l.opacity > 0.)
            .collect();
        if visible.is_empty() {
            return Ok(None);
        }
        let width = visible.iter().map(|l| l.image.width).max().unwrap();
        let height = visible.iter().map(|l| l.image.height).max().unwrap();
        let mut image = RgbaImage::new(width, height, [0; 4], limits)?;
        if image.memory_bytes().saturating_add(self.memory_bytes()) > limits.mesh.max_bytes {
            return Err(Error::Budget("surface flatten bytes"));
        }
        for y in 0..height {
            ctx.checkpoint(width as u64 * visible.len() as u64)?;
            for x in 0..width {
                let mut out = decode(channel, channel.neutral());
                out[3] = 0.;
                for layer in &visible {
                    let sx = (x as u64 * layer.image.width as u64 / width as u64) as usize;
                    let sy = (y as u64 * layer.image.height as u64 / height as u64) as usize;
                    let i = sy * layer.image.width as usize + sx;
                    let src = decode(
                        channel,
                        layer.image.pixels[i * 4..i * 4 + 4].try_into().unwrap(),
                    );
                    let alpha =
                        layer.opacity * layer.mask.as_ref().map_or(1., |m| m[i] as f64 / 255.);
                    out = composite(out, src, alpha, layer.blend);
                }
                image.pixels[(y as usize * width as usize + x as usize) * 4..][..4]
                    .copy_from_slice(&encode(channel, out));
            }
        }
        Ok(Some(image))
    }
}
impl SurfaceState {
    pub fn memory_bytes(&self) -> usize {
        self.materials
            .values()
            .fold(64usize, |n, m| n.saturating_add(m.memory_bytes()))
            .saturating_add(
                self.vertex_colors
                    .keys()
                    .map(|(s, _)| s.len() + 96)
                    .sum::<usize>(),
            )
    }
    pub fn validate(&self, limits: &Limits, ctx: &mut mesh::Context<'_>) -> Result<()> {
        if self.materials.len() > limits.max_materials
            || self.vertex_colors.len() > limits.mesh.max_vertices
        {
            return Err(Error::Budget("surface entries"));
        }
        if self.memory_bytes() > limits.max_source_bytes
            || self.memory_bytes() > limits.mesh.max_bytes
        {
            return Err(Error::Budget("surface bytes"));
        }
        for value in self.materials.values() {
            value.validate(limits, ctx)?;
        }
        for ((name, id), color) in &self.vertex_colors {
            ctx.checkpoint(1)?;
            if !name_valid(name, limits) || id.0 == 0 || !color_valid(color) {
                return Err(Error::Invalid("vertex color"));
            }
        }
        Ok(())
    }
    pub fn apply(
        &mut self,
        op: &SurfaceOperation,
        objects: &BTreeMap<String, mesh::Mesh>,
        legacy: &BTreeMap<u32, Material>,
        limits: &Limits,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<()> {
        ctx.checkpoint(0)?;
        self.validate(limits, ctx)?;
        if self
            .memory_bytes()
            .saturating_mul(2)
            .saturating_add(op.memory_bytes())
            > limits.mesh.max_bytes
        {
            return Err(Error::Budget("surface transaction bytes"));
        }
        let mut next = self.clone();
        next.apply_inner(op, objects, legacy, limits, ctx)?;
        next.validate(limits, ctx)?;
        for material in next.materials.keys() {
            if !legacy.contains_key(material) {
                return Err(Error::Invalid("surface references unknown material"));
            }
        }
        ctx.checkpoint(0)?;
        *self = next;
        Ok(())
    }
    fn ensure(
        &mut self,
        id: u32,
        legacy: &BTreeMap<u32, Material>,
        limits: &Limits,
    ) -> Result<&mut SurfaceMaterial> {
        if !self.materials.contains_key(&id) {
            let value = SurfaceMaterial::from_legacy(
                legacy
                    .get(&id)
                    .ok_or(Error::Invalid("unknown surface material"))?,
                limits,
            )?;
            self.materials.insert(id, value);
        }
        Ok(self.materials.get_mut(&id).unwrap())
    }
    fn layer(&mut self, id: u32, ch: SurfaceChannel, name: &str) -> Result<&mut SurfaceLayer> {
        self.materials
            .get_mut(&id)
            .and_then(|m| m.channels.get_mut(&ch))
            .and_then(|ls| ls.iter_mut().find(|l| l.id == name))
            .ok_or(Error::Invalid("unknown surface layer"))
    }
    fn apply_inner(
        &mut self,
        op: &SurfaceOperation,
        objects: &BTreeMap<String, mesh::Mesh>,
        legacy: &BTreeMap<u32, Material>,
        limits: &Limits,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<()> {
        use SurfaceOperation::*;
        match op {
            Material { material, value } => {
                let mut value = value.clone();
                if value.channels.is_empty() {
                    // Factor-only updates also preserve an image authored
                    // before the material acquired a rich surface sidecar.
                    value.channels = self.ensure(*material, legacy, limits)?.channels.clone();
                }
                self.materials.insert(*material, value);
            }
            RemoveMaterial { material } => {
                self.materials.remove(material);
            }
            Layer {
                material,
                channel,
                layer,
            } => {
                let layers = self
                    .ensure(*material, legacy, limits)?
                    .channels
                    .entry(*channel)
                    .or_default();
                if let Some(old) = layers.iter_mut().find(|l| l.id == layer.id) {
                    *old = layer.clone();
                } else {
                    layers.push(layer.clone());
                }
            }
            RemoveLayer {
                material,
                channel,
                layer,
            } => {
                let ls = self
                    .materials
                    .get_mut(material)
                    .and_then(|m| m.channels.get_mut(channel))
                    .ok_or(Error::Invalid("unknown surface channel"))?;
                let n = ls
                    .iter()
                    .position(|l| &l.id == layer)
                    .ok_or(Error::Invalid("unknown surface layer"))?;
                ls.remove(n);
            }
            MoveLayer {
                material,
                channel,
                layer,
                index,
            } => {
                let ls = self
                    .materials
                    .get_mut(material)
                    .and_then(|m| m.channels.get_mut(channel))
                    .ok_or(Error::Invalid("unknown surface channel"))?;
                if *index >= ls.len() {
                    return Err(Error::Invalid("layer index"));
                }
                let n = ls
                    .iter()
                    .position(|l| &l.id == layer)
                    .ok_or(Error::Invalid("unknown surface layer"))?;
                let value = ls.remove(n);
                ls.insert(*index, value);
            }
            Mask {
                material,
                channel,
                layer,
                mask,
            } => {
                let target = self.layer(*material, *channel, layer)?;
                if mask
                    .as_ref()
                    .is_some_and(|m| m.len() != target.image.pixels.len() / 4)
                {
                    return Err(Error::Invalid("mask size"));
                }
                target.mask = mask.clone();
                target.mips.clear();
            }
            Pattern {
                material,
                channel,
                layer,
                pattern,
            } => {
                let target = self.layer(*material, *channel, layer)?;
                render_pattern(target, *channel, pattern, ctx)?;
            }
            Derive { material, channel, layer, recipe } => {
                recipe.validate(*channel, limits)?;
                if recipe.source_channel == *channel && recipe.source_layer.as_deref().is_none_or(|source| source == layer) {
                    return Err(Error::Invalid("derivation cannot overwrite its own height source"));
                }
                // Like layer insertion, derivation can begin from a legacy
                // base-color PNG without an explicit conversion operation.
                self.ensure(*material, legacy, limits)?;
                let source_material = &self.materials[material];
                let dimensions = source_material.channels.get(&recipe.source_channel)
                    .into_iter().flatten().filter(|value| recipe.source_layer.as_ref()
                        .map_or(value.visible && value.opacity > 0., |name| &value.id == name))
                    .fold([0usize; 2], |size, value| [size[0].max(value.image.width as usize), size[1].max(value.image.height as usize)]);
                let scratch_bytes = dimensions[0].saturating_mul(dimensions[1]).saturating_mul(8);
                if self.memory_bytes().saturating_mul(2).saturating_add(scratch_bytes) > limits.mesh.max_bytes {
                    return Err(Error::Budget("surface derivation bytes"));
                }
                let source = derivation::source_image(source_material, recipe, limits, ctx)?;
                let image = recipe.derive_image(&source, *channel, limits, ctx)?;
                let mut output = SurfaceLayer::new(layer, image);
                output.derived = Some(recipe.clone());
                let target = self.ensure(*material, legacy, limits)?;
                if *channel == SurfaceChannel::Normal { target.normal_scale = 1.; }
                else { target.metallic = 1.; target.roughness = 1.; }
                let layers = target.channels.entry(*channel).or_default();
                if let Some(previous) = layers.iter_mut().find(|value| value.id == *layer) {
                    output.mask = previous.mask.clone();
                    if output.mask.as_ref().is_some_and(|mask| mask.len() != output.image.pixels.len() / 4) { output.mask = None; }
                    output.opacity = previous.opacity;
                    output.blend = previous.blend;
                    output.visible = previous.visible;
                    *previous = output;
                } else { layers.push(output); }
            }
            Stroke {
                material,
                channel,
                layer,
                points,
                radius,
                hardness,
                opacity,
                color,
                mask,
            } => {
                brush_validate(*radius, *hardness, *opacity, color)?;
                if points.is_empty()
                    || points.len() > 256
                    || points
                        .iter()
                        .flatten()
                        .any(|x| !x.is_finite() || x.abs() > 16.)
                {
                    return Err(Error::Invalid("stroke points"));
                }
                let target = self.layer(*material, *channel, layer)?;
                let (w, h) = (target.image.width, target.image.height);
                for y in 0..h {
                    ctx.checkpoint(w as u64 * points.len() as u64)?;
                    for x in 0..w {
                        let p = [(x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64];
                        let d = if points.len() == 1 {
                            distance2(p, points[0])
                        } else {
                            points
                                .windows(2)
                                .map(|s| segment_distance(p, s[0], s[1]))
                                .fold(f64::INFINITY, f64::min)
                        };
                        paint_pixel(
                            target,
                            *channel,
                            (y * w + x) as usize,
                            *color,
                            *opacity * falloff(d, *radius, *hardness),
                            *mask,
                        );
                    }
                }
                target.mips.clear();
                target.pattern = None;
                target.derived = None;
            }
            ProjectedStroke { .. } => geometry::project(self, op, objects, limits, ctx)?,
            VertexPaint {
                object,
                vertices,
                color,
                opacity,
            } => {
                if !color_valid(color)
                    || !unit(*opacity)
                    || vertices.is_empty()
                    || vertices.len() > limits.mesh.max_vertices
                {
                    return Err(Error::Invalid("vertex paint"));
                }
                let mesh = objects
                    .get(object)
                    .ok_or(Error::Invalid("unknown paint object"))?;
                for id in vertices {
                    ctx.checkpoint(1)?;
                    mesh.vertex(*id)
                        .ok_or(Error::Invalid("unknown painted vertex"))?;
                    let value = self
                        .vertex_colors
                        .entry((object.clone(), *id))
                        .or_insert([1.; 4]);
                    for c in 0..4 {
                        value[c] = value[c] * (1. - opacity) + color[c] * opacity;
                    }
                }
            }
            Bake { .. } => geometry::bake(self, op, objects, legacy, limits, ctx)?,
            Dilate {
                material,
                channel,
                layer,
                iterations,
                preserve_alpha,
            } => {
                let target = self.layer(*material, *channel, layer)?;
                target
                    .image
                    .dilate(*iterations, *preserve_alpha, limits, ctx)?;
                target.mips.clear();
                target.pattern = None;
                target.derived = None;
            }
            Mips {
                material,
                channel,
                layer,
            } => {
                let target = self.layer(*material, *channel, layer)?;
                target.mips = target.image.mip_chain(channel.encoding(), limits, ctx)?;
            }
        }
        Ok(())
    }
    /// Preserve colors on stable vertices, transfer to newly created vertices
    /// by closest source triangle interpolation, and discard deleted identities.
    pub fn reconcile(
        &mut self,
        before: &BTreeMap<String, mesh::Mesh>,
        after: &BTreeMap<String, mesh::Mesh>,
        limits: &Limits,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<()> {
        geometry::reconcile(self, before, after, limits, ctx)
    }
    /// Transfer source paint onto a disposable derived mesh without altering
    /// source identities or copying the retained material images.
    pub(crate) fn transfer_vertex_colors(
        &mut self,
        source_name: &str,
        source: &mesh::Mesh,
        target_name: &str,
        target: &mesh::Mesh,
        limits: &Limits,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<()> {
        geometry::transfer_vertex_colors(self, source_name, source, target_name, target, limits, ctx)
    }
}
impl SurfaceOperation {
    pub fn memory_bytes(&self) -> usize {
        use SurfaceOperation::*;
        match self {
            Material { value, .. } => value.memory_bytes(),
            Layer { layer, .. } => layer.memory_bytes(),
            Mask { mask, .. } => mask.as_ref().map_or(0, Vec::len) + 256,
            Stroke { points, .. } => points.len() * 16 + 512,
            VertexPaint { vertices, .. } => vertices.len() * 8 + 256,
            _ => 512,
        }
    }
    pub fn object_name(&self) -> Option<&str> {
        match self {
            Self::ProjectedStroke { object, .. } | Self::VertexPaint { object, .. } => Some(object),
            Self::Bake { target, .. } => Some(target),
            _ => None,
        }
    }
}
fn decode(ch: SurfaceChannel, p: [u8; 4]) -> [f64; 4] {
    let mut out = p.map(|v| v as f64 / 255.);
    if ch.encoding() == ImageEncoding::Srgb {
        for c in 0..3 {
            out[c] = crate::texture::srgb_to_linear(out[c]);
        }
    }
    out
}
fn encode(ch: SurfaceChannel, mut p: [f64; 4]) -> [u8; 4] {
    if ch == SurfaceChannel::Normal {
        let n = geometry::normalize([p[0] * 2. - 1., p[1] * 2. - 1., p[2] * 2. - 1.]);
        for c in 0..3 {
            p[c] = n[c] * 0.5 + 0.5;
        }
    }
    if ch.encoding() == ImageEncoding::Srgb {
        for c in 0..3 {
            p[c] = crate::texture::linear_to_srgb(p[c]);
        }
    }
    p.map(crate::texture::unit_byte)
}
fn composite(dst: [f64; 4], src: [f64; 4], opacity: f64, blend: SurfaceBlend) -> [f64; 4] {
    let a = src[3] * opacity;
    let alpha = a + dst[3] * (1. - a);
    if alpha <= 0. {
        return [0.; 4];
    }
    let mut out = [0., 0., 0., alpha];
    for c in 0..3 {
        let mixed = match blend {
            SurfaceBlend::Over => src[c],
            SurfaceBlend::Multiply => src[c] * dst[c],
            SurfaceBlend::Add => (src[c] + dst[c]).min(1.),
            SurfaceBlend::Subtract => (dst[c] - src[c]).max(0.),
        };
        let blended = src[c] * (1. - dst[3]) + mixed * dst[3];
        out[c] = (blended * a + dst[c] * dst[3] * (1. - a)) / alpha;
    }
    out
}
fn brush_validate(radius: f64, hardness: f64, opacity: f64, color: &[f64; 4]) -> Result<()> {
    if !radius.is_finite()
        || radius <= 0.
        || !unit(hardness)
        || !unit(opacity)
        || !color_valid(color)
    {
        Err(Error::Invalid("surface brush"))
    } else {
        Ok(())
    }
}
fn falloff(distance: f64, radius: f64, hardness: f64) -> f64 {
    if distance >= radius {
        0.
    } else if distance <= radius * hardness {
        1.
    } else {
        (radius - distance) / (radius * (1. - hardness))
    }
}
fn distance2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0] * d[0] + d[1] * d[1];
    let t = if len == 0. {
        0.
    } else {
        ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len
    }
    .clamp(0., 1.);
    distance2(p, [a[0] + d[0] * t, a[1] + d[1] * t])
}
fn paint_pixel(
    layer: &mut SurfaceLayer,
    ch: SurfaceChannel,
    i: usize,
    color: [f64; 4],
    opacity: f64,
    mask: bool,
) {
    if opacity <= 0. {
        return;
    }
    if mask {
        let values = layer
            .mask
            .get_or_insert_with(|| vec![255; layer.image.pixels.len() / 4]);
        values[i] = crate::texture::unit_byte(
            values[i] as f64 / 255. * (1. - opacity) + color[0] * opacity,
        );
    } else {
        let dst = decode(ch, layer.image.pixels[i * 4..i * 4 + 4].try_into().unwrap());
        layer.image.pixels[i * 4..i * 4 + 4].copy_from_slice(&encode(
            ch,
            composite(dst, color, opacity, SurfaceBlend::Over),
        ));
    }
}
fn render_pattern(
    layer: &mut SurfaceLayer,
    ch: SurfaceChannel,
    pattern: &SurfacePattern,
    ctx: &mut mesh::Context<'_>,
) -> Result<()> {
    if !pattern_valid(pattern) {
        return Err(Error::Invalid("surface pattern"));
    }
    let (w, h) = (layer.image.width, layer.image.height);
    for y in 0..h {
        ctx.checkpoint(w as u64)?;
        for x in 0..w {
            let u = (x as f64 + 0.5) / w as f64 * pattern.scale[0];
            let v = (y as f64 + 0.5) / h as f64 * pattern.scale[1];
            let t = match pattern.kind {
                PatternKind::Checker => ((u.floor() as i64 + v.floor() as i64) & 1) as f64,
                PatternKind::Stripes => (u.floor() as i64 & 1) as f64,
                PatternKind::Gradient => u.fract(),
                PatternKind::Noise => {
                    let mut n = pattern.seed
                        ^ (u.floor() as u64).wrapping_mul(0x9e3779b97f4a7c15)
                        ^ (v.floor() as u64).wrapping_mul(0xbf58476d1ce4e5b9);
                    n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                    n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
                    ((n ^ (n >> 31)) >> 11) as f64 / ((1u64 << 53) - 1) as f64
                },
                PatternKind::Perlin | PatternKind::Fbm | PatternKind::Yarn =>
                    pattern_sampler::sample(pattern.kind, u, v, pattern.scale, pattern.seed),
            };
            let mut color = [0.; 4];
            for c in 0..4 {
                color[c] = pattern.color_a[c] * (1. - t) + pattern.color_b[c] * t;
            }
            layer.image.pixels[((y * w + x) * 4) as usize..][..4]
                .copy_from_slice(&encode(ch, color));
        }
    }
    layer.pattern = Some(pattern.clone());
    layer.derived = None;
    layer.mips.clear();
    Ok(())
}
