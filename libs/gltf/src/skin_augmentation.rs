//! Lossless skin and skeleton augmentation for an existing GLB.
//!
//! SkinTokens predicts absolute joint heads plus dense per-vertex weights.
//! Rebuilding the mesh from those results would discard material graphs,
//! textures, extensions, primitive boundaries and vendor metadata. This
//! module instead preserves the original JSON fields and BIN bytes, appends
//! only skinning accessors, and adds translation-only joint nodes. A native
//! animation retargeter can subsequently add rotations to those nodes.

use crate::augment::{number, object, string, validation, GlbRewrite};
use crate::{GltfError, GltfNode};
use makepad_math::{Mat4f, Quat};
use makepad_micro_serde::JsonValue;
use std::collections::{BTreeMap, HashMap, HashSet};

const ARRAY_BUFFER: usize = 34962;
const FLOAT: usize = 5126;
const UNSIGNED_BYTE: usize = 5121;
const UNSIGNED_SHORT: usize = 5123;
const EPS: f32 = 1.0e-8;

/// One SkinTokens joint. `global_translation` is the decoded absolute joint
/// head in the source GLB's scene coordinate system. The augmenter converts
/// it to a parent-local glTF node translation. SkinTokens does not predict a
/// bone roll or rest rotation, so the native contract intentionally uses an
/// identity rest rotation and lets animation target hierarchy-edge vectors.
#[derive(Clone, Debug, PartialEq)]
pub struct GlbSkinJoint {
    pub name: String,
    pub parent: Option<usize>,
    pub global_translation: [f32; 3],
}

/// Dense SkinTokens weights for one original mesh primitive.
///
/// `weights` is vertex-major `[vertex_count, joint_count]`. The augmenter
/// reproduces the reference export boundary by sorting each row, keeping the
/// strongest four influences, and renormalizing those four to sum to one.
#[derive(Clone, Debug, PartialEq)]
pub struct GlbPrimitiveSkin {
    /// Existing node that instantiates the mesh to skin.
    pub node: usize,
    /// Primitive ordinal within that node's mesh.
    pub primitive: usize,
    /// Float64 is intentional: the SkinTokens reference transfers sampled
    /// f32 sigmoid columns with SciPy/NumPy f64 distances and performs its
    /// top-four normalization in f64. The four serialized glTF lanes are
    /// converted to f32 only after that division.
    pub weights: Vec<f64>,
}

fn as_object_mut<'a>(value: &'a mut JsonValue, label: &str) -> Result<&'a mut HashMap<String, JsonValue>, GltfError> {
    match value {
        JsonValue::Object(fields) => Ok(fields),
        _ => Err(validation(format!("glTF '{label}' is not an object"))),
    }
}

fn as_array_mut<'a>(value: &'a mut JsonValue, label: &str) -> Result<&'a mut Vec<JsonValue>, GltfError> {
    match value {
        JsonValue::Array(values) => Ok(values),
        _ => Err(validation(format!("glTF '{label}' is not an array"))),
    }
}

fn trs_matrix(node: &GltfNode) -> Result<Mat4f, GltfError> {
    if let Some(matrix) = node.matrix {
        if node.translation.is_some() || node.rotation.is_some() || node.scale.is_some() {
            return Err(validation("glTF node mixes matrix and TRS transforms"));
        }
        if matrix.iter().any(|value| !value.is_finite()) {
            return Err(validation("glTF node matrix contains a non-finite value"));
        }
        return Ok(Mat4f { v: matrix });
    }
    let t = node.translation.unwrap_or([0.0; 3]);
    let r = node.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let s = node.scale.unwrap_or([1.0; 3]);
    if t.iter().chain(&r).chain(&s).any(|value| !value.is_finite()) {
        return Err(validation("glTF node TRS contains a non-finite value"));
    }
    if s.iter().any(|value| value.abs() <= EPS) {
        return Err(validation("glTF node has a singular scale"));
    }
    let q_len = r.iter().map(|value| value * value).sum::<f32>().sqrt();
    if q_len <= EPS {
        return Err(validation("glTF node has a zero-length quaternion"));
    }
    let q = Quat {
        x: r[0] / q_len,
        y: r[1] / q_len,
        z: r[2] / q_len,
        w: r[3] / q_len,
    };
    let (x2, y2, z2) = (q.x + q.x, q.y + q.y, q.z + q.z);
    let (xx, yy, zz) = (q.x * x2, q.y * y2, q.z * z2);
    let (xy, xz, yz) = (q.x * y2, q.x * z2, q.y * z2);
    let (wx, wy, wz) = (q.w * x2, q.w * y2, q.w * z2);
    Ok(Mat4f {
        v: [
            (1.0 - yy - zz) * s[0],
            (xy + wz) * s[0],
            (xz - wy) * s[0],
            0.0,
            (xy - wz) * s[1],
            (1.0 - xx - zz) * s[1],
            (yz + wx) * s[1],
            0.0,
            (xz + wy) * s[2],
            (yz - wx) * s[2],
            (1.0 - xx - yy) * s[2],
            0.0,
            t[0],
            t[1],
            t[2],
            1.0,
        ],
    })
}

fn node_world_matrices(nodes: &[GltfNode]) -> Result<Vec<Mat4f>, GltfError> {
    let mut parents = vec![None; nodes.len()];
    for (parent, node) in nodes.iter().enumerate() {
        for &child in node.children.as_deref().unwrap_or(&[]) {
            if child >= nodes.len() {
                return Err(validation(format!("glTF node child {child} is out of range")));
            }
            if parents[child].replace(parent).is_some() {
                return Err(validation(format!("glTF node {child} has multiple parents")));
            }
        }
    }
    let locals = nodes.iter().map(trs_matrix).collect::<Result<Vec<_>, _>>()?;
    let mut worlds = vec![Mat4f::identity(); nodes.len()];
    let mut state = vec![0u8; nodes.len()];
    fn resolve(
        node: usize,
        parents: &[Option<usize>],
        locals: &[Mat4f],
        worlds: &mut [Mat4f],
        state: &mut [u8],
    ) -> Result<Mat4f, GltfError> {
        match state[node] {
            2 => return Ok(worlds[node]),
            1 => return Err(validation(format!("glTF node hierarchy cycles at node {node}"))),
            _ => {}
        }
        state[node] = 1;
        let world = match parents[node] {
            Some(parent) => Mat4f::mul(&resolve(parent, parents, locals, worlds, state)?, &locals[node]),
            None => locals[node],
        };
        worlds[node] = world;
        state[node] = 2;
        Ok(world)
    }
    for node in 0..nodes.len() {
        resolve(node, &parents, &locals, &mut worlds, &mut state)?;
    }
    Ok(worlds)
}

fn inverse_checked(matrix: Mat4f, label: &str) -> Result<Mat4f, GltfError> {
    let inverse = matrix.invert();
    let identity = Mat4f::mul(&matrix, &inverse);
    let expected = Mat4f::identity();
    let error = identity
        .v
        .iter()
        .zip(expected.v)
        .map(|(actual, expected)| (actual - expected).abs())
        .fold(0.0f32, f32::max);
    if !error.is_finite() || error > 2.0e-3 {
        return Err(validation(format!("{label} is singular or numerically unstable")));
    }
    Ok(inverse)
}

fn f32_bytes(values: impl IntoIterator<Item = f32>) -> Vec<u8> {
    let values = values.into_iter();
    let mut bytes = Vec::with_capacity(values.size_hint().0 * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn validate_joints(joints: &[GlbSkinJoint]) -> Result<(usize, Vec<Vec<usize>>), GltfError> {
    if joints.is_empty() {
        return Err(validation("skin requires at least one joint"));
    }
    if joints.len() > u16::MAX as usize + 1 {
        return Err(validation("glTF JOINTS_0 cannot address more than 65536 joints"));
    }
    let mut names = HashSet::with_capacity(joints.len());
    let mut roots = Vec::new();
    let mut children = vec![Vec::new(); joints.len()];
    for (joint_index, joint) in joints.iter().enumerate() {
        if joint.name.trim().is_empty() || !names.insert(joint.name.as_str()) {
            return Err(validation(format!("joint {joint_index} has an empty or duplicate name")));
        }
        if joint.global_translation.iter().any(|value| !value.is_finite()) {
            return Err(validation(format!("joint {joint_index} has a non-finite head")));
        }
        match joint.parent {
            Some(parent) if parent >= joints.len() => {
                return Err(validation(format!("joint {joint_index} parent {parent} is out of range")));
            }
            Some(parent) if parent == joint_index => {
                return Err(validation(format!("joint {joint_index} is its own parent")));
            }
            Some(parent) => children[parent].push(joint_index),
            None => roots.push(joint_index),
        }
    }
    if roots.len() != 1 {
        return Err(validation(format!("skin must have exactly one skeleton root, found {}", roots.len())));
    }
    let mut seen = vec![0u8; joints.len()];
    fn visit(joint: usize, children: &[Vec<usize>], seen: &mut [u8]) -> Result<(), GltfError> {
        if seen[joint] == 1 {
            return Err(validation(format!("joint hierarchy cycles at joint {joint}")));
        }
        if seen[joint] == 2 {
            return Ok(());
        }
        seen[joint] = 1;
        for &child in &children[joint] {
            visit(child, children, seen)?;
        }
        seen[joint] = 2;
        Ok(())
    }
    visit(roots[0], &children, &mut seen)?;
    if seen.iter().any(|state| *state != 2) {
        return Err(validation("joint hierarchy contains a disconnected cycle"));
    }
    Ok((roots[0], children))
}

fn pack_weights(
    dense: &[f64],
    vertex_count: usize,
    joint_count: usize,
) -> Result<(Vec<[u16; 4]>, Vec<[f32; 4]>), GltfError> {
    if dense.len() != vertex_count * joint_count {
        return Err(validation(format!(
            "dense skin has {} values, expected {} vertices x {} joints = {}",
            dense.len(), vertex_count, joint_count, vertex_count * joint_count
        )));
    }
    let mut joints = Vec::with_capacity(vertex_count);
    let mut weights = Vec::with_capacity(vertex_count);
    let mut order = Vec::with_capacity(joint_count);
    for (vertex, row) in dense.chunks_exact(joint_count).enumerate() {
        if row.iter().any(|weight| !weight.is_finite() || *weight < 0.0) {
            return Err(validation(format!("skin weights for vertex {vertex} are non-finite or negative")));
        }
        order.clear();
        order.extend(0..joint_count);
        order.sort_unstable_by(|left, right| {
            row[*right]
                .total_cmp(&row[*left])
                .then_with(|| left.cmp(right))
        });
        let mut joint_row = [0u16; 4];
        let mut weight_row = [0.0f32; 4];
        let kept = 4.min(joint_count);
        let sum = order[..kept].iter().map(|joint| row[*joint]).sum::<f64>();
        if !sum.is_finite() || sum <= 0.0 {
            return Err(validation(format!("skin weights for vertex {vertex} have zero top-four sum")));
        }
        for lane in 0..kept {
            joint_row[lane] = order[lane] as u16;
            weight_row[lane] = (row[order[lane]] / sum) as f32;
        }
        joints.push(joint_row);
        weights.push(weight_row);
    }
    Ok((joints, weights))
}

/// Add SkinTokens joints and weights to an existing, unskinned GLB without
/// rebuilding any original geometry or material data.
///
/// Each mesh node receives its own skin because inverse bind matrices include
/// that node's global bind transform. A mesh instanced by more than one
/// skinned node is rejected: glTF attributes live on the mesh, while the
/// SkinTokens inference samples each world-space occurrence independently.
pub fn augment_glb_skin(
    input: &[u8],
    joints: &[GlbSkinJoint],
    primitive_skins: &[GlbPrimitiveSkin],
) -> Result<Vec<u8>, GltfError> {
    let (root_joint, children) = validate_joints(joints)?;
    if primitive_skins.is_empty() {
        return Err(validation("skin requires weights for at least one mesh primitive"));
    }
    let mut rewrite = GlbRewrite::begin(input)?;
    if rewrite.document.skins.as_ref().is_some_and(|skins| !skins.is_empty())
        || rewrite.document.nodes_slice().iter().any(|node| node.skin.is_some())
    {
        return Err(validation("lossless skin augmentation requires an unskinned input GLB"));
    }
    let original_node_count = rewrite.document.nodes_slice().len();
    let worlds = node_world_matrices(rewrite.document.nodes_slice())?;
    let mut grouped: BTreeMap<usize, BTreeMap<usize, &GlbPrimitiveSkin>> = BTreeMap::new();
    let mut mesh_owner = HashMap::new();
    for primitive_skin in primitive_skins {
        let node = rewrite
            .document
            .nodes_slice()
            .get(primitive_skin.node)
            .ok_or_else(|| validation(format!("skin target node {} is out of range", primitive_skin.node)))?;
        let mesh_index = node
            .mesh
            .ok_or_else(|| validation(format!("skin target node {} has no mesh", primitive_skin.node)))?;
        if let Some(previous) = mesh_owner.insert(mesh_index, primitive_skin.node) {
            if previous != primitive_skin.node {
                return Err(validation(format!(
                    "mesh {mesh_index} is instanced by skin target nodes {previous} and {}; per-instance weights are ambiguous",
                    primitive_skin.node
                )));
            }
        }
        if grouped
            .entry(primitive_skin.node)
            .or_default()
            .insert(primitive_skin.primitive, primitive_skin)
            .is_some()
        {
            return Err(validation(format!(
                "duplicate skin weights for node {} primitive {}",
                primitive_skin.node, primitive_skin.primitive
            )));
        }
    }
    for (&node_index, primitive_map) in &grouped {
        let mesh_index = rewrite.document.nodes_slice()[node_index].mesh.unwrap();
        let mesh = &rewrite.document.meshes_slice()[mesh_index];
        if primitive_map.len() != mesh.primitives.len()
            || (0..mesh.primitives.len()).any(|primitive| !primitive_map.contains_key(&primitive))
        {
            return Err(validation(format!(
                "node {node_index} must provide weights for all {} mesh primitives",
                mesh.primitives.len()
            )));
        }
    }

    let mut views = rewrite.take_array("bufferViews")?;
    let mut accessors = rewrite.take_array("accessors")?;
    let mut meshes_json = rewrite.take_array("meshes")?;
    let mut nodes_json = rewrite.take_array("nodes")?;
    if nodes_json.len() != original_node_count
        || meshes_json.len() != rewrite.document.meshes_slice().len()
    {
        return Err(validation("typed glTF document and source JSON disagree"));
    }

    for (&node_index, primitive_map) in &grouped {
        let mesh_index = rewrite.document.nodes_slice()[node_index].mesh.unwrap();
        let mesh_json = as_object_mut(&mut meshes_json[mesh_index], &format!("meshes[{mesh_index}]"))?;
        let primitives_json = as_array_mut(
            mesh_json
                .get_mut("primitives")
                .ok_or_else(|| validation(format!("mesh {mesh_index} has no primitives array")))?,
            &format!("meshes[{mesh_index}].primitives"),
        )?;
        for (&primitive_index, primitive_skin) in primitive_map {
            let primitive = &rewrite.document.meshes_slice()[mesh_index].primitives[primitive_index];
            if primitive.attributes.contains_key("JOINTS_0") || primitive.attributes.contains_key("WEIGHTS_0") {
                return Err(validation(format!("mesh {mesh_index} primitive {primitive_index} is already skinned")));
            }
            let position_accessor = primitive.attributes.get("POSITION").ok_or_else(|| {
                validation(format!("mesh {mesh_index} primitive {primitive_index} has no POSITION"))
            })?;
            let vertex_count = rewrite.document.accessors_slice()[*position_accessor].count;
            let (joint_rows, weight_rows) = pack_weights(&primitive_skin.weights, vertex_count, joints.len())?;
            let joint_bytes = if joints.len() <= 256 {
                joint_rows
                    .iter()
                    .flat_map(|row| row.iter().map(|joint| *joint as u8))
                    .collect::<Vec<_>>()
            } else {
                joint_rows
                    .iter()
                    .flat_map(|row| row.iter().flat_map(|joint| joint.to_le_bytes()))
                    .collect::<Vec<_>>()
            };
            let joint_accessor = rewrite.append_accessor(
                &mut views,
                &mut accessors,
                &joint_bytes,
                if joints.len() <= 256 { UNSIGNED_BYTE } else { UNSIGNED_SHORT },
                vertex_count,
                "VEC4",
                false,
                Some(ARRAY_BUFFER),
                None,
                None,
            );
            let weight_bytes = f32_bytes(weight_rows.iter().flat_map(|row| row.iter().copied()));
            let weight_accessor = rewrite.append_accessor(
                &mut views,
                &mut accessors,
                &weight_bytes,
                FLOAT,
                vertex_count,
                "VEC4",
                false,
                Some(ARRAY_BUFFER),
                None,
                None,
            );
            let primitive_json = as_object_mut(
                &mut primitives_json[primitive_index],
                &format!("meshes[{mesh_index}].primitives[{primitive_index}]"),
            )?;
            let attributes = as_object_mut(
                primitive_json
                    .get_mut("attributes")
                    .ok_or_else(|| validation("mesh primitive has no attributes object"))?,
                "mesh primitive attributes",
            )?;
            attributes.insert("JOINTS_0".to_string(), number(joint_accessor));
            attributes.insert("WEIGHTS_0".to_string(), number(weight_accessor));
        }
    }

    let joint_node_base = nodes_json.len();
    for (joint_index, joint) in joints.iter().enumerate() {
        let translation = match joint.parent {
            Some(parent) => [
                joint.global_translation[0] - joints[parent].global_translation[0],
                joint.global_translation[1] - joints[parent].global_translation[1],
                joint.global_translation[2] - joints[parent].global_translation[2],
            ],
            None => joint.global_translation,
        };
        let mut fields = HashMap::from([
            ("name".to_string(), string(joint.name.clone())),
            (
                "translation".to_string(),
                JsonValue::Array(translation.into_iter().map(|value| JsonValue::F64(value as f64)).collect()),
            ),
        ]);
        if !children[joint_index].is_empty() {
            fields.insert(
                "children".to_string(),
                JsonValue::Array(children[joint_index].iter().map(|child| number(joint_node_base + child)).collect()),
            );
        }
        nodes_json.push(JsonValue::Object(fields));
    }

    let mut skins_json = Vec::with_capacity(grouped.len());
    let joint_node_indices = JsonValue::Array((0..joints.len()).map(|joint| number(joint_node_base + joint)).collect());
    for (&node_index, _) in &grouped {
        let mesh_world = worlds[node_index];
        let mut matrices = Vec::with_capacity(joints.len() * 16);
        for joint in joints {
            let mut joint_world = Mat4f::identity();
            joint_world.v[12..15].copy_from_slice(&joint.global_translation);
            let inverse_joint = inverse_checked(joint_world, "joint bind transform")?;
            matrices.extend_from_slice(&Mat4f::mul(&inverse_joint, &mesh_world).v);
        }
        let matrix_bytes = f32_bytes(matrices);
        let inverse_bind_accessor = rewrite.append_accessor(
            &mut views,
            &mut accessors,
            &matrix_bytes,
            FLOAT,
            joints.len(),
            "MAT4",
            false,
            None,
            None,
            None,
        );
        let skin_index = skins_json.len();
        skins_json.push(object([
            ("name", string(format!("makepad-skin-{node_index}"))),
            ("inverseBindMatrices", number(inverse_bind_accessor)),
            ("skeleton", number(joint_node_base + root_joint)),
            ("joints", joint_node_indices.clone()),
        ]));
        let node_json = as_object_mut(&mut nodes_json[node_index], &format!("nodes[{node_index}]"))?;
        node_json.insert("skin".to_string(), number(skin_index));
    }

    let mut scenes_json = rewrite.take_array("scenes")?;
    let root_node = number(joint_node_base + root_joint);
    if scenes_json.is_empty() {
        let mut parented = vec![false; original_node_count];
        for node in rewrite.document.nodes_slice() {
            for &child in node.children.as_deref().unwrap_or(&[]) {
                parented[child] = true;
            }
        }
        let mut roots = (0..original_node_count)
            .filter(|node| !parented[*node])
            .map(number)
            .collect::<Vec<_>>();
        roots.push(root_node.clone());
        scenes_json.push(object([("nodes", JsonValue::Array(roots))]));
        rewrite.insert("scene", number(0));
    } else {
        for (scene_index, scene) in scenes_json.iter_mut().enumerate() {
            let scene = as_object_mut(scene, &format!("scenes[{scene_index}]"))?;
            let nodes = scene
                .entry("nodes".to_string())
                .or_insert_with(|| JsonValue::Array(Vec::new()));
            let nodes = as_array_mut(nodes, &format!("scenes[{scene_index}].nodes"))?;
            // The joint nodes were appended above, so no source scene can
            // already reference this newly allocated index.
            nodes.push(root_node.clone());
        }
    }

    rewrite.put_array("bufferViews", views);
    rewrite.put_array("accessors", accessors);
    rewrite.put_array("meshes", meshes_json);
    rewrite.put_array("nodes", nodes_json);
    rewrite.put_array("skins", skins_json);
    rewrite.put_array("scenes", scenes_json);
    rewrite.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_glb_bytes, replace_glb_node_animations, GlbAnimPath, GlbNodeAnimChannel, GlbNodeAnimClip};

    fn fixture() -> Vec<u8> {
        let positions = [
            0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0,
        ];
        let mut bin = f32_bytes(positions);
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0","generator":"oracle"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"name":"mesh","mesh":0,"translation":[5,0,0],"extras":{{"opaque":7}}}}],"meshes":[{{"name":"body","extensions":{{"VENDOR_mesh":{{"keep":true}}}},"primitives":[{{"attributes":{{"POSITION":0}},"material":0}},{{"attributes":{{"POSITION":1}},"material":1}}]}}],"materials":[{{"name":"red","extras":{{"keep":"a"}}}},{{"name":"blue"}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},{{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36,"target":34962}},{{"buffer":0,"byteOffset":36,"byteLength":36,"target":34962}}],"buffers":[{{"byteLength":{}}}],"extensions":{{"VENDOR_root":{{"answer":42}}}}}}"#,
            bin.len()
        );
        let mut json = json.into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut glb = Vec::with_capacity(total);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        glb.extend_from_slice(&json);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        glb.extend_from_slice(&bin);
        glb
    }

    fn joints() -> Vec<GlbSkinJoint> {
        vec![
            GlbSkinJoint { name: "root".into(), parent: None, global_translation: [5.0, 0.0, 0.0] },
            GlbSkinJoint { name: "child".into(), parent: Some(0), global_translation: [5.0, 1.0, 0.0] },
        ]
    }

    fn primitive_skins() -> Vec<GlbPrimitiveSkin> {
        vec![
            GlbPrimitiveSkin {
                node: 0,
                primitive: 0,
                weights: vec![1.0, 0.0, 0.25, 0.75, 0.5, 0.5],
            },
            GlbPrimitiveSkin {
                node: 0,
                primitive: 1,
                weights: vec![0.0, 1.0, 0.6, 0.4, 0.1, 0.9],
            },
        ]
    }

    #[test]
    fn preserves_original_bytes_and_opaque_structure() {
        let input = fixture();
        let input_parsed = parse_glb_bytes(&input).unwrap();
        let output = augment_glb_skin(&input, &joints(), &primitive_skins()).unwrap();
        let parsed = parse_glb_bytes(&output).unwrap();
        let old_bin = input_parsed.bin_chunk.unwrap();
        assert_eq!(&parsed.bin_chunk.as_ref().unwrap()[..old_bin.len()], old_bin);
        assert_eq!(parsed.document.nodes_slice().len(), 3);
        assert_eq!(parsed.document.skins.as_ref().unwrap().len(), 1);
        for primitive in &parsed.document.meshes_slice()[0].primitives {
            assert!(primitive.attributes.contains_key("JOINTS_0"));
            assert!(primitive.attributes.contains_key("WEIGHTS_0"));
            assert!(primitive.material.is_some());
        }
        let json_len = u32::from_le_bytes(output[12..16].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&output[20..20 + json_len]).unwrap();
        assert!(json.contains("VENDOR_root"));
        assert!(json.contains("VENDOR_mesh"));
        assert!(json.contains("opaque"));
    }

    #[test]
    fn output_is_deterministic_and_animation_ready() {
        let input = fixture();
        let make = || augment_glb_skin(&input, &joints(), &primitive_skins()).unwrap();
        let first = make();
        assert_eq!(first, make());
        let animated = replace_glb_node_animations(
            &first,
            &[GlbNodeAnimClip {
                name: "walk".into(),
                channels: vec![GlbNodeAnimChannel {
                    node: 2,
                    path: GlbAnimPath::Rotation,
                    times: vec![0.0, 1.0],
                    values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.70710677, 0.70710677],
                }],
            }],
        )
        .unwrap();
        assert_eq!(parse_glb_bytes(&animated).unwrap().document.animations.unwrap().len(), 1);
    }

    #[test]
    fn rejects_partial_primitive_mapping_and_zero_rows() {
        let input = fixture();
        let partial = vec![primitive_skins().remove(0)];
        assert!(augment_glb_skin(&input, &joints(), &partial)
            .unwrap_err()
            .to_string()
            .contains("all 2 mesh primitives"));
        let mut bad = primitive_skins();
        bad[0].weights[..2].fill(0.0);
        assert!(augment_glb_skin(&input, &joints(), &bad)
            .unwrap_err()
            .to_string()
            .contains("zero top-four sum"));
    }

    #[test]
    fn preserves_real_textured_trellis_asset_if_available() {
        let path = std::path::Path::new(
            "local/character_verify/native_mario_seed424242_oraclecontract_20k.glb",
        );
        let Ok(input) = std::fs::read(path) else {
            return;
        };
        let before = parse_glb_bytes(&input).unwrap();
        let mut mappings = Vec::new();
        for (node_index, node) in before.document.nodes_slice().iter().enumerate() {
            let Some(mesh_index) = node.mesh else {
                continue;
            };
            for (primitive_index, primitive) in before.document.meshes_slice()[mesh_index]
                .primitives
                .iter()
                .enumerate()
            {
                let position = primitive.attributes["POSITION"];
                let vertex_count = before.document.accessors_slice()[position].count;
                mappings.push(GlbPrimitiveSkin {
                    node: node_index,
                    primitive: primitive_index,
                    weights: vec![1.0; vertex_count],
                });
            }
        }
        let output = augment_glb_skin(
            &input,
            &[GlbSkinJoint {
                name: "root".into(),
                parent: None,
                global_translation: [0.0; 3],
            }],
            &mappings,
        )
        .unwrap();
        let after = parse_glb_bytes(&output).unwrap();
        let old_bin = before.bin_chunk.unwrap();
        assert_eq!(&after.bin_chunk.as_ref().unwrap()[..old_bin.len()], old_bin);
        assert_eq!(after.document.materials_slice().len(), before.document.materials_slice().len());
        assert_eq!(after.document.textures_slice().len(), before.document.textures_slice().len());
        assert_eq!(after.document.images_slice().len(), before.document.images_slice().len());
        assert_eq!(after.document.meshes_slice().len(), before.document.meshes_slice().len());
    }
}
