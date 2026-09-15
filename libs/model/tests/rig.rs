use makepad_gltf::{JsonValue, ParsedGlb};
use makepad_model::*;

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
fn skeleton() -> Skeleton {
    Skeleton {
        joints: vec![
            Joint {
                name: "root".into(),
                parent: None,
                translation: [0., -1., 0.],
            },
            Joint {
                name: "arm".into(),
                parent: Some(0),
                translation: [0., 1., 0.],
            },
        ],
    }
}
fn wave() -> AnimationClip {
    AnimationClip {
        name: "wave".into(),
        channels: vec![AnimationChannel {
            joint: 1,
            path: AnimationPath::Rotation,
            keys: vec![
                Keyframe {
                    time: 0.,
                    value: [0., 0., 0., 1.],
                },
                Keyframe {
                    time: 1.,
                    value: [
                        0.,
                        0.,
                        std::f64::consts::FRAC_1_SQRT_2,
                        std::f64::consts::FRAC_1_SQRT_2,
                    ],
                },
            ],
        }],
    }
}
fn number(v: &JsonValue) -> usize {
    match v {
        JsonValue::U64(v) => *v as usize,
        JsonValue::I64(v) => *v as usize,
        JsonValue::F64(v) => *v as usize,
        _ => panic!("number"),
    }
}
fn array(v: &JsonValue) -> &[JsonValue] {
    match v {
        JsonValue::Array(v) => v,
        _ => panic!("array"),
    }
}
fn accessor_bytes(parsed: &ParsedGlb, index: usize) -> &[u8] {
    let a = &parsed.document.accessors_slice()[index];
    let v = &parsed.document.buffer_views_slice()[a.buffer_view.unwrap()];
    let offset = v.byte_offset.unwrap_or(0) + a.byte_offset.unwrap_or(0);
    &parsed.bin_chunk.as_ref().unwrap()[offset..]
}
fn f32s(parsed: &ParsedGlb, index: usize, count: usize) -> Vec<f32> {
    accessor_bytes(parsed, index)[..count * 4]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

#[test]
fn character_roundtrip_preserves_materials_skin_bind_and_animation_accessors() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(
        &mut doc,
        "body",
        vec![Operation::Cube {
            object: "body".into(),
            size: [2.; 3],
        }],
    );
    let face = doc.object("body").unwrap().faces()[0].id;
    apply(
        &mut doc,
        "rig",
        vec![
            Operation::SetMaterial {
                material: 7,
                value: Material {
                    color: [0.2, 0.4, 0.8],
                    ..Default::default()
                },
            },
            Operation::TextureSolid {
                material: 7,
                width: 8,
                height: 8,
                color: [80, 120, 200],
            },
            Operation::PaintTexture {
                material: 7,
                center: [0.5, 0.5],
                radius: 0.25,
                color: [250, 40, 30],
            },
            Operation::AssignMaterial {
                object: "body".into(),
                faces: vec![face],
                material: 7,
            },
            Operation::SetSkeleton {
                skeleton: skeleton(),
            },
            Operation::AutoWeights {
                object: "body".into(),
            },
            Operation::SetClip { clip: wave() },
        ],
    );
    let compiled = doc.compile(None).unwrap();
    let parsed = makepad_gltf::parse_glb_bytes(&compiled.glb).unwrap();
    assert_eq!(parsed.document.meshes_slice()[0].primitives.len(), 2);
    assert_eq!(parsed.document.materials_slice().len(), 1);
    assert_eq!(parsed.document.images_slice().len(), 1);
    assert_eq!(
        parsed.document.nodes_slice()[2].translation,
        Some([0., 1., 0.])
    );
    let skin = &parsed.document.skins.as_ref().unwrap()[0];
    assert_eq!(
        array(skin.key("joints").unwrap())
            .iter()
            .map(number)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    let inverse = f32s(
        &parsed,
        number(skin.key("inverseBindMatrices").unwrap()),
        32,
    );
    assert_eq!(&inverse[12..15], &[0., 1., 0.]);
    assert_eq!(&inverse[28..31], &[0., 0., 0.]);
    let expected = doc.skeleton().unwrap().inverse_bind_matrices().unwrap();
    assert_eq!(expected[0][1][3], 1.);
    let mut saw_arm = false;
    for primitive in &parsed.document.meshes_slice()[0].primitives {
        let weights = primitive.attributes["WEIGHTS_0"];
        let joints = primitive.attributes["JOINTS_0"];
        let count = parsed.document.accessors_slice()[weights].count;
        let values = f32s(&parsed, weights, count * 4);
        let indices = accessor_bytes(&parsed, joints);
        assert_eq!(
            parsed.document.accessors_slice()[joints].component_type,
            5121
        );
        for i in 0..count {
            assert!((values[i * 4..i * 4 + 4].iter().sum::<f32>() - 1.).abs() < 1e-6);
            saw_arm |= indices[i * 4] == 1;
        }
        assert!(primitive.material.is_some());
        assert!(primitive.attributes.contains_key("TEXCOORD_0"));
    }
    assert!(saw_arm);
    let animation = &parsed.document.animations.as_ref().unwrap()[0];
    assert_eq!(animation.key("name").unwrap().string().unwrap(), "wave");
    let channel = &array(animation.key("channels").unwrap())[0];
    assert_eq!(
        number(channel.key("target").unwrap().key("node").unwrap()),
        2
    );
    let sampler = &array(animation.key("samplers").unwrap())[0];
    assert_eq!(
        f32s(&parsed, number(sampler.key("input").unwrap()), 2),
        vec![0., 1.]
    );
    let q = f32s(&parsed, number(sampler.key("output").unwrap()), 8);
    assert!((q[6] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    let before = doc.head();
    doc.checkpoint(before).unwrap();
    assert_eq!(doc.head().content, before.content);
    assert_eq!(doc.head().generation, before.generation + 1);
    let source = doc.to_bytes(None).unwrap();
    let reopened = Document::from_bytes(&source, Limits::default(), None).unwrap();
    assert_eq!(reopened.to_bytes(None).unwrap(), source);
    assert_eq!(reopened.compile(None).unwrap().glb, compiled.glb);
    assert_eq!(reopened.skeleton(), doc.skeleton());
    assert_eq!(reopened.clips(), doc.clips());
}

#[test]
fn invalid_rig_and_animation_roll_back_and_compile_never_prunes_weights() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(
        &mut doc,
        "cube",
        vec![Operation::Cube {
            object: "body".into(),
            size: [1.; 3],
        }],
    );
    let before = doc.to_bytes(None).unwrap();
    let mut bad = skeleton();
    bad.joints[0].parent = Some(1);
    assert!(doc
        .apply(
            Transaction {
                request_id: "bad".into(),
                expected: doc.head(),
                operations: vec![Operation::SetSkeleton { skeleton: bad }]
            },
            None
        )
        .is_err());
    assert_eq!(doc.to_bytes(None).unwrap(), before);
    let mut rig = skeleton();
    for i in 2..5 {
        rig.joints.push(Joint {
            name: format!("j{i}"),
            parent: Some(0),
            translation: [i as f64, 0., 0.],
        });
    }
    apply(
        &mut doc,
        "bind",
        vec![
            Operation::SetSkeleton { skeleton: rig },
            Operation::AutoWeights {
                object: "body".into(),
            },
        ],
    );
    let vertex = doc.object("body").unwrap().vertices()[0].id;
    apply(
        &mut doc,
        "many",
        vec![Operation::SetWeights {
            object: "body".into(),
            vertex,
            weights: (0..5)
                .map(|joint| mesh::JointWeight { joint, weight: 0.2 })
                .collect(),
        }],
    );
    assert!(matches!(doc.compile(None), Err(Error::Invalid(_))));
    assert_eq!(
        doc.object("body")
            .unwrap()
            .vertex(vertex)
            .unwrap()
            .weights
            .len(),
        5
    );
    let before = doc.to_bytes(None).unwrap();
    let mut clip = wave();
    clip.channels[0].keys[1].time = 0.;
    assert!(doc
        .apply(
            Transaction {
                request_id: "bad-time".into(),
                expected: doc.head(),
                operations: vec![Operation::SetClip { clip }]
            },
            None
        )
        .is_err());
    assert_eq!(doc.to_bytes(None).unwrap(), before);
    let mut changed = skeleton();
    changed.joints[0].name = "other".into();
    assert!(doc
        .apply(
            Transaction {
                request_id: "retarget".into(),
                expected: doc.head(),
                operations: vec![Operation::SetSkeleton { skeleton: changed }]
            },
            None
        )
        .is_err());
}

#[test]
fn procedural_texture_logs_replay_and_cancel_atomically() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(
        &mut doc,
        "texture",
        vec![Operation::TextureSolid {
            material: 0,
            width: 32,
            height: 32,
            color: [10, 20, 30],
        }],
    );
    let solid = doc.materials()[&0].base_color_png.clone();
    apply(
        &mut doc,
        "paint",
        vec![Operation::PaintTexture {
            material: 0,
            center: [0.5, 0.5],
            radius: 0.1,
            color: [255, 0, 0],
        }],
    );
    let painted = doc.materials()[&0].base_color_png.clone();
    assert_ne!(painted, solid);
    let source = doc.to_bytes(None).unwrap();
    let reopened = Document::from_bytes(&source, Limits::default(), None).unwrap();
    assert_eq!(reopened.materials()[&0].base_color_png, painted);
    doc.undo(doc.head(), None).unwrap();
    assert_eq!(doc.materials()[&0].base_color_png, solid);
    doc.redo(doc.head(), None).unwrap();
    assert_eq!(doc.materials()[&0].base_color_png, painted);
    let before = doc.to_bytes(None).unwrap();
    let calls = std::cell::Cell::new(0);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() > 12
    };
    assert!(doc
        .apply(
            Transaction {
                request_id: "cancel".into(),
                expected: doc.head(),
                operations: vec![Operation::PaintTexture {
                    material: 0,
                    center: [0.5, 0.5],
                    radius: 0.9,
                    color: [0, 255, 0]
                }]
            },
            Some(&cancel)
        )
        .is_err());
    assert_eq!(doc.to_bytes(None).unwrap(), before);
}

#[test]
fn expanded_modeling_operations_survive_source_replay() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(
        &mut doc,
        "cube",
        vec![Operation::Cube {
            object: "body".into(),
            size: [2.; 3],
        }],
    );
    let face = doc.object("body").unwrap().faces()[1].id;
    let corners = doc.object("body").unwrap().face_corners(face).unwrap();
    let corner = corners[0].id;
    let edge = mesh::EdgeKey::new(corners[0].vertex, corners[1].vertex);
    apply(
        &mut doc,
        "detail",
        vec![
            Operation::EdgeAttributes {
                object: "body".into(),
                edge,
                attributes: mesh::EdgeAttributes {
                    seam: true,
                    crease: 0.5,
                },
            },
            Operation::CornerNormal {
                object: "body".into(),
                corner,
                normal: Some([0., 0., 1.]),
            },
            Operation::Inset {
                object: "body".into(),
                face,
                distance: 0.2,
            },
            Operation::Weld {
                object: "body".into(),
                vertices: vec![],
                distance: 0.,
            },
        ],
    );
    let source = doc.to_bytes(None).unwrap();
    let reopened = Document::from_bytes(&source, Limits::default(), None).unwrap();
    assert_eq!(reopened.to_bytes(None).unwrap(), source);
    assert!(
        reopened.object("body").unwrap().edge_attributes()[&edge]
            .attributes
            .seam
    );
    assert_eq!(reopened.compile(None).unwrap().triangles, 20);
}

#[test]
fn organic_modifier_log_replays_to_same_skinned_output() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(
        &mut doc,
        "base",
        vec![
            Operation::Cube {
                object: "body".into(),
                size: [2.; 3],
            },
            Operation::SetSkeleton {
                skeleton: skeleton(),
            },
            Operation::AutoWeights {
                object: "body".into(),
            },
            Operation::Subdivide {
                object: "body".into(),
                levels: 2,
            },
        ],
    );
    let vertices = doc
        .object("body")
        .unwrap()
        .vertices()
        .iter()
        .map(|v| v.id)
        .collect::<Vec<_>>();
    let faces = doc
        .object("body")
        .unwrap()
        .faces()
        .iter()
        .map(|f| f.id)
        .collect::<Vec<_>>();
    apply(
        &mut doc,
        "sculpt",
        vec![
            Operation::Smooth {
                object: "body".into(),
                vertices: vertices.clone(),
                iterations: 2,
                factor: 0.2,
                preserve_boundary: true,
            },
            Operation::Brush {
                object: "body".into(),
                vertices,
                center: [0., 0.8, 0.],
                radius: 0.7,
                delta: [0., 0.1, 0.],
                max_displacement: 0.1,
            },
            Operation::ProjectUv {
                object: "body".into(),
                faces,
                axis: 2,
                scale: [0.5; 2],
                offset: [0.5; 2],
            },
            Operation::SetClip { clip: wave() },
        ],
    );
    let glb = doc.compile(None).unwrap();
    assert_eq!(glb.triangles, 192);
    let source = doc.to_bytes(None).unwrap();
    let reopened = Document::from_bytes(&source, Limits::default(), None).unwrap();
    assert_eq!(reopened.compile(None).unwrap().glb, glb.glb);
    assert_eq!(reopened.to_bytes(None).unwrap(), source);
}
