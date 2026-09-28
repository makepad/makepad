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
        part_poses: Vec::new(),
    }
}

#[test]
fn equal_length_static_content_changes_invalidate_placed_scene_caches() {
    let mut renderer = Renderer::default();
    let original = model("kit/lamp", 1.0, false, 0.0);
    renderer.set_models(vec![original.clone()]);
    let original_rev = renderer.models_rev;

    // An identical frame is steady state and keeps delivered remaps.
    renderer.lm_remaps.push(vec4f(1.0, 1.0, 1.0, 1.0));
    renderer.set_models(vec![original]);
    assert_eq!(renderer.models_rev, original_rev);
    assert_eq!(renderer.lm_remaps.len(), 1);

    // Same count, different transform: this was the length-only hole.
    renderer.set_models(vec![model("kit/lamp", 2.0, false, 0.0)]);
    assert_eq!(renderer.models_rev, original_rev.wrapping_add(1));
    assert!(renderer.lm_remaps.is_empty());
    let moved_rev = renderer.models_rev;

    // Model identity and static depth order are content as well.
    renderer.set_models(vec![model("kit/lantern", 2.0, false, 0.0)]);
    assert_eq!(renderer.models_rev, moved_rev.wrapping_add(1));
    let renamed_rev = renderer.models_rev;
    renderer.set_models(vec![model("kit/lantern", 2.0, false, 3.0)]);
    assert_eq!(renderer.models_rev, renamed_rev.wrapping_add(1));
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
    renderer.water_rev = Some(1);
    renderer.set_models(vec![model("town/house", 9.0, false, 0.0)]);
    renderer.set_world_attachments(vec![model("fps/pistol", 9.0, false, 0.0)]);
    renderer.world_attachment_ground.push(5.5);
    renderer.lm_remaps.push(vec4f(0.5, 0.5, 0.25, 0.25));
    renderer.lm_ground = Some((Vec4f::default(), Vec4f::default()));
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
    assert!(renderer.water_tiles.is_empty());
    assert_eq!(renderer.water_rev, None);
    assert!(renderer.placed_models.is_empty());
    assert!(renderer.world_attachments.is_empty());
    assert!(renderer.world_attachment_ground.is_empty());
    assert!(renderer.view_models.is_empty());
    assert_eq!(renderer.placed_scene_signature, None);
    assert!(renderer.receiver_boxes.is_empty());
    assert!(renderer.occluder_boxes.is_empty());
    assert!(renderer.shadow_mesh.is_empty());
    assert!(renderer.lm_remaps.is_empty());
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
}
