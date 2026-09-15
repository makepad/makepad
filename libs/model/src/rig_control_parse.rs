use super::*;
fn named(v: &Value, key: &str, l: &Limits) -> Result<String> {
    let s = text(v, key)?;
    name(s, l)?;
    Ok(s.into())
}
fn bool_or(v: &Value, key: &str, default: bool) -> Result<bool> {
    v.get(key).map(boolean).unwrap_or(Ok(default))
}
fn float_or(v: &Value, key: &str, default: f64) -> Result<f64> {
    v.get(key).map(float).unwrap_or(Ok(default))
}
fn int_or(v: &Value, key: &str, default: u32) -> Result<u32> {
    v.get(key).map(integer).unwrap_or(Ok(default))
}
fn joint(v: &Value, l: &Limits) -> Result<u32> {
    let i = integer(v)?;
    if i as usize >= l.max_joints {
        return Err(Error::Budget("joint index"));
    }
    Ok(i)
}
fn joint_map(v: &Value, l: &Limits) -> Result<BTreeMap<u32, u32>> {
    let mut map = BTreeMap::new();
    for row in rows(v, l.max_joints)? {
        fields(row, &["source", "target"])?;
        if map
            .insert(
                joint(need(row, "source")?, l)?,
                joint(need(row, "target")?, l)?,
            )
            .is_some()
        {
            return Err(Error::Invalid("duplicate source joint"));
        }
    }
    Ok(map)
}
fn joint_set(v: &Value, l: &Limits) -> Result<BTreeSet<u32>> {
    let mut set = BTreeSet::new();
    for value in rows(v, l.max_joints)? {
        if !set.insert(joint(value, l)?) {
            return Err(Error::Invalid("duplicate joint"));
        }
    }
    Ok(set)
}
fn vertices(v: &Value, l: &Limits) -> Result<Vec<mesh::VertexId>> {
    let ids = selections(v, l.mesh.max_vertices)?;
    let mut seen = BTreeSet::new();
    if ids.iter().any(|v| *v == 0 || !seen.insert(*v)) {
        return Err(Error::Invalid("duplicate/zero vertex ID"));
    }
    Ok(ids.into_iter().map(mesh::VertexId).collect())
}
fn unit(v: f64) -> Result<f64> {
    if (0.0..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(Error::Invalid("expected factor in 0..1"))
    }
}
fn positive(v: f64) -> Result<f64> {
    if v > 0. && v.is_finite() {
        Ok(v)
    } else {
        Err(Error::Invalid("expected positive number"))
    }
}
fn fps(v: &Value) -> Result<f64> {
    let value = float_or(v, "fps", 30.)?;
    if (1.0..=120.0).contains(&value) {
        Ok(value)
    } else {
        Err(Error::Invalid("fps must be1..120"))
    }
}
fn constraints(v: &Value, l: &Limits) -> Result<RigConstraint> {
    fields(v, &["op", "name", "joint", "enabled", "kind"])?;
    let k = need(v, "kind")?;
    let kind = match text(k, "type")? {
        "copy" => {
            fields(
                k,
                &[
                    "type",
                    "target",
                    "translation",
                    "rotation",
                    "scale",
                    "influence",
                ],
            )?;
            ConstraintKind::Copy {
                target: joint(need(k, "target")?, l)?,
                translation: bool_or(k, "translation", true)?,
                rotation: bool_or(k, "rotation", true)?,
                scale: bool_or(k, "scale", false)?,
                influence: unit(float_or(k, "influence", 1.)?)?,
            }
        }
        "aim" => {
            fields(k, &["type", "target", "axis", "influence"])?;
            let axis = k.get("axis").map(array).unwrap_or(Ok([0., 1., 0.]))?;
            normalized(axis)?;
            ConstraintKind::Aim {
                target: joint(need(k, "target")?, l)?,
                axis,
                influence: unit(float_or(k, "influence", 1.)?)?,
            }
        }
        "limit" => {
            fields(
                k,
                &[
                    "type",
                    "min_translation",
                    "max_translation",
                    "min_scale",
                    "max_scale",
                    "max_angle",
                ],
            )?;
            ConstraintKind::Limit {
                min_translation: array(need(k, "min_translation")?)?,
                max_translation: array(need(k, "max_translation")?)?,
                min_scale: array(need(k, "min_scale")?)?,
                max_scale: array(need(k, "max_scale")?)?,
                max_angle: float(need(k, "max_angle")?)?,
            }
        }
        "ik" => {
            fields(
                k,
                &["type", "middle", "end", "target", "pole", "clamp_reach"],
            )?;
            ConstraintKind::TwoBoneIk {
                middle: joint(need(k, "middle")?, l)?,
                end: joint(need(k, "end")?, l)?,
                target: joint(need(k, "target")?, l)?,
                pole: array(need(k, "pole")?)?,
                clamp_reach: bool_or(k, "clamp_reach", false)?,
            }
        }
        _ => return Err(Error::Invalid("constraint type")),
    };
    let c = RigConstraint {
        name: named(v, "name", l)?,
        joint: joint(need(v, "joint")?, l)?,
        enabled: bool_or(v, "enabled", true)?,
        kind,
    };
    c.validate(l.max_joints)?;
    Ok(c)
}
pub(super) fn options(v: &Value, l: &Limits) -> Result<ClipOptions> {
    fields(
        v,
        &[
            "op",
            "name",
            "interpolation",
            "root_motion",
            "events",
            "morph_keys",
        ],
    )?;
    let interpolation = match v.get("interpolation") {
        None => Interpolation::Linear,
        Some(Value::Str(s)) => match s.as_str() {
            "linear" => Interpolation::Linear,
            "step" => Interpolation::Step,
            "cubic" => Interpolation::Cubic,
            _ => return Err(Error::Invalid("clip interpolation")),
        },
        _ => return Err(Error::Invalid("clip interpolation")),
    };
    let mut events = Vec::new();
    if let Some(value) = v.get("events") {
        let mut previous = 0.;
        for row in rows(value, 1024)? {
            fields(row, &["time", "name", "payload"])?;
            let time = float(need(row, "time")?)?;
            if time < previous || time > l.max_clip_duration {
                return Err(Error::Invalid("clip event order/time"));
            }
            let payload = match row.get("payload") {
                None => String::new(),
                Some(Value::Str(s)) if s.len() <= 4096 => s.clone(),
                _ => return Err(Error::Invalid("clip event payload")),
            };
            events.push(ClipEvent {
                time,
                name: named(row, "name", l)?,
                payload,
            });
            previous = time;
        }
    }
    let mut morph_keys = BTreeMap::new();
    let mut total = 0usize;
    if let Some(value) = v.get("morph_keys") {
        for row in rows(value, 32)? {
            fields(row, &["morph", "keys"])?;
            let mut keys = Vec::new();
            let mut previous = -1f64;
            for k in rows(need(row, "keys")?, l.max_keyframes.saturating_sub(total))? {
                fields(k, &["time", "weight"])?;
                let time = float(need(k, "time")?)?;
                let weight = float(need(k, "weight")?)?;
                valid_weight(weight)?;
                if time < 0. || time > l.max_clip_duration || time as f32 <= previous as f32 {
                    return Err(Error::Invalid("morph key order/time"));
                }
                keys.push(MorphKey { time, weight });
                previous = time;
                total += 1;
            }
            if keys.is_empty() {
                return Err(Error::Invalid("empty morph keys"));
            }
            if morph_keys.insert(named(row, "morph", l)?, keys).is_some() {
                return Err(Error::Invalid("duplicate morph animation"));
            }
        }
    }
    Ok(ClipOptions {
        interpolation,
        root_motion: bool_or(v, "root_motion", false)?,
        events,
        morph_keys,
    })
}
impl RigOperation {
    pub fn parse(v: &Value, l: &Limits) -> Result<Option<Self>> {
        let op = text(v, "op")?;
        use RigOperation::*;
        let result = match op {
            "rig_mirror" => {
                fields(v,&["op","joints","axis","names"])?;
                let axis=integer(need(v,"axis")?)? as usize;
                if axis>2{return Err(Error::Invalid("mirror axis"));}
                let joints=joint_set(need(v,"joints")?,l)?.into_iter().collect::<Vec<_>>();
                let mut names=BTreeMap::new();for row in rows(need(v,"names")?,l.max_joints)?{fields(row,&["joint","name"])?;if names.insert(joint(need(row,"joint")?,l)?,named(row,"name",l)?).is_some(){return Err(Error::Invalid("duplicate mirror name joint"));}}
                if joints.is_empty()||names.keys().copied().collect::<Vec<_>>()!=joints{return Err(Error::Invalid("mirror requires a name for every selected joint"));}
                MirrorJoints{joints,axis,names}
            },

            "rig_rest" => {
                fields(v, &["op", "joint", "transform"])?;
                Rest {
                    joint: joint(need(v, "joint")?, l)?,
                    transform: transform(need(v, "transform")?)?,
                }
            }
            "rig_rename" => {
                fields(v, &["op", "joint", "name"])?;
                RenameJoint {
                    joint: joint(need(v, "joint")?, l)?,
                    name: named(v, "name", l)?,
                }
            }
            "rig_parent" => {
                fields(v, &["op", "joint", "parent"])?;
                ReparentJoint {
                    joint: joint(need(v, "joint")?, l)?,
                    parent: match need(v, "parent")? {
                        Value::Null => None,
                        p => Some(joint(p, l)?),
                    },
                }
            }
            "rig_roll" => {
                fields(v, &["op", "joint", "angle"])?;
                Roll {
                    joint: joint(need(v, "joint")?, l)?,
                    angle: float(need(v, "angle")?)?,
                }
            }
            "pose" => {
                fields(v, &["op", "name", "joints"])?;
                let mut joints = BTreeMap::new();
                for row in rows(need(v, "joints")?, l.max_joints)? {
                    fields(row, &["joint", "transform"])?;
                    if joints
                        .insert(
                            joint(need(row, "joint")?, l)?,
                            transform(need(row, "transform")?)?,
                        )
                        .is_some()
                    {
                        return Err(Error::Invalid("duplicate pose joint"));
                    }
                }
                Pose(super::Pose {
                    name: named(v, "name", l)?,
                    joints,
                })
            }
            "delete_pose" => {
                fields(v, &["op", "name"])?;
                DeletePose {
                    name: named(v, "name", l)?,
                }
            }
            "constraint" => Constraint(constraints(v, l)?),
            "delete_constraint" => {
                fields(v, &["op", "name"])?;
                DeleteConstraint {
                    name: named(v, "name", l)?,
                }
            }
            "solve_pose" => {
                fields(v, &["op", "pose"])?;
                SolvePose {
                    pose: named(v, "pose", l)?,
                }
            }
            "ik" => {
                fields(
                    v,
                    &[
                        "op",
                        "pose",
                        "root",
                        "middle",
                        "end",
                        "target",
                        "pole",
                        "clamp_reach",
                    ],
                )?;
                Ik {
                    pose: named(v, "pose", l)?,
                    root: joint(need(v, "root")?, l)?,
                    middle: joint(need(v, "middle")?, l)?,
                    end: joint(need(v, "end")?, l)?,
                    target: array(need(v, "target")?)?,
                    pole: array(need(v, "pole")?)?,
                    clamp_reach: bool_or(v, "clamp_reach", false)?,
                }
            }
            "bake_clip" => {
                fields(v, &["op", "name", "poses", "fps", "solve_constraints"])?;
                let mut poses = Vec::new();
                for row in rows(need(v, "poses")?, l.max_keyframes)? {
                    fields(row, &["time", "pose"])?;
                    poses.push((float(need(row, "time")?)?, named(row, "pose", l)?));
                }
                let name = named(v, "name", l)?;
                let fps = fps(v)?;
                name_check_keys(&name, &poses, fps, l)?;
                BakeClip {
                    name,
                    poses,
                    fps,
                    solve_constraints: bool_or(v, "solve_constraints", true)?,
                }
            }
            "blend_clip" => {
                fields(v, &["op", "name", "a", "b", "weight", "fps"])?;
                BlendClip {
                    name: named(v, "name", l)?,
                    a: named(v, "a", l)?,
                    b: named(v, "b", l)?,
                    weight: unit(float(need(v, "weight")?)?)?,
                    fps: fps(v)?,
                }
            }
            "retarget_clip" => {
                fields(v, &["op", "name", "source", "joints", "translation_scale"])?;
                RetargetClip {
                    name: named(v, "name", l)?,
                    source: named(v, "source", l)?,
                    joints: joint_map(need(v, "joints")?, l)?,
                    translation_scale: positive(float_or(v, "translation_scale", 1.)?)?,
                }
            }
            "clip_options" => ClipOptions {
                name: named(v, "name", l)?,
                options: options(v, l)?,
            },
            "weight_locks" => {
                fields(v, &["op", "object", "joints"])?;
                WeightLocks {
                    object: named(v, "object", l)?,
                    joints: joint_set(need(v, "joints")?, l)?,
                }
            }
            "weight_edit" => {
                fields(v, &["op", "object", "vertices", "edit"])?;
                let value = need(v, "edit")?;
                let edit = match text(value, "type")? {
                    "assign" => {
                        fields(value, &["type", "joint"])?;
                        let joint=integer(need(value,"joint")?)?;
                        if joint as usize>=l.max_joints{return Err(Error::Budget("weight assignment joint"));}
                        WeightEdit::Assign{joint}
                    }
                    "normalize" => {
                        fields(value, &["type", "max_influences"])?;
                        let max = int_or(value, "max_influences", 4)? as usize;
                        if max == 0 || max > l.mesh.max_weights_per_vertex {
                            return Err(Error::Budget("weight influences"));
                        }
                        WeightEdit::Normalize {
                            max_influences: max,
                        }
                    }
                    "smooth" => {
                        fields(value, &["type", "iterations", "factor"])?;
                        let iterations = int_or(value, "iterations", 1)?;
                        if iterations == 0 || iterations > 64 {
                            return Err(Error::Budget("weight smooth iterations"));
                        }
                        WeightEdit::Smooth {
                            iterations,
                            factor: unit(float_or(value, "factor", 0.5)?)?,
                        }
                    }
                    "mirror" => {
                        fields(value, &["type", "axis", "joints", "tolerance"])?;
                        let axis = integer(need(value, "axis")?)? as usize;
                        if axis > 2 {
                            return Err(Error::Invalid("mirror axis"));
                        }
                        WeightEdit::Mirror {
                            axis,
                            joints: joint_map(need(value, "joints")?, l)?,
                            tolerance: positive(float_or(value, "tolerance", 0.001)?)?,
                        }
                    }
                    "transfer" => {
                        fields(value, &["type", "source", "max_distance"])?;
                        WeightEdit::Transfer {
                            source: named(value, "source", l)?,
                            max_distance: positive(float(need(value, "max_distance")?)?)?,
                        }
                    }
                    "bind" => {
                        fields(value, &["type", "max_influences", "power"])?;
                        let max = int_or(value, "max_influences", 4)? as usize;
                        if max == 0 || max > l.mesh.max_weights_per_vertex {
                            return Err(Error::Budget("weight influences"));
                        }
                        WeightEdit::Bind {
                            max_influences: max,
                            power: {let p=positive(float_or(value, "power", 2.)?)?;if p>8.{return Err(Error::Invalid("binding power must be at most8"));}p},
                        }
                    }
                    _ => return Err(Error::Invalid("weight edit type")),
                };
                Weights {
                    object: named(v, "object", l)?,
                    vertices: vertices(need(v, "vertices")?, l)?,
                    edit,
                }
            }
            "morph" => {
                fields(v, &["op", "name", "object", "deltas", "weight"])?;
                let mut deltas = BTreeMap::new();
                for row in rows(need(v, "deltas")?, l.mesh.max_vertices)? {
                    fields(row, &["vertex", "delta"])?;
                    let id = stable_id(need(row, "vertex")?)?;
                    let delta: [f64; 3] = array(need(row, "delta")?)?;
                    if id == 0
                        || delta.iter().any(|v| !(*v as f32).is_finite())
                        || deltas.insert(mesh::VertexId(id), delta).is_some()
                    {
                        return Err(Error::Invalid("morph vertex/delta"));
                    }
                }
                let weight = float_or(v, "weight", 0.)?;
                valid_weight(weight)?;
                Morph {
                    name: named(v, "name", l)?,
                    object: named(v, "object", l)?,
                    deltas,
                    weight,
                }
            }
            "capture_morph" => {
                fields(v, &["op", "name", "object", "target", "weight"])?;
                let weight = float_or(v, "weight", 0.)?;
                valid_weight(weight)?;
                CaptureMorph {
                    name: named(v, "name", l)?,
                    object: named(v, "object", l)?,
                    target: named(v, "target", l)?,
                    weight,
                }
            }
            "delete_morph" => {
                fields(v, &["op", "name"])?;
                DeleteMorph {
                    name: named(v, "name", l)?,
                }
            }
            "morph_weight" => {
                fields(v, &["op", "name", "weight"])?;
                let weight = float(need(v, "weight")?)?;
                valid_weight(weight)?;
                MorphWeight {
                    name: named(v, "name", l)?,
                    weight,
                }
            }
            _ => return Ok(None),
        };
        if result.memory_bytes() > l.max_transaction_bytes {
            return Err(Error::Budget("rig operation bytes"));
        }
        Ok(Some(result))
    }
    pub fn object_name(&self) -> Option<&str> {
        match self {
            Self::WeightLocks { object, .. }
            | Self::Weights { object, .. }
            | Self::Morph { object, .. }
            | Self::CaptureMorph { object, .. } => Some(object),
            _ => None,
        }
    }
    pub fn memory_bytes(&self) -> usize {
        use RigOperation::*;
        match self {
            MirrorJoints{joints,names,..}=>joints.len()*4+names.values().map(|n|n.len()+64).sum::<usize>()+256,
            Pose(p) => p.name.len() + p.joints.len() * 256 + 256,
            Constraint(c) => c.name.len() + 512,
            BakeClip { name, poses, .. } => {
                name.len() + poses.iter().map(|(_, p)| p.len() + 32).sum::<usize>() + 256
            }
            RetargetClip {
                name,
                source,
                joints,
                ..
            } => name.len() + source.len() + joints.len() * 64 + 256,
            ClipOptions { name, options } => {
                name.len()
                    + options
                        .events
                        .iter()
                        .map(|e| e.name.len() + e.payload.len() + 64)
                        .sum::<usize>()
                    + options
                        .morph_keys
                        .iter()
                        .map(|(n, k)| n.len() + k.len() * 32)
                        .sum::<usize>()
                    + 256
            }
            WeightLocks { object, joints } => object.len() + joints.len() * 64 + 256,
            Weights {
                object,
                vertices,
                edit,
            } => {
                object.len()
                    + vertices.len() * 8
                    + match edit {
                        WeightEdit::Mirror { joints, .. } => joints.len() * 64 + 256,
                        WeightEdit::Transfer { source, .. } => source.len() + 256,
                        _ => 256,
                    }
            }
            Morph {
                name,
                object,
                deltas,
                ..
            } => name.len() + object.len() + deltas.len() * 96 + 256,
            _ => 512,
        }
    }
}
fn int(v: u32) -> Value {
    Value::Int(v as i64)
}
fn scalar(v: f64) -> Value {
    Value::F64(v)
}
fn map_value(map: &BTreeMap<u32, u32>) -> Value {
    Value::Arr(
        map.iter()
            .map(|(a, b)| json::obj(vec![("source", int(*a)), ("target", int(*b))]))
            .collect(),
    )
}
pub(super) fn constraint_value(c: &RigConstraint) -> Value {
    let kind = match &c.kind {
        ConstraintKind::Copy {
            target,
            translation,
            rotation,
            scale,
            influence,
        } => json::obj(vec![
            ("type", json::s("copy")),
            ("target", int(*target)),
            ("translation", Value::Bool(*translation)),
            ("rotation", Value::Bool(*rotation)),
            ("scale", Value::Bool(*scale)),
            ("influence", scalar(*influence)),
        ]),
        ConstraintKind::Aim {
            target,
            axis,
            influence,
        } => json::obj(vec![
            ("type", json::s("aim")),
            ("target", int(*target)),
            ("axis", vec_value(axis)),
            ("influence", scalar(*influence)),
        ]),
        ConstraintKind::Limit {
            min_translation,
            max_translation,
            min_scale,
            max_scale,
            max_angle,
        } => json::obj(vec![
            ("type", json::s("limit")),
            ("min_translation", vec_value(min_translation)),
            ("max_translation", vec_value(max_translation)),
            ("min_scale", vec_value(min_scale)),
            ("max_scale", vec_value(max_scale)),
            ("max_angle", scalar(*max_angle)),
        ]),
        ConstraintKind::TwoBoneIk {
            middle,
            end,
            target,
            pole,
            clamp_reach,
        } => json::obj(vec![
            ("type", json::s("ik")),
            ("middle", int(*middle)),
            ("end", int(*end)),
            ("target", int(*target)),
            ("pole", vec_value(pole)),
            ("clamp_reach", Value::Bool(*clamp_reach)),
        ]),
    };
    json::obj(vec![
        ("op", json::s("constraint")),
        ("name", json::s(&c.name)),
        ("joint", int(c.joint)),
        ("enabled", Value::Bool(c.enabled)),
        ("kind", kind),
    ])
}
pub(super) fn options_value(name: &str, o: &ClipOptions) -> Value {
    json::obj(vec![
        ("op", json::s("clip_options")),
        ("name", json::s(name)),
        (
            "interpolation",
            json::s(match o.interpolation {
                Interpolation::Linear => "linear",
                Interpolation::Step => "step",
                Interpolation::Cubic => "cubic",
            }),
        ),
        ("root_motion", Value::Bool(o.root_motion)),
        (
            "events",
            Value::Arr(
                o.events
                    .iter()
                    .map(|e| {
                        json::obj(vec![
                            ("time", scalar(e.time)),
                            ("name", json::s(&e.name)),
                            ("payload", json::s(&e.payload)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "morph_keys",
            Value::Arr(
                o.morph_keys
                    .iter()
                    .map(|(name, keys)| {
                        json::obj(vec![
                            ("morph", json::s(name)),
                            (
                                "keys",
                                Value::Arr(
                                    keys.iter()
                                        .map(|k| {
                                            json::obj(vec![
                                                ("time", scalar(k.time)),
                                                ("weight", scalar(k.weight)),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}
impl RigOperation {
    pub fn value(&self) -> Value {
        use RigOperation::*;
        match self {
            MirrorJoints{joints,axis,names}=>json::obj(vec![("op",json::s("rig_mirror")),("joints",Value::Arr(joints.iter().map(|i|int(*i)).collect())),("axis",Value::Int(*axis as i64)),("names",Value::Arr(names.iter().map(|(joint,name)|json::obj(vec![("joint",int(*joint)),("name",json::s(name))])).collect()))]),
            Rest { joint, transform } => json::obj(vec![
                ("op", json::s("rig_rest")),
                ("joint", int(*joint)),
                ("transform", transform_value(transform)),
            ]),
            RenameJoint { joint, name } => json::obj(vec![
                ("op", json::s("rig_rename")),
                ("joint", int(*joint)),
                ("name", json::s(name)),
            ]),
            ReparentJoint { joint, parent } => json::obj(vec![
                ("op", json::s("rig_parent")),
                ("joint", int(*joint)),
                ("parent", parent.map(int).unwrap_or(Value::Null)),
            ]),
            Roll { joint, angle } => json::obj(vec![
                ("op", json::s("rig_roll")),
                ("joint", int(*joint)),
                ("angle", scalar(*angle)),
            ]),
            Pose(p) => json::obj(vec![
                ("op", json::s("pose")),
                ("name", json::s(&p.name)),
                (
                    "joints",
                    Value::Arr(
                        p.joints
                            .iter()
                            .map(|(i, t)| {
                                json::obj(vec![
                                    ("joint", int(*i)),
                                    ("transform", transform_value(t)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
            DeletePose { name } => json::obj(vec![
                ("op", json::s("delete_pose")),
                ("name", json::s(name)),
            ]),
            Constraint(c) => constraint_value(c),
            DeleteConstraint { name } => json::obj(vec![
                ("op", json::s("delete_constraint")),
                ("name", json::s(name)),
            ]),
            SolvePose { pose } => {
                json::obj(vec![("op", json::s("solve_pose")), ("pose", json::s(pose))])
            }
            Ik {
                pose,
                root,
                middle,
                end,
                target,
                pole,
                clamp_reach,
            } => json::obj(vec![
                ("op", json::s("ik")),
                ("pose", json::s(pose)),
                ("root", int(*root)),
                ("middle", int(*middle)),
                ("end", int(*end)),
                ("target", vec_value(target)),
                ("pole", vec_value(pole)),
                ("clamp_reach", Value::Bool(*clamp_reach)),
            ]),
            BakeClip {
                name,
                poses,
                fps,
                solve_constraints,
            } => json::obj(vec![
                ("op", json::s("bake_clip")),
                ("name", json::s(name)),
                (
                    "poses",
                    Value::Arr(
                        poses
                            .iter()
                            .map(|(t, p)| {
                                json::obj(vec![("time", scalar(*t)), ("pose", json::s(p))])
                            })
                            .collect(),
                    ),
                ),
                ("fps", scalar(*fps)),
                ("solve_constraints", Value::Bool(*solve_constraints)),
            ]),
            BlendClip {
                name,
                a,
                b,
                weight,
                fps,
            } => json::obj(vec![
                ("op", json::s("blend_clip")),
                ("name", json::s(name)),
                ("a", json::s(a)),
                ("b", json::s(b)),
                ("weight", scalar(*weight)),
                ("fps", scalar(*fps)),
            ]),
            RetargetClip {
                name,
                source,
                joints,
                translation_scale,
            } => json::obj(vec![
                ("op", json::s("retarget_clip")),
                ("name", json::s(name)),
                ("source", json::s(source)),
                ("joints", map_value(joints)),
                ("translation_scale", scalar(*translation_scale)),
            ]),
            ClipOptions { name, options } => options_value(name, options),
            WeightLocks { object, joints } => json::obj(vec![
                ("op", json::s("weight_locks")),
                ("object", json::s(object)),
                (
                    "joints",
                    Value::Arr(joints.iter().map(|v| int(*v)).collect()),
                ),
            ]),
            Weights {
                object,
                vertices,
                edit,
            } => {
                let edit = match edit {
                    WeightEdit::Assign{joint} => json::obj(vec![("type",json::s("assign")),("joint",int(*joint))]),
                    WeightEdit::Normalize { max_influences } => json::obj(vec![
                        ("type", json::s("normalize")),
                        ("max_influences", Value::Int(*max_influences as i64)),
                    ]),
                    WeightEdit::Smooth { iterations, factor } => json::obj(vec![
                        ("type", json::s("smooth")),
                        ("iterations", int(*iterations)),
                        ("factor", scalar(*factor)),
                    ]),
                    WeightEdit::Mirror {
                        axis,
                        joints,
                        tolerance,
                    } => json::obj(vec![
                        ("type", json::s("mirror")),
                        ("axis", Value::Int(*axis as i64)),
                        ("joints", map_value(joints)),
                        ("tolerance", scalar(*tolerance)),
                    ]),
                    WeightEdit::Transfer {
                        source,
                        max_distance,
                    } => json::obj(vec![
                        ("type", json::s("transfer")),
                        ("source", json::s(source)),
                        ("max_distance", scalar(*max_distance)),
                    ]),
                    WeightEdit::Bind {
                        max_influences,
                        power,
                    } => json::obj(vec![
                        ("type", json::s("bind")),
                        ("max_influences", Value::Int(*max_influences as i64)),
                        ("power", scalar(*power)),
                    ]),
                };
                json::obj(vec![
                    ("op", json::s("weight_edit")),
                    ("object", json::s(object)),
                    (
                        "vertices",
                        Value::Arr(vertices.iter().map(|v| json::s(v.0.to_string())).collect()),
                    ),
                    ("edit", edit),
                ])
            }
            Morph {
                name,
                object,
                deltas,
                weight,
            } => json::obj(vec![
                ("op", json::s("morph")),
                ("name", json::s(name)),
                ("object", json::s(object)),
                (
                    "deltas",
                    Value::Arr(
                        deltas
                            .iter()
                            .map(|(v, d)| {
                                json::obj(vec![
                                    ("vertex", json::s(v.0.to_string())),
                                    ("delta", vec_value(d)),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("weight", scalar(*weight)),
            ]),
            CaptureMorph {
                name,
                object,
                target,
                weight,
            } => json::obj(vec![
                ("op", json::s("capture_morph")),
                ("name", json::s(name)),
                ("object", json::s(object)),
                ("target", json::s(target)),
                ("weight", scalar(*weight)),
            ]),
            DeleteMorph { name } => json::obj(vec![
                ("op", json::s("delete_morph")),
                ("name", json::s(name)),
            ]),
            MorphWeight { name, weight } => json::obj(vec![
                ("op", json::s("morph_weight")),
                ("name", json::s(name)),
                ("weight", scalar(*weight)),
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_rig_command_has_a_stable_strict_json_roundtrip() {
        let l = Limits::default();
        let texts = [
            r#"{"op":"rig_rest","joint":0,"transform":{"translation":[1,2,3]}}"#,
            r#"{"op":"rig_rename","joint":0,"name":"root"}"#,
            r#"{"op":"rig_parent","joint":1,"parent":0}"#,
            r#"{"op":"rig_roll","joint":1,"angle":0.2}"#,
            r#"{"op":"pose","name":"p","joints":[{"joint":0,"transform":{}}]}"#,
            r#"{"op":"delete_pose","name":"p"}"#,
            r#"{"op":"constraint","name":"c","joint":1,"kind":{"type":"copy","target":0}}"#,
            r#"{"op":"constraint","name":"c","joint":1,"kind":{"type":"aim","target":0}}"#,
            r#"{"op":"constraint","name":"c","joint":1,"kind":{"type":"limit","min_translation":[0,0,0],"max_translation":[1,1,1],"min_scale":[1,1,1],"max_scale":[2,2,2],"max_angle":1}}"#,
            r#"{"op":"constraint","name":"c","joint":0,"kind":{"type":"ik","middle":1,"end":2,"target":3,"pole":[1,0,0]}}"#,
            r#"{"op":"delete_constraint","name":"c"}"#,
            r#"{"op":"solve_pose","pose":"p"}"#,
            r#"{"op":"ik","pose":"p","root":0,"middle":1,"end":2,"target":[1,1,0],"pole":[0,0,1]}"#,
            r#"{"op":"bake_clip","name":"walk","poses":[{"time":0,"pose":"a"},{"time":1,"pose":"b"}]}"#,
            r#"{"op":"blend_clip","name":"walk","a":"a","b":"b","weight":0.5}"#,
            r#"{"op":"retarget_clip","name":"walk","source":"a","joints":[{"source":0,"target":1}]}"#,
            r#"{"op":"clip_options","name":"walk","interpolation":"step","root_motion":true,"events":[{"time":0,"name":"step","payload":"a"}],"morph_keys":[{"morph":"smile","keys":[{"time":0,"weight":0},{"time":1,"weight":1}]}]}"#,
            r#"{"op":"weight_locks","object":"body","joints":[0,1]}"#,
            r#"{"op":"weight_edit","object":"body","vertices":["1"],"edit":{"type":"normalize"}}"#,
            r#"{"op":"weight_edit","object":"body","vertices":[],"edit":{"type":"smooth"}}"#,
            r#"{"op":"weight_edit","object":"body","vertices":[],"edit":{"type":"mirror","axis":0,"joints":[{"source":0,"target":1}]}}"#,
            r#"{"op":"weight_edit","object":"body","vertices":[],"edit":{"type":"transfer","source":"high","max_distance":1}}"#,
            r#"{"op":"weight_edit","object":"body","vertices":[],"edit":{"type":"bind"}}"#,
            r#"{"op":"morph","name":"shape","object":"body","deltas":[{"vertex":"1","delta":[0,1,0]}]}"#,
            r#"{"op":"capture_morph","name":"shape","object":"body","target":"copy"}"#,
            r#"{"op":"delete_morph","name":"shape"}"#,
            r#"{"op":"morph_weight","name":"shape","weight":0.5}"#,
        ];
        for text in texts {
            let op = RigOperation::parse(&json::parse(text.as_bytes()).unwrap(), &l)
                .unwrap()
                .unwrap();
            let canonical = op.value().to_json();
            let again = RigOperation::parse(&json::parse(canonical.as_bytes()).unwrap(), &l)
                .unwrap()
                .unwrap();
            assert_eq!(op, again, "{text}");
            assert_eq!(again.value().to_json(), canonical);
        }
    }
    #[test]
    fn malformed_rig_commands_reject_unknowns_duplicates_and_bounds() {
        let l = Limits::default();
        for text in [
            r#"{"op":"rig_rest","joint":0,"transform":{"rotation":[0,0,0,0]}}"#,
            r#"{"op":"pose","name":"x","joints":[{"joint":0,"transform":{}},{"joint":0,"transform":{}}]}"#,
            r#"{"op":"constraint","name":"x","joint":0,"kind":{"type":"aim","target":1,"axis":[0,0,0]}}"#,
            r#"{"op":"weight_edit","object":"x","vertices":["01"],"edit":{"type":"normalize"}}"#,
            r#"{"op":"weight_edit","object":"x","vertices":[],"edit":{"type":"normalize","max_influences":0}}"#,
            r#"{"op":"morph","name":"x","object":"o","deltas":[],"weight":3}"#,
            r#"{"op":"clip_options","name":"x","root_motion":"yes"}"#,
            r#"{"op":"ik","pose":"p","root":0,"middle":1,"end":2,"target":[1,1,0],"pole":[0,0,1],"ignored":true}"#,
        ] {
            assert!(
                RigOperation::parse(&json::parse(text.as_bytes()).unwrap(), &l).is_err(),
                "{text}"
            );
        }
    }
}
