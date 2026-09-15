//! SD AutoencoderKL (the Hunyuan paint VAE) on the Makepad CUDA planar ops.
//!
//! Encode uses the posterior **mean** (never `sample`) so the oracle and the
//! native path share a deterministic latent. Downsamplers are the official
//! 3x3 stride-2 with extra right/bottom zero pad.

use crate::cuda_unet::Planar;
use crate::torch_bin::{self, TensorRecord, TorchDtype};
use makepad_ai_common::backend::cuda::{
    gpu_add, gpu_attention_planar_single, gpu_conv2d_planar_cached, gpu_conv2d_planar_strided,
    gpu_device_available, gpu_download, gpu_group_norm_planar, gpu_silu, gpu_upload,
    gpu_upsample_nearest2x, GpuTensor,
};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub const SCALE: f32 = 0.18215;
pub const GN_EPS: f32 = 1e-6;
pub const GN_GROUPS: usize = 32;
const NS: &str = "paint-vae";

struct W {
    data: Vec<f32>,
    shape: Vec<usize>,
}

impl W {
    fn as_slice(&self) -> &[f32] {
        &self.data
    }
}

pub struct SdVae {
    w: HashMap<String, W>,
}

fn f32_from_record(record: &TensorRecord, raw: &[u8]) -> Result<Vec<f32>, String> {
    match record.dtype {
        TorchDtype::F32 => {
            if raw.len() != record.numel * 4 {
                return Err(format!("{} f32 byte length", record.name));
            }
            Ok(raw
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect())
        }
        TorchDtype::F16 => {
            if raw.len() != record.numel * 2 {
                return Err(format!("{} f16 byte length", record.name));
            }
            Ok(raw
                .chunks_exact(2)
                .map(|b| {
                    let bits = u16::from_le_bytes([b[0], b[1]]);
                    half_to_f32(bits)
                })
                .collect())
        }
        other => Err(format!("{} unsupported dtype {other:?}", record.name)),
    }
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 1) as u32;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let man = (bits & 0x3ff) as u32;
    let out = if exp == 0 {
        if man == 0 {
            sign << 31
        } else {
            let mut m = man;
            let mut e = 127 - 15 + 1;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            (sign << 31) | (e << 23) | ((m & 0x3ff) << 13)
        }
    } else if exp == 31 {
        (sign << 31) | (0xff << 23) | (man << 13)
    } else {
        (sign << 31) | ((exp + (127 - 15)) << 23) | (man << 13)
    };
    f32::from_bits(out)
}

impl SdVae {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !gpu_device_available() {
            return Err("CUDA unavailable".into());
        }
        let mut file = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
        let index = torch_bin::read_index_from(&mut file).map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        let mut w = HashMap::new();
        for record in &index.tensors {
            if !(record.name.starts_with("encoder.")
                || record.name.starts_with("quant_conv.")
                || record.name.starts_with("decoder.")
                || record.name.starts_with("post_quant_conv."))
            {
                continue;
            }
            index
                .read_tensor_into(&mut file, record, &mut raw)
                .map_err(|e| e.to_string())?;
            let data = f32_from_record(record, &raw)?;
            w.insert(
                record.name.clone(),
                W {
                    data,
                    shape: record.shape.clone(),
                },
            );
        }
        if !w.contains_key("encoder.conv_in.weight") || !w.contains_key("quant_conv.weight") {
            return Err("VAE archive missing encoder.conv_in / quant_conv".into());
        }
        Ok(Self { w })
    }

    fn get(&self, name: &str) -> Result<&W, String> {
        self.w
            .get(name)
            .ok_or_else(|| format!("missing VAE tensor {name}"))
    }

    /// Official .bin still uses the pre-diffusers `query/key/value/proj_attn`
    /// names; newer dumps use `to_q/to_k/to_v/to_out.0`.
    fn get_any(&self, names: &[&str]) -> Result<&W, String> {
        for name in names {
            if let Some(w) = self.w.get(*name) {
                return Ok(w);
            }
        }
        Err(format!("missing VAE tensor (tried {})", names.join(", ")))
    }

    fn conv(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
        kw: usize,
        pad: usize,
    ) -> Result<GpuTensor, String> {
        let weight = self.get(&format!("{prefix}.weight"))?;
        let bias = self.get(&format!("{prefix}.bias"))?;
        let oc = weight.shape[0];
        gpu_conv2d_planar_cached(
            x,
            width,
            height,
            NS,
            prefix,
            weight.as_slice(),
            bias.as_slice(),
            oc,
            kw,
            kw,
            pad,
            pad,
        )
    }

    fn downsample(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
    ) -> Result<(GpuTensor, usize, usize), String> {
        // Official: F.pad((0,1,0,1)) then conv stride-2 pad-0. In-kernel OOB
        // zeros implement the extra right/bottom pad when pad=0 and we allow
        // the 3x3 window to walk one pixel past H/W.
        let weight = self.get(&format!("{prefix}.weight"))?;
        let bias = self.get(&format!("{prefix}.bias"))?;
        let oc = weight.shape[0];
        let out_w = width / 2;
        let out_h = height / 2;
        let y = gpu_conv2d_planar_strided(
            x,
            width,
            height,
            out_w,
            out_h,
            NS,
            prefix,
            weight.as_slice(),
            bias.as_slice(),
            oc,
            3,
            3,
            0,
            0,
            2,
            2,
        )?;
        Ok((y, out_w, out_h))
    }

    fn resnet(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
    ) -> Result<GpuTensor, String> {
        let cin = x.rows();
        let n1w = self.get(&format!("{prefix}.norm1.weight"))?;
        let n1b = self.get(&format!("{prefix}.norm1.bias"))?;
        let h = gpu_group_norm_planar(
            x,
            width,
            height,
            GN_GROUPS,
            NS,
            &format!("{prefix}.norm1"),
            n1w.as_slice(),
            n1b.as_slice(),
            GN_EPS,
        )?;
        let h = gpu_silu(&h)?;
        let h = self.conv(&h, width, height, &format!("{prefix}.conv1"), 3, 1)?;
        let n2w = self.get(&format!("{prefix}.norm2.weight"))?;
        let n2b = self.get(&format!("{prefix}.norm2.bias"))?;
        let h = gpu_group_norm_planar(
            &h,
            width,
            height,
            GN_GROUPS,
            NS,
            &format!("{prefix}.norm2"),
            n2w.as_slice(),
            n2b.as_slice(),
            GN_EPS,
        )?;
        let h = gpu_silu(&h)?;
        let h = self.conv(&h, width, height, &format!("{prefix}.conv2"), 3, 1)?;
        if self.w.contains_key(&format!("{prefix}.conv_shortcut.weight")) {
            let skip = self.conv(x, width, height, &format!("{prefix}.conv_shortcut"), 1, 0)?;
            gpu_add(&h, &skip)
        } else if h.rows() != cin {
            Err(format!("{prefix} channel change without shortcut"))
        } else {
            gpu_add(&h, x)
        }
    }

    fn mid_attn(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
    ) -> Result<GpuTensor, String> {
        let gn_w = self.get(&format!("{prefix}.group_norm.weight"))?;
        let gn_b = self.get(&format!("{prefix}.group_norm.bias"))?;
        let h = gpu_group_norm_planar(
            x,
            width,
            height,
            GN_GROUPS,
            NS,
            &format!("{prefix}.gn"),
            gn_w.as_slice(),
            gn_b.as_slice(),
            GN_EPS,
        )?;
        let q = self.linear_named(&h, width, height, prefix, &["to_q", "query"])?;
        let k = self.linear_named(&h, width, height, prefix, &["to_k", "key"])?;
        let v = self.linear_named(&h, width, height, prefix, &["to_v", "value"])?;
        let scale = 1.0 / (q.rows() as f32).sqrt();
        let attn = gpu_attention_planar_single(&q, &k, &v, scale)?;
        let proj = self.linear_named(&attn, width, height, prefix, &["to_out.0", "proj_attn"])?;
        gpu_add(x, &proj)
    }

    fn linear_named(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
        suffixes: &[&str],
    ) -> Result<GpuTensor, String> {
        let weight_names: Vec<String> = suffixes
            .iter()
            .map(|s| format!("{prefix}.{s}.weight"))
            .collect();
        let bias_names: Vec<String> = suffixes
            .iter()
            .map(|s| format!("{prefix}.{s}.bias"))
            .collect();
        let weight_refs: Vec<&str> = weight_names.iter().map(String::as_str).collect();
        let bias_refs: Vec<&str> = bias_names.iter().map(String::as_str).collect();
        let weight = self.get_any(&weight_refs)?;
        let bias = self.get_any(&bias_refs)?;
        gpu_conv2d_planar_cached(
            x,
            width,
            height,
            NS,
            &weight_names[0],
            weight.as_slice(),
            bias.as_slice(),
            weight.shape[0],
            1,
            1,
            0,
            0,
        )
    }

    /// Encode RGB [0,1] planar (3 x H*W) to scaled latent mean (4 x h*w).
    pub fn encode_mean(
        &self,
        rgb01: &[f32],
        width: usize,
        height: usize,
    ) -> Result<Planar, String> {
        if rgb01.len() != 3 * width * height {
            return Err("VAE encode expects planar RGB".into());
        }
        let x01: Vec<f32> = rgb01.iter().map(|v| v * 2.0 - 1.0).collect();
        let mut h = gpu_upload(&x01, 3, width * height)?;
        let mut w = width;
        let mut ht = height;
        h = self.conv(&h, w, ht, "encoder.conv_in", 3, 1)?;
        for block in 0..4 {
            for r in 0..2 {
                h = self.resnet(&h, w, ht, &format!("encoder.down_blocks.{block}.resnets.{r}"))?;
            }
            if block < 3 {
                let (y, nw, nh) =
                    self.downsample(&h, w, ht, &format!("encoder.down_blocks.{block}.downsamplers.0.conv"))?;
                h = y;
                w = nw;
                ht = nh;
            }
        }
        h = self.resnet(&h, w, ht, "encoder.mid_block.resnets.0")?;
        h = self.mid_attn(&h, w, ht, "encoder.mid_block.attentions.0")?;
        h = self.resnet(&h, w, ht, "encoder.mid_block.resnets.1")?;
        let nw = self.get("encoder.conv_norm_out.weight")?;
        let nb = self.get("encoder.conv_norm_out.bias")?;
        h = gpu_group_norm_planar(
            &h,
            w,
            ht,
            GN_GROUPS,
            NS,
            "encoder.conv_norm_out",
            nw.as_slice(),
            nb.as_slice(),
            GN_EPS,
        )?;
        h = gpu_silu(&h)?;
        h = self.conv(&h, w, ht, "encoder.conv_out", 3, 1)?;
        h = self.conv(&h, w, ht, "quant_conv", 1, 0)?;
        // Posterior mean is the first 4 channels of the 8-ch quant_conv.
        let moments = gpu_download(&h)?;
        let plane = w * ht;
        let mean = &moments[..4 * plane];
        let scaled: Vec<f32> = mean.iter().map(|v| v * SCALE).collect();
        let t = gpu_upload(&scaled, 4, plane)?;
        Ok(Planar {
            t,
            width: w,
            height: ht,
        })
    }

    /// Decode a scaled latent (4 x h*w) to RGB [0,1] planar (3 x H*W).
    pub fn decode_rgb01(
        &self,
        scaled_latent: &[f32],
        lat_w: usize,
        lat_h: usize,
    ) -> Result<(Vec<f32>, usize, usize), String> {
        if scaled_latent.len() != 4 * lat_w * lat_h {
            return Err("VAE decode expects 4-ch planar latent".into());
        }
        let unscaled: Vec<f32> = scaled_latent.iter().map(|v| v / SCALE).collect();
        let mut h = gpu_upload(&unscaled, 4, lat_w * lat_h)?;
        let mut w = lat_w;
        let mut ht = lat_h;
        h = self.conv(&h, w, ht, "post_quant_conv", 1, 0)?;
        h = self.conv(&h, w, ht, "decoder.conv_in", 3, 1)?;
        h = self.resnet(&h, w, ht, "decoder.mid_block.resnets.0")?;
        h = self.mid_attn(&h, w, ht, "decoder.mid_block.attentions.0")?;
        h = self.resnet(&h, w, ht, "decoder.mid_block.resnets.1")?;
        for block in 0..4 {
            for r in 0..3 {
                h = self.resnet(&h, w, ht, &format!("decoder.up_blocks.{block}.resnets.{r}"))?;
            }
            if block < 3 {
                h = gpu_upsample_nearest2x(&h, w, ht)?;
                w *= 2;
                ht *= 2;
                h = self.conv(
                    &h,
                    w,
                    ht,
                    &format!("decoder.up_blocks.{block}.upsamplers.0.conv"),
                    3,
                    1,
                )?;
            }
        }
        let nw = self.get("decoder.conv_norm_out.weight")?;
        let nb = self.get("decoder.conv_norm_out.bias")?;
        h = gpu_group_norm_planar(
            &h,
            w,
            ht,
            GN_GROUPS,
            NS,
            "decoder.conv_norm_out",
            nw.as_slice(),
            nb.as_slice(),
            GN_EPS,
        )?;
        h = gpu_silu(&h)?;
        h = self.conv(&h, w, ht, "decoder.conv_out", 3, 1)?;
        let raw = gpu_download(&h)?;
        let rgb: Vec<f32> = raw.iter().map(|v| (v * 0.5 + 0.5).clamp(0.0, 1.0)).collect();
        Ok((rgb, w, ht))
    }

    pub fn encode_mean_nchw(
        &self,
        rgb01_nchw: &[f32],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, String> {
        // NCHW RGB -> planar [c][y*x]
        if rgb01_nchw.len() != 3 * width * height {
            return Err("nchw size".into());
        }
        let mut planar = vec![0.0f32; 3 * width * height];
        for c in 0..3 {
            for i in 0..width * height {
                planar[c * width * height + i] = rgb01_nchw[c * width * height + i];
            }
        }
        let out = self.encode_mean(&planar, width, height)?;
        gpu_download(&out.t)
    }
}

/// Same deterministic ramp the official oracle uses.
pub fn ramp_rgb_nchw(width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; 3 * width * height];
    for y in 0..height {
        for x in 0..width {
            let xf = x as f32 / (width - 1).max(1) as f32;
            let yf = y as f32 / (height - 1).max(1) as f32;
            let i = y * width + x;
            out[i] = 0.5 * (xf + yf);
            out[width * height + i] = xf;
            out[2 * width * height + i] = yf;
        }
    }
    out
}
