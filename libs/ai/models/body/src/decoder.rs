//! Promptable body-pose decoder and its six-step refinement loop.

use crate::backend::{
    gpu_add, gpu_download, gpu_gelu_erf, gpu_layer_norm_mul_add, gpu_linear_f32_resident,
    gpu_slice_rows, gpu_two_way_layer_resident, gpu_upload, GpuTensor, GpuTwoWayAttention,
    GpuTwoWayLayer, GpuTwoWayLinear,
};
use crate::heads::{DecoderHeads, GpuStepHeads, HostLinear};
use crate::weights::BodyWeights;
use crate::{
    DEC_DEPTH, DEC_DIM, DEC_FFN, DEC_HEADS, DEC_INNER, DEC_NORM_EPS, DINO_DIM, NCAM,
    NPOSE, NUM_KEYPOINTS, DiffusionError, Result,
};

/// The legacy one-dummy-prompt row count. Actual runs derive their row count
/// from the prompt list (`144 + N`).
pub const TOKEN_ROWS: usize = 5 + 2 * NUM_KEYPOINTS;
const BASE_TOKEN_ROWS: usize = 4 + 2 * NUM_KEYPOINTS;
#[cfg(test)]
const KEYPOINT_ROW: usize = 5;
#[cfg(test)]
const KEYPOINT3D_ROW: usize = KEYPOINT_ROW + NUM_KEYPOINTS;

#[derive(Clone, Copy)]
pub struct DecoderNames {
    pub decoder: &'static str,
    pub pose_head: &'static str,
    pub camera_head: &'static str,
    pub init_pose: &'static str,
    pub init_camera: &'static str,
    pub init_to_token: &'static str,
    pub prev_to_token: &'static str,
    pub keypoint_embedding: &'static str,
    pub keypoint3d_embedding: &'static str,
    pub keypoint_posemb: &'static str,
    pub keypoint3d_posemb: &'static str,
    pub keypoint_feat: &'static str,
}

impl DecoderNames {
    pub const BODY: Self = Self {
        decoder: "decoder",
        pose_head: "head_pose.proj",
        camera_head: "head_camera.proj",
        init_pose: "init_pose.weight",
        init_camera: "init_camera.weight",
        init_to_token: "init_to_token_mhr",
        prev_to_token: "prev_to_token_mhr",
        keypoint_embedding: "keypoint_embedding.weight",
        keypoint3d_embedding: "keypoint3d_embedding.weight",
        keypoint_posemb: "keypoint_posemb_linear",
        keypoint3d_posemb: "keypoint3d_posemb_linear",
        keypoint_feat: "keypoint_feat_linear",
    };

    pub const HAND: Self = Self {
        decoder: "decoder_hand",
        pose_head: "head_pose_hand.proj",
        camera_head: "head_camera_hand.proj",
        init_pose: "init_pose_hand.weight",
        init_camera: "init_camera_hand.weight",
        init_to_token: "init_to_token_mhr_hand",
        prev_to_token: "prev_to_token_mhr_hand",
        keypoint_embedding: "keypoint_embedding_hand.weight",
        keypoint3d_embedding: "keypoint3d_embedding_hand.weight",
        keypoint_posemb: "keypoint_posemb_linear_hand",
        keypoint3d_posemb: "keypoint3d_posemb_linear_hand",
        keypoint_feat: "keypoint_feat_linear_hand",
    };
}

#[derive(Clone)]
struct NormWeights {
    weight: Vec<f32>,
    bias: Vec<f32>,
}

impl NormWeights {
    fn load(weights: &BodyWeights, name: &str, dim: usize) -> Result<Self> {
        Ok(Self {
            weight: weights.f32_shaped(&format!("{name}.weight"), &[dim])?,
            bias: weights.f32_shaped(&format!("{name}.bias"), &[dim])?,
        })
    }
}

#[derive(Clone)]
struct AttentionWeights {
    q: HostLinear,
    k: HostLinear,
    v: HostLinear,
    out: HostLinear,
}

impl AttentionWeights {
    fn load(
        weights: &BodyWeights,
        name: &str,
        query_dim: usize,
        key_value_dim: usize,
        output_name: &str,
    ) -> Result<Self> {
        Ok(Self {
            q: HostLinear::load(weights, &format!("{name}.q_proj"), DEC_INNER, query_dim)?,
            k: HostLinear::load(
                weights,
                &format!("{name}.k_proj"),
                DEC_INNER,
                key_value_dim,
            )?,
            v: HostLinear::load(
                weights,
                &format!("{name}.v_proj"),
                DEC_INNER,
                key_value_dim,
            )?,
            out: HostLinear::load(weights, &format!("{name}.{output_name}"), DEC_DIM, DEC_INNER)?,
        })
    }
}

#[derive(Clone)]
struct DecoderLayerWeights {
    ln_pe_1: NormWeights,
    ln_pe_2: NormWeights,
    ln1: NormWeights,
    self_attn: AttentionWeights,
    ln2_1: NormWeights,
    ln2_2: NormWeights,
    cross_attn: AttentionWeights,
    ln3: NormWeights,
    ffn_first: HostLinear,
    ffn_second: HostLinear,
}

#[derive(Clone)]
struct TokenWeights {
    init_to_token: HostLinear,
    prev_to_token: HostLinear,
    prompt_to_token: HostLinear,
    invalid_point_embed: Vec<f32>,
    point_pe: Vec<f32>,
    point_embeddings: Vec<f32>,
    hand_box_embedding: Vec<f32>,
    keypoint_embedding: Vec<f32>,
    keypoint3d_embedding: Vec<f32>,
}

/// Host representation of every f32 tensor owned by the decoder lane.
/// Loading this type does not require a GPU and is used by fixture tests for
/// token assembly and head parity.
pub struct DecoderWeights {
    layers: Vec<DecoderLayerWeights>,
    norm_final: NormWeights,
    heads: DecoderHeads,
    init_pose: Vec<f32>,
    init_camera: Vec<f32>,
    tokens: TokenWeights,
}

impl DecoderWeights {
    pub fn load(weights: &BodyWeights) -> Result<Self> {
        Self::load_named(weights, DecoderNames::BODY)
    }

    pub fn load_named(weights: &BodyWeights, names: DecoderNames) -> Result<Self> {
        let mut layers = Vec::with_capacity(DEC_DEPTH);
        for index in 0..DEC_DEPTH {
            let prefix = format!("{}.layers.{index}", names.decoder);
            layers.push(DecoderLayerWeights {
                ln_pe_1: NormWeights::load(weights, &format!("{prefix}.ln_pe_1"), DEC_DIM)?,
                ln_pe_2: NormWeights::load(weights, &format!("{prefix}.ln_pe_2"), DINO_DIM)?,
                ln1: NormWeights::load(weights, &format!("{prefix}.ln1"), DEC_DIM)?,
                self_attn: AttentionWeights::load(
                    weights,
                    &format!("{prefix}.self_attn"),
                    DEC_DIM,
                    DEC_DIM,
                    "proj",
                )?,
                ln2_1: NormWeights::load(weights, &format!("{prefix}.ln2_1"), DEC_DIM)?,
                ln2_2: NormWeights::load(weights, &format!("{prefix}.ln2_2"), DINO_DIM)?,
                cross_attn: AttentionWeights::load(
                    weights,
                    &format!("{prefix}.cross_attn"),
                    DEC_DIM,
                    DINO_DIM,
                    "proj",
                )?,
                ln3: NormWeights::load(weights, &format!("{prefix}.ln3"), DEC_DIM)?,
                ffn_first: HostLinear::load(
                    weights,
                    &format!("{prefix}.ffn.layers.0.0"),
                    DEC_FFN,
                    DEC_DIM,
                )?,
                ffn_second: HostLinear::load(
                    weights,
                    &format!("{prefix}.ffn.layers.1"),
                    DEC_DIM,
                    DEC_FFN,
                )?,
            });
        }
        Ok(Self {
            layers,
            norm_final: NormWeights::load(
                weights,
                &format!("{}.norm_final", names.decoder),
                DEC_DIM,
            )?,
            heads: DecoderHeads::load_named(
                weights,
                names.pose_head,
                names.camera_head,
                names.keypoint_posemb,
                names.keypoint3d_posemb,
                names.keypoint_feat,
            )?,
            init_pose: weights.f32_shaped(names.init_pose, &[1, NPOSE])?,
            init_camera: weights.f32_shaped(names.init_camera, &[1, NCAM])?,
            tokens: TokenWeights {
                init_to_token: HostLinear::load(
                    weights,
                    names.init_to_token,
                    DEC_DIM,
                    NPOSE + NCAM + 3,
                )?,
                prev_to_token: HostLinear::load(
                    weights,
                    names.prev_to_token,
                    DEC_DIM,
                    NPOSE + NCAM,
                )?,
                prompt_to_token: HostLinear::load(
                    weights,
                    "prompt_to_token",
                    DEC_DIM,
                    DINO_DIM,
                )?,
                invalid_point_embed: weights.f32_shaped(
                    "prompt_encoder.invalid_point_embed.weight",
                    &[1, DINO_DIM],
                )?,
                point_pe: weights.f32_shaped(
                    "prompt_encoder.pe_layer.positional_encoding_gaussian_matrix",
                    &[2, DINO_DIM / 2],
                )?,
                point_embeddings: (0..NUM_KEYPOINTS)
                    .map(|label| {
                        weights.f32_shaped(
                            &format!("prompt_encoder.point_embeddings.{label}.weight"),
                            &[1, DINO_DIM],
                        )
                    })
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect(),
                hand_box_embedding: weights
                    .f32_shaped("hand_box_embedding.weight", &[2, DEC_DIM])?,
                keypoint_embedding: weights
                    .f32_shaped(names.keypoint_embedding, &[NUM_KEYPOINTS, DEC_DIM])?,
                keypoint3d_embedding: weights
                    .f32_shaped(names.keypoint3d_embedding, &[NUM_KEYPOINTS, DEC_DIM])?,
            },
        })
    }

    pub fn build_tokens(&self, condition_info: [f32; 3]) -> TokenSet {
        build_tokens(&self.tokens, &self.init_pose, &self.init_camera, condition_info)
    }

    pub fn pose_head_raw(&self, input: &[f32]) -> Vec<f32> {
        self.heads.pose(input)
    }

    pub fn camera_head_raw(&self, input: &[f32]) -> Vec<f32> {
        self.heads.camera(input)
    }

    pub fn keypoint_posemb(&self, input: &[f32]) -> Vec<f32> {
        self.heads.keypoint_posemb(input)
    }

    pub fn keypoint3d_posemb(&self, input: &[f32]) -> Vec<f32> {
        self.heads.keypoint3d_posemb(input)
    }

    pub fn keypoint_features(&self, input: &[f32]) -> Vec<f32> {
        self.heads.keypoint_features(input)
    }
}

struct GpuLinear {
    weight: GpuTensor,
    bias: GpuTensor,
}

impl GpuLinear {
    fn upload(linear: HostLinear) -> Result<Self> {
        let (weight, bias, output, input) = linear.into_parts();
        Ok(Self {
            weight: gpu_upload(&weight, output, input).map_err(DiffusionError::model)?,
            bias: gpu_upload(&bias, 1, output).map_err(DiffusionError::model)?,
        })
    }

    fn forward(&self, input: &GpuTensor) -> Result<GpuTensor> {
        gpu_linear_f32_resident(input, &self.weight, Some(&self.bias))
            .map_err(DiffusionError::model)
    }
}

struct GpuAttention {
    q: GpuLinear,
    k: GpuLinear,
    v: GpuLinear,
    out: GpuLinear,
}

impl GpuAttention {
    fn upload(weights: AttentionWeights) -> Result<Self> {
        Ok(Self {
            q: GpuLinear::upload(weights.q)?,
            k: GpuLinear::upload(weights.k)?,
            v: GpuLinear::upload(weights.v)?,
            out: GpuLinear::upload(weights.out)?,
        })
    }

    fn forward(&self, query: &GpuTensor, key: &GpuTensor, value: &GpuTensor) -> Result<GpuTensor> {
        let query = self.q.forward(query)?;
        let key = self.k.forward(key)?;
        let value = self.v.forward(value)?;
        let attended = crate::dino::attention_d64(&query, &key, &value, DEC_HEADS)?;
        self.out.forward(&attended)
    }
}

struct DecoderLayer {
    ln_pe_1: NormWeights,
    ln_pe_2: NormWeights,
    ln1: NormWeights,
    self_attn: GpuAttention,
    ln2_1: NormWeights,
    ln2_2: NormWeights,
    cross_attn: GpuAttention,
    ln3: NormWeights,
    ffn_first: GpuLinear,
    ffn_second: GpuLinear,
}

impl DecoderLayer {
    fn upload(weights: DecoderLayerWeights) -> Result<Self> {
        Ok(Self {
            ln_pe_1: weights.ln_pe_1,
            ln_pe_2: weights.ln_pe_2,
            ln1: weights.ln1,
            self_attn: GpuAttention::upload(weights.self_attn)?,
            ln2_1: weights.ln2_1,
            ln2_2: weights.ln2_2,
            cross_attn: GpuAttention::upload(weights.cross_attn)?,
            ln3: weights.ln3,
            ffn_first: GpuLinear::upload(weights.ffn_first)?,
            ffn_second: GpuLinear::upload(weights.ffn_second)?,
        })
    }
}

pub struct Decoder {
    layers: Vec<DecoderLayer>,
    norm_final: NormWeights,
    heads: DecoderHeads,
    step_heads: GpuStepHeads,
    init_pose: Vec<f32>,
    init_camera: Vec<f32>,
    tokens: TokenWeights,
    /// Set once the backend declined the whole-layer resident call; the
    /// per-op path is used from then on.
    resident_refused: std::cell::Cell<bool>,
}

impl GpuLinear {
    fn two_way_ref(&self) -> GpuTwoWayLinear<'_> {
        GpuTwoWayLinear {
            weight: &self.weight,
            bias: Some(&self.bias),
        }
    }
}

impl GpuAttention {
    fn two_way_ref(&self) -> GpuTwoWayAttention<'_> {
        GpuTwoWayAttention {
            q: self.q.two_way_ref(),
            k: self.k.two_way_ref(),
            v: self.v.two_way_ref(),
            out: self.out.two_way_ref(),
        }
    }
}

impl DecoderLayer {
    fn two_way_ref<'a>(&'a self, norm_final: &'a NormWeights, pe_on_self: bool) -> GpuTwoWayLayer<'a> {
        let affine = |n: &'a NormWeights| (n.weight.as_slice(), n.bias.as_slice());
        GpuTwoWayLayer {
            ln_pe_1: affine(&self.ln_pe_1),
            ln_pe_2: affine(&self.ln_pe_2),
            ln1: affine(&self.ln1),
            ln2_1: affine(&self.ln2_1),
            ln2_2: affine(&self.ln2_2),
            ln3: affine(&self.ln3),
            ln_final: affine(norm_final),
            self_attn: self.self_attn.two_way_ref(),
            cross_attn: self.cross_attn.two_way_ref(),
            ffn_first: self.ffn_first.two_way_ref(),
            ffn_second: self.ffn_second.two_way_ref(),
            n_head: DEC_HEADS,
            eps: DEC_NORM_EPS,
            pe_on_self,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TokenSet {
    pub tokens: Vec<f32>,
    pub token_augment: Vec<f32>,
}

impl TokenSet {
    pub fn rows(&self) -> usize {
        self.tokens.len() / DEC_DIM
    }

    fn prompt_count(&self) -> usize {
        self.rows() - BASE_TOKEN_ROWS
    }

    fn hand_box_row(&self) -> usize {
        2 + self.prompt_count()
    }

    fn keypoint_row(&self) -> usize {
        4 + self.prompt_count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointPrompt {
    /// Point coordinates in the body crop, normalised to `[0, 1]`.
    pub point: [f32; 2],
    /// One of the 70 keypoint labels.
    pub label: usize,
}

#[derive(Clone, Debug)]
pub struct StepInput {
    pub layer: usize,
    pub pose_pred_519: Vec<f32>,
    pub cam_pred_3: Vec<f32>,
    pub tokens_normed_row0: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct StepFeedback {
    pub kp2d_cropped: Vec<f32>,
    pub depth: Vec<f32>,
    pub kp3d: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct DecoderOutput {
    pub tokens_normed: Vec<f32>,
    pub hand_boxes: [[f32; 4]; 2],
    pub hand_logits: [[f32; 2]; 2],
    pub last_pose_pred: Vec<f32>,
    pub last_cam_pred: Vec<f32>,
    pub timing: LoopTiming,
}

/// Where the loop's time went, milliseconds over all six steps: the GPU
/// layer chain, the pose-row fetch + heads, the caller's step (rig, camera,
/// projection), and the refinement update (FFNs, sampling, uploads).
#[derive(Clone, Copy, Debug, Default)]
pub struct LoopTiming {
    pub layers_ms: f32,
    pub heads_ms: f32,
    pub step_ms: f32,
    pub refine_ms: f32,
}

impl Decoder {
    pub fn load(weights: &BodyWeights) -> Result<Self> {
        Self::from_weights(DecoderWeights::load(weights)?)
    }

    pub fn load_named(weights: &BodyWeights, names: DecoderNames) -> Result<Self> {
        Self::from_weights(DecoderWeights::load_named(weights, names)?)
    }

    fn from_weights(weights: DecoderWeights) -> Result<Self> {
        let mut layers = Vec::with_capacity(DEC_DEPTH);
        for layer in weights.layers {
            layers.push(DecoderLayer::upload(layer)?);
        }
        let step_heads = GpuStepHeads::upload(&weights.heads)?;
        Ok(Self {
            layers,
            norm_final: weights.norm_final,
            heads: weights.heads,
            step_heads,
            init_pose: weights.init_pose,
            init_camera: weights.init_camera,
            tokens: weights.tokens,
            resident_refused: std::cell::Cell::new(false),
        })
    }

    pub fn build_tokens(&self, condition_info: [f32; 3]) -> TokenSet {
        build_tokens(&self.tokens, &self.init_pose, &self.init_camera, condition_info)
    }

    pub fn run_with_prompts(
        &self,
        condition_info: [f32; 3],
        prompts: &[PointPrompt],
        prev_estimate: &[f32],
        context: &GpuTensor,
        context_pe: &[f32],
        step: impl FnMut(StepInput) -> StepFeedback,
    ) -> Result<DecoderOutput> {
        if prev_estimate.len() != NPOSE + NCAM {
            return Err(DiffusionError::workflow(format!(
                "decoder previous estimate has {} values, expected {}",
                prev_estimate.len(),
                NPOSE + NCAM,
            )));
        }
        let tokens = build_prompted_tokens(
            &self.tokens,
            &self.init_pose,
            &self.init_camera,
            condition_info,
            prompts,
            prev_estimate,
        )?;
        self.run(tokens, context, context_pe, step)
    }

    pub fn run(
        &self,
        tokens: TokenSet,
        context: &GpuTensor,
        context_pe: &[f32],
        step: impl FnMut(StepInput) -> StepFeedback,
    ) -> Result<DecoderOutput> {
        self.run_impl(tokens, context, context_pe, step, None)
    }

    #[cfg(test)]
    pub(crate) fn run_traced(
        &self,
        tokens: TokenSet,
        context: &GpuTensor,
        context_pe: &[f32],
        step: impl FnMut(StepInput) -> StepFeedback,
        trace: &mut dyn FnMut(usize, &GpuTensor, &[f32]) -> Result<()>,
    ) -> Result<DecoderOutput> {
        self.run_impl(tokens, context, context_pe, step, Some(trace))
    }

    #[cfg(test)]
    pub(crate) fn init_pose(&self) -> &[f32] {
        &self.init_pose
    }

    fn run_impl<F>(
        &self,
        mut tokens: TokenSet,
        context: &GpuTensor,
        context_pe: &[f32],
        mut step: F,
        mut trace: Option<&mut dyn FnMut(usize, &GpuTensor, &[f32]) -> Result<()>>,
    ) -> Result<DecoderOutput>
    where
        F: FnMut(StepInput) -> StepFeedback,
    {
        validate_run_inputs(&tokens, context, context_pe)?;
        let context_rows = context.rows();
        let grid_side = (context_rows as f64).sqrt().round() as usize;
        let context_host = gpu_download(context).map_err(DiffusionError::model)?;
        let context_pe = gpu_upload(context_pe, context_rows, DINO_DIM)
            .map_err(DiffusionError::model)?;
        let token_rows = tokens.rows();
        let keypoint_row = tokens.keypoint_row();
        let hand_box_row = tokens.hand_box_row();
        let mut hidden = gpu_upload(&tokens.tokens, token_rows, DEC_DIM)
            .map_err(DiffusionError::model)?;
        let mut final_normed = Vec::new();
        let mut last_pose = Vec::new();
        let mut last_camera = Vec::new();
        let mut timing = LoopTiming::default();

        for (layer_index, layer) in self.layers.iter().enumerate() {
            let t0 = std::time::Instant::now();
            let token_pe = gpu_upload(&tokens.token_augment, token_rows, DEC_DIM)
                .map_err(DiffusionError::model)?;
            // The whole layer in one backend call when the backend offers it
            // (Metal: one command buffer, no host round trips between ops).
            let mut resident = None;
            if !self.resident_refused.get() {
                let layer_ref = layer.two_way_ref(&self.norm_final, layer_index != 0);
                match gpu_two_way_layer_resident(&hidden, &token_pe, context, &context_pe, &layer_ref)
                {
                    Ok(pair) => resident = Some(pair),
                    Err(_) => self.resident_refused.set(true),
                }
            }
            let normed = if let Some((new_hidden, normed)) = resident {
                hidden = new_hidden;
                normed
            } else {
                let token_pe = layer_norm_gpu(&token_pe, &layer.ln_pe_1)?;
                let image_pe = layer_norm_gpu(&context_pe, &layer.ln_pe_2)?;

                let normed = layer_norm_gpu(&hidden, &layer.ln1)?;
                let self_update = if layer_index == 0 {
                    layer.self_attn.forward(&normed, &normed, &normed)?
                } else {
                    let qk = gpu_add(&normed, &token_pe).map_err(DiffusionError::model)?;
                    layer.self_attn.forward(&qk, &qk, &normed)?
                };
                hidden = gpu_add(&hidden, &self_update).map_err(DiffusionError::model)?;

                let query = layer_norm_gpu(&hidden, &layer.ln2_1)?;
                let query = gpu_add(&query, &token_pe).map_err(DiffusionError::model)?;
                let context_normed = layer_norm_gpu(context, &layer.ln2_2)?;
                let key = gpu_add(&context_normed, &image_pe).map_err(DiffusionError::model)?;
                let cross_update = layer
                    .cross_attn
                    .forward(&query, &key, &context_normed)?;
                hidden = gpu_add(&hidden, &cross_update).map_err(DiffusionError::model)?;

                let normed = layer_norm_gpu(&hidden, &layer.ln3)?;
                let ffn = layer.ffn_first.forward(&normed)?;
                let ffn = gpu_gelu_erf(&ffn).map_err(DiffusionError::model)?;
                let ffn = layer.ffn_second.forward(&ffn)?;
                hidden = gpu_add(&hidden, &ffn).map_err(DiffusionError::model)?;

                layer_norm_gpu(&hidden, &self.norm_final)?
            };
            timing.layers_ms += t0.elapsed().as_secs_f32() * 1000.0;
            let t1 = std::time::Instant::now();
            // Only the pose token leaves the GPU mid-loop; the whole block
            // is downloaded once at the end (the hand-box rows and the
            // output) or when a trace wants every layer.
            let last_layer = layer_index + 1 == DEC_DEPTH;
            let pose_row = if last_layer || trace.is_some() {
                final_normed = gpu_download(&normed).map_err(DiffusionError::model)?;
                final_normed[..DEC_DIM].to_vec()
            } else {
                let row = gpu_slice_rows(&normed, 0, 1).map_err(DiffusionError::model)?;
                gpu_download(&row).map_err(DiffusionError::model)?
            };
            if let Some(callback) = &mut trace {
                (**callback)(layer_index, &hidden, &final_normed)?;
            }
            let pose_token = &pose_row[..DEC_DIM];
            last_pose = self.step_heads.pose(pose_token)?;
            add_in_place(&mut last_pose, &self.init_pose);
            last_camera = self.step_heads.camera(pose_token)?;
            add_in_place(&mut last_camera, &self.init_camera);
            timing.heads_ms += t1.elapsed().as_secs_f32() * 1000.0;
            let t2 = std::time::Instant::now();
            let feedback = step(StepInput {
                layer: layer_index,
                pose_pred_519: last_pose.clone(),
                cam_pred_3: last_camera.clone(),
                tokens_normed_row0: pose_token.to_vec(),
            });
            timing.step_ms += t2.elapsed().as_secs_f32() * 1000.0;

            if layer_index + 1 < DEC_DEPTH {
                let t3 = std::time::Instant::now();
                let delta = self.refinement_update(
                    &mut tokens.token_augment,
                    &context_host,
                    grid_side,
                    token_rows,
                    keypoint_row,
                    feedback,
                )?;
                let delta = gpu_upload(&delta, token_rows, DEC_DIM)
                    .map_err(DiffusionError::model)?;
                hidden = gpu_add(&hidden, &delta).map_err(DiffusionError::model)?;
                timing.refine_ms += t3.elapsed().as_secs_f32() * 1000.0;
            }
        }

        let mut hand_boxes = [[0.0; 4]; 2];
        let mut hand_logits = [[0.0; 2]; 2];
        for hand in 0..2 {
            let row = &final_normed
                [(hand_box_row + hand) * DEC_DIM..(hand_box_row + hand + 1) * DEC_DIM];
            hand_boxes[hand] = self.heads.bbox(row);
            hand_logits[hand] = self.heads.hand_logits(row);
        }
        Ok(DecoderOutput {
            tokens_normed: final_normed,
            hand_boxes,
            hand_logits,
            last_pose_pred: last_pose,
            last_cam_pred: last_camera,
            timing,
        })
    }

    fn refinement_update(
        &self,
        token_augment: &mut [f32],
        context: &[f32],
        grid_side: usize,
        token_rows: usize,
        keypoint_row: usize,
        feedback: StepFeedback,
    ) -> Result<Vec<f32>> {
        if feedback.kp2d_cropped.len() != NUM_KEYPOINTS * 2
            || feedback.depth.len() != NUM_KEYPOINTS
            || feedback.kp3d.len() != NUM_KEYPOINTS * 3
        {
            return Err(DiffusionError::workflow(format!(
                "decoder feedback shapes are kp2d={} depth={} kp3d={}, expected {}, {}, {}",
                feedback.kp2d_cropped.len(),
                feedback.depth.len(),
                feedback.kp3d.len(),
                NUM_KEYPOINTS * 2,
                NUM_KEYPOINTS,
                NUM_KEYPOINTS * 3,
            )));
        }

        let valid: Vec<bool> = feedback
            .kp2d_cropped
            .chunks_exact(2)
            .zip(&feedback.depth)
            .map(|(point, &depth)| {
                (0.0..=1.0).contains(&(point[0] + 0.5))
                    && (0.0..=1.0).contains(&(point[1] + 0.5))
                    && depth >= 1e-5
            })
            .collect();
        let posemb = self.step_heads.keypoint_posemb(&feedback.kp2d_cropped)?;
        let mut sampled = vec![0.0f32; NUM_KEYPOINTS * DINO_DIM];
        for (index, point) in feedback.kp2d_cropped.chunks_exact(2).enumerate() {
            if valid[index] {
                let value = bilinear_sample(
                    context,
                    grid_side,
                    grid_side,
                    DINO_DIM,
                    2.0 * point[0],
                    2.0 * point[1],
                );
                sampled[index * DINO_DIM..(index + 1) * DINO_DIM]
                    .copy_from_slice(&value);
            }
        }
        let features = self.step_heads.keypoint_features(&sampled)?;

        let hip9 = &feedback.kp3d[9 * 3..10 * 3];
        let hip10 = &feedback.kp3d[10 * 3..11 * 3];
        let pelvis = [
            0.5 * (hip9[0] + hip10[0]),
            0.5 * (hip9[1] + hip10[1]),
            0.5 * (hip9[2] + hip10[2]),
        ];
        let mut centered = feedback.kp3d;
        for point in centered.chunks_exact_mut(3) {
            for axis in 0..3 {
                point[axis] -= pelvis[axis];
            }
        }
        let posemb3d = self.step_heads.keypoint3d_posemb(&centered)?;

        let keypoint3d_row = keypoint_row + NUM_KEYPOINTS;
        let mut delta = vec![0.0f32; token_rows * DEC_DIM];
        for index in 0..NUM_KEYPOINTS {
            let row2d = (keypoint_row + index) * DEC_DIM;
            if valid[index] {
                token_augment[row2d..row2d + DEC_DIM]
                    .copy_from_slice(&posemb[index * DEC_DIM..(index + 1) * DEC_DIM]);
                delta[row2d..row2d + DEC_DIM]
                    .copy_from_slice(&features[index * DEC_DIM..(index + 1) * DEC_DIM]);
            } else {
                token_augment[row2d..row2d + DEC_DIM].fill(0.0);
            }
            let row3d = (keypoint3d_row + index) * DEC_DIM;
            token_augment[row3d..row3d + DEC_DIM]
                .copy_from_slice(&posemb3d[index * DEC_DIM..(index + 1) * DEC_DIM]);
        }
        Ok(delta)
    }
}

fn build_tokens(
    weights: &TokenWeights,
    init_pose: &[f32],
    init_camera: &[f32],
    condition_info: [f32; 3],
) -> TokenSet {
    let previous_input: Vec<f32> = init_pose.iter().chain(init_camera).copied().collect();
    build_token_rows(
        weights,
        init_pose,
        init_camera,
        condition_info,
        vec![weights
            .prompt_to_token
            .forward_row(&weights.invalid_point_embed)],
        &previous_input,
    )
}

fn build_prompted_tokens(
    weights: &TokenWeights,
    init_pose: &[f32],
    init_camera: &[f32],
    condition_info: [f32; 3],
    prompts: &[PointPrompt],
    previous_input: &[f32],
) -> Result<TokenSet> {
    let mut prompt_tokens = Vec::with_capacity(prompts.len());
    for prompt in prompts {
        if prompt.label >= NUM_KEYPOINTS {
            return Err(DiffusionError::workflow(format!(
                "decoder prompt label {} is outside 0..{}",
                prompt.label, NUM_KEYPOINTS,
            )));
        }
        let mut embedding = vec![0.0f32; DINO_DIM];
        let x = 2.0 * prompt.point[0] - 1.0;
        let y = 2.0 * prompt.point[1] - 1.0;
        for k in 0..DINO_DIM / 2 {
            let angle = 2.0
                * std::f32::consts::PI
                * (x * weights.point_pe[k] + y * weights.point_pe[DINO_DIM / 2 + k]);
            embedding[k] = angle.sin();
            embedding[DINO_DIM / 2 + k] = angle.cos();
        }
        let label = &weights.point_embeddings
            [prompt.label * DINO_DIM..(prompt.label + 1) * DINO_DIM];
        for (value, label) in embedding.iter_mut().zip(label) {
            *value += label;
        }
        prompt_tokens.push(weights.prompt_to_token.forward_row(&embedding));
    }
    Ok(build_token_rows(
        weights,
        init_pose,
        init_camera,
        condition_info,
        prompt_tokens,
        previous_input,
    ))
}

fn build_token_rows(
    weights: &TokenWeights,
    init_pose: &[f32],
    init_camera: &[f32],
    condition_info: [f32; 3],
    prompt_tokens: Vec<Vec<f32>>,
    previous_input: &[f32],
) -> TokenSet {
    let mut init_input = Vec::with_capacity(3 + NPOSE + NCAM);
    init_input.extend_from_slice(&condition_info);
    init_input.extend_from_slice(init_pose);
    init_input.extend_from_slice(init_camera);
    let pose_token = weights.init_to_token.forward_row(&init_input);

    let previous_token = weights.prev_to_token.forward_row(previous_input);
    let rows = BASE_TOKEN_ROWS + prompt_tokens.len();

    let mut tokens = Vec::with_capacity(rows * DEC_DIM);
    tokens.extend_from_slice(&pose_token);
    tokens.extend_from_slice(&previous_token);
    for prompt in &prompt_tokens {
        tokens.extend_from_slice(prompt);
    }
    tokens.extend_from_slice(&weights.hand_box_embedding);
    tokens.extend_from_slice(&weights.keypoint_embedding);
    tokens.extend_from_slice(&weights.keypoint3d_embedding);
    debug_assert_eq!(tokens.len(), rows * DEC_DIM);

    let mut token_augment = vec![0.0f32; rows * DEC_DIM];
    token_augment[DEC_DIM..2 * DEC_DIM].copy_from_slice(&previous_token);
    for (index, prompt) in prompt_tokens.iter().enumerate() {
        let row = 2 + index;
        token_augment[row * DEC_DIM..(row + 1) * DEC_DIM].copy_from_slice(prompt);
    }
    TokenSet {
        tokens,
        token_augment,
    }
}

fn validate_run_inputs(tokens: &TokenSet, context: &GpuTensor, context_pe: &[f32]) -> Result<()> {
    let rows = tokens.rows();
    if rows < BASE_TOKEN_ROWS
        || tokens.tokens.len() != rows * DEC_DIM
        || tokens.token_augment.len() != rows * DEC_DIM
    {
        return Err(DiffusionError::workflow(format!(
            "decoder token shapes are {} and {}, expected matching (144 + N) x {}",
            tokens.tokens.len(),
            tokens.token_augment.len(),
            DEC_DIM,
        )));
    }
    let rows = context.rows();
    let side = (rows as f64).sqrt().round() as usize;
    if context.cols() != DINO_DIM || side * side != rows {
        return Err(DiffusionError::workflow(format!(
            "decoder context is {}x{}, expected a square patch grid x {}",
            context.rows(),
            context.cols(),
            DINO_DIM,
        )));
    }
    if context_pe.len() != rows * DINO_DIM {
        return Err(DiffusionError::workflow(format!(
            "decoder context PE has {} values, expected {}",
            context_pe.len(),
            rows * DINO_DIM,
        )));
    }
    Ok(())
}

fn layer_norm_gpu(input: &GpuTensor, norm: &NormWeights) -> Result<GpuTensor> {
    gpu_layer_norm_mul_add(input, &norm.weight, &norm.bias, DEC_NORM_EPS)
        .map_err(DiffusionError::model)
}

fn add_in_place(values: &mut [f32], offsets: &[f32]) {
    debug_assert_eq!(values.len(), offsets.len());
    for (value, offset) in values.iter_mut().zip(offsets) {
        *value += offset;
    }
}

#[cfg(test)]
fn self_attention_qk_input(layer: usize, normed: &[f32], token_pe: &[f32]) -> Vec<f32> {
    debug_assert_eq!(normed.len(), token_pe.len());
    if layer == 0 {
        normed.to_vec()
    } else {
        normed.iter().zip(token_pe).map(|(x, pe)| x + pe).collect()
    }
}

#[cfg(test)]
fn layer_norm_host(
    input: &[f32],
    rows: usize,
    cols: usize,
    weight: &[f32],
    bias: &[f32],
    eps: f32,
) -> Vec<f32> {
    debug_assert_eq!(input.len(), rows * cols);
    debug_assert_eq!(weight.len(), cols);
    debug_assert_eq!(bias.len(), cols);
    let mut output = vec![0.0f32; input.len()];
    for (source, target) in input
        .chunks_exact(cols)
        .zip(output.chunks_exact_mut(cols))
    {
        let mean = source.iter().sum::<f32>() / cols as f32;
        let variance = source
            .iter()
            .map(|value| {
                let centered = value - mean;
                centered * centered
            })
            .sum::<f32>()
            / cols as f32;
        let inv_std = 1.0 / (variance + eps).sqrt();
        for column in 0..cols {
            target[column] = (source[column] - mean) * inv_std * weight[column] + bias[column];
        }
    }
    output
}

/// Align-corners-false bilinear sampling of an interleaved HWC grid.
fn bilinear_sample(
    grid: &[f32],
    width: usize,
    height: usize,
    channels: usize,
    normalized_x: f32,
    normalized_y: f32,
) -> Vec<f32> {
    debug_assert_eq!(grid.len(), width * height * channels);
    let x = (normalized_x + 1.0) * 0.5 * width as f32 - 0.5;
    let y = (normalized_y + 1.0) * 0.5 * height as f32 - 0.5;
    let x0 = x.floor() as isize;
    let y0 = y.floor() as isize;
    let dx = x - x0 as f32;
    let dy = y - y0 as f32;
    let neighbors = [
        (x0, y0, (1.0 - dx) * (1.0 - dy)),
        (x0 + 1, y0, dx * (1.0 - dy)),
        (x0, y0 + 1, (1.0 - dx) * dy),
        (x0 + 1, y0 + 1, dx * dy),
    ];
    let mut output = vec![0.0f32; channels];
    for (nx, ny, weight) in neighbors {
        if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
            continue;
        }
        let offset = (ny as usize * width + nx as usize) * channels;
        for channel in 0..channels {
            output[channel] += weight * grid[offset + channel];
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::gpu_device_available;
    use crate::fixture;
    use crate::NUM_PATCHES;

    fn identity(input: &[f32]) -> Vec<f32> {
        input.to_vec()
    }

    #[test]
    fn token_layout_and_augment_rows_match_contract() {
        let pose = vec![1.0; DEC_DIM];
        let previous = vec![2.0; DEC_DIM];
        let prompt = vec![3.0; DEC_DIM];
        let hand: Vec<f32> = (0..2 * DEC_DIM).map(|i| 10.0 + i as f32).collect();
        let keypoint: Vec<f32> = (0..NUM_KEYPOINTS * DEC_DIM)
            .map(|i| 20.0 + i as f32)
            .collect();
        let keypoint3d: Vec<f32> = (0..NUM_KEYPOINTS * DEC_DIM)
            .map(|i| 30.0 + i as f32)
            .collect();
        let weights = TokenWeights {
            init_to_token: HostLinear::constant(NPOSE + NCAM + 3, pose.clone()),
            prev_to_token: HostLinear::constant(NPOSE + NCAM, previous.clone()),
            prompt_to_token: HostLinear::constant(DINO_DIM, prompt.clone()),
            invalid_point_embed: vec![0.0; DINO_DIM],
            point_pe: vec![0.0; DINO_DIM],
            point_embeddings: vec![0.0; NUM_KEYPOINTS * DINO_DIM],
            hand_box_embedding: hand.clone(),
            keypoint_embedding: keypoint.clone(),
            keypoint3d_embedding: keypoint3d.clone(),
        };
        let tokens = build_tokens(
            &weights,
            &vec![0.0; NPOSE],
            &vec![0.0; NCAM],
            [0.0; 3],
        );
        assert_eq!(&tokens.tokens[..DEC_DIM], pose);
        assert_eq!(&tokens.tokens[DEC_DIM..2 * DEC_DIM], previous);
        assert_eq!(&tokens.tokens[2 * DEC_DIM..3 * DEC_DIM], prompt);
        assert_eq!(&tokens.tokens[3 * DEC_DIM..5 * DEC_DIM], hand);
        assert_eq!(
            &tokens.tokens[KEYPOINT_ROW * DEC_DIM..KEYPOINT3D_ROW * DEC_DIM],
            keypoint,
        );
        assert_eq!(&tokens.tokens[KEYPOINT3D_ROW * DEC_DIM..], keypoint3d);
        assert_eq!(
            &tokens.token_augment[DEC_DIM..2 * DEC_DIM],
            previous,
        );
        assert_eq!(&tokens.token_augment[2 * DEC_DIM..3 * DEC_DIM], prompt);
        assert!(tokens.token_augment[..DEC_DIM].iter().all(|&x| x == 0.0));
        assert!(tokens.token_augment[3 * DEC_DIM..]
            .iter()
            .all(|&x| x == 0.0));
    }

    #[test]
    fn bilinear_sampling_matches_centers_and_padding() {
        let grid = vec![1.0, 10.0, 2.0, 20.0, 3.0, 30.0, 4.0, 40.0];
        for y in 0..2 {
            for x in 0..2 {
                let nx = 2.0 * (x as f32 + 0.5) / 2.0 - 1.0;
                let ny = 2.0 * (y as f32 + 0.5) / 2.0 - 1.0;
                let sampled = bilinear_sample(&grid, 2, 2, 2, nx, ny);
                let offset = (y * 2 + x) * 2;
                assert_eq!(sampled, grid[offset..offset + 2]);
            }
        }
        assert_eq!(bilinear_sample(&grid, 2, 2, 2, 2.0, 2.0), [0.0, 0.0]);
    }

    #[test]
    fn first_layer_skips_token_pe_for_self_attention() {
        let normed = [1.0, 2.0, 3.0, 4.0];
        let pe = [10.0, 20.0, 30.0, 40.0];
        let layer0 = identity(&self_attention_qk_input(0, &normed, &pe));
        let layer1 = identity(&self_attention_qk_input(1, &normed, &pe));
        assert_eq!(layer0, normed);
        assert_eq!(layer1, [11.0, 22.0, 33.0, 44.0]);
    }

    #[test]
    fn host_layer_norm_uses_biased_variance() {
        let output = layer_norm_host(
            &[1.0, 2.0, 3.0, 4.0],
            1,
            4,
            &[1.0, 2.0, 3.0, 4.0],
            &[0.5, 0.5, 0.5, 0.5],
            0.0,
        );
        let inv_std = 1.0 / 1.25f32.sqrt();
        let expected = [
            -1.5 * inv_std + 0.5,
            -0.5 * inv_std * 2.0 + 0.5,
            0.5 * inv_std * 3.0 + 0.5,
            1.5 * inv_std * 4.0 + 0.5,
        ];
        for (actual, expected) in output.iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
        }
    }

    fn planar_to_tokens(values: &[f32]) -> Vec<f32> {
        assert_eq!(values.len(), DINO_DIM * NUM_PATCHES);
        let mut output = vec![0.0f32; NUM_PATCHES * DINO_DIM];
        for channel in 0..DINO_DIM {
            for token in 0..NUM_PATCHES {
                output[token * DINO_DIM + channel] = values[channel * NUM_PATCHES + token];
            }
        }
        output
    }

    fn fixture_values(name: &str) -> Vec<f32> {
        fixture::load(name)
            .unwrap_or_else(|| panic!("missing fixture {name}"))
            .1
    }

    fn fixture_weights() -> Option<BodyWeights> {
        let Some(path) = fixture::weights_path() else {
            eprintln!("skipping body decoder fixture: weights_path.txt is absent");
            return None;
        };
        Some(BodyWeights::load(&path).unwrap_or_else(|error| {
            panic!("failed to load fixture weights {}: {error}", path.display())
        }))
    }

    fn assert_close(name: &str, actual: &[f32], expected: &[f32], tolerance: f32) {
        assert_eq!(
            actual.len(),
            expected.len(),
            "{name} length {} != {}",
            actual.len(),
            expected.len(),
        );
        let (index, max) = actual
            .iter()
            .zip(expected)
            .enumerate()
            .map(|(index, (actual, expected))| (index, (actual - expected).abs()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or((0, 0.0));
        eprintln!("body decoder parity {name}: max abs {max:.6}");
        assert!(
            max <= tolerance,
            "{name} max abs {max} at {index}: actual={} expected={} tolerance={tolerance}",
            actual.get(index).copied().unwrap_or(0.0),
            expected.get(index).copied().unwrap_or(0.0),
        );
    }

    // GPU matmul/attention accumulation order differs from the reference's;
    // the residual stream is O(10) wide, the heads O(1).
    const TOKEN_TOLERANCE: f32 = 5e-2;
    const HEAD_TOLERANCE: f32 = 2e-2;

    #[test]
    fn fixture_token_assembly() {
        if fixture::oracle_dir().is_none() {
            eprintln!("skipping body decoder fixture: oracle directory is absent");
            return;
        }
        let Some(weights) = fixture_weights() else {
            return;
        };
        let weights = DecoderWeights::load(&weights).expect("load decoder weights");
        let condition = fixture_values("condition_info");
        assert_eq!(condition.len(), 3);
        let tokens = weights.build_tokens([condition[0], condition[1], condition[2]]);
        assert_close(
            "decoder_tokens_in",
            &tokens.tokens,
            &fixture_values("decoder_tokens_in"),
            1e-4,
        );
        assert_close(
            "decoder_token_augment_in",
            &tokens.token_augment,
            &fixture_values("decoder_token_augment_in"),
            1e-4,
        );
    }

    #[test]
    fn fixture_reprompt_token_assembly_and_shifted_rows() {
        use crate::fixture::OracleRoot;
        if fixture::oracle_dir_for(OracleRoot::Full).is_none() {
            eprintln!("SKIP fixture_reprompt_token_assembly: oracle_full absent");
            return;
        }
        let Some(weights) = fixture_weights() else {
            return;
        };
        let weights = DecoderWeights::load(&weights).expect("load decoder weights");
        let condition = fixture::load_from(OracleRoot::Full, "condition_info_0")
            .expect("condition_info_0")
            .1;
        let prompt_values = fixture::load_from(OracleRoot::Full, "keypoint_prompt")
            .expect("keypoint_prompt")
            .1;
        let prompts: Vec<PointPrompt> = prompt_values
            .chunks_exact(3)
            .map(|row| PointPrompt {
                point: [row[0], row[1]],
                label: row[2] as usize,
            })
            .collect();
        let previous = fixture::load_from(OracleRoot::Full, "prev_to_token_in_1")
            .expect("prev_to_token_in_1")
            .1;
        let tokens = build_prompted_tokens(
            &weights.tokens,
            &weights.init_pose,
            &weights.init_camera,
            [condition[0], condition[1], condition[2]],
            &prompts,
            &previous,
        )
        .expect("build prompted tokens");
        assert_eq!(tokens.rows(), BASE_TOKEN_ROWS + prompts.len());
        assert_close(
            "decoder_tokens_in_1",
            &tokens.tokens,
            &fixture::load_from(OracleRoot::Full, "decoder_tokens_in_1")
                .unwrap()
                .1,
            1.0e-4,
        );
        assert_close(
            "decoder_token_augment_in_1",
            &tokens.token_augment,
            &fixture::load_from(OracleRoot::Full, "decoder_token_augment_in_1")
                .unwrap()
                .1,
            1.0e-4,
        );
    }

    #[test]
    fn fixture_host_heads_and_refinement_ffns() {
        if fixture::oracle_dir().is_none() {
            eprintln!("skipping body head fixture: oracle directory is absent");
            return;
        }
        let Some(weights) = fixture_weights() else {
            return;
        };
        let heads = DecoderHeads::load(&weights).expect("load decoder heads");
        assert_close(
            "head_pose_proj_out_0",
            &heads.pose(&fixture_values("head_pose_proj_in_0")),
            &fixture_values("head_pose_proj_out_0"),
            1e-4,
        );
        assert_close(
            "head_camera_proj_out_0",
            &heads.camera(&fixture_values("head_camera_proj_in_0")),
            &fixture_values("head_camera_proj_out_0"),
            1e-4,
        );
        assert_close(
            "kp_posemb_out_0",
            &heads.keypoint_posemb(&fixture_values("kp_posemb_in_0")),
            &fixture_values("kp_posemb_out_0"),
            1e-4,
        );
        assert_close(
            "kp3d_posemb_out_0",
            &heads.keypoint3d_posemb(&fixture_values("kp3d_posemb_in_0")),
            &fixture_values("kp3d_posemb_out_0"),
            1e-4,
        );
        let expected_features = fixture_values("kp_feat_out_0");
        let mut actual_features = heads.keypoint_features(&fixture_values("kp_feat_in_0"));
        for (actual, expected) in actual_features
            .chunks_exact_mut(DEC_DIM)
            .zip(expected_features.chunks_exact(DEC_DIM))
        {
            if expected.iter().all(|&value| value == 0.0) {
                actual.fill(0.0);
            }
        }
        assert_close(
            "kp_feat_out_0",
            &actual_features,
            &expected_features,
            1e-4,
        );
    }

    #[test]
    fn fixture_gpu_decoder_layers() {
        if fixture::oracle_dir().is_none() {
            eprintln!("skipping body decoder GPU fixture: oracle directory is absent");
            return;
        }
        if !gpu_device_available() {
            eprintln!("skipping body decoder GPU fixture: GPU device is absent");
            return;
        }
        let Some(weights) = fixture_weights() else {
            return;
        };
        let decoder = Decoder::load(&weights).expect("load GPU decoder");
        // The oracle captured the context and its PE channel-first
        // ([1280, 32, 32]); the decoder takes them as token rows.
        let context_values = planar_to_tokens(&fixture_values("decoder_image_in"));
        let context = gpu_upload(&context_values, NUM_PATCHES, DINO_DIM)
            .expect("upload decoder fixture context");
        let context_pe = planar_to_tokens(&fixture_values("decoder_image_augment_in"));
        let token_set = TokenSet {
            tokens: fixture_values("decoder_tokens_in"),
            token_augment: fixture_values("decoder_token_augment_in"),
        };
        let mut trace = |layer: usize, hidden: &GpuTensor, normed: &[f32]| -> Result<()> {
            let hidden = gpu_download(hidden).map_err(DiffusionError::model)?;
            assert_close(
                &format!("dec{layer}_tokens_out"),
                &hidden,
                &fixture_values(&format!("dec{layer}_tokens_out")),
                TOKEN_TOLERANCE,
            );
            assert_close(
                &format!("norm_final_out_{layer}"),
                normed,
                &fixture_values(&format!("norm_final_out_{layer}")),
                HEAD_TOLERANCE,
            );
            Ok(())
        };
        let step = |input: StepInput| -> StepFeedback {
            let layer = input.layer;
            assert_close(
                &format!("head_pose_proj_in_{layer}"),
                &input.tokens_normed_row0,
                &fixture_values(&format!("head_pose_proj_in_{layer}")),
                HEAD_TOLERANCE,
            );
            let pose_raw: Vec<f32> = input
                .pose_pred_519
                .iter()
                .zip(&decoder.init_pose)
                .map(|(value, offset)| value - offset)
                .collect();
            assert_close(
                &format!("head_pose_proj_out_{layer}"),
                &pose_raw,
                &fixture_values(&format!("head_pose_proj_out_{layer}")),
                HEAD_TOLERANCE,
            );
            let camera_raw: Vec<f32> = input
                .cam_pred_3
                .iter()
                .zip(&decoder.init_camera)
                .map(|(value, offset)| value - offset)
                .collect();
            assert_close(
                &format!("head_camera_proj_out_{layer}"),
                &camera_raw,
                &fixture_values(&format!("head_camera_proj_out_{layer}")),
                HEAD_TOLERANCE,
            );
            if layer + 1 == DEC_DEPTH {
                return StepFeedback::default();
            }
            let kp2d = fixture_values(&format!("kp_posemb_in_{layer}"));
            let kp3d = fixture_values(&format!("kp3d_posemb_in_{layer}"));
            let feature_output = fixture_values(&format!("kp_feat_out_{layer}"));
            let depth = feature_output
                .chunks_exact(DEC_DIM)
                .map(|row| {
                    if row.iter().all(|&value| value == 0.0) {
                        0.0
                    } else {
                        1.0
                    }
                })
                .collect();
            StepFeedback {
                kp2d_cropped: kp2d,
                depth,
                kp3d,
            }
        };
        let output = decoder
            .run_impl(token_set, &context, &context_pe, step, Some(&mut trace))
            .expect("run decoder fixture");
        assert_close(
            "norm_final_out_5",
            &output.tokens_normed,
            &fixture_values("norm_final_out_5"),
            HEAD_TOLERANCE,
        );
    }
}
