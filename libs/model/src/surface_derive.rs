use super::*;

pub(super) fn source_image(
    material: &SurfaceMaterial,
    recipe: &SurfaceDerivation,
    limits: &Limits,
    ctx: &mut mesh::Context<'_>,
) -> Result<RgbaImage> {
    if let Some(name) = &recipe.source_layer {
        let source = material.channels.get(&recipe.source_channel)
            .and_then(|layers| layers.iter().find(|layer| &layer.id == name))
            .ok_or(Error::Invalid("unknown height source layer"))?;
        if material.memory_bytes().saturating_mul(2)
            .saturating_add(source.image.memory_bytes().saturating_mul(2)) > limits.mesh.max_bytes {
            return Err(Error::Budget("surface derivation source bytes"));
        }
        // A named layer may be hidden in its display stack. Its opacity and
        // mask still contribute to height coverage; blend needs no backdrop.
        let mut image = source.image.clone();
        for (i, pixel) in image.pixels.chunks_exact_mut(4).enumerate() {
            if i % image.width as usize == 0 { ctx.checkpoint(image.width as u64)?; }
            pixel[3] = crate::texture::unit_byte(pixel[3] as f64 / 255.
                * source.opacity * source.mask.as_ref().map_or(1., |mask| mask[i] as f64 / 255.));
        }
        Ok(image)
    } else {
        material.flatten(recipe.source_channel, limits, ctx)?
            .ok_or(Error::Invalid("height source channel has no visible layers"))
    }
}

pub(super) fn derive_image(
    source: &RgbaImage,
    channel: SurfaceChannel,
    recipe: &SurfaceDerivation,
    limits: &Limits,
    ctx: &mut mesh::Context<'_>,
) -> Result<RgbaImage> {
    recipe.validate(channel, limits)?;
    source.validate(limits)?;
    let mut out = RgbaImage::new(source.width, source.height, [0; 4], limits)?;
    let (width, height) = (source.width as i64, source.height as i64);
    let linear = (recipe.source_channel.encoding() == ImageEncoding::Srgb)
        .then(crate::texture::srgb_decode_table);
    // Decode each source texel once, before the four neighboring normal taps.
    // Fall back to direct samples when the existing working budget cannot fit
    // the temporary heights, rather than rejecting a previously valid image.
    let height_at = |i: usize| {
        let p = decode_bytes(recipe.source_channel, source.pixels[i * 4..i * 4 + 4].try_into().unwrap(), linear.as_ref());
        let value = match recipe.source_channel {
            SurfaceChannel::Occlusion => p[0],
            SurfaceChannel::MetallicRoughness => p[1],
            _ => 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2],
        };
        value * p[3]
    };
    let count = source.pixels.len() / 4;
    let working_bytes = limits.mesh.max_bytes.min(ctx.limits.max_bytes);
    let heights = if channel == SurfaceChannel::Normal
        && source.memory_bytes().saturating_add(out.memory_bytes())
            .saturating_add(count.saturating_mul(std::mem::size_of::<f64>())) <= working_bytes {
        let mut heights = Vec::with_capacity(count);
        for row in 0..source.height as usize {
            ctx.checkpoint(source.width as u64)?;
            for x in 0..source.width as usize {
                heights.push(height_at(row * source.width as usize + x));
            }
        }
        Some(heights)
    } else { None };
    let sample = |x: i64, y: i64| {
        let at = |n: i64, size: i64| if recipe.wrap { n.rem_euclid(size) } else { n.clamp(0, size - 1) };
        let i = (at(y, height) * width + at(x, width)) as usize;
        heights.as_ref().map_or_else(|| height_at(i), |heights| heights[i])
    };
    for y in 0..height {
        ctx.checkpoint(width as u64 * if heights.is_some() { 4 } else { 5 })?;
        for x in 0..width {
            // glTF data textures ignore alpha. Coverage already modulates
            // height, so emit a complete neutral surface through transparent
            // source regions instead of compositing empty normal texels.
            let alpha = 255;
            let pixel = if channel == SurfaceChannel::Normal {
                // Derivatives are per UV unit, hence resolution independent.
                // V increases down image rows, matching the document UV rule.
                let dx = (sample(x + 1, y) - sample(x - 1, y)) * width as f64 * 0.5 * recipe.strength;
                let dy = (sample(x, y + 1) - sample(x, y - 1)) * height as f64 * 0.5 * recipe.strength;
                let length = (dx * dx + dy * dy + 1.).sqrt();
                [crate::texture::unit_byte(0.5 - dx / length * 0.5),
                    crate::texture::unit_byte(0.5 - dy / length * 0.5),
                    crate::texture::unit_byte(0.5 + 0.5 / length), alpha]
            } else {
                let height = sample(x, y);
                let roughness = (1. - height) * recipe.roughness_min + height * recipe.roughness_max;
                [255, crate::texture::unit_byte(roughness), crate::texture::unit_byte(recipe.metallic), alpha]
            };
            out.pixels[((y * width + x) * 4) as usize..][..4].copy_from_slice(&pixel);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mip_chain_reference(
        source: &RgbaImage,
        encoding: ImageEncoding,
        limits: &Limits,
        ctx: &mut crate::mesh::Context<'_>,
    ) -> Result<Vec<RgbaImage>> {
        use crate::texture::{srgb_to_linear, linear_to_srgb, unit_byte};
        source.validate(limits)?;
        if source.pixels.len().saturating_mul(3) > limits.mesh.max_bytes {
            return Err(Error::Budget("mip working bytes"));
        }
        let mut levels = Vec::new();
        let mut previous = source;
        while previous.width > 1 || previous.height > 1 {
            let (w, h) = ((previous.width / 2).max(1), (previous.height / 2).max(1));
            let mut image = RgbaImage::new(w, h, [0; 4], limits)?;
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

    fn derive_image_reference(
        source: &RgbaImage,
        channel: SurfaceChannel,
        recipe: &SurfaceDerivation,
        limits: &Limits,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<RgbaImage> {
        recipe.validate(channel, limits)?;
        source.validate(limits)?;
        let mut out = RgbaImage::new(source.width, source.height, [0; 4], limits)?;
        let (width, height) = (source.width as i64, source.height as i64);
        let sample = |x: i64, y: i64| {
            let at = |n: i64, size: i64| if recipe.wrap { n.rem_euclid(size) } else { n.clamp(0, size - 1) };
            let i = (at(y, height) * width + at(x, width)) as usize * 4;
            let p = decode(recipe.source_channel, source.pixels[i..i + 4].try_into().unwrap());
            let value = match recipe.source_channel {
                SurfaceChannel::Occlusion => p[0],
                SurfaceChannel::MetallicRoughness => p[1],
                _ => 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2],
            };
            // Transparent, masked-out pixels carry no height, rather than a
            // hidden RGB value introducing bumps across a transparent gutter.
            value * p[3]
        };
        for y in 0..height {
            ctx.checkpoint(width as u64 * 5)?;
            for x in 0..width {
                // glTF data textures ignore alpha. Coverage already modulates
                // height, so emit a complete neutral surface through transparent
                // source regions instead of compositing empty normal texels.
                let alpha = 255;
                let pixel = if channel == SurfaceChannel::Normal {
                    // Derivatives are per UV unit, hence resolution independent.
                    // V increases down image rows, matching the document UV rule.
                    let dx = (sample(x + 1, y) - sample(x - 1, y)) * width as f64 * 0.5 * recipe.strength;
                    let dy = (sample(x, y + 1) - sample(x, y - 1)) * height as f64 * 0.5 * recipe.strength;
                    let length = (dx * dx + dy * dy + 1.).sqrt();
                    [crate::texture::unit_byte(0.5 - dx / length * 0.5),
                        crate::texture::unit_byte(0.5 - dy / length * 0.5),
                        crate::texture::unit_byte(0.5 + 0.5 / length), alpha]
                } else {
                    let height = sample(x, y);
                    let roughness = (1. - height) * recipe.roughness_min + height * recipe.roughness_max;
                    [255, crate::texture::unit_byte(roughness), crate::texture::unit_byte(recipe.metallic), alpha]
                };
                out.pixels[((y * width + x) * 4) as usize..][..4].copy_from_slice(&pixel);
            }
        }
        Ok(out)
    }

    fn recipe() -> SurfaceDerivation {
        SurfaceDerivation { source_channel: SurfaceChannel::Occlusion, source_layer: None,
            strength: 0.05, roughness_min: 0.2, roughness_max: 0.9, metallic: 0.7, wrap: true }
    }
    #[test]
    fn flat_height_is_neutral_and_roughness_uses_declared_endpoints() {
        let limits = Limits::default();
        for size in [1, 8] {
            let source = RgbaImage::new(size, size, [91, 91, 91, 255], &limits).unwrap();
            for wrap in [false, true] {
                let recipe = SurfaceDerivation { wrap, ..recipe() };
                let result = derive_image(&source, SurfaceChannel::Normal, &recipe, &limits, &mut mesh::Context::default()).unwrap();
                assert!(result.pixels.chunks_exact(4).all(|p| p == [128, 128, 255, 255]));
            }
        }
        let source = RgbaImage { width: 2, height: 1, pixels: vec![0, 0, 0, 255, 255, 255, 255, 255] };
        let result = derive_image(&source, SurfaceChannel::MetallicRoughness, &recipe(), &limits, &mut mesh::Context::default()).unwrap();
        assert_eq!(result.pixels, [255, 51, 179, 255, 255, 230, 179, 255]);
        let empty = RgbaImage::new(8, 8, [255, 255, 255, 0], &limits).unwrap();
        let result = derive_image(&empty, SurfaceChannel::Normal, &recipe(), &limits, &mut mesh::Context::default()).unwrap();
        assert!(result.pixels.chunks_exact(4).all(|p| p == [128, 128, 255, 255]));
    }
    #[test]
    fn height_ramp_normals_match_the_geometric_uv_frame() {
        use crate::transform::{add, dot, mul, normalized};
        let limits = Limits::default();
        let mut source = RgbaImage::new(8, 8, [0, 0, 0, 255], &limits).unwrap();
        for y in 0..8 { for x in 0..8 {
            source.pixels[(y * 8 + x) * 4] = ((x + y) * 16) as u8;
        }}
        let recipe = SurfaceDerivation { strength: 1., wrap: false, ..recipe() };
        let result = recipe.derive_image(&source, SurfaceChannel::Normal, &limits, &mut mesh::Context::default()).unwrap();
        let pixel = &result.pixels[(3 * 8 + 3) * 4..][..4];
        assert!(pixel[0] < 128 && pixel[1] < 128, "both tangent slopes oppose rising height");
        let tangent = [1., 0., 0.];
        let bitangent = [0., -1., 0.]; // Increasing V follows decreasing world Y.
        let normal = [0., 0., 1.];
        let mapped: [f64; 3] = std::array::from_fn(|i| pixel[i] as f64 / 127.5 - 1.);
        let world = normalized(add(add(mul(tangent, mapped[0]), mul(bitangent, mapped[1])), mul(normal, mapped[2]))).unwrap();
        let slope = 8. * 16. / 255.;
        // The displaced surface is P(u,v)=(u,-v,h(u,v)). Its normal is
        // perpendicular to both actual displaced surface derivatives.
        assert!(dot(world, [1., 0., slope]).abs() < 0.012);
        assert!(dot(world, [0., -1., slope]).abs() < 0.012);
        assert!(world[2] > 0.);
    }
    #[test]
    fn wrapped_height_normals_are_deterministic_under_tile_translation() {
        // Keep the original per-tap portable path as the byte-level oracle.
        let limits = Limits::default();
        let pattern = SurfacePattern { kind: PatternKind::Fbm,
            color_a: [0.015, 0.08, 0.2, 0.3], color_b: [0.9, 0.7, 0.4, 1.],
            scale: [5.25, 3.5], seed: 29 };
        let mut layer = SurfaceLayer::blank("fbm", SurfaceChannel::BaseColor, 64, 64, &limits).unwrap();
        render_pattern(&mut layer, SurfaceChannel::BaseColor, &pattern, &mut mesh::Context::default()).unwrap();
        let mut expected = layer.image.clone();
        for y in 0..64 { for x in 0..64 {
            let u = (x as f64 + 0.5) / 64. * pattern.scale[0];
            let v = (y as f64 + 0.5) / 64. * pattern.scale[1];
            let t = pattern_sampler::sample(pattern.kind, u, v, pattern.scale, pattern.seed);
            let color = std::array::from_fn(|c| pattern.color_a[c] * (1. - t) + pattern.color_b[c] * t);
            expected.pixels[(y * 64 + x) * 4..][..4].copy_from_slice(&encode(SurfaceChannel::BaseColor, color));
        }}
        assert_eq!(layer.image, expected, "64x64 FBM retains portable sRGB rounding");
        // The one-layer shortcut must retain transparent-RGB clearing and
        // straight-alpha bytes, including all 256 possible alpha values.
        for (i, pixel) in layer.image.pixels.chunks_exact_mut(4).enumerate() {
            pixel[3] = i as u8;
        }
        for blend in [SurfaceBlend::Over, SurfaceBlend::Multiply, SurfaceBlend::Add, SurfaceBlend::Subtract] {
            layer.blend = blend;
            let mut flattened_reference = layer.image.clone();
            for pixel in flattened_reference.pixels.chunks_exact_mut(4) {
                let mut backdrop = decode(SurfaceChannel::BaseColor, SurfaceChannel::BaseColor.neutral());
                backdrop[3] = 0.;
                let src = decode(SurfaceChannel::BaseColor, (&*pixel).try_into().unwrap());
                pixel.copy_from_slice(&encode(SurfaceChannel::BaseColor, composite(backdrop, src, 1., blend)));
            }
            let mut material = SurfaceMaterial::default();
            material.channels.insert(SurfaceChannel::BaseColor, vec![layer.clone()]);
            assert_eq!(material.flatten(SurfaceChannel::BaseColor, &limits, &mut mesh::Context::default()).unwrap().unwrap(), flattened_reference, "flatten {blend:?}");
        }
        assert_eq!(layer.image.mip_chain(ImageEncoding::Srgb, &limits, &mut mesh::Context::default()).unwrap(),
            mip_chain_reference(&layer.image, ImageEncoding::Srgb, &limits, &mut mesh::Context::default()).unwrap(),
            "all 64x64 FBM mip bytes retain portable premultiplied filtering");
        let transfer = crate::texture::SrgbBytes::new();
        for i in 0..255 {
            let boundary = crate::texture::srgb_to_linear((i as f64 + 0.5) / 255.).to_bits();
            for bits in boundary - 32..=boundary + 32 {
                let value = f64::from_bits(bits);
                assert_eq!(transfer.encode(value), crate::texture::unit_byte(crate::texture::linear_to_srgb(value)), "sRGB threshold {i}");
            }
            for offset in [-256., -128., -64., 64., 128., 256.] {
                let value = f64::from_bits(boundary) + offset * f64::EPSILON;
                assert_eq!(transfer.encode(value), crate::texture::unit_byte(crate::texture::linear_to_srgb(value)), "sRGB fallback edge {i}, {offset}");
            }
        }
        for wrap in [false, true] {
            let recipe = SurfaceDerivation { source_channel: SurfaceChannel::BaseColor, wrap, ..recipe() };
            for channel in [SurfaceChannel::Normal, SurfaceChannel::MetallicRoughness] {
                let actual = derive_image(&layer.image, channel, &recipe, &limits, &mut mesh::Context::default()).unwrap();
                let reference = derive_image_reference(&layer.image, channel, &recipe, &limits, &mut mesh::Context::default()).unwrap();
                assert_eq!(actual, reference, "64x64 derived bytes for {channel:?}, wrap={wrap}");
                if channel == SurfaceChannel::Normal {
                    let mut constrained = mesh::Context::default();
                    // Source and output fit; decoded heights do not. The
                    // operation still succeeds through direct neighbor taps.
                    constrained.limits.max_bytes = layer.image.memory_bytes() * 2;
                    let fallback = derive_image(&layer.image, channel, &recipe, &limits, &mut constrained).unwrap();
                    assert_eq!(fallback, reference, "height scratch fallback preserves bytes, wrap={wrap}");
                }
            }
        }
        let limits = Limits::default();
        let mut source = RgbaImage::new(16, 8, [0, 0, 0, 255], &limits).unwrap();
        for y in 0..8 { for x in 0..16 {
            let h = (127. + 90. * (std::f64::consts::TAU * x as f64 / 16.).sin()) as u8;
            source.pixels[(y * 16 + x) * 4] = h;
        }}
        let a = derive_image(&source, SurfaceChannel::Normal, &recipe(), &limits, &mut mesh::Context::default()).unwrap();
        let mut shifted = source.clone();
        for row in shifted.pixels.chunks_exact_mut(64) { row.rotate_left(12); }
        let b = derive_image(&shifted, SurfaceChannel::Normal, &recipe(), &limits, &mut mesh::Context::default()).unwrap();
        let mut expected = a.clone();
        for row in expected.pixels.chunks_exact_mut(64) { row.rotate_left(12); }
        assert_eq!(b, expected, "sampling crosses the UV seam, without clamping or random state");
        assert_eq!(a, derive_image(&source, SurfaceChannel::Normal, &recipe(), &limits, &mut mesh::Context::default()).unwrap());
        let clamped = derive_image(&source, SurfaceChannel::Normal, &SurfaceDerivation { wrap: false, ..recipe() }, &limits, &mut mesh::Context::default()).unwrap();
        assert_ne!(&a.pixels[..4], &clamped.pixels[..4]);
        assert!(a.pixels.chunks_exact(4).any(|pixel| pixel[0] != 128));
    }
}
