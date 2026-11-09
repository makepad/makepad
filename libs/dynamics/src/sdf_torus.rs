//! Torus signed-distance field.
//!
//! Translated from `plugin/sdf/torus.cc` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2022 DeepMind Technologies Limited);
//! changed: translated to Rust; Sdf and model attribute storage replace plugin registration.

use crate::model::Model;
use crate::sdf::Sdf;
use crate::types::MINVAL;

pub struct Torus {
    pub radius: [f64; 2],
}

fn distance(p: &[f64; 3], radius: &[f64; 2]) -> f64 {
    let q = (p[0] * p[0] + p[1] * p[1]).sqrt() - radius[0];
    (q * q + p[2] * p[2]).sqrt() - radius[1]
}

impl Torus {
    /// Read radius1, radius2 in TorusAttribute::names order.
    pub fn from_plugin(m: &Model, instance: usize) -> Result<Torus, String> {
        if instance >= m.nplugin {
            return Err("Invalid Torus plugin instance".into());
        }
        let mut offset = *m
            .plugin_attradr
            .get(instance)
            .ok_or("Invalid Torus plugin attribute address")? as usize;
        let mut radius = [0.35, 0.15];
        for i in 0..2 {
            // PluginAttrSeek counts NUL terminators starting at plugin_attradr.
            let value = m
                .plugin_attr
                .get(offset..)
                .ok_or("Invalid Torus plugin attribute address")?;
            let len = value
                .iter()
                .position(|&c| c == 0)
                .ok_or("Unterminated Torus plugin attribute")?;
            let value = &value[..=len];
            // CheckAttr and SdfDefault::GetDefault use these six ASCII whitespace bytes.
            let whitespace = |c: u8| b" \t\n\x0b\x0c\r".contains(&c);
            if !value[..len].iter().all(|&c| whitespace(c)) {
                // Use the same C-library conversion, including hex floats, infinities,
                // NaNs, underflow and overflow, rather than Rust's different grammar.
                unsafe extern "C" {
                    fn strtod(s: *const std::ffi::c_char, end: *mut *mut std::ffi::c_char) -> f64;
                }
                let start = value.as_ptr().cast::<std::ffi::c_char>();
                let mut end = std::ptr::null_mut();
                // SAFETY: value includes its NUL terminator and strtod only reads it;
                // end is an output pointer into that same slice.
                let num = unsafe { strtod(start, &mut end) };
                let consumed = unsafe { end.offset_from(start) } as usize;
                if consumed == 0 || !value[consumed..len].iter().all(|&c| whitespace(c)) {
                    return Err("Invalid radius1 or radius2 parameters in Torus plugin".into());
                }
                radius[i] = num;
            }
            offset += len + 1;
        }
        Ok(Torus { radius })
    }
}

impl Sdf for Torus {
    fn distance(&self, point: &[f64; 3]) -> f64 {
        distance(point, &self.radius)
    }

    fn gradient(&self, grad: &mut [f64; 3], p: &[f64; 3]) {
        let len_xy = (p[0] * p[0] + p[1] * p[1]).sqrt();
        let q = len_xy - self.radius[0];
        let grad_q = [
            if len_xy >= MINVAL { p[0] / len_xy } else { 1.0 },
            if len_xy >= MINVAL { p[1] / len_xy } else { 0.0 },
        ];
        let len_qz = (q * q + p[2] * p[2]).sqrt();
        grad[0] = q * grad_q[0] / if len_qz > MINVAL { len_qz } else { MINVAL };
        grad[1] = q * grad_q[1] / if len_qz > MINVAL { len_qz } else { MINVAL };
        grad[2] = p[2] / if len_qz > MINVAL { len_qz } else { MINVAL };
    }
}
