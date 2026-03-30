use makepad_widgets;

use makepad_widgets::*;
use makepad_xr::scene::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Block = Cube{
        size: vec3(0.160, 0.082, 0.075)
        corner_radius: 0.018
        roughness: 0.28
        metallic: 0.02
    }

    let Platform = Cube{
        body: mod.widgets.XrBodyKind.Fixed
        size: vec3(1.45, 0.08, 0.44)
        corner_radius: 0.022
        roughness: 0.82
        metallic: 0.0
        color: #x2b3643
    }

    let TestPedestal = Cube{
        body: mod.widgets.XrBodyKind.Fixed
        size: vec3(0.28, 0.18, 0.28)
        corner_radius: 0.026
        roughness: 0.18
        metallic: 0.04
    }

    let XrUiButton = mod.widgets.ButtonFlat{
        draw_bg +: {
            border_size: 0.0
            border_radius: 10.0
            pixel: fn() {
                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled);
                return Pal.premul(fill)
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui:  XrRoot{
            window.inner_size: vec2(1400, 900)
            pass.clear_color: #x0b1118
            camera.fov_y: 52.0
            camera.distance: 1.8
            env.gravity: 9.8
            env.env_cube: true
            env.depth_mesh: false

            scene_select := XrSelect{
                pos: vec3(0.0, -0.02, -0.62)
                scale: vec3(0.5, 0.5, 0.5)
                active_child: @ico_shoot_scene

                test_scene := XrNode{
                    on_render: ||{
                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(1.65, 0.08, 1.18)
                            corner_radius: 0.04
                            roughness: 0.92
                            metallic: 0.0
                            color: #x243444
                            pos: vec3(0.42, -0.22, -0.72)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.18, 0.72, 1.16)
                            corner_radius: 0.04
                            roughness: 0.88
                            metallic: 0.0
                            color: #x1c2733
                            pos: vec3(1.20, 0.10, -0.72)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(1.62, 0.72, 0.18)
                            corner_radius: 0.04
                            roughness: 0.88
                            metallic: 0.0
                            color: #x1a2430
                            pos: vec3(0.42, 0.10, -1.22)
                        }

                        TestPedestal{
                            pos: vec3(0.05, -0.05, -0.76)
                            color: #xff6a4d
                        }

                        TestPedestal{
                            pos: vec3(0.42, 0.02, -0.76)
                            color: #x58d68d
                            size: vec3(0.24, 0.32, 0.24)
                        }

                        TestPedestal{
                            pos: vec3(0.78, -0.01, -0.76)
                            color: #x68a8ff
                            size: vec3(0.24, 0.26, 0.24)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.24, 0.24, 0.24)
                            corner_radius: 0.024
                            roughness: 0.12
                            metallic: 0.02
                            color: #xffff7a
                            pos: vec3(0.42, 0.34, -0.76)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.16, 0.82, 0.16)
                            corner_radius: 0.03
                            roughness: 0.22
                            metallic: 0.04
                            color: #xff8a54
                            pos: vec3(0.42, 0.34, -1.02)
                        }
                    }
                }

                block_scene := XrNode{
                    pos: vec3(0.0, -0.16, 0.0)
                    scale: vec3(0.62, 0.62, 0.62)
                    on_render: ||{
                        Platform{pos: vec3(0.05, -0.06, -0.10)}
                        for row in 0..20 {
                            for col in 0..8 {
                                let offset = if row % 2 == 0 {0.0} else {0.08}
                                let color = if (row + col) % 6 == 0 {
                                    #xff5a4f
                                } else if (row + col) % 6 == 1 {
                                    #x3ecf8e
                                } else if (row + col) % 6 == 2 {
                                    #x57a1ff
                                } else if (row + col) % 6 == 3 {
                                    #xffc857
                                } else if (row + col) % 6 == 4 {
                                    #xff8f3f
                                } else {
                                    #xd16dff
                                }
                                Block{
                                    pos: vec3(-0.46 + col * 0.16 + offset, 0.028 + row * 0.084, -0.10)
                                    color: color
                                }
                            }
                        }
                    }
                }

                ico_box_scene := XrNode{
                    pos: vec3(0.0, -0.18, 0.0)
                    scale: vec3(0.62, 0.62, 0.62)
                    on_render: ||{
                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.78, 0.08, 0.92)
                            corner_radius: 0.02
                            roughness: 0.92
                            metallic: 0.0
                            color: #x212c39
                            pos: vec3(0.05, -0.06, -0.10)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.06, 0.60, 0.92)
                            corner_radius: 0.02
                            roughness: 0.84
                            metallic: 0.0
                            color: #x19232e
                            pos: vec3(-0.31, 0.20, -0.10)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.06, 0.60, 0.92)
                            corner_radius: 0.02
                            roughness: 0.84
                            metallic: 0.0
                            color: #x19232e
                            pos: vec3(0.41, 0.20, -0.10)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.78, 0.60, 0.06)
                            corner_radius: 0.02
                            roughness: 0.84
                            metallic: 0.0
                            color: #x17202a
                            pos: vec3(0.05, 0.20, -0.53)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.94, 0.24, 0.06)
                            corner_radius: 0.02
                            roughness: 0.86
                            metallic: 0.0
                            color: #x223140
                            pos: vec3(0.05, 0.02, 0.33)
                        }

                        for layer in 0..4 {
                            for row in 0..8 {
                                for col in 0..5 {
                                    let diffuse = #xa0a4aa
                                    let color = if (col + row * 2 + layer * 3) % 6 == 0 {
                                        #xff6f59
                                    } else if (col + row * 2 + layer * 3) % 6 == 1 {
                                        #x46d39a
                                    } else if (col + row * 2 + layer * 3) % 6 == 2 {
                                        #x66a9ff
                                    } else if (col + row * 2 + layer * 3) % 6 == 3 {
                                        #xffc857
                                    } else if (col + row * 2 + layer * 3) % 6 == 4 {
                                        #xff8a4c
                                    } else {
                                        #xd58cff
                                    }
                                    IcoSphere{
                                        density: 0.75
                                        friction: 0.48
                                        restitution: 0.03
                                        radius: 0.040
                                        diffuse: diffuse
                                        color: color
                                        pos: vec3(-0.118 + col * 0.084, 0.04 + layer * 0.082, -0.436 + row * 0.084)
                                    }
                                }
                            }
                        }
                    }
                }

                ico_shoot_scene := Shooter{
                    pos: vec3(0.0, -0.16, 0.0)
                    scale: vec3(0.62, 0.62, 0.62)
                    projectile_emit_rate_hz: 14.0
                    projectile_emit_speed_mps: 15.0
                    on_render: ||{
                        Platform{
                            pos: vec3(0.05, -0.06, -0.12)
                            size: vec3(1.52, 0.08, 0.52)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(1.10, 0.52, 0.06)
                            corner_radius: 0.02
                            roughness: 0.86
                            metallic: 0.0
                            color: #x1c2834
                            pos: vec3(0.05, 0.20, -0.42)
                        }

                        for index in 0..80 {
                            let diffuse = #xa0a4aa
                            let color = if index % 6 == 0 {
                                #xff6f59
                            } else if index % 6 == 1 {
                                #x46d39a
                            } else if index % 6 == 2 {
                                #x66a9ff
                            } else if index % 6 == 3 {
                                #xffc857
                            } else if index % 6 == 4 {
                                #xff8a4c
                            } else {
                                #xd58cff
                            }
                            IcoSphere{
                                spawn_pool: true
                                density: 0.75
                                friction: 0.48
                                restitution: 0.04
                                radius: 0.080
                                diffuse: diffuse
                                color: color
                                pos: vec3(-2.4, -6.0 - index * 0.004, 0.0)
                            }
                        }
                    }
                }

                helmet_scene := XrNode{
                    on_render: ||{
                        Platform{pos: vec3(0.05, -0.06, -0.10)}
                        for row in 0..1 {
                            for col in 0..1 {
                                Gltf{
                                    body: mod.widgets.XrBodyKind.Dynamic
                                    physics_size: vec3(0.17, 0.21, 0.17)
                                    density: 0.9
                                    friction: 0.7
                                    restitution: 0.08
                                    pos: vec3(-0.23 + col * 0.22 + if row % 2 == 0 {0.0} else {0.08}, 0.08 + row * 0.22, -0.10)
                                    src: crate_resource("self://resources/DamagedHelmet.glb")
                                    mesh_scale: vec3(0.38, 0.38, 0.38)
                                    mesh_rotation: vec3(0.0, 1.5708, 0.0)
                                    mesh_position: vec3(0.0, 0.32, 0.0)
                                }
                            }
                        }
                    }
                }

                tree_scene := XrNode{
                    on_render: ||{
                        Platform{pos: vec3(0.05, -0.06, -0.10)}
                        fractal_tree := FractalTree{
                            body: mod.widgets.XrBodyKind.Fixed
                            physics_size: vec3(0.34, 0.92, 0.34)
                            pos: vec3(0.05, -0.02, -0.10)
                            scale: vec3(0.72, 0.72, 0.72)
                            child_scale: 0.57735026
                            length_scale_0: 0.60
                            length_scale_1: 1.78
                            length_scale_2: 1.88
                            length_scale_3: 0.97
                            length_scale_4: 1.03
                            length_scale_rest: 1.08
                            branch_split_angle: 0.58
                            branch_yaw_step: 2.0943952
                            branch_yaw_phase_step: 1.0471976
                        }
                    }
                }

                refraction_scene := XrNode{
                    on_render: ||{
                        Platform{pos: vec3(0.05, -0.06, -0.10)}
                        for row in 0..4 {
                            for col in 0..4 {
                                let offset = if row % 2 == 0 {0.0} else {0.06}
                                RefractiveCube{
                                    pos: vec3(-0.22 + col * 0.12 + offset, 0.05 + row * 0.11, -0.10)
                                    size: vec3(0.115, 0.105, 0.085)
                                    color: vec4(0.82, 0.93, 1.0, 0.12)
                                    focus_distance: 1.6
                                }
                            }
                        }
                    }
                }
            }
            xr_peer_sync := XrPeerSync{
                auto_alignment_enabled: true
            }

            control_strip := XrView{
                visible: false
                show_in_non_xr: true
                wrist_left: true
                logical_size: vec2(1220, 700)
                pixel_scale: 0.000215
                dpi_factor: 2.0
                SolidView{
                    width: Fill
                    height: Fill
                    flow: Down
                    padding: 16
                    spacing: 12
                    draw_bg.color: #x162331ee

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 10
                        align: Align{y: 0.5}

                        title := Label{
                            text: "XR Scene Picker"
                            draw_text.color: #xeff7ff
                            draw_text.text_style.font_size: 18.0
                        }

                        detail := Label{
                            width: Fill
                            text: "Quest scenes: 160 faceted icos, blocks, fingertip shooter, tree, refraction."
                            draw_text.color: #xb8c8d8
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8

                        test_scene_button := XrUiButton{
                            width: 88
                            text: "XR Test"
                            on_press: || ui.scene_select.test_scene()
                        }

                        ico_box_scene_button := XrUiButton{
                            width: 72
                            text: "Icos"
                            on_press: || ui.scene_select.ico_box_scene()
                        }

                        ico_shoot_scene_button := XrUiButton{
                            width: 84
                            text: "Shooter"
                            on_press: || ui.scene_select.ico_shoot_scene()
                        }

                        block_scene_button := XrUiButton{
                            width: 88
                            text: "Blocks"
                            on_press: || ui.scene_select.block_scene()
                        }

                        helmet_scene_button := XrUiButton{
                            width: 88
                            text: "Helmet"
                            on_press: || ui.scene_select.helmet_scene()
                        }

                        tree_scene_button := XrUiButton{
                            width: 88
                            text: "Tree"
                            on_press: || ui.scene_select.tree_scene()
                        }

                        refraction_scene_button := XrUiButton{
                            width: 104
                            text: "Refraction"
                            on_press: || ui.scene_select.refraction_scene()
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}

                        physics_scale_label := Label{
                            width: 104
                            text: "Physics Speed"
                            draw_text.color: #xe8f4ff
                        }

                        physics_scale_025_button := XrUiButton{
                            width: 58
                            text: "0.25"
                            on_press: || ui.root.set_physics_time_scale(0.25)
                        }

                        physics_scale_05_button := XrUiButton{
                            width: 58
                            text: "0.5"
                            on_press: || ui.root.set_physics_time_scale(0.5)
                        }

                        physics_scale_10_button := XrUiButton{
                            width: 58
                            text: "1.0"
                            on_press: || ui.root.set_physics_time_scale(1.0)
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 12
                        align: Align{y: 0.5}

                        depth_toggle_button := XrUiButton{
                            width: 132
                            text: "Toggle Env Mesh"
                            on_press: || ui.root.set_depth(!ui.root.depth_mesh_visible())
                        }

                        query_hits_toggle_button := XrUiButton{
                            width: 148
                            text: "Toggle Query Hits"
                            on_press: || ui.root.set_depth_query_hits(!ui.root.depth_query_hits_visible())
                        }

                        scene_status := Label{
                            width: Fill
                            text: "Default scene: 160 faceted icosahedra with sphere colliders. Shooter mode fires pooled icos from the main index fingertip."
                            draw_text.color: #xe8f4ff
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}

                        depth_resolution_label := Label{
                            width: 104
                            text: "Depth Voxel"
                            draw_text.color: #xe8f4ff
                        }

                        depth_resolution_3_button := XrUiButton{
                            width: 64
                            text: "3 cm"
                            on_press: || ui.root.set_depth_voxel_size(0.03)
                        }

                        depth_resolution_5_button := XrUiButton{
                            width: 64
                            text: "5 cm"
                            on_press: || ui.root.set_depth_voxel_size(0.05)
                        }

                        depth_resolution_10_button := XrUiButton{
                            width: 72
                            text: "10 cm"
                            on_press: || ui.root.set_depth_voxel_size(0.10)
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}

                        render_scale_label := Label{
                            width: 104
                            text: "Render Scale"
                            draw_text.color: #xe8f4ff
                        }

                        scale_08_button := XrUiButton{
                            width: 58
                            text: "0.8x"
                            on_press: || ui.root.set_render_scale(0.8)
                        }

                        scale_10_button := XrUiButton{
                            width: 58
                            text: "1.0x"
                            on_press: || ui.root.set_render_scale(1.0)
                        }

                        scale_12_button := XrUiButton{
                            width: 58
                            text: "1.2x"
                            on_press: || ui.root.set_render_scale(1.2)
                        }

                        scale_14_button := XrUiButton{
                            width: 58
                            text: "1.4x"
                            on_press: || ui.root.set_render_scale(1.4)
                        }

                        scale_15_button := XrUiButton{
                            width: 58
                            text: "1.5x"
                            on_press: || ui.root.set_render_scale(1.5)
                        }
                    }

                    View{
                        width: Fill
                        height: Fill
                        flow: Right
                        spacing: 12

                        View{
                            width: Fill{weight: 66.0}
                            height: Fill
                            flow: Down
                            spacing: 8

                            SolidView{
                                width: Fill
                                height: Fit
                                padding: Inset{left: 10 right: 10 top: 6 bottom: 6}
                                draw_bg.color: #x0d1824

                                debug_field := Label{
                                    width: Fill
                                    text: "Connected peers: 0"
                                    draw_text.color: #xe8f4ff
                                    draw_text.flow: Flow.Right{wrap: true}
                                }
                            }
                        }

                        View{
                            width: Fill{weight: 34.0}
                            height: Fill
                        }
                    }
                }
            }

            wrist_toggle := XrView{
                visible: false
                mode: mod.widgets.XrViewMode.StuckToWrist
                wrist_left: true
                logical_size: vec2(112, 104)
                pixel_scale: 0.00030
                dpi_factor: 1.6
                depth_scale: 120.0
                SolidView{
                    width: Fill
                    height: Fill
                    flow: Down
                    padding: 6
                    spacing: 4
                    draw_bg.color: #x0f1b27ee
                    draw_bg.border_radius: 18.0

                    wrist_menu_button := XrUiButton{
                        width: Fill
                        height: 40
                        text: "Menu"
                        draw_bg.border_radius: 12.0
                        on_press: || ui.control_strip.toggle_visible_next_to_wrist()
                    }

                    wrist_reset_button := XrUiButton{
                        width: Fill
                        height: 40
                        text: "Reset"
                        draw_bg.border_radius: 12.0
                        on_press: || ui.root.reset_physics()
                    }
                }
            }

            xr_permissions := mod.widgets.XrPermissionsFlow{}
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    network_started: bool,
    #[rust]
    last_debug_text: String,
    #[rust]
    suppress_activity_broadcast: Option<XrActivityId>,
}

impl App {
    fn current_activity(&self, cx: &mut Cx) -> Option<XrActivityId> {
        self.ui
            .widget(cx, ids!(scene_select))
            .borrow::<XrSelect>()
            .map(|select| select.activity_id())
    }

    fn active_scene_widget(&self, cx: &mut Cx) -> Option<WidgetRef> {
        self.ui
            .widget(cx, ids!(scene_select))
            .borrow::<XrSelect>()
            .and_then(|select| select.active_child_widget_ref())
    }

    fn apply_activity(&mut self, cx: &mut Cx, activity_id: XrActivityId) -> Option<WidgetRef> {
        self.ui
            .widget(cx, ids!(scene_select))
            .borrow_mut::<XrSelect>()
            .and_then(|mut select| select.set_activity(cx, activity_id))
    }

    fn ensure_network_started(&mut self, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        if let Some(mut peer_sync) = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow_mut::<XrPeerSync>()
        {
            peer_sync.set_enabled(cx, true);
            self.network_started = true;
        }
    }

    fn ensure_activity_announced(&mut self, cx: &mut Cx) {
        let Some(activity_id) = self.current_activity(cx) else {
            return;
        };
        if let Some(mut peer_sync) = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow_mut::<XrPeerSync>()
        {
            if peer_sync.enabled() && peer_sync.current_activity().is_none() {
                let _ = peer_sync.set_local_activity(cx, activity_id);
            }
        }
    }

    fn refresh_spawnable_registry(&mut self, cx: &mut Cx, force: bool) {
        let Some(activity_id) = self.current_activity(cx) else {
            return;
        };
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        let should_refresh = force
            || peer_sync_widget
                .borrow::<XrPeerSync>()
                .is_some_and(|peer_sync| peer_sync.spawnable_activity() != Some(activity_id));
        if !should_refresh {
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                peer_sync.flush_pending_shared_object_controls(cx);
            }
            return;
        }
        let Some(scene_widget) = self.active_scene_widget(cx) else {
            return;
        };
        let bindings = collect_scene_spawnable_objects(activity_id, &scene_widget);
        {
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                peer_sync.set_spawnable_objects(activity_id, bindings);
                peer_sync.flush_pending_shared_object_controls(cx);
            };
        }
    }

    fn apply_remote_body_spawn(&mut self, cx: &mut Cx, spawn: XrBodySpawn) {
        if let Some(mut root) = self.ui.borrow_mut::<XrRoot>() {
            root.spawn_body(cx, spawn);
        }
    }

    fn apply_remote_body_despawn(&mut self, cx: &mut Cx, widget_uid: WidgetUid) {
        if let Some(mut root) = self.ui.borrow_mut::<XrRoot>() {
            root.despawn_body(cx, widget_uid);
        }
    }

    fn apply_body_impulse(&mut self, cx: &mut Cx, impulse: XrBodyImpulse) {
        if let Some(mut root) = self.ui.borrow_mut::<XrRoot>() {
            root.apply_body_impulse(cx, impulse);
        }
    }

    fn publish_local_shared_object_states(&mut self, cx: &mut Cx) {
        let runtime_bodies = self.ui.borrow::<XrRoot>().map(|root| root.runtime_bodies());
        let Some(runtime_bodies) = runtime_bodies else {
            return;
        };
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        let maybe_peer_sync = peer_sync_widget.borrow_mut::<XrPeerSync>();
        if let Some(mut peer_sync) = maybe_peer_sync {
            peer_sync.publish_local_shared_object_states(cx, runtime_bodies.as_ref());
        }
    }

    fn refresh_debug_fields(&mut self, cx: &mut Cx) {
        let (
            surface_count,
            compute_ms,
            query_ms,
            rapier_ms,
            frame_cpu_ms,
            frame_update_cpu_ms,
            frame_draw_cpu_ms,
        ) = if let Some(root) = self.ui.borrow::<XrRoot>() {
            (
                root.physics_depth_query_surface_count(),
                root.physics_compute_ms(),
                root.physics_tsdf_query_ms(),
                root.physics_rapier_step_ms(),
                root.frame_cpu_ms(),
                root.frame_update_cpu_ms(),
                root.frame_draw_cpu_ms(),
            )
        } else {
            return;
        };
        let connected_peers = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .map(|peer_sync| {
                (
                    peer_sync.connected_peer_count(),
                    peer_sync.shared_object_count(),
                )
            })
            .unwrap_or((0, 0));
        let tsdf_memory_mb = cx
            .xr_tsdf()
            .latest_tsdf_snapshot()
            .as_ref()
            .map(|snapshot| {
                let grid = &snapshot.grid;
                grid.heap_bytes() as f64 / 1_000_000.0
            })
            .unwrap_or(0.0);
        let (depth_frames_seen, depth_frames_dropped) = cx
            .xr_tsdf()
            .state()
            .read()
            .ok()
            .map(|state| (state.stats.frames_seen, state.stats.frames_dropped))
            .unwrap_or((0, 0));
        let depth_frames_kept = depth_frames_seen.saturating_sub(depth_frames_dropped);
        let gpu_time_text = cx
            .xr_gpu_frame_time_ms()
            .map(|gpu_ms| format!("{gpu_ms:.2} ms"))
            .unwrap_or_else(|| "waiting".to_string());
        let debug_text = format!(
            "Connected peers: {}\nShared objects: {}\nPhysics planes: {surface_count}\nPhysics compute time: {compute_ms:.2} ms\nQuery time: {query_ms:.2} ms\nRapier time: {rapier_ms:.2} ms\nCPU frame time: {frame_cpu_ms:.2} ms\nUpdate time: {frame_update_cpu_ms:.2} ms\nDraw time: {frame_draw_cpu_ms:.2} ms\nTSDF size: {tsdf_memory_mb:.1} MB\nDepth frames kept: {depth_frames_kept}\nGPU time: {gpu_time_text}",
            connected_peers.0,
            connected_peers.1,
        );
        if self.last_debug_text != debug_text {
            self.ui
                .widget(cx, ids!(debug_field))
                .set_text(cx, &debug_text);
            self.last_debug_text = debug_text;
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let scene_select_uid = self.ui.widget(cx, ids!(scene_select)).widget_uid();
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        let peer_sync_uid = peer_sync_widget.widget_uid();

        let mut remote_activity = None;
        let mut remote_body_spawns = Vec::new();
        let mut remote_body_impulses = Vec::new();
        let mut remote_body_despawns = Vec::new();
        let mut local_activity = None;
        let mut local_body_spawns = Vec::new();
        let mut scene_changed = false;

        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            if widget_action.widget_uid == peer_sync_uid {
                match widget_action.cast::<XrPeerSyncAction>() {
                    XrPeerSyncAction::ActivityChanged(activity_id) => {
                        remote_activity = Some(activity_id);
                    }
                    XrPeerSyncAction::BodySpawn(spawn) => {
                        remote_body_spawns.push(spawn);
                    }
                    XrPeerSyncAction::BodyImpulse(impulse) => {
                        remote_body_impulses.push(impulse);
                    }
                    XrPeerSyncAction::BodyDespawn(widget_uid) => {
                        remote_body_despawns.push(widget_uid);
                    }
                    XrPeerSyncAction::None => {}
                }
            }
            if widget_action.widget_uid == scene_select_uid {
                if let XrSelectAction::ActiveChildChanged(activity_id) =
                    widget_action.cast::<XrSelectAction>()
                {
                    local_activity = Some(activity_id);
                }
            }
            if let Some(body_spawn) = widget_action.action.downcast_ref::<XrBodySpawn>() {
                local_body_spawns.push(*body_spawn);
            }
            if matches!(widget_action.cast::<XrNodeAction>(), XrNodeAction::SceneChanged) {
                scene_changed = true;
            }
        }

        if scene_changed {
            self.refresh_spawnable_registry(cx, true);
        }

        if let Some(activity_id) = remote_activity {
            if self.current_activity(cx) != Some(activity_id) {
                self.suppress_activity_broadcast = Some(activity_id);
                if self.apply_activity(cx, activity_id).is_none() {
                    self.suppress_activity_broadcast = None;
                }
            }
            self.refresh_spawnable_registry(cx, true);
        }

        if let Some(activity_id) = local_activity {
            self.refresh_spawnable_registry(cx, true);
            if self.suppress_activity_broadcast == Some(activity_id) {
                self.suppress_activity_broadcast = None;
            } else if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                let _ = peer_sync.set_local_activity(cx, activity_id);
            }
        }

        for widget_uid in remote_body_despawns {
            self.apply_remote_body_despawn(cx, widget_uid);
        }

        for spawn in remote_body_spawns {
            self.apply_remote_body_spawn(cx, spawn);
        }

        for impulse in remote_body_impulses {
            self.apply_body_impulse(cx, impulse);
        }

        if !local_body_spawns.is_empty() {
            self.refresh_spawnable_registry(cx, false);
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                for spawn in local_body_spawns {
                    let _ = peer_sync.send_local_body_spawn(spawn);
                }
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Startup) {
            self.ensure_network_started(cx);
        }
        self.ensure_activity_announced(cx);
        self.refresh_spawnable_registry(cx, false);
        if matches!(event, Event::XrUpdate(_))
            || (matches!(event, Event::NextFrame(_)) && !cx.in_xr_mode())
        {
            self.publish_local_shared_object_states(cx);
        }
        self.refresh_debug_fields(cx);
    }
}
