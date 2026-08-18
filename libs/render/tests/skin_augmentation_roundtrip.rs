//! Lossless SkinTokens augmentation -> engine parser/deformation gate.

use makepad_render::skin::{PoseBuffer, SkinnedModel, SKIN_VERTEX_FLOATS};
use makepad_gltf::{
    augment_glb_skin, replace_glb_node_animations, write_glb_mesh, GlbAnimPath,
    GlbNodeAnimChannel, GlbNodeAnimClip, GlbPrimitiveSkin, GlbSkinJoint,
};

fn positions(model: &SkinnedModel, clip: Option<(usize, f32)>) -> Vec<[f32; 3]> {
    let pose = match clip {
        Some((clip, time)) => {
            let mut pose = PoseBuffer::new();
            model.sample_clip(clip, time, &mut pose);
            pose
        }
        None => model.rest_pose(),
    };
    let mut palette = Vec::new();
    model.palette(&pose, &mut palette);
    let mut packed = Vec::new();
    model.skin_to_packed(&palette, &mut packed);
    packed
        .chunks_exact(SKIN_VERTEX_FLOATS)
        .map(|vertex| [vertex[0], vertex[1], vertex[2]])
        .collect()
}

fn assert_point(actual: [f32; 3], expected: [f32; 3]) {
    for axis in 0..3 {
        assert!(
            (actual[axis] - expected[axis]).abs() < 2.0e-5,
            "actual={actual:?}, expected={expected:?}"
        );
    }
}

#[test]
fn augmented_mesh_is_identity_at_bind_and_deforms_about_child_head() {
    let source_positions = [
        [0.0f32, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 2.0, 0.0],
    ];
    let source = write_glb_mesh(&source_positions, &[0, 1, 2, 1, 3, 2]);
    let joints = [
        GlbSkinJoint {
            name: "root".into(),
            parent: None,
            global_translation: [0.0, 0.0, 0.0],
        },
        GlbSkinJoint {
            name: "child".into(),
            parent: Some(0),
            global_translation: [0.0, 1.0, 0.0],
        },
    ];
    let rigged = augment_glb_skin(
        &source,
        &joints,
        &[GlbPrimitiveSkin {
            node: 0,
            primitive: 0,
            weights: vec![
                1.0, 0.0, // v0 root
                1.0, 0.0, // v1 root
                0.0, 1.0, // v2 child
                0.0, 1.0, // v3 child
            ],
        }],
    )
    .unwrap();
    let child_node = 2;
    let animated = replace_glb_node_animations(
        &rigged,
        &[GlbNodeAnimClip {
            name: "walk".into(),
            channels: vec![GlbNodeAnimChannel {
                node: child_node,
                path: GlbAnimPath::Rotation,
                times: vec![0.0, 1.0, 2.0],
                values: vec![
                    0.0, 0.0, 0.0, 1.0,
                    0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2,
                    std::f32::consts::FRAC_1_SQRT_2,
                    0.0, 0.0, 0.0, 1.0,
                ],
            }],
        }],
    )
    .unwrap();
    let model = SkinnedModel::parse_glb(&animated).expect("engine accepts augmented character");
    assert_eq!(model.joint_count(), 2);
    assert_eq!(model.vertex_count(), 4);
    for (actual, expected) in positions(&model, None).into_iter().zip(source_positions) {
        assert_point(actual, expected);
    }
    let posed = positions(&model, Some((0, 1.0)));
    assert_point(posed[0], [0.0, 0.0, 0.0]);
    assert_point(posed[2], [0.0, 1.0, 0.0]);
    assert_point(posed[3], [-1.0, 1.0, 0.0]);
}
