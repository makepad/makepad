use super::*;
use crate::{
    json::Value,
    service::{array, fields, float, integer, need, selections, stable_id, text},
};
fn flag(v: &Value, key: &str, default: bool) -> Result<bool> {
    match v.get(key) {
        None => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(Error::Invalid("surface boolean")),
    }
}
fn number(v: &Value, key: &str, default: f64) -> Result<f64> {
    v.get(key).map(float).unwrap_or(Ok(default))
}
fn count(v: &Value, key: &str, default: u32) -> Result<u32> {
    v.get(key).map(integer).unwrap_or(Ok(default))
}
fn vec4(v: &Value, key: &str, default: [f64; 4]) -> Result<[f64; 4]> {
    v.get(key).map(array).unwrap_or(Ok(default))
}
fn channel(v: &Value) -> Result<SurfaceChannel> { named_channel(v, "channel") }
fn named_channel(v: &Value, key: &str) -> Result<SurfaceChannel> {
    match text(v, key)? {
        "base_color" => Ok(SurfaceChannel::BaseColor),
        "metallic_roughness" => Ok(SurfaceChannel::MetallicRoughness),
        "normal" => Ok(SurfaceChannel::Normal),
        "occlusion" => Ok(SurfaceChannel::Occlusion),
        "emissive" => Ok(SurfaceChannel::Emissive),
        _ => Err(Error::Invalid("surface channel")),
    }
}
fn bytes(v: &Value, max: usize) -> Result<Vec<u8>> {
    let s = v
        .as_str()
        .ok_or(Error::Invalid("expected lowercase hex bytes"))?;
    if s.len() % 2 != 0 || s.len() / 2 > max {
        return Err(Error::Budget("surface image bytes"));
    }
    fn digit(c: u8) -> Result<u8> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            _ => Err(Error::Invalid("lowercase hex byte")),
        }
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|c| Ok(digit(c[0])? * 16 + digit(c[1])?))
        .collect()
}
fn pattern(v: &Value) -> Result<SurfacePattern> {
    fields(v, &["kind", "color_a", "color_b", "scale", "seed"])?;
    let kind = match text(v, "kind")? {
        "checker" => PatternKind::Checker,
        "stripes" => PatternKind::Stripes,
        "gradient" => PatternKind::Gradient,
        "noise" => PatternKind::Noise,
        "perlin" => PatternKind::Perlin,
        "fbm" => PatternKind::Fbm,
        "yarn" => PatternKind::Yarn,
        _ => return Err(Error::Invalid("surface pattern kind")),
    };
    let p = SurfacePattern {
        kind,
        color_a: array(need(v, "color_a")?)?,
        color_b: array(need(v, "color_b")?)?,
        scale: v.get("scale").map(array).unwrap_or(Ok([8., 8.]))?,
        seed: v.get("seed").map(stable_id).unwrap_or(Ok(0))?,
    };
    if !pattern_valid(&p) {
        return Err(Error::Invalid("surface pattern"));
    }
    Ok(p)
}
impl SurfaceOperation {
    /// Strict source-authoring operation parser. Image and mask payloads use
    /// lowercase hex, with decoded byte budgets checked before allocation.
    pub fn parse(v: &Value, limits: &Limits) -> Result<Option<Self>> {
        let name = text(v, "op")?;
        if !name.starts_with("surface_") {
            return Ok(None);
        }
        use SurfaceOperation::*;
        let op = match name {
            "surface_material" => {
                fields(
                    v,
                    &[
                        "op",
                        "material",
                        "base_color",
                        "metallic",
                        "roughness",
                        "normal_scale",
                        "occlusion_strength",
                        "emissive",
                        "emissive_strength",
                        "alpha",
                        "alpha_cutoff",
                        "double_sided",
                        "fur",
                    ],
                )?;
                let mut value = SurfaceMaterial::default();
                value.fur = match v.get("fur") {
                    None | Some(Value::Null) => None,
                    Some(fur) => {
                        fields(fur, &["length", "density", "scale", "seed"])?;
                        Some(makepad_gltf::GlbFurMaterial {
                            length: number(fur, "length", 0.015)?,
                            density: number(fur, "density", 0.65)?,
                            scale: number(fur, "scale", 180.0)?,
                            seed: count(fur, "seed", 0)?,
                        })
                    }
                };
                value.base_color = vec4(v, "base_color", value.base_color)?;
                value.metallic = number(v, "metallic", value.metallic)?;
                value.roughness = number(v, "roughness", value.roughness)?;
                value.normal_scale = number(v, "normal_scale", value.normal_scale)?;
                value.occlusion_strength =
                    number(v, "occlusion_strength", value.occlusion_strength)?;
                value.emissive = v.get("emissive").map(array).unwrap_or(Ok(value.emissive))?;
                value.emissive_strength = number(v, "emissive_strength", value.emissive_strength)?;
                value.alpha = match v.get("alpha").and_then(Value::as_str).unwrap_or("opaque") {
                    "opaque" => SurfaceAlpha::Opaque,
                    "mask" => SurfaceAlpha::Mask,
                    "blend" => SurfaceAlpha::Blend,
                    _ => return Err(Error::Invalid("surface alpha mode")),
                };
                if v.get("alpha").is_some_and(|x| x.as_str().is_none()) {
                    return Err(Error::Invalid("surface alpha mode"));
                }
                value.alpha_cutoff = number(v, "alpha_cutoff", value.alpha_cutoff)?;
                value.double_sided = flag(v, "double_sided", false)?;
                value.validate(limits, &mut mesh::Context::new(limits.mesh.clone(), None))?;
                Material {
                    material: integer(need(v, "material")?)?,
                    value,
                }
            }
            "surface_remove_material" => {
                fields(v, &["op", "material"])?;
                RemoveMaterial {
                    material: integer(need(v, "material")?)?,
                }
            }
            "surface_layer" => {
                fields(
                    v,
                    &[
                        "op", "material", "channel", "layer", "width", "height", "color",
                        "rgba_hex", "png_hex", "mask_hex", "opacity", "blend", "visible",
                    ],
                )?;
                let ch = channel(v)?;
                let image = if let Some(png) = v.get("png_hex") {
                    if v.get("rgba_hex").is_some()
                        || v.get("width").is_some()
                        || v.get("height").is_some()
                        || v.get("color").is_some()
                    {
                        return Err(Error::Invalid("PNG layer conflicts with raw image fields"));
                    }
                    RgbaImage::from_png(&bytes(png, limits.max_texture_bytes)?, limits)?
                } else {
                    let w = integer(need(v, "width")?)?;
                    let h = integer(need(v, "height")?)?;
                    let mut image = RgbaImage::new(
                        w,
                        h,
                        encode(ch, vec4(v, "color", decode(ch, ch.neutral()))?),
                        limits,
                    )?;
                    if let Some(raw) = v.get("rgba_hex") {
                        if v.get("color").is_some() {
                            return Err(Error::Invalid("raw image conflicts with color"));
                        }
                        image.pixels = bytes(raw, limits.max_texture_bytes)?;
                        image.validate(limits)?;
                    }
                    image
                };
                let mut layer = SurfaceLayer::new(text(v, "layer")?, image);
                layer.opacity = number(v, "opacity", 1.)?;
                layer.visible = flag(v, "visible", true)?;
                layer.blend = match v.get("blend") {
                    None => SurfaceBlend::Over,
                    Some(Value::Str(s)) => match s.as_str() {
                        "over" => SurfaceBlend::Over,
                        "multiply" => SurfaceBlend::Multiply,
                        "add" => SurfaceBlend::Add,
                        "subtract" => SurfaceBlend::Subtract,
                        _ => return Err(Error::Invalid("surface blend")),
                    },
                    _ => return Err(Error::Invalid("surface blend")),
                };
                layer.mask = v
                    .get("mask_hex")
                    .map(|x| bytes(x, limits.max_texture_bytes / 4))
                    .transpose()?;
                Layer {
                    material: integer(need(v, "material")?)?,
                    channel: ch,
                    layer,
                }
            }
            "surface_remove_layer" => {
                fields(v, &["op", "material", "channel", "layer"])?;
                RemoveLayer {
                    material: integer(need(v, "material")?)?,
                    channel: channel(v)?,
                    layer: text(v, "layer")?.into(),
                }
            }
            "surface_move_layer" => {
                fields(v, &["op", "material", "channel", "layer", "index"])?;
                MoveLayer {
                    material: integer(need(v, "material")?)?,
                    channel: channel(v)?,
                    layer: text(v, "layer")?.into(),
                    index: integer(need(v, "index")?)? as usize,
                }
            }
            "surface_mask" => {
                fields(v, &["op", "material", "channel", "layer", "mask_hex"])?;
                Mask {
                    material: integer(need(v, "material")?)?,
                    channel: channel(v)?,
                    layer: text(v, "layer")?.into(),
                    mask: match need(v, "mask_hex")? {
                        Value::Null => None,
                        value => Some(bytes(value, limits.max_texture_bytes / 4)?),
                    },
                }
            }
            "surface_pattern" => {
                fields(v, &["op", "material", "channel", "layer", "pattern"])?;
                Pattern {
                    material: integer(need(v, "material")?)?,
                    channel: channel(v)?,
                    layer: text(v, "layer")?.into(),
                    pattern: pattern(need(v, "pattern")?)?,
                }
            }
            "surface_derive" => {
                fields(v, &["op", "material", "source_channel", "source_layer", "channel", "layer",
                    "strength", "roughness_min", "roughness_max", "metallic", "wrap"])?;
                let channel = channel(v)?;
                let recipe = SurfaceDerivation {
                    source_channel: named_channel(v, "source_channel")?,
                    source_layer: v.get("source_layer").map(|_| text(v, "source_layer").map(String::from)).transpose()?,
                    strength: number(v, "strength", 1.)?,
                    roughness_min: number(v, "roughness_min", 0.2)?,
                    roughness_max: number(v, "roughness_max", 0.9)?,
                    metallic: number(v, "metallic", 0.)?,
                    wrap: flag(v, "wrap", true)?,
                };
                recipe.validate(channel, limits)?;
                Derive { material: integer(need(v, "material")?)?, channel,
                    layer: text(v, "layer")?.into(), recipe }
            }
            "surface_stroke" => {
                fields(
                    v,
                    &[
                        "op", "material", "channel", "layer", "points", "radius", "hardness",
                        "opacity", "color", "mask",
                    ],
                )?;
                let points = need(v, "points")?
                    .as_arr()
                    .ok_or(Error::Invalid("stroke point array"))?;
                if points.is_empty() || points.len() > 256 {
                    return Err(Error::Budget("stroke points"));
                }
                Stroke {
                    material: integer(need(v, "material")?)?,
                    channel: channel(v)?,
                    layer: text(v, "layer")?.into(),
                    points: points.iter().map(array).collect::<Result<_>>()?,
                    radius: float(need(v, "radius")?)?,
                    hardness: number(v, "hardness", 0.5)?,
                    opacity: number(v, "opacity", 1.)?,
                    color: array(need(v, "color")?)?,
                    mask: flag(v, "mask", false)?,
                }
            }
            "surface_projected_stroke" => {
                fields(
                    v,
                    &[
                        "op",
                        "object",
                        "material",
                        "channel",
                        "layer",
                        "origin",
                        "direction",
                        "radius",
                        "depth",
                        "hardness",
                        "opacity",
                        "color",
                        "mask",
                    ],
                )?;
                ProjectedStroke {
                    object: text(v, "object")?.into(),
                    material: integer(need(v, "material")?)?,
                    channel: channel(v)?,
                    layer: text(v, "layer")?.into(),
                    origin: array(need(v, "origin")?)?,
                    direction: array(need(v, "direction")?)?,
                    radius: float(need(v, "radius")?)?,
                    depth: float(need(v, "depth")?)?,
                    hardness: number(v, "hardness", 0.5)?,
                    opacity: number(v, "opacity", 1.)?,
                    color: array(need(v, "color")?)?,
                    mask: flag(v, "mask", false)?,
                }
            }
            "surface_vertex_paint" => {
                fields(v, &["op", "object", "vertices", "color", "opacity"])?;
                VertexPaint {
                    object: text(v, "object")?.into(),
                    vertices: selections(need(v, "vertices")?, limits.mesh.max_vertices)?
                        .into_iter()
                        .map(mesh::VertexId)
                        .collect(),
                    color: array(need(v, "color")?)?,
                    opacity: number(v, "opacity", 1.)?,
                }
            }
            "surface_bake" => {
                fields(
                    v,
                    &[
                        "op",
                        "source",
                        "target",
                        "material",
                        "channel",
                        "layer",
                        "width",
                        "height",
                        "max_distance",
                        "ao_samples",
                        "ao_distance",
                        "dilation",
                    ],
                )?;
                Bake {
                    source: text(v, "source")?.into(),
                    target: text(v, "target")?.into(),
                    material: integer(need(v, "material")?)?,
                    channel: channel(v)?,
                    layer: text(v, "layer")?.into(),
                    width: integer(need(v, "width")?)?,
                    height: integer(need(v, "height")?)?,
                    max_distance: float(need(v, "max_distance")?)?,
                    ao_samples: count(v, "ao_samples", 16)?,
                    ao_distance: number(v, "ao_distance", 1.)?,
                    dilation: count(v, "dilation", 2)?,
                }
            }
            "surface_dilate" => {
                fields(
                    v,
                    &[
                        "op",
                        "material",
                        "channel",
                        "layer",
                        "iterations",
                        "preserve_alpha",
                    ],
                )?;
                Dilate {
                    material: integer(need(v, "material")?)?,
                    channel: channel(v)?,
                    layer: text(v, "layer")?.into(),
                    iterations: integer(need(v, "iterations")?)?,
                    preserve_alpha: flag(v, "preserve_alpha", true)?,
                }
            }
            "surface_mips" => {
                fields(v, &["op", "material", "channel", "layer"])?;
                Mips {
                    material: integer(need(v, "material")?)?,
                    channel: channel(v)?,
                    layer: text(v, "layer")?.into(),
                }
            }
            _ => return Err(Error::Invalid("unknown surface operation")),
        };
        if op.memory_bytes() > limits.max_transaction_bytes {
            return Err(Error::Budget("surface operation bytes"));
        }
        Ok(Some(op))
    }
}
