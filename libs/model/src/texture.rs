use crate::{Error, Limits, Result};
use makepad_zune_png::{
    makepad_zune_core::{
        bit_depth::BitDepth,
        bytestream::ZCursor,
        colorspace::ColorSpace,
        options::{DecoderOptions, EncoderOptions},
    },
    PngDecoder, PngEncoder,
};

/// Opaque RGB images are the portable first material contract. Work takes place
/// on the document worker; callers can set the result through SetMaterial.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}
impl Texture {
    pub fn solid(width: u32, height: u32, color: [u8; 3], limits: &Limits) -> Result<Self> {
        let len = dimensions(width, height, limits)?;
        let mut rgb = Vec::with_capacity(len);
        for _ in 0..len / 3 {
            rgb.extend_from_slice(&color);
        }
        Ok(Self { width, height, rgb })
    }
    pub fn from_png(bytes: &[u8], limits: &Limits) -> Result<Self> {
        if bytes.len() > limits.max_texture_bytes {
            return Err(Error::Budget("texture bytes"));
        }
        let options = DecoderOptions::default()
            .set_max_width(limits.max_texture_dimension as usize)
            .set_max_height(limits.max_texture_dimension as usize)
            .set_strict_mode(true)
            .png_set_confirm_crc(true)
            .png_set_strip_to_8bit(true)
            .png_set_decode_animated(false);
        let mut decoder = PngDecoder::new_with_options(ZCursor::new(bytes), options);
        decoder
            .decode_headers()
            .map_err(|_| Error::Invalid("PNG headers"))?;
        let (w, h) = decoder
            .dimensions()
            .ok_or(Error::Invalid("PNG dimensions"))?;
        let len = dimensions(w as u32, h as u32, limits)?;
        let pixels = decoder
            .decode_raw()
            .map_err(|_| Error::Invalid("PNG pixels"))?;
        let mut rgb = Vec::with_capacity(len);
        match decoder.colorspace() {
            Some(ColorSpace::RGB) if pixels.len() == len => rgb = pixels,
            Some(ColorSpace::RGBA) if pixels.len() == len / 3 * 4 => {
                for p in pixels.chunks_exact(4) {
                    if p[3] != 255 {
                        return Err(Error::Invalid(
                            "transparent materials are not supported yet",
                        ));
                    }
                    rgb.extend_from_slice(&p[..3]);
                }
            }
            Some(ColorSpace::Luma) if pixels.len() == len / 3 => {
                for p in pixels {
                    rgb.extend_from_slice(&[p; 3]);
                }
            }
            Some(ColorSpace::LumaA) if pixels.len() == len / 3 * 2 => {
                for p in pixels.chunks_exact(2) {
                    if p[1] != 255 {
                        return Err(Error::Invalid(
                            "transparent materials are not supported yet",
                        ));
                    }
                    rgb.extend_from_slice(&[p[0]; 3]);
                }
            }
            _ => return Err(Error::Invalid("PNG pixel format")),
        }
        Ok(Self {
            width: w as u32,
            height: h as u32,
            rgb,
        })
    }
    pub fn to_png(&self, limits: &Limits) -> Result<Vec<u8>> {
        if self.rgb.len() != dimensions(self.width, self.height, limits)? {
            return Err(Error::Invalid("texture pixel count"));
        }
        let mut out = Vec::new();
        let options = EncoderOptions::new(
            self.width as usize,
            self.height as usize,
            ColorSpace::RGB,
            BitDepth::Eight,
        );
        PngEncoder::new(&self.rgb, options)
            .encode(&mut out)
            .map_err(|_| Error::Invalid("PNG encode"))?;
        if out.len() > limits.max_texture_bytes {
            return Err(Error::Budget("encoded texture bytes"));
        }
        Ok(out)
    }
    /// Deterministic filled UV-space disk. Pixels are top-left origin; UV V is
    /// converted by the caller. No antialiasing or random state is implicit.
    pub fn paint_disk(
        &mut self,
        center: [f64; 2],
        radius: f64,
        color: [u8; 3],
        limits: &Limits,
        cancelled: Option<&dyn Fn() -> bool>,
    ) -> Result<()> {
        let mut ctx = crate::mesh::Context::new(limits.mesh.clone(), cancelled);
        self.paint_disk_ctx(center, radius, color, limits, &mut ctx)
    }
    pub(crate) fn paint_disk_ctx(
        &mut self,
        center: [f64; 2],
        radius: f64,
        color: [u8; 3],
        limits: &Limits,
        ctx: &mut crate::mesh::Context<'_>,
    ) -> Result<()> {
        ctx.checkpoint(0)?;
        if self.rgb.len() != dimensions(self.width, self.height, limits)? {
            return Err(Error::Invalid("texture pixel count"));
        }
        if center.iter().any(|v| !v.is_finite()) || !radius.is_finite() || radius <= 0.0 {
            return Err(Error::Invalid("paint disk"));
        }
        if self.rgb.len().saturating_mul(2) > limits.mesh.max_bytes {
            return Err(Error::Budget("paint working bytes"));
        }
        let mut candidate = self.rgb.clone();
        for y in 0..self.height {
            ctx.checkpoint(self.width as u64)?;
            for x in 0..self.width {
                let dx = (x as f64 + 0.5) / self.width as f64 - center[0];
                let dy = (y as f64 + 0.5) / self.height as f64 - center[1];
                if dx.hypot(dy) <= radius {
                    let p = (y as usize * self.width as usize + x as usize) * 3;
                    candidate[p..p + 3].copy_from_slice(&color);
                }
            }
        }
        self.rgb = candidate;
        Ok(())
    }
}
fn dimensions(width: u32, height: u32, limits: &Limits) -> Result<usize> {
    if width == 0
        || height == 0
        || width > limits.max_texture_dimension
        || height > limits.max_texture_dimension
    {
        return Err(Error::Budget("texture dimensions"));
    }
    let len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(3))
        .ok_or(Error::Budget("texture pixels"))?;
    if len > limits.max_texture_bytes {
        return Err(Error::Budget("texture pixels"));
    }
    Ok(len)
}

/// Surface layers retain RGBA pixels, including authored transparency. Color
/// layers encode RGB as sRGB; data/normal layers store normalized linear bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageEncoding {
    Srgb,
    Linear,
    Normal,
}
impl RgbaImage {
    pub fn new(width: u32, height: u32, color: [u8; 4], limits: &Limits) -> Result<Self> {
        let len = rgba_dimensions(width, height, limits)?;
        let mut pixels = Vec::with_capacity(len);
        for _ in 0..len / 4 {
            pixels.extend_from_slice(&color);
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }
    pub fn validate(&self, limits: &Limits) -> Result<()> {
        if self.pixels.len() != rgba_dimensions(self.width, self.height, limits)? {
            return Err(Error::Invalid("RGBA pixel count"));
        }
        Ok(())
    }
    pub fn from_png(bytes: &[u8], limits: &Limits) -> Result<Self> {
        if bytes.len() > limits.max_texture_bytes {
            return Err(Error::Budget("PNG bytes"));
        }
        let options = DecoderOptions::default()
            .set_max_width(limits.max_texture_dimension as usize)
            .set_max_height(limits.max_texture_dimension as usize)
            .set_strict_mode(true)
            .png_set_confirm_crc(true)
            .png_set_strip_to_8bit(true)
            .png_set_decode_animated(false);
        let mut decoder = PngDecoder::new_with_options(ZCursor::new(bytes), options);
        decoder
            .decode_headers()
            .map_err(|_| Error::Invalid("PNG headers"))?;
        let (w, h) = decoder
            .dimensions()
            .ok_or(Error::Invalid("PNG dimensions"))?;
        let len = rgba_dimensions(w as u32, h as u32, limits)?;
        let raw = decoder
            .decode_raw()
            .map_err(|_| Error::Invalid("PNG pixels"))?;
        let mut pixels = Vec::with_capacity(len);
        match decoder.colorspace() {
            Some(ColorSpace::RGBA) if raw.len() == len => pixels = raw,
            Some(ColorSpace::RGB) if raw.len() == len / 4 * 3 => {
                for p in raw.chunks_exact(3) {
                    pixels.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
            Some(ColorSpace::Luma) if raw.len() == len / 4 => {
                for p in raw {
                    pixels.extend_from_slice(&[p, p, p, 255]);
                }
            }
            Some(ColorSpace::LumaA) if raw.len() == len / 2 => {
                for p in raw.chunks_exact(2) {
                    pixels.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
                }
            }
            _ => return Err(Error::Invalid("PNG pixel format")),
        }
        Ok(Self {
            width: w as u32,
            height: h as u32,
            pixels,
        })
    }
    pub fn to_png(&self, limits: &Limits) -> Result<Vec<u8>> {
        self.validate(limits)?;
        let options = EncoderOptions::new(
            self.width as usize,
            self.height as usize,
            ColorSpace::RGBA,
            BitDepth::Eight,
        );
        let mut bytes = Vec::new();
        PngEncoder::new(&self.pixels, options)
            .encode(&mut bytes)
            .map_err(|_| Error::Invalid("PNG encode"))?;
        if bytes.len() > limits.max_texture_bytes {
            return Err(Error::Budget("PNG encoded bytes"));
        }
        Ok(bytes)
    }
    pub fn memory_bytes(&self) -> usize {
        self.pixels.len().saturating_add(64)
    }
    /// Coverage-driven padding: nearest covered neighbors propagate by one
    /// texel per iteration. Optional alpha preservation pads RGB only.
    pub fn dilate(
        &mut self,
        iterations: u32,
        preserve_alpha: bool,
        limits: &Limits,
        ctx: &mut crate::mesh::Context<'_>,
    ) -> Result<()> {
        self.validate(limits)?;
        if iterations > 64 {
            return Err(Error::Budget("dilation iterations"));
        }
        if self.pixels.len().saturating_mul(3) > limits.mesh.max_bytes {
            return Err(Error::Budget("dilation working bytes"));
        }
        let mut pixels = self.pixels.clone();
        let mut coverage: Vec<bool> = pixels.chunks_exact(4).map(|p| p[3] != 0).collect();
        for _ in 0..iterations {
            let mut next = pixels.clone();
            let mut next_coverage = coverage.clone();
            let mut changed = false;
            for y in 0..self.height as usize {
                ctx.checkpoint(self.width as u64)?;
                for x in 0..self.width as usize {
                    let i = y * self.width as usize + x;
                    if coverage[i] {
                        continue;
                    }
                    let mut count = 0.;
                    let mut sum = [0.; 4];
                    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        if nx < 0 || ny < 0 || nx >= self.width as i32 || ny >= self.height as i32 {
                            continue;
                        }
                        let ni = ny as usize * self.width as usize + nx as usize;
                        if coverage[ni] {
                            for c in 0..4 {
                                sum[c] += pixels[ni * 4 + c] as f64;
                            }
                            count += 1.;
                        }
                    }
                    if count > 0. {
                        for c in 0..if preserve_alpha { 3 } else { 4 } {
                            next[i * 4 + c] = (sum[c] / count).round() as u8;
                        }
                        next_coverage[i] = true;
                        changed = true;
                    }
                }
            }
            pixels = next;
            coverage = next_coverage;
            if !changed {
                break;
            }
        }
        ctx.checkpoint(1)?;
        self.pixels = pixels;
        Ok(())
    }
    /// Mip levels 1..N. Color filtering is linear/premultiplied-alpha and
    /// normal filtering renormalizes tangent-space vectors.
    pub fn mip_chain(
        &self,
        encoding: ImageEncoding,
        limits: &Limits,
        ctx: &mut crate::mesh::Context<'_>,
    ) -> Result<Vec<Self>> {
        self.validate(limits)?;
        if self.pixels.len().saturating_mul(3) > limits.mesh.max_bytes {
            return Err(Error::Budget("mip working bytes"));
        }
        let mut levels = Vec::new();
        let mut previous = self;
        while previous.width > 1 || previous.height > 1 {
            let (w, h) = ((previous.width / 2).max(1), (previous.height / 2).max(1));
            let mut image = Self::new(w, h, [0; 4], limits)?;
            for y in 0..h {
                ctx.checkpoint(w as u64)?;
                for x in 0..w {
                    // Partition the full source footprint, including odd final
                    // rows/columns, so no source texel silently disappears.
                    let x0 = x * previous.width / w;
                    let x1 = (x + 1) * previous.width / w;
                    let y0 = y * previous.height / h;
                    let y1 = (y + 1) * previous.height / h;
                    let mut sum = [0.; 4];
                    let mut count = 0.;
                    for sy in y0..y1 {
                        for sx in x0..x1 {
                            let p =
                                &previous.pixels[((sy * previous.width + sx) * 4) as usize..][..4];
                            let a = p[3] as f64 / 255.;
                            sum[3] += a;
                            for c in 0..3 {
                                sum[c] += match encoding {
                                    ImageEncoding::Srgb => srgb_to_linear(p[c] as f64 / 255.) * a,
                                    ImageEncoding::Normal => p[c] as f64 / 127.5 - 1.,
                                    ImageEncoding::Linear => p[c] as f64 / 255.,
                                };
                            }
                            count += 1.;
                        }
                    }
                    let dst = ((y * w + x) * 4) as usize;
                    if encoding == ImageEncoding::Normal {
                        let length = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
                        if length > 1e-20 {
                            for c in 0..3 {
                                sum[c] = sum[c] / length * 0.5 + 0.5;
                            }
                        } else {
                            sum[..3].copy_from_slice(&[0.5, 0.5, 1.]);
                        }
                    }
                    for c in 0..3 {
                        let value = match encoding {
                            ImageEncoding::Srgb => {
                                linear_to_srgb(if sum[3] > 1e-20 { sum[c] / sum[3] } else { 0. })
                            }
                            ImageEncoding::Normal => sum[c],
                            ImageEncoding::Linear => sum[c] / count,
                        };
                        image.pixels[dst + c] = unit_byte(value);
                    }
                    image.pixels[dst + 3] = unit_byte(sum[3] / count);
                }
            }
            levels.push(image);
            previous = levels.last().unwrap();
        }
        Ok(levels)
    }
}
pub(crate) fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
pub(crate) fn linear_to_srgb(value: f64) -> f64 {
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1. / 2.4) - 0.055
    }
}
pub(crate) fn unit_byte(value: f64) -> u8 {
    (value.clamp(0., 1.) * 255.).round() as u8
}
fn rgba_dimensions(w: u32, h: u32, limits: &Limits) -> Result<usize> {
    if w == 0 || h == 0 || w > limits.max_texture_dimension || h > limits.max_texture_dimension {
        return Err(Error::Budget("RGBA dimensions"));
    }
    let len = (w as usize)
        .checked_mul(h as usize)
        .and_then(|v| v.checked_mul(4))
        .ok_or(Error::Budget("RGBA dimensions"))?;
    if len > limits.max_texture_bytes {
        return Err(Error::Budget("RGBA pixels"));
    }
    Ok(len)
}
