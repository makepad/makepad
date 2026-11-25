use super::*;
use crate::model::tests::{rooms_and_door_glb, Door};

/// The importer-shaped fixture's door: closed at t=0 (on the floor),
/// open at t=1 (lifted 3), authored open.
fn door() -> crate::model::AnimPart {
    let mut m = StaticModel::parse_glb(&rooms_and_door_glb(Door::Animated)).unwrap();
    assert_eq!(m.anim_parts.len(), 1);
    m.anim_parts.pop().unwrap()
}

fn lift(part: &crate::model::AnimPart, time: f32) -> f32 {
    part.transform_at(time).v[13]
}

#[test]
fn an_untriggered_part_sits_in_the_authored_default() {
    let part = door();
    let states = ModelStates::default();
    let (state, time, target) =
        states.clock(&ModelTarget::Model("map".into()), "map", &part);
    assert_eq!(state, 1, "extras.default = open");
    assert!((time - 1.0).abs() < 1.0e-6);
    assert!((target - 1.0).abs() < 1.0e-6);
    assert!((lift(&part, time) - 3.0).abs() < 1.0e-6);
}

#[test]
fn closing_travels_the_clip_in_the_blend_time() {
    let part = door();
    let mut states = ModelStates::default();
    let key = ModelTarget::Model("map".into());
    assert!(states.set(key.clone(), &part, "closed", 1.0));

    // Half the blend, half the travel — linear in time, by contract.
    states.tick(0.5);
    let (_, time, _) = states.clock(&key, "map", &part);
    assert!((time - 0.5).abs() < 1.0e-5, "time {time}");
    assert!((lift(&part, time) - 1.5).abs() < 1.0e-4);

    // The rest of it, and then it stops dead rather than overshooting.
    states.tick(0.5);
    let (state, time, _) = states.clock(&key, "map", &part);
    assert_eq!(state, 0);
    assert!(time.abs() < 1.0e-6, "time {time}");
    states.tick(5.0);
    let (_, time, target) = states.clock(&key, "map", &part);
    assert!(time.abs() < 1.0e-6 && (time - target).abs() < 1.0e-6);
    assert!(lift(&part, time).abs() < 1.0e-6, "closed sits on the floor");
}

#[test]
fn a_command_mid_move_reverses_from_where_the_part_is() {
    let part = door();
    let mut states = ModelStates::default();
    let key = ModelTarget::Model("map".into());
    states.set(key.clone(), &part, "closed", 1.0);
    states.tick(0.5);
    let (_, half, _) = states.clock(&key, "map", &part);

    // Re-aimed at open from exactly half way: no jump, and the reversal
    // takes its own blend rather than resuming the old one.
    states.set(key.clone(), &part, "open", 1.0);
    let (state, time, target) = states.clock(&key, "map", &part);
    assert_eq!(state, 1);
    assert!((time - half).abs() < 1.0e-6, "the door jumped: {time} vs {half}");
    assert!((target - 1.0).abs() < 1.0e-6);

    states.tick(0.5);
    let (_, time, _) = states.clock(&key, "map", &part);
    assert!((time - 0.75).abs() < 1.0e-5, "time {time}");
    states.tick(0.5);
    let (_, time, _) = states.clock(&key, "map", &part);
    assert!((time - 1.0).abs() < 1.0e-6, "time {time}");
    assert!((lift(&part, time) - 3.0).abs() < 1.0e-5);
}

#[test]
fn localgen_manifest_clip_loops_until_a_game_command_overrides_it() {
    let mut part = door();
    part.kind = Some("localgen-bob".into());
    let mut states = ModelStates::default();
    let key = ModelTarget::Model("prop".into());

    states.tick(0.25);
    let (_, idle, _) = states.clock(&key, "prop", &part);
    assert!((idle - 0.25).abs() < 1.0e-6);
    states.tick(1.0);
    let (_, wrapped, _) = states.clock(&key, "prop", &part);
    assert!((wrapped - 0.25).abs() < 1.0e-6);

    assert!(states.set(key.clone(), &part, "closed", 0.0));
    states.tick(0.25);
    let (_, overridden, _) = states.clock(&key, "prop", &part);
    assert!(overridden.abs() < 1.0e-6);
}

#[test]
fn a_zero_blend_snaps_and_an_unknown_state_changes_nothing() {
    let part = door();
    let mut states = ModelStates::default();
    let key = ModelTarget::Model("map".into());
    assert!(states.set(key.clone(), &part, "closed", 0.0));
    let (_, time, _) = states.clock(&key, "map", &part);
    assert!(time.abs() < 1.0e-6, "zero blend is instant");

    assert!(!states.set(key.clone(), &part, "ajar", 1.0), "no such state");
    let (state, time, _) = states.clock(&key, "map", &part);
    assert_eq!(state, 0);
    assert!(time.abs() < 1.0e-6);
}

/// The collision half: a closed door is a wall in the doorway, an open
/// one is not. Same boxes, moved by the same matrix the draw uses.
#[test]
fn collider_boxes_move_with_the_part() {
    let part = door();
    let local = part.collider_boxes();
    assert!(!local.is_empty());
    let mut instance = Mat4f::identity();
    instance.v[13] = 10.0; // the level itself is placed 10 up

    let world_at = |time: f32| {
        let m = Mat4f::mul(&instance, &part.transform_at(time));
        world_boxes(&m, &local)
    };
    let (closed, cmin, cmax) = world_at(part.state_time(0));
    let (open, omin, omax) = world_at(part.state_time(1));
    assert_eq!(closed.len(), local.len());
    // Closed: the slab fills the opening at the level's own height.
    assert!((cmin.y - 10.0).abs() < 1.0e-4, "{cmin:?}");
    assert!((cmax.y - 13.0).abs() < 1.0e-4, "{cmax:?}");
    // Open: the same slab has risen out of the way by the clip's travel.
    assert!((omin.y - 13.0).abs() < 1.0e-4, "{omin:?}");
    assert!((omax.y - 16.0).abs() < 1.0e-4, "{omax:?}");
    assert!(open.iter().zip(&closed).all(|(o, c)| {
        (o.0.y - c.0.y - 3.0).abs() < 1.0e-3 && (o.0.x - c.0.x).abs() < 1.0e-4
    }));
    // Half way is half way here too — a door caught moving is where it
    // looks, not snapped to an end state.
    let (_, hmin, _) = world_at(0.5);
    assert!((hmin.y - 11.5).abs() < 1.0e-3, "{hmin:?}");
}

#[test]
fn a_slot_command_wins_over_a_model_command() {
    use crate::custom_material::DrawSceneCustom;
    use makepad_draw::makepad_platform::makepad_script::script_eval;
    use makepad_render_material::{HookMask, HookSet, MaterialDesc};

    let mut model = StaticModel::parse_glb(&rooms_and_door_glb(Door::Animated)).unwrap();
    let [part] = model.anim_parts.as_mut_slice() else {
        panic!("expected one animated door");
    };
    part.layers[0].name = "glass".into();
    part.layers[0].pbr.surface = Some(crate::material_surface::MaterialSurface {
        alpha_mode: 2,
        base_alpha: 0.5,
        ..Default::default()
    }.into());
    let mut states = ModelStates::default();
    states.set(ModelTarget::Model("map".into()), part, "closed", 0.0);
    states.set(ModelTarget::Instance(2), part, "open", 0.0);

    // Slot 2 asked for open; every other copy follows the model command.
    let (state, _, _) = states.clock(&ModelTarget::Instance(2), "map", part);
    assert_eq!(state, 1);
    let (state, _, _) = states.clock(&ModelTarget::Instance(3), "map", part);
    assert_eq!(state, 0, "an unaddressed slot follows the model command");

    states.set(ModelTarget::Instance(2), part, "closed", 1.0);
    states.tick(0.5);
    let mut frame = Mat4f::rotation(vec3f(0.0, 0.0, 0.5));
    frame.v[12] = 7.0;
    let mut room = ModelInstance::on_body_authored("map".into(), 1.0, Vec3f::default(), &frame);
    room.dynamic = false;
    let params = vec4(0.25, 0.5, 0.75, 1.0);
    let mut rooms = vec![room; 4];
    rooms[2].transform.v[12] += 2.0;
    let expected_pose = Mat4f::mul(&rooms[2].transform, &part.transform_at(0.5));
    rooms[2].layer_materials.push(LayerMaterialInstance {
        layer: "glass".into(),
        material: CustomMaterialInstance { name: "door_glass".into(), params },
    });

    let mut cx = Cx::new(Box::new(|_, _| {}));
    let (program, mut diffuse) = cx.with_vm(|vm| {
        vm.capture_errors();
        makepad_draw::script_mod(vm);
        vm.heap_mut().new_module(id!(prelude));
        script_eval!(vm, { mod.prelude.widgets_internal = {
            ..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw: mod.draw
        } });
        vm.heap_mut().new_module(id!(widgets));
        crate::custom_material::register(vm);
        let spec = script_eval!(vm, {
            use mod.prelude.widgets_internal.*
            {
                vertex: fn(p: vec3, n: vec3, uv: vec2) -> vec3 { return p + n * self.params.x }
                bounds_pad: 0.25
            }
        }).as_object().unwrap();
        let hooks = HookSet::from_spec(vm, spec, HookMask::ALL).unwrap();
        let program = DrawSceneCustom::build(
            vm, &MaterialDesc::default(), &hooks, HookMask::ALL, Vec4f::default(),
        ).unwrap();
        (program, DrawSceneSkinned::script_new_with_default(vm))
    });
    let shadow = program.shadow.expect("the door's vertex hook has a derived caster");
    let mut renderer = Renderer::default();
    renderer.pbr_materials_enabled = false;
    let prepared = PreparedStaticPreview::prepare(model).unwrap();
    let uploaded = renderer.upload_static_preview(&mut cx, prepared);
    let body_geometry = uploaded.geometry.geometry_id();
    let door_geometry = uploaded.anim_parts[0].draws[0].0.geometry_id();

    // Place the same doors as ordinary static layers before their program arrives.
    let mut fixed = uploaded.clone();
    fixed.extra_draws.extend(fixed.anim_parts.pop().unwrap().draws);
    let pad = Vec3f::all(program.bounds_pad);
    let padded = crate::lightmap::world_bounds(&rooms[2].transform, (fixed.min - pad, fixed.max + pad));
    renderer.install_uploaded_static("map", fixed.clone());
    renderer.set_models(rooms.clone());
    assert!(renderer.csm_static_casters.iter().all(|m| m.material.is_none()));
    assert_eq!(renderer.csm_static_casters.iter().filter(|m| m.geometry == door_geometry).count(), 4);
    assert!(renderer.install_custom_material("door_glass".into(), program));
    renderer.sync_csm_custom_casters();
    let (overridden, stock) = renderer.csm_static_casters.split_last().unwrap();
    let caster = overridden.material.as_ref().expect("the sync after an install rebuilds the already placed door's caster");
    assert_eq!(overridden.geometry, door_geometry);
    assert_eq!((caster.shader, caster.params), (shadow, params));
    assert_eq!(caster.texture.texture_id(), uploaded.anim_parts[0].draws[0].1.texture_id());
    assert_eq!((overridden.min, overridden.max), padded);
    assert!(stock.iter().all(|m| m.material.is_none()), "the other layers keep their stock casters");
    assert_eq!(stock.iter().filter(|m| m.geometry == door_geometry).count(), 3, "three doors are unbound");

    // A review camera borrows the host's programs only while it draws; its static door resolves them then.
    let mut review = renderer.fork_scene_for_review();
    assert!(review.csm_static_casters.iter().all(|m| m.material.is_none()), "a fresh fork holds no programs");
    review.swap_review_materials(&mut renderer);
    review.sync_csm_custom_casters();
    let borrowed = review.csm_static_casters.last().and_then(|m| m.material.as_ref()).map(|c| c.shader);
    assert_eq!(borrowed, Some(shadow), "a review camera's static door casts through the borrowed program");
    review.swap_review_materials(&mut renderer);
    renderer.sync_csm_custom_casters();
    assert!(renderer.csm_static_casters.last().unwrap().material.is_some(), "the host keeps its resolved casters");

    let blocks = renderer.csm_static_blocks.as_ptr();
    let edited_params = vec4(0.125, 0.5, 0.75, 1.0);
    renderer.rebuild_models(|copies| copies[2].layer_materials[0].material.params = edited_params);
    let edited = renderer.csm_static_casters.last().unwrap();
    assert_eq!(edited.geometry, door_geometry);
    assert_eq!(edited.material.as_ref().unwrap().params, edited_params, "the static door follows its current parameters");
    assert_eq!(renderer.csm_static_blocks.as_ptr(), blocks, "a parameter edit reuses the spatial blocks");

    renderer.rebuild_models(|copies| copies[2].layer_materials.clear());
    assert_eq!(renderer.csm_static_casters.iter().filter(|m| m.geometry == door_geometry && m.material.is_none()).count(), 4,
        "unbinding the static door restores its stock caster");
    renderer.set_models(rooms);
    renderer.install_uploaded_static("map", uploaded);
    renderer.model_anim_state = states;
    renderer.placed_blocks.refresh(&renderer.static_models, &renderer.placed_models, renderer.placed_scene_signature);

    let eye = Vec3f::default();
    let fog = (Vec3f::default(), 0.0);
    let sun = SunLight::default();
    let mut stats = RenderStats::default();
    // Two frames: in the first the program's lane builds its variant, whose
    // pipeline the HAL has not made yet; by the second it has.
    let made = super::stock_ibl::tests::Made::none();
    let mut last_frame = None;
    for frame in 0..2 {
        if frame == 1 {
            let variant = renderer.custom_material("door_glass").unwrap().built_shader(renderer.lane_features());
            made.pipeline(variant.expect("the first frame built the program's variant"));
        }
        renderer.prepare_custom_lanes(&mut cx);
        let pass = DrawPass::new(&mut cx);
        let mut list = DrawList::new(&mut cx);
        let stock_items = {
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut cx, &event);
            draw.begin_pass(&pass, Some(1.0));
            list.begin_always(&mut draw);
            let mut cx3d = Cx3d::new(&mut draw);
            let rooms = std::mem::take(&mut renderer.placed_models);
            renderer.draw_models_inner(
                &mut cx3d, ModelDraw::Diffuse(&mut diffuse), super::draw_models::LayerPhase::Opaque,
                eye, 1.0, &rooms, WorldModelLane::Placed, fog, &sun, None, &mut stats,
            );
            let items = &cx3d.draw_lists[list.id()].draw_items;
            let stock_items = items.len();
            let drawn = |geometry| -> usize {
                (0..stock_items).filter_map(|i| {
                    let item = &items[i];
                    let call = item.draw_call()?;
                    (call.geometry_id == Some(geometry))
                        .then(|| item.instances.as_ref().unwrap().len() / call.total_instance_slots)
                }).sum()
            };
            assert_eq!(drawn(body_geometry), rooms.len(), "frame {frame}: every room body draws through the stock lane");
            // One lane per layer: the stock lane draws the overridden glass
            // until the program's pipeline is made, then leaves it to it.
            assert_eq!(drawn(door_geometry), rooms.len() - frame, "frame {frame}: the stock lane draws the doors its program does not");

            renderer.draw_custom_models(
                &mut cx3d, eye, 1.0, &rooms, WorldModelLane::Placed, fog, &sun, None, &mut stats,
            );
            renderer.placed_models = rooms;
            assert_eq!(cx3d.draw_lists[list.id()].draw_items.len(), stock_items, "frame {frame}: glass waits for replay");
            // Another draw can use this program before the blended door replays.
            let program = renderer.custom_draws.get_mut("door_glass").unwrap();
            program.draw.set_params(&cx3d, Vec4f::default());
            renderer.draw_blended_models(&mut cx3d, &mut diffuse, eye, 1.0, fog, &sun, None, &mut stats);
            list.end(&mut draw);
            draw.end_pass(&pass);
            stock_items
        };
        if frame == 0 {
            assert_eq!(cx.draw_lists[list.id()].draw_items.len(), stock_items, "an unmade program draws none of its glass");
        }
        last_frame = Some((list, stock_items));
    }

    let (list, stock_items) = last_frame.unwrap();
    let items = &cx.draw_lists[list.id()].draw_items;
    assert_eq!(items.len(), stock_items + 1, "replay adds one draw for the glass");
    let item = &items[stock_items];
    let call = item.draw_call().unwrap();
    assert_eq!(call.geometry_id, Some(door_geometry));
    assert!(call.options.alpha_blend);
    let program = renderer.custom_material("door_glass").unwrap();
    assert_eq!(Some(call.draw_shader_id), program.draw.draw_vars.draw_shader_id);
    let data = item.instances.as_ref().unwrap();
    assert_eq!(data.len(), call.total_instance_slots, "the batch contains exactly one door");
    let layout = &cx.draw_shaders[call.draw_shader_id.index].mapping;
    let transform = layout.instances.find(live_id!(transform)).unwrap().offset;
    let submitted_pose = &data[transform..transform + 16];
    assert_eq!(submitted_pose, expected_pose.v.as_slice());
    let offset = layout.dyn_uniforms.find(live_id!(params)).unwrap().offset;
    assert_eq!(&call.dyn_uniforms[offset..offset + 4], &[params.x, params.y, params.z, params.w]);

    let world = World::default();
    let mut static_movers = Default::default();
    let mut movers = Vec::new();
    renderer.collect_lm_movers(&world, eye, 1.0, None, &mut static_movers, &mut movers);
    // The caster-log lookup is allocation-free on Unix when the variable is unset.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    if std::env::var_os("MAKEPAD_CSM_CASTER_LOG").is_none() {
        use crate::shaders::shader_registration_tests::allocations;
        let before = allocations::count();
        renderer.collect_lm_movers(&world, eye, 1.0, None, &mut static_movers, &mut movers);
        assert_eq!(allocations::count(), before, "collecting the same door casters allocated");
    }
    let mut derived = movers.iter().filter_map(|m| m.material.as_ref().map(|caster| (m, caster)));
    let (door, caster) = derived.next().expect("the overridden door casts through its vertex hook");
    assert!(derived.next().is_none(), "only the overridden door uses the derived caster");
    assert_eq!(door.geometry, door_geometry);
    assert_eq!(door.transform.v.as_slice(), submitted_pose);
    assert_eq!(caster.shader, shadow);
    assert_eq!(caster.params, params);

    renderer.install_uploaded_static("map", fixed);
    assert!(renderer.csm_static_casters.iter().any(|m| m.material.is_some()));
    renderer.retain_custom_materials(&[]);
    renderer.sync_csm_custom_casters();
    assert_eq!(renderer.csm_static_casters.iter().filter(|m| m.geometry == door_geometry && m.material.is_none()).count(), 4,
        "removing the program restores every static door's stock caster");
}

/// Slot numbers only mean anything against one placed list.
#[test]
fn a_changed_placed_scene_drops_slot_commands_but_keeps_model_ones() {
    let part = door();
    let mut renderer = Renderer::default();
    renderer.model_anim_state.set(ModelTarget::Model("map".into()), &part, "closed", 0.0);
    renderer.model_anim_state.set(ModelTarget::Instance(0), &part, "closed", 0.0);

    let mut transform = Mat4f::identity();
    transform.v[12] = 7.0;
    renderer.set_models(vec![ModelInstance {
        model: "map".to_string(),
        transform,
        tint: vec4(1.0, 1.0, 1.0, 1.0),
        color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
        dynamic: false,
        depth_order: 0.0,
        custom_material: None,
        layer_materials: Vec::new(),
        part_poses: Vec::new(),
        lamps: Default::default(),
    }]);
    assert!(renderer
        .model_anim_state
        .map
        .keys()
        .all(|(t, _)| matches!(t, StateTarget::Model(_))));
    assert_eq!(renderer.model_anim_state.map.len(), 1);
}

#[test]
fn model_instance_appearance_is_per_copy_and_neutral_by_default() {
    let bounds = (vec3f(-1.0, 0.0, -1.0), vec3f(1.0, 2.0, 1.0));
    let frame = Mat4f::identity();
    let neutral = ModelInstance::on_body("dog".into(), bounds, 1.0, 1.0, &frame);
    assert_eq!(neutral.tint, vec4(1.0, 1.0, 1.0, 1.0));
    assert_eq!(neutral.color_adjust, vec4(0.0, 1.0, 1.0, 0.0));

    let blue = neutral
        .clone()
        .with_tint(vec4(0.2, 0.5, 1.0, 1.0))
        .with_color_adjust(vec4(210.0, 1.1, 0.9, 0.0));
    assert_eq!(blue.tint, vec4(0.2, 0.5, 1.0, 1.0));
    assert_eq!(blue.color_adjust, vec4(210.0, 1.1, 0.9, 0.0));
    assert_eq!(neutral.tint, vec4(1.0, 1.0, 1.0, 1.0));
}

#[test]
fn authored_model_origin_and_front_follow_full_bank_pitch_frame() {
    let origin = vec3f(0.3, -0.7, 1.2);
    for angles in [vec3f(0.0,0.0,0.0), vec3f(0.31,1.1,-0.64)] {
        let mut frame = Mat4f::rotation(angles);
        frame.v[12] = 17.0; frame.v[13] = 28.0; frame.v[14] = -9.0;
        let instance = ModelInstance::on_body_authored("trainer".into(), 2.0, origin, &frame);
        for (model_point, body_point) in [
            (origin, vec3f(0.0,0.0,0.0)),
            (origin + vec3f(0.0,0.0,1.0),vec3f(0.0,0.0,-2.0)),
            (origin + vec3f(1.0,0.0,0.0),vec3f(-2.0,0.0,0.0)),
            (origin + vec3f(0.0,1.0,0.0),vec3f(0.0,2.0,0.0)),
        ] {
            let got = instance.transform.transform_vec4(vec4(model_point.x,model_point.y,model_point.z,1.0));
            let expected = frame.transform_vec4(vec4(body_point.x,body_point.y,body_point.z,1.0));
            assert!((got.x-expected.x).abs()<1e-5 && (got.y-expected.y).abs()<1e-5 && (got.z-expected.z).abs()<1e-5);
        }
    }
}

#[test]
fn model_targets_come_from_ids_and_slots() {
    assert_eq!(
        ModelTarget::from("maps/e1m1"),
        ModelTarget::Model("maps/e1m1".to_string())
    );
    assert_eq!(ModelTarget::from(4usize), ModelTarget::Instance(4));
}

/// An ordinary prop has no parts, and every query says so without
/// pretending the model is missing.
#[test]
fn a_model_without_parts_answers_empty() {
    let renderer = Renderer::default();
    assert!(renderer.model_anim_part_names("kit/lamp").is_empty());
    assert!(renderer.model_anim_part("kit/lamp", "door_1").is_none());
    assert!(renderer.anim_part_boxes().is_empty());
    assert!(renderer.model_states("kit/lamp").is_empty());
}
