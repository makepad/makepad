use makepad_code_atlas::{CodeGeometry, LineGeom, Miniature, OccurrenceId};
use makepad_studio::atlas::splat::*;
use makepad_studio::makepad_widgets::*;
use std::sync::Arc;

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        pos: dvec2(x, y),
        size: dvec2(w, h),
    }
}

fn geometry(lines: usize) -> CodeGeometry {
    CodeGeometry {
        lines: vec![
            LineGeom {
                indent_cols: 4,
                extent_cols: 24,
                density: 10,
                mix: [51, 51, 102, 51],
                ..Default::default()
            };
            lines
        ]
        .into(),
        folds: Arc::from([]),
        chunks: Arc::from([]),
        max_extent: 24,
        ink_columns: lines as u64 * 10,
        lossy: false,
    }
}

fn batch(lines: usize) -> SplatBatch {
    SplatBatch::build(
        &geometry(lines),
        rect(0.0, 0.0, 200.0, lines as f64 * 2.0),
        2.0,
        Vec4f::default(),
    )
}

fn key(id: u8, band: CodeBand) -> SplatKey {
    SplatKey {
        occurrence: OccurrenceId([id; 20]),
        kind: SplatKind::Full,
        band,
    }
}

fn near(a: f32, b: f32) {
    assert!((a - b).abs() < 0.00001, "{a} != {b}");
}

#[test]
fn threshold_arithmetic_uses_world_pitch_and_live_entry_at_twelve() {
    for (i, band) in CodeBand::ALL[..3].iter().copied().enumerate() {
        let next = CodeBand::ALL[i + 1];
        assert_eq!(band.at_pitch(CodeBand::ENTER[i] - 0.0001), band);
        assert_eq!(band.at_pitch(CodeBand::ENTER[i]), next);
        assert_eq!(next.at_pitch(CodeBand::EXIT[i]), next);
        assert_eq!(next.at_pitch(CodeBand::EXIT[i] - 0.0001), band);
        let mut state = BandState::with_band(0.25, band);
        assert_eq!(
            state.update(CodeBand::ENTER[i] / 0.25, 0.0),
            BandChange::Crossed {
                from: band,
                to: next
            }
        );
        assert_eq!(state.projected_pitch(48.0), 12.0);
    }
    assert_eq!(CodeBand::Density.at_pitch(20.0), CodeBand::Editor);
    assert_eq!(CodeBand::Editor.at_pitch(0.1), CodeBand::Density);
}

#[test]
fn oscillation_around_nominal_thresholds_keeps_hysteresis() {
    for (low, high, nominal, entry) in [
        (CodeBand::Density, CodeBand::Splat, 1.5, 1.65),
        (CodeBand::Splat, CodeBand::Structure, 6.0, 6.6),
        (CodeBand::Structure, CodeBand::Editor, 12.0, 12.0),
    ] {
        let mut state = BandState::with_band(1.0, low);
        let mut crossings = 0;
        for step in 0..200 {
            let p = nominal * if step % 2 == 0 { 0.99 } else { 1.01 };
            if matches!(
                state.update(p, step as f64 * 10.0),
                BandChange::Crossed { .. }
            ) {
                crossings += 1;
            }
        }
        assert_eq!(crossings, if nominal == 12.0 { 1 } else { 0 });
        state.update(entry, 2_000.0);
        state.update(entry, 2_120.0);
        assert!(state.is_settled());
        for step in 0..200 {
            let p = nominal * if step % 2 == 0 { 0.99 } else { 1.01 };
            assert_eq!(
                state.update(p, 2_130.0 + step as f64 * 10.0),
                BandChange::None
            );
            assert_eq!(state.band(), high);
        }
    }
}

#[test]
fn interrupted_fade_reverses_from_current_weight_and_stops() {
    let mut state = BandState::new(1.0);
    state.update(2.0, 0.0);
    state.update(2.0, 30.0);
    near(state.weights()[CodeBand::Splat as usize], 0.15625);
    let before = state.weights();
    assert_eq!(
        state.update(1.0, 30.0),
        BandChange::Crossed {
            from: CodeBand::Splat,
            to: CodeBand::Density
        }
    );
    assert_eq!(state.weights(), before);
    state.update(1.0, 90.0);
    near(state.weights()[CodeBand::Density as usize], 0.921875);
    assert!(!state.is_settled());
    assert!(matches!(
        state.update(1.0, 150.0),
        BandChange::Fading { weight: 1.0, .. }
    ));
    assert!(state.is_settled());
    assert_eq!(state.update(1.0, 1_000.0), BandChange::None);
    let forward = BandTransition::new(CodeBand::Density, CodeBand::Splat, 0.0);
    assert_eq!(forward.reverse(30.0).weights(30.0), forward.weights(30.0));
}

#[test]
fn third_band_interrupt_preserves_all_visible_endpoints() {
    let mut state = BandState::new(1.0);
    state.update(2.0, 0.0);
    state.update(2.0, 60.0);
    let before = state.weights();
    state.update(8.0, 60.0);
    assert_eq!(state.weights(), before);
    assert_eq!(
        state.resident_bands().collect::<Vec<_>>(),
        vec![CodeBand::Density, CodeBand::Splat, CodeBand::Structure]
    );
    state.update(8.0, 120.0);
    assert_eq!(state.weights(), [0.25, 0.25, 0.5, 0.0]);
    state.update(8.0, 180.0);
    assert!(state.is_settled());
}

#[test]
fn nonfinite_input_and_reversed_clock_cannot_poison_state() {
    let mut state = BandState::new(1.0);
    state.update(2.0, 100.0);
    state.update(2.0, 160.0);
    let weights = state.weights();
    state.update(2.0, 110.0);
    assert_eq!(state.weights(), weights);
    for (z, t) in [(f64::NAN, 180.0), (2.0, f64::INFINITY), (-1.0, 180.0)] {
        assert_eq!(state.update(z, t), BandChange::None);
        assert_eq!(state.weights(), weights);
    }
    state.update(2.0, 220.0);
    assert!(state.is_settled());
}

#[test]
fn cull_lines_forty_through_sixty_without_rebuilding() {
    let b = SplatBatch::build(
        &geometry(100),
        rect(10.0, 20.0, 200.0, 200.0),
        2.0,
        Vec4f::default(),
    );
    let ptr = b.instances().as_ptr();
    assert_eq!(b.cull(rect(10.0, 100.0, 100.0, 42.0)), 40..61);
    assert_eq!(b.cull(rect(10.0, 100.0, 100.0, 40.0)), 40..60);
    assert_eq!(b.cull(rect(10.0, 100.5, 100.0, 39.0)), 40..60);
    assert_eq!(b.cull(rect(211.0, 100.0, 100.0, 40.0)), 0..0);
    assert_eq!(b.cull(rect(10.0, -100.0, 100.0, 1.0)), 0..0);
    assert_eq!(b.cull(rect(10.0, 220.0, 100.0, 1.0)), 100..100);
    assert_eq!(b.instances().as_ptr(), ptr);
}

#[test]
fn world_columns_do_not_stretch_and_blank_lines_keep_space() {
    let mut g = geometry(3);
    Arc::make_mut(&mut g.lines)[1] = LineGeom {
        flags: makepad_code_atlas::code_view::LINE_BLANK,
        ..Default::default()
    };
    let a = SplatBatch::build(&g, rect(0.0, 0.0, 200.0, 6.0), 2.0, Vec4f::default());
    let b = SplatBatch::build(&g, rect(0.0, 0.0, 400.0, 6.0), 2.0, Vec4f::default());
    assert_eq!(a.instances()[0].indent, 5.0);
    assert_eq!(a.instances()[0].extent, 25.0);
    assert_eq!(a.instances()[0].extent, b.instances()[0].extent);
    assert_eq!(a.instances()[1].y, 2.0);
    assert_eq!(a.instances()[2].y, 4.0);
    assert_eq!(a.instances()[1].density, 0.0);
    near(a.instances()[0].density, 0.5);
    near(a.instances()[0].class_mix.z, 0.4);
}

#[test]
fn miniature_bins_preserve_source_height_and_partial_tail() {
    let m = Miniature::of(&geometry(1_001));
    let b = SplatBatch::build(&m, rect(0.0, 0.0, 200.0, 2_002.0), 2.0, Vec4f::default());
    assert_eq!(b.kind(), SplatKind::Miniature);
    assert_eq!(b.instances().len(), 251);
    assert_eq!(b.instances()[0].h, 8.0);
    assert_eq!(b.instances()[250].y, 2_000.0);
    assert_eq!(b.instances()[250].h, 2.0);
    assert_eq!(b.cull(rect(0.0, 80.0, 20.0, 42.0)), 10..16);
    assert_eq!(b.gpu_bytes(), 251 * SPLAT_INSTANCE_BYTES);
}

#[test]
fn miniature_explicit_spans_and_saturated_bins_keep_world_coordinates() {
    let bin = makepad_code_atlas::MiniBin {
        lines: 100,
        extent: 20,
        density: 10,
        ..Default::default()
    };
    let mut miniature = Miniature {
        lines: 1_000,
        max_extent: 20,
        ink_columns: 10_000,
        mix: [0; 4],
        bins: vec![bin; 10].into(),
    };
    let b = SplatBatch::build(
        &miniature,
        rect(0.0, 0.0, 100.0, 1_000.0),
        1.0,
        Vec4f::default(),
    );
    assert_eq!(b.instances()[9].y, 900.0);
    assert_eq!(b.instances()[9].h, 100.0);
    miniature.lines = 20_000_000;
    miniature.bins = vec![
        makepad_code_atlas::MiniBin {
            lines: u16::MAX,
            ..bin
        };
        256
    ]
    .into();
    let b = SplatBatch::build(
        &miniature,
        rect(0.0, 0.0, 100.0, 20_000_000.0),
        1.0,
        Vec4f::default(),
    );
    let last = b.instances().last().unwrap();
    assert_eq!(last.y + last.h, 20_000_000.0);
    assert_eq!(last.h, 78_125.0);
}

#[test]
fn a_cropped_last_line_keeps_its_baseline() {
    let b = SplatBatch::build(
        &geometry(100),
        rect(0.0, 0.0, 100.0, 81.0),
        2.0,
        Vec4f::default(),
    );
    assert_eq!(b.instances().len(), 41);
    assert_eq!(b.instances()[40].y, 80.0);
    assert_eq!(b.instances()[40].h, 2.0);
    assert_eq!(b.cull(rect(0.0, 81.0, 100.0, 1.0)), 41..41);
}

#[test]
fn byte_accounting_and_lru_evict_by_bytes_and_recency() {
    let mut store = SplatStore::with_capacity(60 * SPLAT_INSTANCE_BYTES);
    let a = key(1, CodeBand::Splat);
    let b = key(2, CodeBand::Splat);
    let c = key(3, CodeBand::Splat);
    assert_eq!(store.insert(a, batch(20)), SplatAdmission::Inserted);
    assert_eq!(store.insert(b, batch(30)), SplatAdmission::Inserted);
    assert_eq!(store.retained_bytes(), 50 * SPLAT_INSTANCE_BYTES);
    store.touch(a);
    assert_eq!(store.insert(c, batch(30)), SplatAdmission::Inserted);
    assert!(store.contains(a));
    assert!(!store.contains(b));
    assert!(store.contains(c));
    assert_eq!(store.retained_bytes(), 50 * SPLAT_INSTANCE_BYTES);
    assert_eq!(store.insert(b, batch(61)), SplatAdmission::TooLarge);
    assert_eq!(
        SplatStore::with_capacity(usize::MAX).capacity(),
        16 * 1024 * 1024
    );
}

#[test]
fn pinned_crossfade_endpoints_defer_admission_until_settled() {
    let mut store = SplatStore::with_capacity(40 * SPLAT_INSTANCE_BYTES);
    let a = key(1, CodeBand::Density);
    let b = key(1, CodeBand::Splat);
    store.insert(a, batch(20));
    let mut state = BandState::new(1.0);
    let change = state.update(2.0, 0.0);
    assert_eq!(
        store.rebuild_crossed(a.occurrence, SplatKind::Full, change, || batch(20)),
        SplatAdmission::Inserted
    );
    store.pin_surface(a.occurrence, &state);
    let c = key(2, CodeBand::Splat);
    assert_eq!(store.insert(c, batch(10)), SplatAdmission::Deferred);
    assert!(store.contains(a) && store.contains(b));
    let reversed = state.update(1.0, 60.0);
    assert_eq!(
        store.rebuild_crossed(a.occurrence, SplatKind::Full, reversed, || panic!(
            "resident endpoint rebuilt"
        )),
        SplatAdmission::Resident
    );
    state.update(1.0, 180.0);
    store.pin_surface(a.occurrence, &state);
    assert_eq!(store.insert(c, batch(10)), SplatAdmission::Inserted);
    assert!(store.contains(a));
    assert!(!store.contains(b));
}

#[test]
fn crossing_rebuilds_exactly_the_crossed_surface() {
    let mut store = SplatStore::new();
    let a = key(1, CodeBand::Density);
    let b = key(2, CodeBand::Density);
    store.insert(a, batch(20));
    store.insert(b, batch(20));
    let untouched = store.get(b).unwrap().instances().as_ptr();
    let mut sa = BandState::new(1.0);
    let mut sb = BandState::new(0.5);
    store.begin_frame();
    let ca = sa.update(1.7, 0.0);
    let cb = sb.update(1.7, 0.0);
    assert_eq!(
        store.rebuild_crossed(a.occurrence, SplatKind::Full, ca, || batch(20)),
        SplatAdmission::Inserted
    );
    assert_eq!(
        store.rebuild_crossed(b.occurrence, SplatKind::Full, cb, || panic!(
            "uncrossed surface rebuilt"
        )),
        SplatAdmission::Resident
    );
    assert_eq!(store.rebuilds_this_frame, 1);
    assert_eq!(store.get(b).unwrap().instances().as_ptr(), untouched);
    assert!(store.contains(key(1, CodeBand::Splat)));
    assert!(!store.contains(key(2, CodeBand::Splat)));
}

// Use real widget/theme registration and compile the actual splat shader.
fn shader_cx() -> Cx {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    makepad_widgets::makepad_platform::makepad_network::install_ui_waker(None);
    cx.with_vm(|vm| {
        makepad_widgets::makepad_platform::script::script_mod(vm);
        makepad_widgets::script_mod(vm);
        vm.bx.captured_errors = Some(Vec::new());
        makepad_studio::atlas::splat::script_mod(vm);
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "splat registration: {errors:?}");
    });
    cx
}

fn record(
    store: &mut SplatStore,
    cx: &mut Cx,
    pass: &DrawPass,
    root: &mut DrawList,
    keys: &[SplatKey],
    view: Rect,
) {
    let event = DrawEvent::default();
    let mut draw = CxDraw::new(cx, &event);
    draw.begin_pass(pass, None);
    root.begin_always(&mut draw);
    let mut draw = Cx2d::new(&mut draw);
    for key in keys {
        assert!(store.draw(&mut draw, *key, view));
    }
    root.end(&mut draw);
    draw.end_pass(pass);
}

#[test]
fn retained_shader_drag_200_steps_has_zero_uploads_or_rebuilds() {
    let mut cx = shader_cx();
    let pass = DrawPass::new(&mut cx);
    pass.set_size(&mut cx, dvec2(500.0, 500.0));
    let mut root = DrawList::new(&mut cx);
    let mut store = SplatStore::new();
    let k = key(1, CodeBand::Splat);
    store.begin_frame();
    store.insert(k, batch(100));
    record(
        &mut store,
        &mut cx,
        &pass,
        &mut root,
        &[k],
        rect(0.0, 0.0, 500.0, 500.0),
    );
    assert_eq!(store.uploads_this_frame, 1);
    assert_eq!(store.rebuilds_this_frame, 1);
    let child = root.debug_child_draw_list_ids(&cx)[0];
    assert_eq!(cx.draw_lists[child].draw_items.len(), 1);
    let item = &mut cx.draw_lists[child].draw_items[0];
    assert_eq!(
        item.instances.as_ref().unwrap().len() * 4,
        store.retained_bytes()
    );
    // A headless test does not submit a native GPU command buffer. Clear
    // the initial dirty bit exactly as the backend does after its upload,
    // then verify all actual retained camera/uniform paths leave it clear.
    item.kind.draw_call_mut().unwrap().instance_dirty = false;
    let ptr = item.instances.as_ref().unwrap().as_ptr();
    let mut band = BandState::with_band(2.0, CodeBand::Splat);
    for step in 0..200 {
        store.begin_camera_frame();
        let zoom = 1.0 + step as f64 / 200.0;
        let change = band.update(zoom, step as f64 * 10.0);
        assert_eq!(change, BandChange::None);
        store.rebuild_crossed(k.occurrence, k.kind, change, || {
            panic!("camera rebuilt geometry")
        });
        let mut camera = Mat4f::identity();
        camera.v[0] = zoom as f32;
        camera.v[5] = zoom as f32;
        camera.v[12] = step as f32;
        root.set_view_transform(&mut cx, &camera);
        let values = SplatUniforms {
            band_weight: 1.0,
            projected_pitch: (zoom * 2.0) as f32,
            heat: 0.0,
            heat_colour: Vec4f::default(),
        };
        store.set_uniforms(&mut cx, k, values);
        record(
            &mut store,
            &mut cx,
            &pass,
            &mut root,
            &[k],
            rect(0.0, step as f64 * 0.25, 500.0, 100.0),
        );
        let item = &cx.draw_lists[child].draw_items[0];
        assert!(!item.kind.draw_call().unwrap().instance_dirty);
        assert_eq!(item.instances.as_ref().unwrap().as_ptr(), ptr);
        assert_eq!(store.uploads_this_frame, 0);
        assert_eq!(store.rebuilds_this_frame, 0);
        cx.passes[pass.draw_pass_id()].paint_dirty = false;
        assert!(!store.set_uniforms(&mut cx, k, values));
        assert!(!cx.passes[pass.draw_pass_id()].paint_dirty);
    }
    assert!(band.is_settled());
    store.begin_camera_frame();
    let crossing = band.update(4.0, 2_000.0);
    assert!(matches!(
        crossing,
        BandChange::Crossed {
            to: CodeBand::Structure,
            ..
        }
    ));
    assert_eq!(
        store.rebuild_crossed(k.occurrence, k.kind, crossing, || batch(100)),
        SplatAdmission::Inserted
    );
    let incoming = key(1, CodeBand::Structure);
    record(
        &mut store,
        &mut cx,
        &pass,
        &mut root,
        &[k, incoming],
        rect(0.0, 0.0, 500.0, 500.0),
    );
    assert_eq!(store.rebuilds_this_frame, 1);
    assert_eq!(store.uploads_this_frame, 1);
    assert!(
        !cx.draw_lists[child].draw_items[0]
            .kind
            .draw_call()
            .unwrap()
            .instance_dirty
    );
    store.retire_all();
    assert_eq!(store.retained_bytes(), 200 * SPLAT_INSTANCE_BYTES);
    store.release_completed(&mut cx, 0);
    assert_eq!(store.retained_bytes(), 200 * SPLAT_INSTANCE_BYTES);
    // No actual GPU submissions exist in this headless context.
    store.release_completed(&mut cx, u64::MAX);
    assert_eq!(store.retained_bytes(), 0);
}

#[test]
fn recorded_lru_eviction_keeps_bytes_until_gpu_completion() {
    let mut cx = shader_cx();
    let pass = DrawPass::new(&mut cx);
    pass.set_size(&mut cx, dvec2(500.0, 500.0));
    let mut root = DrawList::new(&mut cx);
    let mut store = SplatStore::with_capacity(60 * SPLAT_INSTANCE_BYTES);
    let a = key(1, CodeBand::Splat);
    let b = key(2, CodeBand::Splat);
    let c = key(3, CodeBand::Splat);
    let frame = store.begin_frame();
    store.insert(a, batch(20));
    record(
        &mut store,
        &mut cx,
        &pass,
        &mut root,
        &[a],
        rect(0.0, 0.0, 500.0, 500.0),
    );
    store.insert(b, batch(30));
    // The older recorded entry wins eviction over the newer CPU reservation.
    assert_eq!(store.insert(c, batch(30)), SplatAdmission::Deferred);
    assert!(!store.contains(a));
    assert!(store.contains(b));
    assert!(!store.contains(c));
    assert_eq!(store.retained_bytes(), 50 * SPLAT_INSTANCE_BYTES);
    store.release_completed(&mut cx, frame - 1);
    assert_eq!(store.retained_bytes(), 50 * SPLAT_INSTANCE_BYTES);
    // This headless context has no native submissions pending.
    store.release_completed(&mut cx, frame);
    assert_eq!(store.retained_bytes(), 30 * SPLAT_INSTANCE_BYTES);
    assert_eq!(store.insert(c, batch(30)), SplatAdmission::Inserted);
    assert_eq!(store.retained_bytes(), 60 * SPLAT_INSTANCE_BYTES);
    store.retire_all();
    store.release_completed(&mut cx, frame);
    assert_eq!(store.retained_bytes(), 0);
}

#[test]
fn camera_frames_defer_cold_uploads_except_the_crossed_destination() {
    let mut cx = shader_cx();
    let pass = DrawPass::new(&mut cx);
    pass.set_size(&mut cx, dvec2(500.0, 500.0));
    let mut root = DrawList::new(&mut cx);
    let mut store = SplatStore::new();
    let k = key(1, CodeBand::Splat);
    store.begin_frame();
    store.insert(k, batch(20));
    store.begin_camera_frame();
    assert_eq!(
        store.insert(key(2, CodeBand::Splat), batch(20)),
        SplatAdmission::Deferred
    );
    {
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        draw.begin_pass(&pass, None);
        root.begin_always(&mut draw);
        let mut draw = Cx2d::new(&mut draw);
        assert!(!store.draw(&mut draw, k, rect(0.0, 0.0, 500.0, 500.0)));
        root.end(&mut draw);
        draw.end_pass(&pass);
    }
    assert_eq!(store.uploads_this_frame, 0);
    assert_eq!(store.rebuilds_this_frame, 0);
    let change = BandChange::Crossed {
        from: CodeBand::Density,
        to: CodeBand::Splat,
    };
    assert_eq!(
        store.rebuild_crossed(k.occurrence, k.kind, change, || panic!(
            "existing CPU batch rebuilt"
        )),
        SplatAdmission::Resident
    );
    record(
        &mut store,
        &mut cx,
        &pass,
        &mut root,
        &[k],
        rect(0.0, 0.0, 500.0, 500.0),
    );
    assert_eq!(store.uploads_this_frame, 1);
    assert_eq!(store.rebuilds_this_frame, 0);
    store.retire_all();
    store.release_completed(&mut cx, u64::MAX);
}
