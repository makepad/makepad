//! Working-tree measurement rig for the Rendered-pane roof noise: loads a
//! GLB through the real loader + scene build (overlap analysis included),
//! rebuilds the tracer's `SceneInput` exactly as the seam does, and then
//! histograms per-sample primary hits and sun-shadow visibility on roof
//! pixels with the bit-exact CPU twin.
//!
//! Run explicitly (skipped without the env):
//! ```sh
//! FAB_PROBE_GLB=/abs/path/woodside.glb cargo test -p makepad-fab --release --test roof_probe -- --nocapture
//! ```

use fab::model::{LoadCancel, Loader, Scene, SceneSnapshot};
use fab::model::makepad_math::{vec3f, Vec3f};
use makepad_raytrace::{cpu_ref, pack::PackedScene, Camera, SceneInput, Sun};

fn v3(a: [f32; 3]) -> Vec3f {
    vec3f(a[0], a[1], a[2])
}

fn scene_input_all_visible(snap: &SceneSnapshot) -> SceneInput {
    let mut s = SceneInput { up: vec3f(0.0, 0.0, 1.0), ..Default::default() };
    s.positions = snap.positions.clone();
    s.normals = snap.normals.clone();
    s.uvs = snap.uvs.clone();
    let n_tris = snap.indices.len() / 3;
    for t in 0..n_tris {
        s.indices.extend_from_slice(&snap.indices[t * 3..t * 3 + 3]);
        s.tri_material.push(snap.triangle_material.get(t).copied().unwrap_or(0));
        s.tri_priority.push(snap.triangle_priority.get(t).copied().unwrap_or(0));
        s.tri_coplanar_group.push(snap.triangle_coplanar_group.get(t).copied().unwrap_or(0));
    }
    s.materials = snap
        .materials
        .iter()
        .map(|m| makepad_raytrace::Material {
            albedo: [m.albedo[0], m.albedo[1], m.albedo[2]],
            roughness: m.roughness,
            metal: m.metallic,
            emission: m.emission,
            ior: if m.ior > 1.0 { m.ior } else { 1.5 },
            transmission: m.transmission,
            texture: if m.texture == u32::MAX { None } else { Some(m.texture as usize) },
            two_sided: m.double_sided,
        })
        .collect();
    if s.materials.is_empty() {
        s.materials.push(makepad_raytrace::Material::default());
    }
    s.images = snap
        .textures
        .iter()
        .map(|t| {
            let data = t
                .rgba
                .chunks_exact(4)
                .map(|p| {
                    ((p[3] as u32) << 24) | ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32
                })
                .collect();
            makepad_raytrace::Image { width: t.width as usize, height: t.height as usize, data }
        })
        .collect();
    s
}

#[test]
fn roof_hit_distribution() {
    let Ok(path) = std::env::var("FAB_PROBE_GLB") else {
        eprintln!("roof_probe: FAB_PROBE_GLB not set; skipping");
        return;
    };
    let loader = makepad_fab_loader_gltf::GltfLoader;
    let cancel_fn = || false;
    let cancel: LoadCancel = &cancel_fn;
    let document = loader
        .load_cancellable(std::path::Path::new(&path), &mut |_| {}, cancel)
        .expect("load glb");
    let scene = Scene::from_document_with(document, &mut |_, _| {});
    let snap = SceneSnapshot::from_scene(&scene);
    eprintln!(
        "probe: {} tris, {} materials, priorities non-zero {} groups non-zero {}",
        snap.indices.len() / 3,
        snap.materials.len(),
        snap.triangle_priority.iter().filter(|p| **p != 0).count(),
        snap.triangle_coplanar_group.iter().filter(|g| **g != 0).count(),
    );
    // FAB_PROBE_MAT=1: per-material triangle counts, texture presence and
    // uv spread — which surfaces the tracer shades flat that the raster
    // textures (the lawn parity question).
    if std::env::var("FAB_PROBE_MAT").is_ok() {
        let n_tris = snap.indices.len() / 3;
        let nm = snap.materials.len();
        let mut tri_count = vec![0usize; nm];
        let mut uv_min = vec![[f32::MAX; 2]; nm];
        let mut uv_max = vec![[f32::MIN; 2]; nm];
        for t in 0..n_tris {
            let m = snap.triangle_material.get(t).copied().unwrap_or(0) as usize;
            if m >= nm {
                continue;
            }
            tri_count[m] += 1;
            for k in 0..3 {
                let vi = snap.indices[t * 3 + k] as usize;
                if let Some(uv) = snap.uvs.get(vi) {
                    for a in 0..2 {
                        uv_min[m][a] = uv_min[m][a].min(uv[a]);
                        uv_max[m][a] = uv_max[m][a].max(uv[a]);
                    }
                }
            }
        }
        for (m, mat) in snap.materials.iter().enumerate() {
            if tri_count[m] == 0 {
                continue;
            }
            let tex = if mat.texture == u32::MAX {
                "none".to_string()
            } else {
                let t = &snap.textures[mat.texture as usize];
                format!("#{} {}x{}", mat.texture, t.width, t.height)
            };
            eprintln!(
                "mat {m:2}: tris {:6} albedo {:?} rough {:.2} tex {} uv [{:.2},{:.2}]..[{:.2},{:.2}]",
                tri_count[m], mat.albedo, mat.roughness, tex,
                uv_min[m][0], uv_min[m][1], uv_max[m][0], uv_max[m][1]
            );
        }
    }
    let mut input = scene_input_all_visible(&snap);
    // The interactive camera + sun of the live repro session (seam log).
    input.camera = Camera {
        pos: v3([-5.297323, 17.044632, 23.215685]),
        target: v3([-31.696442, 64.76611, 3.9240212]),
        up: v3([0.0, 0.0, 1.0]),
        fov_y: 40.0f32.to_radians(),
        ..Default::default()
    };
    input.sun = Sun {
        dir: v3([-0.4174682, 0.33721808, 0.84380347]),
        turbidity: 3.0,
        sky_strength: 1.0,
        sun_strength: 4.0,
    };
    let packed = PackedScene::pack(&input);
    if input.materials.len() > 18 {
        let base = 18 * 16;
        eprintln!(
            "probe: images {} atlas {:?}; mat18 tex flag {} rect {:?}",
            input.images.len(),
            packed.atlas.as_ref().map(|a| (a.width, a.height)),
            packed.mat.data[base + 10],
            &packed.mat.data[base + 12..base + 16],
        );
        // The grass image's own contrast: if the source texels are nearly
        // uniform, a flat traced lawn is faithful and the raster's texture
        // comes from somewhere else.
        if let Some(img) = input.images.get(8) {
            let mut lum: Vec<f32> = img
                .data
                .iter()
                .map(|p| {
                    let r = ((p >> 16) & 255) as f32;
                    let g = ((p >> 8) & 255) as f32;
                    let b = (p & 255) as f32;
                    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
                })
                .collect();
            lum.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mean = lum.iter().sum::<f32>() / lum.len() as f32;
            eprintln!(
                "probe: image8 {}x{} lum mean {:.3} p5 {:.3} p95 {:.3}",
                img.width, img.height, mean,
                lum[lum.len() / 20], lum[lum.len() * 19 / 20]
            );
        }
    }
    let mut tracer = cpu_ref::cpu_tracer(&input, &packed);
    if std::env::var("FAB_PROBE_NO_SKIN").is_ok() {
        tracer.shadow_skin = 0.0;
    }
    eprintln!("probe: shadow_skin {:.4} m", tracer.shadow_skin);
    let (w, h) = (1234u32, 1488u32); // the live pane's native size
    let sun_dir = input.sun.dir;

    // Pass 1: primary hits at pixel centre; find roof pixels (steep upward
    // normal) in a native-resolution band across the roof area.
    let mut roof_pixels: Vec<(u32, u32)> = Vec::new();
    for py in (h * 32 / 100..h * 42 / 100).step_by(3) {
        for px in (0..w).step_by(3) {
            let hit = tracer.primary_hit(px, py, w, h, (0.0, 0.0));
            if hit.x < 0.0 {
                continue;
            }
            let ti = hit.x as usize;
            let i0 = input.indices[ti * 3] as usize;
            let i1 = input.indices[ti * 3 + 1] as usize;
            let i2 = input.indices[ti * 3 + 2] as usize;
            let p0 = v3(input.positions[i0]);
            let p1 = v3(input.positions[i1]);
            let p2 = v3(input.positions[i2]);
            let ng = Vec3f::cross(p1 - p0, p2 - p0).normalize();
            // Roof slope: normal well off vertical walls, well off flat ground.
            if ng.z.abs() > 0.3 && ng.z.abs() < 0.95 {
                roof_pixels.push((px, py));
            }
        }
    }
    eprintln!("probe: {} roof-slope pixels of {}", roof_pixels.len(), w * h);

    // Pass 2: per-sample primary hit + sun shadow on those pixels.
    let n_samples = 32u32;
    let mut alternating_layer = 0usize;
    let mut cross_group_alternating = 0usize;
    let mut cross_group_examples: Vec<(u32, u32, f32, Vec<(i32, u32, u16)>)> = Vec::new();
    let mut alternating_shadow = 0usize;
    let mut dark_total = 0u64;
    let mut lit_total = 0u64;
    let mut sampled = 0usize;
    let mut worst: Vec<(u32, u32, usize, f32, f32)> = Vec::new();
    for &(px, py) in &roof_pixels {
        let mut tris = std::collections::HashMap::<i32, u32>::new();
        let mut t_min = f32::MAX;
        let mut t_max = 0.0f32;
        let mut shadow_lit = 0u32;
        let mut shadow_dark = 0u32;
        let mut facing_away = 0u32;
        for sidx in 0..n_samples {
            let (tri, t, sh) = tracer.probe_primary_and_sun(px, py, w, h, 1, sidx, sun_dir);
            if tri < 0 {
                continue;
            }
            *tris.entry(tri).or_insert(0) += 1;
            t_min = t_min.min(t);
            t_max = t_max.max(t);
            if sh < -1.0 {
                facing_away += 1;
            } else if sh > 0.5 {
                shadow_lit += 1;
            } else {
                shadow_dark += 1;
            }
        }
        let _ = facing_away;
        if tris.is_empty() {
            continue;
        }
        dark_total += shadow_dark as u64;
        lit_total += shadow_lit as u64;
        sampled += 1;
        // Distinct triangles whose depth band is tight = stacked layers, not
        // a silhouette edge (silhouettes have big depth spread).
        let spread = t_max - t_min;
        if tris.len() > 1 && spread < 0.05 {
            alternating_layer += 1;
            // Same coplanar group = the deterministic priority tie-break
            // already owns the decision; cross-group / zero-group pairs are
            // the ones a per-sample flip can still slip through.
            let groups: std::collections::HashSet<u32> = tris
                .keys()
                .map(|t| input.tri_coplanar_group.get(*t as usize).copied().unwrap_or(0))
                .collect();
            if groups.len() > 1 || groups.contains(&0) {
                if cross_group_examples.len() < 8 {
                    let detail: Vec<(i32, u32, u16)> = tris
                        .keys()
                        .map(|t| {
                            (
                                *t,
                                input.tri_coplanar_group.get(*t as usize).copied().unwrap_or(0),
                                input.tri_priority.get(*t as usize).copied().unwrap_or(0),
                            )
                        })
                        .collect();
                    cross_group_examples.push((px, py, spread, detail));
                }
                cross_group_alternating += 1;
            }
        }
        if shadow_lit > 0 && shadow_dark > 0 {
            alternating_shadow += 1;
            let dark = shadow_dark as f32 / (shadow_lit + shadow_dark) as f32;
            if (0.2..=0.8).contains(&dark) && worst.len() < 12 {
                worst.push((px, py, tris.len(), spread, dark));
            }
        }
    }
    eprintln!(
        "probe: of {} sampled roof pixels — {} alternate between stacked layers (<5 cm band), {} of those cross groups, {} alternate sun shadow; dark samples {} / lit {} ({:.1} % dark)",
        sampled, alternating_layer, cross_group_alternating, alternating_shadow, dark_total, lit_total,
        100.0 * dark_total as f64 / (dark_total + lit_total).max(1) as f64
    );
    for (px, py, spread, detail) in &cross_group_examples {
        eprintln!("  cross-group px({px},{py}) spread {spread:.4}: {detail:?} (tri, group, priority)");
    }
    // Blocker-distance histogram over the dark samples: how far away is the
    // thing that blocks a roof pixel's sun ray?
    let mut blockers: Vec<f32> = Vec::new();
    for &(px, py) in &roof_pixels {
        for sidx in 0..8 {
            let t = tracer.probe_blocker_distance(px, py, w, h, 1, sidx, sun_dir);
            if t.is_finite() && t >= 0.0 {
                blockers.push(t);
            }
        }
    }
    blockers.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !blockers.is_empty() {
        let q = |f: f64| blockers[((blockers.len() - 1) as f64 * f) as usize];
        eprintln!(
            "probe: {} blocked sun samples; blocker t p10 {:.4} p50 {:.4} p90 {:.4} p95 {:.4} p99 {:.4} max {:.3}",
            blockers.len(), q(0.10), q(0.50), q(0.90), q(0.95), q(0.99), blockers.last().unwrap()
        );
        for lim in [0.005f32, 0.01, 0.02, 0.05, 0.1, 0.5] {
            let n = blockers.iter().filter(|t| **t < lim).count();
            eprintln!("    < {:>5.3} m: {:5.1} %", lim, 100.0 * n as f32 / blockers.len() as f32);
        }
    }
    for (px, py, n, spread, dark) in &worst {
        eprintln!("  worst: px({px},{py}) tris {n} depth spread {spread:.4} m dark fraction {dark:.2}");
    }
    // Optional: write small converged CPU renders (skin on) for visual
    // before/after evidence — FAB_PROBE_RENDER=<dir>.
    if let Ok(dir) = std::env::var("FAB_PROBE_RENDER") {
        let _ = std::fs::create_dir_all(&dir);
        let (rw, rh, spp) = (308u32, 372u32, 24u32);
        let t0 = std::time::Instant::now();
        let img = tracer.render(rw, rh, spp, 1);
        eprintln!("probe: {}x{} at {} spp rendered in {:.1}s (skin {:.4})", rw, rh, spp, t0.elapsed().as_secs_f64(), tracer.shadow_skin);
        let mut bgra = vec![0u8; (rw * rh * 4) as usize];
        for (i, px) in img.iter().enumerate() {
            let m = makepad_raytrace::gpu::tonemap_rgb(*px, 1.0);
            bgra[i * 4] = (m[2] * 255.0 + 0.5) as u8;
            bgra[i * 4 + 1] = (m[1] * 255.0 + 0.5) as u8;
            bgra[i * 4 + 2] = (m[0] * 255.0 + 0.5) as u8;
            bgra[i * 4 + 3] = 255;
        }
        let name = if tracer.shadow_skin > 0.0 { "roof_skin_on.png" } else { "roof_skin_off.png" };
        let path = std::path::Path::new(&dir).join(name);
        makepad_raytrace::png::write_bgra8(&path, rw as usize, rh as usize, &bgra).expect("png");
        eprintln!("probe: wrote {}", path.display());
    }
    // Per-sample detail on the four worst pixels: which tri/material/depth
    // each sample lands on and whether its sun ray is lit.
    for &(px, py, ..) in worst.iter().take(4) {
        eprintln!("  detail px({px},{py}):");
        for sidx in 0..12 {
            let (tri, t, sh) = tracer.probe_primary_and_sun(px, py, w, h, 1, sidx, sun_dir);
            if tri < 0 {
                eprintln!("    s{sidx}: miss");
                continue;
            }
            let mat = input.tri_material[tri as usize];
            let pr = input.tri_priority[tri as usize];
            let gr = input.tri_coplanar_group[tri as usize];
            let el = snap.triangle_element.get(tri as usize).copied().unwrap_or(u32::MAX);
            eprintln!(
                "    s{sidx}: tri {tri} el {el} mat {mat} prio {pr} group {gr} t {t:.4} sun {sh:.2}"
            );
        }
    }
}
