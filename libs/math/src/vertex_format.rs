//! Compact vertex/instance fetch types.
//!
//! Physical GPU formats used in `#[repr(C)]` POD vertex structs. The shader
//! language sees the logical type (`vec2f` / `vec4f`); fetch converts and
//! normalizes where the name says.

use std::fmt;

/// IEEE 754 binary16 encode (round-to-nearest-even, clamps to inf).
#[inline]
pub fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let frac = bits & 0x007f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if frac != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let frac = frac | 0x0080_0000;
        let shift = (14 - e) as u32;
        let half = (frac >> shift) as u16;
        let rem = frac & ((1 << shift) - 1);
        let round = (rem > (1 << (shift - 1)))
            || (rem == (1 << (shift - 1)) && (half & 1) != 0);
        return sign | half.wrapping_add(round as u16);
    }
    let half = (((e as u32) << 10) | (frac >> 13)) as u16;
    let remaining = frac & 0x1fff;
    let round = (remaining > 0x1000) || (remaining == 0x1000 && (half & 1) != 0);
    sign | half.wrapping_add(round as u16)
}

/// IEEE 754 binary16 decode — inverse of [`f32_to_f16_bits`].
#[inline]
pub fn f16_bits_to_f32(h: u16) -> f32 {
    let h = h as u32;
    let sign = (h & 0x8000) << 16;
    let exp = (h >> 10) & 0x1f;
    let frac = h & 0x3ff;
    if exp == 0 {
        if frac == 0 {
            return f32::from_bits(sign);
        }
        let v = frac as f32 * (-24f32).exp2();
        return if sign != 0 { -v } else { v };
    }
    if exp == 0x1f {
        return f32::from_bits(sign | 0x7f80_0000 | (frac << 13));
    }
    f32::from_bits(sign | ((exp + 112) << 23) | (frac << 13))
}

#[inline]
fn quantize_unorm(x: f32, max: f32) -> u16 {
    (x.clamp(0.0, 1.0) * max + 0.5) as u16
}

#[inline]
fn quantize_snorm(x: f32, max: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * max).round() as i16
}

/// Two IEEE binary16 values. Logical shader type: `vec2f`.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct F16x2 {
    pub x: u16,
    pub y: u16,
}

impl F16x2 {
    #[inline]
    pub fn from_f32(x: f32, y: f32) -> Self {
        Self {
            x: f32_to_f16_bits(x),
            y: f32_to_f16_bits(y),
        }
    }

    #[inline]
    pub fn to_f32(self) -> (f32, f32) {
        (f16_bits_to_f32(self.x), f16_bits_to_f32(self.y))
    }
}

/// Four IEEE binary16 values. Logical shader type: `vec4f`.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct F16x4 {
    pub x: u16,
    pub y: u16,
    pub z: u16,
    pub w: u16,
}

impl F16x4 {
    #[inline]
    pub fn from_f32(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self {
            x: f32_to_f16_bits(x),
            y: f32_to_f16_bits(y),
            z: f32_to_f16_bits(z),
            w: f32_to_f16_bits(w),
        }
    }

    #[inline]
    pub fn to_f32(self) -> (f32, f32, f32, f32) {
        (
            f16_bits_to_f32(self.x),
            f16_bits_to_f32(self.y),
            f16_bits_to_f32(self.z),
            f16_bits_to_f32(self.w),
        )
    }
}

/// Two unsigned 16-bit integers, fetched as `vec2f` (cast, not normalized).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct U16x2 {
    pub x: u16,
    pub y: u16,
}

impl U16x2 {
    #[inline]
    pub fn from_u16(x: u16, y: u16) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn from_f32(x: f32, y: f32) -> Self {
        Self {
            x: x.round().clamp(0.0, 65535.0) as u16,
            y: y.round().clamp(0.0, 65535.0) as u16,
        }
    }
}

/// Two signed 16-bit integers, fetched as `vec2f` (cast, not normalized).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct I16x2 {
    pub x: i16,
    pub y: i16,
}

impl I16x2 {
    #[inline]
    pub fn from_i16(x: i16, y: i16) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn from_f32(x: f32, y: f32) -> Self {
        Self {
            x: x.round().clamp(-32768.0, 32767.0) as i16,
            y: y.round().clamp(-32768.0, 32767.0) as i16,
        }
    }
}

/// Two unsigned 16-bit values, fetched as `vec2f` in 0..1 (`/ 65535`).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct UNorm16x2 {
    pub x: u16,
    pub y: u16,
}

impl UNorm16x2 {
    #[inline]
    pub fn from_f32(x: f32, y: f32) -> Self {
        Self {
            x: quantize_unorm(x, 65535.0),
            y: quantize_unorm(y, 65535.0),
        }
    }

    #[inline]
    pub fn to_f32(self) -> (f32, f32) {
        (self.x as f32 / 65535.0, self.y as f32 / 65535.0)
    }
}

/// Two signed 16-bit values, fetched as `vec2f` in -1..1 (`/ 32767`).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SNorm16x2 {
    pub x: i16,
    pub y: i16,
}

impl SNorm16x2 {
    #[inline]
    pub fn from_f32(x: f32, y: f32) -> Self {
        Self {
            x: quantize_snorm(x, 32767.0),
            y: quantize_snorm(y, 32767.0),
        }
    }

    #[inline]
    pub fn to_f32(self) -> (f32, f32) {
        (
            (self.x as f32 / 32767.0).max(-1.0),
            (self.y as f32 / 32767.0).max(-1.0),
        )
    }
}

/// Four unsigned 8-bit values, fetched as `vec4f` in 0..1 (`/ 255`).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct UNorm8x4(pub [u8; 4]);

impl UNorm8x4 {
    #[inline]
    pub fn from_f32(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self([
            quantize_unorm(x, 255.0) as u8,
            quantize_unorm(y, 255.0) as u8,
            quantize_unorm(z, 255.0) as u8,
            quantize_unorm(w, 255.0) as u8,
        ])
    }

    #[inline]
    pub fn to_f32(self) -> (f32, f32, f32, f32) {
        (
            self.0[0] as f32 / 255.0,
            self.0[1] as f32 / 255.0,
            self.0[2] as f32 / 255.0,
            self.0[3] as f32 / 255.0,
        )
    }
}

/// Four signed 8-bit values, fetched as `vec4f` in -1..1 (`/ 127`).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SNorm8x4(pub [i8; 4]);

impl SNorm8x4 {
    #[inline]
    pub fn from_f32(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self([
            quantize_snorm(x, 127.0) as i8,
            quantize_snorm(y, 127.0) as i8,
            quantize_snorm(z, 127.0) as i8,
            quantize_snorm(w, 127.0) as i8,
        ])
    }

    #[inline]
    pub fn to_f32(self) -> (f32, f32, f32, f32) {
        (
            (self.0[0] as f32 / 127.0).max(-1.0),
            (self.0[1] as f32 / 127.0).max(-1.0),
            (self.0[2] as f32 / 127.0).max(-1.0),
            (self.0[3] as f32 / 127.0).max(-1.0),
        )
    }
}

impl fmt::Display for F16x2 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (x, y) = self.to_f32();
        write!(f, "F16x2({x}, {y})")
    }
}
