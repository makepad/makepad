//! The app surface: a package's bound Splash module, mounted in an isolate
//! of its own through the same host node faces use, shown as the pane on
//! the right of the window.
//!
//! The widget owns its [`FaceHost`]. The app mounts a surface into it,
//! feeds it the live instance's inputs, values and node states, and takes
//! its edits back as [`FaceBridgeCall`]s with `surface: true`, only for
//! controls whose binding resolved. Nothing here talks to the engine; the
//! app's worker lanes do, off the UI thread. The mounted root is a child of
//! this widget for every tree walk, so its controls can be found and
//! driven like any other widget.

use crate::faces::{BridgeCall, FaceBridgeCall, FaceHost};
use makepad_flow::document::binding::{self, Binding, BindingTarget, DeclaredEndpoint, EndpointBinding};
use makepad_flow::document::interface::FlowInterface;
use makepad_flow::{Graph, InstanceRow, NodeTypeCatalog, SequenceRef, ValueBytes, ValueRef};
use makepad_flow_ui::values::ValueCache;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SurfaceViewBase = #(SurfaceView::register_widget(vm))
    mod.widgets.SurfaceView = set_type_default() do mod.widgets.SurfaceViewBase{
        width: Fill
        height: Fill
    }
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct SurfaceView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[rust]
    area: Area,
    #[rust]
    host: Option<FaceHost>,
    #[rust]
    bindings: Vec<Binding>,
}

impl SurfaceView {
    /// Mount `source` for `instance`. The candidate is evaluated first; only
    /// a surface that evaluated replaces what was mounted before. A failure
    /// comes back as the error and leaves the previous surface in place.
    pub fn mount(
        &mut self,
        cx: &mut Cx,
        instance: &str,
        file_name: &str,
        source: &str,
        graph: &Graph,
        catalog: &[NodeTypeCatalog],
        registry: &makepad_flow::document::Registry,
    ) -> Result<(), String> {
        let parent = self.widget_uid();
        let host = FaceHost::mount_surface(cx, parent, instance, file_name, source, graph, catalog, registry);
        if let Some(error) = host.error.clone() {
            host.free(cx);
            return Err(error);
        }
        self.unmount(cx);
        self.host = Some(host);
        self.bindings.clear();
        cx.widget_tree_mark_dirty(parent);
        self.redraw(cx);
        Ok(())
    }

    pub fn unmount(&mut self, cx: &mut Cx) {
        if let Some(host) = self.host.take() {
            crate::frames::clear_host(cx, &host);
            host.free(cx);
            cx.widget_tree_mark_dirty(self.widget_uid());
        }
        self.bindings.clear();
        self.redraw(cx);
    }

    pub fn is_mounted(&self) -> bool {
        self.host.is_some()
    }

    /// Bring the mounted surface to the appearance the main VM shows, if it
    /// changed. The same widgets, leases and bindings stay; a bound control
    /// keeps its enabled state. Returns whether the surface was restyled, so
    /// the app can re-run its post-mount configuration of live widgets.
    pub fn apply_appearance(&mut self, cx: &mut Cx) -> bool {
        let Some(host) = self.host.as_mut() else {
            return false;
        };
        if !host.apply_appearance(cx) {
            return false;
        }
        host.apply_bindings(cx, &self.bindings);
        self.redraw(cx);
        true
    }

    /// Bind provider-backed frames after mount, or after the instance's
    /// admitted live generation changes.
    pub fn configure_live_widgets(&mut self, cx: &mut Cx, frames: &crate::frames::FrameSources) -> Vec<String> {
        let Some(host) = self.host.as_ref() else { return Vec::new(); };
        frames.bind_host(cx, host)
    }

    /// Resolve the surface's bindings against the flow as it is now: a
    /// control whose target is gone or of a type it cannot carry is
    /// disabled, not dropped, and comes back when the target does.
    ///
    /// `endpoints` are what the instance's running live plan admits: a
    /// control that names one of them writes the live host, a readout that
    /// names one follows it.
    pub fn apply_interface(&mut self, cx: &mut Cx, interface: &FlowInterface, endpoints: &[DeclaredEndpoint]) -> &[Binding] {
        if let Some(host) = self.host.as_mut() {
            let declared = host.declared_bindings();
            self.bindings = binding::resolve_all_with(interface, &declared, endpoints);
            host.apply_bindings(cx, &self.bindings);
        }
        &self.bindings
    }

    /// Whether `node.port` is bound for writing, to an input port
    /// (`endpoint` false) or to a live endpoint (`endpoint` true).
    fn bound_for_write(&self, node: &str, port: &str, endpoint: bool) -> bool {
        self.bindings.iter().any(|binding| {
            binding.writes
                && binding.node == node
                && binding.port == port
                && binding.target.as_ref().is_some_and(|target| {
                    if endpoint {
                        matches!(target, BindingTarget::Endpoint { class: EndpointBinding::Control | EndpointBinding::Toggle, .. })
                    } else {
                        matches!(target, BindingTarget::Input { .. })
                    }
                })
        })
    }

    fn bound_for_pulse(&self, node: &str, name: &str) -> bool {
        self.bindings.iter().any(|binding| binding.writes
            && binding.node == node && binding.port == name
            && matches!(binding.target, Some(BindingTarget::Endpoint { class: EndpointBinding::Trigger, .. })))
    }

    /// Every `(node, name)` of a live endpoint this surface shows or writes:
    /// what the app reads from the live host while the surface is up.
    pub fn endpoint_bindings(&self) -> Vec<(String, String, bool)> {
        self.bindings
            .iter()
            .filter(|binding| matches!(binding.target, Some(BindingTarget::Endpoint { class: EndpointBinding::Control | EndpointBinding::Toggle | EndpointBinding::Probe, .. })))
            .map(|binding| (binding.node.clone(), binding.port.clone(), binding.writes))
            .collect()
    }

    pub fn push_number(&mut self, cx: &mut Cx, node: &str, port: &str, value: f64) {
        if let Some(host) = self.host.as_mut() {
            host.push_number(cx, node, port, value);
        }
    }

    pub fn fill_inputs(&mut self, cx: &mut Cx, row: &InstanceRow) {
        if let Some(host) = self.host.as_mut() {
            host.fill_inputs(cx, row);
        }
    }

    /// Returns whether the surface wants the value's bytes.
    pub fn push_value(&mut self, cx: &mut Cx, node: &str, port: &str, value: &ValueRef, bytes: Option<&ValueBytes>) -> bool {
        self.host
            .as_mut()
            .is_some_and(|host| host.push_value(cx, node, port, value, bytes))
    }

    pub fn push_sequence(&mut self, cx: &mut Cx, node: &str, port: &str, sequence: &SequenceRef) {
        if let Some(host) = self.host.as_mut() {
            host.push_sequence(cx, node, port, sequence);
        }
    }

    pub fn push_state(&mut self, cx: &mut Cx, node: &str, state: &str) {
        if let Some(host) = self.host.as_mut() {
            host.push_state(cx, node, state);
        }
    }

    pub fn reset_run(&mut self) {
        if let Some(host) = self.host.as_mut() {
            host.reset_run();
        }
    }

    /// Every readout back to empty and every wanted digest forgotten: what
    /// an earlier graph revision produced is not this revision's value.
    pub fn clear_values(&mut self, cx: &mut Cx) {
        if let Some(host) = self.host.as_mut() {
            host.clear_values(cx);
        }
    }

    pub fn wanted(&self) -> Vec<String> {
        self.host.as_ref().map(|host| host.wanted.clone()).unwrap_or_default()
    }

    pub fn deliver_bytes(&mut self, cx: &mut Cx, cache: &mut ValueCache, digest: &str) {
        if let Some(host) = self.host.as_mut() {
            host.deliver_bytes(cx, cache, digest);
        }
    }

    /// Keyboard focus is inside the surface: the app routes key events
    /// only here.
    pub fn owns_key_focus(&self, cx: &Cx) -> bool {
        self.host.as_ref().is_some_and(|host| host.owns_key_focus(cx))
    }
}

impl WidgetNode for SurfaceView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }

    /// The mounted root is this widget's one child, so tree walks, `ui find`
    /// and by-path lookups reach the surface's controls.
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if let Some(root) = self.host.as_ref().and_then(FaceHost::surface_root) {
            visit(live_id!(surface), root);
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        if let Some(root) = self.host.as_ref().and_then(FaceHost::surface_root) {
            root.find_widgets_from_point(cx, point, found);
        }
    }
}

impl Widget for SurfaceView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let Some(host) = self.host.as_mut() else {
            return;
        };
        if matches!(event, Event::Draw(_)) {
            return;
        }
        host.handle_event(cx, event, scope, None);
        if let Event::Actions(actions) = event {
            let instance = host.instance.clone();
            let binds = host.bind_changes(cx, actions);
            let controls = host.control_changes(actions);
            let pulses = host.pulse_changes(actions);
            let run = host.surface_run_clicked(actions);
            let run_binding = host.flow_face.as_ref().and_then(|face| {
                let button = face.root.child(live_id!(run_button));
                face.binds.iter().find(|bind| bind.widget == button)
                    .map(|bind| (bind.node.clone(), bind.port.clone()))
            });
            // An admitted trigger named run_button emits its pulse once;
            // every existing unbound run button keeps its batch-run behavior.
            let run = run && !run_binding.is_some_and(|(node, name)| self.bound_for_pulse(&node, &name));
            for (node, name, value) in controls {
                // A control bound to a live endpoint writes while it moves.
                if self.bound_for_write(&node, &name, true) {
                    cx.action(FaceBridgeCall {
                        instance: instance.clone(),
                        surface: true,
                        call: BridgeCall::Control { node, name, value },
                    });
                }
            }
            for (node, name) in pulses {
                if self.bound_for_pulse(&node, &name) {
                    cx.action(FaceBridgeCall {
                        instance: instance.clone(), surface: true,
                        call: BridgeCall::Pulse { node, name },
                    });
                }
            }
            for (node, port, text) in binds {
                // Only a control whose binding resolved writes; a change on
                // an unbound (disabled) or stale one goes nowhere. One bound
                // to a live endpoint was written above and is not an input.
                if !self.bound_for_write(&node, &port, false) {
                    continue;
                }
                let value_json = makepad_strict_json::s(text).to_json();
                cx.action(FaceBridgeCall {
                    instance: instance.clone(),
                    surface: true,
                    call: BridgeCall::Input { node, port, value_json },
                });
            }
            if run {
                cx.action(FaceBridgeCall {
                    instance,
                    surface: true,
                    call: BridgeCall::Run { outputs: None },
                });
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        if let Some(host) = self.host.as_mut() {
            host.draw_surface(cx, Walk::fill());
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
