//! DINO feature projector (`unet.image_proj_model_dino`) from the pinned
//! Hunyuan3D-Paint-2.1 UNet: Linear(1536 -> 4*1024) + LayerNorm(1024).
//! Used after DINOv2-giant; this file is only the projector.

use crate::torch_bin::{self, TorchDtype};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub const DINO_DIM: usize = 1536;
pub const TOKEN_DIM: usize = 1024;
pub const TOKEN_COUNT: usize = 4;

pub struct DinoProj {
    weight: Vec<f32>, // [4*1024, 1536]
    bias: Vec<f32>,   // [4*1024]
    ln_w: Vec<f32>,   // [1024]
    ln_b: Vec<f32>,   // [1024]
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

fn load_f32(record: &crate::torch_bin::TensorRecord, raw: &[u8]) -> Result<Vec<f32>, String> {
    match record.dtype {
        TorchDtype::F32 => Ok(raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect()),
        TorchDtype::F16 => Ok(raw
            .chunks_exact(2)
            .map(|b| half_to_f32(u16::from_le_bytes([b[0], b[1]])))
            .collect()),
        other => Err(format!("{} {other:?}", record.name)),
    }
}

impl DinoProj {
    pub fn load_from_unet_bin(path: &Path) -> Result<Self, String> {
        let mut file = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
        let index = torch_bin::read_index_from(&mut file).map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        let mut take = |name: &str| -> Result<Vec<f32>, String> {
            let rec = index
                .find(name)
                .ok_or_else(|| format!("missing {name}"))?;
            index
                .read_tensor_into(&mut file, rec, &mut raw)
                .map_err(|e| e.to_string())?;
            load_f32(rec, &raw)
        };
        Ok(Self {
            weight: take("unet.image_proj_model_dino.proj.weight")?,
            bias: take("unet.image_proj_model_dino.proj.bias")?,
            ln_w: take("unet.image_proj_model_dino.norm.weight")?,
            ln_b: take("unet.image_proj_model_dino.norm.bias")?,
        })
    }

    /// `image_embeds` is `[tokens_in, 1536]` (usually the flattened DINO
    /// sequence reduced to one 1536 vector per batch item, or many).
    /// Returns `[tokens_in, 4, 1024]`.
    pub fn forward(&self, image_embeds: &[f32], rows: usize) -> Result<Vec<f32>, String> {
        if image_embeds.len() != rows * DINO_DIM {
            return Err(format!(
                "dino proj input {} vs {}x{}",
                image_embeds.len(),
                rows,
                DINO_DIM
            ));
        }
        let out_f = TOKEN_COUNT * TOKEN_DIM;
        let mut projected = vec![0.0f32; rows * out_f];
        for r in 0..rows {
            let src = &image_embeds[r * DINO_DIM..(r + 1) * DINO_DIM];
            for o in 0..out_f {
                let mut acc = self.bias[o];
                let wrow = &self.weight[o * DINO_DIM..(o + 1) * DINO_DIM];
                for i in 0..DINO_DIM {
                    acc += src[i] * wrow[i];
                }
                projected[r * out_f + o] = acc;
            }
        }
        // LayerNorm over last dim 1024, per (row, token)
        let mut out = projected;
        for slot in out.chunks_exact_mut(TOKEN_DIM) {
            let mean = slot.iter().sum::<f32>() / TOKEN_DIM as f32;
            let var = slot.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / TOKEN_DIM as f32;
            let inv = 1.0 / (var + 1e-5).sqrt();
            for i in 0..TOKEN_DIM {
                slot[i] = (slot[i] - mean) * inv * self.ln_w[i] + self.ln_b[i];
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projector_shapes_are_pinned() {
        assert_eq!(TOKEN_COUNT * TOKEN_DIM, 4096);
        assert_eq!(DINO_DIM, 1536);
    }
}
