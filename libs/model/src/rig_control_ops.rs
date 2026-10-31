use super::*;
fn finite(values: &[f64]) -> Result<()> {
    if values.iter().any(|v| !v.is_finite()) { Err(Error::Invalid("expected finite number")) } else { Ok(()) }
}
fn bounded(len: usize, max: usize) -> Result<()> {
    if len > max { Err(Error::Budget("authoring array")) } else { Ok(()) }
}
fn check_joint(joint: u32, l: &Limits) -> Result<()> {
    if joint as usize >= l.max_joints { Err(Error::Budget("joint index")) } else { Ok(()) }
}
fn check_map(joints: &BTreeMap<u32, u32>, l: &Limits) -> Result<()> {
    bounded(joints.len(), l.max_joints)?;
    for (&a, &b) in joints { check_joint(a, l)?; check_joint(b, l)?; }
    Ok(())
}
fn check_fps(value: f64) -> Result<()> {
    finite(&[value])?;
    if (1.0..=120.0).contains(&value) { Ok(()) } else { Err(Error::Invalid("fps must be1..120")) }
}
impl ClipOptions {
    fn check(&self, l: &Limits) -> Result<()> {
        bounded(self.events.len(), 1024)?;
        let mut previous = 0.;
        for e in &self.events {
            finite(&[e.time])?;
            if e.time < previous || e.time > l.max_clip_duration { return Err(Error::Invalid("clip event order/time")); }
            if e.payload.len() > 4096 { return Err(Error::Invalid("clip event payload")); }
            name(&e.name, l)?; previous = e.time;
        }
        bounded(self.morph_keys.len(), 32)?;
        let mut total = 0usize;
        for (morph, keys) in &self.morph_keys {
            bounded(keys.len(), l.max_keyframes.saturating_sub(total))?;
            if keys.is_empty() { return Err(Error::Invalid("empty morph keys")); }
            let mut previous = -1f64;
            for k in keys {
                finite(&[k.time, k.weight])?; valid_weight(k.weight)?;
                if k.time < 0. || k.time > l.max_clip_duration || k.time as f32 <= previous as f32 { return Err(Error::Invalid("morph key order/time")); }
                previous = k.time;
            }
            total += keys.len(); name(morph, l)?;
        }
        Ok(())
    }
}
impl RigOperation {
    pub fn check(&self, l: &Limits) -> Result<()> {
        use RigOperation::*;
        match self {
            MirrorJoints { joints, axis, names } => {
                if *axis > 2 { return Err(Error::Invalid("mirror axis")); }
                bounded(joints.len(), l.max_joints)?; bounded(names.len(), l.max_joints)?;
                for &j in joints { check_joint(j, l)?; }
                for (&j, n) in names { check_joint(j, l)?; name(n, l)?; }
                if joints.windows(2).any(|p| p[0] >= p[1]) { return Err(Error::Invalid("duplicate joint")); }
                if joints.is_empty() || !names.keys().copied().eq(joints.iter().copied()) { return Err(Error::Invalid("mirror requires a name for every selected joint")); }
            }
            Rest { joint, transform } => { check_joint(*joint, l)?; transform.validate()?; }
            RenameJoint { joint, name: n } => { check_joint(*joint, l)?; name(n, l)?; }
            ReparentJoint { joint, parent } => { check_joint(*joint, l)?; if let Some(p) = parent { check_joint(*p, l)?; } }
            Roll { joint, angle } => { check_joint(*joint, l)?; finite(&[*angle])?; }
            Pose(p) => {
                bounded(p.joints.len(), l.max_joints)?;
                for (&j, t) in &p.joints { check_joint(j, l)?; t.validate()?; }
                name(&p.name, l)?;
            }
            DeletePose { name: n } | DeleteConstraint { name: n } | DeleteMorph { name: n } => name(n, l)?,
            Constraint(c) => {
                match &c.kind {
                    ConstraintKind::Copy { target, influence, .. } | ConstraintKind::Aim { target, influence, .. } => {
                        check_joint(*target, l)?; finite(&[*influence])?; unit(*influence)?;
                        if let ConstraintKind::Aim { axis, .. } = &c.kind { finite(axis)?; normalized(*axis)?; }
                    }
                    ConstraintKind::Limit { min_translation, max_translation, min_scale, max_scale, max_angle } => {
                        finite(min_translation)?; finite(max_translation)?; finite(min_scale)?; finite(max_scale)?; finite(&[*max_angle])?;
                    }
                    ConstraintKind::TwoBoneIk { middle, end, target, pole, .. } => {
                        check_joint(*middle, l)?; check_joint(*end, l)?; check_joint(*target, l)?; finite(pole)?;
                    }
                }
                name(&c.name, l)?; check_joint(c.joint, l)?; c.validate(l.max_joints)?;
            }
            SolvePose { pose } => name(pose, l)?,
            Ik { pose, root, middle, end, target, pole, .. } => {
                name(pose, l)?; check_joint(*root, l)?; check_joint(*middle, l)?; check_joint(*end, l)?; finite(target)?; finite(pole)?;
            }
            BakeClip { name: n, poses, fps, .. } => {
                bounded(poses.len(), l.max_keyframes)?;
                for (time, pose) in poses { finite(&[*time])?; name(pose, l)?; }
                name(n, l)?; check_fps(*fps)?; name_check_keys(n, poses, *fps, l)?;
            }
            BlendClip { name: n, a, b, weight, fps } => { name(n, l)?; name(a, l)?; name(b, l)?; finite(&[*weight])?; unit(*weight)?; check_fps(*fps)?; }
            RetargetClip { name: n, source, joints, translation_scale } => { name(n, l)?; name(source, l)?; check_map(joints, l)?; finite(&[*translation_scale])?; positive(*translation_scale)?; }
            ClipOptions { name: n, options } => { name(n, l)?; options.check(l)?; }
            WeightLocks { object, joints } => { name(object, l)?; bounded(joints.len(), l.max_joints)?; for &j in joints { check_joint(j, l)?; } }
            Weights { object, vertices, edit } => {
                match edit {
                    WeightEdit::Assign { joint } => if *joint as usize >= l.max_joints { return Err(Error::Budget("weight assignment joint")); },
                    WeightEdit::Normalize { max_influences } | WeightEdit::Bind { max_influences, .. } => {
                        if *max_influences > u32::MAX as usize { return Err(Error::Invalid("expected u32 integer")); }
                        if *max_influences == 0 || *max_influences > l.mesh.max_weights_per_vertex { return Err(Error::Budget("weight influences")); }
                        if let WeightEdit::Bind { power, .. } = edit { finite(&[*power])?; positive(*power)?; if *power > 8. { return Err(Error::Invalid("binding power must be at most8")); } }
                    }
                    WeightEdit::Smooth { iterations, factor } => {
                        if *iterations == 0 || *iterations > 64 { return Err(Error::Budget("weight smooth iterations")); }
                        finite(&[*factor])?; unit(*factor)?;
                    }
                    WeightEdit::Mirror { axis, joints, tolerance } => {
                        if *axis > 2 { return Err(Error::Invalid("mirror axis")); }
                        check_map(joints, l)?; finite(&[*tolerance])?; positive(*tolerance)?;
                    }
                    WeightEdit::Transfer { source, max_distance } => { name(source, l)?; finite(&[*max_distance])?; positive(*max_distance)?; }
                }
                name(object, l)?;
                if vertices.len() > l.mesh.max_vertices { return Err(Error::Budget("selection size")); }
                let mut seen = BTreeSet::new();
                if vertices.iter().any(|v| v.0 == 0 || !seen.insert(v)) { return Err(Error::Invalid("duplicate/zero vertex ID")); }
            }
            Morph { name: n, object, deltas, weight } => {
                bounded(deltas.len(), l.mesh.max_vertices)?;
                for (id, delta) in deltas { finite(delta)?; if id.0 == 0 || delta.iter().any(|v| !(*v as f32).is_finite()) { return Err(Error::Invalid("morph vertex/delta")); } }
                finite(&[*weight])?; valid_weight(*weight)?; name(n, l)?; name(object, l)?;
            }
            CaptureMorph { name: n, object, target, weight } => { finite(&[*weight])?; valid_weight(*weight)?; name(n, l)?; name(object, l)?; name(target, l)?; }
            MorphWeight { name: n, weight } => { finite(&[*weight])?; valid_weight(*weight)?; name(n, l)?; }
        }
        if self.memory_bytes() > l.max_transaction_bytes { return Err(Error::Budget("rig operation bytes")); }
        Ok(())
    }
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
    fn every_rig_operation_has_a_stable_binary_roundtrip() {
        let l = Limits::default();
        let operations = [
            RigOperation::Rest {
                joint: 0,
                transform: Transform {
                    translation: [1.0, 2.0, 3.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            },
            RigOperation::RenameJoint { joint: 0, name: "root".into() },
            RigOperation::ReparentJoint { joint: 1, parent: Some(0) },
            RigOperation::Roll { joint: 1, angle: 0.2 },
            RigOperation::Pose(Pose {
                name: "p".into(),
                joints: std::collections::BTreeMap::from([
                    (
                        0,
                        Transform {
                            translation: [0.0, 0.0, 0.0],
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0, 1.0, 1.0]
                        }
                    )
                ])
            }),
            RigOperation::DeletePose { name: "p".into() },
            RigOperation::Constraint(RigConstraint {
                name: "c".into(),
                joint: 1,
                enabled: true,
                kind: ConstraintKind::Copy {
                    translation: true,
                    rotation: true,
                    scale: false,
                    influence: 1.0,
                    target: 0
                }
            }),
            RigOperation::Constraint(RigConstraint {
                name: "c".into(),
                joint: 1,
                enabled: true,
                kind: ConstraintKind::Aim { axis: [0.0, 1.0, 0.0], influence: 1.0, target: 0 }
            }),
            RigOperation::Constraint(RigConstraint {
                name: "c".into(),
                joint: 1,
                enabled: true,
                kind: ConstraintKind::Limit {
                    min_translation: [0.0, 0.0, 0.0],
                    max_translation: [1.0, 1.0, 1.0],
                    min_scale: [1.0, 1.0, 1.0],
                    max_scale: [2.0, 2.0, 2.0],
                    max_angle: 1.0
                }
            }),
            RigOperation::Constraint(RigConstraint {
                name: "c".into(),
                joint: 0,
                enabled: true,
                kind: ConstraintKind::TwoBoneIk {
                    clamp_reach: false,
                    middle: 1,
                    end: 2,
                    target: 3,
                    pole: [1.0, 0.0, 0.0]
                }
            }),
            RigOperation::DeleteConstraint { name: "c".into() },
            RigOperation::SolvePose { pose: "p".into() },
            RigOperation::Ik { pose: "p".into(), root: 0, middle: 1, end: 2, target: [1.0, 1.0, 0.0], pole: [0.0, 0.0, 1.0], clamp_reach: false },
            RigOperation::BakeClip { name: "walk".into(), poses: vec![(0.0, "a".into()), (1.0, "b".into())], fps: 30.0, solve_constraints: true },
            RigOperation::BlendClip { name: "walk".into(), a: "a".into(), b: "b".into(), weight: 0.5, fps: 30.0 },
            RigOperation::RetargetClip {
                name: "walk".into(),
                source: "a".into(),
                joints: std::collections::BTreeMap::from([(0, 1)]),
                translation_scale: 1.0
            },
            RigOperation::ClipOptions {
                name: "walk".into(),
                options: ClipOptions {
                    interpolation: Interpolation::Step,
                    root_motion: true,
                    events: vec![ClipEvent { time: 0.0, name: "step".into(), payload: "a".into() }],
                    morph_keys: std::collections::BTreeMap::from([
                        ("smile".into(), vec![MorphKey { time: 0.0, weight: 0.0 }, MorphKey { time: 1.0, weight: 1.0 }])
                    ])
                }
            },
            RigOperation::WeightLocks { object: "body".into(), joints: std::collections::BTreeSet::from([0, 1]) },
            RigOperation::Weights { object: "body".into(), vertices: vec![mesh::VertexId(1)], edit: WeightEdit::Normalize { max_influences: 4 } },
            RigOperation::Weights { object: "body".into(), vertices: vec![], edit: WeightEdit::Smooth { iterations: 1, factor: 0.5 } },
            RigOperation::Weights {
                object: "body".into(),
                vertices: vec![],
                edit: WeightEdit::Mirror {
                    tolerance: 0.001,
                    axis: 0,
                    joints: std::collections::BTreeMap::from([(0, 1)])
                }
            },
            RigOperation::Weights {
                object: "body".into(),
                vertices: vec![],
                edit: WeightEdit::Transfer { source: "high".into(), max_distance: 1.0 }
            },
            RigOperation::Weights { object: "body".into(), vertices: vec![], edit: WeightEdit::Bind { max_influences: 4, power: 2.0 } },
            RigOperation::Morph {
                name: "shape".into(),
                object: "body".into(),
                deltas: std::collections::BTreeMap::from([(mesh::VertexId(1), [0.0, 1.0, 0.0])]),
                weight: 0.0
            },
            RigOperation::CaptureMorph { name: "shape".into(), object: "body".into(), target: "copy".into(), weight: 0.0 },
            RigOperation::DeleteMorph { name: "shape".into() },
            RigOperation::MorphWeight { name: "shape".into(), weight: 0.5 },
        ];
        for op in operations {
            op.check(&l).unwrap();
            let mut w=crate::canon::Writer::new(usize::MAX);op.write(&mut w).unwrap();
            let mut r=crate::canon::Reader::new(&w.bytes);
            let again=RigOperation::read(&mut r,&l).unwrap();r.end().unwrap();
            assert_eq!(op,again);
            let mut canonical=crate::canon::Writer::new(usize::MAX);again.write(&mut canonical).unwrap();
            assert_eq!(w.bytes,canonical.bytes);
        }
    }
    #[test]
    fn malformed_rig_commands_reject_unknowns_duplicates_and_bounds() {
        let l = Limits::default();
        for op in [
            RigOperation::Rest {
                joint: 0,
                transform: Transform {
                    translation: [0.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 0.0],
                    scale: [1.0, 1.0, 1.0]
                }
            },
            RigOperation::Constraint(RigConstraint {
                name: "x".into(),
                joint: 0,
                enabled: true,
                kind: ConstraintKind::Aim { axis: [0.0, 0.0, 0.0], influence: 1.0, target: 1 }
            }),
            RigOperation::Weights { object: "x".into(), vertices: vec![], edit: WeightEdit::Normalize { max_influences: 0 } },
            RigOperation::Morph { name: "x".into(), object: "o".into(), deltas: std::collections::BTreeMap::from([]), weight: 3.0 },
        ] {
            assert!(op.check(&l).is_err(),"{op:?}");
        }
    }
}


impl RigOperation {
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

impl RigOperation {
    /// The same operation with every joint index passed through `map` (a program's
    /// joint-name table resolved against the document's skeleton).
    pub fn map_joints(&self, map: &dyn Fn(u32) -> Result<u32>) -> Result<Self> {
        use RigOperation::*;
        let pairs = |m: &BTreeMap<u32, u32>| -> Result<BTreeMap<u32, u32>> { m.iter().map(|(a, b)| Ok((map(*a)?, map(*b)?))).collect() };
        Ok(match self.clone() {
            MirrorJoints { joints, axis, names } => MirrorJoints {
                joints: joints.into_iter().map(map).collect::<Result<_>>()?,
                axis,
                names: names.into_iter().map(|(j, n)| Ok((map(j)?, n))).collect::<Result<_>>()?,
            },
            Rest { joint, transform } => Rest { joint: map(joint)?, transform },
            RenameJoint { joint, name } => RenameJoint { joint: map(joint)?, name },
            ReparentJoint { joint, parent } => ReparentJoint { joint: map(joint)?, parent: parent.map(map).transpose()? },
            Roll { joint, angle } => Roll { joint: map(joint)?, angle },
            Pose(p) => Pose(super::Pose { name: p.name, joints: p.joints.into_iter().map(|(j, t)| Ok((map(j)?, t))).collect::<Result<_>>()? }),
            Constraint(c) => Constraint(RigConstraint {
                name: c.name,
                joint: map(c.joint)?,
                enabled: c.enabled,
                kind: match c.kind {
                    ConstraintKind::Copy { target, translation, rotation, scale, influence } => ConstraintKind::Copy { target: map(target)?, translation, rotation, scale, influence },
                    ConstraintKind::Aim { target, axis, influence } => ConstraintKind::Aim { target: map(target)?, axis, influence },
                    limit @ ConstraintKind::Limit { .. } => limit,
                    ConstraintKind::TwoBoneIk { middle, end, target, pole, clamp_reach } => ConstraintKind::TwoBoneIk { middle: map(middle)?, end: map(end)?, target: map(target)?, pole, clamp_reach },
                },
            }),
            Ik { pose, root, middle, end, target, pole, clamp_reach } => Ik { pose, root: map(root)?, middle: map(middle)?, end: map(end)?, target, pole, clamp_reach },
            RetargetClip { name, source, joints, translation_scale } => RetargetClip { name, source, joints: pairs(&joints)?, translation_scale },
            WeightLocks { object, joints } => WeightLocks { object, joints: joints.into_iter().map(map).collect::<Result<_>>()? },
            Weights { object, vertices, edit } => Weights {
                object,
                vertices,
                edit: match edit {
                    WeightEdit::Assign { joint } => WeightEdit::Assign { joint: map(joint)? },
                    WeightEdit::Mirror { axis, joints, tolerance } => WeightEdit::Mirror { axis, joints: pairs(&joints)?, tolerance },
                    other => other,
                },
            },
            other @ (DeletePose { .. } | DeleteConstraint { .. } | SolvePose { .. } | BakeClip { .. } | BlendClip { .. } | ClipOptions { .. }
                | Morph { .. } | CaptureMorph { .. } | DeleteMorph { .. } | MorphWeight { .. }) => other,
        })
    }
}
