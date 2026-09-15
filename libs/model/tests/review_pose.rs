use makepad_model::transform::*;
use makepad_model::*;
use makepad_render::{StaticModel, MODEL_VERTEX_FLOATS};
use std::collections::BTreeMap;

fn apply(doc: &mut Document, request: &str, operations: Vec<Operation>) {
    doc.apply(
        Transaction {
            request_id: request.into(),
            expected: doc.head(),
            operations,
        },
        None,
    )
    .unwrap();
}

fn node(object: &str, parent: Option<&str>, transform: Transform) -> Operation {
    Operation::Scene(SceneOperation::Node {
        object: object.into(),
        node: SceneNode {
            parent: parent.map(str::to_string),
            transform,
            ..Default::default()
        },
    })
}

fn points(model: &StaticModel) -> Vec<[f64; 3]> {
    assert!(
        model.driven_parts.is_empty(),
        "review must contain the baked wheel geometry"
    );
    assert!(model.anim_parts.is_empty());
    model
        .vertices
        .chunks_exact(MODEL_VERTEX_FLOATS)
        .map(|v| [v[0] as f64, v[1] as f64, v[2] as f64])
        .collect()
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    length(sub(a, b))
}

fn assert_points(actual: &[[f64; 3]], expected: &[[f64; 3]]) {
    assert!(!actual.is_empty());
    // GLB exporters split corner normals/UVs, so compare geometric point
    // sets in both directions without depending on vertex packing/order.
    for (label, source, target) in [
        ("unexpected", actual, expected),
        ("missing", expected, actual),
    ] {
        for point in source {
            let nearest = target
                .iter()
                .map(|candidate| distance(*point, *candidate))
                .fold(f64::INFINITY, f64::min);
            assert!(
                nearest < 2.0e-5,
                "{label} review vertex {point:?}; nearest distance {nearest}"
            );
        }
    }
}

fn baked(doc: &Document, pose: &ReviewPose) -> Vec<[f64; 3]> {
    let compiled = doc.compile_review_pose(pose, None).unwrap();
    let parsed = makepad_gltf::parse_glb_bytes(&compiled.glb).unwrap();
    assert!(parsed.document.skins.as_ref().is_none_or(Vec::is_empty));
    assert!(parsed
        .document
        .animations
        .as_ref()
        .is_none_or(Vec::is_empty));
    let model = StaticModel::parse_glb(&compiled.glb).unwrap();
    assert_eq!(model.indices.len(), compiled.triangles * 3);
    points(&model)
}

fn vehicle() -> Document {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(
        &mut doc,
        "vehicle",
        vec![
            Operation::Cube {
                object: "body".into(),
                size: [2., 0.6, 5.],
            },
            Operation::Cube {
                object: "front".into(),
                size: [0.3, 0.7, 0.5],
            },
            Operation::Cube {
                object: "rear".into(),
                size: [0.25, 0.6, 0.4],
            },
            Operation::Cube {
                object: "hub".into(),
                size: [0.1, 0.2, 0.15],
            },
            node(
                "body",
                None,
                Transform {
                    translation: [4., 2., -3.],
                    scale: [1.5; 3],
                    ..Default::default()
                },
            ),
            node(
                "front",
                Some("body"),
                Transform {
                    translation: [1., -0.8, 2.],
                    scale: [0.8; 3],
                    ..Default::default()
                },
            ),
            node(
                "rear",
                Some("body"),
                Transform {
                    translation: [1., -0.8, -2.],
                    scale: [0.9; 3],
                    ..Default::default()
                },
            ),
            node(
                "hub",
                Some("front"),
                Transform {
                    translation: [0.3, 0.03, 0.1],
                    rotation: quat_axis_angle([1., 0., 0.], 0.4).unwrap(),
                    ..Default::default()
                },
            ),
            Operation::Scene(SceneOperation::VehicleWheel {
                object: "front".into(),
                wheel: VehicleWheel {
                    connection: "wheel_front_left".into(),
                    pivot: [0.1, 0.05, -0.2],
                    radius: 0.42,
                    width: 0.36,
                    visual: None,
                },
            }),
            Operation::Scene(SceneOperation::VehicleWheel {
                object: "rear".into(),
                wheel: VehicleWheel {
                    connection: "wheel_rear_left".into(),
                    pivot: [-0.05, 0.1, 0.08],
                    radius: 0.405,
                    width: 0.3375,
                    visual: None,
                },
            }),
        ],
    );
    doc
}

fn expected_vehicle(doc: &Document, steer: f64, suspension: f64) -> Vec<[f64; 3]> {
    // This explicit formula follows Sandbox's rotation about model-space Y
    // with angle -wheel.steer. The authored front pivot is not its origin.
    let anchor = [5.62, 0.86, -0.24];
    let (sin, cos) = steer.sin_cos();
    doc.objects()
        .flat_map(|(name, mesh)| {
            let frame = doc.scene().world_matrix(name).unwrap();
            mesh.vertices().iter().map(move |v| {
                let world = transform_point(frame, v.position);
                match name {
                    "front" | "hub" => {
                        let d = sub(world, anchor);
                        [
                            anchor[0] + cos * d[0] - sin * d[2],
                            world[1] + suspension,
                            anchor[2] + sin * d[0] + cos * d[2],
                        ]
                    }
                    "rear" => [world[0], world[1] + suspension, world[2]],
                    "body" => world,
                    other => panic!("unexpected fixture object {other}"),
                }
            })
        })
        .collect()
}

#[test]
fn reflected_wheel_child_review_preserves_outward_winding_uvs_and_corner_normals() {
    let mut doc = vehicle();
    // The wheel frame itself must remain positive and uniformly scaled. Its
    // decorative child is a valid reflected object, even during steering.
    let mut child = doc.scene().nodes["hub"].clone();
    child.transform.scale = [-1., 0.7, 1.2];
    let mesh = doc.object("hub").unwrap();
    let mut operations = mesh.corners().iter().map(|corner| Operation::CornerNormal {
        object: "hub".into(), corner: corner.id,
        normal: Some(normalized(mesh.vertex(corner.vertex).unwrap().position).unwrap()),
    }).collect::<Vec<_>>();
    operations.push(Operation::Scene(SceneOperation::Node { object: "hub".into(), node: child }));
    // The compact review fixture has only the two left wheels. Add their
    // right counterparts so this regression also passes publication admission.
    for (source, size) in [("front", [0.3,0.7,0.5]), ("rear", [0.25,0.6,0.4])] {
        let object = format!("{source}_right");
        let mut node = doc.scene().nodes[source].clone();
        node.transform.translation[0] = -1.;
        let mut wheel = doc.scene().wheels[source].clone();
        wheel.connection = format!("wheel_{source}_right");
        operations.extend([
            Operation::Cube { object: object.clone(), size },
            Operation::Scene(SceneOperation::Node { object: object.clone(), node }),
            Operation::Scene(SceneOperation::VehicleWheel { object, wheel }),
        ]);
    }
    apply(&mut doc, "mirrored-child", operations);
    let before = doc.to_bytes(None).unwrap();
    doc.compile(None).unwrap(); // Fixture is admitted for ordinary publication.
    for (steer, suspension) in [(0., 0.), (0.35, 0.12)] {
        let product = doc.compile_review_pose(&ReviewPose::Vehicle { steer, suspension }, None).unwrap();
        let loaded = makepad_gltf::load_gltf_from_bytes(&product.glb, None).unwrap();
        let decoded = loaded.document.meshes_slice().iter().enumerate().flat_map(|(mesh, value)|
            (0..value.primitives.len()).map(move |primitive| (mesh, primitive)))
            .map(|(mesh, primitive)| makepad_gltf::decode_mesh_primitive(&loaded, mesh, primitive).unwrap()).collect::<Vec<_>>();
        assert_eq!(decoded.len(), product.primitives.len());
        let index = product.primitives.iter().position(|source| source.object == "hub").unwrap();
        let actual = &decoded[index];
        let wheel = &doc.scene().wheels["front"];
        let anchor = transform_point(doc.scene().world_matrix("front").unwrap(), wheel.pivot);
        let rotation = Transform { rotation: quat_axis_angle([0.,1.,0.],-steer).unwrap(), ..Default::default() }.matrix().unwrap();
        let translate = |position| Transform { translation: position, ..Default::default() }.matrix().unwrap();
        let motion = matrix_mul(translate(add(anchor,[0.,suspension,0.])),matrix_mul(rotation,translate(mul(anchor,-1.))));
        let frame = matrix_mul(motion,doc.scene().world_matrix("hub").unwrap());
        let center = transform_point(frame,[0.;3]);
        for triangle in actual.indices.chunks_exact(3) {
            let p: Vec<_> = triangle.iter().map(|&index|actual.positions[index as usize].map(f64::from)).collect();
            let geometric_normal = cross(sub(p[1],p[0]),sub(p[2],p[0]));
            let middle = mul(add(add(p[0],p[1]),p[2]),1./3.);
            assert!(dot(geometric_normal,sub(middle,center))>0., "reflected review triangle faces inward");
        }
        let mut expected = doc.object("hub").unwrap().clone();
        let ids = expected.vertices().iter().map(|v|v.id).collect::<Vec<_>>();
        let mut ctx = mesh::Context::default();
        expected.transform(&ids,frame,&mut ctx).unwrap();
        let expected = expected.triangulate(&mut ctx).unwrap();
        for (index, position) in actual.positions.iter().enumerate() {
            let normal = actual.normals.as_ref().unwrap()[index].map(f64::from);
            let uv = actual.texcoords0.as_ref().unwrap()[index].map(f64::from);
            assert!(expected.vertices.iter().any(|vertex|
                distance(position.map(f64::from),vertex.position)<2e-5
                && distance(normal,vertex.normal)<2e-5
                && (uv[0]-vertex.uv[0]).abs()<1e-6 && (uv[1]-vertex.uv[1]).abs()<1e-6),
                "reflected corner changed its normal or UV association");
        }
        assert_eq!(doc.to_bytes(None).unwrap(),before);
    }
}

#[test]
fn vehicle_review_turns_front_wheel_and_child_about_world_anchor_without_steering_rear() {
    let doc = vehicle();
    let source = doc.to_bytes(None).unwrap();
    let head = doc.head();
    let rest = baked(
        &doc,
        &ReviewPose::Vehicle {
            steer: 0.,
            suspension: 0.,
        },
    );
    assert_points(&rest, &expected_vehicle(&doc, 0., 0.));
    for (steer, suspension) in [
        (0.55, 0.),
        (-0.55, 0.),
        (0., 0.24),
        (0.55, 0.24),
        (-0.55, -0.2),
    ] {
        let actual = baked(&doc, &ReviewPose::Vehicle { steer, suspension });
        assert_points(&actual, &expected_vehicle(&doc, steer, suspension));
        assert!(
            actual
                .iter()
                .any(|point| rest.iter().all(|p| distance(*point, *p) > 0.01)),
            "pose did not move geometry"
        );
        assert_eq!(doc.head(), head);
        assert_eq!(doc.to_bytes(None).unwrap(), source);
    }
    let defaults = doc.default_review_poses();
    assert_eq!(defaults.len(), 4);
    assert!(matches!(defaults[0], ReviewPose::Vehicle { steer, suspension } if steer < 0. && (suspension-0.336).abs()<1e-9));
    assert!(matches!(defaults[1], ReviewPose::Vehicle { steer, suspension } if steer > 0. && (suspension-0.336).abs()<1e-9));
    assert!(
        matches!(defaults[2], ReviewPose::Vehicle { suspension, .. } if (suspension + 0.336).abs() < 1e-9)
    );
    assert!(
        matches!(defaults[3], ReviewPose::Vehicle { suspension, .. } if (suspension + 0.336).abs() < 1e-9)
    );
}

#[test]
fn authored_visual_limits_bound_review_without_mutating_the_vehicle() {
    let mut doc=vehicle();
    let visual=VisualWheelMotion{steer_gain:0.5,steer_max:0.18,compression:0.06,droop:0.09};
    let operations=doc.scene().wheels.iter().map(|(object,wheel)|{
        let mut wheel=wheel.clone();wheel.visual=Some(visual);
        Operation::Scene(SceneOperation::VehicleWheel{object:object.clone(),wheel})
    }).collect();
    apply(&mut doc,"visual-limits",operations);
    let source=doc.to_bytes(None).unwrap();
    for (input,travel,angle,height) in [(0.55,0.32,0.18,0.06),(-0.55,-0.32,-0.18,-0.09),(0.1,0.02,0.05,0.02)] {
        assert_points(&baked(&doc,&ReviewPose::Vehicle{steer:input,suspension:travel}),&expected_vehicle(&doc,angle,height));
        assert_eq!(doc.to_bytes(None).unwrap(),source);
    }
    let defaults=doc.default_review_poses();
    assert!(matches!(defaults[0],ReviewPose::Vehicle{suspension,..} if (suspension-0.06).abs()<1e-9));
    assert!(matches!(defaults[3],ReviewPose::Vehicle{suspension,..} if (suspension+0.09).abs()<1e-9));
}

fn rest_root() -> Transform {
    Transform {
        translation: [2., -1., 3.],
        rotation: quat_axis_angle([0., 0., 1.], 0.3).unwrap(),
        scale: [1.5, 0.8, 1.2],
    }
}
fn rest_child() -> Transform {
    Transform {
        translation: [0., 2., 0.5],
        rotation: quat_axis_angle([1., 0., 0.], -0.2).unwrap(),
        scale: [0.8, 1.1, 0.9],
    }
}
fn posed_root() -> Transform {
    Transform {
        translation: [2.4, -0.7, 2.9],
        ..rest_root()
    }
}
fn posed_child() -> Transform {
    Transform {
        translation: [0.1, 2.2, 0.8],
        rotation: quat_axis_angle([0., 0., 1.], 0.6).unwrap(),
        scale: [0.9, 1.4, 0.8],
    }
}

fn character(weighted: bool) -> Document {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(
        &mut doc,
        "character",
        vec![
            Operation::Cube {
                object: "character".into(),
                size: [1., 0.7, 1.3],
            },
            node(
                "character",
                None,
                Transform {
                    translation: [2.8, 1.4, 2.6],
                    rotation: quat_axis_angle([0., 1., 0.], 0.4).unwrap(),
                    scale: [0.7, 1.3, 0.9],
                },
            ),
            Operation::SetSkeleton {
                skeleton: Skeleton {
                    joints: vec![
                        Joint {
                            name: "root".into(),
                            parent: None,
                            translation: [0.; 3],
                        },
                        Joint {
                            name: "arm".into(),
                            parent: Some(0),
                            translation: [0., 2., 0.],
                        },
                    ],
                },
            },
            Operation::Rig(RigOperation::Rest {
                joint: 0,
                transform: rest_root(),
            }),
            Operation::Rig(RigOperation::Rest {
                joint: 1,
                transform: rest_child(),
            }),
            Operation::Rig(RigOperation::Pose(Pose {
                name: "reach".into(),
                joints: BTreeMap::from([(0, posed_root()), (1, posed_child())]),
            })),
            Operation::SetClip {
                clip: AnimationClip {
                    name: "wave".into(),
                    channels: vec![
                        AnimationChannel {
                            joint: 0,
                            path: AnimationPath::Translation,
                            keys: vec![
                                Keyframe {
                                    time: 0.,
                                    value: [2., -1., 3., 0.],
                                },
                                Keyframe {
                                    time: 2.,
                                    value: [2.8, -0.8, 2.6, 0.],
                                },
                            ],
                        },
                        AnimationChannel {
                            joint: 1,
                            path: AnimationPath::Rotation,
                            keys: vec![
                                Keyframe {
                                    time: 0.,
                                    value: rest_child().rotation,
                                },
                                Keyframe {
                                    time: 2.,
                                    value: quat_axis_angle([1., 0., 0.], 0.8).unwrap(),
                                },
                            ],
                        },
                    ],
                },
            },
        ],
    );
    if weighted {
        let operations = doc
            .object("character")
            .unwrap()
            .vertices()
            .iter()
            .map(|v| Operation::SetWeights {
                object: "character".into(),
                vertex: v.id,
                weights: vec![
                    mesh::JointWeight {
                        joint: 0,
                        weight: 0.25,
                    },
                    mesh::JointWeight {
                        joint: 1,
                        weight: 0.75,
                    },
                ],
            })
            .collect();
        apply(&mut doc, "weights", operations);
    }
    let deltas = doc
        .object("character")
        .unwrap()
        .vertices()
        .iter()
        .map(|v| (v.id, [0.1, 0.2, 0.]))
        .collect();
    apply(
        &mut doc,
        "morph",
        vec![
            Operation::Rig(RigOperation::Morph {
                name: "breathing".into(),
                object: "character".into(),
                deltas,
                weight: 0.4,
            }),
            Operation::Rig(RigOperation::ClipOptions {
                name: "wave".into(),
                options: ClipOptions {
                    morph_keys: BTreeMap::from([(
                        "breathing".into(),
                        vec![
                            MorphKey {
                                time: 0.,
                                weight: 0.,
                            },
                            MorphKey {
                                time: 2.,
                                weight: 1.,
                            },
                        ],
                    )]),
                    ..Default::default()
                },
            }),
        ],
    );
    doc
}

fn expected_character(
    doc: &Document,
    root: Transform,
    child: Transform,
    morph_weight: f64,
) -> Vec<[f64; 3]> {
    let rest = [
        rest_root().matrix().unwrap(),
        matrix_mul(
            rest_root().matrix().unwrap(),
            rest_child().matrix().unwrap(),
        ),
    ];
    let posed = [
        root.matrix().unwrap(),
        matrix_mul(root.matrix().unwrap(), child.matrix().unwrap()),
    ];
    let skin = [
        matrix_mul(posed[0], inverse(rest[0]).unwrap()),
        matrix_mul(posed[1], inverse(rest[1]).unwrap()),
    ];
    let object = doc.scene().world_matrix("character").unwrap();
    doc.object("character")
        .unwrap()
        .vertices()
        .iter()
        .map(|v| {
            let p = transform_point(
                object,
                add(v.position, [0.1 * morph_weight, 0.2 * morph_weight, 0.]),
            );
            add(
                mul(transform_point(skin[0], p), 0.25),
                mul(transform_point(skin[1], p), 0.75),
            )
        })
        .collect()
}

#[test]
fn named_pose_bakes_blended_skinning_in_world_space_with_nontrivial_rest_and_object_transforms() {
    let doc = character(true);
    let source = doc.to_bytes(None).unwrap();
    let head = doc.head();
    let actual = baked(
        &doc,
        &ReviewPose::Pose {
            name: "reach".into(),
        },
    );
    let expected = expected_character(&doc, posed_root(), posed_child(), 0.4);
    assert_points(&actual, &expected);
    let rest = expected_character(&doc, rest_root(), rest_child(), 0.4);
    assert!(actual
        .iter()
        .any(|p| rest.iter().all(|q| distance(*p, *q) > 0.1)));
    assert_eq!(doc.head(), head);
    assert_eq!(doc.to_bytes(None).unwrap(), source);
}

#[test]
fn clip_samples_bake_interpolated_joints_and_morphs_and_preserve_source() {
    let doc = character(true);
    let source = doc.to_bytes(None).unwrap();
    let head = doc.head();
    let mut samples = Vec::new();
    for time in [0., 0.5, 1.5, 2.] {
        let actual = baked(
            &doc,
            &ReviewPose::Clip {
                name: "wave".into(),
                time,
            },
        );
        let root = Transform {
            translation: [2. + 0.4 * time, -1. + 0.1 * time, 3. - 0.2 * time],
            ..rest_root()
        };
        let child = Transform {
            rotation: quat_axis_angle([1., 0., 0.], -0.2 + 0.5 * time).unwrap(),
            ..rest_child()
        };
        assert_points(&actual, &expected_character(&doc, root, child, time / 2.));
        samples.push(actual);
        assert_eq!(doc.head(), head);
        assert_eq!(doc.to_bytes(None).unwrap(), source);
    }
    assert!(samples[1]
        .iter()
        .any(|p| samples[2].iter().all(|q| distance(*p, *q) > 0.05)));
    let defaults = doc.default_review_poses();
    assert_eq!(defaults.len(), 3);
    assert!(matches!(&defaults[0], ReviewPose::Pose { name } if name == "reach"));
    assert!(matches!(&defaults[1], ReviewPose::Clip { name, time: 0.5 } if name == "wave"));
    assert!(matches!(&defaults[2], ReviewPose::Clip { name, time: 1.5 } if name == "wave"));
}

#[test]
fn malformed_motion_requests_are_rejected() {
    for text in [
        "[]",
        "{}",
        r#"{"kind":"unknown"}"#,
        r#"{"kind":"vehicle","steer":0,"suspension":0,"extra":true}"#,
        r#"{"kind":"vehicle","steer":0}"#,
        r#"{"kind":"vehicle","steer":"left","suspension":0}"#,
        r#"{"kind":"vehicle","steer":1.21,"suspension":0}"#,
        r#"{"kind":"vehicle","steer":0,"suspension":5.01}"#,
        r#"{"kind":"pose","name":""}"#,
        r#"{"kind":"clip","name":"wave","time":-0.1}"#,
        r#"{"kind":"clip","name":"wave","time":3601}"#,
        r#"{"kind":"clip","name":"wave"}"#,
    ] {
        assert!(
            ReviewPose::from_json(&json::parse(text.as_bytes()).unwrap()).is_err(),
            "accepted {text}"
        );
    }
    let bad = json::obj(vec![
        ("kind", json::s("vehicle")),
        ("steer", json::Value::F64(f64::NAN)),
        ("suspension", json::Value::Int(0)),
    ]);
    assert!(ReviewPose::from_json(&bad).is_err());
    let bad_name = json::obj(vec![
        ("kind", json::s("pose")),
        ("name", json::s(&"p".repeat(97))),
    ]);
    assert!(ReviewPose::from_json(&bad_name).is_err());
}

#[test]
fn invalid_typed_poses_and_cancellation_refuse_without_mutating_documents() {
    let car = vehicle();
    let person = character(true);
    for doc in [&car, &person] {
        let source = doc.to_bytes(None).unwrap();
        let head = doc.head();
        for pose in [
            ReviewPose::Vehicle {
                steer: f64::NAN,
                suspension: 0.,
            },
            ReviewPose::Vehicle {
                steer: 2.,
                suspension: 0.,
            },
            ReviewPose::Vehicle {
                steer: 0.,
                suspension: 6.,
            },
            ReviewPose::Pose {
                name: "missing".into(),
            },
            ReviewPose::Clip {
                name: "missing".into(),
                time: 0.,
            },
            ReviewPose::Clip {
                name: "wave".into(),
                time: f64::NAN,
            },
            ReviewPose::Clip {
                name: "wave".into(),
                time: -1.,
            },
            ReviewPose::Clip {
                name: "wave".into(),
                time: 2.01,
            },
        ] {
            assert!(
                doc.compile_review_pose(&pose, None).is_err(),
                "accepted {pose:?}"
            );
        }
        for pose in doc.default_review_poses() {
            assert_eq!(
                doc.compile_review_pose(&pose, Some(&|| true)).unwrap_err(),
                Error::Mesh(mesh::MeshError::Cancelled)
            );
        }
        assert_eq!(doc.head(), head);
        assert_eq!(doc.to_bytes(None).unwrap(), source);
    }
}

#[test]
fn skeleton_without_weights_fails_instead_of_returning_a_rest_pose() {
    let doc = character(false);
    let source = doc.to_bytes(None).unwrap();
    let head = doc.head();
    for pose in [
        ReviewPose::Pose {
            name: "reach".into(),
        },
        ReviewPose::Clip {
            name: "wave".into(),
            time: 0.5,
        },
    ] {
        assert_eq!(
            doc.compile_review_pose(&pose, None).unwrap_err(),
            Error::Invalid("motion review requires normalized vertex weights")
        );
    }
    assert_eq!(doc.head(), head);
    assert_eq!(doc.to_bytes(None).unwrap(), source);
}
