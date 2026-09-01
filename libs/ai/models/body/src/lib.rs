//! SAM 3D Body, natively: one RGB crop of a person -> the MHR body
//! parameters, 70 keypoints and the posed mesh, on the makepad-ai-common
//! `gpu_*` surface (Metal on Apple, CUDA on Linux/Windows), with no Python
//! and no subprocess.
//!
//! The architecture is implemented after the SAM 3D Body paper
//! (arXiv:2602.15989) and the port spec in `local/agent_state/sam3dbody/`
//! (tensor inventory, op order, formulas), the DINOv3 backbone after the
//! Apache-2.0 HF transformers implementation the TRELLIS conditioner in
//! `makepad-ai-trellis` already follows, and the MHR rig after the
//! Apache-2.0 Momentum Human Rig release. Our code stays MIT with the
//! crate. Weights are SAM-Licensed and pulled at runtime from the
//! Comfy-Org repack (`Comfy-Org/sam-3d-body`); this repository never
//! redistributes them and never fetches facebook/* checkpoints.
//!
//! Module map (each a lane of the port; see the spec for the contract):
//! - [`weights`]: the single-file safetensors reader + architecture check.
//! - `dino`: the ViT-H+/16 backbone (32 blocks, 20 heads, SwiGLU, rope).
//! - `decoder`: prompt encoder, ray conditioning, the 6-layer promptable
//!   decoder with per-layer pose refinement, the pose/camera/hand-box heads.
//! - `mhr`: the rig — parameter transform, kinematics, blendshapes, pose
//!   correctives, linear blend skinning, keypoint regression.

pub use makepad_ai_common::backend;
pub use makepad_ai_common::error;
pub use makepad_ai_common::{emit_progress, DiffusionError, ProgressHook, Result};

pub mod condition;
pub mod decoder;
pub mod dino;
pub mod hands;
mod heads;
pub mod mhr;
pub mod model;
pub mod packet;
pub mod pose;
pub mod preprocess;
pub mod weights;

#[cfg(test)]
pub mod fixture;

/// Model input: the person crop the backbone sees.
pub const IMAGE_SIZE: usize = 512;
/// Backbone patch size; `IMAGE_SIZE / PATCH` = 32 patches a side.
pub const PATCH: usize = 16;
pub const PATCHES_SIDE: usize = IMAGE_SIZE / PATCH;
pub const NUM_PATCHES: usize = PATCHES_SIDE * PATCHES_SIDE;
/// Backbone width, depth, heads, SwiGLU hidden, prefix rows (cls + 4).
pub const DINO_DIM: usize = 1280;
pub const DINO_DEPTH: usize = 32;
pub const DINO_HEADS: usize = 20;
pub const DINO_HEAD_DIM: usize = 64;
pub const DINO_FFN: usize = 5120;
pub const DINO_PREFIX_TOKENS: usize = 5;
pub const DINO_NORM_EPS: f32 = 1e-5;
pub const DINO_ROPE_BASE: f32 = 100.0;
/// Decoder token width, attention inner width, heads, FFN width, depth.
pub const DEC_DIM: usize = 1024;
pub const DEC_INNER: usize = 512;
pub const DEC_HEADS: usize = 8;
pub const DEC_FFN: usize = 1024;
pub const DEC_DEPTH: usize = 6;
pub const DEC_NORM_EPS: f32 = 1e-6;
/// Pose head output: 6 (global rot 6d) + 260 (body pose continuous) + 45
/// (shape) + 28 (scale) + 108 (two hands x 54) + 72 (expression).
pub const NPOSE: usize = 519;
pub const NCAM: usize = 3;
pub const BODY_CONT_DIM: usize = 260;
pub const NUM_SHAPE: usize = 45;
pub const NUM_SCALE: usize = 28;
pub const NUM_HAND: usize = 54;
pub const NUM_EXPR: usize = 72;
pub const NUM_KEYPOINTS: usize = 70;
/// MHR rig sizes.
pub const MHR_JOINTS: usize = 127;
pub const MHR_VERTS: usize = 18439;
pub const MHR_FACES: usize = 36874;
pub const MHR_MODEL_PARAMS: usize = 249;
pub const MHR_JOINT_PARAMS: usize = 889;
pub const MHR_KEYPOINTS_ALL: usize = 308;
pub const ROPE_HALF: usize = DINO_HEAD_DIM / 2;
