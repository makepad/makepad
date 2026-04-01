use crate::scene::{
    arm_pair_metrics, flat_head_forward, hand_closed_fist_contact_point_geometry_only,
};
use crate::prelude::*;
use makepad_widgets::event::{XrSyncAnchor, XrSyncAnchorExtrema};
use std::{
    collections::{HashMap, VecDeque},
    sync::{mpsc::TryRecvError, Arc, Mutex},
    time::{Duration, Instant},
};

#[path = "alignment.rs"]
mod alignment;
#[path = "diagnostics.rs"]
mod diagnostics;
#[path = "lifecycle.rs"]
mod lifecycle;
#[path = "local_state.rs"]
mod local_state;
#[path = "metrics.rs"]
mod metrics;
#[path = "registry.rs"]
mod registry;
#[path = "render.rs"]
mod render;
#[path = "shared_objects.rs"]
mod shared_objects;
#[path = "space.rs"]
mod space;

use self::{alignment::*, diagnostics::*, local_state::*, metrics::*, registry::*};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.XrPeerSyncBase = #(XrPeerSync::register_widget(vm))
    mod.widgets.XrPeerSync = set_type_default() do mod.widgets.XrPeerSyncBase{
        body: mod.widgets.XrBodyKind.Disabled
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum XrPeerSyncAction {
    ActivityChanged(XrActivityId),
    ActivityPoseReset(Pose),
    BodySpawn(XrBodySpawn),
    BodyImpulse(XrBodyImpulse),
    BodyDespawn(WidgetUid),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrPeerSync {
    #[redraw]
    #[live]
    draw_cube: DrawCube,
    #[live(false)]
    auto_alignment_enabled: bool,
    #[rust]
    enabled: bool,
    #[rust]
    net_config_override: Option<XrNetConfig>,
    #[rust]
    runtime: XrPeerSyncRuntime,
    #[rust]
    diagnostics: XrPeerSyncDiagnostics,
    #[cast]
    #[deref]
    node: XrNode,
}

impl XrPeerSync {
    const HEADSET_SIZE: Vec3f = Vec3f {
        x: 0.12,
        y: 0.05,
        z: 0.08,
    };
    const HAND_SIZE: Vec3f = Vec3f {
        x: 0.08,
        y: 0.05,
        z: 0.10,
    };
    const REMOTE_HAND_PALM_SIZE: Vec3f = Vec3f {
        x: 0.055,
        y: 0.018,
        z: 0.075,
    };
    const REMOTE_HAND_JOINT_SIZE: f32 = 0.016;
    const ANCHOR_MARKER_SIZE: f32 = 0.060;
    const ANCHOR_CONFIRMATION_SECONDS: f64 = 2.0;
    const LOCAL_SYNC_SAMPLE_PREVIEW_SECONDS: f64 = 1.0;
    const SYNC_MATCH_RECEIVE_WINDOW_SECONDS: f64 = 0.45;
    const SYNC_SAMPLE_PAIR_WINDOW_SECONDS: f64 = 0.10;
    const SYNC_SAMPLE_HISTORY_SECONDS: f64 = 1.5;
    const SYNC_SAMPLE_SESSION_RESET_SECONDS: f64 = 1.0;
    const SYNC_EXISTING_MARKER_REUSE_DISTANCE_METERS: f32 = 1.0;
    const DESCRIPTOR_SEND_MIN_CHANGE_PERCENT: f32 = 4.0;
    const SYNC_MATCH_ACTIVE_WINDOW_SECONDS: f64 = 1.35;
    const FIST_ACK_STICKY_WINDOW_SECONDS: f64 = 0.35;
    const FIST_ACK_MAX_VERTICAL_DELTA_METERS: f32 = 0.22;
    const FIST_ACK_MAX_DEPTH_DELTA_METERS: f32 = 0.22;
    const FIST_ACK_MIN_HAND_GAP_METERS: f32 = 0.06;
    const FIST_ACK_MAX_HAND_GAP_METERS: f32 = 0.78;
    const FIST_ACK_MIN_CHEST_DISTANCE_METERS: f32 = 0.10;
    const FIST_ACK_MAX_CHEST_DISTANCE_METERS: f32 = 1.05;
    const FIST_ACK_MAX_ARM_ELEVATION_DEGREES: f32 = 60.0;
    const DESCRIPTOR_MAX_HEIGHT_METERS: f32 = 2.00;
    const DESCRIPTOR_MIN_HEIGHT_METERS: f32 = 0.08;
    const DESCRIPTOR_CELL_FOOTPRINT: f32 = 0.62;
    const SHOW_LOCAL_DESCRIPTOR_DEBUG: bool = false;
    const CLOCK_PING_INTERVAL_SECONDS: f64 = 1.0;
    const SHARED_OBJECT_SHADOW_MAX_EXTRAPOLATION_SECONDS: f32 = 0.10;
    const SHARED_OBJECT_SHADOW_INTERPOLATION_DELAY_SECONDS: f64 = 0.12;
    const SHARED_OBJECT_SHADOW_REAPPLY_POSITION_EPSILON_METERS: f32 = 0.015;
    const SHARED_OBJECT_SHADOW_REAPPLY_ORIENTATION_EPSILON_DEGREES: f32 = 1.5;
    const SHARED_OBJECT_SHADOW_REAPPLY_LINVEL_EPSILON_MPS: f32 = 0.08;
    const SHARED_OBJECT_SHADOW_REAPPLY_ANGVEL_EPSILON_RADPS: f32 = 0.08;
    const SHARED_OBJECT_TAKEOVER_DISTANCE_METERS: f32 = 0.18;
    const SHARED_OBJECT_TAKEOVER_RELATIVE_SPEED_MAX: f32 = 2.4;
    const SHARED_OBJECT_TAKEOVER_EFFECTIVE_DELAY_SECONDS: f64 = 0.12;
    const SHARED_OBJECT_TAKEOVER_EFFECTIVE_TICK_OFFSET: u32 = 3;
    const SHARED_OBJECT_IMPULSE_DISTANCE_METERS: f32 = 0.16;
    const SHARED_OBJECT_IMPULSE_MIN_HAND_SPEED: f32 = 0.65;
    const SHARED_OBJECT_IMPULSE_SCALE: f32 = 0.08;
    const SHARED_OBJECT_BOOTSTRAP_OWNER_TAG: u64 = 0x626f6f7473747261;

    pub fn status_text(&self) -> &str {
        self.diagnostics.status_text()
    }

    pub fn connected_peer_count(&self) -> usize {
        self.runtime.registry.len()
    }

    pub fn local_is_activity_authority(&self) -> bool {
        let Some(activity) = self.runtime.accepted_activity else {
            return false;
        };
        self.runtime
            .net_node
            .as_ref()
            .is_some_and(|node| node.node_id() == activity.changed_by)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn auto_alignment_enabled(&self) -> bool {
        self.auto_alignment_enabled
    }

    pub fn current_activity(&self) -> Option<XrActivityId> {
        Some(self.runtime.accepted_activity?.activity_id)
    }

    pub fn spawnable_activity(&self) -> Option<XrActivityId> {
        self.runtime.shared_objects.activity_id()
    }

    pub fn shared_object_count(&self) -> usize {
        self.runtime.shared_objects.active_count()
    }

    pub fn pending_shared_object_control_count(&self) -> usize {
        self.runtime.pending_shared_object_controls.len()
    }

    pub fn tx_body_spawn_count(&self) -> u64 {
        self.runtime.metrics.tx_body_spawn_count
    }

    pub fn tx_shared_object_state_count(&self) -> u64 {
        self.runtime.metrics.tx_shared_object_state_count
    }

    pub fn rx_body_spawn_count(&self) -> u64 {
        self.runtime.metrics.rx_body_spawn_count
    }

    pub fn rx_shared_object_state_count(&self) -> u64 {
        self.runtime.metrics.rx_shared_object_state_count
    }

    pub fn remote_shadow_apply_count(&self) -> u64 {
        self.runtime.metrics.remote_shadow_apply_count
    }

    pub fn last_network_event_label(&self) -> &str {
        self.runtime.metrics.last_event_label()
    }

    pub fn network_status_text(&self) -> &str {
        self.diagnostics.network_status_text()
    }

    pub fn clock_synced_peer_count(&self) -> usize {
        self.runtime
            .registry
            .peers
            .values()
            .filter(|peer| peer.clock_offset_seconds.is_some())
            .count()
    }

    pub fn clock_ping_tx_count(&self) -> u64 {
        self.runtime.metrics.tx_clock_ping_count
    }

    pub fn clock_ping_rx_count(&self) -> u64 {
        self.runtime.metrics.rx_clock_ping_count
    }

    pub fn clock_pong_tx_count(&self) -> u64 {
        self.runtime.metrics.tx_clock_pong_count
    }

    pub fn clock_pong_rx_count(&self) -> u64 {
        self.runtime.metrics.rx_clock_pong_count
    }

    pub fn non_xr_draw_clock_count(&self) -> u64 {
        self.runtime.metrics.non_xr_draw_clock_count
    }

    pub fn alignment_debug_text(&self) -> &str {
        self.diagnostics.alignment_debug_text()
    }

    pub fn touch_sync_status_text(&self) -> String {
        if !self.enabled {
            return "Touch sync: off".to_string();
        }
        let manual_status = self.manual_touch_sync_status_text();
        if self.auto_alignment_enabled && manual_status == "Touch sync: idle" {
            return "Touch sync: auto align on".to_string();
        }
        manual_status
    }

    pub fn local_touch_signal_text(&self) -> String {
        if !self.enabled {
            return "TouchRaw: off".to_string();
        }
        let Some(state) = self.runtime.local.latest_xr_state.as_ref() else {
            return "TouchRaw: waiting for local XR".to_string();
        };
        Self::touch_signal_text_for_state("TouchRaw", "TouchLF", "TouchRF", state)
    }

    pub fn remote_touch_signal_text(&self) -> String {
        if !self.enabled {
            return "TouchPeer: off".to_string();
        }
        let Some((peer_id, peer_state)) = self.runtime.registry.preferred_peer() else {
            return "TouchPeer: waiting for peer".to_string();
        };
        let Some(state_frame) = peer_state.latest_state.as_ref() else {
            return format!("TouchPeer {:08x}: waiting for peer XR", peer_id.0);
        };
        Self::touch_signal_text_for_state(
            &format!("TouchPeer {:08x}", peer_id.0),
            "TouchPLF",
            "TouchPRF",
            &state_frame.state,
        )
    }

    pub fn alignment_state_text(&self) -> &str {
        self.diagnostics.alignment_state_text()
    }

    pub fn peer_scene_text(&self) -> &str {
        self.diagnostics.peer_scene_text()
    }

    pub fn aligned_peer_height_map(&self) -> Option<XrDepthAlignHeightMap> {
        let (_, peer_state) = self.runtime.registry.preferred_peer()?;
        let transform = peer_state.descriptor_remote_to_local.or_else(|| {
            peer_state
                .last_solve_diagnostic
                .and_then(|diagnostic| diagnostic.best_solution)
                .map(|solution| solution.remote_to_local_transform())
        })?;
        let descriptor = peer_state.latest_descriptor?.descriptor;
        descriptor.transformed(&transform).height_map
    }

    pub fn raw_peer_alignment_descriptor(
        &self,
    ) -> Option<(XrNetPeerId, XrNetAlignmentDescriptorFrame)> {
        let (peer_id, peer_state) = self.runtime.registry.preferred_peer()?;
        Some((peer_id, peer_state.latest_descriptor?))
    }

    pub fn raw_peer_height_map(&self) -> Option<XrDepthAlignHeightMap> {
        let (_, descriptor) = self.raw_peer_alignment_descriptor()?;
        descriptor.descriptor.height_map
    }

    pub fn raw_alignment_dump_pair(&self) -> Option<XrNetAlignmentDescriptorDumpPair> {
        let local_descriptor = self.runtime.local.descriptor.clone()?;
        let (peer_id, remote_descriptor) = self.raw_peer_alignment_descriptor()?;
        if local_descriptor.descriptor.height_map.is_none()
            || remote_descriptor.descriptor.height_map.is_none()
        {
            return None;
        }
        Some(XrNetAlignmentDescriptorDumpPair::new(
            peer_id,
            local_descriptor,
            remote_descriptor,
        ))
    }

    pub fn local_slice_preview(&self) -> Option<XrDepthAlignSlicePreview> {
        self.runtime.local.slice_preview.clone()
    }

    fn apply_surface_analysis_enabled(&self, cx: &mut Cx) {
        cx.xr_tsdf()
            .set_surface_analysis_enabled(self.enabled && self.auto_alignment_enabled);
    }

    pub fn set_enabled(&mut self, cx: &mut Cx, enabled: bool) -> bool {
        if self.enabled == enabled {
            return self.enabled;
        }
        self.enabled = enabled;
        self.apply_surface_analysis_enabled(cx);
        self.runtime = XrPeerSyncRuntime::default();
        self.diagnostics = XrPeerSyncDiagnostics::default();

        if enabled {
            if self.auto_alignment_enabled {
                self.runtime.alignment_worker = Some(XrPeopleAlignmentWorker::new(cx.xr_tsdf()));
            }
            self.ensure_net_node();
            self.diagnostics
                .set_enabled_defaults(self.auto_alignment_enabled, self.runtime.net_node.is_some());
        } else {
            self.diagnostics.set_disabled();
        }
        self.redraw(cx);
        self.enabled
    }

    pub fn set_auto_alignment_enabled(&mut self, cx: &mut Cx, enabled: bool) -> bool {
        if self.auto_alignment_enabled == enabled {
            self.apply_surface_analysis_enabled(cx);
            return self.auto_alignment_enabled;
        }
        let restart = self.enabled;
        self.auto_alignment_enabled = enabled;
        if restart {
            self.set_enabled(cx, false);
            self.set_enabled(cx, true);
        } else {
            self.apply_surface_analysis_enabled(cx);
        }
        self.auto_alignment_enabled
    }

    pub fn set_net_config_override(&mut self, config: XrNetConfig) {
        self.net_config_override = Some(config);
    }

    pub fn set_local_activity(
        &mut self,
        _cx: &mut Cx,
        activity_id: XrActivityId,
    ) -> Option<XrNetActivityState> {
        if !self.enabled {
            return None;
        }
        self.ensure_net_node();
        let changed_at = if self.runtime.local.state_time != 0.0 {
            self.runtime.local.state_time
        } else {
            Cx::time_now()
        };
        let local_node_id = self.runtime.net_node.as_ref()?.node_id();
        if self.runtime.accepted_activity.is_some_and(|current| {
            current.activity_id == activity_id && current.changed_by == local_node_id
        }) {
            return self.runtime.accepted_activity;
        }
        let state = self
            .runtime
            .net_node
            .as_mut()?
            .send_activity(activity_id, changed_at);
        self.runtime.accepted_activity = Some(state);
        self.runtime.registry.clear_remote_activity_poses();
        self.runtime.metrics.record_activity_tx(state);
        Some(state)
    }

    pub fn set_spawnable_objects<I>(&mut self, activity_id: XrActivityId, bindings: I) -> usize
    where
        I: IntoIterator<Item = XrSpawnableObjectBinding>,
    {
        self.runtime
            .shared_objects
            .replace_spawnables(activity_id, bindings);
        self.runtime.shared_objects.len()
    }
}

impl Widget for XrPeerSync {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(set_enabled) {
            let mut enabled = self.enabled;
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                enabled = vm
                    .bx
                    .heap
                    .cast_to_bool(vm.bx.heap.vec_value(args_obj, 0, trap));
            }
            let enabled = vm.with_cx_mut(|cx| self.set_enabled(cx, enabled));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(enabled));
        }
        if method == live_id!(toggle_enabled) || method == live_id!(toggle_test) {
            let enabled = vm.with_cx_mut(|cx| self.set_enabled(cx, !self.enabled));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(enabled));
        }
        if method == live_id!(enabled) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(self.enabled));
        }
        if method == live_id!(set_auto_alignment_enabled) || method == live_id!(set_auto_align) {
            let mut enabled = self.auto_alignment_enabled;
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                enabled = vm
                    .bx
                    .heap
                    .cast_to_bool(vm.bx.heap.vec_value(args_obj, 0, trap));
            }
            let enabled = vm.with_cx_mut(|cx| self.set_auto_alignment_enabled(cx, enabled));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(enabled));
        }
        if method == live_id!(auto_alignment_enabled) || method == live_id!(auto_align) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(self.auto_alignment_enabled));
        }
        self.node.script_call(vm, method, args)
    }

    fn script_result(&mut self, vm: &mut ScriptVm, id: ScriptAsyncId, result: ScriptValue) {
        self.node.script_result(vm, id, result);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.enabled {
            if let Event::XrUpdate(update) = event {
                self.refresh_from_local_state(cx, update.state.as_ref());
            } else if !cx.in_xr_mode() {
                if let Some(local_time) = Self::timed_event_local_time(event) {
                    self.service_non_xr_local_clock(local_time);
                }
            }
            self.poll_network(cx);
            self.apply_alignment_results(cx);
            self.refresh_status();
        }
        self.node.handle_event(cx, event, scope);
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if !self.enabled {
            return self.node.draw_3d(cx, scope);
        }
        if cx.scene_state_3d().is_none() {
            return DrawStep::done();
        }
        let world = if cx.cx.in_xr_mode() {
            Mat4f::identity()
        } else {
            self.node.local_transform()
        };
        self.draw_cube.begin_many_instances(cx);
        self.draw_pending_sync_anchor_preview(cx, &world);
        self.draw_recent_anchor_confirmation(cx, &world);
        if self.auto_alignment_enabled && Self::SHOW_LOCAL_DESCRIPTOR_DEBUG {
            self.draw_local_descriptor(cx, &world);
        }
        self.draw_remote_peers(cx, &world);
        self.draw_cube.end_many_instances(cx);
        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}

#[cfg(test)]
include!("../tests/scene/xr_peer_sync_descriptor_pair.rs");
#[cfg(test)]
include!("../tests/scene/xr_peer_sync_alignment.rs");
#[cfg(test)]
include!("../tests/scene/xr_peer_sync_touch_sync.rs");
#[cfg(test)]
include!("../tests/scene/xr_peer_sync_shared_objects.rs");
