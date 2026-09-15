use makepad_model::transform::*;
use makepad_model::*;
use std::collections::{BTreeMap, BTreeSet};
fn skeleton() -> Skeleton {
    Skeleton {
        joints: vec![
            Joint {
                name: "root".into(),
                parent: None,
                translation: [0.; 3],
            },
            Joint {
                name: "middle".into(),
                parent: Some(0),
                translation: [0., 1., 0.],
            },
            Joint {
                name: "end".into(),
                parent: Some(1),
                translation: [0., 1., 0.],
            },
        ],
    }
}
fn points(local: &[Transform]) -> Vec<[f64; 3]> {
    let mut world = IDENTITY_MATRIX;
    local
        .iter()
        .map(|t| {
            world = matrix_mul(world, t.matrix().unwrap());
            transform_point(world, [0.; 3])
        })
        .collect()
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b) * (a - b))
        .sum::<f64>()
        .sqrt()
}
fn apply(d: &mut Document, name: &str, operations: Vec<Operation>) {
    d.apply(
        Transaction {
            request_id: name.into(),
            expected: d.head(),
            operations,
        },
        None,
    )
    .unwrap();
}
#[test]
fn analytic_ik_reaches_target_preserves_lengths_and_refuses_unreachable_chain() {
    let skel = skeleton();
    let mut local = skel
        .joints
        .iter()
        .map(|j| Transform {
            translation: j.translation,
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let target = [1., 1., 0.];
    solve_ik(&skel, &mut local, 0, 1, 2, target, [0., 0., 1.], false).unwrap();
    let p = points(&local);
    assert!(distance(p[2], target) < 1e-7);
    assert!((distance(p[0], p[1]) - 1.).abs() < 1e-8);
    assert!((distance(p[1], p[2]) - 1.).abs() < 1e-8);
    let before = local.clone();
    assert!(solve_ik(
        &skel,
        &mut local,
        0,
        1,
        2,
        [4., 4., 0.],
        [0., 0., 1.],
        false
    )
    .is_err());
    assert_eq!(local, before);
    solve_ik(&skel, &mut local, 0, 1, 2, [4., 4., 0.], [0., 0., 1.], true).unwrap();
    let p = points(&local);
    assert!((distance(p[0], p[2]) - 2.).abs() < 1e-6);
    let mut d = Document::new(Limits::default()).unwrap();
    apply(
        &mut d,
        "skeleton",
        vec![
            Operation::SetSkeleton { skeleton: skel },
            Operation::Rig(RigOperation::Pose(Pose {
                name: "rest".into(),
                joints: BTreeMap::new(),
            })),
        ],
    );
    let source = d.to_bytes(None).unwrap();
    let bad = Transaction {
        request_id: "bad-ik".into(),
        expected: d.head(),
        operations: vec![Operation::Rig(RigOperation::Ik {
            pose: "rest".into(),
            root: 0,
            middle: 1,
            end: 2,
            target: [0., 8., 0.],
            pole: [0., 0., 1.],
            clamp_reach: false,
        })],
    };
    assert!(d.apply(bad, None).is_err());
    assert_eq!(d.to_bytes(None).unwrap(), source);
}
#[test]
fn locked_weights_survive_smoothing_binding_and_limit_failure() {
    let mut d = Document::new(Limits::default()).unwrap();
    apply(
        &mut d,
        "body",
        vec![
            Operation::Cube {
                object: "body".into(),
                size: [1.; 3],
            },
            Operation::SetSkeleton {
                skeleton: skeleton(),
            },
        ],
    );
    let vertices = d
        .object("body")
        .unwrap()
        .vertices()
        .iter()
        .map(|v| v.id)
        .collect::<Vec<_>>();
    let weights = vertices
        .iter()
        .map(|vertex| Operation::SetWeights {
            object: "body".into(),
            vertex: *vertex,
            weights: vec![
                mesh::JointWeight {
                    joint: 0,
                    weight: 0.25,
                },
                mesh::JointWeight {
                    joint: 1,
                    weight: 0.25,
                },
                mesh::JointWeight {
                    joint: 2,
                    weight: 0.5,
                },
            ],
        })
        .collect();
    apply(&mut d, "weights", weights);
    apply(
        &mut d,
        "locks",
        vec![
            Operation::Rig(RigOperation::WeightLocks {
                object: "body".into(),
                joints: BTreeSet::from([0]),
            }),
            Operation::Rig(RigOperation::Weights {
                object: "body".into(),
                vertices: vec![],
                edit: WeightEdit::Smooth {
                    iterations: 2,
                    factor: 0.8,
                },
            }),
            Operation::Rig(RigOperation::Weights {
                object: "body".into(),
                vertices: vec![],
                edit: WeightEdit::Bind {
                    max_influences: 3,
                    power: 2.,
                },
            }),
        ],
    );
    for v in d.object("body").unwrap().vertices() {
        assert!((v.weights.iter().find(|w| w.joint == 0).unwrap().weight - 0.25).abs() < 1e-12);
        assert!((v.weights.iter().map(|w| w.weight).sum::<f64>() - 1.).abs() < 1e-12);
    }
    let source = d.to_bytes(None).unwrap();
    let failed = Transaction {
        request_id: "too-few".into(),
        expected: d.head(),
        operations: vec![Operation::Rig(RigOperation::Weights {
            object: "body".into(),
            vertices: vec![],
            edit: WeightEdit::Normalize { max_influences: 1 },
        })],
    };
    assert!(d.apply(failed, None).is_err());
    assert_eq!(source, d.to_bytes(None).unwrap());
    let opened = Document::from_bytes(&source, Limits::default(), None).unwrap();
    assert_eq!(opened.rig(), d.rig());
    assert_eq!(opened.to_bytes(None).unwrap(), source);
}
#[test]
fn parsed_controls_morph_topology_constraints_and_clip_options_roundtrip() {
    let mut d = Document::new(Limits::default()).unwrap();
    apply(
        &mut d,
        "body",
        vec![
            Operation::Cube {
                object: "body".into(),
                size: [1.; 3],
            },
            Operation::SetSkeleton {
                skeleton: skeleton(),
            },
            Operation::AutoWeights {
                object: "body".into(),
            },
        ],
    );
    let id = d.object("body").unwrap().vertices()[0].id.0;
    let commands = format!(
        r#"[{{"op":"rig_rest","joint":0,"transform":{{"rotation":[0,0,0,1]}}}},{{"op":"pose","name":"rest","joints":[]}},{{"op":"pose","name":"raised","joints":[{{"joint":0,"transform":{{"translation":[0,0.5,0]}}}}]}},{{"op":"bake_clip","name":"bounce","poses":[{{"time":0,"pose":"rest"}},{{"time":1,"pose":"raised"}}],"fps":4}},{{"op":"morph","name":"shape","object":"body","deltas":[{{"vertex":"{id}","delta":[0.1,0,0]}}]}},{{"op":"clip_options","name":"bounce","interpolation":"cubic","root_motion":true,"events":[{{"time":0.5,"name":"peak","payload":"hello"}}],"morph_keys":[{{"morph":"shape","keys":[{{"time":0,"weight":0}},{{"time":1,"weight":1}}]}}]}}]"#
    );
    let ops = parse_operations(&json::parse(commands.as_bytes()).unwrap(), d.limits()).unwrap();
    apply(&mut d, "controls", ops);
    let source = d.to_bytes(None).unwrap();
    let reopened = Document::from_bytes(&source, Limits::default(), None).unwrap();
    assert_eq!(reopened.rig(), d.rig());
    assert_eq!(reopened.to_bytes(None).unwrap(), source);
    assert_ne!(d.rig().morphs["shape"].topology, [0; 32]);
    assert_eq!(d.rig().clip_options["bounce"].events[0].payload, "hello");
    let face = d.object("body").unwrap().faces()[0].id;
    let changed = Transaction {
        request_id: "invalid-topology".into(),
        expected: d.head(),
        operations: vec![Operation::DeleteFaces {
            object: "body".into(),
            faces: vec![face],
        }],
    };
    assert!(d.apply(changed, None).is_err());
    assert_eq!(d.to_bytes(None).unwrap(), source);
    let cycle = Transaction {
        request_id: "cycle".into(),
        expected: d.head(),
        operations: vec![
            Operation::Rig(RigOperation::Constraint(RigConstraint {
                name: "a".into(),
                joint: 0,
                enabled: true,
                kind: ConstraintKind::Copy {
                    target: 1,
                    translation: true,
                    rotation: false,
                    scale: false,
                    influence: 1.,
                },
            })),
            Operation::Rig(RigOperation::Constraint(RigConstraint {
                name: "b".into(),
                joint: 1,
                enabled: true,
                kind: ConstraintKind::Copy {
                    target: 0,
                    translation: true,
                    rotation: false,
                    scale: false,
                    influence: 1.,
                },
            })),
        ],
    };
    assert!(d.apply(cycle, None).is_err());
    assert_eq!(d.to_bytes(None).unwrap(), source);
}
#[test]
fn mirrored_bones_preserve_existing_indices_weights_and_world_rest_symmetry(){
    let mut d=Document::new(Limits::default()).unwrap();let skel=Skeleton{joints:vec![Joint{name:"center".into(),parent:None,translation:[0.;3]},Joint{name:"left".into(),parent:Some(0),translation:[1.,0.,0.]},Joint{name:"left_tip".into(),parent:Some(1),translation:[0.,1.,0.]}]};apply(&mut d,"base",vec![Operation::Cube{object:"body".into(),size:[1.;3]},Operation::SetSkeleton{skeleton:skel},Operation::AutoWeights{object:"body".into()},Operation::Rig(RigOperation::Rest{joint:1,transform:Transform{translation:[1.,0.,0.],rotation:quat_axis_angle([0.,0.,1.],0.4).unwrap(),scale:[1.;3]}})]);let before_mesh=d.object("body").unwrap().clone();let before_skeleton=d.skeleton().unwrap().clone();let before_world=d.rig().global_rest(d.skeleton().unwrap()).unwrap();
    let mirror=RigOperation::MirrorJoints{joints:vec![1,2],axis:0,names:BTreeMap::from([(1,"right".into()),(2,"right_tip".into())])};let json=mirror.value();assert_eq!(RigOperation::parse(&json,d.limits()).unwrap().unwrap(),mirror);apply(&mut d,"mirror",vec![Operation::Rig(mirror)]);let after=d.skeleton().unwrap();assert_eq!(&after.joints[..3],&before_skeleton.joints);assert_eq!(after.joints[3].parent,Some(0));assert_eq!(after.joints[4].parent,Some(3));assert_eq!(d.object("body").unwrap(),&before_mesh);
    let world=d.rig().global_rest(after).unwrap();let mut f=IDENTITY_MATRIX;f[0][0]= -1.;for(a,b)in [(1,3),(2,4)]{let expected=matrix_mul(matrix_mul(f,before_world[a]),f);for i in 0..4{for j in 0..4{assert!((world[b][i][j]-expected[i][j]).abs()<1e-8);}}}let source=d.to_bytes(None).unwrap();assert_eq!(Document::from_bytes(&source,Limits::default(),None).unwrap().to_bytes(None).unwrap(),source);
    let bad=Transaction{request_id:"collision".into(),expected:d.head(),operations:vec![Operation::Rig(RigOperation::MirrorJoints{joints:vec![1],axis:0,names:BTreeMap::from([(1,"right".into())])})]};assert!(d.apply(bad,None).is_err());assert_eq!(d.to_bytes(None).unwrap(),source);
}
#[test]
fn mirror_refuses_local_shear_and_constraint_order_includes_target_ancestors(){
    let mut d=Document::new(Limits::default()).unwrap();let skel=Skeleton{joints:vec![Joint{name:"root".into(),parent:None,translation:[0.;3]},Joint{name:"target_parent".into(),parent:Some(0),translation:[0.;3]},Joint{name:"target".into(),parent:Some(1),translation:[0.,1.,0.]},Joint{name:"aim".into(),parent:Some(0),translation:[0.;3]}]};apply(&mut d,"setup",vec![Operation::SetSkeleton{skeleton:skel},Operation::Rig(RigOperation::Pose(Pose{name:"p".into(),joints:BTreeMap::new()})),Operation::Rig(RigOperation::Constraint(RigConstraint{name:"a_aim".into(),joint:3,enabled:true,kind:ConstraintKind::Aim{target:2,axis:[0.,1.,0.],influence:1.}})),Operation::Rig(RigOperation::Constraint(RigConstraint{name:"z_parent".into(),joint:1,enabled:true,kind:ConstraintKind::Limit{min_translation:[2.,0.,0.],max_translation:[2.,0.,0.],min_scale:[1.;3],max_scale:[1.;3],max_angle:std::f64::consts::PI}})),Operation::Rig(RigOperation::SolvePose{pose:"p".into()})]);let rotation=d.rig().poses["p"].joints[&3].rotation;let facing=quat_rotate(rotation,[0.,1.,0.]);assert!(distance(facing,normalized([2.,1.,0.]).unwrap())<1e-8);
    apply(&mut d,"scaled-root",vec![Operation::Rig(RigOperation::Rest{joint:0,transform:Transform{rotation:quat_axis_angle([0.,0.,1.],0.4).unwrap(),scale:[2.,1.,1.],..Default::default()}})]);let source=d.to_bytes(None).unwrap();let tx=Transaction{request_id:"shear".into(),expected:d.head(),operations:vec![Operation::Rig(RigOperation::MirrorJoints{joints:vec![1],axis:0,names:BTreeMap::from([(1,"mirrored".into())])})]};assert!(d.apply(tx,None).is_err());assert_eq!(d.to_bytes(None).unwrap(),source);
}
#[test]
fn public_ik_bad_hierarchy_and_overlapping_constraint_writers_fail_atomically(){
    let mut skel=skeleton();let mut local=vec![Transform::default();3];let before=local.clone();skel.joints[0].parent=Some(1);assert!(solve_ik(&skel,&mut local,0,1,2,[1.,1.,0.],[0.,0.,1.],false).is_err());assert_eq!(local,before);
    let mut d=Document::new(Limits::default()).unwrap();let mut skel=skeleton();skel.joints.push(Joint{name:"target".into(),parent:Some(0),translation:[1.,1.,0.]});apply(&mut d,"base",vec![Operation::SetSkeleton{skeleton:skel}]);let source=d.to_bytes(None).unwrap();let tx=Transaction{request_id:"overlap".into(),expected:d.head(),operations:vec![Operation::Rig(RigOperation::Constraint(RigConstraint{name:"ik".into(),joint:0,enabled:true,kind:ConstraintKind::TwoBoneIk{middle:1,end:2,target:3,pole:[0.,0.,1.],clamp_reach:false}})),Operation::Rig(RigOperation::Constraint(RigConstraint{name:"middle_limit".into(),joint:1,enabled:true,kind:ConstraintKind::Limit{min_translation:[-2.;3],max_translation:[2.;3],min_scale:[1.;3],max_scale:[1.;3],max_angle:1.}}))]};assert!(d.apply(tx,None).is_err());assert_eq!(d.to_bytes(None).unwrap(),source);
}
