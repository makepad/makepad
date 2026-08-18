//! Writer→parser round-trip gate for the character chain.
//!
//! `makepad_gltf::write_glb_mesh_skinned` is the chain's animated-GLB
//! emitter; `skin.rs` is the engine's parser and THE acceptance contract
//! (character-chain campaign): whatever the writer emits must load, sample
//! and skin here with exact expected transforms. If either side drifts,
//! this breaks before a box ever renders a wrong character.

use makepad_render::skin::{SkinnedModel, PoseBuffer, SKIN_VERTEX_FLOATS};
use makepad_gltf::{
    write_glb_mesh_skinned, GlbAnimChannel, GlbAnimClip, GlbAnimPath, GlbJoint, GlbSkinnedMesh,
};

const SIN45: f32 = 0.70710678;

/// Two-joint chain: root at origin, tip 1m up. Four single-weight vertices,
/// "walk" rotates the tip 90° about Z while the root bobs up, "idle" is a
/// static scale channel. Every expected position below is hand-derivable.
fn test_rig() -> Vec<u8> {
    let positions = [
        [0.0f32, 0.0, 0.0], // root-bound
        [1.0, 0.0, 0.0],    // root-bound
        [0.0, 1.0, 0.0],    // tip-bound (at the tip joint)
        [0.0, 2.0, 0.0],    // tip-bound (1m past the tip)
    ];
    let indices = [0u32, 1, 2, 1, 3, 2];
    let joints_0 = [[0u16, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0], [1, 0, 0, 0]];
    let weights_0 = [[1.0f32, 0.0, 0.0, 0.0]; 4];
    let joints = [
        GlbJoint::at("root", None, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
        GlbJoint::at("tip", Some(0), [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    let clips = [
        GlbAnimClip {
            name: "walk".into(),
            channels: vec![
                GlbAnimChannel {
                    joint: 1,
                    path: GlbAnimPath::Rotation,
                    times: vec![0.0, 1.0, 2.0],
                    values: vec![
                        0.0, 0.0, 0.0, 1.0, // identity
                        0.0, 0.0, SIN45, SIN45, // 90° about Z
                        0.0, 0.0, 0.0, 1.0,
                    ],
                },
                GlbAnimChannel {
                    joint: 0,
                    path: GlbAnimPath::Translation,
                    times: vec![0.0, 1.0, 2.0],
                    values: vec![0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0],
                },
            ],
        },
        GlbAnimClip {
            name: "idle".into(),
            channels: vec![GlbAnimChannel {
                joint: 1,
                path: GlbAnimPath::Scale,
                times: vec![0.0, 2.0],
                values: vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            }],
        },
    ];
    write_glb_mesh_skinned(&GlbSkinnedMesh {
        positions: &positions,
        normals: None,
        uvs: Some(&[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]),
        indices: &indices,
        joints_0: &joints_0,
        weights_0: &weights_0,
        joints: &joints,
        clips: &clips,
        base_color_png: None,
    })
}

/// Skinned positions for `clip` at `t` through the full engine path
/// (sample → palette → CPU skin), positions extracted from the packed lanes.
fn skinned_positions(model: &SkinnedModel, clip: usize, t: f32) -> Vec<[f32; 3]> {
    let mut pose = PoseBuffer::new();
    model.sample_clip(clip, t, &mut pose);
    let mut palette = Vec::new();
    model.palette(&pose, &mut palette);
    let mut packed = Vec::new();
    model.skin_to_packed(&palette, &mut packed);
    packed
        .chunks_exact(SKIN_VERTEX_FLOATS)
        .map(|v| [v[0], v[1], v[2]])
        .collect()
}

fn assert_close(got: [f32; 3], want: [f32; 3], what: &str) {
    for d in 0..3 {
        assert!(
            (got[d] - want[d]).abs() < 1.0e-5,
            "{what}: got {got:?}, want {want:?}"
        );
    }
}

#[test]
fn writer_output_parses_and_names_clips() {
    let model = SkinnedModel::parse_glb(&test_rig()).expect("engine parses writer output");
    assert_eq!(model.joint_count(), 2);
    assert_eq!(model.vertex_count(), 4);
    assert_eq!(model.indices(), &[0, 1, 2, 1, 3, 2]);
    assert_eq!(model.skipped_unskinned, 0);
    assert_eq!(model.clips.len(), 2);
    assert_eq!(model.clips[0].name, "walk");
    assert_eq!(model.clips[1].name, "idle");
    assert!((model.clips[0].duration - 2.0).abs() < 1.0e-6);
    assert!((model.clips[1].duration - 2.0).abs() < 1.0e-6);
    // The locomotion resolver finds the gait pair by name substring — the
    // motion domain's clip-name contract (idle/walk/jump) rides on this.
    assert_eq!(model.clip_index("walk"), Some(0));
    assert_eq!(model.clip_index_any(&["nope", "idle"]), Some(1));
    assert_eq!(model.gait_clips(), Some((1, 0)));
}

#[test]
fn writer_output_skins_to_exact_positions() {
    let model = SkinnedModel::parse_glb(&test_rig()).expect("engine parses writer output");

    // t=0: rest pose everywhere — identity palette.
    let rest = skinned_positions(&model, 0, 0.0);
    assert_close(rest[0], [0.0, 0.0, 0.0], "rest v0");
    assert_close(rest[1], [1.0, 0.0, 0.0], "rest v1");
    assert_close(rest[2], [0.0, 1.0, 0.0], "rest v2");
    assert_close(rest[3], [0.0, 2.0, 0.0], "rest v3");

    // t=1 (exact key): root bobbed +0.5Y, tip rotated 90° about Z.
    // v3 sits 1m along the tip bone: rotates to -X, rides the bob.
    let mid = skinned_positions(&model, 0, 1.0);
    assert_close(mid[0], [0.0, 0.5, 0.0], "walk@1 v0");
    assert_close(mid[1], [1.0, 0.5, 0.0], "walk@1 v1");
    assert_close(mid[2], [0.0, 1.5, 0.0], "walk@1 v2");
    assert_close(mid[3], [-1.0, 1.5, 0.0], "walk@1 v3");

    // t=0.5 (between keys): translation lerps to +0.25Y; rotation nlerps —
    // for unit quats at f=0.5 that's the exact 45° halfway rotation.
    let quarter = skinned_positions(&model, 0, 0.5);
    assert_close(quarter[0], [0.0, 0.25, 0.0], "walk@.5 v0");
    assert_close(quarter[3], [-SIN45, 1.25 + SIN45, 0.0], "walk@.5 v3");

    // The idle clip leaves the mesh at rest.
    let idle = skinned_positions(&model, 1, 0.7);
    assert_close(idle[3], [0.0, 2.0, 0.0], "idle v3");

    // Bounds stay finite and cover the posed mesh.
    let mut pose = PoseBuffer::new();
    model.sample_clip(0, 1.0, &mut pose);
    let mut palette = Vec::new();
    model.palette(&pose, &mut palette);
    let (mn, mx) = model.posed_bounds(&palette).expect("bounds");
    assert!(mn.x <= -1.0 && mx.y >= 1.5, "posed bounds cover the swing");
}

#[test]
fn prune_weights_removes_cross_limb_stragglers() {
    // A "webbing" vertex: 0.9 tip / 0.1 root — the auto-skinning artifact
    // class (faint cross-limb influence that sags when limbs separate).
    let positions = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, 1.0, 0.0]];
    let joints = [
        GlbJoint::at("root", None, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
        GlbJoint::at("tip", Some(0), [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    // Times span 2s: sample_clip WRAPS t modulo duration, so a clip ending
    // exactly at the probe time would sample the rest pose instead.
    let clips = [GlbAnimClip {
        name: "walk".into(),
        channels: vec![
            GlbAnimChannel {
                joint: 1,
                path: GlbAnimPath::Rotation,
                times: vec![0.0, 1.0, 2.0],
                values: vec![
                    0.0, 0.0, 0.0, 1.0, //
                    0.0, 0.0, SIN45, SIN45, //
                    0.0, 0.0, 0.0, 1.0,
                ],
            },
            GlbAnimChannel {
                joint: 0,
                path: GlbAnimPath::Translation,
                times: vec![0.0, 1.0, 2.0],
                values: vec![0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0],
            },
        ],
    }];
    let glb = write_glb_mesh_skinned(&GlbSkinnedMesh {
        positions: &positions,
        normals: None,
        uvs: None,
        indices: &[0, 1, 2],
        joints_0: &[[0u16, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0]],
        weights_0: &[
            [1.0f32, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.9, 0.1, 0.0, 0.0], // v2: joints [tip, root] via joints_0? see below
        ],
        joints: &joints,
        clips: &clips,
        base_color_png: None,
    });
    // NOTE: v2's joints_0 lane is [1,0,0,0] with weights [0.9, 0.1] — i.e.
    // 0.9 on the tip and a 0.1 straggler on the root (slot 1 = joint 0).
    let mut model = SkinnedModel::parse_glb(&glb).expect("parse");
    // Pre-prune: the straggler pulls v2 off the pure tip transform.
    let before = skinned_positions(&model, 0, 1.0);
    assert_close(before[2], [0.05, 1.95, 0.0], "webbed v2");
    // Prune the 0.1 straggler (below threshold): v2 rides the tip exactly.
    let culled = model.prune_weights(0.15, 0.99);
    assert_eq!(culled, 1);
    let after = skinned_positions(&model, 0, 1.0);
    assert_close(after[0], [0.0, 0.5, 0.0], "root vert untouched");
    assert_close(after[2], [0.0, 2.0, 0.0], "hardened v2");
    // Idempotent on clean weights.
    assert_eq!(model.prune_weights(0.15, 0.99), 0);
    // Hardening: a 0.6/0.4 vertex is untouched at harden_at 0.85 but snaps
    // to its dominant joint at harden_at 0.5.
    let glb2 = write_glb_mesh_skinned(&GlbSkinnedMesh {
        positions: &positions,
        normals: None,
        uvs: None,
        indices: &[0, 1, 2],
        joints_0: &[[0u16, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0]],
        weights_0: &[
            [1.0f32, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.6, 0.4, 0.0, 0.0],
        ],
        joints: &joints,
        clips: &clips,
        base_color_png: None,
    });
    let mut soft = SkinnedModel::parse_glb(&glb2).expect("parse");
    assert_eq!(soft.prune_weights(0.05, 0.85), 0, "0.6 is no dominant at 0.85");
    assert_eq!(soft.prune_weights(0.05, 0.5), 1, "0.6 dominates at 0.5");
    let hard = skinned_positions(&soft, 0, 1.0);
    assert_close(hard[2], [0.0, 2.0, 0.0], "hardened at 0.5");
}

#[test]
fn cull_stretched_triangles_removes_cross_limb_webbing() {
    // Two "limbs" (independent root joints); a bridge triangle spans them.
    // The "spread" clip translates limb B 2m away — the bridge shears to
    // ~5x its rest edge while the intra-limb triangle rides rigidly.
    let positions = [
        [0.0f32, 0.0, 0.0], // A
        [0.2, 0.0, 0.0],    // A
        [0.1, 0.2, 0.0],    // A — intra-limb triangle 0,1,2
        [0.3, 0.0, 0.0],    // B — bridge triangle 1,3,2 spans A/B
    ];
    let joints = [
        GlbJoint::at("limb_a", None, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
        GlbJoint::at("limb_b", None, [0.3, 0.0, 0.0], [0.3, 0.0, 0.0]),
    ];
    let clips = [GlbAnimClip {
        name: "spread".into(),
        channels: vec![GlbAnimChannel {
            joint: 1,
            path: GlbAnimPath::Translation,
            times: vec![0.0, 1.0, 2.0],
            values: vec![0.3, 0.0, 0.0, 2.3, 0.0, 0.0, 0.3, 0.0, 0.0],
        }],
    }];
    let glb = write_glb_mesh_skinned(&GlbSkinnedMesh {
        positions: &positions,
        normals: None,
        uvs: None,
        indices: &[0, 1, 2, 1, 3, 2],
        joints_0: &[[0u16; 4], [0; 4], [0; 4], [1, 0, 0, 0]],
        weights_0: &[[1.0f32, 0.0, 0.0, 0.0]; 4],
        joints: &joints,
        clips: &clips,
        base_color_png: None,
    });
    let mut model = SkinnedModel::parse_glb(&glb).expect("parse");
    assert_eq!(model.indices().len(), 6);
    let culled = model.cull_stretched_triangles(&[0], 6, 3.0);
    assert_eq!(culled, 1, "the bridge triangle goes");
    assert_eq!(model.indices(), &[0, 1, 2], "the intra-limb triangle stays");
}

#[test]
fn deformation_audit_reports_without_mutating_topology() {
    let positions = [
        [0.0f32, 0.0, 0.0],
        [0.2, 0.0, 0.0],
        [0.1, 0.2, 0.0],
        [0.3, 0.0, 0.0],
    ];
    let joints = [
        GlbJoint::at("limb_a", None, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
        GlbJoint::at("limb_b", None, [0.3, 0.0, 0.0], [0.3, 0.0, 0.0]),
    ];
    let clips = [GlbAnimClip {
        name: "spread".into(),
        channels: vec![GlbAnimChannel {
            joint: 1,
            path: GlbAnimPath::Translation,
            times: vec![0.0, 1.0, 2.0],
            values: vec![0.3, 0.0, 0.0, 2.3, 0.0, 0.0, 0.3, 0.0, 0.0],
        }],
    }];
    let glb = write_glb_mesh_skinned(&GlbSkinnedMesh {
        positions: &positions,
        normals: None,
        uvs: None,
        indices: &[0, 1, 2, 1, 3, 2],
        joints_0: &[[0u16; 4], [0; 4], [0; 4], [1, 0, 0, 0]],
        weights_0: &[[1.0f32, 0.0, 0.0, 0.0]; 4],
        joints: &joints,
        clips: &clips,
        base_color_png: None,
    });
    let model = SkinnedModel::parse_glb(&glb).unwrap();
    let indices_before = model.indices().to_vec();
    let audit = model.audit_deformation(&[0], 6);

    assert_eq!(audit.triangles, indices_before.len() / 3);
    assert_eq!(audit.samples, 6);
    assert!(audit.over_3x > 0, "fixture should expose pathological stretch");
    assert!(audit.max_stretch > 3.0);
    assert_eq!(model.indices(), indices_before);
}

#[test]
fn audit_external_generated_character_if_requested() {
    let Some(path) = std::env::var_os("MAKEPAD_SKIN_AUDIT_GLB") else {
        eprintln!("MAKEPAD_SKIN_AUDIT_GLB unset; skipping external audit");
        return;
    };
    let path = std::path::PathBuf::from(path);
    let bytes = std::fs::read(&path).unwrap_or_else(|error| {
        panic!("cannot read deformation-audit input {}: {error}", path.display())
    });
    let model = SkinnedModel::parse_glb(&bytes).unwrap_or_else(|error| {
        panic!("cannot parse deformation-audit input {}: {error}", path.display())
    });
    assert!(!model.clips.is_empty(), "audit input has no animation clips");
    for (clip, animation) in model.clips.iter().enumerate() {
        let audit = model.audit_deformation(&[clip], 12);
        let temporal = model.audit_temporal_motion(clip, 30.0);
        let runtime_loop = model.audit_loop_blended_motion(clip, 30.0, 0.2, 2);
        eprintln!(
            "skin deformation audit: file={} clip={} triangles={} samples={} over_2x={} over_3x={} p95={:.4} p99={:.4} max={:.4}",
            path.display(),
            animation.name,
            audit.triangles,
            audit.samples,
            audit.over_2x,
            audit.over_3x,
            audit.p95_stretch,
            audit.p99_stretch,
            audit.max_stretch,
        );
        eprintln!(
            "skin temporal audit: file={} clip={} frames={} pairs={} joint_max_deg={:.4}@node{}/frame{} joint_p99_deg={:.4} vertex_max={:.6}@v{}/frame{} vertex_p99={:.6} seam_joint_deg={:.4} seam_vertex={:.6}",
            path.display(),
            animation.name,
            temporal.frames,
            temporal.frame_pairs,
            temporal.max_joint_angle_degrees,
            temporal.max_joint_node,
            temporal.max_joint_frame,
            temporal.p99_joint_angle_degrees,
            temporal.max_vertex_delta,
            temporal.max_vertex,
            temporal.max_vertex_frame,
            temporal.p99_vertex_delta,
            temporal.seam_joint_angle_degrees,
            temporal.seam_vertex_delta,
        );
        eprintln!(
            "skin runtime loop audit: file={} clip={} cycles={} frames={} pairs={} wraps={} joint_max_deg={:.4}@node{}/frame{} vertex_max={:.6}@v{}/frame{} wrap_joint_deg={:.4} wrap_vertex={:.6}",
            path.display(),
            animation.name,
            runtime_loop.cycles,
            runtime_loop.frames,
            runtime_loop.frame_pairs,
            runtime_loop.wraps,
            runtime_loop.max_joint_angle_degrees,
            runtime_loop.max_joint_node,
            runtime_loop.max_joint_frame,
            runtime_loop.max_vertex_delta,
            runtime_loop.max_vertex,
            runtime_loop.max_vertex_frame,
            runtime_loop.wrap_joint_angle_degrees,
            runtime_loop.wrap_vertex_delta,
        );
        let outliers = model.temporal_joint_outliers(clip, 30.0);
        let ranked: Vec<_> = outliers
            .iter()
            .take(8)
            .map(|outlier| {
                (
                    outlier.node,
                    model.node_name(outlier.node).unwrap_or("?"),
                    outlier.frame,
                    outlier.angle_degrees,
                )
            })
            .collect();
        eprintln!(
            "skin temporal joint ranking: file={} clip={} top={ranked:?}",
            path.display(),
            animation.name,
        );
        if std::env::var_os("MAKEPAD_SKIN_AUDIT_DETAIL").is_some() {
            let mut previous = PoseBuffer::new();
            let mut current = PoseBuffer::new();
            for frame in 0..temporal.frames {
                let duration = animation.duration;
                let before_wrap = duration - (1.0 / 30.0f32).min(duration) * 1.0e-4;
                let time = ((frame + 1) as f32 / 30.0).min(before_wrap);
                model.sample_clip(clip, time, &mut current);
                if frame > 0 {
                    let mut ranked: Vec<_> = current
                        .iter()
                        .zip(&previous)
                        .enumerate()
                        .map(|(node, (right, left))| {
                            let dot = (left.r.x * right.r.x
                                + left.r.y * right.r.y
                                + left.r.z * right.r.z
                                + left.r.w * right.r.w)
                                .abs()
                                .clamp(-1.0, 1.0);
                            (2.0 * dot.acos() * 180.0 / std::f32::consts::PI, node)
                        })
                        .collect();
                    ranked.sort_by(|left, right| right.0.total_cmp(&left.0));
                    if ranked[0].0 > 20.0 {
                        eprintln!(
                            "skin temporal detail: clip={} frame={} time={:.6} top={:?}",
                            animation.name,
                            frame,
                            time,
                            &ranked[..ranked.len().min(6)]
                        );
                    }
                }
                std::mem::swap(&mut previous, &mut current);
            }
        }
    }
}

#[test]
fn writer_u16_joint_lane_survives_large_rigs() {
    // 300 joints forces the writer off the u8 JOINTS_0 lane; the engine
    // parser reads u16 fine (its 256 limit applies to the GPU packer only).
    let joints: Vec<GlbJoint> = (0..300)
        .map(|i| {
            GlbJoint::at(
                &format!("j{i}"),
                if i == 0 { None } else { Some(i - 1) },
                [0.0, if i == 0 { 0.0 } else { 0.01 }, 0.0],
                [0.0, 0.01 * i as f32, 0.0],
            )
        })
        .collect();
    let positions = [[0.0f32, 3.0, 0.0], [1.0, 3.0, 0.0], [0.0, 4.0, 0.0]];
    let glb = write_glb_mesh_skinned(&GlbSkinnedMesh {
        positions: &positions,
        normals: None,
        uvs: None,
        indices: &[0, 1, 2],
        joints_0: &[[299u16, 0, 0, 0]; 3],
        weights_0: &[[1.0f32, 0.0, 0.0, 0.0]; 3],
        joints: &joints,
        clips: &[],
        base_color_png: None,
    });
    let model = SkinnedModel::parse_glb(&glb).expect("u16 joints parse");
    assert_eq!(model.joint_count(), 300);
    // Rest pose = bind pose: vertices come back where they were authored.
    let rest = skinned_positions(&model, 0, 0.0); // no clips: rest sample
    assert_close(rest[0], [0.0, 3.0, 0.0], "u16 rest v0");
    assert_close(rest[2], [0.0, 4.0, 0.0], "u16 rest v2");
}
