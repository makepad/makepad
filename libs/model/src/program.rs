//! Whole-model programs: an ordered list of operation values, as written by
//! a front end such as the Splash `game.model_new` builder, applied to a fresh
//! document and compiled once. A program is pure data, so the same list always
//! yields the same model and can be cached by its content hash. Besides every
//! ordinary `model.apply` operation, programs accept a few macro operations:
//!
//! - `humanoid_rig {height, clips?, sockets?}`: the humanoid template skeleton,
//!   procedural `idle`/`walk`/`run` clips and `hand.r`/`hand.l` sockets.
//! - `character {...}`: a whole generated character (see `character.rs`):
//!   skinned body, face rig, hair, clothing, gear, sockets and clip set.
//! - `bind {object, joint}`: rigidly binds an object to a joint by name.
//! - `bind_all {}`: rigidly binds every still-unweighted object to the joint
//!   whose bone passes closest to the object's centre.
//! - `import_glb {alias, prefix, material_base?, transform?}`: geometry,
//!   materials, skin and clips of a GLB the host resolves by alias.
//! - `weathering {ao?, distance?, edges?, samples?, objects?}`: bakes ambient
//!   occlusion and convex-edge wear into vertex colours.
#[path = "program_weathering.rs"]
mod weathering;
use crate::{json::{self, Value}, parse_operations, transform::*, CompiledModel, Document, Error, Limits, Operation, Transaction};

#[derive(Clone, Debug, PartialEq)]
pub struct ProgramFailure { pub index: usize, pub op: String, pub message: String }
impl std::fmt::Display for ProgramFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "operation {} ({}): {}", self.index, self.op, self.message) }
}
pub struct BuiltProgram { pub document: Document, pub compiled: CompiledModel, pub warnings: Vec<String> }

/// Readable text for engine errors, for authors rather than engine tests.
pub fn describe_error(error: &Error) -> String {
    match error {
        Error::Invalid(s) | Error::Corrupt(s) => (*s).into(),
        Error::Budget(s) => format!("over budget: {s}"),
        Error::MissingObject(n) => format!("unknown object '{n}'"),
        Error::DuplicateObject(n) => format!("object '{n}' already exists"),
        Error::MissingField { field, .. } => format!("missing field '{field}'"),
        Error::Mesh(e) => format!("{e:?}"),
        other => format!("{other:?}"),
    }
}

const CHUNK: usize = 64;

struct Runner<'a> {
    doc: Document,
    limits: Limits,
    pending: Vec<(usize, Operation)>,
    serial: u64,
    cancelled: Option<&'a dyn Fn() -> bool>,
}
impl Runner<'_> {
    fn flush(&mut self, names: &[String]) -> Result<(), ProgramFailure> {
        while !self.pending.is_empty() {
            let take = self.pending.len().min(CHUNK.min(self.limits.max_operations));
            let batch: Vec<(usize, Operation)> = self.pending.drain(..take).collect();
            if self.apply(batch.iter().map(|(_, op)| op.clone()).collect()).is_err() {
                // Replay one by one to attribute the failure to its operation.
                for (index, op) in batch {
                    if let Err(error) = self.apply(vec![op]) {
                        return Err(ProgramFailure { index, op: names.get(index).cloned().unwrap_or_default(), message: describe_error(&error) });
                    }
                }
            }
            let head = self.doc.head();
            self.doc.checkpoint(head).map_err(|e| ProgramFailure { index: 0, op: "checkpoint".into(), message: describe_error(&e) })?;
        }
        Ok(())
    }
    fn apply(&mut self, operations: Vec<Operation>) -> crate::Result<()> {
        self.serial += 1;
        self.doc.apply(Transaction { request_id: format!("program-{}", self.serial), expected: self.doc.head(), operations }, self.cancelled).map(|_| ())
    }
}

fn joint_world(doc: &Document) -> Vec<(String, Option<usize>, [f64; 3])> {
    let Some(skeleton) = doc.skeleton() else { return Vec::new() };
    let mut out: Vec<(String, Option<usize>, [f64; 3])> = Vec::new();
    for j in &skeleton.joints {
        let parent = j.parent.map(|p| p as usize);
        let base = parent.and_then(|p| out.get(p)).map_or([0.; 3], |p| p.2);
        out.push((j.name.clone(), parent, add(base, j.translation)));
    }
    out
}
fn segment_distance(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = sub(b, a); let l = dot(ab, ab);
    let t = if l <= 1e-12 { 0. } else { (dot(sub(p, a), ab) / l).clamp(0., 1.) };
    length(sub(p, add(a, mul(ab, t))))
}
fn weight_edit(object: &str, joint: usize) -> Value {
    json::obj(vec![("op", json::s("weight_edit")), ("object", json::s(object)), ("vertices", Value::Arr(Vec::new())),
        ("edit", json::obj(vec![("type", json::s("assign")), ("joint", Value::Int(joint as i64))]))])
}
fn transform_matrix(v: &Value) -> Result<Matrix4, String> {
    let get = |k: &str, n: usize, d: &[f64]| -> Result<Vec<f64>, String> {
        match v.get(k) { None => Ok(d.to_vec()), Some(a) => a.as_arr().filter(|a| a.len() == n).ok_or(format!("transform.{k} needs {n} numbers"))?
            .iter().map(|x| match x { Value::F64(f) => Ok(*f), Value::Int(i) => Ok(*i as f64), _ => Err(format!("transform.{k} must be numeric")) }).collect() }
    };
    let (t, r, s) = (get("translation", 3, &[0.; 3])?, get("rotation", 4, &[0., 0., 0., 1.])?, get("scale", 3, &[1.; 3])?);
    Transform { translation: [t[0], t[1], t[2]], rotation: [r[0], r[1], r[2], r[3]], scale: [s[0], s[1], s[2]] }.matrix().map_err(|e| describe_error(&e))
}

/// Builds and compiles a program. `fetch` resolves `import_glb` aliases to
/// GLB bytes (it runs on the calling worker; the host decides where bytes
/// come from). Nothing here blocks on anything but that callback.
pub fn build_program(ops: &[Value], limits: Limits, fetch: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>, cancelled: Option<&dyn Fn() -> bool>) -> Result<BuiltProgram, ProgramFailure> {
    let names: Vec<String> = ops.iter().map(|v| v.get("op").and_then(Value::as_str).unwrap_or("?").to_string()).collect();
    let fail = |index: usize, message: String| ProgramFailure { index, op: names.get(index).cloned().unwrap_or_default(), message };
    let doc = Document::new(limits.clone()).map_err(|e| fail(0, describe_error(&e)))?;
    let mut run = Runner { doc, limits: limits.clone(), pending: Vec::new(), serial: 0, cancelled };
    let mut warnings = Vec::new();
    let mut bound = std::collections::BTreeSet::new();
    let parse_one = |index: usize, v: &Value| -> Result<Vec<Operation>, ProgramFailure> {
        parse_operations(&Value::Arr(vec![v.clone()]), &limits).map_err(|e| fail(index, describe_error(&e)))
    };
    for (index, v) in ops.iter().enumerate() {
        if cancelled.is_some_and(|c| c()) { return Err(fail(index, "cancelled".into())); }
        match names[index].as_str() {
            "humanoid_rig" => {
                let height = v.get("height").and_then(|h| match h { Value::F64(f) => Some(*f), Value::Int(i) => Some(*i as f64), _ => None }).unwrap_or(1.8);
                if !(0.2..=20.).contains(&height) { return Err(fail(index, "humanoid height must be 0.2..20 m".into())); }
                run.pending.push((index, Operation::SetSkeleton { skeleton: crate::templates::humanoid_skeleton(height) }));
                let clips: Vec<String> = match v.get("clips").and_then(Value::as_arr) {
                    Some(list) => list.iter().filter_map(|c| c.as_str().map(str::to_string)).collect(),
                    None => crate::templates::HUMANOID_CLIPS.iter().map(|s| s.to_string()).collect(),
                };
                for clip in clips {
                    let clip = crate::templates::humanoid_clip(&clip, height).ok_or_else(|| fail(index, format!("unknown humanoid clip '{clip}' ({}, {})", crate::templates::HUMANOID_CLIPS.join(", "), crate::templates::HUMANOID_ACTION_CLIPS.join(", "))))?;
                    run.pending.push((index, Operation::SetClip { clip }));
                }
                if v.get("sockets").and_then(Value::as_bool).unwrap_or(true) {
                    let joints = crate::templates::humanoid_joints(height);
                    for (socket, joint) in [("hand.r", "hand_r"), ("hand.l", "hand_l")] {
                        let j = joints.iter().position(|x| x.0 == joint).unwrap();
                        let op = json::obj(vec![("op", json::s("socket")), ("name", json::s(socket)), ("attachment", json::obj(vec![("joint", Value::Int(j as i64))])),
                            ("transform", json::obj(vec![("translation", Value::Arr(vec![Value::F64(0.), Value::F64(-0.045 * height), Value::F64(0.)]))]))]);
                        run.pending.extend(parse_one(index, &op)?.into_iter().map(|o| (index, o)));
                    }
                }
            }
            "character" => {
                if let Some(e) = crate::character::character_spec_error(v) { return Err(fail(index, e)); }
                let built = crate::character::build_character(v, &run.limits).map_err(|e| fail(index, describe_error(&e)))?;
                warnings.extend(built.warnings);
                for op in &built.early { run.pending.extend(parse_one(index, op)?.into_iter().map(|o| (index, o))); }
                run.pending.extend(built.operations.into_iter().map(|o| (index, o)));
                for op in &built.late { run.pending.extend(parse_one(index, op)?.into_iter().map(|o| (index, o))); }
            }
            "fps_arms" => {
                if let Some(e) = crate::character::character_spec_error(v) { return Err(fail(index, e)); }
                let spec = crate::character::CharacterSpec::parse(v).map_err(|e| fail(index, e))?;
                let arms = crate::character::FpsArms::parse(v).map_err(|e| fail(index, e))?;
                let built = crate::character::build_fps_arms(&spec, &arms, &run.limits).map_err(|e| fail(index, describe_error(&e)))?;
                warnings.extend(built.warnings);
                for op in &built.early { run.pending.extend(parse_one(index, op)?.into_iter().map(|o| (index, o))); }
                run.pending.extend(built.operations.into_iter().map(|o| (index, o)));
                for op in &built.late { run.pending.extend(parse_one(index, op)?.into_iter().map(|o| (index, o))); }
            }
            "bind" => {
                run.flush(&names)?;
                let object = v.get("object").and_then(Value::as_str).ok_or_else(|| fail(index, "bind needs object".into()))?.to_string();
                let joint = v.get("joint").and_then(Value::as_str).ok_or_else(|| fail(index, "bind needs a joint name".into()))?;
                let joints = joint_world(&run.doc);
                if joints.is_empty() { return Err(fail(index, "bind needs a skeleton first (humanoid_rig or skeleton)".into())); }
                let j = joints.iter().position(|x| x.0 == joint).ok_or_else(|| fail(index, format!("unknown joint '{joint}'")))?;
                run.pending.extend(parse_one(index, &weight_edit(&object, j))?.into_iter().map(|o| (index, o)));
                bound.insert(object);
            }
            "bind_all" => {
                run.flush(&names)?;
                let joints = joint_world(&run.doc);
                if joints.is_empty() { return Err(fail(index, "bind_all needs a skeleton first".into())); }
                let mut edits = Vec::new();
                for (name, mesh) in run.doc.objects() {
                    if bound.contains(name) || mesh.vertices().iter().any(|v| !v.weights.is_empty()) || mesh.vertices().is_empty() { continue; }
                    let world = run.doc.scene().world_matrix(name).unwrap_or(IDENTITY_MATRIX);
                    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                    for vtx in mesh.vertices() { let p = transform_point(world, vtx.position); for d in 0..3 { lo[d] = lo[d].min(p[d]); hi[d] = hi[d].max(p[d]); } }
                    let c = mul(add(lo, hi), 0.5);
                    // A bone runs from a joint to each child; leaves are points.
                    let best = (0..joints.len()).min_by(|&a, &b| {
                        let d = |i: usize| { let children: Vec<_> = joints.iter().filter(|j| j.1 == Some(i)).collect();
                            if children.is_empty() { length(sub(c, joints[i].2)) } else { children.iter().map(|ch| segment_distance(c, joints[i].2, ch.2)).fold(f64::INFINITY, f64::min) } };
                        d(a).total_cmp(&d(b))
                    }).unwrap();
                    edits.push(weight_edit(name, best));
                }
                for e in edits { run.pending.extend(parse_one(index, &e)?.into_iter().map(|o| (index, o))); }
            }
            "weathering" => {
                run.flush(&names)?;
                let num = |k: &str, d: f64| v.get(k).and_then(|x| match x { Value::F64(f) => Some(*f), Value::Int(i) => Some(*i as f64), _ => None }).unwrap_or(d);
                let objects: Option<Vec<String>> = v.get("objects").and_then(Value::as_arr).map(|a| a.iter().filter_map(|o| o.as_str().map(str::to_string)).collect());
                let (ao, distance, edges, samples) = (num("ao", 0.6).clamp(0., 1.), num("distance", 0.25).clamp(0.005, 10.), num("edges", 0.5).clamp(0., 1.), num("samples", 16.).clamp(1., 64.) as usize);
                for op in weathering::weathering_ops(&run.doc, objects.as_deref(), ao, distance, edges, samples) {
                    run.pending.extend(parse_one(index, &op)?.into_iter().map(|o| (index, o)));
                }
            }
            "import_glb" => {
                let alias = v.get("alias").and_then(Value::as_str).ok_or_else(|| fail(index, "import_glb needs alias".into()))?;
                let prefix = v.get("prefix").and_then(Value::as_str).unwrap_or("import");
                let base = v.get("material_base").and_then(Value::as_u64).unwrap_or(256) as u32;
                let bytes = fetch(alias).map_err(|e| fail(index, format!("import_glb '{alias}': {e}")))?;
                let root = v.get("transform").map(transform_matrix).transpose().map_err(|e| fail(index, e))?;
                let imported = crate::import::import_glb(&bytes, prefix, base, root.unwrap_or(IDENTITY_MATRIX), &run.limits).map_err(|e| fail(index, describe_error(&e)))?;
                warnings.extend(imported.warnings);
                run.pending.extend(imported.operations.into_iter().map(|o| (index, o)));
            }
            _ => { let parsed = parse_one(index, v)?; run.pending.extend(parsed.into_iter().map(|o| (index, o))); }
        }
    }
    run.flush(&names)?;
    let compiled = run.doc.compile(cancelled).map_err(|e| ProgramFailure { index: ops.len(), op: "compile".into(), message: describe_error(&e) })?;
    Ok(BuiltProgram { document: run.doc, compiled, warnings })
}
