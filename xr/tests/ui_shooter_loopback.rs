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

const TEST_DRAW_CYCLES: usize = 320;
const TEST_DRAW_CYCLE_SLACK: usize = 120;
// Peer discovery is wall-clock driven, while the no-draw headless loop is cycle bounded.
// Leave generous real-time slack for loopback networking so the suite stays stable when warm.
const TEST_DRAW_CYCLE_NETWORK_SLACK: usize = 1500;
const TEST_IO_TIMEOUT: Duration = Duration::from_secs(4);
const SYNTHETIC_DT: f64 = 1.0 / 60.0;
static UI_SHOOTER_TEST_LOCK: Mutex<()> = Mutex::new(());

fn shooter_activity_id() -> XrActivityId {
    XrActivityId(makepad_xr::live_id!(ico_shoot_scene))
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
enum ShooterUiRole {
    Emitter,
    Receiver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShooterGestureProfile {
    TipTrackedPoint,
    AimPointWithoutTipBit,
    ReopenPointWithStickyGrabBit,
    LongHeldPoint,
    AlternatingHandsWithStickyGrabBit,
    RepeatedSparseCloseOpen,
}

#[derive(Clone, Debug)]
struct ShooterUiAppConfig {
    role: ShooterUiRole,
    gesture_profile: ShooterGestureProfile,
    net_config: XrNetConfig,
    expected_peers: usize,
    inject_local_xr_updates: bool,
    local_back_wall_visible: bool,
    synthetic_frame_budget: usize,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct ShooterUiAppReport {
    role: ShooterUiRole,
    connected_peer_peak: usize,
    clock_synced_peer_peak: usize,
    clock_ping_tx_count_peak: u64,
    clock_ping_rx_count_peak: u64,
    clock_pong_tx_count_peak: u64,
    clock_pong_rx_count_peak: u64,
    non_xr_draw_clock_count_peak: u64,
    local_spawn_count: usize,
    remote_spawn_count: usize,
    local_spawn_tx_count: usize,
    local_spawn_tx_fail_count: usize,
    peer_sync_tx_body_spawn_count_peak: u64,
    peer_sync_rx_body_spawn_count_peak: u64,
    peer_sync_tx_shared_object_state_count_peak: u64,
    peer_sync_rx_shared_object_state_count_peak: u64,
    remote_shadow_apply_count_peak: u64,
    pending_shared_object_control_count_peak: usize,
    last_network_event: String,
    shared_object_peak: usize,
    active_projectile_peak: usize,
    tracked_projectile_widget_uid: Option<WidgetUid>,
    tracked_projectile_presence_count: usize,
    runtime_body_count_peak: usize,
    runtime_body_widget_samples: Vec<WidgetUid>,
    physics_scene_body_count_peak: usize,
    physics_body_spawn_apply_count_peak: usize,
    physics_body_spawn_miss_count_peak: usize,
    physics_revision_peak: u64,
    scene_changed_count: usize,
    scene_changed_after_spawn_count: usize,
    spawnable_binding_count_peak: usize,
    spawnable_widget_samples: Vec<WidgetUid>,
    active_scene_child_count_peak: usize,
    projectile_z_samples: Vec<f32>,
    projectile_linvel_z_samples: Vec<f32>,
    local_activity: Option<XrActivityId>,
    accepted_activity: Option<XrActivityId>,
    spawnable_activity: Option<XrActivityId>,
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(ShooterLoopbackApp::script_component(vm)){
        ui: XrRoot{
            window.inner_size: vec2(960, 720)
            pass.clear_color: #x0b1118
            camera.fov_y: 48.0
            camera.distance: 2.6
            env.gravity: 9.8
            env.env_cube: false
            env.depth_mesh: false

            scene_select := XrSelect{
                active_child: @ico_shoot_scene

                ico_shoot_scene := Shooter{
                    projectile_emit_rate_hz: 14.0
                    projectile_emit_speed_mps: 12.0
                    on_render: ||{
                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(5.0, 0.18, 5.0)
                            pos: vec3(0.0, -1.05, -1.4)
                            corner_radius: 0.02
                            roughness: 0.92
                            metallic: 0.0
                            color: #x213140
                        }

                        back_wall := Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(5.0, 3.0, 0.18)
                            pos: vec3(0.0, 0.25, -4.2)
                            corner_radius: 0.02
                            roughness: 0.86
                            metallic: 0.0
                            restitution: 0.88
                            color: #x182430
                        }

                        for index in 0..24 {
                            IcoSphere{
                                spawn_pool: true
                                shared_object_policy: mod.widgets.XrSharedObjectPolicy.PooledOnDemand
                                density: 0.75
                                friction: 0.48
                                restitution: 0.04
                                radius: 0.075
                                diffuse: #xa0a4aa
                                color: #x66a9ff
                                pos: vec3(-20.0, -20.0 - index * 0.2, -20.0)
                            }
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
struct ShooterLoopbackApp {
    #[live]
    ui: WidgetRef,
    #[rust]
    network_started: bool,
    #[rust]
    suppress_activity_broadcast: Option<XrActivityId>,
    #[rust]
    config: Option<ShooterUiAppConfig>,
    #[rust]
    report_tx: Option<mpsc::Sender<ShooterUiAppReport>>,
    #[rust]
    local_scene_overrides_applied: bool,
    #[rust]
    local_spawn_count: usize,
    #[rust]
    remote_spawn_count: usize,
    #[rust]
    local_spawn_tx_count: usize,
    #[rust]
    local_spawn_tx_fail_count: usize,
    #[rust]
    peer_sync_tx_body_spawn_count_peak: u64,
    #[rust]
    peer_sync_rx_body_spawn_count_peak: u64,
    #[rust]
    peer_sync_tx_shared_object_state_count_peak: u64,
    #[rust]
    peer_sync_rx_shared_object_state_count_peak: u64,
    #[rust]
    remote_shadow_apply_count_peak: u64,
    #[rust]
    pending_shared_object_control_count_peak: usize,
    #[rust]
    last_network_event: String,
    #[rust]
    connected_peer_peak: usize,
    #[rust]
    clock_synced_peer_peak: usize,
    #[rust]
    clock_ping_tx_count_peak: u64,
    #[rust]
    clock_ping_rx_count_peak: u64,
    #[rust]
    clock_pong_tx_count_peak: u64,
    #[rust]
    clock_pong_rx_count_peak: u64,
    #[rust]
    non_xr_draw_clock_count_peak: u64,
    #[rust]
    shared_object_peak: usize,
    #[rust]
    active_projectile_peak: usize,
    #[rust]
    tracked_projectile_widget_uid: Option<WidgetUid>,
    #[rust]
    tracked_projectile_presence_count: usize,
    #[rust]
    runtime_body_count_peak: usize,
    #[rust]
    runtime_body_widget_samples: Vec<WidgetUid>,
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
    scene_changed_after_spawn_count: usize,
    #[rust]
    spawnable_binding_count_peak: usize,
    #[rust]
    spawnable_widget_samples: Vec<WidgetUid>,
    #[rust]
    active_scene_child_count_peak: usize,
    #[rust]
    projectile_z_samples: Vec<f32>,
    #[rust]
    projectile_linvel_z_samples: Vec<f32>,
    #[rust]
    synthetic_frames_started: bool,
    #[rust]
    synthetic_frame_index: usize,
    #[rust]
    synthetic_time_origin: Option<f64>,
    #[rust]
    last_injected_state: Option<Rc<XrState>>,
    #[rust]
    report_sent: bool,
}

impl ShooterLoopbackApp {
    fn install_test_config(
        &mut self,
        config: ShooterUiAppConfig,
        report_tx: mpsc::Sender<ShooterUiAppReport>,
    ) {
        self.config = Some(config);
        self.report_tx = Some(report_tx);
        self.local_scene_overrides_applied = false;
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

    fn apply_local_scene_overrides(&mut self, cx: &mut Cx) {
        if self.local_scene_overrides_applied {
            return;
        }
        let visible = self
            .config
            .as_ref()
            .map(|config| config.local_back_wall_visible)
            .unwrap_or(true);
        let mut back_wall = self
            .ui
            .widget(cx, ids!(scene_select.ico_shoot_scene.back_wall));
        if back_wall.is_empty() {
            let Some(scene_widget) = self.active_scene_widget(cx) else {
                return;
            };
            back_wall = scene_widget.widget(cx, ids!(back_wall));
        }
        if back_wall.is_empty() {
            return;
        }
        back_wall.set_visible(cx, visible);
        if let Some(mut root) = self.ui.borrow_mut::<XrRoot>() {
            root.force_scene_rebuild(cx);
        }
        self.local_scene_overrides_applied = true;
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
        if self.spawnable_widget_samples.len() < 8 {
            for binding in &bindings {
                if self.spawnable_widget_samples.contains(&binding.widget_uid) {
                    continue;
                }
                self.spawnable_widget_samples.push(binding.widget_uid);
                if self.spawnable_widget_samples.len() >= 8 {
                    break;
                }
            }
        }
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

    fn collect_runtime_observation(&mut self, cx: &mut Cx) {
        if let Some(root) = self.ui.borrow::<XrRoot>() {
            let runtime_bodies = root.runtime_bodies();
            self.runtime_body_count_peak = self.runtime_body_count_peak.max(runtime_bodies.len());
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
            if self.runtime_body_widget_samples.len() < 8 {
                for widget_uid in runtime_bodies.keys().copied() {
                    if self.runtime_body_widget_samples.contains(&widget_uid) {
                        continue;
                    }
                    self.runtime_body_widget_samples.push(widget_uid);
                    if self.runtime_body_widget_samples.len() >= 8 {
                        break;
                    }
                }
            }
            let active_projectile_positions: Vec<Vec3f> = runtime_bodies
                .values()
                .map(|body| body.pose.position)
                .filter(|pos| pos.x.abs() < 4.0 && pos.y > -4.0 && pos.z > -8.0)
                .collect();
            self.active_projectile_peak = self
                .active_projectile_peak
                .max(active_projectile_positions.len());
            if let Some(widget_uid) = self.tracked_projectile_widget_uid {
                if let Some(body) = runtime_bodies.get(&widget_uid) {
                    self.tracked_projectile_presence_count += 1;
                    let tracked_z = body.pose.position.z;
                    let tracked_linvel_z = body.linvel.z;
                    let should_push_z = self
                        .projectile_z_samples
                        .last()
                        .is_none_or(|last| (last - tracked_z).abs() > 0.01);
                    if should_push_z {
                        self.projectile_z_samples.push(tracked_z);
                    }
                    let should_push_linvel = self
                        .projectile_linvel_z_samples
                        .last()
                        .is_none_or(|last| (last - tracked_linvel_z).abs() > 0.01);
                    if should_push_linvel {
                        self.projectile_linvel_z_samples.push(tracked_linvel_z);
                    }
                }
            } else if let Some(max_z) = active_projectile_positions
                .iter()
                .map(|pos| pos.z)
                .max_by(|a, b| a.total_cmp(b))
            {
                let should_push = self
                    .projectile_z_samples
                    .last()
                    .is_none_or(|last| (last - max_z).abs() > 0.01);
                if should_push {
                    self.projectile_z_samples.push(max_z);
                }
            }
        }
        if let Some(peer_sync) = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
        {
            self.connected_peer_peak = self
                .connected_peer_peak
                .max(peer_sync.connected_peer_count());
            self.clock_synced_peer_peak = self
                .clock_synced_peer_peak
                .max(peer_sync.clock_synced_peer_count());
            self.clock_ping_tx_count_peak = self
                .clock_ping_tx_count_peak
                .max(peer_sync.clock_ping_tx_count());
            self.clock_ping_rx_count_peak = self
                .clock_ping_rx_count_peak
                .max(peer_sync.clock_ping_rx_count());
            self.clock_pong_tx_count_peak = self
                .clock_pong_tx_count_peak
                .max(peer_sync.clock_pong_tx_count());
            self.clock_pong_rx_count_peak = self
                .clock_pong_rx_count_peak
                .max(peer_sync.clock_pong_rx_count());
            self.non_xr_draw_clock_count_peak = self
                .non_xr_draw_clock_count_peak
                .max(peer_sync.non_xr_draw_clock_count());
            self.shared_object_peak = self.shared_object_peak.max(peer_sync.shared_object_count());
            self.peer_sync_tx_body_spawn_count_peak = self
                .peer_sync_tx_body_spawn_count_peak
                .max(peer_sync.tx_body_spawn_count());
            self.peer_sync_rx_body_spawn_count_peak = self
                .peer_sync_rx_body_spawn_count_peak
                .max(peer_sync.rx_body_spawn_count());
            self.peer_sync_tx_shared_object_state_count_peak = self
                .peer_sync_tx_shared_object_state_count_peak
                .max(peer_sync.tx_shared_object_state_count());
            self.peer_sync_rx_shared_object_state_count_peak = self
                .peer_sync_rx_shared_object_state_count_peak
                .max(peer_sync.rx_shared_object_state_count());
            self.remote_shadow_apply_count_peak = self
                .remote_shadow_apply_count_peak
                .max(peer_sync.remote_shadow_apply_count());
            self.pending_shared_object_control_count_peak = self
                .pending_shared_object_control_count_peak
                .max(peer_sync.pending_shared_object_control_count());
            self.last_network_event = peer_sync.last_network_event_label().to_string();
        }
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
            && self.current_activity(cx) == Some(shooter_activity_id())
            && peer_sync.spawnable_activity() == Some(shooter_activity_id())
    }

    fn inject_synthetic_xr_update(&mut self, cx: &mut Cx) {
        if self.report_sent || !self.ready_for_synthetic_frames(cx) {
            return;
        }
        let Some(config) = self.config.clone() else {
            return;
        };
        if !self.synthetic_frames_started {
            self.synthetic_frames_started = true;
            self.synthetic_time_origin = Some(Cx::time_now());
        }
        if config.inject_local_xr_updates {
            let time_origin = self.synthetic_time_origin.unwrap_or_else(Cx::time_now);
            let time = time_origin + self.synthetic_frame_index as f64 * SYNTHETIC_DT;
            let state = Rc::new(synthetic_shooter_state(
                time,
                self.synthetic_frame_index,
                &config,
            ));
            let last = self
                .last_injected_state
                .clone()
                .unwrap_or_else(|| state.clone());
            self.last_injected_state = Some(state.clone());
            self.synthetic_frame_index += 1;
            self.dispatch_event(cx, &Event::XrUpdate(XrUpdateEvent { state, last }));
            self.collect_runtime_observation(cx);
        } else {
            let time_origin = self.synthetic_time_origin.unwrap_or_else(Cx::time_now);
            let time = time_origin + self.synthetic_frame_index as f64 * SYNTHETIC_DT;
            self.synthetic_frame_index += 1;
            self.ui.widget(cx, ids!(xr_peer_sync)).handle_event(
                cx,
                &Event::NextFrame(NextFrameEvent {
                    frame: self.synthetic_frame_index as u64,
                    time,
                    set: Default::default(),
                }),
                &mut Scope::empty(),
            );
            self.publish_local_shared_object_states(cx);
            self.collect_runtime_observation(cx);
        }

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
        let report = ShooterUiAppReport {
            role: config.role,
            connected_peer_peak: self.connected_peer_peak,
            clock_synced_peer_peak: self.clock_synced_peer_peak,
            clock_ping_tx_count_peak: self.clock_ping_tx_count_peak,
            clock_ping_rx_count_peak: self.clock_ping_rx_count_peak,
            clock_pong_tx_count_peak: self.clock_pong_tx_count_peak,
            clock_pong_rx_count_peak: self.clock_pong_rx_count_peak,
            non_xr_draw_clock_count_peak: self.non_xr_draw_clock_count_peak,
            local_spawn_count: self.local_spawn_count,
            remote_spawn_count: self.remote_spawn_count,
            local_spawn_tx_count: self.local_spawn_tx_count,
            local_spawn_tx_fail_count: self.local_spawn_tx_fail_count,
            peer_sync_tx_body_spawn_count_peak: self.peer_sync_tx_body_spawn_count_peak,
            peer_sync_rx_body_spawn_count_peak: self.peer_sync_rx_body_spawn_count_peak,
            peer_sync_tx_shared_object_state_count_peak: self
                .peer_sync_tx_shared_object_state_count_peak,
            peer_sync_rx_shared_object_state_count_peak: self
                .peer_sync_rx_shared_object_state_count_peak,
            remote_shadow_apply_count_peak: self.remote_shadow_apply_count_peak,
            pending_shared_object_control_count_peak: self.pending_shared_object_control_count_peak,
            last_network_event: self.last_network_event.clone(),
            shared_object_peak: self.shared_object_peak,
            active_projectile_peak: self.active_projectile_peak,
            tracked_projectile_widget_uid: self.tracked_projectile_widget_uid,
            tracked_projectile_presence_count: self.tracked_projectile_presence_count,
            runtime_body_count_peak: self.runtime_body_count_peak,
            runtime_body_widget_samples: self.runtime_body_widget_samples.clone(),
            physics_scene_body_count_peak: self.physics_scene_body_count_peak,
            physics_body_spawn_apply_count_peak: self.physics_body_spawn_apply_count_peak,
            physics_body_spawn_miss_count_peak: self.physics_body_spawn_miss_count_peak,
            physics_revision_peak: self.physics_revision_peak,
            scene_changed_count: self.scene_changed_count,
            scene_changed_after_spawn_count: self.scene_changed_after_spawn_count,
            spawnable_binding_count_peak: self.spawnable_binding_count_peak,
            spawnable_widget_samples: self.spawnable_widget_samples.clone(),
            active_scene_child_count_peak: self.active_scene_child_count_peak,
            projectile_z_samples: self.projectile_z_samples.clone(),
            projectile_linvel_z_samples: self.projectile_linvel_z_samples.clone(),
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

    fn debug_state(&self, cx: &mut Cx) -> String {
        let current_activity = self.current_activity(cx);
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        let peer_state = peer_sync_widget
            .borrow::<XrPeerSync>()
            .map(|peer_sync| {
                format!(
                    "connected={} accepted={:?} spawnable={:?} shared={} status={} last={}",
                    peer_sync.connected_peer_count(),
                    peer_sync.current_activity(),
                    peer_sync.spawnable_activity(),
                    peer_sync.shared_object_count(),
                    peer_sync.network_status_text(),
                    peer_sync.last_network_event_label(),
                )
            })
            .unwrap_or_else(|| "peer_sync=missing".to_string());
        format!(
            "network_started={} current_activity={current_activity:?} frames_started={} frame_index={} clock_synced_peak={} clock_ping_tx_peak={} clock_ping_rx_peak={} clock_pong_tx_peak={} clock_pong_rx_peak={} non_xr_draw_clock_peak={} tx_shared_state_peak={} rx_shared_state_peak={} shadow_apply_peak={} local_spawns={} remote_spawns={} local_spawn_tx={} local_spawn_tx_fail={} active_peak={} z_samples={:?} {peer_state}",
            self.network_started,
            self.synthetic_frames_started,
            self.synthetic_frame_index,
            self.clock_synced_peer_peak,
            self.clock_ping_tx_count_peak,
            self.clock_ping_rx_count_peak,
            self.clock_pong_tx_count_peak,
            self.clock_pong_rx_count_peak,
            self.non_xr_draw_clock_count_peak,
            self.peer_sync_tx_shared_object_state_count_peak,
            self.peer_sync_rx_shared_object_state_count_peak,
            self.remote_shadow_apply_count_peak,
            self.local_spawn_count,
            self.remote_spawn_count,
            self.local_spawn_tx_count,
            self.local_spawn_tx_fail_count,
            self.active_projectile_peak,
            self.projectile_z_samples,
        )
    }

    fn dispatch_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Startup) {
            self.apply_local_scene_overrides(cx);
            self.ensure_network_started(cx);
        }
        self.ensure_activity_announced(cx);
        self.apply_local_scene_overrides(cx);
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

impl MatchEvent for ShooterLoopbackApp {
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
            if let Some(body_spawn) = widget_action.action.downcast_ref::<XrBodySpawn>() {
                local_body_spawns.push(*body_spawn);
            }
            if matches!(
                widget_action.cast::<XrNodeAction>(),
                XrNodeAction::SceneChanged
            ) {
                scene_changed = true;
            }
        }

        if scene_changed {
            self.local_scene_overrides_applied = false;
            self.scene_changed_count = self.scene_changed_count.saturating_add(1);
            if self.local_spawn_count > 0 {
                self.scene_changed_after_spawn_count =
                    self.scene_changed_after_spawn_count.saturating_add(1);
            }
            self.refresh_spawnable_registry(cx, true);
        }

        if let Some(activity_id) = remote_activity {
            if self.current_activity(cx) != Some(activity_id) {
                self.local_scene_overrides_applied = false;
                self.suppress_activity_broadcast = Some(activity_id);
                if self.apply_activity(cx, activity_id).is_none() {
                    self.suppress_activity_broadcast = None;
                } else {
                    self.apply_local_scene_overrides(cx);
                }
            }
            self.refresh_spawnable_registry(cx, true);
        }

        if let Some(activity_id) = local_activity {
            self.local_scene_overrides_applied = false;
            self.apply_local_scene_overrides(cx);
            self.refresh_spawnable_registry(cx, true);
            if self.suppress_activity_broadcast == Some(activity_id) {
                self.suppress_activity_broadcast = None;
            } else if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                let _ = peer_sync.set_local_activity(cx, activity_id);
            }
        }

        let first_remote_spawn_widget = remote_body_spawns.first().map(|spawn| spawn.widget_uid);
        let remote_spawn_count = remote_body_spawns.len();

        for widget_uid in remote_body_despawns {
            self.apply_remote_body_despawn(cx, widget_uid);
        }
        for spawn in remote_body_spawns {
            self.apply_remote_body_spawn(cx, spawn);
        }
        for impulse in remote_body_impulses {
            self.apply_body_impulse(cx, impulse);
        }

        let mut applied_local_body_spawns = Vec::new();
        if !local_body_spawns.is_empty() {
            self.refresh_spawnable_registry(cx, false);
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                for spawn in local_body_spawns {
                    if let Some(spawn) = peer_sync.send_local_body_spawn(spawn) {
                        applied_local_body_spawns.push(spawn);
                        self.apply_remote_body_spawn(cx, spawn);
                        self.local_spawn_tx_count += 1;
                    } else {
                        self.local_spawn_tx_fail_count += 1;
                    }
                }
            }
        }

        self.remote_spawn_count += remote_spawn_count;
        self.local_spawn_count += applied_local_body_spawns.len();
        let role = self.config.as_ref().map(|config| config.role);
        if self.tracked_projectile_widget_uid.is_none() {
            match role {
                Some(ShooterUiRole::Emitter) => {
                    self.tracked_projectile_widget_uid = applied_local_body_spawns
                        .first()
                        .map(|spawn| spawn.widget_uid);
                }
                Some(ShooterUiRole::Receiver) => {
                    self.tracked_projectile_widget_uid = first_remote_spawn_widget;
                }
                None => {}
            }
        }
    }
}

impl AppMain for ShooterLoopbackApp {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_xr::makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.dispatch_event(cx, event);
    }
}

fn synthetic_shooter_state(time: f64, frame_index: usize, config: &ShooterUiAppConfig) -> XrState {
    let mut state = XrState {
        time,
        head_pose: Pose::new(Quat::default(), vec3f(0.0, 1.6, 0.35)),
        ..Default::default()
    };
    if matches!(
        config.gesture_profile,
        ShooterGestureProfile::AlternatingHandsWithStickyGrabBit
    ) {
        configure_alternating_shooter_hands(&mut state, frame_index, config.role);
        return state;
    }
    if matches!(
        config.gesture_profile,
        ShooterGestureProfile::RepeatedSparseCloseOpen
    ) {
        configure_repeated_sparse_close_open_hand(&mut state, frame_index, config.role);
        return state;
    }
    if let Some(stage) = synthetic_shooter_hand_stage(frame_index, config) {
        match stage {
            SyntheticShooterHandStage::Pointing { sticky_grab_bit } => {
                configure_pointing_hand(&mut state.right_hand, true, config.gesture_profile);
                if sticky_grab_bit {
                    state.right_hand.tips_active |= XrHand::GRAB_ACTIVE;
                }
            }
            SyntheticShooterHandStage::ClosedGrab => {
                configure_closed_hand(&mut state.right_hand, true);
            }
        }
    }
    state
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyntheticShooterHandStage {
    Pointing { sticky_grab_bit: bool },
    ClosedGrab,
}

fn synthetic_shooter_hand_stage(
    frame_index: usize,
    config: &ShooterUiAppConfig,
) -> Option<SyntheticShooterHandStage> {
    if !matches!(config.role, ShooterUiRole::Emitter) {
        return None;
    }
    match config.gesture_profile {
        ShooterGestureProfile::TipTrackedPoint | ShooterGestureProfile::AimPointWithoutTipBit => (5
            ..8)
            .contains(&frame_index)
            .then_some(SyntheticShooterHandStage::Pointing {
                sticky_grab_bit: false,
            }),
        ShooterGestureProfile::LongHeldPoint => {
            (5..225)
                .contains(&frame_index)
                .then_some(SyntheticShooterHandStage::Pointing {
                    sticky_grab_bit: false,
                })
        }
        ShooterGestureProfile::RepeatedSparseCloseOpen => {
            let local = frame_index.saturating_sub(5) % 28;
            if frame_index < 5 || frame_index >= 5 + 28 * 8 {
                None
            } else if local < 8 {
                Some(SyntheticShooterHandStage::Pointing {
                    sticky_grab_bit: false,
                })
            } else if local < 14 {
                Some(SyntheticShooterHandStage::ClosedGrab)
            } else if local < 18 {
                None
            } else {
                Some(SyntheticShooterHandStage::Pointing {
                    sticky_grab_bit: false,
                })
            }
        }
        ShooterGestureProfile::AlternatingHandsWithStickyGrabBit => None,
        ShooterGestureProfile::ReopenPointWithStickyGrabBit => {
            if (5..10).contains(&frame_index) {
                Some(SyntheticShooterHandStage::Pointing {
                    sticky_grab_bit: false,
                })
            } else if (10..15).contains(&frame_index) {
                Some(SyntheticShooterHandStage::ClosedGrab)
            } else if (15..22).contains(&frame_index) {
                Some(SyntheticShooterHandStage::Pointing {
                    sticky_grab_bit: true,
                })
            } else {
                None
            }
        }
    }
}

fn configure_pointing_hand(hand: &mut XrHand, dominant: bool, profile: ShooterGestureProfile) {
    hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
    if dominant {
        hand.flags |= XrHand::DOMINANT_HAND;
    }
    hand.tips_active = match profile {
        ShooterGestureProfile::TipTrackedPoint
        | ShooterGestureProfile::ReopenPointWithStickyGrabBit
        | ShooterGestureProfile::LongHeldPoint
        | ShooterGestureProfile::AlternatingHandsWithStickyGrabBit
        | ShooterGestureProfile::RepeatedSparseCloseOpen => 1 << XrHand::INDEX_TIP,
        ShooterGestureProfile::AimPointWithoutTipBit => 0,
    };
    hand.tips[XrHand::INDEX_TIP] = 0.040;

    let orientation = Quat::default();
    let base = vec3f(0.20, 1.22, -0.22);
    let step = vec3f(0.0, 0.0, -0.045);
    hand.joints[XrHand::CENTER] = Pose::new(orientation, base + vec3f(0.0, -0.03, 0.03));
    hand.joints[XrHand::WRIST] = Pose::new(orientation, base + vec3f(0.0, -0.05, 0.08));
    hand.joints[XrHand::INDEX_BASE] = Pose::new(orientation, base);
    hand.joints[XrHand::INDEX_KNUCKLE1] = Pose::new(orientation, base + step);
    hand.joints[XrHand::INDEX_KNUCKLE2] = Pose::new(orientation, base + step * 2.0);
    hand.joints[XrHand::INDEX_KNUCKLE3] = Pose::new(orientation, base + step * 3.0);
    hand.aim_pose = Pose::new(orientation, base + step * 3.8);
}

fn configure_closed_hand(hand: &mut XrHand, dominant: bool) {
    hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
    if dominant {
        hand.flags |= XrHand::DOMINANT_HAND;
    }
    hand.tips_active = XrHand::GRAB_ACTIVE;
    hand.tips[XrHand::INDEX_TIP] = 0.030;

    let orientation = Quat::default();
    let base = vec3f(0.20, 1.22, -0.22);
    hand.joints[XrHand::CENTER] = Pose::new(orientation, base + vec3f(0.0, -0.03, 0.03));
    hand.joints[XrHand::WRIST] = Pose::new(orientation, base + vec3f(0.0, -0.05, 0.08));
    hand.joints[XrHand::INDEX_BASE] = Pose::new(orientation, base);
    hand.joints[XrHand::INDEX_KNUCKLE1] = Pose::new(orientation, base + vec3f(0.0, 0.0, -0.030));
    hand.joints[XrHand::INDEX_KNUCKLE2] =
        Pose::new(orientation, base + vec3f(0.018, -0.012, -0.040));
    hand.joints[XrHand::INDEX_KNUCKLE3] =
        Pose::new(orientation, base + vec3f(0.034, -0.030, -0.032));
    hand.aim_pose = Pose::new(orientation, base + vec3f(0.0, 0.0, -0.12));
}

fn configure_sparse_tracking_hand(hand: &mut XrHand, dominant: bool) {
    hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
    if dominant {
        hand.flags |= XrHand::DOMINANT_HAND;
    }
    hand.tips_active = XrHand::GRAB_ACTIVE;

    let orientation = Quat::default();
    let base = vec3f(0.20, 1.22, -0.22);
    hand.joints[XrHand::CENTER] = Pose::new(orientation, base + vec3f(0.0, -0.03, 0.03));
    hand.joints[XrHand::WRIST] = Pose::new(orientation, base + vec3f(0.0, -0.05, 0.08));
    hand.aim_pose = Pose::new(orientation, base + vec3f(0.0, 0.0, -0.12));
}

fn offset_hand(hand: &mut XrHand, delta: Vec3f) {
    for joint in &mut hand.joints {
        joint.position += delta;
    }
    hand.aim_pose.position += delta;
}

fn configure_alternating_shooter_hands(
    state: &mut XrState,
    frame_index: usize,
    role: ShooterUiRole,
) {
    if !matches!(role, ShooterUiRole::Emitter) || frame_index < 5 {
        return;
    }

    let phase = ((frame_index - 5) / 12) % 2;
    match phase {
        0 => {
            configure_pointing_hand(
                &mut state.left_hand,
                true,
                ShooterGestureProfile::AlternatingHandsWithStickyGrabBit,
            );
            offset_hand(&mut state.left_hand, vec3f(-0.42, 0.0, 0.0));
            state.left_hand.flags |= XrHand::DOMINANT_HAND;
            configure_closed_hand(&mut state.right_hand, false);
            state.right_hand.tips_active |= XrHand::GRAB_ACTIVE;
        }
        _ => {
            configure_closed_hand(&mut state.left_hand, false);
            offset_hand(&mut state.left_hand, vec3f(-0.42, 0.0, 0.0));
            state.left_hand.tips_active |= XrHand::GRAB_ACTIVE;
            configure_pointing_hand(
                &mut state.right_hand,
                true,
                ShooterGestureProfile::AlternatingHandsWithStickyGrabBit,
            );
            state.right_hand.flags |= XrHand::DOMINANT_HAND;
        }
    }
}

fn configure_repeated_sparse_close_open_hand(
    state: &mut XrState,
    frame_index: usize,
    role: ShooterUiRole,
) {
    if !matches!(role, ShooterUiRole::Emitter) || frame_index < 5 || frame_index >= 5 + 28 * 8 {
        return;
    }
    let local = (frame_index - 5) % 28;
    if local < 8 || local >= 18 {
        configure_pointing_hand(
            &mut state.right_hand,
            true,
            ShooterGestureProfile::RepeatedSparseCloseOpen,
        );
    } else if local < 14 {
        configure_closed_hand(&mut state.right_hand, true);
    } else {
        configure_sparse_tracking_hand(&mut state.right_hand, true);
    }
}

fn projectile_moved(report: &ShooterUiAppReport) -> bool {
    let Some(first) = report.projectile_z_samples.first() else {
        return false;
    };
    let Some(last) = report.projectile_z_samples.last() else {
        return false;
    };
    (first - last).abs() > 0.12
}

fn projectile_preimpact_z_samples(report: &ShooterUiAppReport) -> Vec<f32> {
    report
        .projectile_z_samples
        .iter()
        .copied()
        .take_while(|z| *z > -3.6)
        .collect()
}

fn projectile_preimpact_steady_z_samples(report: &ShooterUiAppReport) -> Vec<f32> {
    const MAX_BACKWARD_JUMP: f32 = 0.08;
    const MAX_FORWARD_STEP: f32 = 0.45;

    let mut best: Vec<f32> = Vec::new();
    let mut current: Vec<f32> = Vec::new();
    for z in projectile_preimpact_z_samples(report).into_iter().skip(4) {
        if let Some(previous) = current.last().copied() {
            let backward_jump = (z - previous).max(0.0);
            let forward_step = (previous - z).max(0.0);
            if backward_jump > MAX_BACKWARD_JUMP || forward_step > MAX_FORWARD_STEP {
                if current.len() > best.len() {
                    best = current;
                }
                current = Vec::new();
            }
        }
        current.push(z);
    }
    if current.len() > best.len() {
        best = current;
    }
    best
}

fn projectile_preimpact_max_backward_jump(report: &ShooterUiAppReport) -> f32 {
    projectile_preimpact_steady_z_samples(report)
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).max(0.0))
        .fold(0.0, f32::max)
}

fn projectile_preimpact_max_forward_step(report: &ShooterUiAppReport) -> f32 {
    projectile_preimpact_steady_z_samples(report)
        .windows(2)
        .map(|pair| (pair[0] - pair[1]).max(0.0))
        .fold(0.0, f32::max)
}

fn projectile_sustained_forward_travel_after_spawn(
    report: &ShooterUiAppReport,
    skip_samples: usize,
) -> f32 {
    let samples = projectile_preimpact_z_samples(report);
    let Some(start) = samples.get(skip_samples).copied() else {
        return 0.0;
    };
    let min_after_start = samples
        .iter()
        .copied()
        .skip(skip_samples)
        .fold(start, f32::min);
    (start - min_after_start).max(0.0)
}

fn projectile_observer_linvel_peak_abs_z(report: &ShooterUiAppReport) -> f32 {
    report
        .projectile_linvel_z_samples
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0, f32::max)
}

fn projectile_min_z(report: &ShooterUiAppReport) -> f32 {
    report
        .projectile_z_samples
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min)
}

fn projectile_postimpact_recovery_distance(report: &ShooterUiAppReport, impact_z: f32) -> f32 {
    let samples = &report.projectile_z_samples;
    let Some((impact_index, impact_sample)) = samples
        .iter()
        .copied()
        .enumerate()
        .find(|(_, z)| *z <= impact_z)
    else {
        return 0.0;
    };
    let mut rebound_max = impact_sample;
    let mut previous = impact_sample;
    for sample in samples.iter().copied().skip(impact_index.saturating_add(1)) {
        // Pooled projectile widgets can jump back near the emitter when they are recycled for the
        // next shot. Treat that as a new generation instead of a giant "rebound".
        if sample - previous > 0.40 {
            break;
        }
        rebound_max = rebound_max.max(sample);
        previous = sample;
    }
    (rebound_max - impact_sample).max(0.0)
}

fn projectile_positive_linvel_peak_z(report: &ShooterUiAppReport) -> f32 {
    report
        .projectile_linvel_z_samples
        .iter()
        .copied()
        .filter(|z| *z > 0.0)
        .fold(0.0, f32::max)
}

fn run_test_app(config: ShooterUiAppConfig) -> ShooterUiAppReport {
    run_test_app_with_limits(config, None, TEST_IO_TIMEOUT)
}

fn run_test_app_with_limits(
    config: ShooterUiAppConfig,
    draw_cycles_override: Option<usize>,
    io_timeout: Duration,
) -> ShooterUiAppReport {
    let (report_tx, report_rx) = mpsc::channel();
    let app_ref = Rc::new(RefCell::new(None::<ShooterLoopbackApp>));
    let app_ref_closure = app_ref.clone();
    let config_closure = config.clone();
    let report_tx_closure = report_tx.clone();

    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if let Event::Startup = event {
            *app_ref_closure.borrow_mut() = Some(cx.with_vm(|vm| {
                let value = <ShooterLoopbackApp as AppMain>::script_mod(vm);
                let mut app = <ShooterLoopbackApp as ScriptNew>::script_from_value(vm, value);
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

    let draw_cycles = draw_cycles_override.unwrap_or_else(|| {
        TEST_DRAW_CYCLES.max(
            config
                .synthetic_frame_budget
                .saturating_add(TEST_DRAW_CYCLE_SLACK)
                .saturating_add(
                    config
                        .expected_peers
                        .saturating_mul(TEST_DRAW_CYCLE_NETWORK_SLACK),
                ),
        )
    });
    Cx::headless_no_draw_event_loop_for_draw_cycles(cx.clone(), draw_cycles);

    match report_rx.recv_timeout(io_timeout) {
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
            panic!("test app should report before the bounded headless loop exits: {debug_state}");
        }
    }
}

#[test]
fn single_headless_shooter_app_emits_projectiles_from_synthetic_xr_updates() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let report = run_test_app(ShooterUiAppConfig {
        role: ShooterUiRole::Emitter,
        gesture_profile: ShooterGestureProfile::TipTrackedPoint,
        net_config: localhost_config(701, 47046, 47047, 47048, vec![]),
        expected_peers: 0,
        inject_local_xr_updates: true,
        local_back_wall_visible: true,
        synthetic_frame_budget: 150,
    });

    assert_eq!(report.role, ShooterUiRole::Emitter);
    assert_eq!(report.local_activity, Some(shooter_activity_id()));
    assert_eq!(report.spawnable_activity, Some(shooter_activity_id()));
    assert_eq!(report.local_spawn_count, 1, "{report:?}");
    assert_eq!(report.remote_spawn_count, 0);
    assert_eq!(report.local_spawn_tx_count, 1, "{report:?}");
    assert_eq!(report.local_spawn_tx_fail_count, 0, "{report:?}");
    assert!(report.spawnable_binding_count_peak >= 24, "{report:?}");
    assert!(report.physics_scene_body_count_peak >= 26, "{report:?}");
    assert!(
        report.physics_body_spawn_apply_count_peak >= 1,
        "{report:?}"
    );
    assert_eq!(report.physics_body_spawn_miss_count_peak, 0, "{report:?}");
    assert_eq!(report.scene_changed_after_spawn_count, 0, "{report:?}");
    assert!(report.active_projectile_peak >= 1, "{report:?}");
    assert!(report.tracked_projectile_presence_count >= 2, "{report:?}");
    assert!(projectile_moved(&report), "{report:?}");
}

#[test]
fn single_headless_shooter_app_emits_projectiles_from_openxr_aim_point_without_tip_bit() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let report = run_test_app(ShooterUiAppConfig {
        role: ShooterUiRole::Emitter,
        gesture_profile: ShooterGestureProfile::AimPointWithoutTipBit,
        net_config: localhost_config(702, 47056, 47057, 47058, vec![]),
        expected_peers: 0,
        inject_local_xr_updates: true,
        local_back_wall_visible: true,
        synthetic_frame_budget: 150,
    });

    assert_eq!(report.role, ShooterUiRole::Emitter);
    assert_eq!(report.local_activity, Some(shooter_activity_id()));
    assert_eq!(report.spawnable_activity, Some(shooter_activity_id()));
    assert_eq!(report.local_spawn_count, 1, "{report:?}");
    assert_eq!(report.remote_spawn_count, 0);
    assert_eq!(report.local_spawn_tx_count, 1, "{report:?}");
    assert_eq!(report.local_spawn_tx_fail_count, 0, "{report:?}");
    assert!(report.spawnable_binding_count_peak >= 24, "{report:?}");
    assert!(report.physics_scene_body_count_peak >= 26, "{report:?}");
    assert!(
        report.physics_body_spawn_apply_count_peak >= 1,
        "{report:?}"
    );
    assert_eq!(report.physics_body_spawn_miss_count_peak, 0, "{report:?}");
    assert_eq!(report.scene_changed_after_spawn_count, 0, "{report:?}");
    assert!(report.active_projectile_peak >= 1, "{report:?}");
    assert!(report.tracked_projectile_presence_count >= 2, "{report:?}");
    assert!(projectile_moved(&report), "{report:?}");
}

#[test]
fn single_headless_shooter_app_reemits_after_close_open_with_sticky_grab_bit() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let report = run_test_app(ShooterUiAppConfig {
        role: ShooterUiRole::Emitter,
        gesture_profile: ShooterGestureProfile::ReopenPointWithStickyGrabBit,
        net_config: localhost_config(703, 47066, 47067, 47068, vec![]),
        expected_peers: 0,
        inject_local_xr_updates: true,
        local_back_wall_visible: true,
        synthetic_frame_budget: 170,
    });

    assert_eq!(report.role, ShooterUiRole::Emitter);
    assert_eq!(report.local_activity, Some(shooter_activity_id()));
    assert_eq!(report.spawnable_activity, Some(shooter_activity_id()));
    assert!(report.local_spawn_count >= 2, "{report:?}");
    assert_eq!(report.remote_spawn_count, 0);
    assert!(report.local_spawn_tx_count >= 2, "{report:?}");
    assert_eq!(report.local_spawn_tx_fail_count, 0, "{report:?}");
    assert!(
        report.physics_body_spawn_apply_count_peak >= 2,
        "{report:?}"
    );
    assert_eq!(report.physics_body_spawn_miss_count_peak, 0, "{report:?}");
    assert_eq!(report.scene_changed_after_spawn_count, 0, "{report:?}");
    assert!(report.active_projectile_peak >= 1, "{report:?}");
    assert!(report.tracked_projectile_presence_count >= 2, "{report:?}");
    assert!(projectile_moved(&report), "{report:?}");
}

#[test]
fn single_headless_shooter_app_keeps_emitting_and_advancing_physics_during_long_hold() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let report = run_test_app(ShooterUiAppConfig {
        role: ShooterUiRole::Emitter,
        gesture_profile: ShooterGestureProfile::LongHeldPoint,
        net_config: localhost_config(704, 47076, 47077, 47078, vec![]),
        expected_peers: 0,
        inject_local_xr_updates: true,
        local_back_wall_visible: true,
        synthetic_frame_budget: 260,
    });

    assert_eq!(report.role, ShooterUiRole::Emitter);
    assert_eq!(report.local_activity, Some(shooter_activity_id()));
    assert_eq!(report.spawnable_activity, Some(shooter_activity_id()));
    assert!(report.local_spawn_count >= 20, "{report:?}");
    assert_eq!(report.remote_spawn_count, 0);
    assert_eq!(
        report.local_spawn_tx_count, report.local_spawn_count,
        "{report:?}"
    );
    assert_eq!(report.local_spawn_tx_fail_count, 0, "{report:?}");
    assert!(
        report.physics_body_spawn_apply_count_peak >= report.local_spawn_count,
        "{report:?}"
    );
    assert_eq!(report.physics_body_spawn_miss_count_peak, 0, "{report:?}");
    assert_eq!(report.scene_changed_after_spawn_count, 0, "{report:?}");
    assert!(report.active_projectile_peak >= 6, "{report:?}");
    assert!(report.tracked_projectile_presence_count >= 8, "{report:?}");
    assert!(report.projectile_z_samples.len() >= 8, "{report:?}");
    assert!(projectile_moved(&report), "{report:?}");
}

#[test]
fn single_headless_shooter_app_keeps_emitting_when_alternating_left_and_right_hands() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let report = run_test_app(ShooterUiAppConfig {
        role: ShooterUiRole::Emitter,
        gesture_profile: ShooterGestureProfile::AlternatingHandsWithStickyGrabBit,
        net_config: localhost_config(705, 47086, 47087, 47088, vec![]),
        expected_peers: 0,
        inject_local_xr_updates: true,
        local_back_wall_visible: true,
        synthetic_frame_budget: 260,
    });

    assert_eq!(report.role, ShooterUiRole::Emitter);
    assert_eq!(report.local_activity, Some(shooter_activity_id()));
    assert_eq!(report.spawnable_activity, Some(shooter_activity_id()));
    assert!(report.local_spawn_count >= 12, "{report:?}");
    assert_eq!(report.remote_spawn_count, 0);
    assert_eq!(
        report.local_spawn_tx_count, report.local_spawn_count,
        "{report:?}"
    );
    assert_eq!(report.local_spawn_tx_fail_count, 0, "{report:?}");
    assert!(
        report.physics_body_spawn_apply_count_peak >= report.local_spawn_count,
        "{report:?}"
    );
    assert_eq!(report.physics_body_spawn_miss_count_peak, 0, "{report:?}");
    assert!(report.active_projectile_peak >= 4, "{report:?}");
    assert!(report.tracked_projectile_presence_count >= 6, "{report:?}");
    assert!(report.projectile_z_samples.len() >= 6, "{report:?}");
    assert!(projectile_moved(&report), "{report:?}");
}

#[test]
fn single_headless_shooter_app_survives_repeated_sparse_close_open_cycles() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let report = run_test_app(ShooterUiAppConfig {
        role: ShooterUiRole::Emitter,
        gesture_profile: ShooterGestureProfile::RepeatedSparseCloseOpen,
        net_config: localhost_config(706, 47096, 47097, 47098, vec![]),
        expected_peers: 0,
        inject_local_xr_updates: true,
        local_back_wall_visible: true,
        synthetic_frame_budget: 300,
    });

    assert_eq!(report.role, ShooterUiRole::Emitter);
    assert_eq!(report.local_activity, Some(shooter_activity_id()));
    assert_eq!(report.spawnable_activity, Some(shooter_activity_id()));
    assert!(report.local_spawn_count >= 12, "{report:?}");
    assert_eq!(report.remote_spawn_count, 0);
    assert_eq!(
        report.local_spawn_tx_count, report.local_spawn_count,
        "{report:?}"
    );
    assert_eq!(report.local_spawn_tx_fail_count, 0, "{report:?}");
    assert!(
        report.physics_body_spawn_apply_count_peak >= report.local_spawn_count,
        "{report:?}"
    );
    assert_eq!(report.physics_body_spawn_miss_count_peak, 0, "{report:?}");
    assert!(report.active_projectile_peak >= 4, "{report:?}");
    assert!(report.tracked_projectile_presence_count >= 8, "{report:?}");
    assert!(report.projectile_z_samples.len() >= 8, "{report:?}");
    assert!(projectile_moved(&report), "{report:?}");
}

#[test]
fn two_headless_shooter_apps_emit_and_replicate_projectiles_over_loopback() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let emitter_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Emitter,
            gesture_profile: ShooterGestureProfile::TipTrackedPoint,
            net_config: localhost_config(711, 47146, 47147, 47148, vec![47156]),
            expected_peers: 1,
            inject_local_xr_updates: true,
            local_back_wall_visible: true,
            synthetic_frame_budget: 170,
        })
    });
    let receiver_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Receiver,
            gesture_profile: ShooterGestureProfile::TipTrackedPoint,
            net_config: localhost_config(712, 47156, 47157, 47158, vec![47146]),
            expected_peers: 1,
            inject_local_xr_updates: true,
            local_back_wall_visible: true,
            synthetic_frame_budget: 170,
        })
    });

    let emitter_report = emitter_thread
        .join()
        .expect("emitter app thread should complete successfully");
    let receiver_report = receiver_thread
        .join()
        .expect("receiver app thread should complete successfully");

    assert_eq!(emitter_report.role, ShooterUiRole::Emitter);
    assert_eq!(emitter_report.local_activity, Some(shooter_activity_id()));
    assert_eq!(
        emitter_report.spawnable_activity,
        Some(shooter_activity_id())
    );
    assert!(
        emitter_report.connected_peer_peak >= 1,
        "{emitter_report:?}"
    );
    assert_eq!(emitter_report.local_spawn_count, 1, "{emitter_report:?}");
    assert_eq!(emitter_report.local_spawn_tx_count, 1, "{emitter_report:?}");
    assert_eq!(
        emitter_report.local_spawn_tx_fail_count, 0,
        "{emitter_report:?}"
    );
    assert!(
        emitter_report.spawnable_binding_count_peak >= 24,
        "{emitter_report:?}"
    );
    assert!(
        emitter_report.physics_scene_body_count_peak >= 26,
        "{emitter_report:?}"
    );
    assert!(
        emitter_report.physics_body_spawn_apply_count_peak >= 1,
        "{emitter_report:?}"
    );
    assert_eq!(
        emitter_report.physics_body_spawn_miss_count_peak, 0,
        "{emitter_report:?}"
    );
    assert_eq!(
        emitter_report.scene_changed_after_spawn_count, 0,
        "{emitter_report:?}"
    );
    assert!(emitter_report.shared_object_peak >= 1, "{emitter_report:?}");
    assert!(
        emitter_report.active_projectile_peak >= 1,
        "{emitter_report:?}"
    );
    assert!(
        emitter_report.tracked_projectile_presence_count >= 2,
        "{emitter_report:?}"
    );
    assert!(projectile_moved(&emitter_report), "{emitter_report:?}");

    assert_eq!(receiver_report.role, ShooterUiRole::Receiver);
    assert_eq!(receiver_report.local_activity, Some(shooter_activity_id()));
    assert_eq!(
        receiver_report.spawnable_activity,
        Some(shooter_activity_id())
    );
    assert!(
        receiver_report.connected_peer_peak >= 1,
        "{receiver_report:?}"
    );
    assert_eq!(receiver_report.local_spawn_count, 0);
    assert!(
        receiver_report.remote_spawn_count >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.spawnable_binding_count_peak >= 24,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.physics_scene_body_count_peak >= 26,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.physics_body_spawn_apply_count_peak >= 1,
        "{receiver_report:?}"
    );
    assert_eq!(
        receiver_report.physics_body_spawn_miss_count_peak, 0,
        "{receiver_report:?}"
    );
    assert_eq!(
        receiver_report.scene_changed_after_spawn_count, 0,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.shared_object_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.active_projectile_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.tracked_projectile_presence_count >= 2,
        "{receiver_report:?}"
    );
    assert!(projectile_moved(&receiver_report), "{receiver_report:?}");
    assert!(
        projectile_sustained_forward_travel_after_spawn(&receiver_report, 2) >= 1.0,
        "{receiver_report:?}"
    );
    assert!(
        projectile_observer_linvel_peak_abs_z(&receiver_report) >= 1.0,
        "{receiver_report:?}"
    );
}

#[test]
fn two_headless_shooter_apps_replicate_projectiles_to_desktop_like_observer() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let emitter_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Emitter,
            gesture_profile: ShooterGestureProfile::TipTrackedPoint,
            net_config: localhost_config(715, 47186, 47187, 47188, vec![47196]),
            expected_peers: 1,
            inject_local_xr_updates: true,
            local_back_wall_visible: true,
            synthetic_frame_budget: 170,
        })
    });
    let receiver_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Receiver,
            gesture_profile: ShooterGestureProfile::TipTrackedPoint,
            net_config: localhost_config(716, 47196, 47197, 47198, vec![47186]),
            expected_peers: 1,
            inject_local_xr_updates: false,
            local_back_wall_visible: true,
            synthetic_frame_budget: 170,
        })
    });

    let emitter_report = emitter_thread
        .join()
        .expect("emitter app thread should complete successfully");
    let receiver_report = receiver_thread
        .join()
        .expect("receiver app thread should complete successfully");

    assert_eq!(emitter_report.role, ShooterUiRole::Emitter);
    assert_eq!(receiver_report.role, ShooterUiRole::Receiver);
    assert!(
        receiver_report.connected_peer_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.clock_synced_peer_peak >= 1,
        "{receiver_report:?}"
    );
    assert_eq!(receiver_report.local_spawn_count, 0, "{receiver_report:?}");
    assert!(
        receiver_report.remote_spawn_count >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.shared_object_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.active_projectile_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.tracked_projectile_presence_count >= 2,
        "{receiver_report:?}"
    );
    assert!(projectile_moved(&receiver_report), "{receiver_report:?}");
    assert!(
        projectile_sustained_forward_travel_after_spawn(&receiver_report, 2) >= 1.0,
        "{receiver_report:?}"
    );
    assert!(
        projectile_observer_linvel_peak_abs_z(&receiver_report) >= 1.0,
        "{receiver_report:?}"
    );
}

#[test]
fn two_headless_shooter_apps_replicate_wall_bounces_to_desktop_like_observer() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let emitter_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Emitter,
            gesture_profile: ShooterGestureProfile::TipTrackedPoint,
            net_config: localhost_config(717, 47206, 47207, 47208, vec![47216]),
            expected_peers: 1,
            inject_local_xr_updates: true,
            local_back_wall_visible: true,
            synthetic_frame_budget: 140,
        })
    });
    let receiver_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Receiver,
            gesture_profile: ShooterGestureProfile::TipTrackedPoint,
            net_config: localhost_config(718, 47216, 47217, 47218, vec![47206]),
            expected_peers: 1,
            inject_local_xr_updates: false,
            local_back_wall_visible: true,
            synthetic_frame_budget: 140,
        })
    });

    let emitter_report = emitter_thread
        .join()
        .expect("emitter app thread should complete successfully");
    let receiver_report = receiver_thread
        .join()
        .expect("receiver app thread should complete successfully");

    assert_eq!(emitter_report.role, ShooterUiRole::Emitter);
    assert_eq!(receiver_report.role, ShooterUiRole::Receiver);

    assert!(
        emitter_report.peer_sync_tx_shared_object_state_count_peak >= 8,
        "{emitter_report:?}"
    );
    assert!(
        projectile_postimpact_recovery_distance(&emitter_report, -4.0) >= 0.18,
        "{emitter_report:?}"
    );
    assert!(
        projectile_positive_linvel_peak_z(&emitter_report) >= 0.20,
        "{emitter_report:?}"
    );

    assert!(
        receiver_report.connected_peer_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.clock_synced_peer_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.peer_sync_rx_shared_object_state_count_peak >= 8,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.remote_shadow_apply_count_peak >= 6,
        "{receiver_report:?}"
    );
    assert!(
        projectile_min_z(&receiver_report) <= -4.0,
        "{receiver_report:?}"
    );
    assert!(
        projectile_positive_linvel_peak_z(&receiver_report) >= 0.20,
        "{receiver_report:?}"
    );
}

#[test]
fn two_headless_shooter_apps_ignore_observer_only_wall_and_follow_authority() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let emitter_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Emitter,
            gesture_profile: ShooterGestureProfile::TipTrackedPoint,
            net_config: localhost_config(719, 47226, 47227, 47228, vec![47236]),
            expected_peers: 1,
            inject_local_xr_updates: true,
            local_back_wall_visible: false,
            synthetic_frame_budget: 170,
        })
    });
    let receiver_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Receiver,
            gesture_profile: ShooterGestureProfile::TipTrackedPoint,
            net_config: localhost_config(720, 47236, 47237, 47238, vec![47226]),
            expected_peers: 1,
            inject_local_xr_updates: false,
            local_back_wall_visible: true,
            synthetic_frame_budget: 170,
        })
    });

    let emitter_report = emitter_thread
        .join()
        .expect("emitter app thread should complete successfully");
    let receiver_report = receiver_thread
        .join()
        .expect("receiver app thread should complete successfully");

    assert_eq!(emitter_report.role, ShooterUiRole::Emitter);
    assert!(
        projectile_min_z(&emitter_report) <= -4.8,
        "{emitter_report:?}"
    );
    assert!(
        projectile_postimpact_recovery_distance(&emitter_report, -4.0) <= 0.08,
        "{emitter_report:?}"
    );
    assert!(
        projectile_positive_linvel_peak_z(&emitter_report) <= 0.08,
        "{emitter_report:?}"
    );

    assert_eq!(receiver_report.role, ShooterUiRole::Receiver);
    assert!(
        receiver_report.connected_peer_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.clock_synced_peer_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.peer_sync_rx_shared_object_state_count_peak >= 8,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.remote_shadow_apply_count_peak >= 6,
        "{receiver_report:?}"
    );
    assert!(
        projectile_min_z(&receiver_report) <= -4.8,
        "{receiver_report:?}"
    );
    assert!(
        projectile_postimpact_recovery_distance(&receiver_report, -4.0) <= 0.08,
        "{receiver_report:?}"
    );
    assert!(
        projectile_positive_linvel_peak_z(&receiver_report) <= 0.08,
        "{receiver_report:?}"
    );
}

#[test]
#[ignore = "process-local loopback discovery remains flaky in the full ui_shooter_loopback binary; run this stress alone when needed"]
fn two_headless_shooter_apps_keep_replicating_projectiles_during_long_hold() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let emitter_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Emitter,
            gesture_profile: ShooterGestureProfile::LongHeldPoint,
            net_config: localhost_config(713, 47166, 47167, 47168, vec![47176]),
            expected_peers: 1,
            inject_local_xr_updates: true,
            local_back_wall_visible: true,
            synthetic_frame_budget: 280,
        })
    });
    let receiver_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Receiver,
            gesture_profile: ShooterGestureProfile::LongHeldPoint,
            net_config: localhost_config(714, 47176, 47177, 47178, vec![47166]),
            expected_peers: 1,
            inject_local_xr_updates: true,
            local_back_wall_visible: true,
            synthetic_frame_budget: 280,
        })
    });

    let emitter_report = emitter_thread
        .join()
        .expect("emitter app thread should complete successfully");
    let receiver_report = receiver_thread
        .join()
        .expect("receiver app thread should complete successfully");

    assert_eq!(emitter_report.role, ShooterUiRole::Emitter);
    assert_eq!(emitter_report.local_activity, Some(shooter_activity_id()));
    assert_eq!(
        emitter_report.spawnable_activity,
        Some(shooter_activity_id())
    );
    assert!(
        emitter_report.connected_peer_peak >= 1,
        "{emitter_report:?}"
    );
    assert!(emitter_report.local_spawn_count >= 20, "{emitter_report:?}");
    assert_eq!(
        emitter_report.local_spawn_tx_count, emitter_report.local_spawn_count,
        "{emitter_report:?}"
    );
    assert_eq!(
        emitter_report.local_spawn_tx_fail_count, 0,
        "{emitter_report:?}"
    );
    assert!(
        emitter_report.physics_body_spawn_apply_count_peak >= emitter_report.local_spawn_count,
        "{emitter_report:?}"
    );
    assert_eq!(
        emitter_report.physics_body_spawn_miss_count_peak, 0,
        "{emitter_report:?}"
    );
    assert!(emitter_report.shared_object_peak >= 8, "{emitter_report:?}");
    assert!(
        emitter_report.tracked_projectile_presence_count >= 8,
        "{emitter_report:?}"
    );
    assert!(
        emitter_report.projectile_z_samples.len() >= 8,
        "{emitter_report:?}"
    );
    assert!(projectile_moved(&emitter_report), "{emitter_report:?}");

    assert_eq!(receiver_report.role, ShooterUiRole::Receiver);
    assert_eq!(receiver_report.local_activity, Some(shooter_activity_id()));
    assert_eq!(
        receiver_report.spawnable_activity,
        Some(shooter_activity_id())
    );
    assert!(
        receiver_report.connected_peer_peak >= 1,
        "{receiver_report:?}"
    );
    assert_eq!(receiver_report.local_spawn_count, 0);
    assert!(
        receiver_report.remote_spawn_count >= 20,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.peer_sync_rx_body_spawn_count_peak >= 20,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.shared_object_peak >= 8,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.active_projectile_peak >= 6,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.tracked_projectile_presence_count >= 8,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.projectile_z_samples.len() >= 8,
        "{receiver_report:?}"
    );
    assert!(projectile_moved(&receiver_report), "{receiver_report:?}");
}

#[test]
fn two_headless_shooter_apps_emit_and_replicate_projectiles_from_openxr_aim_point() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let emitter_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Emitter,
            gesture_profile: ShooterGestureProfile::AimPointWithoutTipBit,
            net_config: localhost_config(713, 47166, 47167, 47168, vec![47176]),
            expected_peers: 1,
            inject_local_xr_updates: true,
            local_back_wall_visible: true,
            synthetic_frame_budget: 170,
        })
    });
    let receiver_thread = thread::spawn(|| {
        run_test_app(ShooterUiAppConfig {
            role: ShooterUiRole::Receiver,
            gesture_profile: ShooterGestureProfile::AimPointWithoutTipBit,
            net_config: localhost_config(714, 47176, 47177, 47178, vec![47166]),
            expected_peers: 1,
            inject_local_xr_updates: true,
            local_back_wall_visible: true,
            synthetic_frame_budget: 170,
        })
    });

    let emitter_report = emitter_thread
        .join()
        .expect("emitter app thread should complete successfully");
    let receiver_report = receiver_thread
        .join()
        .expect("receiver app thread should complete successfully");

    assert_eq!(emitter_report.role, ShooterUiRole::Emitter);
    assert_eq!(emitter_report.local_activity, Some(shooter_activity_id()));
    assert_eq!(
        emitter_report.spawnable_activity,
        Some(shooter_activity_id())
    );
    assert!(
        emitter_report.connected_peer_peak >= 1,
        "{emitter_report:?}"
    );
    assert_eq!(emitter_report.local_spawn_count, 1, "{emitter_report:?}");
    assert_eq!(emitter_report.local_spawn_tx_count, 1, "{emitter_report:?}");
    assert_eq!(
        emitter_report.local_spawn_tx_fail_count, 0,
        "{emitter_report:?}"
    );
    assert!(
        emitter_report.spawnable_binding_count_peak >= 24,
        "{emitter_report:?}"
    );
    assert!(
        emitter_report.physics_scene_body_count_peak >= 26,
        "{emitter_report:?}"
    );
    assert!(
        emitter_report.physics_body_spawn_apply_count_peak >= 1,
        "{emitter_report:?}"
    );
    assert_eq!(
        emitter_report.physics_body_spawn_miss_count_peak, 0,
        "{emitter_report:?}"
    );
    assert_eq!(
        emitter_report.scene_changed_after_spawn_count, 0,
        "{emitter_report:?}"
    );
    assert!(emitter_report.shared_object_peak >= 1, "{emitter_report:?}");
    assert!(
        emitter_report.active_projectile_peak >= 1,
        "{emitter_report:?}"
    );
    assert!(
        emitter_report.tracked_projectile_presence_count >= 2,
        "{emitter_report:?}"
    );
    assert!(projectile_moved(&emitter_report), "{emitter_report:?}");

    assert_eq!(receiver_report.role, ShooterUiRole::Receiver);
    assert_eq!(receiver_report.local_activity, Some(shooter_activity_id()));
    assert_eq!(
        receiver_report.spawnable_activity,
        Some(shooter_activity_id())
    );
    assert!(
        receiver_report.connected_peer_peak >= 1,
        "{receiver_report:?}"
    );
    assert_eq!(receiver_report.local_spawn_count, 0);
    assert!(
        receiver_report.remote_spawn_count >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.spawnable_binding_count_peak >= 24,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.physics_scene_body_count_peak >= 26,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.physics_body_spawn_apply_count_peak >= 1,
        "{receiver_report:?}"
    );
    assert_eq!(
        receiver_report.physics_body_spawn_miss_count_peak, 0,
        "{receiver_report:?}"
    );
    assert_eq!(
        receiver_report.scene_changed_after_spawn_count, 0,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.shared_object_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.active_projectile_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.tracked_projectile_presence_count >= 2,
        "{receiver_report:?}"
    );
    assert!(projectile_moved(&receiver_report), "{receiver_report:?}");
    assert!(
        projectile_preimpact_steady_z_samples(&receiver_report).len() >= 6,
        "{receiver_report:?}"
    );
    assert!(
        projectile_preimpact_max_backward_jump(&receiver_report) <= 0.08,
        "{receiver_report:?}"
    );
    assert!(
        projectile_preimpact_max_forward_step(&receiver_report) <= 0.45,
        "{receiver_report:?}"
    );
}

#[test]
#[ignore = "long-hold regression coverage for observer projectile sync in headless multiplayer"]
fn two_headless_shooter_apps_repro_frozen_observer_projectiles() {
    let _guard = UI_SHOOTER_TEST_LOCK.lock().unwrap();
    let emitter_thread = thread::spawn(|| {
        run_test_app_with_limits(
            ShooterUiAppConfig {
                role: ShooterUiRole::Emitter,
                gesture_profile: ShooterGestureProfile::LongHeldPoint,
                net_config: localhost_config(731, 47286, 47287, 47288, vec![47296]),
                expected_peers: 1,
                inject_local_xr_updates: true,
                local_back_wall_visible: true,
                synthetic_frame_budget: 280,
            },
            Some(8_000),
            Duration::from_secs(12),
        )
    });
    let receiver_thread = thread::spawn(|| {
        run_test_app_with_limits(
            ShooterUiAppConfig {
                role: ShooterUiRole::Receiver,
                gesture_profile: ShooterGestureProfile::LongHeldPoint,
                net_config: localhost_config(732, 47296, 47297, 47298, vec![47286]),
                expected_peers: 1,
                inject_local_xr_updates: true,
                local_back_wall_visible: true,
                synthetic_frame_budget: 280,
            },
            Some(8_000),
            Duration::from_secs(12),
        )
    });

    let emitter_report = emitter_thread
        .join()
        .expect("emitter app thread should complete successfully");
    let receiver_report = receiver_thread
        .join()
        .expect("receiver app thread should complete successfully");

    assert_eq!(emitter_report.role, ShooterUiRole::Emitter);
    assert!(
        emitter_report.connected_peer_peak >= 1,
        "{emitter_report:?}"
    );
    assert!(emitter_report.local_spawn_count >= 20, "{emitter_report:?}");

    assert_eq!(receiver_report.role, ShooterUiRole::Receiver);
    assert!(
        receiver_report.connected_peer_peak >= 1,
        "{receiver_report:?}"
    );
    assert!(
        receiver_report.remote_spawn_count >= 20,
        "receiver never observed enough remote projectile spawns: {receiver_report:?}"
    );
    assert!(
        receiver_report.shared_object_peak >= 8,
        "receiver never accumulated the replicated projectile set: {receiver_report:?}"
    );
    assert!(
        projectile_sustained_forward_travel_after_spawn(&receiver_report, 2) >= 0.8,
        "observer projectile still stalled after spawn instead of making sustained forward progress: {receiver_report:?}"
    );
    assert!(
        projectile_observer_linvel_peak_abs_z(&receiver_report) >= 1.0,
        "observer projectile still lost almost all z velocity instead of tracking the remote shadow: {receiver_report:?}"
    );
    assert!(
        projectile_preimpact_steady_z_samples(&receiver_report).len() >= 12,
        "observer never maintained a long enough steady preimpact flight segment: {receiver_report:?}"
    );
}
