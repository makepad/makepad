#![allow(unexpected_cfgs)]
#![cfg(headless)]

use makepad_xr::makepad_widgets::*;
use makepad_xr::{net::*, scene::*};
use std::{
    cell::RefCell,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    rc::Rc,
    sync::mpsc,
    sync::Mutex,
    thread,
    time::Duration,
};

const TEST_DRAW_CYCLES: usize = 360;
const TEST_IO_TIMEOUT: Duration = Duration::from_secs(4);
const SYNTHETIC_DT: f64 = 1.0 / 60.0;
const HOLD_STABILIZE_FRAMES: usize = 16;
const SHARED_CUBE_START_POS: Vec3f = Vec3f {
    x: 0.0,
    y: 1.08,
    z: -0.62,
};
const SHARED_CUBE_GRAB_RADIUS: f32 = 0.07;
static UI_SHARED_OBJECT_TEST_LOCK: Mutex<()> = Mutex::new(());

fn shared_cube_activity_id() -> XrActivityId {
    XrActivityId(makepad_xr::live_id!(shared_cube_scene))
}

fn localhost_config(
    node_id: u64,
    discovery_port: u16,
    data_port: u16,
    sync_port: u16,
    discovery_targets: Vec<u16>,
) -> XrNetConfig {
    XrNetConfig {
        node_id: XrNetPeerId(node_id),
        discovery_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), discovery_port),
        data_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), data_port),
        sync_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), sync_port),
        discovery_targets: discovery_targets
            .into_iter()
            .map(|port| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
            .collect(),
        timing: XrNetTimingConfig {
            discovery_interval: Duration::from_millis(20),
            peer_timeout: Duration::from_millis(250),
            poll_interval: Duration::from_millis(5),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SharedCubeUiRole {
    Initiator,
    Acceptor,
}

#[derive(Clone, Debug)]
struct SharedCubeUiAppConfig {
    role: SharedCubeUiRole,
    net_config: XrNetConfig,
    expected_peers: usize,
    synthetic_frame_budget: usize,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct SharedCubeUiAppReport {
    role: SharedCubeUiRole,
    connected_peer_peak: usize,
    shared_object_peak: usize,
    remote_body_spawn_count: usize,
    remote_body_impulse_count: usize,
    remote_body_despawn_count: usize,
    cube_widget_uid: Option<WidgetUid>,
    cube_presence_count: usize,
    cube_hold_frame_count: usize,
    cube_hold_after_remote_frame_count: usize,
    cube_right_hand_hold_frame_count: usize,
    cube_held_hand_center_distance_peak: f32,
    cube_held_hand_surface_error_peak: f32,
    cube_held_pinch_anchor_distance_peak: f32,
    cube_held_pinch_anchor_surface_error_peak: f32,
    cube_release_observed: bool,
    cube_post_release_speed_peak: f32,
    cube_x_min: f32,
    cube_x_max: f32,
    cube_z_min: f32,
    cube_z_max: f32,
    cube_position_samples: Vec<Vec3f>,
    cube_linvel_samples: Vec<Vec3f>,
    physics_scene_body_count_peak: usize,
    physics_body_spawn_apply_count_peak: usize,
    physics_body_spawn_miss_count_peak: usize,
    physics_revision_peak: u64,
    scene_changed_count: usize,
    scene_changed_after_interaction_count: usize,
    spawnable_binding_count_peak: usize,
    active_scene_child_count_peak: usize,
    local_activity: Option<XrActivityId>,
    accepted_activity: Option<XrActivityId>,
    spawnable_activity: Option<XrActivityId>,
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(SharedCubeLoopbackApp::script_component(vm)){
        ui: XrRoot{
            window.inner_size: vec2(960, 720)
            pass.clear_color: #x11161d
            camera.fov_y: 48.0
            camera.distance: 2.6
            env.gravity: 0.0
            env.env_cube: false
            env.depth_mesh: false

            scene_select := XrSelect{
                active_child: @shared_cube_scene

                shared_cube_scene := XrNode{
                    on_render: ||{
                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(5.0, 0.18, 5.0)
                            pos: vec3(0.0, -1.05, -1.4)
                            corner_radius: 0.02
                            roughness: 0.92
                            metallic: 0.0
                            color: #x223140
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(5.0, 3.0, 0.18)
                            pos: vec3(0.0, 0.25, -4.2)
                            corner_radius: 0.02
                            roughness: 0.88
                            metallic: 0.0
                            color: #x182430
                        }

                        shared_cube := Cube{
                            body: mod.widgets.XrBodyKind.Dynamic
                            shared_object_policy: mod.widgets.XrSharedObjectPolicy.BootstrapShared
                            size: vec3(0.14, 0.14, 0.14)
                            pos: vec3(
                                #(SHARED_CUBE_START_POS.x),
                                #(SHARED_CUBE_START_POS.y),
                                #(SHARED_CUBE_START_POS.z)
                            )
                            density: 1.0
                            friction: 0.72
                            restitution: 0.02
                            roughness: 0.36
                            metallic: 0.02
                            color: #xffc868
                        }
                    }
                }
            }

            xr_peer_sync := XrPeerSync{
                auto_alignment_enabled: false
            }
        }
    }
}

#[derive(Script, ScriptHook)]
struct SharedCubeLoopbackApp {
    #[live]
    ui: WidgetRef,
    #[rust]
    network_started: bool,
    #[rust]
    suppress_activity_broadcast: Option<XrActivityId>,
    #[rust]
    config: Option<SharedCubeUiAppConfig>,
    #[rust]
    report_tx: Option<mpsc::Sender<SharedCubeUiAppReport>>,
    #[rust]
    connected_peer_peak: usize,
    #[rust]
    shared_object_peak: usize,
    #[rust]
    remote_body_spawn_count: usize,
    #[rust]
    remote_body_impulse_count: usize,
    #[rust]
    remote_body_despawn_count: usize,
    #[rust]
    cube_widget_uid: Option<WidgetUid>,
    #[rust]
    cube_presence_count: usize,
    #[rust]
    cube_hold_frame_count: usize,
    #[rust]
    cube_hold_after_remote_frame_count: usize,
    #[rust]
    cube_right_hand_hold_frame_count: usize,
    #[rust]
    cube_right_hand_hold_streak: usize,
    #[rust]
    cube_held_hand_center_distance_peak: f32,
    #[rust]
    cube_held_hand_surface_error_peak: f32,
    #[rust]
    cube_held_pinch_anchor_distance_peak: f32,
    #[rust]
    cube_held_pinch_anchor_surface_error_peak: f32,
    #[rust]
    cube_release_observed: bool,
    #[rust]
    cube_post_release_speed_peak: f32,
    #[rust]
    cube_x_min: f32,
    #[rust]
    cube_x_max: f32,
    #[rust]
    cube_z_min: f32,
    #[rust]
    cube_z_max: f32,
    #[rust]
    cube_position_samples: Vec<Vec3f>,
    #[rust]
    cube_linvel_samples: Vec<Vec3f>,
    #[rust]
    physics_scene_body_count_peak: usize,
    #[rust]
    physics_body_spawn_apply_count_peak: usize,
    #[rust]
    physics_body_spawn_miss_count_peak: usize,
    #[rust]
    physics_revision_peak: u64,
    #[rust]
    scene_changed_count: usize,
    #[rust]
    scene_changed_after_interaction_count: usize,
    #[rust]
    spawnable_binding_count_peak: usize,
    #[rust]
    active_scene_child_count_peak: usize,
    #[rust]
    synthetic_frames_started: bool,
    #[rust]
    synthetic_frame_index: usize,
    #[rust]
    synthetic_time_origin: Option<f64>,
    #[rust]
    last_injected_state: Option<Rc<XrState>>,
    #[rust]
    acceptor_remote_sync_frame: Option<usize>,
    #[rust]
    report_sent: bool,
}

impl SharedCubeLoopbackApp {
    fn install_test_config(
        &mut self,
        config: SharedCubeUiAppConfig,
        report_tx: mpsc::Sender<SharedCubeUiAppReport>,
    ) {
        self.config = Some(config);
        self.report_tx = Some(report_tx);
    }

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

    fn shared_cube_widget_uid(&mut self, cx: &mut Cx) -> Option<WidgetUid> {
        if self.cube_widget_uid.is_none() {
            let widget = self.ui.widget(cx, ids!(shared_cube));
            if !widget.is_empty() {
                self.cube_widget_uid = Some(widget.widget_uid());
            }
        }
        self.cube_widget_uid
    }

    fn ensure_network_started(&mut self, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        let config = self.config.clone();
        if let Some(mut peer_sync) = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow_mut::<XrPeerSync>()
        {
            if let Some(config) = config {
                peer_sync.set_net_config_override(config.net_config);
            }
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
        if let Some(scene_widget) = self.active_scene_widget(cx) {
            if let Some(node) = scene_widget.cast_inner::<XrNode>() {
                self.active_scene_child_count_peak =
                    self.active_scene_child_count_peak.max(node.child_count());
            }
        }
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
        self.spawnable_binding_count_peak = self.spawnable_binding_count_peak.max(bindings.len());
        let maybe_peer_sync = peer_sync_widget.borrow_mut::<XrPeerSync>();
        if let Some(mut peer_sync) = maybe_peer_sync {
            peer_sync.set_spawnable_objects(activity_id, bindings);
            peer_sync.flush_pending_shared_object_controls(cx);
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
        if let Some(mut peer_sync) = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow_mut::<XrPeerSync>()
        {
            peer_sync.publish_local_shared_object_states(cx, runtime_bodies.as_ref());
        }
    }

    fn sample_cube_body(
        &mut self,
        body: &XrRuntimeBodyState,
        right_hand_center: Option<Vec3f>,
        right_hand_pinch_anchor: Option<Vec3f>,
    ) {
        self.cube_presence_count = self.cube_presence_count.saturating_add(1);
        self.cube_x_min = self.cube_x_min.min(body.pose.position.x);
        self.cube_x_max = self.cube_x_max.max(body.pose.position.x);
        self.cube_z_min = self.cube_z_min.min(body.pose.position.z);
        self.cube_z_max = self.cube_z_max.max(body.pose.position.z);
        if body.held_by.is_some() {
            self.cube_hold_frame_count = self.cube_hold_frame_count.saturating_add(1);
            if self.remote_body_spawn_count > 0 {
                self.cube_hold_after_remote_frame_count =
                    self.cube_hold_after_remote_frame_count.saturating_add(1);
            }
            if body.held_by == Some(XrSharedHand::RightHand) {
                self.cube_right_hand_hold_frame_count =
                    self.cube_right_hand_hold_frame_count.saturating_add(1);
                self.cube_right_hand_hold_streak =
                    self.cube_right_hand_hold_streak.saturating_add(1);
                if self.cube_right_hand_hold_streak >= HOLD_STABILIZE_FRAMES {
                    if let Some(right_hand_center) = right_hand_center {
                        let center_distance = (body.pose.position - right_hand_center).length();
                        self.cube_held_hand_center_distance_peak = self
                            .cube_held_hand_center_distance_peak
                            .max(center_distance);
                        self.cube_held_hand_surface_error_peak = self
                            .cube_held_hand_surface_error_peak
                            .max((center_distance - SHARED_CUBE_GRAB_RADIUS).abs());
                    }
                    if let Some(right_hand_pinch_anchor) = right_hand_pinch_anchor {
                        let pinch_anchor_distance =
                            (body.pose.position - right_hand_pinch_anchor).length();
                        self.cube_held_pinch_anchor_distance_peak = self
                            .cube_held_pinch_anchor_distance_peak
                            .max(pinch_anchor_distance);
                        self.cube_held_pinch_anchor_surface_error_peak = self
                            .cube_held_pinch_anchor_surface_error_peak
                            .max((pinch_anchor_distance - SHARED_CUBE_GRAB_RADIUS).abs());
                    }
                }
            } else {
                self.cube_right_hand_hold_streak = 0;
            }
        } else if self.cube_hold_frame_count > 0 {
            self.cube_right_hand_hold_streak = 0;
            self.cube_release_observed = true;
            self.cube_post_release_speed_peak =
                self.cube_post_release_speed_peak.max(body.linvel.length());
        }
        let should_push_position = self
            .cube_position_samples
            .last()
            .is_none_or(|last| (*last - body.pose.position).length() > 0.015);
        if should_push_position {
            self.cube_position_samples.push(body.pose.position);
        }
        let should_push_linvel = self
            .cube_linvel_samples
            .last()
            .is_none_or(|last| (*last - body.linvel).length() > 0.05);
        if should_push_linvel {
            self.cube_linvel_samples.push(body.linvel);
        }
    }

    fn collect_runtime_observation(&mut self, cx: &mut Cx) {
        let cube_widget_uid = self.shared_cube_widget_uid(cx);
        let mut sampled_cube_body = None;
        let right_hand_center = self
            .last_injected_state
            .as_ref()
            .and_then(|state| state.right_hand.tracking_pose())
            .map(|pose| pose.position);
        let right_hand_pinch_anchor = self
            .last_injected_state
            .as_ref()
            .and_then(|state| state.right_hand.pinch_anchor_pose())
            .map(|pose| pose.position);
        if let Some(root) = self.ui.borrow::<XrRoot>() {
            let runtime_bodies = root.runtime_bodies();
            self.physics_scene_body_count_peak = self
                .physics_scene_body_count_peak
                .max(root.physics_scene_body_count());
            self.physics_body_spawn_apply_count_peak = self
                .physics_body_spawn_apply_count_peak
                .max(root.physics_body_spawn_apply_count());
            self.physics_body_spawn_miss_count_peak = self
                .physics_body_spawn_miss_count_peak
                .max(root.physics_body_spawn_miss_count());
            self.physics_revision_peak = self.physics_revision_peak.max(root.physics_revision());
            if let Some(cube_widget_uid) = cube_widget_uid {
                if let Some(body) = runtime_bodies.get(&cube_widget_uid) {
                    sampled_cube_body = Some(body.clone());
                }
            }
        }
        if let Some(body) = sampled_cube_body.as_ref() {
            self.sample_cube_body(body, right_hand_center, right_hand_pinch_anchor);
        }
        if let Some(peer_sync) = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
        {
            self.connected_peer_peak = self
                .connected_peer_peak
                .max(peer_sync.connected_peer_count());
            self.shared_object_peak = self.shared_object_peak.max(peer_sync.shared_object_count());
        }
    }

    fn current_cube_body_position(&mut self, cx: &mut Cx) -> Option<Vec3f> {
        let cube_widget_uid = self.shared_cube_widget_uid(cx)?;
        self.ui
            .borrow::<XrRoot>()
            .and_then(|root| root.runtime_bodies().get(&cube_widget_uid).cloned())
            .map(|body| body.pose.position)
    }

    fn ready_for_synthetic_frames(&self, cx: &mut Cx) -> bool {
        let Some(config) = self.config.as_ref() else {
            return false;
        };
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        let Some(peer_sync) = peer_sync_widget.borrow::<XrPeerSync>() else {
            return false;
        };
        peer_sync.connected_peer_count() >= config.expected_peers
            && self.current_activity(cx) == Some(shared_cube_activity_id())
            && peer_sync.spawnable_activity() == Some(shared_cube_activity_id())
    }

    fn inject_synthetic_xr_update(&mut self, cx: &mut Cx) {
        if self.report_sent || !self.ready_for_synthetic_frames(cx) {
            return;
        }
        let Some(config) = self.config.clone() else {
            return;
        };
        if matches!(config.role, SharedCubeUiRole::Acceptor)
            && self.acceptor_remote_sync_frame.is_none()
            && self.remote_body_spawn_count > 0
        {
            self.acceptor_remote_sync_frame = Some(self.synthetic_frame_index);
        }
        if !self.synthetic_frames_started {
            self.synthetic_frames_started = true;
            self.synthetic_time_origin = Some(Cx::time_now());
        }
        let time_origin = self.synthetic_time_origin.unwrap_or_else(Cx::time_now);
        let time = time_origin + self.synthetic_frame_index as f64 * SYNTHETIC_DT;
        let cube_target_position = self.current_cube_body_position(cx);
        let state = Rc::new(synthetic_shared_cube_state(
            time,
            config.role,
            self.synthetic_frame_index,
            self.acceptor_remote_sync_frame,
            cube_target_position,
        ));
        let last = self
            .last_injected_state
            .clone()
            .unwrap_or_else(|| state.clone());
        self.last_injected_state = Some(state.clone());
        self.synthetic_frame_index += 1;
        self.dispatch_event(cx, &Event::XrUpdate(XrUpdateEvent { state, last }));
        self.collect_runtime_observation(cx);

        if self.synthetic_frame_index >= config.synthetic_frame_budget {
            self.finish_report(cx);
        }
    }

    fn finish_report(&mut self, cx: &mut Cx) {
        if self.report_sent {
            return;
        }
        let Some(config) = self.config.as_ref() else {
            return;
        };
        let accepted_activity = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .and_then(|peer_sync| peer_sync.current_activity());
        let spawnable_activity = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .and_then(|peer_sync| peer_sync.spawnable_activity());
        let report = SharedCubeUiAppReport {
            role: config.role,
            connected_peer_peak: self.connected_peer_peak,
            shared_object_peak: self.shared_object_peak,
            remote_body_spawn_count: self.remote_body_spawn_count,
            remote_body_impulse_count: self.remote_body_impulse_count,
            remote_body_despawn_count: self.remote_body_despawn_count,
            cube_widget_uid: self.cube_widget_uid,
            cube_presence_count: self.cube_presence_count,
            cube_hold_frame_count: self.cube_hold_frame_count,
            cube_hold_after_remote_frame_count: self.cube_hold_after_remote_frame_count,
            cube_right_hand_hold_frame_count: self.cube_right_hand_hold_frame_count,
            cube_held_hand_center_distance_peak: self.cube_held_hand_center_distance_peak,
            cube_held_hand_surface_error_peak: self.cube_held_hand_surface_error_peak,
            cube_held_pinch_anchor_distance_peak: self.cube_held_pinch_anchor_distance_peak,
            cube_held_pinch_anchor_surface_error_peak: self
                .cube_held_pinch_anchor_surface_error_peak,
            cube_release_observed: self.cube_release_observed,
            cube_post_release_speed_peak: self.cube_post_release_speed_peak,
            cube_x_min: self.cube_x_min,
            cube_x_max: self.cube_x_max,
            cube_z_min: self.cube_z_min,
            cube_z_max: self.cube_z_max,
            cube_position_samples: self.cube_position_samples.clone(),
            cube_linvel_samples: self.cube_linvel_samples.clone(),
            physics_scene_body_count_peak: self.physics_scene_body_count_peak,
            physics_body_spawn_apply_count_peak: self.physics_body_spawn_apply_count_peak,
            physics_body_spawn_miss_count_peak: self.physics_body_spawn_miss_count_peak,
            physics_revision_peak: self.physics_revision_peak,
            scene_changed_count: self.scene_changed_count,
            scene_changed_after_interaction_count: self.scene_changed_after_interaction_count,
            spawnable_binding_count_peak: self.spawnable_binding_count_peak,
            active_scene_child_count_peak: self.active_scene_child_count_peak,
            local_activity: self.current_activity(cx),
            accepted_activity,
            spawnable_activity,
        };
        if let Some(report_tx) = &self.report_tx {
            let _ = report_tx.send(report);
        }
        self.report_sent = true;
        cx.quit();
    }

    fn debug_state(&mut self, cx: &mut Cx) -> String {
        let current_activity = self.current_activity(cx);
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        let peer_state = peer_sync_widget
            .borrow::<XrPeerSync>()
            .map(|peer_sync| {
                format!(
                    "connected={} accepted={:?} spawnable={:?} shared={}",
                    peer_sync.connected_peer_count(),
                    peer_sync.current_activity(),
                    peer_sync.spawnable_activity(),
                    peer_sync.shared_object_count(),
                )
            })
            .unwrap_or_else(|| "peer_sync=missing".to_string());
        format!(
            "network_started={} current_activity={current_activity:?} frames_started={} frame_index={} remote_spawns={} hold_frames={} hold_after_remote={} release={} release_speed_peak={:.3} cube_x=({:.3},{:.3}) cube_z=({:.3},{:.3}) {peer_state}",
            self.network_started,
            self.synthetic_frames_started,
            self.synthetic_frame_index,
            self.remote_body_spawn_count,
            self.cube_hold_frame_count,
            self.cube_hold_after_remote_frame_count,
            self.cube_release_observed,
            self.cube_post_release_speed_peak,
            self.cube_x_min,
            self.cube_x_max,
            self.cube_z_min,
            self.cube_z_max,
        )
    }

    fn dispatch_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Startup) {
            self.ensure_network_started(cx);
        }
        self.ensure_activity_announced(cx);
        self.refresh_spawnable_registry(cx, false);
        if matches!(event, Event::NextFrame(_)) {
            self.inject_synthetic_xr_update(cx);
        }
        if matches!(event, Event::XrUpdate(_)) {
            self.publish_local_shared_object_states(cx);
            self.collect_runtime_observation(cx);
        }
    }
}

impl MatchEvent for SharedCubeLoopbackApp {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let scene_select_uid = self.ui.widget(cx, ids!(scene_select)).widget_uid();
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        let peer_sync_uid = peer_sync_widget.widget_uid();

        let mut remote_activity = None;
        let mut remote_body_spawns = Vec::new();
        let mut remote_body_impulses = Vec::new();
        let mut remote_body_despawns = Vec::new();
        let mut local_activity = None;
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
                    XrPeerSyncAction::ActivityPoseReset(_) => {}
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
            if matches!(
                widget_action.cast::<XrNodeAction>(),
                XrNodeAction::SceneChanged
            ) {
                scene_changed = true;
            }
        }

        if scene_changed {
            self.scene_changed_count = self.scene_changed_count.saturating_add(1);
            if self.shared_object_peak > 0 || self.cube_hold_frame_count > 0 {
                self.scene_changed_after_interaction_count =
                    self.scene_changed_after_interaction_count.saturating_add(1);
            }
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

        self.remote_body_spawn_count += remote_body_spawns.len();
        self.remote_body_impulse_count += remote_body_impulses.len();
        self.remote_body_despawn_count += remote_body_despawns.len();

        for widget_uid in remote_body_despawns {
            self.apply_remote_body_despawn(cx, widget_uid);
        }
        for spawn in remote_body_spawns {
            self.apply_remote_body_spawn(cx, spawn);
        }
        for impulse in remote_body_impulses {
            self.apply_body_impulse(cx, impulse);
        }
    }
}

impl AppMain for SharedCubeLoopbackApp {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_xr::makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.dispatch_event(cx, event);
    }
}

fn lerp_vec3(a: Vec3f, b: Vec3f, t: f32) -> Vec3f {
    a + (b - a) * t.clamp(0.0, 1.0)
}

#[derive(Clone, Copy)]
struct SyntheticHandPlan {
    center: Vec3f,
    gripping: bool,
    dominant: bool,
}

fn initiator_hand_plan(frame: usize) -> Option<SyntheticHandPlan> {
    if frame < 15 {
        return None;
    }
    if frame < 70 {
        let t = (frame - 15) as f32 / (70 - 15) as f32;
        return Some(SyntheticHandPlan {
            center: lerp_vec3(
                SHARED_CUBE_START_POS + vec3f(0.0, 0.0, 0.03),
                vec3f(0.18, 1.15, -0.49),
                t,
            ),
            gripping: true,
            dominant: true,
        });
    }
    if frame < 102 {
        return Some(SyntheticHandPlan {
            center: vec3f(0.18, 1.15, -0.49),
            gripping: true,
            dominant: true,
        });
    }
    if frame < 112 {
        let t = (frame - 102) as f32 / (112 - 102) as f32;
        return Some(SyntheticHandPlan {
            center: lerp_vec3(vec3f(0.18, 1.15, -0.49), vec3f(0.28, 1.19, -0.31), t),
            gripping: false,
            dominant: true,
        });
    }
    if frame < 124 {
        return Some(SyntheticHandPlan {
            center: vec3f(0.28, 1.19, -0.31),
            gripping: false,
            dominant: true,
        });
    }
    None
}

fn acceptor_hand_plan(
    frame: usize,
    cube_target_position: Option<Vec3f>,
) -> Option<SyntheticHandPlan> {
    let cube_target_position = cube_target_position?;
    if frame < 4 {
        return None;
    }
    if frame < 14 {
        return Some(SyntheticHandPlan {
            center: cube_target_position + vec3f(0.0, 0.0, 0.03),
            gripping: false,
            dominant: true,
        });
    }
    if frame < 28 {
        return Some(SyntheticHandPlan {
            center: cube_target_position + vec3f(0.0, 0.0, 0.03),
            gripping: true,
            dominant: true,
        });
    }
    if frame < 72 {
        let t = (frame - 28) as f32 / (72 - 28) as f32;
        return Some(SyntheticHandPlan {
            center: lerp_vec3(
                cube_target_position + vec3f(0.0, 0.0, 0.03),
                vec3f(-0.22, 1.20, -0.43),
                t,
            ),
            gripping: true,
            dominant: true,
        });
    }
    if frame < 84 {
        let t = (frame - 72) as f32 / (84 - 72) as f32;
        return Some(SyntheticHandPlan {
            center: lerp_vec3(vec3f(-0.22, 1.20, -0.43), vec3f(-0.32, 1.24, -0.30), t),
            gripping: false,
            dominant: true,
        });
    }
    if frame < 100 {
        return Some(SyntheticHandPlan {
            center: vec3f(-0.32, 1.24, -0.30),
            gripping: false,
            dominant: true,
        });
    }
    None
}

fn synthetic_shared_cube_state(
    time: f64,
    role: SharedCubeUiRole,
    frame: usize,
    acceptor_remote_sync_frame: Option<usize>,
    cube_target_position: Option<Vec3f>,
) -> XrState {
    let mut state = XrState {
        time,
        head_pose: Pose::new(Quat::default(), vec3f(0.0, 1.6, 0.35)),
        ..Default::default()
    };
    let hand_plan = match role {
        SharedCubeUiRole::Initiator => initiator_hand_plan(frame),
        SharedCubeUiRole::Acceptor => acceptor_remote_sync_frame
            .map(|start_frame| frame.saturating_sub(start_frame))
            .and_then(|relative_frame| acceptor_hand_plan(relative_frame, cube_target_position)),
    };
    if let Some(hand_plan) = hand_plan {
        configure_grab_hand(&mut state.right_hand, hand_plan);
    }
    state
}

fn configure_grab_hand(hand: &mut XrHand, plan: SyntheticHandPlan) {
    let orientation = Quat::default();
    let pinch_anchor = plan.center;
    let palm_center = if plan.gripping {
        pinch_anchor + vec3f(0.0, 0.0, 0.060)
    } else {
        pinch_anchor
    };
    let wrist = palm_center + vec3f(0.0, -0.04, 0.055);
    let (
        thumb_knuckle1,
        thumb_knuckle2,
        index_knuckle1,
        index_knuckle2,
        index_knuckle3,
        index_tip_len,
    ) = if plan.gripping {
        let thumb_tip = pinch_anchor + vec3f(-0.028, 0.004, 0.0);
        let index_tip = pinch_anchor + vec3f(0.028, -0.004, 0.0);
        (
            palm_center + vec3f(-0.050, 0.004, 0.020),
            thumb_tip + vec3f(0.0, 0.0, 0.032),
            palm_center + vec3f(0.030, 0.004, 0.020),
            palm_center + vec3f(0.045, -0.006, -0.004),
            index_tip + vec3f(0.0, 0.0, 0.030),
            0.030,
        )
    } else {
        (
            palm_center + vec3f(-0.045, 0.001, -0.002),
            palm_center + vec3f(-0.037, 0.006, -0.028),
            palm_center + vec3f(0.024, 0.001, -0.010),
            palm_center + vec3f(0.022, 0.000, -0.036),
            palm_center + vec3f(0.020, -0.002, -0.060),
            0.036,
        )
    };

    hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
    if plan.dominant {
        hand.flags |= XrHand::DOMINANT_HAND;
    }
    if plan.gripping {
        hand.flags |= XrHand::PINCH_INDEX;
    }

    hand.joints[XrHand::CENTER] = Pose::new(orientation, palm_center);
    hand.joints[XrHand::WRIST] = Pose::new(orientation, wrist);

    hand.joints[XrHand::THUMB_BASE] =
        Pose::new(orientation, palm_center + vec3f(-0.034, -0.005, 0.012));
    hand.joints[XrHand::THUMB_KNUCKLE1] = Pose::new(orientation, thumb_knuckle1);
    hand.joints[XrHand::THUMB_KNUCKLE2] = Pose::new(orientation, thumb_knuckle2);

    hand.joints[XrHand::INDEX_BASE] =
        Pose::new(orientation, palm_center + vec3f(0.024, 0.000, 0.012));
    hand.joints[XrHand::INDEX_KNUCKLE1] = Pose::new(orientation, index_knuckle1);
    hand.joints[XrHand::INDEX_KNUCKLE2] = Pose::new(orientation, index_knuckle2);
    hand.joints[XrHand::INDEX_KNUCKLE3] = Pose::new(orientation, index_knuckle3);

    hand.joints[XrHand::MIDDLE_BASE] =
        Pose::new(orientation, palm_center + vec3f(0.008, 0.000, 0.008));
    hand.joints[XrHand::MIDDLE_KNUCKLE1] =
        Pose::new(orientation, palm_center + vec3f(0.008, 0.001, -0.015));
    hand.joints[XrHand::MIDDLE_KNUCKLE2] =
        Pose::new(orientation, palm_center + vec3f(0.007, 0.000, -0.042));
    hand.joints[XrHand::MIDDLE_KNUCKLE3] =
        Pose::new(orientation, palm_center + vec3f(0.006, -0.001, -0.066));

    hand.joints[XrHand::RING_BASE] =
        Pose::new(orientation, palm_center + vec3f(-0.008, 0.000, 0.006));
    hand.joints[XrHand::RING_KNUCKLE1] =
        Pose::new(orientation, palm_center + vec3f(-0.008, 0.001, -0.014));
    hand.joints[XrHand::RING_KNUCKLE2] =
        Pose::new(orientation, palm_center + vec3f(-0.009, 0.000, -0.038));
    hand.joints[XrHand::RING_KNUCKLE3] =
        Pose::new(orientation, palm_center + vec3f(-0.010, -0.001, -0.060));

    hand.joints[XrHand::LITTLE_BASE] =
        Pose::new(orientation, palm_center + vec3f(-0.023, -0.001, 0.006));
    hand.joints[XrHand::LITTLE_KNUCKLE1] =
        Pose::new(orientation, palm_center + vec3f(-0.024, 0.000, -0.012));
    hand.joints[XrHand::LITTLE_KNUCKLE2] =
        Pose::new(orientation, palm_center + vec3f(-0.026, -0.001, -0.032));
    hand.joints[XrHand::LITTLE_KNUCKLE3] =
        Pose::new(orientation, palm_center + vec3f(-0.028, -0.002, -0.050));

    hand.tips = [0.032, index_tip_len, 0.038, 0.034, 0.030];
    hand.tips_active = 0b1_1111;
    if plan.gripping {
        hand.tips_active |= XrHand::GRAB_ACTIVE;
        hand.pinch[XrHand::PINCH_STRENGTH_INDEX] = u8::MAX;
    } else {
        hand.pinch[XrHand::PINCH_STRENGTH_INDEX] = 0;
    }
    hand.aim_pose = Pose::new(orientation, palm_center + vec3f(0.0, 0.0, -0.075));
}

fn run_shared_cube_test_app(config: SharedCubeUiAppConfig) -> SharedCubeUiAppReport {
    let (report_tx, report_rx) = mpsc::channel();
    let app_ref = Rc::new(RefCell::new(None::<SharedCubeLoopbackApp>));
    let app_ref_closure = app_ref.clone();
    let config_closure = config.clone();
    let report_tx_closure = report_tx.clone();

    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if let Event::Startup = event {
            *app_ref_closure.borrow_mut() = Some(cx.with_vm(|vm| {
                let value = <SharedCubeLoopbackApp as AppMain>::script_mod(vm);
                let mut app = <SharedCubeLoopbackApp as ScriptNew>::script_from_value(vm, value);
                app.install_test_config(config_closure.clone(), report_tx_closure.clone());
                app
            }));
        }
        if let Some(app) = &mut *app_ref_closure.borrow_mut() {
            <dyn AppMain>::handle_event(app, cx, event);
        }
    }))));

    {
        let mut cx_ref = cx.borrow_mut();
        cx_ref.init_cx_os();
    }

    Cx::headless_no_draw_event_loop_for_draw_cycles(cx.clone(), TEST_DRAW_CYCLES);

    match report_rx.recv_timeout(TEST_IO_TIMEOUT) {
        Ok(report) => report,
        Err(_) => {
            let debug_state = {
                let mut cx_ref = cx.borrow_mut();
                let mut app_ref_borrow = app_ref.borrow_mut();
                if let Some(app) = app_ref_borrow.as_mut() {
                    app.debug_state(&mut cx_ref)
                } else {
                    "app missing after bounded headless loop".to_string()
                }
            };
            panic!(
                "shared-cube test app should report before the bounded headless loop exits: {debug_state}"
            );
        }
    }
}

#[test]
fn single_headless_shared_cube_app_grabs_moves_and_releases_cube() {
    let _guard = UI_SHARED_OBJECT_TEST_LOCK.lock().unwrap();
    let report = run_shared_cube_test_app(SharedCubeUiAppConfig {
        role: SharedCubeUiRole::Initiator,
        net_config: localhost_config(721, 47246, 47247, 47248, vec![]),
        expected_peers: 0,
        synthetic_frame_budget: 190,
    });

    assert_eq!(report.role, SharedCubeUiRole::Initiator);
    assert_eq!(report.local_activity, Some(shared_cube_activity_id()));
    assert_eq!(report.spawnable_activity, Some(shared_cube_activity_id()));
    assert!(report.spawnable_binding_count_peak >= 1, "{report:?}");
    assert!(report.physics_scene_body_count_peak >= 3, "{report:?}");
    assert_eq!(report.physics_body_spawn_miss_count_peak, 0, "{report:?}");
    assert!(report.shared_object_peak >= 1, "{report:?}");
    assert!(report.cube_presence_count >= 8, "{report:?}");
    assert!(report.cube_hold_frame_count >= 8, "{report:?}");
    assert!(report.cube_right_hand_hold_frame_count >= 8, "{report:?}");
    assert!(
        report.cube_held_pinch_anchor_surface_error_peak <= 0.04,
        "{report:?}"
    );
    assert!(
        report.cube_held_pinch_anchor_surface_error_peak < report.cube_held_hand_surface_error_peak,
        "{report:?}"
    );
    assert!(report.cube_release_observed, "{report:?}");
    assert!(report.cube_post_release_speed_peak >= 0.35, "{report:?}");
    assert!(
        report.cube_x_max >= SHARED_CUBE_GRAB_RADIUS * 1.8,
        "{report:?}"
    );
    assert!(report.cube_position_samples.len() >= 4, "{report:?}");
}

#[test]
fn two_headless_shared_cube_apps_take_over_and_release_cube_over_loopback() {
    let _guard = UI_SHARED_OBJECT_TEST_LOCK.lock().unwrap();
    let initiator_thread = thread::spawn(|| {
        run_shared_cube_test_app(SharedCubeUiAppConfig {
            role: SharedCubeUiRole::Initiator,
            net_config: localhost_config(731, 47346, 47347, 47348, vec![47356]),
            expected_peers: 1,
            synthetic_frame_budget: 220,
        })
    });
    let acceptor_thread = thread::spawn(|| {
        run_shared_cube_test_app(SharedCubeUiAppConfig {
            role: SharedCubeUiRole::Acceptor,
            net_config: localhost_config(732, 47356, 47357, 47358, vec![47346]),
            expected_peers: 1,
            synthetic_frame_budget: 220,
        })
    });

    let initiator_report = initiator_thread
        .join()
        .expect("initiator app thread should complete successfully");
    let acceptor_report = acceptor_thread
        .join()
        .expect("acceptor app thread should complete successfully");

    assert_eq!(initiator_report.role, SharedCubeUiRole::Initiator);
    assert_eq!(
        initiator_report.local_activity,
        Some(shared_cube_activity_id())
    );
    assert_eq!(
        initiator_report.spawnable_activity,
        Some(shared_cube_activity_id())
    );
    assert!(
        initiator_report.connected_peer_peak >= 1,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        initiator_report.shared_object_peak >= 1,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        initiator_report.cube_hold_frame_count >= 8,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        initiator_report.cube_held_pinch_anchor_surface_error_peak <= 0.04,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        initiator_report.cube_held_pinch_anchor_surface_error_peak
            < initiator_report.cube_held_hand_surface_error_peak,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        initiator_report.cube_release_observed,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        initiator_report.cube_position_samples.len() >= 4,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        initiator_report.remote_body_spawn_count >= 1,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert_eq!(
        initiator_report.physics_body_spawn_miss_count_peak, 0,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert_eq!(
        initiator_report.scene_changed_after_interaction_count, 0,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );

    assert_eq!(acceptor_report.role, SharedCubeUiRole::Acceptor);
    assert_eq!(
        acceptor_report.local_activity,
        Some(shared_cube_activity_id())
    );
    assert_eq!(
        acceptor_report.spawnable_activity,
        Some(shared_cube_activity_id())
    );
    assert!(
        acceptor_report.connected_peer_peak >= 1,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        acceptor_report.cube_held_pinch_anchor_surface_error_peak <= 0.09,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        acceptor_report.cube_held_pinch_anchor_surface_error_peak
            < acceptor_report.cube_held_hand_surface_error_peak,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        acceptor_report.shared_object_peak >= 1,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        acceptor_report.remote_body_spawn_count >= 1,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        acceptor_report.cube_hold_after_remote_frame_count >= 6,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        acceptor_report.cube_release_observed,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert!(
        acceptor_report.cube_position_samples.len() >= 8,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert_eq!(
        acceptor_report.physics_body_spawn_miss_count_peak, 0,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
    assert_eq!(
        acceptor_report.scene_changed_after_interaction_count, 0,
        "initiator={initiator_report:?} acceptor={acceptor_report:?}"
    );
}
