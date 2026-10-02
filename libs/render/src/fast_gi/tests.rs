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
    let triangles = instances.iter().map(|i| i.mesh.indices.len() / 3).sum();
    Arc::new(Snapshot { instances, triangles })
}
fn small_layout() -> VoxelLayout { VoxelLayout::new(GiConfig { grid: [4, 2, 4], cascades: 2, ..GiConfig::default() }) }

#[test]
fn gi_shaders_compile_without_errors() {
    use makepad_draw::makepad_platform::makepad_script::script_eval;
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        vm.bx.captured_errors = Some(Vec::new());
        makepad_draw::script_mod(vm);
        vm.bx.heap.new_module(id!(prelude));
        script_eval!(vm, {
            mod.prelude.widgets_internal = {..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw:mod.draw}
        });
        vm.bx.heap.new_module(id!(widgets));
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
        ] {
            let id = vars.draw_shader_id.expect("GI shader registered");
            assert_eq!(format!("{:?}", vm.cx().draw_shaders[id.index].mapping.color_format), format);
            assert!(!vars.options.alpha_blend, "GI payloads must overwrite, not blend");
        }
        for (name, result) in [
            ("trace", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiTrace)})),
            ("relight", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiRelight)})),
            ("gather", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiGather)})),
            ("scatter", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiScatter)})),
            ("cube receiver", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneCube)})),
            ("model receiver", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneSkinned)})),
        ] {
            let errors = vm.bx.heap.string_with(result, |_h, v| v.to_string()).expect("compile result string");
            assert!(errors.is_empty(), "{name}: {errors}");
        }
    });
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
    let mut c = Cascade::new([4, 2, 4], 1.0);
    assert_eq!(c.scroll([0, 0, 0]).len(), 32, "a first window traces everything");
    for s in 0..c.count() { assert_eq!(c.slot(c.cell(s).unwrap()), s); }
    let mut out = schedule(std::slice::from_mut(&mut c), 1000);
    assert_eq!(out.len(), 32, "each probe at most once per frame");
    assert!(out.iter().all(|b| b.hysteresis == 0.0));
    assert_eq!(c.scroll([1, 0, 0]).len(), 8, "one x slab of 2x4 probes");
    out = schedule(std::slice::from_mut(&mut c), 8);
    assert!(out.iter().all(|b| b.cell[0] == 4), "the new slab is traced first: {out:?}");
    assert_eq!(c.pending(), 0);
    assert_eq!(c.scroll([100, 0, 0]).len(), 32, "a teleport re-targets every slot");
}

#[test]
fn edits_queue_nearby_probes_ahead_of_refresh_with_low_hysteresis() {
    let mut c = Cascade::new([8, 4, 8], 1.0);
    c.scroll([0, 0, 0]);
    let _ = schedule(std::slice::from_mut(&mut c), 10_000);
    let n = c.invalidate([3.0, 1.0, 3.0], [3.5, 1.5, 3.5], [0.43, 0.47, 0.41]);
    assert!(n > 0 && n < 64, "{n} probes near a 0.5 m edit");
    let out = schedule(std::slice::from_mut(&mut c), n);
    assert_eq!(out.len(), n);
    assert!(out.iter().all(|b| b.hysteresis == DIRTY_HYSTERESIS));
    let rest = schedule(std::slice::from_mut(&mut c), 5);
    assert!(rest.iter().all(|b| b.hysteresis == REFRESH_HYSTERESIS));
}

#[test]
fn refresh_visits_every_probe_once_per_cycle_and_splits_the_budget() {
    let mut cs = vec![Cascade::new([4, 2, 4], 1.0), Cascade::new([4, 2, 4], 2.0), Cascade::new([4, 2, 4], 4.0)];
    for c in &mut cs { c.scroll([0, 0, 0]); }
    let _ = schedule(&mut cs, 10_000);
    let mut seen = vec![0; 32];
    for _ in 0..4 {
        let out = schedule(&mut cs, 16);
        assert_eq!(out.len(), 16);
        assert_eq!(out.iter().filter(|b| b.cascade == 0).count(), 8, "half the budget goes to the fine cascade");
        let mut slots: Vec<_> = out.iter().map(|b| (b.cascade, b.slot)).collect(); slots.sort(); slots.dedup();
        assert_eq!(slots.len(), 16);
        for b in out.iter().filter(|b| b.cascade == 0) { seen[b.slot] += 1; }
    }
    assert!(seen.iter().all(|&n| n == 1), "{seen:?}");
    // A cascade with queued work gets the others' unused share first.
    cs[2].scroll([50, 0, 0]);
    let out = schedule(&mut cs, 16);
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
    lamp = Instance::new(lamp.mesh.clone(), lamp.transform, lamp.tint, vec3f(3.0, 1.5, 0.0), 1.0).unwrap();
    let mut w = VoxelWorld::new(l);
    w.update(Some(snapshot(vec![lamp])), &[l.window_for(0, Vec3f::default()), l.window_for(1, Vec3f::default())]);
    let px = w.emission[l.texel(0, [2, 1, 0])];
    assert_eq!(((px >> 16) & 255, (px >> 8) & 255, px & 255), (255, 128, 0), "hue relative to the strongest channel");
    let a = (px >> 24) as f32 / 255.0;
    assert!((a / (1.0 - a) - 3.0).abs() < 0.2, "intensity m/(1+m) decodes back to 3");
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

