//! HY-Motion 1.0 shared contracts and deterministic host-side math.
//!
//! This module intentionally describes the **FULL** 1.04B model only.  The
//! Lite checkpoint is a different architecture and silently accepting it
//! here would violate the runtime's quality contract.
//!
//! Reference: Tencent-Hunyuan/HY-Motion-1.0, full checkpoint `config.yml`.
//! The released network always allocates 360 motion rows and 128 text rows,
//! but padding rows are key-masked and every non-attention operation is
//! row-local.  Therefore inference may remove padding rows exactly, provided
//! text RoPE positions retain their reference offset at 360.  The fixed-seed
//! oracle measured cosine 0.9999999999992 (max abs 1.72e-5) between the
//! reference 488-row path and this 120+12-row packing, while reducing the
//! reference denoise time from 2.040 s to 1.317 s on the same RTX PRO 6000.

use crate::{DiffusionError, Result};

pub const HY_MOTION_MODEL_NAME: &str = "HY-Motion-1.0";
pub const HY_MOTION_INPUT_DIM: usize = 201;
pub const HY_MOTION_OUTPUT_DIM: usize = 201;
pub const HY_MOTION_HIDDEN: usize = 1280;
pub const HY_MOTION_HEADS: usize = 20;
pub const HY_MOTION_HEAD_DIM: usize = 64;
pub const HY_MOTION_MLP: usize = 5120;
pub const HY_MOTION_LAYERS: usize = 27;
pub const HY_MOTION_DOUBLE_LAYERS: usize = 9;
pub const HY_MOTION_SINGLE_LAYERS: usize = 18;
pub const HY_MOTION_TRAIN_FRAMES: usize = 360;
pub const HY_MOTION_MIN_FRAMES: usize = 20;
pub const HY_MOTION_MAX_FRAMES: usize = HY_MOTION_TRAIN_FRAMES;
pub const HY_MOTION_TEXT_TOKENS: usize = 128;
pub const HY_MOTION_CONTEXT_DIM: usize = 4096;
pub const HY_MOTION_VECTOR_DIM: usize = 768;
pub const HY_MOTION_FPS: usize = 30;
pub const HY_MOTION_STEPS: usize = 50;
pub const HY_MOTION_CFG: f32 = 5.0;
pub const HY_MOTION_TIME_FACTOR: f32 = 1000.0;
pub const HY_MOTION_ROPE_THETA: f64 = 10_000.0;
pub const HY_MOTION_NARROWBAND_FRAMES: usize = 60;
pub const HY_MOTION_NORM_EPS: f32 = 1.0e-6;
pub const HY_MOTION_BODY_JOINTS: usize = 22;
pub const HY_MOTION_ACTIVE_OUTPUT_DIM: usize = 3 + HY_MOTION_BODY_JOINTS * 6;

/// The one architecture accepted by this port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HyMotionConfig {
    pub input_dim: usize,
    pub output_dim: usize,
    pub hidden: usize,
    pub heads: usize,
    pub layers: usize,
    pub train_frames: usize,
    pub context_dim: usize,
    pub vector_dim: usize,
    pub narrowband_frames: usize,
    pub apply_rope_to_single_branch: bool,
}

impl HyMotionConfig {
    pub const FULL: Self = Self {
        input_dim: HY_MOTION_INPUT_DIM,
        output_dim: HY_MOTION_OUTPUT_DIM,
        hidden: HY_MOTION_HIDDEN,
        heads: HY_MOTION_HEADS,
        layers: HY_MOTION_LAYERS,
        train_frames: HY_MOTION_TRAIN_FRAMES,
        context_dim: HY_MOTION_CONTEXT_DIM,
        vector_dim: HY_MOTION_VECTOR_DIM,
        narrowband_frames: HY_MOTION_NARROWBAND_FRAMES,
        apply_rope_to_single_branch: false,
    };

    pub fn validate_full(self) -> Result<()> {
        if self != Self::FULL {
            return Err(DiffusionError::model(format!(
                "HY-Motion runtime accepts the full 1.0 architecture only: got {self:?}"
            )));
        }
        if self.hidden % self.heads != 0
            || self.hidden / self.heads != HY_MOTION_HEAD_DIM
            || self.layers % 3 != 0
            || self.layers / 3 != HY_MOTION_DOUBLE_LAYERS
        {
            return Err(DiffusionError::model(
                "HY-Motion full architecture has inconsistent derived dimensions",
            ));
        }
        Ok(())
    }
}

/// Active packed shape after exact padding-row removal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyMotionPackedShape {
    pub motion_tokens: usize,
    pub text_tokens: usize,
    /// Reference position id for every packed token. Motion stays at 0..M;
    /// text retains the official padded offset 360..360+T.
    pub rope_positions: Vec<usize>,
}

impl HyMotionPackedShape {
    pub fn new(motion_tokens: usize, text_tokens: usize) -> Result<Self> {
        if !(HY_MOTION_MIN_FRAMES..=HY_MOTION_TRAIN_FRAMES).contains(&motion_tokens) {
            return Err(DiffusionError::workflow(format!(
                "HY-Motion frame count must be {HY_MOTION_MIN_FRAMES}..={HY_MOTION_TRAIN_FRAMES}, got {motion_tokens}"
            )));
        }
        if text_tokens == 0 || text_tokens > HY_MOTION_TEXT_TOKENS {
            return Err(DiffusionError::workflow(format!(
                "HY-Motion text token count must be 1..={HY_MOTION_TEXT_TOKENS}, got {text_tokens}"
            )));
        }
        let mut rope_positions = Vec::with_capacity(motion_tokens + text_tokens);
        rope_positions.extend(0..motion_tokens);
        rope_positions.extend(
            HY_MOTION_TRAIN_FRAMES..HY_MOTION_TRAIN_FRAMES + text_tokens,
        );
        Ok(Self {
            motion_tokens,
            text_tokens,
            rope_positions,
        })
    }

    pub fn total_tokens(&self) -> usize {
        self.motion_tokens + self.text_tokens
    }

    /// Whether the reference additive attention mask permits this Q/K pair.
    /// Padding rows have already been removed, so only the narrowband and the
    /// asymmetric `text query -> motion key` prohibition remain.
    pub fn attention_allowed(&self, query: usize, key: usize) -> bool {
        if query >= self.total_tokens() || key >= self.total_tokens() {
            return false;
        }
        let query_is_motion = query < self.motion_tokens;
        let key_is_motion = key < self.motion_tokens;
        match (query_is_motion, key_is_motion) {
            (true, true) => query.abs_diff(key) <= HY_MOTION_NARROWBAND_FRAMES,
            (true, false) => true,
            (false, true) => false,
            (false, false) => true,
        }
    }

    /// Dense additive mask in row-major query/key order.  This is mainly a
    /// validation oracle; the CUDA path uses the structured mask directly.
    pub fn additive_attention_mask(&self) -> Vec<f32> {
        let tokens = self.total_tokens();
        let mut mask = vec![f32::NEG_INFINITY; tokens * tokens];
        for query in 0..tokens {
            for key in 0..tokens {
                if self.attention_allowed(query, key) {
                    mask[query * tokens + key] = 0.0;
                }
            }
        }
        mask
    }
}

/// Official explicit-Euler integration grid, including both endpoints.
pub fn hy_motion_euler_times(steps: usize) -> Result<Vec<f32>> {
    if steps == 0 {
        return Err(DiffusionError::workflow(
            "HY-Motion Euler step count must be non-zero",
        ));
    }
    Ok((0..=steps)
        .map(|index| index as f32 / steps as f32)
        .collect())
}

/// `basic + scale * (conditioned - basic)` in the checkpoint's CFG order.
pub fn hy_motion_cfg(
    basic: &[f32],
    conditioned: &[f32],
    scale: f32,
) -> Result<Vec<f32>> {
    if basic.len() != conditioned.len() {
        return Err(DiffusionError::model(format!(
            "HY-Motion CFG shape mismatch: {} vs {}",
            basic.len(),
            conditioned.len()
        )));
    }
    Ok(basic
        .iter()
        .zip(conditioned)
        .map(|(&base, &text)| base + scale * (text - base))
        .collect())
}

/// One explicit Euler update, matching torchdiffeq's `method="euler"`.
pub fn hy_motion_euler_step(latent: &mut [f32], velocity: &[f32], dt: f32) -> Result<()> {
    if latent.len() != velocity.len() {
        return Err(DiffusionError::model(format!(
            "HY-Motion Euler shape mismatch: {} vs {}",
            latent.len(),
            velocity.len()
        )));
    }
    for (x, &v) in latent.iter_mut().zip(velocity) {
        *x += dt * v;
    }
    Ok(())
}

/// Timestep embedding used by both the MMDiT and its text refiner: COS half,
/// then SIN half, with `t *= 1000` for the full checkpoint.
pub fn hy_motion_timestep_embedding(timestep: f32, dim: usize, factor: f32) -> Vec<f32> {
    let half = dim / 2;
    let mut out = vec![0.0f32; dim];
    for index in 0..half {
        let frequency = 10_000.0f64.powf(-(index as f64) / half as f64);
        let angle = timestep as f64 * factor as f64 * frequency;
        out[index] = angle.cos() as f32;
        out[half + index] = angle.sin() as f32;
    }
    out
}

/// Interleaved RoPE tables for arbitrary (possibly gapped) reference
/// positions. Returned rows hold `head_dim/2` pair frequencies.
pub fn hy_motion_rope_tables(
    positions: &[usize],
    head_dim: usize,
    theta: f64,
) -> Result<(Vec<f32>, Vec<f32>)> {
    if head_dim == 0 || head_dim % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "HY-Motion RoPE head dimension must be positive and even, got {head_dim}"
        )));
    }
    let half = head_dim / 2;
    let mut cos = vec![0.0f32; positions.len() * half];
    let mut sin = vec![0.0f32; positions.len() * half];
    for (row, &position) in positions.iter().enumerate() {
        for pair in 0..half {
            let frequency = theta.powf(-((2 * pair) as f64) / head_dim as f64);
            let angle = position as f64 * frequency;
            cos[row * half + pair] = angle.cos() as f32;
            sin[row * half + pair] = angle.sin() as f32;
        }
    }
    Ok((cos, sin))
}

/// Apply the reference's interleaved complex-pair RoPE to one packed row.
pub fn hy_motion_apply_rope_row(
    row: &mut [f32],
    position_row: usize,
    heads: usize,
    head_dim: usize,
    cos: &[f32],
    sin: &[f32],
) -> Result<()> {
    if row.len() != heads * head_dim || head_dim % 2 != 0 {
        return Err(DiffusionError::model("HY-Motion RoPE row shape mismatch"));
    }
    let half = head_dim / 2;
    let start = position_row
        .checked_mul(half)
        .ok_or_else(|| DiffusionError::model("HY-Motion RoPE table offset overflow"))?;
    if start + half > cos.len() || start + half > sin.len() {
        return Err(DiffusionError::model("HY-Motion RoPE table is too short"));
    }
    for head in 0..heads {
        for pair in 0..half {
            let index = head * head_dim + pair * 2;
            let real = row[index];
            let imag = row[index + 1];
            let c = cos[start + pair];
            let s = sin[start + pair];
            row[index] = real * c - imag * s;
            row[index + 1] = imag * c + real * s;
        }
    }
    Ok(())
}

/// Convert the checkpoint's 6D representation to a column-major 3x3
/// rotation matrix (columns b1, b2, cross(b1,b2)). The six inputs are laid
/// out as a PyTorch `[3,2]` view: a1=(0,2,4), a2=(1,3,5).
pub fn hy_motion_rot6d_to_matrix(value: [f32; 6]) -> [[f32; 3]; 3] {
    let a1 = [value[0], value[2], value[4]];
    let a2 = [value[1], value[3], value[5]];
    let b1 = normalize3(a1);
    let projection = dot3(b1, a2);
    let b2 = normalize3([
        a2[0] - projection * b1[0],
        a2[1] - projection * b1[1],
        a2[2] - projection * b1[2],
    ]);
    let b3 = cross3(b1, b2);
    [
        [b1[0], b2[0], b3[0]],
        [b1[1], b2[1], b3[1]],
        [b1[2], b2[2], b3[2]],
    ]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize3(value: [f32; 3]) -> [f32; 3] {
    // torch.nn.functional.normalize uses max(norm, eps); eps defaults 1e-12.
    let inv = 1.0 / dot3(value, value).sqrt().max(1.0e-12);
    [value[0] * inv, value[1] * inv, value[2] * inv]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_contract_is_internally_consistent() {
        HyMotionConfig::FULL.validate_full().unwrap();
        assert_eq!(HY_MOTION_DOUBLE_LAYERS + HY_MOTION_SINGLE_LAYERS, 27);
        assert_eq!(HY_MOTION_HEADS * HY_MOTION_HEAD_DIM, HY_MOTION_HIDDEN);
        assert_eq!(HY_MOTION_ACTIVE_OUTPUT_DIM, 135);
        // The released decoder intentionally ignores the final 66 channels.
        assert_eq!(HY_MOTION_OUTPUT_DIM - HY_MOTION_ACTIVE_OUTPUT_DIM, 66);
    }

    #[test]
    fn trimmed_shape_retains_reference_text_positions() {
        let shape = HyMotionPackedShape::new(120, 12).unwrap();
        assert_eq!(shape.total_tokens(), 132);
        assert_eq!(&shape.rope_positions[..3], &[0, 1, 2]);
        assert_eq!(shape.rope_positions[119], 119);
        assert_eq!(shape.rope_positions[120], 360);
        assert_eq!(shape.rope_positions[131], 371);
    }

    #[test]
    fn structured_mask_matches_reference_quadrants() {
        let shape = HyMotionPackedShape::new(120, 12).unwrap();
        // Motion narrowband is inclusive at 60.
        assert!(shape.attention_allowed(0, 60));
        assert!(!shape.attention_allowed(0, 61));
        // Motion queries see all valid text; text queries never see motion.
        assert!(shape.attention_allowed(0, 120));
        assert!(!shape.attention_allowed(120, 0));
        assert!(shape.attention_allowed(120, 131));
    }

    #[test]
    fn euler_and_cfg_match_hand_computed_values() {
        let times = hy_motion_euler_times(50).unwrap();
        assert_eq!(times.len(), 51);
        assert!((times[1] - 0.02).abs() < 1e-7);
        assert_eq!(*times.last().unwrap(), 1.0);
        let velocity = hy_motion_cfg(&[1.0, -2.0], &[3.0, 2.0], 5.0).unwrap();
        assert_eq!(velocity, vec![11.0, 18.0]);
        let mut latent = [0.5, -0.5];
        hy_motion_euler_step(&mut latent, &velocity, 0.02).unwrap();
        assert!((latent[0] - 0.72).abs() < 1e-6);
        assert!((latent[1] - -0.14).abs() < 1e-6);
    }

    #[test]
    fn timestep_embedding_uses_cos_then_sin_and_factor_1000() {
        let embedding = hy_motion_timestep_embedding(0.0, 8, HY_MOTION_TIME_FACTOR);
        assert_eq!(&embedding[..4], &[1.0, 1.0, 1.0, 1.0]);
        assert_eq!(&embedding[4..], &[0.0, 0.0, 0.0, 0.0]);
        let at_one = hy_motion_timestep_embedding(1.0, 8, HY_MOTION_TIME_FACTOR);
        assert!((at_one[0] - 1000.0f32.cos()).abs() < 1e-6);
        assert!((at_one[4] - 1000.0f32.sin()).abs() < 1e-6);
    }

    #[test]
    fn gapped_interleaved_rope_matches_scalar_complex_product() {
        let shape = HyMotionPackedShape::new(20, 1).unwrap();
        let (cos, sin) = hy_motion_rope_tables(
            &shape.rope_positions,
            4,
            HY_MOTION_ROPE_THETA,
        )
        .unwrap();
        let mut row = [1.0, 2.0, 3.0, 4.0];
        hy_motion_apply_rope_row(&mut row, 20, 1, 4, &cos, &sin).unwrap();
        let (s0, c0) = 360.0f32.sin_cos();
        let (s1, c1) = 3.6f32.sin_cos();
        assert!((row[0] - (c0 - 2.0 * s0)).abs() < 2e-5);
        assert!((row[1] - (s0 + 2.0 * c0)).abs() < 2e-5);
        assert!((row[2] - (3.0 * c1 - 4.0 * s1)).abs() < 2e-5);
        assert!((row[3] - (3.0 * s1 + 4.0 * c1)).abs() < 2e-5);
    }

    #[test]
    fn rot6d_identity_and_orthogonality() {
        let identity = hy_motion_rot6d_to_matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        for row in 0..3 {
            for col in 0..3 {
                let expected = if row == col { 1.0 } else { 0.0 };
                assert!((identity[row][col] - expected).abs() < 1e-6);
            }
        }
        let matrix = hy_motion_rot6d_to_matrix([0.7, -0.2, 0.4, 0.9, -0.1, 0.3]);
        for col in 0..3 {
            let norm = (0..3).map(|row| matrix[row][col].powi(2)).sum::<f32>();
            assert!((norm - 1.0).abs() < 1e-5);
        }
    }
}
