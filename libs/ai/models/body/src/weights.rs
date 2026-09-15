//! The Comfy-Org `sam_3d_body_dinov3_bf16.safetensors` reader.
//!
//! One file holds everything the port needs: the backbone (bf16), the two
//! decoders and heads (f32), and the MHR rig data under `mhr.*` (skeleton,
//! parameter transform, blendshape bases, pose-corrective MLP, skinning).
//! Reads are by name with the shape checked at the call site, so a wrong
//! repack fails at load, not mid-inference.

use makepad_ai_common::dtype::f16_word_to_f32;
use makepad_ai_common::{DiffusionError, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::path::{Path, PathBuf};

/// Production identity (`Comfy-Org/sam-3d-body`, revision
/// 60476aced0b8de0a0e82a318c79a85061cc97434).
pub const WEIGHTS_REPO: &str = "Comfy-Org/sam-3d-body";
pub const WEIGHTS_PATH: &str = "detection/sam_3d_body_dinov3_bf16.safetensors";
pub const WEIGHTS_SIZE: u64 = 2_830_737_652;

pub struct BodyWeights {
    pub path: PathBuf,
    header: MlxSafetensorsHeader,
}

impl BodyWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let header = MlxSafetensorsHeader::load(&path).map_err(|err| {
            DiffusionError::model(format!("body weights {}: {err:?}", path.display()))
        })?;
        let weights = Self { path, header };
        weights.validate_architecture()?;
        Ok(weights)
    }

    pub fn file_len(&self) -> u64 {
        self.header.file_len
    }

    pub fn has(&self, name: &str) -> bool {
        self.header.tensors.contains_key(name)
    }

    pub fn tensor_names(&self) -> impl Iterator<Item = &String> {
        self.header.tensors.keys()
    }

    pub fn shape(&self, name: &str) -> Result<Vec<usize>> {
        let entry = self.entry(name)?;
        entry
            .shape
            .iter()
            .map(|&value| {
                usize::try_from(value).map_err(|_| {
                    DiffusionError::model(format!("body tensor {name} dimension {value} exceeds usize"))
                })
            })
            .collect()
    }

    pub fn dtype(&self, name: &str) -> Result<MlxDType> {
        Ok(self.entry(name)?.dtype)
    }

    pub fn bytes(&self, name: &str) -> Result<Vec<u8>> {
        self.header
            .read_tensor_bytes(name)
            .map_err(|err| DiffusionError::model(format!("body read tensor {name}: {err:?}")))
    }

    /// Any floating tensor as f32 (bf16/f16 widened exactly).
    pub fn f32(&self, name: &str) -> Result<Vec<f32>> {
        let dtype = self.dtype(name)?;
        let bytes = self.bytes(name)?;
        let values = match dtype {
            MlxDType::F16 => bytes
                .chunks_exact(2)
                .map(|c| f16_word_to_f32(u16::from_le_bytes([c[0], c[1]])))
                .collect(),
            MlxDType::BF16 => bytes
                .chunks_exact(2)
                .map(|c| f32::from_bits(u32::from(u16::from_le_bytes([c[0], c[1]])) << 16))
                .collect(),
            MlxDType::F32 => bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
            other => {
                return Err(DiffusionError::model(format!(
                    "body tensor {name} has unsupported floating dtype {other:?}"
                )))
            }
        };
        Ok(values)
    }

    pub fn f32_shaped(&self, name: &str, expected: &[usize]) -> Result<Vec<f32>> {
        self.expect_shape(name, expected)?;
        self.f32(name)
    }

    /// bf16 words verbatim (the backbone linears go to the GPU as bf16).
    pub fn bf16_words(&self, name: &str) -> Result<Vec<u16>> {
        match self.dtype(name)? {
            MlxDType::BF16 => Ok(self
                .bytes(name)?
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect()),
            other => Err(DiffusionError::model(format!(
                "body tensor {name} is {other:?}, expected bf16"
            ))),
        }
    }

    /// Any integer tensor widened to i64 (`mhr.*` indices, faces).
    pub fn i64(&self, name: &str) -> Result<Vec<i64>> {
        let dtype = self.dtype(name)?;
        let bytes = self.bytes(name)?;
        let values = match dtype {
            MlxDType::I32 => bytes
                .chunks_exact(4)
                .map(|c| i64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])))
                .collect(),
            MlxDType::I64 => bytes
                .chunks_exact(8)
                .map(|c| i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
                .collect(),
            MlxDType::Bool | MlxDType::U8 => bytes.iter().map(|&b| i64::from(b)).collect(),
            other => {
                return Err(DiffusionError::model(format!(
                    "body tensor {name} has unsupported integer dtype {other:?}"
                )))
            }
        };
        Ok(values)
    }

    pub fn i64_shaped(&self, name: &str, expected: &[usize]) -> Result<Vec<i64>> {
        self.expect_shape(name, expected)?;
        self.i64(name)
    }

    pub fn expect_shape(&self, name: &str, expected: &[usize]) -> Result<()> {
        let shape = self.shape(name)?;
        if shape != expected {
            return Err(DiffusionError::model(format!(
                "body tensor {name} shape {shape:?}, expected {expected:?}"
            )));
        }
        Ok(())
    }

    fn entry(&self, name: &str) -> Result<&makepad_ai_loader::MlxTensorEntry> {
        self.header.tensors.get(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "body tensor {name} missing from {}",
                self.path.display()
            ))
        })
    }

    fn expect(&self, name: &str, dtype: MlxDType, shape: &[usize]) -> Result<()> {
        let actual_dtype = self.dtype(name)?;
        let actual_shape = self.shape(name)?;
        if actual_dtype != dtype || actual_shape != shape {
            return Err(DiffusionError::model(format!(
                "body tensor {name} is {actual_dtype:?} {actual_shape:?}, expected {dtype:?} {shape:?}"
            )));
        }
        Ok(())
    }

    /// Fail closed on anything but the Comfy-Org repack: the HF-transformers
    /// backbone naming (`backbone.layer.N.attention.q_proj`), the bundled
    /// `mhr.*` rig, the body decoder and heads at the shapes the port is
    /// written for. A Meta `.ckpt`-style header (`backbone.encoder.blocks`)
    /// is refused by name.
    fn validate_architecture(&self) -> Result<()> {
        use crate::*;
        if self.has("backbone.encoder.blocks.0.attn.qkv.weight") {
            return Err(DiffusionError::model(
                "body weights refused a Meta checkpoint-style header; only the Comfy-Org repack is accepted",
            ));
        }
        self.expect("backbone.embeddings.cls_token", MlxDType::BF16, &[1, 1, DINO_DIM])?;
        self.expect("backbone.embeddings.register_tokens", MlxDType::BF16, &[1, 4, DINO_DIM])?;
        self.expect(
            "backbone.embeddings.patch_embeddings.weight",
            MlxDType::BF16,
            &[DINO_DIM, 3, PATCH, PATCH],
        )?;
        for i in [0, DINO_DEPTH - 1] {
            self.expect(
                &format!("backbone.layer.{i}.attention.q_proj.weight"),
                MlxDType::BF16,
                &[DINO_DIM, DINO_DIM],
            )?;
            self.expect(
                &format!("backbone.layer.{i}.mlp.gate_proj.weight"),
                MlxDType::BF16,
                &[DINO_FFN, DINO_DIM],
            )?;
            self.expect(
                &format!("backbone.layer.{i}.mlp.down_proj.weight"),
                MlxDType::BF16,
                &[DINO_DIM, DINO_FFN],
            )?;
            self.expect(&format!("backbone.layer.{i}.layer_scale1.lambda1"), MlxDType::BF16, &[DINO_DIM])?;
        }
        self.expect("backbone.norm.weight", MlxDType::BF16, &[DINO_DIM])?;
        for i in [0, DEC_DEPTH - 1] {
            self.expect(
                &format!("decoder.layers.{i}.cross_attn.k_proj.weight"),
                MlxDType::F32,
                &[DEC_INNER, DINO_DIM],
            )?;
            self.expect(
                &format!("decoder.layers.{i}.ffn.layers.0.0.weight"),
                MlxDType::F32,
                &[DEC_FFN, DEC_DIM],
            )?;
        }
        self.expect("decoder.norm_final.weight", MlxDType::F32, &[DEC_DIM])?;
        self.expect("head_pose.proj.layers.1.weight", MlxDType::F32, &[NPOSE, DEC_DIM])?;
        self.expect("head_camera.proj.layers.1.weight", MlxDType::F32, &[NCAM, DEC_DIM])?;
        self.expect("init_pose.weight", MlxDType::F32, &[1, NPOSE])?;
        self.expect("init_to_token_mhr.weight", MlxDType::F32, &[DEC_DIM, NPOSE + NCAM + 3])?;
        self.expect("prev_to_token_mhr.weight", MlxDType::F32, &[DEC_DIM, NPOSE + NCAM])?;
        self.expect("keypoint_embedding.weight", MlxDType::F32, &[NUM_KEYPOINTS, DEC_DIM])?;
        self.expect("ray_cond_emb.conv.weight", MlxDType::F32, &[DINO_DIM, DINO_DIM + 99, 1, 1])?;
        self.expect("head_pose.keypoint_mapping", MlxDType::F32, &[MHR_KEYPOINTS_ALL, MHR_VERTS + MHR_JOINTS])?;
        self.expect("mhr.base_shape", MlxDType::F32, &[MHR_VERTS, 3])?;
        self.expect("mhr.identity_basis", MlxDType::F32, &[NUM_SHAPE, MHR_VERTS, 3])?;
        self.expect("mhr.expr_basis", MlxDType::F32, &[NUM_EXPR, MHR_VERTS, 3])?;
        self.expect("mhr.param_transform", MlxDType::F32, &[MHR_JOINT_PARAMS, MHR_MODEL_PARAMS])?;
        self.expect("mhr.skel_joint_parents", MlxDType::I32, &[MHR_JOINTS])?;
        self.expect("mhr.lbs_inverse_bind_pose", MlxDType::F32, &[MHR_JOINTS, 8])?;
        Ok(())
    }
}
