pub use makepad_widgets;

use makepad_widgets::makepad_platform::permission::{Permission, PermissionStatus};
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.XrSceneBase = #(XrScene::register_widget(vm))
    mod.widgets.XrScene = set_type_default() do mod.widgets.XrSceneBase{
        draw_cube +: {}
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.25
            spec_strength: 0.9
            env_intensity: 1.8
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 820)
                body +: {
                    phase_view := AdaptiveView{
                        width: Fill
                        height: Fill
                        retain_unused_variants: false

                        Preflight := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            align: Align{x: 0.5 y: 0.5}
                            padding: Inset{left: 36 right: 36 top: 36 bottom: 36}
                            spacing: 14
                            show_bg: true
                            draw_bg +: {
                                color_top: uniform(#x0b1422)
                                color_bottom: uniform(#x051018)
                                color_glow: uniform(#x1b4663)
                                pixel: fn() {
                                    let uv = self.pos;
                                    let base = mix(self.color_top, self.color_bottom, uv.y);
                                    let glow = smoothstep(0.72, 0.0, length(uv - vec2(0.18, 0.24)));
                                    return mix(base, self.color_glow, glow * 0.24);
                                }
                            }

                            panel := RoundedView{
                                width: 560
                                height: Fit
                                flow: Down
                                spacing: 10
                                padding: Inset{left: 22 right: 22 top: 20 bottom: 20}
                                draw_bg.color: #x09131cdd
                                draw_bg.radius: 16.0

                                title := H1{
                                    text: "XR Preflight"
                                    draw_text.color: #xeff7ff
                                }

                                detail_label := Label{
                                    width: Fill
                                    text: "Allow Quest scene access here before starting XR. The passthrough depth path uses Meta's scene permission for environment depth and occlusion."
                                    draw_text.color: #xb8c8d8
                                }

                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 10

                                    allow_button := Button{
                                        width: Fill
                                        text: "Allow Quest Scene Access"
                                    }

                                    start_xr_button := Button{
                                        width: Fill
                                        text: "Start XR"
                                    }
                                }

                                status_label := Label{
                                    width: Fill
                                    text: "Checking startup requirements."
                                    draw_text.color: #x8fe4d6
                                }
                            }
                        }

                        XrRuntime := View{
                            width: 0
                            height: 0
                        }
                    }
                }
            }

            xr_scene := mod.widgets.XrScene{}
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PulseTrail {
    pose: Pose,
    born_at: f64,
    length: f32,
    radius: f32,
    speed: f32,
    color: Vec4f,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AppPhase {
    #[default]
    Preflight,
    XrRuntime,
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrScene {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_cube: DrawCube,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,
    #[rust]
    pulses: Vec<PulseTrail>,
    #[rust]
    reference_cube_pose: Option<Pose>,
    #[rust(0.0)]
    last_emit_at: f64,
}

impl XrScene {
    const EMIT_INTERVAL: f64 = 0.045;
    const PULSE_TTL: f64 = 0.95;

    fn draw_pose_box(
        &mut self,
        cx: &mut Cx2d,
        pose: Pose,
        size: Vec3f,
        color: Vec4f,
        depth_clip: f32,
        _metallic: f32,
        _roughness: f32,
    ) {
        self.draw_cube.transform = pose.to_mat4();
        self.draw_cube.cube_pos = vec3(0.0, 0.0, 0.0);
        self.draw_cube.cube_size = size;
        self.draw_cube.color = color;
        self.draw_cube.depth_clip = depth_clip;
        self.draw_cube.draw(cx);
    }

    fn draw_forward_box(
        &mut self,
        cx: &mut Cx2d,
        pose: Pose,
        size: Vec3f,
        forward_offset: f32,
        color: Vec4f,
        depth_clip: f32,
        _metallic: f32,
        _roughness: f32,
    ) {
        self.draw_cube.transform = pose.to_mat4();
        self.draw_cube.cube_pos = vec3(0.0, 0.0, forward_offset);
        self.draw_cube.cube_size = size;
        self.draw_cube.color = color;
        self.draw_cube.depth_clip = depth_clip;
        self.draw_cube.draw(cx);
    }

    fn draw_anchor_markers(&mut self, cx: &mut Cx2d, anchor: XrAnchor) {
        self.draw_pose_box(
            cx,
            Pose::new(anchor.to_quat(), anchor.left),
            vec3(0.025, 0.025, 0.025),
            vec4(0.14, 0.62, 1.0, 1.0),
            1.0,
            0.10,
            0.42,
        );
        self.draw_pose_box(
            cx,
            Pose::new(anchor.to_quat_rev(), anchor.right),
            vec3(0.025, 0.025, 0.025),
            vec4(0.20, 1.0, 0.56, 1.0),
            1.0,
            0.10,
            0.42,
        );
    }

    fn ensure_reference_cube_pose(&mut self, state: &XrState) {
        if self.reference_cube_pose.is_some() {
            return;
        }

        let pose = Pose::new(
            state.head_pose.orientation,
            state.vec_in_head_space(vec3(0.0, -0.05, -1.15)),
        );
        log!(
            "XR debug cube spawned at ({:.2}, {:.2}, {:.2})",
            pose.position.x,
            pose.position.y,
            pose.position.z
        );
        self.reference_cube_pose = Some(pose);
    }

    fn draw_reference_cube(&mut self, cx: &mut Cx2d, state: &XrState) {
        let Some(pose) = self.reference_cube_pose else {
            return;
        };

        self.draw_pbr.set_use_pass_camera(true);
        self.draw_pbr.camera_pos = state.head_pose.position;
        self.draw_pbr.set_depth_write(true);
        self.draw_pbr.set_depth_clip(1.0);
        self.draw_pbr.set_base_color_texture(None);
        self.draw_pbr.set_metal_roughness_texture(None);
        self.draw_pbr.set_normal_texture(None);
        self.draw_pbr.set_occlusion_texture(None);
        self.draw_pbr.set_emissive_texture(None);
        let env_tex = self.draw_pbr.default_env_texture(cx);
        self.draw_pbr.set_env_texture(Some(env_tex));
        self.draw_pbr
            .set_base_color_factor(vec4(0.98, 0.24, 0.18, 1.0));
        self.draw_pbr.set_metal_roughness(0.0, 0.55);
        self.draw_pbr.set_transform(pose.to_mat4());
        let _ = self
            .draw_pbr
            .draw_rounded_cube(cx, vec3(0.08, 0.08, 0.08), 0.02, 1, 4);
    }

    fn draw_headset(&mut self, cx: &mut Cx2d, state: &XrState) {
        self.draw_pose_box(
            cx,
            state.head_pose,
            vec3(0.16, 0.10, 0.12),
            vec4(0.92, 0.95, 0.98, 1.0),
            0.0,
            0.08,
            0.56,
        );
        self.draw_forward_box(
            cx,
            state.head_pose,
            vec3(0.08, 0.045, 0.05),
            -0.06,
            vec4(0.12, 0.18, 0.22, 1.0),
            0.0,
            0.04,
            0.30,
        );
    }

    fn draw_hand(&mut self, cx: &mut Cx2d, hand: &XrHand, is_left: bool) {
        if !hand.in_view() {
            return;
        }

        let joint_color = if is_left {
            vec4(0.22, 0.78, 1.0, 1.0)
        } else {
            vec4(1.0, 0.68, 0.30, 1.0)
        };
        let tip_color = if is_left {
            vec4(0.42, 0.98, 1.0, 1.0)
        } else {
            vec4(1.0, 0.86, 0.44, 1.0)
        };

        for joint in &hand.joints {
            self.draw_pose_box(
                cx,
                *joint,
                vec3(0.011, 0.011, 0.016),
                joint_color,
                0.0,
                0.06,
                0.72,
            );
        }

        for (finger_index, knuckle_index) in XrHand::END_KNUCKLES.iter().enumerate() {
            if !hand.tip_active(finger_index) {
                continue;
            }
            let tip_len = hand.tips[finger_index].max(0.006);
            self.draw_forward_box(
                cx,
                hand.joints[*knuckle_index],
                vec3(0.007, 0.007, 0.018 + tip_len * 0.6),
                -0.014 - tip_len * 0.3,
                tip_color,
                0.0,
                0.02,
                0.20,
            );
        }
    }

    fn draw_controller(&mut self, cx: &mut Cx2d, controller: &XrController, color: Vec4f) {
        if !controller.active() && controller.trigger <= 0.05 && controller.grip <= 0.05 {
            return;
        }

        self.draw_pose_box(
            cx,
            controller.grip_pose,
            vec3(0.035, 0.035, 0.070),
            vec4(color.x * 0.7, color.y * 0.7, color.z * 0.7, 1.0),
            0.0,
            0.18,
            0.44,
        );
        self.draw_forward_box(
            cx,
            controller.aim_pose,
            vec3(0.009, 0.009, 0.22),
            -0.12,
            color,
            0.0,
            0.04,
            0.16,
        );
    }

    fn emit_pulses_from_state(&mut self, state: &XrState) {
        self.pulses.push(PulseTrail {
            pose: state.left_hand.joints[XrHand::INDEX_KNUCKLE3],
            born_at: state.time,
            length: 0.18,
            radius: 0.006,
            speed: 0.55,
            color: vec4(0.22, 0.88, 1.0, 1.0),
        });
        self.pulses.push(PulseTrail {
            pose: state.right_hand.joints[XrHand::INDEX_KNUCKLE3],
            born_at: state.time,
            length: 0.18,
            radius: 0.006,
            speed: 0.55,
            color: vec4(1.0, 0.72, 0.26, 1.0),
        });

        if state.left_controller.active() || state.left_controller.trigger > 0.4 {
            self.pulses.push(PulseTrail {
                pose: state.left_controller.aim_pose,
                born_at: state.time,
                length: 0.26,
                radius: 0.010,
                speed: 0.80,
                color: vec4(0.24, 0.78, 1.0, 1.0),
            });
        }
        if state.right_controller.active() || state.right_controller.trigger > 0.4 {
            self.pulses.push(PulseTrail {
                pose: state.right_controller.aim_pose,
                born_at: state.time,
                length: 0.26,
                radius: 0.010,
                speed: 0.80,
                color: vec4(1.0, 0.66, 0.22, 1.0),
            });
        }
    }

    fn draw_pulses(&mut self, cx: &mut Cx2d, now: f64) {
        for pulse in &self.pulses {
            let age = (now - pulse.born_at).max(0.0) as f32;
            let life = (1.0 - (age as f64 / Self::PULSE_TTL) as f32).max(0.0);
            if life <= 0.0 {
                continue;
            }

            let length = pulse.length * (0.55 + age * 1.8);
            let radius = (pulse.radius * (0.45 + life * 0.75)).max(0.0025);
            let color = vec4(
                pulse.color.x * (0.35 + life * 0.85),
                pulse.color.y * (0.35 + life * 0.85),
                pulse.color.z * (0.35 + life * 0.85),
                1.0,
            );

            self.draw_cube.transform = pulse.pose.to_mat4();
            self.draw_cube.cube_pos = vec3(0.0, 0.0, -0.05 - age * pulse.speed - length * 0.5);
            self.draw_cube.cube_size = vec3(radius, radius, length);
            self.draw_cube.color = color;
            self.draw_cube.depth_clip = 1.0;
            self.draw_cube.draw(cx);
        }
    }
}

impl Widget for XrScene {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::XrUpdate(e) = event {
            self.ensure_reference_cube_pose(&e.state);
            if e.state.time - self.last_emit_at >= Self::EMIT_INTERVAL {
                self.emit_pulses_from_state(&e.state);
                self.last_emit_at = e.state.time;
            }
            self.pulses
                .retain(|pulse| e.state.time - pulse.born_at <= Self::PULSE_TTL);
            self.redraw(cx);
        }
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, _scope: &mut Scope) -> DrawStep {
        let Some(state) = cx.draw_event.xr_state.as_ref() else {
            return DrawStep::done();
        };

        let cx = &mut Cx2d::new(cx.cx);

        self.draw_reference_cube(cx, state);
        self.draw_headset(cx, state);
        self.draw_hand(cx, &state.left_hand, true);
        self.draw_hand(cx, &state.right_hand, false);
        self.draw_controller(cx, &state.left_controller, vec4(0.24, 0.78, 1.0, 1.0));
        self.draw_controller(cx, &state.right_controller, vec4(1.0, 0.66, 0.22, 1.0));
        if let Some(anchor) = state.anchor {
            self.draw_anchor_markers(cx, anchor);
        }
        self.draw_pulses(cx, state.time);

        DrawStep::done()
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    phase: AppPhase,
    #[rust]
    scene_access: Option<PermissionStatus>,
    #[rust]
    pending_scene_access_check: Option<i32>,
    #[rust]
    pending_scene_access_request: Option<i32>,
    #[rust]
    ui_refresh_next_frame: Option<NextFrame>,
    #[rust]
    xr_start_next_frame: Option<NextFrame>,
}

impl App {
    fn is_android_preflight() -> bool {
        cfg!(target_os = "android")
    }

    fn scene_access_granted(&self) -> bool {
        !Self::is_android_preflight()
            || matches!(self.scene_access, Some(PermissionStatus::Granted))
    }

    fn phase_variant(&self) -> LiveId {
        match self.phase {
            AppPhase::Preflight => live_id!(Preflight),
            AppPhase::XrRuntime => live_id!(XrRuntime),
        }
    }

    fn apply_phase(&mut self, cx: &mut Cx) {
        let phase_variant = self.phase_variant();
        self.ui
            .adaptive_view(cx, ids!(phase_view))
            .set_variant_selector(move |_cx, _parent_size| phase_variant);
        cx.redraw_all();
    }

    fn schedule_ui_refresh(&mut self, cx: &mut Cx) {
        self.ui_refresh_next_frame = Some(cx.new_next_frame());
        cx.redraw_all();
    }

    fn allow_button_text(&self) -> &'static str {
        if self.pending_scene_access_check.is_some() {
            "Checking Quest Scene Access..."
        } else if self.pending_scene_access_request.is_some() {
            "Waiting for Quest Permission..."
        } else if matches!(self.scene_access, Some(PermissionStatus::Granted)) {
            "Re-check Quest Scene Access"
        } else {
            "Allow Quest Scene Access"
        }
    }

    fn detail_text(&self) -> &'static str {
        if !Self::is_android_preflight() {
            "This build can start XR directly from the splash screen."
        } else {
            match self.scene_access {
                Some(PermissionStatus::Granted) => {
                    "Quest scene access is granted. Start XR when you are ready."
                }
                Some(PermissionStatus::DeniedCanRetry) => {
                    "Quest scene access was denied. Use the allow button to ask again."
                }
                Some(PermissionStatus::DeniedPermanent) => {
                    "Quest scene access was denied again. Retry is still available here, but Android may require system settings before the dialog reappears."
                }
                Some(PermissionStatus::NotDetermined) | None => {
                    "Allow Quest scene access before starting XR. This unlocks environment depth and passthrough occlusion."
                }
            }
        }
    }

    fn status_text(&self) -> &'static str {
        if self.pending_scene_access_check.is_some() {
            "Checking current Quest permission status."
        } else if self.pending_scene_access_request.is_some() {
            "Approve the Quest permission dialog to continue."
        } else if !Self::is_android_preflight() {
            "XR is ready to launch from this splash screen."
        } else {
            match self.scene_access {
                Some(PermissionStatus::Granted) => "Quest scene access granted.",
                Some(PermissionStatus::DeniedCanRetry) => {
                    "Quest scene access denied. You can request it again."
                }
                Some(PermissionStatus::DeniedPermanent) => {
                    "Quest scene access denied. Retry may require Android settings."
                }
                Some(PermissionStatus::NotDetermined) | None => {
                    "Quest scene access has not been granted yet."
                }
            }
        }
    }

    fn refresh_preflight_ui(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight {
            return;
        }
        self.ui
            .label(cx, ids!(detail_label))
            .set_text(cx, self.detail_text());
        self.ui
            .label(cx, ids!(status_label))
            .set_text(cx, self.status_text());

        let allow_button = self.ui.button(cx, ids!(allow_button));
        allow_button.set_visible(cx, Self::is_android_preflight());
        allow_button.set_enabled(
            cx,
            Self::is_android_preflight()
                && self.pending_scene_access_check.is_none()
                && self.pending_scene_access_request.is_none(),
        );
        self.ui
            .widget(cx, ids!(allow_button))
            .set_text(cx, self.allow_button_text());

        self.ui
            .button(cx, ids!(start_xr_button))
            .set_enabled(cx, self.scene_access_granted());
    }

    fn begin_scene_access_check(&mut self, cx: &mut Cx) {
        if !Self::is_android_preflight() || self.pending_scene_access_check.is_some() {
            return;
        }
        self.pending_scene_access_check = Some(cx.check_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn request_scene_access(&mut self, cx: &mut Cx) {
        if !Self::is_android_preflight()
            || self.pending_scene_access_check.is_some()
            || self.pending_scene_access_request.is_some()
        {
            return;
        }
        self.pending_scene_access_request = Some(cx.request_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn begin_xr_runtime(&mut self, cx: &mut Cx) {
        if self.phase == AppPhase::XrRuntime {
            return;
        }
        self.phase = AppPhase::XrRuntime;
        self.apply_phase(cx);
        self.xr_start_next_frame = Some(cx.new_next_frame());
    }

    fn maybe_start_xr_on_ready(&mut self, cx: &mut Cx) -> bool {
        if self.phase != AppPhase::Preflight || !self.scene_access_granted() {
            return false;
        }
        self.begin_xr_runtime(cx);
        true
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.phase = AppPhase::Preflight;
        if !Self::is_android_preflight() {
            self.scene_access = Some(PermissionStatus::Granted);
            self.maybe_start_xr_on_ready(cx);
            return;
        }
        self.apply_phase(cx);
        self.schedule_ui_refresh(cx);
        self.begin_scene_access_check(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(allow_button)).clicked(actions) {
            self.request_scene_access(cx);
        }

        if self.ui.button(cx, ids!(start_xr_button)).clicked(actions) && self.scene_access_granted()
        {
            self.begin_xr_runtime(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::NextFrame(ne) => {
                if self
                    .ui_refresh_next_frame
                    .is_some_and(|next_frame| ne.set.contains(&next_frame))
                {
                    self.ui_refresh_next_frame = None;
                    self.refresh_preflight_ui(cx);
                }

                if self
                    .xr_start_next_frame
                    .is_some_and(|next_frame| ne.set.contains(&next_frame))
                {
                    self.xr_start_next_frame = None;
                    cx.xr_start_presenting();
                }
            }
            Event::PermissionResult(result) if result.permission == Permission::SceneAccess => {
                if self.pending_scene_access_check == Some(result.request_id) {
                    self.pending_scene_access_check = None;
                } else if self.pending_scene_access_request == Some(result.request_id) {
                    self.pending_scene_access_request = None;
                } else {
                    return;
                }
                self.scene_access = Some(result.status);
                if !self.maybe_start_xr_on_ready(cx) {
                    self.schedule_ui_refresh(cx);
                }
            }
            Event::Resume => {
                if Self::is_android_preflight() && self.pending_scene_access_request.is_none() {
                    self.begin_scene_access_check(cx);
                }
            }
            _ => {}
        }
    }
}
