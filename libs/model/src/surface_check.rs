use super::*;
impl SurfaceOperation {
    pub fn check(&self, limits: &Limits) -> Result<()> {
        fn finite(values: &[f64]) -> Result<()> {
            if values.iter().any(|v| !v.is_finite()) { Err(Error::Invalid("expected finite number")) } else { Ok(()) }
        }
        fn mask_bytes(mask: &Option<Vec<u8>>, limits: &Limits) -> Result<()> {
            if mask.as_ref().is_some_and(|m| m.len() > limits.max_texture_bytes / 4) { Err(Error::Budget("surface image bytes")) } else { Ok(()) }
        }
        use SurfaceOperation::*;
        match self {
            Material { value, .. } => value.validate(limits, &mut mesh::Context::new(limits.mesh.clone(), None))?,
            Layer { channel, layer, .. } => {
                layer.image.validate(limits)?; finite(&[layer.opacity])?; mask_bytes(&layer.mask, limits)?;
                if layer.pattern.as_ref().is_some_and(|p| !pattern_valid(p)) { return Err(Error::Invalid("surface pattern")); }
                if let Some(recipe) = &layer.derived { recipe.validate(*channel, limits)?; }
                let (mut width, mut height) = (layer.image.width, layer.image.height);
                for mip in &layer.mips {
                    if width == 1 && height == 1 { return Err(Error::Invalid("extra mip level")); }
                    width = (width / 2).max(1); height = (height / 2).max(1); mip.validate(limits)?;
                    if (mip.width, mip.height) != (width, height) { return Err(Error::Invalid("surface mip dimensions")); }
                }
                if !layer.mips.is_empty() && (width, height) != (1, 1) { return Err(Error::Invalid("incomplete mip chain")); }
            }
            Mask { mask, .. } => mask_bytes(mask, limits)?,
            Pattern { pattern, .. } => {
                finite(&pattern.color_a)?; finite(&pattern.color_b)?; finite(&pattern.scale)?;
                if !pattern_valid(pattern) { return Err(Error::Invalid("surface pattern")); }
            }
            Derive { channel, recipe, .. } => recipe.validate(*channel, limits)?,
            Stroke { points, radius, hardness, opacity, color, .. } => {
                if points.is_empty() || points.len() > 256 { return Err(Error::Budget("stroke points")); }
                for p in points { finite(p)?; } finite(&[*radius, *hardness, *opacity])?; finite(color)?;
            }
            ProjectedStroke { origin, direction, radius, depth, hardness, opacity, color, .. } => {
                finite(origin)?; finite(direction)?; finite(&[*radius, *depth, *hardness, *opacity])?; finite(color)?;
            }
            VertexPaint { vertices, color, opacity, .. } => {
                if vertices.len() > limits.mesh.max_vertices { return Err(Error::Budget("selection size")); }
                finite(color)?; finite(&[*opacity])?;
            }
            VertexColors { vertices, colors, .. } => {
                if vertices.len() > limits.mesh.max_vertices { return Err(Error::Budget("selection size")); }
                for color in colors { finite(color)?; }
            }
            Bake { max_distance, ao_distance, .. } => finite(&[*max_distance, *ao_distance])?,
            MoveLayer { index, .. } => if *index > u32::MAX as usize { return Err(Error::Invalid("expected u32 integer")); },
            _ => {}
        }
        if self.memory_bytes() > limits.max_transaction_bytes { return Err(Error::Budget("surface operation bytes")); }
        Ok(())
    }
}

#[cfg(test)]
mod typed_tests {
    use super::*;
    fn pattern() -> SurfacePattern { SurfacePattern { kind: PatternKind::Checker, color_a: [0.; 4], color_b: [1.; 4], scale: [8.; 2], seed: u64::MAX } }
    fn recipe() -> SurfaceDerivation { SurfaceDerivation { source_channel: SurfaceChannel::BaseColor, source_layer: Some("height".into()), strength: 1., roughness_min: 0.2, roughness_max: 0.9, metallic: 0., wrap: true } }
    fn roundtrip(op: &SurfaceOperation, l: &Limits) {
        op.check(l).unwrap();
        let mut w = Writer::new(usize::MAX); write_surface_operation(&mut w, op).unwrap();
        let mut r = Reader::new(&w.bytes); let decoded = read_surface_operation(&mut r, l).unwrap(); r.end().unwrap(); assert_eq!(&decoded, op);
        let mut out = Writer::new(usize::MAX); write_surface_operation(&mut out, &decoded).unwrap(); assert_eq!(w.bytes, out.bytes);
    }
    #[test]
    fn literal_operations_use_existing_codec() {
        let l = Limits::default(); let channel = SurfaceChannel::BaseColor;
        let image = RgbaImage { width: 1, height: 1, pixels: vec![128, 64, 32, 255] };
        let layer = SurfaceLayer { id: "paint".into(), image, mask: Some(vec![255]), opacity: 0.5, blend: SurfaceBlend::Multiply, visible: true, pattern: Some(pattern()), derived: None, mips: vec![] };
        let ops = vec![
            SurfaceOperation::Material { material: 0, value: SurfaceMaterial::default() },
            SurfaceOperation::RemoveMaterial { material: 0 },
            SurfaceOperation::Layer { material: 0, channel, layer },
            SurfaceOperation::RemoveLayer { material: 0, channel, layer: "paint".into() },
            SurfaceOperation::MoveLayer { material: 0, channel, layer: "paint".into(), index: 1 },
            SurfaceOperation::Mask { material: 0, channel, layer: "paint".into(), mask: Some(vec![255]) },
            SurfaceOperation::Pattern { material: 0, channel, layer: "paint".into(), pattern: pattern() },
            SurfaceOperation::Derive { material: 0, channel: SurfaceChannel::Normal, layer: "normal".into(), recipe: recipe() },
            SurfaceOperation::Stroke { material: 0, channel, layer: "paint".into(), points: vec![[0.2, 0.3], [0.7, 0.8]], radius: 0.1, hardness: 0.5, opacity: 1., color: [1.; 4], mask: false },
            SurfaceOperation::ProjectedStroke { object: "body".into(), material: 0, channel, layer: "paint".into(), origin: [0.; 3], direction: [0., 0., 1.], radius: 1., depth: 2., hardness: 0.5, opacity: 1., color: [1.; 4], mask: true },
            SurfaceOperation::VertexPaint { object: "body".into(), vertices: vec![mesh::VertexId(u64::MAX)], color: [1.; 4], opacity: 0.5 },
            SurfaceOperation::VertexColors { object: "body".into(), vertices: vec![mesh::VertexId(u64::MAX)], colors: vec![[1.; 4]] },
            SurfaceOperation::Bake { source: "source".into(), target: "target".into(), material: 0, channel, layer: "paint".into(), width: 8, height: 8, max_distance: 1., ao_samples: 16, ao_distance: 1., dilation: 2 },
            SurfaceOperation::Dilate { material: 0, channel, layer: "paint".into(), iterations: 2, preserve_alpha: true },
            SurfaceOperation::Mips { material: 0, channel, layer: "paint".into() },
        ];
        for op in &ops { roundtrip(op, &l); }
        roundtrip(&SurfaceOperation::Mask { material: 0, channel, layer: "paint".into(), mask: None }, &l);
        for kind in [PatternKind::Checker, PatternKind::Stripes, PatternKind::Gradient, PatternKind::Noise, PatternKind::Perlin, PatternKind::Fbm, PatternKind::Yarn, PatternKind::Bricks, PatternKind::Tiles, PatternKind::Planks, PatternKind::Vents, PatternKind::Grille, PatternKind::Knurl, PatternKind::Camo, PatternKind::Leaves, PatternKind::Needles] {
            roundtrip(&SurfaceOperation::Pattern { material: 0, channel, layer: "paint".into(), pattern: SurfacePattern { kind, ..pattern() } }, &l);
        }
    }
    #[test]
    fn typed_checks_enforce_defaults_and_budgets() {
        let l = Limits::default();
        for op in [
            SurfaceOperation::Material { material: 0, value: SurfaceMaterial::default() },
            SurfaceOperation::Stroke {
                material: 0,
                channel: SurfaceChannel::BaseColor,
                layer: "paint".into(),
                points: vec![[0., 1.]],
                radius: 0.1,
                hardness: 0.5,
                opacity: 1.,
                color: [1.; 4],
                mask: false
            },
        ] { roundtrip(&op, &l); }
        for op in [
            SurfaceOperation::Material { material: 0, value: SurfaceMaterial { metallic: 2., ..Default::default() } },
            SurfaceOperation::Pattern { material: 0, channel: SurfaceChannel::BaseColor, layer: "paint".into(), pattern: SurfacePattern { scale: [0.; 2], ..pattern() } },
            SurfaceOperation::Derive { material: 0, channel: SurfaceChannel::BaseColor, layer: "paint".into(), recipe: recipe() },
            SurfaceOperation::Stroke { material: 0, channel: SurfaceChannel::BaseColor, layer: "paint".into(), points: vec![], radius: 1., hardness: 0.5, opacity: 1., color: [1.; 4], mask: false },
            SurfaceOperation::VertexPaint { object: "body".into(), vertices: vec![], color: [f64::NAN; 4], opacity: 1. },
        ] { assert!(op.check(&l).is_err()); }
        let tiny = Limits { max_texture_bytes: 4, max_transaction_bytes: 1, ..l.clone() };
        assert!(SurfaceOperation::Mask { material: 0, channel: SurfaceChannel::BaseColor, layer: "paint".into(), mask: Some(vec![255; 2]) }.check(&tiny).is_err());
        assert!(SurfaceOperation::RemoveMaterial { material: 0 }.check(&tiny).is_err());
        assert!(matches!(read_surface_operation(&mut Reader::new(&[255]), &l), Err(Error::Corrupt(_))));
    }
}
