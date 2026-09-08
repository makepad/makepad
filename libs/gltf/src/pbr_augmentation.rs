//! Standard glTF PBR material and per-primitive attribute augmentation.
use crate::{
    augment::{number, object, string, validation, GlbRewrite},
    GltfError, JsonValue,
};
use std::collections::HashMap;
#[derive(Clone, Debug, Default)]
pub struct GlbPbrTexture {
    pub png: Vec<u8>,
    pub mip_pngs: Vec<Vec<u8>>,
}
#[derive(Clone, Debug)]
pub struct GlbPbrMaterial {
    /// Short fur rendered from repeated surface shells; no strand geometry.
    pub fur: Option<GlbFurMaterial>,
    pub base_color: [f64; 4],
    pub metallic: f64,
    pub roughness: f64,
    pub normal_scale: f64,
    pub occlusion_strength: f64,
    pub emissive: [f64; 3],
    pub emissive_strength: f64,
    pub alpha_mode: String,
    pub alpha_cutoff: f64,
    pub double_sided: bool,
    pub base_color_texture: Option<GlbPbrTexture>,
    pub metallic_roughness_texture: Option<GlbPbrTexture>,
    pub normal_texture: Option<GlbPbrTexture>,
    pub occlusion_texture: Option<GlbPbrTexture>,
    pub emissive_texture: Option<GlbPbrTexture>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlbFurMaterial {
    pub length: f64,
    pub density: f64,
    /// Procedural strand cells per model metre.
    pub scale: f64,
    pub seed: u32,
}

impl GlbFurMaterial {
    pub fn valid(self) -> bool {
        self.length.is_finite() && (0.001..=0.05).contains(&self.length)
            && self.density.is_finite() && (0.05..=1.0).contains(&self.density)
            && self.scale.is_finite() && (20.0..=1000.0).contains(&self.scale)
            && self.seed <= 65535
    }
}
#[derive(Clone, Debug)]
pub struct GlbPbrPrimitive {
    pub mesh: usize,
    pub primitive: usize,
    pub material: usize,
    pub colors: Vec<[f32; 4]>,
    pub tangents: Vec<[f32; 4]>,
}
fn floats(values: &[f64]) -> JsonValue {
    JsonValue::Array(values.iter().map(|v| JsonValue::F64(*v)).collect())
}
fn image(
    edit: &mut GlbRewrite,
    views: &mut Vec<JsonValue>,
    images: &mut Vec<JsonValue>,
    png: &[u8],
) -> Result<usize, GltfError> {
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") || png.len() > 64 * 1024 * 1024 {
        return Err(validation("invalid/oversized embedded PBR PNG"));
    }
    while edit.bin.len() % 4 != 0 {
        edit.bin.push(0);
    }
    let offset = edit.bin.len();
    edit.bin.extend_from_slice(png);
    let view = views.len();
    views.push(object([
        ("buffer", number(0)),
        ("byteOffset", number(offset)),
        ("byteLength", number(png.len())),
    ]));
    let index = images.len();
    images.push(object([
        ("bufferView", number(view)),
        ("mimeType", string("image/png")),
    ]));
    Ok(index)
}
fn texture(
    edit: &mut GlbRewrite,
    views: &mut Vec<JsonValue>,
    images: &mut Vec<JsonValue>,
    textures: &mut Vec<JsonValue>,
    sampler: usize,
    map: &GlbPbrTexture,
) -> Result<(JsonValue, Vec<JsonValue>), GltfError> {
    let source = image(edit, views, images, &map.png)?;
    let index = textures.len();
    textures.push(object([
        ("source", number(source)),
        ("sampler", number(sampler)),
    ]));
    let mut mips = Vec::new();
    if map.mip_pngs.len() > 32 {
        return Err(validation("too many PBR mip levels"));
    }
    for mip in &map.mip_pngs {
        mips.push(number(image(edit, views, images, mip)?));
    }
    Ok((
        object([("index", number(index)), ("texCoord", number(0))]),
        mips,
    ))
}
/// Replaces material assignments only on explicitly supplied primitives. All
/// source JSON/BIN data, skinning, animations and extensions remain intact.
/// `extras.makepadMips` stores embedded IMAGE indices for levels 1..N; ordinary
/// glTF clients use the standard level-zero images and sampler mip filtering.
pub fn augment_glb_pbr(
    input: &[u8],
    materials: &[GlbPbrMaterial],
    primitives: &[GlbPbrPrimitive],
) -> Result<Vec<u8>, GltfError> {
    if materials.len() > 256 || primitives.len() > 4096 {
        return Err(validation("PBR augmentation count budget"));
    }
    let mut edit = GlbRewrite::begin(input)?;
    let mut views = edit.take_array("bufferViews")?;
    let mut accessors = edit.take_array("accessors")?;
    let mut images = edit.take_array("images")?;
    let mut textures = edit.take_array("textures")?;
    let mut samplers = edit.take_array("samplers")?;
    let mut old_materials = edit.take_array("materials")?;
    let first_material = old_materials.len();
    let sampler = samplers.len();
    samplers.push(object([
        ("magFilter", number(9729)),
        ("minFilter", number(9987)),
        ("wrapS", number(10497)),
        ("wrapT", number(10497)),
    ]));
    let mut emissive_extension = false;
    for m in materials {
        if m.fur.is_some_and(|fur| !fur.valid()) {
            return Err(validation("invalid fur material"));
        }
        if m.base_color
            .iter()
            .chain(&m.emissive)
            .chain([
                &m.metallic,
                &m.roughness,
                &m.occlusion_strength,
                &m.alpha_cutoff,
            ])
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !m.normal_scale.is_finite()
            || m.normal_scale < 0.
            || !m.emissive_strength.is_finite()
            || m.emissive_strength < 0.
            || !["OPAQUE", "MASK", "BLEND"].contains(&m.alpha_mode.as_str())
        {
            return Err(validation("invalid PBR material factors"));
        }
        let mut root = HashMap::from([
            ("alphaMode".into(), string(&m.alpha_mode)),
            ("alphaCutoff".into(), JsonValue::F64(m.alpha_cutoff)),
            ("doubleSided".into(), JsonValue::Bool(m.double_sided)),
            ("emissiveFactor".into(), floats(&m.emissive)),
        ]);
        let mut pbr = HashMap::from([
            ("baseColorFactor".into(), floats(&m.base_color)),
            ("metallicFactor".into(), JsonValue::F64(m.metallic)),
            ("roughnessFactor".into(), JsonValue::F64(m.roughness)),
        ]);
        let mut mip_maps = HashMap::new();
        for (name, map) in [
            ("baseColorTexture", &m.base_color_texture),
            ("metallicRoughnessTexture", &m.metallic_roughness_texture),
            ("normalTexture", &m.normal_texture),
            ("occlusionTexture", &m.occlusion_texture),
            ("emissiveTexture", &m.emissive_texture),
        ] {
            if let Some(map) = map {
                let (mut info, mips) = texture(
                    &mut edit,
                    &mut views,
                    &mut images,
                    &mut textures,
                    sampler,
                    map,
                )?;
                if let JsonValue::Object(fields) = &mut info {
                    if name == "normalTexture" {
                        fields.insert("scale".into(), JsonValue::F64(m.normal_scale));
                    }
                    if name == "occlusionTexture" {
                        fields.insert("strength".into(), JsonValue::F64(m.occlusion_strength));
                    }
                }
                if name == "baseColorTexture" || name == "metallicRoughnessTexture" {
                    pbr.insert(name.into(), info);
                } else {
                    root.insert(name.into(), info);
                }
                if !mips.is_empty() {
                    mip_maps.insert(name.into(), JsonValue::Array(mips));
                }
            }
        }
        root.insert("pbrMetallicRoughness".into(), JsonValue::Object(pbr));
        let mut extras = HashMap::from([("makepadSurface".into(), JsonValue::Bool(true))]);
        if let Some(fur) = m.fur {
            extras.insert("makepadFur".into(), object([
                ("length", JsonValue::F64(fur.length)),
                ("density", JsonValue::F64(fur.density)),
                ("scale", JsonValue::F64(fur.scale)),
                ("seed", number(fur.seed as usize)),
            ]));
        }
        if !mip_maps.is_empty() { extras.insert("makepadMips".into(), JsonValue::Object(mip_maps)); }
        root.insert("extras".into(), JsonValue::Object(extras));
        if m.emissive_strength != 1. {
            emissive_extension = true;
            root.insert(
                "extensions".into(),
                object([(
                    "KHR_materials_emissive_strength",
                    object([("emissiveStrength", JsonValue::F64(m.emissive_strength))]),
                )]),
            );
        }
        old_materials.push(JsonValue::Object(root));
    }
    let mut meshes = edit.take_array("meshes")?;
    let mut assigned = std::collections::BTreeSet::new();
    for p in primitives {
        if p.material >= materials.len() || !assigned.insert((p.mesh, p.primitive)) {
            return Err(validation("PBR primitive assignment"));
        }
        let source = edit
            .document
            .meshes_slice()
            .get(p.mesh)
            .and_then(|m| m.primitives.get(p.primitive))
            .ok_or_else(|| validation("PBR primitive not found"))?;
        let position = source
            .attributes
            .get("POSITION")
            .ok_or_else(|| validation("PBR POSITION missing"))?;
        let count = edit
            .document
            .accessors_slice()
            .get(*position)
            .ok_or_else(|| validation("PBR position accessor"))?
            .count;
        let mut attrs = Vec::new();
        for (name, values) in [("COLOR_0", &p.colors), ("TANGENT", &p.tangents)] {
            if values.is_empty() {
                continue;
            }
            if values.len() != count || values.iter().flatten().any(|x| !x.is_finite()) {
                return Err(validation("PBR attribute dimensions"));
            }
            if name == "COLOR_0" && values.iter().flatten().any(|v| !(0.0..=1.0).contains(v)) {
                return Err(validation("PBR vertex color range"));
            }
            let bytes: Vec<u8> = values
                .iter()
                .flatten()
                .flat_map(|v| v.to_le_bytes())
                .collect();
            let index = edit.append_accessor(
                &mut views,
                &mut accessors,
                &bytes,
                5126,
                count,
                "VEC4",
                false,
                Some(34962),
                None,
                None,
            );
            attrs.push((name, index));
        }
        let Some(JsonValue::Object(mesh)) = meshes.get_mut(p.mesh) else {
            return Err(validation("PBR mesh JSON"));
        };
        let Some(JsonValue::Array(ps)) = mesh.get_mut("primitives") else {
            return Err(validation("PBR primitive JSON"));
        };
        let Some(JsonValue::Object(primitive)) = ps.get_mut(p.primitive) else {
            return Err(validation("PBR primitive JSON"));
        };
        primitive.insert("material".into(), number(first_material + p.material));
        let Some(JsonValue::Object(attributes)) = primitive.get_mut("attributes") else {
            return Err(validation("PBR attributes JSON"));
        };
        for (name, index) in attrs {
            attributes.insert(name.into(), number(index));
        }
    }
    if emissive_extension {
        let used = edit.array_mut("extensionsUsed")?;
        if !used
            .iter()
            .any(|v| matches!(v,JsonValue::String(s) if s=="KHR_materials_emissive_strength"))
        {
            used.push(string("KHR_materials_emissive_strength"));
        }
    }
    edit.put_array("bufferViews", views);
    edit.put_array("accessors", accessors);
    edit.put_array("images", images);
    edit.put_array("textures", textures);
    edit.put_array("samplers", samplers);
    edit.put_array("materials", old_materials);
    edit.put_array("meshes", meshes);
    edit.finish()
}
