use super::objects::{path, roots, state_flags};
use super::*;
use makepad_a11y::{mirror::text, Live, Node};
use makepad_dbus_wire::Result;

#[derive(Default)]
struct Window {
    root: NodeId,
    focus: NodeId,
    nodes: HashMap<NodeId, Node>,
}
#[derive(Default)]
pub(super) struct History {
    windows: HashMap<u64, Window>,
    roots: Vec<(u64, NodeId)>,
}
fn snapshot(key: u64) -> Window {
    // Enumerate identities once, then copy one record per short mirror-lock hold.
    // No marshalling, text analysis or bus calls run inside those holds.
    let Some((root, focus, ids)) = a11y::with_tree(key, |t| {
        (t.root, t.focus, t.nodes.keys().copied().collect::<Vec<_>>())
    }) else {
        return Window::default();
    };
    let nodes = ids
        .into_iter()
        .filter_map(|id| {
            a11y::with_tree(key, |t| t.node(id).cloned())
                .flatten()
                .map(|n| (id, n))
        })
        .collect();
    Window { root, focus, nodes }
}
fn children_changed(
    bus: &mut Connection,
    object: &str,
    old: &[ObjRef],
    new: &[ObjRef],
) -> Result<()> {
    if old == new {
        return Ok(());
    }
    // Removes run backwards so each index describes the list at that point.
    for (i, child) in old.iter().enumerate().rev() {
        emit_object_children_changed(bus, object, "remove", i as i32, 0, &child.value(), &[])?;
    }
    for (i, child) in new.iter().enumerate() {
        emit_object_children_changed(bus, object, "add", i as i32, 0, &child.value(), &[])?;
    }
    Ok(())
}
fn property(bus: &mut Connection, path: &str, name: &str, value: &Value) -> Result<()> {
    emit_object_property_change(bus, path, name, 0, 0, value, &[])
}
fn text_changed(bus: &mut Connection, path: &str, old: &str, new: &str) -> Result<()> {
    if old == new {
        return Ok(());
    }
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let a = text::char_to_byte(old, prefix as u32) as usize;
    let b = text::char_to_byte(new, prefix as u32) as usize;
    let suffix = old[a..]
        .chars()
        .rev()
        .zip(new[b..].chars().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let removed = old[a..].chars().count() - suffix;
    let inserted = new[b..].chars().count() - suffix;
    let old_end = text::char_to_byte(old, (prefix + removed) as u32) as usize;
    let new_end = text::char_to_byte(new, (prefix + inserted) as u32) as usize;
    if removed != 0 {
        emit_object_text_changed(
            bus,
            path,
            "delete",
            prefix as i32,
            removed as i32,
            &Value::Str(old[a..old_end].into()),
            &[],
        )?;
    }
    if inserted != 0 {
        emit_object_text_changed(
            bus,
            path,
            "insert",
            prefix as i32,
            inserted as i32,
            &Value::Str(new[b..new_end].into()),
            &[],
        )?;
    }
    Ok(())
}
impl History {
    pub fn refresh_all(&mut self, objects: &mut Objects, bus: &mut Connection) -> Result<()> {
        let mut keys: Vec<_> = self.windows.keys().copied().collect();
        keys.extend(roots().into_iter().map(|(k, _)| k));
        keys.sort_unstable();
        keys.dedup();
        for key in keys {
            self.update(key, &[], objects, bus)?;
        }
        Ok(())
    }
    pub fn focus(
        &mut self,
        key: u64,
        focused: bool,
        objects: &mut Objects,
        bus: &mut Connection,
    ) -> Result<()> {
        if let Some(root) = a11y::with_tree(key, |t| t.root).filter(|n| !n.is_none()) {
            let path = path(key, root);
            let value = Value::I32(0);
            if focused {
                emit_window_activate(bus, &path, "", 0, 0, &value, &[])?;
            } else {
                emit_window_deactivate(bus, &path, "", 0, 0, &value, &[])?;
            }
            let _ = objects;
        }
        Ok(())
    }
    pub fn update(
        &mut self,
        key: u64,
        notices: &[Notification],
        objects: &mut Objects,
        bus: &mut Connection,
    ) -> Result<()> {
        let now = snapshot(key);
        let old = self.windows.remove(&key).unwrap_or_default();
        // Populate the cache before announcing newly reachable children.
        for (id, node) in &now.nodes {
            if !old.nodes.contains_key(id) {
                if let Ok(item) = objects.cache_item(&path(key, *id)) {
                    emit_cache_add_accessible(bus, CACHE, &item)?;
                }
            } else if let Some(before) = old.nodes.get(id) {
                self.changed(key, before, node, old.focus, now.focus, objects, bus)?;
            }
        }
        if old.nodes.is_empty()
            && !now.nodes.is_empty()
            && lock(&objects.shared).focus.get(&key) == Some(&true)
        {
            self.focus(key, true, objects, bus)?;
        }
        let new_roots = roots();
        children_changed(
            bus,
            ROOT,
            &self
                .roots
                .iter()
                .map(|(k, n)| objects.node_ref(*k, *n))
                .collect::<Vec<_>>(),
            &new_roots
                .iter()
                .map(|(k, n)| objects.node_ref(*k, *n))
                .collect::<Vec<_>>(),
        )?;
        self.roots = new_roots;
        for id in old.nodes.keys() {
            if !now.nodes.contains_key(id) {
                // Parent diffs precede cache removal, so the removed object is still addressable in client caches.
                emit_cache_remove_accessible(bus, CACHE, &objects.node_ref(key, *id))?;
            }
        }
        if old.focus != now.focus && !now.focus.is_none() && now.nodes.contains_key(&now.focus) {
            emit_focus_focus(bus, &path(key, now.focus), "", 0, 0, &Value::I32(0), &[])?;
            if !old.nodes.contains_key(&now.focus) {
                emit_object_state_changed(
                    bus,
                    &path(key, now.focus),
                    "focused",
                    1,
                    0,
                    &Value::I32(0),
                    &[],
                )?;
            }
        }
        for notification in notices {
            match notification {
                Notification::Announce { text, live } => {
                    let source = if now.root.is_none() {
                        ROOT.into()
                    } else {
                        path(key, now.root)
                    };
                    emit_object_announcement(
                        bus,
                        &source,
                        "",
                        *live as i32,
                        0,
                        &Value::Str(text.clone()),
                        &[],
                    )?;
                }
                Notification::LiveRegionChanged(id) => {
                    if let Some(n) = now.nodes.get(id) {
                        let spoken = n.text.as_ref().map_or_else(
                            || {
                                if n.value.is_empty() {
                                    n.name.clone()
                                } else {
                                    n.value.clone()
                                }
                            },
                            |t| t.content.clone(),
                        );
                        if n.live != Live::Off {
                            emit_object_announcement(
                                bus,
                                &path(key, *id),
                                "",
                                n.live as i32,
                                0,
                                &Value::Str(spoken),
                                &[],
                            )?;
                        }
                    }
                }
                _ => (), // the retained snapshots supply consistent old/new values, including coalesced draws
            }
        }
        if !now.nodes.is_empty() {
            self.windows.insert(key, now);
        } else {
            let mut s = lock(&objects.shared);
            s.origins.remove(&key);
            s.focus.remove(&key);
        }
        Ok(())
    }
    fn changed(
        &self,
        key: u64,
        old: &Node,
        new: &Node,
        old_focus: NodeId,
        new_focus: NodeId,
        objects: &Objects,
        bus: &mut Connection,
    ) -> Result<()> {
        let path = path(key, new.id);
        if old.name != new.name {
            property(bus, &path, "accessible-name", &Value::Str(new.name.clone()))?;
        }
        if old.description != new.description {
            property(
                bus,
                &path,
                "accessible-description",
                &Value::Str(new.description.clone()),
            )?;
        }
        if old.value != new.value || old.range != new.range || old.toggle != new.toggle {
            let value = new
                .range
                .map_or_else(|| Value::Str(new.value.clone()), |r| Value::F64(r.now));
            property(bus, &path, "accessible-value", &value)?;
            if let Some(range) = new.range {
                bus.emit_signal(
                    &path,
                    "org.freedesktop.DBus.Properties",
                    "PropertiesChanged",
                    Body::from_values(&[
                        Value::Str(VALUE_INTERFACE.into()),
                        Value::Array {
                            element: "{sv}".into(),
                            values: vec![Value::DictEntry(Box::new((
                                Value::Str("CurrentValue".into()),
                                Value::Variant(Box::new(Value::F64(range.now))),
                            )))],
                        },
                        Value::Array {
                            element: "s".into(),
                            values: vec![],
                        },
                    ])?,
                )?;
            }
        }
        for ((_, name, before), (_, _, after)) in state_flags(old, old.id == old_focus)
            .into_iter()
            .zip(state_flags(new, new.id == new_focus))
        {
            if before != after {
                emit_object_state_changed(
                    bus,
                    &path,
                    name,
                    i32::from(after),
                    0,
                    &Value::I32(0),
                    &[],
                )?;
            }
        }
        children_changed(
            bus,
            &path,
            &old.children
                .iter()
                .map(|id| objects.node_ref(key, *id))
                .collect::<Vec<_>>(),
            &new.children
                .iter()
                .map(|id| objects.node_ref(key, *id))
                .collect::<Vec<_>>(),
        )?;
        let old_text = old.text.as_ref().map_or("", |t| t.content.as_str());
        let new_text = new.text.as_ref().map_or("", |t| t.content.as_str());
        text_changed(bus, &path, old_text, new_text)?;
        if let Some(t) = &new.text {
            let before = old
                .text
                .as_ref()
                .map(|t| (text::byte_to_char(&t.content, t.selection.1), t.selection));
            let caret = text::byte_to_char(&t.content, t.selection.1);
            if before.is_none_or(|b| b.0 != caret) {
                emit_object_text_caret_moved(bus, &path, "", caret as i32, 0, &Value::I32(0), &[])?;
            }
            if before.is_none_or(|b| b.1 != t.selection) {
                emit_object_text_selection_changed(bus, &path, "", 0, 0, &Value::I32(0), &[])?;
            }
        }
        Ok(())
    }
}
