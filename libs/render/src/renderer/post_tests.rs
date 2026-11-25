use super::*;
use makepad_draw::makepad_platform::makepad_script::script_eval;

#[test]
fn stock_reflections_and_fog_share_one_apply() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_draw::script_mod(vm);
        vm.heap_mut().new_module(id!(prelude));
        script_eval!(vm, { mod.prelude.widgets_internal = { ..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw: mod.draw } });
        vm.heap_mut().new_module(id!(widgets));
        crate::script_mod(vm);
    });
    let mut renderer = Renderer::default();
    renderer.set_hdr_output(true);
    renderer.set_camera_relative(true);
    renderer.set_ssr(crate::SsrSettings { strength: 1.0, ..Default::default() });
    renderer.set_volume_fog(makepad_scene::VolumeFog { density: 0.03, ..Default::default() });

    let size = dvec2(64.0, 48.0);
    let mut world = World::new();
    world.camera.target = vec3f(9000.0, 2.0, -3100.0);
    let rig = crate::CameraRig { yaw: 0.3, pitch: -0.2, in_test: true, effects: Default::default() };
    let camera = crate::scene_state(&world, Rect { size, ..Default::default() }, 0.0, &rig).unwrap();
    assert_ne!(renderer.render_origin_for(camera.camera_pos), Vec3f::default(), "exercise the snapped origin");

    let host = DrawPass::new(&mut cx);
    let scene = DrawPass::new(&mut cx);
    host.set_size(&mut cx, size);
    scene.set_size(&mut cx, size);
    let hdr = TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true };
    let color = Texture::new_with_format(&mut cx, hdr.clone());
    let surface = Texture::new_with_format(&mut cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true });
    let envspec = Texture::new_with_format(&mut cx, hdr);
    let depth = Texture::new_with_format(&mut cx, TextureFormat::DepthD32Sampled { size: TextureSize::Auto, initial: true });
    for tex in [&color, &surface, &envspec] { scene.add_color_texture(&mut cx, tex, Default::default()); }
    scene.set_depth_texture(&mut cx, &depth, DrawPassClearDepth::ClearWith(1.0));

    let event = DrawEvent::default();
    let mut draw = CxDraw::new(&mut cx, &event);
    let mut cx = Cx2d::new(&mut draw);
    cx.begin_pass(&host, Some(1.0));
    cx.make_child_pass(&scene);
    renderer.run_ssr(&mut cx, &scene, &depth, size);
    renderer.run_volume_fog(&mut cx, &scene, &depth, size);
    cx.begin_pass(&scene, Some(1.0));
    renderer.set_pass_camera(cx.cx, &scene, &camera);
    renderer.prepare_ssr_camera(scene.draw_pass_id(), camera.camera_pos);
    renderer.prepare_volume_fog_lighting(scene.draw_pass_id(), SunLight::default(), camera.camera_pos);
    cx.end_pass(&scene);
    let first = renderer.run_post(&mut cx, size, &color, host.draw_pass_id());
    if let Some(first) = first { scene.set_pass_parent(cx.cx, first); }
    cx.end_pass(&host);

    let counts = renderer.post_apply_counts();
    assert_eq!((counts.merged, counts.apart), (1, 0), "stock reflections and fog must share one apply: {}",
        counts.last_reason.unwrap_or("both effects did not record"));
    let fogged = renderer.volume_fog.pass.applied().unwrap().texture_id();
    let reflected = renderer.ssr.pass.applied().unwrap().texture_id();
    assert!(cx.passes.id_iter().any(|id| matches!(cx.passes[id].color_textures.as_slice(),
        [fog, ssr] if fog.texture.texture_id() == fogged && ssr.texture.texture_id() == reflected)),
        "the merged pass must write fogged colour and reflection history together");
}
