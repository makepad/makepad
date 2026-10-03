use super::*;
use makepad_scene::{SeaLook, WaterWave};

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
        look: SeaLook::default(),
        unbounded: false,
        mesh: None,
    }
}

fn water_decl() -> &'static str {
    let src = crate::shaders::SHADER_SOURCE;
    let at = src.find("mod.draw.DrawSceneWater").expect("water shader missing");
    let end = src[at..].find("\n    mod.draw.").map_or(src.len(), |e| at + e);
    &src[at..end.max(at + 1)]
}

/// The CPU/GPU agreement gate for the swell, pinned structurally (a
/// headless unit test cannot run the GPU):
///
/// (a) the uniform packer hands the shader the sim's RAW `WaterWave`
///     fields, bit-for-bit, in the documented slot layout;
/// (b) the shader evaluates the EXACT canonical expression the sim
///     documents (`sim::water::wave_terms`: phase, set envelope, height,
///     slope — including the same omission of the envelope derivative)
///     over those slots, asserted as source text, and displaces the
///     vertex by it at the volume's level.
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
    let decl = water_decl();
    for expr in [
        "let phase = wa.z * (wa.x * p.x + wa.y * p.y) - wa.w * t + wb.y",
        "let e = 0.5 + 0.5 * cos(phase / wb.z)",
        "env = e * e",
        "let slope = wb.x * env * cos(phase) * wa.z",
        "return vec3(wb.x * env * sin(phase), slope * wa.x, slope * wa.y)",
        "let t = self.water_params.y",
        "let pos = vec3(xz.x, base + acc.x, xz.y)",
    ] {
        assert!(decl.contains(expr), "water shader lost the canonical expression: {expr}");
    }
    for i in 0..MAX_WAVES {
        assert!(decl.contains(&format!("self.wave_a{i}, self.wave_b{i}, t)")), "wave slot {i} not summed");
    }
}

/// The CPU twin the renderer uses for "is the eye under water" is the
/// same sum the shader displaces by.
#[test]
fn swell_height_matches_the_canonical_sum() {
    let v = test_volume();
    let w = v.waves[0];
    let (x, z, t) = (3.0f32, -2.0f32, 7.5f32);
    let phase = w.k * (w.dir_x * x + w.dir_z * z) - w.omega * t + w.phase;
    let e = 0.5 + 0.5 * (phase / w.group).cos();
    let expect = v.level() + w.amp * e * e * phase.sin();
    assert!((v.swell_height(x, z, t) - expect).abs() < 1.0e-4);
    let view = WaterView { rev: 1, volumes: vec![v], bed: None };
    assert!(eye_under_water(&view, vec3f(x, expect - 0.1, z), t).is_some());
    assert!(eye_under_water(&view, vec3f(x, expect + 0.1, z), t).is_none());
    assert!(eye_under_water(&view, vec3f(50.0, -1.0, 0.0), t).is_none(), "outside the box");
}

/// The surface mesh: indices in range, every triangle facing up, the
/// first ring fine, the last at the horizon, and about as many vertices as
/// one old 128² sheet (the user's GPU budget).
#[test]
fn the_ring_mesh_is_well_formed_and_cheap() {
    let (vertices, indices) = water::water_ring_mesh_data();
    assert_eq!(vertices.len() % 16, 0, "PbrVertex stride");
    let n = vertices.len() / 16;
    assert!(n < 24_000, "{n} vertices");
    assert_eq!(indices.len() % 3, 0);
    assert!(indices.iter().all(|i| (*i as usize) < n));
    let p = |i: u32| {
        let o = i as usize * 16;
        vec3f(vertices[o], vertices[o + 1], vertices[o + 2])
    };
    for tri in indices.chunks(3) {
        let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
        let ny = Vec3f::cross(b - a, c - a).y;
        assert!(ny > 0.0, "triangle {tri:?} faces down");
    }
    let far = (0..n).map(|i| p(i as u32).length()).fold(0.0f32, f32::max);
    assert!(far >= 30_000.0, "horizon ring at {far}");
    // Spacing grows with distance.
    assert!(vertices[16 + 3] < 0.05 && vertices[(n - 1) * 16 + 3] > 1000.0);
}
