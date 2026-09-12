use super::*;
fn write_array<const N: usize>(w: &mut Writer, a: &[f64; N]) -> Result<()> {
    for v in a {
        w.f64(*v)?;
    }
    Ok(())
}
fn read_array<const N: usize>(r: &mut Reader<'_>) -> Result<[f64; N]> {
    let mut a = [0.; N];
    for v in &mut a {
        *v = r.f64()?;
    }
    Ok(a)
}
fn boolean(r: &mut Reader<'_>) -> Result<bool> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Corrupt("surface boolean")),
    }
}
fn image_write(w: &mut Writer, i: &RgbaImage) -> Result<()> {
    w.u32(i.width)?;
    w.u32(i.height)?;
    w.blob(&i.pixels)
}
fn image_read(r: &mut Reader<'_>, l: &Limits) -> Result<RgbaImage> {
    let image = RgbaImage {
        width: r.u32()?,
        height: r.u32()?,
        pixels: r.blob(l.max_texture_bytes)?.to_vec(),
    };
    image.validate(l)?;
    Ok(image)
}
fn pattern_write(w: &mut Writer, p: &SurfacePattern) -> Result<()> {
    w.u8(p.kind as u8)?;
    write_array(w, &p.color_a)?;
    write_array(w, &p.color_b)?;
    write_array(w, &p.scale)?;
    w.u64(p.seed)
}
fn pattern_read(r: &mut Reader<'_>) -> Result<SurfacePattern> {
    let kind = match r.u8()? {
        0 => PatternKind::Checker,
        1 => PatternKind::Stripes,
        2 => PatternKind::Gradient,
        3 => PatternKind::Noise,
        4 => PatternKind::Perlin,
        5 => PatternKind::Fbm,
        6 => PatternKind::Yarn,
        _ => return Err(Error::Corrupt("surface pattern kind")),
    };
    Ok(SurfacePattern {
        kind,
        color_a: read_array(r)?,
        color_b: read_array(r)?,
        scale: read_array(r)?,
        seed: r.u64()?,
    })
}
fn derivation_write(w: &mut Writer, recipe: &SurfaceDerivation) -> Result<()> {
    w.u8(recipe.source_channel as u8)?;
    w.u8(recipe.source_layer.is_some() as u8)?;
    if let Some(name) = &recipe.source_layer { w.string(name)?; }
    for value in [recipe.strength, recipe.roughness_min, recipe.roughness_max, recipe.metallic] { w.f64(value)?; }
    w.u8(recipe.wrap as u8)
}
fn derivation_read(r: &mut Reader<'_>, limits: &Limits) -> Result<SurfaceDerivation> {
    Ok(SurfaceDerivation {
        source_channel: channel_read(r)?,
        source_layer: if boolean(r)? { Some(r.string(limits.max_name_bytes)?) } else { None },
        strength: r.f64()?, roughness_min: r.f64()?, roughness_max: r.f64()?, metallic: r.f64()?,
        wrap: boolean(r)?,
    })
}
fn channel_read(r: &mut Reader<'_>) -> Result<SurfaceChannel> {
    match r.u8()? {
        0 => Ok(SurfaceChannel::BaseColor),
        1 => Ok(SurfaceChannel::MetallicRoughness),
        2 => Ok(SurfaceChannel::Normal),
        3 => Ok(SurfaceChannel::Occlusion),
        4 => Ok(SurfaceChannel::Emissive),
        _ => Err(Error::Corrupt("surface channel")),
    }
}
fn layer_write(w: &mut Writer, l: &SurfaceLayer) -> Result<()> {
    w.string(&l.id)?;
    image_write(w, &l.image)?;
    w.u8(l.mask.is_some() as u8)?;
    if let Some(mask) = &l.mask {
        w.blob(mask)?;
    }
    w.f64(l.opacity)?;
    w.u8(l.blend as u8)?;
    w.u8(l.visible as u8)?;
    // Values 0/1 keep the exact legacy layout. Bit 1 introduces a recipe;
    // older readers reject it explicitly rather than misreading a layer.
    w.u8(l.pattern.is_some() as u8 | ((l.derived.is_some() as u8) << 1))?;
    if let Some(p) = &l.pattern { pattern_write(w, p)?; }
    if let Some(recipe) = &l.derived { derivation_write(w, recipe)?; }
    w.count(l.mips.len())?;
    for mip in &l.mips {
        image_write(w, mip)?;
    }
    Ok(())
}
fn layer_read(r: &mut Reader<'_>, l: &Limits) -> Result<SurfaceLayer> {
    let id = r.string(l.max_name_bytes)?;
    let image = image_read(r, l)?;
    let mask = if boolean(r)? {
        Some(r.blob(l.max_texture_bytes / 4)?.to_vec())
    } else {
        None
    };
    let opacity = r.f64()?;
    let blend = match r.u8()? {
        0 => SurfaceBlend::Over,
        1 => SurfaceBlend::Multiply,
        2 => SurfaceBlend::Add,
        3 => SurfaceBlend::Subtract,
        _ => return Err(Error::Corrupt("surface blend")),
    };
    let visible = boolean(r)?;
    let recipe_flags = r.u8()?;
    if recipe_flags > 3 { return Err(Error::Corrupt("surface recipe flags")); }
    let pattern = if recipe_flags & 1 != 0 { Some(pattern_read(r)?) } else { None };
    let derived = if recipe_flags & 2 != 0 { Some(derivation_read(r, l)?) } else { None };
    let mut mips = Vec::new();
    for _ in 0..r.count(32)? {
        mips.push(image_read(r, l)?);
    }
    Ok(SurfaceLayer {
        id,
        image,
        mask,
        opacity,
        blend,
        visible,
        pattern,
        derived,
        mips,
    })
}
fn material_write(w: &mut Writer, m: &SurfaceMaterial) -> Result<()> {
    write_array(w, &m.base_color)?;
    for v in [
        m.metallic,
        m.roughness,
        m.normal_scale,
        m.occlusion_strength,
    ] {
        w.f64(v)?;
    }
    write_array(w, &m.emissive)?;
    w.f64(m.emissive_strength)?;
    w.u8(m.alpha as u8)?;
    w.f64(m.alpha_cutoff)?;
    // The legacy 0/1 layout stays byte-identical for materials without fur.
    w.u8(m.double_sided as u8 | ((m.fur.is_some() as u8) << 1))?;
    if let Some(fur) = m.fur {
        for value in [fur.length, fur.density, fur.scale] { w.f64(value)?; }
        w.u32(fur.seed)?;
    }
    w.count(m.channels.len())?;
    for (ch, layers) in &m.channels {
        w.u8(*ch as u8)?;
        w.count(layers.len())?;
        for layer in layers {
            layer_write(w, layer)?;
        }
    }
    Ok(())
}
fn material_read(r: &mut Reader<'_>, l: &Limits) -> Result<SurfaceMaterial> {
    let base_color = read_array(r)?;
    let metallic = r.f64()?;
    let roughness = r.f64()?;
    let normal_scale = r.f64()?;
    let occlusion_strength = r.f64()?;
    let emissive = read_array(r)?;
    let emissive_strength = r.f64()?;
    let alpha = match r.u8()? {
        0 => SurfaceAlpha::Opaque,
        1 => SurfaceAlpha::Mask,
        2 => SurfaceAlpha::Blend,
        _ => return Err(Error::Corrupt("surface alpha")),
    };
    let alpha_cutoff = r.f64()?;
    let flags = r.u8()?;
    if flags > 3 { return Err(Error::Corrupt("surface material flags")); }
    let double_sided = flags & 1 != 0;
    let fur = if flags & 2 != 0 {
        Some(makepad_gltf::GlbFurMaterial {
            length: r.f64()?, density: r.f64()?, scale: r.f64()?, seed: r.u32()?,
        })
    } else { None };
    let mut channels = BTreeMap::new();
    for _ in 0..r.count(5)? {
        let ch = channel_read(r)?;
        let mut layers = Vec::new();
        for _ in 0..r.count(32)? {
            layers.push(layer_read(r, l)?);
        }
        if channels.insert(ch, layers).is_some() {
            return Err(Error::Corrupt("duplicate surface channel"));
        }
    }
    Ok(SurfaceMaterial {
        fur,
        base_color,
        metallic,
        roughness,
        normal_scale,
        occlusion_strength,
        emissive,
        emissive_strength,
        alpha,
        alpha_cutoff,
        double_sided,
        channels,
    })
}
pub(crate) fn write_surface(w: &mut Writer, s: &SurfaceState) -> Result<()> {
    w.count(s.materials.len())?;
    for (id, m) in &s.materials {
        w.u32(*id)?;
        material_write(w, m)?;
    }
    w.count(s.vertex_colors.len())?;
    for ((object, id), color) in &s.vertex_colors {
        w.string(object)?;
        w.u64(id.0)?;
        write_array(w, color)?;
    }
    Ok(())
}
pub(crate) fn read_surface(r: &mut Reader<'_>, l: &Limits) -> Result<SurfaceState> {
    let mut s = SurfaceState::default();
    for _ in 0..r.count(l.max_materials)? {
        let id = r.u32()?;
        let value = material_read(r, l)?;
        if s.materials.insert(id, value).is_some() {
            return Err(Error::Corrupt("duplicate surface material"));
        }
        if s.memory_bytes() > l.max_source_bytes {
            return Err(Error::Budget("surface source"));
        }
    }
    for _ in 0..r.count(l.mesh.max_vertices)? {
        let name = r.string(l.max_name_bytes)?;
        let id = mesh::VertexId(r.u64()?);
        let color = read_array(r)?;
        if s.vertex_colors.insert((name, id), color).is_some() {
            return Err(Error::Corrupt("duplicate vertex color"));
        }
    }
    s.validate(l, &mut mesh::Context::new(l.mesh.clone(), None))?;
    Ok(s)
}
fn target_write(w: &mut Writer, id: u32, ch: SurfaceChannel, layer: &str) -> Result<()> {
    w.u32(id)?;
    w.u8(ch as u8)?;
    w.string(layer)
}
fn target_read(r: &mut Reader<'_>, l: &Limits) -> Result<(u32, SurfaceChannel, String)> {
    Ok((r.u32()?, channel_read(r)?, r.string(l.max_name_bytes)?))
}
pub(crate) fn write_surface_operation(w: &mut Writer, op: &SurfaceOperation) -> Result<()> {
    use SurfaceOperation::*;
    match op {
        Material { material, value } => {
            w.u8(0)?;
            w.u32(*material)?;
            material_write(w, value)?;
        }
        RemoveMaterial { material } => {
            w.u8(1)?;
            w.u32(*material)?;
        }
        Layer {
            material,
            channel,
            layer,
        } => {
            w.u8(2)?;
            w.u32(*material)?;
            w.u8(*channel as u8)?;
            layer_write(w, layer)?;
        }
        RemoveLayer {
            material,
            channel,
            layer,
        } => {
            w.u8(3)?;
            target_write(w, *material, *channel, layer)?;
        }
        MoveLayer {
            material,
            channel,
            layer,
            index,
        } => {
            w.u8(4)?;
            target_write(w, *material, *channel, layer)?;
            w.count(*index)?;
        }
        Mask {
            material,
            channel,
            layer,
            mask,
        } => {
            w.u8(5)?;
            target_write(w, *material, *channel, layer)?;
            w.u8(mask.is_some() as u8)?;
            if let Some(mask) = mask {
                w.blob(mask)?;
            }
        }
        Pattern {
            material,
            channel,
            layer,
            pattern,
        } => {
            w.u8(6)?;
            target_write(w, *material, *channel, layer)?;
            pattern_write(w, pattern)?;
        }
        Derive { material, channel, layer, recipe } => {
            w.u8(13)?;
            target_write(w, *material, *channel, layer)?;
            derivation_write(w, recipe)?;
        }
        Stroke {
            material,
            channel,
            layer,
            points,
            radius,
            hardness,
            opacity,
            color,
            mask,
        } => {
            w.u8(7)?;
            target_write(w, *material, *channel, layer)?;
            w.count(points.len())?;
            for point in points {
                write_array(w, point)?;
            }
            for v in [radius, hardness, opacity] {
                w.f64(*v)?;
            }
            write_array(w, color)?;
            w.u8(*mask as u8)?;
        }
        ProjectedStroke {
            object,
            material,
            channel,
            layer,
            origin,
            direction,
            radius,
            depth,
            hardness,
            opacity,
            color,
            mask,
        } => {
            w.u8(8)?;
            target_write(w, *material, *channel, layer)?;
            w.string(object)?;
            write_array(w, origin)?;
            write_array(w, direction)?;
            for v in [radius, depth, hardness, opacity] {
                w.f64(*v)?;
            }
            write_array(w, color)?;
            w.u8(*mask as u8)?;
        }
        VertexPaint {
            object,
            vertices,
            color,
            opacity,
        } => {
            w.u8(9)?;
            w.string(object)?;
            w.count(vertices.len())?;
            for v in vertices {
                w.u64(v.0)?;
            }
            write_array(w, color)?;
            w.f64(*opacity)?;
        }
        Bake {
            source,
            target,
            material,
            channel,
            layer,
            width,
            height,
            max_distance,
            ao_samples,
            ao_distance,
            dilation,
        } => {
            w.u8(10)?;
            target_write(w, *material, *channel, layer)?;
            w.string(source)?;
            w.string(target)?;
            w.u32(*width)?;
            w.u32(*height)?;
            w.f64(*max_distance)?;
            w.u32(*ao_samples)?;
            w.f64(*ao_distance)?;
            w.u32(*dilation)?;
        }
        Dilate {
            material,
            channel,
            layer,
            iterations,
            preserve_alpha,
        } => {
            w.u8(11)?;
            target_write(w, *material, *channel, layer)?;
            w.u32(*iterations)?;
            w.u8(*preserve_alpha as u8)?;
        }
        Mips {
            material,
            channel,
            layer,
        } => {
            w.u8(12)?;
            target_write(w, *material, *channel, layer)?;
        }
    }
    Ok(())
}
pub(crate) fn read_surface_operation(r: &mut Reader<'_>, l: &Limits) -> Result<SurfaceOperation> {
    use SurfaceOperation::*;
    Ok(match r.u8()? {
        0 => Material {
            material: r.u32()?,
            value: material_read(r, l)?,
        },
        1 => RemoveMaterial { material: r.u32()? },
        2 => Layer {
            material: r.u32()?,
            channel: channel_read(r)?,
            layer: layer_read(r, l)?,
        },
        3 => {
            let (material, channel, layer) = target_read(r, l)?;
            RemoveLayer {
                material,
                channel,
                layer,
            }
        }
        4 => {
            let (material, channel, layer) = target_read(r, l)?;
            MoveLayer {
                material,
                channel,
                layer,
                index: r.u32()? as usize,
            }
        }
        5 => {
            let (material, channel, layer) = target_read(r, l)?;
            Mask {
                material,
                channel,
                layer,
                mask: if boolean(r)? {
                    Some(r.blob(l.max_texture_bytes / 4)?.to_vec())
                } else {
                    None
                },
            }
        }
        6 => {
            let (material, channel, layer) = target_read(r, l)?;
            Pattern {
                material,
                channel,
                layer,
                pattern: pattern_read(r)?,
            }
        }
        7 => {
            let (material, channel, layer) = target_read(r, l)?;
            let mut points = Vec::new();
            for _ in 0..r.count(256)? {
                points.push(read_array(r)?);
            }
            Stroke {
                material,
                channel,
                layer,
                points,
                radius: r.f64()?,
                hardness: r.f64()?,
                opacity: r.f64()?,
                color: read_array(r)?,
                mask: boolean(r)?,
            }
        }
        8 => {
            let (material, channel, layer) = target_read(r, l)?;
            ProjectedStroke {
                object: r.string(l.max_name_bytes)?,
                material,
                channel,
                layer,
                origin: read_array(r)?,
                direction: read_array(r)?,
                radius: r.f64()?,
                depth: r.f64()?,
                hardness: r.f64()?,
                opacity: r.f64()?,
                color: read_array(r)?,
                mask: boolean(r)?,
            }
        }
        9 => {
            let object = r.string(l.max_name_bytes)?;
            let mut vertices = Vec::new();
            for _ in 0..r.count(l.mesh.max_vertices)? {
                vertices.push(mesh::VertexId(r.u64()?));
            }
            VertexPaint {
                object,
                vertices,
                color: read_array(r)?,
                opacity: r.f64()?,
            }
        }
        10 => {
            let (material, channel, layer) = target_read(r, l)?;
            Bake {
                source: r.string(l.max_name_bytes)?,
                target: r.string(l.max_name_bytes)?,
                material,
                channel,
                layer,
                width: r.u32()?,
                height: r.u32()?,
                max_distance: r.f64()?,
                ao_samples: r.u32()?,
                ao_distance: r.f64()?,
                dilation: r.u32()?,
            }
        }
        11 => {
            let (material, channel, layer) = target_read(r, l)?;
            Dilate {
                material,
                channel,
                layer,
                iterations: r.u32()?,
                preserve_alpha: boolean(r)?,
            }
        }
        12 => {
            let (material, channel, layer) = target_read(r, l)?;
            Mips {
                material,
                channel,
                layer,
            }
        }
        13 => {
            let (material, channel, layer) = target_read(r, l)?;
            let recipe = derivation_read(r, l)?;
            recipe.validate(channel, l)?;
            Derive { material, channel, layer, recipe }
        }
        _ => return Err(Error::Corrupt("surface operation tag")),
    })
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    #[test]
    fn legacy_layer_fixture_reencodes_without_new_recipe_bytes() {
        // Captured layout predates derivation recipes: no optional fields may
        // appear in checkpoints that do not use surface_derive.
        let hex = "060000006c65676163790100000001000000040000004080c0ff00000000000000f03f00010000000000";
        let bytes: Vec<u8> = hex.as_bytes().chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect();
        let layer = layer_read(&mut Reader::new(&bytes), &Limits::default()).unwrap();
        assert_eq!(layer.id, "legacy");
        assert!(layer.derived.is_none());
        assert_eq!(layer.image.pixels, [64, 128, 192, 255]);
        let mut writer = Writer::new(1024);
        layer_write(&mut writer, &layer).unwrap();
        assert_eq!(writer.bytes, bytes);
    }
}
