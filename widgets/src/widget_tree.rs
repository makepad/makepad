use {
    crate::makepad_draw::{cx_2d::Cx2d, cx_3d::Cx3d, *},
    crate::widget::{WidgetRef, WidgetUid},
    crate::widget_async::update_global_ui_handle,
    std::cell::RefCell,
    std::collections::{HashMap, HashSet},
};

// WidgetTree contains WidgetRef (Rc-based) and RefCell,
// but we only ever access the tree from the main thread.
// The OnceLock for the empty static tree requires Sync.
unsafe impl Send for WidgetTree {}
unsafe impl Sync for WidgetTree {}

const NONE: u32 = u32::MAX;

// ============================================================================
// WidgetTree: persistent graph + dense query index
// ============================================================================

pub struct WidgetTree {
    inner: RefCell<WidgetTreeInner>,
}

#[derive(Default)]
struct WidgetTreeInner {
    // Hot path (dense query index)
    names: Vec<LiveId>,
    subtree_end: Vec<u32>,
    no_index: Vec<bool>,
    nodes: Vec<WidgetTreeNode>,
    uid_map: HashMap<WidgetUid, u32>,

    root_uid: WidgetUid,

    // Persistent graph, lazily synced from WidgetNode::children()
    graph: HashMap<WidgetUid, GraphNode>,
    dirty: HashSet<WidgetUid>,
    dense_dirty: bool,
}

struct WidgetTreeNode {
    #[allow(unused)]
    uid: WidgetUid,
    widget: WidgetRef,
    parent: u32,
}

struct GraphNode {
    name: LiveId,
    widget: WidgetRef,
    no_index: bool,
    parent: Option<WidgetUid>,
    children: Vec<WidgetUid>,
}

impl Default for WidgetTree {
    fn default() -> Self {
        Self {
            inner: RefCell::new(WidgetTreeInner::default()),
        }
    }
}

impl WidgetTree {
    pub fn observe_node(
        &self,
        uid: WidgetUid,
        name: LiveId,
        widget: WidgetRef,
        parent: Option<WidgetUid>,
    ) {
        if uid == WidgetUid(0) || widget.is_empty() {
            return;
        }

        let no_index = widget.skip_widget_tree_search();
        let mut inner = self.inner.borrow_mut();

        let mut old_parent = None;
        let mut node_is_new = false;
        let mut widget_changed = false;
        let mut name_changed = false;
        let mut no_index_changed = false;
        let mut parent_changed = false;

        match inner.graph.get_mut(&uid) {
            Some(node) => {
                old_parent = node.parent;
                if node.name != name {
                    node.name = name;
                    name_changed = true;
                }
                if node.widget != widget {
                    node.widget = widget;
                    widget_changed = true;
                }
                if node.no_index != no_index {
                    node.no_index = no_index;
                    no_index_changed = true;
                }
                if node.parent != parent {
                    node.parent = parent;
                    parent_changed = true;
                }
            }
            None => {
                inner.graph.insert(
                    uid,
                    GraphNode {
                        name,
                        widget,
                        no_index,
                        parent,
                        children: Vec::new(),
                    },
                );
                node_is_new = true;
            }
        }

        if node_is_new || name_changed || no_index_changed || parent_changed {
            inner.dense_dirty = true;
        }

        if parent.is_none() && inner.root_uid == WidgetUid(0) {
            inner.root_uid = uid;
        }

        if old_parent != parent {
            if let Some(prev_parent_uid) = old_parent {
                if let Some(prev_parent) = inner.graph.get_mut(&prev_parent_uid) {
                    if let Some(pos) = prev_parent.children.iter().position(|child| *child == uid) {
                        prev_parent.children.remove(pos);
                        inner.dense_dirty = true;
                    }
                }
            }
        }

        if let Some(parent_uid) = parent {
            let mut replaced_same_name = Vec::new();
            let existing_children = inner
                .graph
                .get(&parent_uid)
                .map(|parent| parent.children.clone())
                .unwrap_or_default();
            for existing_uid in existing_children {
                if existing_uid == uid {
                    continue;
                }
                if let Some(existing_node) = inner.graph.get(&existing_uid) {
                    if existing_node.name == name {
                        replaced_same_name.push(existing_uid);
                    }
                }
            }
            if let Some(parent_node) = inner.graph.get_mut(&parent_uid) {
                if !parent_node.children.iter().any(|child| *child == uid) {
                    parent_node.children.push(uid);
                    inner.dense_dirty = true;
                }
                inner.dirty.insert(parent_uid);
            }

            if !replaced_same_name.is_empty() {
                if let Some(parent_node) = inner.graph.get_mut(&parent_uid) {
                    parent_node
                        .children
                        .retain(|child_uid| !replaced_same_name.contains(child_uid));
                }
                for old_uid in replaced_same_name {
                    let should_remove = inner
                        .graph
                        .get(&old_uid)
                        .map_or(false, |node| node.parent == Some(parent_uid));
                    if should_remove {
                        Self::remove_subtree(&mut inner, old_uid);
                    }
                }
                inner.dense_dirty = true;
            }
        }

        if node_is_new || widget_changed {
            inner.dirty.insert(uid);
        }
    }

    pub fn insert_child(&self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef) {
        if parent_uid == WidgetUid(0) || widget.is_empty() {
            return;
        }
        let Some(child_uid) = widget.try_widget_uid() else {
            return;
        };
        if child_uid == WidgetUid(0) {
            return;
        }
        let child_no_index = widget.skip_widget_tree_search();

        let mut inner = self.inner.borrow_mut();

        if !inner.graph.contains_key(&parent_uid) {
            inner.graph.insert(
                parent_uid,
                GraphNode {
                    name: LiveId(0),
                    widget: WidgetRef::empty(),
                    no_index: false,
                    parent: None,
                    children: Vec::new(),
                },
            );
            if inner.root_uid == WidgetUid(0) {
                inner.root_uid = parent_uid;
            }
            inner.dense_dirty = true;
        }

        let mut old_parent = None;
        let mut child_is_new = false;
        let mut name_changed = false;
        let mut no_index_changed = false;
        let mut parent_changed = false;

        match inner.graph.get_mut(&child_uid) {
            Some(node) => {
                old_parent = node.parent;
                if node.name != name {
                    node.name = name;
                    name_changed = true;
                }
                if node.widget != widget {
                    node.widget = widget;
                }
                if node.no_index != child_no_index {
                    node.no_index = child_no_index;
                    no_index_changed = true;
                }
                if node.parent != Some(parent_uid) {
                    node.parent = Some(parent_uid);
                    parent_changed = true;
                }
            }
            None => {
                inner.graph.insert(
                    child_uid,
                    GraphNode {
                        name,
                        widget,
                        no_index: child_no_index,
                        parent: Some(parent_uid),
                        children: Vec::new(),
                    },
                );
                child_is_new = true;
            }
        }

        if child_is_new || name_changed || no_index_changed || parent_changed {
            inner.dense_dirty = true;
        }

        if old_parent != Some(parent_uid) {
            if let Some(prev_parent_uid) = old_parent {
                if let Some(prev_parent) = inner.graph.get_mut(&prev_parent_uid) {
                    if let Some(pos) =
                        prev_parent.children.iter().position(|entry| *entry == child_uid)
                    {
                        prev_parent.children.remove(pos);
                        inner.dense_dirty = true;
                    }
                }
            }
        }

        let mut replaced_same_name = Vec::new();
        let existing_children = inner
            .graph
            .get(&parent_uid)
            .map(|parent| parent.children.clone())
            .unwrap_or_default();
        for existing_uid in existing_children {
            if existing_uid == child_uid {
                continue;
            }
            if let Some(existing_node) = inner.graph.get(&existing_uid) {
                if existing_node.name == name {
                    replaced_same_name.push(existing_uid);
                }
            }
        }
        if let Some(parent_node) = inner.graph.get_mut(&parent_uid) {
            if !parent_node.children.iter().any(|entry| *entry == child_uid) {
                parent_node.children.push(child_uid);
                inner.dense_dirty = true;
            }
        }

        if !replaced_same_name.is_empty() {
            if let Some(parent_node) = inner.graph.get_mut(&parent_uid) {
                parent_node
                    .children
                    .retain(|child_uid| !replaced_same_name.contains(child_uid));
            }
            for old_uid in replaced_same_name {
                let should_remove = inner
                    .graph
                    .get(&old_uid)
                    .map_or(false, |node| node.parent == Some(parent_uid));
                if should_remove {
                    Self::remove_subtree(&mut inner, old_uid);
                }
            }
            inner.dense_dirty = true;
        }

        inner.dirty.insert(child_uid);
    }

    pub fn mark_dirty(&self, uid: WidgetUid) {
        if uid == WidgetUid(0) {
            return;
        }
        let mut inner = self.inner.borrow_mut();
        inner.dirty.insert(uid);
    }

    pub fn seed_from_widget(&self, widget: WidgetRef) {
        if widget.is_empty() {
            return;
        }

        let Some(uid) = widget.try_widget_uid() else {
            return;
        };
        if uid == WidgetUid(0) {
            return;
        }
        let no_index = widget.skip_widget_tree_search();

        let mut inner = self.inner.borrow_mut();
        if let Some(node) = inner.graph.get_mut(&uid) {
            let mut widget_changed = false;
            let mut no_index_changed = false;
            if node.widget != widget {
                node.widget = widget;
                widget_changed = true;
            }
            if node.no_index != no_index {
                node.no_index = no_index;
                no_index_changed = true;
            }
            if widget_changed {
                inner.dirty.insert(uid);
                inner.dense_dirty = true;
            }
            if no_index_changed {
                inner.dense_dirty = true;
            }
            if inner.root_uid == WidgetUid(0) {
                inner.root_uid = uid;
            }
            return;
        }

        inner.graph.insert(
            uid,
            GraphNode {
                name: LiveId(0),
                widget,
                no_index,
                parent: None,
                children: Vec::new(),
            },
        );
        if inner.root_uid == WidgetUid(0) {
            inner.root_uid = uid;
        }
        inner.dirty.insert(uid);
        inner.dense_dirty = true;
    }

    pub fn set_root_widget(&self, widget: WidgetRef) {
        if widget.is_empty() {
            return;
        }

        let Some(uid) = widget.try_widget_uid() else {
            return;
        };
        if uid == WidgetUid(0) {
            return;
        }
        let no_index = widget.skip_widget_tree_search();

        let mut inner = self.inner.borrow_mut();
        let mut old_parent = None;
        let mut node_is_new = false;
        let mut widget_changed = false;
        let mut name_changed = false;
        let mut no_index_changed = false;
        let mut parent_changed = false;

        match inner.graph.get_mut(&uid) {
            Some(node) => {
                old_parent = node.parent;
                if node.name != LiveId(0) {
                    node.name = LiveId(0);
                    name_changed = true;
                }
                if node.widget != widget {
                    node.widget = widget;
                    widget_changed = true;
                }
                if node.no_index != no_index {
                    node.no_index = no_index;
                    no_index_changed = true;
                }
                if node.parent.is_some() {
                    node.parent = None;
                    parent_changed = true;
                }
            }
            None => {
                inner.graph.insert(
                    uid,
                    GraphNode {
                        name: LiveId(0),
                        widget,
                        no_index,
                        parent: None,
                        children: Vec::new(),
                    },
                );
                node_is_new = true;
            }
        }

        if old_parent.is_some() {
            if let Some(prev_parent_uid) = old_parent {
                if let Some(prev_parent) = inner.graph.get_mut(&prev_parent_uid) {
                    if let Some(pos) = prev_parent.children.iter().position(|child| *child == uid) {
                        prev_parent.children.remove(pos);
                        inner.dense_dirty = true;
                    }
                }
            }
        }

        if inner.root_uid != uid {
            inner.root_uid = uid;
            inner.dense_dirty = true;
        }

        if node_is_new || widget_changed || name_changed || no_index_changed || parent_changed {
            inner.dense_dirty = true;
        }

        inner.dirty.insert(uid);
    }

    pub fn refresh_from_borrowed<F>(&self, uid: WidgetUid, mut visit: F)
    where
        F: FnMut(&mut dyn FnMut(LiveId, WidgetRef)),
    {
        if uid == WidgetUid(0) {
            return;
        }

        let mut discovered_children: Vec<(LiveId, WidgetRef)> = Vec::new();
        {
            let mut collect = |name: LiveId, child: WidgetRef| {
                if !child.is_empty() {
                    discovered_children.push((name, child));
                }
            };
            visit(&mut collect);
        }

        let mut inner = self.inner.borrow_mut();
        if !inner.graph.contains_key(&uid) {
            inner.graph.insert(
                uid,
                GraphNode {
                    name: LiveId(0),
                    widget: WidgetRef::empty(),
                    no_index: false,
                    parent: None,
                    children: Vec::new(),
                },
            );
            if inner.root_uid == WidgetUid(0) {
                inner.root_uid = uid;
            }
            inner.dense_dirty = true;
        }

        let mut pending = Vec::new();
        if Self::refresh_node_children_from_discovered(
            &mut inner,
            uid,
            &mut pending,
            discovered_children,
        ) {
            inner.dirty.remove(&uid);
            for child_uid in pending {
                inner.dirty.insert(child_uid);
            }
        } else {
            inner.dirty.insert(uid);
        }
    }

    fn sync_dirty(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.dirty.is_empty() && !inner.dense_dirty {
            return;
        }

        let mut pending: Vec<WidgetUid> = inner.dirty.drain().collect();
        let mut retry = Vec::new();
        while let Some(uid) = pending.pop() {
            if !Self::refresh_node_children(&mut inner, uid, &mut pending) {
                retry.push(uid);
            }
        }

        for uid in retry {
            inner.dirty.insert(uid);
        }

        if inner.dense_dirty {
            Self::rebuild_dense(&mut inner);
        }
    }

    fn refresh_node_children(
        inner: &mut WidgetTreeInner,
        uid: WidgetUid,
        pending: &mut Vec<WidgetUid>,
    ) -> bool {
        let parent_widget = match inner.graph.get(&uid) {
            Some(node) => node.widget.clone(),
            None => return true,
        };
        if parent_widget.is_empty() {
            // Placeholder node (seeded from borrowed context without a WidgetRef):
            // keep existing child edges until a concrete WidgetRef is seeded.
            return true;
        }

        let mut discovered_children: Vec<(LiveId, WidgetRef)> = Vec::new();
        if !parent_widget.try_children(&mut |name, child| {
            if !child.is_empty() {
                discovered_children.push((name, child));
            }
        }) {
            inner.dirty.insert(uid);
            return false;
        }

        if !Self::refresh_node_children_from_discovered(inner, uid, pending, discovered_children) {
            inner.dirty.insert(uid);
            return false;
        }
        true
    }

    fn refresh_node_children_from_discovered(
        inner: &mut WidgetTreeInner,
        uid: WidgetUid,
        pending: &mut Vec<WidgetUid>,
        discovered_children: Vec<(LiveId, WidgetRef)>,
    ) -> bool {
        let mut resolved_children: Vec<(LiveId, WidgetRef, WidgetUid)> =
            Vec::with_capacity(discovered_children.len());
        for (child_name, child_widget) in discovered_children {
            let Some(child_uid) = child_widget.try_widget_uid() else {
                return false;
            };
            if child_uid == WidgetUid(0) {
                return false;
            }
            resolved_children.push((child_name, child_widget, child_uid));
        }

        let old_children = match inner.graph.get_mut(&uid) {
            Some(node) => std::mem::take(&mut node.children),
            None => return true,
        };

        let mut new_children: Vec<WidgetUid> = Vec::with_capacity(resolved_children.len());

        for (child_name, child_widget, child_uid) in resolved_children {
            if !new_children.iter().any(|entry| *entry == child_uid) {
                new_children.push(child_uid);
            }
            let child_no_index = child_widget.skip_widget_tree_search();

            let mut old_parent = None;
            let mut child_is_new = false;
            let mut child_widget_changed = false;
            let mut child_no_index_changed = false;

            match inner.graph.get_mut(&child_uid) {
                Some(child_node) => {
                    old_parent = child_node.parent;
                    if child_node.name != child_name {
                        child_node.name = child_name;
                        inner.dense_dirty = true;
                    }
                    if child_node.widget != child_widget {
                        child_node.widget = child_widget;
                        child_widget_changed = true;
                    }
                    if child_node.no_index != child_no_index {
                        child_node.no_index = child_no_index;
                        child_no_index_changed = true;
                    }
                    if child_node.parent != Some(uid) {
                        child_node.parent = Some(uid);
                        inner.dense_dirty = true;
                    }
                }
                None => {
                    inner.graph.insert(
                        child_uid,
                        GraphNode {
                            name: child_name,
                            widget: child_widget,
                            no_index: child_no_index,
                            parent: Some(uid),
                            children: Vec::new(),
                        },
                    );
                    child_is_new = true;
                    inner.dense_dirty = true;
                }
            }

            if old_parent != Some(uid) {
                if let Some(prev_parent_uid) = old_parent {
                    if let Some(prev_parent) = inner.graph.get_mut(&prev_parent_uid) {
                        if let Some(pos) =
                            prev_parent.children.iter().position(|entry| *entry == child_uid)
                        {
                            prev_parent.children.remove(pos);
                            inner.dense_dirty = true;
                        }
                    }
                }
            }

            if child_no_index_changed {
                inner.dense_dirty = true;
            }

            if child_is_new || child_widget_changed {
                inner.dirty.insert(child_uid);
                pending.push(child_uid);
            }
        }

        let parent_children_changed = match inner.graph.get_mut(&uid) {
            Some(node) => {
                let changed = node.children != new_children;
                node.children = new_children;
                changed
            }
            None => false,
        };

        if parent_children_changed {
            inner.dense_dirty = true;
        }

        for removed_uid in old_children {
            let still_child = inner
                .graph
                .get(&uid)
                .map_or(false, |node| node.children.iter().any(|child| *child == removed_uid));
            if still_child {
                continue;
            }

            let should_remove = inner
                .graph
                .get(&removed_uid)
                .map_or(false, |node| node.parent == Some(uid));
            if should_remove {
                Self::remove_subtree(inner, removed_uid);
            }
        }

        true
    }

    fn remove_subtree(inner: &mut WidgetTreeInner, uid: WidgetUid) {
        let Some(node) = inner.graph.remove(&uid) else {
            return;
        };

        inner.dirty.remove(&uid);
        inner.dense_dirty = true;

        for child_uid in node.children {
            let has_same_parent = inner
                .graph
                .get(&child_uid)
                .map_or(false, |child| child.parent == Some(uid));
            if has_same_parent {
                Self::remove_subtree(inner, child_uid);
            }
        }
    }

    fn rebuild_dense(inner: &mut WidgetTreeInner) {
        inner.names.clear();
        inner.subtree_end.clear();
        inner.no_index.clear();
        inner.nodes.clear();
        inner.uid_map.clear();

        if inner.graph.is_empty() {
            inner.root_uid = WidgetUid(0);
            inner.dense_dirty = false;
            return;
        }

        if inner.root_uid == WidgetUid(0) || !inner.graph.contains_key(&inner.root_uid) {
            inner.root_uid = inner
                .graph
                .iter()
                .find_map(|(uid, node)| node.parent.is_none().then_some(*uid))
                .or_else(|| inner.graph.keys().next().copied())
                .unwrap_or(WidgetUid(0));
        }

        let mut visited = HashSet::new();

        if inner.root_uid != WidgetUid(0) {
            Self::build_dense_from(inner, inner.root_uid, NONE, &mut visited);
        }

        let roots: Vec<WidgetUid> = inner
            .graph
            .iter()
            .filter_map(|(uid, node)| {
                if node.parent.is_none() && !visited.contains(uid) {
                    Some(*uid)
                } else {
                    None
                }
            })
            .collect();

        for uid in roots {
            Self::build_dense_from(inner, uid, NONE, &mut visited);
        }
        inner.dense_dirty = false;
    }

    fn build_dense_from(
        inner: &mut WidgetTreeInner,
        uid: WidgetUid,
        parent_idx: u32,
        visited: &mut HashSet<WidgetUid>,
    ) {
        if !visited.insert(uid) {
            return;
        }

        let Some(node) = inner.graph.get(&uid) else {
            return;
        };

        let name = node.name;
        let widget = node.widget.clone();
        let no_index = node.no_index;
        let children = node.children.clone();

        let idx = inner.names.len() as u32;
        inner.names.push(name);
        inner.subtree_end.push(idx + 1);
        inner.no_index.push(no_index);
        inner.nodes.push(WidgetTreeNode {
            uid,
            widget,
            parent: parent_idx,
        });
        inner.uid_map.insert(uid, idx);

        for child_uid in children {
            if inner.graph.contains_key(&child_uid) {
                Self::build_dense_from(inner, child_uid, idx, visited);
            }
        }

        inner.subtree_end[idx as usize] = inner.names.len() as u32;
    }

    pub fn find_within_from_borrowed<F>(
        &self,
        root_uid: WidgetUid,
        path: &[LiveId],
        visit: F,
    ) -> WidgetRef
    where
        F: FnMut(&mut dyn FnMut(LiveId, WidgetRef)),
    {
        self.refresh_from_borrowed(root_uid, visit);
        self.find_within(root_uid, path)
    }

    pub fn find_all_within_from_borrowed<F>(
        &self,
        root_uid: WidgetUid,
        path: &[LiveId],
        visit: F,
    ) -> Vec<WidgetRef>
    where
        F: FnMut(&mut dyn FnMut(LiveId, WidgetRef)),
    {
        self.refresh_from_borrowed(root_uid, visit);
        self.find_all_within(root_uid, path)
    }

    pub fn find_flood_from_borrowed<F>(
        &self,
        origin_uid: WidgetUid,
        path: &[LiveId],
        visit: F,
    ) -> WidgetRef
    where
        F: FnMut(&mut dyn FnMut(LiveId, WidgetRef)),
    {
        self.refresh_from_borrowed(origin_uid, visit);
        self.find_flood(origin_uid, path)
    }

    pub fn find_all_flood_from_borrowed<F>(
        &self,
        origin_uid: WidgetUid,
        path: &[LiveId],
        visit: F,
    ) -> Vec<WidgetRef>
    where
        F: FnMut(&mut dyn FnMut(LiveId, WidgetRef)),
    {
        self.refresh_from_borrowed(origin_uid, visit);
        self.find_all_flood(origin_uid, path)
    }

    /// Find a widget within the subtree of `root_uid` by matching a path of LiveIds.
    /// If `root_uid` is not currently indexed, this falls back to searching the
    /// entire indexed graph.
    pub fn find_within(&self, root_uid: WidgetUid, path: &[LiveId]) -> WidgetRef {
        self.sync_dirty();
        let inner = self.inner.borrow();
        let (start, end) = match inner.uid_map.get(&root_uid) {
            Some(&idx) => (idx as usize, inner.subtree_end[idx as usize] as usize),
            None => (0, inner.names.len()),
        };

        let target = match path.last() {
            Some(&id) => id,
            None => return WidgetRef::empty(),
        };

        let mut result = WidgetRef::empty();
        let mut i = start;
        while i < end {
            if inner.names[i] == target
                && (path.len() == 1 || Self::verify_path(&inner, &path[..path.len() - 1], i))
            {
                result = inner.nodes[i].widget.clone();
            }
            if inner.no_index[i] {
                i = inner.subtree_end[i] as usize;
            } else {
                i += 1;
            }
        }
        result
    }

    /// Find all widgets matching path within the subtree of `root_uid`.
    /// If `root_uid` is not currently indexed, this falls back to searching the
    /// entire indexed graph.
    pub fn find_all_within(&self, root_uid: WidgetUid, path: &[LiveId]) -> Vec<WidgetRef> {
        self.sync_dirty();
        let inner = self.inner.borrow();

        let mut results = Vec::new();
        let (start, end) = match inner.uid_map.get(&root_uid) {
            Some(&idx) => (idx as usize, inner.subtree_end[idx as usize] as usize),
            None => (0, inner.names.len()),
        };

        let target = match path.last() {
            Some(&id) => id,
            None => return results,
        };

        let mut i = start;
        while i < end {
            if inner.names[i] == target
                && (path.len() == 1 || Self::verify_path(&inner, &path[..path.len() - 1], i))
            {
                results.push(inner.nodes[i].widget.clone());
            }
            if inner.no_index[i] {
                i = inner.subtree_end[i] as usize;
            } else {
                i += 1;
            }
        }
        results
    }

    fn verify_path(inner: &WidgetTreeInner, remaining: &[LiveId], node_idx: usize) -> bool {
        let mut current = inner.nodes[node_idx].parent;
        for &segment in remaining.iter().rev() {
            loop {
                if current == NONE {
                    return false;
                }
                if inner.names[current as usize] == segment {
                    current = inner.nodes[current as usize].parent;
                    break;
                }
                current = inner.nodes[current as usize].parent;
            }
        }
        true
    }

    /// Look up a widget by its UID.
    pub fn widget(&self, uid: WidgetUid) -> WidgetRef {
        self.sync_dirty();
        let inner = self.inner.borrow();
        match inner.uid_map.get(&uid) {
            Some(&idx) => inner.nodes[idx as usize].widget.clone(),
            None => WidgetRef::empty(),
        }
    }

    /// Build the path of LiveIds from root to the node with the given UID.
    pub fn path_to(&self, uid: WidgetUid) -> Vec<LiveId> {
        self.sync_dirty();
        let inner = self.inner.borrow();

        let mut path = Vec::new();
        if let Some(&idx) = inner.uid_map.get(&uid) {
            let mut current = idx;
            loop {
                path.push(inner.names[current as usize]);
                let parent = inner.nodes[current as usize].parent;
                if parent == NONE {
                    break;
                }
                current = parent;
            }
            path.reverse();
        }
        path
    }

    /// Flood-fill search: find a widget by path starting from `origin_uid`,
    /// expanding outward through the tree.
    pub fn find_flood(&self, origin_uid: WidgetUid, path: &[LiveId]) -> WidgetRef {
        self.sync_dirty();
        let inner = self.inner.borrow();

        let target = match path.last() {
            Some(&id) => id,
            None => return WidgetRef::empty(),
        };

        let origin_idx = match inner.uid_map.get(&origin_uid) {
            Some(&idx) => idx as usize,
            None => {
                return Self::find_within_range(&inner, 0, inner.names.len(), target, path);
            }
        };

        let origin_end = inner.subtree_end[origin_idx] as usize;
        let result = Self::find_within_range(&inner, origin_idx, origin_end, target, path);
        if !result.is_empty() {
            return result;
        }

        let mut exclude_start = origin_idx;
        let mut exclude_end = origin_end;
        let mut current = inner.nodes[origin_idx].parent;

        while current != NONE {
            let cur = current as usize;
            let cur_end = inner.subtree_end[cur] as usize;

            let result = Self::find_within_range_excluding(
                &inner,
                cur,
                cur_end,
                exclude_start,
                exclude_end,
                target,
                path,
            );
            if !result.is_empty() {
                return result;
            }

            exclude_start = cur;
            exclude_end = cur_end;
            current = inner.nodes[cur].parent;
        }

        Self::find_within_range_excluding(
            &inner,
            0,
            inner.names.len(),
            exclude_start,
            exclude_end,
            target,
            path,
        )
    }

    /// Flood-fill search returning all matches, expanding outward from `origin_uid`.
    pub fn find_all_flood(&self, origin_uid: WidgetUid, path: &[LiveId]) -> Vec<WidgetRef> {
        self.sync_dirty();
        let inner = self.inner.borrow();

        let mut results = Vec::new();
        let target = match path.last() {
            Some(&id) => id,
            None => return results,
        };

        let origin_idx = match inner.uid_map.get(&origin_uid) {
            Some(&idx) => idx as usize,
            None => {
                Self::collect_within_range(&inner, &mut results, 0, inner.names.len(), target, path);
                return results;
            }
        };

        let origin_end = inner.subtree_end[origin_idx] as usize;
        Self::collect_within_range(
            &inner,
            &mut results,
            origin_idx,
            origin_end,
            target,
            path,
        );

        let mut exclude_start = origin_idx;
        let mut exclude_end = origin_end;
        let mut current = inner.nodes[origin_idx].parent;

        while current != NONE {
            let cur = current as usize;
            let cur_end = inner.subtree_end[cur] as usize;

            Self::collect_within_range_excluding(
                &inner,
                &mut results,
                cur,
                cur_end,
                exclude_start,
                exclude_end,
                target,
                path,
            );

            exclude_start = cur;
            exclude_end = cur_end;
            current = inner.nodes[cur].parent;
        }

        Self::collect_within_range_excluding(
            &inner,
            &mut results,
            0,
            inner.names.len(),
            exclude_start,
            exclude_end,
            target,
            path,
        );

        results
    }

    fn find_within_range(
        inner: &WidgetTreeInner,
        start: usize,
        end: usize,
        target: LiveId,
        path: &[LiveId],
    ) -> WidgetRef {
        let mut i = start;
        while i < end {
            if inner.names[i] == target
                && (path.len() == 1 || Self::verify_path(inner, &path[..path.len() - 1], i))
            {
                return inner.nodes[i].widget.clone();
            }
            if inner.no_index[i] {
                i = inner.subtree_end[i] as usize;
            } else {
                i += 1;
            }
        }
        WidgetRef::empty()
    }

    fn find_within_range_excluding(
        inner: &WidgetTreeInner,
        start: usize,
        end: usize,
        excl_start: usize,
        excl_end: usize,
        target: LiveId,
        path: &[LiveId],
    ) -> WidgetRef {
        let mut i = start;
        while i < end {
            if i == excl_start {
                i = excl_end;
                continue;
            }
            if inner.names[i] == target
                && (path.len() == 1 || Self::verify_path(inner, &path[..path.len() - 1], i))
            {
                return inner.nodes[i].widget.clone();
            }
            if inner.no_index[i] {
                i = inner.subtree_end[i] as usize;
            } else {
                i += 1;
            }
        }
        WidgetRef::empty()
    }

    fn collect_within_range(
        inner: &WidgetTreeInner,
        results: &mut Vec<WidgetRef>,
        start: usize,
        end: usize,
        target: LiveId,
        path: &[LiveId],
    ) {
        let mut i = start;
        while i < end {
            if inner.names[i] == target
                && (path.len() == 1 || Self::verify_path(inner, &path[..path.len() - 1], i))
            {
                results.push(inner.nodes[i].widget.clone());
            }
            if inner.no_index[i] {
                i = inner.subtree_end[i] as usize;
            } else {
                i += 1;
            }
        }
    }

    fn collect_within_range_excluding(
        inner: &WidgetTreeInner,
        results: &mut Vec<WidgetRef>,
        start: usize,
        end: usize,
        excl_start: usize,
        excl_end: usize,
        target: LiveId,
        path: &[LiveId],
    ) {
        let mut i = start;
        while i < end {
            if i == excl_start {
                i = excl_end;
                continue;
            }
            if inner.names[i] == target
                && (path.len() == 1 || Self::verify_path(inner, &path[..path.len() - 1], i))
            {
                results.push(inner.nodes[i].widget.clone());
            }
            if inner.no_index[i] {
                i = inner.subtree_end[i] as usize;
            } else {
                i += 1;
            }
        }
    }

    /// Check if the tree is empty (no indexed nodes yet).
    pub fn is_empty(&self) -> bool {
        self.sync_dirty();
        self.inner.borrow().names.is_empty()
    }

    pub fn root_uid(&self) -> WidgetUid {
        self.inner.borrow().root_uid
    }
}

// ============================================================================
// WidgetTreeState: persistent tree + node cursor stack
// ============================================================================

#[derive(Default)]
pub struct WidgetTreeState {
    pub tree: WidgetTree,
    pub cursor_stack: Vec<WidgetUid>,
}

impl WidgetTreeState {
    fn get_or_init(cx: &mut Cx) -> &mut WidgetTreeState {
        if cx.widget_tree_ptr.is_null() {
            let boxed = Box::new(WidgetTreeState::default());
            cx.widget_tree_ptr = Box::into_raw(boxed) as *mut ();
        }
        unsafe { &mut *(cx.widget_tree_ptr as *mut WidgetTreeState) }
    }
}

// ============================================================================
// CxWidgetExt: extension trait on Cx for widget tree operations
// ============================================================================

pub trait CxWidgetExt {
    fn widget_tree(&self) -> &WidgetTree;
    fn with_node<F, R>(&mut self, uid: WidgetUid, name: LiveId, widget: WidgetRef, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R;
    fn with_widget_tree<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R;
    fn widget_tree_mark_dirty(&mut self, uid: WidgetUid);
    fn widget_tree_insert_child(&mut self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef);
}

fn widget_tree_ptr(cx: &Cx) -> *mut () {
    cx.widget_tree_ptr
}

fn get_or_init_state(cx: &mut Cx) -> &mut WidgetTreeState {
    WidgetTreeState::get_or_init(cx)
}

fn get_state_mut(ptr: *mut ()) -> &'static mut WidgetTreeState {
    unsafe { &mut *(ptr as *mut WidgetTreeState) }
}

impl CxWidgetExt for Cx {
    fn widget_tree(&self) -> &WidgetTree {
        if self.widget_tree_ptr.is_null() {
            static EMPTY: std::sync::OnceLock<WidgetTree> = std::sync::OnceLock::new();
            return EMPTY.get_or_init(WidgetTree::default);
        }
        let state = unsafe { &*(self.widget_tree_ptr as *const WidgetTreeState) };
        &state.tree
    }

    fn with_node<F, R>(&mut self, uid: WidgetUid, name: LiveId, widget: WidgetRef, f: F) -> R
    where
        F: FnOnce(&mut Cx) -> R,
    {
        let state = get_or_init_state(self);
        let parent_uid = state.cursor_stack.last().copied();
        state.tree.observe_node(uid, name, widget, parent_uid);
        state.cursor_stack.push(uid);

        let r = f(self);

        let state = get_state_mut(self.widget_tree_ptr);
        state.cursor_stack.pop();
        r
    }

    fn with_widget_tree<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Cx) -> R,
    {
        {
            let state = get_or_init_state(self);
            state.cursor_stack.clear();
        }

        let r = f(self);

        let root_uid = {
            let state = get_state_mut(self.widget_tree_ptr);
            state.cursor_stack.clear();
            state.tree.root_uid()
        };
        update_global_ui_handle(self, root_uid);
        r
    }

    fn widget_tree_mark_dirty(&mut self, uid: WidgetUid) {
        let state = get_or_init_state(self);
        state.tree.mark_dirty(uid);
    }

    fn widget_tree_insert_child(
        &mut self,
        parent_uid: WidgetUid,
        name: LiveId,
        widget: WidgetRef,
    ) {
        let state = get_or_init_state(self);
        state.tree.insert_child(parent_uid, name, widget);
    }
}

impl<'a, 'b> CxWidgetExt for Cx2d<'a, 'b> {
    fn widget_tree(&self) -> &WidgetTree {
        let ptr = widget_tree_ptr(self);
        if ptr.is_null() {
            static EMPTY: std::sync::OnceLock<WidgetTree> = std::sync::OnceLock::new();
            return EMPTY.get_or_init(WidgetTree::default);
        }
        let state = unsafe { &*(ptr as *const WidgetTreeState) };
        &state.tree
    }

    fn with_node<F, R>(&mut self, uid: WidgetUid, name: LiveId, widget: WidgetRef, f: F) -> R
    where
        F: FnOnce(&mut Cx2d<'a, 'b>) -> R,
    {
        let ptr = {
            let cx: &mut Cx = self;
            let state = get_or_init_state(cx);
            let parent_uid = state.cursor_stack.last().copied();
            state.tree.observe_node(uid, name, widget, parent_uid);
            state.cursor_stack.push(uid);
            cx.widget_tree_ptr
        };

        let r = f(self);

        let state = get_state_mut(ptr);
        state.cursor_stack.pop();
        r
    }

    fn with_widget_tree<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Cx2d<'a, 'b>) -> R,
    {
        {
            let cx: &mut Cx = self;
            let state = get_or_init_state(cx);
            state.cursor_stack.clear();
        }
        let r = f(self);
        {
            let cx: &mut Cx = self;
            let state = get_state_mut(cx.widget_tree_ptr);
            state.cursor_stack.clear();
        }
        r
    }

    fn widget_tree_mark_dirty(&mut self, uid: WidgetUid) {
        let cx: &mut Cx = self;
        let state = get_or_init_state(cx);
        state.tree.mark_dirty(uid);
    }

    fn widget_tree_insert_child(
        &mut self,
        parent_uid: WidgetUid,
        name: LiveId,
        widget: WidgetRef,
    ) {
        let cx: &mut Cx = self;
        let state = get_or_init_state(cx);
        state.tree.insert_child(parent_uid, name, widget);
    }
}

impl<'a, 'b> CxWidgetExt for Cx3d<'a, 'b> {
    fn widget_tree(&self) -> &WidgetTree {
        let ptr = widget_tree_ptr(self);
        if ptr.is_null() {
            static EMPTY: std::sync::OnceLock<WidgetTree> = std::sync::OnceLock::new();
            return EMPTY.get_or_init(WidgetTree::default);
        }
        let state = unsafe { &*(ptr as *const WidgetTreeState) };
        &state.tree
    }

    fn with_node<F, R>(&mut self, uid: WidgetUid, name: LiveId, widget: WidgetRef, f: F) -> R
    where
        F: FnOnce(&mut Cx3d<'a, 'b>) -> R,
    {
        let ptr = {
            let cx: &mut Cx = self;
            let state = get_or_init_state(cx);
            let parent_uid = state.cursor_stack.last().copied();
            state.tree.observe_node(uid, name, widget, parent_uid);
            state.cursor_stack.push(uid);
            cx.widget_tree_ptr
        };

        let r = f(self);

        let state = get_state_mut(ptr);
        state.cursor_stack.pop();
        r
    }

    fn with_widget_tree<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Cx3d<'a, 'b>) -> R,
    {
        {
            let cx: &mut Cx = self;
            let state = get_or_init_state(cx);
            state.cursor_stack.clear();
        }
        let r = f(self);
        {
            let cx: &mut Cx = self;
            let state = get_state_mut(cx.widget_tree_ptr);
            state.cursor_stack.clear();
        }
        r
    }

    fn widget_tree_mark_dirty(&mut self, uid: WidgetUid) {
        let cx: &mut Cx = self;
        let state = get_or_init_state(cx);
        state.tree.mark_dirty(uid);
    }

    fn widget_tree_insert_child(
        &mut self,
        parent_uid: WidgetUid,
        name: LiveId,
        widget: WidgetRef,
    ) {
        let cx: &mut Cx = self;
        let state = get_or_init_state(cx);
        state.tree.insert_child(parent_uid, name, widget);
    }
}
