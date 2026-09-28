use super::*;
use makepad_scene::WaterWave;

fn test_volume() -> WaterSurface {
    let mut wave = WaterWave::new(0.6, -0.8, 0.7, 18.0, 5.0);
    wave.phase = 1.25;
    wave.group = 4.0;
    WaterSurface {
        min: vec3f(-10.0, -5.0, -10.0),
        max: vec3f(10.0, 0.0, 10.0),
        waves: vec![wave],
        color: vec4(0.2, 0.5, 0.8, 0.6),
        entity: 0,
        draw_sheet: true,
    }
}

/// The W1 CPU/GPU agreement gate, pinned structurally (a headless unit
/// test cannot run the GPU — documented approach):
///
/// (a) the uniform packer hands the shader the sim's RAW `WaterWave`
///     fields, bit-for-bit, in the documented slot layout — no unit
///     conversion exists to drift;
/// (b) the shader source evaluates the EXACT canonical expression the
///     sim documents (`sim::water::wave_terms`: phase, set envelope,
///     height, slope — including the same deliberate omission of the
///     envelope derivative) over those slots, asserted as source text.
///
/// Editing either side of the agreement forces an edit here, which is
/// the agreement being enforced.
#[test]
fn the_shader_evaluates_the_sims_wave_expression_over_the_sims_coefficients() {
    let volume = test_volume();
    let w = volume.waves[0];
    let (a, b) = pack_wave_uniforms(&volume);
    assert_eq!(a[0], [w.dir_x, w.dir_z, w.k, w.omega], "wave_a slot layout");
    assert_eq!(b[0], [w.amp, w.phase, w.group, 0.0], "wave_b slot layout");
    for i in 1..MAX_WAVES {
        assert_eq!(a[i], [0.0; 4], "unused wave_a slot {i} must stay zero");
        assert_eq!(b[i], [0.0; 4], "unused wave_b slot {i} must stay zero");
    }

    let src = crate::shaders::SHADER_SOURCE;
    let at = src.find("mod.draw.DrawSceneWater").expect("water shader missing");
    let end = src[at..].find("\n    mod.draw.").map_or(src.len(), |e| at + e);
    let decl = &src[at..end.max(at + 1)];
    for expr in [
        // phase = k·(dir·p) − ω·t + phase0
        "let phase = wa.z * (wa.x * p.x + wa.y * p.y) - wa.w * t + wb.y",
        // set envelope e², e = ½ + ½·cos(phase/group)
        "let e = 0.5 + 0.5 * cos(phase / wb.z)",
        "env = e * e",
        // slope shared term (envelope derivative omitted, like the CPU)
        "let slope = wb.x * env * cos(phase) * wa.z",
        // (height, dh/dx, dh/dz)
        "return vec3(wb.x * env * sin(phase), slope * wa.x, slope * wa.y)",
        // one shared time base
        "let t = self.water_params.y",
    ] {
        assert!(
            decl.contains(expr),
            "water shader lost the canonical expression `{expr}` — update \
             sim::water and this pin TOGETHER or physics and visuals split"
        );
    }
    // Slot count stays in lockstep with the sim's cap and the renderer's
    // uniform id tables.
    assert_eq!(MAX_WAVES, 8, "MAX_WAVES moved: update wave_a*/wave_b* slots");
    assert!(decl.contains("wave_a7:") && !decl.contains("wave_a8"));
}

#[test]
fn the_sheet_grid_spans_the_volume_at_the_still_level() {
    let volume = test_volume();
    let (vertices, indices, min, max) = water_sheet_data(&volume);
    assert!(!indices.is_empty());
    assert_eq!(vertices.len() % 16, 0, "PbrVertex stride");
    // Every vertex sits at the still level inside the bounds; the wave
    // headroom lives in the culling box, not the mesh.
    for v in vertices.chunks_exact(16) {
        assert_eq!(v[1], volume.level());
        assert!(v[0] >= volume.min.x - 1.0e-3 && v[0] <= volume.max.x + 1.0e-3);
        assert!(v[2] >= volume.min.z - 1.0e-3 && v[2] <= volume.max.z + 1.0e-3);
        // The volume's color rides per vertex (alpha = translucency).
        assert_eq!(v[11], volume.color.w);
    }
    assert!(min.y < volume.level() && max.y > volume.level());
    // Indices address real vertices.
    let count = (vertices.len() / 16) as u32;
    assert!(indices.iter().all(|i| *i < count));
}

/// A sea kilometres wide gets cells far longer than its waves (the grid is
/// capped at 128 a side). Displacing those waves per vertex aliased them
/// into long radial stripes (the "light shafts" of the flight review), so
/// the shader must fade every wave by the cell it is sampled at, and shade
/// normals per pixel by the pixel's footprint.
#[test]
fn waves_the_grid_cannot_resolve_fade_instead_of_striping() {
    let mut sea = test_volume();
    sea.min = vec3f(-30_000.0, -20.0, -30_000.0);
    sea.max = vec3f(30_000.0, 0.0, 30_000.0);
    sea.waves = vec![WaterWave::new(1.0, 0.0, 0.7, 70.0, 6.0), WaterWave::new(0.6, 0.8, 0.35, 28.0, 4.0)];
    let cell = water_sheet_cell(&sea);
    assert!(cell > 400.0, "a 60 km sea at 128 cells: {cell} m cells");
    // Same fade the shader uses: 0 once a wavelength spans < 4 cells.
    let fade = |k: f32, f: f32| (2.0 - k * f / std::f32::consts::FRAC_PI_4).clamp(0.0, 1.0);
    for w in &sea.waves {
        assert_eq!(fade(w.k, cell), 0.0, "a {:.0} m wave on {cell:.0} m cells must not displace", std::f32::consts::TAU / w.k);
    }
    assert_eq!(fade(sea.waves[1].k, 0.5), 1.0, "up close (0.5 m pixels) the 28 m wave shows");
    assert_eq!(water_sheet_cell(&test_volume()) <= 16.0, true);

    let src = crate::shaders::SHADER_SOURCE;
    let at = src.find("mod.draw.DrawSceneWater").expect("water shader missing");
    let end = src[at..].find("\n    mod.draw.").map_or(src.len(), |e| at + e);
    let decl = &src[at..end];
    assert!(decl.contains("return clamp(2.0 - k * f / 0.7853982, 0.0, 1.0)"));
    for i in 0..MAX_WAVES {
        assert!(decl.contains(&format!("self.wave_term(pos_in.xz, self.wave_a{i}, self.wave_b{i}, t) * self.wave_fade(self.wave_a{i}.z, cell)")));
        assert!(decl.contains(&format!("self.px_slope(p, self.wave_a{i}, self.wave_b{i}, t, f)")));
    }
    assert!(decl.contains("let ws = self.wave_slope(self.v_wp.xz, self.water_params.y, foot)"));
    // The pixel stage has its own copy of the slope: it must never call the
    // vertex stage's helpers (Metal emits a helper for one stage only).
    let pixel = &decl[decl.find("pixel: fn()").expect("water pixel")..];
    assert!(!pixel.contains("self.wave_term(") && !pixel.contains("self.wave_fade("));
    assert!(decl.contains("let slope = wb.x * env * cos(phase) * wa.z * self.px_fade(wa.z, f)"));
}
