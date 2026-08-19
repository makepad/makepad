//! Portable reference forward for the v4.26 IFNet.
//!
//! Every op here is the literal PyTorch semantic the released model uses, so
//! this module doubles as the specification the device kernels in
//! [`crate::rife_model`] must reproduce:
//!
//! - [`conv2d`] — `nn.Conv2d`, weight `[out, in, kh, kw]`, zero padding.
//! - [`conv_transpose2d`] — `nn.ConvTranspose2d`, weight `[in, out, kh, kw]`,
//!   `output_padding = 0`.
//! - [`pixel_shuffle`] — `nn.PixelShuffle(r)`, `in[(c*r + ky)*r + kx]`.
//! - [`resize_bilinear`] — `F.interpolate(mode="bilinear")`, i.e.
//!   `align_corners=False` (source `(o + 0.5) * in/out - 0.5`, clamped at 0).
//! - [`warp`] — RIFE's `warplayer.warp`: backward warp through
//!   `F.grid_sample(mode="bilinear", padding_mode="border",
//!   align_corners=True)`.  After folding the reference's
//!   `linspace(-1, 1, W)` grid and its `flow / ((W - 1) / 2)` normalization,
//!   the sample point is exactly `(x + flow_x, y + flow_y)` in pixels.
//!
//! It is a reference, not the production path: a 640x384 pair takes seconds
//! here and about a millisecond on the device path.

use crate::rife::{
    padded_extent, ConvWeight, DeconvWeight, HeadWeights, IfBlockWeights, ResConvWeight,
    RifeFramePair, RifeModelWeights, RifeScale, RIFE_LASTCONV_PLANES, RIFE_LRELU_SLOPE,
};
use crate::{DiffusionError, Result};

/// Planar batch-1 tensor: `data[channel * width * height + y * width + x]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Planes {
    pub channels: usize,
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Planes {
    pub fn new(channels: usize, width: usize, height: usize) -> Self {
        Self {
            channels,
            width,
            height,
            data: vec![0.0; channels * width * height],
        }
    }

    pub fn from_data(channels: usize, width: usize, height: usize, data: Vec<f32>) -> Result<Self> {
        if data.len() != channels * width * height {
            return Err(DiffusionError::model(format!(
                "rife planes: {} values for {channels}x{height}x{width}",
                data.len()
            )));
        }
        Ok(Self {
            channels,
            width,
            height,
            data,
        })
    }

    pub fn plane(&self) -> usize {
        self.width * self.height
    }

    pub fn channel(&self, index: usize) -> &[f32] {
        let plane = self.plane();
        &self.data[index * plane..(index + 1) * plane]
    }

    /// `x[start..start + len]` along the channel axis (PyTorch `x[:, a:b]`).
    pub fn slice_channels(&self, start: usize, len: usize) -> Planes {
        let plane = self.plane();
        Planes {
            channels: len,
            width: self.width,
            height: self.height,
            data: self.data[start * plane..(start + len) * plane].to_vec(),
        }
    }
}

/// `torch.cat(parts, 1)`.
pub fn concat_channels(parts: &[&Planes]) -> Result<Planes> {
    let first = parts
        .first()
        .ok_or_else(|| DiffusionError::model("rife concat: no parts"))?;
    let (width, height) = (first.width, first.height);
    let mut channels = 0;
    let mut data = Vec::new();
    for part in parts {
        if part.width != width || part.height != height {
            return Err(DiffusionError::model("rife concat: extent mismatch"));
        }
        channels += part.channels;
        data.extend_from_slice(&part.data);
    }
    Ok(Planes {
        channels,
        width,
        height,
        data,
    })
}

pub fn leaky_relu_in_place(x: &mut Planes, slope: f32) {
    for value in x.data.iter_mut() {
        if *value < 0.0 {
            *value *= slope;
        }
    }
}

/// `nn.Conv2d(in, out, k, stride, pad)` with zero padding.
pub fn conv2d(x: &Planes, weight: &ConvWeight) -> Result<Planes> {
    if x.channels != weight.in_channels {
        return Err(DiffusionError::model(format!(
            "rife conv2d expects {} channels, got {}",
            weight.in_channels, x.channels
        )));
    }
    let (kw, kh, stride, pad) = (weight.kw, weight.kh, weight.stride, weight.pad);
    let out_w = (x.width + 2 * pad).saturating_sub(kw) / stride + 1;
    let out_h = (x.height + 2 * pad).saturating_sub(kh) / stride + 1;
    let mut out = Planes::new(weight.out_channels, out_w, out_h);
    let in_plane = x.plane();
    let out_plane = out_w * out_h;
    for oc in 0..weight.out_channels {
        let bias = weight.bias[oc];
        for oy in 0..out_h {
            for ox in 0..out_w {
                let mut sum = bias;
                for ic in 0..weight.in_channels {
                    let w_base = ((oc * weight.in_channels) + ic) * kh * kw;
                    let x_base = ic * in_plane;
                    for ky in 0..kh {
                        let iy = (oy * stride + ky) as isize - pad as isize;
                        if iy < 0 || iy >= x.height as isize {
                            continue;
                        }
                        let row = x_base + iy as usize * x.width;
                        for kx in 0..kw {
                            let ix = (ox * stride + kx) as isize - pad as isize;
                            if ix < 0 || ix >= x.width as isize {
                                continue;
                            }
                            sum += x.data[row + ix as usize] * weight.weights[w_base + ky * kw + kx];
                        }
                    }
                }
                out.data[oc * out_plane + oy * out_w + ox] = sum;
            }
        }
    }
    Ok(out)
}

/// `nn.ConvTranspose2d(in, out, k, stride, pad)`, `output_padding = 0`.
pub fn conv_transpose2d(x: &Planes, weight: &DeconvWeight) -> Result<Planes> {
    if x.channels != weight.in_channels {
        return Err(DiffusionError::model(format!(
            "rife conv_transpose2d expects {} channels, got {}",
            weight.in_channels, x.channels
        )));
    }
    let (kw, kh, stride, pad) = (weight.kw, weight.kh, weight.stride, weight.pad);
    let out_w = (x.width - 1) * stride + kw - 2 * pad;
    let out_h = (x.height - 1) * stride + kh - 2 * pad;
    let mut out = Planes::new(weight.out_channels, out_w, out_h);
    let out_plane = out_w * out_h;
    for oc in 0..weight.out_channels {
        let bias = weight.bias[oc];
        for value in out.data[oc * out_plane..(oc + 1) * out_plane].iter_mut() {
            *value = bias;
        }
    }
    let in_plane = x.plane();
    for ic in 0..weight.in_channels {
        for iy in 0..x.height {
            for ix in 0..x.width {
                let value = x.data[ic * in_plane + iy * x.width + ix];
                if value == 0.0 {
                    continue;
                }
                for oc in 0..weight.out_channels {
                    let w_base = ((ic * weight.out_channels) + oc) * kh * kw;
                    for ky in 0..kh {
                        let oy = (iy * stride + ky) as isize - pad as isize;
                        if oy < 0 || oy >= out_h as isize {
                            continue;
                        }
                        let row = oc * out_plane + oy as usize * out_w;
                        for kx in 0..kw {
                            let ox = (ix * stride + kx) as isize - pad as isize;
                            if ox < 0 || ox >= out_w as isize {
                                continue;
                            }
                            out.data[row + ox as usize] +=
                                value * weight.weights[w_base + ky * kw + kx];
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// `nn.PixelShuffle(scale)`.
pub fn pixel_shuffle(x: &Planes, scale: usize) -> Result<Planes> {
    if scale == 0 || x.channels % (scale * scale) != 0 {
        return Err(DiffusionError::model(
            "rife pixel_shuffle: channels not divisible by scale^2",
        ));
    }
    let out_channels = x.channels / (scale * scale);
    let (out_w, out_h) = (x.width * scale, x.height * scale);
    let mut out = Planes::new(out_channels, out_w, out_h);
    let in_plane = x.plane();
    let out_plane = out_w * out_h;
    for c in 0..out_channels {
        for oy in 0..out_h {
            let (iy, ky) = (oy / scale, oy % scale);
            for ox in 0..out_w {
                let (ix, kx) = (ox / scale, ox % scale);
                let feature = (c * scale + ky) * scale + kx;
                out.data[c * out_plane + oy * out_w + ox] =
                    x.data[feature * in_plane + iy * x.width + ix];
            }
        }
    }
    Ok(out)
}

/// `F.interpolate(mode="bilinear")` — PyTorch's default `align_corners=False`.
pub fn resize_bilinear(x: &Planes, out_w: usize, out_h: usize) -> Planes {
    if out_w == x.width && out_h == x.height {
        return x.clone();
    }
    let mut out = Planes::new(x.channels, out_w, out_h);
    let in_plane = x.plane();
    let out_plane = out_w * out_h;
    let sy = x.height as f32 / out_h as f32;
    let sx = x.width as f32 / out_w as f32;
    for oy in 0..out_h {
        let fy = (((oy as f32 + 0.5) * sy) - 0.5).max(0.0);
        let y0 = (fy.floor() as usize).min(x.height - 1);
        let y1 = (y0 + 1).min(x.height - 1);
        let ly = fy - y0 as f32;
        for ox in 0..out_w {
            let fx = (((ox as f32 + 0.5) * sx) - 0.5).max(0.0);
            let x0 = (fx.floor() as usize).min(x.width - 1);
            let x1 = (x0 + 1).min(x.width - 1);
            let lx = fx - x0 as f32;
            for c in 0..x.channels {
                let src = &x.data[c * in_plane..(c + 1) * in_plane];
                let top = src[y0 * x.width + x0] * (1.0 - lx) + src[y0 * x.width + x1] * lx;
                let bot = src[y1 * x.width + x0] * (1.0 - lx) + src[y1 * x.width + x1] * lx;
                out.data[c * out_plane + oy * out_w + ox] = top * (1.0 - ly) + bot * ly;
            }
        }
    }
    out
}

/// RIFE backward warp: sample `x` at `(px + flow_x, py + flow_y)`, bilinear,
/// border-clamped.  `flow` is two planes at the same extent as `x`.
pub fn warp(x: &Planes, flow: &Planes) -> Result<Planes> {
    if flow.channels < 2 || flow.width != x.width || flow.height != x.height {
        return Err(DiffusionError::model("rife warp: flow/extent mismatch"));
    }
    let mut out = Planes::new(x.channels, x.width, x.height);
    let plane = x.plane();
    let (w, h) = (x.width, x.height);
    let max_x = (w - 1) as f32;
    let max_y = (h - 1) as f32;
    for py in 0..h {
        for px in 0..w {
            let index = py * w + px;
            let fx = (px as f32 + flow.data[index]).clamp(0.0, max_x);
            let fy = (py as f32 + flow.data[plane + index]).clamp(0.0, max_y);
            let x0 = fx.floor() as usize;
            let y0 = fy.floor() as usize;
            let x1 = (x0 + 1).min(w - 1);
            let y1 = (y0 + 1).min(h - 1);
            let lx = fx - x0 as f32;
            let ly = fy - y0 as f32;
            for c in 0..x.channels {
                let src = &x.data[c * plane..(c + 1) * plane];
                let top = src[y0 * w + x0] * (1.0 - lx) + src[y0 * w + x1] * lx;
                let bot = src[y1 * w + x0] * (1.0 - lx) + src[y1 * w + x1] * lx;
                out.data[c * plane + index] = top * (1.0 - ly) + bot * ly;
            }
        }
    }
    Ok(out)
}

/// `LeakyReLU(conv(x) * beta + x, 0.2)` — RIFE's `ResConv`.
pub fn res_conv(x: &Planes, weight: &ResConvWeight) -> Result<Planes> {
    let mut out = conv2d(x, &weight.conv)?;
    if weight.beta.len() != out.channels {
        return Err(DiffusionError::model("rife res_conv: beta length mismatch"));
    }
    let plane = out.plane();
    for c in 0..out.channels {
        let beta = weight.beta[c];
        for index in 0..plane {
            let slot = c * plane + index;
            let value = out.data[slot] * beta + x.data[slot];
            out.data[slot] = if value < 0.0 {
                value * RIFE_LRELU_SLOPE
            } else {
                value
            };
        }
    }
    Ok(out)
}

/// `Head` / `encode`: three convs with LeakyReLU, then a transposed conv back
/// to full resolution.  The input is clamped to `[0, 1]` like the reference.
pub fn head_forward(image: &Planes, weights: &HeadWeights) -> Result<Planes> {
    let mut clamped = image.clone();
    for value in clamped.data.iter_mut() {
        *value = value.clamp(0.0, 1.0);
    }
    let mut x = conv2d(&clamped, &weights.cnn0)?;
    leaky_relu_in_place(&mut x, RIFE_LRELU_SLOPE);
    let mut x = conv2d(&x, &weights.cnn1)?;
    leaky_relu_in_place(&mut x, RIFE_LRELU_SLOPE);
    let mut x = conv2d(&x, &weights.cnn2)?;
    leaky_relu_in_place(&mut x, RIFE_LRELU_SLOPE);
    conv_transpose2d(&x, &weights.cnn3)
}

/// One `IFBlock`.  Returns `(flow, mask, feat)` at the input extent.
pub fn if_block_forward(
    x: &Planes,
    flow: Option<&Planes>,
    weights: &IfBlockWeights,
    scale: f32,
) -> Result<(Planes, Planes, Planes)> {
    let (full_w, full_h) = (x.width, x.height);
    let small_w = (full_w as f32 / scale) as usize;
    let small_h = (full_h as f32 / scale) as usize;
    if small_w == 0 || small_h == 0 {
        return Err(DiffusionError::model(
            "rife if_block: canvas too small for the requested scale",
        ));
    }
    let mut input = resize_bilinear(x, small_w, small_h);
    if let Some(flow) = flow {
        let mut down = resize_bilinear(flow, small_w, small_h);
        for value in down.data.iter_mut() {
            *value /= scale;
        }
        input = concat_channels(&[&input, &down])?;
    }
    if input.channels != weights.in_planes {
        return Err(DiffusionError::model(format!(
            "rife if_block expects {} input planes, got {}",
            weights.in_planes, input.channels
        )));
    }
    let mut feat = conv2d(&input, &weights.conv0_a)?;
    leaky_relu_in_place(&mut feat, RIFE_LRELU_SLOPE);
    let mut feat = conv2d(&feat, &weights.conv0_b)?;
    leaky_relu_in_place(&mut feat, RIFE_LRELU_SLOPE);
    for res in &weights.convblock {
        feat = res_conv(&feat, res)?;
    }
    let tmp = conv_transpose2d(&feat, &weights.lastconv)?;
    let tmp = pixel_shuffle(&tmp, 2)?;
    let mut tmp = resize_bilinear(&tmp, full_w, full_h);
    if tmp.channels != RIFE_LASTCONV_PLANES {
        return Err(DiffusionError::model(format!(
            "rife if_block produced {} planes, expected {RIFE_LASTCONV_PLANES}",
            tmp.channels
        )));
    }
    let plane = tmp.plane();
    for value in tmp.data[..4 * plane].iter_mut() {
        *value *= scale;
    }
    let flow_out = tmp.slice_channels(0, 4);
    let mask = tmp.slice_channels(4, 1);
    let feat_out = tmp.slice_channels(5, RIFE_LASTCONV_PLANES - 5);
    Ok((flow_out, mask, feat_out))
}

/// The full IFNet forward on already-padded `[3, ph, pw]` images in `[0, 1]`.
/// Returns the merged intermediate frame, still padded.
pub fn ifnet_forward(
    weights: &RifeModelWeights,
    img0: &Planes,
    img1: &Planes,
    timestep: f32,
    scale_list: &[f32],
) -> Result<Planes> {
    if img0.channels != 3 || img1.channels != 3 {
        return Err(DiffusionError::model("rife ifnet: images must be RGB"));
    }
    if weights.blocks.len() != scale_list.len() {
        return Err(DiffusionError::model(
            "rife ifnet: scale list length must match the block count",
        ));
    }
    let f0 = head_forward(img0, &weights.encode)?;
    let f1 = head_forward(img1, &weights.encode)?;
    let mut time = Planes::new(1, img0.width, img0.height);
    time.data.fill(timestep);

    let mut warped0 = img0.clone();
    let mut warped1 = img1.clone();
    let mut flow: Option<Planes> = None;
    let mut mask = Planes::new(1, img0.width, img0.height);
    let mut feat = Planes::new(RIFE_LASTCONV_PLANES - 5, img0.width, img0.height);

    for (index, block) in weights.blocks.iter().enumerate() {
        let scale = scale_list[index];
        match &flow {
            None => {
                let input = concat_channels(&[img0, img1, &f0, &f1, &time])?;
                let (flow_out, mask_out, feat_out) =
                    if_block_forward(&input, None, block, scale)?;
                flow = Some(flow_out);
                mask = mask_out;
                feat = feat_out;
            }
            Some(current) => {
                let wf0 = warp(&f0, &current.slice_channels(0, 2))?;
                let wf1 = warp(&f1, &current.slice_channels(2, 2))?;
                let input =
                    concat_channels(&[&warped0, &warped1, &wf0, &wf1, &time, &mask, &feat])?;
                let (delta, mask_out, feat_out) =
                    if_block_forward(&input, Some(current), block, scale)?;
                let mut updated = current.clone();
                for (slot, value) in updated.data.iter_mut().zip(delta.data.iter()) {
                    *slot += *value;
                }
                flow = Some(updated);
                mask = mask_out;
                feat = feat_out;
            }
        }
        let current = flow.as_ref().expect("flow set above");
        warped0 = warp(img0, &current.slice_channels(0, 2))?;
        warped1 = warp(img1, &current.slice_channels(2, 2))?;
    }

    let mut merged = Planes::new(3, img0.width, img0.height);
    let plane = merged.plane();
    for index in 0..plane {
        let m = 1.0 / (1.0 + (-mask.data[index]).exp());
        for c in 0..3 {
            merged.data[c * plane + index] =
                warped0.data[c * plane + index] * m + warped1.data[c * plane + index] * (1.0 - m);
        }
    }
    Ok(merged)
}

/// Packs an interleaved RGB8 frame into padded planar `[3, ph, pw]` floats in
/// `[0, 1]`, zero-filling the right/bottom padding (`F.pad` default).
pub fn pack_padded_rgb8(
    pixels: &[u8],
    width: usize,
    height: usize,
    padded_w: usize,
    padded_h: usize,
) -> Planes {
    let mut planes = Planes::new(3, padded_w, padded_h);
    let plane = padded_w * padded_h;
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 3;
            for c in 0..3 {
                planes.data[c * plane + y * padded_w + x] = f32::from(pixels[src + c]) / 255.0;
            }
        }
    }
    planes
}

/// Crops the padded planar result back to `width x height` and quantizes to
/// interleaved RGB8 the way the reference writer does.
pub fn unpack_rgb8(planes: &Planes, width: usize, height: usize) -> Vec<u8> {
    let mut rgb = vec![0u8; width * height * 3];
    let plane = planes.plane();
    for y in 0..height {
        for x in 0..width {
            let src = y * planes.width + x;
            let dst = (y * width + x) * 3;
            for c in 0..3 {
                rgb[dst + c] = (planes.data[c * plane + src].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    rgb
}

/// Portable end-to-end interpolation of one frame pair.
pub fn interpolate_rgb8(
    weights: &RifeModelWeights,
    pair: RifeFramePair<'_>,
    timestep: f32,
    scale: RifeScale,
) -> Result<Vec<u8>> {
    let (padded_w, padded_h) = padded_extent(pair.width, pair.height, scale);
    let img0 = pack_padded_rgb8(pair.frame0, pair.width, pair.height, padded_w, padded_h);
    let img1 = pack_padded_rgb8(pair.frame1, pair.width, pair.height, padded_w, padded_h);
    let merged = ifnet_forward(weights, &img0, &img1, timestep, &scale.scale_list())?;
    Ok(unpack_rgb8(&merged, pair.width, pair.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rife::{
        block_in_planes, RifeBackendKind, Rife, RIFE_ENCODE_CHANNELS, RIFE_NUM_BLOCKS,
    };

    fn conv(out_c: usize, in_c: usize, k: usize, stride: usize, pad: usize, w: Vec<f32>, b: Vec<f32>) -> ConvWeight {
        ConvWeight {
            out_channels: out_c,
            in_channels: in_c,
            kh: k,
            kw: k,
            stride,
            pad,
            weights: w,
            bias: b,
        }
    }

    /// 1x1 kernel, stride 1: a pure per-channel affine — the simplest exact
    /// hand computation.
    #[test]
    fn conv2d_1x1_is_an_affine_over_channels() {
        let x = Planes::from_data(2, 2, 1, vec![1.0, 2.0, 10.0, 20.0]).unwrap();
        // out[0] = 3*in0 + 4*in1 + 1
        let w = conv(1, 2, 1, 1, 0, vec![3.0, 4.0], vec![1.0]);
        let out = conv2d(&x, &w).unwrap();
        assert_eq!(out.channels, 1);
        assert_eq!((out.width, out.height), (2, 1));
        assert_eq!(out.data, vec![3.0 + 40.0 + 1.0, 6.0 + 80.0 + 1.0]);
    }

    /// 3x3 stride 1 pad 1 on a 3x3 ramp with an all-ones kernel: each output
    /// is the sum of the 3x3 zero-padded neighbourhood.
    #[test]
    fn conv2d_3x3_same_padding_sums_the_neighbourhood() {
        let x = Planes::from_data(
            1,
            3,
            3,
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0],
        )
        .unwrap();
        let w = conv(1, 1, 3, 1, 1, vec![1.0; 9], vec![0.0]);
        let out = conv2d(&x, &w).unwrap();
        assert_eq!((out.width, out.height), (3, 3));
        // Top-left sees {1,2,4,5} = 12; centre sees all 45; bottom-right
        // sees {5,6,8,9} = 28.
        assert_eq!(out.data[0], 12.0);
        assert_eq!(out.data[4], 45.0);
        assert_eq!(out.data[8], 28.0);
    }

    /// Stride 2, pad 1, k 3 halves an even extent (the `conv0` shape).
    #[test]
    fn conv2d_stride2_halves_the_extent() {
        let x = Planes::from_data(1, 4, 4, (0..16).map(|v| v as f32).collect()).unwrap();
        let w = conv(1, 1, 3, 2, 1, vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0], vec![0.0]);
        let out = conv2d(&x, &w).unwrap();
        assert_eq!((out.width, out.height), (2, 2));
        // Centre tap at (0,0), (2,0), (0,2), (2,2) of the source.
        assert_eq!(out.data, vec![0.0, 2.0, 8.0, 10.0]);
    }

    /// `ConvTranspose2d(1, 1, 4, 2, 1)` with an all-ones kernel on a 2x2 of
    /// ones: interior taps gather four contributions, edges fewer.
    #[test]
    fn conv_transpose2d_doubles_and_scatters() {
        let x = Planes::from_data(1, 2, 2, vec![1.0, 1.0, 1.0, 1.0]).unwrap();
        let weight = DeconvWeight {
            in_channels: 1,
            out_channels: 1,
            kh: 4,
            kw: 4,
            stride: 2,
            pad: 1,
            weights: vec![1.0; 16],
            bias: vec![0.0],
        };
        let out = conv_transpose2d(&x, &weight).unwrap();
        assert_eq!((out.width, out.height), (4, 4));
        // Hand-computed coverage for k=4, s=2, p=1 on a 2-wide axis: output
        // `o` gathers taps `kx = o + 1 - 2*ix` that land in [0, 4) for some
        // input `ix` in [0, 2), i.e. counts [1, 2, 2, 1]. The 2D pattern is
        // the outer product of that vector with itself.
        let axis = [1.0f32, 2.0, 2.0, 1.0];
        let expected: Vec<f32> = (0..4)
            .flat_map(|y| (0..4).map(move |x| axis[y] * axis[x]))
            .collect();
        assert_eq!(out.data, expected);
    }

    #[test]
    fn conv_transpose2d_bias_lands_on_every_output_pixel() {
        let x = Planes::from_data(1, 2, 2, vec![0.0; 4]).unwrap();
        let weight = DeconvWeight {
            in_channels: 1,
            out_channels: 2,
            kh: 4,
            kw: 4,
            stride: 2,
            pad: 1,
            weights: vec![1.0; 32],
            bias: vec![-1.0, 7.0],
        };
        let out = conv_transpose2d(&x, &weight).unwrap();
        assert_eq!(out.channel(0), &[-1.0; 16]);
        assert_eq!(out.channel(1), &[7.0; 16]);
    }

    #[test]
    fn pixel_shuffle_matches_the_pytorch_permutation() {
        // 4 input channels, 1x1 spatial, scale 2 -> 1 channel, 2x2.
        let x = Planes::from_data(4, 1, 1, vec![10.0, 11.0, 12.0, 13.0]).unwrap();
        let out = pixel_shuffle(&x, 2).unwrap();
        assert_eq!((out.channels, out.width, out.height), (1, 2, 2));
        // out[ky * 2 + kx] == in[(0 * 2 + ky) * 2 + kx]
        assert_eq!(out.data, vec![10.0, 11.0, 12.0, 13.0]);
    }

    #[test]
    fn pixel_shuffle_interleaves_two_output_channels() {
        // 8 channels, 1x1, scale 2 -> 2 channels of 2x2.
        let x = Planes::from_data(8, 1, 1, (0..8).map(|v| v as f32).collect()).unwrap();
        let out = pixel_shuffle(&x, 2).unwrap();
        assert_eq!(out.channel(0), &[0.0, 1.0, 2.0, 3.0]);
        assert_eq!(out.channel(1), &[4.0, 5.0, 6.0, 7.0]);
    }

    /// PyTorch `F.interpolate(align_corners=False)` on a 1x2 -> 1x4 row:
    /// sources are -0.25, 0.25, 0.75, 1.25 clamped to [0, ...].
    #[test]
    fn resize_bilinear_matches_align_corners_false() {
        let x = Planes::from_data(1, 2, 1, vec![0.0, 4.0]).unwrap();
        let out = resize_bilinear(&x, 4, 1);
        assert_eq!(out.data, vec![0.0, 1.0, 3.0, 4.0]);
    }

    #[test]
    fn resize_bilinear_downsample_averages_pairs() {
        let x = Planes::from_data(1, 4, 1, vec![0.0, 2.0, 4.0, 6.0]).unwrap();
        let out = resize_bilinear(&x, 2, 1);
        // Sources 0.5 and 2.5 -> midpoints of (0,2) and (4,6).
        assert_eq!(out.data, vec![1.0, 5.0]);
    }

    #[test]
    fn resize_bilinear_is_identity_at_equal_extent() {
        let x = Planes::from_data(2, 3, 2, (0..12).map(|v| v as f32).collect()).unwrap();
        assert_eq!(resize_bilinear(&x, 3, 2), x);
    }

    #[test]
    fn warp_with_zero_flow_is_the_identity() {
        let x = Planes::from_data(2, 3, 2, (0..12).map(|v| v as f32).collect()).unwrap();
        let flow = Planes::new(2, 3, 2);
        assert_eq!(warp(&x, &flow).unwrap(), x);
    }

    #[test]
    fn warp_shifts_by_whole_pixels_and_clamps_at_the_border() {
        let x = Planes::from_data(1, 4, 1, vec![1.0, 2.0, 3.0, 4.0]).unwrap();
        let mut flow = Planes::new(2, 4, 1);
        flow.data[..4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
        // Sample x+1: [2, 3, 4, clamp -> 4].
        assert_eq!(warp(&x, &flow).unwrap().data, vec![2.0, 3.0, 4.0, 4.0]);
    }

    #[test]
    fn warp_half_pixel_averages_neighbours() {
        let x = Planes::from_data(1, 3, 1, vec![0.0, 10.0, 20.0]).unwrap();
        let mut flow = Planes::new(2, 3, 1);
        flow.data[..3].copy_from_slice(&[0.5, 0.5, -0.5]);
        assert_eq!(warp(&x, &flow).unwrap().data, vec![5.0, 15.0, 15.0]);
    }

    #[test]
    fn warp_is_vertical_too() {
        let x = Planes::from_data(1, 2, 2, vec![0.0, 1.0, 10.0, 11.0]).unwrap();
        let mut flow = Planes::new(2, 2, 2);
        // dy = +1 everywhere.
        flow.data[4..8].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
        assert_eq!(warp(&x, &flow).unwrap().data, vec![10.0, 11.0, 10.0, 11.0]);
    }

    #[test]
    fn res_conv_is_leaky_relu_of_beta_scaled_residual() {
        let x = Planes::from_data(1, 1, 1, vec![2.0]).unwrap();
        let weight = ResConvWeight {
            conv: conv(1, 1, 1, 1, 0, vec![1.0], vec![0.0]),
            beta: vec![-2.0],
        };
        // conv(x) = 2, * beta = -4, + x = -2, leaky(0.2) = -0.4
        let out = res_conv(&x, &weight).unwrap();
        assert!((out.data[0] - (-0.4)).abs() < 1e-6, "{:?}", out.data);
    }

    #[test]
    fn leaky_relu_only_scales_negatives() {
        let mut x = Planes::from_data(1, 3, 1, vec![-1.0, 0.0, 2.0]).unwrap();
        leaky_relu_in_place(&mut x, 0.2);
        assert_eq!(x.data, vec![-0.2, 0.0, 2.0]);
    }

    #[test]
    fn pack_and_unpack_round_trip_through_padding() {
        let pixels: Vec<u8> = (0..(3 * 2 * 3)).map(|v| (v * 7 % 256) as u8).collect();
        let planes = pack_padded_rgb8(&pixels, 3, 2, 8, 8);
        assert_eq!((planes.width, planes.height), (8, 8));
        // The padding column/row is zero.
        assert_eq!(planes.data[7], 0.0);
        assert_eq!(unpack_rgb8(&planes, 3, 2), pixels);
    }

    // -- tiny synthetic IFNet ------------------------------------------------

    /// Deterministic small-magnitude pseudo-random weights: enough to
    /// exercise every op without a checkpoint.
    struct Noise(u32);

    impl Noise {
        fn values(&mut self, count: usize) -> Vec<f32> {
            (0..count)
                .map(|_| {
                    self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((self.0 >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.2
                })
                .collect()
        }

        fn conv(
            &mut self,
            out_c: usize,
            in_c: usize,
            k: usize,
            stride: usize,
            pad: usize,
        ) -> ConvWeight {
            ConvWeight {
                out_channels: out_c,
                in_channels: in_c,
                kh: k,
                kw: k,
                stride,
                pad,
                weights: self.values(out_c * in_c * k * k),
                bias: self.values(out_c),
            }
        }

        /// `ConvTranspose2d(in, out, 4, 2, 1)` — the only deconv shape RIFE
        /// uses.
        fn deconv(&mut self, in_c: usize, out_c: usize) -> DeconvWeight {
            DeconvWeight {
                in_channels: in_c,
                out_channels: out_c,
                kh: 4,
                kw: 4,
                stride: 2,
                pad: 1,
                weights: self.values(in_c * out_c * 16),
                bias: self.values(out_c),
            }
        }
    }

    /// A structurally faithful but tiny IFNet: real plane counts (so every
    /// concat/slice contract is exercised), toy channel widths.
    fn tiny_model() -> RifeModelWeights {
        let mut rng = Noise(12345);
        let encode = HeadWeights {
            cnn0: rng.conv(4, 3, 3, 2, 1),
            cnn1: rng.conv(4, 4, 3, 1, 1),
            cnn2: rng.conv(4, 4, 3, 1, 1),
            cnn3: rng.deconv(4, RIFE_ENCODE_CHANNELS),
        };
        let blocks = (0..RIFE_NUM_BLOCKS)
            .map(|index| {
                let c = 4;
                let in_planes = block_in_planes(index);
                IfBlockWeights {
                    channels: c,
                    in_planes,
                    conv0_a: rng.conv(c / 2, in_planes, 3, 2, 1),
                    conv0_b: rng.conv(c, c / 2, 3, 2, 1),
                    convblock: (0..8)
                        .map(|_| ResConvWeight {
                            conv: rng.conv(c, c, 3, 1, 1),
                            beta: rng.values(c),
                        })
                        .collect(),
                    lastconv: rng.deconv(c, 4 * RIFE_LASTCONV_PLANES),
                }
            })
            .collect();
        RifeModelWeights { encode, blocks }
    }

    fn ramp_frame(width: usize, height: usize, phase: u8) -> Vec<u8> {
        (0..width * height * 3)
            .map(|index| ((index as u32 * 3 + phase as u32 * 17) % 256) as u8)
            .collect()
    }

    /// End-to-end shape/finiteness/determinism gate for the whole graph.
    #[test]
    fn tiny_ifnet_forward_produces_a_frame_of_the_input_size() {
        let model = tiny_model();
        let (w, h) = (40, 30);
        let a = ramp_frame(w, h, 0);
        let b = ramp_frame(w, h, 9);
        let pair = RifeFramePair::new(&a, &b, w, h).unwrap();
        let out = interpolate_rgb8(&model, pair, 0.5, RifeScale::Full).unwrap();
        assert_eq!(out.len(), w * h * 3);
        let again = interpolate_rgb8(&model, pair, 0.5, RifeScale::Full).unwrap();
        assert_eq!(out, again, "the reference forward must be deterministic");
    }

    /// The block graph must actually consume the timestep: two different
    /// timesteps cannot produce the same frame.
    #[test]
    fn tiny_ifnet_forward_depends_on_the_timestep() {
        let model = tiny_model();
        let (w, h) = (40, 30);
        let a = ramp_frame(w, h, 0);
        let b = ramp_frame(w, h, 9);
        let pair = RifeFramePair::new(&a, &b, w, h).unwrap();
        let quarter = interpolate_rgb8(&model, pair, 0.25, RifeScale::Full).unwrap();
        let three_quarters = interpolate_rgb8(&model, pair, 0.75, RifeScale::Full).unwrap();
        assert_ne!(quarter, three_quarters);
    }

    /// Every intermediate plane count in the graph, asserted once: block 0
    /// takes 15 planes, later blocks 28 (24 concatenated + 4 flow), and the
    /// head emits `RIFE_ENCODE_CHANNELS`.
    #[test]
    fn tiny_ifnet_plane_contracts_hold() {
        let model = tiny_model();
        let padded = Planes::new(3, 64, 64);
        let f0 = head_forward(&padded, &model.encode).unwrap();
        assert_eq!(f0.channels, RIFE_ENCODE_CHANNELS);
        assert_eq!((f0.width, f0.height), (64, 64));
        let time = Planes::new(1, 64, 64);
        let first = concat_channels(&[&padded, &padded, &f0, &f0, &time]).unwrap();
        assert_eq!(first.channels, model.blocks[0].in_planes);
        let (flow, mask, feat) =
            if_block_forward(&first, None, &model.blocks[0], 16.0).unwrap();
        assert_eq!((flow.channels, mask.channels, feat.channels), (4, 1, 8));
        assert_eq!((flow.width, flow.height), (64, 64));
        let later =
            concat_channels(&[&padded, &padded, &f0, &f0, &time, &mask, &feat]).unwrap();
        assert_eq!(later.channels + 4, model.blocks[1].in_planes);
    }

    #[test]
    fn interpolate_rejects_out_of_range_timesteps() {
        let model = tiny_model();
        let rife = Rife::from_model_weights(model, RifeBackendKind::Reference);
        let (w, h) = (8, 8);
        let a = ramp_frame(w, h, 0);
        let b = ramp_frame(w, h, 1);
        let pair = RifeFramePair::new(&a, &b, w, h).unwrap();
        assert!(rife.interpolate_rgb8(pair, 0.0).is_err());
        assert!(rife.interpolate_rgb8(pair, 1.0).is_err());
        assert!(rife.interpolate_rgb8(pair, f32::NAN).is_err());
        assert!(rife.interpolate_rgb8(pair, 0.5).is_ok());
    }

    // -- real checkpoint (opt-in) -------------------------------------------

    fn real_model() -> Option<RifeModelWeights> {
        let path = std::env::var("MAKEPAD_RIFE_WEIGHTS").ok()?;
        let weights = crate::rife::RifeWeights::load(&path).expect("load pinned rife checkpoint");
        Some(weights.prepare_model(None).expect("prepare rife model"))
    }

    /// A smooth synthetic scene, optionally translated by `shift` pixels in
    /// x — a motion the flow estimator must resolve exactly.
    fn scene(width: usize, height: usize, shift: isize) -> Vec<u8> {
        let mut rgb = vec![0u8; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                let sx = x as isize - shift;
                let value = |k: f32| -> u8 {
                    let v = 0.5
                        + 0.35 * ((sx as f32 * 0.13 + k) .sin())
                        + 0.12 * ((y as f32 * 0.09 + k * 2.0).cos());
                    (v.clamp(0.0, 1.0) * 255.0).round() as u8
                };
                let dst = (y * width + x) * 3;
                rgb[dst] = value(0.0);
                rgb[dst + 1] = value(1.7);
                rgb[dst + 2] = value(3.1);
            }
        }
        rgb
    }

    fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs() as f64)
            .sum::<f64>()
            / a.len() as f64
    }

    /// The identity gate: two identical frames must interpolate to that same
    /// frame.  A mis-transcribed block order, a wrong plane split or a
    /// swapped warp direction all break this immediately.
    /// `MAKEPAD_RIFE_WEIGHTS=... cargo test --release`.
    #[test]
    fn real_checkpoint_reproduces_a_static_frame() {
        let Some(model) = real_model() else { return };
        let (w, h) = (64, 64);
        let frame = scene(w, h, 0);
        let pair = RifeFramePair::new(&frame, &frame, w, h).unwrap();
        let out = interpolate_rgb8(&model, pair, 0.5, RifeScale::Full).unwrap();
        assert_eq!(out.len(), frame.len());
        let error = mean_abs_diff(&out, &frame);
        // Measured 0.47 levels with the pinned v4.26 checkpoint.
        assert!(
            error < 1.5,
            "static-frame interpolation drifted by {error} levels on average"
        );
    }

    /// The motion gate: a pure 8-pixel translation must interpolate to the
    /// 4-pixel translation, and clearly better than either endpoint.  This
    /// is the check that the flow actually flows.
    #[test]
    fn real_checkpoint_halves_a_pure_translation() {
        let Some(model) = real_model() else { return };
        let (w, h) = (64, 64);
        let frame0 = scene(w, h, 0);
        let frame1 = scene(w, h, 8);
        let expected = scene(w, h, 4);
        let pair = RifeFramePair::new(&frame0, &frame1, w, h).unwrap();
        let out = interpolate_rgb8(&model, pair, 0.5, RifeScale::Full).unwrap();
        // Compare away from the borders, where the synthetic scene has no
        // off-canvas content to warp in.
        let crop = |image: &[u8]| -> Vec<u8> {
            let mut cropped = Vec::new();
            for y in 12..h - 12 {
                for x in 12..w - 12 {
                    cropped.extend_from_slice(&image[(y * w + x) * 3..(y * w + x) * 3 + 3]);
                }
            }
            cropped
        };
        let (out, expected, frame0) = (crop(&out), crop(&expected), crop(&frame0));
        let midpoint_error = mean_abs_diff(&out, &expected);
        let endpoint_error = mean_abs_diff(&frame0, &expected);
        // Measured 1.79 vs 28.33 levels with the pinned v4.26 checkpoint.
        assert!(
            midpoint_error < 4.0 && midpoint_error * 5.0 < endpoint_error,
            "interpolated frame is {midpoint_error} from the true midpoint but the \
             endpoint is only {endpoint_error} — the flow is not being followed"
        );
    }

    #[test]
    fn frame_pair_rejects_mismatched_buffers() {
        assert!(RifeFramePair::new(&[0; 12], &[0; 12], 2, 2).is_ok());
        assert!(RifeFramePair::new(&[0; 12], &[0; 11], 2, 2).is_err());
        assert!(RifeFramePair::new(&[0; 12], &[0; 12], 0, 2).is_err());
    }
}
