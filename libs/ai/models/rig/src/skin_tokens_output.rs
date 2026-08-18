//! Blender-free SkinTokens geometry handoff.
//!
//! This module joins the two exact host boundaries around the native neural
//! runtime: sampled-point weights are transferred back to the source mesh by
//! [`SkinTokensMesh`], then the original GLB is losslessly augmented with a
//! translation-only skeleton and top-four glTF skin attributes. Materials,
//! textures, primitive boundaries and opaque extension JSON remain intact.

use crate::skin_tokens_mesh::{SkinTokensMesh, SkinTokensNormalization, SkinTokensSamples};
use crate::skin_tokens_tokenizer::SkinTokensSkeleton;
use crate::{band_progress, emit_progress, hook_ref, DiffusionError, ProgressHook, Result};
use makepad_gltf::{augment_glb_skin, GlbPrimitiveSkin, GlbSkinJoint};

/// Convert a normalized Blender-frame SkinTokens head into the source GLB's
/// scene/world Y-up frame. Blender import maps glTF `(x,y,z)` to
/// `(x,-z,y)`, so the inverse mapping after denormalization is `(x,z,-y)`.
pub fn skin_tokens_joint_to_gltf(
    mesh: &SkinTokensMesh,
    normalized_blender: [f32; 3],
) -> [f32; 3] {
    normalized_blender_to_gltf(&mesh.normalization, normalized_blender)
}

fn normalized_blender_to_gltf(
    normalization: &SkinTokensNormalization,
    normalized_blender: [f32; 3],
) -> [f32; 3] {
    let blender = normalization.denormalize(normalized_blender);
    [blender[0], blender[2], -blender[1]]
}

/// Produce a rigged GLB from raw sample-major SkinVAE sigmoid columns.
///
/// `sample_weights` has shape `[samples.positions.len(), skeleton.joints.len()]`.
/// It must remain raw model output: the official pipeline interpolates each
/// independent joint column first and only normalizes the strongest four
/// influences during export. This function performs both operations in that
/// order and never invokes Python, Torch, SciPy, Blender, or an external GLB
/// converter.
pub fn skin_tokens_rig_glb(
    input_glb: &[u8],
    mesh: &SkinTokensMesh,
    samples: &SkinTokensSamples,
    skeleton: &SkinTokensSkeleton,
    sample_weights: &[f32],
) -> Result<Vec<u8>> {
    skin_tokens_rig_glb_with_progress(
        input_glb,
        mesh,
        samples,
        skeleton,
        sample_weights,
        None,
    )
}

/// Cancellable/progress-reporting form of [`skin_tokens_rig_glb`].
pub fn skin_tokens_rig_glb_with_progress(
    input_glb: &[u8],
    mesh: &SkinTokensMesh,
    samples: &SkinTokensSamples,
    skeleton: &SkinTokensSkeleton,
    sample_weights: &[f32],
    mut progress: Option<ProgressHook<'_>>,
) -> Result<Vec<u8>> {
    validate_skeleton(skeleton)?;
    let joint_count = skeleton.joints.len();
    let mut transfer_progress = band_progress(&mut progress, 0.0, 0.9);
    let vertex_weights = mesh.transfer_sample_weights_with_progress(
        samples,
        sample_weights,
        joint_count,
        hook_ref(&mut transfer_progress),
    )?;
    drop(transfer_progress);
    emit_progress(&mut progress, "export-rigged-glb", 0.9)?;

    let joints = skeleton
        .joints
        .iter()
        .enumerate()
        .map(|(joint, &head)| GlbSkinJoint {
            // Upstream's unnamed Asset skeleton uses generated generic bone
            // names. Native retargeting intentionally classifies hierarchy
            // and geometry, so no semantic-name inference is hidden here.
            name: format!("bone_{joint}"),
            parent: skeleton.parents[joint],
            global_translation: skin_tokens_joint_to_gltf(mesh, head),
        })
        .collect::<Vec<_>>();

    let mut primitive_skins = Vec::with_capacity(mesh.parts.len());
    for part in &mesh.parts {
        let node = part.node_index.ok_or_else(|| {
            DiffusionError::workflow(format!(
                "SkinTokens source mesh {}/{} has no scene node; lossless skin attachment is ambiguous",
                part.mesh_index, part.primitive_index,
            ))
        })?;
        let start = part.vertex_start * joint_count;
        let end = (part.vertex_start + part.vertex_count) * joint_count;
        primitive_skins.push(GlbPrimitiveSkin {
            node,
            primitive: part.primitive_index,
            weights: vertex_weights[start..end].to_vec(),
        });
    }
    let output = augment_glb_skin(input_glb, &joints, &primitive_skins)
        .map_err(|error| DiffusionError::workflow(format!("SkinTokens GLB export failed: {error}")))?;
    emit_progress(&mut progress, "export-rigged-glb", 1.0)?;
    Ok(output)
}

fn validate_skeleton(skeleton: &SkinTokensSkeleton) -> Result<()> {
    if skeleton.joints.is_empty() || skeleton.parents.len() != skeleton.joints.len() {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens skeleton has {} joints and {} parent entries",
            skeleton.joints.len(),
            skeleton.parents.len(),
        )));
    }
    let mut roots = 0usize;
    for (joint, (&head, parent)) in skeleton
        .joints
        .iter()
        .zip(&skeleton.parents)
        .enumerate()
    {
        if head.iter().any(|value| !value.is_finite()) {
            return Err(DiffusionError::workflow(format!(
                "SkinTokens joint {joint} has a non-finite head",
            )));
        }
        match parent {
            Some(parent) if *parent >= joint => {
                return Err(DiffusionError::workflow(format!(
                    "SkinTokens joint {joint} has non-prior parent {parent}",
                )))
            }
            Some(_) => {}
            None => roots += 1,
        }
    }
    if roots != 1 {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens skeleton has {roots} roots, expected one",
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joint_coordinate_mapping_inverts_blender_import() {
        let normalization = SkinTokensNormalization {
            center: [0.0; 3],
            scale: 2.0,
            translation: [0.5, -0.25, 1.0],
        };
        let normalized = normalization.normalize([2.0, -3.0, 4.0]);
        // The denormalized Blender point (2,-3,4) corresponds to source glTF
        // world point (2,4,3).
        let gltf = normalized_blender_to_gltf(&normalization, normalized);
        for (actual, expected) in gltf.into_iter().zip([2.0, 4.0, 3.0]) {
            assert!((actual - expected).abs() < 1.0e-6);
        }
    }

    #[test]
    fn rejects_non_prior_parent_before_expensive_transfer() {
        let skeleton = SkinTokensSkeleton {
            joints: vec![[0.0; 3], [0.0, 1.0, 0.0]],
            parents: vec![None, Some(1)],
            class_token: None,
            parts: vec![],
        };
        assert!(validate_skeleton(&skeleton).is_err());
    }
}
