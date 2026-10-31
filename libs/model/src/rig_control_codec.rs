use super::*;
fn map_write(w: &mut Writer, map: &BTreeMap<u32, u32>) -> Result<()> {
    w.count(map.len())?;
    for (&a, &b) in map { w.u32(a)?; w.u32(b)?; }
    Ok(())
}
fn map_read(r: &mut Reader, l: &Limits) -> Result<BTreeMap<u32, u32>> {
    let mut map = BTreeMap::new();
    for _ in 0..r.count(l.max_joints)? {
        let key = r.u32()?;
        if map.last_key_value().is_some_and(|(&last, _)| last >= key) { return Err(Error::Corrupt("rig joint map order")); }
        map.insert(key, r.u32()?);
    }
    Ok(map)
}
fn weight_write(w: &mut Writer, edit: &WeightEdit) -> Result<()> {
    match edit {
        WeightEdit::Assign { joint } => { w.u8(0)?; w.u32(*joint)?; }
        WeightEdit::Normalize { max_influences } => { w.u8(1)?; w.count(*max_influences)?; }
        WeightEdit::Smooth { iterations, factor } => { w.u8(2)?; w.u32(*iterations)?; w.f64(*factor)?; }
        WeightEdit::Mirror { axis, joints, tolerance } => { w.u8(3)?; w.count(*axis)?; map_write(w, joints)?; w.f64(*tolerance)?; }
        WeightEdit::Transfer { source, max_distance } => { w.u8(4)?; w.string(source)?; w.f64(*max_distance)?; }
        WeightEdit::Bind { max_influences, power } => { w.u8(5)?; w.count(*max_influences)?; w.f64(*power)?; }
    }
    Ok(())
}
fn weight_read(r: &mut Reader, l: &Limits) -> Result<WeightEdit> {
    Ok(match r.u8()? {
        0 => WeightEdit::Assign { joint: r.u32()? },
        1 => WeightEdit::Normalize { max_influences: r.u32()? as usize },
        2 => WeightEdit::Smooth { iterations: r.u32()?, factor: r.f64()? },
        3 => WeightEdit::Mirror { axis: r.u32()? as usize, joints: map_read(r, l)?, tolerance: r.f64()? },
        4 => WeightEdit::Transfer { source: name_read(r, l)?, max_distance: r.f64()? },
        5 => WeightEdit::Bind { max_influences: r.u32()? as usize, power: r.f64()? },
        _ => return Err(Error::Corrupt("weight edit tag")),
    })
}
impl RigOperation {
    pub(crate) fn write(&self, w: &mut Writer) -> Result<()> {
        use RigOperation::*;
        match self {
            MirrorJoints { joints, axis, names } => {
                w.u8(0)?; w.count(joints.len())?; for j in joints { w.u32(*j)?; }
                w.count(*axis)?; w.count(names.len())?; for (j, n) in names { w.u32(*j)?; w.string(n)?; }
            }
            Rest { joint, transform } => { w.u8(1)?; w.u32(*joint)?; transform_write(w, transform)?; }
            RenameJoint { joint, name } => { w.u8(2)?; w.u32(*joint)?; w.string(name)?; }
            ReparentJoint { joint, parent } => { w.u8(3)?; w.u32(*joint)?; w.u8(parent.is_some() as u8)?; if let Some(p) = parent { w.u32(*p)?; } }
            Roll { joint, angle } => { w.u8(4)?; w.u32(*joint)?; w.f64(*angle)?; }
            Pose(p) => { w.u8(5)?; pose_write(w, p)?; }
            DeletePose { name } => { w.u8(6)?; w.string(name)?; }
            Constraint(c) => { w.u8(7)?; constraint_write(w, c)?; }
            DeleteConstraint { name } => { w.u8(8)?; w.string(name)?; }
            SolvePose { pose } => { w.u8(9)?; w.string(pose)?; }
            Ik { pose, root, middle, end, target, pole, clamp_reach } => {
                w.u8(10)?; w.string(pose)?; w.u32(*root)?; w.u32(*middle)?; w.u32(*end)?;
                array_write(w, target)?; array_write(w, pole)?; w.u8(*clamp_reach as u8)?;
            }
            BakeClip { name, poses, fps, solve_constraints } => {
                w.u8(11)?; w.string(name)?; w.count(poses.len())?;
                for (time, pose) in poses { w.f64(*time)?; w.string(pose)?; }
                w.f64(*fps)?; w.u8(*solve_constraints as u8)?;
            }
            BlendClip { name, a, b, weight, fps } => { w.u8(12)?; w.string(name)?; w.string(a)?; w.string(b)?; w.f64(*weight)?; w.f64(*fps)?; }
            RetargetClip { name, source, joints, translation_scale } => { w.u8(13)?; w.string(name)?; w.string(source)?; map_write(w, joints)?; w.f64(*translation_scale)?; }
            ClipOptions { name, options } => { w.u8(14)?; w.string(name)?; options_write(w, options)?; }
            WeightLocks { object, joints } => { w.u8(15)?; w.string(object)?; w.count(joints.len())?; for j in joints { w.u32(*j)?; } }
            Weights { object, vertices, edit } => { w.u8(16)?; w.string(object)?; w.count(vertices.len())?; for v in vertices { w.u64(v.0)?; } weight_write(w, edit)?; }
            Morph { name, object, deltas, weight } => {
                w.u8(17)?; w.string(name)?; w.string(object)?; w.count(deltas.len())?;
                for (id, delta) in deltas { w.u64(id.0)?; array_write(w, delta)?; }
                w.f64(*weight)?;
            }
            CaptureMorph { name, object, target, weight } => { w.u8(18)?; w.string(name)?; w.string(object)?; w.string(target)?; w.f64(*weight)?; }
            DeleteMorph { name } => { w.u8(19)?; w.string(name)?; }
            MorphWeight { name, weight } => { w.u8(20)?; w.string(name)?; w.f64(*weight)?; }
        }
        Ok(())
    }
    pub(crate) fn read(r: &mut Reader, l: &Limits) -> Result<Self> {
        use RigOperation::*;
        let value = match r.u8()? {
            0 => {
                let mut joints = Vec::new(); for _ in 0..r.count(l.max_joints)? { joints.push(r.u32()?); }
                let axis = r.u32()? as usize;
                let mut names = BTreeMap::new();
                for _ in 0..r.count(l.max_joints)? {
                    let j = r.u32()?;
                    if names.last_key_value().is_some_and(|(&last, _)| last >= j) { return Err(Error::Corrupt("mirror name order")); }
                    names.insert(j, name_read(r, l)?);
                }
                MirrorJoints { joints, axis, names }
            }
            1 => Rest { joint: r.u32()?, transform: transform_read(r)? },
            2 => RenameJoint { joint: r.u32()?, name: name_read(r, l)? },
            3 => ReparentJoint { joint: r.u32()?, parent: if flag(r)? { Some(r.u32()?) } else { None } },
            4 => Roll { joint: r.u32()?, angle: r.f64()? },
            5 => Pose(pose_read(r, l)?),
            6 => DeletePose { name: name_read(r, l)? },
            7 => Constraint(constraint_read(r, l)?),
            8 => DeleteConstraint { name: name_read(r, l)? },
            9 => SolvePose { pose: name_read(r, l)? },
            10 => Ik { pose: name_read(r, l)?, root: r.u32()?, middle: r.u32()?, end: r.u32()?, target: array_read(r)?, pole: array_read(r)?, clamp_reach: flag(r)? },
            11 => {
                let name = name_read(r, l)?; let mut poses = Vec::new();
                for _ in 0..r.count(l.max_keyframes)? { poses.push((r.f64()?, name_read(r, l)?)); }
                BakeClip { name, poses, fps: r.f64()?, solve_constraints: flag(r)? }
            }
            12 => BlendClip { name: name_read(r, l)?, a: name_read(r, l)?, b: name_read(r, l)?, weight: r.f64()?, fps: r.f64()? },
            13 => RetargetClip { name: name_read(r, l)?, source: name_read(r, l)?, joints: map_read(r, l)?, translation_scale: r.f64()? },
            14 => ClipOptions { name: name_read(r, l)?, options: options_read(r, l)? },
            15 => {
                let object = name_read(r, l)?; let mut joints = BTreeSet::new();
                for _ in 0..r.count(l.max_joints)? {
                    let j = r.u32()?; if joints.last().is_some_and(|last| *last >= j) { return Err(Error::Corrupt("locked joint order")); } joints.insert(j);
                }
                WeightLocks { object, joints }
            }
            16 => {
                let object = name_read(r, l)?; let mut vertices = Vec::new();
                for _ in 0..r.count(l.mesh.max_vertices)? { vertices.push(mesh::VertexId(r.u64()?)); }
                Weights { object, vertices, edit: weight_read(r, l)? }
            }
            17 => {
                let name = name_read(r, l)?; let object = name_read(r, l)?; let mut deltas = BTreeMap::new();
                for _ in 0..r.count(l.mesh.max_vertices)? {
                    let id = mesh::VertexId(r.u64()?);
                    if deltas.last_key_value().is_some_and(|(last, _)| *last >= id) { return Err(Error::Corrupt("morph delta order")); }
                    deltas.insert(id, array_read(r)?);
                }
                Morph { name, object, deltas, weight: r.f64()? }
            }
            18 => CaptureMorph { name: name_read(r, l)?, object: name_read(r, l)?, target: name_read(r, l)?, weight: r.f64()? },
            19 => DeleteMorph { name: name_read(r, l)? },
            20 => MorphWeight { name: name_read(r, l)?, weight: r.f64()? },
            _ => return Err(Error::Corrupt("rig operation tag")),
        };
        value.check(l)?;
        Ok(value)
    }
}
fn array_write<const N: usize>(w: &mut Writer, a: &[f64; N]) -> Result<()> {
    for v in a {
        w.f64(*v)?;
    }
    Ok(())
}
fn array_read<const N: usize>(r: &mut Reader<'_>) -> Result<[f64; N]> {
    let mut a = [0.; N];
    for v in &mut a {
        *v = r.f64()?;
    }
    Ok(a)
}
fn transform_write(w: &mut Writer, t: &Transform) -> Result<()> {
    t.validate()?;
    array_write(w, &t.translation)?;
    array_write(w, &t.rotation)?;
    array_write(w, &t.scale)
}
fn transform_read(r: &mut Reader<'_>) -> Result<Transform> {
    let t = Transform {
        translation: array_read(r)?,
        rotation: array_read(r)?,
        scale: array_read(r)?,
    };
    t.validate()?;
    Ok(t)
}
fn flag(r: &mut Reader<'_>) -> Result<bool> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Corrupt("rig boolean")),
    }
}
fn name_read(r: &mut Reader<'_>, l: &Limits) -> Result<String> {
    let n = r.string(l.max_name_bytes)?;
    name(&n, l)?;
    Ok(n)
}
fn pose_write(w: &mut Writer, p: &Pose) -> Result<()> {
    w.string(&p.name)?;
    w.count(p.joints.len())?;
    for (joint, t) in &p.joints {
        w.u32(*joint)?;
        transform_write(w, t)?;
    }
    Ok(())
}
fn pose_read(r: &mut Reader<'_>, l: &Limits) -> Result<Pose> {
    let name = name_read(r, l)?;
    let mut joints = BTreeMap::new();
    for _ in 0..r.count(l.max_joints)? {
        let joint = r.u32()?;
        if joints.last_key_value().is_some_and(|(&last, _)| last >= joint) {
            return Err(Error::Corrupt("pose joint order"));
        }
        if joints.insert(joint, transform_read(r)?).is_some() {
            return Err(Error::Corrupt("duplicate pose joint"));
        }
    }
    Ok(Pose { name, joints })
}
fn constraint_write(w: &mut Writer, c: &RigConstraint) -> Result<()> {
    w.string(&c.name)?;
    w.u32(c.joint)?;
    w.u8(c.enabled as u8)?;
    match &c.kind {
        ConstraintKind::Copy {
            target,
            translation,
            rotation,
            scale,
            influence,
        } => {
            w.u8(0)?;
            w.u32(*target)?;
            w.u8(*translation as u8)?;
            w.u8(*rotation as u8)?;
            w.u8(*scale as u8)?;
            w.f64(*influence)?;
        }
        ConstraintKind::Aim {
            target,
            axis,
            influence,
        } => {
            w.u8(1)?;
            w.u32(*target)?;
            array_write(w, axis)?;
            w.f64(*influence)?;
        }
        ConstraintKind::Limit {
            min_translation,
            max_translation,
            min_scale,
            max_scale,
            max_angle,
        } => {
            w.u8(2)?;
            array_write(w, min_translation)?;
            array_write(w, max_translation)?;
            array_write(w, min_scale)?;
            array_write(w, max_scale)?;
            w.f64(*max_angle)?;
        }
        ConstraintKind::TwoBoneIk {
            middle,
            end,
            target,
            pole,
            clamp_reach,
        } => {
            w.u8(3)?;
            w.u32(*middle)?;
            w.u32(*end)?;
            w.u32(*target)?;
            array_write(w, pole)?;
            w.u8(*clamp_reach as u8)?;
        }
    }
    Ok(())
}
fn constraint_read(r: &mut Reader<'_>, l: &Limits) -> Result<RigConstraint> {
    let name = name_read(r, l)?;
    let joint = r.u32()?;
    let enabled = flag(r)?;
    let kind = match r.u8()? {
        0 => ConstraintKind::Copy {
            target: r.u32()?,
            translation: flag(r)?,
            rotation: flag(r)?,
            scale: flag(r)?,
            influence: r.f64()?,
        },
        1 => ConstraintKind::Aim {
            target: r.u32()?,
            axis: array_read(r)?,
            influence: r.f64()?,
        },
        2 => ConstraintKind::Limit {
            min_translation: array_read(r)?,
            max_translation: array_read(r)?,
            min_scale: array_read(r)?,
            max_scale: array_read(r)?,
            max_angle: r.f64()?,
        },
        3 => ConstraintKind::TwoBoneIk {
            middle: r.u32()?,
            end: r.u32()?,
            target: r.u32()?,
            pole: array_read(r)?,
            clamp_reach: flag(r)?,
        },
        _ => return Err(Error::Corrupt("constraint kind")),
    };
    let c = RigConstraint {
        name,
        joint,
        enabled,
        kind,
    };
    c.validate(l.max_joints)?;
    Ok(c)
}
fn options_write(w: &mut Writer, o: &ClipOptions) -> Result<()> {
    w.u8(match o.interpolation {
        Interpolation::Linear => 0,
        Interpolation::Step => 1,
        Interpolation::Cubic => 2,
    })?;
    w.u8(o.root_motion as u8)?;
    w.count(o.events.len())?;
    for e in &o.events {
        w.f64(e.time)?;
        w.string(&e.name)?;
        w.string(&e.payload)?;
    }
    w.count(o.morph_keys.len())?;
    for (name, keys) in &o.morph_keys {
        w.string(name)?;
        w.count(keys.len())?;
        for k in keys {
            w.f64(k.time)?;
            w.f64(k.weight)?;
        }
    }
    Ok(())
}
fn options_read(r: &mut Reader<'_>, l: &Limits) -> Result<ClipOptions> {
    let interpolation = match r.u8()? {
        0 => Interpolation::Linear,
        1 => Interpolation::Step,
        2 => Interpolation::Cubic,
        _ => return Err(Error::Corrupt("clip interpolation")),
    };
    let root_motion = flag(r)?;
    let mut events = Vec::new();
    let mut previous = 0.;
    for _ in 0..r.count(1024)? {
        let time = r.f64()?;
        let name = name_read(r, l)?;
        let payload = r.string(4096)?;
        if time < previous || time > l.max_clip_duration {
            return Err(Error::Corrupt("event time order"));
        }
        previous = time;
        events.push(ClipEvent {
            time,
            name,
            payload,
        });
    }
    let mut morph_keys = BTreeMap::new();
    let mut total = 0usize;
    for _ in 0..r.count(32)? {
        let name = name_read(r, l)?;
        if morph_keys.last_key_value().is_some_and(|(last, _)| last >= &name) {
            return Err(Error::Corrupt("morph animation order"));
        }
        let mut keys = Vec::new();
        let mut previous = -1f64;
        for _ in 0..r.count(l.max_keyframes.saturating_sub(total))? {
            let time = r.f64()?;
            let weight = r.f64()?;
            valid_weight(weight)?;
            if time < 0. || time > l.max_clip_duration || time as f32 <= previous as f32 {
                return Err(Error::Corrupt("morph key time order"));
            }
            previous = time;
            keys.push(MorphKey { time, weight });
            total += 1;
        }
        if keys.is_empty() || morph_keys.insert(name, keys).is_some() {
            return Err(Error::Corrupt("empty/duplicate morph keys"));
        }
    }
    Ok(ClipOptions {
        interpolation,
        root_motion,
        events,
        morph_keys,
    })
}
impl RigState {
    /// Direct canonical snapshot; no operation replay reconstructs hidden
    /// topology pins, rests, animation options or constraint state.
    pub(crate) fn write(&self, w: &mut Writer) -> Result<()> {
        w.count(self.rests.len())?;
        for (joint, t) in &self.rests {
            w.u32(*joint)?;
            transform_write(w, t)?;
        }
        w.count(self.poses.len())?;
        for (key, p) in &self.poses {
            if key != &p.name {
                return Err(Error::Invalid("pose map identity"));
            }
            pose_write(w, p)?;
        }
        w.count(self.constraints.len())?;
        for (key, c) in &self.constraints {
            if key != &c.name {
                return Err(Error::Invalid("constraint map identity"));
            }
            constraint_write(w, c)?;
        }
        w.count(self.morphs.len())?;
        for (key, m) in &self.morphs {
            if key != &m.name {
                return Err(Error::Invalid("morph map identity"));
            }
            w.string(&m.name)?;
            w.string(&m.object)?;
            w.raw(&m.topology)?;
            w.f64(m.weight)?;
            w.count(m.deltas.len())?;
            for (id, delta) in &m.deltas {
                w.u64(id.0)?;
                array_write(w, delta)?;
            }
        }
        w.count(self.clip_options.len())?;
        for (name, o) in &self.clip_options {
            w.string(name)?;
            options_write(w, o)?;
        }
        w.count(self.locked_groups.len())?;
        for (object, joints) in &self.locked_groups {
            w.string(object)?;
            w.count(joints.len())?;
            for j in joints {
                w.u32(*j)?;
            }
        }
        Ok(())
    }
    pub(crate) fn read(r: &mut Reader<'_>, l: &Limits) -> Result<Self> {
        let mut s = Self::default();
        for _ in 0..r.count(l.max_joints)? {
            let joint = r.u32()?;
            if joint as usize >= l.max_joints || s.rests.insert(joint, transform_read(r)?).is_some()
            {
                return Err(Error::Corrupt("rest joint"));
            }
        }
        for _ in 0..r.count(64)? {
            let p = pose_read(r, l)?;
            if s.poses.insert(p.name.clone(), p).is_some() {
                return Err(Error::Corrupt("duplicate pose"));
            }
            size(&s, l)?;
        }
        for _ in 0..r.count(128)? {
            let c = constraint_read(r, l)?;
            if s.constraints.insert(c.name.clone(), c).is_some() {
                return Err(Error::Corrupt("duplicate constraint"));
            }
            size(&s, l)?;
        }
        for _ in 0..r.count(32)? {
            let name = name_read(r, l)?;
            let object = name_read(r, l)?;
            let topology = r.raw(32)?.try_into().unwrap();
            let weight = r.f64()?;
            valid_weight(weight)?;
            let mut deltas = BTreeMap::new();
            for _ in 0..r.count(l.mesh.max_vertices)? {
                let id = mesh::VertexId(r.u64()?);
                let delta: [f64; 3] = array_read(r)?;
                if id.0 == 0
                    || delta.iter().any(|v| !(*v as f32).is_finite())
                    || deltas.insert(id, delta).is_some()
                {
                    return Err(Error::Corrupt("morph vertex/delta"));
                }
            }
            let m = MorphTarget {
                name: name.clone(),
                object,
                topology,
                deltas,
                weight,
            };
            if s.morphs.insert(name, m).is_some() {
                return Err(Error::Corrupt("duplicate morph"));
            }
            size(&s, l)?;
        }
        for _ in 0..r.count(l.max_clips)? {
            let name = name_read(r, l)?;
            if s.clip_options.insert(name, options_read(r, l)?).is_some() {
                return Err(Error::Corrupt("duplicate clip options"));
            }
            size(&s, l)?;
        }
        for _ in 0..r.count(l.max_objects)? {
            let object = name_read(r, l)?;
            let mut joints = BTreeSet::new();
            for _ in 0..r.count(l.max_joints)? {
                let j = r.u32()?;
                if j as usize >= l.max_joints || !joints.insert(j) {
                    return Err(Error::Corrupt("locked joint"));
                }
            }
            if s.locked_groups.insert(object, joints).is_some() {
                return Err(Error::Corrupt("duplicate locked object"));
            }
        }
        size(&s, l)?;
        Ok(s)
    }
}
fn size(s: &RigState, l: &Limits) -> Result<()> {
    if s.memory_bytes() > l.max_source_bytes || s.memory_bytes() > l.mesh.max_bytes {
        Err(Error::Budget("rig decoded bytes"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn roundtrip(op: &RigOperation, l: &Limits) {
        op.check(l).unwrap();
        let mut w = Writer::new(usize::MAX); op.write(&mut w).unwrap();
        let mut r = Reader::new(&w.bytes); let decoded = RigOperation::read(&mut r, l).unwrap(); r.end().unwrap();
        assert_eq!(&decoded, op);
        let mut out = Writer::new(usize::MAX); decoded.write(&mut out).unwrap(); assert_eq!(out.bytes, w.bytes);
        for cut in [0, 1, w.bytes.len() - 1] { assert!(RigOperation::read(&mut Reader::new(&w.bytes[..cut]), l).is_err()); }
    }
    fn options() -> ClipOptions {
        ClipOptions {
            interpolation: Interpolation::Cubic, root_motion: true,
            events: vec![ClipEvent { time: 0.25, name: "step".into(), payload: "left\nfoot".into() }],
            morph_keys: BTreeMap::from([("smile".into(), vec![MorphKey { time: 0., weight: -0.5 }, MorphKey { time: 1., weight: 1. }])]),
        }
    }
    #[test]
    fn literal_operations_cover_every_tag_and_nested_variant() {
        let l = Limits::default();
        let ops = vec![
            RigOperation::MirrorJoints { joints: vec![1, 2], axis: 0, names: BTreeMap::from([(1, "arm_r".into()), (2, "hand_r".into())]) },
            RigOperation::Rest { joint: 1, transform: Transform { translation: [1., 2., 3.], ..Default::default() } },
            RigOperation::RenameJoint { joint: 1, name: "arm".into() },
            RigOperation::ReparentJoint { joint: 1, parent: Some(0) },
            RigOperation::Roll { joint: 1, angle: 0.5 },
            RigOperation::Pose(Pose { name: "ready".into(), joints: BTreeMap::from([(1, Transform::default())]) }),
            RigOperation::DeletePose { name: "ready".into() },
            RigOperation::Constraint(RigConstraint { name: "copy".into(), joint: 1, enabled: false, kind: ConstraintKind::Copy { target: 0, translation: true, rotation: false, scale: true, influence: 0.5 } }),
            RigOperation::DeleteConstraint { name: "copy".into() },
            RigOperation::SolvePose { pose: "ready".into() },
            RigOperation::Ik { pose: "ready".into(), root: 0, middle: 1, end: 2, target: [1., 2., 0.], pole: [0., 0., 1.], clamp_reach: true },
            RigOperation::BakeClip { name: "walk".into(), poses: vec![(0., "ready".into()), (1., "step".into())], fps: 30., solve_constraints: true },
            RigOperation::BlendClip { name: "blend".into(), a: "walk".into(), b: "run".into(), weight: 0.5, fps: 24. },
            RigOperation::RetargetClip { name: "retarget".into(), source: "walk".into(), joints: BTreeMap::from([(0, 1), (1, 2)]), translation_scale: 2. },
            RigOperation::ClipOptions { name: "walk".into(), options: options() },
            RigOperation::WeightLocks { object: "body".into(), joints: BTreeSet::from([0, 2]) },
            RigOperation::Weights { object: "body".into(), vertices: vec![mesh::VertexId(u64::MAX), mesh::VertexId(1)], edit: WeightEdit::Assign { joint: 1 } },
            RigOperation::Morph { name: "smile".into(), object: "body".into(), deltas: BTreeMap::from([(mesh::VertexId(u64::MAX), [0.1, 0.2, 0.3])]), weight: -0.5 },
            RigOperation::CaptureMorph { name: "smile".into(), object: "body".into(), target: "target".into(), weight: 1. },
            RigOperation::DeleteMorph { name: "smile".into() },
            RigOperation::MorphWeight { name: "smile".into(), weight: 0.25 },
        ];
        for (tag, op) in ops.iter().enumerate() {
            roundtrip(op, &l); let mut w = Writer::new(usize::MAX); op.write(&mut w).unwrap(); assert_eq!(w.bytes[0], tag as u8);
        }
        for edit in [
            WeightEdit::Normalize { max_influences: 4 }, WeightEdit::Smooth { iterations: 2, factor: 0.5 },
            WeightEdit::Mirror { axis: 2, joints: BTreeMap::from([(0, 1), (1, 0)]), tolerance: 0.001 },
            WeightEdit::Transfer { source: "source".into(), max_distance: 1. }, WeightEdit::Bind { max_influences: 4, power: 2. },
        ] { roundtrip(&RigOperation::Weights { object: "body".into(), vertices: vec![], edit }, &l); }
        for kind in [
            ConstraintKind::Aim { target: 0, axis: [0., 2., 0.], influence: 1. },
            ConstraintKind::Limit { min_translation: [-1.; 3], max_translation: [1.; 3], min_scale: [0.1; 3], max_scale: [2.; 3], max_angle: 1. },
            ConstraintKind::TwoBoneIk { middle: 2, end: 3, target: 0, pole: [1., 0., 0.], clamp_reach: false },
        ] { roundtrip(&RigOperation::Constraint(RigConstraint { name: "constraint".into(), joint: 1, enabled: true, kind }), &l); }
        roundtrip(&RigOperation::ReparentJoint { joint: 1, parent: None }, &l);
        for interpolation in [Interpolation::Linear, Interpolation::Step] {
            roundtrip(&RigOperation::ClipOptions { name: "walk".into(), options: ClipOptions { interpolation, ..Default::default() } }, &l);
        }
    }
    #[test]
    fn typed_checks_keep_defaults_normalization_and_limits() {
        let l = Limits::default();
        for op in [
            RigOperation::MirrorJoints { joints: vec![1, 2], axis: 0, names: BTreeMap::from([(1, "a".into()), (2, "b".into())]) },
            RigOperation::Weights { object: "body".into(), vertices: vec![], edit: WeightEdit::Smooth { iterations: 1, factor: 0.5 } },
            RigOperation::ClipOptions { name: "walk".into(), options: ClipOptions::default() },
            RigOperation::Rest { joint: 0, transform: Transform::default() },
        ] { roundtrip(&op, &l); }
        for op in [
            RigOperation::MirrorJoints { joints: vec![2, 1], axis: 0, names: BTreeMap::from([(1, "a".into()), (2, "b".into())]) },
            RigOperation::Roll { joint: l.max_joints as u32, angle: 0. },
            RigOperation::Roll { joint: 0, angle: f64::NAN },
            RigOperation::RenameJoint { joint: 0, name: "bad\nname".into() },
            RigOperation::Rest { joint: 0, transform: Transform { scale: [0.; 3], ..Default::default() } },
            RigOperation::BakeClip { name: "walk".into(), poses: vec![(0., "a".into()), (0., "b".into())], fps: 30., solve_constraints: true },
            RigOperation::BlendClip { name: "blend".into(), a: "a".into(), b: "b".into(), weight: 2., fps: 30. },
            RigOperation::Morph { name: "shape".into(), object: "body".into(), deltas: BTreeMap::from([(mesh::VertexId(0), [0.; 3])]), weight: 0. },
            RigOperation::MorphWeight { name: "shape".into(), weight: 3. },
            RigOperation::ClipOptions { name: "walk".into(), options: ClipOptions { events: vec![ClipEvent { time: -1., name: "event".into(), payload: String::new() }], ..Default::default() } },
        ] { assert!(op.check(&l).is_err()); }
        for edit in [
            WeightEdit::Assign { joint: l.max_joints as u32 }, WeightEdit::Normalize { max_influences: 0 },
            WeightEdit::Smooth { iterations: 65, factor: 0.5 }, WeightEdit::Smooth { iterations: 1, factor: 2. },
            WeightEdit::Mirror { axis: 3, joints: BTreeMap::new(), tolerance: 0.1 },
            WeightEdit::Transfer { source: "source".into(), max_distance: 0. }, WeightEdit::Bind { max_influences: 4, power: 9. },
        ] { assert!(RigOperation::Weights { object: "body".into(), vertices: vec![], edit }.check(&l).is_err()); }
        for vertices in [vec![mesh::VertexId(0)], vec![mesh::VertexId(1); 2]] {
            assert!(RigOperation::Weights { object: "body".into(), vertices, edit: WeightEdit::Assign { joint: 0 } }.check(&l).is_err());
        }
        let op = RigOperation::ClipOptions { name: "walk".into(), options: options() };
        let tiny = Limits { max_keyframes: 1, ..l.clone() }; assert!(op.check(&tiny).is_err());
        let tiny = Limits { max_transaction_bytes: 1, ..l.clone() }; assert!(op.check(&tiny).is_err());
        assert!(matches!(RigOperation::read(&mut Reader::new(&[255]), &l), Err(Error::Corrupt(_))));
        let mut w = Writer::new(32); w.u8(3).unwrap(); w.u32(1).unwrap(); w.u8(2).unwrap();
        assert!(matches!(RigOperation::read(&mut Reader::new(&w.bytes), &l), Err(Error::Corrupt(_))));
    }
    #[test]
    fn direct_rig_snapshot_preserves_topology_and_every_control() {
        let l = Limits::default();
        let mut s = RigState::default();
        let t = Transform {
            translation: [1., 2., 3.],
            rotation: quat_axis_angle([1., 0., 0.], 0.4).unwrap(),
            scale: [2., 2., 2.],
        };
        s.rests.insert(1, t);
        s.poses.insert(
            "pose".into(),
            Pose {
                name: "pose".into(),
                joints: BTreeMap::from([(1, t)]),
            },
        );
        for (name, joint, kind) in [
            (
                "copy",
                1,
                ConstraintKind::Copy {
                    target: 0,
                    translation: true,
                    rotation: false,
                    scale: true,
                    influence: 0.4,
                },
            ),
            (
                "aim",
                2,
                ConstraintKind::Aim {
                    target: 0,
                    axis: [0., 1., 0.],
                    influence: 0.7,
                },
            ),
            (
                "limit",
                3,
                ConstraintKind::Limit {
                    min_translation: [-1.; 3],
                    max_translation: [1.; 3],
                    min_scale: [0.1; 3],
                    max_scale: [2.; 3],
                    max_angle: 1.,
                },
            ),
            (
                "ik",
                4,
                ConstraintKind::TwoBoneIk {
                    middle: 5,
                    end: 6,
                    target: 0,
                    pole: [1., 0., 0.],
                    clamp_reach: true,
                },
            ),
        ] {
            s.constraints.insert(
                name.into(),
                RigConstraint {
                    name: name.into(),
                    joint,
                    enabled: false,
                    kind,
                },
            );
        }
        s.morphs.insert(
            "shape".into(),
            MorphTarget {
                name: "shape".into(),
                object: "body".into(),
                topology: std::array::from_fn(|i| (i * 7) as u8),
                deltas: BTreeMap::from([(mesh::VertexId(3), [0.1, 0.2, 0.3])]),
                weight: -0.5,
            },
        );
        s.clip_options.insert(
            "walk".into(),
            ClipOptions {
                interpolation: Interpolation::Cubic,
                root_motion: true,
                events: vec![ClipEvent {
                    time: 0.25,
                    name: "step".into(),
                    payload: "left\nfoot".into(),
                }],
                morph_keys: BTreeMap::from([(
                    "shape".into(),
                    vec![
                        MorphKey {
                            time: 0.,
                            weight: 0.,
                        },
                        MorphKey {
                            time: 1.,
                            weight: 1.,
                        },
                    ],
                )]),
            },
        );
        s.locked_groups
            .insert("body".into(), BTreeSet::from([0, 2]));
        let mut w = Writer::new(l.max_source_bytes);
        s.write(&mut w).unwrap();
        let bytes = w.bytes;
        let mut reader = Reader::new(&bytes);
        let decoded = RigState::read(&mut reader, &l).unwrap();
        reader.end().unwrap();
        assert_eq!(decoded, s);
        let mut out = Writer::new(l.max_source_bytes);
        decoded.write(&mut out).unwrap();
        assert_eq!(out.bytes, bytes);
        for cut in [0, 1, 4, bytes.len() / 2, bytes.len() - 1] {
            assert!(RigState::read(&mut Reader::new(&bytes[..cut]), &l).is_err());
        }
        let mut tiny = l;
        tiny.max_source_bytes = 128;
        assert!(RigState::read(&mut Reader::new(&bytes), &tiny).is_err());
    }
    #[test]
    fn malformed_rig_boolean_tag_duplicates_are_rejected() {
        let l = Limits::default();
        let mut w = Writer::new(2048);
        w.count(0).unwrap();
        w.count(0).unwrap();
        w.count(1).unwrap();
        w.string("bad").unwrap();
        w.u32(1).unwrap();
        w.u8(2).unwrap();
        assert!(RigState::read(&mut Reader::new(&w.bytes), &l).is_err());
        let mut w = Writer::new(2048);
        w.count(2).unwrap();
        for _ in 0..2 {
            w.u32(0).unwrap();
            transform_write(&mut w, &Transform::default()).unwrap();
        }
        assert!(RigState::read(&mut Reader::new(&w.bytes), &l).is_err());
    }
}
