//! GLB import into editable objects. Each mesh node becomes one object; static
//! meshes have their world transform baked in, skinned meshes stay in their
//! bind space. Coincident positions (with equal skin weights) are welded, so
//! imported shells have real topology while corner UVs and normals keep their
//! seams. Materials keep base colour, embedded PNG base colour texture,
//! metallic/roughness, emission and alpha mode.
//!
//! A skinned GLB also brings its skeleton (bind pose as rest, a synthetic
//! `root` when the skin has several roots), vertex weights and every
//! animation as an editable clip. Static meshes parented under a joint node
//! are bound rigidly to that joint, so props on a rig follow it.
use crate::{json::{self, Value}, mesh, transform::*, AnimationChannel, AnimationClip, AnimationPath, Error, Joint, Keyframe, Limits, Material, Operation, Result, Skeleton};
use makepad_gltf::{decode_mesh_primitive, load_gltf_from_bytes, load_image_bytes, GltfNode, JsonValue, LoadedGltf};
use std::collections::{BTreeMap, HashMap};

pub struct ImportedGlb {
    pub operations: Vec<Operation>,
    pub objects: Vec<String>,
    /// Imported bounds in model metres, after node transforms.
    pub bounds: [[f64; 3]; 2],
    pub warnings: Vec<String>,
    /// Joint names in document order when the GLB was skinned.
    pub joints: Vec<String>,
}

fn node_matrix(node: &GltfNode) -> Matrix4 {
    if let Some(m) = node.matrix {
        // glTF matrices are column-major; ours are row-major.
        return std::array::from_fn(|r| std::array::from_fn(|c| m[c * 4 + r] as f64));
    }
    trs_matrix(node.translation.unwrap_or([0.; 3]).map(|v| v as f64), node.rotation.unwrap_or([0., 0., 0., 1.]).map(|v| v as f64), node.scale.unwrap_or([1.; 3]).map(|v| v as f64))
}
fn trs_matrix(t: [f64; 3], r: [f64; 4], s: [f64; 3]) -> Matrix4 {
    let [x, y, z, w] = r;
    let rot = [[1. - 2. * (y * y + z * z), 2. * (x * y - z * w), 2. * (x * z + y * w)],
        [2. * (x * y + z * w), 1. - 2. * (x * x + z * z), 2. * (y * z - x * w)],
        [2. * (x * z - y * w), 2. * (y * z + x * w), 1. - 2. * (x * x + y * y)]];
    let mut m = IDENTITY_MATRIX;
    for i in 0..3 { for j in 0..3 { m[i][j] = rot[i][j] * s[j]; } m[i][3] = t[i]; }
    m
}
/// Translation, rotation and (per-axis) scale of an affine matrix without shear.
fn decompose(m: Matrix4) -> ([f64; 3], [f64; 4], [f64; 3]) {
    let t = [m[0][3], m[1][3], m[2][3]];
    let col = |j: usize| [m[0][j], m[1][j], m[2][j]];
    let mut s = [length(col(0)), length(col(1)), length(col(2))];
    let det = dot(col(0), cross(col(1), col(2)));
    if det < 0. { s[0] = -s[0]; }
    let r: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| m[i][j] / if s[j].abs() > 1e-12 { s[j] } else { 1. }));
    let trace = r[0][0] + r[1][1] + r[2][2];
    let q = if trace > 0. {
        let k = (trace + 1.).sqrt() * 2.;
        [(r[2][1] - r[1][2]) / k, (r[0][2] - r[2][0]) / k, (r[1][0] - r[0][1]) / k, 0.25 * k]
    } else if r[0][0] > r[1][1] && r[0][0] > r[2][2] {
        let k = (1. + r[0][0] - r[1][1] - r[2][2]).sqrt() * 2.;
        [0.25 * k, (r[0][1] + r[1][0]) / k, (r[0][2] + r[2][0]) / k, (r[2][1] - r[1][2]) / k]
    } else if r[1][1] > r[2][2] {
        let k = (1. + r[1][1] - r[0][0] - r[2][2]).sqrt() * 2.;
        [(r[0][1] + r[1][0]) / k, 0.25 * k, (r[1][2] + r[2][1]) / k, (r[0][2] - r[2][0]) / k]
    } else {
        let k = (1. + r[2][2] - r[0][0] - r[1][1]).sqrt() * 2.;
        [(r[0][2] + r[2][0]) / k, (r[1][2] + r[2][1]) / k, 0.25 * k, (r[1][0] - r[0][1]) / k]
    };
    (t, quat_normalize(q).unwrap_or([0., 0., 0., 1.]), s)
}
fn clean_name(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' }).take(40).collect();
    if s.is_empty() { "part".into() } else { s }
}
fn num(v: &JsonValue) -> Option<f64> { match v { JsonValue::U64(n) => Some(*n as f64), JsonValue::I64(n) => Some(*n as f64), JsonValue::F64(n) => Some(*n), _ => None } }
fn index(v: Option<&JsonValue>) -> Option<usize> { v.and_then(num).map(|n| n as usize) }
fn array(v: Option<&JsonValue>) -> Vec<JsonValue> { match v { Some(JsonValue::Array(a)) => a.clone(), _ => Vec::new() } }

/// Any accessor as flat f32 lanes (normalized integers mapped to 0..1).
fn read_accessor(loaded: &LoadedGltf, accessor: usize) -> Result<(Vec<f32>, usize)> {
    let doc = &loaded.document;
    let a = doc.accessors_slice().get(accessor).ok_or(Error::Invalid("import_glb: accessor index"))?;
    let comps = match a.accessor_type.as_str() { "SCALAR" => 1, "VEC2" => 2, "VEC3" => 3, "VEC4" => 4, "MAT4" => 16, _ => return Err(Error::Invalid("import_glb: accessor type")) };
    let size = match a.component_type { 5120 | 5121 => 1, 5122 | 5123 => 2, 5125 | 5126 => 4, _ => return Err(Error::Invalid("import_glb: component type")) };
    let view = doc.buffer_views.as_deref().and_then(|v| v.get(a.buffer_view?)).ok_or(Error::Invalid("import_glb: accessor without buffer view"))?;
    let buffer = loaded.buffers.get(view.buffer).ok_or(Error::Invalid("import_glb: buffer index"))?;
    let stride = view.byte_stride.unwrap_or(comps * size);
    let base = view.byte_offset.unwrap_or(0) + a.byte_offset.unwrap_or(0);
    let normalized = a.normalized.unwrap_or(false);
    let mut out = Vec::with_capacity(a.count * comps);
    for i in 0..a.count {
        for c in 0..comps {
            let at = base + i * stride + c * size;
            let b = buffer.get(at..at + size).ok_or(Error::Invalid("import_glb: accessor out of bounds"))?;
            let v = match a.component_type {
                5126 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                5121 => if normalized { b[0] as f32 / 255. } else { b[0] as f32 },
                5123 => { let x = u16::from_le_bytes([b[0], b[1]]); if normalized { x as f32 / 65535. } else { x as f32 } }
                5125 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
                5120 => { let x = b[0] as i8 as f32; if normalized { (x / 127.).max(-1.) } else { x } }
                _ => { let x = i16::from_le_bytes([b[0], b[1]]) as f32; if normalized { (x / 32767.).max(-1.) } else { x } }
            };
            out.push(v);
        }
    }
    Ok((out, comps))
}

struct Rig { ordinal: HashMap<usize, u32>, skeleton: Skeleton, rests: Vec<Matrix4>, root_parent: HashMap<usize, Matrix4> }

fn build_rig(loaded: &LoadedGltf, root: Matrix4, parents: &[Option<usize>], worlds: &[Matrix4], limits: &Limits, warnings: &mut Vec<String>) -> Result<Option<Rig>> {
    let skins = loaded.document.skins.as_deref().unwrap_or(&[]);
    let Some(skin) = skins.first() else { return Ok(None) };
    if skins.len() > 1 { warnings.push("import_glb: only the first skin is imported".into()); }
    let joints: Vec<usize> = array(skin.key("joints")).iter().filter_map(num).map(|n| n as usize).collect();
    if joints.is_empty() { return Ok(None); }
    let ibm = match index(skin.key("inverseBindMatrices")) { Some(a) => Some(read_accessor(loaded, a)?.0), None => None };
    // Bind world of each joint (in mesh space) = inverse(IBM), else its node world.
    let mut bind: HashMap<usize, Matrix4> = HashMap::new();
    for (k, &node) in joints.iter().enumerate() {
        let w = match &ibm {
            Some(m) if m.len() >= (k + 1) * 16 => inverse(std::array::from_fn(|r| std::array::from_fn(|c| m[k * 16 + c * 4 + r] as f64))).unwrap_or(worlds[node]),
            _ => worlds[node],
        };
        bind.insert(node, matrix_mul(root, w));
    }
    let is_joint = |n: usize| bind.contains_key(&n);
    let joint_parent = |mut n: usize| -> Option<usize> { while let Some(p) = parents[n] { if is_joint(p) { return Some(p); } n = p; } None };
    // Parents first; several roots get a synthetic root at the origin.
    let roots: Vec<usize> = joints.iter().copied().filter(|&j| joint_parent(j).is_none()).collect();
    let synthetic = roots.len() > 1;
    let mut order: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = roots.iter().rev().copied().collect();
    while let Some(n) = stack.pop() {
        order.push(n);
        let mut kids: Vec<usize> = joints.iter().copied().filter(|&j| joint_parent(j) == Some(n)).collect();
        kids.reverse(); stack.extend(kids);
    }
    let offset = synthetic as usize;
    if order.len() + offset > limits.max_joints { return Err(Error::Budget("import_glb joints (raise the document joint limit)")); }
    let ordinal: HashMap<usize, u32> = order.iter().enumerate().map(|(i, n)| (*n, (i + offset) as u32)).collect();
    let nodes = loaded.document.nodes.as_deref().unwrap_or(&[]);
    let mut skeleton = Skeleton { joints: Vec::new() };
    let mut rests = Vec::new(); let mut names: BTreeMap<String, usize> = BTreeMap::new();
    if synthetic { skeleton.joints.push(Joint { name: "root".into(), parent: None, translation: [0.; 3] }); rests.push(IDENTITY_MATRIX); }
    let mut root_parent = HashMap::new();
    for &n in &order {
        let parent = joint_parent(n);
        let pw = parent.map_or(IDENTITY_MATRIX, |p| bind[&p]);
        let local = matrix_mul(inverse(pw)?, bind[&n]);
        // The transform above a root joint (an armature node, the import root)
        // is folded into that joint's clips.
        if parent.is_none() { root_parent.insert(n, matrix_mul(root, parents[n].map_or(IDENTITY_MATRIX, |p| worlds[p]))); }
        let mut name = clean_name(nodes.get(n).and_then(|x| x.name.as_deref()).unwrap_or("joint"));
        let dup = names.entry(name.clone()).or_insert(0); *dup += 1; if *dup > 1 { name = format!("{name}_{dup}"); }
        skeleton.joints.push(Joint { name, parent: parent.map(|p| ordinal[&p]).or(synthetic.then_some(0)), translation: [local[0][3], local[1][3], local[2][3]] });
        rests.push(local);
    }
    Ok(Some(Rig { ordinal, skeleton, rests, root_parent }))
}

fn read_clips(loaded: &LoadedGltf, rig: &Rig, limits: &Limits, warnings: &mut Vec<String>) -> Vec<AnimationClip> {
    let mut clips = Vec::new();
    for (ai, anim) in loaded.document.animations.as_deref().unwrap_or(&[]).iter().enumerate() {
        let samplers = array(anim.key("samplers"));
        let mut channels = Vec::new();
        for ch in array(anim.key("channels")) {
            let Some(target) = ch.key("target") else { continue };
            let Some(node) = index(target.key("node")) else { continue };
            let Some(&joint) = rig.ordinal.get(&node) else { continue };
            let path = match target.key("path").and_then(JsonValue::string).map(String::as_str) { Some("translation") => AnimationPath::Translation, Some("rotation") => AnimationPath::Rotation, Some("scale") => AnimationPath::Scale, _ => continue };
            let Some(sampler) = index(ch.key("sampler")).and_then(|s| samplers.get(s)) else { continue };
            let cubic = sampler.key("interpolation").and_then(JsonValue::string).is_some_and(|s| s == "CUBICSPLINE");
            let (Some(input), Some(output)) = (index(sampler.key("input")), index(sampler.key("output"))) else { continue };
            let (Ok((times, _)), Ok((values, comps))) = (read_accessor(loaded, input), read_accessor(loaded, output)) else { continue };
            let lanes = if path == AnimationPath::Rotation { 4 } else { 3 };
            if comps != lanes { continue; }
            let parent = rig.root_parent.get(&node).copied();
            let (_, pr, ps) = parent.map(decompose).unwrap_or(([0.; 3], [0., 0., 0., 1.], [1.; 3]));
            let mut keys: Vec<Keyframe> = Vec::new();
            for (k, &time) in times.iter().enumerate() {
                let at = if cubic { (k * 3 + 1) * lanes } else { k * lanes };
                let Some(v) = values.get(at..at + lanes) else { break };
                let v: Vec<f64> = v.iter().map(|x| *x as f64).collect();
                let value = match path {
                    AnimationPath::Translation => { let p = [v[0], v[1], v[2]]; let p = if let Some(m) = parent { transform_point(m, p) } else { p }; [p[0], p[1], p[2], 0.] }
                    AnimationPath::Rotation => { let q = quat_normalize([v[0], v[1], v[2], v[3]]).unwrap_or([0., 0., 0., 1.]); if parent.is_some() { quat_normalize(quat_mul(pr, q)).unwrap_or(q) } else { q } }
                    AnimationPath::Scale => [v[0] * ps[0], v[1] * ps[1], v[2] * ps[2], 0.],
                };
                let t = time as f64;
                if keys.last().is_some_and(|l| t <= l.time) { continue; }
                keys.push(Keyframe { time: t, value });
            }
            if keys.len() >= 2 && keys.last().unwrap().time <= limits.max_clip_duration { channels.push(AnimationChannel { joint, path, keys }); }
        }
        if channels.is_empty() { continue; }
        let base = clean_name(anim.key("name").and_then(JsonValue::string).map(String::as_str).unwrap_or("clip")).to_lowercase();
        let name = if clips.iter().any(|c: &AnimationClip| c.name == base) { format!("{base}_{ai}") } else { base };
        clips.push(AnimationClip { name, channels });
    }
    if clips.len() > limits.max_clips { warnings.push(format!("import_glb: kept {} of {} clips", limits.max_clips, clips.len())); clips.truncate(limits.max_clips); }
    clips
}

/// `prefix` names the objects (`<prefix>_<node>`), `material_base` is the
/// first material ordinal the imported materials occupy and `root` places the
/// whole model.
pub fn import_glb(glb: &[u8], prefix: &str, material_base: u32, root: Matrix4, limits: &Limits) -> Result<ImportedGlb> {
    let loaded = load_gltf_from_bytes(glb, None).map_err(|_| Error::Invalid("import_glb: not a readable GLB"))?;
    let doc = &loaded.document;
    let nodes = doc.nodes.as_deref().unwrap_or(&[]);
    let mut warnings = Vec::new();
    let mut parents: Vec<Option<usize>> = vec![None; nodes.len()];
    for (i, n) in nodes.iter().enumerate() { for &c in n.children.as_deref().unwrap_or(&[]) { if c < parents.len() { parents[c] = Some(i); } } }
    // Node worlds in model space (without the import root).
    let mut worlds = vec![IDENTITY_MATRIX; nodes.len()];
    let mut done = vec![false; nodes.len()];
    for start in 0..nodes.len() {
        let mut chain = Vec::new(); let mut at = Some(start);
        while let Some(i) = at { if done[i] || chain.len() > 256 { break; } chain.push(i); at = parents[i]; }
        while let Some(i) = chain.pop() { worlds[i] = matrix_mul(parents[i].map_or(IDENTITY_MATRIX, |p| worlds[p]), node_matrix(&nodes[i])); done[i] = true; }
    }
    let rig = build_rig(&loaded, root, &parents, &worlds, limits, &mut warnings)?;
    let skin_joints: Vec<usize> = doc.skins.as_deref().and_then(|s| s.first()).map(|s| array(s.key("joints")).iter().filter_map(num).map(|n| n as usize).collect()).unwrap_or_default();
    let mut operations = Vec::new(); let mut objects = Vec::new();
    let mut used_materials: BTreeMap<Option<usize>, u32> = BTreeMap::new();
    let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    let mut names: HashMap<String, usize> = HashMap::new();
    for (node_index, node) in nodes.iter().enumerate() {
        let Some(mesh_index) = node.mesh else { continue };
        let skinned = node.skin.is_some() && rig.is_some();
        let world = if skinned { root } else { matrix_mul(root, worlds[node_index]) };
        // A static mesh under a joint node rides that joint rigidly.
        let rigid_joint = rig.as_ref().and_then(|r| { let mut at = Some(node_index); while let Some(i) = at { if let Some(o) = r.ordinal.get(&i) { return Some(*o); } at = parents[i]; } Some(0) });
        let primitive_count = doc.meshes.as_deref().and_then(|m| m.get(mesh_index)).map_or(0, |m| m.primitives.len());
        let normal_matrix = inverse(world).map(|inv| std::array::from_fn::<_, 3, _>(|r| std::array::from_fn::<_, 3, _>(|c| inv[c][r]))).unwrap_or([[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]);
        let mirrored = { let m = world; m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]) < 0. };
        let mut positions: Vec<[f64; 3]> = Vec::new(); let mut vweights: Vec<Vec<mesh::JointWeight>> = Vec::new();
        let mut weld: HashMap<(Vec<u64>, Vec<(u32, u32)>), u32> = HashMap::new();
        let mut polygons: Vec<mesh::Polygon> = Vec::new(); let mut corner_normals: Vec<Option<[f64; 3]>> = Vec::new();
        for p in 0..primitive_count {
            let prim = match decode_mesh_primitive(&loaded, mesh_index, p) { Ok(p) => p, Err(_) => { warnings.push(format!("import_glb: skipped a non-triangle primitive of mesh {mesh_index}")); continue; } };
            let attrs = &doc.meshes.as_deref().unwrap()[mesh_index].primitives[p].attributes;
            let skin_data = if skinned {
                match (attrs.get("JOINTS_0"), attrs.get("WEIGHTS_0")) { (Some(&j), Some(&w)) => Some((read_accessor(&loaded, j)?.0, read_accessor(&loaded, w)?.0)), _ => None }
            } else { None };
            let next = material_base + used_materials.len() as u32;
            let material = *used_materials.entry(prim.material).or_insert(next);
            let mut ids = Vec::with_capacity(prim.positions.len());
            for (i, q) in prim.positions.iter().enumerate() {
                let w = transform_point(world, q.map(|v| v as f64));
                let weights: Vec<mesh::JointWeight> = match (&skin_data, &rig) {
                    (Some((j, wt)), Some(r)) => {
                        let mut list: Vec<mesh::JointWeight> = (0..4).filter_map(|k| {
                            let weight = *wt.get(i * 4 + k)? as f64; if weight <= 1e-6 { return None; }
                            let node = *skin_joints.get(*j.get(i * 4 + k)? as usize)?;
                            Some(mesh::JointWeight { joint: *r.ordinal.get(&node)?, weight })
                        }).collect();
                        list.sort_by_key(|x| x.joint); list.dedup_by(|a, b| { if a.joint == b.joint { b.weight += a.weight; true } else { false } });
                        let sum: f64 = list.iter().map(|x| x.weight).sum();
                        if sum <= 0. { vec![mesh::JointWeight { joint: rigid_joint.unwrap_or(0), weight: 1. }] } else { list.into_iter().map(|x| mesh::JointWeight { weight: x.weight / sum, ..x }).collect() }
                    }
                    _ => rigid_joint.map(|j| vec![mesh::JointWeight { joint: j, weight: 1. }]).unwrap_or_default(),
                };
                let key = (w.iter().map(|v| (*v as f32).to_bits() as u64).collect::<Vec<_>>(), weights.iter().map(|x| (x.joint, (x.weight as f32).to_bits())).collect::<Vec<_>>());
                let id = *weld.entry(key).or_insert_with(|| { positions.push(w); vweights.push(weights.clone()); (positions.len() - 1) as u32 });
                ids.push(id);
            }
            for t in prim.indices.chunks_exact(3) {
                let mut t = [t[0] as usize, t[1] as usize, t[2] as usize];
                if t.iter().any(|i| *i >= ids.len()) { return Err(Error::Invalid("import_glb: index out of range")); }
                if mirrored { t.swap(1, 2); }
                let v = t.map(|i| ids[i]);
                if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] { continue; }
                let (a, b, c) = (positions[v[0] as usize], positions[v[1] as usize], positions[v[2] as usize]);
                if length(cross(sub(b, a), sub(c, a))) < 1e-12 { continue; }
                let uvs = prim.texcoords0.as_ref().map(|uv| t.iter().map(|&i| uv[i].map(|v| v as f64)).collect()).unwrap_or_default();
                polygons.push(mesh::Polygon { vertices: v.to_vec(), uvs, material });
                for &i in &t {
                    corner_normals.push(prim.normals.as_ref().and_then(|n| {
                        let n = n[i].map(|v| v as f64);
                        normalized(std::array::from_fn(|r| (0..3).map(|c| normal_matrix[r][c] * n[c]).sum())).ok()
                    }));
                }
            }
        }
        if polygons.is_empty() { continue; }
        let mut ctx = mesh::Context::new(limits.mesh.clone(), None);
        let mut m = mesh::Mesh::from_weighted_polygons(&positions, &vweights, &polygons, &mut ctx)?;
        let normals: Vec<_> = m.corners().iter().zip(&corner_normals).filter_map(|(c, n)| n.map(|n| (c.id, Some(n)))).collect();
        if !normals.is_empty() { m.set_corner_normals_bulk(&normals, &mut ctx)?; }
        for p in &positions { for d in 0..3 { bounds[0][d] = bounds[0][d].min(p[d]); bounds[1][d] = bounds[1][d].max(p[d]); } }
        let base = format!("{prefix}_{}", clean_name(node.name.as_deref().unwrap_or("part")));
        let n = names.entry(base.clone()).or_insert(0); *n += 1;
        let object = if *n == 1 { base } else { format!("{base}_{n}") };
        if object.len() > limits.max_name_bytes { return Err(Error::Invalid("import_glb: object name too long; use a shorter prefix")); }
        operations.push(Operation::ImportMesh { object: object.clone(), source: m.to_bytes(&mut ctx)? });
        objects.push(object);
    }
    if objects.is_empty() { return Err(Error::Invalid("import_glb: the model has no triangle meshes")); }
    let materials = doc.materials.as_deref().unwrap_or(&[]);
    // Skeleton and rests first, then materials, then meshes that name both.
    let meshes = std::mem::take(&mut operations);
    let mut joint_names = Vec::new();
    if let Some(r) = &rig {
        operations.push(Operation::SetSkeleton { skeleton: r.skeleton.clone() });
        for (j, rest) in r.rests.iter().enumerate() {
            let (t, q, s) = decompose(*rest);
            let v = json::obj(vec![("op", json::s("rig_rest")), ("joint", Value::Int(j as i64)), ("transform", json::obj(vec![
                ("translation", Value::Arr(t.iter().map(|x| Value::F64(*x)).collect())), ("rotation", Value::Arr(q.iter().map(|x| Value::F64(*x)).collect())), ("scale", Value::Arr(s.iter().map(|x| Value::F64(*x)).collect()))]))]);
            if let Some(op) = crate::RigOperation::parse(&v, limits)? { operations.push(Operation::Rig(op)); }
        }
        joint_names = r.skeleton.joints.iter().map(|j| j.name.clone()).collect();
    }
    for (source, ordinal) in &used_materials {
        if *ordinal as usize >= limits.max_materials { return Err(Error::Budget("import_glb materials")); }
        let gm = source.and_then(|i| materials.get(i));
        let pbr = gm.and_then(|m| m.pbr_metallic_roughness.as_ref());
        let factor = pbr.and_then(|p| p.base_color_factor).unwrap_or([1.; 4]).map(|v| v as f64);
        let png = pbr.and_then(|p| p.base_color_texture.as_ref()).and_then(|t| doc.textures.as_deref()?.get(t.index)?.source)
            .and_then(|image| load_image_bytes(&loaded, image).ok()).filter(|b| b.starts_with(b"\x89PNG"));
        operations.push(Operation::SetMaterial { material: *ordinal, value: Material { color: [factor[0], factor[1], factor[2]], base_color_png: png.unwrap_or_default() } });
        let emissive = gm.and_then(|m| m.emissive_factor).unwrap_or([0.; 3]).map(|v| v as f64);
        let alpha = match gm.and_then(|m| m.alpha_mode.as_deref()) { Some("BLEND") => "blend", Some("MASK") => "mask", _ => "opaque" };
        let surface = json::obj(vec![("op", json::s("surface_material")), ("material", Value::Int(*ordinal as i64)),
            ("base_color", Value::Arr(factor.iter().map(|v| Value::F64(v.clamp(0., 1.))).collect())),
            ("metallic", Value::F64(pbr.and_then(|p| p.metallic_factor).unwrap_or(0.) as f64)), ("roughness", Value::F64(pbr.and_then(|p| p.roughness_factor).unwrap_or(0.8) as f64)),
            ("emissive", Value::Arr(emissive.iter().map(|v| Value::F64(v.clamp(0., 1.))).collect())), ("alpha", json::s(alpha)),
            ("double_sided", Value::Bool(gm.and_then(|m| m.double_sided).unwrap_or(false)))]);
        if let Some(op) = crate::SurfaceOperation::parse(&surface, limits)? { operations.push(Operation::Surface(op)); }
    }
    operations.extend(meshes);
    if let Some(r) = &rig {
        for clip in read_clips(&loaded, r, limits, &mut warnings) { operations.push(Operation::SetClip { clip }); }
    } else if !doc.animations.as_deref().unwrap_or(&[]).is_empty() {
        warnings.push("import_glb: node animations without a skin were not imported".into());
    }
    Ok(ImportedGlb { operations, objects, bounds, warnings, joints: joint_names })
}
