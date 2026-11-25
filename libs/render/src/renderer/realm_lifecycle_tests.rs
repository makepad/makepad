use super::*;

#[test]
fn clustered_realtime_skips_world_bakes_and_mode_switch_clears_old_shading() {
    use crate::gpu_lightmap::{BakeTrigger, GpuLightmapMode};
    let mut renderer = Renderer::default();
    renderer.set_clustered_lighting(true);
    renderer.set_gpu_lightmap_mode(GpuLightmapMode::Realtime);
    let world = World::new();
    renderer.kick_lightmap_bake(&world, &SunLight::default(), BakeTrigger::FirstBake);
    assert!(!renderer.world_atlas_required());
    assert!(!renderer.gpu_baker.has_state());
    assert!(renderer.lightmap_bake_progress().is_none());
    assert!(renderer.lm_kick_key.is_none());

    renderer.set_gpu_lightmap_mode(GpuLightmapMode::OnChange);
    assert!(renderer.world_atlas_required());
    renderer.bake.update(&world, &SunLight::default());
    renderer.lm_ground = Some((Vec4f::default(), Vec4f::default()));
    renderer.set_gpu_lightmap_mode(GpuLightmapMode::Realtime);
    assert!(renderer.lm_ground.is_none());
    assert_eq!(renderer.bake.stats().probes, 0);
    assert_eq!(renderer.dynamic_shade(Vec3f::default()), 1.0);

    renderer.set_clustered_lighting(false);
    assert!(renderer.world_atlas_required(), "legacy A/B path must still have its bake");
    renderer.set_clustered_lighting(true);
    assert!(!renderer.world_atlas_required());
}

fn model(id: &str, x: f32, dynamic: bool, depth_order: f32) -> ModelInstance {
    let mut transform = Mat4f::identity();
    transform.v[12] = x;
    ModelInstance {
        model: id.to_string(),
        transform,
        tint: vec4(1.0, 1.0, 1.0, 1.0),
        color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
        dynamic,
        depth_order,
        custom_material: None,
        layer_materials: Vec::new(),
        part_poses: Vec::new(),
        lamps: Default::default(),
    }
}

#[test]
fn equal_length_static_content_changes_invalidate_placed_scene_caches() {
    use crate::model::tests::{rooms_and_door_glb, Door};

    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut renderer = Renderer::default();
    renderer.set_gpu_lightmap_mode(crate::GpuLightmapMode::OnChange);
    let glb = rooms_and_door_glb(Door::Plain);
    let ao = Cx::encode_rgba_as_png(1, 1, &[255; 4]).unwrap();
    let prepare = || {
        let mesh = StaticModel::parse_glb(&glb).unwrap();
        PreparedStaticPreview::prepare_with_sidecars(mesh, Some(&ao), None).unwrap()
    };
    let prepared = prepare();
    let chart_key = prepared.lm_source.as_ref().unwrap().chart_key;
    renderer.install_static_preview(&mut cx, "kit/lamp", prepared, false).unwrap();
    let original = model("kit/lamp", 1.0, false, 0.0);
    let kept = model("kit/lamp", 3.0, false, 0.0);
    renderer.set_models(vec![original.clone(), kept.clone()]);
    let original_rev = renderer.models_rev;

    // An identical frame is steady state and keeps delivered remaps.
    let first_rect = vec4f(0.0, 0.0, 0.5, 0.5);
    let kept_rect = vec4f(0.5, 0.0, 0.5, 0.5);
    renderer.lightmap = Some(Texture::new(&mut cx));
    renderer.lm_remaps = vec![first_rect, kept_rect];
    renderer.lm_remap_owners = [&original, &kept].map(|copy| Some(LmRemapOwner {
        chart_key, transform: copy.transform.v.map(f32::to_bits),
    })).to_vec();
    renderer.lm_remaps_rev = Some(original_rev);
    renderer.set_models(vec![original, kept.clone()]);
    renderer.rekey_lm_remaps();
    assert_eq!(renderer.models_rev, original_rev);
    assert_eq!(renderer.lm_remaps, [first_rect, kept_rect]);

    // The kept copy changes slots. The other moves to an unbaked position.
    renderer.set_models(vec![kept.clone(), model("kit/lamp", 2.0, false, 0.0)]);
    assert_eq!(renderer.models_rev, original_rev.wrapping_add(1));
    let mut fork = renderer.fork_scene_for_review();
    fork.rekey_lm_remaps();
    assert_eq!(fork.lm_remaps, [kept_rect, Vec4f::default()], "the fork can draw before the live re-key");
    renderer.rekey_lm_remaps();
    assert_eq!(renderer.lm_remaps, [kept_rect, Vec4f::default()]);
    let moved_rev = renderer.models_rev;
    fork.mirror_review_assets_from(&renderer, true);
    assert_eq!(fork.lm_remaps_rev, None, "a live re-key changes ownership at the same length");
    fork.set_models(vec![kept.clone()]);
    fork.rekey_lm_remaps();
    fork.mirror_review_assets_from(&renderer, true);
    assert_eq!(fork.lm_remaps, [kept_rect], "a shorter fork keeps its own slots between live changes");

    // Model identity and static depth order are content as well.
    renderer.set_models(vec![kept.clone(), model("kit/lantern", 2.0, false, 0.0)]);
    assert_eq!(renderer.models_rev, moved_rev.wrapping_add(1));
    let renamed_rev = renderer.models_rev;
    renderer.set_models(vec![kept.clone(), model("kit/lantern", 2.0, false, 3.0)]);
    assert_eq!(renderer.models_rev, renamed_rev.wrapping_add(1));

    // Two copies of the installed mesh. Only the static copy contributes albedo to GI.
    let world = World::new();
    let bake_key = |r: &Renderer| lightmap_world_key(&world, r.models_rev, 6);
    let mut copies = vec![kept, model("kit/lamp", 1.0, true, 0.0)];
    renderer.set_models(copies.clone());
    let gi_before = renderer.gi_scene_key(&world);
    let bake_before = bake_key(&renderer);

    copies[0].tint.x = 0.25;
    renderer.set_models(copies.clone());
    let gi_after_tint = renderer.gi_scene_key(&world);
    assert_ne!(gi_after_tint, gi_before, "changing only a static copy's tint must request a new GI snapshot");
    assert_eq!(bake_key(&renderer), bake_before, "changing only a static copy's tint must preserve the lightmap bake key");

    copies[1].tint.x = 0.25;
    renderer.set_models(copies.clone());
    let gi_after_dynamic = renderer.gi_scene_key(&world);
    assert_eq!(gi_after_dynamic, gi_after_tint, "a dynamic copy's tint must leave the GI scene key unchanged");

    let bake_after_dynamic = bake_key(&renderer);
    renderer.set_models(copies);
    assert_eq!(renderer.gi_scene_key(&world), gi_after_dynamic, "resubmitting identical copies must preserve the GI scene key");
    assert_eq!(bake_key(&renderer), bake_after_dynamic, "resubmitting identical copies must preserve the lightmap bake key");

    renderer.install_static_preview(&mut cx, "kit/lamp", prepare(), false).unwrap();
    let gi_after_replacement = renderer.gi_scene_key(&world);
    assert_ne!(gi_after_replacement, gi_after_dynamic, "replacing a placed static model's asset must request a new GI snapshot");
    assert_eq!(bake_key(&renderer), bake_after_dynamic, "replacing an asset with the same chart must preserve the lightmap bake key");

    renderer.install_static_preview(&mut cx, "kit/spare", prepare(), false).unwrap();
    assert_eq!(renderer.gi_scene_key(&world), gi_after_replacement, "installing an unplaced asset must preserve the GI scene key");
}

#[test]
fn pbr_material_policy_defaults_on_and_can_be_disabled() {
    let mut renderer = Renderer::default();
    assert!(renderer.pbr_materials_enabled());
    renderer.set_pbr_materials_enabled(false);
    assert!(!renderer.pbr_materials_enabled());
}

#[test]
fn a_repaint_repacks_the_slabs_but_never_rekicks_the_bake() {
    let mut world = World::new();
    let slab = static_slab_key(&world, 1);
    let bake = lightmap_world_key(&world, 3, 6);
    world.mark_paint_dirty();
    assert_ne!(static_slab_key(&world, 1), slab, "a repainted lamp must reach the screen");
    assert_eq!(lightmap_world_key(&world, 3, 6), bake, "a repaint is not a world edit");
    world.mark_render_dirty();
    assert_ne!(lightmap_world_key(&world, 3, 6), bake, "geometry still re-kicks the bake");
    assert_ne!(static_slab_key(&world, 2), static_slab_key(&world, 1), "a rebake still repacks");
}

#[test]
fn a_night_clock_does_not_rekick_baked_sun_visibility() {
    use crate::gpu_lightmap::GpuLightmapMode::{OnChange, Realtime};
    let midnight = SunLight::from_time_of_day(0.0, 52.0).dir;
    for i in 0..120 {
        let dir = SunLight::from_time_of_day(i as f32 / 60.0, 52.0).dir;
        assert!(!lightmap_sun_changed(Some(midnight), dir, OnChange));
    }
    let noon = SunLight::from_time_of_day(12.0, 52.0).dir;
    assert!(lightmap_sun_changed(Some(midnight), noon, OnChange));
    assert!(lightmap_sun_changed(Some(noon), midnight, OnChange));
    assert!(!lightmap_sun_changed(Some(noon), noon, OnChange));
    assert!(lightmap_sun_changed(Some(noon), SunLight::from_time_of_day(13.0, 52.0).dir, OnChange));
    assert!(!lightmap_sun_changed(Some(midnight), noon, Realtime));
}

#[test]
fn sun_presentation_changes_preserve_layout_but_invalidate_baked_daylight() {
    use crate::gpu_lightmap::GpuLightmapMode::{OnChange, Realtime};
    let mut world = World::new();
    world.sun = makepad_scene::SunConfig {
        dir: Some(vec3f(0.0, 1.0, 0.0)),
        color: Some(vec3f(0.05, 0.05, 0.05)),
        ambient: Some(vec3f(0.05, 0.05, 0.05)),
        ..Default::default()
    };
    let dim = crate::sun::resolve_sun(&world.sun);
    let models_rev = 9;
    let geometry = (world.render_rev, models_rev);
    let old_key = lightmap_world_key(&world, models_rev, Renderer::legacy_daylight_key(&dim));
    // Color and ambient affect baked lamp headroom even when direction
    // is identical. They must reach the daylight key, not geometry.
    for ambient_only in [false, true] {
        world.sun.color = Some(if ambient_only { vec3f(0.05, 0.05, 0.05) } else { vec3f(1.0, 1.0, 1.0) });
        world.sun.ambient = Some(if ambient_only { vec3f(1.0, 1.0, 1.0) } else { vec3f(0.05, 0.05, 0.05) });
        let sun = crate::sun::resolve_sun(&world.sun);
        let key = lightmap_world_key(&world, models_rev, Renderer::legacy_daylight_key(&sun));
        assert_eq!((key.0, key.1), geometry);
        assert_ne!(key.2, old_key.2, "baked lamp headroom must refresh");
        assert!(!lightmap_sun_changed(Some(dim.dir), sun.dir, OnChange));
        assert!(!lightmap_sun_changed(Some(dim.dir), sun.dir, Realtime));
    }
    let lit = crate::sun::resolve_sun(&world.sun);
    let lit_key = lightmap_world_key(&world, models_rev, Renderer::legacy_daylight_key(&lit));
    world.sun.shadow_alpha = Some(0.17);
    let softer = crate::sun::resolve_sun(&world.sun);
    assert_eq!(softer.shadow_alpha, 0.17);
    assert_eq!(lightmap_world_key(&world, models_rev, Renderer::legacy_daylight_key(&softer)), lit_key,
        "shadow opacity is analytic, not an atlas contribution");

    // A directional change refreshes baked visibility in OnChange,
    // while Realtime gets the new direction through its per-frame CSM.
    world.sun.dir = Some(vec3f(1.0, 1.0, 0.0));
    let moved = crate::sun::resolve_sun(&world.sun);
    assert!(lightmap_sun_changed(Some(lit.dir), moved.dir, OnChange));
    assert!(!lightmap_sun_changed(Some(lit.dir), moved.dir, Realtime));
    assert_eq!((world.render_rev, models_rev), geometry,
        "an in-flight atlas still has a valid layout after sun updates");
}

#[test]
fn dynamic_motion_does_not_rebake_the_static_scene() {
    let mut renderer = Renderer::default();
    renderer.set_models(vec![model("cars/ambulance", 1.0, true, 0.0)]);
    let rev = renderer.models_rev;
    renderer.lm_remaps.push(Vec4f::default());

    renderer.set_models(vec![model("cars/ambulance", 40.0, true, 0.0)]);

    assert_eq!(renderer.models_rev, rev);
    assert_eq!(renderer.lm_remaps.len(), 1);
}

#[test]
fn world_attachments_are_dynamic_but_never_enter_placed_scene_identity() {
    let mut renderer = Renderer::default();
    renderer.set_models(vec![model("town/house", 2.0, false, 0.0)]);
    let rev = renderer.models_rev;
    let signature = renderer.placed_scene_signature;
    renderer.lm_remaps.push(Vec4f::default());

    // Deliberately pass `dynamic: false`: lane ownership, not a caller
    // flag, makes attachments analytically lit moving geometry.
    renderer.set_world_attachments(vec![model("kit/lamp", 4.0, false, 0.0)]);
    assert!(WorldModelLane::Attachment.is_dynamic(&renderer.world_attachments[0]));
    assert_eq!(renderer.models_rev, rev);
    assert_eq!(renderer.placed_scene_signature, signature);
    assert_eq!(renderer.lm_remaps.len(), 1);
    assert_eq!(renderer.placed_models.len(), 1);
    assert_eq!(renderer.placed_models[0].model, "town/house");
    assert_eq!(renderer.world_attachments.len(), 1);

    // Per-frame socket motion only replaces the attachment queue.
    renderer.set_world_attachments(vec![model("kit/lamp", 40.0, false, 0.0)]);
    assert_eq!(renderer.models_rev, rev);
    assert_eq!(renderer.placed_scene_signature, signature);
    assert_eq!(renderer.lm_remaps.len(), 1);
}

#[test]
fn world_attachment_light_hysteresis_has_its_own_key_namespace() {
    for slot in [0, 1, 77, u32::MAX as usize] {
        let attachment = WorldModelLane::Attachment.light_key(slot);
        assert_ne!(attachment, WorldModelLane::Placed.light_key(slot));
        assert_ne!(attachment, 0x8000_0000_0000_0000 | slot as u64);
        assert_eq!(attachment >> 62, 3);
    }
}

#[test]
fn replicated_tracer_velocity_is_its_full_3d_visual_axis() {
    let dir = vec3f(0.31, 0.47, -0.83).normalize();
    let tracer = Entity {
        kind: BodyKind::Mover,
        forward_axis: Some({
            let velocity = dir * 90.0;
            velocity * (1.0 / velocity.length_squared().sqrt())
        }),
        ..Default::default()
    };

    let rotation = Renderer::entity_rotation(&tracer);
    let visual_axis = vec3f(rotation.v[8], rotation.v[9], rotation.v[10]);
    assert!(
        visual_axis.dot(dir) > 0.999_999,
        "tracer visual axis {visual_axis:?} diverged from replicated velocity {dir:?}"
    );
}

#[test]
fn view_models_never_enter_world_identity_or_world_model_ownership() {
    let mut renderer = Renderer::default();
    renderer.set_models(vec![model("town/house", 2.0, false, 0.0)]);
    let rev = renderer.models_rev;
    let signature = renderer.placed_scene_signature;
    renderer.lm_remaps.push(Vec4f::default());

    renderer.set_view_models(vec![model("fps/pistol", 0.0, true, 0.0)]);
    assert_eq!(renderer.models_rev, rev);
    assert_eq!(renderer.placed_scene_signature, signature);
    assert_eq!(renderer.lm_remaps.len(), 1);
    assert_eq!(renderer.placed_models.len(), 1);
    assert_eq!(renderer.placed_models[0].model, "town/house");
    assert_eq!(renderer.view_models.len(), 1);

    // Camera-relative motion updates only the private presentation queue.
    renderer.set_view_models(vec![model("fps/pistol", 100.0, true, 0.0)]);
    assert_eq!(renderer.models_rev, rev);
    assert_eq!(renderer.placed_scene_signature, signature);
    assert_eq!(renderer.lm_remaps.len(), 1);
}

#[test]
fn entering_a_realm_clears_world_identity_but_preserves_device_policy() {
    let mut renderer = Renderer::default();
    let cluster_config = crate::clustered::ClusterConfig { lights_per_cluster: 16, ..Default::default() };
    renderer.set_cluster_config(cluster_config);
    renderer.set_shadow_budget(7);
    renderer.set_stage(Stage::mr_diorama(vec3f(1.0, 2.0, 3.0), 0.4, 0.08));
    renderer.set_gpu_lightmap_mode(crate::gpu_lightmap::GpuLightmapMode::Realtime);
    let csm_config = renderer.set_csm_config(1024, 48.0);
    let mut settings = renderer.bake_settings();
    settings.ao_rays = 3;
    settings.max_probes = 19;
    renderer.set_bake_settings(settings);

    renderer.slab_key = Some((1, 1, 1));
    renderer.slab_instance_count = 23;
    renderer.terrain_revision = 1;
    renderer.water.bed_key = Some((1, 1));
    renderer.set_models(vec![model("town/house", 9.0, false, 0.0)]);
    renderer.set_world_attachments(vec![model("fps/pistol", 9.0, false, 0.0)]);
    renderer.world_attachment_ground.push(5.5);
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let atlas = Texture::new(&mut cx);
    let top = (Texture::new(&mut cx), -2.0, 8.0);
    let region = vec4f(0.5, 0.5, 0.25, 0.25);
    let ground = (region, vec4f(-4.0, -4.0, 8.0, 8.0));
    let owner = LmRemapOwner { chart_key: 42, transform: renderer.placed_models[0].transform.v.map(f32::to_bits) };
    let set_lighting = |r: &mut Renderer| {
        r.lightmap = Some(atlas.clone());
        r.lm_top = Some(top.clone());
        r.lm_ground = Some(ground);
        r.lm_remaps = vec![region];
        r.lm_remap_owners = vec![Some(owner)];
        r.lm_remaps_rev = Some(r.models_rev);
    };
    set_lighting(&mut renderer);
    renderer.receiver_boxes.push((Vec3f::default(), vec3f(1.0, 1.0, 1.0)));
    renderer.occluder_boxes.push((Vec3f::default(), vec3f(1.0, 1.0, 1.0)));
    renderer.shadow_mesh.vertices.push(1.0);
    renderer.lm_lights.push(crate::lightmap::LmLight::omni(
        Vec3f::default(),
        vec3f(1.0, 0.8, 0.5),
        8.0,
    ));
    renderer.frame_lights.push(crate::lightmap::LmLight::omni(
        Vec3f::default(),
        vec3f(1.0, 1.0, 1.0),
        3.0,
    ));
    renderer.frame_baked_count = 1;
    renderer.host_lights.push(crate::lightmap::LmLight::omni(
        Vec3f::default(),
        vec3f(1.0, 1.0, 1.0),
        2.0,
    ));
    renderer.lamp_cache.push(crate::lightmap::LmLight::omni(
        Vec3f::default(),
        vec3f(1.0, 1.0, 1.0),
        4.0,
    ));
    renderer.lamp_cache_rev = Some((renderer.models_rev, 256));
    renderer.light_rank.push((1.0, 0));
    renderer.light_sel.push(0);
    renderer.light_block_scratch[0] = 1.0;
    renderer.light_cell_memory.insert(7, (2, 3));
    renderer.char_ground.push(4.0);
    renderer.model_ground.push(5.0);
    renderer.lm_kick_key = Some((1, renderer.models_rev, 32));
    renderer.shadow_points.push(vec3f(1.0, 2.0, 3.0));
    renderer.shadow_gate.built = Some((1, renderer.models_rev, 1, 32));
    let models_rev = renderer.models_rev;
    let bake_generation = renderer.bake.generation();
    let quality = renderer.quality();

    renderer.enter_realm();

    assert_eq!(renderer.slab_key, None);
    assert_eq!(renderer.slab_instance_count, 0);
    assert!(renderer.terrain_tiles.is_empty());
    assert_eq!(renderer.terrain_revision, 0);
    assert!(renderer.voxel_tiles.is_empty());
    assert_eq!(renderer.water.bed_key, None);
    assert!(renderer.placed_models.is_empty());
    assert!(renderer.world_attachments.is_empty());
    assert!(renderer.world_attachment_ground.is_empty());
    assert!(renderer.view_models.is_empty());
    assert_eq!(renderer.placed_scene_signature, None);
    assert!(renderer.receiver_boxes.is_empty());
    assert!(renderer.occluder_boxes.is_empty());
    assert!(renderer.shadow_mesh.is_empty());
    assert!(renderer.lm_remaps.is_empty());
    assert!(renderer.lm_remap_owners.is_empty());
    assert!(renderer.lightmap.is_none());
    assert!(renderer.lm_top.is_none());
    assert_eq!(renderer.lm_ground, None);
    assert!(!renderer.gpu_baker.has_state());
    assert!(renderer.lm_lights.is_empty());
    assert!(renderer.frame_lights.is_empty());
    assert_eq!(renderer.frame_baked_count, 0);
    assert!(renderer.host_lights.is_empty());
    assert!(renderer.lamp_cache.is_empty());
    assert_eq!(renderer.lamp_cache_rev, None);
    assert!(renderer.light_rank.is_empty());
    assert!(renderer.light_sel.is_empty());
    assert_eq!(renderer.light_block_scratch, [0.0; LIGHT_BLOCK_FLOATS]);
    assert!(renderer.light_cell_memory.is_empty());
    assert!(renderer.char_ground.is_empty());
    assert!(renderer.model_ground.is_empty());
    assert_eq!(renderer.lm_kick_key, None);
    assert!(renderer.shadow_points.is_empty());
    assert!(renderer.shadow_gate.built.is_none());
    assert!(renderer.shadow_gate.pending.is_none());
    assert_eq!(renderer.models_rev, models_rev.wrapping_add(1));
    assert_eq!(renderer.bake.generation(), bake_generation.wrapping_add(1));

    assert_eq!(renderer.shadow_budget(), 7);
    assert_eq!(renderer.clustered.config(), cluster_config);
    assert_eq!(renderer.gpu_lightmap_mode(), crate::gpu_lightmap::GpuLightmapMode::Realtime);
    assert_eq!(renderer.csm_config(), csm_config);
    assert_eq!(renderer.bake_settings().ao_rays, 3);
    assert_eq!(renderer.bake_settings().max_probes, 19);
    assert_eq!(renderer.quality(), quality);
    assert_eq!(renderer.stage().mode, StageMode::MrDiorama);
    assert_eq!(renderer.stage().origin, vec3f(1.0, 2.0, 3.0));
    assert_eq!(renderer.stage().yaw, 0.4);
    assert_eq!(renderer.stage().scale, 0.08);

    renderer.set_models(vec![model("town/house", 9.0, false, 0.0)]);
    renderer.terrain_revision = 1;
    let rev = renderer.models_rev;
    renderer.lm_kick_key = Some((1, rev, 32));
    set_lighting(&mut renderer);
    renderer.enter_revised_realm();
    assert_eq!(renderer.models_rev, rev.wrapping_add(1));
    assert!(renderer.placed_models.is_empty());
    assert_eq!(renderer.placed_scene_signature, None);
    assert_eq!(renderer.terrain_revision, 0);
    assert_eq!(renderer.lm_kick_key, None);
    assert_eq!(renderer.lightmap, Some(atlas));
    assert_eq!(renderer.lm_ground, Some(ground));
    assert_eq!(renderer.lm_top, Some(top));
    assert_eq!(renderer.lm_remaps, [region]);
    assert_eq!(renderer.lm_remap_owners, [Some(owner)]);
    assert_eq!(renderer.lm_remaps_rev, None);
}

#[test]
fn a_review_fork_carries_the_hosts_element_lookup_and_shadow_budget() {
    use crate::GpuLightmapMode::{OnChange, Realtime};

    let mut live = Renderer::default();
    live.set_shadow_budget(7);
    live.set_element_lookup(None, 0x5eed);
    let mut fork = live.fork_scene_for_review();
    assert_eq!(fork.shadow_budget(), 7, "the fork keeps the host's silhouette caster budget");
    assert_eq!(fork.gpu_baker.element_lookup_casters(), 0x5eed, "the fork keeps the host's element casters key");

    let tier = |r: &Renderer| (r.gpu_lightmap_mode(), r.clustered_lighting(), r.atlas_borrowed);
    let borrows = live.gpu_lightmap_mode() == OnChange;
    assert_eq!(
        tier(&fork),
        (live.gpu_lightmap_mode(), !borrows || live.clustered_lighting(), borrows),
    );

    live.set_gpu_lightmap_mode(Realtime);
    live.set_clustered_lighting(false);
    assert!(live.shadows_baked(), "the mixed host still uses a world atlas");
    assert_eq!(tier(&live.fork_scene_for_review()), (Realtime, true, false));
    fork.mirror_review_frame_from(&live);
    assert_eq!(tier(&fork), (Realtime, true, false));

    live.set_gpu_lightmap_mode(OnChange);
    for clustered in [false, true] {
        live.set_clustered_lighting(clustered);
        assert_eq!(tier(&live.fork_scene_for_review()), (OnChange, clustered, true));
        fork.mirror_review_frame_from(&live);
        assert_eq!(tier(&fork), (OnChange, clustered, true));
    }
    fork.mirror_review_assets_from(&live, false);
    assert_eq!(tier(&fork), (Realtime, true, false), "a capture in its own light casts its own shadows");
    fork.mirror_review_assets_from(&live, true);
    assert_eq!(tier(&fork), (OnChange, true, true), "a capture in the host's light borrows again");
    live.set_gpu_lightmap_mode(Realtime);
    fork.mirror_review_frame_from(&live);
    assert_eq!(tier(&fork), (Realtime, true, false), "the open pane stops borrowing");
}

#[test]
fn retire_placed_copy_keeps_other_slots_intact() {
    use crate::asset_lights::{AssetLightEmitter, AssetLightKind};
    use crate::custom_material::{DrawMaterialShadow, DrawSceneCustom};
    use crate::gpu_lightmap::{BakeTrigger, GpuBakeMesh, MoverMaterial};
    use crate::model::tests::{room_and_sky_glb, rooms_and_door_glb, Door};
    use crate::shadow_csm::{cascade_overlaps, CsmCascade};
    use makepad_draw::makepad_platform::makepad_script::script_eval;
    use makepad_render_material::{HookMask, HookSet, MaterialDesc};
    use std::path::Path;

    // Only authoritative refill/mirroring, animation-clock teardown and this
    // host contract test may ask for the unfiltered list.
    fn check_access(root: &Path, dir: &Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                check_access(root, &path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let file = path.strip_prefix(root).unwrap();
                let source = std::fs::read_to_string(&path).unwrap();
                if source.contains(concat!("all_including_", "retired(")) {
                    assert_eq!(file, Path::new("renderer/realm.rs"), "raw placed store accessed in {}", file.display());
                }
                if source.contains(concat!("placed_", "models(")) {
                    let allowed = ["renderer/realm.rs", "renderer/model_load.rs", "renderer/realm_lifecycle_tests.rs"];
                    assert!(allowed.iter().any(|name| file == Path::new(name)),
                        "public all-copies accessor used in {}", file.display());
                }
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    check_access(&src, &src);

    const TREE: usize = 2;
    const LAMP: usize = 5;
    const EMITTER: usize = 8;
    const SKY: usize = 11;
    const OTHER_LAMP: usize = 13;
    const OTHER_EMITTER: usize = 15;
    const OTHER_SKY: usize = 17;
    const DYNAMIC: usize = 23;
    let mut renderer = Renderer::default();
    renderer.set_clustered_lighting(true);
    renderer.set_gpu_lightmap_mode(crate::GpuLightmapMode::Realtime);
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let program = cx.with_vm(|vm| {
        vm.capture_errors();
        makepad_draw::geometry::script_mod(vm);
        makepad_draw::shader::draw_cube::script_mod(vm);
        vm.heap_mut().new_module(id!(prelude));
        script_eval!(vm, { mod.prelude.widgets_internal = {
            ..mod.std, ..mod.pod, ..mod.math, ..mod.shader, draw: mod.draw
        } });
        vm.heap_mut().new_module(id!(widgets));
        crate::shaders::DrawSceneSky::register_domes(vm);
        // Retirement needs a real installed material and derived caster ID.
        // Both can use a triangle shader; compiling the scene PBR graph here
        // only measures shader setup, which the material tests already cover.
        let scene_shader = DrawSceneCustom::script_shader(vm);
        let shadow_shader = DrawMaterialShadow::script_shader(vm);
        script_eval!(vm, {
            use mod.prelude.widgets_internal.*
            use mod.geom
            let triangle = {
                vertex_pos: vertex_position(vec4f)
                fb0: fragment_output(0, vec4f)
                geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
                mat_vertex: fn(p: vec3, n: vec3, uv: vec2) -> vec3 { return p }
                vertex: fn() {
                    let p = vec3(self.geom.px, self.geom.py, self.geom.pz)
                    self.vertex_pos = vec4(self.mat_vertex(p, vec3(0.0, 1.0, 0.0), vec2(0.0)), 1.0)
                }
                fragment: fn() { self.fb0 = vec4(1.0) }
            }
            mod.draw.DrawSceneCustom = mod.std.set_type_default() do #(scene_shader) {
                ..triangle
                params: uniform(vec4(0.0, 0.0, 0.0, 0.0))
            }
            mod.draw.DrawMaterialShadow = mod.std.set_type_default() do #(shadow_shader) {
                ..triangle
            }
        });
        let spec = script_eval!(vm, { use mod.prelude.widgets_internal.*
            {vertex: fn(p: vec3, n: vec3, uv: vec2) -> vec3 { return p + n * self.params.x }}
        }).as_object().unwrap();
        let hooks = HookSet::from_spec(vm, spec, HookMask::ALL).unwrap();
        DrawSceneCustom::build(vm, &MaterialDesc::default(), &hooks, HookMask::ALL, Vec4f::default()).unwrap()
    });
    assert!(program.shadow.is_some(), "fixture needs a derived caster for the material source index");
    assert!(renderer.install_custom_material("relief".into(), program), "fixture material must install");
    let prepare = |door, ao: Option<&[u8]>| {
        let mesh = StaticModel::parse_glb(&rooms_and_door_glb(door)).unwrap();
        PreparedStaticPreview::prepare_with_sidecars(mesh, ao, None).unwrap()
    };
    let ao = Cx::encode_rgba_as_png(1, 1, &[255; 4]).unwrap();
    let mut tree = renderer.upload_static_preview(&mut cx, prepare(Door::Plain, None));
    let lamp = renderer.upload_static_preview(&mut cx, prepare(Door::Plain, Some(&ao)));
    // Two distinct geometries split a copy's caster indices in the final sort.
    tree.extra_draws.push((lamp.geometry.clone(), lamp.texture.clone(), lamp.detail.clone(),
        lamp.detail_scale, lamp.material.clone()));
    let mut emitter = renderer.upload_static_preview(&mut cx, prepare(Door::Animated, None));
    emitter.emitters = vec![AssetLightEmitter {
        node: 0, rest_transform: Mat4f::identity(), animation: None, color: Vec3f::all(1.0),
        intensity: 2.0, range: 5.0, kind: AssetLightKind::Point,
    }].into();
    let mut sky_model = StaticModel::parse_glb(&room_and_sky_glb(Some("cylinder"), 1)).unwrap();
    // The GLB parser fixture uses image markers. Supply the real one-pixel PNG.
    sky_model.sky.as_mut().expect("fixture sky faces").images = vec![ao.clone()];
    sky_model.texture_png = None;
    let sky = renderer.upload_static_preview(&mut cx,
        PreparedStaticPreview::prepare_with_sidecars(sky_model, None, None).unwrap());
    let sky_geometry = sky.sky.as_ref().expect("uploaded sky geometry").geometry.geometry_id();
    for (id, mesh) in [("grove", tree), ("lantern", lamp), ("beacon", emitter), ("vault", sky)] {
        renderer.install_uploaded_static(id, mesh);
        renderer.model_casts_shadow.insert(id.into(), true);
    }
    let copies: Vec<_> = (0..=DYNAMIC).map(|slot| {
        let id = match slot {
            LAMP | OTHER_LAMP => "lantern",
            EMITTER | OTHER_EMITTER => "beacon",
            SKY | OTHER_SKY => "vault",
            _ => "grove",
        };
        // Straddle the block boundary at zero with a small bake footprint.
        let mut copy = model(id, if slot % 2 == 0 { -2.0 } else { 1.0 }, slot == DYNAMIC, 0.0);
        for axis in [0, 5, 10] { copy.transform.v[axis] = 0.125; }
        // The fixtures are planar and their moving part only translates in Y.
        // Z identifies an original slot in every consumer, including light positions.
        copy.transform.v[14] = slot as f32 / 32.0;
        if id == "beacon" {
            copy.custom_material = Some(CustomMaterialInstance {
                name: "relief".into(), params: vec4(slot as f32 / 256.0, 0.0, 0.0, 0.0),
            });
        }
        copy
    }).collect();
    // The retired tree is in the retained prefix. Its custom-material sibling
    // at EMITTER is the first record in the resubmitted tail.
    let refill = |r: &mut Renderer, rows: &[ModelInstance], initial: bool| {
        r.refill_models_with_prefix(1, |prefix| {
            assert!(initial, "a steady prefix must keep its original slots without refilling");
            for m in &rows[..EMITTER] { prefix.push(m); }
        }, |tail| {
            for m in &rows[EMITTER..] { tail.push(m); }
        });
    };
    refill(&mut renderer, &copies, true);
    let refresh = |r: &mut Renderer| r.placed_blocks.refresh(&r.static_models, &r.placed_models, r.placed_scene_signature);
    refresh(&mut renderer);
    let (models, scene, paint, gi_rev) = (renderer.models_rev, renderer.placed_scene_signature,
        renderer.placed_paint_signature, renderer.models_gi_rev);
    let world = World::default();
    let sun = SunLight::default();
    fn slot(z: f32) -> usize { (z * 32.0).round() as usize }
    fn bits(v: Vec3f) -> [u32; 3] { [v.x, v.y, v.z].map(f32::to_bits) }
    let material = |m: &MoverMaterial| (m.shader, [m.params.x, m.params.y, m.params.z, m.params.w].map(f32::to_bits),
        m.cutoff.to_bits(), m.texture.texture_id());
    let mesh = |m: &GpuBakeMesh| (m.geometry, m.transform.v.map(f32::to_bits), m.material.as_ref().map(material),
        m.cutout.as_ref().map(|t| t.texture_id()), m.msdf, m.band);
    let light = |l: &crate::lightmap::LmLight| (slot(l.pos.z), (bits(l.pos), bits(l.color), l.radius.to_bits(),
        bits(l.dir), l.spot.to_bits(), l.cone, l.shadows, l.volumetric, l.area));

    let cascade = CsmCascade {
        rx: vec4(0.01, 0.0, 0.0, 0.0), ry: vec4(0.0, 0.01, 0.0, 0.0),
        rz: vec4(0.0, 0.0, 0.01, 0.5), ..Default::default()
    };
    // Record the three sky submissions in separate lists in one frame.
    let pass = DrawPass::new(&mut cx);
    let mut root_list = DrawList::new(&mut cx);
    let mut sky_lists = Vec::with_capacity(3);
    let event = DrawEvent::default();
    let mut frame = CxDraw::new(&mut cx, &event);
    frame.begin_pass(&pass, Some(1.0));
    root_list.begin_always(&mut frame);
    let mut sky_submissions = |r: &mut Renderer, frame: &mut CxDraw| {
        let mut list = DrawList::new(frame.cx);
        list.begin_always(frame);
        r.draw_sky_faces(&mut Cx3d::new(frame), Vec3f::default(), None, &mut RenderStats::default());
        list.end(frame);
        let items = &frame.draw_lists[list.id()].draw_items;
        let mut submissions = Vec::new();
        for i in 0..items.len() {
            let item = &items[i];
            let call = item.draw_call().expect("a sky submission must be a draw call");
            assert_eq!(call.geometry_id, Some(sky_geometry), "sky pass must submit the uploaded face geometry");
            let transform = frame.draw_shaders[call.draw_shader_id.index].mapping.instances
                .find(live_id!(transform)).expect("sky draw must carry its model transform").offset;
            for instance in item.instances.as_ref().unwrap().chunks_exact(call.total_instance_slots) {
                submissions.push((slot(instance[transform + 14]), instance.iter().copied().map(f32::to_bits).collect::<Vec<_>>()));
            }
        }
        sky_lists.push(list);
        submissions
    };
    let mut consumers = |r: &mut Renderer, frame: &mut CxDraw| {
        let sky = sky_submissions(r, frame);
        let cx = &mut *frame.cx;
        let gi = r.gi_snapshot(cx, &world, (Vec3f::all(-64.0), Vec3f::all(64.0)));
        let gi: Vec<_> = gi.instances.iter().map(|i| (slot(i.transform.v[14]),
            (i.key, i.triangles(), bits(i.lo), bits(i.hi)))).collect();
        let mut local = Vec::new();
        r.collect_local_static_casters(cx, &world, &mut local);
        let row = |m: &GpuBakeMesh| (slot(m.transform.v[14]), (mesh(m), bits(m.min), bits(m.max)));
        let local: Vec<_> = local.iter().map(row).collect();
        let bake = r.lightmap_bake_job(&world, &sun, BakeTrigger::WorldEdit).expect("the lamp has a chart");
        let baked: Vec<_> = bake.casters_only.iter().map(row).collect();
        let charts: Vec<_> = bake.mesh_map.iter().enumerate().map(|(k, &slot)| (slot, (
            bake.scene.meshes[k].transform.v.map(f32::to_bits), bake.scene.meshes[k].source.chart_key,
            bake.mesh_geometry[k], bake.mesh_materials[k].as_ref().map(material)))).collect();
        let lamps: Vec<_> = r.harvest_lamps().iter().map(light).collect();
        r.build_frame_lights(&sun);
        let lights: Vec<_> = r.frame_lights.iter().map(light).collect();
        let movers = |r: &Renderer| {
            let mut out = Vec::new();
            r.collect_lm_movers(&world, Vec3f::default(), 1.0, None, &mut Default::default(), &mut out);
            out.iter().map(|m| (slot(m.transform.v[14]), (m.geometry, m.transform.v.map(f32::to_bits),
                bits(m.min), bits(m.max), m.material.as_ref().map(material)))).collect::<Vec<_>>()
        };
        let moving = movers(r);
        let held = std::mem::take(&mut r.placed_blocks);
        let uncached = movers(r);
        r.placed_blocks = held;
        assert_eq!(moving, uncached, "cached loose movers and the live fallback must agree");
        (gi, local, baked, charts, lamps, lights, moving, sky)
    };
    fn survivors<T: std::fmt::Debug + PartialEq>(before: &[(usize, T)], after: &[(usize, T)], removed: &[usize], why: &str) {
        let expected: Vec<_> = before.iter().filter(|(slot, _)| !removed.contains(slot)).collect();
        assert_eq!(after.iter().collect::<Vec<_>>(), expected, "{why}");
    }
    let baseline = consumers(&mut renderer, &mut frame);
    let (gi, local, baked, charts, lamps, lights, movers, skies) = &baseline;
    assert_eq!(skies.iter().map(|r| r.0).collect::<Vec<_>>(), [SKY, OTHER_SKY],
        "both sky copies must submit their face geometry before retirement");
    for i in [TREE, LAMP, EMITTER, SKY] {
        assert!(gi.iter().any(|r| r.0 == i) && local.iter().any(|r| r.0 == i),
            "slot {i} must reach GI and local shadows before retirement");
    }
    assert!(baked.iter().any(|r| r.0 == TREE) && baked.iter().any(|r| r.0 == EMITTER),
        "the bake must start with the tree and emitter as caster-only copies");
    assert_eq!(charts.iter().map(|r| r.0).collect::<Vec<_>>(), [LAMP, OTHER_LAMP], "the bake must have both chart owners");
    assert_eq!(lamps.iter().map(|r| r.0).collect::<Vec<_>>(), [LAMP, OTHER_LAMP], "both lamps must be harvested");
    assert_eq!(lights.iter().map(|r| r.0).collect::<Vec<_>>(), [LAMP, OTHER_LAMP, EMITTER, OTHER_EMITTER], "both authored emitters must light the frame");
    assert!(movers.iter().any(|r| r.0 == EMITTER) && movers.iter().any(|r| r.0 == OTHER_EMITTER),
        "both static animated copies must initially cast as movers");

    let static_before = renderer.csm_static_casters.clone();
    let sources = renderer.csm_material_sources.clone();
    let csm_blocks = |r: &Renderer| r.csm_static_blocks.iter()
        .map(|b| (b.start, b.end, bits(b.min), bits(b.max))).collect::<Vec<_>>();
    let runs = csm_blocks(&renderer);
    assert!(runs.len() > 1, "the smaller fixture must still span cascade runs");
    let (blocks, loose, visible) = renderer.placed_blocks.snapshot(&renderer);
    assert_eq!(visible.len(), copies.len(), "the unculled draw order must start with every copy");
    assert!(!sources.is_empty(), "fixture must include material-backed cascade entries");
    for source in &sources {
        let caster = &static_before[source.caster];
        assert!(caster.material.is_some() && slot(caster.transform.v[14]) == source.instance,
            "each material source must point to its own final sorted caster");
    }
    assert!(static_before.iter().filter(|m| slot(m.transform.v[14]) == TREE).count() >= 2,
        "tree retirement must exercise multiple sorted caster indices");
    assert!(blocks.len() > 1 && blocks.iter().any(|b| b.2.iter().any(|m| m.0 == TREE as u32)),
        "tree must be a member of one of several static blocks");
    assert!(loose.contains(&(EMITTER as u32)), "animated emitter must start loose");
    let cache = renderer.lamp_cache_rev;
    assert!(cache.is_some(), "the frame must have populated the lamp cache");
    let verify = |r: &Renderer, removed: &[usize]| {
        assert_eq!(r.placed_models().len(), copies.len(), "the host must still see every original slot");
        for (original, held) in copies.iter().zip(r.placed_models()) {
            assert_eq!((&original.model, original.transform.v), (&held.model, held.transform.v),
                "the public list must keep the host's original slot order and poses");
        }
        assert_eq!(r.placed_models.live().map(|(i, _)| i).collect::<Vec<_>>(),
            (0..copies.len()).filter(|i| !removed.contains(i)).collect::<Vec<_>>(), "live enumeration must preserve slot IDs");
        let (actual_blocks, actual_loose, actual_visible) = r.placed_blocks.snapshot(r);
        assert_eq!(actual_visible, visible.iter().copied().filter(|i| !removed.contains(i)).collect::<Vec<_>>(),
            "colour draw order must lose exactly the retired slots");
        let mut expected_blocks = blocks.clone();
        for (_, _, members) in &mut expected_blocks { members.retain(|m| !removed.contains(&(m.0 as usize))); }
        assert_eq!(actual_blocks, expected_blocks, "only retired block members may change; conservative bounds and ordering survive");
        assert_eq!(actual_loose, loose.iter().copied().filter(|i| !removed.contains(&(*i as usize))).collect::<Vec<_>>(),
            "loose copies must remain in ascending order with exactly the retired slots removed");
        assert_eq!(r.csm_static_casters.len(), static_before.len(), "retiring must preserve every final caster index");
        assert_eq!(r.csm_material_sources, sources, "derived material sources must retain their final caster indices");
        assert_eq!(csm_blocks(r), runs, "the static cascade runs and bounds must be bit-identical");
        for (old, now) in static_before.iter().zip(&r.csm_static_casters) {
            let owner = slot(old.transform.v[14]);
            assert_eq!(mesh(now), mesh(old), "caster geometry, pose, material and band must survive for slot {owner}");
            let (min, max) = if removed.contains(&owner) { (Vec3f::all(f32::INFINITY), Vec3f::all(f32::NEG_INFINITY)) }
                else { (old.min, old.max) };
            assert_eq!((bits(now.min), bits(now.max)), (bits(min), bits(max)), "only retired caster bounds may change, slot {owner}");
            assert_eq!(cascade_overlaps(&cascade, now.min, now.max), !removed.contains(&owner),
                "a cascade covering the whole forest must accept precisely the live casters, slot {owner}");
        }
    };
    let mut verify_consumers = |r: &mut Renderer, frame: &mut CxDraw, removed: &[usize]| {
        let (g, l, b, c, h, f, m, s) = consumers(r, frame);
        survivors(skies, &s, removed, "submitted sky face instances");
        survivors(gi, &g, removed, "GI snapshot inputs");
        survivors(local, &l, removed, "local static casters");
        survivors(baked, &b, removed, "caster-only bake inputs");
        survivors(charts, &c, removed, "bake chart indices and transforms");
        survivors(lamps, &h, removed, "harvested lamps");
        survivors(lights, &f, removed, "frame lights");
        survivors(movers, &m, removed, "animated and dynamic movers");
    };

    renderer.retire_placed_copy(TREE);
    assert_eq!(renderer.lamp_cache_rev, cache, "tree retirement must preserve the lamp cache");
    assert_eq!(renderer.models_gi_rev, gi_rev + 1, "one tree retirement advances GI once");
    renderer.retire_placed_copy(EMITTER);
    assert_eq!(renderer.lamp_cache_rev, cache, "authored emitter retirement must preserve the harvested cache");
    assert_eq!(renderer.models_gi_rev, gi_rev + 2, "one emitter retirement advances GI once");
    renderer.retire_placed_copy(SKY);
    assert_eq!(renderer.lamp_cache_rev, cache, "sky retirement must preserve the harvested cache");
    assert_eq!(renderer.models_gi_rev, gi_rev + 3, "one sky retirement advances GI once");
    verify(&renderer, &[TREE, EMITTER, SKY]);

    refresh(&mut renderer);
    renderer.retire_placed_copy(TREE);
    renderer.retire_placed_copy(DYNAMIC);
    renderer.retire_placed_copy(usize::MAX);
    let tail = copies.len();
    renderer.placed_models.append_items(vec![copies[TREE].clone()]);
    renderer.retire_placed_copy(tail);
    assert_eq!(renderer.placed_models().len(), tail, "transient tail is outside the authoritative list");
    assert!(renderer.placed_models.get(tail).is_some(), "a transient item must remain live");
    renderer.placed_models.truncate_items(tail);
    assert_eq!(renderer.models_gi_rev, gi_rev + 3, "repeat, dynamic, missing and transient retirements are no-ops");
    assert_eq!(renderer.lamp_cache_rev, cache, "invalid retirements must preserve cached lamps");
    verify(&renderer, &[TREE, EMITTER, SKY]);

    let mut review = renderer.fork_scene_for_review();
    let review_gi = review.models_gi_rev;
    renderer.retire_placed_copy(LAMP);
    assert_eq!(renderer.lamp_cache_rev, None, "retiring a harvested lamp must invalidate the cache");
    assert_eq!(renderer.models_gi_rev, gi_rev + 4, "one lamp retirement advances GI once");
    let removed = [TREE, LAMP, EMITTER, SKY];
    verify(&renderer, &removed);
    let identity = |r: &Renderer| (r.models_rev, r.placed_scene_signature, r.placed_paint_signature);
    assert_eq!(identity(&renderer), (models, scene, paint), "retirement must preserve the submitted identity");

    review.mirror_review_frame_from(&renderer);
    assert_eq!(review.models_gi_rev, review_gi + 1, "review must apply only the newly retired lamp");
    let check_refill = |r: &Renderer| {
        assert_eq!(identity(r), (models, scene, paint), "changing submission paths must preserve both signatures and models_rev");
        assert_eq!(r.models_gi_rev, gi_rev + 4, "identical contents must keep the GI snapshot on every path");
        assert!(removed.iter().all(|&i| r.placed_models.get(i).is_none()), "every path must keep all four tombstones");
        assert_eq!(r.csm_static_casters.len(), static_before.len(), "a refill must not rebuild and compact the retired caster entries");
    };
    refill(&mut renderer, &copies, false);
    check_refill(&renderer);
    renderer.set_models(copies.clone());
    check_refill(&renderer);
    renderer.rebuild_models_with_prefix(2, |prefix| prefix.extend_from_slice(&copies), |_| {});
    check_refill(&renderer);
    refill(&mut renderer, &copies, true); // Move the boundary from the whole list back to EMITTER.
    check_refill(&renderer);
    renderer.rebuild_models(|rows| rows.clone_from_slice(&copies));
    check_refill(&renderer);
    refresh(&mut renderer);
    verify(&renderer, &removed);
    verify_consumers(&mut renderer, &mut frame, &removed);
    review.mirror_review_frame_from(&renderer);
    assert_eq!(review.models_gi_rev, review_gi + 1, "mirroring an identical retired list must keep the review snapshot");
    assert!(removed.iter().all(|&i| review.placed_models.get(i).is_none()), "review must keep every host retirement");

    let mut changed = copies.clone();
    changed[EMITTER].tint.x = 0.25;
    refill(&mut renderer, &changed, true);
    refresh(&mut renderer);
    assert_eq!((renderer.models_rev, renderer.placed_scene_signature), (models, scene), "paint preserves the scene identity");
    assert_ne!(renderer.placed_paint_signature, paint, "a static RGB edit must reach the paint signature");
    assert_eq!(renderer.models_gi_rev, gi_rev + 5, "a paint-only change updates GI once");
    verify(&renderer, &removed);

    // A live material edit still updates its caster in place after another
    // binding retired. Rebuilding here would erase the dead caster entries.
    let live_caster = sources.iter().find(|s| s.instance == OTHER_EMITTER).unwrap().caster;
    changed[OTHER_EMITTER].custom_material.as_mut().unwrap().params.x = 0.75;
    refill(&mut renderer, &changed, false);
    assert_eq!(renderer.csm_static_casters.len(), static_before.len(), "editing a live caster's params must keep the sorted slots");
    assert_eq!(renderer.csm_static_casters[live_caster].material.as_ref().unwrap().params.x, 0.75,
        "the surviving caster must take its new material params");

    // Static depth order changes scene identity at the same length. All
    // geometry and original tints are back, so every consumer can be compared
    // bit for bit with its first snapshot.
    changed[EMITTER] = copies[EMITTER].clone();
    changed[OTHER_EMITTER] = copies[OTHER_EMITTER].clone();
    changed[SKY].depth_order = 1.0;
    refill(&mut renderer, &changed, false);
    refresh(&mut renderer);
    assert_ne!(renderer.placed_scene_signature, scene, "a static depth-order edit must change the scene signature");
    assert_eq!(renderer.models_rev, models + 1, "a changed scene must advance models_rev once");
    assert_eq!(renderer.models_gi_rev, gi_rev + 6, "restoring the changed list must resnapshot GI once");
    verify(&renderer, &[]);
    verify_consumers(&mut renderer, &mut frame, &[]);
    review.mirror_review_frame_from(&renderer);
    assert_eq!(review.placed_models.live().count(), copies.len(), "the changed host list must restore the review's slots too");

    let dropped = DYNAMIC - 1;
    renderer.retire_placed_copy(TREE);
    renderer.retire_placed_copy(dropped);
    renderer.set_models(changed[..dropped].to_vec());
    assert_eq!(renderer.placed_models.live().count(), dropped, "a shorter list restores every slot it retains");
    assert!(renderer.csm_static_casters.iter().any(|m| slot(m.transform.v[14]) == TREE && cascade_overlaps(&cascade, m.min, m.max)),
        "the shorter list must rebuild the still-listed tree's visible casters");
    assert!(renderer.csm_static_casters.iter().all(|m| slot(m.transform.v[14]) < dropped),
        "a retired copy omitted from the changed list must leave the caster table");
    root_list.end(&mut frame);
    frame.end_pass(&pass);
}
