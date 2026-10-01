//! Widgets in a collect run (`MAKEPAD_RUN=collect-web`, see
//! `makepad_platform::collect`).
//!
//! The run sends one `Event::Scan` through the app. Every [`WidgetRef`] it
//! reaches records its type (with its path: who needs it), runs the
//! widget's [`Widget::scan`] hook and its `handle_event`, then forwards the
//! scan to every child `children()` names, so a widget whose event
//! handling skips hidden or inactive children is still visited, once.
//!
//! Widgets that create children from data (a list's rows, a switcher's
//! pages) spawn the variants they can create and forward the scan into
//! them, with [`scan_spawn`]. Which variants: a `scan:` list on the widget
//! ([`scan_list`]); `scan: [@Row, @Header]` names templates, an object
//! (`scan: [RowView{}]`) is one itself. The spawned widgets are kept and
//! drawn while the run lasts, so their shaders, glyphs and draw-time needs
//! are collected as any drawn widget's are.

use crate::{makepad_draw::*, widget::*, widget_tree::CxWidgetExt};

/// One entry of a `scan:` list.
#[derive(Clone, Debug)]
pub enum ScanEntry {
    /// A template of the widget's own, by name (`@Row` or `"Row"`).
    Name(LiveId),
    /// A template object (`RowView{}`).
    Template(ScriptObject),
}

/// The widget type's registered name, else its Rust type's last segment.
fn type_name(cx: &Cx, widget: &WidgetRef) -> String {
    let Some(type_id) = widget.widget_type_id() else { return "-".into() };
    let registry = cx.components.get::<WidgetRegistry>();
    registry.map.get(&type_id).map(|(info, _)| info.name.to_string()).unwrap_or_else(|| "-".into())
}

/// The scan of one widget (`WidgetRef::handle_event` with `Event::Scan`):
/// once per widget, recorded with its type and path, its own hook and
/// event handling, then every child.
pub(crate) fn scan_widget(widget: &WidgetRef, cx: &mut Cx, scan: &ScanEvent, event: &Event, scope: &mut Scope) {
    let Some(uid) = widget.try_widget_uid() else { return };
    if !scan.first_visit(uid.0) {
        return;
    }
    let ty = type_name(cx, widget);
    let path: Vec<String> = cx.widget_tree().path_to(uid).iter().map(|id| id.to_string()).collect();
    let who = if path.is_empty() { format!("{ty} in {}", scan.current_by()) } else { format!("{ty} at {}", path.join(".")) };
    let _by = scan.by(who);
    scan.need_widget_type(&ty);
    widget.scan_inner(cx, scan, event, scope);
    let mut children = Vec::new();
    widget.children(&mut |_, child| children.push(child));
    for child in children {
        child.handle_event(cx, event, scope);
    }
}

/// The widget's `scan:` list, read from its source object; `None` when it
/// has none.
pub fn scan_list(vm: &mut ScriptVm, source: ScriptObject) -> Option<Vec<ScanEntry>> {
    let list = vm.bx.heap.value(source, id!(scan).into(), NoTrap);
    let array = list.as_array()?;
    let mut out = Vec::new();
    for i in 0..vm.bx.heap.array_len(array) {
        let v = vm.bx.heap.array_index(array, i, NoTrap);
        if let Some(id) = v.as_id() {
            out.push(ScanEntry::Name(id));
        } else if let Some(obj) = v.as_object() {
            out.push(ScanEntry::Template(obj));
        } else if let Some(id) = vm.string_with(v, |_, s| LiveId::from_str(s)) {
            out.push(ScanEntry::Name(id));
        }
    }
    Some(out)
}

/// Spawns one variant from its template object and forwards the scan into
/// it. The caller keeps the returned widget and draws it while the run
/// lasts (its draw-time needs are collected then).
pub fn scan_spawn(cx: &mut Cx, scan: &ScanEvent, scope: &mut Scope, name: LiveId, template: ScriptObject) -> WidgetRef {
    let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template.into()));
    let _by = scan.by(format!("{} spawned {name}", scan.current_by()));
    widget.handle_event(cx, &Event::Scan(scan.clone()), scope);
    widget
}

/// The widgets a spawner made for the scan, drawn with it until the run
/// ends.
#[derive(Default)]
pub struct ScanSpawned {
    pub widgets: Vec<(LiveId, WidgetRef)>,
}

impl ScanSpawned {
    /// Spawns the variants of a widget: its `scan:` list when it has one
    /// (names resolved by `template`), else every template it holds.
    pub fn spawn(
        &mut self,
        cx: &mut Cx,
        scan: &ScanEvent,
        scope: &mut Scope,
        source: ScriptObject,
        templates: &[(LiveId, ScriptObject)],
    ) {
        let list = cx.with_vm(|vm| scan_list(vm, source));
        let wanted: Vec<(LiveId, ScriptObject)> = match list {
            None => templates.to_vec(),
            Some(list) => list
                .into_iter()
                .filter_map(|entry| match entry {
                    ScanEntry::Template(obj) => Some((LiveId(0), obj)),
                    ScanEntry::Name(id) => match templates.iter().find(|(t, _)| *t == id) {
                        Some(t) => Some(*t),
                        None => {
                            scan.note("warning", &format!("{}: `scan:` names `{id}`, which is not one of its templates", scan.current_by()));
                            None
                        }
                    },
                })
                .collect(),
        };
        for (name, template) in wanted {
            let widget = scan_spawn(cx, scan, scope, name, template);
            self.widgets.push((name, widget));
        }
    }

    /// Draws the spawned variants (collect runs only: the window is hidden).
    pub fn draw(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        for (_, widget) in &self.widgets {
            widget.draw_all(cx, scope);
        }
    }
}
