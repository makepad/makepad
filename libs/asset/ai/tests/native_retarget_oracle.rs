#![cfg(feature = "motion-native")]

use makepad_asset_ai::motion_retarget::{
    retarget_hy_motion_glb_with_report, HyMotionClipRef, RetargetOptions,
};
use makepad_ai_motion::hy_motion_decode::HyMotionDecoded;
use makepad_gltf::parse_glb_bytes;
use makepad_micro_serde::JsonValue;
use makepad_zip_file::zip_read_central_directory;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/character_verify").join(name)
}

fn zip_member(path: &Path, name: &str) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap();
    let mut cursor = Cursor::new(bytes);
    let directory = zip_read_central_directory(&mut cursor).unwrap();
    directory
        .file_headers
        .iter()
        .find(|header| header.file_name == name)
        .unwrap_or_else(|| panic!("{} has no {name}", path.display()))
        .extract(&mut cursor)
        .unwrap()
}

fn npy_f32(bytes: &[u8]) -> (Vec<usize>, Vec<f32>) {
    assert_eq!(&bytes[..6], b"\x93NUMPY");
    let major = bytes[6];
    let (header_len, data_start) = match major {
        1 => (u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize, 10),
        2 | 3 => (u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize, 12),
        _ => panic!("unsupported npy version {major}"),
    };
    let header = std::str::from_utf8(&bytes[data_start..data_start + header_len]).unwrap();
    assert!(header.contains("'<f4'") || header.contains("\"<f4\""), "{header}");
    assert!(header.contains("False"), "Fortran npy unsupported: {header}");
    let shape_body = header
        .split("'shape':")
        .nth(1)
        .unwrap()
        .split('(')
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap();
    let shape: Vec<usize> = shape_body
        .split(',')
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    let data = &bytes[data_start + header_len..];
    assert_eq!(data.len() % 4, 0);
    let values = data
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    (shape, values)
}

fn axis_angle_matrix(axis_angle: &[f32]) -> [f32; 9] {
    let v = [axis_angle[0] as f64, axis_angle[1] as f64, axis_angle[2] as f64];
    let theta = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if theta < 1.0e-9 {
        return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    }
    let a = [v[0] / theta, v[1] / theta, v[2] / theta];
    let (sin, cos) = theta.sin_cos();
    let one = 1.0 - cos;
    [
        (cos + a[0] * a[0] * one) as f32,
        (a[0] * a[1] * one - a[2] * sin) as f32,
        (a[0] * a[2] * one + a[1] * sin) as f32,
        (a[1] * a[0] * one + a[2] * sin) as f32,
        (cos + a[1] * a[1] * one) as f32,
        (a[1] * a[2] * one - a[0] * sin) as f32,
        (a[2] * a[0] * one - a[1] * sin) as f32,
        (a[2] * a[1] * one + a[0] * sin) as f32,
        (cos + a[2] * a[2] * one) as f32,
    ]
}

fn oracle_motion() -> Option<HyMotionDecoded> {
    let path = fixture("hy_debug/oracle_walk_official.npz");
    if !path.is_file() {
        return None;
    }
    let (trans_shape, translations) = npy_f32(&zip_member(&path, "trans.npy"));
    let (pose_shape, poses) = npy_f32(&zip_member(&path, "poses.npy"));
    let (key_shape, keypoints_3d) = npy_f32(&zip_member(&path, "keypoints3d.npy"));
    assert_eq!(trans_shape[1], 3);
    assert_eq!(pose_shape, vec![trans_shape[0], 156]);
    assert_eq!(key_shape, vec![trans_shape[0], 52, 3]);
    let frames = trans_shape[0];
    let mut root_rotation_matrices = Vec::with_capacity(frames * 9);
    for pose in poses.chunks_exact(156) {
        root_rotation_matrices.extend_from_slice(&axis_angle_matrix(&pose[..3]));
    }
    Some(HyMotionDecoded {
        frames,
        latent_denorm: Vec::new(),
        rotations_6d: Vec::new(),
        translations,
        local_rotation_matrices: Vec::new(),
        root_rotation_matrices,
        keypoints_3d,
    })
}

fn usize_value(value: &JsonValue) -> usize {
    match value {
        JsonValue::U64(value) => *value as usize,
        JsonValue::I64(value) => *value as usize,
        JsonValue::F64(value) => *value as usize,
        other => panic!("not usize: {other:?}"),
    }
}

fn accessor_f32(parsed: &makepad_gltf::ParsedGlb, index: usize) -> Vec<f32> {
    let accessor = &parsed.document.accessors_slice()[index];
    assert_eq!(accessor.component_type, 5126);
    let lanes = match accessor.accessor_type.as_str() {
        "SCALAR" => 1,
        "VEC3" => 3,
        "VEC4" => 4,
        other => panic!("unsupported {other}"),
    };
    let view = &parsed.document.buffer_views_slice()[accessor.buffer_view.unwrap()];
    let start = view.byte_offset.unwrap_or(0) + accessor.byte_offset.unwrap_or(0);
    let stride = view.byte_stride.unwrap_or(lanes * 4);
    let bin = parsed.bin_chunk.as_ref().unwrap();
    let mut output = Vec::with_capacity(accessor.count * lanes);
    for item in 0..accessor.count {
        for lane in 0..lanes {
            let at = start + item * stride + lane * 4;
            output.push(f32::from_le_bytes(bin[at..at + 4].try_into().unwrap()));
        }
    }
    output
}

#[derive(Debug)]
struct ChannelData {
    times: Vec<f32>,
    values: Vec<f32>,
}

fn animation_channels(bytes: &[u8], clip_name: &str) -> HashMap<(usize, String), ChannelData> {
    let parsed = parse_glb_bytes(bytes).unwrap();
    let animation = parsed
        .document
        .animations
        .as_ref()
        .unwrap()
        .iter()
        .find(|animation| {
            matches!(animation.key("name"), Some(JsonValue::String(name)) if name == clip_name)
        })
        .unwrap();
    let samplers = match animation.key("samplers").unwrap() {
        JsonValue::Array(values) => values,
        _ => panic!(),
    };
    let channels = match animation.key("channels").unwrap() {
        JsonValue::Array(values) => values,
        _ => panic!(),
    };
    channels
        .iter()
        .map(|channel| {
            let sampler = usize_value(channel.key("sampler").unwrap());
            let target = channel.key("target").unwrap();
            let node = usize_value(target.key("node").unwrap());
            let path = target.key("path").unwrap().string().unwrap().clone();
            let input = usize_value(samplers[sampler].key("input").unwrap());
            let output = usize_value(samplers[sampler].key("output").unwrap());
            ((node, path), ChannelData {
                times: accessor_f32(&parsed, input),
                values: accessor_f32(&parsed, output),
            })
        })
        .collect()
}

fn sample_channel(channel: &ChannelData, lanes: usize, time: f32) -> Vec<f32> {
    let upper = channel.times.partition_point(|value| *value <= time);
    if upper == 0 {
        return channel.values[..lanes].to_vec();
    }
    if upper == channel.times.len() {
        return channel.values[channel.values.len() - lanes..].to_vec();
    }
    let left = upper - 1;
    if (channel.times[left] - time).abs() < 1.0e-6 {
        return channel.values[left * lanes..left * lanes + lanes].to_vec();
    }
    let mix = (time - channel.times[left]) / (channel.times[upper] - channel.times[left]);
    let mut value: Vec<f32> = (0..lanes)
        .map(|lane| {
            let a = channel.values[left * lanes + lane];
            let b = channel.values[upper * lanes + lane];
            a + (b - a) * mix
        })
        .collect();
    if lanes == 4 {
        let length = value.iter().map(|v| v * v).sum::<f32>().sqrt();
        for component in &mut value {
            *component /= length;
        }
    }
    value
}

fn quat_angle(left: &[f32], right: &[f32]) -> f64 {
    let nl = left.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
    let nr = right.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
    let dot = left
        .iter()
        .zip(right)
        .map(|(a, b)| *a as f64 * *b as f64)
        .sum::<f64>()
        / (nl * nr);
    2.0 * dot.abs().clamp(-1.0, 1.0).acos()
}

#[test]
fn corrected_blender_oracle_pose_parity_and_release_benchmark() {
    // This Blender fixture predates two deliberate native-retarget fixes:
    // terminal wrists now follow their real hand branch, and ankles transfer
    // a full source-rest-relative foot frame instead of baking the source and
    // target bind-pose pitch mismatch into the shoes. These are the only
    // rotation channels allowed to diverge from that historical reference;
    // every other joint remains a strict oracle.
    const CORRECTED_WRISTS_AND_ANKLES: [usize; 4] = [8, 18, 26, 30];
    const ROTATION_EPSILON: f64 = 2.0e-4;

    let Some(motion) = oracle_motion() else {
        eprintln!("HY retarget oracle fixtures absent; skipping");
        return;
    };
    let rig_path = fixture("native_mario_seed424242_oraclecontract_20k_rigged.glb");
    let reference_path = std::env::var_os("MAKEPAD_RETARGET_ORACLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixture("native_mario_seed424242_oraclecontract_20k_retargetfix_walk.glb"));
    if !rig_path.is_file() || !reference_path.is_file() {
        eprintln!("Mario retarget fixtures absent; skipping");
        return;
    }
    let rig = std::fs::read(rig_path).unwrap();
    let reference = std::fs::read(reference_path).unwrap();
    let started = Instant::now();
    let output = retarget_hy_motion_glb_with_report(
        &rig,
        &[HyMotionClipRef { name: "idle", motion: &motion }],
        &RetargetOptions::default(),
    )
    .unwrap();
    if let Some(path) = std::env::var_os("MAKEPAD_RETARGET_OUTPUT") {
        std::fs::write(path, &output.glb).unwrap();
    }
    let cold_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let mut timings = Vec::new();
    for _ in 0..9 {
        let started = Instant::now();
        let again = retarget_hy_motion_glb_with_report(
            &rig,
            &[HyMotionClipRef { name: "idle", motion: &motion }],
            &RetargetOptions::default(),
        )
        .unwrap();
        assert_eq!(again.glb, output.glb, "native export must be deterministic");
        timings.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    timings.sort_by(f64::total_cmp);

    let ours = animation_channels(&output.glb, "idle");
    let oracle = animation_channels(&reference, "idle");
    assert_eq!(ours.len(), 102);
    assert_eq!(oracle.len(), 102);
    let mut max_translation = 0.0f64;
    let mut max_scale = 0.0f64;
    let mut max_rotation = 0.0f64;
    let mut max_strict_rotation = 0.0f64;
    let mut max_time = 0.0f64;
    let mut worst_rotation = None;
    let mut worst_strict_rotation = None;
    let mut worst_translation = None;
    let mut worst_scale = None;
    let mut relaxed_rotation_nodes = HashSet::new();
    let mut rotation_by_node = HashMap::<usize, (f64, usize)>::new();
    for ((node, path), expected) in &oracle {
        let actual = &ours[&(*node, path.clone())];
        assert_eq!(actual.times.len(), motion.frames, "node {node} {path}");
        let lanes = if path == "rotation" { 4 } else { 3 };
        assert_eq!(actual.values.len(), motion.frames * lanes);
        max_time = max_time
            .max((actual.times[0] as f64 - expected.times[0] as f64).abs())
            .max(
                (actual.times[motion.frames - 1] as f64
                    - expected.times[expected.times.len() - 1] as f64)
                    .abs(),
            );
        if expected.times.len() == motion.frames {
            for (actual, expected) in actual.times.iter().zip(&expected.times) {
                max_time = max_time.max((*actual as f64 - *expected as f64).abs());
            }
        }
        match path.as_str() {
            "rotation" => {
                for frame in 0..motion.frames {
                    let expected = oracle_frame(expected, 4, frame, motion.frames, actual.times[frame]);
                    let actual_values = &actual.values[frame * 4..frame * 4 + 4];
                    let angle = quat_angle(actual_values, &expected);
                    if angle > max_rotation {
                        max_rotation = angle;
                        worst_rotation = Some((*node, frame));
                    }
                    if CORRECTED_WRISTS_AND_ANKLES.contains(node) {
                        if angle >= ROTATION_EPSILON {
                            relaxed_rotation_nodes.insert(*node);
                        }
                    } else if angle > max_strict_rotation {
                        max_strict_rotation = angle;
                        worst_strict_rotation = Some((*node, frame));
                    }
                    let node_worst = rotation_by_node.entry(*node).or_insert((0.0, frame));
                    if angle > node_worst.0 {
                        *node_worst = (angle, frame);
                    }
                }
            }
            "translation" => {
                for frame in 0..motion.frames {
                    let expected = oracle_frame(expected, 3, frame, motion.frames, actual.times[frame]);
                    for (lane, (actual, expected)) in actual.values[frame * 3..frame * 3 + 3]
                        .iter()
                        .zip(expected)
                        .enumerate()
                    {
                        let difference = (*actual as f64 - expected as f64).abs();
                        if difference > max_translation {
                            max_translation = difference;
                            worst_translation = Some((*node, frame, lane, *actual, expected));
                        }
                    }
                }
            }
            "scale" => {
                for frame in 0..motion.frames {
                    let expected = oracle_frame(expected, 3, frame, motion.frames, actual.times[frame]);
                    for (lane, (actual, expected)) in actual.values[frame * 3..frame * 3 + 3]
                        .iter()
                        .zip(expected)
                        .enumerate()
                    {
                        let difference = (*actual as f64 - expected as f64).abs();
                        if difference > max_scale {
                            max_scale = difference;
                            worst_scale = Some((*node, frame, lane, *actual, expected));
                        }
                    }
                }
            }
            _ => unreachable!(),
        }
    }
    let parsed_reference = parse_glb_bytes(&reference).unwrap();
    let reference_nodes = parsed_reference.document.nodes_slice();
    let mut rotation_ranked: Vec<_> = rotation_by_node.into_iter().collect();
    rotation_ranked.sort_by(|left, right| right.1.0.total_cmp(&left.1.0));
    let rotation_ranked: Vec<_> = rotation_ranked
        .into_iter()
        .take(8)
        .map(|(node, (angle, frame))| {
            (node, reference_nodes[node].name.as_deref().unwrap_or("?"), frame, angle)
        })
        .collect();
    eprintln!(
        "native retarget: cold={cold_ms:.3}ms median={:.3}ms bytes={} report={{joints:{} mapped:{} mirrored:{} height:{:.9} scale:{:.9}}}; parity max_time={max_time:.9} max_translation={max_translation:.9} worst_translation={worst_translation:?} max_scale={max_scale:.9} worst_scale={worst_scale:?} max_rotation_rad={max_rotation:.9} worst={worst_rotation:?} max_strict_rotation_rad={max_strict_rotation:.9} strict_worst={worst_strict_rotation:?}; rotation_ranked={rotation_ranked:?}",
        timings[timings.len() / 2],
        output.glb.len(),
        output.report.joints,
        output.report.mapped_joints,
        output.report.mirrored,
        output.report.rig_height,
        output.report.motion_scale,
    );
    assert!(max_time < 1.0e-6, "key-time diff {max_time}");
    assert!(max_translation < 2.0e-5, "translation diff {max_translation}");
    assert!(max_scale < 2.0e-5, "scale diff {max_scale}");
    assert_eq!(
        relaxed_rotation_nodes,
        HashSet::from(CORRECTED_WRISTS_AND_ANKLES),
        "only the corrected wrists and ankles may differ from the historical Blender retarget"
    );
    assert!(
        max_strict_rotation < ROTATION_EPSILON,
        "strict rotation diff {max_strict_rotation} at {worst_strict_rotation:?}"
    );
}

#[test]
fn idle_walk_jump_clip_contract_and_in_place_root() {
    let Some(motion) = oracle_motion() else {
        eprintln!("HY retarget oracle fixtures absent; skipping");
        return;
    };
    let rig_path = fixture("native_mario_seed424242_oraclecontract_20k_rigged.glb");
    if !rig_path.is_file() {
        eprintln!("Mario rig fixture absent; skipping");
        return;
    }
    let rig = std::fs::read(rig_path).unwrap();
    let clips = [
        HyMotionClipRef { name: "idle", motion: &motion },
        HyMotionClipRef { name: "walk", motion: &motion },
        HyMotionClipRef { name: "jump", motion: &motion },
    ];
    let output = retarget_hy_motion_glb_with_report(&rig, &clips, &RetargetOptions::default())
        .unwrap();
    assert_eq!(output.report.clips, 3);
    assert_eq!(output.report.frames, motion.frames * 3);

    let parsed = parse_glb_bytes(&output.glb).unwrap();
    let animations = parsed.document.animations.as_ref().unwrap();
    let names: Vec<_> = animations
        .iter()
        .map(|animation| animation.key("name").unwrap().string().unwrap().as_str())
        .collect();
    assert_eq!(names, ["idle", "walk", "jump"]);

    // The fixed oracle rig's skeleton root is node 33. Horizontal movement
    // belongs to the game controller in in-place mode; vertical crouch/jump
    // displacement must remain in the clip.
    for name in names {
        let channels = animation_channels(&output.glb, name);
        assert_eq!(channels.len(), 102);
        let root = &channels[&(33, "translation".to_string())];
        let first = &root.values[..3];
        let mut min_y = first[1];
        let mut max_y = first[1];
        for value in root.values.chunks_exact(3) {
            assert!((value[0] - first[0]).abs() < 1.0e-7);
            assert!((value[2] - first[2]).abs() < 1.0e-7);
            min_y = min_y.min(value[1]);
            max_y = max_y.max(value[1]);
        }
        assert!(max_y - min_y > 0.005, "vertical root motion was stripped");
    }
}

#[test]
fn fresh_ui_rig_has_real_wrist_directions() {
    let Some(motion) = oracle_motion() else {
        eprintln!("HY retarget oracle fixtures absent; skipping");
        return;
    };
    let rig_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/ai_content_library/lib-13.glb");
    if !rig_path.is_file() {
        eprintln!("fresh UI rig fixture absent; skipping");
        return;
    }
    let rig = std::fs::read(rig_path).unwrap();
    let output = retarget_hy_motion_glb_with_report(
        &rig,
        &[HyMotionClipRef {
            name: "idle",
            motion: &motion,
        }],
        &RetargetOptions::default(),
    )
    .unwrap();
    assert_eq!(output.report.joints, 34);
    // 19 driven segments: the two terminal foot endpoints are direction
    // markers inherited from their ankles, not duplicate driven segments.
    assert_eq!(output.report.mapped_joints, 19);

    // This fresh SkinTokens skeleton branches at each wrist. The old
    // classifier stopped there and used an imported synthetic +Y tail,
    // producing 143/149-degree first-frame wrist turns. Following the real
    // distal hand branches keeps the same motion below a conservative 45°.
    let channels = animation_channels(&output.glb, "idle");
    let parsed = parse_glb_bytes(&output.glb).unwrap();
    let nodes = parsed.document.nodes_slice();
    let wrist_nodes: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            matches!(node.name.as_deref(), Some("bone_9" | "bone_19")).then_some(index)
        })
        .collect();
    assert_eq!(wrist_nodes.len(), 2);
    let rest = [0.0, 0.0, 0.0, 1.0];
    for wrist in wrist_nodes {
        let first = &channels[&(wrist, "rotation".to_string())].values[..4];
        let angle_degrees = quat_angle(first, &rest).to_degrees();
        assert!(
            angle_degrees < 45.0,
            "fresh rig wrist node {wrist} turns {angle_degrees:.2}° at frame zero"
        );
    }

    // The final nodes in both leg chains are foot-direction endpoints, not
    // extra anatomical segments. They must retain their rest-local rotation
    // while inheriting the driven ankle; mapping ankle -> foot onto them a
    // second time caused 60-70 degree one-frame snaps in generated walks.
    let foot_endpoints: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            matches!(node.name.as_deref(), Some("bone_29" | "bone_33")).then_some(index)
        })
        .collect();
    assert_eq!(foot_endpoints.len(), 2);
    for endpoint in foot_endpoints {
        let rotation = &channels[&(endpoint, "rotation".to_string())].values;
        let first = &rotation[..4];
        let max_delta = rotation
            .chunks_exact(4)
            .map(|frame| quat_angle(first, frame))
            .fold(0.0f64, f64::max);
        assert!(
            max_delta < 1.0e-6,
            "foot endpoint node {endpoint} was redundantly animated ({max_delta} radians)"
        );
    }
}

#[test]
fn fresh_yoshi_rig_with_raised_hip_heads_retargets_if_present() {
    let Some(motion) = oracle_motion() else {
        eprintln!("HY retarget oracle fixtures absent; skipping");
        return;
    };
    let rig_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/ai_content_library/lib-19.glb");
    if !rig_path.is_file() {
        eprintln!("fresh Yoshi rig fixture absent; skipping");
        return;
    }
    let rig = std::fs::read(&rig_path).unwrap();
    let output = retarget_hy_motion_glb_with_report(
        &rig,
        &[HyMotionClipRef {
            name: "idle",
            motion: &motion,
        }],
        &RetargetOptions::default(),
    )
    .unwrap();
    assert_eq!(output.report.joints, 22);
    assert_eq!(output.report.mapped_joints, 17);
    assert_eq!(output.report.clips, 1);
    assert_eq!(output.report.frames, motion.frames);

    let parsed = parse_glb_bytes(&output.glb).unwrap();
    let animations = parsed.document.animations.as_ref().unwrap();
    assert_eq!(animations.len(), 1);
    assert!(matches!(
        animations[0].key("name"),
        Some(JsonValue::String(name)) if name == "idle"
    ));
}

#[test]
fn fresh_elf_rig_with_terminal_hand_leaves_retargets_if_present() {
    let Some(motion) = oracle_motion() else {
        eprintln!("HY retarget oracle fixtures absent; skipping");
        return;
    };
    let rig_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/ai_content_library/lib-34.glb");
    if !rig_path.is_file() {
        eprintln!("fresh elf rig fixture absent; skipping");
        return;
    }
    let rig = std::fs::read(&rig_path).unwrap();
    let output = retarget_hy_motion_glb_with_report(
        &rig,
        &[HyMotionClipRef {
            name: "idle",
            motion: &motion,
        }],
        &RetargetOptions::default(),
    )
    .unwrap();
    assert_eq!(output.report.joints, 27);
    assert_eq!(output.report.mapped_joints, 18);

    let parsed = parse_glb_bytes(&output.glb).unwrap();
    let nodes = parsed.document.nodes_slice();
    let channels = animation_channels(&output.glb, "idle");
    for hand in [11usize, 15usize] {
        assert!(nodes[hand].children.as_deref().unwrap_or(&[]).is_empty());
        let rotation = &channels[&(hand, "rotation".to_string())].values;
        let first = &rotation[..4];
        let max_delta = rotation
            .chunks_exact(4)
            .map(|frame| quat_angle(first, frame))
            .fold(0.0f64, f64::max);
        assert!(
            max_delta < 1.0e-6,
            "terminal hand node {hand} was independently animated ({max_delta} radians)"
        );
    }
}

#[test]
fn clean_elf_rig_with_low_hands_and_split_ankles_retargets_if_present() {
    let Some(motion) = oracle_motion() else {
        eprintln!("HY retarget oracle fixtures absent; skipping");
        return;
    };
    let rig_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/ai_content_library/lib-49.glb");
    if !rig_path.is_file() {
        eprintln!("clean elf rig fixture absent; skipping");
        return;
    }
    let rig = std::fs::read(&rig_path).unwrap();
    let output = retarget_hy_motion_glb_with_report(
        &rig,
        &[HyMotionClipRef {
            name: "idle",
            motion: &motion,
        }],
        &RetargetOptions::default(),
    )
    .unwrap();
    assert_eq!(output.report.joints, 25);
    assert_eq!(output.report.mapped_joints, 18);

    let parsed = parse_glb_bytes(&output.glb).unwrap();
    let animations = parsed.document.animations.as_ref().unwrap();
    assert_eq!(animations.len(), 1);
    assert!(matches!(
        animations[0].key("name"),
        Some(JsonValue::String(name)) if name == "idle"
    ));

    let channels = animation_channels(&output.glb, "idle");
    assert_eq!(channels.len(), 25 * 3);
    for endpoint in [17usize, 19, 23, 25] {
        let rotation = &channels[&(endpoint, "rotation".to_string())].values;
        let first = &rotation[..4];
        let max_delta = rotation
            .chunks_exact(4)
            .map(|frame| quat_angle(first, frame))
            .fold(0.0f64, f64::max);
        assert!(
            max_delta < 1.0e-6,
            "split-foot endpoint node {endpoint} was independently animated ({max_delta} radians)"
        );
    }
}

#[test]
fn export_fresh_ui_foot_frame_candidate_if_requested() {
    let Some(path) = std::env::var_os("MAKEPAD_RETARGET_FRESH_OUTPUT") else {
        eprintln!("MAKEPAD_RETARGET_FRESH_OUTPUT unset; skipping candidate export");
        return;
    };
    let Some(motion) = oracle_motion() else {
        eprintln!("HY retarget oracle fixtures absent; skipping");
        return;
    };
    let rig_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/ai_content_library/lib-13.glb");
    let rig = std::fs::read(&rig_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", rig_path.display()));
    let clips = [
        HyMotionClipRef {
            name: "idle",
            motion: &motion,
        },
        HyMotionClipRef {
            name: "walk",
            motion: &motion,
        },
        HyMotionClipRef {
            name: "jump",
            motion: &motion,
        },
    ];
    let output = retarget_hy_motion_glb_with_report(&rig, &clips, &RetargetOptions::default())
        .unwrap();
    std::fs::write(&path, &output.glb)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", Path::new(&path).display()));
}

fn oracle_frame(
    channel: &ChannelData,
    lanes: usize,
    frame: usize,
    total_frames: usize,
    time: f32,
) -> Vec<f32> {
    if channel.times.len() == total_frames {
        channel.values[frame * lanes..frame * lanes + lanes].to_vec()
    } else {
        sample_channel(channel, lanes, time)
    }
}
