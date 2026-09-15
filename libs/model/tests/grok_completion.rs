//! Independent Grok completion-lane adversarial tests.
//! Public API only. Does not mirror production unit tests.
use makepad_gltf::JsonValue;
use makepad_model::json;
use makepad_model::transform::*;
use makepad_model::*;
use std::cell::Cell;
use std::collections::BTreeMap;

const V1_ROBOT: &[u8] = include_bytes!("fixtures/v1_robot.mpmodel");

fn apply(doc: &mut Document, id: &str, operations: Vec<Operation>) -> Applied {
    doc.apply(
        Transaction {
            request_id: id.into(),
            expected: doc.head(),
            operations,
        },
        None,
    )
    .unwrap()
}

fn apply_json(doc: &mut Document, id: &str, text: &str) -> Applied {
    let operations = parse_operations(&json::parse(text.as_bytes()).unwrap(), doc.limits()).unwrap();
    apply(doc, id, operations)
}

fn try_apply(doc: &mut Document, id: &str, operations: Vec<Operation>) -> Result<Applied> {
    doc.apply(
        Transaction {
            request_id: id.into(),
            expected: doc.head(),
            operations,
        },
        None,
    )
}

fn try_json(doc: &mut Document, id: &str, text: &str) -> Result<Applied> {
    let operations = parse_operations(&json::parse(text.as_bytes()).unwrap(), doc.limits())?;
    try_apply(doc, id, operations)
}

fn snap(doc: &Document) -> (Head, Vec<u8>) {
    (doc.head(), doc.to_bytes(None).unwrap())
}

fn assert_unchanged(doc: &Document, before: &(Head, Vec<u8>)) {
    assert_eq!(doc.head(), before.0, "head moved after rejected edit");
    assert_eq!(
        doc.to_bytes(None).unwrap(),
        before.1,
        "source bytes moved after rejected edit"
    );
}

fn cube(object: &str, size: [f64; 3]) -> Operation {
    Operation::Cube {
        object: object.into(),
        size,
    }
}

fn chain() -> Skeleton {
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

fn arm() -> Skeleton {
    Skeleton {
        joints: vec![
            Joint {
                name: "hips".into(),
                parent: None,
                translation: [0.; 3],
            },
            Joint {
                name: "limb".into(),
                parent: Some(0),
                translation: [0.; 3],
            },
            Joint {
                name: "middle".into(),
                parent: Some(1),
                translation: [0., 1., 0.],
            },
            Joint {
                name: "end".into(),
                parent: Some(2),
                translation: [0., 1., 0.],
            },
            Joint {
                name: "handle".into(),
                parent: Some(0),
                translation: [1., 1., 0.],
            },
        ],
    }
}

fn field<'a>(v: &'a JsonValue, key: &str) -> &'a JsonValue {
    let JsonValue::Object(map) = v else {
        panic!("expected object for {key}, got {v:?}");
    };
    &map[key]
}

fn arr(v: &JsonValue) -> &[JsonValue] {
    let JsonValue::Array(v) = v else {
        panic!("expected array, got {v:?}");
    };
    v
}

fn idx(v: &JsonValue) -> usize {
    match v {
        JsonValue::U64(i) => *i as usize,
        JsonValue::I64(i) => *i as usize,
        _ => panic!("expected index, got {v:?}"),
    }
}

fn f32s(glb: &makepad_gltf::ParsedGlb, accessor: usize, lanes: usize) -> Vec<f32> {
    let a = &glb.document.accessors_slice()[accessor];
    let view = &glb.document.buffer_views_slice()[a.buffer_view.unwrap()];
    let at = view.byte_offset.unwrap_or(0) + a.byte_offset.unwrap_or(0);
    let bin = glb.bin_chunk.as_ref().unwrap();
    bin[at..at + a.count * lanes * 4]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

fn lights_ext(glb: &makepad_gltf::ParsedGlb) -> &[JsonValue] {
    arr(field(
        field(glb.document.extensions.as_ref().unwrap(), "KHR_lights_punctual"),
        "lights",
    ))
}

#[test]
fn modifier_reference_graph_instances_disabled_and_cycles() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, "seed", vec![cube("a", [2.; 3])]);
    apply_json(
        &mut doc,
        "links",
        r#"[
            {"op":"instance","object":"b","source":"a","transform":{"translation":[6,0,0]}},
            {"op":"instance","object":"c","source":"b","transform":{"translation":[-6,0,0]}},
            {"op":"modifier","object":"a","name":"copies","operation":{"op":"array","count":2,"offset":[3,0,0]}}
        ]"#,
    );
    assert_eq!(doc.object("a").unwrap().vertices().len(), 8);
    assert_eq!(doc.object("b").unwrap().faces().len(), 0);
    assert_eq!(doc.object("c").unwrap().faces().len(), 0);
    assert_eq!(doc.scene().nodes["b"].linked_to.as_deref(), Some("a"));
    assert_eq!(doc.scene().nodes["c"].linked_to.as_deref(), Some("b"));

    let compiled = doc.compile(None).unwrap();
    // Source plus two linked instances share the arrayed cage.
    assert_eq!(compiled.triangles, 12 * 2 * 3);
    assert!(compiled.bounds[1][0] > 7.9);
    assert!(compiled.bounds[0][0] < -5.9);
    let before = snap(&doc);
    assert_eq!(doc.compile(None).unwrap().glb, compiled.glb);
    assert_unchanged(&doc, &before);

    apply_json(
        &mut doc,
        "disabled-wrap",
        r#"[{"op":"modifier","object":"a","name":"wrap","enabled":false,"operation":{"op":"shrinkwrap","vertices":[],"reference":"c","max_distance":10,"offset":0}}]"#,
    );
    assert!(!doc.scene().modifiers["a"]
        .iter()
        .find(|m| m.name == "wrap")
        .unwrap()
        .enabled);
    assert_eq!(doc.compile(None).unwrap().triangles, compiled.triangles);

    let before = snap(&doc);
    let err = try_json(
        &mut doc,
        "live-wrap",
        r#"[{"op":"modifier","object":"a","name":"wrap","enabled":true,"operation":{"op":"shrinkwrap","vertices":[],"reference":"c","max_distance":10,"offset":0}}]"#,
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::Invalid(_)),
        "enabled wrap through the instance chain must be an explicit cycle, got {err:?}"
    );
    assert_unchanged(&doc, &before);

    let err = try_json(
        &mut doc,
        "fixed-selection",
        r#"[{"op":"modifier","object":"a","name":"bevel","operation":{"op":"bevel_edges","edges":[["1","2"]],"width":0.1}}]"#,
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::Invalid(_)),
        "modifiers may not pin element IDs, got {err:?}"
    );

    apply_json(&mut doc, "bake-c", r#"[{"op":"apply_modifiers","object":"c"}]"#);
    assert!(doc.scene().nodes["c"].linked_to.is_none());
    assert_eq!(doc.object("c").unwrap().vertices().len(), 16);
    apply_json(&mut doc, "d", r#"[{"op":"cube","object":"d","size":[1,1,1]}]"#);
    apply_json(
        &mut doc,
        "wrap-d",
        r#"[{"op":"modifier","object":"d","name":"to-a","operation":{"op":"shrinkwrap","vertices":[],"reference":"a","max_distance":20,"offset":0}}]"#,
    );
    let product = doc.compile(None).unwrap();
    assert!(product.triangles > 0);
    apply_json(&mut doc, "order", r#"[{"op":"modifier_order","object":"a","names":["wrap","copies"]}]"#);
    let before = snap(&doc);
    let err = try_json(
        &mut doc,
        "bad-order",
        r#"[{"op":"modifier_order","object":"a","names":["copies"]}]"#,
    )
    .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert_unchanged(&doc, &before);
}

#[test]
fn v1_source_migrates_then_accepts_v2_authoring() {
    assert_eq!(&V1_ROBOT[..8], b"MPMODEL\0");
    assert_eq!(u32::from_le_bytes(V1_ROBOT[8..12].try_into().unwrap()), 1);
    let calls = Cell::new(0u32);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() > 1
    };
    let cancelled = Document::from_bytes(V1_ROBOT, Limits::default(), Some(&cancel));
    assert!(
        matches!(cancelled, Err(Error::Mesh(mesh::MeshError::Cancelled))),
        "v1 decode must honor cancellation, got {cancelled:?}"
    );

    let mut doc = Document::from_bytes(V1_ROBOT, Limits::default(), None).unwrap();
    assert_eq!(
        doc.head().generation,
        u64::from_le_bytes(V1_ROBOT[12..20].try_into().unwrap())
    );
    assert_eq!(&doc.head().content, &V1_ROBOT[20..52]);
    let v1_glb = doc.compile(None).unwrap().glb;
    assert!(v1_glb.starts_with(b"glTF"));

    let upgraded = doc.to_bytes(None).unwrap();
    assert_eq!(u32::from_le_bytes(upgraded[8..12].try_into().unwrap()), 2);
    let round = Document::from_bytes(&upgraded, Limits::default(), None).unwrap();
    assert_eq!(round.head(), doc.head());
    assert_eq!(round.compile(None).unwrap().glb, v1_glb);

    let object = doc.objects().next().unwrap().0.to_owned();
    apply(
        &mut doc,
        "v2-light",
        vec![Operation::Scene(SceneOperation::Light(LightEmitter {
            name: "probe".into(),
            attachment: Attachment::Object(object.clone()),
            transform: Transform {
                translation: [0., 1., 0.],
                ..Default::default()
            },
            kind: LightKind::Point,
            color: [1., 0.5, 0.1],
            intensity: 40.,
            range: 8.,
        }))],
    );
    apply_json(
        &mut doc,
        "v2-mod",
        &format!(
            r#"[{{"op":"modifier","object":"{object}","name":"shell","operation":{{"op":"array","count":2,"offset":[0.25,0,0]}}}}]"#
        ),
    );
    let v2 = doc.to_bytes(None).unwrap();
    assert_eq!(u32::from_le_bytes(v2[8..12].try_into().unwrap()), 2);
    let reopened = Document::from_bytes(&v2, Limits::default(), None).unwrap();
    assert_eq!(reopened.head(), doc.head());
    assert!(reopened.scene().emitters.contains_key("probe"));
    assert_eq!(reopened.scene().modifiers[&object][0].name, "shell");
    let glb = makepad_gltf::parse_glb_bytes(&reopened.compile(None).unwrap().glb).unwrap();
    assert_eq!(lights_ext(&glb).len(), 1);

    let mut bad_version = V1_ROBOT.to_vec();
    bad_version[8..12].copy_from_slice(&0u32.to_le_bytes());
    assert!(matches!(
        Document::from_bytes(&bad_version, Limits::default(), None),
        Err(Error::Corrupt(_))
    ));
    bad_version[8..12].copy_from_slice(&3u32.to_le_bytes());
    assert!(matches!(
        Document::from_bytes(&bad_version, Limits::default(), None),
        Err(Error::Corrupt(_))
    ));
    assert!(Document::from_bytes(&V1_ROBOT[..40], Limits::default(), None).is_err());
}

#[test]
fn arbitrary_rest_ik_and_constraint_dependencies() {
    let mut local = chain()
        .joints
        .iter()
        .map(|j| Transform {
            translation: j.translation,
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let rest_root = Transform {
        translation: [0.; 3],
        rotation: quat_axis_angle([0., 0., 1.], std::f64::consts::FRAC_PI_2).unwrap(),
        scale: [1.; 3],
    };
    local[0] = rest_root;
    let target = [0., 1., 0.];
    solve_ik(&chain(), &mut local, 0, 1, 2, target, [0., 0., 1.], false).unwrap();
    let (world, _) = globals_from(&chain(), &local);
    let end = transform_point(world[2], [0.; 3]);
    assert!(
        length(sub(end, target)) < 1e-6,
        "IK with rotated rest missed {end:?}"
    );
    assert!(
        (length(sub(
            transform_point(world[1], [0.; 3]),
            transform_point(world[0], [0.; 3])
        )) - 1.)
            .abs()
            < 1e-8
    );

    let mut doc = Document::new(Limits::default()).unwrap();
    apply(
        &mut doc,
        "skel",
        vec![
            Operation::SetSkeleton { skeleton: arm() },
            Operation::Rig(RigOperation::Rest {
                joint: 1,
                transform: rest_root,
            }),
            Operation::Rig(RigOperation::Pose(Pose {
                name: "aim".into(),
                joints: BTreeMap::new(),
            })),
        ],
    );
    apply(
        &mut doc,
        "ik",
        vec![Operation::Rig(RigOperation::Ik {
            pose: "aim".into(),
            root: 1,
            middle: 2,
            end: 3,
            target,
            pole: [0., 0., 1.],
            clamp_reach: false,
        })],
    );
    let solved = doc
        .rig()
        .local_pose(doc.skeleton().unwrap(), Some(&doc.rig().poses["aim"]))
        .unwrap();
    let (world, _) = globals_from(doc.skeleton().unwrap(), &solved);
    assert!(length(sub(transform_point(world[3], [0.; 3]), target)) < 1e-5);

    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "overlap",
        vec![
            Operation::Rig(RigOperation::Constraint(RigConstraint {
                name: "copy-middle".into(),
                joint: 2,
                enabled: true,
                kind: ConstraintKind::Copy {
                    target: 4,
                    translation: true,
                    rotation: false,
                    scale: false,
                    influence: 1.,
                },
            })),
            Operation::Rig(RigOperation::Constraint(RigConstraint {
                name: "reach".into(),
                joint: 1,
                enabled: true,
                kind: ConstraintKind::TwoBoneIk {
                    middle: 2,
                    end: 3,
                    target: 4,
                    pole: [0., 0., 1.],
                    clamp_reach: true,
                },
            })),
        ],
    )
    .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert_unchanged(&doc, &before);

    apply(
        &mut doc,
        "disabled-copy",
        vec![
            Operation::Rig(RigOperation::Constraint(RigConstraint {
                name: "copy-middle".into(),
                joint: 2,
                enabled: false,
                kind: ConstraintKind::Copy {
                    target: 4,
                    translation: true,
                    rotation: false,
                    scale: false,
                    influence: 1.,
                },
            })),
            Operation::Rig(RigOperation::Constraint(RigConstraint {
                name: "reach".into(),
                joint: 1,
                enabled: true,
                kind: ConstraintKind::TwoBoneIk {
                    middle: 2,
                    end: 3,
                    target: 4,
                    pole: [0., 0., 1.],
                    clamp_reach: true,
                },
            })),
        ],
    );
    apply(
        &mut doc,
        "solve",
        vec![Operation::Rig(RigOperation::SolvePose {
            pose: "aim".into(),
        })],
    );

    apply(
        &mut doc,
        "limit",
        vec![Operation::Rig(RigOperation::Constraint(RigConstraint {
            name: "fold".into(),
            joint: 4,
            enabled: true,
            kind: ConstraintKind::Limit {
                min_translation: [0.5, 0.5, 0.],
                max_translation: [0.5, 0.5, 0.],
                min_scale: [1.; 3],
                max_scale: [1.; 3],
                max_angle: std::f64::consts::PI,
            },
        }))],
    );
    apply(
        &mut doc,
        "poses",
        vec![
            Operation::Rig(RigOperation::Pose(Pose {
                name: "rest".into(),
                joints: BTreeMap::new(),
            })),
            Operation::Rig(RigOperation::BakeClip {
                name: "reach-clip".into(),
                poses: vec![(0., "rest".into()), (1., "aim".into())],
                fps: 4.,
                solve_constraints: true,
            }),
        ],
    );
    assert!(doc.clips().contains_key("reach-clip"));
    let bytes = doc.to_bytes(None).unwrap();
    assert_eq!(
        Document::from_bytes(&bytes, Limits::default(), None)
            .unwrap()
            .to_bytes(None)
            .unwrap(),
        bytes
    );

    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "shear-ik",
        vec![
            Operation::Rig(RigOperation::Rest {
                joint: 1,
                transform: Transform {
                    translation: [0.; 3],
                    rotation: [0., 0., 0., 1.],
                    scale: [2., 1., 1.],
                },
            }),
            Operation::Rig(RigOperation::Ik {
                pose: "rest".into(),
                root: 1,
                middle: 2,
                end: 3,
                target: [0.2, 0.2, 0.],
                pole: [0., 0., 1.],
                clamp_reach: false,
            }),
        ],
    )
    .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert_unchanged(&doc, &before);

    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "reparent-cycle",
        vec![Operation::Rig(RigOperation::ReparentJoint {
            joint: 1,
            parent: Some(2),
        })],
    )
    .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert_unchanged(&doc, &before);
}

fn globals_from(skeleton: &Skeleton, local: &[Transform]) -> (Vec<Matrix4>, Vec<Quaternion>) {
    let mut world = Vec::new();
    let mut rotations = Vec::new();
    for (i, t) in local.iter().enumerate() {
        let m = t.matrix().unwrap();
        if let Some(parent) = skeleton.joints[i].parent {
            world.push(matrix_mul(world[parent as usize], m));
            rotations.push(quat_normalize(quat_mul(rotations[parent as usize], t.rotation)).unwrap());
        } else {
            world.push(m);
            rotations.push(t.rotation);
        }
    }
    (world, rotations)
}

#[test]
fn morph_output_follows_links_and_refuses_collapse_or_lod() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, "mesh", vec![cube("body", [2.; 3]), cube("prop", [1.; 3])]);
    apply_json(
        &mut doc,
        "link",
        r#"[{"op":"instance","object":"copy","source":"body","transform":{"translation":[4,0,0]}}]"#,
    );
    let deltas: BTreeMap<_, _> = doc
        .object("body")
        .unwrap()
        .vertices()
        .iter()
        .map(|v| (v.id, [0., 0.25, 0.]))
        .collect();
    apply(
        &mut doc,
        "shape",
        vec![Operation::Rig(RigOperation::Morph {
            name: "raise".into(),
            object: "body".into(),
            deltas: deltas.clone(),
            weight: 0.4,
        })],
    );
    let glb = makepad_gltf::parse_glb_bytes(&doc.compile(None).unwrap().glb).unwrap();
    let mut saw_body = false;
    let mut saw_copy = false;
    let mut saw_prop_zero = false;
    for node in glb.document.nodes_slice() {
        let Some(mesh) = node.mesh else { continue };
        let mesh = &glb.document.meshes_slice()[mesh];
        assert_eq!(mesh.weights, Some(vec![0.4]));
        for primitive in &mesh.primitives {
            let pos = f32s(&glb, primitive.targets.as_ref().unwrap()[0]["POSITION"], 3);
            match node.name.as_deref() {
                Some("body") | Some("copy") => {
                    assert!(
                        pos.chunks_exact(3).all(|p| p == [0., 0.25, 0.]),
                        "{} deltas {:?}",
                        node.name.as_deref().unwrap(),
                        pos
                    );
                    if node.name.as_deref() == Some("body") {
                        saw_body = true;
                    } else {
                        saw_copy = true;
                    }
                }
                Some("prop") => {
                    assert!(pos.iter().all(|v| *v == 0.), "unrelated object leaked morph");
                    saw_prop_zero = true;
                }
                _ => {}
            }
        }
    }
    assert!(saw_body && saw_copy && saw_prop_zero);

    apply(
        &mut doc,
        "capture",
        vec![
            Operation::Scene(SceneOperation::Duplicate {
                object: "shaped".into(),
                source: "body".into(),
            }),
            Operation::Rig(RigOperation::CaptureMorph {
                name: "from-dup".into(),
                object: "body".into(),
                target: "shaped".into(),
                weight: 0.,
            }),
        ],
    );
    assert_eq!(doc.rig().morphs["from-dup"].object, "body");

    let before = snap(&doc);
    let face = doc.object("body").unwrap().faces()[0].id;
    let err = try_apply(
        &mut doc,
        "topo",
        vec![Operation::DeleteFaces {
            object: "body".into(),
            faces: vec![face],
        }],
    )
    .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert_unchanged(&doc, &before);

    let collapse: BTreeMap<_, _> = doc
        .object("body")
        .unwrap()
        .vertices()
        .iter()
        .map(|v| (v.id, [-v.position[0], -v.position[1], -v.position[2]]))
        .collect();
    apply(
        &mut doc,
        "crush",
        vec![Operation::Rig(RigOperation::Morph {
            name: "crush".into(),
            object: "body".into(),
            deltas: collapse,
            weight: 1.,
        })],
    );
    let err = doc.compile(None).unwrap_err();
    assert!(
        matches!(err, Error::Invalid(_)),
        "collapsed shape key must fail compile, got {err:?}"
    );

    let mut lod = Document::new(Limits::default()).unwrap();
    let sphere = parse_operations(
        &json::parse(
            br#"[{"op":"sphere","object":"ball","radius":1,"segments":12,"rings":8}]"#,
        )
        .unwrap(),
        lod.limits(),
    )
    .unwrap();
    apply(&mut lod, "ball", sphere);
    let morph: BTreeMap<_, _> = lod
        .object("ball")
        .unwrap()
        .vertices()
        .iter()
        .take(4)
        .map(|v| (v.id, [0., 0.05, 0.]))
        .collect();
    apply(
        &mut lod,
        "shape",
        vec![
            Operation::Rig(RigOperation::Morph {
                name: "nub".into(),
                object: "ball".into(),
                deltas: morph,
                weight: 0.2,
            }),
            Operation::Scene(SceneOperation::Lods {
                object: "ball".into(),
                levels: vec![LodLevel {
                    target_faces: 80,
                    max_error: 1.,
                    distance: 12.,
                }],
            }),
        ],
    );
    let err = lod.compile(None).unwrap_err();
    assert!(
        matches!(err, Error::Invalid(_)),
        "LOD over a live shape key must fail, got {err:?}"
    );
}

#[test]
fn surface_refs_bake_instance_and_unknown_material() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(
        &mut doc,
        "mesh",
        vec![
            Operation::Plane {
                object: "hi".into(),
                size: [2., 2.],
            },
            Operation::Plane {
                object: "lo".into(),
                size: [2., 2.],
            },
        ],
    );
    apply_json(
        &mut doc,
        "link",
        r#"[{"op":"instance","object":"inst","source":"hi"}]"#,
    );
    let ids: Vec<_> = doc.object("hi").unwrap().vertices().iter().map(|v| v.id).collect();
    apply(
        &mut doc,
        "paint",
        vec![Operation::Surface(SurfaceOperation::VertexPaint {
            object: "hi".into(),
            vertices: ids.clone(),
            color: [0.2, 0.4, 0.6, 1.],
            opacity: 1.,
        })],
    );
    assert_eq!(doc.surface().vertex_colors.len(), ids.len());

    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "paint-inst",
        vec![Operation::Surface(SurfaceOperation::VertexPaint {
            object: "inst".into(),
            vertices: ids.clone(),
            color: [1., 0., 0., 1.],
            opacity: 1.,
        })],
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::Invalid(_)),
        "linked instance has no stored vertices to paint, got {err:?}"
    );
    assert_unchanged(&doc, &before);

    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "mat-gap",
        vec![Operation::Surface(SurfaceOperation::Material {
            material: 9,
            value: SurfaceMaterial::default(),
        })],
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::Invalid(_)),
        "surface material must name an existing legacy slot, got {err:?}"
    );
    assert_unchanged(&doc, &before);

    let l = doc.limits().clone();
    apply(
        &mut doc,
        "layer",
        vec![Operation::Surface(SurfaceOperation::Layer {
            material: 0,
            channel: SurfaceChannel::BaseColor,
            layer: SurfaceLayer::new("base", RgbaImage::new(8, 8, [20, 40, 80, 255], &l).unwrap()),
        })],
    );
    apply(
        &mut doc,
        "bake-inst",
        vec![Operation::Surface(SurfaceOperation::Bake {
            source: "inst".into(),
            target: "lo".into(),
            material: 0,
            channel: SurfaceChannel::BaseColor,
            layer: "from-inst".into(),
            width: 8,
            height: 8,
            max_distance: 4.,
            ao_samples: 4,
            ao_distance: 1.,
            dilation: 1,
        })],
    );
    assert!(
        doc.surface().materials[&0].channels[&SurfaceChannel::BaseColor]
            .iter()
            .any(|layer| layer.id == "from-inst"),
        "bake must evaluate linked instance geometry rather than the empty stored mesh"
    );
    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "bake-missing",
        vec![Operation::Surface(SurfaceOperation::Bake {
            source: "nope".into(),
            target: "lo".into(),
            material: 0,
            channel: SurfaceChannel::BaseColor,
            layer: "ghost".into(),
            width: 8,
            height: 8,
            max_distance: 2.,
            ao_samples: 4,
            ao_distance: 1.,
            dilation: 1,
        })],
    )
    .unwrap_err();
    assert!(matches!(err, Error::Invalid(_) | Error::MissingObject(_)), "{err:?}");
    assert_unchanged(&doc, &before);
    apply_json(&mut doc, "unique", r#"[{"op":"make_unique","object":"inst"}]"#);

    apply(
        &mut doc,
        "drop-hi",
        vec![Operation::DeleteObject {
            object: "hi".into(),
        }],
    );
    assert!(doc
        .surface()
        .vertex_colors
        .keys()
        .all(|(name, _)| name != "hi"));
    let bytes = doc.to_bytes(None).unwrap();
    let round = Document::from_bytes(&bytes, Limits::default(), None).unwrap();
    assert_eq!(round.surface().vertex_colors, doc.surface().vertex_colors);
    assert_eq!(round.surface().materials, doc.surface().materials);
}

#[test]
fn budget_cancel_and_atomic_failures() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, "body", vec![cube("body", [1.; 3])]);
    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "partial",
        vec![
            cube("other", [1.; 3]),
            Operation::DeleteObject {
                object: "missing".into(),
            },
        ],
    )
    .unwrap_err();
    assert!(matches!(err, Error::MissingObject(_)), "{err:?}");
    assert_unchanged(&doc, &before);
    assert!(doc.object("other").is_none());

    apply_json(
        &mut doc,
        "inst",
        r#"[{"op":"instance","object":"copy","source":"body"}]"#,
    );
    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "drop-src",
        vec![Operation::DeleteObject {
            object: "body".into(),
        }],
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::MissingObject(_)),
        "deleting an instance source must fail atomically, got {err:?}"
    );
    assert_unchanged(&doc, &before);

    apply(
        &mut doc,
        "lamp",
        vec![Operation::Scene(SceneOperation::Light(LightEmitter {
            name: "lamp".into(),
            attachment: Attachment::Object("body".into()),
            transform: Transform::default(),
            kind: LightKind::Point,
            color: [1.; 3],
            intensity: 1.,
            range: 4.,
        }))],
    );
    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "drop-lit",
        vec![
            Operation::DeleteObject {
                object: "copy".into(),
            },
            Operation::DeleteObject {
                object: "body".into(),
            },
        ],
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::Invalid(_) | Error::MissingObject(_)),
        "object with attached light cannot disappear under it, got {err:?}"
    );
    assert_unchanged(&doc, &before);

    let lights: Vec<_> = (0..33)
        .map(|i| {
            Operation::Scene(SceneOperation::Light(LightEmitter {
                name: format!("l{i}"),
                attachment: Attachment::Root,
                transform: Transform::default(),
                kind: LightKind::Point,
                color: [1.; 3],
                intensity: 1.,
                range: 1.,
            }))
        })
        .collect();
    let before = snap(&doc);
    let err = try_apply(&mut doc, "too-many-lights", lights).unwrap_err();
    assert!(matches!(err, Error::Budget(_)), "{err:?}");
    assert_unchanged(&doc, &before);

    let mods: String = (0..17)
        .map(|i| {
            format!(
                r#"{{"op":"modifier","object":"body","name":"m{i}","operation":{{"op":"array","count":2,"offset":[0.01,0,0]}}}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let before = snap(&doc);
    let err = try_json(&mut doc, "too-many-mods", &format!("[{mods}]")).unwrap_err();
    assert!(
        matches!(err, Error::Budget(_) | Error::Invalid(_)),
        "{err:?}"
    );
    assert_unchanged(&doc, &before);

    {
        let mut lod_doc = Document::new(Limits::default()).unwrap();
        apply(&mut lod_doc, "body", vec![cube("body", [1.; 3])]);
        apply(
            &mut lod_doc,
            "lod-noop",
            vec![Operation::Scene(SceneOperation::Lods {
                object: "body".into(),
                levels: vec![LodLevel {
                    target_faces: 12,
                    max_error: 1.,
                    distance: 10.,
                }],
            })],
        );
        let after_lod = snap(&lod_doc);
        let compiled = lod_doc.compile(None);
        assert!(
            compiled.is_err(),
            "LOD that cannot reduce a cube must fail at compile, got {:?}",
            compiled.as_ref().map(|p| p.triangles)
        );
        assert_unchanged(&lod_doc, &after_lod);
    }

    apply_json(
        &mut doc,
        "stack",
        r#"[{"op":"modifier","object":"body","name":"copies","operation":{"op":"array","count":2,"offset":[1,0,0]}}]"#,
    );
    let calls = Cell::new(0u32);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() > 2
    };
    let err = doc.compile(Some(&cancel)).unwrap_err();
    assert!(
        matches!(err, Error::Mesh(mesh::MeshError::Cancelled)),
        "compile must be cancellation-sensitive, got {err:?}"
    );
    assert_eq!(doc.to_bytes(None).unwrap(), doc.to_bytes(None).unwrap());

    apply(
        &mut doc,
        "clip",
        vec![
            Operation::SetSkeleton {
                skeleton: Skeleton {
                    joints: vec![Joint {
                        name: "root".into(),
                        parent: None,
                        translation: [0.; 3],
                    }],
                },
            },
            Operation::SetClip {
                clip: AnimationClip {
                    name: "wave".into(),
                    channels: vec![AnimationChannel {
                        joint: 0,
                        path: AnimationPath::Translation,
                        keys: vec![
                            Keyframe {
                                time: 0.,
                                value: [0.; 4],
                            },
                            Keyframe {
                                time: 1.,
                                value: [1., 0., 0., 0.],
                            },
                        ],
                    }],
                },
            },
            Operation::Rig(RigOperation::ClipOptions {
                name: "wave".into(),
                options: ClipOptions {
                    interpolation: Interpolation::Linear,
                    root_motion: false,
                    events: vec![ClipEvent {
                        time: 0.25,
                        name: "tick".into(),
                        payload: "x".into(),
                    }],
                    morph_keys: BTreeMap::new(),
                },
            }),
        ],
    );
    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "drop-clip",
        vec![Operation::DeleteClip {
            name: "wave".into(),
        }],
    );
    match err {
        Ok(_) => {
            assert!(
                !doc.rig().clip_options.contains_key("wave"),
                "deleting a clip must drop its options; leftover options are a defect"
            );
        }
        Err(e) => {
            assert!(
                matches!(e, Error::Invalid(_)),
                "clip delete with options must be explicit, got {e:?}"
            );
            assert_unchanged(&doc, &before);
        }
    }
}

#[test]
fn linked_instances_lods_and_export_lights() {
    let mut doc = Document::new(Limits::default()).unwrap();
    let sphere = parse_operations(
        &json::parse(br#"[{"op":"sphere","object":"car","radius":1,"segments":12,"rings":8}]"#)
            .unwrap(),
        doc.limits(),
    )
    .unwrap();
    apply(&mut doc, "car", sphere);
    apply(
        &mut doc,
        "place",
        vec![
            Operation::Scene(SceneOperation::Node {
                object: "car".into(),
                node: SceneNode {
                    parent: None,
                    linked_to: None,
                    transform: Transform {
                        translation: [2., 0., 0.],
                        rotation: quat_axis_angle([0., 1., 0.], 0.3).unwrap(),
                        scale: [1.; 3],
                    },
                },
            }),
            Operation::Scene(SceneOperation::Instance {
                object: "shadow".into(),
                source: "car".into(),
                transform: Transform {
                    translation: [-4., 0., 0.],
                    ..Default::default()
                },
            }),
        ],
    );
    apply(
        &mut doc,
        "skel",
        vec![
            Operation::SetSkeleton {
                skeleton: Skeleton {
                    joints: vec![Joint {
                        name: "root".into(),
                        parent: None,
                        translation: [0., 0.2, 0.],
                    }],
                },
            },
            Operation::AutoWeights {
                object: "car".into(),
            },
            Operation::Rig(RigOperation::Rest {
                joint: 0,
                transform: Transform {
                    translation: [0., 0.2, 0.],
                    rotation: quat_axis_angle([1., 0., 0.], 0.4).unwrap(),
                    scale: [1.; 3],
                },
            }),
        ],
    );
    apply(
        &mut doc,
        "attach",
        vec![
            Operation::Scene(SceneOperation::Light(LightEmitter {
                name: "head".into(),
                attachment: Attachment::Object("shadow".into()),
                transform: Transform {
                    translation: [0., 0., -1.2],
                    ..Default::default()
                },
                kind: LightKind::Spot {
                    inner: 0.1,
                    outer: 0.35,
                },
                color: [1., 0.9, 0.7],
                intensity: 250.,
                range: 20.,
            })),
            Operation::Scene(SceneOperation::Light(LightEmitter {
                name: "bone-lamp".into(),
                attachment: Attachment::Joint(0),
                transform: Transform::default(),
                kind: LightKind::Point,
                color: [0.2, 0.4, 1.],
                intensity: 8.,
                range: 6.,
            })),
            Operation::Scene(SceneOperation::Socket(Socket {
                name: "hitch".into(),
                attachment: Attachment::Object("car".into()),
                transform: Transform {
                    translation: [0., 0., 1.],
                    ..Default::default()
                },
            })),
            Operation::Scene(SceneOperation::Lods {
                object: "shadow".into(),
                levels: vec![
                    LodLevel {
                        target_faces: 120,
                        max_error: 1.,
                        distance: 8.,
                    },
                    LodLevel {
                        target_faces: 80,
                        max_error: 2.,
                        distance: 24.,
                    },
                ],
            }),
            Operation::Scene(SceneOperation::Collider {
                object: "car".into(),
                proxy: CollisionProxy::Mesh {
                    object: "shadow".into(),
                },
            }),
        ],
    );
    let before = snap(&doc);
    let product = doc.compile(None).unwrap();
    assert_unchanged(&doc, &before);
    let glb = makepad_gltf::parse_glb_bytes(&product.glb).unwrap();
    let nodes = glb.document.nodes_slice();
    let lights = lights_ext(&glb);
    assert_eq!(lights.len(), 2);
    assert!(lights.iter().any(|l| matches!(field(l, "type"), JsonValue::String(s) if s == "spot")));
    assert!(lights.iter().any(|l| matches!(field(l, "type"), JsonValue::String(s) if s == "point")));

    let shadow = nodes
        .iter()
        .position(|n| n.name.as_deref() == Some("shadow"))
        .unwrap();
    let head = nodes
        .iter()
        .position(|n| n.name.as_deref() == Some("head"))
        .unwrap();
    assert!(
        nodes[shadow]
            .children
            .as_ref()
            .is_some_and(|c| c.contains(&head)),
        "spot must parent under the instance it is attached to"
    );
    let lod_owner = nodes
        .iter()
        .find(|n| {
            n.extensions.as_ref().is_some_and(|v| {
                matches!(v, JsonValue::Object(f) if f.contains_key("MSFT_lod"))
            })
        })
        .expect("instance LOD missing MSFT_lod");
    let ids = arr(field(field(lod_owner.extensions.as_ref().unwrap(), "MSFT_lod"), "ids"));
    assert_eq!(ids.len(), 2);
    let count = |i: usize| {
        let mesh = nodes[i].mesh.expect("LOD node needs a mesh");
        glb.document.meshes_slice()[mesh]
            .primitives
            .iter()
            .map(|p| glb.document.accessors_slice()[p.indices.unwrap()].count / 3)
            .sum::<usize>()
    };
    let mut previous = count(
        nodes
            .iter()
            .position(|n| n.name.as_deref() == lod_owner.name.as_deref())
            .unwrap(),
    );
    for id in ids {
        let n = count(idx(id));
        assert!(n < previous, "LOD {n} did not reduce from {previous}");
        previous = n;
        assert_eq!(nodes[idx(id)].skin, Some(0));
    }

    let car = nodes
        .iter()
        .find(|n| n.name.as_deref() == Some("car"))
        .unwrap();
    assert!(matches!(
        field(field(car.extras.as_ref().unwrap(), "MAKEPAD_collision"), "kind"),
        JsonValue::String(s) if s == "mesh"
    ));
    let hitch = nodes
        .iter()
        .position(|n| n.name.as_deref() == Some("hitch"))
        .unwrap();
    let car_i = nodes
        .iter()
        .position(|n| n.name.as_deref() == Some("car"))
        .unwrap();
    assert!(nodes[car_i].children.as_ref().unwrap().contains(&hitch));

    let joint = nodes
        .iter()
        .position(|n| n.name.as_deref() == Some("root"))
        .unwrap();
    let lamp = nodes
        .iter()
        .position(|n| n.name.as_deref() == Some("bone-lamp"))
        .unwrap();
    assert!(nodes[joint].children.as_ref().unwrap().contains(&lamp));
    assert!(nodes[joint].rotation.is_some());

    apply_json(
        &mut doc,
        "grow",
        r#"[{"op":"modifier","object":"car","name":"copies","operation":{"op":"array","count":2,"offset":[0,2,0]}}]"#,
    );
    let grown = doc.compile(None).unwrap();
    assert!(
        grown.triangles > product.triangles,
        "instance must follow source modifiers: {} vs {}",
        grown.triangles,
        product.triangles
    );

    let before = snap(&doc);
    let err = try_apply(
        &mut doc,
        "bad-spot",
        vec![Operation::Scene(SceneOperation::Light(LightEmitter {
            name: "bad".into(),
            attachment: Attachment::Root,
            transform: Transform::default(),
            kind: LightKind::Spot {
                inner: 0.4,
                outer: 0.2,
            },
            color: [1.; 3],
            intensity: 1.,
            range: 3.,
        }))],
    )
    .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert_unchanged(&doc, &before);

    apply(
        &mut doc,
        "join",
        vec![Operation::Scene(SceneOperation::Join {
            object: "both".into(),
            sources: vec!["car".into(), "shadow".into()],
        })],
    );
    assert!(doc.object("both").unwrap().faces().len() > doc.object("car").unwrap().faces().len());
}
