//! Declarative live controls on ordinary Splash widgets.
//!
//! `bind := "node.port"` writes and follows a control; `show := "node.port"`
//! only reads. Containers may declare `flow_scope := "alias"` and
//! `flow_path := "component"`. The host owns aliases, instance lifetimes,
//! admission and the single bounded write queue. Widgets own none of these.
//! Repeated UI instances inherit literal scope/path metadata unchanged and
//! intentionally bind as twins. Distinct graph occurrences require distinct
//! flow_path values supplied by the caller; widget ids never imply paths.

use makepad_flow::document::binding::{self, ControlKind, DeclaredEndpoint, EndpointBinding};
use makepad_flow::document::interface::FlowInterface;
use makepad_widgets::*;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowBindingAddress {
    pub scope: String,
    pub node: String,
    pub port: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlowBindingValue {
    Number(f64),
    Toggle(bool),
    Pulse,
}

#[derive(Clone, Debug)]
pub struct FlowBindingEndpoint<T> {
    pub target: T,
    pub class: EndpointBinding,
}

#[derive(Clone, Debug)]
pub enum FlowBindingWrite<R> {
    Admitted(Option<FlowBindingValue>),
    Queued(R),
    Refused(String),
}

/// The embedding host implements this once, never once per control.
pub trait FlowBindingHost {
    /// Includes host, instance incarnation, generation and endpoint identity.
    type Target: Clone + PartialEq;
    type Receipt: Clone + Eq;

    fn resolve(&self, address: &FlowBindingAddress) -> Result<FlowBindingEndpoint<Self::Target>, String>;
    /// Read exactly this target. A stale target or unavailable observation
    /// returns None. Controls return accepted targets, probes measured values;
    /// neither returns a queued intent or an authored default.
    fn read(&self, target: &Self::Target) -> Option<FlowBindingValue>;
    fn write(&mut self, target: &Self::Target, value: FlowBindingValue) -> FlowBindingWrite<Self::Receipt>;
    /// None means still pending; success carries the admitted normalized value.
    fn poll(&mut self, receipt: &Self::Receipt) -> Option<Result<Option<FlowBindingValue>, String>>;
    /// Release receipt tracking. This must not retarget or replay its write.
    fn forget(&mut self, receipt: Self::Receipt);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowBindingStatus {
    Ready,
    Admitted,
    Queued,
    Unbound(String),
    Refused(String),
}

#[derive(Clone, Debug)]
pub struct FlowBindingInteraction {
    pub widget: WidgetUid,
    pub address: FlowBindingAddress,
    pub value: FlowBindingValue,
    pub status: FlowBindingStatus,
}

/// Inspectable provenance for one declared UI binding. `accepted` is sampled
/// from this exact resolved target by the host, independently of the widget's
/// displayed intent and the binder's receipt history. A stale/unavailable
/// target and a write-only trigger have no accepted readout.
#[derive(Clone)]
pub struct FlowBindingSnapshot<T> {
    pub widget: WidgetRef,
    /// Declarative widget caption, when supplied. Never its mutable value text.
    pub label: Option<String>,
    pub address: FlowBindingAddress,
    pub writes: bool,
    pub endpoint: Option<FlowBindingEndpoint<T>>,
    pub status: FlowBindingStatus,
    pub accepted: Option<FlowBindingValue>,
}

struct BoundWidget<T> {
    widget: WidgetRef,
    label: Option<String>,
    address: FlowBindingAddress,
    writes: bool,
    control: ControlKind,
    error: Option<String>,
    endpoint: Option<FlowBindingEndpoint<T>>,
    status: FlowBindingStatus,
}

struct Pending<R> {
    receipt: R,
    sequence: u64,
    value: FlowBindingValue,
}

struct TargetState<T, R> {
    target: T,
    sequence: u64,
    pending: Vec<Pending<R>>,
    accepted: Option<FlowBindingValue>,
    status: FlowBindingStatus,
}

/// Mount in the widget tree's owning VM. Refresh on the host's existing
/// control tick, even when no graph or engine snapshot changed.
pub struct FlowBindings<T, R> {
    widgets: Vec<BoundWidget<T>>,
    targets: Vec<TargetState<T, R>>,
    errors: Vec<String>,
    gated: HashSet<WidgetUid>,
}

impl<T, R> Default for FlowBindings<T, R> {
    fn default() -> Self {
        Self { widgets: Vec::new(), targets: Vec::new(), errors: Vec::new(), gated: HashSet::new() }
    }
}

impl<T: Clone + PartialEq, R: Clone + Eq> FlowBindings<T, R> {
    /// Recollect after initial apply or a tree/source reload. All old widget
    /// refs are replaced. Receipts survive only while an exactly equal host
    /// target is still bound; refresh retires removed/replaced targets.
    /// The caller certifies that the whole tree belongs to this VM; otherwise
    /// use rebuild_excluding for separately mounted surfaces.
    pub fn rebuild<H: FlowBindingHost<Target = T, Receipt = R>>(
        &mut self,
        vm: &mut ScriptVm,
        root: &WidgetRef,
        default_scope: &str,
        host: &mut H,
    ) {
        self.rebuild_excluding(vm, root, default_scope, &[], host);
    }

    /// The host identifies any subtrees mounted in another VM or managed
    /// by another binder. Excluded roots are not inspected or traversed.
    pub fn rebuild_excluding<H: FlowBindingHost<Target = T, Receipt = R>>(
        &mut self,
        vm: &mut ScriptVm,
        root: &WidgetRef,
        default_scope: &str,
        excluded: &[WidgetUid],
        _host: &mut H,
    ) {
        self.widgets.clear();
        self.errors.clear();
        let scope = BindingScope { alias: default_scope.to_owned(), path: String::new(), error: None };
        self.collect(vm, root, &scope, excluded, &mut HashSet::new(), 0);
    }

    /// Collect only host-selected ordinary UI roots. Each root starts with
    /// the default alias and an empty component path; aliases/paths declared
    /// on that root are included. Repeated widget identities are visited once.
    pub fn rebuild_roots<H: FlowBindingHost<Target = T, Receipt = R>>(
        &mut self,
        vm: &mut ScriptVm,
        roots: &[WidgetRef],
        default_scope: &str,
        _host: &mut H,
    ) {
        self.widgets.clear();
        self.errors.clear();
        let scope = BindingScope { alias: default_scope.to_owned(), path: String::new(), error: None };
        let mut seen = HashSet::new();
        for root in roots { self.collect(vm, root, &scope, &[], &mut seen, 0); }
    }

    /// Point the writing binding of `widget` at `node.port` in the scope it
    /// was declared in. The next `refresh` resolves the new address, retires
    /// the old target and presents the new value. Returns whether anything
    /// changed. A rebuild restores the authored address; the host re-applies
    /// its targets after one.
    pub fn retarget(&mut self, widget: WidgetUid, node: &str, port: &str) -> bool {
        let mut changed = false;
        for binding in self.widgets.iter_mut().filter(|binding| binding.writes && binding.widget.widget_uid() == widget) {
            if binding.address.node == node && binding.address.port == port {
                continue;
            }
            binding.address.node = node.to_string();
            binding.address.port = port.to_string();
            binding.error = None;
            binding.endpoint = None;
            binding.status = FlowBindingStatus::Ready;
            changed = true;
        }
        changed
    }

    pub fn statuses(&self) -> impl Iterator<Item = (WidgetUid, &FlowBindingAddress, &FlowBindingStatus)> {
        self.widgets.iter().map(|binding| (binding.widget.widget_uid(), &binding.address, &binding.status))
    }

    /// Sample binding provenance without resolving an address again, polling
    /// receipts, changing controls or falling back to an earlier accepted
    /// value. Call after refresh when the host's aliases or generation change.
    pub fn snapshots<H: FlowBindingHost<Target = T, Receipt = R>>(&self, host: &H) -> Vec<FlowBindingSnapshot<T>> {
        self.widgets.iter().map(|binding| FlowBindingSnapshot {
            widget: binding.widget.clone(),
            label: binding.label.clone(),
            address: binding.address.clone(),
            writes: binding.writes,
            endpoint: binding.endpoint.clone(),
            status: binding.status.clone(),
            accepted: binding.endpoint.as_ref().and_then(|endpoint| host.read(&endpoint.target)),
        }).collect()
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Release observations on close. Pending writes remain owned by the
    /// host, which is responsible for stopping its instance and queue.
    pub fn clear<H: FlowBindingHost<Target = T, Receipt = R>>(&mut self, host: &mut H) {
        for state in self.targets.drain(..) {
            for pending in state.pending { host.forget(pending.receipt); }
        }
        self.widgets.clear();
        self.errors.clear();
    }

    fn collect(&mut self, vm: &ScriptVm, widget: &WidgetRef, inherited: &BindingScope, excluded: &[WidgetUid], seen: &mut HashSet<WidgetUid>, depth: usize) {
        if widget.is_empty() || excluded.contains(&widget.widget_uid()) || !seen.insert(widget.widget_uid()) { return; }
        if depth > 128 || seen.len() > 16384 {
            self.errors.push("flow binding tree exceeds the traversal limit".into());
            return;
        }
        let mut scope = inherited.clone();
        let source = widget.script_source();
        if source != ScriptObject::ZERO {
            match metadata_text(vm, source, "flow_scope") {
                Ok(Some(alias)) if !alias.is_empty() => {
                    scope.alias = alias;
                    scope.path.clear();
                }
                Ok(_) => {}
                Err(error) => scope.error = Some(error),
            }
            match metadata_text(vm, source, "flow_path") {
                Ok(Some(path)) if valid_path(&path) => {
                    if !path.is_empty() {
                        if !scope.path.is_empty() { scope.path.push('/'); }
                        scope.path.push_str(&path);
                    }
                }
                Ok(Some(_)) => scope.error = Some("flow_path must contain named component segments separated by /".into()),
                Ok(None) => {}
                Err(error) => scope.error = Some(error),
            }
            let bind = metadata_text(vm, source, "bind");
            let show = metadata_text(vm, source, "show");
            let both = matches!(&bind, Ok(Some(_))) && matches!(&show, Ok(Some(_)));
            for (writes, declaration) in [(true, bind), (false, show)] {
                // Media/provider widgets own their own bind/show extensions.
                // Do not reinterpret or disable them as scalar controls.
                let Some(control) = control_kind(widget) else { continue };
                let (text, mut error) = match declaration {
                    Ok(None) => continue,
                    Ok(Some(text)) => (text, scope.error.clone()),
                    Err(error) => (String::new(), Some(error)),
                };
                let (node, port) = match text.split_once('.') {
                    Some((node, port)) if !node.is_empty() && valid_path(node) && !port.is_empty() && !port.contains('/') => {
                        let node = if scope.path.is_empty() { node.to_owned() } else { format!("{}/{node}", scope.path) };
                        (node, port.to_owned())
                    }
                    _ => { error = Some("binding must name node.port (component nodes use /)".into()); (text, String::new()) }
                };
                if both { error = Some("one widget cannot declare both bind and show; bind already follows its accepted control".into()); }
                if writes && control == ControlKind::Readout { error = Some("this widget cannot write a live control".into()); }
                let status = error.clone().map(FlowBindingStatus::Unbound).unwrap_or(FlowBindingStatus::Ready);
                self.widgets.push(BoundWidget {
                    widget: widget.clone(),
                    label: metadata_text(vm, source, "text").ok().flatten().filter(|text| !text.trim().is_empty()),
                    address: FlowBindingAddress { scope: scope.alias.clone(), node, port },
                    writes, control, error, endpoint: None, status,
                });
            }
        }
        widget.children(&mut |_, child| self.collect(vm, &child, &scope, excluded, seen, depth + 1));
    }

    /// Original Actions remain intact for optional host policy (for example,
    /// touching a manual fader can cancel automation before this is called).
    /// Button events are visited in action order and never coalesced here.
    pub fn handle_actions<H: FlowBindingHost<Target = T, Receipt = R>>(
        &mut self, cx: &mut Cx, actions: &Actions, host: &mut H,
    ) -> Vec<FlowBindingInteraction> {
        let mut interactions = Vec::new();
        for action in actions {
            let Some(action) = action.as_widget_action() else { continue };
            for index in 0..self.widgets.len() {
                let binding = &self.widgets[index];
                if !binding.writes || binding.widget.widget_uid() != action.widget_uid { continue; }
                let value = match binding.control {
                    ControlKind::Number => match action.cast::<SliderAction>() {
                        SliderAction::Slide(value) | SliderAction::EndSlide(value) | SliderAction::TextSlide(value) | SliderAction::Reset(value) => Some(FlowBindingValue::Number(value)),
                        _ => None,
                    },
                    ControlKind::Bool => match action.cast::<CheckBoxAction>() {
                        CheckBoxAction::Change(value) => Some(FlowBindingValue::Toggle(value)),
                        _ => None,
                    },
                    ControlKind::Action if matches!(action.cast::<ButtonAction>(), ButtonAction::Clicked(_)) => Some(FlowBindingValue::Pulse),
                    _ => None,
                };
                let Some(value) = value else { continue };
                let widget = binding.widget.widget_uid();
                let address = binding.address.clone();
                let status = if let Some(endpoint) = binding.endpoint.clone() {
                    let position = self.target_index(&endpoint.target);
                    let state = &mut self.targets[position];
                    if matches!(action.cast::<SliderAction>(), SliderAction::EndSlide(_))
                        && (state.pending.iter().any(|pending| pending.sequence == state.sequence && pending.value == value)
                            || (state.status == FlowBindingStatus::Admitted && state.accepted == Some(value))) {
                        continue;
                    }
                    state.sequence = state.sequence.wrapping_add(1);
                    match host.write(&endpoint.target, value) {
                        FlowBindingWrite::Admitted(normalized) => {
                            state.status = FlowBindingStatus::Admitted;
                            if let Some(normalized) = normalized {
                                state.accepted = Some(normalized);
                                self.present_target(cx, &endpoint.target, normalized);
                            }
                        }
                        FlowBindingWrite::Queued(receipt) => {
                            // A coalescing host may reuse its receipt. Keep the
                            // newest intent and release it only once.
                            state.pending.retain(|pending| pending.receipt != receipt);
                            state.pending.push(Pending { receipt, sequence: state.sequence, value });
                            state.status = FlowBindingStatus::Queued;
                            self.present_target(cx, &endpoint.target, value);
                        }
                        FlowBindingWrite::Refused(reason) => { state.status = FlowBindingStatus::Refused(reason); }
                    }
                    self.targets[position].status.clone()
                } else {
                    binding.status.clone()
                };
                self.widgets[index].status = status.clone();
                interactions.push(FlowBindingInteraction { widget, address, value, status });
            }
        }
        // Re-resolving every binding is the host pump's (`refresh`), once per frame: a fader
        // sends an action per pointer move, and this runs for each of them.
        interactions
    }

    fn target_index(&mut self, target: &T) -> usize {
        if let Some(index) = self.targets.iter().position(|state| &state.target == target) { return index; }
        self.targets.push(TargetState { target: target.clone(), sequence: 0, pending: Vec::new(), accepted: None, status: FlowBindingStatus::Ready });
        self.targets.len() - 1
    }

    fn present_target(&self, cx: &mut Cx, target: &T, value: FlowBindingValue) {
        for binding in &self.widgets {
            if binding.writes && binding.endpoint.as_ref().is_some_and(|endpoint| &endpoint.target == target) {
                present(cx, &binding.widget, value);
            }
        }
    }

    /// Bound widgets the host holds disabled whatever their binding says (a
    /// deck without playable stems, a recording's clip controls), until the
    /// next call. Refresh writes each bound widget's disabled state once,
    /// from its binding and its gate together: a gate asserted after the
    /// binder's enable flips the control on every refresh, and each flip
    /// restarts its disable animation and asks for another frame.
    pub fn set_gated(&mut self, gated: impl IntoIterator<Item = WidgetUid>) {
        self.gated.clear();
        self.gated.extend(gated);
    }

    pub fn refresh<H: FlowBindingHost<Target = T, Receipt = R>>(&mut self, cx: &mut Cx, host: &mut H) {
        for binding in &mut self.widgets {
            let gated = self.gated.contains(&binding.widget.widget_uid());
            let resolved = match &binding.error {
                Some(error) => Err(error.clone()),
                None => host.resolve(&binding.address).and_then(|endpoint| {
                    // Share the document resolver's admission policy instead
                    // of letting each app invent widget/endpoint coercions.
                    let declared = [(binding.address.node.clone(), binding.address.port.clone(), binding.writes, binding.control)];
                    let endpoints = [DeclaredEndpoint { node: binding.address.node.clone(), name: binding.address.port.clone(), class: endpoint.class }];
                    let check = binding::resolve_all_with(&FlowInterface::default(), &declared, &endpoints);
                    match check.into_iter().next().and_then(|check| check.reason) {
                        Some(reason) => Err(reason),
                        None => Ok(endpoint),
                    }
                }),
            };
            match resolved {
                Ok(endpoint) => {
                    let changed = binding.endpoint.as_ref().is_none_or(|old| old.target != endpoint.target);
                    binding.endpoint = Some(endpoint);
                    if changed { binding.status = FlowBindingStatus::Ready; }
                    // A show-only slider/check box must never suggest that
                    // dragging it can alter its observed endpoint.
                    binding.widget.set_disabled(cx, gated || (!binding.writes && binding.control != ControlKind::Readout));
                }
                Err(reason) => {
                    binding.endpoint = None;
                    binding.status = FlowBindingStatus::Unbound(reason);
                    binding.widget.set_disabled(cx, true);
                }
            }
        }
        let mut index = 0;
        while index < self.targets.len() {
            if !self.widgets.iter().any(|binding| binding.endpoint.as_ref().is_some_and(|endpoint| endpoint.target == self.targets[index].target)) {
                let state = self.targets.remove(index);
                for pending in state.pending { host.forget(pending.receipt); }
            } else { index += 1; }
        }
        let resolved: Vec<T> = self.widgets.iter().filter_map(|binding| binding.endpoint.as_ref().map(|endpoint| endpoint.target.clone())).collect();
        for target in resolved { self.target_index(&target); }
        for index in 0..self.targets.len() {
            let state = &mut self.targets[index];
            let mut pending_index = 0;
            while pending_index < state.pending.len() {
                let Some(result) = host.poll(&state.pending[pending_index].receipt) else { pending_index += 1; continue };
                let pending = state.pending.remove(pending_index);
                host.forget(pending.receipt);
                if pending.value == FlowBindingValue::Pulse {
                    if let Err(reason) = &result {
                        // Every pulse matters, including an earlier failure
                        // followed by another pulse. Keep bounded diagnostics.
                        if self.errors.len() >= 128 { self.errors.remove(0); }
                        self.errors.push(format!("queued flow trigger refused: {reason}"));
                    }
                }
                if pending.sequence == state.sequence {
                    state.status = match result {
                        Ok(normalized) => {
                            if let Some(value) = normalized { state.accepted = Some(value); }
                            FlowBindingStatus::Admitted
                        }
                        Err(reason) => FlowBindingStatus::Refused(reason),
                    };
                }
            }
            // A receipt may predate a later automation/MIDI write in this
            // same pump. Current accepted readback wins over that receipt;
            // its normalized value remains a fallback when unavailable.
            if let Some(value) = host.read(&state.target) { state.accepted = Some(value); }
        }
        for binding in &mut self.widgets {
            let Some(endpoint) = &binding.endpoint else {
                if !binding.writes && binding.control == ControlKind::Readout { unavailable(cx, &binding.widget); }
                continue;
            };
            let state = self.targets.iter().find(|state| state.target == endpoint.target);
            if binding.writes {
                if let Some(state) = state {
                    binding.status = state.status.clone();
                    if let Some(pending) = state.pending.iter().rev().find(|pending| pending.sequence == state.sequence && pending.value != FlowBindingValue::Pulse) {
                        present(cx, &binding.widget, pending.value);
                        continue;
                    }
                    if let Some(value) = state.accepted {
                        present(cx, &binding.widget, value);
                        continue;
                    }
                }
            }
            if let Some(value) = host.read(&endpoint.target) {
                present(cx, &binding.widget, value);
            } else if !binding.writes && binding.control == ControlKind::Readout {
                unavailable(cx, &binding.widget);
            }
        }
    }
}

#[derive(Clone)]
struct BindingScope {
    alias: String,
    path: String,
    error: Option<String>,
}

fn valid_path(path: &str) -> bool {
    path.is_empty() || path.split('/').all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('.') && !part.chars().any(char::is_whitespace))
}

/// Nearest metadata wins; explicit nil stops inheritance. Both map fields
/// and named `:=` entries are supported, as in the existing face collector.
fn metadata_text(vm: &ScriptVm, mut object: ScriptObject, name: &str) -> Result<Option<String>, String> {
    let key: ScriptValue = LiveId::from_str(name).into();
    let mut seen = Vec::new();
    loop {
        if seen.len() >= 128 || seen.contains(&object) { return Err(format!("cyclic or over-deep {name} metadata")); }
        seen.push(object);
        let data = vm.bx.heap_ref();
        if let Some(value) = data.object_own_value(object, &key).or_else(|| data.vec_ref(object).iter().rev().find(|entry| entry.key == key).map(|entry| entry.value)) {
            if value.is_nil() { return Ok(None); }
            return vm.bx.heap_ref().string_with(value, |_heap, text| text.to_owned())
                .map(|text| (!text.is_empty()).then_some(text)).ok_or_else(|| format!("{name} must be a string"));
        }
        let Some(parent) = vm.bx.heap_ref().proto(object).as_object() else { return Ok(None) };
        object = parent;
    }
}

fn control_kind(widget: &WidgetRef) -> Option<ControlKind> {
    if widget.borrow::<Slider>().is_some() { Some(ControlKind::Number) }
    else if widget.borrow::<CheckBox>().is_some() { Some(ControlKind::Bool) }
    else if widget.borrow::<Button>().is_some() { Some(ControlKind::Action) }
    else if widget.borrow::<Label>().is_some() { Some(ControlKind::Readout) }
    else { None }
}

fn unavailable(cx: &mut Cx, widget: &WidgetRef) {
    if widget.text() != "—" { widget.set_text(cx, "—"); }
}

fn present(cx: &mut Cx, widget: &WidgetRef, value: FlowBindingValue) {
    let number = match value { FlowBindingValue::Number(value) if value.is_finite() => value, FlowBindingValue::Toggle(on) => if on { 1.0 } else { 0.0 }, _ => return };
    if widget.borrow::<Slider>().is_some() {
        let slider = widget.as_slider();
        if slider.value() != Some(number) { slider.set_value(cx, number); }
    } else if widget.borrow::<CheckBox>().is_some() {
        let check = widget.as_check_box();
        if check.active(cx) != (number >= 0.5) { check.set_active(cx, number >= 0.5, Animate::No); }
    } else if widget.borrow::<Button>().is_none() {
        let text = match value {
            FlowBindingValue::Toggle(on) => on.to_string(),
            _ if number.fract() == 0.0 && number.abs() < 1e9 => format!("{number:.0}"),
            _ => format!("{number:.3}"),
        };
        if widget.text() != text { widget.set_text(cx, &text); }
    }
}
