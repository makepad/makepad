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

#[cfg(test)]
mod tests {
    use super::*;
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
