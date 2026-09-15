use makepad_model::transform::*;
use makepad_model::*;
use makepad_render::{StaticModel, MODEL_VERTEX_FLOATS};

fn apply(doc: &mut Document, id: &str, operations: Vec<Operation>) {
    doc.apply(
        Transaction {
            request_id: id.into(),
            expected: doc.head(),
            operations,
        },
        None,
    )
    .unwrap();
}
fn commands(doc: &mut Document, id: &str, text: &str) {
    let operations =
        parse_operations(&json::parse(text.as_bytes()).unwrap(), doc.limits()).unwrap();
    apply(doc, id, operations);
}
fn fixture() -> Document {
    let mut doc = Document::new(Limits {
        max_joints: 128,
        ..Default::default()
    })
    .unwrap();
    commands(
        &mut doc,
        "shapes",
        r#"[
      {"op":"sphere","object":"body","radius":0.45,"segments":16,"rings":8},
      {"op":"sphere","object":"eye","radius":0.14,"segments":12,"rings":6},
      {"op":"object_node","object":"body","node":{"transform":{"translation":[0,0.7,0]}}},
      {"op":"object_node","object":"eye","node":{"transform":{"translation":[0.18,0.84,-0.4]}}},
      {"op":"surface_material","material":0,"roughness":0.12}
    ]"#,
    );
    commands(
        &mut doc,
        "bind",
        r#"[{"op":"soft_body_bind","object":"body","attachments":[{"name":"eye-frame","objects":["eye"],"pivot":[0.18,0.84,-0.4]}]}]"#,
    );
    doc
}
fn part<'a>(compiled: &'a CompiledModel, name: &str) -> &'a PrimitiveSource {
    compiled
        .primitives
        .iter()
        .find(|p| p.object == name)
        .unwrap()
}
fn max_displacement(a: &[[f32; 3]], b: &[[f32; 3]]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(a, b)| length(sub(a.map(f64::from), b.map(f64::from))))
        .fold(0., f64::max)
}

#[test]
fn actual_soft_scenarios_deform_body_keep_eyes_rigid_and_export_smooth_static_frames() {
    let doc = fixture();
    let source = doc.to_bytes(None).unwrap();
    let head = doc.head();
    let rest = doc.compile(None).unwrap();
    let rest_body = part(&rest, "body");
    let rest_eye = part(&rest, "eye");
    let defaults = doc.default_review_poses();
    assert_eq!(defaults.len(), 3);
    let mut body_frames = Vec::new();
    for scenario in ["acceleration", "landing", "wall"] {
        let output = doc
            .compile_review_pose(
                &ReviewPose::SoftBody {
                    scenario: scenario.into(),
                },
                None,
            )
            .unwrap();
        let body = part(&output, "body");
        let eye = part(&output, "eye");
        assert!(
            max_displacement(&body.positions, &rest_body.positions) > 0.002,
            "{scenario} did not physically deform"
        );
        // A single body anchor would rigidly move every vertex by the same
        // delta. The solved shell must have spatially varying displacement.
        let deltas = body
            .positions
            .iter()
            .zip(&rest_body.positions)
            .map(|(a, b)| sub(a.map(f64::from), b.map(f64::from)))
            .collect::<Vec<_>>();
        let variation = deltas
            .iter()
            .map(|delta| length(sub(*delta, deltas[0])))
            .fold(0., f64::max);
        eprintln!(
            "{scenario}: displacement={}, shape variation={variation}",
            max_displacement(&body.positions, &rest_body.positions)
        );
        assert!(
            variation > 1e-5,
            "{scenario} only translated rigidly: {variation}"
        );
        assert!(
            max_displacement(&eye.positions, &rest_eye.positions) > 0.0001,
            "{scenario} did not move attached eye"
        );
        for (i, point) in eye.positions.iter().enumerate().step_by(7) {
            for j in [0, eye.positions.len() / 3, eye.positions.len() / 2] {
                let posed = length(sub(point.map(f64::from), eye.positions[j].map(f64::from)));
                let authored = length(sub(
                    rest_eye.positions[i].map(f64::from),
                    rest_eye.positions[j].map(f64::from),
                ));
                assert!(
                    (posed - authored).abs() < 2e-5,
                    "{scenario} distorted the rigid eye"
                );
            }
        }
        for primitive in &output.primitives {
            assert!(primitive
                .normals
                .iter()
                .all(|n| n.iter().all(|v| v.is_finite())
                    && (length(n.map(f64::from)) - 1.).abs() < 1e-5));
            let smooth = primitive
                .normals
                .chunks_exact(3)
                .filter(|triangle| {
                    length(sub(triangle[0].map(f64::from), triangle[1].map(f64::from))) > 0.01
                })
                .count();
            assert!(
                smooth > primitive.normals.len() / 12,
                "{scenario} flattened explicit sphere normals"
            );
        }
        let parsed = makepad_gltf::parse_glb_bytes(&output.glb).unwrap();
        assert!(parsed.document.skins.as_ref().is_none_or(Vec::is_empty));
        assert!(parsed
            .document
            .animations
            .as_ref()
            .is_none_or(Vec::is_empty));
        let static_model = StaticModel::parse_glb(&output.glb).unwrap();
        assert_eq!(static_model.indices.len(), output.triangles * 3);
        assert_eq!(doc.head(), head);
        assert_eq!(doc.to_bytes(None).unwrap(), source);
        body_frames.push(body.positions.clone());
    }
    for (a, b) in [(0, 1), (0, 2), (1, 2)] {
        assert!(max_displacement(&body_frames[a], &body_frames[b]) > 0.002);
    }
    let reopened = Document::from_bytes(&source, doc.limits().clone(), None).unwrap();
    assert_eq!(reopened.to_bytes(None).unwrap(), source);
    assert_eq!(
        reopened
            .compile_review_pose(&defaults[0], None)
            .unwrap()
            .glb,
        doc.compile_review_pose(&defaults[0], None).unwrap().glb
    );
}

#[test]
fn wall_and_landing_reviews_keep_visible_body_vertices_outside_contact_planes() {
    let doc = fixture();
    // This fixture has 114 body vertices, all retained as collision samples;
    // the enclosing control cage and the rigid eye are not contact surfaces.
    assert_eq!(
        doc.soft_body().unwrap().surface_samples.len(),
        doc.object("body").unwrap().vertices().len()
    );
    for scenario in ["wall", "landing"] {
        let output = doc
            .compile_review_pose(
                &ReviewPose::SoftBody {
                    scenario: scenario.into(),
                },
                None,
            )
            .unwrap();
        let body = part(&output, "body");
        if scenario == "wall" {
            let plane_x = 0.45 - 0.45 * 0.18 * (34. / 35.);
            let max_x = body
                .positions
                .iter()
                .map(|p| p[0])
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(
                max_x <= plane_x + 0.003,
                "wall penetrated by {} m",
                max_x - plane_x
            );
        } else {
            let min_y = body
                .positions
                .iter()
                .map(|p| p[1])
                .fold(f32::INFINITY, f32::min);
            assert!(
                min_y >= 0.25 - 0.003,
                "ground penetrated by {} m",
                0.25 - min_y
            );
        }
    }
}

#[test]
fn soft_review_invalid_requests_and_mid_solve_cancellation_leave_source_unchanged() {
    let doc = fixture();
    let before = doc.to_bytes(None).unwrap();
    assert!(doc
        .compile_review_pose(
            &ReviewPose::SoftBody {
                scenario: "unrecognized".into()
            },
            None
        )
        .is_err());
    let polls = std::cell::Cell::new(0usize);
    let cancelled = || {
        polls.set(polls.get() + 1);
        polls.get() > 20
    };
    assert!(doc
        .compile_review_pose(
            &ReviewPose::SoftBody {
                scenario: "wall".into()
            },
            Some(&cancelled)
        )
        .is_err());
    assert!(polls.get() > 20);
    assert_eq!(doc.to_bytes(None).unwrap(), before);
    for text in [
        r#"{"kind":"soft_body","scenario":"settled"}"#,
        r#"{"kind":"soft_body","scenario":"wall","time":0}"#,
        r#"{"kind":"soft_body","scenario":3}"#,
    ] {
        assert!(ReviewPose::from_json(&json::parse(text.as_bytes()).unwrap()).is_err());
    }
    let mut unbound = Document::new(Limits::default()).unwrap();
    commands(
        &mut unbound,
        "sphere",
        r#"[{"op":"sphere","object":"body","radius":1,"segments":12,"rings":6}]"#,
    );
    assert!(unbound
        .compile_review_pose(
            &ReviewPose::SoftBody {
                scenario: "wall".into()
            },
            None
        )
        .is_err());
}

#[test]
fn imported_static_node_scale_uses_inverse_transpose_without_double_mirror_sign() {
    for scale in [[2.3, 0.6, 1.4], [-2.3, 0.6, 1.4]] {
        let mut doc = Document::new(Limits::default()).unwrap();
        commands(
            &mut doc,
            "sphere",
            r#"[{"op":"sphere","object":"body","radius":0.7,"segments":12,"rings":8}]"#,
        );
        let frame = Transform {
            translation: [3., -1., 2.],
            rotation: quat_axis_angle([1., 2., 3.], 0.67).unwrap(),
            scale,
        };
        apply(
            &mut doc,
            "node",
            vec![Operation::Scene(SceneOperation::Node {
                object: "body".into(),
                node: SceneNode {
                    transform: frame,
                    ..Default::default()
                },
            })],
        );
        let compiled = doc.compile(None).unwrap();
        let parsed = StaticModel::parse_glb(&compiled.glb).unwrap();
        let rest = part(&compiled, "body");
        for vertex in parsed.vertices.chunks_exact(MODEL_VERTEX_FLOATS) {
            let actual_position = [vertex[0] as f64, vertex[1] as f64, vertex[2] as f64];
            let nearest = rest
                .positions
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    length(sub(
                        actual_position,
                        transform_point(frame.matrix().unwrap(), a.map(f64::from)),
                    ))
                    .total_cmp(&length(sub(
                        actual_position,
                        transform_point(frame.matrix().unwrap(), b.map(f64::from)),
                    )))
                })
                .unwrap()
                .0;
            let original = rest.normals[nearest].map(f64::from);
            let expected = normalized(quat_rotate(
                frame.rotation,
                std::array::from_fn(|i| original[i] / scale[i]),
            ))
            .unwrap();
            // Decode the actual packed octahedral normal consumed by shaders.
            let bits = vertex[3].to_bits();
            let half = |bits: u16| {
                let exponent = ((bits >> 10) & 31) as i32;
                assert_ne!(exponent, 31, "non-finite packed normal");
                let fraction = (bits & 1023) as f64;
                let magnitude = if exponent == 0 {
                    fraction * 2_f64.powi(-24)
                } else {
                    (1024. + fraction) * 2_f64.powi(exponent - 25)
                };
                if bits & 32768 != 0 {
                    -magnitude
                } else {
                    magnitude
                }
            };
            let x = half(bits as u16);
            let y = half((bits >> 16) as u16);
            let z = 1. - x.abs() - y.abs();
            let n = if z < 0. {
                [
                    (1. - y.abs()) * if x < 0. { -1. } else { 1. },
                    (1. - x.abs()) * if y < 0. { -1. } else { 1. },
                    z,
                ]
            } else {
                [x, y, z]
            };
            let actual = normalized(n).unwrap();
            assert!(
                dot(actual, expected) > 0.999,
                "packed normal {actual:?}, expected {expected:?}"
            );
        }
    }
}

#[test]
fn runtime_affine_palette_keeps_animated_gaze_below_a_rigid_physical_eye_frame() {
    let mut doc = fixture();
    let metadata = doc.soft_body().unwrap().clone();
    let attachment = &metadata.attachments[0];
    let eye_joint = attachment.joint as usize;
    let mut skeleton = doc.skeleton().unwrap().clone();
    let gaze_joint = skeleton.joints.len();
    let local = [0.03, 0.02, -0.05];
    skeleton.joints.push(Joint {
        name: "gaze-control".into(),
        parent: Some(eye_joint as u32),
        translation: local,
    });
    let mut operations = vec![Operation::SetSkeleton { skeleton }];
    operations.extend(doc.object("eye").unwrap().vertices().iter().map(|v| {
        Operation::SetWeights {
            object: "eye".into(),
            vertex: v.id,
            weights: vec![mesh::JointWeight {
                joint: gaze_joint as u32,
                weight: 1.,
            }],
        }
    }));
    apply(&mut doc, "gaze", operations);
    let compiled = doc.compile(None).unwrap();
    let model = makepad_render::skin::SkinnedModel::parse_glb_validated(&compiled.glb).unwrap();
    let mut pose = model.rest_pose();
    let gaze_node = model.node_index("gaze-control").unwrap();
    let gaze_rotation = quat_axis_angle([1., 0., 0.], 0.3).unwrap();
    pose[gaze_node].r.x = gaze_rotation[0] as f32;
    pose[gaze_node].r.y = gaze_rotation[1] as f32;
    pose[gaze_node].r.z = gaze_rotation[2] as f32;
    pose[gaze_node].r.w = gaze_rotation[3] as f32;
    let pivot = attachment.rest_pivot.map(f64::from);
    let physical_global = Transform {
        translation: add(pivot, [0.02, -0.01, 0.04]),
        rotation: quat_axis_angle([0., 1., 0.], 0.4).unwrap(),
        ..Default::default()
    }
    .matrix()
    .unwrap();
    let eye_rest = Transform {
        translation: pivot,
        ..Default::default()
    }
    .matrix()
    .unwrap();
    let deformation = matrix_mul(physical_global, inverse(eye_rest).unwrap());
    let mut palette = Vec::new();
    model.palette(&pose, &mut palette);
    let mut matrix = palette[eye_joint];
    for row in 0..4 {
        for col in 0..4 {
            matrix.v[col * 4 + row] = deformation[row][col] as f32;
        }
    }
    model.palette_with_affine_overrides(&pose, &[(eye_joint as u16, matrix)], &mut palette);
    let expected_global = matrix_mul(
        physical_global,
        Transform {
            translation: local,
            rotation: gaze_rotation,
            ..Default::default()
        }
        .matrix()
        .unwrap(),
    );
    let gaze_rest = Transform {
        translation: add(pivot, local),
        ..Default::default()
    }
    .matrix()
    .unwrap();
    let expected_palette = matrix_mul(expected_global, inverse(gaze_rest).unwrap());
    for row in 0..4 {
        for col in 0..4 {
            assert!(
                (palette[eye_joint].v[col * 4 + row] as f64 - deformation[row][col]).abs() < 2e-5
            );
            assert!(
                (palette[gaze_joint].v[col * 4 + row] as f64 - expected_palette[row][col]).abs()
                    < 2e-5,
                "gaze pose lost below physical frame"
            );
        }
    }
    let socket = model
        .node_mesh_transform_from_palette(&pose, &palette, gaze_node)
        .unwrap();
    for row in 0..4 {
        for col in 0..4 {
            assert!(
                (socket.v[col * 4 + row] as f64 - expected_global[row][col]).abs() < 2e-5,
                "socket/light frame disagrees with skin"
            );
        }
    }
}
