//! Animation-only GLB augmentation.
//!
//! Unlike [`crate::write_glb_mesh_skinned`], this path deliberately does not
//! rebuild the mesh. Auto-riggers commonly emit multiple material/texture
//! records and implementation-specific node metadata; rebuilding those just
//! to add keyframes is both lossy and expensive. Instead we retain the input
//! BIN payload, append dense f32 animation accessors, and replace only the
//! top-level `animations` array plus the corresponding accessor metadata.

use crate::augment::{number, object, string, validation, GlbRewrite};
use crate::{GlbAnimPath, GltfError};
use makepad_micro_serde::JsonValue;
use std::collections::HashSet;

/// One animation channel targeting an existing glTF node (not a skin-joint
/// ordinal). Values use glTF's native conventions: translations/scales are
/// XYZ and rotations are quaternions in XYZW order.
pub struct GlbNodeAnimChannel {
    pub node: usize,
    pub path: GlbAnimPath,
    pub times: Vec<f32>,
    pub values: Vec<f32>,
}

/// A named animation clip to attach to an existing GLB.
pub struct GlbNodeAnimClip {
    pub name: String,
    pub channels: Vec<GlbNodeAnimChannel>,
}

fn f32_bytes(values: &[f32]) -> Result<Vec<u8>, GltfError> {
    if values.iter().any(|value| !value.is_finite()) {
        return Err(validation("animation contains a non-finite value"));
    }
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    Ok(bytes)
}

fn append_accessor(
    rewrite: &mut GlbRewrite,
    views: &mut Vec<JsonValue>,
    accessors: &mut Vec<JsonValue>,
    values: &[f32],
    lanes: usize,
    accessor_type: &'static str,
    min_max: bool,
) -> Result<usize, GltfError> {
    let bytes = f32_bytes(values)?;
    let count = values.len() / lanes;
    let (min, max) = if min_max {
        let (min, max) = if values.is_empty() {
            (0.0, 0.0)
        } else {
            values.iter().fold(
                (f32::INFINITY, f32::NEG_INFINITY),
                |(min, max), value| (min.min(*value), max.max(*value)),
            )
        };
        (
            Some(JsonValue::Array(vec![JsonValue::F64(min as f64)])),
            Some(JsonValue::Array(vec![JsonValue::F64(max as f64)])),
        )
    } else {
        (None, None)
    };
    Ok(rewrite.append_accessor(
        views,
        accessors,
        &bytes,
        5126,
        count,
        accessor_type,
        false,
        None,
        min,
        max,
    ))
}

/// Replace the animations in `input` while preserving its original mesh,
/// skin, textures, images, and opaque extension data.
///
/// The input must be a self-contained GLB with buffer 0 in its BIN chunk.
/// Existing animations are intentionally replaced: the character pipeline's
/// output contract is the requested named clip set, not a merge with stale
/// actions left by an auto-rigger.
pub fn replace_glb_node_animations(
    input: &[u8],
    clips: &[GlbNodeAnimClip],
) -> Result<Vec<u8>, GltfError> {
    let mut rewrite = GlbRewrite::begin(input)?;
    let node_count = rewrite.document.nodes_slice().len();
    let mut clip_names = HashSet::with_capacity(clips.len());
    for clip in clips {
        if clip.name.is_empty() {
            return Err(validation("animation clip name must not be empty"));
        }
        if !clip_names.insert(clip.name.as_str()) {
            return Err(validation(format!(
                "duplicate animation clip name '{}'",
                clip.name
            )));
        }
        if clip.channels.is_empty() {
            return Err(validation(format!(
                "animation '{}' has no channels",
                clip.name
            )));
        }
        let mut targets = HashSet::with_capacity(clip.channels.len());
        for channel in &clip.channels {
            if channel.node >= node_count {
                return Err(validation(format!(
                    "animation target node {} is out of range ({node_count})",
                    channel.node
                )));
            }
            if rewrite.document.nodes_slice()[channel.node].matrix.is_some() {
                return Err(validation(format!(
                    "animation '{}' targets matrix node {}; convert it to TRS first",
                    clip.name, channel.node
                )));
            }
            let path_id = match channel.path {
                GlbAnimPath::Translation => 0u8,
                GlbAnimPath::Rotation => 1u8,
                GlbAnimPath::Scale => 2u8,
            };
            if !targets.insert((channel.node, path_id)) {
                return Err(validation(format!(
                    "animation '{}' repeats node {} path",
                    clip.name, channel.node
                )));
            }
            let lanes = if channel.path == GlbAnimPath::Rotation { 4 } else { 3 };
            if channel.times.is_empty() || channel.values.len() != channel.times.len() * lanes {
                return Err(validation(format!(
                    "animation '{}' channel has inconsistent key/value lengths",
                    clip.name
                )));
            }
            if channel
                .times
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            {
                return Err(validation(format!(
                    "animation '{}' key times are not strictly ascending",
                    clip.name
                )));
            }
        }
    }

    let mut views = rewrite.take_array("bufferViews")?;
    let mut accessors = rewrite.take_array("accessors")?;

    let mut animations = Vec::with_capacity(clips.len());
    for clip in clips {
        let mut time_accessors: Vec<(&[f32], usize)> = Vec::new();
        let mut samplers = Vec::with_capacity(clip.channels.len());
        let mut channels = Vec::with_capacity(clip.channels.len());
        for (sampler_index, channel) in clip.channels.iter().enumerate() {
            let input_accessor = match time_accessors
                .iter()
                .find(|(times, _)| *times == channel.times.as_slice())
            {
                Some((_, accessor)) => *accessor,
                None => {
                    let accessor = append_accessor(
                        &mut rewrite,
                        &mut views,
                        &mut accessors,
                        &channel.times,
                        1,
                        "SCALAR",
                        true,
                    )?;
                    time_accessors.push((&channel.times, accessor));
                    accessor
                }
            };
            let (lanes, accessor_type, path) = match channel.path {
                GlbAnimPath::Translation => (3, "VEC3", "translation"),
                GlbAnimPath::Rotation => (4, "VEC4", "rotation"),
                GlbAnimPath::Scale => (3, "VEC3", "scale"),
            };
            let output_accessor = append_accessor(
                &mut rewrite,
                &mut views,
                &mut accessors,
                &channel.values,
                lanes,
                accessor_type,
                false,
            )?;
            samplers.push(object([
                ("input", number(input_accessor)),
                ("output", number(output_accessor)),
                ("interpolation", string("LINEAR")),
            ]));
            channels.push(object([
                ("sampler", number(sampler_index)),
                (
                    "target",
                    object([("node", number(channel.node)), ("path", string(path))]),
                ),
            ]));
        }
        animations.push(object([
            ("name", string(clip.name.clone())),
            ("samplers", JsonValue::Array(samplers)),
            ("channels", JsonValue::Array(channels)),
        ]));
    }
    rewrite.put_array("bufferViews", views);
    rewrite.put_array("accessors", accessors);
    if animations.is_empty() {
        rewrite.remove("animations");
    } else {
        rewrite.insert("animations", JsonValue::Array(animations));
    }
    rewrite.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_glb_bytes, write_glb_mesh_skinned, GlbJoint, GlbSkinnedMesh};

    fn rig() -> Vec<u8> {
        let positions = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        write_glb_mesh_skinned(&GlbSkinnedMesh {
            positions: &positions,
            normals: None,
            uvs: None,
            indices: &[0, 1, 2],
            joints_0: &[[0, 0, 0, 0]; 3],
            weights_0: &[[1.0, 0.0, 0.0, 0.0]; 3],
            joints: &[GlbJoint::at("root", None, [0.0; 3], [0.0; 3])],
            clips: &[],
            base_color_png: None,
        })
    }

    #[test]
    fn augments_without_rebuilding_original_bin_prefix() {
        let input = rig();
        let parsed_input = parse_glb_bytes(&input).unwrap();
        let output = replace_glb_node_animations(
            &input,
            &[GlbNodeAnimClip {
                name: "walk".into(),
                channels: vec![GlbNodeAnimChannel {
                    node: 1,
                    path: GlbAnimPath::Rotation,
                    times: vec![0.0, 1.0],
                    values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.70710677, 0.70710677],
                }],
            }],
        )
        .unwrap();
        let parsed_output = parse_glb_bytes(&output).unwrap();
        let old_bin = parsed_input.bin_chunk.unwrap();
        let new_bin = parsed_output.bin_chunk.unwrap();
        assert_eq!(&new_bin[..old_bin.len()], old_bin.as_slice());
        assert_eq!(parsed_output.document.animations.as_ref().unwrap().len(), 1);
        assert_eq!(parsed_output.document.nodes_slice().len(), 2);
    }

    #[test]
    fn output_is_bit_deterministic() {
        let input = rig();
        let make = || {
            replace_glb_node_animations(
                &input,
                &[GlbNodeAnimClip {
                    name: "idle".into(),
                    channels: vec![GlbNodeAnimChannel {
                        node: 1,
                        path: GlbAnimPath::Translation,
                        times: vec![0.0, 0.5],
                        values: vec![0.0; 6],
                    }],
                }],
            )
            .unwrap()
        };
        assert_eq!(make(), make());
    }

    #[test]
    fn empty_clip_set_removes_animation_property() {
        let input = rig();
        let animated = replace_glb_node_animations(
            &input,
            &[GlbNodeAnimClip {
                name: "idle".into(),
                channels: vec![GlbNodeAnimChannel {
                    node: 1,
                    path: GlbAnimPath::Translation,
                    times: vec![0.0, 0.5],
                    values: vec![0.0; 6],
                }],
            }],
        )
        .unwrap();
        let cleared = replace_glb_node_animations(&animated, &[]).unwrap();
        assert!(parse_glb_bytes(&cleared).unwrap().document.animations.is_none());
    }

    #[test]
    fn rejects_duplicate_targets() {
        let input = rig();
        let channel = || GlbNodeAnimChannel {
            node: 1,
            path: GlbAnimPath::Scale,
            times: vec![0.0],
            values: vec![1.0; 3],
        };
        let error = replace_glb_node_animations(
            &input,
            &[GlbNodeAnimClip {
                name: "bad".into(),
                channels: vec![channel(), channel()],
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("repeats node 1 path"));
    }
}
