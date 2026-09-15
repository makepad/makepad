use super::*;
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
