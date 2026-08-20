//! Doors and hazard floors as their own nodes inside an exported map GLB.
//!
//! The level mesh stays one static node. Each door becomes its own node
//! named `door_N` holding only the moving geometry, with:
//! - `extras`: `{"kind":"door","states":["closed","open"],"default":"open",
//!   "axis":"y","closed":…,"open":…,"travel":…,"seconds":…}`,
//! - a rest `translation` at the OPEN pose, so a static viewer (and a
//!   walker that ignores animation) sees a walkable doorway, and
//! - one glTF animation named exactly `door_N` whose single LINEAR
//!   `translation` channel runs t=0 CLOSED → t=`seconds` OPEN.
//!
//! The geometry is authored in the closed pose, so the animation's first
//! keyframe is the identity translation and the node's rest transform is
//! the last one.
//!
//! A hazard floor (nukage, lava, water) uses the same mechanism without an
//! animation: its triangles leave the level mesh for a `hazard_N` node whose
//! extras say what the liquid does, so level collision can tag them "avoid"
//! in one pass.
//!
//! This is a JSON-level augmentation of a finished GLB: the level mesh, its
//! atlas image and its material are reused untouched (an added node is
//! textured by the same atlas material, index 0).

/// Doom's `PLATWAIT`: a lift waits three seconds at the bottom.
pub const LIFT_WAIT_SECONDS: f32 = 3.0;

use makepad_asset_client::json::{self, Value};
use makepad_gltf::compute_vertex_normals;

/// How long a door takes to travel, seconds. Doom's `VDOORSPEED` is 2 map
/// units per tic, so a 124-unit door takes ~1.8 s; one second is the round
/// figure a game can rescale.
pub const DOOR_SECONDS: f32 = 1.0;

/// Geometry that leaves the level mesh for a node of its own.
#[derive(Clone, Debug, Default)]
pub struct ExtraNode {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Baked light (COLOR_0), same convention as the level mesh around it.
    pub colors: Vec<[f32; 3]>,
    /// Rest translation of the node. A door rests OPEN; a hazard floor does
    /// not move at all.
    pub rest: [f32; 3],
    /// `extras` object entries, in the order a reader sees them.
    pub extras: Vec<(String, Value)>,
    /// When set, a LINEAR translation clip named like the node: t=0 at the
    /// origin (the pose the geometry is authored in), t=`seconds` at the
    /// far pose. A door rests at its far pose (open); a lift rests at the
    /// origin (up) and travels away from it.
    pub animation: Option<(f32, [f32; 3])>,
    /// When non-empty, this node gets its OWN material(s) over these images
    /// instead of the level's atlas (the sky is not part of the level
    /// atlas). More than one image is a layered sky: the node's `extras`
    /// gain a `layers` array of glTF TEXTURE indices in layer order.
    pub images: Vec<Vec<u8>>,
}

impl ExtraNode {
    /// A door: geometry authored CLOSED, resting OPEN, with its clip.
    pub fn door(
        name: impl Into<String>,
        positions: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
        colors: Vec<[f32; 3]>,
        closed_y: f32,
        open_y: f32,
        secret: bool,
        key: Option<&str>,
    ) -> Self {
        let travel = open_y - closed_y;
        Self {
            name: name.into(),
            positions,
            uvs,
            indices,
            colors,
            rest: [0.0, travel, 0.0],
            extras: [
                ("kind".to_string(), json::s("door")),
                (
                    "states".to_string(),
                    Value::Arr(vec![json::s("closed"), json::s("open")]),
                ),
                ("default".to_string(), json::s("open")),
                ("axis".to_string(), json::s("y")),
                ("closed".to_string(), Value::F64(closed_y as f64)),
                ("open".to_string(), Value::F64(open_y as f64)),
                ("travel".to_string(), Value::F64(travel as f64)),
                ("seconds".to_string(), Value::F64(DOOR_SECONDS as f64)),
                ("secret".to_string(), Value::Bool(secret)),
            ]
            .into_iter()
            .chain(key.map(|k| ("key".to_string(), json::s(k))))
            .collect(),
            animation: Some((DOOR_SECONDS, [0.0, travel, 0.0])),
            images: Vec::new(),
        }
    }

    /// A door that slides along an arbitrary vector (Quake's `func_door`:
    /// `angle` picks the axis, the travel is the brush's own size on it
    /// minus `lip`). Same contract as [`ExtraNode::door`]: authored CLOSED,
    /// resting OPEN, one clip named like the node.
    pub fn door_vector(
        name: impl Into<String>,
        positions: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
        travel: [f32; 3],
        axis: &str,
    ) -> Self {
        let distance = (travel[0] * travel[0] + travel[1] * travel[1] + travel[2] * travel[2])
            .sqrt();
        Self {
            name: name.into(),
            positions,
            uvs,
            indices,
            colors: Vec::new(),
            rest: travel,
            extras: vec![
                ("kind".into(), json::s("door")),
                (
                    "states".into(),
                    Value::Arr(vec![json::s("closed"), json::s("open")]),
                ),
                ("default".into(), json::s("open")),
                ("axis".into(), json::s(axis)),
                ("travel".into(), Value::F64(distance as f64)),
                (
                    "offset".into(),
                    Value::Arr(travel.iter().map(|v| Value::F64(*v as f64)).collect()),
                ),
                ("seconds".into(), Value::F64(DOOR_SECONDS as f64)),
                ("secret".into(), Value::Bool(false)),
            ],
            animation: Some((DOOR_SECONDS, travel)),
            images: Vec::new(),
        }
    }

    /// A lift: the floor authored in its UP pose, resting UP, with a clip
    /// that runs t=0 UP -> t=seconds DOWN (`travel` is negative).
    pub fn lift(
        name: impl Into<String>,
        positions: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
        colors: Vec<[f32; 3]>,
        up_y: f32,
        down_y: f32,
    ) -> Self {
        let travel = down_y - up_y;
        Self {
            name: name.into(),
            positions,
            uvs,
            indices,
            colors,
            // Rest is UP: the level is baked with its lifts raised, which is
            // where a walker meets them.
            rest: [0.0, 0.0, 0.0],
            extras: vec![
                ("kind".into(), json::s("lift")),
                (
                    "states".into(),
                    Value::Arr(vec![json::s("up"), json::s("down")]),
                ),
                ("default".into(), json::s("up")),
                ("axis".into(), json::s("y")),
                ("up".into(), Value::F64(up_y as f64)),
                ("down".into(), Value::F64(down_y as f64)),
                ("travel".into(), Value::F64(travel as f64)),
                ("seconds".into(), Value::F64(DOOR_SECONDS as f64)),
                ("wait".into(), Value::F64(LIFT_WAIT_SECONDS as f64)),
            ],
            animation: Some((DOOR_SECONDS, [0.0, travel, 0.0])),
            images: Vec::new(),
        }
    }

    /// The sky: the faces a classic map leaves open (Doom's F_SKY1 ceilings),
    /// kept as geometry so the renderer can direction-map them.
    ///
    /// `projection` is `cylinder` (Doom/Duke), `quake_scroll` (two scrolling
    /// layers) or `cube`. `repeat` is how many times the image wraps per 360°
    /// — Doom's 256-wide sky over a 1024-unit turn is 4.
    #[allow(clippy::too_many_arguments)]
    pub fn sky(
        positions: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
        images: Vec<Vec<u8>>,
        projection: &str,
        repeat: f32,
        offset: f32,
        texture: &str,
        speeds: Option<[f32; 2]>,
        v_span: Option<f32>,
    ) -> Self {
        let mut extras = vec![
            ("kind".into(), json::s("sky")),
            ("projection".into(), json::s(projection)),
            ("repeat".into(), Value::F64(repeat as f64)),
            ("offset".into(), Value::F64(offset as f64)),
            ("texture".into(), json::s(texture.to_ascii_lowercase())),
        ];
        if let Some([back, front]) = speeds {
            extras.push((
                "speeds".into(),
                Value::Arr(vec![Value::F64(back as f64), Value::F64(front as f64)]),
            ));
        }
        if let Some(v) = v_span {
            extras.push(("v_span".into(), Value::F64(v as f64)));
        }
        Self {
            name: "sky".into(),
            positions,
            uvs,
            indices,
            colors: Vec::new(),
            rest: [0.0, 0.0, 0.0],
            extras,
            animation: None,
            images,
        }
    }

    /// A hazard floor: static geometry, tagged for level collision.
    pub fn hazard(
        name: impl Into<String>,
        positions: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
        colors: Vec<[f32; 3]>,
        damage: u8,
        flat: &str,
        liquid: bool,
        solid: bool,
    ) -> Self {
        Self {
            name: name.into(),
            positions,
            uvs,
            indices,
            colors,
            rest: [0.0, 0.0, 0.0],
            extras: vec![
                ("kind".into(), json::s("hazard")),
                ("damage".into(), Value::Int(damage as i64)),
                ("flat".into(), json::s(flat.to_ascii_lowercase())),
                ("liquid".into(), Value::Bool(liquid)),
                ("solid".into(), Value::Bool(solid)),
            ],
            animation: None,
            images: Vec::new(),
        }
    }
}

/// Add `extra` nodes to `glb`. Returns the input unchanged when there is
/// nothing to add; fails closed (returns `Err`) if the GLB is not the shape
/// this writer produced.
pub fn inject_nodes(glb: &[u8], doors: &[ExtraNode]) -> Result<Vec<u8>, String> {
    if doors.is_empty() {
        return Ok(glb.to_vec());
    }
    let (mut root, mut bin) = split_glb(glb)?;
    let mut views = take_array(&mut root, "bufferViews")?;
    let mut accessors = take_array(&mut root, "accessors")?;
    let mut meshes = take_array(&mut root, "meshes")?;
    let mut nodes = take_array(&mut root, "nodes")?;
    let mut animations = take_array(&mut root, "animations").unwrap_or_default();
    let mut scenes = take_array(&mut root, "scenes")?;
    let mut images = take_array(&mut root, "images").unwrap_or_default();
    let mut textures = take_array(&mut root, "textures").unwrap_or_default();
    let mut materials = take_array(&mut root, "materials").unwrap_or_default();
    let mut samplers = take_array(&mut root, "samplers").unwrap_or_default();
    if meshes.is_empty() || nodes.is_empty() || scenes.is_empty() {
        return Err("glb has no mesh/node/scene to extend".into());
    }
    // A door is painted by the level's own atlas material. A GLB without
    // materials (untextured test fixture) gets the glTF default instead of
    // an out-of-bounds index.
    let level_material = (!materials.is_empty()).then_some(0i64);

    for door in doors {
        if door.indices.len() < 3 || door.positions.is_empty() {
            continue;
        }
        if door.uvs.len() != door.positions.len() {
            return Err(format!("{}: uvs must match positions", door.name));
        }
        let normals = compute_vertex_normals(&door.positions, &door.indices);
        let idx = push_accessor(
            &mut bin,
            &mut views,
            &mut accessors,
            &u32_bytes(&door.indices),
            5125,
            door.indices.len(),
            "SCALAR",
            Some(34963),
            None,
        );
        let pos = push_accessor(
            &mut bin,
            &mut views,
            &mut accessors,
            &f32_bytes(&flatten3(&door.positions)),
            5126,
            door.positions.len(),
            "VEC3",
            Some(34962),
            Some(bounds3(&door.positions)),
        );
        let nrm = push_accessor(
            &mut bin,
            &mut views,
            &mut accessors,
            &f32_bytes(&flatten3(&normals)),
            5126,
            normals.len(),
            "VEC3",
            Some(34962),
            None,
        );
        let uv = push_accessor(
            &mut bin,
            &mut views,
            &mut accessors,
            &f32_bytes(&flatten2(&door.uvs)),
            5126,
            door.uvs.len(),
            "VEC2",
            Some(34962),
            None,
        );
        // Baked light travels with the geometry, like the level mesh's.
        let color = (door.colors.len() == door.positions.len()).then(|| {
            push_accessor(
                &mut bin,
                &mut views,
                &mut accessors,
                &f32_bytes(&flatten3(&door.colors)),
                5126,
                door.colors.len(),
                "VEC3",
                Some(34962),
                None,
            )
        });
        // Animation samplers read from the buffer without a target.
        let clip = door.animation.map(|(seconds, far)| {
            let times = [0.0f32, seconds];
            let input = push_accessor(
                &mut bin,
                &mut views,
                &mut accessors,
                &f32_bytes(&times),
                5126,
                times.len(),
                "SCALAR",
                None,
                Some((vec![times[0]], vec![times[1]])),
            );
            let values = [0.0f32, 0.0, 0.0, far[0], far[1], far[2]];
            let output = push_accessor(
                &mut bin,
                &mut views,
                &mut accessors,
                &f32_bytes(&values),
                5126,
                2,
                "VEC3",
                None,
                None,
            );
            (input, output)
        });

        // A node with its own image (the sky) gets its own material; the
        // rest paint with the level's atlas.
        let mut layer_textures: Vec<i64> = Vec::new();
        for png in &door.images {
            while bin.len() % 4 != 0 {
                bin.push(0);
            }
            let offset = bin.len();
            bin.extend_from_slice(png);
            let view_index = views.len();
            views.push(json::obj(vec![
                ("buffer", Value::Int(0)),
                ("byteOffset", Value::Int(offset as i64)),
                ("byteLength", Value::Int(png.len() as i64)),
            ]));
            let image_index = images.len();
            images.push(json::obj(vec![
                ("bufferView", Value::Int(view_index as i64)),
                ("mimeType", json::s("image/png")),
            ]));
            if samplers.is_empty() {
                samplers.push(json::obj(vec![("wrapS", Value::Int(10497))]));
            }
            layer_textures.push(textures.len() as i64);
            textures.push(json::obj(vec![
                ("source", Value::Int(image_index as i64)),
                ("sampler", Value::Int(0)),
            ]));
        }
        let material = match layer_textures.first() {
            Some(&texture_index) => {
                let material_index = materials.len();
                materials.push(json::obj(vec![
                    (
                        "pbrMetallicRoughness",
                        json::obj(vec![
                            (
                                "baseColorTexture",
                                json::obj(vec![("index", Value::Int(texture_index))]),
                            ),
                            ("metallicFactor", Value::F64(0.0)),
                            ("roughnessFactor", Value::F64(1.0)),
                        ]),
                    ),
                    ("doubleSided", Value::Bool(true)),
                ]));
                Some(material_index as i64)
            }
            None => level_material,
        };
        let mesh_index = meshes.len();
        meshes.push(json::obj(vec![
            ("name", json::s(door.name.clone())),
            (
                "primitives",
                Value::Arr(vec![json::obj(vec![
                    (
                        "attributes",
                        Value::Obj(
                            vec![
                                ("POSITION".to_string(), Value::Int(pos as i64)),
                                ("NORMAL".to_string(), Value::Int(nrm as i64)),
                                ("TEXCOORD_0".to_string(), Value::Int(uv as i64)),
                            ]
                            .into_iter()
                            .chain(
                                color.map(|c| ("COLOR_0".to_string(), Value::Int(c as i64))),
                            )
                            .collect(),
                        ),
                    ),
                    ("indices", Value::Int(idx as i64)),
                    ("mode", Value::Int(4)),
                ]
                .into_iter()
                .chain(material.map(|m| ("material", Value::Int(m))))
                .collect::<Vec<_>>())]),
            ),
        ]));

        let node_index = nodes.len();
        let mut node = vec![
            ("name".to_string(), json::s(door.name.clone())),
            ("mesh".to_string(), Value::Int(mesh_index as i64)),
        ];
        // A door rests OPEN, so a viewer that plays nothing still walks
        // through; a static hazard floor rests at the origin.
        if door.rest != [0.0, 0.0, 0.0] {
            node.push((
                "translation".to_string(),
                Value::Arr(door.rest.iter().map(|v| Value::F64(*v as f64)).collect()),
            ));
        }
        if !door.extras.is_empty() {
            let mut extras = door.extras.clone();
            // Layer order for a multi-image sky: back first, then the keyed
            // front layer, as glTF TEXTURE indices.
            if layer_textures.len() > 1 {
                extras.push((
                    "layers".to_string(),
                    Value::Arr(layer_textures.iter().map(|t| Value::Int(*t)).collect()),
                ));
            }
            node.push(("extras".to_string(), Value::Obj(extras)));
        }
        nodes.push(Value::Obj(node));
        push_scene_node(&mut scenes, node_index);

        if let Some((input, output)) = clip {
            animations.push(json::obj(vec![
                ("name", json::s(door.name.clone())),
                (
                    "samplers",
                    Value::Arr(vec![json::obj(vec![
                        ("input", Value::Int(input as i64)),
                        ("interpolation", json::s("LINEAR")),
                        ("output", Value::Int(output as i64)),
                    ])]),
                ),
                (
                    "channels",
                    Value::Arr(vec![json::obj(vec![
                        ("sampler", Value::Int(0)),
                        (
                            "target",
                            json::obj(vec![
                                ("node", Value::Int(node_index as i64)),
                                ("path", json::s("translation")),
                            ]),
                        ),
                    ])]),
                ),
            ]));
        }
    }

    put_array(&mut root, "bufferViews", views);
    put_array(&mut root, "accessors", accessors);
    put_array(&mut root, "meshes", meshes);
    put_array(&mut root, "nodes", nodes);
    put_array(&mut root, "scenes", scenes);
    if !images.is_empty() {
        put_array(&mut root, "images", images);
    }
    if !textures.is_empty() {
        put_array(&mut root, "textures", textures);
    }
    if !materials.is_empty() {
        put_array(&mut root, "materials", materials);
    }
    if !samplers.is_empty() {
        put_array(&mut root, "samplers", samplers);
    }
    if !animations.is_empty() {
        put_array(&mut root, "animations", animations);
    }
    set_buffer_length(&mut root, bin.len());
    Ok(assemble_glb(&root.to_json(), &bin))
}

// ---------------------------------------------------------------------------
// GLB container
// ---------------------------------------------------------------------------

fn split_glb(glb: &[u8]) -> Result<(Value, Vec<u8>), String> {
    if glb.len() < 20 || &glb[0..4] != b"glTF" {
        return Err("not a glb".into());
    }
    let mut off = 12usize;
    let mut json = None;
    let mut bin = Vec::new();
    while off + 8 <= glb.len() {
        let len = u32::from_le_bytes(glb[off..off + 4].try_into().unwrap()) as usize;
        let kind = &glb[off + 4..off + 8];
        let start = off + 8;
        let end = start.checked_add(len).ok_or("glb chunk overflow")?;
        if end > glb.len() {
            return Err("glb chunk truncated".into());
        }
        if kind == b"JSON" {
            json = Some(json::parse(&glb[start..end]).map_err(|e| format!("glb json: {e}"))?);
        } else if kind == b"BIN\0" {
            bin = glb[start..end].to_vec();
        }
        off = end;
    }
    Ok((json.ok_or("glb has no json chunk")?, bin))
}

fn assemble_glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = json.as_bytes().to_vec();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let mut bin_bytes = bin.to_vec();
    while bin_bytes.len() % 4 != 0 {
        bin_bytes.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin_bytes.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin_bytes);
    out
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

/// Remove an array from the document so it can be extended and put back.
/// `images`/`materials` may legitimately be absent (an untextured mesh).
fn take_array(root: &mut Value, name: &str) -> Result<Vec<Value>, String> {
    let Value::Obj(pairs) = root else {
        return Err("glb json is not an object".into());
    };
    match pairs.iter().position(|(k, _)| k == name) {
        Some(i) => match pairs.remove(i).1 {
            Value::Arr(items) => Ok(items),
            _ => Err(format!("glb json {name} is not an array")),
        },
        None => Err(format!("glb json has no {name}")),
    }
}

fn put_array(root: &mut Value, name: &str, items: Vec<Value>) {
    if let Value::Obj(pairs) = root {
        pairs.push((name.to_string(), Value::Arr(items)));
    }
}

fn push_scene_node(scenes: &mut [Value], node: usize) {
    if let Some(Value::Obj(pairs)) = scenes.first_mut() {
        if let Some((_, Value::Arr(list))) = pairs.iter_mut().find(|(k, _)| k == "nodes") {
            list.push(Value::Int(node as i64));
            return;
        }
        pairs.push((
            "nodes".to_string(),
            Value::Arr(vec![Value::Int(node as i64)]),
        ));
    }
}

fn set_buffer_length(root: &mut Value, len: usize) {
    if let Value::Obj(pairs) = root {
        if let Some((_, Value::Arr(buffers))) = pairs.iter_mut().find(|(k, _)| k == "buffers") {
            if let Some(Value::Obj(buffer)) = buffers.first_mut() {
                match buffer.iter_mut().find(|(k, _)| k == "byteLength") {
                    Some((_, v)) => *v = Value::Int(len as i64),
                    None => buffer.push(("byteLength".into(), Value::Int(len as i64))),
                }
            }
        }
    }
}

/// Append `bytes` to the BIN chunk (4-byte aligned) and register a
/// bufferView + accessor over them. Returns the accessor index.
#[allow(clippy::too_many_arguments)]
fn push_accessor(
    bin: &mut Vec<u8>,
    views: &mut Vec<Value>,
    accessors: &mut Vec<Value>,
    bytes: &[u8],
    component_type: i64,
    count: usize,
    kind: &str,
    target: Option<i64>,
    min_max: Option<(Vec<f32>, Vec<f32>)>,
) -> usize {
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let offset = bin.len();
    bin.extend_from_slice(bytes);
    let view_index = views.len();
    let mut view = vec![
        ("buffer", Value::Int(0)),
        ("byteOffset", Value::Int(offset as i64)),
        ("byteLength", Value::Int(bytes.len() as i64)),
    ];
    if let Some(t) = target {
        view.push(("target", Value::Int(t)));
    }
    views.push(json::obj(view));
    let mut accessor = vec![
        ("bufferView", Value::Int(view_index as i64)),
        ("componentType", Value::Int(component_type)),
        ("count", Value::Int(count as i64)),
        ("type", json::s(kind)),
    ];
    if let Some((min, max)) = min_max {
        accessor.push(("min", Value::Arr(f64_array(&min))));
        accessor.push(("max", Value::Arr(f64_array(&max))));
    }
    accessors.push(json::obj(accessor));
    accessors.len() - 1
}

fn f64_array(values: &[f32]) -> Vec<Value> {
    values.iter().map(|v| Value::F64(*v as f64)).collect()
}

fn u32_bytes(values: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn flatten3(values: &[[f32; 3]]) -> Vec<f32> {
    values.iter().flat_map(|v| v.iter().copied()).collect()
}

fn flatten2(values: &[[f32; 2]]) -> Vec<f32> {
    values.iter().flat_map(|v| v.iter().copied()).collect()
}

fn bounds3(values: &[[f32; 3]]) -> (Vec<f32>, Vec<f32>) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in values {
        for i in 0..3 {
            min[i] = min[i].min(v[i]);
            max[i] = max[i].max(v[i]);
        }
    }
    (min.to_vec(), max.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_gltf::write_glb_mesh;

    fn quad_door(travel: f32) -> ExtraNode {
        ExtraNode::door(
            "door_1",
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 2.0, 0.0],
                [0.0, 2.0, 0.0],
            ],
            vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            vec![0, 1, 2, 0, 2, 3],
            vec![[1.0, 1.0, 1.0]; 4],
            0.0,
            travel,
            false,
            None,
        )
    }

    fn base_glb() -> Vec<u8> {
        write_glb_mesh(
            &[[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, 4.0]],
            &[0, 1, 2],
        )
    }

    #[test]
    fn a_door_becomes_a_named_node_with_states_and_an_animation() {
        let glb = inject_nodes(&base_glb(), &[quad_door(1.9375)]).unwrap();
        let (root, bin) = split_glb(&glb).unwrap();

        let nodes = root.get("nodes").unwrap().as_arr().unwrap();
        let door = nodes
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some("door_1"))
            .expect("door node");
        // Rest pose is OPEN so a static walker sees a walkable doorway.
        let t = door.get("translation").unwrap().as_arr().unwrap();
        assert_eq!(t.len(), 3);
        assert!((as_f64(&t[1]) - 1.9375).abs() < 1e-6, "{t:?}");
        assert_eq!(as_f64(&t[0]), 0.0);

        let extras = door.get("extras").unwrap();
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("door"));
        assert_eq!(extras.get("default").and_then(Value::as_str), Some("open"));
        assert_eq!(extras.get("axis").and_then(Value::as_str), Some("y"));
        let states: Vec<&str> = extras
            .get("states")
            .and_then(Value::as_arr)
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(states, vec!["closed", "open"]);
        assert!((as_f64(extras.get("travel").unwrap()) - 1.9375).abs() < 1e-6);

        // The animation is named exactly like the node it drives.
        let anims = root.get("animations").unwrap().as_arr().unwrap();
        assert_eq!(anims.len(), 1);
        let anim = &anims[0];
        assert_eq!(anim.get("name").and_then(Value::as_str), Some("door_1"));
        let channel = &anim.get("channels").unwrap().as_arr().unwrap()[0];
        assert_eq!(
            channel.get("target").unwrap().get("path").and_then(Value::as_str),
            Some("translation")
        );
        let node_index = channel
            .get("target")
            .unwrap()
            .get("node")
            .and_then(Value::as_i64)
            .unwrap() as usize;
        assert_eq!(
            nodes[node_index].get("name").and_then(Value::as_str),
            Some("door_1")
        );
        let sampler = &anim.get("samplers").unwrap().as_arr().unwrap()[0];
        assert_eq!(
            sampler.get("interpolation").and_then(Value::as_str),
            Some("LINEAR")
        );

        // t=0 is the CLOSED pose, t=end the open one.
        let accessors = root.get("accessors").unwrap().as_arr().unwrap();
        let out_i = sampler.get("output").and_then(Value::as_i64).unwrap() as usize;
        let values = read_f32(&root, &bin, &accessors[out_i]);
        assert_eq!(values.len(), 6);
        assert_eq!(&values[0..3], &[0.0, 0.0, 0.0]);
        assert!((values[4] - 1.9375).abs() < 1e-6);
        let in_i = sampler.get("input").and_then(Value::as_i64).unwrap() as usize;
        let times = read_f32(&root, &bin, &accessors[in_i]);
        assert_eq!(times, vec![0.0, DOOR_SECONDS]);
    }

    #[test]
    fn the_level_mesh_is_untouched_and_the_file_still_loads() {
        let base = base_glb();
        let glb = inject_nodes(&base, &[quad_door(1.0)]).unwrap();
        let loaded = makepad_gltf::load_gltf_from_bytes(&glb, None).expect("valid glb");
        let doc = &loaded.document;
        assert_eq!(doc.meshes.as_ref().map(Vec::len), Some(2), "level + door mesh");
        assert_eq!(doc.nodes.as_ref().map(Vec::len), Some(2));
        assert_eq!(doc.animations.as_ref().map(Vec::len), Some(1));
        let scenes = doc.scenes.as_ref().expect("scenes");
        assert_eq!(
            scenes[0].nodes.as_ref().map(Vec::len),
            Some(2),
            "the door joins the scene"
        );
    }

    #[test]
    fn no_doors_leaves_the_glb_byte_identical() {
        let base = base_glb();
        assert_eq!(inject_nodes(&base, &[]).unwrap(), base);
    }

    fn as_f64(v: &Value) -> f64 {
        match v {
            Value::F64(f) => *f,
            Value::Int(i) => *i as f64,
            _ => f64::NAN,
        }
    }

    fn read_f32(root: &Value, bin: &[u8], accessor: &Value) -> Vec<f32> {
        let views = root.get("bufferViews").unwrap().as_arr().unwrap();
        let vi = accessor.get("bufferView").and_then(Value::as_i64).unwrap() as usize;
        let off = views[vi].get("byteOffset").and_then(Value::as_i64).unwrap() as usize;
        let len = views[vi].get("byteLength").and_then(Value::as_i64).unwrap() as usize;
        bin[off..off + len]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }
}
