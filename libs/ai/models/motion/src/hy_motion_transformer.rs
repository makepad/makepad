//! Device-resident full HY-Motion 1.0 transformer.
//!
//! This port starts from a deliberately strict f32 execution path. The
//! fixed-seed oracle is f32, so reduced weight/activation precision is an
//! optimization gate, not an implicit behavior change.

use std::collections::BTreeMap;

use crate::backend::{
    gpu_add, gpu_attention_packed, gpu_attention_packed_motion_text, gpu_concat_cols,
    gpu_concat_rows, gpu_download, gpu_gated_residual_mod, gpu_gelu,
    gpu_layer_norm_mod, gpu_layer_norm_mul_add, gpu_layer_norm_mul_add_grouped,
    gpu_linear_f32_resident, gpu_rms_norm_mul, gpu_rope_interleaved, gpu_silu,
    gpu_slice_cols, gpu_slice_rows, gpu_upload, GpuTensor,
};
use crate::hy_motion::{
    hy_motion_euler_times, hy_motion_rope_tables, hy_motion_timestep_embedding,
    HyMotionPackedShape,
    HY_MOTION_CONTEXT_DIM, HY_MOTION_DOUBLE_LAYERS, HY_MOTION_HEADS,
    HY_MOTION_HEAD_DIM, HY_MOTION_HIDDEN, HY_MOTION_INPUT_DIM, HY_MOTION_MLP,
    HY_MOTION_NARROWBAND_FRAMES, HY_MOTION_NORM_EPS, HY_MOTION_OUTPUT_DIM,
    HY_MOTION_ROPE_THETA, HY_MOTION_SINGLE_LAYERS, HY_MOTION_TIME_FACTOR,
    HY_MOTION_VECTOR_DIM,
};
use crate::hy_motion_weights::{
    hy_motion_tensor_specs, HyMotionCheckpoint, HyMotionTensorSpec,
    HY_MOTION_TEXT_REFINER_LAYERS,
};
use crate::{DiffusionError, Result};

const HY_MOTION_CACHE_NAMESPACE: &str = "hy-motion-1.0-f32";

/// Resident tensors required by the selected execution stage. Matrices and
/// linear biases live on device; one-dimensional values are retained on host
/// too for the existing normalization kernels' constant-vector interface.
pub struct HyMotionDeviceWeights {
    device: BTreeMap<String, GpuTensor>,
    host_vectors: BTreeMap<String, Vec<f32>>,
    parameter_count: usize,
}

/// Timestep-invariant conditioning for one CFG branch.
pub struct HyMotionPreparedBranch {
    context_projected: GpuTensor,
    context_mean: Vec<f32>,
    vector_projected: GpuTensor,
    text_tokens: usize,
}

/// Shape-dependent, timestep-invariant RoPE state.
pub struct HyMotionPreparedShape {
    rope_cos: GpuTensor,
    rope_sin: GpuTensor,
    motion_tokens: usize,
    text_tokens: usize,
}

/// Fully resident shape and both classifier-free-guidance branches. The
/// unconditional branch is derived from the checkpoint's learned null
/// tensors and broadcast to the exact number of real prompt tokens.
pub struct HyMotionPreparedCfg {
    shape: HyMotionPreparedShape,
    basic: HyMotionPreparedBranch,
    conditioned: HyMotionPreparedBranch,
}

impl HyMotionPreparedCfg {
    pub fn motion_tokens(&self) -> usize {
        self.shape.motion_tokens
    }

    pub fn text_tokens(&self) -> usize {
        self.shape.text_tokens
    }
}

impl HyMotionDeviceWeights {
    /// Upload every full-model transformer tensor (plus null/mean/std state).
    pub fn load_full(checkpoint: &mut HyMotionCheckpoint) -> Result<Self> {
        Self::load_filtered(checkpoint, |_| true)
    }

    /// Small stagewise set: context projection and the complete two-block
    /// token refiner. Useful for the first oracle parity gate.
    pub fn load_text_refiner(checkpoint: &mut HyMotionCheckpoint) -> Result<Self> {
        Self::load_filtered(checkpoint, |name| {
            name.starts_with("motion_transformer.ctxt_encoder.")
                || name.starts_with("motion_transformer.text_refiner.")
        })
    }

    pub fn load_filtered(
        checkpoint: &mut HyMotionCheckpoint,
        include: impl Fn(&str) -> bool,
    ) -> Result<Self> {
        let specs = hy_motion_tensor_specs();
        let mut device = BTreeMap::new();
        let mut host_vectors = BTreeMap::new();
        let mut parameter_count = 0usize;
        for spec in specs.iter().filter(|spec| include(&spec.name)) {
            let values = checkpoint.f32(&spec.name)?;
            let (rows, cols) = tensor_matrix_shape(spec)?;
            let tensor = gpu_upload(&values, rows, cols).map_err(|error| {
                DiffusionError::model(format!(
                    "upload HY-Motion tensor {} ({rows}x{cols}): {error}",
                    spec.name
                ))
            })?;
            if spec.shape.len() == 1
                || spec.name == "null_vtxt_feat"
                || spec.name == "null_ctxt_input"
            {
                host_vectors.insert(spec.name.clone(), values);
            }
            parameter_count += spec.parameter_count();
            device.insert(spec.name.clone(), tensor);
        }
        Ok(Self {
            device,
            host_vectors,
            parameter_count,
        })
    }

    pub fn parameter_count(&self) -> usize {
        self.parameter_count
    }

    pub fn contains(&self, name: &str) -> bool {
        self.device.contains_key(name)
    }

    /// Host normalization vectors bundled in the full checkpoint. These are
    /// retained alongside the resident tensors for the exact CPU decoder.
    pub fn normalization_stats(&self) -> Result<(&[f32], &[f32])> {
        Ok((self.vector("mean")?, self.vector("std")?))
    }

    fn tensor(&self, name: &str) -> Result<&GpuTensor> {
        self.device.get(name).ok_or_else(|| {
            DiffusionError::model(format!("HY-Motion device tensor not loaded: {name}"))
        })
    }

    fn vector(&self, name: &str) -> Result<&[f32]> {
        self.host_vectors
            .get(name)
            .map(Vec::as_slice)
            .ok_or_else(|| {
                DiffusionError::model(format!("HY-Motion host vector not loaded: {name}"))
            })
    }

    fn linear(&self, input: &GpuTensor, prefix: &str) -> Result<GpuTensor> {
        let weight_name = format!("{prefix}.weight");
        let bias_name = format!("{prefix}.bias");
        gpu_linear_f32_resident(
            input,
            self.tensor(&weight_name)?,
            Some(self.tensor(&bias_name)?),
        )
        .map_err(|error| DiffusionError::model(format!("HY-Motion {prefix}: {error}")))
    }

    fn affine_layer_norm(&self, input: &GpuTensor, prefix: &str) -> Result<GpuTensor> {
        gpu_layer_norm_mul_add(
            input,
            self.vector(&format!("{prefix}.weight"))?,
            self.vector(&format!("{prefix}.bias"))?,
            HY_MOTION_NORM_EPS,
        )
        .map_err(DiffusionError::model)
    }

    fn affine_layer_norm_grouped(
        &self,
        input: &GpuTensor,
        prefix: &str,
        group_cols: usize,
    ) -> Result<GpuTensor> {
        gpu_layer_norm_mul_add_grouped(
            input,
            group_cols,
            self.vector(&format!("{prefix}.weight"))?,
            self.vector(&format!("{prefix}.bias"))?,
            HY_MOTION_NORM_EPS,
        )
        .map_err(DiffusionError::model)
    }

    fn rms_qk(&self, input: &GpuTensor, weight_name: &str) -> Result<GpuTensor> {
        gpu_rms_norm_mul(
            input,
            HY_MOTION_HEAD_DIM,
            HY_MOTION_CACHE_NAMESPACE,
            weight_name,
            self.vector(weight_name)?,
            HY_MOTION_NORM_EPS,
        )
        .map_err(DiffusionError::model)
    }

    pub fn context_projection(&self, raw_context: &GpuTensor) -> Result<GpuTensor> {
        if raw_context.cols() != HY_MOTION_CONTEXT_DIM {
            return Err(DiffusionError::model(format!(
                "HY-Motion context width {}, expected {HY_MOTION_CONTEXT_DIM}",
                raw_context.cols()
            )));
        }
        self.linear(raw_context, "motion_transformer.ctxt_encoder")
    }

    /// The official text refiner, operating only on real (already-trimmed)
    /// text rows. `context_mean` is the column mean of `context_projected`.
    pub fn text_refiner(
        &self,
        context_projected: &GpuTensor,
        context_mean: &[f32],
        timestep: f32,
    ) -> Result<GpuTensor> {
        if context_projected.cols() != HY_MOTION_HIDDEN
            || context_mean.len() != HY_MOTION_HIDDEN
        {
            return Err(DiffusionError::model(
                "HY-Motion text refiner input shape mismatch",
            ));
        }

        let time_embedding = hy_motion_timestep_embedding(timestep, HY_MOTION_HIDDEN, 1.0);
        let time_embedding = gpu_upload(&time_embedding, 1, HY_MOTION_HIDDEN)
            .map_err(DiffusionError::model)?;
        let time = self.linear(
            &time_embedding,
            "motion_transformer.text_refiner.timestep_encoder.blocks.0",
        )?;
        let time = gpu_silu(&time).map_err(DiffusionError::model)?;
        let time = self.linear(
            &time,
            "motion_transformer.text_refiner.timestep_encoder.blocks.2",
        )?;

        let context_mean = gpu_upload(context_mean, 1, HY_MOTION_HIDDEN)
            .map_err(DiffusionError::model)?;
        let context = self.linear(
            &context_mean,
            "motion_transformer.text_refiner.context_encoder.linears.0",
        )?;
        let context = gpu_silu(&context).map_err(DiffusionError::model)?;
        let context = self.linear(
            &context,
            "motion_transformer.text_refiner.context_encoder.linears.2",
        )?;
        let conditioning = gpu_add(&time, &context).map_err(DiffusionError::model)?;
        let conditioning_silu = gpu_silu(&conditioning).map_err(DiffusionError::model)?;

        let mut hidden = self.linear(
            context_projected,
            "motion_transformer.text_refiner.input_embedder",
        )?;
        let attention_scale = 1.0 / (HY_MOTION_HEAD_DIM as f32).sqrt();
        for layer in 0..HY_MOTION_TEXT_REFINER_LAYERS {
            let prefix = format!(
                "motion_transformer.text_refiner.individual_token_refiner.blocks.{layer}"
            );
            let mods = self.linear(
                &conditioning_silu,
                &format!("{prefix}.adaLN_modulation.linear"),
            )?;
            let norm = self.affine_layer_norm(&hidden, &format!("{prefix}.norm1"))?;
            let qkv = self.linear(&norm, &format!("{prefix}.self_attn_qkv"))?;
            let q = gpu_slice_cols(&qkv, 0, HY_MOTION_HIDDEN)
                .map_err(DiffusionError::model)?;
            let k = gpu_slice_cols(&qkv, HY_MOTION_HIDDEN, HY_MOTION_HIDDEN)
                .map_err(DiffusionError::model)?;
            let v = gpu_slice_cols(&qkv, 2 * HY_MOTION_HIDDEN, HY_MOTION_HIDDEN)
                .map_err(DiffusionError::model)?;
            let q = self.affine_layer_norm_grouped(
                &q,
                &format!("{prefix}.self_attn_q_norm"),
                HY_MOTION_HEAD_DIM,
            )?;
            let k = self.affine_layer_norm_grouped(
                &k,
                &format!("{prefix}.self_attn_k_norm"),
                HY_MOTION_HEAD_DIM,
            )?;
            let attention = gpu_attention_packed(
                &q,
                &k,
                &v,
                HY_MOTION_HEADS,
                attention_scale,
            )
            .map_err(DiffusionError::model)?;
            let attention = self.linear(&attention, &format!("{prefix}.self_attn_proj"))?;
            hidden = gpu_gated_residual_mod(&hidden, &attention, &mods, 0)
                .map_err(DiffusionError::model)?;

            let norm = self.affine_layer_norm(&hidden, &format!("{prefix}.norm2"))?;
            let mlp = self.linear(&norm, &format!("{prefix}.mlp.fc1"))?;
            let mlp = gpu_silu(&mlp).map_err(DiffusionError::model)?;
            let mlp = self.linear(&mlp, &format!("{prefix}.mlp.fc2"))?;
            hidden = gpu_gated_residual_mod(
                &hidden,
                &mlp,
                &mods,
                HY_MOTION_HIDDEN,
            )
            .map_err(DiffusionError::model)?;
        }
        Ok(hidden)
    }

    pub fn input_projection(&self, latent: &GpuTensor) -> Result<GpuTensor> {
        if latent.cols() != HY_MOTION_INPUT_DIM {
            return Err(DiffusionError::model("HY-Motion latent width mismatch"));
        }
        self.linear(latent, "motion_transformer.input_encoder")
    }

    pub fn adapter(&self, vtxt: &GpuTensor, timestep: f32) -> Result<GpuTensor> {
        let vector = self.vector_projection(vtxt)?;
        let time = self.timestep_projection(timestep)?;
        gpu_add(&time, &vector).map_err(DiffusionError::model)
    }

    pub fn vector_projection(&self, vtxt: &GpuTensor) -> Result<GpuTensor> {
        if vtxt.rows() != 1 || vtxt.cols() != HY_MOTION_VECTOR_DIM {
            return Err(DiffusionError::model("HY-Motion CLIP vector shape mismatch"));
        }
        let vector = self.linear(vtxt, "motion_transformer.vtxt_encoder.linears.0")?;
        let vector = gpu_silu(&vector).map_err(DiffusionError::model)?;
        self.linear(&vector, "motion_transformer.vtxt_encoder.linears.2")
    }

    pub fn timestep_projection(&self, timestep: f32) -> Result<GpuTensor> {
        let embedding = hy_motion_timestep_embedding(
            timestep,
            HY_MOTION_HIDDEN,
            HY_MOTION_TIME_FACTOR,
        );
        let embedding = gpu_upload(&embedding, 1, HY_MOTION_HIDDEN)
            .map_err(DiffusionError::model)?;
        let time = self.linear(
            &embedding,
            "motion_transformer.timestep_encoder.blocks.0",
        )?;
        let time = gpu_silu(&time).map_err(DiffusionError::model)?;
        self.linear(&time, "motion_transformer.timestep_encoder.blocks.2")
    }

    pub fn prepare_branch(
        &self,
        context: &[f32],
        vtxt: &[f32],
        text_tokens: usize,
    ) -> Result<HyMotionPreparedBranch> {
        if context.len() != text_tokens * HY_MOTION_CONTEXT_DIM
            || vtxt.len() != HY_MOTION_VECTOR_DIM
        {
            return Err(DiffusionError::model(
                "HY-Motion prepared branch input shape mismatch",
            ));
        }
        let context = gpu_upload(context, text_tokens, HY_MOTION_CONTEXT_DIM)
            .map_err(DiffusionError::model)?;
        let context_projected = self.context_projection(&context)?;
        let context_host = gpu_download(&context_projected).map_err(DiffusionError::model)?;
        let context_mean = mean_rows(&context_host, text_tokens, HY_MOTION_HIDDEN)?;
        let vtxt = gpu_upload(vtxt, 1, HY_MOTION_VECTOR_DIM).map_err(DiffusionError::model)?;
        let vector_projected = self.vector_projection(&vtxt)?;
        Ok(HyMotionPreparedBranch {
            context_projected,
            context_mean,
            vector_projected,
            text_tokens,
        })
    }

    pub fn prepare_shape(
        motion_tokens: usize,
        text_tokens: usize,
    ) -> Result<HyMotionPreparedShape> {
        let packed_shape = HyMotionPackedShape::new(motion_tokens, text_tokens)?;
        let (rope_cos, rope_sin) = hy_motion_rope_tables(
            &packed_shape.rope_positions,
            HY_MOTION_HEAD_DIM,
            HY_MOTION_ROPE_THETA,
        )?;
        Ok(HyMotionPreparedShape {
            rope_cos: gpu_upload(
                &rope_cos,
                packed_shape.total_tokens(),
                HY_MOTION_HEAD_DIM / 2,
            )
            .map_err(DiffusionError::model)?,
            rope_sin: gpu_upload(
                &rope_sin,
                packed_shape.total_tokens(),
                HY_MOTION_HEAD_DIM / 2,
            )
            .map_err(DiffusionError::model)?,
            motion_tokens,
            text_tokens,
        })
    }

    /// Prepare a prompt's conditional branch alongside the learned null CFG
    /// branch. `context` contains only real Qwen tokens, with no padding rows.
    pub fn prepare_cfg(
        &self,
        context: &[f32],
        vtxt: &[f32],
        motion_tokens: usize,
    ) -> Result<HyMotionPreparedCfg> {
        if context.is_empty() || context.len() % HY_MOTION_CONTEXT_DIM != 0 {
            return Err(DiffusionError::workflow(
                "HY-Motion prompt context must contain complete non-empty 4096-wide rows",
            ));
        }
        let text_tokens = context.len() / HY_MOTION_CONTEXT_DIM;
        let shape = Self::prepare_shape(motion_tokens, text_tokens)?;
        let conditioned = self.prepare_branch(context, vtxt, text_tokens)?;
        let null_context = self.vector("null_ctxt_input")?.repeat(text_tokens);
        let basic = self.prepare_branch(
            &null_context,
            self.vector("null_vtxt_feat")?,
            text_tokens,
        )?;
        Ok(HyMotionPreparedCfg {
            shape,
            basic,
            conditioned,
        })
    }

    /// Sample one normalized HY-Motion latent using the official explicit
    /// Euler grid and checkpoint CFG ordering. Conditioning and RoPE remain
    /// device-resident for the entire trajectory; only the changing latent
    /// and the two final velocity fields cross the host boundary per step.
    pub fn sample_cfg_euler(
        &self,
        initial_latent: &[f32],
        prepared: &HyMotionPreparedCfg,
        steps: usize,
        guidance: f32,
    ) -> Result<Vec<f32>> {
        self.sample_cfg_euler_controlled(
            initial_latent,
            prepared,
            steps,
            guidance,
            None,
            None,
        )
    }

    /// [`Self::sample_cfg_euler`] with one callback/cancellation boundary per
    /// Euler step. The callback fires immediately before each pair of CFG
    /// forwards using one-based progress (`1..=steps`).
    pub fn sample_cfg_euler_controlled(
        &self,
        initial_latent: &[f32],
        prepared: &HyMotionPreparedCfg,
        steps: usize,
        guidance: f32,
        mut on_step: Option<&mut dyn FnMut(usize, usize)>,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Vec<f32>> {
        let expected = prepared.motion_tokens() * HY_MOTION_INPUT_DIM;
        if initial_latent.len() != expected {
            return Err(DiffusionError::workflow(format!(
                "HY-Motion initial latent has {} values, expected {expected}",
                initial_latent.len()
            )));
        }
        if !guidance.is_finite() {
            return Err(DiffusionError::workflow(
                "HY-Motion guidance must be finite",
            ));
        }
        let times = hy_motion_euler_times(steps)?;
        let mut latent = initial_latent.to_vec();
        for (step, window) in times.windows(2).enumerate() {
            if cancel.map_or(false, |cancelled| cancelled()) {
                return Err(DiffusionError::Cancelled);
            }
            if let Some(callback) = on_step.as_deref_mut() {
                callback(step + 1, steps);
            }
            let timestep = window[0];
            let dt = window[1] - window[0];
            let basic = self.forward_prepared(
                &latent,
                timestep,
                &prepared.basic,
                &prepared.shape,
            )?;
            let conditioned = self.forward_prepared(
                &latent,
                timestep,
                &prepared.conditioned,
                &prepared.shape,
            )?;
            for ((value, &basic), &conditioned) in
                latent.iter_mut().zip(&basic).zip(&conditioned)
            {
                let velocity = basic + guidance * (conditioned - basic);
                *value += dt * velocity;
            }
        }
        Ok(latent)
    }

    pub fn double_block(
        &self,
        layer: usize,
        motion: GpuTensor,
        text: GpuTensor,
        adapter: &GpuTensor,
        rope_cos: &GpuTensor,
        rope_sin: &GpuTensor,
    ) -> Result<(GpuTensor, GpuTensor)> {
        if layer >= HY_MOTION_DOUBLE_LAYERS {
            return Err(DiffusionError::model("HY-Motion double block out of range"));
        }
        let prefix = format!("motion_transformer.double_blocks.{layer}");
        let adapter_silu = gpu_silu(adapter).map_err(DiffusionError::model)?;
        let motion_mod = self.linear(&adapter_silu, &format!("{prefix}.motion_mod.linear"))?;
        let text_mod = self.linear(&adapter_silu, &format!("{prefix}.text_mod.linear"))?;

        let motion_norm = gpu_layer_norm_mod(
            &motion,
            &motion_mod,
            HY_MOTION_HIDDEN,
            0,
            HY_MOTION_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let text_norm = gpu_layer_norm_mod(
            &text,
            &text_mod,
            HY_MOTION_HIDDEN,
            0,
            HY_MOTION_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let motion_qkv = self.linear(&motion_norm, &format!("{prefix}.motion_qkv"))?;
        let text_qkv = self.linear(&text_norm, &format!("{prefix}.text_qkv"))?;
        let (motion_q, motion_k, motion_v) = split_qkv(&motion_qkv)?;
        let (text_q, text_k, text_v) = split_qkv(&text_qkv)?;
        let motion_q = self.rms_qk(&motion_q, &format!("{prefix}.motion_q_norm.weight"))?;
        let motion_k = self.rms_qk(&motion_k, &format!("{prefix}.motion_k_norm.weight"))?;
        let text_q = self.rms_qk(&text_q, &format!("{prefix}.text_q_norm.weight"))?;
        let text_k = self.rms_qk(&text_k, &format!("{prefix}.text_k_norm.weight"))?;

        let q = gpu_concat_rows(&motion_q, &text_q).map_err(DiffusionError::model)?;
        let k = gpu_concat_rows(&motion_k, &text_k).map_err(DiffusionError::model)?;
        let v = gpu_concat_rows(&motion_v, &text_v).map_err(DiffusionError::model)?;
        let q = gpu_rope_interleaved(&q, HY_MOTION_HEADS, rope_cos, rope_sin)
            .map_err(DiffusionError::model)?;
        let k = gpu_rope_interleaved(&k, HY_MOTION_HEADS, rope_cos, rope_sin)
            .map_err(DiffusionError::model)?;
        let attention = gpu_attention_packed_motion_text(
            &q,
            &k,
            &v,
            HY_MOTION_HEADS,
            1.0 / (HY_MOTION_HEAD_DIM as f32).sqrt(),
            motion.rows(),
            HY_MOTION_NARROWBAND_FRAMES,
        )
        .map_err(DiffusionError::model)?;
        let motion_attention = gpu_slice_rows(&attention, 0, motion.rows())
            .map_err(DiffusionError::model)?;
        let text_attention = gpu_slice_rows(&attention, motion.rows(), text.rows())
            .map_err(DiffusionError::model)?;
        let motion_attention =
            self.linear(&motion_attention, &format!("{prefix}.motion_out_proj"))?;
        let text_attention =
            self.linear(&text_attention, &format!("{prefix}.text_out_proj"))?;
        let mut motion = gpu_gated_residual_mod(
            &motion,
            &motion_attention,
            &motion_mod,
            2 * HY_MOTION_HIDDEN,
        )
        .map_err(DiffusionError::model)?;
        let mut text = gpu_gated_residual_mod(
            &text,
            &text_attention,
            &text_mod,
            2 * HY_MOTION_HIDDEN,
        )
        .map_err(DiffusionError::model)?;

        let motion_norm = gpu_layer_norm_mod(
            &motion,
            &motion_mod,
            4 * HY_MOTION_HIDDEN,
            3 * HY_MOTION_HIDDEN,
            HY_MOTION_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let text_norm = gpu_layer_norm_mod(
            &text,
            &text_mod,
            4 * HY_MOTION_HIDDEN,
            3 * HY_MOTION_HIDDEN,
            HY_MOTION_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let motion_mlp = main_mlp(self, &motion_norm, &format!("{prefix}.motion_mlp"))?;
        let text_mlp = main_mlp(self, &text_norm, &format!("{prefix}.text_mlp"))?;
        motion = gpu_gated_residual_mod(
            &motion,
            &motion_mlp,
            &motion_mod,
            5 * HY_MOTION_HIDDEN,
        )
        .map_err(DiffusionError::model)?;
        text = gpu_gated_residual_mod(
            &text,
            &text_mlp,
            &text_mod,
            5 * HY_MOTION_HIDDEN,
        )
        .map_err(DiffusionError::model)?;
        Ok((motion, text))
    }

    pub fn single_block(
        &self,
        layer: usize,
        joint: GpuTensor,
        motion_tokens: usize,
        adapter: &GpuTensor,
        rope_cos: &GpuTensor,
        rope_sin: &GpuTensor,
    ) -> Result<GpuTensor> {
        if layer >= HY_MOTION_SINGLE_LAYERS || motion_tokens >= joint.rows() {
            return Err(DiffusionError::model("HY-Motion single block shape mismatch"));
        }
        let prefix = format!("motion_transformer.single_blocks.{layer}");
        let adapter_silu = gpu_silu(adapter).map_err(DiffusionError::model)?;
        let mods = self.linear(&adapter_silu, &format!("{prefix}.modulation.linear"))?;
        let norm = gpu_layer_norm_mod(
            &joint,
            &mods,
            HY_MOTION_HIDDEN,
            0,
            HY_MOTION_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let linear1 = self.linear(&norm, &format!("{prefix}.linear1"))?;
        let q = gpu_slice_cols(&linear1, 0, HY_MOTION_HIDDEN)
            .map_err(DiffusionError::model)?;
        let k = gpu_slice_cols(&linear1, HY_MOTION_HIDDEN, HY_MOTION_HIDDEN)
            .map_err(DiffusionError::model)?;
        let v = gpu_slice_cols(&linear1, 2 * HY_MOTION_HIDDEN, HY_MOTION_HIDDEN)
            .map_err(DiffusionError::model)?;
        let mlp = gpu_slice_cols(&linear1, 3 * HY_MOTION_HIDDEN, HY_MOTION_MLP)
            .map_err(DiffusionError::model)?;
        let q = self.rms_qk(&q, &format!("{prefix}.q_norm.weight"))?;
        let k = self.rms_qk(&k, &format!("{prefix}.k_norm.weight"))?;
        let q = gpu_rope_interleaved(&q, HY_MOTION_HEADS, rope_cos, rope_sin)
            .map_err(DiffusionError::model)?;
        let k = gpu_rope_interleaved(&k, HY_MOTION_HEADS, rope_cos, rope_sin)
            .map_err(DiffusionError::model)?;
        let attention = gpu_attention_packed_motion_text(
            &q,
            &k,
            &v,
            HY_MOTION_HEADS,
            1.0 / (HY_MOTION_HEAD_DIM as f32).sqrt(),
            motion_tokens,
            HY_MOTION_NARROWBAND_FRAMES,
        )
        .map_err(DiffusionError::model)?;
        let mlp = gpu_gelu(&mlp).map_err(DiffusionError::model)?;
        let fused = gpu_concat_cols(&[&attention, &mlp]).map_err(DiffusionError::model)?;
        let update = self.linear(&fused, &format!("{prefix}.linear2"))?;
        gpu_gated_residual_mod(&joint, &update, &mods, 2 * HY_MOTION_HIDDEN)
            .map_err(DiffusionError::model)
    }

    pub fn final_layer(&self, motion: &GpuTensor, adapter: &GpuTensor) -> Result<GpuTensor> {
        let adapter_silu = gpu_silu(adapter).map_err(DiffusionError::model)?;
        let mods = self.linear(
            &adapter_silu,
            "motion_transformer.final_layer.adaLN_modulation.linear",
        )?;
        let norm = gpu_layer_norm_mod(
            motion,
            &mods,
            HY_MOTION_HIDDEN,
            0,
            HY_MOTION_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        self.linear(&norm, "motion_transformer.final_layer.linear")
    }

    /// Complete one-branch denoiser forward. CFG calls this once for the null
    /// branch and once for the conditioned branch until a batch-aware fused
    /// attention path is validated.
    pub fn forward(
        &self,
        latent: &[f32],
        context: &[f32],
        vtxt: &[f32],
        timestep: f32,
        motion_tokens: usize,
        text_tokens: usize,
    ) -> Result<Vec<f32>> {
        if latent.len() != motion_tokens * HY_MOTION_INPUT_DIM
            || context.len() != text_tokens * HY_MOTION_CONTEXT_DIM
            || vtxt.len() != HY_MOTION_VECTOR_DIM
        {
            return Err(DiffusionError::model("HY-Motion forward input shape mismatch"));
        }
        let shape = Self::prepare_shape(motion_tokens, text_tokens)?;
        let branch = self.prepare_branch(context, vtxt, text_tokens)?;
        self.forward_prepared(latent, timestep, &branch, &shape)
    }

    /// One forward using resident shape/conditioning state. Only the changing
    /// latent goes up and the final velocity comes down.
    pub fn forward_prepared(
        &self,
        latent: &[f32],
        timestep: f32,
        branch: &HyMotionPreparedBranch,
        shape: &HyMotionPreparedShape,
    ) -> Result<Vec<f32>> {
        if latent.len() != shape.motion_tokens * HY_MOTION_INPUT_DIM
            || branch.text_tokens != shape.text_tokens
        {
            return Err(DiffusionError::model(
                "HY-Motion prepared forward input shape mismatch",
            ));
        }
        let latent = gpu_upload(latent, shape.motion_tokens, HY_MOTION_INPUT_DIM)
            .map_err(DiffusionError::model)?;
        let mut motion = self.input_projection(&latent)?;
        let mut text = self.text_refiner(
            &branch.context_projected,
            &branch.context_mean,
            timestep,
        )?;
        let time = self.timestep_projection(timestep)?;
        let adapter = gpu_add(&time, &branch.vector_projected).map_err(DiffusionError::model)?;

        for layer in 0..HY_MOTION_DOUBLE_LAYERS {
            (motion, text) = self.double_block(
                layer,
                motion,
                text,
                &adapter,
                &shape.rope_cos,
                &shape.rope_sin,
            )?;
        }
        let mut joint = gpu_concat_rows(&motion, &text).map_err(DiffusionError::model)?;
        for layer in 0..HY_MOTION_SINGLE_LAYERS {
            joint = self.single_block(
                layer,
                joint,
                shape.motion_tokens,
                &adapter,
                &shape.rope_cos,
                &shape.rope_sin,
            )?;
        }
        let motion = gpu_slice_rows(&joint, 0, shape.motion_tokens).map_err(DiffusionError::model)?;
        let output = self.final_layer(&motion, &adapter)?;
        if output.cols() != HY_MOTION_OUTPUT_DIM {
            return Err(DiffusionError::model("HY-Motion output width mismatch"));
        }
        gpu_download(&output).map_err(DiffusionError::model)
    }
}

fn tensor_matrix_shape(spec: &HyMotionTensorSpec) -> Result<(usize, usize)> {
    match spec.shape.as_slice() {
        [rows, cols] => Ok((*rows, *cols)),
        shape => Ok((1, shape.iter().product())),
    }
}

fn split_qkv(qkv: &GpuTensor) -> Result<(GpuTensor, GpuTensor, GpuTensor)> {
    if qkv.cols() != 3 * HY_MOTION_HIDDEN {
        return Err(DiffusionError::model("HY-Motion QKV width mismatch"));
    }
    Ok((
        gpu_slice_cols(qkv, 0, HY_MOTION_HIDDEN).map_err(DiffusionError::model)?,
        gpu_slice_cols(qkv, HY_MOTION_HIDDEN, HY_MOTION_HIDDEN)
            .map_err(DiffusionError::model)?,
        gpu_slice_cols(qkv, 2 * HY_MOTION_HIDDEN, HY_MOTION_HIDDEN)
            .map_err(DiffusionError::model)?,
    ))
}

fn main_mlp(
    weights: &HyMotionDeviceWeights,
    input: &GpuTensor,
    prefix: &str,
) -> Result<GpuTensor> {
    let hidden = weights.linear(input, &format!("{prefix}.fc1"))?;
    let hidden = gpu_gelu(&hidden).map_err(DiffusionError::model)?;
    weights.linear(&hidden, &format!("{prefix}.fc2"))
}

pub fn mean_rows(values: &[f32], rows: usize, cols: usize) -> Result<Vec<f32>> {
    if rows == 0 || values.len() != rows * cols {
        return Err(DiffusionError::model("HY-Motion mean_rows shape mismatch"));
    }
    let mut mean = vec![0.0f32; cols];
    for row in values.chunks_exact(cols) {
        for (output, &value) in mean.iter_mut().zip(row) {
            *output += value;
        }
    }
    let inverse = 1.0 / rows as f32;
    for value in &mut mean {
        *value *= inverse;
    }
    Ok(mean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_mean_is_columnwise() {
        let mean = mean_rows(&[1.0, 3.0, 5.0, 7.0, 9.0, 11.0], 3, 2).unwrap();
        assert_eq!(mean, [5.0, 7.0]);
    }
}
