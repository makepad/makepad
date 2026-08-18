//! Frozen, tiny numerical taps for the first native CUDA parity gate.
//!
//! These are not an execution fallback. They are immutable inputs and
//! expected outputs that a CUDA-only validation binary uploads, executes, and
//! downloads. Values are deliberately chosen to distinguish exact-erf GEGLU
//! from the tanh approximation and to catch row/column layout mistakes in the
//! UNet timestep broadcast and 3D-RoPE composition.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrozenTensor {
    pub rows: usize,
    pub cols: usize,
    pub values: &'static [f32],
}

impl FrozenTensor {
    pub const fn new(rows: usize, cols: usize, values: &'static [f32]) -> Self {
        Self { rows, cols, values }
    }

    pub fn validate(self) -> Result<(), String> {
        if self
            .rows
            .checked_mul(self.cols)
            .is_none_or(|len| len != self.values.len())
        {
            return Err(format!(
                "frozen tensor shape {}x{} does not match {} values",
                self.rows,
                self.cols,
                self.values.len()
            ));
        }
        if self.values.iter().any(|value| !value.is_finite()) {
            return Err("frozen tensor contains a non-finite value".to_string());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BinaryTap {
    pub name: &'static str,
    pub left: FrozenTensor,
    pub right: FrozenTensor,
    pub expected: FrozenTensor,
    pub atol: f32,
    pub rtol: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnaryTap {
    pub name: &'static str,
    pub input: FrozenTensor,
    pub expected: FrozenTensor,
    pub atol: f32,
    pub rtol: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RopeTap {
    pub name: &'static str,
    pub input: FrozenTensor,
    pub cos: FrozenTensor,
    pub sin: FrozenTensor,
    pub head_count: usize,
    pub expected: FrozenTensor,
    pub atol: f32,
    pub rtol: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttentionTap {
    pub name: &'static str,
    pub q: FrozenTensor,
    pub k: FrozenTensor,
    pub v: FrozenTensor,
    pub head_count: usize,
    pub scale: f32,
    pub expected: FrozenTensor,
    pub atol: f32,
    pub rtol: f32,
}

pub const MUL: BinaryTap = BinaryTap {
    name: "mul_f32_precise",
    left: FrozenTensor::new(2, 3, &[1.0, -2.0, 0.5, 8.0, -0.25, 3.0]),
    right: FrozenTensor::new(2, 3, &[4.0, 0.5, -6.0, -0.125, 16.0, 2.0]),
    expected: FrozenTensor::new(2, 3, &[4.0, -1.0, -3.0, -1.0, -4.0, 6.0]),
    atol: 0.0,
    rtol: 0.0,
};

/// `row_bias` is shaped as a column solely to make its element count explicit;
/// the CUDA API accepts either orientation as long as it has `x.rows` values.
pub const ADD_ROWS_BROADCAST: BinaryTap = BinaryTap {
    name: "add_rows_broadcast",
    left: FrozenTensor::new(3, 2, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
    right: FrozenTensor::new(3, 1, &[-1.0, 0.5, 2.0]),
    expected: FrozenTensor::new(3, 2, &[0.0, 1.0, 3.5, 4.5, 7.0, 8.0]),
    atol: 0.0,
    rtol: 0.0,
};

/// Value-first GEGLU: `[value | gate]`, exact `erf` GELU. At gate ±1 the
/// tanh approximation differs by enough to fail the 2e-6 tolerance.
pub const GEGLU_ERF: UnaryTap = UnaryTap {
    name: "geglu_exact_erf",
    input: FrozenTensor::new(1, 4, &[2.0, -3.0, 1.0, -1.0]),
    expected: FrozenTensor::new(1, 2, &[1.682_689_5, 0.475_965_77]),
    atol: 2.0e-6,
    rtol: 2.0e-6,
};

pub const ROPE_INTERLEAVED: RopeTap = RopeTap {
    name: "rope_interleaved_layout",
    input: FrozenTensor::new(2, 4, &[1.0, 2.0, 3.0, 4.0, 1.0, 2.0, 3.0, 4.0]),
    cos: FrozenTensor::new(2, 2, &[1.0, 1.0, 0.0, 0.0]),
    sin: FrozenTensor::new(2, 2, &[0.0, 0.0, 1.0, 1.0]),
    head_count: 1,
    expected: FrozenTensor::new(2, 4, &[1.0, 2.0, 3.0, 4.0, -2.0, 1.0, -4.0, 3.0]),
    atol: 0.0,
    rtol: 0.0,
};

pub const CROSS_ATTENTION: AttentionTap = AttentionTap {
    name: "cross_attention_q1_kv2",
    q: FrozenTensor::new(1, 2, &[1.0, 0.0]),
    k: FrozenTensor::new(2, 2, &[1.0, 0.0, 0.0, 1.0]),
    v: FrozenTensor::new(2, 2, &[2.0, 4.0, 6.0, 8.0]),
    head_count: 1,
    scale: 1.0,
    expected: FrozenTensor::new(1, 2, &[3.075_765_6, 5.075_765_6]),
    atol: 2.0e-6,
    rtol: 2.0e-6,
};

#[derive(Clone, Debug, PartialEq)]
pub struct TapMismatch {
    pub index: usize,
    pub expected: f32,
    pub actual: f32,
    pub allowed: f32,
}

pub fn compare(
    expected: FrozenTensor,
    actual: &[f32],
    atol: f32,
    rtol: f32,
) -> Result<(), TapMismatch> {
    if actual.len() != expected.values.len() {
        return Err(TapMismatch {
            index: actual.len().min(expected.values.len()),
            expected: expected.values.len() as f32,
            actual: actual.len() as f32,
            allowed: 0.0,
        });
    }
    for (index, (&want, &got)) in expected.values.iter().zip(actual).enumerate() {
        let allowed = atol + rtol * want.abs();
        if !got.is_finite() || (got - want).abs() > allowed {
            return Err(TapMismatch {
                index,
                expected: want,
                actual: got,
                allowed,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frozen_tensor_has_a_valid_shape() {
        for tensor in [
            MUL.left,
            MUL.right,
            MUL.expected,
            ADD_ROWS_BROADCAST.left,
            ADD_ROWS_BROADCAST.right,
            ADD_ROWS_BROADCAST.expected,
            GEGLU_ERF.input,
            GEGLU_ERF.expected,
            ROPE_INTERLEAVED.input,
            ROPE_INTERLEAVED.cos,
            ROPE_INTERLEAVED.sin,
            ROPE_INTERLEAVED.expected,
            CROSS_ATTENTION.q,
            CROSS_ATTENTION.k,
            CROSS_ATTENTION.v,
            CROSS_ATTENTION.expected,
        ] {
            tensor.validate().unwrap();
        }
    }

    #[test]
    fn comparison_reports_the_first_bad_value() {
        let mut actual = MUL.expected.values.to_vec();
        actual[3] += 0.25;
        let mismatch = compare(MUL.expected, &actual, MUL.atol, MUL.rtol).unwrap_err();
        assert_eq!(mismatch.index, 3);
        assert_eq!(mismatch.expected, -1.0);
        assert_eq!(mismatch.actual, -0.75);
    }

    #[test]
    fn exact_erf_fixture_rejects_tanh_geglu() {
        // PyTorch's tanh approximation at gate +1/-1. If the CUDA graph uses
        // the existing tanh-only fused helper, this tap must fail.
        let tanh_approx = [1.682_384, 0.476_575_14];
        assert!(compare(
            GEGLU_ERF.expected,
            &tanh_approx,
            GEGLU_ERF.atol,
            GEGLU_ERF.rtol
        )
        .is_err());
    }

    #[test]
    fn analytical_cross_attention_fixture_is_consistent() {
        let p0 = std::f32::consts::E / (std::f32::consts::E + 1.0);
        let p1 = 1.0 - p0;
        let actual = [p0 * 2.0 + p1 * 6.0, p0 * 4.0 + p1 * 8.0];
        compare(
            CROSS_ATTENTION.expected,
            &actual,
            CROSS_ATTENTION.atol,
            CROSS_ATTENTION.rtol,
        )
        .unwrap();
    }
}

// ---------------------------------------------------------------------------
// Frozen graph-section fixtures: generated inputs + a pure-f32 host reference.
// The reference output digest is pinned; the CUDA section must match the
// reference within tolerance, and the reference itself may never drift.
// ---------------------------------------------------------------------------

/// Pure-f32 planar reference implementations (rows = channels, cols = w*h).
pub mod reference {
    /// stride-1 zero-padded conv2d, planar layout, weights `[cout][cin][k][k]`.
    pub fn conv2d(
        x: &[f32],
        cin: usize,
        width: usize,
        height: usize,
        weights: &[f32],
        bias: &[f32],
        cout: usize,
        k: usize,
        pad: usize,
    ) -> Vec<f32> {
        assert_eq!(x.len(), cin * width * height);
        assert_eq!(weights.len(), cout * cin * k * k);
        assert_eq!(bias.len(), cout);
        let mut out = vec![0.0f32; cout * width * height];
        for oc in 0..cout {
            for oy in 0..height {
                for ox in 0..width {
                    let mut acc = bias[oc];
                    for ic in 0..cin {
                        for ky in 0..k {
                            for kx in 0..k {
                                let iy = oy as isize + ky as isize - pad as isize;
                                let ix = ox as isize + kx as isize - pad as isize;
                                if iy < 0 || ix < 0 || iy >= height as isize || ix >= width as isize {
                                    continue;
                                }
                                let xv = x[ic * width * height + iy as usize * width + ix as usize];
                                let wv = weights[((oc * cin + ic) * k + ky) * k + kx];
                                acc += xv * wv;
                            }
                        }
                    }
                    out[oc * width * height + oy * width + ox] = acc;
                }
            }
        }
        out
    }

    pub fn group_norm(
        x: &[f32],
        channels: usize,
        plane: usize,
        groups: usize,
        gamma: &[f32],
        beta: &[f32],
        eps: f32,
    ) -> Vec<f32> {
        assert_eq!(x.len(), channels * plane);
        assert_eq!(channels % groups, 0);
        let per = channels / groups;
        let mut out = vec![0.0f32; x.len()];
        for g in 0..groups {
            let span = per * plane;
            let start = g * span;
            let mean = x[start..start + span].iter().sum::<f32>() / span as f32;
            let var = x[start..start + span]
                .iter()
                .map(|v| (v - mean) * (v - mean))
                .sum::<f32>()
                / span as f32;
            let inv = 1.0 / (var + eps).sqrt();
            for c in 0..per {
                let ch = g * per + c;
                for p in 0..plane {
                    let at = ch * plane + p;
                    out[at] = (x[at] - mean) * inv * gamma[ch] + beta[ch];
                }
            }
        }
        out
    }

    pub fn silu(x: &[f32]) -> Vec<f32> {
        x.iter().map(|v| v / (1.0 + (-v).exp())).collect()
    }

    /// `x [t, cin] @ w^T + b` with weights pre-rounded to f16 (the device
    /// linear consumes f16 bytes; the reference must share the quantization).
    pub fn linear_nt_f16w(x: &[f32], t: usize, cin: usize, w_f16: &[f32], n: usize, bias: &[f32]) -> Vec<f32> {
        assert_eq!(x.len(), t * cin);
        assert_eq!(w_f16.len(), n * cin);
        assert_eq!(bias.len(), n);
        let mut out = vec![0.0f32; t * n];
        for row in 0..t {
            for o in 0..n {
                let mut acc = 0.0f32;
                for i in 0..cin {
                    acc += x[row * cin + i] * w_f16[o * cin + i];
                }
                out[row * n + o] = acc + bias[o];
            }
        }
        out
    }

    /// The SD ResNet block: gn1 -> silu -> conv1 -> +temb(silu -> f16 linear,
    /// per-channel) -> gn2 -> silu -> conv2 -> + conv_shortcut(x).
    #[allow(clippy::too_many_arguments)]
    pub fn resnet_block(
        x: &[f32],
        inputs: &super::ResnetSectionInputs,
    ) -> Vec<f32> {
        let s = inputs;
        let plane = s.width * s.height;
        let h = group_norm(x, s.cin, plane, s.gn1_groups, &s.gn1_gamma, &s.gn1_beta, 1e-5);
        let h = silu(&h);
        let mut h = conv2d(&h, s.cin, s.width, s.height, &s.conv1_w, &s.conv1_b, s.cout, 3, 1);
        let temb_act = silu(&s.temb);
        let temb_proj = linear_nt_f16w(&temb_act, 1, s.temb_dim, &s.temb_w_f16, s.cout, &s.temb_b);
        for c in 0..s.cout {
            for p in 0..plane {
                h[c * plane + p] += temb_proj[c];
            }
        }
        let h = group_norm(&h, s.cout, plane, s.gn2_groups, &s.gn2_gamma, &s.gn2_beta, 1e-5);
        let h = silu(&h);
        let h = conv2d(&h, s.cout, s.width, s.height, &s.conv2_w, &s.conv2_b, s.cout, 3, 1);
        let shortcut = conv2d(x, s.cin, s.width, s.height, &s.short_w, &s.short_b, s.cout, 1, 0);
        h.iter().zip(shortcut.iter()).map(|(a, b)| a + b).collect()
    }
}

/// Deterministic generated inputs for the ResNet section tap.
pub struct ResnetSectionInputs {
    pub cin: usize,
    pub cout: usize,
    pub width: usize,
    pub height: usize,
    pub gn1_groups: usize,
    pub gn2_groups: usize,
    pub temb_dim: usize,
    pub x: Vec<f32>,
    pub temb: Vec<f32>,
    pub gn1_gamma: Vec<f32>,
    pub gn1_beta: Vec<f32>,
    pub conv1_w: Vec<f32>,
    pub conv1_b: Vec<f32>,
    /// Already rounded through f16 (device parity), stored as f32 values.
    pub temb_w_f16: Vec<f32>,
    pub temb_b: Vec<f32>,
    pub gn2_gamma: Vec<f32>,
    pub gn2_beta: Vec<f32>,
    pub conv2_w: Vec<f32>,
    pub conv2_b: Vec<f32>,
    pub short_w: Vec<f32>,
    pub short_b: Vec<f32>,
}

fn gen(seed: u64, len: usize, scale: f32) -> Vec<f32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            z ^= z >> 31;
            // Map the top 24 bits to [-scale, scale] exactly.
            let unit = (z >> 40) as f32 / (1u64 << 24) as f32;
            (unit * 2.0 - 1.0) * scale
        })
        .collect()
}

pub fn resnet_section_inputs() -> ResnetSectionInputs {
    let (cin, cout, width, height, temb_dim) = (4, 8, 4, 4, 8);
    let temb_w: Vec<f32> = gen(11, cout * temb_dim, 0.5)
        .into_iter()
        .map(crate::cond_assembly::f16_round)
        .collect();
    ResnetSectionInputs {
        cin,
        cout,
        width,
        height,
        gn1_groups: 2,
        gn2_groups: 4,
        temb_dim,
        x: gen(1, cin * width * height, 1.0),
        temb: gen(2, temb_dim, 1.0),
        gn1_gamma: gen(3, cin, 0.5).iter().map(|v| 1.0 + v).collect(),
        gn1_beta: gen(4, cin, 0.2),
        conv1_w: gen(5, cout * cin * 9, 0.3),
        conv1_b: gen(6, cout, 0.1),
        temb_w_f16: temb_w,
        temb_b: gen(12, cout, 0.1),
        gn2_gamma: gen(7, cout, 0.5).iter().map(|v| 1.0 + v).collect(),
        gn2_beta: gen(8, cout, 0.2),
        conv2_w: gen(9, cout * cout * 9, 0.2),
        conv2_b: gen(10, cout, 0.1),
        short_w: gen(13, cout * cin, 0.4),
        short_b: gen(14, cout, 0.1),
    }
}

/// Pinned sha256 of the reference ResNet-section output (f32 LE bytes).
/// Any change to the generators or reference math must be deliberate.
pub const RESNET_SECTION_DIGEST: &str =
    "f389dad8503717795268d67247f08fa098ae3d508fa992ed905a8891e1cbf6fd";

pub fn resnet_section_reference() -> Vec<f32> {
    let inputs = resnet_section_inputs();
    reference::resnet_block(&inputs.x, &inputs)
}

pub fn digest_f32(values: &[f32]) -> String {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    crate::digest::sha256_hex(&bytes)
}

#[cfg(test)]
mod section_tests {
    use super::*;

    #[test]
    fn resnet_section_reference_digest_is_pinned() {
        let out = resnet_section_reference();
        assert_eq!(out.len(), 8 * 16);
        assert!(out.iter().all(|v| v.is_finite()));
        assert_eq!(digest_f32(&out), RESNET_SECTION_DIGEST);
    }

    #[test]
    fn generators_are_deterministic() {
        let a = resnet_section_inputs();
        let b = resnet_section_inputs();
        assert_eq!(a.x, b.x);
        assert_eq!(a.temb_w_f16, b.temb_w_f16);
        // f16 pre-rounding is idempotent (device shares the same values).
        assert!(a
            .temb_w_f16
            .iter()
            .all(|v| crate::cond_assembly::f16_round(*v) == *v));
    }
}
