use super::*;
use super::cascades::*;
use super::voxels::*;

fn unit_box(pos: Vec3f, size: Vec3f) -> Instance {
    let (v, i) = crate::geometry::shape_geometry_data(makepad_scene::Shape::Box);
    let mesh = Arc::new(Mesh::new(v, i, 12, -1, None).unwrap());
    let mut t = Mat4f::identity();
    t.v[0] = size.x; t.v[5] = size.y; t.v[10] = size.z;
    t.v[12] = pos.x; t.v[13] = pos.y; t.v[14] = pos.z;
    Instance::new(mesh, t, vec3f(0.5, 0.25, 1.0), Vec3f::default(), 1.0).unwrap()
}
fn snapshot(instances: Vec<Instance>) -> Arc<Snapshot> {
    snapshot_in(instances, None)
}
fn snapshot_in(mut instances: Vec<Instance>, region: Option<(Vec3f, Vec3f)>) -> Arc<Snapshot> {
    if let Some(region) = region { instances.retain(|i| i.overlaps(region)); }
    let triangles = instances.iter().map(|i| i.triangles()).sum();
    Arc::new(Snapshot { instances, triangles, region })
}
fn small_layout() -> VoxelLayout { VoxelLayout::new(GiConfig { grid: [4, 2, 4], cascades: 2, ..GiConfig::default() }) }

#[test]
fn gi_albedo_uses_the_same_mip_for_rgba_astc_and_bc7() {
    use crate::material_surface::{PixelSemantic, PreparedTexture};
    use makepad_draw::makepad_platform::CompressedTextureFormat::{Astc4x4, Bc7};

    // One checkered block distinguishes mip filtering from striding. Solid blocks elsewhere
    // keep the encode cheap. The selected level has a partial block at its right edge.
    let mut image = ImageBuffer::default();
    image.width = 12;
    image.height = 256;
    image.data = (0..image.width * image.height).map(|i| {
        let (x, y) = (i % image.width, i / image.width);
        if x < 4 && y < 4 && (x + y) % 2 == 0 { 0xff70_3020 }
        else if x >= 8 || y / 8 % 2 == 0 { 0xffd0_6010 }
        else { 0xff80_4818 }
    }).collect();
    let rgba = PreparedTexture::prepare(image, PixelSemantic::Color);
    let ktx2 = rgba.to_basis(true, &makepad_texcomp::run_inline).expect("encode the colour mip chain");
    let view = makepad_texcomp::basis::UastcView::parse(&ktx2).expect("read the stored colour mip chain");
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let texture = rgba.clone().upload(&mut cx);
    let image = MeshImage::from_texture(&mut cx, &texture).expect("RGBA has a GI image");
    let (w, h, words) = image.texels().expect("RGBA supplies GI texels");
    assert_eq!((w, h), (6, 128), "GI selects the finest mip with both sides at most 128");
    assert_eq!(words, &rgba.data[12 * 256..12 * 256 + w * h], "GI copies the selected RGBA mip exactly");

    let means = |words: &[u32]| -> [f64; 4] {
        std::array::from_fn(|c| {
            let sum: u32 = words.iter().map(|p| (p >> (8 * c)) & 255).sum();
            f64::from(sum) / words.len() as f64
        })
    };
    let rgba_means = means(words);
    let [astc, bc7] = [Astc4x4, Bc7].map(|format| {
        let compressed = PreparedTexture::from_uastc_view(&view, format, rgba.hash).expect("transcode the same colour mip chain");
        let texture = compressed.upload(&mut cx);
        let image = MeshImage::from_texture(&mut cx, &texture).expect("compressed blocks supply a GI image");
        let (cw, ch, words) = image.texels().expect("the selected compressed level decodes");
        assert_eq!((cw, ch), (w, h), "{format:?} and RGBA select the same GI image size");
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::texture_restore::note(&mut cx, &texture, true, &[rgba.hash], 0, rgba.hash);
            let TextureFormat::VecMipCompressed { data, .. } = texture.get_format(&mut cx) else {
                panic!("{format:?} uploads as a block-compressed chain");
            };
            *data = None;
            let held = MeshImage::from_texture(&mut cx, &texture).expect("GI finds the held tail after upload");
            crate::texture_restore::forget(texture.texture_id());
            assert_eq!(held.texels(), Some((cw, ch, words)), "{format:?} keeps the same GI texels after its CPU chain is dropped");
        }
        means(words)
    });
    for (c, channel) in ["blue", "green", "red", "alpha"].iter().enumerate() {
        let (r, a, b) = (rgba_means[c], astc[c], bc7[c]);
        assert!(r.max(a).max(b) - r.min(a).min(b) <= 1.5,
            "GI {channel} means agree within 1.5 levels across RGBA ({r}), ASTC ({a}) and BC7 ({b})");
    }
}

// CPU mirror of GiVoxels.vox_end_status. Distance-limit and window-exit
// endpoints have the same rule; neither termination alone proves sky.
fn trace_end_status(end: Vec3f, (lo, hi): (Vec3f, Vec3f)) -> f32 {
    if end.x < lo.x || end.y < lo.y || end.z < lo.z || end.x > hi.x || end.y > hi.y || end.z > hi.z {
        0.0
    } else {
        0.25
    }
}

// Both paths use f32 as in vox_trace. Keep the single-axis reference step
// independent of the jump's crossing counts, wrap and budget arithmetic.
#[derive(Debug)]
struct VoxelTrace {
    cell: Option<[i32; 3]>,
    level: usize,
    axis: i32,
    status: f32,
    distance: f32,
}

fn trace_voxels(world: &VoxelWorld, ro: [f32; 3], rd: [f32; 3], tmax: f32, first: usize,
    jump: bool, jumps: &mut [usize; 2]) -> VoxelTrace {
    let axes = [0, 1, 2];
    let d = world.layout.dims.map(|i| i as i32);
    let stp = rd.map(|v| if v >= 0.0 { 1 } else { -1 });
    let inv = axes.map(|i| stp[i] as f32 / rd[i].abs().max(0.000001));
    let earliest = |v: [f32; 3]| if v[0] < v[1] && v[0] < v[2] { 0 } else if v[1] < v[2] { 1 } else { 2 };
    let (mut level, mut steps, mut t, mut skip) = (first, 0, 0.0, false);
    while level < world.windows.len() && t < tmax {
        let win = world.windows[level].unwrap();
        let voxel = world.layout.voxel_size(level);
        let cell = axes.map(|i| ((ro[i] + rd[i] * t) / voxel).floor() as i32);
        if axes.iter().all(|&i| cell[i] >= win[i] && cell[i] < win[i] + d[i]) {
            let mut next = axes.map(|i| (((cell[i] + stp[i].max(0)) as f32) * voxel - ro[i]) * inv[i]);
            let delta = inv.map(|v| v.abs() * voxel);
            let rate = delta.map(f32::recip);
            let mut m = axes.map(|i| cell[i].rem_euclid(d[i]));
            let mut left = axes.map(|i| if stp[i] > 0 { win[i] + d[i] - 1 - cell[i] } else { cell[i] - win[i] });
            let (first_next, first_left) = (next, left);
            let mut jumped = false;
            let mut axis = 0;
            loop {
                steps += 1;
                let alpha = if skip { 0 } else { world.albedo[world.layout.texel(level, m)] >> 24 };
                if steps > 320 || alpha == 255 {
                    let c = axes.map(|i| win[i] + (m[i] - win[i]).rem_euclid(d[i]));
                    return VoxelTrace { cell: Some(c), level, axis, status: if steps > 320 { -1.0 } else { 1.0 }, distance: t };
                }
                skip = false;
                if jump && (alpha == 32 || alpha == 64) {
                    let size = (alpha / 8) as i32;
                    let n = axes.map(|i| if stp[i] > 0 { size - m[i] % size } else { m[i] % size + 1 });
                    let face = axes.map(|i| next[i] + (n[i] - 1) as f32 * delta[i]);
                    let a = earliest(face);
                    let exit = face[a];
                    if exit < tmax {
                        let crossed = axes.map(|i| {
                            if i == a { return n[i]; }
                            let k = (((exit - next[i]) * rate[i] + 0.5).floor() as i32).clamp(0, n[i] - 1);
                            let near = next[i] + k as f32 * delta[i];
                            k + i32::from(near < exit || (near == exit && i > a))
                        });
                        let after = steps - 1 + crossed.iter().sum::<i32>();
                        if after <= 320 {
                            jumped = true;
                            jumps[usize::from(size == 8)] += 1;
                            steps = after;
                            t = exit;
                            axis = stp[a] * (a as i32 + 1);
                            for i in axes {
                                m[i] += stp[i] * crossed[i];
                                if m[i] < 0 { m[i] += d[i]; } else if m[i] >= d[i] { m[i] -= d[i]; }
                                left[i] -= crossed[i];
                                next[i] += crossed[i] as f32 * delta[i];
                            }
                            if left.iter().any(|&l| l < 0) { break; }
                            continue;
                        }
                    }
                }
                let a = earliest(next);
                t = next[a];
                m[a] = (m[a] + stp[a]).rem_euclid(d[a]);
                left[a] -= 1;
                next[a] += delta[a];
                axis = stp[a] * (a as i32 + 1);
                if t >= tmax || left[a] < 0 { break; }
            }
            if jumped {
                if let Some(i) = left.iter().position(|&n| n < 0) {
                    t = (0..first_left[i]).fold(first_next[i], |time, _| time + delta[i]);
                }
            }
        }
        level += 1;
        skip = true;
    }
    let [x, y, z] = axes.map(|i| ro[i] + rd[i] * t.min(tmax));
    let status = trace_end_status(vec3f(x, y, z), (Vec3f::all(-20.0), Vec3f::all(20.0)));
    VoxelTrace { cell: None, level, axis: 0, status, distance: t.min(tmax) }
}

#[test]
fn empty_brick_jumps_match_the_fine_voxel_march() {
    let mut seed = 0x6c34_8d21_u32;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed >> 8) as f32 / 16_777_216.0
    };
    let (mut rays, mut jumps, mut misses, mut max_diff) = (0, [0; 2], [0; 4], 0.0_f32);
    let mut compare = |world: &VoxelWorld, ro, rd, tmax, first| {
        let fine = trace_voxels(world, ro, rd, tmax, first, false, &mut [0; 2]);
        let mut taken = [0; 2];
        let fast = trace_voxels(world, ro, rd, tmax, first, true, &mut taken);
        for (total, n) in jumps.iter_mut().zip(taken) { *total += n; }
        for (count, differs) in misses.iter_mut().zip([fine.cell != fast.cell, fine.level != fast.level,
            fine.axis != fast.axis, fine.status != fast.status]) { *count += usize::from(differs); }
        rays += 1;
        let diff = (fine.distance - fast.distance).abs();
        max_diff = max_diff.max(diff);
        assert!(diff <= 64.0 * f32::EPSILON * fine.distance.max(1.0), "{ro:?} {rd:?}: {fine:?} vs {fast:?}");
        (fast, taken)
    };
    let layout = VoxelLayout { dims: [32, 16, 32], levels: 3, voxel: 0.5, home: [0.0; 3] };
    let windows = [[-8, -8, -16], [-16, -8, -16], [-16, -8, -16]];
    for _ in 0..4 {
        let mut world = VoxelWorld::new(layout);
        let mut boxes = Vec::new();
        for i in 0..64 {
            let span = if i < 32 { 16.0 } else { 48.0 };
            let mut coord = || (random() - 0.5) * span;
            let pos = vec3f(coord(), coord() * 0.5, coord());
            boxes.push(unit_box(pos, vec3f(0.1 + 3.0 * random(), 0.1 + 3.0 * random(), 0.1 + 3.0 * random())));
        }
        world.update(Some(snapshot(boxes)), &windows);
        for i in 0..1024 {
            let ro = [0, 1, 2].map(|_| (random() - 0.5) * 16.0);
            let dir = vec3f(random() - 0.5, random() - 0.5, random() - 0.5).normalize();
            compare(&world, ro, [dir.x, dir.y, dir.z], 64.0 * random(), i % 3);
        }
    }

    // Exit-axis ties and internal crossings at the exit time. Put a small
    // solid in the first cell the fine march visits outside the empty brick.
    let targets = [
        ([4.5, 0.5, 0.5], [1.0; 3], [8, 4, 4], 1),
        ([0.5, 4.5, 0.5], [1.0; 3], [3, 8, 4], 2),
        ([0.5, 0.5, 4.5], [1.0; 3], [3, 3, 8], 3),
        ([0.5; 3], [1.0; 3], [7, 7, 8], 3),
        ([15.5; 3], [-1.0; 3], [8, 8, 7], -3),
    ];
    let marker = |[x, y, z]: [i32; 3]| quad(vec3f(x as f32 + 0.25, y as f32 + 0.25, z as f32 + 0.5),
        vec3f(0.5, 0.5, 1.0), Vec3f::all(1.0), Vec3f::default());
    let mut ties = VoxelWorld::new(VoxelLayout { dims: [16; 3], levels: 1, voxel: 1.0, ..layout });
    ties.update(Some(snapshot(targets.iter().map(|&(_, _, c, _)| marker(c)).collect())), &[[0; 3]]);
    for (ro, rd, c, axis) in targets {
        let (hit, _) = compare(&ties, ro, rd, 16.0, 0);
        assert_eq!((hit.cell, hit.axis, hit.status), (Some(c), axis, 1.0), "fine tie order at {ro:?}");
    }

    let mut levels = VoxelWorld::new(VoxelLayout { dims: [8; 3], levels: 2, voxel: 1.0, ..layout });
    levels.update(Some(snapshot(vec![marker([8, 7, 0]), marker([10, 7, 0])])), &[[0; 3]; 2]);
    let diag = std::f32::consts::FRAC_1_SQRT_2;
    let (hit, _) = compare(&levels, [0.25, 0.0, 0.5], [diag, diag, 0.0], 16.0, 0);
    assert_eq!((hit.cell, hit.level, hit.axis), (Some([4, 3, 0]), 1, 1),
        "fine additions place the coarse entry just below x=8, so cell 4 is tested");
    let (hit, _) = compare(&levels, [0.25, 7.25, 0.5], [1.0, 0.0, 0.0], 16.0, 0);
    assert_eq!((hit.cell, hit.level, hit.axis), (Some([5, 3, 0]), 1, 1),
        "entering at exactly x=8 skips coarse cell 4 and tests cell 5");

    let mut long = VoxelWorld::new(VoxelLayout { dims: [336, 8, 8], levels: 1, voxel: 1.0, ..layout });
    long.update(None, &[[0; 3]]);
    let (ro, rd) = ([1.25, 0.5, 0.5], [1.0, 0.0, 0.0]);
    for tmax in [6.5, 6.75] {
        let (end, taken) = compare(&long, ro, rd, tmax, 0);
        assert_eq!((end.distance, end.status, taken), (tmax, 0.25, [0, 0]), "walk the endpoint brick at {tmax}");
    }
    let (end, _) = compare(&long, ro, rd, 319.75, 0);
    assert_eq!((end.distance, end.status), (319.75, 0.0), "crossing 320 can reach tmax");
    let (spent, _) = compare(&long, ro, rd, 320.0, 0);
    assert_eq!((spent.cell, spent.axis, spent.distance, spent.status), (Some([321, 0, 0]), 1, 319.75, -1.0),
        "the budget ends inside an empty brick, exactly at fine crossing 320");
    let (spent, _) = compare(&long, ro, [1.0, 1.0 / 128.0, 0.0], 330.0, 0);
    assert_eq!((spent.cell, spent.distance, spent.status), (Some([319, 2, 0]), 317.75, -1.0),
        "318 x crossings and two internal y crossings spend the whole budget");
    eprintln!("{rays} rays, {jumps:?} jumps (4/8), {misses:?} cell/level/axis/status mismatches, max distance difference {max_diff}");
    assert!(jumps.iter().all(|&n| n > 0), "both empty-brick sizes must be traversed");
    assert_eq!(misses, [0; 4]);
}

fn skyline_height(map: &Skyline, x: f32, z: f32) -> f32 {
    let col = (((x - map.min[0]) / map.cell).floor() as usize).min(map.width - 1);
    let row = (((z - map.min[1]) / map.cell).floor() as usize).min(map.height - 1);
    map.heights[row * map.width + col]
}

// CPU mirror of DrawGiTrace.sky_march, starting at the voxel trace endpoint.
fn sky_march(map: &Skyline, bounds: (Vec3f, Vec3f), p: Vec3f, rd: Vec3f) -> (f32, f32) {
    let (mut t, mut d) = (0.0, map.cell);
    for _ in 0..12 {
        t += d;
        d *= 1.5;
        let q = p + rd * t;
        if trace_end_status(q, bounds) == 0.0 { return (t, 0.0); }
        if q.y < skyline_height(map, q.x, q.z) { return (t, 0.5); }
    }
    (t, 0.25)
}

#[test]
fn trace_endings_are_sky_only_outside_the_padded_scene() {
    let bounds = (vec3f(-20.0, -2.0, -20.0), vec3f(20.0, 10.0, 20.0));
    for (case, end, expected) in [
        ("up through the top", vec3f(0.0, 11.0, 0.0), 0.0),
        ("tmax inside the scene", vec3f(5.0, 2.0, 0.0), 0.25),
        ("coarsest window side inside the scene", vec3f(16.0, 2.0, 0.0), 0.25),
        ("outside the scene side", vec3f(21.0, 2.0, 0.0), 0.0),
        ("outside the negative side", vec3f(-21.0, 2.0, 0.0), 0.0),
        ("outside the far side", vec3f(0.0, 2.0, 21.0), 0.0),
        ("outside the near side", vec3f(0.0, 2.0, -21.0), 0.0),
        ("below the scene", vec3f(0.0, -3.0, 0.0), 0.0),
        ("on the padded box", vec3f(20.0, 10.0, 20.0), 0.25),
    ] {
        assert_eq!(trace_end_status(end, bounds), expected, "{case}");
    }
    // A DDA step beyond the distance limit cannot prove that the ray left.
    let (origin, direction, t, tmax) = (vec3f(19.0, 2.0, 0.0), vec3f(1.0, 0.0, 0.0), 3.0_f32, 0.5_f32);
    assert_eq!(trace_end_status(origin + direction * t.min(tmax), bounds), 0.25, "classify the clamped endpoint");

    let instances = vec![
        unit_box(vec3f(-3.0, 0.0, 0.0), vec3f(2.0, 2.0, 2.0)),
        unit_box(vec3f(3.0, 2.0, 0.0), vec3f(2.0, 2.0, 2.0)),
    ];
    let mut gi = FastGi::default();
    let old_bounds = gi.scene_bounds;
    gi.submit_scene([0; 6], Vec3f::default(), Snapshot { instances, triangles: 24, region: None });
    let (_, padded) = gi.pending_scene.as_ref().unwrap();
    assert_eq!(*padded, (vec3f(-5.0, -2.0, -2.0), vec3f(5.0, 4.0, 2.0)), "union world bounds and pad by the coarsest voxel");
    assert_eq!(gi.scene_bounds, old_bounds, "a pending scene must not replace the committed voxel bounds");
    assert_eq!(trace_end_status(vec3f(4.5, 2.0, 0.0), *padded), 0.25, "the padding is still inside");
    let empty_scene = snapshot(vec![]);
    let empty = empty_scene.padded_bounds(1.0);
    assert_eq!(trace_end_status(Vec3f::default(), empty), 0.0, "an empty scene is open");
    let map = Skyline::build(&empty_scene, empty, 1.0);
    assert_eq!((map.min, map.cell, map.width, map.height), ([0.0; 2], 1.0, 1, 1));
    assert_eq!(map.heights, vec![empty.0.y], "the empty map holds the scene floor");

    let floor = unit_box(vec3f(0.0, -0.5, 0.0), vec3f(32.0, 1.0, 16.0));
    let flat = snapshot(vec![floor.clone()]);
    let map = Skyline::build(&flat, flat.padded_bounds(0.5), 1.0);
    assert_eq!((map.min, map.width, map.height), ([-16.5, -8.5], 33, 17));
    assert!(map.heights.iter().all(|&y| y == floor.hi.y), "the floor covers every column");

    let tall = unit_box(vec3f(5.0, 6.0, 1.0), vec3f(2.0, 12.0, 2.0));
    let scene = snapshot(vec![tall, floor]);
    let bounds = scene.padded_bounds(0.5);
    let map = Skyline::build(&scene, bounds, 1.0);
    for row in 0..map.height {
        for col in 0..map.width {
            let x = map.min[0] + (col as f32 + 0.5) * map.cell;
            let z = map.min[1] + (row as f32 + 0.5) * map.cell;
            let top = if (4.0..=6.0).contains(&x) && (0.0..=2.0).contains(&z) { 12.0 } else { 0.0 };
            assert_eq!(map.heights[row * map.width + col], top, "skyline at ({x}, {z})");
        }
    }
    let up = sky_march(&map, bounds, vec3f(0.0, 1.0, 1.0), vec3f(0.0, 1.0, 0.0));
    assert_eq!(up.1, 0.0, "one metre above the floor next to the tall box sees sky");
    let toward = sky_march(&map, bounds, vec3f(0.0, 6.0, 1.0), vec3f(1.0, 0.0, 0.0));
    assert_eq!(toward, (4.75, 0.5), "the third sample hits the tall box at half height");
    let away = sky_march(&map, bounds, vec3f(0.0, 0.0, 1.0), vec3f(-1.0, 0.0, 0.0));
    assert_eq!(away.1, 0.0, "a ray along the floor leaves through the side");

    let wide = snapshot(vec![unit_box(vec3f(0.0, -0.5, 0.0), vec3f(1024.0, 1.0, 4.0))]);
    let bounds = wide.padded_bounds(0.5);
    let map = Skyline::build(&wide, bounds, 1.0);
    assert_eq!((map.cell, map.width, map.height), (1025.0 / 512.0, 512, 3));
    let (distance, status) = sky_march(&map, bounds, vec3f(-100.0, 0.25, 0.0), vec3f(1.0, 0.0, 0.0));
    assert_eq!(status, 0.25, "a ray still inside the wide scene is unknown");
    assert!((distance - map.cell * 2.0 * (1.5_f32.powi(12) - 1.0)).abs() < 0.001, "exactly twelve steps");
}

#[test]
fn ran_out_shader_samples_are_excluded_from_gather_sums_and_backfaces() {
    // Inspect complete brace-delimited blocks so a gate merely appearing
    // elsewhere cannot satisfy the exclusion checks.
    fn body<'a>(source: &'a str, marker: &str) -> &'a str {
        let tail = &source[source.find(marker).expect(marker) + marker.len()..];
        let start = tail.find('{').expect("block start") + 1;
        let mut depth = 1;
        let end = tail[start..].char_indices().find_map(|(i, c)| {
            if c == '{' { depth += 1; }
            if c == '}' { depth -= 1; }
            (depth == 0).then_some(start + i)
        }).expect("closed block");
        &tail[start..end]
    }
    let source: String = include_str!("shaders.rs").chars().filter(|c| !c.is_whitespace()).collect();
    let ending = body(&source, "vox_end_status:fn");
    assert!(ending.contains("ifp.x<lo.x||p.y<lo.y||p.z<lo.z||p.x>hi.x||p.y>hi.y||p.z>hi.z{return0.0}"), "CPU mirror and shader must use the same scene faces");
    assert!(ending.ends_with("return0.25"), "inside the scene is ran out");
    let march = body(&source, "vox_trace:fn");
    assert!(march.contains("letend=min(t,tmax)returnvec4(end,level,0.0,self.vox_end_status(ro+rd*end))"), "classify the actual clamped endpoint");

    let trace = body(&source, "mod.draw.DrawGiTrace=");
    let invalid_ray = "returnvec4(0.0,-2.0,0.0,0.0)";
    let sky_ray = "returnvec4(self.gi_pass.z,-1.0,0.0,0.0)";
    assert_eq!(body(trace, "ifhit.w<0.0"), invalid_ray, "a spent voxel step budget stays invalid");
    assert_eq!(trace.matches("self.sky_march(").count(), 1, "only an unresolved voxel ray marches");
    let unresolved = body(trace, "ifhit.w==0.25");
    assert!(unresolved.starts_with("letm=self.sky_march(probe.xyz+rd*hit.x,rd)"), "continue from the voxel endpoint");
    assert_eq!(body(unresolved, "ifm.y==0.0"), sky_ray, "a skyline escape packs as sky");
    assert_eq!(body(unresolved, "ifm.y==0.5"), "returnvec4(hit.x+m.x,-3.0,0.0,0.0)", "closed far carries the full distance");
    assert!(unresolved.ends_with(invalid_ray), "a still-unknown ray packs as invalid");
    assert_eq!(body(trace, "ifhit.w<0.5"), sky_ray, "a voxel escape packs as sky");
    for ending in ["ifhit.w<0.0", "ifhit.w==0.25"] {
        assert!(trace.find(ending).unwrap() < trace.find("ifhit.w<0.5").unwrap(), "handle {ending} before sky");
    }
    let relight = body(&source, "mod.draw.DrawGiRelight=");
    let closed_far = "ifhit.y<(-2.5)&&probe.w>=0.0";
    let invalid = "ifprobe.w<0.0||(hit.y<(-1.5)&&hit.y>(-2.5))";
    assert_eq!(body(relight, closed_far), "returnvec4(0.0,0.0,0.0,hit.x)", "a valid closed-far ray is black at its distance");
    assert!(relight.find(closed_far).unwrap() < relight.find(invalid).unwrap(), "handle closed far before invalid rays");
    assert_eq!(body(relight, invalid), "returnvec4(0.0,0.0,0.0,-2.0)", "invalid rays keep negative distance");
    assert!(relight.find(invalid).unwrap() < relight.find("ifhit.y<(-0.5)").unwrap(), "invalid rays must not become sky radiance");
    assert!(relight.contains("ifshadow.w<0.5&&shadow.w>(-0.5){light=self.gi_sun_color*ndl}"), "sun exhaustion keeps its previous visibility");

    let gather = body(&source, "mod.draw.DrawGiGather=");
    for (function, sums) in [
        ("irradiance:fn", &["open=open+", "sum=sum+", "total=total+"][..]),
        ("moments:fn", &["m=m+", "total=total+"][..]),
    ] {
        let function_body = body(gather, function);
        let valid = body(function_body, "ifsample.w>=0.0");
        for &sum in sums {
            assert_eq!(function_body.matches(sum).count(), 1, "one {function} {sum} update");
            assert_eq!(valid.matches(sum).count(), 1, "{function} {sum} must exclude ran-out weights");
        }
        assert_eq!(body(function_body, "iftotal<=0.0001"), "returnvec4(0.0,0.0,0.0,-1.0)", "empty {function} footprint keeps no-data handling");
    }
    assert_eq!(gather.matches("back=back+").count(), 1, "one backface counter");
    assert_eq!(body(gather, "ifw<(-0.5)&&w>(-1.5)"), "back=back+1.0", "ran-out distance -2 must not invalidate the probe");
}

#[test]
fn batch_bands_cover_exactly_the_logical_rows() {
    for (rows, expected_band) in [
        (0, 4), (1, 4), (42, 4), (43, 4), (84, 4), (85, 4), (168, 4),
        (169, 8), (330, 8), (336, 8), (337, 16), (672, 16), (673, 32),
        (1344, 32), (1345, 32), (4096, 32),
    ] {
        let band = band_rows(rows);
        assert_eq!(band, expected_band, "band height at {rows} live rows");
        let height = batch_target_height(rows);
        let mut logical = 0;
        for rect in band_quads(rows, band, GATHER_WIDTH) {
            assert_eq!(rect.pos.x, 0.0);
            assert_eq!(rect.size.x, GATHER_WIDTH as f64);
            let start = rect.pos.y as usize;
            for physical in start..start + rect.size.y as usize {
                assert!(physical < height, "band outside its target for {rows} rows");
                assert_eq!(physical, logical / band * BAND_PITCH + logical % band, "shaded a gap or skipped a row");
                // Invert the pass's texel centre and address it again, in shader precision.
                let p = (physical as f32 + 0.5).floor();
                let pitch = BAND_PITCH as f32;
                let b = (p / pitch).floor();
                let j = b * band as f32 + (p - b * pitch);
                assert_eq!(j, logical as f32, "producer read the wrong scheduler row");
                let b = (j / band as f32).floor();
                let uv = (b * pitch + (j - b * band as f32) + 0.5) / height as f32;
                assert_eq!((uv * height as f32).floor(), p, "consumer missed its texel");
                logical += 1;
            }
        }
        assert_eq!(logical, rows, "draw exactly the live rows, including a partial band");
    }
    for (capacity, height) in [(32, 1344), (1344, 1344), (1345, 1376), (4096, 4096)] {
        assert_eq!(batch_target_height(capacity), height, "target allocation for {capacity} probes");
    }
}

#[test]
fn scatter_shader_places_atlas_tiles_without_widget_clipping() {
    let source: String = include_str!("shaders.rs").chars().filter(|c| !c.is_whitespace()).collect();
    let scatter = source.split_once("mod.draw.DrawGiScatter=").unwrap().1
        .split_once("mod.draw.FastGiScreen=").unwrap().0;
    assert!(!scatter.contains("clip_and_transform_vertex("), "retained tiles and unaligned clears have no turtle clip");
    for expression in [
        "let a=self.gi_batch.sample_nearest(vec2(0.25,self.batch_y(self.gi_tile.x)),0.0)",
        "let x=a.x-floor(a.x/self.gi_atlas.z)*self.gi_atlas.z",
        "let y=floor(a.x/self.gi_atlas.z)",
        "let base=a.y*self.gi_atlas.w*19.0",
        "if self.gi_tile.w>0.5 {offset=self.gi_atlas.w*8.0}",
        "if self.gi_tile.w>1.5 {offset=self.gi_atlas.w*18.0}",
        "p=vec2(x*w,base+offset+y*w)",
        "self.pos=self.geom.pos",
        "let vertex=p+self.geom.pos*self.rect_size",
        "self.world=self.draw_list.view_transform*vec4(vertex.x,vertex.y,self.draw_depth+self.draw_call.zbias,1.0)",
        "self.vertex_pos=self.draw_pass.camera_projection*(self.draw_pass.camera_view*self.world)",
        "if self.gi_tile.x<0.0 {return vec4(0.0,0.0,0.0,0.0)}",
        "let col=self.gi_tile.y+min(floor(self.pos.x*w),w-1.0)+min(floor(self.pos.y*w),w-1.0)*w",
        "self.gi_update.sample_nearest(vec2((col+0.5)/#(super::GATHER_WIDTH as f64),self.target_y(self.gi_tile.x)))",
    ] {
        let compact: String = expression.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(scatter.contains(&compact), "scatter CPU mirror and unclipped pass projection: {expression}");
    }
}

// Check the production row encoder and retained instance layout together.
// The shader expressions mirrored here are pinned by the test above.
fn check_scatter_batch_destinations(cx: &Cx, scatter: &DrawGiScatter, cache: &ScatterCache, batch_height: usize) {
    let shader = scatter.quad.draw_vars.draw_shader_id.unwrap();
    let mapping = &cx.draw_shaders[shader.index].mapping;
    let offset = |id| mapping.instances.inputs.iter().find(|p| p.id == id).unwrap().offset;
    let tile_offset = offset(live_id!(gi_tile));
    let size_offset = offset(live_id!(rect_size));
    let retained = cache.instances.as_ref().unwrap();
    for rows in [batch_height, 4, 1, 0, batch_height] {
        let (instances, count) = cache.batch(rows);
        assert_eq!(count, 3 * rows, "draw exactly the live batch, including after shrinking");
        assert_eq!(instances.id(), retained.id(), "batch changes reuse the same publication");
        let records = instances.data()[..count * instances.slots()].chunks_exact(instances.slots());
        for (i, record) in records.enumerate() {
            let row = record[tile_offset] as usize;
            assert_eq!(row, i / 3, "every live row is drawn in order");
            assert!(row < rows, "unused capacity never reaches the scatter shader");
        }
    }

    for config in [
        GiConfig::default(),
        GiConfig { grid: [6, 2, 6], ..GiConfig::default() },
        GiConfig { grid: [4, 2, 4], ..GiConfig::default() },
    ] {
        let [_, _, columns, tile_rows] = config.atlas();
        let probes = config.probe_count();
        let destinations = [(0, 0), (0, probes - 1), (1, (columns + 1).min(probes - 1)), (2, probes / 2 + 1)];
        let mut texture = vec![f32::NAN; batch_height * 8];
        for (row, &(cascade, slot)) in destinations.iter().enumerate() {
            let item = BatchItem { cascade, slot, cell: [-3, 5, 7], hysteresis: REFRESH_HYSTERESIS };
            texture[row * 8..row * 8 + 8].copy_from_slice(&work::batch_row(&item));
        }
        let (instances, count) = cache.batch(destinations.len());
        for (i, record) in instances.data()[..count * instances.slots()].chunks_exact(instances.slots()).enumerate() {
            let tile = &record[tile_offset..tile_offset + 4];
            let row = i / 3;
            let region = i % 3;
            let (column, width) = [(0, 8), (64, 10), (164, 1)][region];
            assert_eq!(tile, &[row as f32, column as f32, width as f32, region as f32], "cached row/column/width/region");
            assert_eq!(&record[size_offset..size_offset + 2], &[width as f32; 2], "each destination is a complete tile");

            // Vertex lookup into the dense batch, at texel centres and LOD 0.
            let uv = [0.25_f32, (tile[0] + 0.5) / batch_height as f32];
            let texel = (uv[1] * batch_height as f32).floor() as usize * 2 + (uv[0] * 2.0).floor() as usize;
            let a = &texture[texel * 4..texel * 4 + 4];
            let (cascade, slot) = destinations[row];
            assert_eq!(&a[..2], &[slot as f32, cascade as f32], "batch destination survives the vertex fetch");
            let x = a[0] - (a[0] / columns as f32).floor() * columns as f32;
            let y = (a[0] / columns as f32).floor();
            let base = a[1] * tile_rows as f32 * 19.0;
            let mut offset = 0.0;
            if tile[3] > 0.5 { offset = tile_rows as f32 * 8.0; }
            if tile[3] > 1.5 { offset = tile_rows as f32 * 18.0; }
            let origin = [x * tile[2], base + offset + y * tile[2]];

            // The CPU tile closure before 04171b5fdd, using integer texels.
            let (x, y, base) = (slot % columns, slot / columns, cascade * tile_rows * 19);
            let old = match region {
                0 => [x * 8, base + y * 8],
                1 => [x * 10, base + tile_rows * 8 + y * 10],
                _ => [x, base + tile_rows * 18 + y],
            };
            assert_eq!(origin, old.map(|v| v as f32), "same atlas origin for cascade {cascade}, slot {slot}, region {region}");
            for (x, y) in [(0, 0), (width - 1, 0), (0, width - 1), (width - 1, width - 1)] {
                let pos = [(x as f32 + 0.5) / tile[2], (y as f32 + 0.5) / tile[2]];
                let col = tile[1] + (pos[0] * tile[2]).floor() + (pos[1] * tile[2]).floor() * tile[2];
                assert_eq!(col as usize, column + y * width + x, "tile corners read the same gather columns; state reads 164");
                assert_eq!(
                    [origin[0] + pos[0] * tile[2], origin[1] + pos[1] * tile[2]],
                    [old[0] as f32 + x as f32 + 0.5, old[1] as f32 + y as f32 + 0.5],
                    "unclipped tile corners cover the original atlas texel block",
                );
            }
        }
    }
}

#[test]
fn screen_cache_rays_match_the_unprojected_view() {
    let eye = vec3f(2.0, 3.0, -4.0);
    let look = Mat4f::look_at(eye, vec3f(-1.0, 0.5, 2.0), vec3f(0.0, 1.0, 0.0));
    let mut projection = Mat4f::perspective(62.0, 1.6, 0.5, 80.0);
    projection.v[8] = 0.17;
    projection.v[9] = -0.23;
    let clip = Mat4f::mul(&projection, &look);
    let inverse = clip.invert();
    for depth_map in [[1.0, 0.0], [2.0, -1.0]] {
        let view = GiScreenView::from_clip(clip, depth_map).expect("a perspective clip has screen rays");
        let cache_eye = vec3f(view.eye[0], view.eye[1], view.eye[2]);
        assert!((cache_eye - eye).length() < 1e-4, "cache eye must match the camera for {depth_map:?}");
        let [ray0, ray_u, ray_v] = view.rays.map(|[x, y, z, _]| vec3f(x, y, z));
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5), (0.18, 0.73)] {
            let ray = ray0 + ray_u * u + ray_v * v;
            for raw in [0.1, 0.5, 0.9] {
                let z = raw * depth_map[0] + depth_map[1];
                let h = inverse.transform_vec4(vec4(2.0 * u - 1.0, 1.0 - 2.0 * v, z, 1.0));
                let expected = h.to_vec3f() / h.w - eye;
                assert!((ray.normalize() - expected.normalize()).length() < 1e-4,
                    "ray direction must match unprojection at ({u}, {v}), depth {raw}, map {depth_map:?}");
                if raw == 0.5 {
                    assert!((ray - expected).length() < 1e-4,
                        "unnormalised ray must reach mid depth at ({u}, {v}) for {depth_map:?}");
                }
            }
        }
    }
    let ortho = Mat4f::ortho(-3.0, 3.0, 2.0, -2.0, 0.5, 80.0, 1.0, 1.0);
    assert!(GiScreenView::from_clip(Mat4f::mul(&ortho, &look), [1.0, 0.0]).is_none(),
        "an orthographic clip must decline the perspective cache");
    let mut singular = clip;
    singular.v[..4].fill(0.0);
    assert!(GiScreenView::from_clip(singular, [1.0, 0.0]).is_none(), "a singular clip must decline the cache");
}

pub(super) fn register_gi_producers(vm: &mut ScriptVm, hardware: bool) {
    use makepad_platform::makepad_script::script_eval;
    makepad_draw::script_mod(vm);
    vm.heap_mut().new_module(id!(prelude));
    script_eval!(vm, {
        mod.prelude.widgets_internal = {..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw:mod.draw}
    });
    vm.heap_mut().new_module(id!(widgets));
    makepad_render_graph::pass_stdlib(vm);
    crate::local_shadows::sampling::script_mod(vm);
    if hardware { crate::local_shadows::hardware_sampling::script_mod(vm); }
    crate::clustered::script_mod(vm);
    crate::fast_gi::script_mod(vm);
}

#[test]
fn gi_shaders_compile_without_errors() {
    use makepad_draw::makepad_platform::makepad_script::script_eval;
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        vm.capture_errors();
        register_gi_producers(vm, false);
        crate::shaders::script_mod(vm);
        crate::volume_fog::script_mod(vm);
        crate::fast_gi::screen_fill_script_mod(vm);
        crate::local_shadows::script_mod(vm);
        let setup = vm.take_errors();
        assert!(setup.is_empty(), "script setup errors: {setup:#?}");
        // The model lanes read the screen cache through FastGiScreen's functions, not the
        // sampling stubs of the same names (a spread keeps the first definition of a key).
        let draw = vm.value(vm.modules(), id!(draw)).as_object().expect("mod.draw");
        let screen = vm.value(draw, id!(FastGiScreen)).as_object().expect("FastGiScreen");
        for lane in [id!(DrawSceneSkinned), id!(DrawScenePbr), id!(DrawSceneSplat)] {
            let shader = vm.value(draw, lane).as_object().expect("a model lane's shader");
            for function in [id!(gi_ambient_at), id!(gi_screen_hit)] {
                assert_eq!(vm.value(shader, function), vm.value(screen, function), "{lane}: {function} must be the screen cache reader's");
            }
        }
        let trace = DrawGiTrace::script_new_with_default(vm).quad.draw_vars;
        let trace_id = trace.draw_shader_id.expect("GI trace registered");
        assert!(vm.cx().draw_shaders[trace_id.index].mapping.textures.iter().any(|t| t.id == live_id!(gi_skyline)),
            "the compiled trace exposes the skyline sampler");
        // Hits and radiance carry packed integers and distances (f32); the
        // gathered tiles and the field are filterable f16.
        for (vars, format) in [
            (trace, "Rgba32F"),
            (DrawGiRelight::script_new_with_default(vm).quad.draw_vars, "Rgba32F"),
            (DrawGiGather::script_new_with_default(vm).quad.draw_vars, "Rgba16F"),
            (DrawGiScatter::script_new_with_default(vm).quad.draw_vars, "Rgba16F"),
            (DrawGiScreenFill::script_new_with_default(vm).quad.draw_vars, "Rgba32F"),
            (shaders::DrawGiProbeCounts::script_new_with_default(vm).quad.draw_vars, "Bgra8NoBlend"),
        ] {
            let id = vars.draw_shader_id.expect("GI shader registered");
            assert_eq!(format!("{:?}", vm.cx().draw_shaders[id.index].mapping.color_format), format);
            assert!(!vars.options.alpha_blend, "GI payloads must overwrite, not blend");
        }
        // This uses the actual shader instance layout, without a GPU. A
        // second frame and a smaller batch must reuse the immutable bytes.
        let mut scatter = DrawGiScatter::script_new_with_default(vm);
        let mut cache = ScatterCache::default();
        let batch_height = 32;
        let target_height = batch_target_height(batch_height);
        let bytes = cache.prepare(&mut scatter, target_height);
        assert!(bytes > 0, "first encoding publishes scatter geometry");
        let retained = cache.instances.as_ref().unwrap().clone();
        assert_eq!(retained.byte_len(), bytes);
        assert_eq!(retained.count(), target_height * 3, "reserve three regions per row of target capacity");
        check_scatter_batch_destinations(vm.cx(), &scatter, &cache, batch_height);
        assert_eq!(cache.prepare(&mut scatter, target_height), 0, "unchanged frame encodes zero static bytes");
        assert_eq!(cache.instances.as_ref().unwrap().id(), retained.id(), "same resource avoids an upload");
        assert!(cache.instances.as_ref().unwrap().upload_since(retained.id()).is_empty(), "no unchanged instance bytes uploaded");
        assert!(cache.prepare(&mut scatter, batch_target_height(target_height + 1)) > 0, "a taller target rebuilds the cache");
        for (name, result) in [
            ("trace", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiTrace)})),
            ("relight", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiRelight)})),
            ("gather", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiGather)})),
            ("scatter", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiScatter)})),
            ("screen fill", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiScreenFill)})),
            ("cube receiver", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneCube)})),
            ("model receiver", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneSkinned)})),
        ] {
            let errors = vm.heap().string_with(result, |_h, v| v.to_string()).expect("compile result string");
            assert!(errors.is_empty(), "{name}: {errors}");
        }
    });
}

#[test]
fn settings_preserve_cascade_defaults_and_use_the_config_clamps() {
    use makepad_scene::GiSettings;
    let defaults = GiConfig::default();
    assert_eq!(GiConfig::from(GiSettings::default()), defaults);
    let settings = GiSettings { spacing: 0.75, strength: 0.6, bounce: 0.9 };
    let config = GiConfig::from(settings);
    assert_eq!(config, GiConfig { spacing: 0.75, strength: 0.6, feedback: 0.9, ..defaults });
    assert_eq!(config.cascade_spacing(2), 3.0, "spacing controls the finest cascade");
    assert_eq!(config.anchor, None, "the field follows the camera");
    assert_eq!(config.bounce, defaults.bounce, "settings.bounce controls feedback, not reflectance");
    for (value, spacing, strength, feedback) in [
        (-1.0, 0.001, 0.0, 0.0),
        (100.0, 8.0, 1.0, 0.95),
        (f32::NAN, 1.0, 1.0, 0.85),
        (f32::INFINITY, 1.0, 1.0, 0.85),
    ] {
        assert_eq!(
            GiConfig::from(GiSettings { spacing: value, strength: value, bounce: value }),
            GiConfig { spacing, strength, feedback, ..defaults },
            "the shared config contract handles {value}",
        );
    }
}

#[test]
fn config_is_even_bounded_and_fits_webgl_rows() {
    let c = GiConfig { grid: [99, 99, 99], cascades: 9, spacing: f32::NAN, ..GiConfig::default() }.clamped();
    assert_eq!(c.grid, [16, 8, 16]);
    assert!(c.probe_count() <= 2048);
    assert_eq!((c.cascades, c.spacing), (MAX_CASCADES, 1.0));
    let c = GiConfig { grid: [5, 1, 7], ..GiConfig::default() }.clamped();
    assert!(c.grid.iter().all(|g| g % 2 == 0), "voxel dims (4x grid) must hold whole bricks");
    let l = VoxelLayout::new(GiConfig::default());
    assert_eq!((l.dims, l.width(), l.level_height(), l.height()), ([64, 32, 64], 512, 256, 768));
    assert_eq!(GiConfig::default().cascade_spacing(2), 4.0);
}

#[test]
fn toroidal_voxel_texels_are_a_bijection_over_any_window() {
    let l = small_layout();
    for origin in [[0, 0, 0], [-13, 5, 1000], [8, -8, -24]] {
        let mut seen = std::collections::HashSet::new();
        for z in 0..l.dims[2] as i32 { for y in 0..l.dims[1] as i32 { for x in 0..l.dims[0] as i32 {
            assert!(seen.insert(l.texel(1, [origin[0] + x, origin[1] + y, origin[2] + z])));
        }}}
        assert_eq!(seen.len(), l.level_voxels());
        let (start, len) = level_span(&l, 1);
        assert!(seen.iter().all(|t| *t >= start && *t < start + len));
    }
}

#[test]
fn triangle_box_overlap_matches_dense_sampling() {
    let tri = [vec3f(0.1, 0.1, 0.1), vec3f(2.3, 0.4, 0.2), vec3f(0.6, 1.9, 1.1)];
    for z in -1..3 { for y in -1..3 { for x in -1..3 {
        let c = vec3f(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5);
        // Dense barycentric sampling: any sample inside the box proves overlap.
        let mut hit = false;
        for i in 0..=60 { for j in 0..=60 - i {
            let (u, v) = (i as f32 / 60.0, j as f32 / 60.0);
            let p = tri[0] + (tri[1] - tri[0]) * u + (tri[2] - tri[0]) * v;
            let d = p - c;
            if d.x.abs() <= 0.5 && d.y.abs() <= 0.5 && d.z.abs() <= 0.5 { hit = true; }
        }}
        if hit { assert!(tri_box_overlap(c, 0.5, tri), "missed voxel {c:?}"); }
        let far = vec3f(c.x + 40.0, c.y, c.z);
        assert!(!tri_box_overlap(far, 0.5, tri));
    }}}
}

#[test]
fn box_voxelizes_as_a_closed_shell_with_outward_normals_and_albedo() {
    let l = small_layout();
    let mut w = VoxelWorld::new(l);
    let windows = [l.window_for(0, Vec3f::default()), l.window_for(1, Vec3f::default())];
    let u = w.update(Some(snapshot(vec![unit_box(vec3f(0.1, 0.1, 0.1), vec3f(1.0, 1.0, 1.0))])), &windows);
    assert_eq!(u.levels, vec![true, true]);
    // Level 0: 0.25 m voxels, box spans [-0.4, 0.6]; its shell is solid,
    // its middle is air (surfaces, not volumes).
    assert_eq!(w.solid(0, [-2, 1, 0]), Some(true));
    assert_eq!(w.solid(0, [2, 1, 0]), Some(true));
    assert_eq!(w.solid(0, [0, 1, 0]), Some(false));
    assert_eq!(w.solid(0, [5, 1, 0]), Some(false));
    let t = l.texel(0, [2, 1, 0]);
    let px = w.albedo[t];
    assert_eq!(((px >> 16) & 255, (px >> 8) & 255, px & 255), (128, 64, 255), "authored albedo is kept (sRGB bytes)");
    let n = w.normal[t];
    assert!(((n >> 16) & 255) > 200, "+x face normal points out");
}

#[test]
fn incremental_edits_and_scrolls_equal_a_full_rebuild() {
    let config = GiConfig { grid: [4, 2, 4], cascades: 2, feedback: 0.0, ..GiConfig::default() };
    let l = VoxelLayout::new(config);
    let padding = l.voxel_size(l.levels - 1);
    let a = unit_box(vec3f(1.0, 0.0, 1.0), vec3f(1.0, 2.0, 1.0));
    let b = unit_box(vec3f(-3.0, 0.5, 2.0), vec3f(0.3, 1.0, 3.0));
    // The positive X edge lies inside a skyline cell, leaving room to move the exact bounds.
    let floor = unit_box(vec3f(0.0, -1.0, 0.0), vec3f(30.25, 0.2, 30.0));
    let moved = unit_box(vec3f(-1.0, 0.5, -2.0), vec3f(0.3, 1.0, 3.0));
    let w0 = [l.window_for(0, Vec3f::default()), l.window_for(1, Vec3f::default())];
    let far = vec3f(9.0, 1.0, -5.0);
    let w1 = [l.window_for(0, far), l.window_for(1, far)];
    let mut inc = VoxelWorld::new(l);
    let ridge = unit_box(vec3f(-11.0, 2.0, 0.0), vec3f(1.0, 1.0, 1.0));
    let instances = vec![floor.clone(), a.clone(), b.clone(), ridge.clone()];
    let region = (vec3f(-10.0, -5.0, -10.0), vec3f(10.0, 5.0, 10.0));
    let offset = vec3f(-2.0, 0.0, 0.0);
    let old_region = (region.0 + offset, region.1 + offset);
    let original = snapshot_in(instances.clone(), Some(old_region));
    let first = inc.update(Some(original.clone()), &[l.window_for(0, offset), w0[1]]);
    assert!(first.dirty.is_empty(), "the first build is not an edit");
    assert!(!first.edited, "publishing the first skyline must not count as an edit");
    let map = first.skyline.as_ref().expect("the first snapshot publishes a skyline");
    assert_eq!(map.cell, l.voxel_size(l.levels - 1));
    assert_eq!(skyline_height(map, -1.0, -2.0), floor.hi.y);
    assert_eq!(skyline_height(map, -3.0, 2.0), b.hi.y);
    assert_eq!(skyline_height(map, -11.0, 0.0), ridge.hi.y, "the first region includes the ridge");

    let mut cs = [Cascade::new(config, 0), Cascade::new(config, 1)];
    let mut committed = [None; 2];
    let mut clears = Vec::new();
    let mut batch = Vec::new();
    commit_windows(&mut cs, &mut committed, &inc.windows, &first, config, &mut clears);
    for _ in 0..13 { schedule(&mut cs, 64, true, &mut batch); }
    assert!(cs.iter().all(|c| c.owed() == 0), "the field must be asleep before the re-centre");

    let scene = snapshot_in(instances, Some(region));
    let bounds = scene.padded_bounds(padding);
    assert!(bounds.1.y < original.padded_bounds(padding).1.y, "the new region drops the ridge from the ray bounds");
    let recentre = inc.update(Some(scene), &w0);
    assert_eq!(recentre.dirty, [(ridge.lo, ridge.hi)], "the dropped ridge must remain in the brick diff");
    assert!(!recentre.edited, "losing a remote ridge to the region edge must preserve probe history");
    let map = recentre.skyline.as_ref().expect("the re-centre must publish the changed far-sky map");
    assert_eq!(skyline_height(map, -11.0, 0.0), floor.hi.y, "new traces must see the floor where the ridge left");
    clears.clear();
    let commit = commit_windows(&mut cs, &mut committed, &inc.windows, &recentre, config, &mut clears);
    let mut counts = WakeCounts::default();
    counts.record_commit(&commit);
    assert_eq!(counts.count(WakeCause::Recentre), 1, "the publication must count one re-centre");
    assert_eq!(counts.count(WakeCause::SceneEdit), 0, "the remote membership change must count no scene edit");
    assert_eq!(commit.scrolled, 16, "moving one fine brick exposes two probe slabs");
    assert_eq!(commit.retraced, 16, "the other half of the fine window needs a warm re-trace");
    for (level, c) in cs.iter().enumerate() {
        assert_eq!(c.owed(), if level == 0 { 32 } else { 0 }, "only the moved fine level owes work");
        for slot in 0..c.count() {
            let expected = if level == 1 { 0 } else if clears.contains(&(level, slot)) { 7 } else { 19 };
            assert_eq!(c.remaining(slot), expected, "level {level} slot {slot} after the fine-window move");
        }
    }
    for _ in 0..19 { schedule(&mut cs, 64, true, &mut batch); }

    let returned = inc.update(Some(original), &w0);
    assert!(returned.recentred && !returned.edited, "the ridge entering from beyond the previous region is also membership");
    let removed = inc.update(Some(snapshot_in(vec![floor.clone(), a.clone(), b.clone()], Some(old_region))), &w0);
    assert!(removed.edited && !removed.recentred, "removing the remote ridge inside the same region must wake the field");

    // Just beyond the coarsest window, but within its expanded probe invalidation range.
    let buried = unit_box(vec3f(4.5, -1.0, 1.0), vec3f(0.125, 0.125, 0.125));
    let scene = snapshot(vec![floor.clone(), a.clone(), b, buried.clone()]);
    assert_eq!(scene.padded_bounds(padding), bounds, "the buried box must stay inside the padded bounds");
    let outside = inc.update(Some(scene), &w0);
    assert_eq!(outside.dirty.len(), 1, "the snapshot diff must keep the buried box");
    assert_eq!(outside.bricks, 0, "the buried box must miss both windows");
    assert!(outside.skyline.is_none(), "a box below the floor top must leave the skyline unchanged");
    assert!(!outside.edited, "a diff that touches no transport input must not count as an edit");
    assert_eq!(commit_windows(&mut cs, &mut committed, &inc.windows, &outside, config, &mut clears),
        WindowCommit::default(), "committing the buried box must leave the field asleep");
    assert!(cs.iter().all(|c| c.owed() == 0), "the outside edit must leave zero probe debt");

    // Edit: b moves. Only its old and new boxes are dirty, a is untouched
    // even though the instance order changed.
    let edit = inc.update(Some(snapshot(vec![moved.clone(), a.clone(), floor.clone(), buried.clone()])), &w0);
    assert_eq!(edit.dirty.len(), 2);
    assert!(edit.edited, "moving a resident box must count as an edit");
    let commit = commit_windows(&mut cs, &mut committed, &inc.windows, &edit, config, &mut clears);
    assert!(commit.edited && cs.iter().all(|c| c.owed() == c.count()), "the resident move must wake both drained cascades");
    assert!(edit.bricks < 2 * 16, "edit re-voxelized {} bricks", edit.bricks);
    let map = edit.skyline.as_ref().expect("the edited snapshot publishes a skyline");
    assert_eq!(skyline_height(map, -1.0, -2.0), moved.hi.y, "the added instance raises its column");
    assert_eq!(skyline_height(map, -3.0, 2.0), floor.hi.y, "the old footprint clears");
    assert_eq!(map.heights[0], floor.lo.y - map.cell, "uncovered padding holds the scene floor");
    let mut instances = vec![floor, moved, a, buried];
    // Both regions include the large box at (1, 0, 1). Only the second
    // includes the small box under its surface, inside a kept brick.
    let narrow = (region.0, vec3f(region.1.x, region.1.y, 1.0));
    instances.push(unit_box(vec3f(1.0, 0.5, 1.25), vec3f(0.125, 0.125, 0.125)));
    let same = inc.update(Some(snapshot_in(instances.clone(), Some(narrow))), &w0);
    assert_eq!((same.dirty.len(), same.bricks), (0, 0), "an identical snapshot does nothing");
    assert!(same.rows.iter().all(Option::is_none), "identical static geometry uploads zero texture rows");
    assert!(same.skyline.is_none(), "an identical snapshot keeps the published skyline");

    let resident = inc.update(Some(snapshot_in(instances.clone(), Some(region))), &w0);
    assert!(resident.recentred && resident.bricks > 0 && resident.skyline.is_none() && resident.edited,
        "a membership entry under a resident surface must wake from its bricks alone");

    let edge_scene = |x| {
        let mut scene = instances.clone();
        scene.push(unit_box(vec3f(x, 0.5, 1.0), vec3f(0.125, 0.125, 0.125)));
        snapshot(scene)
    };
    let raised = edge_scene(15.0);
    assert_eq!(raised.padded_bounds(padding), bounds, "the raised box must change only the remote skyline");
    let high = inc.update(Some(raised.clone()), &w0);
    assert_eq!(high.bricks, 0, "the raised box must miss both windows");
    let map = high.skyline.as_ref().expect("a higher remote column must publish a skyline");
    assert_eq!(skyline_height(map, 15.0, 1.0), 0.5625, "the remote column must report the box top");
    assert!(high.edited && !high.recentred, "an unfiltered snapshot adding a remote column must wake the field");
    let loss = inc.update(Some(snapshot_in(instances.clone(), Some(region))), &w0);
    assert!(loss.edited && !loss.recentred, "losing a column beyond the new region must wake when the old snapshot was unfiltered");
    inc.update(Some(raised), &w0);

    let extended = edge_scene(15.25);
    assert!(extended.padded_bounds(padding).1.x > bounds.1.x, "moving within the last covered column must extend the ray bounds");
    let edge = inc.update(Some(extended.clone()), &w0);
    assert_eq!(edge.bricks, 0, "the box extending the bounds must still miss both windows");
    assert!(edge.skyline.is_none(), "the same grid and column heights must reuse the skyline texture");
    assert!(edge.edited, "a change to the exact ray bounds alone must wake the field");

    let mut final_instances = extended.instances.clone();
    final_instances.push(unit_box(vec3f(-8.0, -1.0, 1.0), vec3f(0.125, 0.125, 0.125)));
    let final_scene = snapshot(final_instances);
    let mixed = inc.update(Some(final_scene.clone()), &[w1[0], w0[1]]);
    assert!(mixed.dirty.len() == 1 && mixed.bricks > 0 && !mixed.edited,
        "a remote buried edit in the same job as a fine-window scroll must not take credit for its bricks");
    inc.update(None, &w0); // Start the full two-level scroll from the original windows.
    let scroll = inc.update(None, &w1);
    assert!(scroll.skyline.is_none(), "scrolling the voxel windows reuses the published skyline");
    assert!(!scroll.edited, "bricks exposed only by scrolling must not count as a scene edit");
    let mut full = VoxelWorld::new(l);
    full.update(Some(final_scene), &w1);
    assert!(scroll.bricks > 0 && scroll.bricks < full.windows.len() * 16);
    for level in 0..2 {
        let (s, n) = level_span(&l, level);
        assert!(inc.albedo[s..s + n] == full.albedo[s..s + n], "level {level} albedo differs from a full rebuild");
        assert!(inc.normal[s..s + n] == full.normal[s..s + n]);
    }
}

#[test]
fn mesh_rejects_bad_layouts_and_instances_reject_nonfinite_transforms() {
    assert!(Mesh::new(vec![0.0; 12], vec![0, 1, 2], 3, 5, None).is_none(), "packed colour needs stride 6");
    assert!(Mesh::new(vec![0.0; 9], vec![0, 1, 5], 3, -1, None).is_none(), "index out of range");
    let m = Arc::new(Mesh::new(vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0], vec![0, 1, 2], 3, -1, None).unwrap());
    let mut t = Mat4f::identity(); t.v[12] = f32::NAN;
    assert!(Instance::new(m.clone(), t, vec3f(1.0, 1.0, 1.0), Vec3f::default(), 1.0).is_none());
    let a = Instance::new(m.clone(), Mat4f::identity(), vec3f(1.0, 1.0, 1.0), Vec3f::default(), 1.0).unwrap();
    let b = Instance::new(m.clone(), Mat4f::identity(), vec3f(1.0, 0.0, 1.0), Vec3f::default(), 1.0).unwrap();
    assert_ne!(a.key, b.key, "a recolour is an edit");
    assert_eq!(a.key, Instance::new(m, Mat4f::identity(), vec3f(1.0, 1.0, 1.0), Vec3f::default(), 1.0).unwrap().key);
    let img = gi_image(1000, 10, &vec![7u32; 10000]).unwrap();
    assert_eq!((img.0, img.1, img.2.len()), (128, 10, 1280));
}

fn game_cascade(grid: [usize; 3], level: usize) -> Cascade {
    Cascade::new(GiConfig { grid, ..GiConfig::default() }, level)
}

impl FastGi {
    /// Exercise the renderer settings entry point against an actual resident cascade.
    pub(crate) fn test_transport_issue(&mut self, cx: &mut Cx, lights: &crate::clustered::ClusteredLights) -> Option<f32> {
        if self.cascades.is_empty() {
            let mut cascade = Cascade::new(GiConfig { grid: [4, 2, 4], ..self.config }, 0);
            cascade.scroll([0; 3]);
            self.cascades.push(cascade);
        }
        self.update_transport(cx, Vec3f::default(), &crate::sun::SunLight::default(), lights);
        let count = self.cascades[0].count();
        schedule(&mut self.cascades, count, !self.config.settle, &mut self.batch);
        self.batch.first().map(|item| item.hysteresis)
    }
}

#[test]
fn changing_sun_and_selected_lamp_wake_the_field_until_the_final_input_settles() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let center = Vec3f::default();
    let mut sun = crate::sun::SunLight::default();
    let mut lights = crate::clustered::ClusteredLights::default();
    lights.set_config(crate::clustered::ClusterConfig { tiles_x: 1, tiles_y: 1, slices: 2, ..Default::default() });
    let mut lamp = crate::lightmap::LmLight::omni(center, vec3f(2.0, 1.0, 1.0), 2.0);
    lamp.shadows = true;
    lights.update(&mut cx, std::slice::from_ref(&lamp), None);
    for settle in [false, true] {
        lights.shadows.prepare(std::slice::from_ref(&lamp), &[true], center);
        let mut gi = FastGi::default();
        let config = GiConfig { grid: [4, 2, 4], cascades: 2, settle, feedback: 0.0, ..GiConfig::default() };
        gi.set_config(config);
        gi.set_mode(GiMode::Fast);
        gi.cascades = (0..2).map(|k| Cascade::new(config, k)).collect();
        for cascade in &mut gi.cascades {
            cascade.scroll([0; 3]);
            if settle {
                let needed: [bool; 32] = std::array::from_fn(|i| i % 2 == 0);
                cascade.set_needed([0; 3], &needed);
            }
        }
        let per_level = if settle { 16 } else { 32 };
        let mut out = Vec::with_capacity(64);
        let ids = gi.update_transport(&mut cx, center, &sun, &lights);
        assert_eq!(ids, [0.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0]);
        for _ in 0..13 { schedule(&mut gi.cascades, 64, !settle, &mut out); }
        assert!(!gi.wants_frames(), "both coverage policies stop after cold convergence");

        sun.dir = vec3f(-sun.dir.x, sun.dir.y, sun.dir.z);
        gi.update_transport(&mut cx, center, &sun, &lights);
        assert_eq!(gi.wake_counts.count(WakeCause::SunOrSky), 1, "a sun change counts one sun or sky restart across both levels");
        assert!(gi.cascades.iter().all(|c| c.owed() == per_level), "sun direction re-arms every required probe");
        for _ in 0..19 { schedule(&mut gi.cascades, 64, !settle, &mut out); }
        assert!(!gi.wants_frames());

        for changes in 1..=3 {
            lamp.color.y += 0.125;
            lights.update(&mut cx, std::slice::from_ref(&lamp), None);
            assert_eq!(gi.update_transport(&mut cx, center, &sun, &lights), ids, "lamp indices stayed fixed");
            assert_eq!(gi.wake_counts.count(WakeCause::Lamps), changes, "each same-index colour change counts one lamps restart");
            assert!(gi.cascades.iter().all(|c| c.owed() == per_level), "same-index colour re-arms both levels");
            schedule(&mut gi.cascades, 1, !settle, &mut out);
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].hysteresis, REFRESH_HYSTERESIS);
        }
        // The last colour stopped after a single row. Its unchanged key must
        // let the rest of that sweep and the following transport issues finish.
        for _ in 0..19 {
            gi.update_transport(&mut cx, center, &sun, &lights);
            schedule(&mut gi.cascades, 64, !settle, &mut out);
        }
        assert!(!gi.wants_frames(), "a stopped light animation must let the field sleep");
        assert_eq!(gi.wake_counts.count(WakeCause::Lamps), 3, "draining an unchanged lamp adds no restart");
        gi.update_transport(&mut cx, center, &sun, &lights);
        schedule(&mut gi.cascades, 64, !settle, &mut out);
        assert!(out.is_empty());

        let lamp_wakes = |gi: &FastGi| (
            gi.wake_counts.count(WakeCause::Lamps), gi.wake_counts.count(WakeCause::LampShadows),
        );
        let before = lamp_wakes(&gi);
        let other_pos = vec3f(1.0, 0.0, 0.0);
        let mut pair = [crate::lightmap::LmLight { pos: other_pos, shadows: false, ..lamp.clone() }, lamp.clone()];
        for (list, index, change) in [(&pair[..], 1.0, "inserting"), (&pair[1..], 0.0, "removing")] {
            lights.update(&mut cx, list, None);
            lights.shadows.prepare(list, &[true; 2][..list.len()], center);
            let mut expected = ids;
            expected[0] = index;
            assert_eq!(gi.update_transport(&mut cx, center, &sun, &lights), expected,
                "{change} the earlier light returns the selected lamp's current index");
            assert_eq!(lamp_wakes(&gi), before, "{change} the earlier light preserves both lamp and shadow wake counts");
            assert!(!gi.wants_frames(), "{change} the earlier light leaves the field asleep");
        }

        // Keep the single-lamp field asleep for the shadow checks below.
        pair[0].shadows = true;
        lights.update(&mut cx, &pair, None);
        lights.shadows.prepare(&pair, &[true; 2], center);
        let mut ranked_gi = FastGi::default();
        ranked_gi.set_mode(GiMode::Fast);
        ranked_gi.cascades.push(game_cascade([4, 2, 4], 0));
        ranked_gi.cascades[0].scroll([0; 3]);
        let at_lamp = ranked_gi.update_transport(&mut cx, center, &sun, &lights);
        assert_eq!(at_lamp, [1., 0., -1., -1., -1., -1., -1., -1.], "the original lamp ranks first at its position");
        let before_swap = lamp_wakes(&ranked_gi);
        let at_other = ranked_gi.update_transport(&mut cx, other_pos, &sun, &lights);
        assert_eq!(at_other, [0., 1., -1., -1., -1., -1., -1., -1.], "moving the selection centre reverses both lamp ids");
        assert_eq!(lamp_wakes(&ranked_gi), before_swap, "a rank swap preserves both lamp and shadow wake counts");
        lights.update(&mut cx, std::slice::from_ref(&lamp), None);

        // Leave the three packed lamp texels fixed and prepare without plan_cache,
        // so each wake must come from the shadow sampling inputs alone.
        let mut check_wake = |lights: &crate::clustered::ClusteredLights, cause: &str| {
            let before = gi.wake_counts.count(WakeCause::LampShadows);
            let (static_dirty, mover_dirty, _) = lights.shadows.gi_faces(0);
            assert!(!static_dirty && !mover_dirty, "{cause} wake is independent of dirty content");
            assert_eq!(gi.update_transport(&mut cx, center, &sun, lights), ids);
            assert_eq!(gi.wake_counts.count(WakeCause::LampShadows), before + 1, "a {cause} change counts one lamp shadows restart");
            assert!(gi.cascades.iter().all(|c| c.owed() == per_level), "{cause} must wake every required slot");
            for _ in 0..19 { schedule(&mut gi.cascades, 64, !settle, &mut out); }
            gi.update_transport(&mut cx, center, &sun, lights);
            assert!(!gi.wants_frames(), "unchanged {cause} must let the field sleep");
        };
        let eye = vec3f(80.0, 0.0, 0.0);
        lights.shadows.prepare(std::slice::from_ref(&lamp), &[true], eye);
        check_wake(&lights, "cube projection after resizing its guard texels");
        let mut spot = crate::lightmap::LmLight { cone: Some((20.0, 35.0)), dir: vec3f(0.0, 0.0, -1.0), ..lamp.clone() };
        lights.shadows.prepare(std::slice::from_ref(&spot), &[true], eye);
        check_wake(&lights, "face set");
        spot.dir = vec3f(1.0, 0.0, 0.0);
        lights.shadows.prepare(std::slice::from_ref(&spot), &[true], eye);
        check_wake(&lights, "projection");
        lights.shadows.soft_filter = !lights.shadows.soft_filter;
        check_wake(&lights, "filter");
        lights.shadows.source_radius += 0.25;
        check_wake(&lights, "source radius");

        // Spot projections are independent of tile size. Relight will read the
        // new address when it next runs, even if the selected spot is off-screen.
        let before = lamp_wakes(&gi);
        let tile = lights.shadows.records[0].tile;
        for visible in [true, false] {
            lights.shadows.prepare(std::slice::from_ref(&spot), &[visible], center);
            assert_ne!(lights.shadows.records[0].tile, tile, "the new UV tile differs from the held one");
            assert_eq!(gi.update_transport(&mut cx, center, &sun, &lights), ids);
            assert_eq!(lamp_wakes(&gi), before, "tile size and visibility preserve the held camera rows");
            assert!(!gi.wants_frames(), "the converged field stays asleep");
        }
    }
}

#[test]
fn mover_shadow_faces_restart_nothing_and_refresh_once_when_clean() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let center = Vec3f::default();
    let mut lamps: Vec<_> = (0..9).map(|i| crate::lightmap::LmLight {
        dir: vec3f(0.0, 0.0, -1.0), cone: Some((20.0, 35.0)), shadows: true,
        ..crate::lightmap::LmLight::omni(vec3f(20.0 * i as f32, 0.0, 0.0), vec3f(1.0, 1.0, 1.0), 8.0)
    }).collect();
    let mut lights = crate::clustered::ClusteredLights::default();
    lights.set_config(crate::clustered::ClusterConfig { tiles_x: 1, tiles_y: 1, slices: 2, ..Default::default() });
    lights.shadows.set_config(crate::local_shadows::LocalShadowConfig { resolution: 128, ..Default::default() });
    let mesh = Geometry::new(&mut cx);
    let skin = crate::gpu_lightmap::GpuLmSkin { joint_tex: cx.null_texture(), joint_base: 0.0 };
    let mut mover = crate::gpu_lightmap::GpuLmMover {
        geometry: mesh.geometry_id(), transform: Mat4f::identity(),
        min: vec3f(-0.25, -0.25, -3.25), max: vec3f(0.25, 0.25, -2.75),
        skin: None, morph: None, material: None,
    };
    let mut gi = FastGi::default();
    gi.set_config(GiConfig { feedback: 0.0, ..Default::default() });
    gi.set_mode(GiMode::Fast);
    let mut frame = |lights: &mut crate::clustered::ClusteredLights, lamps: &[crate::lightmap::LmLight], movers: &[crate::gpu_lightmap::GpuLmMover]| {
        lights.update(&mut cx, lamps, None);
        lights.shadows.prepare(lamps, &[true; 10][..lamps.len()], center);
        lights.shadows.plan_cache_for_test(movers);
        let before = gi.wake_counts.count(WakeCause::LampShadows);
        let issue = gi.test_transport_issue(&mut cx, lights);
        let shadow_restarts = gi.wake_counts.count(WakeCause::LampShadows) - before;
        (issue, gi.wake_counts.count(WakeCause::MoverShadowsCleared), shadow_restarts)
    };
    for i in 0..13 {
        let (issue, cleared, shadows) = frame(&mut lights, &lamps, &[]);
        assert!(issue.is_some(), "the cold field issues probes");
        assert_eq!((cleared, shadows), (0, u32::from(i == 0)), "the initial static miss restarts once");
    }
    assert_eq!(lights.gi_light_ids(center), [0., 1., 2., 3., 4., 5., 6., 7.], "the ninth lamp is outside the GI selection");
    assert_eq!(frame(&mut lights, &lamps, &[]), (None, 0, 0), "the static field drains");
    mover.transform.v[12] = 160.0;
    for movers in [std::slice::from_ref(&mover), &[]] {
        assert_eq!(frame(&mut lights, &lamps, movers), (None, 0, 0), "the ninth lamp cannot wake GI");
        assert!(lights.shadows.gi_faces(8).1, "the unselected face has a mover-only miss");
    }
    for x in [0.0, 0.02] {
        mover.transform.v[12] = x;
        assert_eq!(frame(&mut lights, &lamps, std::slice::from_ref(&mover)), (None, 0, 0), "mover entry and motion leave a drained field asleep");
        let (static_dirty, mover_dirty, _) = lights.shadows.gi_faces(0);
        assert!(!static_dirty && mover_dirty, "only the mover changed the selected face");
    }
    assert_eq!(frame(&mut lights, &lamps, std::slice::from_ref(&mover)), (None, 0, 0), "a parked rigid mover lets GI sleep");
    assert_eq!(frame(&mut lights, &lamps, &[]), (None, 0, 0), "an untraced mover's departure issues nothing");
    assert!(lights.shadows.gi_faces(0).1, "departure redraws the previously occupied face");
    assert_eq!(frame(&mut lights, &lamps, &[]), (None, 0, 0), "a clean face owes no refresh for an untraced mover");

    // A colour change wakes the field on the same frame it sees a mover.
    lamps[1].color.y = 0.75;
    mover.skin = Some(skin);
    for i in 0..20 {
        mover.transform.v[12] = lamps[1].pos.x + i as f32 * 0.02;
        let (issue, cleared, shadows) = frame(&mut lights, &lamps, std::slice::from_ref(&mover));
        let (static_dirty, mover_dirty, _) = lights.shadows.gi_faces(1);
        assert!(!static_dirty && mover_dirty, "a moving caster dirties only mover content throughout the warm sweep");
        assert_eq!(issue, (i < 19).then_some(REFRESH_HYSTERESIS), "the colour change pays one warm sweep and drains while the mover is present");
        assert_eq!(cleared, 0, "a mover still in the face cannot trigger its cleanup");
        assert_eq!(shadows, u32::from(i == 0), "only the recoloured lamp row changes the shadow face set");
    }
    // The clean first lamp takes the touched lamp's old index.
    lamps.insert(0, crate::lightmap::LmLight { shadows: false, ..lamps[0].clone() });
    assert_eq!(frame(&mut lights, &lamps, std::slice::from_ref(&mover)), (None, 0, 0),
        "renumbering a touched lamp leaves the field asleep without a cleanup restart");
    assert_eq!(lights.gi_light_ids(center), [1., 2., 3., 4., 5., 6., 7., 8.], "all selected lamps shifted one index");
    assert!(lights.shadows.gi_faces(2).1, "the mover is still in the renumbered lamp's face");
    assert_eq!(frame(&mut lights, &lamps, &[]), (None, 0, 0), "redrawing the departing mover leaves the field asleep");
    assert!(lights.shadows.gi_faces(2).1, "the departure frame still has a mover-only miss");
    for _ in 0..19 {
        assert_eq!(frame(&mut lights, &lamps, &[]), (Some(REFRESH_HYSTERESIS), 1, 0), "the first clean frame starts exactly one warm sweep");
        let (static_dirty, mover_dirty, _) = lights.shadows.gi_faces(2);
        assert!(!static_dirty && !mover_dirty, "the emptied face stays cached throughout the refresh");
    }
    assert_eq!(frame(&mut lights, &lamps, &[]), (None, 1, 0), "the field sleeps after the single cleanup sweep");
}

#[test]
fn slots_round_trip_and_scrolling_queues_only_the_exposed_slab() {
    let mut c = game_cascade([4, 2, 4], 0);
    assert_eq!(c.scroll([0, 0, 0]).len(), 32, "a first window traces everything");
    for s in 0..c.count() { assert_eq!(c.slot(c.cell(s).unwrap()), s); }
    let mut out = Vec::with_capacity(1000);
    schedule(std::slice::from_mut(&mut c), 1000, true, &mut out);
    assert_eq!(out.len(), 32, "each probe at most once per frame");
    assert_eq!(c.owed(), 32, "every resident probe still owes cold transport after its first trace");
    assert!(out.iter().all(|b| b.hysteresis == 0.0));
    assert_eq!(c.scroll([1, 0, 0]).len(), 8, "one x slab of 2x4 probes");
    schedule(std::slice::from_mut(&mut c), 8, true, &mut out);
    assert!(out.iter().all(|b| b.cell[0] == 4), "the new slab is traced first: {out:?}");
    assert_eq!(c.pending(), 0);
    assert_eq!(c.scroll([100, 0, 0]).len(), 32, "a teleport re-targets every slot");
}

#[test]
fn committed_scroll_preserves_kept_probes_and_traces_exposed_slots() {
    for settle in [false, true] { check_committed_scroll(settle); }
}

fn check_committed_scroll(settle: bool) {
    let config = GiConfig { grid: [4, 2, 4], cascades: 2, settle, ..GiConfig::default() };
    let mut cs = [Cascade::new(config, 0), Cascade::new(config, 1)];
    let mut committed = [None; 2];
    let mut windows = [Some([0; 3]); 2];
    let mut clears = Vec::with_capacity(64);
    let mut out = Vec::with_capacity(64);
    let all_needed = VoxelUpdate { needed: vec![Some(vec![true; 32]); 2], ..VoxelUpdate::default() };
    let unchanged = VoxelUpdate::default();
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &all_needed, config, &mut clears),
        WindowCommit { scrolled: 64, new_cells: if settle { 64 } else { 0 }, first: true, ..Default::default() },
        "the first publication reports exposed probes and only newly required cells");
    assert!(cs.iter().all(|c| (0..c.count()).all(|s| c.remaining(s) == 39)),
        "the first publication arms every required slot for the full cold countdown");
    for _ in 0..39 { schedule(&mut cs, 64, !settle, &mut out); }
    assert!(cs.iter().all(|c| c.owed() == 0), "start the scroll from a drained field");

    clears.clear();
    windows[0] = Some([4, 0, 0]); // Four voxels per probe cell.
    let slab_move = WindowCommit { scrolled: 8, moved: true, ..Default::default() };
    if settle {
        let mut needed = [true; 32];
        needed[0] = false; // The incoming mask will require this slot for its new cell.
        cs[0].set_needed([0; 3], &needed);
    }
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &all_needed, config, &mut clears),
        WindowCommit { retraced: 24, new_cells: usize::from(settle), ..slab_move },
        "moving the fine window re-traces its three kept slabs and exposes one");
    let slab: Vec<_> = (0..32).step_by(4).map(|slot| (0, slot)).collect();
    assert_eq!(clears, slab, "only retargeted cells are cleared");
    assert_eq!((cs[0].owed(), cs[1].owed()), (32, 0), "only the fine window wakes");
    for s in 0..32 {
        assert_eq!(cs[0].remaining(s), if slab.contains(&(0, s)) { 7 } else { 19 }, "fine slot {s} after the move");
        assert_eq!(cs[1].remaining(s), 0, "coarser slot {s} stays asleep");
    }
    schedule_after_clears(&mut cs, 8, !settle, !clears.is_empty(), &mut out);
    assert!(out.is_empty(), "the commit frame, its clears not yet drawn, issues no probes");
    assert_eq!((cs[0].owed(), cs[1].owed()), (32, 0), "the held frame keeps every countdown");
    assert!((0..32).all(|s| cs[0].remaining(s) == if slab.contains(&(0, s)) { 7 } else { 19 }),
        "the held frame spends no issue");
    clears.clear(); // the scatter drew them
    schedule_after_clears(&mut cs, 8, !settle, !clears.is_empty(), &mut out);
    assert_eq!(out.iter().map(|b| (b.cascade, b.slot)).collect::<Vec<_>>(), slab,
        "each cleared slot gets its priority trace");
    assert!(out.iter().all(|b| b.hysteresis == 0.0 && b.cell[0] == 4), "the new cells discard old history");
    assert_eq!(cs[0].pending(), 0, "the first traces drain the slab queue");

    clears.clear();
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &unchanged, config, &mut clears),
        WindowCommit::default(), "re-adopting the same windows is a no-op");
    assert!(clears.is_empty(), "re-adopting the same windows adds no clears");
    let mut issued = out.len();
    for n in 1..=19 {
        schedule(&mut cs, 64, !settle, &mut out);
        for b in &out {
            assert_eq!(b.cascade, 0, "the fine move leaves the coarse cascade asleep");
            if b.cell[0] == 4 {
                assert!(n <= 6, "the exposed slab finishes after six more ray sets");
                assert_eq!(b.hysteresis, n as f32 / (n + 1) as f32, "direct issue {} averages the next ray set", n + 1);
            } else {
                assert_eq!(b.hysteresis, REFRESH_HYSTERESIS, "kept probes re-trace from their held light");
            }
        }
        issued += out.len();
    }
    assert_eq!(issued, 8 * 7 + 24 * 19, "eight exposed slots and 24 warm re-traces cost exactly 512 issues");
    assert!(cs.iter().all(|c| c.owed() == 0), "the slab and the kept probes both drain");
    schedule(&mut cs, 64, !settle, &mut out);
    assert!(out.is_empty(), "a settled field schedules no further issues");

    if settle {
        let mut needed = [true; 32];
        needed[1] = false; // Traced cell [1, 0, 0] will stay in the coarse window.
        cs[1].set_needed([0; 3], &needed);
    }
    windows[1] = Some([4, 0, 0]);
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &unchanged, config, &mut clears),
        WindowCommit { retraced: 32 + 24 - usize::from(settle), ..slab_move },
        "the coarse move re-traces every fine slot and every required kept coarse slot");
    for s in 0..32 {
        assert_eq!(cs[0].remaining(s), 19, "fine slot {s} traces the moved coarse level");
        let expected = if clears.contains(&(1, s)) { 39 } else if settle && s == 1 { 0 } else { 19 };
        assert_eq!(cs[1].remaining(s), expected, "coarse slot {s} after the move");
    }
    schedule(&mut cs, 64, !settle, &mut out);
    let before = cs.each_ref().map(|c| (0..c.count()).map(|s| c.remaining(s)).collect::<Vec<_>>());
    clears.clear();
    let recentre = VoxelUpdate { recentred: true, dirty: vec![(vec3f(-12.0, 0.0, 0.0), vec3f(-11.0, 1.0, 1.0))], ..Default::default() };
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &recentre, config, &mut clears),
        WindowCommit { recentred: true, ..Default::default() }, "membership alone arms no re-traces");
    assert!(clears.is_empty());
    for (level, c) in cs.iter().enumerate() {
        assert!((0..c.count()).all(|s| c.remaining(s) == before[level][s]), "membership preserves level {level}'s countdowns");
    }

    windows[0] = Some([8, 0, 0]);
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &recentre, config, &mut clears),
        WindowCommit { retraced: 24, recentred: true, ..slab_move }, "the fine move tops up its in-flight re-traces");
    for s in 0..32 {
        assert_eq!(cs[0].remaining(s), if clears.contains(&(0, s)) { 39 } else { 19 }, "fine slot {s} after the second move");
        assert_eq!(cs[1].remaining(s), before[1][s], "the fine move preserves coarse slot {s}'s countdown");
    }
    clears.clear();
    windows[0] = Some([12, 0, 0]);
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &unchanged, config, &mut clears), slab_move,
        "a further fine move leaves the full re-traces and pending first traces alone");
    schedule(&mut cs, 8, !settle, &mut out);
    assert!(out.len() == 8 && out.iter().all(|b| b.cascade == 0 && b.cell[0] == 5 && b.hysteresis == 0.0),
        "the preceding slab keeps its priority traces ahead of the newest slab");
    cs[0].restart();
    for s in 0..cs[0].count() {
        match cs[0].cell(s).unwrap()[0] {
            5 => assert_eq!(cs[0].remaining(s), 38, "restart keeps the first trace and restores transport for slot {s}"),
            6 => assert_eq!(cs[0].remaining(s), 39, "restart restores the full cold count for untraced slot {s}"),
            _ => assert_eq!(cs[0].remaining(s), 84, "re-traced slot {s} restarts warm"),
        }
    }

    clears.clear();
    windows.fill(Some([400, 0, 0]));
    // In settle mode the coarse window exposes slots but owes nothing. Its move
    // alone must still give the fine window the cold count in this publication.
    let cut = VoxelUpdate { needed: vec![None, Some(vec![!settle; 32])], ..VoxelUpdate::default() };
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &cut, config, &mut clears),
        WindowCommit { scrolled: 64, moved: true, ..Default::default() }, "a teleport replaces both windows");
    let coarse_owed = if settle { 0 } else { 32 };
    assert_eq!((cs[0].owed(), cs[1].owed()), (32, coarse_owed), "a cut wakes every required exposed slot");
    assert!((0..32).all(|s| cs[0].remaining(s) == 39), "a coarser exposure in this commit makes the fine slots cold");
    if !settle {
        assert!((0..32).all(|s| cs[1].remaining(s) == 39), "the top cascade always gets the cold count");
    }
    schedule(&mut cs, 64, !settle, &mut out);
    assert_eq!(out.len(), 32 + coarse_owed, "every required teleported slot gets a first trace");
    assert!(out.iter().all(|b| b.hysteresis == 0.0), "a full teleport leaves no retained history");

    clears.clear();
    windows[0] = Some([404, 0, 0]);
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &unchanged, config, &mut clears), slab_move,
        "only the fine window moves while the field is settling");
    let issues = if coarse_owed > 0 { 39 } else { 7 };
    assert!(clears.iter().all(|&(k, s)| cs[k].remaining(s) == issues),
        "a stationary coarser cascade with debt requires cold issues, a drained one permits direct issues");
    assert!((0..32).filter(|&s| !clears.contains(&(0, s))).all(|s| cs[0].remaining(s) == 38),
        "kept fine probes retain their longer cold transport countdown");
    schedule(&mut cs, 64, !settle, &mut out);
    assert_eq!(out.iter().filter(|b| b.cascade == 0 && !clears.contains(&(0, b.slot)) && b.hysteresis == 0.5).count(),
        24, "all kept fine probes continue cold transport");
    for _ in 1..39 { schedule(&mut cs, 64, !settle, &mut out); }
    assert!(cs.iter().all(|c| c.owed() == 0), "the teleported field must drain before the remote edit");
    let mut remote_edit = VoxelUpdate { dirty: vec![(Vec3f::default(), vec3f(1.0, 1.0, 1.0))], ..Default::default() };
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &remote_edit, config, &mut clears),
        WindowCommit::default(), "remote dirty boxes with unchanged transport inputs must commit without a wake");
    assert!(cs.iter().all(|c| c.owed() == 0), "the no-op commit must leave every cascade at zero debt");
    schedule(&mut cs, 64, !settle, &mut out);
    assert!(out.is_empty(), "the sleeping field must schedule no probes after the no-op commit");
    remote_edit.edited = true;
    assert_eq!(commit_windows(&mut cs, &mut committed, &windows, &remote_edit, config, &mut clears),
        WindowCommit { edited: true, ..Default::default() },
        "the same remote boxes with changed transport must count as a scene edit even with zero dirty probes");
    assert_eq!((cs[0].owed(), cs[1].owed()), (32, coarse_owed), "a remote transport change must wake every required probe");
}

#[test]
fn edits_queue_nearby_probes_ahead_of_refresh_with_low_hysteresis() {
    let mut c = game_cascade([8, 4, 8], 0);
    c.scroll([0, 0, 0]);
    let mut out = Vec::with_capacity(10_000);
    schedule(std::slice::from_mut(&mut c), 10_000, true, &mut out);
    let n = c.invalidate([3.0, 1.0, 3.0], [3.5, 1.5, 3.5], [0.43, 0.47, 0.41]);
    assert!(n > 0 && n < 64, "{n} probes near a 0.5 m edit");
    schedule(std::slice::from_mut(&mut c), n, true, &mut out);
    assert_eq!(out.len(), n);
    assert!(out.iter().all(|b| b.hysteresis == DIRTY_HYSTERESIS));
    let priority: Vec<_> = out.iter().map(|b| b.slot).collect();
    assert_eq!(c.pending(), 0, "all nearby probes received their priority issue");
    schedule(std::slice::from_mut(&mut c), 5, true, &mut out);
    let rows: Vec<_> = out.iter().map(|b| (b.slot, b.hysteresis)).collect();
    assert_eq!(rows, [(0, 0.5), (161, 0.5), (66, 0.5), (227, 0.5), (132, 0.5)],
        "the stride resumes cold transport, including edited slot 66");
    schedule(std::slice::from_mut(&mut c), 251, true, &mut out);
    for slot in priority {
        let histories = rows.iter().copied().chain(out.iter().map(|b| (b.slot, b.hysteresis)))
            .filter(|&(issued, _)| issued == slot).map(|(_, h)| h);
        assert!(histories.eq([0.5]), "edited slot {slot} must start transport exactly once at 0.5");
    }
    for _ in 0..37 { schedule(std::slice::from_mut(&mut c), 256, true, &mut out); }
    assert_eq!(c.owed(), 0, "the edit and the rest of the cold field drain");
    c.restart();
    schedule(std::slice::from_mut(&mut c), 5, true, &mut out);
    assert_eq!(out.len(), 5);
    assert!(out.iter().all(|b| b.hysteresis == REFRESH_HYSTERESIS));
}

#[test]
fn refresh_visits_every_probe_once_per_cycle_and_splits_the_budget() {
    let mut cs = vec![game_cascade([4, 2, 4], 0), game_cascade([4, 2, 4], 1), game_cascade([4, 2, 4], 2)];
    for c in &mut cs { c.scroll([0, 0, 0]); }
    let mut out = Vec::with_capacity(10_000);
    for _ in 0..39 { schedule(&mut cs, 10_000, true, &mut out); }
    assert!(cs.iter().all(|c| c.owed() == 0));
    for c in &mut cs { c.restart(); }
    let mut seen = vec![0; 32];
    for _ in 0..4 {
        schedule(&mut cs, 16, true, &mut out);
        assert_eq!(out.len(), 16);
        assert!(out.iter().all(|b| b.hysteresis == REFRESH_HYSTERESIS));
        assert_eq!(out.iter().filter(|b| b.cascade == 0).count(), 8, "half the budget goes to the fine cascade");
        let mut slots: Vec<_> = out.iter().map(|b| (b.cascade, b.slot)).collect(); slots.sort(); slots.dedup();
        assert_eq!(slots.len(), 16);
        for b in out.iter().filter(|b| b.cascade == 0) { seen[b.slot] += 1; }
    }
    assert!(seen.iter().all(|&n| n == 1), "{seen:?}");
    // A cascade with queued work gets the others' unused share first.
    cs[2].scroll([50, 0, 0]);
    schedule(&mut cs, 16, true, &mut out);
    assert_eq!(out.iter().filter(|b| b.cascade == 2).count(), 16);
}

#[test]
fn one_row_rotates_between_busy_cascades_and_keeps_each_stride() {
    let mut cs = [0, 1, 2].map(|k| game_cascade([4, 2, 4], k));
    for c in &mut cs { c.scroll([0; 3]); }
    let mut out = Vec::with_capacity(96);
    let mut first_traces = [0; 3];
    for _ in 0..3 {
        schedule(&mut cs, 1, true, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].hysteresis, 0.0);
        first_traces[out[0].cascade] += 1;
    }
    assert_eq!(first_traces, [1; 3], "queued first traces share a one-row budget");

    for _ in 0..39 { schedule(&mut cs, 96, true, &mut out); }
    assert!(cs.iter().all(|c| c.owed() == 0));
    for c in &mut cs { c.restart(); }
    let mut seen = [[false; 32]; 3];
    for _ in 0..32 {
        let mut served = [0; 3];
        for _ in 0..3 {
            schedule(&mut cs, 1, true, &mut out);
            assert_eq!(out.len(), 1);
            let probe = out[0];
            assert_eq!(probe.hysteresis, 0.85);
            served[probe.cascade] += 1;
            assert!(!seen[probe.cascade][probe.slot], "each stride continues where it stopped");
            seen[probe.cascade][probe.slot] = true;
        }
        assert_eq!(served, [1; 3], "a persistent warm cascade cannot starve");
    }

    let mut finite_rows = [0; 3];
    for _ in 0..3 {
        schedule(&mut cs, 37, false, &mut out);
        for probe in &out { finite_rows[probe.cascade] += 1; }
    }
    assert_eq!(finite_rows, [55, 28, 28], "finite refresh rotates the odd row too");
}

#[test]
fn finite_batches_match_the_queued_and_strided_sequence_then_stop() {
    let mut cascade = game_cascade([4, 2, 2], 0);
    let mut out = Vec::with_capacity(16);
    let mut check = |cascade: &mut Cascade, expected: &[(usize, f32)]| {
        schedule(std::slice::from_mut(cascade), expected.len(), true, &mut out);
        assert_eq!(out.len(), expected.len());
        for (item, &(slot, history)) in out.iter().zip(expected) {
            assert_eq!((item.cascade, item.slot, item.hysteresis), (0, slot, history));
        }
    };
    cascade.scroll([0; 3]);
    check(&mut cascade, &(0..16).map(|s| (s, 0.0)).collect::<Vec<_>>());
    // The original 16-slot sweep, four rows per frame.
    for slots in [[0, 11, 6, 1], [12, 7, 2, 13], [8, 3, 14, 9], [4, 15, 10, 5]] {
        check(&mut cascade, &slots.map(|s| (s, 0.5)));
    }
    cascade.invalidate([-1.5; 3], [-1.5; 3], [0.0; 3]);
    check(&mut cascade, &[(0, 0.2), (11, 0.5), (6, 0.5), (1, 0.5)]);
    cascade.scroll([1, 0, 0]);
    check(&mut cascade, &[(0, 0.0), (4, 0.0), (8, 0.0), (12, 0.0), (7, 0.5), (2, 0.5)]);
    for _ in 0..38 { schedule(std::slice::from_mut(&mut cascade), 16, true, &mut out); }
    assert_eq!(cascade.owed(), 0);
    schedule(std::slice::from_mut(&mut cascade), 16, true, &mut out);
    assert!(out.is_empty(), "the same queue and stride terminate after the last exposed slot converges");
}

#[test]
fn finite_issues_drain_cold_warm_and_near_edit_debt() {
    let config = GiConfig { grid: [4, 2, 4], settle: true, ..GiConfig::default() };
    let mut c = Cascade::new(config, 0);
    c.scroll([0; 3]);
    let mut mask = [false; 32];
    mask[0] = true;
    mask[31] = true;
    c.set_needed([0; 3], &mask);
    mask[31] = false;
    c.set_needed([0; 3], &mask);
    assert_eq!((c.pending(), c.owed()), (1, 1), "dropping a queued slot retires its debt");
    let mut out = Vec::with_capacity(1);
    let check_issue = |c: &mut Cascade, out: &mut Vec<BatchItem>, history| {
        assert_eq!(c.owed(), 1);
        schedule(std::slice::from_mut(c), 1, false, out);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].slot, out[0].hysteresis), (0, history));
    };
    for first in [0.0, 0.2] {
        if first != 0.0 {
            c.invalidate([-1.5; 3], [-1.5; 3], [0.0; 3]);
            c.restart(); // A simultaneous global wake preserves the near-edit issue.
        }
        check_issue(&mut c, &mut out, first);
        if first == 0.2 {
            check_issue(&mut c, &mut out, 0.5);
            c.restart();
            assert_eq!(c.pending(), 0, "a traced near-edit probe resumes on the refresh stride");
        }
        for _ in 0..32 { check_issue(&mut c, &mut out, 0.5); }
        for n in 1..=6 { check_issue(&mut c, &mut out, n as f32 / (n + 1) as f32); }
        assert_eq!(c.owed(), 0, "the transport and noise phases consume the remaining debt");
    }
    c.restart();
    for _ in 0..84 { check_issue(&mut c, &mut out, 0.85); }
    assert_eq!(c.owed(), 0);
    c.set_transport(GiConfig { feedback: 0.0, ..config });
    for _ in 0..19 { check_issue(&mut c, &mut out, 0.85); }
    assert_eq!(c.owed(), 0, "zero feedback still owes the direct-light decay");

    let visits = c.refresh_visits;
    schedule(std::slice::from_mut(&mut c), 32, false, &mut out);
    assert!(out.is_empty());
    c.set_needed([0; 3], &[false; 32]);
    c.restart();
    schedule(std::slice::from_mut(&mut c), 32, false, &mut out);
    assert!(out.is_empty());
    assert_eq!(c.refresh_visits, visits, "drained and empty masks both skip the slot walk");
}

#[test]
fn off_allocates_nothing_and_mode_changes_discard_the_field() {
    let mut gi = FastGi::default();
    assert!(gi.gpu.is_none());
    assert!(!gi.wants_scene([0; 6], Vec3f::default()), "off never asks for a snapshot");
    gi.set_mode(GiMode::Fast);
    assert!(gi.wants_scene([0; 6], Vec3f::default()));
    gi.submit_scene([0; 6], Vec3f::default(), Snapshot { instances: vec![], triangles: 0, region: None });
    assert!(!gi.wants_scene([1; 6], Vec3f::default()), "one snapshot in flight at a time");
    assert_eq!(gi.owes(), None, "a field that has not run owes nothing: a view never drawn must not hold its host");
    gi.center = Some(Vec3f::default());
    assert_eq!(gi.owes(), Some("a scene snapshot"), "a submitted scene not yet voxelised is owed");
    gi.worker_failed = true;
    assert_eq!(gi.owes(), None, "after a failed voxel job (logged) nothing is owed: the field draws its fallback");
    gi.worker_failed = false;
    gi.set_config(GiConfig { strength: 0.5, feedback: 0.2, ..gi.config() });
    assert!(gi.pending_scene.is_some(), "lighting-only controls keep the scene");
    gi.set_config(GiConfig { spacing: 2.0, ..gi.config() });
    assert!(gi.pending_scene.is_none() && gi.scene_key.is_none());
    gi.set_mode(GiMode::Off);
    assert!(gi.gpu.is_none() && gi.voxels.is_none());
    assert_eq!(gi.owes(), None, "a field that is off owes its host nothing");
}

#[test]
fn a_realm_change_keeps_the_field_and_rediffs_the_scene() {
    let mut gi = FastGi::default();
    gi.set_mode(GiMode::Fast);
    gi.voxels = Some(Box::new(VoxelWorld::new(small_layout())));
    gi.submit_scene([7; 6], Vec3f::default(), Snapshot { instances: vec![], triangles: 0, region: None });
    gi.reset();
    assert!(gi.voxels.is_some(), "hot reload keeps the voxels: the next snapshot is a diff");
    assert!(gi.pending_scene.is_none(), "the departing world's snapshot is not sent");
    assert!(gi.wants_scene([7; 6], Vec3f::default()), "even an equal key re-snapshots the new world");
}

#[test]
fn emission_keeps_its_hue_and_intensity() {
    let l = small_layout();
    let mut lamp = unit_box(vec3f(0.1, 0.1, 0.1), vec3f(1.0, 1.0, 1.0));
    let Solid::Mesh(lamp_mesh) = lamp.solid.clone() else { panic!("a mesh") };
    lamp = Instance::new(lamp_mesh, lamp.transform, lamp.tint, vec3f(3.0, 1.5, 0.0), 1.0).unwrap();
    let mut w = VoxelWorld::new(l);
    w.update(Some(snapshot(vec![lamp])), &[l.window_for(0, Vec3f::default()), l.window_for(1, Vec3f::default())]);
    let px = w.emission[l.texel(0, [2, 1, 0])];
    assert_eq!(((px >> 16) & 255, (px >> 8) & 255, px & 255), (255, 128, 0), "hue relative to the strongest channel");
    let a = (px >> 24) as f32 / 255.0;
    assert!((a / (1.0 - a) - 3.0).abs() < 0.2, "intensity m/(1+m) decodes back to 3");
}

fn quad(pos: Vec3f, size: Vec3f, tint: Vec3f, emission: Vec3f) -> Instance {
    let mesh = Arc::new(Mesh::new(
        vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
        vec![0, 1, 2, 0, 2, 3], 3, -1, None,
    ).unwrap());
    let mut transform = Mat4f::identity();
    transform.v[0] = size.x; transform.v[5] = size.y; transform.v[10] = size.z;
    transform.v[12] = pos.x; transform.v[13] = pos.y; transform.v[14] = pos.z;
    Instance::new(mesh, transform, tint, emission, 1.0).unwrap()
}

fn decoded_emission(px: u32) -> Vec3f {
    let alpha = (px >> 24) as f32 / 255.0;
    vec3f(((px >> 16) & 255) as f32, ((px >> 8) & 255) as f32, (px & 255) as f32)
        * (alpha / (1.0 - alpha) / 255.0)
}

#[test]
fn emissive_quads_scale_with_clipped_area_and_saturate_at_a_full_face() {
    let layout = VoxelLayout { dims: [BRICK; 3], levels: 1, voxel: 1.0, home: [0.0; 3] };
    let mut world = VoxelWorld::new(layout);
    let white = vec3f(1.0, 1.0, 1.0);
    let emission = vec3f(4.0, 2.0, 0.0);
    world.update(Some(snapshot(vec![
        quad(vec3f(0.4, 0.4, 0.5), vec3f(0.2, 0.2, 1.0), white, emission),
        quad(vec3f(2.0, 0.0, 0.5), white, white, emission),
        // Clip the two triangles of a large panel across two voxel faces.
        quad(vec3f(0.0, 2.0, 0.5), vec3f(2.0, 1.0, 1.0), white, emission),
        // More than a face of emissive area keeps the full radiance.
        quad(vec3f(3.0, 2.0, 0.25), white, white, emission),
        quad(vec3f(3.0, 2.0, 0.75), white, white, emission),
        // A small source straddling a voxel boundary splits its area.
        quad(vec3f(1.9, 3.4, 0.5), vec3f(0.2, 0.2, 1.0), white, emission),
    ])), &[[0, 0, 0]]);
    let read = |cell| decoded_emission(world.emission[layout.texel(0, cell)]);
    let small = read([0, 0, 0]);
    let full = read([2, 0, 0]);
    // The existing 8-bit m/(1+m) encoding quantizes the intensity.
    assert!((full.x - emission.x).abs() < 0.02, "a full face retains full emission");
    assert!((small.x - full.x / 25.0).abs() < 0.003, "a 0.2 m square covers 1/25 of a 1 m face");
    assert!((small.y - small.x * 0.5).abs() < 0.002, "coverage preserves the emission hue");
    for cell in [[0, 2, 0], [1, 2, 0], [3, 2, 0]] {
        assert!((read(cell).x - full.x).abs() < 0.02, "full coverage at {cell:?} retains full radiance");
    }
    for cell in [[1, 3, 0], [2, 3, 0]] {
        assert!((read(cell).x - emission.x * 0.02).abs() < 0.003, "only the clipped area contributes at {cell:?}");
    }
    assert_eq!(world.solid(0, [3, 0, 0]), Some(false), "an edge-only contact has zero area");
}

#[test]
fn voxel_albedo_weights_surface_area_and_encodes_empty_bricks() {
    // Brute-force the aligned regions against the known geometry in world coordinates.
    fn check_alpha(world: &VoxelWorld, solid: fn([i32; 3]) -> bool) {
        let [ox, oy, oz] = world.windows[0].unwrap();
        for z in oz..oz + 8 { for y in oy..oy + 8 { for x in ox..ox + 8 {
            let cell = [x, y, z];
            let empty = |size| {
                let [bx, by, bz] = cell.map(|i| i.div_euclid(size) * size);
                (bz..bz + size).all(|k| (by..by + size).all(|j| (bx..bx + size).all(|i| !solid([i, j, k]))))
            };
            let expected = if solid(cell) { 255 } else if empty(8) { 64 } else if empty(4) { 32 } else { 0 };
            let texel = world.albedo[world.layout.texel(0, cell)];
            assert_eq!(texel >> 24, expected, "wrong alpha at {cell:?}");
            if expected < 128 { assert_eq!(texel & 0x00ff_ffff, 0, "empty RGB at {cell:?}"); }
        }}}
    }

    let layout = VoxelLayout { dims: [BRICK; 3], levels: 1, voxel: 1.0, home: [0.0; 3] };
    let mut world = VoxelWorld::new(layout);
    let red = vec3f(1.0, 0.0, 0.0);
    let blue = vec3f(0.0, 0.0, 1.0);
    let emission = vec3f(4.0, 0.0, 0.0);
    world.update(Some(snapshot(vec![
        // Four red triangles cover a quarter face; two blue triangles
        // cover the other three quarters. Triangle counts must not matter.
        quad(vec3f(0.0, 0.0, 0.5), vec3f(0.125, 1.0, 1.0), red, emission),
        quad(vec3f(0.125, 0.0, 0.5), vec3f(0.125, 1.0, 1.0), red, emission),
        quad(vec3f(0.25, 0.0, 0.5), vec3f(0.75, 1.0, 1.0), blue, Vec3f::default()),
        // A dark full face behind them increases total surface area to
        // two faces, but must not dilute the quarter face of emission.
        quad(vec3f(0.0, 0.0, 0.75), vec3f(1.0, 1.0, 1.0), blue, Vec3f::default()),
    ])), &[[0, 0, 0]]);
    let t = layout.texel(0, [0, 0, 0]);
    let albedo = world.albedo[t];
    assert_eq!(((albedo >> 16) & 255, (albedo >> 8) & 255, albedo & 255), (32, 0, 223), "albedo is the area-weighted mean");
    assert!((decoded_emission(world.emission[t]).x - 1.0).abs() < 0.01, "only the quarter face that emits sets emission coverage");
    check_alpha(&world, |cell| cell == [0, 0, 0]);

    // Move the solid into a previously empty 4-brick. The brick to the left
    // has one full face in each z slice and only enters the texture on scroll.
    let mut scene = vec![quad(vec3f(7.25, 0.25, 7.5), vec3f(0.5, 0.5, 1.0), red, Vec3f::default())];
    for z in 0..8 {
        scene.push(quad(vec3f(-8.0, 0.0, z as f32 + 0.5), vec3f(8.0, 8.0, 1.0), blue, Vec3f::default()));
    }
    world.track_needed = true;
    let edit = world.update(Some(snapshot(scene)), &[[0, 0, 0]]);
    check_alpha(&world, |cell| cell == [7, 0, 7]);
    assert_eq!(edit.needed[0].as_ref().unwrap(), &[false, false, false, false, false, true, false, true],
        "only probes touching the solid corner are needed");
    world.update(None, &[[-8, 0, 0]]);
    check_alpha(&world, |_| true);
    let scroll = world.update(None, &[[-16, 0, 0]]);
    check_alpha(&world, |_| false);
    assert!(scroll.needed[0].as_ref().unwrap().iter().all(|p| !p), "an empty 8-brick needs no probes");
}

#[test]
fn camera_drift_asks_for_a_new_snapshot_before_the_window_leaves_the_region() {
    let mut gi = FastGi::default();
    gi.set_mode(GiMode::Fast);
    gi.submit_scene([0; 6], Vec3f::default(), Snapshot { instances: vec![], triangles: 0, region: None });
    gi.pending_scene = None;
    assert!(!gi.wants_scene([0; 6], vec3f(20.0, 0.0, 0.0)));
    assert!(gi.wants_scene([0; 6], vec3f(30.0, 0.0, 0.0)));
    let (lo, hi) = gi.region(Vec3f::default());
    let e = gi.coarsest_extent();
    // Worst case: drift 0.4 extent + half a window + a brick of hysteresis.
    let reach = e.x * 0.4 + e.x * 0.5 + 1.5 * 8.0 * 1.0;
    assert!(hi.x >= reach && lo.x <= -reach);
}

#[test]
fn batches_stay_full_through_warmup_edits_and_sweeps_without_reallocating() {
    let grid = [4, 2, 4];
    let mut gi = FastGi::default();
    gi.set_config(GiConfig { grid, ..GiConfig::default() });
    gi.cascades = (0..3).map(|i| game_cascade(grid, i)).collect();
    for c in &mut gi.cascades { c.scroll([0; 3]); }
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut sun = crate::sun::SunLight::default();
    let clustered = crate::clustered::ClusteredLights::default();
    let capacity = batch_capacity(32, 3);
    let mut out = Vec::with_capacity(capacity);
    let allocation = out.as_ptr();
    for frame in 0..600 {
        sun.color.x = frame as f32 / 600.0;
        gi.update_transport(&mut cx, Vec3f::default(), &sun, &clustered);
        let cascades = &mut gi.cascades;
        if frame == 200 { cascades[0].invalidate([0.0; 3], [1.0; 3], HOME); }
        if frame == 350 { cascades[0].scroll([20, 0, 0]); }
        if frame == 400 { for c in cascades.iter_mut() { c.scroll([20, 0, 0]); } }
        // Includes awkward rounding and the largest legal weighted batch.
        let budget = if frame < 300 { 37 } else if frame < 500 { capacity } else { capacity - 1 };
        schedule(cascades, budget, true, &mut out);
        assert_eq!(out.len(), budget, "frame {frame}: no short frame or catch-up batch");
        if frame == 100 {
            let issued = [0, 1, 2].map(|k| out.iter().filter(|p| p.cascade == k).count());
            assert_eq!(issued, [19, 9, 9], "ordinary rounding stays on the fine cascade");
        }
        assert_eq!(out.as_ptr(), allocation, "reuse frame storage");
        assert_eq!(cascades.iter().map(Cascade::owed).sum::<usize>(), 96, "re-arm counts each resident slot once");
        let mut seen = [[false; 32]; 3];
        for probe in &out {
            assert!(!seen[probe.cascade][probe.slot], "each slot at most once per frame");
            seen[probe.cascade][probe.slot] = true;
        }
    }
}

#[test]
fn gpu_window_keeps_delayed_samples_and_expires_old_peaks() {
    let mut times = StageTimes::default();
    assert_eq!(times.stats(), (0.0, 0.0, None));
    times.record(8.0, 8);
    for _ in 0..119 { times.record(1.0, 8); }
    assert_eq!(times.stats(), (127.0 / 120.0, 8.0, Some(127.0 / 960.0)), "peak remains for 120 measurements");
    times.record(1.0, 8);
    assert_eq!(times.stats(), (1.0, 1.0, Some(0.125)), "peak expires at the next completed frame");
    for ms in [2.0, 3.0, 4.0] { times.record(ms, 8); }
    assert_eq!(times.stats(), (126.0 / 120.0, 4.0, Some(126.0 / 960.0)), "every delayed completion contributes");
    times.record(f64::NAN, 8);
    assert_eq!(times.stats().1, 4.0, "invalid timers do not poison the window");

    let mut frames = FrameTimes::default();
    assert_eq!(frames.stats(), (0.0, 0.0, None));
    assert_eq!(frames.max600(), 0.0);
    // Three completed frames delivered together. The maximum of their sums
    // is 17, not the sum of the four independent pass maxima (28).
    assert!(frames.push([&[8.0, 1.0, 2.0], &[1.0, 8.0, 3.0], &[2.0, 3.0, 8.0], &[4.0, 4.0, 4.0]], 8));
    assert_eq!(frames.stats(), (16.0, 17.0, Some(2.0)), "sum each frame before averaging or taking its maximum");
    assert!(!frames.push([&[1.0], &[], &[1.0], &[1.0]], 8), "missing completions cannot be paired");
    assert!(!frames.push([&[f32::NAN], &[1.0], &[1.0], &[1.0]], 8));
    assert!(!frames.push([&[]; 4], 8));
    assert_eq!(frames.stats(), (16.0, 17.0, Some(2.0)), "missing timings are not zero-cost frames");
    for _ in 0..119 { frames.push([&[1.0]; 4], 8); }
    assert_eq!(frames.stats().1, 17.0, "sum peak lasts 120 completions");
    frames.push([&[1.0]; 4], 8);
    assert_eq!(frames.stats(), (4.0, 4.0, Some(0.5)), "sum peak expires from the short window");
    for _ in 0..479 { frames.push([&[1.0]; 4], 8); }
    assert_eq!(frames.max600(), 17.0, "long peak survives without diagnostic logging");
    frames.push([&[1.0]; 4], 8);
    assert_eq!(frames.max600(), 4.0, "long peak expires after 600 completions");
}

#[test]
fn batch_controller_converges_slowly_and_never_accumulates_skipped_work() {
    let mut control = BatchController::new(64, 2.0);
    let mut times = FrameTimes::default();
    let aim = BatchController::aim(2.0);
    assert_eq!(aim, 1.6, "reserve 20% of GI's own share");
    let mut history = [64; 121];
    let mut now = 0.0;
    let mut batch = 64;
    let capacity = 256;
    for frame in 0..7200 {
        // Under aim, then cheaper, then over aim: both directions must
        // converge using whole-frame costs, including a larger gather pass.
        let target = if frame < 2400 { 128 } else if frame < 4800 { 192 } else { 160 };
        let ms = (batch as f64 * aim / target as f64 / 8.0) as f32;
        times.push([&[ms], &[ms * 2.0], &[ms * 4.0], &[ms]], batch);
        let cost = times.stats().2.unwrap();
        let previous = batch;
        now += 1.0 / 120.0;
        control.advance(now, Some(cost), true);
        batch = control.cap(2.0, capacity);
        if previous as f64 * cost > aim {
            assert!(batch <= previous, "an over-aim sum cannot grow the cap at {frame}");
        }
        assert!(batch <= capacity, "never exceed the unique weighted probe capacity");
        if frame >= 120 {
            let second_ago = history[(frame - 120) % history.len()];
            // Rounding a continuous cap to whole probes costs at most one.
            assert!((batch as f64) <= second_ago as f64 * 1.1 + 1.0, "growth exceeds 10%/second at {frame}");
            assert!((batch as f64) >= second_ago as f64 / 1.1 - 1.0, "shrink exceeds 10%/second at {frame}");
        }
        history[frame % 121] = batch;
        if frame % 2400 == 2399 {
            assert!(batch.abs_diff(target) <= 1, "converges to aim/cost: {batch} vs {target}");
            assert!(batch as f64 * aim / target as f64 <= aim + 1e-6, "does not settle above the aim");
        }
    }
    let before = batch;
    now += 1.0 / 120.0;
    control.advance(now, None, true);
    assert_eq!(control.cap(2.0, capacity), before, "missing timings cannot grow the cap");
    now += 2.0 / 120.0;
    control.advance(now, Some(0.001), true);
    assert_eq!(control.cap(2.0, capacity), before, "a skipped frame cannot make the next batch larger");
    control.advance(now + 60.0, Some(0.001), false);
    control.advance(now + 120.0, Some(0.001), true);
    assert_eq!(control.cap(2.0, capacity), before, "an idle interval earns no work credit");
    let half_budget = control.cap(1.0, capacity);
    assert!(half_budget * 2 <= before, "cap scales with gpu_budget_ms");
    assert!(before.abs_diff(half_budget * 2) <= 1, "budget scaling loses only integer rounding");
    let mut probes = [game_cascade([8, 4, 8], 0)];
    probes[0].scroll([0; 3]);
    let mut out = Vec::with_capacity(capacity);
    schedule(&mut probes, half_budget, true, &mut out);
    assert_eq!(out.len(), half_budget, "warmup obeys the budget-derived cap too");
}
