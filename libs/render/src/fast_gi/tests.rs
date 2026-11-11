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
    let triangles = instances.iter().map(|i| i.triangles()).sum();
    Arc::new(Snapshot { instances, triangles })
}
fn small_layout() -> VoxelLayout { VoxelLayout::new(GiConfig { grid: [4, 2, 4], cascades: 2, ..GiConfig::default() }) }

// CPU mirror of GiVoxels.vox_end_status. Distance-limit and window-exit
// endpoints have the same rule; neither termination alone proves sky.
fn trace_end_status(end: Vec3f, (lo, hi): (Vec3f, Vec3f)) -> f32 {
    if end.x < lo.x || end.y < lo.y || end.z < lo.z || end.x > hi.x || end.y > hi.y || end.z > hi.z {
        0.0
    } else {
        0.25
    }
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
    gi.submit_scene([0; 5], Vec3f::default(), Snapshot { instances, triangles: 24 });
    let (_, padded) = gi.pending_scene.as_ref().unwrap();
    assert_eq!(*padded, (vec3f(-5.0, -2.0, -2.0), vec3f(5.0, 4.0, 2.0)), "union world bounds and pad by the coarsest voxel");
    assert_eq!(gi.scene_bounds, old_bounds, "a pending scene must not replace the committed voxel bounds");
    assert_eq!(trace_end_status(vec3f(4.5, 2.0, 0.0), *padded), 0.25, "the padding is still inside");
    let empty = Snapshot { instances: vec![], triangles: 0 }.padded_bounds(1.0);
    assert_eq!(trace_end_status(Vec3f::default(), empty), 0.0, "an empty scene is open");
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
    let invalid = "ifhit.w<0.0||hit.w==0.25";
    assert_eq!(body(trace, invalid), "returnvec4(0.0,-2.0,0.0,0.0)", "ran out packs as invalid");
    assert!(trace.find(invalid).unwrap() < trace.find("ifhit.w<0.5").unwrap(), "ran out must be handled before sky");
    let relight = body(&source, "mod.draw.DrawGiRelight=");
    let invalid = "ifprobe.w<0.0||hit.y<(-1.5)";
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
fn scatter_shader_places_atlas_tiles_without_widget_clipping() {
    let source: String = include_str!("shaders.rs").chars().filter(|c| !c.is_whitespace()).collect();
    let scatter = source.split_once("mod.draw.DrawGiScatter=").unwrap().1
        .split_once("mod.draw.FastGiScreen=").unwrap().0;
    assert!(!scatter.contains("clip_and_transform_vertex("), "retained tiles and unaligned clears have no turtle clip");
    for expression in [
        "let a=self.gi_batch.sample_nearest(vec2(0.25,(self.gi_tile.x+0.5)/self.gi_update_size.y),0.0)",
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
        "self.gi_update.sample_nearest(vec2((col+0.5)/self.gi_update_size.x,(self.gi_tile.x+0.5)/self.gi_update_size.y))",
    ] {
        let compact: String = expression.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(scatter.contains(&compact), "scatter CPU mirror and unclipped pass projection: {expression}");
    }
}

// Check the production row encoder and retained instance layout together.
// The shader expressions mirrored here are pinned by the test above.
fn check_scatter_batch_destinations(cx: &Cx, scatter: &DrawGiScatter, cache: &ScatterCache) {
    let shader = scatter.quad.draw_vars.draw_shader_id.unwrap();
    let mapping = &cx.draw_shaders[shader.index].mapping;
    let offset = |id| mapping.instances.inputs.iter().find(|p| p.id == id).unwrap().offset;
    let tile_offset = offset(live_id!(gi_tile));
    let size_offset = offset(live_id!(rect_size));
    let retained = cache.instances.as_ref().unwrap();
    let capacity = retained.count() / 3;
    for rows in [capacity, 4, 1, 0, capacity] {
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
        let mut texture = vec![f32::NAN; capacity * 8];
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

            // Normalized nearest lookup: column zero of two, row / capacity
            // (not / batch length). Sample at texel centres, explicitly LOD 0.
            let uv = [0.25_f32, (tile[0] + 0.5) / capacity as f32];
            let texel = (uv[1] * capacity as f32).floor() as usize * 2 + (uv[0] * 2.0).floor() as usize;
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
fn gi_shaders_compile_without_errors() {
    use makepad_draw::makepad_platform::makepad_script::script_eval;
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        vm.capture_errors();
        makepad_draw::script_mod(vm);
        vm.heap_mut().new_module(id!(prelude));
        script_eval!(vm, {
            mod.prelude.widgets_internal = {..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw:mod.draw}
        });
        vm.heap_mut().new_module(id!(widgets));
        makepad_render_graph::pass_stdlib(vm);
        crate::local_shadows::sampling::script_mod(vm);
        crate::clustered::script_mod(vm);
        crate::fast_gi::script_mod(vm);
        crate::shaders::script_mod(vm);
        crate::local_shadows::script_mod(vm);
        let setup = vm.take_errors();
        assert!(setup.is_empty(), "script setup errors: {setup:#?}");
        // Hits and radiance carry packed integers and distances (f32); the
        // gathered tiles and the field are filterable f16.
        for (vars, format) in [
            (DrawGiTrace::script_new_with_default(vm).quad.draw_vars, "Rgba32F"),
            (DrawGiRelight::script_new_with_default(vm).quad.draw_vars, "Rgba32F"),
            (DrawGiGather::script_new_with_default(vm).quad.draw_vars, "Rgba16F"),
            (DrawGiScatter::script_new_with_default(vm).quad.draw_vars, "Rgba16F"),
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
        let bytes = cache.prepare(&mut scatter, 16);
        assert!(bytes > 0, "first encoding publishes scatter geometry");
        let retained = cache.instances.as_ref().unwrap().clone();
        assert_eq!(retained.byte_len(), bytes);
        assert_eq!(retained.count(), 16 * 3, "three regions per batch row");
        check_scatter_batch_destinations(vm.cx(), &scatter, &cache);
        assert_eq!(cache.prepare(&mut scatter, 16), 0, "unchanged frame encodes zero static bytes");
        assert_eq!(cache.instances.as_ref().unwrap().id(), retained.id(), "same resource avoids an upload");
        assert!(cache.instances.as_ref().unwrap().upload_since(retained.id()).is_empty(), "no unchanged instance bytes uploaded");
        assert!(cache.prepare(&mut scatter, 32) > 0, "a changed capacity rebuilds the cache");
        for (name, result) in [
            ("trace", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiTrace)})),
            ("relight", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiRelight)})),
            ("gather", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiGather)})),
            ("scatter", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiScatter)})),
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
    let l = small_layout();
    let a = unit_box(vec3f(1.0, 0.0, 1.0), vec3f(1.0, 2.0, 1.0));
    let b = unit_box(vec3f(-3.0, 0.5, 2.0), vec3f(0.3, 1.0, 3.0));
    let floor = unit_box(vec3f(0.0, -1.0, 0.0), vec3f(30.0, 0.2, 30.0));
    let moved = unit_box(vec3f(-1.0, 0.5, -2.0), vec3f(0.3, 1.0, 3.0));
    let w0 = [l.window_for(0, Vec3f::default()), l.window_for(1, Vec3f::default())];
    let far = vec3f(9.0, 1.0, -5.0);
    let w1 = [l.window_for(0, far), l.window_for(1, far)];
    let mut inc = VoxelWorld::new(l);
    let first = inc.update(Some(snapshot(vec![floor.clone(), a.clone(), b.clone()])), &w0);
    assert!(first.dirty.is_empty(), "the first build is not an edit");
    // Edit: b moves. Only its old and new boxes are dirty, a is untouched
    // even though the instance order changed.
    let edit = inc.update(Some(snapshot(vec![moved.clone(), a.clone(), floor.clone()])), &w0);
    assert_eq!(edit.dirty.len(), 2);
    assert!(edit.bricks < 2 * 16, "edit re-voxelized {} bricks", edit.bricks);
    let same = inc.update(Some(snapshot(vec![floor.clone(), moved.clone(), a.clone()])), &w0);
    assert_eq!((same.dirty.len(), same.bricks), (0, 0), "an identical snapshot does nothing");
    assert!(same.rows.iter().all(Option::is_none), "identical static geometry uploads zero texture rows");
    let scroll = inc.update(None, &w1);
    let mut full = VoxelWorld::new(l);
    full.update(Some(snapshot(vec![floor, a, moved])), &w1);
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

#[test]
fn slots_round_trip_and_scrolling_queues_only_the_exposed_slab() {
    let mut c = Cascade::new([4, 2, 4], 1.0, false);
    assert_eq!(c.scroll([0, 0, 0]).len(), 32, "a first window traces everything");
    for s in 0..c.count() { assert_eq!(c.slot(c.cell(s).unwrap()), s); }
    let mut out = Vec::with_capacity(1000);
    schedule(std::slice::from_mut(&mut c), 1000, &mut out);
    assert_eq!(out.len(), 32, "each probe at most once per frame");
    assert!(out.iter().all(|b| b.hysteresis == 0.0));
    assert_eq!(c.scroll([1, 0, 0]).len(), 8, "one x slab of 2x4 probes");
    schedule(std::slice::from_mut(&mut c), 8, &mut out);
    assert!(out.iter().all(|b| b.cell[0] == 4), "the new slab is traced first: {out:?}");
    assert_eq!(c.pending(), 0);
    assert_eq!(c.scroll([100, 0, 0]).len(), 32, "a teleport re-targets every slot");
}

#[test]
fn edits_queue_nearby_probes_ahead_of_refresh_with_low_hysteresis() {
    let mut c = Cascade::new([8, 4, 8], 1.0, false);
    c.scroll([0, 0, 0]);
    let mut out = Vec::with_capacity(10_000);
    schedule(std::slice::from_mut(&mut c), 10_000, &mut out);
    let n = c.invalidate([3.0, 1.0, 3.0], [3.5, 1.5, 3.5], [0.43, 0.47, 0.41]);
    assert!(n > 0 && n < 64, "{n} probes near a 0.5 m edit");
    schedule(std::slice::from_mut(&mut c), n, &mut out);
    assert_eq!(out.len(), n);
    assert!(out.iter().all(|b| b.hysteresis == DIRTY_HYSTERESIS));
    schedule(std::slice::from_mut(&mut c), 5, &mut out);
    assert!(out.iter().all(|b| b.hysteresis == REFRESH_HYSTERESIS));
}

#[test]
fn refresh_visits_every_probe_once_per_cycle_and_splits_the_budget() {
    let mut cs = vec![Cascade::new([4, 2, 4], 1.0, false), Cascade::new([4, 2, 4], 2.0, false), Cascade::new([4, 2, 4], 4.0, false)];
    for c in &mut cs { c.scroll([0, 0, 0]); }
    let mut out = Vec::with_capacity(10_000);
    schedule(&mut cs, 10_000, &mut out);
    let mut seen = vec![0; 32];
    for _ in 0..4 {
        schedule(&mut cs, 16, &mut out);
        assert_eq!(out.len(), 16);
        assert_eq!(out.iter().filter(|b| b.cascade == 0).count(), 8, "half the budget goes to the fine cascade");
        let mut slots: Vec<_> = out.iter().map(|b| (b.cascade, b.slot)).collect(); slots.sort(); slots.dedup();
        assert_eq!(slots.len(), 16);
        for b in out.iter().filter(|b| b.cascade == 0) { seen[b.slot] += 1; }
    }
    assert!(seen.iter().all(|&n| n == 1), "{seen:?}");
    // A cascade with queued work gets the others' unused share first.
    cs[2].scroll([50, 0, 0]);
    schedule(&mut cs, 16, &mut out);
    assert_eq!(out.iter().filter(|b| b.cascade == 2).count(), 16);
}

#[test]
fn off_allocates_nothing_and_mode_changes_discard_the_field() {
    let mut gi = FastGi::default();
    assert!(gi.gpu.is_none());
    assert!(!gi.wants_scene([0; 5], Vec3f::default()), "off never asks for a snapshot");
    gi.set_mode(GiMode::Fast);
    assert!(gi.wants_scene([0; 5], Vec3f::default()));
    gi.submit_scene([0; 5], Vec3f::default(), Snapshot { instances: vec![], triangles: 0 });
    assert!(!gi.wants_scene([1; 5], Vec3f::default()), "one snapshot in flight at a time");
    gi.set_config(GiConfig { strength: 0.5, feedback: 0.2, ..gi.config() });
    assert!(gi.pending_scene.is_some(), "lighting-only controls keep the scene");
    gi.set_config(GiConfig { spacing: 2.0, ..gi.config() });
    assert!(gi.pending_scene.is_none() && gi.scene_key.is_none());
    gi.set_mode(GiMode::Off);
    assert!(gi.gpu.is_none() && gi.voxels.is_none());
}

#[test]
fn a_realm_change_keeps_the_field_and_rediffs_the_scene() {
    let mut gi = FastGi::default();
    gi.set_mode(GiMode::Fast);
    gi.voxels = Some(Box::new(VoxelWorld::new(small_layout())));
    gi.submit_scene([7; 5], Vec3f::default(), Snapshot { instances: vec![], triangles: 0 });
    let epoch = gi.meshes.epoch;
    gi.reset();
    assert!(gi.voxels.is_some(), "hot reload keeps the voxels: the next snapshot is a diff");
    assert!(gi.pending_scene.is_none(), "the departing world's snapshot is not sent");
    assert!(gi.wants_scene([7; 5], Vec3f::default()), "even an equal key re-snapshots the new world");
    assert_ne!(gi.meshes.epoch, epoch, "restarted revision counters cannot alias cached tiles");
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
fn voxel_albedo_weights_surface_area_and_dark_surfaces_do_not_dilute_emission() {
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
}

#[test]
fn camera_drift_asks_for_a_new_snapshot_before_the_window_leaves_the_region() {
    let mut gi = FastGi::default();
    gi.set_mode(GiMode::Fast);
    gi.submit_scene([0; 5], Vec3f::default(), Snapshot { instances: vec![], triangles: 0 });
    gi.pending_scene = None;
    assert!(!gi.wants_scene([0; 5], vec3f(20.0, 0.0, 0.0)));
    assert!(gi.wants_scene([0; 5], vec3f(30.0, 0.0, 0.0)));
    let (lo, hi) = gi.region(Vec3f::default());
    let e = gi.coarsest_extent();
    // Worst case: drift 0.4 extent + half a window + a brick of hysteresis.
    let reach = e.x * 0.4 + e.x * 0.5 + 1.5 * 8.0 * 1.0;
    assert!(hi.x >= reach && lo.x <= -reach);
}

#[test]
fn batches_stay_full_through_warmup_edits_and_sweeps_without_reallocating() {
    let grid = [4, 2, 4];
    let mut cascades: Vec<_> = (0..3).map(|i| Cascade::new(grid, (1 << i) as f32, false)).collect();
    for c in &mut cascades { c.scroll([0; 3]); }
    let capacity = batch_capacity(32, 3);
    let mut out = Vec::with_capacity(capacity);
    let allocation = out.as_ptr();
    for frame in 0..600 {
        if frame == 200 { cascades[0].invalidate([0.0; 3], [1.0; 3], HOME); }
        if frame == 350 { cascades[0].scroll([20, 0, 0]); }
        if frame == 400 { for c in &mut cascades { c.scroll([20, 0, 0]); } }
        // Includes awkward rounding and the largest legal weighted batch.
        let budget = if frame < 300 { 37 } else if frame < 500 { capacity } else { capacity - 1 };
        schedule(&mut cascades, budget, &mut out);
        assert_eq!(out.len(), budget, "frame {frame}: no short frame or catch-up batch");
        assert_eq!(out.as_ptr(), allocation, "reuse frame storage");
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
    let mut probes = [Cascade::new([8, 4, 8], 1.0, false)];
    probes[0].scroll([0; 3]);
    let mut out = Vec::with_capacity(capacity);
    schedule(&mut probes, half_budget, &mut out);
    assert_eq!(out.len(), half_budget, "warmup obeys the budget-derived cap too");
}
