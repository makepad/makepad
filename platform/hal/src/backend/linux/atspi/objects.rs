use super::*;
use makepad_a11y::{
    ActionArg, ActionRequest, Actions, Node, RangeValue, Rect, Role, States, TableInfo, Toggle,
};
use makepad_dbus_wire::{MethodCall, MethodError, MethodResult, ObjectHandler};

#[derive(Default)]
pub(super) struct Shared {
    pub origins: HashMap<u64, Option<(f32, f32)>>,
    pub focus: HashMap<u64, bool>,
    pub parent: Option<ObjRef>,
    id: i32,
    pub pending: Vec<(u64, u64)>,
}
#[derive(Clone)]
pub(super) struct Objects {
    pub bus: String,
    address: String,
    pub shared: Arc<Mutex<Shared>>,
}
pub(super) struct View {
    pub key: u64,
    pub node: Node,
    pub parent: Option<NodeId>,
    pub index: i32,
    pub focused: bool,
    pub selection: bool,
}
pub(super) type CacheItem = (
    ObjRef,
    ObjRef,
    ObjRef,
    i32,
    i32,
    Vec<String>,
    String,
    u32,
    String,
    Vec<u32>,
);
pub(super) fn missing() -> MethodError {
    MethodError::new(
        "org.freedesktop.DBus.Error.UnknownObject",
        "Semantic node no longer exists",
    )
}
pub(super) fn invalid() -> MethodError {
    MethodError::invalid_args("Index, range or coordinate type is invalid")
}
pub(super) fn unsupported() -> MethodError {
    MethodError::new(
        "org.freedesktop.DBus.Error.NotSupported",
        "Operation is not supported by this node",
    )
}
pub(super) fn path(key: u64, id: NodeId) -> String {
    format!("{PREFIX}/{:x}_{:x}_{:x}", key >> 32, key as u32, id.0)
}
pub(super) fn parse(path: &str) -> Option<(u64, NodeId)> {
    let mut parts = path.strip_prefix(PREFIX)?.strip_prefix('/')?.split('_');
    let session = u32::from_str_radix(parts.next()?, 16).ok()?;
    let window = u32::from_str_radix(parts.next()?, 16).ok()?;
    let node = u64::from_str_radix(parts.next()?, 16).ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((a11y::window_key(session, window), NodeId(node)))
}
pub(super) fn roots() -> Vec<(u64, NodeId)> {
    let mut roots = a11y::with_mirror(|m| {
        m.trees
            .iter()
            .filter(|(_, t)| t.node(t.root).is_some())
            .map(|(k, t)| (*k, t.root))
            .collect::<Vec<_>>()
    });
    roots.sort_unstable();
    roots
}
pub(super) fn view(path: &str) -> MethodResult<View> {
    let (key, id) = parse(path).ok_or_else(missing)?;
    a11y::with_tree(key, |t| {
        let node = t.node(id)?.clone();
        Some(View {
            key,
            parent: t.parent(id),
            index: t.index_in_parent(id).map_or(-1, |i| i as i32),
            focused: t.focus == id,
            selection: t
                .children(id)
                .any(|c| c.states.contains(States::SELECTABLE)),
            node,
        })
    })
    .flatten()
    .ok_or_else(missing)
}
impl Objects {
    pub fn new(bus: String, address: String, shared: Arc<Mutex<Shared>>) -> Self {
        {
            let mut state = lock(&shared);
            state.parent = None;
            state.id = 0;
        }
        Self {
            bus,
            address,
            shared,
        }
    }
    pub fn reference(&self, path: &str) -> ObjRef {
        ObjRef {
            bus: self.bus.clone(),
            path: path.into(),
        }
    }
    pub fn node_ref(&self, key: u64, node: NodeId) -> ObjRef {
        self.reference(&path(key, node))
    }
    pub fn null(&self) -> ObjRef {
        ObjRef {
            bus: String::new(),
            path: "/org/a11y/atspi/null".into(),
        }
    }
    fn application_name(&self) -> String {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Makepad".into())
    }
    pub fn interfaces(&self, path: &str) -> MethodResult<Vec<String>> {
        let interfaces = if path == CACHE {
            vec![CACHE_INTERFACE]
        } else if path == ROOT {
            vec![ACCESSIBLE_INTERFACE, APPLICATION_INTERFACE]
        } else {
            let v = view(path)?;
            let mut interfaces = vec![ACCESSIBLE_INTERFACE, COMPONENT_INTERFACE];
            if !actions(&v.node).is_empty() {
                interfaces.push(ACTION_INTERFACE);
            }
            if v.node.range.is_some() {
                interfaces.push(VALUE_INTERFACE);
            }
            if v.node.text.is_some() {
                interfaces.push(TEXT_INTERFACE);
                if v.node.states.contains(States::EDITABLE)
                    && v.node.actions.contains(Actions::REPLACE_TEXT)
                {
                    interfaces.push(EDITABLE_TEXT_INTERFACE);
                }
            }
            if v.selection {
                interfaces.push(SELECTION_INTERFACE);
            }
            if v.node.table.is_some() {
                if matches!(v.node.role, Role::Table | Role::Grid) {
                    interfaces.push(TABLE_INTERFACE);
                } else if matches!(
                    v.node.role,
                    Role::Cell | Role::ColumnHeader | Role::RowHeader
                ) {
                    interfaces.push(TABLE_CELL_INTERFACE);
                }
            }
            interfaces
        };
        Ok(interfaces.into_iter().map(str::to_owned).collect())
    }
    fn has_interface(&self, path: &str, interface: &str) -> MethodResult<()> {
        if self.interfaces(path)?.iter().any(|i| i == interface) {
            Ok(())
        } else {
            Err(MethodError::new(
                "org.freedesktop.DBus.Error.UnknownInterface",
                interface,
            ))
        }
    }
    pub fn enqueue(&mut self, path: &str, action: Actions, arg: ActionArg) -> MethodResult<bool> {
        let v = view(path)?;
        let offered = if action == Actions::CUSTOM {
            matches!(&arg, ActionArg::Custom(name) if v.node.custom_actions.contains(name))
        } else {
            v.node.actions.contains(action)
        };
        if v.node.states.intersects(States::DISABLED | States::HIDDEN) || !offered {
            return Ok(false);
        }
        let req = a11y::inject_action(
            v.key,
            ActionRequest {
                req: 0,
                node: v.node.id,
                action,
                arg,
            },
        );
        lock(&self.shared).pending.push((req, v.key));
        let mut identity = lock(identity());
        identity.last_request = Some((req, v.key, v.node.id));
        identity.completed.clear(); // only the latest action is used by the loopback observer
        Ok(true)
    }
    pub fn reap_actions(&mut self) {
        let pending = std::mem::take(&mut lock(&self.shared).pending);
        let mut remaining = Vec::new();
        for (req, key) in pending {
            if let Some(result) = a11y::wait_done(req, Duration::ZERO) {
                let mut identity = lock(identity());
                if identity.last_request.is_some_and(|r| r.0 == req) {
                    identity.completed.insert(req, result);
                }
            } else {
                remaining.push((req, key));
            }
        }
        lock(&self.shared).pending.extend(remaining);
    }
    pub fn offset(&self, path: &str, coord: u32) -> MethodResult<(f32, f32)> {
        let v = view(path)?;
        match coord {
            x if x == AtspiCoordType::Window as u32 => Ok((0., 0.)),
            x if x == AtspiCoordType::Screen as u32 => lock(&self.shared)
                .origins
                .get(&v.key)
                .copied()
                .flatten()
                .ok_or_else(unsupported),
            x if x == AtspiCoordType::Parent as u32 => {
                let origin = v
                    .parent
                    .and_then(|p| {
                        a11y::with_tree(v.key, |t| t.node(p).map(|n| (n.bounds.x, n.bounds.y)))
                            .flatten()
                    })
                    .unwrap_or((0., 0.));
                Ok((-origin.0, -origin.1))
            }
            _ => Err(invalid()),
        }
    }
    pub fn rect(&self, path: &str, rect: Rect, coord: u32) -> MethodResult<(i32, i32, i32, i32)> {
        let (x, y) = self.offset(path, coord)?;
        let left = (rect.x + x).floor();
        let top = (rect.y + y).floor();
        Ok((
            left as i32,
            top as i32,
            ((rect.x + x + rect.w).ceil() - left) as i32,
            ((rect.y + y + rect.h).ceil() - top) as i32,
        ))
    }
    pub fn cache_item(&mut self, path: &str) -> MethodResult<CacheItem> {
        Ok((
            self.reference(path),
            self.reference(ROOT),
            AccessibleServer::prop_parent(self, path)?,
            AccessibleServer::get_index_in_parent(self, path)?,
            AccessibleServer::prop_child_count(self, path)?,
            self.interfaces(path)?,
            AccessibleServer::prop_name(self, path)?,
            AccessibleServer::get_role(self, path)?,
            AccessibleServer::prop_description(self, path)?,
            AccessibleServer::get_state(self, path)?,
        ))
    }
    fn selected(&self, path: &str, only_selected: bool) -> MethodResult<Vec<NodeId>> {
        let v = view(path)?;
        Ok(a11y::with_tree(v.key, |t| {
            t.children(v.node.id)
                .filter(|n| {
                    n.states.contains(if only_selected {
                        States::SELECTED
                    } else {
                        States::SELECTABLE
                    })
                })
                .map(|n| n.id)
                .collect()
        })
        .unwrap_or_default())
    }
    fn child_action(&mut self, path: &str, index: i32, select: bool) -> MethodResult<bool> {
        let child = AccessibleServer::get_child_at_index(self, path, index)?;
        let v = view(&child.path)?;
        let action = if !select {
            Actions::REMOVE_SELECTION
        } else if v.node.actions.contains(Actions::SELECT) {
            Actions::SELECT
        } else {
            Actions::ADD_SELECTION
        };
        self.enqueue(&child.path, action, ActionArg::None)
    }
    fn table(&self, path: &str) -> MethodResult<TableInfo> {
        view(path)?.node.table.ok_or_else(unsupported)
    }
    fn range(&self, path: &str) -> MethodResult<RangeValue> {
        view(path)?.node.range.ok_or_else(unsupported)
    }
    fn cells(&self, path: &str) -> MethodResult<Vec<Node>> {
        let v = view(path)?;
        // Walk only this table, not other tables in the same window; nested tables own their cells.
        let mut out = Vec::new();
        let mut stack = v.node.children;
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(n) = a11y::with_tree(v.key, |t| t.node(id).cloned()).flatten() else {
                continue;
            };
            if matches!(n.role, Role::Table | Role::Grid) {
                continue;
            }
            stack.extend(n.children.iter().copied());
            if matches!(n.role, Role::Cell | Role::ColumnHeader | Role::RowHeader)
                && n.table.is_some()
            {
                out.push(n);
            }
        }
        Ok(out)
    }

    fn cell(&self, path: &str, row: i32, col: i32) -> MethodResult<Option<Node>> {
        if row < 0 || col < 0 {
            return Err(invalid());
        }
        Ok(self.cells(path)?.into_iter().find(|n| {
            n.table.is_some_and(|t| {
                row as u32 >= t.row
                    && (row as u32) < t.row.saturating_add(t.row_span.max(1))
                    && col as u32 >= t.col
                    && (col as u32) < t.col.saturating_add(t.col_span.max(1))
            })
        }))
    }
}

macro_rules! handlers {
    ($(($interface:ident, $xml:ident, $dispatch:ident, $get:ident, $all:ident, $set:ident)),* $(,)?) => {
        impl ObjectHandler for Objects {
            fn call(&mut self, call: MethodCall<'_>) -> MethodResult<Body> {
                let interfaces = self.interfaces(call.path)?;
                $(if interfaces.iter().any(|i| i == $interface) {
                    if let Some(result) = $dispatch(self, &call) { return result; }
                })*
                Err(MethodError::unknown_method())
            }
            fn introspect(&self, path: &str) -> String {
                let interfaces = self.interfaces(path).unwrap_or_default();
                let mut xml = String::from("<node>");
                $(if interfaces.iter().any(|i| i == $interface) { xml.push_str($xml); })*
                xml.push_str("</node>"); xml
            }
            fn get_property(&mut self, path: &str, interface: &str, name: &str) -> MethodResult<Value> {
                self.has_interface(path, interface)?;
                match interface { $($interface => $get(self, path, name),)* _ => Err(unsupported()) }
            }
            fn get_all_properties(&mut self, path: &str, interface: &str) -> MethodResult<Vec<(String, Value)>> {
                self.has_interface(path, interface)?;
                match interface { $($interface => $all(self, path),)* _ => Err(unsupported()) }
            }
            fn set_property(&mut self, path: &str, interface: &str, name: &str, value: Value) -> MethodResult<()> {
                self.has_interface(path, interface)?;
                match interface { $($interface => $set(self, path, name, value),)* _ => Err(unsupported()) }
            }
        }
    }
}
handlers! {
    (ACCESSIBLE_INTERFACE, ACCESSIBLE_INTROSPECTION_XML, dispatch_accessible, get_accessible_property, get_accessible_all_properties, set_accessible_property),
    (APPLICATION_INTERFACE, APPLICATION_INTROSPECTION_XML, dispatch_application, get_application_property, get_application_all_properties, set_application_property),
    (COMPONENT_INTERFACE, COMPONENT_INTROSPECTION_XML, dispatch_component, get_component_property, get_component_all_properties, set_component_property),
    (ACTION_INTERFACE, ACTION_INTROSPECTION_XML, dispatch_action, get_action_property, get_action_all_properties, set_action_property),
    (VALUE_INTERFACE, VALUE_INTROSPECTION_XML, dispatch_value, get_value_property, get_value_all_properties, set_value_property),
    (TEXT_INTERFACE, TEXT_INTROSPECTION_XML, dispatch_text, get_text_property, get_text_all_properties, set_text_property),
    (EDITABLE_TEXT_INTERFACE, EDITABLE_TEXT_INTROSPECTION_XML, dispatch_editable_text, get_editable_text_property, get_editable_text_all_properties, set_editable_text_property),
    (SELECTION_INTERFACE, SELECTION_INTROSPECTION_XML, dispatch_selection, get_selection_property, get_selection_all_properties, set_selection_property),
    (TABLE_INTERFACE, TABLE_INTROSPECTION_XML, dispatch_table, get_table_property, get_table_all_properties, set_table_property),
    (TABLE_CELL_INTERFACE, TABLE_CELL_INTROSPECTION_XML, dispatch_table_cell, get_table_cell_property, get_table_cell_all_properties, set_table_cell_property),
    (CACHE_INTERFACE, CACHE_INTROSPECTION_XML, dispatch_cache, get_cache_property, get_cache_all_properties, set_cache_property),
}
macro_rules! version {
    () => {
        fn prop_version(&mut self, _: &str) -> MethodResult<u32> {
            Ok(1)
        }
    };
}
pub(super) use version;
impl AccessibleServer for Objects {
    version!();
    fn prop_name(&mut self, path: &str) -> MethodResult<String> {
        if path == ROOT {
            Ok(self.application_name())
        } else {
            Ok(view(path)?.node.name)
        }
    }
    fn prop_description(&mut self, path: &str) -> MethodResult<String> {
        if path == ROOT {
            Ok(String::new())
        } else {
            Ok(view(path)?.node.description)
        }
    }
    fn prop_parent(&mut self, path: &str) -> MethodResult<ObjRef> {
        if path == ROOT {
            return Ok(lock(&self.shared).parent.clone().unwrap_or(ObjRef {
                bus: REGISTRY.into(),
                path: ROOT.into(),
            }));
        }
        let v = view(path)?;
        Ok(v.parent
            .map_or_else(|| self.reference(ROOT), |id| self.node_ref(v.key, id)))
    }
    fn prop_child_count(&mut self, path: &str) -> MethodResult<i32> {
        Ok(self.get_children(path)?.len() as i32)
    }
    fn prop_locale(&mut self, _: &str) -> MethodResult<String> {
        Ok(locale())
    }
    fn prop_accessible_id(&mut self, path: &str) -> MethodResult<String> {
        Ok(path.rsplit('/').next().unwrap_or("").into())
    }
    fn prop_help_text(&mut self, path: &str) -> MethodResult<String> {
        AccessibleServer::prop_description(self, path)
    }
    fn get_children(&mut self, path: &str) -> MethodResult<Vec<ObjRef>> {
        if path == ROOT {
            return Ok(roots()
                .into_iter()
                .map(|(k, n)| self.node_ref(k, n))
                .collect());
        }
        let v = view(path)?;
        let children = a11y::with_tree(v.key, |t| {
            v.node
                .children
                .iter()
                .copied()
                .filter(|n| t.node(*n).is_some())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
        Ok(children
            .into_iter()
            .map(|n| self.node_ref(v.key, n))
            .collect())
    }
    fn get_child_at_index(&mut self, path: &str, index: i32) -> MethodResult<ObjRef> {
        if index < 0 {
            return Err(invalid());
        }
        self.get_children(path)?
            .get(index as usize)
            .cloned()
            .ok_or_else(invalid)
    }
    fn get_index_in_parent(&mut self, path: &str) -> MethodResult<i32> {
        if path == ROOT {
            return Ok(-1);
        }
        let v = view(path)?;
        Ok(if v.parent.is_some() {
            v.index
        } else {
            roots()
                .iter()
                .position(|r| *r == (v.key, v.node.id))
                .map_or(-1, |i| i as i32)
        })
    }
    fn get_relation_set(&mut self, path: &str) -> MethodResult<Vec<(u32, Vec<ObjRef>)>> {
        if path == ROOT {
            return Ok(vec![]);
        }
        let v = view(path)?;
        let r = &v.node.relations;
        let relations = [
            (AtspiRelationType::LabelledBy, &r.labelled_by),
            (AtspiRelationType::DescribedBy, &r.described_by),
            (AtspiRelationType::ControllerFor, &r.controls),
            (AtspiRelationType::FlowsTo, &r.flows_to),
            (AtspiRelationType::ErrorMessage, &r.error_message),
        ];
        Ok(relations
            .into_iter()
            .filter(|(_, ids)| !ids.is_empty())
            .map(|(kind, ids)| {
                (
                    kind as u32,
                    ids.iter().map(|n| self.node_ref(v.key, *n)).collect(),
                )
            })
            .collect())
    }
    fn get_role(&mut self, path: &str) -> MethodResult<u32> {
        Ok(if path == ROOT {
            AtspiRole::Application as u32
        } else {
            view(path)?.node.role.atspi_role()
        })
    }
    fn get_role_name(&mut self, path: &str) -> MethodResult<String> {
        if path == ROOT {
            return Ok("application".into());
        }
        let n = view(path)?.node;
        let name = match n.role {
            Role::Generic | Role::Pane | Role::TabPanel => "panel",
            Role::Window => "frame",
            Role::Group | Role::RadioGroup | Role::Figure => "grouping",
            Role::Toolbar => "tool bar",
            Role::Radio => "radio button",
            Role::MenuItemCheck => "check menu item",
            Role::MenuItemRadio => "radio menu item",
            Role::TabList => "page tab list",
            Role::Tab => "page tab",
            Role::Option => "list item",
            Role::Grid => "table",
            Role::Row => "table row",
            Role::Cell => "table cell",
            Role::ColumnHeader => "table column header",
            Role::RowHeader => "table row header",
            Role::ScrollArea => "scroll pane",
            Role::Progress => "progress bar",
            Role::Meter => "level bar",
            Role::TextField | Role::SearchField => "entry",
            Role::TextArea => "text",
            Role::PasswordField => "password text",
            Role::StaticText => "label",
            Role::AlertDialog => "alert",
            Role::Status => "status bar",
            Role::Tooltip => "tool tip",
            Role::Splitter => "split pane",
            Role::Document => "document frame",
            Role::Note => "comment",
            Role::Navigation | Role::Main => "landmark",
            Role::ColorWell => "color chooser",
            Role::DisclosureButton => "toggle button",
            _ => return Ok(n.role.short().replace('_', " ")),
        };
        Ok(name.into())
    }
    fn get_localized_role_name(&mut self, path: &str) -> MethodResult<String> {
        self.get_role_name(path)
    }
    fn get_state(&mut self, path: &str) -> MethodResult<Vec<u32>> {
        if path == ROOT {
            return Ok(vec![0, 0]);
        }
        let v = view(path)?;
        Ok(state_words(&v.node, v.focused))
    }
    fn get_attributes(&mut self, path: &str) -> MethodResult<Vec<(String, String)>> {
        if path == ROOT {
            return Ok(vec![]);
        }
        let n = view(path)?.node;
        let mut attributes = vec![("class".into(), n.class), ("path".into(), n.path)];
        if n.level != 0 {
            attributes.push(("level".into(), n.level.to_string()));
        }
        if n.live != makepad_a11y::Live::Off {
            attributes.push((
                "live".into(),
                if n.live == makepad_a11y::Live::Assertive {
                    "assertive"
                } else {
                    "polite"
                }
                .into(),
            ));
        }
        Ok(attributes)
    }
    fn get_application(&mut self, _: &str) -> MethodResult<ObjRef> {
        Ok(self.reference(ROOT))
    }
    fn get_interfaces(&mut self, path: &str) -> MethodResult<Vec<String>> {
        self.interfaces(path)
    }
}
fn locale() -> String {
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "C".into())
}
impl ApplicationServer for Objects {
    fn get_locale(&mut self, _: &str, _: u32) -> MethodResult<String> {
        Ok(locale())
    }
    fn get_application_bus_address(&mut self, _: &str) -> MethodResult<String> {
        Ok(self.address.clone())
    }
    fn prop_toolkit_name(&mut self, _: &str) -> MethodResult<String> {
        Ok("Makepad".into())
    }
    fn prop_version(&mut self, _: &str) -> MethodResult<String> {
        Ok(env!("CARGO_PKG_VERSION").into())
    }
    fn prop_toolkit_version(&mut self, _: &str) -> MethodResult<String> {
        Ok(env!("CARGO_PKG_VERSION").into())
    }
    fn prop_atspi_version(&mut self, _: &str) -> MethodResult<String> {
        Ok("2.1".into())
    }
    fn prop_interface_version(&mut self, _: &str) -> MethodResult<u32> {
        Ok(1)
    }
    fn prop_id(&mut self, _: &str) -> MethodResult<i32> {
        Ok(lock(&self.shared).id)
    }
    fn set_prop_id(&mut self, _: &str, value: i32) -> MethodResult<()> {
        lock(&self.shared).id = value;
        Ok(())
    }
}
impl ComponentServer for Objects {
    version!();
    fn get_extents(&mut self, path: &str, coord: u32) -> MethodResult<(i32, i32, i32, i32)> {
        self.rect(path, view(path)?.node.bounds, coord)
    }
    fn get_position(&mut self, path: &str, coord: u32) -> MethodResult<(i32, i32)> {
        let (x, y, _, _) = self.get_extents(path, coord)?;
        Ok((x, y))
    }
    fn get_size(&mut self, path: &str) -> MethodResult<(i32, i32)> {
        let (_, _, w, h) = self.get_extents(path, AtspiCoordType::Window as u32)?;
        Ok((w, h))
    }
    fn contains(&mut self, path: &str, x: i32, y: i32, coord: u32) -> MethodResult<bool> {
        let (dx, dy) = self.offset(path, coord)?;
        Ok(view(path)?
            .node
            .bounds
            .contains(x as f32 - dx, y as f32 - dy))
    }
    fn get_accessible_at_point(
        &mut self,
        path: &str,
        x: i32,
        y: i32,
        coord: u32,
    ) -> MethodResult<ObjRef> {
        let v = view(path)?;
        let (dx, dy) = self.offset(path, coord)?;
        let hit = a11y::with_tree(v.key, |t| {
            t.hit_test(x as f32 - dx, y as f32 - dy)
                .filter(|id| *id == v.node.id || t.ancestors(*id).contains(&v.node.id))
        })
        .flatten();
        Ok(hit.map_or_else(|| self.null(), |id| self.node_ref(v.key, id)))
    }
    fn grab_focus(&mut self, path: &str) -> MethodResult<bool> {
        self.enqueue(path, Actions::FOCUS, ActionArg::None)
    }
    fn scroll_to(&mut self, path: &str, _: u32) -> MethodResult<bool> {
        self.enqueue(path, Actions::SCROLL_INTO_VIEW, ActionArg::None)
    }
    fn get_layer(&mut self, _: &str) -> MethodResult<u32> {
        Ok(3)
    } // ATSPI_LAYER_WIDGET
    fn get_mdiz_order(&mut self, _: &str) -> MethodResult<i16> {
        Ok(0)
    }
    fn get_alpha(&mut self, _: &str) -> MethodResult<f64> {
        Ok(1.)
    }
}
pub(super) fn actions(n: &Node) -> Vec<(Actions, String)> {
    let mut out: Vec<_> = Actions::NAMES
        .iter()
        .filter(|(a, _)| n.actions.contains(*a) && *a != Actions::CUSTOM)
        .map(|(a, name)| (*a, (*name).to_owned()))
        .collect();
    out.extend(
        n.custom_actions
            .iter()
            .map(|name| (Actions::CUSTOM, name.clone())),
    );
    out
}
impl ActionServer for Objects {
    version!();
    fn prop_n_actions(&mut self, path: &str) -> MethodResult<i32> {
        Ok(actions(&view(path)?.node).len() as i32)
    }
    fn get_name(&mut self, path: &str, index: i32) -> MethodResult<String> {
        actions(&view(path)?.node)
            .get(index as usize)
            .map(|(_, n)| n.clone())
            .ok_or_else(invalid)
    }
    fn get_localized_name(&mut self, path: &str, index: i32) -> MethodResult<String> {
        self.get_name(path, index)
    }
    fn get_description(&mut self, path: &str, index: i32) -> MethodResult<String> {
        self.get_name(path, index)
    }
    fn get_key_binding(&mut self, path: &str, index: i32) -> MethodResult<String> {
        self.get_name(path, index)?;
        Ok(view(path)?.node.key_shortcut)
    }
    fn get_actions(&mut self, path: &str) -> MethodResult<Vec<(String, String, String)>> {
        let n = view(path)?.node;
        Ok(actions(&n)
            .into_iter()
            .map(|(_, a)| (a.clone(), a, n.key_shortcut.clone()))
            .collect())
    }
    fn do_action(&mut self, path: &str, index: i32) -> MethodResult<bool> {
        let n = view(path)?.node;
        let (action, name) = actions(&n)
            .get(index as usize)
            .cloned()
            .ok_or_else(invalid)?;
        let arg = if action == Actions::CUSTOM {
            ActionArg::Custom(name)
        } else if action == Actions::SET_VALUE {
            n.range.map_or_else(
                || ActionArg::Text(n.value.clone()),
                |r| ActionArg::Number(r.now),
            )
        } else if action == Actions::SET_TEXT_SELECTION {
            n.text.as_ref().map_or(ActionArg::None, |t| {
                ActionArg::Range(t.selection.0, t.selection.1)
            })
        } else if action == Actions::REPLACE_TEXT {
            return Ok(false);
        } else if action == Actions::SCROLL {
            ActionArg::Scroll(0., n.bounds.h)
        } else {
            ActionArg::None
        };
        self.enqueue(path, action, arg)
    }
}
impl ValueServer for Objects {
    version!();
    fn prop_minimum_value(&mut self, path: &str) -> MethodResult<f64> {
        Ok(self.range(path)?.min)
    }
    fn prop_maximum_value(&mut self, path: &str) -> MethodResult<f64> {
        Ok(self.range(path)?.max)
    }
    fn prop_minimum_increment(&mut self, path: &str) -> MethodResult<f64> {
        Ok(self.range(path)?.step)
    }
    fn prop_current_value(&mut self, path: &str) -> MethodResult<f64> {
        Ok(self.range(path)?.now)
    }
    fn prop_text(&mut self, path: &str) -> MethodResult<String> {
        Ok(view(path)?.node.value)
    }
    fn set_prop_current_value(&mut self, path: &str, value: f64) -> MethodResult<()> {
        let r = self.range(path)?;
        if !value.is_finite() || value < r.min || value > r.max {
            return Err(invalid());
        }
        if self.enqueue(path, Actions::SET_VALUE, ActionArg::Number(value))? {
            Ok(())
        } else {
            Err(unsupported())
        }
    }
}
impl SelectionServer for Objects {
    version!();
    fn prop_n_selected_children(&mut self, path: &str) -> MethodResult<i32> {
        Ok(self.selected(path, true)?.len() as i32)
    }
    fn get_selected_child(&mut self, path: &str, index: i32) -> MethodResult<ObjRef> {
        let key = view(path)?.key;
        self.selected(path, true)?
            .get(index as usize)
            .map(|n| self.node_ref(key, *n))
            .ok_or_else(invalid)
    }
    fn select_child(&mut self, path: &str, index: i32) -> MethodResult<bool> {
        self.child_action(path, index, true)
    }
    fn deselect_child(&mut self, path: &str, index: i32) -> MethodResult<bool> {
        self.child_action(path, index, false)
    }
    fn deselect_selected_child(&mut self, path: &str, index: i32) -> MethodResult<bool> {
        let n = self.get_selected_child(path, index)?;
        self.enqueue(&n.path, Actions::REMOVE_SELECTION, ActionArg::None)
    }
    fn is_child_selected(&mut self, path: &str, index: i32) -> MethodResult<bool> {
        let n = self.get_child_at_index(path, index)?;
        Ok(view(&n.path)?.node.states.contains(States::SELECTED))
    }
    fn select_all(&mut self, path: &str) -> MethodResult<bool> {
        let v = view(path)?;
        if !v.node.states.contains(States::MULTISELECTABLE) {
            return Ok(false);
        }
        let mut result = true;
        for id in self.selected(path, false)? {
            result &= self.enqueue(
                &self.node_ref(v.key, id).path,
                Actions::ADD_SELECTION,
                ActionArg::None,
            )?;
        }
        Ok(result)
    }
    fn clear_selection(&mut self, path: &str) -> MethodResult<bool> {
        let key = view(path)?.key;
        let mut result = true;
        for id in self.selected(path, true)? {
            result &= self.enqueue(
                &self.node_ref(key, id).path,
                Actions::REMOVE_SELECTION,
                ActionArg::None,
            )?;
        }
        Ok(result)
    }
}
impl CacheServer for Objects {
    version!();
    fn get_items(&mut self, _: &str) -> MethodResult<Vec<CacheItem>> {
        let mut items = vec![self.cache_item(ROOT)?];
        for (key, _) in roots() {
            let mut ids = a11y::with_tree(key, |t| t.nodes.keys().copied().collect::<Vec<_>>())
                .unwrap_or_default();
            ids.sort_unstable();
            for id in ids {
                if let Ok(item) = self.cache_item(&path(key, id)) {
                    items.push(item);
                }
            }
        }
        Ok(items)
    }
}
impl TableServer for Objects {
    version!();
    fn prop_n_rows(&mut self, path: &str) -> MethodResult<i32> {
        Ok(self.table(path)?.rows as i32)
    }
    fn prop_n_columns(&mut self, path: &str) -> MethodResult<i32> {
        Ok(self.table(path)?.cols as i32)
    }
    fn prop_caption(&mut self, _: &str) -> MethodResult<ObjRef> {
        Ok(self.null())
    }
    fn prop_summary(&mut self, _: &str) -> MethodResult<ObjRef> {
        Ok(self.null())
    }
    fn get_accessible_at(&mut self, path: &str, row: i32, col: i32) -> MethodResult<ObjRef> {
        let key = view(path)?.key;
        Ok(self
            .cell(path, row, col)?
            .map_or_else(|| self.null(), |n| self.node_ref(key, n.id)))
    }
    fn get_index_at(&mut self, path: &str, row: i32, col: i32) -> MethodResult<i32> {
        let t = self.table(path)?;
        if row < 0 || col < 0 || row as u32 >= t.rows || col as u32 >= t.cols {
            return Ok(-1);
        }
        Ok((row as u64 * t.cols as u64 + col as u64)
            .try_into()
            .unwrap_or(-1))
    }
    fn get_row_at_index(&mut self, path: &str, index: i32) -> MethodResult<i32> {
        let t = self.table(path)?;
        if index < 0 || t.cols == 0 || index as u64 >= t.rows as u64 * t.cols as u64 {
            return Ok(-1);
        }
        Ok(index / t.cols as i32)
    }
    fn get_column_at_index(&mut self, path: &str, index: i32) -> MethodResult<i32> {
        if self.get_row_at_index(path, index)? < 0 {
            return Ok(-1);
        }
        Ok(index % self.table(path)?.cols as i32)
    }
    fn get_row_extent_at(&mut self, path: &str, row: i32, col: i32) -> MethodResult<i32> {
        Ok(self
            .cell(path, row, col)?
            .and_then(|n| n.table)
            .map_or(0, |t| t.row_span.max(1) as i32))
    }
    fn get_column_extent_at(&mut self, path: &str, row: i32, col: i32) -> MethodResult<i32> {
        Ok(self
            .cell(path, row, col)?
            .and_then(|n| n.table)
            .map_or(0, |t| t.col_span.max(1) as i32))
    }
    fn get_row_header(&mut self, path: &str, row: i32) -> MethodResult<ObjRef> {
        let key = view(path)?.key;
        Ok(self
            .cells(path)?
            .iter()
            .find(|n| n.role == Role::RowHeader && n.table.is_some_and(|t| t.row as i32 == row))
            .map_or_else(|| self.null(), |n| self.node_ref(key, n.id)))
    }
    fn get_column_header(&mut self, path: &str, col: i32) -> MethodResult<ObjRef> {
        let key = view(path)?.key;
        Ok(self
            .cells(path)?
            .iter()
            .find(|n| n.role == Role::ColumnHeader && n.table.is_some_and(|t| t.col as i32 == col))
            .map_or_else(|| self.null(), |n| self.node_ref(key, n.id)))
    }
    fn get_row_description(&mut self, path: &str, row: i32) -> MethodResult<String> {
        let h = self.get_row_header(path, row)?;
        Ok(view(&h.path).map_or_else(|_| String::new(), |v| v.node.name))
    }
    fn get_column_description(&mut self, path: &str, col: i32) -> MethodResult<String> {
        let h = self.get_column_header(path, col)?;
        Ok(view(&h.path).map_or_else(|_| String::new(), |v| v.node.name))
    }
    fn is_selected(&mut self, path: &str, row: i32, col: i32) -> MethodResult<bool> {
        Ok(self
            .cell(path, row, col)?
            .is_some_and(|n| n.states.contains(States::SELECTED)))
    }
    fn get_row_column_extents_at_index(
        &mut self,
        path: &str,
        index: i32,
    ) -> MethodResult<(bool, i32, i32, i32, i32, bool)> {
        let row = self.get_row_at_index(path, index)?;
        let col = self.get_column_at_index(path, index)?;
        if row < 0 {
            return Ok((false, -1, -1, 0, 0, false));
        }
        let Some(n) = self.cell(path, row, col)? else {
            return Ok((false, row, col, 0, 0, false));
        };
        let t = n.table.unwrap();
        Ok((
            true,
            t.row as i32,
            t.col as i32,
            t.row_span.max(1) as i32,
            t.col_span.max(1) as i32,
            n.states.contains(States::SELECTED),
        ))
    }
}
impl TableCellServer for Objects {
    version!();
    fn prop_position(&mut self, path: &str) -> MethodResult<(i32, i32)> {
        let t = self.table(path)?;
        Ok((t.row as i32, t.col as i32))
    }
    fn prop_row_span(&mut self, path: &str) -> MethodResult<i32> {
        Ok(self.table(path)?.row_span.max(1) as i32)
    }
    fn prop_column_span(&mut self, path: &str) -> MethodResult<i32> {
        Ok(self.table(path)?.col_span.max(1) as i32)
    }
    fn get_row_column_span(&mut self, path: &str) -> MethodResult<(bool, i32, i32, i32, i32)> {
        let t = self.table(path)?;
        Ok((
            true,
            t.row as i32,
            t.col as i32,
            t.row_span.max(1) as i32,
            t.col_span.max(1) as i32,
        ))
    }
    fn prop_table(&mut self, path: &str) -> MethodResult<ObjRef> {
        let v = view(path)?;
        let table = a11y::with_tree(v.key, |t| {
            t.ancestors(v.node.id).into_iter().rev().find(|n| {
                t.node(*n)
                    .is_some_and(|n| matches!(n.role, Role::Table | Role::Grid))
            })
        })
        .flatten();
        Ok(table.map_or_else(|| self.null(), |n| self.node_ref(v.key, n)))
    }
    fn get_column_header_cells(&mut self, path: &str) -> MethodResult<Vec<ObjRef>> {
        let t = self.table(path)?;
        let parent = self.prop_table(path)?;
        let header = self.get_column_header(&parent.path, t.col as i32)?;
        Ok(if header.bus.is_empty() {
            vec![]
        } else {
            vec![header]
        })
    }
    fn get_row_header_cells(&mut self, path: &str) -> MethodResult<Vec<ObjRef>> {
        let t = self.table(path)?;
        let parent = self.prop_table(path)?;
        let header = self.get_row_header(&parent.path, t.row as i32)?;
        Ok(if header.bus.is_empty() {
            vec![]
        } else {
            vec![header]
        })
    }
}
pub(super) fn state_flags(n: &Node, focused: bool) -> Vec<(AtspiStateType, &'static str, bool)> {
    use AtspiStateType as A;
    let s = n.states;
    let mut out = vec![
        (A::Enabled, "enabled", !s.contains(States::DISABLED)),
        (A::Sensitive, "sensitive", !s.contains(States::DISABLED)),
        (A::Visible, "visible", !s.contains(States::HIDDEN)),
        (
            A::Showing,
            "showing",
            !s.intersects(States::HIDDEN | States::OFFSCREEN),
        ),
        (
            A::Focused,
            "focused",
            focused || s.contains(States::FOCUSED),
        ),
        (A::Checkable, "checkable", n.toggle != Toggle::None),
        (A::Checked, "checked", n.toggle == Toggle::On),
        (A::Indeterminate, "indeterminate", n.toggle == Toggle::Mixed),
        (
            A::Expandable,
            "expandable",
            s.intersects(States::EXPANDED | States::COLLAPSED)
                || n.actions.intersects(Actions::EXPAND | Actions::COLLAPSE),
        ),
        (
            A::SingleLine,
            "single-line",
            n.text.is_some() && !s.contains(States::MULTILINE),
        ),
        (
            A::SelectableText,
            "selectable-text",
            n.actions.contains(Actions::SET_TEXT_SELECTION),
        ),
    ];
    out.extend(
        [
            (States::FOCUSABLE, A::Focusable, "focusable"),
            (States::SELECTED, A::Selected, "selected"),
            (States::SELECTABLE, A::Selectable, "selectable"),
            (States::EXPANDED, A::Expanded, "expanded"),
            (States::COLLAPSED, A::Collapsed, "collapsed"),
            (States::PRESSED, A::Pressed, "pressed"),
            (States::READ_ONLY, A::ReadOnly, "read-only"),
            (States::REQUIRED, A::Required, "required"),
            (States::BUSY, A::Busy, "busy"),
            (States::MODAL, A::Modal, "modal"),
            (
                States::MULTISELECTABLE,
                A::Multiselectable,
                "multiselectable",
            ),
            (States::EDITABLE, A::Editable, "editable"),
            (States::DEFAULT, A::IsDefault, "is-default"),
            (States::VISITED, A::Visited, "visited"),
            (States::INVALID, A::InvalidEntry, "invalid-entry"),
            (States::HAS_POPUP, A::HasPopup, "has-popup"),
            (States::MULTILINE, A::MultiLine, "multi-line"),
            (States::VERTICAL, A::Vertical, "vertical"),
            (States::HORIZONTAL, A::Horizontal, "horizontal"),
            (States::ACTIVE, A::Active, "active"),
        ]
        .into_iter()
        .map(|(flag, state, name)| (state, name, s.contains(flag))),
    );
    out
}
pub(super) fn state_words(n: &Node, focused: bool) -> Vec<u32> {
    let mut words = vec![0, 0];
    for (state, _, enabled) in state_flags(n, focused) {
        if enabled {
            let bit = state as usize;
            words[bit / 32] |= 1 << (bit % 32);
        }
    }
    words
}
