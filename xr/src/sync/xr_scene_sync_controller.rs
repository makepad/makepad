use crate::prelude::*;
use makepad_widgets::{makepad_derive_widget::*, widget::*};

const ACTIVITY_POSE_SYNC_INTERVAL_SECONDS: f64 = 0.6;
const ACTIVITY_POSE_SYNC_POSITION_EPSILON_METERS: f32 = 0.015;
const ACTIVITY_POSE_SYNC_ROTATION_EPSILON_DEGREES: f32 = 1.5;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.XrSceneSyncControllerBase = #(XrSceneSyncController::register_widget(vm))
    mod.widgets.XrSceneSyncController = set_type_default() do mod.widgets.XrSceneSyncControllerBase{}
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrSceneSyncController {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    network_started: bool,
    #[rust]
    suppress_activity_broadcast: Option<XrActivityId>,
    #[rust]
    pending_shared_scene_reset: bool,
    #[rust]
    last_activity_pose_sync: Option<Pose>,
    #[rust]
    last_activity_pose_sync_activity: Option<XrActivityId>,
    #[rust]
    last_activity_pose_sync_at: f64,
}

impl XrSceneSyncController {
    fn root_widget_ref(&self, cx: &Cx) -> WidgetRef {
        cx.widget_tree().widget(cx.widget_tree().root_uid())
    }

    fn poses_match(left: Pose, right: Pose) -> bool {
        let translation_delta = (left.position - right.position).length();
        let rotation_dot = left.orientation.dot(right.orientation).abs().clamp(0.0, 1.0);
        let rotation_delta_degrees = (2.0 * rotation_dot.acos()).to_degrees();
        translation_delta <= ACTIVITY_POSE_SYNC_POSITION_EPSILON_METERS
            && rotation_delta_degrees <= ACTIVITY_POSE_SYNC_ROTATION_EPSILON_DEGREES
    }

    fn current_activity(&self, ui: &WidgetRef, cx: &mut Cx) -> Option<XrActivityId> {
        ui.widget(cx, ids!(scene_select))
            .borrow::<XrSelect>()
            .map(|select| select.activity_id())
    }

    fn active_scene_widget(&self, ui: &WidgetRef, cx: &mut Cx) -> Option<WidgetRef> {
        ui.widget(cx, ids!(scene_select))
            .borrow::<XrSelect>()
            .and_then(|select| select.active_child_widget_ref())
    }

    fn apply_activity(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        activity_id: XrActivityId,
    ) -> Option<WidgetRef> {
        ui.widget(cx, ids!(scene_select))
            .borrow_mut::<XrSelect>()
            .and_then(|mut select| select.set_activity(cx, activity_id))
    }

    fn ensure_network_started(&mut self, ui: &WidgetRef, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        if let Some(mut peer_sync) = ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow_mut::<XrPeerSync>()
        {
            peer_sync.set_enabled(cx, true);
            self.network_started = true;
        }
    }

    fn ensure_activity_announced(&mut self, ui: &WidgetRef, cx: &mut Cx) {
        let Some(activity_id) = self.current_activity(ui, cx) else {
            return;
        };
        if let Some(mut peer_sync) = ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow_mut::<XrPeerSync>()
        {
            if peer_sync.enabled() && peer_sync.current_activity().is_none() {
                let _ = peer_sync.set_local_activity(cx, activity_id);
            }
        }
    }

    fn sync_authoritative_activity_pose(&mut self, ui: &WidgetRef, cx: &mut Cx) {
        let Some(activity_id) = self.current_activity(ui, cx) else {
            self.last_activity_pose_sync = None;
            self.last_activity_pose_sync_activity = None;
            self.last_activity_pose_sync_at = 0.0;
            return;
        };

        let should_sync = ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .is_some_and(|peer_sync| {
                peer_sync.enabled()
                    && peer_sync.connected_peer_count() != 0
                    && peer_sync.local_is_activity_authority()
            });
        if !should_sync {
            self.last_activity_pose_sync = None;
            self.last_activity_pose_sync_activity = None;
            self.last_activity_pose_sync_at = 0.0;
            return;
        }

        let Some(content_pose) = ui.borrow::<XrRoot>().and_then(|root| root.content_pose()) else {
            return;
        };
        let now = Cx::time_now();
        let activity_changed = self.last_activity_pose_sync_activity != Some(activity_id);
        let pose_changed = self
            .last_activity_pose_sync
            .is_none_or(|previous| !Self::poses_match(previous, content_pose));
        let interval_elapsed =
            now - self.last_activity_pose_sync_at >= ACTIVITY_POSE_SYNC_INTERVAL_SECONDS;
        if !(activity_changed || pose_changed || interval_elapsed) {
            return;
        }

        if let Some(mut peer_sync) = ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow_mut::<XrPeerSync>()
        {
            if peer_sync.send_activity_pose_reset(content_pose) {
                self.last_activity_pose_sync = Some(content_pose);
                self.last_activity_pose_sync_activity = Some(activity_id);
                self.last_activity_pose_sync_at = now;
            }
        }
    }

    fn refresh_spawnable_registry(&mut self, ui: &WidgetRef, cx: &mut Cx, force: bool) {
        let Some(activity_id) = self.current_activity(ui, cx) else {
            return;
        };
        let peer_sync_widget = ui.widget(cx, ids!(xr_peer_sync));
        let should_refresh = force
            || peer_sync_widget
                .borrow::<XrPeerSync>()
                .is_some_and(|peer_sync| peer_sync.spawnable_activity() != Some(activity_id));
        if !should_refresh {
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                peer_sync.flush_pending_shared_object_controls(cx);
            };
            return;
        }
        let Some(scene_widget) = self.active_scene_widget(ui, cx) else {
            return;
        };
        let bindings = collect_scene_spawnable_objects(activity_id, &scene_widget);
        if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
            peer_sync.set_spawnable_objects(activity_id, bindings);
            peer_sync.flush_pending_shared_object_controls(cx);
        };
    }

    fn apply_remote_body_spawn(&mut self, ui: &WidgetRef, cx: &mut Cx, spawn: XrBodySpawn) {
        if let Some(mut root) = ui.borrow_mut::<XrRoot>() {
            root.spawn_body(cx, spawn);
        }
    }

    fn apply_remote_body_despawn(&mut self, ui: &WidgetRef, cx: &mut Cx, widget_uid: WidgetUid) {
        if let Some(mut root) = ui.borrow_mut::<XrRoot>() {
            root.despawn_body(cx, widget_uid);
        }
    }

    fn apply_body_impulse(&mut self, ui: &WidgetRef, cx: &mut Cx, impulse: XrBodyImpulse) {
        if let Some(mut root) = ui.borrow_mut::<XrRoot>() {
            root.apply_body_impulse(cx, impulse);
        }
    }

    fn publish_local_shared_object_states(&mut self, ui: &WidgetRef, cx: &mut Cx) {
        let runtime_bodies = ui.borrow::<XrRoot>().map(|root| root.runtime_bodies());
        let Some(runtime_bodies) = runtime_bodies else {
            return;
        };
        let peer_sync_widget = ui.widget(cx, ids!(xr_peer_sync));
        if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
            peer_sync.publish_local_shared_object_states(cx, runtime_bodies.as_ref());
        };
    }

    fn apply_pending_shared_scene_reset(&mut self, ui: &WidgetRef, cx: &mut Cx) {
        if !self.pending_shared_scene_reset {
            return;
        }
        let runtime_bodies = ui.borrow::<XrRoot>().map(|root| root.runtime_bodies());
        let Some(runtime_bodies) = runtime_bodies else {
            return;
        };
        if runtime_bodies.is_empty() {
            return;
        }
        let reset_applied = {
            let peer_sync_widget = ui.widget(cx, ids!(xr_peer_sync));
            let reset_applied = if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                peer_sync.reset_local_shared_bootstrap_objects(runtime_bodies.as_ref());
                true
            } else {
                false
            };
            reset_applied
        };
        if reset_applied {
            self.pending_shared_scene_reset = false;
        }
    }

    fn handle_actions(&mut self, ui: &WidgetRef, cx: &mut Cx, actions: &Actions) {
        let root_uid = ui.widget_uid();
        let scene_select_uid = ui.widget(cx, ids!(scene_select)).widget_uid();
        let peer_sync_widget = ui.widget(cx, ids!(xr_peer_sync));
        let peer_sync_uid = peer_sync_widget.widget_uid();

        let mut remote_activity = None;
        let mut remote_body_spawns = Vec::new();
        let mut remote_body_impulses = Vec::new();
        let mut remote_body_despawns = Vec::new();
        let mut remote_activity_pose_reset = None;
        let mut local_activity = None;
        let mut local_body_spawns = Vec::new();
        let mut local_activity_pose_reset = None;
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
                    XrPeerSyncAction::ActivityPoseReset(pose) => {
                        remote_activity_pose_reset = Some(pose);
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
            if widget_action.widget_uid == root_uid {
                match widget_action.cast::<XrRootAction>() {
                    XrRootAction::PhysicsReset => {
                        self.pending_shared_scene_reset = true;
                    }
                    XrRootAction::ContentPoseReset(pose) => {
                        local_activity_pose_reset = Some(pose);
                    }
                    XrRootAction::None => {}
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
            self.refresh_spawnable_registry(ui, cx, true);
        }

        if let Some(activity_id) = remote_activity {
            if self.current_activity(ui, cx) != Some(activity_id) {
                self.suppress_activity_broadcast = Some(activity_id);
                if self.apply_activity(ui, cx, activity_id).is_none() {
                    self.suppress_activity_broadcast = None;
                }
            }
            self.refresh_spawnable_registry(ui, cx, true);
        }

        if let Some(activity_id) = local_activity {
            self.refresh_spawnable_registry(ui, cx, true);
            if self.suppress_activity_broadcast == Some(activity_id) {
                self.suppress_activity_broadcast = None;
            } else if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                let _ = peer_sync.set_local_activity(cx, activity_id);
            }
        }

        if let Some(pose) = remote_activity_pose_reset {
            if let Some(mut root) = ui.borrow_mut::<XrRoot>() {
                root.set_content_pose(cx, pose);
            }
            self.pending_shared_scene_reset = true;
        }

        if let Some(pose) = local_activity_pose_reset {
            self.pending_shared_scene_reset = true;
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                let _ = peer_sync.send_activity_pose_reset(pose);
            }
        }

        for widget_uid in remote_body_despawns {
            self.apply_remote_body_despawn(ui, cx, widget_uid);
        }

        for spawn in remote_body_spawns {
            self.apply_remote_body_spawn(ui, cx, spawn);
        }

        for impulse in remote_body_impulses {
            self.apply_body_impulse(ui, cx, impulse);
        }

        if !local_body_spawns.is_empty() {
            self.refresh_spawnable_registry(ui, cx, false);
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                for spawn in local_body_spawns {
                    if let Some(spawn) = peer_sync.send_local_body_spawn(spawn) {
                        self.apply_remote_body_spawn(ui, cx, spawn);
                    }
                }
            }
        }
    }

}

impl Widget for XrSceneSyncController {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let root = self.root_widget_ref(cx);
        if let Event::Actions(actions) = event {
            self.handle_actions(&root, cx, actions);
        }
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::Startup) {
            self.ensure_network_started(&root, cx);
        }
        self.ensure_activity_announced(&root, cx);
        self.sync_authoritative_activity_pose(&root, cx);
        self.refresh_spawnable_registry(&root, cx, false);
        self.apply_pending_shared_scene_reset(&root, cx);
        if matches!(event, Event::XrUpdate(_))
            || (matches!(event, Event::NextFrame(_)) && !cx.in_xr_mode())
        {
            self.publish_local_shared_object_states(&root, cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}
