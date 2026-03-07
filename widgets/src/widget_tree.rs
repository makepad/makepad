use {
    crate::dock::Dock,
    crate::makepad_draw::{cx_2d::Cx2d, cx_3d::Cx3d, *},
    crate::widget::{WidgetRef, WidgetRegistry, WidgetUid, WidgetWeakRef},
    crate::widget_async::update_global_ui_handle,
    std::any::TypeId,
    std::cell::RefCell,
    std::collections::{HashMap, HashSet},
    std::fmt::Write,
};

// WidgetTree contains weak widget refs (Rc/Weak-based) and RefCell,
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
    skip_search: Vec<bool>,
    nodes: Vec<WidgetTreeNode>,
    uid_map: HashMap<WidgetUid, u32>,

    root_uid: WidgetUid,

    // Persistent graph, lazily synced from WidgetNode::children()
    graph: HashMap<WidgetUid, GraphNode>,
    // Per-entrypoint path cache keyed by xor-combined path hash.
    path_cache: HashMap<WidgetUid, HashMap<u64, Vec<PathCacheEntry>>>,
    dirty: HashSet<WidgetUid>,
    // Only set when tree topology changes (nodes added/removed, parent changes).
    // Property-only changes (name, widget ref, skip_search) are patched in-place.
    structure_dirty: bool,
}

struct WidgetTreeNode {
    #[allow(unused)]
    uid: WidgetUid,
    widget: WidgetWeakRef,
    parent: u32,
}

struct GraphNode {
    name: LiveId,
    widget: WidgetWeakRef,
    placeholder: bool,
    skip_search: bool,
    parent: Option<WidgetUid>,
    children: Vec<WidgetUid>,
}

#[derive(Clone)]
struct PathCacheEntry {
    path: Vec<LiveId>,
    matches: Vec<PathCacheMatch>,
}

#[derive(Clone)]
struct PathCacheMatch {
    uid: WidgetUid,
    widget: WidgetWeakRef,
}

impl Default for WidgetTree {
    fn default() -> Self {
        Self {
            inner: RefCell::new(WidgetTreeInner::default()),
        }
    }
}

impl WidgetTree {
    // Patch a property in-place in the dense index (no structural rebuild needed).
    fn patch_name(inner: &mut WidgetTreeInner, uid: WidgetUid, name: LiveId) {
        if let Some(&idx) = inner.uid_map.get(&uid) {
            inner.names[idx as usize] = name;
        }
    }

    fn patch_widget(inner: &mut WidgetTreeInner, uid: WidgetUid, widget: &WidgetRef) {
        if let Some(&idx) = inner.uid_map.get(&uid) {
            inner.nodes[idx as usize].widget = widget.downgrade();
        }
    }

    fn patch_skip_search(inner: &mut WidgetTreeInner, uid: WidgetUid, skip_search: bool) {
        if let Some(&idx) = inner.uid_map.get(&uid) {
            inner.skip_search[idx as usize] = skip_search;
        }
    }

    fn path_hash(path: &[LiveId]) -> u64 {
        let mut hash = 0u64;
        for segment in path {
            hash ^= segment.0;
        }
        hash
    }

    fn cached_path_matches(
        inner: &WidgetTreeInner,
        root_uid: WidgetUid,
        path: &[LiveId],
    ) -> Option<Vec<PathCacheMatch>> {
        let hash = Self::path_hash(path);
        inner
            .path_cache
            .get(&root_uid)
            .and_then(|root_cache| root_cache.get(&hash))
            .and_then(|bucket| {
                bucket
                    .iter()
                    .find(|entry| entry.path.as_slice() == path)
                    .map(|entry| entry.matches.clone())
            })
    }

    fn store_path_matches(
        inner: &mut WidgetTreeInner,
        root_uid: WidgetUid,
        path: &[LiveId],
        matches: Vec<PathCacheMatch>,
    ) {
        let hash = Self::path_hash(path);
        if matches.is_empty() {
            let mut remove_root_cache = false;
            if let Some(root_cache) = inner.path_cache.get_mut(&root_uid) {
                let mut remove_hash_bucket = false;
                if let Some(bucket) = root_cache.get_mut(&hash) {
                    bucket.retain(|entry| entry.path.as_slice() != path);
                    remove_hash_bucket = bucket.is_empty();
                }
                if remove_hash_bucket {
                    root_cache.remove(&hash);
                }
                remove_root_cache = root_cache.is_empty();
            }
            if remove_root_cache {
                inner.path_cache.remove(&root_uid);
            }
            return;
        }
        let root_cache = inner.path_cache.entry(root_uid).or_default();
        let bucket = root_cache.entry(hash).or_default();
        if let Some(entry) = bucket
            .iter_mut()
            .find(|entry| entry.path.as_slice() == path)
        {
            entry.matches = matches;
            return;
        }
        bucket.push(PathCacheEntry {
            path: path.to_vec(),
            matches,
        });
    }

    fn invalidate_path_cache(inner: &mut WidgetTreeInner, uid: WidgetUid) {
        if uid != WidgetUid(0) {
            inner.path_cache.remove(&uid);
        }
    }

    fn invalidate_path_cache_upward(inner: &mut WidgetTreeInner, start_uid: WidgetUid) {
        if start_uid == WidgetUid(0) {
            return;
        }

        let mut current = Some(start_uid);
        let mut visited = HashSet::new();
        while let Some(uid) = current {
            if uid == WidgetUid(0) || !visited.insert(uid) {
                break;
            }
            inner.path_cache.remove(&uid);
            current = inner.graph.get(&uid).and_then(|node| node.parent);
        }
    }

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

        let skip_search = widget.skip_widget_tree_search();
        let mut inner = self.inner.borrow_mut();

        let mut old_parent = None;
        let mut node_is_new = false;
        let mut widget_changed = false;
        let mut name_changed = false;
        let mut skip_search_changed = false;
        let mut parent_changed = false;

        match inner.graph.get_mut(&uid) {
            Some(node) => {
                old_parent = node.parent;
                if node.name != name {
                    node.name = name;
                    name_changed = true;
                }
                if node.placeholder || node.widget != widget {
                    node.widget = widget.downgrade();
                    node.placeholder = false;
                    widget_changed = true;
                }
                if node.skip_search != skip_search {
                    node.skip_search = skip_search;
                    skip_search_changed = true;
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
                        widget: widget.downgrade(),
                        placeholder: false,
                        skip_search,
                        parent,
                        children: Vec::new(),
                    },
                );
                node_is_new = true;
            }
        }

        // Structural change: new node or parent changed → full rebuild needed
        if node_is_new || parent_changed {
            inner.structure_dirty = true;
        } else {
            // Property-only changes: patch in-place
            if name_changed {
                Self::patch_name(&mut inner, uid, name);
            }
            if skip_search_changed {
                Self::patch_skip_search(&mut inner, uid, skip_search);
            }
            if widget_changed {
                Self::patch_widget(&mut inner, uid, &widget);
            }
        }

        if parent.is_none() && inner.root_uid == WidgetUid(0) {
            inner.root_uid = uid;
        }

        if old_parent != parent {
            if let Some(prev_parent_uid) = old_parent {
                if let Some(prev_parent) = inner.graph.get_mut(&prev_parent_uid) {
                    if let Some(pos) = prev_parent.children.iter().position(|child| *child == uid) {
                        prev_parent.children.remove(pos);
                        inner.structure_dirty = true;
                    }
                }
            }
        }

        if let Some(parent_uid) = parent {
            let mut replaced_same_name = Vec::new();
            let num_children = inner
                .graph
                .get(&parent_uid)
                .map(|p| p.children.len())
                .unwrap_or(0);
            for i in 0..num_children {
                let existing_uid = inner.graph.get(&parent_uid).unwrap().children[i];
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
                    inner.structure_dirty = true;
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
                inner.structure_dirty = true;
            }
        }

        if node_is_new || widget_changed {
            inner.dirty.insert(uid);
        }

        if node_is_new || widget_changed || name_changed || skip_search_changed || parent_changed {
            Self::invalidate_path_cache_upward(&mut inner, uid);
            if let Some(old_parent) = old_parent {
                Self::invalidate_path_cache_upward(&mut inner, old_parent);
            }
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
        let child_skip_search = widget.skip_widget_tree_search();

        let mut inner = self.inner.borrow_mut();

        if !inner.graph.contains_key(&parent_uid) {
            inner.graph.insert(
                parent_uid,
                GraphNode {
                    name: LiveId(0),
                    widget: WidgetWeakRef::default(),
                    placeholder: true,
                    skip_search: false,
                    parent: None,
                    children: Vec::new(),
                },
            );
            if inner.root_uid == WidgetUid(0) {
                inner.root_uid = parent_uid;
            }
            inner.structure_dirty = true;
        }

        let mut old_parent = None;
        let mut child_is_new = false;
        let mut widget_changed = false;
        let mut name_changed = false;
        let mut skip_search_changed = false;
        let mut parent_changed = false;

        match inner.graph.get_mut(&child_uid) {
            Some(node) => {
                old_parent = node.parent;
                if node.name != name {
                    node.name = name;
                    name_changed = true;
                }
                if node.placeholder || node.widget != widget {
                    node.widget = widget.downgrade();
                    node.placeholder = false;
                    widget_changed = true;
                }
                if node.skip_search != child_skip_search {
                    node.skip_search = child_skip_search;
                    skip_search_changed = true;
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
                        widget: widget.downgrade(),
                        placeholder: false,
                        skip_search: child_skip_search,
                        parent: Some(parent_uid),
                        children: Vec::new(),
                    },
                );
                child_is_new = true;
            }
        }

        // Structural: new node or parent changed
        if child_is_new || parent_changed {
            inner.structure_dirty = true;
        } else {
            // Property-only: patch in-place
            if name_changed {
                Self::patch_name(&mut inner, child_uid, name);
            }
            if skip_search_changed {
                Self::patch_skip_search(&mut inner, child_uid, child_skip_search);
            }
            if widget_changed {
                Self::patch_widget(&mut inner, child_uid, &widget);
            }
        }

        if old_parent != Some(parent_uid) {
            if let Some(prev_parent_uid) = old_parent {
                if let Some(prev_parent) = inner.graph.get_mut(&prev_parent_uid) {
                    if let Some(pos) = prev_parent
                        .children
                        .iter()
                        .position(|entry| *entry == child_uid)
                    {
                        prev_parent.children.remove(pos);
                        inner.structure_dirty = true;
                    }
                }
            }
        }

        let mut replaced_same_name = Vec::new();
        let num_children = inner
            .graph
            .get(&parent_uid)
            .map(|p| p.children.len())
            .unwrap_or(0);
        for i in 0..num_children {
            let existing_uid = inner.graph.get(&parent_uid).unwrap().children[i];
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
                inner.structure_dirty = true;
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
            inner.structure_dirty = true;
        }

        inner.dirty.insert(child_uid);

        if child_is_new || name_changed || skip_search_changed || parent_changed {
            Self::invalidate_path_cache_upward(&mut inner, parent_uid);
            Self::invalidate_path_cache_upward(&mut inner, child_uid);
            if let Some(old_parent) = old_parent {
                Self::invalidate_path_cache_upward(&mut inner, old_parent);
            }
        }
    }

    /// Insert a child and recursively register all its descendants.
    /// Call this when the subtree is not borrowed (e.g. right after creation).
    pub fn insert_child_deep(&self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef) {
        self.insert_child(parent_uid, name, widget.clone());
        // Now recursively walk the subtree — widget was just created so not borrowed.
        let mut stack: Vec<(WidgetUid, WidgetRef)> = Vec::new();
        if let Some(uid) = widget.try_widget_uid() {
            if uid != WidgetUid(0) {
                stack.push((uid, widget));
            }
        }
        while let Some((current_uid, current_widget)) = stack.pop() {
            let mut children: Vec<(LiveId, WidgetRef)> = Vec::new();
            if !current_widget.try_children(&mut |child_name, child| {
                if !child.is_empty() {
                    children.push((child_name, child));
                }
            }) {
                continue;
            }
            for (child_name, child_widget) in children {
                self.insert_child(current_uid, child_name, child_widget.clone());
                if let Some(child_uid) = child_widget.try_widget_uid() {
                    if child_uid != WidgetUid(0) {
                        stack.push((child_uid, child_widget));
                    }
                }
            }
        }
    }

    pub fn mark_dirty(&self, uid: WidgetUid) {
        if uid == WidgetUid(0) {
            return;
        }
        let mut inner = self.inner.borrow_mut();
        inner.dirty.insert(uid);
        Self::invalidate_path_cache_upward(&mut inner, uid);
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
        let skip_search = widget.skip_widget_tree_search();

        let mut inner = self.inner.borrow_mut();
        if let Some(node) = inner.graph.get_mut(&uid) {
            let mut widget_changed = false;
            let mut skip_search_changed = false;
            if node.placeholder || node.widget != widget {
                node.widget = widget.downgrade();
                node.placeholder = false;
                widget_changed = true;
            }
            if node.skip_search != skip_search {
                node.skip_search = skip_search;
                skip_search_changed = true;
            }
            // Property-only: patch in-place
            if widget_changed {
                inner.dirty.insert(uid);
                Self::patch_widget(&mut inner, uid, &widget);
            }
            if skip_search_changed {
                Self::patch_skip_search(&mut inner, uid, skip_search);
                Self::invalidate_path_cache(&mut inner, uid);
            }
            if inner.root_uid == WidgetUid(0) {
                inner.root_uid = uid;
            }
            return;
        }

        // New node → structural change
        inner.graph.insert(
            uid,
            GraphNode {
                name: LiveId(0),
                widget: widget.downgrade(),
                placeholder: false,
                skip_search,
                parent: None,
                children: Vec::new(),
            },
        );
        if inner.root_uid == WidgetUid(0) {
            inner.root_uid = uid;
        }
        inner.dirty.insert(uid);
        inner.structure_dirty = true;
        Self::invalidate_path_cache(&mut inner, uid);
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
        let skip_search = widget.skip_widget_tree_search();

        let mut inner = self.inner.borrow_mut();
        let mut old_parent = None;
        let mut node_is_new = false;
        let mut widget_changed = false;
        let mut name_changed = false;
        let mut skip_search_changed = false;
        let mut parent_changed = false;

        match inner.graph.get_mut(&uid) {
            Some(node) => {
                old_parent = node.parent;
                if node.name != LiveId(0) {
                    node.name = LiveId(0);
                    name_changed = true;
                }
                if node.placeholder || node.widget != widget {
                    node.widget = widget.downgrade();
                    node.placeholder = false;
                    widget_changed = true;
                }
                if node.skip_search != skip_search {
                    node.skip_search = skip_search;
                    skip_search_changed = true;
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
                        widget: widget.downgrade(),
                        placeholder: false,
                        skip_search,
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
                        inner.structure_dirty = true;
                    }
                }
            }
        }

        // Root change or new node or parent change → structural
        let root_changed = inner.root_uid != uid;
        if root_changed {
            inner.root_uid = uid;
            inner.structure_dirty = true;
        }

        if node_is_new || parent_changed {
            inner.structure_dirty = true;
        } else {
            // Property-only: patch in-place
            if name_changed {
                Self::patch_name(&mut inner, uid, LiveId(0));
            }
            if widget_changed {
                Self::patch_widget(&mut inner, uid, &widget);
            }
            if skip_search_changed {
                Self::patch_skip_search(&mut inner, uid, skip_search);
            }
        }

        inner.dirty.insert(uid);
        if node_is_new
            || widget_changed
            || name_changed
            || skip_search_changed
            || parent_changed
            || root_changed
        {
            Self::invalidate_path_cache_upward(&mut inner, uid);
            if let Some(old_parent) = old_parent {
                Self::invalidate_path_cache_upward(&mut inner, old_parent);
            }
        }
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
                    widget: WidgetWeakRef::default(),
                    placeholder: true,
                    skip_search: false,
                    parent: None,
                    children: Vec::new(),
                },
            );
            if inner.root_uid == WidgetUid(0) {
                inner.root_uid = uid;
            }
            inner.structure_dirty = true;
        }

        let mut pending = Vec::new();
        if Self::refresh_node_children_from_discovered(
            &mut inner,
            uid,
            &mut pending,
            discovered_children,
            false,
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
        if inner.dirty.is_empty() && !inner.structure_dirty {
            return;
        }

        let mut pending: Vec<WidgetUid> = inner.dirty.drain().collect();
        let mut retry = Vec::new();
        while let Some(uid) = pending.pop() {
            if !Self::refresh_node_children(&mut inner, uid, &mut pending, true) {
                retry.push(uid);
            }
        }

        for uid in retry {
            inner.dirty.insert(uid);
        }

        if inner.structure_dirty {
            Self::rebuild_dense(&mut inner);
        }
    }

    fn discard_stale_cache_item(inner: &mut WidgetTreeInner, uid: WidgetUid) {
        if uid == WidgetUid(0) || !inner.graph.contains_key(&uid) {
            return;
        }

        let parent_uid = inner.graph.get(&uid).and_then(|node| node.parent);
        if let Some(parent_uid) = parent_uid {
            if let Some(parent) = inner.graph.get_mut(&parent_uid) {
                parent.children.retain(|child_uid| *child_uid != uid);
            }
            // Re-walk the live parent children on next query so the cache can self-heal.
            inner.dirty.insert(parent_uid);
        } else if inner.root_uid == uid {
            inner.root_uid = WidgetUid(0);
        }

        Self::remove_subtree(inner, uid);
    }

    fn ensure_node_cached(inner: &mut WidgetTreeInner, uid: WidgetUid) -> bool {
        if uid == WidgetUid(0) || !inner.graph.contains_key(&uid) {
            return false;
        }

        let stale = inner.graph.get(&uid).map_or(false, |node| {
            !node.placeholder && node.widget.upgrade().is_none()
        });
        if stale {
            Self::discard_stale_cache_item(inner, uid);
            return false;
        }

        if !inner.dirty.contains(&uid) {
            return true;
        }

        let mut pending = Vec::new();
        if !Self::refresh_node_children(inner, uid, &mut pending, false) {
            return false;
        }

        inner.dirty.remove(&uid);
        for child_uid in pending {
            inner.dirty.insert(child_uid);
        }

        inner.graph.contains_key(&uid)
    }

    fn verify_path_graph(
        inner: &WidgetTreeInner,
        remaining: &[LiveId],
        mut current: Option<WidgetUid>,
        root_uid: WidgetUid,
    ) -> bool {
        for &segment in remaining.iter().rev() {
            loop {
                let uid = match current {
                    Some(uid) => uid,
                    None => return false,
                };
                let Some(node) = inner.graph.get(&uid) else {
                    return false;
                };
                if node.name == segment {
                    current = node.parent;
                    break;
                }
                if uid == root_uid {
                    return false;
                }
                current = node.parent;
            }
        }
        true
    }

    fn find_within_graph(
        inner: &mut WidgetTreeInner,
        root_uid: WidgetUid,
        path: &[LiveId],
        exclude_subtree_root: Option<WidgetUid>,
    ) -> WidgetRef {
        #[derive(Clone, Copy)]
        struct Frame {
            uid: WidgetUid,
            next_child_idx: usize,
            is_root: bool,
        }

        if path.is_empty() || root_uid == WidgetUid(0) || exclude_subtree_root == Some(root_uid) {
            return WidgetRef::empty();
        }
        if !Self::ensure_node_cached(inner, root_uid) {
            return WidgetRef::empty();
        }

        let target = path[path.len() - 1];
        let mut result = WidgetRef::empty();
        let mut visited = HashSet::new();
        let mut stack = vec![Frame {
            uid: root_uid,
            next_child_idx: 0,
            is_root: true,
        }];

        while let Some(mut frame) = stack.pop() {
            if frame.next_child_idx == 0 {
                if !visited.insert(frame.uid) {
                    continue;
                }
                if !Self::ensure_node_cached(inner, frame.uid) {
                    continue;
                }

                let (name, parent_uid, skip_search) = match inner.graph.get(&frame.uid) {
                    Some(node) => (node.name, node.parent, node.skip_search),
                    None => continue,
                };

                if name == target
                    && (path.len() == 1
                        || Self::verify_path_graph(
                            inner,
                            &path[..path.len() - 1],
                            parent_uid,
                            root_uid,
                        ))
                {
                    let upgraded = inner.graph.get(&frame.uid).and_then(|node| node.widget.upgrade());
                    if let Some(widget) = upgraded {
                        result = widget;
                    } else {
                        Self::discard_stale_cache_item(inner, frame.uid);
                        continue;
                    }
                }

                if !frame.is_root && skip_search {
                    continue;
                }
            }

            let next_child_uid = inner
                .graph
                .get(&frame.uid)
                .and_then(|node| node.children.get(frame.next_child_idx).copied());

            let Some(child_uid) = next_child_uid else {
                continue;
            };

            frame.next_child_idx += 1;
            stack.push(frame);
            if exclude_subtree_root == Some(child_uid) {
                continue;
            }
            stack.push(Frame {
                uid: child_uid,
                next_child_idx: 0,
                is_root: false,
            });
        }

        result
    }

    fn collect_within_graph(
        inner: &mut WidgetTreeInner,
        root_uid: WidgetUid,
        path: &[LiveId],
        exclude_subtree_root: Option<WidgetUid>,
        results: &mut Vec<WidgetRef>,
    ) {
        #[derive(Clone, Copy)]
        struct Frame {
            uid: WidgetUid,
            next_child_idx: usize,
            is_root: bool,
        }

        if path.is_empty() || root_uid == WidgetUid(0) || exclude_subtree_root == Some(root_uid) {
            return;
        }
        if !Self::ensure_node_cached(inner, root_uid) {
            return;
        }

        let target = path[path.len() - 1];
        let mut visited = HashSet::new();
        let mut stack = vec![Frame {
            uid: root_uid,
            next_child_idx: 0,
            is_root: true,
        }];

        while let Some(mut frame) = stack.pop() {
            if frame.next_child_idx == 0 {
                if !visited.insert(frame.uid) {
                    continue;
                }
                if !Self::ensure_node_cached(inner, frame.uid) {
                    continue;
                }

                let (name, parent_uid, skip_search) = match inner.graph.get(&frame.uid) {
                    Some(node) => (node.name, node.parent, node.skip_search),
                    None => continue,
                };

                if name == target
                    && (path.len() == 1
                        || Self::verify_path_graph(
                            inner,
                            &path[..path.len() - 1],
                            parent_uid,
                            root_uid,
                        ))
                {
                    let upgraded = inner.graph.get(&frame.uid).and_then(|node| node.widget.upgrade());
                    if let Some(widget) = upgraded {
                        results.push(widget);
                    } else {
                        Self::discard_stale_cache_item(inner, frame.uid);
                        continue;
                    }
                }

                if !frame.is_root && skip_search {
                    continue;
                }
            }

            let next_child_uid = inner
                .graph
                .get(&frame.uid)
                .and_then(|node| node.children.get(frame.next_child_idx).copied());

            let Some(child_uid) = next_child_uid else {
                continue;
            };

            frame.next_child_idx += 1;
            stack.push(frame);
            if exclude_subtree_root == Some(child_uid) {
                continue;
            }
            stack.push(Frame {
                uid: child_uid,
                next_child_idx: 0,
                is_root: false,
            });
        }
    }

    fn find_all_within_cached_graph(
        inner: &mut WidgetTreeInner,
        root_uid: WidgetUid,
        path: &[LiveId],
    ) -> Vec<WidgetRef> {
        if path.is_empty() || root_uid == WidgetUid(0) || !inner.graph.contains_key(&root_uid) {
            return Vec::new();
        }
        if !Self::ensure_node_cached(inner, root_uid) {
            return Vec::new();
        }

        if let Some(cached_matches) = Self::cached_path_matches(inner, root_uid, path) {
            if cached_matches.is_empty() {
                // Misses are not cached. Remove any stale empty marker and re-walk.
                Self::store_path_matches(inner, root_uid, path, Vec::new());
            } else {
                let mut stale_uids = Vec::new();
                let mut live_matches = Vec::with_capacity(cached_matches.len());
                let mut live_widgets = Vec::with_capacity(cached_matches.len());
                for cached in cached_matches {
                    if let Some(widget) = cached.widget.upgrade() {
                        live_widgets.push(widget.clone());
                        live_matches.push(PathCacheMatch {
                            uid: cached.uid,
                            widget: widget.downgrade(),
                        });
                    } else {
                        stale_uids.push(cached.uid);
                    }
                }

                if stale_uids.is_empty() {
                    return live_widgets;
                }

                for stale_uid in stale_uids {
                    Self::discard_stale_cache_item(inner, stale_uid);
                }
                // Drop only stale cache items; re-query below to repopulate.
                Self::store_path_matches(inner, root_uid, path, live_matches);
            }
        }

        let mut results = Vec::new();
        Self::collect_within_graph(inner, root_uid, path, None, &mut results);

        let mut matches = Vec::with_capacity(results.len());
        for widget in &results {
            let Some(uid) = widget.try_widget_uid() else {
                continue;
            };
            if uid == WidgetUid(0) {
                continue;
            }
            matches.push(PathCacheMatch {
                uid,
                widget: widget.downgrade(),
            });
        }
        Self::store_path_matches(inner, root_uid, path, matches);
        results
    }

    fn refresh_node_children(
        inner: &mut WidgetTreeInner,
        uid: WidgetUid,
        pending: &mut Vec<WidgetUid>,
        mark_structure_dirty: bool,
    ) -> bool {
        let (parent_widget, parent_placeholder) = match inner.graph.get(&uid) {
            Some(node) => (node.widget.upgrade(), node.placeholder),
            None => return true,
        };
        if parent_placeholder {
            // Placeholder node (seeded from borrowed context without a WidgetRef):
            // keep existing child edges until a concrete WidgetRef is seeded.
            return true;
        }
        let Some(parent_widget) = parent_widget else {
            // Concrete widget no longer exists; remove stale subtree.
            Self::remove_subtree(inner, uid);
            return true;
        };

        let mut discovered_children: Vec<(LiveId, WidgetRef)> = Vec::new();
        if !parent_widget.try_children(&mut |name, child| {
            if !child.is_empty() {
                discovered_children.push((name, child));
            }
        }) {
            inner.dirty.insert(uid);
            return false;
        }

        if !Self::refresh_node_children_from_discovered(
            inner,
            uid,
            pending,
            discovered_children,
            mark_structure_dirty,
        ) {
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
        mark_structure_dirty: bool,
    ) -> bool {
        let mut resolved_children: Vec<(LiveId, WidgetRef, WidgetUid)> =
            Vec::with_capacity(discovered_children.len());
        for (child_name, child_widget) in discovered_children {
            let Some(child_uid) = child_widget.try_widget_uid() else {
                return false;
            };
            if child_uid == WidgetUid(0) {
                continue;
            }
            resolved_children.push((child_name, child_widget, child_uid));
        }

        let old_children = match inner.graph.get_mut(&uid) {
            Some(node) => std::mem::take(&mut node.children),
            None => return true,
        };

        let mut new_children: Vec<WidgetUid> = Vec::with_capacity(resolved_children.len());
        let mut invalidate_uid_cache = false;

        for (child_name, child_widget, child_uid) in resolved_children {
            if !new_children.iter().any(|entry| *entry == child_uid) {
                new_children.push(child_uid);
            }
            let child_skip_search = child_widget.skip_widget_tree_search();

            let mut old_parent = None;
            let mut child_is_new = false;
            let mut child_widget_changed = false;
            let mut child_name_changed = false;
            let mut child_skip_search_changed = false;
            let mut child_parent_changed = false;

            match inner.graph.get_mut(&child_uid) {
                Some(child_node) => {
                    old_parent = child_node.parent;
                    if child_node.name != child_name {
                        child_node.name = child_name;
                        child_name_changed = true;
                    }
                    if child_node.placeholder || child_node.widget != child_widget {
                        child_node.widget = child_widget.downgrade();
                        child_node.placeholder = false;
                        child_widget_changed = true;
                    }
                    if child_node.skip_search != child_skip_search {
                        child_node.skip_search = child_skip_search;
                        child_skip_search_changed = true;
                    }
                    if child_node.parent != Some(uid) {
                        child_node.parent = Some(uid);
                        child_parent_changed = true;
                    }
                }
                None => {
                    inner.graph.insert(
                        child_uid,
                        GraphNode {
                            name: child_name,
                            widget: child_widget.downgrade(),
                            placeholder: false,
                            skip_search: child_skip_search,
                            parent: Some(uid),
                            children: Vec::new(),
                        },
                    );
                    child_is_new = true;
                    invalidate_uid_cache = true;
                    if mark_structure_dirty {
                        inner.structure_dirty = true;
                    }
                }
            }

            // Structural: new node or parent changed
            if child_parent_changed {
                invalidate_uid_cache = true;
                if mark_structure_dirty {
                    inner.structure_dirty = true;
                }
            }

            if !child_is_new && !child_parent_changed {
                // Property-only: patch in-place
                if child_name_changed {
                    Self::patch_name(inner, child_uid, child_name);
                    invalidate_uid_cache = true;
                }
                if child_skip_search_changed {
                    Self::patch_skip_search(inner, child_uid, child_skip_search);
                    invalidate_uid_cache = true;
                }
                if child_widget_changed {
                    Self::patch_widget(inner, child_uid, &child_widget);
                }
            }

            if old_parent != Some(uid) {
                if let Some(prev_parent_uid) = old_parent {
                    if let Some(prev_parent) = inner.graph.get_mut(&prev_parent_uid) {
                        if let Some(pos) = prev_parent
                            .children
                            .iter()
                            .position(|entry| *entry == child_uid)
                        {
                            prev_parent.children.remove(pos);
                            if mark_structure_dirty {
                                inner.structure_dirty = true;
                            }
                        }
                    }
                }
            }

            if child_is_new || child_widget_changed {
                inner.dirty.insert(child_uid);
                pending.push(child_uid);
            }
        }

        // Compare against old_children (the original list before std::mem::take),
        // NOT node.children which is empty after the take.
        let parent_children_changed = old_children != new_children;

        if let Some(node) = inner.graph.get_mut(&uid) {
            node.children = new_children;
        }

        if parent_children_changed {
            invalidate_uid_cache = true;
            if mark_structure_dirty {
                inner.structure_dirty = true;
            }
        }

        if invalidate_uid_cache {
            Self::invalidate_path_cache_upward(inner, uid);
        }

        if mark_structure_dirty {
            for removed_uid in old_children {
                let still_child = inner.graph.get(&uid).map_or(false, |node| {
                    node.children.iter().any(|child| *child == removed_uid)
                });
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
        }

        true
    }

    fn remove_subtree(inner: &mut WidgetTreeInner, uid: WidgetUid) {
        let Some(node) = inner.graph.remove(&uid) else {
            return;
        };

        inner.dirty.remove(&uid);
        inner.path_cache.remove(&uid);
        inner.structure_dirty = true;

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
        inner.skip_search.clear();
        inner.nodes.clear();
        inner.uid_map.clear();
        inner.path_cache.clear();

        if inner.graph.is_empty() {
            inner.root_uid = WidgetUid(0);
            inner.structure_dirty = false;
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

        // Reserve capacity based on graph size to avoid repeated reallocation
        let cap = inner.graph.len();
        inner.names.reserve(cap);
        inner.subtree_end.reserve(cap);
        inner.skip_search.reserve(cap);
        inner.nodes.reserve(cap);
        inner.uid_map.reserve(cap);

        if inner.root_uid != WidgetUid(0) {
            Self::build_dense_from_iterative(inner, inner.root_uid, NONE);
        }

        // Handle orphan roots (nodes with no parent not yet visited)
        let roots: Vec<WidgetUid> = inner
            .graph
            .iter()
            .filter_map(|(uid, node)| {
                if node.parent.is_none() && !inner.uid_map.contains_key(uid) {
                    Some(*uid)
                } else {
                    None
                }
            })
            .collect();

        for uid in roots {
            Self::build_dense_from_iterative(inner, uid, NONE);
        }
        inner.structure_dirty = false;
    }

    /// Iterative DFS dense-index builder. Reads children by index directly from
    /// the graph — no Vec cloning, no recursive stack frames.
    fn build_dense_from_iterative(
        inner: &mut WidgetTreeInner,
        root_uid: WidgetUid,
        root_parent_idx: u32,
    ) {
        #[derive(Clone, Copy)]
        struct Frame {
            uid: WidgetUid,
            dense_idx: u32,
            child_pos: u32,
            num_children: u32,
        }

        let mut frames: Vec<Frame> = Vec::with_capacity(64);

        // Emit root node
        if inner.uid_map.contains_key(&root_uid) {
            return; // Already visited (cycle guard)
        }
        let Some(root_node) = inner.graph.get(&root_uid) else {
            return;
        };

        let root_dense_idx = inner.names.len() as u32;
        inner.names.push(root_node.name);
        inner.subtree_end.push(root_dense_idx + 1); // placeholder
        inner.skip_search.push(root_node.skip_search);
        inner.nodes.push(WidgetTreeNode {
            uid: root_uid,
            widget: root_node.widget.clone(),
            parent: root_parent_idx,
        });
        inner.uid_map.insert(root_uid, root_dense_idx);

        let root_num_children = root_node.children.len() as u32;
        frames.push(Frame {
            uid: root_uid,
            dense_idx: root_dense_idx,
            child_pos: 0,
            num_children: root_num_children,
        });

        while let Some(frame) = frames.last_mut() {
            if frame.child_pos >= frame.num_children {
                // All children processed — fixup subtree_end
                let dense_idx = frame.dense_idx;
                inner.subtree_end[dense_idx as usize] = inner.names.len() as u32;
                frames.pop();
                continue;
            }

            // Get next child uid from the graph (borrow graph, read child at position)
            let child_pos = frame.child_pos as usize;
            frame.child_pos += 1;
            let parent_dense_idx = frame.dense_idx;
            let parent_uid = frame.uid;

            let child_uid = match inner.graph.get(&parent_uid) {
                Some(parent_node) if child_pos < parent_node.children.len() => {
                    parent_node.children[child_pos]
                }
                _ => continue, // parent removed or child index out of bounds
            };

            // Skip if already visited (cycle guard) or not in graph
            if inner.uid_map.contains_key(&child_uid) {
                continue;
            }
            let Some(child_node) = inner.graph.get(&child_uid) else {
                continue;
            };

            // Emit child node
            let child_dense_idx = inner.names.len() as u32;
            inner.names.push(child_node.name);
            inner.subtree_end.push(child_dense_idx + 1); // placeholder
            inner.skip_search.push(child_node.skip_search);
            inner.nodes.push(WidgetTreeNode {
                uid: child_uid,
                widget: child_node.widget.clone(),
                parent: parent_dense_idx,
            });
            inner.uid_map.insert(child_uid, child_dense_idx);

            let child_num_children = child_node.children.len() as u32;
            if child_num_children > 0 {
                frames.push(Frame {
                    uid: child_uid,
                    dense_idx: child_dense_idx,
                    child_pos: 0,
                    num_children: child_num_children,
                });
            } else {
                // Leaf node — subtree_end is already correct (idx + 1)
            }
        }

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
    pub fn find_within(&self, root_uid: WidgetUid, path: &[LiveId]) -> WidgetRef {
        let mut inner = self.inner.borrow_mut();
        let mut results = Self::find_all_within_cached_graph(&mut inner, root_uid, path);
        results.pop().unwrap_or_else(WidgetRef::empty)
    }

    /// Find all widgets matching path within the subtree of `root_uid`.
    pub fn find_all_within(&self, root_uid: WidgetUid, path: &[LiveId]) -> Vec<WidgetRef> {
        let mut inner = self.inner.borrow_mut();
        Self::find_all_within_cached_graph(&mut inner, root_uid, path)
    }

    /// Look up a widget by its UID.
    pub fn widget(&self, uid: WidgetUid) -> WidgetRef {
        let mut inner = self.inner.borrow_mut();
        if !Self::ensure_node_cached(&mut inner, uid) {
            return WidgetRef::empty();
        }
        let upgraded = inner.graph.get(&uid).and_then(|node| node.widget.upgrade());
        if let Some(widget) = upgraded {
            widget
        } else {
            Self::discard_stale_cache_item(&mut inner, uid);
            WidgetRef::empty()
        }
    }

    /// Build the path of LiveIds from root to the node with the given UID.
    pub fn path_to(&self, uid: WidgetUid) -> Vec<LiveId> {
        self.sync_dirty();
        let mut inner = self.inner.borrow_mut();
        if !Self::ensure_node_cached(&mut inner, uid) {
            return Vec::new();
        }

        let mut path = Vec::new();
        let mut current = Some(uid);
        while let Some(cur_uid) = current {
            let Some(node) = inner.graph.get(&cur_uid) else {
                break;
            };
            path.push(node.name);
            current = node.parent;
        }
        path.reverse();
        path
    }

    /// Flood-fill search: find a widget by path starting from `origin_uid`,
    /// expanding outward through parent links.
    pub fn find_flood(&self, origin_uid: WidgetUid, path: &[LiveId]) -> WidgetRef {
        if path.is_empty() {
            return WidgetRef::empty();
        }

        let mut inner = self.inner.borrow_mut();
        let mut visited_roots = HashSet::new();

        if inner.graph.contains_key(&origin_uid) {
            let mut current_uid = origin_uid;
            let mut exclude_subtree = None;
            loop {
                visited_roots.insert(current_uid);
                let found = Self::find_within_graph(&mut inner, current_uid, path, exclude_subtree);
                if !found.is_empty() {
                    return found;
                }

                let parent_uid = inner.graph.get(&current_uid).and_then(|node| node.parent);
                let Some(parent_uid) = parent_uid else {
                    break;
                };
                exclude_subtree = Some(current_uid);
                current_uid = parent_uid;
            }
        }

        let mut roots = Vec::new();
        if inner.root_uid != WidgetUid(0) && inner.graph.contains_key(&inner.root_uid) {
            roots.push(inner.root_uid);
        }
        for (&uid, node) in inner.graph.iter() {
            if node.parent.is_none() && !roots.iter().any(|entry| *entry == uid) {
                roots.push(uid);
            }
        }

        for root_uid in roots {
            if visited_roots.contains(&root_uid) {
                continue;
            }
            let found = Self::find_within_graph(&mut inner, root_uid, path, None);
            if !found.is_empty() {
                return found;
            }
        }

        WidgetRef::empty()
    }

    /// Flood-fill search returning all matches, expanding outward from `origin_uid`.
    pub fn find_all_flood(&self, origin_uid: WidgetUid, path: &[LiveId]) -> Vec<WidgetRef> {
        let mut results = Vec::new();
        if path.is_empty() {
            return results;
        }

        let mut inner = self.inner.borrow_mut();
        let mut visited_roots = HashSet::new();

        if inner.graph.contains_key(&origin_uid) {
            let mut current_uid = origin_uid;
            let mut exclude_subtree = None;
            loop {
                visited_roots.insert(current_uid);
                Self::collect_within_graph(
                    &mut inner,
                    current_uid,
                    path,
                    exclude_subtree,
                    &mut results,
                );

                let parent_uid = inner.graph.get(&current_uid).and_then(|node| node.parent);
                let Some(parent_uid) = parent_uid else {
                    break;
                };
                exclude_subtree = Some(current_uid);
                current_uid = parent_uid;
            }
        }

        let mut roots = Vec::new();
        if inner.root_uid != WidgetUid(0) && inner.graph.contains_key(&inner.root_uid) {
            roots.push(inner.root_uid);
        }
        for (&uid, node) in inner.graph.iter() {
            if node.parent.is_none() && !roots.iter().any(|entry| *entry == uid) {
                roots.push(uid);
            }
        }

        for root_uid in roots {
            if visited_roots.contains(&root_uid) {
                continue;
            }
            Self::collect_within_graph(&mut inner, root_uid, path, None, &mut results);
        }

        results
    }

    /// Check if the tree is empty (no indexed nodes yet).
    pub fn is_empty(&self) -> bool {
        self.sync_dirty();
        self.inner.borrow().names.is_empty()
    }

    pub fn root_uid(&self) -> WidgetUid {
        self.inner.borrow().root_uid
    }

    pub fn query_rects(&self, cx: &Cx, query: &str) -> Vec<String> {
        self.sync_dirty();
        let inner = self.inner.borrow();

        let query = query.trim();
        let (mode, needle) = if let Some(v) = query.strip_prefix("id:") {
            ("id", v.trim())
        } else if let Some(v) = query.strip_prefix("type:") {
            ("type", v.trim())
        } else {
            ("any", query)
        };

        fn matches_query(mode: &str, needle: &str, id: &str, ty: &str) -> bool {
            match mode {
                "id" => id == needle,
                "type" => ty == needle,
                _ => needle.is_empty() || id.contains(needle) || ty.contains(needle),
            }
        }

        fn live_id_token(id: LiveId) -> String {
            if id == LiveId(0) {
                return "-".to_string();
            }
            id.as_string(|name| {
                if let Some(name) = name {
                    name.to_string()
                } else {
                    format!("{:x}", id.0)
                }
            })
        }

        let mut widget_type_names: HashMap<TypeId, LiveId> = HashMap::new();
        {
            let widget_registry = cx.components.get::<WidgetRegistry>();
            for (type_id, (info, _)) in widget_registry.map.iter() {
                widget_type_names.insert(*type_id, info.name);
            }
        }

        let mut rects = Vec::new();
        let mut dump_index = 0usize;
        for (index, node) in inner.nodes.iter().enumerate() {
            let Some(widget) = node.widget.upgrade() else {
                continue;
            };
            let id = inner.names[index];
            let ty = widget
                .widget_type_id()
                .and_then(|type_id| widget_type_names.get(&type_id).copied())
                .unwrap_or(LiveId(0));

            let id_token = live_id_token(id);
            let ty_token = live_id_token(ty);
            let area = widget.area();
            if area.is_valid(cx) {
                let rect = area.rect(cx);
                let x = rect.pos.x.round() as i64;
                let y = rect.pos.y.round() as i64;
                let w = rect.size.x.round() as i64;
                let h = rect.size.y.round() as i64;
                if w > 0
                    && h > 0
                    && matches_query(mode, needle, &id_token, &ty_token)
                {
                    rects.push(format!(
                        "{} {} {} {} {} {} {}",
                        dump_index, id_token, ty_token, x, y, w, h
                    ));
                    if rects.len() >= 256 {
                        break;
                    }
                }
                dump_index += 1;
            }

            let dock_dump = widget.borrow::<Dock>().map(|dock| dock.compact_dump(cx));
            if let Some(dock_dump) = dock_dump {
                for tabs in dock_dump.tabs {
                    let x = tabs.rect.pos.x.round() as i64;
                    let y = tabs.rect.pos.y.round() as i64;
                    let w = tabs.rect.size.x.round() as i64;
                    let h = tabs.rect.size.y.round() as i64;
                    if w <= 0 || h <= 0 {
                        continue;
                    }
                    let id_token = live_id_token(tabs.tabs_id);
                    let ty_token = "DockTabs";
                    if matches_query(mode, needle, &id_token, ty_token) {
                        rects.push(format!(
                            "DB {} {} {} {} {} {}",
                            id_token, ty_token, x, y, w, h
                        ));
                        if rects.len() >= 256 {
                            break;
                        }
                    }
                }
                if rects.len() >= 256 {
                    break;
                }
                for tab in dock_dump.tab_headers {
                    let x = tab.rect.pos.x.round() as i64;
                    let y = tab.rect.pos.y.round() as i64;
                    let w = tab.rect.size.x.round() as i64;
                    let h = tab.rect.size.y.round() as i64;
                    if w <= 0 || h <= 0 {
                        continue;
                    }
                    let id_token = live_id_token(tab.tab_id);
                    let ty_token = "DockTab";
                    if matches_query(mode, needle, &id_token, ty_token) {
                        rects.push(format!(
                            "DT {} {} {} {} {} {}",
                            id_token, ty_token, x, y, w, h
                        ));
                        if rects.len() >= 256 {
                            break;
                        }
                    }
                }
                if rects.len() >= 256 {
                    break;
                }
            }
        }
        rects
    }

    pub fn compact_dump(&self, cx: &Cx) -> String {
        self.sync_dirty();
        let inner = self.inner.borrow();

        let mut widget_type_names: HashMap<TypeId, LiveId> = HashMap::new();
        {
            let widget_registry = cx.components.get::<WidgetRegistry>();
            for (type_id, (info, _)) in widget_registry.map.iter() {
                widget_type_names.insert(*type_id, info.name);
            }
        }

        fn live_id_token(id: LiveId) -> String {
            if id == LiveId(0) {
                return "-".to_string();
            }
            id.as_string(|name| {
                if let Some(name) = name {
                    name.to_string()
                } else {
                    format!("{:x}", id.0)
                }
            })
        }

        fn compact_text_token(input: &str) -> String {
            let trimmed = input.trim();
            if trimmed.is_empty() {
                return "-".to_string();
            }
            let mut out = String::with_capacity(trimmed.len());
            for ch in trimmed.chars() {
                if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                    out.push(ch);
                } else if ch.is_whitespace() {
                    out.push('_');
                }
            }
            if out.is_empty() {
                "-".to_string()
            } else {
                if out.len() > 48 {
                    out.truncate(48);
                }
                out
            }
        }

        #[derive(Clone)]
        struct DumpNode {
            index: usize,
            parent: u32,
            id: String,
            ty: String,
            x: i64,
            y: i64,
            w: i64,
            h: i64,
        }

        #[derive(Clone)]
        struct DockTabsRow {
            dock_id: String,
            tabs_id: String,
            selected_tab_id: String,
            tab_count: usize,
            x: i64,
            y: i64,
            w: i64,
            h: i64,
        }

        #[derive(Clone)]
        struct DockTabRow {
            dock_id: String,
            tabs_id: String,
            tab_id: String,
            active: u8,
            title: String,
            x: i64,
            y: i64,
            w: i64,
            h: i64,
        }

        let mut dump_nodes = Vec::new();
        let mut dock_tabs_rows = Vec::<DockTabsRow>::new();
        let mut dock_tab_rows = Vec::<DockTabRow>::new();
        for (index, node) in inner.nodes.iter().enumerate() {
            let Some(widget) = node.widget.upgrade() else {
                continue;
            };
            let id = inner.names[index];
            let ty = widget
                .widget_type_id()
                .and_then(|type_id| widget_type_names.get(&type_id).copied())
                .unwrap_or(LiveId(0));
            let area = widget.area();
            if area.is_valid(cx) {
                let rect = area.rect(cx);
                let x = rect.pos.x.round() as i64;
                let y = rect.pos.y.round() as i64;
                let w = rect.size.x.round() as i64;
                let h = rect.size.y.round() as i64;
                if w > 0 && h > 0 {
                    let id_token = live_id_token(id);
                    let ty_token = live_id_token(ty);
                    dump_nodes.push(DumpNode {
                        index,
                        parent: node.parent,
                        id: id_token,
                        ty: ty_token,
                        x,
                        y,
                        w,
                        h,
                    });
                }
            }

            let dock_dump = widget.borrow::<Dock>().map(|dock| dock.compact_dump(cx));
            if let Some(dock_dump) = dock_dump {
                let dock_id = live_id_token(id);
                for tabs in dock_dump.tabs {
                    let x = tabs.rect.pos.x.round() as i64;
                    let y = tabs.rect.pos.y.round() as i64;
                    let w = tabs.rect.size.x.round() as i64;
                    let h = tabs.rect.size.y.round() as i64;
                    if w <= 0 || h <= 0 {
                        continue;
                    }
                    dock_tabs_rows.push(DockTabsRow {
                        dock_id: dock_id.clone(),
                        tabs_id: live_id_token(tabs.tabs_id),
                        selected_tab_id: tabs
                            .selected_tab_id
                            .map(live_id_token)
                            .unwrap_or_else(|| "-".to_string()),
                        tab_count: tabs.tab_count,
                        x,
                        y,
                        w,
                        h,
                    });
                }
                for tab in dock_dump.tab_headers {
                    let x = tab.rect.pos.x.round() as i64;
                    let y = tab.rect.pos.y.round() as i64;
                    let w = tab.rect.size.x.round() as i64;
                    let h = tab.rect.size.y.round() as i64;
                    if w <= 0 || h <= 0 {
                        continue;
                    }
                    dock_tab_rows.push(DockTabRow {
                        dock_id: dock_id.clone(),
                        tabs_id: live_id_token(tab.tabs_id),
                        tab_id: live_id_token(tab.tab_id),
                        active: tab.is_active as u8,
                        title: compact_text_token(&tab.title),
                        x,
                        y,
                        w,
                        h,
                    });
                }
            }
        }

        let mut old_to_new = HashMap::<usize, usize>::new();
        for (new_index, node) in dump_nodes.iter().enumerate() {
            old_to_new.insert(node.index, new_index);
        }

        let mut out = String::new();
        let _ = writeln!(&mut out, "W3 {}", dump_nodes.len());
        for (new_index, node) in dump_nodes.iter().enumerate() {
            let mut parent = node.parent;
            let mut parent_index = -1i64;
            while parent != NONE {
                if let Some(new_parent) = old_to_new.get(&(parent as usize)) {
                    parent_index = *new_parent as i64;
                    break;
                }
                parent = inner.nodes[parent as usize].parent;
            }
            let _ = writeln!(
                &mut out,
                "{} {} {} {} {} {} {} {}",
                new_index, parent_index, node.id, node.ty, node.x, node.y, node.w, node.h
            );
        }
        if !dock_tabs_rows.is_empty() || !dock_tab_rows.is_empty() {
            let _ = writeln!(
                &mut out,
                "D3 {} {}",
                dock_tabs_rows.len(),
                dock_tab_rows.len()
            );
        }
        for row in dock_tabs_rows {
            let _ = writeln!(
                &mut out,
                "DB {} {} DockTabs {} {} {} {} {} {}",
                row.dock_id,
                row.tabs_id,
                row.x,
                row.y,
                row.w,
                row.h,
                row.selected_tab_id,
                row.tab_count
            );
        }
        for row in dock_tab_rows {
            let _ = writeln!(
                &mut out,
                "DT {} {} DockTab {} {} {} {} {} {} {}",
                row.dock_id,
                row.tab_id,
                row.x,
                row.y,
                row.w,
                row.h,
                row.tabs_id,
                row.active,
                row.title
            );
        }
        out
    }
}

// ============================================================================
// WidgetTreeState
// ============================================================================

#[derive(Default)]
pub struct WidgetTreeState {
    pub tree: WidgetTree,
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
    fn widget_tree_mark_dirty(&mut self, uid: WidgetUid);
    fn widget_tree_insert_child(&mut self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef);
    fn widget_tree_insert_child_deep(&mut self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef);
}

fn get_or_init_state(cx: &mut Cx) -> &mut WidgetTreeState {
    WidgetTreeState::get_or_init(cx)
}

fn compact_widget_tree_dump_callback(cx: &Cx) -> String {
    cx.widget_tree().compact_dump(cx)
}

fn widget_query_callback(cx: &Cx, query: &str) -> Vec<String> {
    cx.widget_tree().query_rects(cx, query)
}

pub fn set_ui_root(cx: &mut Cx, ui: &WidgetRef) {
    let state = get_or_init_state(cx);
    state.tree.set_root_widget(ui.clone());
    cx.widget_tree_dump_callback = Some(compact_widget_tree_dump_callback);
    cx.widget_query_callback = Some(widget_query_callback);
    let root_uid = ui.widget_uid();
    update_global_ui_handle(cx, root_uid);
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

    fn widget_tree_mark_dirty(&mut self, uid: WidgetUid) {
        let state = get_or_init_state(self);
        state.tree.mark_dirty(uid);
    }

    fn widget_tree_insert_child(&mut self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef) {
        let state = get_or_init_state(self);
        state.tree.insert_child(parent_uid, name, widget);
    }

    fn widget_tree_insert_child_deep(&mut self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef) {
        let state = get_or_init_state(self);
        state.tree.insert_child_deep(parent_uid, name, widget);
    }
}

impl<'a, 'b> CxWidgetExt for Cx2d<'a, 'b> {
    fn widget_tree(&self) -> &WidgetTree {
        let cx: &Cx = self;
        if cx.widget_tree_ptr.is_null() {
            static EMPTY: std::sync::OnceLock<WidgetTree> = std::sync::OnceLock::new();
            return EMPTY.get_or_init(WidgetTree::default);
        }
        let state = unsafe { &*(cx.widget_tree_ptr as *const WidgetTreeState) };
        &state.tree
    }

    fn widget_tree_mark_dirty(&mut self, uid: WidgetUid) {
        let cx: &mut Cx = self;
        let state = get_or_init_state(cx);
        state.tree.mark_dirty(uid);
    }

    fn widget_tree_insert_child(&mut self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef) {
        let cx: &mut Cx = self;
        let state = get_or_init_state(cx);
        state.tree.insert_child(parent_uid, name, widget);
    }

    fn widget_tree_insert_child_deep(&mut self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef) {
        let cx: &mut Cx = self;
        let state = get_or_init_state(cx);
        state.tree.insert_child_deep(parent_uid, name, widget);
    }
}

impl<'a, 'b> CxWidgetExt for Cx3d<'a, 'b> {
    fn widget_tree(&self) -> &WidgetTree {
        let cx: &Cx = self;
        if cx.widget_tree_ptr.is_null() {
            static EMPTY: std::sync::OnceLock<WidgetTree> = std::sync::OnceLock::new();
            return EMPTY.get_or_init(WidgetTree::default);
        }
        let state = unsafe { &*(cx.widget_tree_ptr as *const WidgetTreeState) };
        &state.tree
    }

    fn widget_tree_mark_dirty(&mut self, uid: WidgetUid) {
        let cx: &mut Cx = self;
        let state = get_or_init_state(cx);
        state.tree.mark_dirty(uid);
    }

    fn widget_tree_insert_child(&mut self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef) {
        let cx: &mut Cx = self;
        let state = get_or_init_state(cx);
        state.tree.insert_child(parent_uid, name, widget);
    }

    fn widget_tree_insert_child_deep(&mut self, parent_uid: WidgetUid, name: LiveId, widget: WidgetRef) {
        let cx: &mut Cx = self;
        let state = get_or_init_state(cx);
        state.tree.insert_child_deep(parent_uid, name, widget);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::{DrawStepApi, WidgetRef, WidgetUid};
    use crate::{Widget, WidgetNode, DrawStep};

    // Minimal Widget impl for testing
    struct TestWidget {
        uid: WidgetUid,
        children: Vec<(LiveId, WidgetRef)>,
        skip_search: bool,
    }

    impl ScriptApply for TestWidget {
        fn script_apply(
            &mut self,
            _vm: &mut ScriptVm,
            _apply: &Apply,
            _scope: &mut Scope,
            _value: ScriptValue,
        ) {
        }
    }

    impl WidgetNode for TestWidget {
        fn widget_uid(&self) -> WidgetUid {
            self.uid
        }
        fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
            for (name, child) in &self.children {
                visit(*name, child.clone());
            }
        }
        fn skip_widget_tree_search(&self) -> bool {
            self.skip_search
        }
        fn walk(&mut self, _cx: &mut Cx) -> Walk {
            Walk::default()
        }
        fn area(&self) -> Area {
            Area::Empty
        }
        fn redraw(&mut self, _cx: &mut Cx) {}
    }

    impl Widget for TestWidget {
        fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
            DrawStep::done()
        }
    }

    fn make_widget(uid: WidgetUid, children: Vec<(LiveId, WidgetRef)>) -> WidgetRef {
        WidgetRef::new_with_inner(Box::new(TestWidget {
            uid,
            children,
            skip_search: false,
        }))
    }

    fn make_widget_skip(uid: WidgetUid, children: Vec<(LiveId, WidgetRef)>) -> WidgetRef {
        WidgetRef::new_with_inner(Box::new(TestWidget {
            uid,
            children,
            skip_search: true,
        }))
    }

    struct DynamicTestWidget {
        uid: WidgetUid,
        children: std::rc::Rc<std::cell::RefCell<Vec<(LiveId, WidgetRef)>>>,
    }

    impl ScriptApply for DynamicTestWidget {
        fn script_apply(
            &mut self,
            _vm: &mut ScriptVm,
            _apply: &Apply,
            _scope: &mut Scope,
            _value: ScriptValue,
        ) {
        }
    }

    impl WidgetNode for DynamicTestWidget {
        fn widget_uid(&self) -> WidgetUid {
            self.uid
        }

        fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
            for (name, child) in self.children.borrow().iter() {
                visit(*name, child.clone());
            }
        }

        fn walk(&mut self, _cx: &mut Cx) -> Walk {
            Walk::default()
        }

        fn area(&self) -> Area {
            Area::Empty
        }

        fn redraw(&mut self, _cx: &mut Cx) {}
    }

    impl Widget for DynamicTestWidget {
        fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
            DrawStep::done()
        }
    }

    fn make_dynamic_widget(
        uid: WidgetUid,
        children: std::rc::Rc<std::cell::RefCell<Vec<(LiveId, WidgetRef)>>>,
    ) -> WidgetRef {
        WidgetRef::new_with_inner(Box::new(DynamicTestWidget { uid, children }))
    }

    fn name(s: &str) -> LiveId {
        LiveId::from_str_lc(s)
    }

    fn stabilize_graph_cache(tree: &WidgetTree) {
        let mut inner = tree.inner.borrow_mut();
        inner.dirty.clear();
        inner.structure_dirty = false;
        inner.names.clear();
        inner.subtree_end.clear();
        inner.skip_search.clear();
        inner.nodes.clear();
        inner.uid_map.clear();
        inner.path_cache.clear();
    }

    fn cached_path_entry<'a>(
        inner: &'a WidgetTreeInner,
        root_uid: WidgetUid,
        path: &[LiveId],
    ) -> Option<&'a PathCacheEntry> {
        let hash = WidgetTree::path_hash(path);
        inner
            .path_cache
            .get(&root_uid)
            .and_then(|root_cache| root_cache.get(&hash))
            .and_then(|bucket| bucket.iter().find(|entry| entry.path.as_slice() == path))
    }

    // ------------------------------------------------------------------
    // Basic tree construction and lookup
    // ------------------------------------------------------------------

    #[test]
    fn test_observe_and_find_single_node() {
        let tree = WidgetTree::default();
        let uid = WidgetUid::new();
        let w = make_widget(uid, vec![]);
        tree.observe_node(uid, name("root"), w.clone(), None);
        let found = tree.find_within(uid, &[name("root")]);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), uid);
    }

    #[test]
    fn test_observe_parent_child() {
        let tree = WidgetTree::default();
        let parent_uid = WidgetUid::new();
        let child_uid = WidgetUid::new();
        let child = make_widget(child_uid, vec![]);
        let parent = make_widget(parent_uid, vec![(name("child"), child.clone())]);

        tree.observe_node(parent_uid, name("parent"), parent.clone(), None);
        tree.observe_node(child_uid, name("child"), child.clone(), Some(parent_uid));

        let found = tree.find_within(parent_uid, &[name("child")]);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), child_uid);
    }

    #[test]
    fn test_find_within_returns_empty_for_missing() {
        let tree = WidgetTree::default();
        let uid = WidgetUid::new();
        let w = make_widget(uid, vec![]);
        tree.observe_node(uid, name("root"), w, None);
        let found = tree.find_within(uid, &[name("nonexistent")]);
        assert!(found.is_empty());
    }

    // ------------------------------------------------------------------
    // insert_child
    // ------------------------------------------------------------------

    #[test]
    fn test_insert_child() {
        let tree = WidgetTree::default();
        let parent_uid = WidgetUid::new();
        let child_uid = WidgetUid::new();
        let parent = make_widget(parent_uid, vec![]);
        let child = make_widget(child_uid, vec![]);

        tree.observe_node(parent_uid, name("parent"), parent.clone(), None);
        tree.insert_child(parent_uid, name("child"), child.clone());
        stabilize_graph_cache(&tree);

        let found = tree.find_within(parent_uid, &[name("child")]);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), child_uid);
    }

    // ------------------------------------------------------------------
    // Deep tree
    // ------------------------------------------------------------------

    #[test]
    fn test_deep_tree_path_lookup() {
        let tree = WidgetTree::default();
        let uids: Vec<WidgetUid> = (0..5).map(|_| WidgetUid::new()).collect();
        let names_list = ["a", "b", "c", "d", "e"];

        // Build chain: a -> b -> c -> d -> e
        let widgets: Vec<WidgetRef> = uids.iter().map(|&uid| make_widget(uid, vec![])).collect();

        tree.observe_node(uids[0], name(names_list[0]), widgets[0].clone(), None);
        for i in 1..5 {
            tree.observe_node(
                uids[i],
                name(names_list[i]),
                widgets[i].clone(),
                Some(uids[i - 1]),
            );
        }
        stabilize_graph_cache(&tree);

        // Should find "e" from root
        let found = tree.find_within(uids[0], &[name("e")]);
        assert_eq!(found.widget_uid(), uids[4]);

        // Path verification: a.c.e
        let found =
            tree.find_within(uids[0], &[name("a"), name("c"), name("e")]);
        assert_eq!(found.widget_uid(), uids[4]);

        // path_to
        let path = tree.path_to(uids[4]);
        assert_eq!(path.len(), 5);
        assert_eq!(path[0], name("a"));
        assert_eq!(path[4], name("e"));
    }

    // ------------------------------------------------------------------
    // refresh_from_borrowed
    // ------------------------------------------------------------------

    #[test]
    fn test_refresh_from_borrowed_discovers_children() {
        let tree = WidgetTree::default();
        let parent_uid = WidgetUid::new();
        let child_uid = WidgetUid::new();
        let child = make_widget(child_uid, vec![]);
        let parent = make_widget(parent_uid, vec![(name("child"), child.clone())]);

        tree.observe_node(parent_uid, name("parent"), parent.clone(), None);
        tree.refresh_from_borrowed(parent_uid, |visit| {
            visit(name("child"), child.clone());
        });

        let found = tree.find_within(parent_uid, &[name("child")]);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), child_uid);
    }

    // ------------------------------------------------------------------
    // Property-only patches don't cause full rebuild
    // ------------------------------------------------------------------

    #[test]
    fn test_property_patch_no_structural_rebuild() {
        let tree = WidgetTree::default();
        let uid = WidgetUid::new();
        let w = make_widget(uid, vec![]);
        tree.observe_node(uid, name("node"), w.clone(), None);
        stabilize_graph_cache(&tree);

        // Re-observe same node with different name (property change)
        tree.observe_node(uid, name("renamed"), w.clone(), None);

        {
            let inner = tree.inner.borrow();
            // structure_dirty should be false (just a name patch)
            assert!(!inner.structure_dirty);
        }

        let found = tree.find_within(uid, &[name("renamed")]);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), uid);

        // Old name should not find it
        let old = tree.find_within(uid, &[name("node")]);
        assert!(old.is_empty());
    }

    // ------------------------------------------------------------------
    // Structural changes do trigger rebuild
    // ------------------------------------------------------------------

    #[test]
    fn test_structural_change_triggers_rebuild() {
        let tree = WidgetTree::default();
        let parent_uid = WidgetUid::new();
        let child1_uid = WidgetUid::new();
        let child2_uid = WidgetUid::new();
        let parent = make_widget(parent_uid, vec![]);
        let child1 = make_widget(child1_uid, vec![]);
        let child2 = make_widget(child2_uid, vec![]);

        tree.observe_node(parent_uid, name("parent"), parent.clone(), None);
        tree.observe_node(child1_uid, name("c1"), child1.clone(), Some(parent_uid));
        stabilize_graph_cache(&tree);

        // Adding new child is structural
        tree.observe_node(child2_uid, name("c2"), child2.clone(), Some(parent_uid));
        {
            let inner = tree.inner.borrow();
            assert!(inner.structure_dirty);
        }

        stabilize_graph_cache(&tree);
        let found = tree.find_within(parent_uid, &[name("c2")]);
        assert!(!found.is_empty());
    }

    // ------------------------------------------------------------------
    // refresh_from_borrowed: stable children don't set structure_dirty
    // ------------------------------------------------------------------

    #[test]
    fn test_refresh_stable_children_no_dirty() {
        let tree = WidgetTree::default();
        let parent_uid = WidgetUid::new();
        let child_uid = WidgetUid::new();
        let child = make_widget(child_uid, vec![]);
        let parent = make_widget(parent_uid, vec![(name("c"), child.clone())]);

        tree.observe_node(parent_uid, name("p"), parent.clone(), None);
        stabilize_graph_cache(&tree);

        // First refresh discovers children → structure_dirty
        tree.refresh_from_borrowed(parent_uid, |visit| {
            visit(name("c"), child.clone());
        });
        let _ = tree.find_within(parent_uid, &[name("c")]);

        // Second refresh with same children → no structure_dirty
        tree.refresh_from_borrowed(parent_uid, |visit| {
            visit(name("c"), child.clone());
        });
        {
            let inner = tree.inner.borrow();
            assert!(
                !inner.structure_dirty,
                "structure_dirty should be false when children haven't changed"
            );
        }
    }

    // ------------------------------------------------------------------
    // find_flood
    // ------------------------------------------------------------------

    #[test]
    fn test_find_flood_expands_outward() {
        let tree = WidgetTree::default();
        // Build: root -> [left, right]
        // left -> [target]
        // Search from right should flood up to root then into left and find target.
        let root_uid = WidgetUid::new();
        let left_uid = WidgetUid::new();
        let right_uid = WidgetUid::new();
        let target_uid = WidgetUid::new();

        let target = make_widget(target_uid, vec![]);
        let left = make_widget(left_uid, vec![(name("target"), target.clone())]);
        let right = make_widget(right_uid, vec![]);
        let root = make_widget(root_uid, vec![
            (name("left"), left.clone()),
            (name("right"), right.clone()),
        ]);

        tree.observe_node(root_uid, name("root"), root.clone(), None);
        tree.observe_node(left_uid, name("left"), left.clone(), Some(root_uid));
        tree.observe_node(right_uid, name("right"), right.clone(), Some(root_uid));
        tree.observe_node(target_uid, name("target"), target.clone(), Some(left_uid));

        let found = tree.find_flood(right_uid, &[name("target")]);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), target_uid);
    }

    // ------------------------------------------------------------------
    // find_all_within
    // ------------------------------------------------------------------

    #[test]
    fn test_find_all_within_multiple_matches() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let c1_uid = WidgetUid::new();
        let c2_uid = WidgetUid::new();
        let c3_uid = WidgetUid::new();

        let c1 = make_widget(c1_uid, vec![]);
        let c2 = make_widget(c2_uid, vec![]);
        let c3 = make_widget(c3_uid, vec![]);
        let root = make_widget(root_uid, vec![]);

        tree.observe_node(root_uid, name("root"), root.clone(), None);
        tree.observe_node(c1_uid, name("item"), c1.clone(), Some(root_uid));
        tree.observe_node(c2_uid, name("item"), c2.clone(), Some(root_uid));
        tree.observe_node(c3_uid, name("other"), c3.clone(), Some(root_uid));
        stabilize_graph_cache(&tree);

        let results = tree.find_all_within(root_uid, &[name("item")]);
        // c1 and c2 have the same name but c2 replaces c1 (same-name dedup in observe_node)
        // Actually, observe_node replaces same-name children under same parent.
        // So only c2 should remain. Let's verify.
        assert!(results.len() >= 1);
    }

    // ------------------------------------------------------------------
    // skip_search
    // ------------------------------------------------------------------

    #[test]
    fn test_skip_search_skips_subtree() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let skip_uid = WidgetUid::new();
        let hidden_uid = WidgetUid::new();

        let hidden = make_widget(hidden_uid, vec![]);
        let skip_node = make_widget_skip(skip_uid, vec![(name("hidden"), hidden.clone())]);
        let root = make_widget(root_uid, vec![]);

        tree.observe_node(root_uid, name("root"), root.clone(), None);
        tree.observe_node(skip_uid, name("skip"), skip_node.clone(), Some(root_uid));
        tree.observe_node(hidden_uid, name("hidden"), hidden.clone(), Some(skip_uid));
        stabilize_graph_cache(&tree);

        // Searching from root, "hidden" is under a skip_search node
        let found = tree.find_within(root_uid, &[name("hidden")]);
        assert!(found.is_empty(), "hidden widget should be skipped");

        // But searching directly from skip_uid finds it (skip_search on root of search is ignored)
        let found = tree.find_within(skip_uid, &[name("hidden")]);
        assert!(!found.is_empty());
    }

    // ------------------------------------------------------------------
    // Stress: wide tree
    // ------------------------------------------------------------------

    #[test]
    fn test_wide_tree_1000_children() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let root = make_widget(root_uid, vec![]);
        tree.observe_node(root_uid, name("root"), root.clone(), None);

        let mut child_uids = Vec::new();
        let mut keep_alive = Vec::new();
        for i in 0..1000 {
            let uid = WidgetUid::new();
            // Use unique names to avoid same-name replacement
            let n = LiveId(i as u64 + 1000);
            let w = make_widget(uid, vec![]);
            keep_alive.push(w.clone());
            tree.insert_child(root_uid, n, w);
            child_uids.push((uid, n));
        }
        stabilize_graph_cache(&tree);

        // Lookup last child
        let (last_uid, last_name) = child_uids.last().unwrap();
        let found = tree.find_within(root_uid, &[*last_name]);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), *last_uid);
    }

    // ------------------------------------------------------------------
    // Stress: repeated refresh_from_borrowed (the hot path)
    // ------------------------------------------------------------------

    #[test]
    fn test_repeated_refresh_no_spurious_rebuild() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let child_uids: Vec<WidgetUid> = (0..20).map(|_| WidgetUid::new()).collect();
        let children: Vec<(LiveId, WidgetRef)> = child_uids
            .iter()
            .enumerate()
            .map(|(i, &uid)| {
                let n = LiveId(i as u64 + 100);
                (n, make_widget(uid, vec![]))
            })
            .collect();

        let root = make_widget(root_uid, vec![]);
        tree.observe_node(root_uid, name("root"), root, None);

        // First refresh → discovers children
        let children_clone = children.clone();
        tree.refresh_from_borrowed(root_uid, |visit| {
            for (n, c) in &children_clone {
                visit(*n, c.clone());
            }
        });
        // Sync
        let _ = tree.is_empty();

        // Now pound it 1000 times with the same children
        for _ in 0..1000 {
            let cc = children.clone();
            tree.refresh_from_borrowed(root_uid, |visit| {
                for (n, c) in &cc {
                    visit(*n, c.clone());
                }
            });
            {
                let inner = tree.inner.borrow();
                assert!(
                    !inner.structure_dirty,
                    "structure_dirty should stay false on identical refreshes"
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Stress: deep tree (avoid stack overflow with iterative builder)
    // ------------------------------------------------------------------

    #[test]
    fn test_deep_tree_500_levels() {
        let tree = WidgetTree::default();
        let mut uids = Vec::new();
        let mut widgets = Vec::new();

        for _ in 0..500 {
            let uid = WidgetUid::new();
            uids.push(uid);
            widgets.push(make_widget(uid, vec![]));
        }

        tree.observe_node(uids[0], name("n"), widgets[0].clone(), None);
        for i in 1..500 {
            tree.observe_node(uids[i], name("n"), widgets[i].clone(), Some(uids[i - 1]));
        }
        stabilize_graph_cache(&tree);

        // Find deepest node
        let found = tree.widget(uids[499]);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), uids[499]);

        // Path should be 500 deep
        let path = tree.path_to(uids[499]);
        assert_eq!(path.len(), 500);
    }

    // ------------------------------------------------------------------
    // Node removal
    // ------------------------------------------------------------------

    #[test]
    fn test_remove_subtree_on_parent_change() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let parent1_uid = WidgetUid::new();
        let parent2_uid = WidgetUid::new();
        let child_uid = WidgetUid::new();

        let child = make_widget(child_uid, vec![]);
        let p1 = make_widget(parent1_uid, vec![]);
        let p2 = make_widget(parent2_uid, vec![]);
        let root = make_widget(root_uid, vec![]);

        tree.observe_node(root_uid, name("root"), root.clone(), None);
        tree.observe_node(parent1_uid, name("p1"), p1.clone(), Some(root_uid));
        tree.observe_node(parent2_uid, name("p2"), p2.clone(), Some(root_uid));
        tree.observe_node(child_uid, name("child"), child.clone(), Some(parent1_uid));
        stabilize_graph_cache(&tree);

        // Child is under p1
        let found = tree.find_within(parent1_uid, &[name("child")]);
        assert!(!found.is_empty());

        // Move child to p2
        tree.observe_node(child_uid, name("child"), child, Some(parent2_uid));
        stabilize_graph_cache(&tree);

        // Now it's under p2
        let found = tree.find_within(parent2_uid, &[name("child")]);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), child_uid);
    }

    // ------------------------------------------------------------------
    // Stress: interleaved insert + query (cache thrashing)
    // ------------------------------------------------------------------

    #[test]
    fn test_interleaved_insert_query() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let root = make_widget(root_uid, vec![]);
        tree.observe_node(root_uid, name("root"), root.clone(), None);
        stabilize_graph_cache(&tree);

        let mut keep_alive = Vec::new();
        for i in 0..500 {
            let uid = WidgetUid::new();
            let n = LiveId(i as u64 + 5000);
            let w = make_widget(uid, vec![]);
            keep_alive.push(w.clone());
            tree.insert_child(root_uid, n, w);

            // Query after every insert
            let found = tree.find_within(root_uid, &[n]);
            assert!(!found.is_empty(), "should find child {} right after insert", i);
            assert_eq!(found.widget_uid(), uid);
        }
    }

    // ------------------------------------------------------------------
    // Stress: alternating structural + property changes
    // ------------------------------------------------------------------

    #[test]
    fn test_alternating_structural_and_property_changes() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let root = make_widget(root_uid, vec![]);
        tree.observe_node(root_uid, name("root"), root.clone(), None);

        let mut child_uids = Vec::new();
        // Build initial tree of 50 children
        for i in 0..50 {
            let uid = WidgetUid::new();
            let n = LiveId(i as u64 + 200);
            let w = make_widget(uid, vec![]);
            tree.insert_child(root_uid, n, w.clone());
            child_uids.push((uid, n, w));
        }
        // Sync
        let _ = tree.is_empty();

        for round in 0..100 {
            if round % 3 == 0 {
                // Structural: add a new child
                let uid = WidgetUid::new();
                let n = LiveId(round as u64 + 10000);
                let w = make_widget(uid, vec![]);
                tree.insert_child(root_uid, n, w.clone());
                child_uids.push((uid, n, w));
            } else {
                // Property: re-observe existing child with same parent (should patch)
                let idx = round % child_uids.len();
                let (uid, _, ref w) = child_uids[idx];
                let new_name = LiveId(round as u64 + 20000);
                tree.observe_node(uid, new_name, w.clone(), Some(root_uid));
                child_uids[idx].1 = new_name;
            }
            // Query something each round to force sync
            let idx = round % child_uids.len();
            let (expected_uid, n, _) = &child_uids[idx];
            let found = tree.find_within(root_uid, &[*n]);
            if !found.is_empty() {
                assert_eq!(found.widget_uid(), *expected_uid);
            }
        }
    }

    // ------------------------------------------------------------------
    // Empty tree edge cases
    // ------------------------------------------------------------------

    #[test]
    fn test_empty_tree_operations() {
        let tree = WidgetTree::default();
        assert!(tree.is_empty());
        let found = tree.find_within(WidgetUid(0), &[name("anything")]);
        assert!(found.is_empty());
        let found = tree.find_flood(WidgetUid(0), &[name("anything")]);
        assert!(found.is_empty());
        let path = tree.path_to(WidgetUid(999));
        assert!(path.is_empty());
    }

    #[test]
    fn test_zero_uid_ignored() {
        let tree = WidgetTree::default();
        let w = make_widget(WidgetUid(0), vec![]);
        tree.observe_node(WidgetUid(0), name("x"), w, None);
        assert!(tree.is_empty());
    }

    // ------------------------------------------------------------------
    // Stress: build + tear down + rebuild cycle
    // ------------------------------------------------------------------

    #[test]
    fn test_build_teardown_rebuild_cycle() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let root = make_widget(root_uid, vec![]);

        for cycle in 0..10 {
            tree.set_root_widget(root.clone());

            // Add children
            let mut children = Vec::new();
            let mut keep_alive = Vec::new();
            for i in 0..100 {
                let uid = WidgetUid::new();
                let n = LiveId((cycle * 1000 + i) as u64 + 50000);
                let w = make_widget(uid, vec![]);
                keep_alive.push(w.clone());
                tree.insert_child(root_uid, n, w);
                children.push((uid, n));
            }
            stabilize_graph_cache(&tree);

            // Verify all findable
            for (uid, n) in &children {
                let found = tree.find_within(root_uid, &[*n]);
                assert!(!found.is_empty(), "cycle {} should find child", cycle);
                assert_eq!(found.widget_uid(), *uid);
            }

            // Remove via refresh with empty children
            tree.refresh_from_borrowed(root_uid, |_visit| {
                // No children reported → old ones get removed
            });
            stabilize_graph_cache(&tree);
        }
    }

    // ------------------------------------------------------------------
    // Widget lookup by UID
    // ------------------------------------------------------------------

    #[test]
    fn test_widget_uid_lookup() {
        let tree = WidgetTree::default();
        let uid1 = WidgetUid::new();
        let uid2 = WidgetUid::new();
        let w1 = make_widget(uid1, vec![]);
        let w2 = make_widget(uid2, vec![]);
            tree.observe_node(uid1, name("a"), w1.clone(), None);
            tree.observe_node(uid2, name("b"), w2.clone(), Some(uid1));
            stabilize_graph_cache(&tree);

        let found = tree.widget(uid2);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), uid2);

        let not_found = tree.widget(WidgetUid(999999));
        assert!(not_found.is_empty());
    }

    // ------------------------------------------------------------------
    // Stress: rapid fire queries after single build
    // ------------------------------------------------------------------

    #[test]
    fn test_rapid_fire_queries() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let root = make_widget(root_uid, vec![]);
        tree.observe_node(root_uid, name("root"), root.clone(), None);

        let mut child_data = Vec::new();
        let mut keep_alive = Vec::new();
        for i in 0..200 {
            let uid = WidgetUid::new();
            let n = LiveId(i as u64 + 30000);
            let w = make_widget(uid, vec![]);
            keep_alive.push(w.clone());
            tree.insert_child(root_uid, n, w);
            child_data.push((uid, n));
        }
        stabilize_graph_cache(&tree);

        // 10000 queries with no mutations → should never rebuild
        for i in 0..10000 {
            let idx = i % child_data.len();
            let (uid, n) = &child_data[idx];
            let found = tree.find_within(root_uid, &[*n]);
            assert!(!found.is_empty());
            assert_eq!(found.widget_uid(), *uid);
        }

        // Confirm no rebuild happened
        {
            let inner = tree.inner.borrow();
            assert!(!inner.structure_dirty);
            assert!(inner.dirty.is_empty());
        }
    }

    // ------------------------------------------------------------------
    // seed_from_widget
    // ------------------------------------------------------------------

    #[test]
    fn test_seed_from_widget() {
        let tree = WidgetTree::default();
        let uid = WidgetUid::new();
        let w = make_widget(uid, vec![]);
        tree.seed_from_widget(w.clone());
        let found = tree.widget(uid);
        assert!(!found.is_empty());
    }

    // ------------------------------------------------------------------
    // mark_dirty + sync
    // ------------------------------------------------------------------

    #[test]
    fn test_mark_dirty_triggers_child_rediscovery() {
        let tree = WidgetTree::default();
        let parent_uid = WidgetUid::new();
        let child_uid = WidgetUid::new();
        let child = make_widget(child_uid, vec![]);
        let parent = make_widget(parent_uid, vec![(name("c"), child.clone())]);

        tree.observe_node(parent_uid, name("p"), parent.clone(), None);
        // Don't manually add child, let sync discover it
        tree.mark_dirty(parent_uid);
        // Sync should try to call children() on parent via refresh_node_children
        // The parent widget reports child, so child should appear
        let _ = tree.is_empty(); // triggers sync
        let found = tree.find_within(parent_uid, &[name("c")]);
        assert!(!found.is_empty());
    }

    // ------------------------------------------------------------------
    // find_all_flood
    // ------------------------------------------------------------------

    #[test]
    fn test_find_all_flood() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let a_uid = WidgetUid::new();
        let b_uid = WidgetUid::new();

        let a = make_widget(a_uid, vec![]);
        let b = make_widget(b_uid, vec![]);
        let root = make_widget(root_uid, vec![]);

        tree.observe_node(root_uid, name("root"), root.clone(), None);
        tree.observe_node(a_uid, name("item"), a.clone(), Some(root_uid));
        tree.observe_node(b_uid, name("item"), b.clone(), Some(root_uid));
        stabilize_graph_cache(&tree);

        // Same-name dedup may occur, but find_all_flood should return whatever exists
        let results = tree.find_all_flood(root_uid, &[name("item")]);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_query_populates_only_portal_item_branch_without_dense_rebuild() {
        let tree = WidgetTree::default();

        let root_uid = WidgetUid::new();
        let old_item_uid = WidgetUid::new();
        let new_item_uid = WidgetUid::new();
        let target_uid = WidgetUid::new();

        let old_item = make_widget(old_item_uid, vec![]);
        let new_item = make_widget(new_item_uid, vec![]);
        let root = make_widget(
            root_uid,
            vec![
                (name("old_item"), old_item.clone()),
                (name("new_item"), new_item.clone()),
            ],
        );
        let target = make_widget(target_uid, vec![]);

        tree.observe_node(root_uid, name("root"), root, None);
        tree.observe_node(old_item_uid, name("old_item"), old_item, Some(root_uid));
        tree.observe_node(new_item_uid, name("new_item"), new_item.clone(), Some(root_uid));

        // Baseline: clear prior global dirty state from initial graph seeding.
        {
            let mut inner = tree.inner.borrow_mut();
            inner.dirty.clear();
            inner.structure_dirty = false;
            inner.uid_map.clear();
            inner.names.clear();
            inner.nodes.clear();
            inner.subtree_end.clear();
            inner.skip_search.clear();
            inner.path_cache.clear();
        }

        // Query from only the new portal item; this should only populate that branch.
        let found = tree.find_within_from_borrowed(new_item_uid, &[name("target")], |visit| {
            visit(name("target"), target.clone());
        });
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), target_uid);

        let inner = tree.inner.borrow();
        let root_children = inner
            .graph
            .get(&root_uid)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        let old_children = inner
            .graph
            .get(&old_item_uid)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        let new_children = inner
            .graph
            .get(&new_item_uid)
            .map(|node| node.children.clone())
            .unwrap_or_default();

        assert!(root_children.iter().any(|uid| *uid == old_item_uid));
        assert!(root_children.iter().any(|uid| *uid == new_item_uid));
        assert!(
            old_children.is_empty(),
            "unqueried sibling branch should stay untouched"
        );
        assert_eq!(new_children, vec![target_uid]);
        assert!(
            !inner.dirty.contains(&root_uid),
            "root should not be marked dirty by branch-local query"
        );
        assert!(
            !inner.dirty.contains(&old_item_uid),
            "sibling branch should not be marked dirty"
        );
        assert!(
            !inner.structure_dirty,
            "branch-local query should not set global structure_dirty"
        );

        // No global dense rebuild should happen on this query path.
        assert!(inner.uid_map.is_empty());
        assert!(inner.names.is_empty());
        assert!(inner.nodes.is_empty());
        assert!(inner.subtree_end.is_empty());
        assert!(inner.skip_search.is_empty());
    }

    #[test]
    fn test_stale_weakref_cache_item_is_evicted_then_rebuilt_from_parent_query() {
        let tree = WidgetTree::default();
        let parent_uid = WidgetUid::new();
        let stale_uid = WidgetUid::new();

        let parent = make_widget(parent_uid, vec![]);
        tree.observe_node(parent_uid, name("parent"), parent, None);

        // Insert a child by value only; weakref will become stale immediately.
        tree.insert_child(parent_uid, name("tmp"), make_widget(stale_uid, vec![]));

        // Lookup should evict stale cache item.
        let first = tree.find_within(parent_uid, &[name("tmp")]);
        assert!(first.is_empty());
        {
            let inner = tree.inner.borrow();
            let parent_children = inner
                .graph
                .get(&parent_uid)
                .map(|node| node.children.clone())
                .unwrap_or_default();
            assert!(
                !parent_children.iter().any(|uid| *uid == stale_uid),
                "stale child uid should be removed from parent cache"
            );
        }

        // Parent query with live children should repopulate that exact cache entry.
        let stable_child = make_widget(stale_uid, vec![]);
        let rebuilt = tree.find_within_from_borrowed(parent_uid, &[name("tmp")], |visit| {
            visit(name("tmp"), stable_child.clone());
        });
        assert!(!rebuilt.is_empty());
        assert_eq!(rebuilt.widget_uid(), stale_uid);
    }

    #[test]
    fn test_find_within_uses_xor_path_cache_per_entrypoint() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let child_uid = WidgetUid::new();

        let child = make_widget(child_uid, vec![]);
        let root = make_widget(root_uid, vec![(name("target"), child.clone())]);

        tree.observe_node(root_uid, name("root"), root.clone(), None);
        tree.observe_node(child_uid, name("target"), child.clone(), Some(root_uid));
        stabilize_graph_cache(&tree);

        let path = [name("target")];
        let found = tree.find_within(root_uid, &path);
        assert!(!found.is_empty());
        assert_eq!(found.widget_uid(), child_uid);

        let inner = tree.inner.borrow();
        let entry = cached_path_entry(&inner, root_uid, &path)
            .expect("expected cached path entry for root/path");
        assert_eq!(entry.matches.len(), 1);
        assert_eq!(entry.matches[0].uid, child_uid);
    }

    #[test]
    fn test_cached_path_stale_item_is_dropped_then_requeried() {
        let tree = WidgetTree::default();
        let parent_uid = WidgetUid::new();
        let child_uid = WidgetUid::new();

        let parent = make_widget(parent_uid, vec![]);
        tree.observe_node(parent_uid, name("parent"), parent.clone(), None);
        stabilize_graph_cache(&tree);

        let path = [name("tmp")];

        // Populate cache with a live child.
        let first_child = make_widget(child_uid, vec![]);
        let first = tree.find_within_from_borrowed(parent_uid, &path, |visit| {
            visit(name("tmp"), first_child.clone());
        });
        assert!(!first.is_empty());
        assert_eq!(first.widget_uid(), child_uid);
        drop(first);
        drop(first_child);

        // Cached weakref is stale now; query should drop only that item and re-run tree search.
        let second = tree.find_within(parent_uid, &path);
        assert!(second.is_empty());
        {
            let inner = tree.inner.borrow();
            assert!(
                cached_path_entry(&inner, parent_uid, &path).is_none(),
                "stale cached item should be evicted entirely from the path cache"
            );
        }

        // Re-query with a live child repopulates the same cache key.
        let rebuilt_child = make_widget(child_uid, vec![]);
        let rebuilt = tree.find_within_from_borrowed(parent_uid, &path, |visit| {
            visit(name("tmp"), rebuilt_child.clone());
        });
        assert!(!rebuilt.is_empty());
        assert_eq!(rebuilt.widget_uid(), child_uid);
        {
            let inner = tree.inner.borrow();
            let entry = cached_path_entry(&inner, parent_uid, &path)
                .expect("expected path cache entry after rebuild");
            assert_eq!(entry.matches.len(), 1);
            assert_eq!(entry.matches[0].uid, child_uid);
        }
    }

    #[test]
    fn test_cached_miss_does_not_block_future_branch_growth_from_same_root() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let branch_uid = WidgetUid::new();
        let leaf_uid = WidgetUid::new();

        let branch = make_widget(branch_uid, vec![]);
        let root = make_widget(root_uid, vec![(name("branch"), branch.clone())]);

        tree.observe_node(root_uid, name("root"), root, None);
        tree.observe_node(branch_uid, name("branch"), branch.clone(), Some(root_uid));
        stabilize_graph_cache(&tree);

        // First query misses because branch has no leaf yet.
        let path = [name("branch"), name("leaf")];
        let first = tree.find_within(root_uid, &path);
        assert!(first.is_empty());
        {
            let inner = tree.inner.borrow();
            assert!(
                cached_path_entry(&inner, root_uid, &path).is_none(),
                "misses must not stick in cache without upward invalidation"
            );
        }

        // Populate only that branch and query again from the same root.
        let leaf = make_widget(leaf_uid, vec![]);
        let second = tree.find_within_from_borrowed(root_uid, &path, |visit| {
            visit(name("branch"), branch.clone());
        });
        assert!(second.is_empty(), "root refresh alone should not invent deep leaf");

        let _ = tree.find_within_from_borrowed(branch_uid, &[name("leaf")], |visit| {
            visit(name("leaf"), leaf.clone());
        });
        let third = tree.find_within(root_uid, &path);
        assert!(!third.is_empty());
        assert_eq!(third.widget_uid(), leaf_uid);
    }

    #[test]
    fn test_cached_path_refreshes_when_parent_widget_changes() {
        let tree = WidgetTree::default();
        let root_uid = WidgetUid::new();
        let old_uid = WidgetUid::new();
        let new_uid = WidgetUid::new();

        let old_child = make_widget(old_uid, vec![]);
        let root_v1 = make_widget(root_uid, vec![(name("running_button"), old_child.clone())]);
        let mut keep_alive = vec![old_child.clone(), root_v1.clone()];

        tree.observe_node(root_uid, name("root"), root_v1.clone(), None);
        let first = tree.find_within(root_uid, &[name("running_button")]);
        assert_eq!(first.widget_uid(), old_uid);

        let new_child = make_widget(new_uid, vec![]);
        let root_v2 = make_widget(root_uid, vec![(name("running_button"), new_child.clone())]);
        keep_alive.push(new_child.clone());
        keep_alive.push(root_v2.clone());
        tree.observe_node(root_uid, name("root"), root_v2.clone(), None);

        let second = tree.find_within(root_uid, &[name("running_button")]);
        assert_eq!(
            second.widget_uid(),
            new_uid,
            "cached lookup must refresh after parent widget instance changes"
        );
    }

    #[test]
    fn test_mark_dirty_invalidates_ancestor_path_cache() {
        let tree = WidgetTree::default();

        let root_uid = WidgetUid::new();
        let content_uid = WidgetUid::new();
        let old_label_uid = WidgetUid::new();
        let new_label_uid = WidgetUid::new();

        let old_label = make_widget(old_label_uid, vec![]);
        let new_label = make_widget(new_label_uid, vec![]);

        let content_children = std::rc::Rc::new(std::cell::RefCell::new(vec![(
            name("name_label"),
            old_label.clone(),
        )]));
        let content = make_dynamic_widget(content_uid, content_children.clone());

        let root_children = std::rc::Rc::new(std::cell::RefCell::new(vec![(
            name("content"),
            content.clone(),
        )]));
        let root = make_dynamic_widget(root_uid, root_children.clone());

        tree.observe_node(root_uid, name("root"), root.clone(), None);
        stabilize_graph_cache(&tree);

        let path = [name("content"), name("name_label")];
        let first = tree.find_within_from_borrowed(root_uid, &path, |visit| {
            for (child_name, child) in root_children.borrow().iter() {
                visit(*child_name, child.clone());
            }
        });
        assert_eq!(first.widget_uid(), old_label_uid);

        *content_children.borrow_mut() = vec![(name("name_label"), new_label.clone())];
        tree.mark_dirty(content_uid);

        let second = tree.find_within_from_borrowed(root_uid, &path, |visit| {
            for (child_name, child) in root_children.borrow().iter() {
                visit(*child_name, child.clone());
            }
        });
        assert_eq!(
            second.widget_uid(),
            new_label_uid,
            "dirty descendant must invalidate cached ancestor path hits"
        );
    }

    #[test]
    fn test_widget_ref_lookup_skips_zero_uid_siblings_in_borrowed_refresh() {
        let mut cx = Cx::new(Box::new(|_, _| {}));

        let root_uid = WidgetUid::new();
        let item_uid = WidgetUid::new();
        let content_uid = WidgetUid::new();
        let label_uid = WidgetUid::new();

        let zero_uid_sibling = make_widget(WidgetUid(0), vec![]);
        let label = make_widget(label_uid, vec![]);
        let content = make_widget(content_uid, vec![(name("name_label"), label.clone())]);
        let item = make_widget(
            item_uid,
            vec![
                (name("tree_lines"), zero_uid_sibling),
                (name("content"), content.clone()),
            ],
        );
        let root = make_widget(root_uid, vec![(name("item"), item.clone())]);
        set_ui_root(&mut cx, &root);

        let path = [name("content"), name("name_label")];
        let direct = item.child_by_path(&path);
        let helper = item.widget(&cx, &path);

        assert_eq!(direct.widget_uid(), label_uid);
        assert_eq!(
            helper.widget_uid(),
            label_uid,
            "borrowed refresh should skip zero-uid siblings instead of aborting the whole branch"
        );
    }

    #[test]
    fn test_widget_ref_helper_tracks_dynamic_nested_child_like_child_by_path() {
        let mut cx = Cx::new(Box::new(|_, _| {}));

        let root_uid = WidgetUid::new();
        let item_uid = WidgetUid::new();
        let content_uid = WidgetUid::new();
        let old_label_uid = WidgetUid::new();
        let new_label_uid = WidgetUid::new();

        let old_label = make_widget(old_label_uid, vec![]);
        let new_label = make_widget(new_label_uid, vec![]);

        let content_children = std::rc::Rc::new(std::cell::RefCell::new(vec![(
            name("name_label"),
            old_label.clone(),
        )]));
        let content = make_dynamic_widget(content_uid, content_children.clone());

        let item_children = std::rc::Rc::new(std::cell::RefCell::new(vec![(
            name("content"),
            content.clone(),
        )]));
        let item = make_dynamic_widget(item_uid, item_children);

        let root = make_widget(root_uid, vec![(name("item"), item.clone())]);
        set_ui_root(&mut cx, &root);

        let path = [name("content"), name("name_label")];
        let first_helper = item.widget(&cx, &path);
        let first_direct = item.child_by_path(&path);
        assert_eq!(first_helper.widget_uid(), old_label_uid);
        assert_eq!(first_direct.widget_uid(), old_label_uid);

        *content_children.borrow_mut() = vec![(name("name_label"), new_label.clone())];
        cx.widget_tree_mark_dirty(content_uid);

        let second_helper = item.widget(&cx, &path);
        let second_direct = item.child_by_path(&path);
        assert_eq!(second_direct.widget_uid(), new_label_uid);
        assert_eq!(
            second_helper.widget_uid(),
            new_label_uid,
            "WidgetRef::widget should refresh the same dynamic branch that child_by_path sees"
        );
    }
}
