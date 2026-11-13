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
        pixels: r.blob()?.to_vec(),
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
        7 => PatternKind::Bricks,
        8 => PatternKind::Tiles,
        9 => PatternKind::Planks,
        10 => PatternKind::Vents,
        11 => PatternKind::Grille,
        12 => PatternKind::Knurl,
        13 => PatternKind::Camo,
        14 => PatternKind::Leaves,
        15 => PatternKind::Needles,
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
fn derivation_read(r: &mut Reader<'_>, _limits: &Limits) -> Result<SurfaceDerivation> {
    Ok(SurfaceDerivation {
        source_channel: channel_read(r)?,
        source_layer: if boolean(r)? { Some(r.string()?) } else { None },
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
    let id = r.string()?;
    let image = image_read(r, l)?;
    let mask = if boolean(r)? {
        Some(r.blob()?.to_vec())
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
    for _ in 0..r.count(12)? {
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
    if m.shading.is_some_and(|s| s.msdf > 0.0 && s.crown > 0.0) {
        return Err(Error::Invalid("surface shading: msdf and crown never combine"));
    }
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
    // The legacy 0/1 layout stays byte-identical for materials without fur
    // or shading terms.
    // Bit 3: the shading block carries a rim term (only when set).
    let rim = m.shading.is_some_and(|s| s.rim > 0.0);
    // Bit 4: a number-plate layout (only when set).
    let plate = m.shading.is_some_and(|s| s.plate > 0.0);
    // Bit 5: a vehicle lamp role (only when set).
    let lamp = m.shading.and_then(|s| s.lamp);
    // Bit 6: the shading block carries distance-field terms.
    let msdf = m.shading.is_some_and(|s| s.msdf > 0.0);
    // Bit 7: with bit 6, the MSDF block carries plate cells (bit 4 is required).
    // With bit 6 clear, an extension byte follows wind, clearcoat, flake and
    // impostor, before rim. Its bit 0 marks crown normals; all other bits are
    // reserved. This unused flag combination preserves existing material bytes.
    let cells = m.shading.and_then(|s| s.msdf_plate_cells);
    let crown = m.shading.is_some_and(|s| s.crown > 0.0);
    w.u8(m.double_sided as u8 | ((m.fur.is_some() as u8) << 1) | ((m.shading.is_some() as u8) << 2) | ((rim as u8) << 3) | ((plate as u8) << 4) | ((lamp.is_some() as u8) << 5) | ((msdf as u8) << 6) | (((cells.is_some() || crown) as u8) << 7))?;
    if let Some(fur) = m.fur {
        for value in [fur.length, fur.density, fur.scale] { w.f64(value)?; }
        w.u32(fur.seed)?;
    }
    if let Some(s) = m.shading {
        for value in [s.wind, s.clearcoat, s.flake, s.impostor] { w.f64(value)?; }
        if crown { w.u8(1)?; }
        if rim { w.f64(s.rim)?; }
        if plate { w.f64(s.plate)?; }
        if let Some(lamp) = lamp { w.u8(lamp_code(lamp))?; }
        if msdf {
            write_array(w, &s.msdf_fill)?;
            write_array(w, &s.msdf_border)?;
            w.f64(s.msdf_border_width)?;
            w.f64(s.msdf_range)?;
            if let Some(cells) = cells { write_array(w, &cells)?; }
        }
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
fn lamp_code(lamp: makepad_gltf::GlbLamp) -> u8 {
    match lamp {
        makepad_gltf::GlbLamp::Tail => 0,
        makepad_gltf::GlbLamp::Brake => 1,
        makepad_gltf::GlbLamp::Reverse => 2,
    }
}
fn lamp_from_code(code: u8) -> Result<makepad_gltf::GlbLamp> {
    match code {
        0 => Ok(makepad_gltf::GlbLamp::Tail),
        1 => Ok(makepad_gltf::GlbLamp::Brake),
        2 => Ok(makepad_gltf::GlbLamp::Reverse),
        _ => Err(Error::Corrupt("surface lamp role")),
    }
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
    if (flags & 248 != 0 && flags & 4 == 0) || (flags & 192 == 192 && flags & 16 == 0) { return Err(Error::Corrupt("surface material flags")); }
    let double_sided = flags & 1 != 0;
    let fur = if flags & 2 != 0 {
        Some(makepad_gltf::GlbFurMaterial {
            length: r.f64()?, density: r.f64()?, scale: r.f64()?, seed: r.u32()?,
        })
    } else { None };
    let shading = if flags & 4 != 0 {
        let (wind, clearcoat, flake, impostor) = (r.f64()?, r.f64()?, r.f64()?, r.f64()?);
        let crown = if flags & 192 == 128 {
            let extension = r.u8()?;
            if extension & !1 != 0 { return Err(Error::Corrupt("surface shading extension flags")); }
            (extension & 1) as f64
        } else { 0.0 };
        let rim = if flags & 8 != 0 { r.f64()? } else { 0.0 };
        let plate = if flags & 16 != 0 { r.f64()? } else { 0.0 };
        let lamp = if flags & 32 != 0 { Some(lamp_from_code(r.u8()?)?) } else { None };
        let mut shading = makepad_gltf::GlbShading { wind, clearcoat, flake, impostor, rim, crown, plate, lamp, ..Default::default() };
        if flags & 64 != 0 {
            shading.msdf = 1.0;
            shading.msdf_fill = read_array(r)?;
            shading.msdf_border = read_array(r)?;
            shading.msdf_border_width = r.f64()?;
            shading.msdf_range = r.f64()?;
            if flags & 128 != 0 { shading.msdf_plate_cells = Some(read_array(r)?); }
        }
        Some(shading)
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
        shading,
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
    for _ in 0..r.count(114)? {
        let id = r.u32()?;
        let value = material_read(r, l)?;
        if s.materials.insert(id, value).is_some() {
            return Err(Error::Corrupt("duplicate surface material"));
        }
    }
    for _ in 0..r.count(44)? {
        let name = r.string()?;
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
fn target_read(r: &mut Reader<'_>, _l: &Limits) -> Result<(u32, SurfaceChannel, String)> {
    Ok((r.u32()?, channel_read(r)?, r.string()?))
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
        VertexColors { object, vertices, colors } => {
            w.u8(14)?;
            w.string(object)?;
            w.count(vertices.len())?;
            for (v, c) in vertices.iter().zip(colors) {
                w.u64(v.0)?;
                write_array(w, c)?;
            }
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
    let value = match r.u8()? {
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
                    Some(r.blob()?.to_vec())
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
            for _ in 0..r.count(16)? {
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
                object: r.string()?,
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
            let object = r.string()?;
            let mut vertices = Vec::new();
            for _ in 0..r.count(8)? {
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
                source: r.string()?,
                target: r.string()?,
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
        14 => {
            let object = r.string()?;
            let (mut vertices, mut colors) = (Vec::new(), Vec::new());
            for _ in 0..r.count(40)? {
                vertices.push(mesh::VertexId(r.u64()?));
                colors.push(read_array(r)?);
            }
            VertexColors { object, vertices, colors }
        }
        _ => return Err(Error::Corrupt("surface operation tag")),
    };
    value.check(l)?;
    Ok(value)
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
        let mut writer = Writer::new();
        layer_write(&mut writer, &layer).unwrap();
        assert_eq!(writer.bytes, bytes);
    }

    #[test]
    fn a_rim_term_roundtrips_and_leaves_older_shading_bytes_alone() {
        let encode = |m: &SurfaceMaterial| { let mut w = Writer::new(); material_write(&mut w, m).unwrap(); w.bytes };
        let coat = makepad_gltf::GlbShading { clearcoat: 1.0, flake: 0.5, ..Default::default() };
        let plain = SurfaceMaterial { shading: Some(coat), ..Default::default() };
        let toy = SurfaceMaterial { shading: Some(makepad_gltf::GlbShading { rim: 0.6, ..coat }), ..Default::default() };
        let (a, b) = (encode(&plain), encode(&toy));
        // The rim is one f64 more, flagged in bit 3; without it nothing moves.
        assert_eq!(b.len(), a.len() + 8);
        for bytes in [a, b] {
            let back = material_read(&mut Reader::new(&bytes), &Limits::default()).unwrap();
            assert_eq!(encode(&back), bytes);
        }
        let back = material_read(&mut Reader::new(&encode(&toy)), &Limits::default()).unwrap();
        assert_eq!(back.shading.unwrap().rim, 0.6);
    }

    #[test]
    fn msdf_roundtrips_without_changing_legacy_material_bytes() {
        let encode = |m: &SurfaceMaterial| { let mut w = Writer::new(); material_write(&mut w, m).unwrap(); w.bytes };
        // Legacy default material layout, independent of the optional MSDF block.
        let hex = "000000000000f03f000000000000f03f000000000000f03f000000000000f03f0000000000000000000000000000f03f000000000000f03f000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f00000000000000e03f0000000000";
        let legacy: Vec<u8> = hex.as_bytes().chunks_exact(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap()).collect();
        assert_eq!(encode(&SurfaceMaterial::default()), legacy, "ordinary materials retain their wire bytes");
        // The old shading block follows the flags and precedes the channel count.
        // Crown being off must preserve that layout, including an optional rim.
        let flags_at = legacy.len() - 5;
        let mut shaded_legacy = legacy.clone();
        shaded_legacy[flags_at] = 4 | 8;
        let terms = [0.25f64, 1.0, 0.5, 12.0, 0.6];
        shaded_legacy.splice(flags_at + 1..flags_at + 1, terms.into_iter().flat_map(f64::to_le_bytes));
        let shaded = SurfaceMaterial { shading: Some(makepad_gltf::GlbShading {
            wind: 0.25, clearcoat: 1.0, flake: 0.5, impostor: 12.0, rim: 0.6, ..Default::default()
        }), ..Default::default() };
        assert_eq!(encode(&shaded), shaded_legacy, "shading without crown retains its wire bytes");
        let mut material = SurfaceMaterial {
            shading: Some(makepad_gltf::GlbShading {
                plate: 1.0, msdf: 1.0, msdf_fill: [0.2, 0.4, 0.6, 0.8],
                msdf_border: [0.9, 0.1, 0.3, 1.0], msdf_border_width: 2.0, msdf_range: 8.0,
                msdf_plate_cells: Some([0.5, 0.1, 0.0, 0.1]), ..Default::default()
            }), ..Default::default()
        };
        let data = RgbaImage { width: 2, height: 1, pixels: vec![64, 128, 192, 0, 192, 64, 0, 255] };
        material.channels.insert(SurfaceChannel::BaseColor, vec![SurfaceLayer::new("text", data.clone())]);
        let bytes = encode(&material);
        let back = material_read(&mut Reader::new(&bytes), &Limits::default()).unwrap();
        assert_eq!(back, material, "colours, texel range and plate cells survive checkpointing");
        assert_eq!(encode(&back), bytes);
        let flat = back.flatten(SurfaceChannel::BaseColor, &Limits::default(), &mut mesh::Context::default()).unwrap().unwrap();
        assert_eq!(flat, data, "zero badge distance must not erase RGB text distance");
        let mips = flat.mip_chain(ImageEncoding::Linear, &Limits::default(), &mut mesh::Context::default()).unwrap();
        assert_eq!(mips[0].pixels, [128, 96, 96, 128], "data mips average all four distances independently");
    }

    #[test]
    fn crown_roundtrips_in_the_shading_extension_and_rejects_invalid_flags() {
        let encode = |m: &SurfaceMaterial| { let mut w = Writer::new(); material_write(&mut w, m).unwrap(); w.bytes };
        // The default material ends with its flags and the four-byte channel count.
        let flags_at = encode(&SurfaceMaterial::default()).len() - 5;
        let extension_at = flags_at + 1 + 4 * 8;
        for shading in [
            makepad_gltf::GlbShading::default(),
            makepad_gltf::GlbShading { wind: 0.25, clearcoat: 1.0, flake: 0.5, impostor: 12.0, rim: 0.6, plate: 1.0, lamp: Some(makepad_gltf::GlbLamp::Tail), ..Default::default() },
        ] {
            let plain = SurfaceMaterial { shading: Some(shading), ..Default::default() };
            let material = SurfaceMaterial { shading: Some(makepad_gltf::GlbShading { crown: 1.0, ..shading }), ..Default::default() };
            let bytes = encode(&material);
            let mut expected = encode(&plain);
            expected[flags_at] |= 128;
            expected.insert(extension_at, 1);
            assert_eq!(bytes, expected, "crown adds only bit 7 and an extension byte before rim");
            let mut reader = Reader::new(&bytes);
            let back = material_read(&mut reader, &Limits::default()).unwrap();
            reader.end().unwrap();
            assert_eq!(back, material, "crown and following optional terms survive checkpointing");
            assert_eq!(encode(&back), bytes);
            let mut corrupt = bytes.clone();
            for bit in 1..8 {
                corrupt[extension_at] = 1 | (1 << bit);
                assert!(matches!(material_read(&mut Reader::new(&corrupt), &Limits::default()), Err(Error::Corrupt(_))), "reserved extension bit {bit} must be rejected");
            }
            let mut absent = bytes.clone();
            absent[extension_at] = 0;
            assert_eq!(material_read(&mut Reader::new(&absent), &Limits::default()).unwrap(), plain, "a clear crown bit means off");
            corrupt[flags_at] &= !4;
            assert!(matches!(material_read(&mut Reader::new(&corrupt), &Limits::default()), Err(Error::Corrupt(_))), "extensions require a shading block");
        }
        let material = SurfaceMaterial { shading: Some(makepad_gltf::GlbShading {
            crown: 1.0, msdf: 1.0, msdf_range: 8.0, ..Default::default()
        }), ..Default::default() };
        let mut writer = Writer::new();
        assert_eq!(material_write(&mut writer, &material), Err(Error::Invalid("surface shading: msdf and crown never combine")));
        assert!(writer.bytes.is_empty(), "incompatible shading is rejected before writing");
    }

    #[test]
    fn a_lamp_role_roundtrips_in_one_byte() {
        let encode = |m: &SurfaceMaterial| { let mut w = Writer::new(); material_write(&mut w, m).unwrap(); w.bytes };
        let plain = SurfaceMaterial { shading: Some(makepad_gltf::GlbShading::default()), ..Default::default() };
        let tail = SurfaceMaterial { shading: Some(makepad_gltf::GlbShading { lamp: Some(makepad_gltf::GlbLamp::Reverse), ..Default::default() }), ..Default::default() };
        let (a, b) = (encode(&plain), encode(&tail));
        assert_eq!(b.len(), a.len() + 1);
        let back = material_read(&mut Reader::new(&b), &Limits::default()).unwrap();
        assert_eq!(back.shading.unwrap().lamp, Some(makepad_gltf::GlbLamp::Reverse));
        assert_eq!(encode(&back), b);
    }
}
