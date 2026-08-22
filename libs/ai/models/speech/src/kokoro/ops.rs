//! Plain f32 ops. Correctness first; `makepad-ggml` and Metal come after the
//! numbers are pinned against the ONNX reference.
//!
//! Convention matches PyTorch: activations are channel-major `[channels, time]`,
//! conv weights are `[out, in, kernel]`.

/// Row-major matrix. For activations, `rows` is channels and `cols` is time.
#[derive(Clone, Debug)]
pub struct Mat {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f32>,
}

impl Mat {
    pub fn zeros(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            data: vec![0.0; rows * cols],
        }
    }

    pub fn from_vec(rows: usize, cols: usize, data: Vec<f32>) -> Self {
        assert_eq!(rows * cols, data.len(), "shape does not match data");
        Self { rows, cols, data }
    }

    #[inline]
    pub fn row(&self, row: usize) -> &[f32] {
        &self.data[row * self.cols..(row + 1) * self.cols]
    }

    #[inline]
    pub fn row_mut(&mut self, row: usize) -> &mut [f32] {
        &mut self.data[row * self.cols..(row + 1) * self.cols]
    }

    #[inline]
    pub fn at(&self, row: usize, col: usize) -> f32 {
        self.data[row * self.cols + col]
    }

    /// `[rows, cols]` -> `[cols, rows]`.
    pub fn transpose(&self) -> Mat {
        let mut out = Mat::zeros(self.cols, self.rows);
        for r in 0..self.rows {
            for c in 0..self.cols {
                out.data[c * self.rows + r] = self.data[r * self.cols + c];
            }
        }
        out
    }
}

/// Gather embedding rows: `tokens` -> `[embed_dim, time]`.
pub fn embed(tokens: &[u16], table: &[f32], embed_dim: usize) -> Mat {
    let mut out = Mat::zeros(embed_dim, tokens.len());
    for (t, token) in tokens.iter().enumerate() {
        let row = *token as usize * embed_dim;
        for c in 0..embed_dim {
            out.data[c * tokens.len() + t] = table[row + c];
        }
    }
    out
}

/// 1-D convolution, stride 1, dilation 1, zero padding.
///
/// `weight` is `[out_channels, in_channels, kernel]` flattened.
pub fn conv1d(x: &Mat, weight: &[f32], bias: &[f32], out_channels: usize, pad: usize) -> Mat {
    conv1d_general(x, weight, bias, out_channels, pad, 1, 1)
}

/// LayerNorm across channels, applied independently at each time step.
///
/// StyleTTS2's `LayerNorm` module transposes so channels are last, calls
/// `F.layer_norm` over the channel dimension, then transposes back — so this
/// normalizes over channels, never over time.
pub fn layer_norm_channels(x: &Mat, gamma: &[f32], beta: &[f32], eps: f32) -> Mat {
    let channels = x.rows;
    let time = x.cols;
    let mut out = Mat::zeros(channels, time);
    for t in 0..time {
        let mean = (0..channels).map(|c| x.at(c, t)).sum::<f32>() / channels as f32;
        let variance = (0..channels)
            .map(|c| {
                let d = x.at(c, t) - mean;
                d * d
            })
            .sum::<f32>()
            / channels as f32;
        let inv = 1.0 / (variance + eps).sqrt();
        for c in 0..channels {
            out.data[c * time + t] = (x.at(c, t) - mean) * inv * gamma[c] + beta[c];
        }
    }
    out
}

/// InstanceNorm1d: normalize over TIME, independently per channel. No affine —
/// the scale and shift arrive from the style vector in AdaIN.
///
/// The axis is the trap: normalizing over channels instead of time produces
/// audio that is recognizably speech and subtly wrong.
pub fn instance_norm_time(x: &Mat, eps: f32) -> Mat {
    let mut out = Mat::zeros(x.rows, x.cols);
    for_each_channel(&mut out, |channel, target| {
        let input = x.row(channel);
        let mean = input.iter().sum::<f32>() / x.cols as f32;
        let variance = input.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / x.cols as f32;
        let inv = 1.0 / (variance + eps).sqrt();
        for t in 0..x.cols {
            target[t] = (input[t] - mean) * inv;
        }
    });
    out
}

/// Nearest-neighbour upsample along time, factor two.
pub fn nearest_upsample2(x: &Mat) -> Mat {
    let mut out = Mat::zeros(x.rows, x.cols * 2);
    for channel in 0..x.rows {
        let input = x.row(channel);
        let target = out.row_mut(channel);
        for t in 0..x.cols {
            target[2 * t] = input[t];
            target[2 * t + 1] = input[t];
        }
    }
    out
}

/// Depthwise `ConvTranspose1d`, one kernel per channel. Used by the `pool`
/// layers: `k=3, stride=2, padding=1, output_padding=1`, so time exactly doubles.
pub fn conv_transpose1d_depthwise(
    x: &Mat,
    weight: &[f32],
    bias: &[f32],
    stride: usize,
    pad: usize,
    output_pad: usize,
) -> Mat {
    let channels = x.rows;
    let kernel = weight.len() / channels;
    let out_time = (x.cols - 1) * stride + kernel + output_pad - 2 * pad;

    let mut out = Mat::zeros(channels, out_time);
    for channel in 0..channels {
        let taps = &weight[channel * kernel..(channel + 1) * kernel];
        let input = x.row(channel);
        let target = out.row_mut(channel);
        for t in 0..x.cols {
            for (k, tap) in taps.iter().enumerate() {
                let position = t * stride + k;
                if position < pad {
                    continue;
                }
                if let Some(cell) = target.get_mut(position - pad) {
                    *cell += input[t] * tap;
                }
            }
        }
        for value in target.iter_mut() {
            *value += bias[channel];
        }
    }
    out
}

/// `ConvTranspose1d` with weight `[in, out, kernel]` — note dim 0 is the *input*
/// channel axis, which is also the axis PyTorch's weight-norm uses here.
pub fn conv_transpose1d(
    x: &Mat,
    weight: &[f32],
    bias: &[f32],
    out_channels: usize,
    stride: usize,
    pad: usize,
) -> Mat {
    let in_channels = x.rows;
    let kernel = weight.len() / (in_channels * out_channels);
    let time = x.cols;
    let out_time = (time - 1) * stride + kernel - 2 * pad;

    // GPU path: compute every tap product as one `[out * kernel, in] x [in, time]`
    // matmul, then overlap-add the taps into place — the same split the iSTFT
    // uses, and the scatter is a rounding error next to the matmul.
    if out_channels * kernel * in_channels * time >= 4 * 1024 * 1024 {
        let mut reordered = vec![0f32; weight.len()];
        for ic in 0..in_channels {
            for oc in 0..out_channels {
                for k in 0..kernel {
                    reordered[(oc * kernel + k) * in_channels + ic] =
                        weight[(ic * out_channels + oc) * kernel + k];
                }
            }
        }
        if let Some(taps) = super::accel::matmul_nn(
            &reordered,
            &x.data,
            out_channels * kernel,
            in_channels,
            time,
        ) {
            let mut out = Mat::zeros(out_channels, out_time);
            for_each_channel(&mut out, |oc, row| {
                for value in row.iter_mut() {
                    *value = bias[oc];
                }
                for k in 0..kernel {
                    let products = &taps[(oc * kernel + k) * time..][..time];
                    for (t, product) in products.iter().enumerate() {
                        let position = t * stride + k;
                        if position < pad {
                            continue;
                        }
                        if let Some(cell) = row.get_mut(position - pad) {
                            *cell += product;
                        }
                    }
                }
            });
            return out;
        }
    }

    let mut out = Mat::zeros(out_channels, out_time);
    for ic in 0..in_channels {
        let input = x.row(ic);
        for oc in 0..out_channels {
            let base = (ic * out_channels + oc) * kernel;
            let taps = &weight[base..base + kernel];
            let target = out.row_mut(oc);
            for t in 0..x.cols {
                let value = input[t];
                if value == 0.0 {
                    continue;
                }
                for (k, tap) in taps.iter().enumerate() {
                    let position = t * stride + k;
                    if position < pad {
                        continue;
                    }
                    if let Some(cell) = target.get_mut(position - pad) {
                        *cell += value * tap;
                    }
                }
            }
        }
    }
    for oc in 0..out_channels {
        let b = bias[oc];
        for value in out.row_mut(oc) {
            *value += b;
        }
    }
    out
}

/// Repeat each row of a `[time, channels]` matrix `durations[i]` times, and
/// return it channel-major as `[channels, frames]`.
///
/// This is the alignment: the export builds a 0/1 matrix with `CumSum`/`Less`
/// and multiplies, which is a repeat by another name.
pub fn expand_to_frames(rows: &Mat, durations: &[usize]) -> Mat {
    let frames: usize = durations.iter().sum();
    let mut out = Mat::zeros(rows.cols, frames);
    let mut frame = 0;
    for (index, count) in durations.iter().enumerate() {
        let source = rows.row(index);
        for _ in 0..*count {
            for channel in 0..rows.cols {
                out.data[channel * frames + frame] = source[channel];
            }
            frame += 1;
        }
    }
    out
}

pub fn leaky_relu(x: &mut Mat, slope: f32) {
    for value in &mut x.data {
        if *value < 0.0 {
            *value *= slope;
        }
    }
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// `y = x W^T + b`, with `x` `[n, in]` and `weight` `[out, in]`.
pub fn linear(x: &Mat, weight: &[f32], bias: &[f32], out_dim: usize) -> Mat {
    let in_dim = x.cols;

    if let Some(mut data) = super::accel::matmul_nt(&x.data, weight, x.rows, in_dim, out_dim) {
        for row in data.chunks_mut(out_dim) {
            for (value, b) in row.iter_mut().zip(bias) {
                *value += b;
            }
        }
        return Mat::from_vec(x.rows, out_dim, data);
    }

    let mut out = Mat::zeros(x.rows, out_dim);
    for_each_channel(&mut out, |row, target| {
        let input = x.row(row);
        for o in 0..out_dim {
            let w = &weight[o * in_dim..(o + 1) * in_dim];
            let mut sum = bias[o];
            for (wi, xi) in w.iter().zip(input) {
                sum += wi * xi;
            }
            target[o] = sum;
        }
    });
    out
}

/// LayerNorm over the last axis of a `[rows, cols]` matrix (BERT's layout).
pub fn layer_norm_rows(x: &Mat, gamma: &[f32], beta: &[f32], eps: f32) -> Mat {
    let mut out = Mat::zeros(x.rows, x.cols);
    for row in 0..x.rows {
        let input = x.row(row);
        let mean = input.iter().sum::<f32>() / x.cols as f32;
        let variance = input.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / x.cols as f32;
        let inv = 1.0 / (variance + eps).sqrt();
        let target = out.row_mut(row);
        for c in 0..x.cols {
            target[c] = (input[c] - mean) * inv * gamma[c] + beta[c];
        }
    }
    out
}

/// LayerNorm over the last axis with no affine — the scale and shift come from
/// the style vector in AdaLayerNorm.
pub fn layer_norm_plain(x: &Mat, eps: f32) -> Mat {
    let mut out = Mat::zeros(x.rows, x.cols);
    for row in 0..x.rows {
        let input = x.row(row);
        let mean = input.iter().sum::<f32>() / x.cols as f32;
        let variance = input.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / x.cols as f32;
        let inv = 1.0 / (variance + eps).sqrt();
        let target = out.row_mut(row);
        for c in 0..x.cols {
            target[c] = (input[c] - mean) * inv;
        }
    }
    out
}

/// Append `extra` to every row: `[rows, cols] -> [rows, cols + extra.len()]`.
pub fn concat_cols(x: &Mat, extra: &[f32]) -> Mat {
    let cols = x.cols + extra.len();
    let mut out = Mat::zeros(x.rows, cols);
    for row in 0..x.rows {
        let target = out.row_mut(row);
        target[..x.cols].copy_from_slice(x.row(row));
        target[x.cols..].copy_from_slice(extra);
    }
    out
}

/// The tanh approximation. Kokoro's export has no `Erf` node and twelve `Tanh`
/// nodes — one per ALBERT layer — so this is the variant the model was trained
/// with, not exact GELU.
#[inline]
pub fn gelu_new(x: f32) -> f32 {
    const SQRT_2_OVER_PI: f32 = 0.7978845608028654;
    0.5 * x * (1.0 + (SQRT_2_OVER_PI * (x + 0.044715 * x * x * x)).tanh())
}

/// `torch.round`: half-to-even, unlike Rust's `f32::round` (half-away-from-zero).
///
/// A single phoneme sitting exactly on `.5` shifts the whole alignment by a
/// frame, and the two conventions disagree only there — so the bug hides until
/// some sentence happens to land on the boundary.
pub fn round_half_even(x: f32) -> f32 {
    let rounded = x.round();
    if (x - x.trunc()).abs() == 0.5 && rounded % 2.0 != 0.0 {
        rounded - x.signum()
    } else {
        rounded
    }
}

pub fn sigmoid_slice(values: &mut [f32]) {
    for value in values.iter_mut() {
        *value = sigmoid(*value);
    }
}

/// In-place softmax over a slice.
pub fn softmax(values: &mut [f32]) {
    let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0;
    for value in values.iter_mut() {
        *value = (*value - max).exp();
        sum += *value;
    }
    for value in values.iter_mut() {
        *value /= sum;
    }
}

pub fn add_into(target: &mut Mat, other: &Mat) {
    for (a, b) in target.data.iter_mut().zip(&other.data) {
        *a += *b;
    }
}

/// Fill each output channel on its own thread. The generator is ~75 GMAC — a
/// single core spends a minute on it — and every output channel writes a
/// disjoint row, so no locking is needed.
fn for_each_channel<F>(out: &mut Mat, body: F)
where
    F: Fn(usize, &mut [f32]) + Sync,
{
    let (rows, cols) = (out.rows, out.cols);
    if rows == 0 || cols == 0 {
        return;
    }
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(rows);
    if threads <= 1 {
        for (channel, row) in out.data.chunks_mut(cols).enumerate() {
            body(channel, row);
        }
        return;
    }

    let per_thread = rows.div_ceil(threads);
    let body = &body;
    std::thread::scope(|scope| {
        let mut rest: &mut [f32] = &mut out.data;
        let mut first = 0;
        while first < rows {
            let take = per_thread.min(rows - first);
            let (head, tail) = rest.split_at_mut(take * cols);
            rest = tail;
            scope.spawn(move || {
                for (offset, row) in head.chunks_mut(cols).enumerate() {
                    body(first + offset, row);
                }
            });
            first += take;
        }
    });
}

/// 1-D convolution with stride and dilation. `weight` is
/// `[out_channels, in_channels, kernel]` flattened.
///
/// The innermost loop is a contiguous `row[t] += tap * input[t + offset]` when
/// `stride == 1`, which is every convolution in the generator's residual blocks
/// — written this way so it vectorizes.
pub fn conv1d_general(
    x: &Mat,
    weight: &[f32],
    bias: &[f32],
    out_channels: usize,
    pad: usize,
    stride: usize,
    dilation: usize,
) -> Mat {
    let in_channels = x.rows;
    let time = x.cols;
    let kernel = weight.len() / (out_channels * in_channels);
    let span = dilation * (kernel - 1) + 1;
    let out_time = (time + 2 * pad).saturating_sub(span) / stride + 1;

    // The GPU path: unfold the input into one patch row per output position, so
    // the convolution is a single `[out, in * kernel] x patches^T` matmul. The
    // patch matrix is built directly in the transposed layout `matmul_nt` wants.
    // `accel` declines small products and non-Apple platforms.
    if out_channels * in_channels * kernel * out_time >= 4 * 1024 * 1024 {
        let width = in_channels * kernel;
        let mut patches = Mat::zeros(out_time, width);
        for_each_channel(&mut patches, |t, row| {
            for ic in 0..in_channels {
                let input = x.row(ic);
                let target = &mut row[ic * kernel..(ic + 1) * kernel];
                for (k, cell) in target.iter_mut().enumerate() {
                    let position = t * stride + k * dilation;
                    *cell = if position >= pad && position - pad < time {
                        input[position - pad]
                    } else {
                        0.0
                    };
                }
            }
        });
        if let Some(mut data) =
            super::accel::matmul_nt(weight, &patches.data, out_channels, width, out_time)
        {
            for (oc, row) in data.chunks_mut(out_time).enumerate() {
                let b = bias[oc];
                for value in row.iter_mut() {
                    *value += b;
                }
            }
            return Mat::from_vec(out_channels, out_time, data);
        }
    }

    let mut out = Mat::zeros(out_channels, out_time);
    for_each_channel(&mut out, |oc, row| {
        row.fill(bias[oc]);
        let base = oc * in_channels * kernel;
        for ic in 0..in_channels {
            let input = x.row(ic);
            let taps = &weight[base + ic * kernel..base + (ic + 1) * kernel];
            for (k, tap) in taps.iter().enumerate() {
                let tap = *tap;
                if tap == 0.0 {
                    continue;
                }
                // Output `t` reads input at `t * stride + offset - pad`.
                let offset = k * dilation;
                let lo = pad.saturating_sub(offset).div_ceil(stride);
                let hi = if time + pad > offset {
                    out_time.min((time + pad - offset).div_ceil(stride))
                } else {
                    0
                };
                for t in lo..hi {
                    row[t] += tap * input[t * stride + offset - pad];
                }
            }
        }
    });
    out
}

/// `nn.ReflectionPad1d`: the padding mirrors around the edge sample without
/// repeating it, so a left pad of 1 copies `input[1]`, not `input[0]`.
pub fn reflect_pad(x: &Mat, left: usize, right: usize) -> Mat {
    let time = x.cols;
    let mut out = Mat::zeros(x.rows, time + left + right);
    for channel in 0..x.rows {
        let input = x.row(channel);
        let row = out.row_mut(channel);
        for i in 0..left {
            row[i] = input[left - i];
        }
        row[left..left + time].copy_from_slice(input);
        for j in 0..right {
            row[left + time + j] = input[time - 2 - j];
        }
    }
    out
}

/// Snake: `x + (1/a) * sin(a * x)^2`, with a trained `a` per channel. No
/// epsilon on the reciprocal — that is BigVGAN's SnakeBeta, not this model.
///
/// Threaded: the generator calls this on `[128, 14521]` maps thirty-odd times,
/// and `sin` dominates the whole decoder if left on one core.
pub fn snake(x: &mut Mat, alpha: &[f32]) {
    for_each_channel(x, |channel, row| {
        let a = alpha[channel];
        let inv = 1.0 / a;
        for value in row {
            let s = (a * *value).sin();
            *value += inv * s * s;
        }
    });
}

/// ONNX `Resize`, `mode=linear`, `coordinate_transformation_mode=half_pixel`.
///
/// Both of SineGen's resamples use this — `align_corners` is *false*, despite
/// upstream's source passing `align_corners=True` on the second one. The export
/// is what we match.
pub fn resize_linear(x: &Mat, scale: f32) -> Mat {
    let time = x.cols;
    let out_time = (time as f32 * scale).floor() as usize;
    let last = (time - 1) as f32;

    let mut out = Mat::zeros(x.rows, out_time);
    for channel in 0..x.rows {
        let input = x.row(channel);
        let row = out.row_mut(channel);
        for (t, cell) in row.iter_mut().enumerate() {
            let source = ((t as f32 + 0.5) / scale - 0.5).clamp(0.0, last);
            let low = source.floor();
            let frac = source - low;
            let low = low as usize;
            let high = (low + 1).min(time - 1);
            *cell = input[low] * (1.0 - frac) + input[high] * frac;
        }
    }
    out
}

/// `nn.Upsample(scale_factor, mode="nearest")`: `out[t] = in[t / factor]`.
pub fn nearest_repeat(x: &Mat, factor: usize) -> Mat {
    let mut out = Mat::zeros(x.rows, x.cols * factor);
    for channel in 0..x.rows {
        let input = x.row(channel);
        let row = out.row_mut(channel);
        for (t, cell) in row.iter_mut().enumerate() {
            *cell = input[t / factor];
        }
    }
    out
}

/// Stack matrices along the channel axis. Every part must share `cols`.
pub fn concat_rows(parts: &[&Mat]) -> Mat {
    let cols = parts[0].cols;
    let rows = parts.iter().map(|part| part.rows).sum();

    let mut out = Mat::zeros(rows, cols);
    let mut at = 0;
    for part in parts {
        out.data[at * cols..(at + part.rows) * cols].copy_from_slice(&part.data);
        at += part.rows;
    }
    out
}

/// One direction of a PyTorch `nn.LSTM` layer.
///
/// PyTorch packs `weight_ih` as `[4 * hidden, input]` with gate order
/// **i, f, g, o** — input, forget, cell candidate, output — and adds *both*
/// `bias_ih` and `bias_hh`. ONNX's LSTM operator uses a different gate order
/// (i, o, f, c); we load PyTorch weights, so PyTorch's order is the one that
/// matters here.
pub struct LstmLayer {
    pub hidden: usize,
    pub input: usize,
    pub weight_ih: Vec<f32>,
    pub weight_hh: Vec<f32>,
    pub bias_ih: Vec<f32>,
    pub bias_hh: Vec<f32>,
}

impl LstmLayer {
    /// `x` is `[time, input]` row-major. Returns `[time, hidden]`.
    pub fn run(&self, x: &Mat, reverse: bool) -> Mat {
        let time = x.rows;
        let hidden = self.hidden;
        let mut out = Mat::zeros(time, hidden);

        let mut h = vec![0.0f32; hidden];
        let mut c = vec![0.0f32; hidden];
        let mut gates = vec![0.0f32; 4 * hidden];

        for step in 0..time {
            let t = if reverse { time - 1 - step } else { step };
            let input = x.row(t);

            for gate in 0..4 * hidden {
                let ih = &self.weight_ih[gate * self.input..(gate + 1) * self.input];
                let hh = &self.weight_hh[gate * hidden..(gate + 1) * hidden];
                let mut sum = self.bias_ih[gate] + self.bias_hh[gate];
                for (w, v) in ih.iter().zip(input) {
                    sum += w * v;
                }
                for (w, v) in hh.iter().zip(&h) {
                    sum += w * v;
                }
                gates[gate] = sum;
            }

            for j in 0..hidden {
                let i = sigmoid(gates[j]);
                let f = sigmoid(gates[hidden + j]);
                let g = gates[2 * hidden + j].tanh();
                let o = sigmoid(gates[3 * hidden + j]);
                c[j] = f * c[j] + i * g;
                h[j] = o * c[j].tanh();
            }
            out.row_mut(t).copy_from_slice(&h);
        }
        out
    }
}

/// Bidirectional LSTM. PyTorch concatenates along the feature axis at each
/// timestep: `[forward_h(t), reverse_h(t)]`, where the reverse pass has already
/// been un-reversed so both refer to the same `t`.
pub struct BiLstm {
    pub forward: LstmLayer,
    pub reverse: LstmLayer,
}

impl BiLstm {
    /// `x` is `[time, input]`. Returns `[time, 2 * hidden]`.
    pub fn run(&self, x: &Mat) -> Mat {
        let forward = self.forward.run(x, false);
        let reverse = self.reverse.run(x, true);
        let hidden = self.forward.hidden;

        let mut out = Mat::zeros(x.rows, 2 * hidden);
        for t in 0..x.rows {
            let row = out.row_mut(t);
            row[..hidden].copy_from_slice(forward.row(t));
            row[hidden..].copy_from_slice(reverse.row(t));
        }
        out
    }
}
