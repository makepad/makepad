//! The CPU reference executor: plain loops, the oracle for the Vulkan kernels
//! and the fallback where no Vulkan device exists.

use crate::ir::{Act, Graph, Op};

/// A feature map `[c, h, w]`, row-major per channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Map {
    pub c: usize,
    pub h: usize,
    pub w: usize,
    pub data: Vec<f32>,
}

impl Map {
    pub fn zeros(c: usize, h: usize, w: usize) -> Map {
        Map { c, h, w, data: vec![0.0; c * h * w] }
    }
    fn at(&self, c: usize, y: usize, x: usize) -> f32 {
        self.data[(c * self.h + y) * self.w + x]
    }
}

/// Output size of a convolution / pooling window along one axis.
pub fn out_size(size: usize, kernel: usize, stride: usize, pad_lo: usize, pad_hi: usize) -> Result<usize, String> {
    let padded = size + pad_lo + pad_hi;
    if padded < kernel || stride == 0 {
        return Err(format!("window {kernel} (stride {stride}) does not fit {size} + padding"));
    }
    Ok((padded - kernel) / stride + 1)
}

#[allow(clippy::too_many_arguments)]
pub fn conv(x: &Map, w: &[f32], bias: Option<&[f32]>, out_c: usize, kernel: (usize, usize), stride: (usize, usize), pad: [usize; 4], groups: usize, act: Act) -> Result<Map, String> {
    if x.c % groups != 0 || out_c % groups != 0 {
        return Err(format!("conv groups {groups} do not divide channels {} -> {out_c}", x.c));
    }
    let (kh, kw) = kernel;
    let in_per = x.c / groups;
    let out_per = out_c / groups;
    if w.len() != out_c * in_per * kh * kw {
        return Err("conv weight size".into());
    }
    let oh = out_size(x.h, kh, stride.0, pad[0], pad[2])?;
    let ow = out_size(x.w, kw, stride.1, pad[1], pad[3])?;
    let mut out = Map::zeros(out_c, oh, ow);
    for oc in 0..out_c {
        let g = oc / out_per;
        let b = bias.map_or(0.0, |b| b[oc]);
        for oy in 0..oh {
            for ox in 0..ow {
                let mut acc = b;
                for ic in 0..in_per {
                    let c = g * in_per + ic;
                    for ky in 0..kh {
                        let iy = (oy * stride.0 + ky) as isize - pad[0] as isize;
                        if iy < 0 || iy >= x.h as isize {
                            continue;
                        }
                        for kx in 0..kw {
                            let ix = (ox * stride.1 + kx) as isize - pad[1] as isize;
                            if ix < 0 || ix >= x.w as isize {
                                continue;
                            }
                            acc += w[((oc * in_per + ic) * kh + ky) * kw + kx] * x.at(c, iy as usize, ix as usize);
                        }
                    }
                }
                out.data[(oc * oh + oy) * ow + ox] = act.apply(acc);
            }
        }
    }
    Ok(out)
}

/// Transposed convolution without padding, weights `[in, out, kh, kw]`.
pub fn conv_transpose(x: &Map, w: &[f32], bias: Option<&[f32]>, out_c: usize, kernel: (usize, usize), stride: (usize, usize), act: Act) -> Result<Map, String> {
    let (kh, kw) = kernel;
    if w.len() != x.c * out_c * kh * kw {
        return Err("transposed conv weight size".into());
    }
    let oh = (x.h - 1) * stride.0 + kh;
    let ow = (x.w - 1) * stride.1 + kw;
    let mut out = Map::zeros(out_c, oh, ow);
    for oc in 0..out_c {
        let b = bias.map_or(0.0, |b| b[oc]);
        out.data[oc * oh * ow..(oc + 1) * oh * ow].iter_mut().for_each(|v| *v = b);
    }
    for ic in 0..x.c {
        for iy in 0..x.h {
            for ix in 0..x.w {
                let v = x.at(ic, iy, ix);
                for oc in 0..out_c {
                    for ky in 0..kh {
                        for kx in 0..kw {
                            let (oy, ox) = (iy * stride.0 + ky, ix * stride.1 + kx);
                            out.data[(oc * oh + oy) * ow + ox] += v * w[((ic * out_c + oc) * kh + ky) * kw + kx];
                        }
                    }
                }
            }
        }
    }
    out.data.iter_mut().for_each(|v| *v = act.apply(*v));
    Ok(out)
}

fn nearest(x: &Map, oh: usize, ow: usize) -> Map {
    let mut out = Map::zeros(x.c, oh, ow);
    for c in 0..x.c {
        for oy in 0..oh {
            let iy = oy * x.h / oh;
            for ox in 0..ow {
                let ix = ox * x.w / ow;
                out.data[(c * oh + oy) * ow + ox] = x.at(c, iy, ix);
            }
        }
    }
    out
}

fn pool(x: &Map, k: usize, stride: usize, pad: usize, max: bool) -> Result<Map, String> {
    let oh = out_size(x.h, k, stride, pad, pad)?;
    let ow = out_size(x.w, k, stride, pad, pad)?;
    let mut out = Map::zeros(x.c, oh, ow);
    for c in 0..x.c {
        for oy in 0..oh {
            for ox in 0..ow {
                let (mut acc, mut n) = (if max { f32::NEG_INFINITY } else { 0.0 }, 0usize);
                for ky in 0..k {
                    for kx in 0..k {
                        let iy = (oy * stride + ky) as isize - pad as isize;
                        let ix = (ox * stride + kx) as isize - pad as isize;
                        if iy < 0 || ix < 0 || iy >= x.h as isize || ix >= x.w as isize {
                            continue;
                        }
                        let v = x.at(c, iy as usize, ix as usize);
                        acc = if max { acc.max(v) } else { acc + v };
                        n += 1;
                    }
                }
                out.data[(c * oh + oy) * ow + ox] = if max { acc } else { acc / n.max(1) as f32 };
            }
        }
    }
    Ok(out)
}

/// Runs the whole graph on `input` (`[3, H, W]`); returns every value's last
/// use freed early, and the final map.
pub fn run(graph: &Graph, input: Map) -> Result<Map, String> {
    let n = graph.ops.len();
    // Free each value after its last reader.
    let mut last_use = vec![0usize; n + 1];
    for (i, op) in graph.ops.iter().enumerate() {
        for v in op.inputs() {
            last_use[v] = i + 1;
        }
    }
    let mut values: Vec<Option<Map>> = vec![None; n + 1];
    values[0] = Some(input);
    for (i, op) in graph.ops.iter().enumerate() {
        let get = |v: usize| values[v].as_ref().ok_or_else(|| format!("op {i}: value {v} was freed or never computed"));
        let w = |t: usize| graph.weights[t].data.as_slice();
        let out = match op {
            Op::Conv { input, weight, bias, out_channels, kernel, stride, pad, groups, act } => {
                conv(get(*input)?, w(*weight), bias.map(w), *out_channels, *kernel, *stride, *pad, *groups, *act)?
            }
            Op::ConvTranspose { input, weight, bias, out_channels, kernel, stride, act } => {
                conv_transpose(get(*input)?, w(*weight), bias.map(w), *out_channels, *kernel, *stride, *act)?
            }
            Op::Add { a, b, act } => {
                let (a, b) = (get(*a)?, get(*b)?);
                if (a.c, a.h, a.w) != (b.c, b.h, b.w) {
                    return Err(format!("op {i}: add of {}x{}x{} and {}x{}x{}", a.c, a.h, a.w, b.c, b.h, b.w));
                }
                Map { c: a.c, h: a.h, w: a.w, data: a.data.iter().zip(&b.data).map(|(x, y)| act.apply(x + y)).collect() }
            }
            Op::Mul { x, s } => {
                let (x, s) = (get(*x)?, get(*s)?);
                let mut out = x.clone();
                if (s.h, s.w) == (1, 1) && s.c == x.c {
                    for c in 0..x.c {
                        out.data[c * x.h * x.w..(c + 1) * x.h * x.w].iter_mut().for_each(|v| *v *= s.data[c]);
                    }
                } else if (s.c, s.h, s.w) == (x.c, x.h, x.w) {
                    out.data.iter_mut().zip(&s.data).for_each(|(v, m)| *v *= m);
                } else {
                    return Err(format!("op {i}: mul shapes"));
                }
                out
            }
            Op::GlobalAvgPool { input } => {
                let x = get(*input)?;
                let hw = (x.h * x.w) as f32;
                Map { c: x.c, h: 1, w: 1, data: x.data.chunks(x.h * x.w).map(|c| c.iter().sum::<f32>() / hw).collect() }
            }
            Op::Pool { input, kernel, stride, pad, max } => pool(get(*input)?, *kernel, *stride, *pad, *max)?,
            Op::UpsampleLike { input, like } => {
                let like = get(*like)?;
                nearest(get(*input)?, like.h, like.w)
            }
            Op::Upsample { input, scale } => {
                let x = get(*input)?;
                nearest(x, x.h * scale, x.w * scale)
            }
            Op::Concat { inputs } => {
                let first = get(inputs[0])?;
                let (h, w) = (first.h, first.w);
                let mut out = Map { c: 0, h, w, data: Vec::new() };
                for v in inputs {
                    let m = get(*v)?;
                    if (m.h, m.w) != (h, w) {
                        return Err(format!("op {i}: concat of different sizes"));
                    }
                    out.c += m.c;
                    out.data.extend_from_slice(&m.data);
                }
                out
            }
            Op::Activate { input, act } => {
                let x = get(*input)?;
                Map { c: x.c, h: x.h, w: x.w, data: x.data.iter().map(|v| act.apply(*v)).collect() }
            }
        };
        for v in op.inputs() {
            if last_use[v] == i + 1 && v != 0 {
                values[v] = None;
            }
        }
        values[i + 1] = Some(out);
    }
    values[n].take().ok_or_else(|| "the graph produced no output".to_string())
}
