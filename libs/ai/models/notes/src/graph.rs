//! The three Basic Pitch heads.
//!
//! [`NotesGraph`] is the portable f32 oracle and normal fallback. The same
//! six convolutions are also described by [`DeviceNotesGraph`] as one ggml
//! graph, compiled through `GraphDevice::{Metal,Cuda}`. Constructing the
//! latter is explicit so CPU-only hosts never probe or touch a GPU.

use crate::config::{CONTOUR_BINS, WINDOW_FRAMES};
use crate::cqt::HarmonicFeatures;
use crate::weights::{ConvWeights, NotesWeights};

#[derive(Clone, Debug)]
struct Tensor3 {
    channels: usize,
    height: usize,
    width: usize,
    data: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct HeadOutput {
    /// `[time][264 contour bins]`.
    pub contours: Vec<f32>,
    /// `[time][88 notes]`.
    pub notes: Vec<f32>,
    /// `[time][88 notes]`.
    pub onsets: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct NotesGraph {
    contour: ConvWeights,
    contour_out: ConvWeights,
    note: ConvWeights,
    note_out: ConvWeights,
    onset: ConvWeights,
    onset_out: ConvWeights,
}

impl NotesGraph {
    pub fn new(weights: &NotesWeights) -> Self {
        Self {
            contour: weights.contour.clone(),
            contour_out: weights.contour_out.clone(),
            note: weights.note.clone(),
            note_out: weights.note_out.clone(),
            onset: weights.onset.clone(),
            onset_out: weights.onset_out.clone(),
        }
    }

    pub fn forward(&self, features: &HarmonicFeatures) -> Result<HeadOutput, String> {
        if features.channels != 8 || features.bins != CONTOUR_BINS {
            return Err(format!(
                "Basic Pitch graph expected [8,T,{CONTOUR_BINS}], got [{},{},{}]",
                features.channels, features.frames, features.bins
            ));
        }
        let input = Tensor3 {
            channels: features.channels,
            height: features.frames,
            width: features.bins,
            data: features.data.clone(),
        };
        let mut contour_hidden = conv2d(&input, &self.contour, 1, 1, 1, 19)?;
        relu(&mut contour_hidden.data);
        let mut contours = conv2d(&contour_hidden, &self.contour_out, 1, 1, 2, 2)?;
        sigmoid(&mut contours.data);

        let mut note_hidden = conv2d(&contours, &self.note, 1, 3, 3, 2)?;
        relu(&mut note_hidden.data);
        let mut notes = conv2d(&note_hidden, &self.note_out, 1, 1, 3, 1)?;
        sigmoid(&mut notes.data);

        let mut onset_hidden = conv2d(&input, &self.onset, 1, 3, 2, 1)?;
        relu(&mut onset_hidden.data);
        let joined = concat_channels(&notes, &onset_hidden)?;
        let mut onsets = conv2d(&joined, &self.onset_out, 1, 1, 1, 1)?;
        sigmoid(&mut onsets.data);

        Ok(HeadOutput {
            contours: contours.data,
            notes: notes.data,
            onsets: onsets.data,
        })
    }
}

fn conv2d(
    input: &Tensor3,
    layer: &ConvWeights,
    stride_time: usize,
    stride_freq: usize,
    pad_time: usize,
    pad_freq: usize,
) -> Result<Tensor3, String> {
    if input.channels != layer.in_channels {
        return Err(format!(
            "conv input has {} channels, weights require {}",
            input.channels, layer.in_channels
        ));
    }
    let out_h = (input.height + 2 * pad_time - layer.kernel_time) / stride_time + 1;
    let out_w = (input.width + 2 * pad_freq - layer.kernel_freq) / stride_freq + 1;
    let plane = out_h * out_w;
    let mut output = vec![0.0f32; layer.out_channels * plane];
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(layer.out_channels);
    let channels_per_worker = layer.out_channels.div_ceil(workers);
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in output
            .chunks_mut(channels_per_worker * plane)
            .enumerate()
        {
            let first_channel = chunk_index * channels_per_worker;
            scope.spawn(move || {
                for local_channel in 0..chunk.len() / plane {
                    let out_channel = first_channel + local_channel;
                    let out = &mut chunk[local_channel * plane..(local_channel + 1) * plane];
                    out.fill(layer.bias[out_channel]);
                    for in_channel in 0..input.channels {
                        for ky in 0..layer.kernel_time {
                            for kx in 0..layer.kernel_freq {
                                let weight_index = (((out_channel * layer.in_channels + in_channel)
                                    * layer.kernel_time
                                    + ky)
                                    * layer.kernel_freq)
                                    + kx;
                                let weight = layer.values[weight_index];
                                for oy in 0..out_h {
                                    let padded_y = oy * stride_time + ky;
                                    if padded_y < pad_time {
                                        continue;
                                    }
                                    let iy = padded_y - pad_time;
                                    if iy >= input.height {
                                        continue;
                                    }
                                    let input_row =
                                        (in_channel * input.height + iy) * input.width;
                                    let output_row = oy * out_w;
                                    for ox in 0..out_w {
                                        let padded_x = ox * stride_freq + kx;
                                        if padded_x < pad_freq {
                                            continue;
                                        }
                                        let ix = padded_x - pad_freq;
                                        if ix < input.width {
                                            out[output_row + ox] +=
                                                input.data[input_row + ix] * weight;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
        }
    });
    Ok(Tensor3 {
        channels: layer.out_channels,
        height: out_h,
        width: out_w,
        data: output,
    })
}

fn concat_channels(a: &Tensor3, b: &Tensor3) -> Result<Tensor3, String> {
    if a.height != b.height || a.width != b.width {
        return Err("cannot concatenate Basic Pitch features with different geometry".to_string());
    }
    let mut data = Vec::with_capacity(a.data.len() + b.data.len());
    data.extend_from_slice(&a.data);
    data.extend_from_slice(&b.data);
    Ok(Tensor3 {
        channels: a.channels + b.channels,
        height: a.height,
        width: a.width,
        data,
    })
}

fn relu(values: &mut [f32]) {
    for value in values {
        *value = value.max(0.0);
    }
}

fn sigmoid(values: &mut [f32]) {
    for value in values {
        *value = if *value >= 0.0 {
            1.0 / (1.0 + (-*value).exp())
        } else {
            let e = value.exp();
            e / (1.0 + e)
        };
    }
}

// -------------------------------------------------------------------------
// Device graph. This is compiled only when a caller explicitly asks for it.

use makepad_ai_common::backend::{
    BufferStorageMode, DeviceGraphSession, DeviceRuntime, GraphDevice,
};
use makepad_ai_common::{
    BufferUsage, Context, Graph, InitParams, Op, TensorId, TensorType,
    UnaryOp,
};

pub struct DeviceNotesGraph {
    ctx: Context,
    session: DeviceGraphSession,
    input: TensorId,
    contours: TensorId,
    notes: TensorId,
    onsets: TensorId,
}

impl DeviceNotesGraph {
    pub fn load(weights: &NotesWeights) -> Result<Self, String> {
        let runtime = DeviceRuntime::new().map_err(|e| e.to_string())?;
        Self::load_with_runtime(weights, runtime)
    }

    pub fn load_with_runtime(
        weights: &NotesWeights,
        runtime: DeviceRuntime,
    ) -> Result<Self, String> {
        let mut ctx = Context::new(InitParams {
            mem_size: 4 << 20,
            mem_buffer: None,
            no_alloc: false,
        });
        let input = ctx
            .new_named_tensor(
                "notes.input",
                TensorType::F32,
                4,
                &[CONTOUR_BINS as i64, WINDOW_FRAMES as i64, 8, 1],
                BufferUsage::Activations,
            )
            .map_err(|e| e.to_string())?;
        let contour = device_layer(&mut ctx, "notes.contour", &weights.contour)?;
        let contour_out = device_layer(&mut ctx, "notes.contour_out", &weights.contour_out)?;
        let note = device_layer(&mut ctx, "notes.note", &weights.note)?;
        let note_out = device_layer(&mut ctx, "notes.note_out", &weights.note_out)?;
        let onset = device_layer(&mut ctx, "notes.onset", &weights.onset)?;
        let onset_out = device_layer(&mut ctx, "notes.onset_out", &weights.onset_out)?;
        ctx.set_no_alloc(true);

        let contour_hidden = device_conv(&mut ctx, input, contour, 1, 1, 19, 1)?;
        let contour_hidden = ctx
            .unary(contour_hidden, UnaryOp::Relu, BufferUsage::Activations)
            .map_err(|e| e.to_string())?;
        let contours = device_conv(&mut ctx, contour_hidden, contour_out, 1, 1, 2, 2)?;
        let contours = ctx
            .unary(contours, UnaryOp::Sigmoid, BufferUsage::Activations)
            .map_err(|e| e.to_string())?;

        let note_hidden = device_conv(&mut ctx, contours, note, 3, 1, 2, 3)?;
        let note_hidden = ctx
            .unary(note_hidden, UnaryOp::Relu, BufferUsage::Activations)
            .map_err(|e| e.to_string())?;
        let notes = device_conv(&mut ctx, note_hidden, note_out, 1, 1, 1, 3)?;
        let notes = ctx
            .unary(notes, UnaryOp::Sigmoid, BufferUsage::Activations)
            .map_err(|e| e.to_string())?;

        let onset_hidden = device_conv(&mut ctx, input, onset, 3, 1, 1, 2)?;
        let onset_hidden = ctx
            .unary(onset_hidden, UnaryOp::Relu, BufferUsage::Activations)
            .map_err(|e| e.to_string())?;
        let joined = ctx
            .concat(notes, onset_hidden, 2, BufferUsage::Activations)
            .map_err(|e| e.to_string())?;
        let onsets = device_conv(&mut ctx, joined, onset_out, 1, 1, 1, 1)?;
        let onsets = ctx
            .unary(onsets, UnaryOp::Sigmoid, BufferUsage::Activations)
            .map_err(|e| e.to_string())?;

        ctx.set_no_alloc(false);
        let mut graph = Graph::new();
        for output in [contours, notes, onsets] {
            graph
                .build_forward_expand(&ctx, output)
                .map_err(|e| e.to_string())?;
        }
        let session = runtime
            .compile_graph(
                &ctx,
                &graph,
                &[contours, notes, onsets],
                BufferStorageMode::Shared,
                BufferStorageMode::Shared,
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            ctx,
            session,
            input,
            contours,
            notes,
            onsets,
        })
    }

    pub fn device(&self) -> GraphDevice {
        self.session.device()
    }

    pub fn forward(&self, features: &HarmonicFeatures) -> Result<HeadOutput, String> {
        if (features.channels, features.frames, features.bins)
            != (8, WINDOW_FRAMES, CONTOUR_BINS)
        {
            return Err("device Basic Pitch graph requires one 172-frame window".to_string());
        }
        let bytes = bytes_of_f32(&features.data);
        let execution = self
            .session
            .execute(
                &self.ctx,
                &[(self.input, bytes)],
                &[self.contours, self.notes, self.onsets],
            )
            .map_err(|e| e.to_string())?;
        Ok(HeadOutput {
            contours: output_f32(&execution.outputs, self.contours)?,
            notes: output_f32(&execution.outputs, self.notes)?,
            onsets: output_f32(&execution.outputs, self.onsets)?,
        })
    }
}

#[derive(Clone, Copy)]
struct DeviceLayer {
    weight: TensorId,
    bias: TensorId,
    out_channels: usize,
}

fn device_layer(
    ctx: &mut Context,
    name: &str,
    layer: &ConvWeights,
) -> Result<DeviceLayer, String> {
    let weight = ctx
        .new_named_tensor(
            format!("{name}.weight"),
            TensorType::F32,
            4,
            &[
                layer.kernel_freq as i64,
                layer.kernel_time as i64,
                layer.in_channels as i64,
                layer.out_channels as i64,
            ],
            BufferUsage::Weights,
        )
        .map_err(|e| e.to_string())?;
    ctx.write_tensor_data(weight, bytes_of_f32(&layer.values))
        .map_err(|e| e.to_string())?;
    let bias = ctx
        .new_named_tensor(
            format!("{name}.bias"),
            TensorType::F32,
            1,
            &[layer.out_channels as i64],
            BufferUsage::Weights,
        )
        .map_err(|e| e.to_string())?;
    ctx.write_tensor_data(bias, bytes_of_f32(&layer.bias))
        .map_err(|e| e.to_string())?;
    Ok(DeviceLayer {
        weight,
        bias,
        out_channels: layer.out_channels,
    })
}

fn device_conv(
    ctx: &mut Context,
    input: TensorId,
    layer: DeviceLayer,
    stride_freq: i32,
    stride_time: i32,
    pad_freq: i32,
    pad_time: i32,
) -> Result<TensorId, String> {
    let output = ctx
        .conv_2d(
            layer.weight,
            input,
            stride_freq,
            stride_time,
            pad_freq,
            pad_time,
            1,
            1,
            BufferUsage::Activations,
        )
        .map_err(|e| e.to_string())?;
    let bias = ctx
        .reshape(layer.bias, &[1, 1, layer.out_channels as i64, 1])
        .map_err(|e| e.to_string())?;
    let bias = ctx
        .repeat(bias, output, BufferUsage::Activations)
        .map_err(|e| e.to_string())?;
    ctx.binary_like_a(Op::Add, output, bias, BufferUsage::Activations)
        .map_err(|e| e.to_string())
}

fn bytes_of_f32(values: &[f32]) -> &[u8] {
    // f32 has no padding and every bit pattern is valid.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), values.len() * 4) }
}

fn output_f32(
    outputs: &std::collections::BTreeMap<TensorId, Vec<u8>>,
    id: TensorId,
) -> Result<Vec<f32>, String> {
    let bytes = outputs
        .get(&id)
        .ok_or_else(|| format!("device graph did not return output tensor {id}"))?;
    if bytes.len() % 4 != 0 {
        return Err("device graph returned a non-f32-aligned output".to_string());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointwise_conv_and_channel_order() {
        let input = Tensor3 {
            channels: 2,
            height: 1,
            width: 2,
            data: vec![1.0, 2.0, 3.0, 4.0],
        };
        let layer = ConvWeights {
            out_channels: 1,
            in_channels: 2,
            kernel_time: 1,
            kernel_freq: 1,
            values: vec![2.0, 10.0],
            bias: vec![0.5],
        };
        let out = conv2d(&input, &layer, 1, 1, 0, 0).unwrap();
        assert_eq!(out.data, vec![32.5, 44.5]);
    }

    #[test]
    #[ignore = "requires an available Metal or CUDA device; CPU graph is the test oracle"]
    fn device_graph_builds() {
        let checkpoint = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../local/models/weights/basic_pitch/nmp.onnx");
        let weights = NotesWeights::load(checkpoint).unwrap();
        let _ = DeviceNotesGraph::load(&weights).unwrap();
    }
}
