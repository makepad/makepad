//! Decoder heads and refinement embeddings: host copies (tests, token
//! assembly) and GPU-resident copies for the per-step work, where the
//! 70-row FFNs are the whole cost of a refinement step on the host.

use crate::backend::{
    gpu_birefnet_relu, gpu_download, gpu_linear_f32_resident, gpu_upload, GpuTensor,
};
use crate::weights::BodyWeights;
use crate::{DiffusionError, DEC_DIM, DINO_DIM, NCAM, NPOSE, Result};

#[derive(Clone)]
pub(crate) struct HostLinear {
    weight: Vec<f32>,
    bias: Vec<f32>,
    input: usize,
    output: usize,
}

impl HostLinear {
    pub(crate) fn load(
        weights: &BodyWeights,
        name: &str,
        output: usize,
        input: usize,
    ) -> Result<Self> {
        Ok(Self {
            weight: weights.f32_shaped(&format!("{name}.weight"), &[output, input])?,
            bias: weights.f32_shaped(&format!("{name}.bias"), &[output])?,
            input,
            output,
        })
    }

    pub(crate) fn forward_row(&self, input: &[f32]) -> Vec<f32> {
        debug_assert_eq!(input.len(), self.input);
        let mut output = self.bias.clone();
        for (row, value) in self.weight.chunks_exact(self.input).zip(&mut output) {
            let mut sum = *value;
            for (&x, &w) in input.iter().zip(row) {
                sum += x * w;
            }
            *value = sum;
        }
        output
    }

    pub(crate) fn forward_rows(&self, input: &[f32]) -> Vec<f32> {
        debug_assert_eq!(input.len() % self.input, 0);
        let rows = input.len() / self.input;
        let mut output = Vec::with_capacity(rows * self.output);
        for row in input.chunks_exact(self.input) {
            output.extend(self.forward_row(row));
        }
        output
    }

    pub(crate) fn into_parts(self) -> (Vec<f32>, Vec<f32>, usize, usize) {
        (self.weight, self.bias, self.output, self.input)
    }

    #[cfg(test)]
    pub(crate) fn constant(input: usize, bias: Vec<f32>) -> Self {
        let output = bias.len();
        Self {
            weight: vec![0.0; input * output],
            bias,
            input,
            output,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ReluFfn {
    first: HostLinear,
    second: HostLinear,
}

impl ReluFfn {
    pub(crate) fn load(
        weights: &BodyWeights,
        prefix: &str,
        input: usize,
        hidden: usize,
        output: usize,
    ) -> Result<Self> {
        Ok(Self {
            first: HostLinear::load(
                weights,
                &format!("{prefix}.layers.0.0"),
                hidden,
                input,
            )?,
            second: HostLinear::load(weights, &format!("{prefix}.layers.1"), output, hidden)?,
        })
    }

    pub(crate) fn forward_row(&self, input: &[f32]) -> Vec<f32> {
        let mut hidden = self.first.forward_row(input);
        relu_in_place(&mut hidden);
        self.second.forward_row(&hidden)
    }

    pub(crate) fn forward_rows(&self, input: &[f32]) -> Vec<f32> {
        debug_assert_eq!(input.len() % self.first.input, 0);
        let rows = input.len() / self.first.input;
        let mut output = Vec::with_capacity(rows * self.second.output);
        for row in input.chunks_exact(self.first.input) {
            output.extend(self.forward_row(row));
        }
        output
    }
}

#[derive(Clone)]
pub(crate) struct BboxMlp {
    first: HostLinear,
    second: HostLinear,
    third: HostLinear,
}

impl BboxMlp {
    fn load(weights: &BodyWeights) -> Result<Self> {
        Ok(Self {
            first: HostLinear::load(weights, "bbox_embed.layers.0", DEC_DIM, DEC_DIM)?,
            second: HostLinear::load(weights, "bbox_embed.layers.1", DEC_DIM, DEC_DIM)?,
            third: HostLinear::load(weights, "bbox_embed.layers.2", 4, DEC_DIM)?,
        })
    }

    fn forward(&self, input: &[f32]) -> [f32; 4] {
        let mut hidden = self.first.forward_row(input);
        relu_in_place(&mut hidden);
        let mut hidden = self.second.forward_row(&hidden);
        relu_in_place(&mut hidden);
        let output = self.third.forward_row(&hidden);
        std::array::from_fn(|i| sigmoid(output[i]))
    }
}

#[derive(Clone)]
pub(crate) struct DecoderHeads {
    pose: ReluFfn,
    camera: ReluFfn,
    keypoint_posemb: ReluFfn,
    keypoint3d_posemb: ReluFfn,
    keypoint_feat: HostLinear,
    bbox: BboxMlp,
    hand_cls: HostLinear,
}

impl DecoderHeads {
    pub(crate) fn load(weights: &BodyWeights) -> Result<Self> {
        Ok(Self {
            pose: ReluFfn::load(weights, "head_pose.proj", DEC_DIM, DEC_DIM, NPOSE)?,
            camera: ReluFfn::load(weights, "head_camera.proj", DEC_DIM, DEC_DIM, NCAM)?,
            keypoint_posemb: ReluFfn::load(
                weights,
                "keypoint_posemb_linear",
                2,
                DEC_DIM,
                DEC_DIM,
            )?,
            keypoint3d_posemb: ReluFfn::load(
                weights,
                "keypoint3d_posemb_linear",
                3,
                DEC_DIM,
                DEC_DIM,
            )?,
            keypoint_feat: HostLinear::load(
                weights,
                "keypoint_feat_linear",
                DEC_DIM,
                DINO_DIM,
            )?,
            bbox: BboxMlp::load(weights)?,
            hand_cls: HostLinear::load(weights, "hand_cls_embed", 2, DEC_DIM)?,
        })
    }

    pub(crate) fn pose(&self, input: &[f32]) -> Vec<f32> {
        self.pose.forward_row(input)
    }

    pub(crate) fn camera(&self, input: &[f32]) -> Vec<f32> {
        self.camera.forward_row(input)
    }

    pub(crate) fn keypoint_posemb(&self, input: &[f32]) -> Vec<f32> {
        self.keypoint_posemb.forward_rows(input)
    }

    pub(crate) fn keypoint3d_posemb(&self, input: &[f32]) -> Vec<f32> {
        self.keypoint3d_posemb.forward_rows(input)
    }

    pub(crate) fn keypoint_features(&self, input: &[f32]) -> Vec<f32> {
        self.keypoint_feat.forward_rows(input)
    }

    pub(crate) fn bbox(&self, input: &[f32]) -> [f32; 4] {
        self.bbox.forward(input)
    }

    pub(crate) fn hand_logits(&self, input: &[f32]) -> [f32; 2] {
        let output = self.hand_cls.forward_row(input);
        [output[0], output[1]]
    }
}

/// A linear layer resident on the GPU, applied to host rows.
pub(crate) struct GpuLinearRows {
    weight: GpuTensor,
    bias: GpuTensor,
    input: usize,
    output: usize,
}

impl GpuLinearRows {
    pub(crate) fn upload(linear: &HostLinear) -> Result<Self> {
        Ok(Self {
            weight: gpu_upload(&linear.weight, linear.output, linear.input)
                .map_err(DiffusionError::model)?,
            bias: gpu_upload(&linear.bias, 1, linear.output).map_err(DiffusionError::model)?,
            input: linear.input,
            output: linear.output,
        })
    }

    pub(crate) fn forward(&self, input: &GpuTensor) -> Result<GpuTensor> {
        gpu_linear_f32_resident(input, &self.weight, Some(&self.bias)).map_err(DiffusionError::model)
    }

    pub(crate) fn forward_rows(&self, input: &[f32]) -> Result<Vec<f32>> {
        debug_assert_eq!(input.len() % self.input, 0);
        let rows = input.len() / self.input;
        let x = gpu_upload(input, rows, self.input).map_err(DiffusionError::model)?;
        let y = self.forward(&x)?;
        debug_assert_eq!(y.cols(), self.output);
        gpu_download(&y).map_err(DiffusionError::model)
    }
}

pub(crate) struct GpuReluFfn {
    first: GpuLinearRows,
    second: GpuLinearRows,
}

impl GpuReluFfn {
    pub(crate) fn upload(ffn: &ReluFfn) -> Result<Self> {
        Ok(Self {
            first: GpuLinearRows::upload(&ffn.first)?,
            second: GpuLinearRows::upload(&ffn.second)?,
        })
    }

    pub(crate) fn forward_rows(&self, input: &[f32]) -> Result<Vec<f32>> {
        debug_assert_eq!(input.len() % self.first.input, 0);
        let rows = input.len() / self.first.input;
        let x = gpu_upload(input, rows, self.first.input).map_err(DiffusionError::model)?;
        let h = self.first.forward(&x)?;
        let h = gpu_birefnet_relu(&h).map_err(DiffusionError::model)?;
        let y = self.second.forward(&h)?;
        gpu_download(&y).map_err(DiffusionError::model)
    }
}

/// The per-step heads on the GPU: pose and camera on the normalised pose
/// token, and the three refinement embeddings over the 70 keypoint rows.
pub(crate) struct GpuStepHeads {
    pose: GpuReluFfn,
    camera: GpuReluFfn,
    keypoint_posemb: GpuReluFfn,
    keypoint3d_posemb: GpuReluFfn,
    keypoint_feat: GpuLinearRows,
}

impl GpuStepHeads {
    pub(crate) fn upload(heads: &DecoderHeads) -> Result<Self> {
        Ok(Self {
            pose: GpuReluFfn::upload(&heads.pose)?,
            camera: GpuReluFfn::upload(&heads.camera)?,
            keypoint_posemb: GpuReluFfn::upload(&heads.keypoint_posemb)?,
            keypoint3d_posemb: GpuReluFfn::upload(&heads.keypoint3d_posemb)?,
            keypoint_feat: GpuLinearRows::upload(&heads.keypoint_feat)?,
        })
    }

    pub(crate) fn pose(&self, input: &[f32]) -> Result<Vec<f32>> {
        self.pose.forward_rows(input)
    }

    pub(crate) fn camera(&self, input: &[f32]) -> Result<Vec<f32>> {
        self.camera.forward_rows(input)
    }

    pub(crate) fn keypoint_posemb(&self, input: &[f32]) -> Result<Vec<f32>> {
        self.keypoint_posemb.forward_rows(input)
    }

    pub(crate) fn keypoint3d_posemb(&self, input: &[f32]) -> Result<Vec<f32>> {
        self.keypoint3d_posemb.forward_rows(input)
    }

    pub(crate) fn keypoint_features(&self, input: &[f32]) -> Result<Vec<f32>> {
        self.keypoint_feat.forward_rows(input)
    }
}

fn relu_in_place(values: &mut [f32]) {
    for value in values {
        *value = value.max(0.0);
    }
}

fn sigmoid(value: f32) -> f32 {
    1.0 / (1.0 + (-value).exp())
}
