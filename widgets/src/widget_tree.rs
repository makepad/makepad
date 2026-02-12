use {
    crate::makepad_draw::{cx_2d::Cx2d, cx_3d::Cx3d, *},
    crate::widget::{WidgetRef, WidgetUid},
    crate::widget_async::update_global_ui_handle,
    std::collections::HashMap,
};

// WidgetTree contains WidgetRef (Rc-based) which isn't Send/Sync,
// but we only ever access the tree from the main thread. The OnceLock
// for the empty static tree requires Sync.
unsafe impl Send for WidgetTree {}
unsafe impl Sync for WidgetTree {}

const NONE: u32 = u32::MAX;

// ============================================================================
// WidgetTree: split hot/cold layout for fast subtree scans
// ============================================================================

pub struct WidgetTree {
    // Hot path: scanned during find_within (12 bytes per node)
    names: Vec<LiveId>,
    subtree_end: Vec<u32>,

    // Cold path: only touched after finding the target index
    nodes: Vec<WidgetTreeNode>,

    // Index: uid -> node index
    uid_map: HashMap<WidgetUid, u32>,
    root_uid: WidgetUid,
}

struct WidgetTreeNode {
    #[allow(unused)]
    uid: WidgetUid,
    widget: WidgetRef,
    parent: u32,
}

impl Default for WidgetTree {
    fn default() -> Self {
        Self {
            names: Vec::new(),
            subtree_end: Vec::new(),
            nodes: Vec::new(),
            uid_map: HashMap::new(),
            root_uid: WidgetUid(0),
        }
    }
}

impl WidgetTree {
    pub fn begin_frame(&mut self) {
        self.names.clear();
        self.subtree_end.clear();
        self.nodes.clear();
        self.uid_map.clear();
        self.root_uid = WidgetUid(0);
    }

    pub fn append(&mut self, uid: WidgetUid, name: LiveId, widget: WidgetRef, parent: u32) -> u32 {
        let idx = self.names.len() as u32;
        self.names.push(name);
        self.subtree_end.push(idx + 1); // will be updated by close_node
        self.nodes.push(WidgetTreeNode {
            uid,
            widget,
            parent,
        });
        self.uid_map.insert(uid, idx);
        if parent == NONE && self.root_uid == WidgetUid(0) {
            self.root_uid = uid;
        }
        idx
    }

    pub fn close_node(&mut self, idx: u32) {
        self.subtree_end[idx as usize] = self.names.len() as u32;
    }

    /// Find a widget within the subtree of `root_uid` by matching a path of LiveIds.
    /// The last element of `path` is matched first (leaf name), then ancestors
    /// are verified upward.
    pub fn find_within(&self, root_uid: WidgetUid, path: &[LiveId]) -> WidgetRef {
        let (start, end) = match self.uid_map.get(&root_uid) {
            Some(&idx) => (idx as usize, self.subtree_end[idx as usize] as usize),
            // Widget not in tree yet - return empty, let the caller's
            // find_widgets fallback handle it.
            None => return WidgetRef::empty(),
        };
        let target = match path.last() {
            Some(&id) => id,
            None => return WidgetRef::empty(),
        };

        for i in start..end {
            if self.names[i] == target {
                if path.len() == 1 || self.verify_path(&path[..path.len() - 1], i) {
                    return self.nodes[i].widget.clone();
                }
            }
        }
        WidgetRef::empty()
    }

    /// Find all widgets matching path within the subtree of root_uid.
    pub fn find_all_within(&self, root_uid: WidgetUid, path: &[LiveId]) -> Vec<WidgetRef> {
        let mut results = Vec::new();
        let (start, end) = match self.uid_map.get(&root_uid) {
            Some(&idx) => (idx as usize, self.subtree_end[idx as usize] as usize),
            None => return results,
        };
        let target = match path.last() {
            Some(&id) => id,
            None => return results,
        };

        for i in start..end {
            if self.names[i] == target {
                if path.len() == 1 || self.verify_path(&path[..path.len() - 1], i) {
                    results.push(self.nodes[i].widget.clone());
                }
            }
        }
        results
    }

    fn verify_path(&self, remaining: &[LiveId], node_idx: usize) -> bool {
        let mut current = self.nodes[node_idx].parent;
        for &segment in remaining.iter().rev() {
            loop {
                if current == NONE {
                    return false;
                }
                if self.names[current as usize] == segment {
                    current = self.nodes[current as usize].parent;
                    break;
                }
                current = self.nodes[current as usize].parent;
            }
        }
        true
    }

    /// Look up a widget by its UID.
    pub fn widget(&self, uid: WidgetUid) -> WidgetRef {
        match self.uid_map.get(&uid) {
            Some(&idx) => self.nodes[idx as usize].widget.clone(),
            None => WidgetRef::empty(),
        }
    }

    /// Build the path of LiveIds from root to the node with the given UID.
    pub fn path_to(&self, uid: WidgetUid) -> Vec<LiveId> {
        let mut path = Vec::new();
        if let Some(&idx) = self.uid_map.get(&uid) {
            let mut current = idx;
            loop {
                path.push(self.names[current as usize]);
                let parent = self.nodes[current as usize].parent;
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
    ///
    /// Search order:
    /// 1. Search within the origin node's own subtree (children, grandchildren, etc.)
    /// 2. Move to parent, search its subtree excluding the already-visited branch
    /// 3. Continue up through ancestors until found or the entire tree is searched
    ///
    /// This is useful when a widget needs to find a sibling, cousin, or any
    /// relative in the UI tree without knowing the exact structural relationship.
    pub fn find_flood(&self, origin_uid: WidgetUid, path: &[LiveId]) -> WidgetRef {
        let target = match path.last() {
            Some(&id) => id,
            None => return WidgetRef::empty(),
        };

        let origin_idx = match self.uid_map.get(&origin_uid) {
            Some(&idx) => idx as usize,
            None => {
                // Origin not in tree, fall back to full tree search
                return self.find_within_range(0, self.names.len(), target, path);
            }
        };

        // Start with the origin's own subtree
        let origin_end = self.subtree_end[origin_idx] as usize;
        let result = self.find_within_range(origin_idx, origin_end, target, path);
        if !result.is_empty() {
            return result;
        }

        // Expand outward: walk up the parent chain
        let mut exclude_start = origin_idx;
        let mut exclude_end = origin_end;
        let mut current = self.nodes[origin_idx].parent;

        while current != NONE {
            let cur = current as usize;
            let cur_end = self.subtree_end[cur] as usize;

            // Search parent's subtree, skipping the branch we already visited
            let result = self.find_within_range_excluding(
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

            // Move up: the entire current subtree becomes the excluded region
            exclude_start = cur;
            exclude_end = cur_end;
            current = self.nodes[cur].parent;
        }

        // Finally, search any top-level nodes outside the ancestor chain
        self.find_within_range_excluding(
            0,
            self.names.len(),
            exclude_start,
            exclude_end,
            target,
            path,
        )
    }

    /// Flood-fill search returning all matches, expanding outward from `origin_uid`.
    ///
    /// Same expansion order as `find_flood`, but collects every match rather
    /// than stopping at the first one. Results are ordered by proximity: closer
    /// nodes (in the tree) appear first.
    pub fn find_all_flood(&self, origin_uid: WidgetUid, path: &[LiveId]) -> Vec<WidgetRef> {
        let mut results = Vec::new();
        let target = match path.last() {
            Some(&id) => id,
            None => return results,
        };

        let origin_idx = match self.uid_map.get(&origin_uid) {
            Some(&idx) => idx as usize,
            None => {
                self.collect_within_range(&mut results, 0, self.names.len(), target, path);
                return results;
            }
        };

        // Start with the origin's own subtree
        let origin_end = self.subtree_end[origin_idx] as usize;
        self.collect_within_range(&mut results, origin_idx, origin_end, target, path);

        // Expand outward: walk up the parent chain
        let mut exclude_start = origin_idx;
        let mut exclude_end = origin_end;
        let mut current = self.nodes[origin_idx].parent;

        while current != NONE {
            let cur = current as usize;
            let cur_end = self.subtree_end[cur] as usize;

            self.collect_within_range_excluding(
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
            current = self.nodes[cur].parent;
        }

        // Finally, search top-level nodes outside the ancestor chain
        self.collect_within_range_excluding(
            &mut results,
            0,
            self.names.len(),
            exclude_start,
            exclude_end,
            target,
            path,
        );

        results
    }

    // -- helpers for flood search --

    fn find_within_range(
        &self,
        start: usize,
        end: usize,
        target: LiveId,
        path: &[LiveId],
    ) -> WidgetRef {
        for i in start..end {
            if self.names[i] == target
                && (path.len() == 1 || self.verify_path(&path[..path.len() - 1], i))
            {
                return self.nodes[i].widget.clone();
            }
        }
        WidgetRef::empty()
    }

    fn find_within_range_excluding(
        &self,
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
            if self.names[i] == target
                && (path.len() == 1 || self.verify_path(&path[..path.len() - 1], i))
            {
                return self.nodes[i].widget.clone();
            }
            i += 1;
        }
        WidgetRef::empty()
    }

    fn collect_within_range(
        &self,
        results: &mut Vec<WidgetRef>,
        start: usize,
        end: usize,
        target: LiveId,
        path: &[LiveId],
    ) {
        for i in start..end {
            if self.names[i] == target
                && (path.len() == 1 || self.verify_path(&path[..path.len() - 1], i))
            {
                results.push(self.nodes[i].widget.clone());
            }
        }
    }

    fn collect_within_range_excluding(
        &self,
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
            if self.names[i] == target
                && (path.len() == 1 || self.verify_path(&path[..path.len() - 1], i))
            {
                results.push(self.nodes[i].widget.clone());
            }
            i += 1;
        }
    }

    /// Check if the tree is empty (no nodes registered yet).
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn root_uid(&self) -> WidgetUid {
        self.root_uid
    }
}

// ============================================================================
// WidgetTreeDouble: front/back double buffer
// ============================================================================

#[derive(Default)]
pub struct WidgetTreeDouble {
    pub front: WidgetTree,
    pub back: WidgetTree,
    pub cursor_stack: Vec<u32>,
}

impl WidgetTreeDouble {
    fn get_or_init(cx: &mut Cx) -> &mut WidgetTreeDouble {
        if cx.widget_tree_ptr.is_null() {
            let boxed = Box::new(WidgetTreeDouble::default());
            cx.widget_tree_ptr = Box::into_raw(boxed) as *mut ();
        }
        unsafe { &mut *(cx.widget_tree_ptr as *mut WidgetTreeDouble) }
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
}

fn widget_tree_ptr(cx: &Cx) -> *mut () {
    cx.widget_tree_ptr
}

fn get_or_init_double(cx: &mut Cx) -> &mut WidgetTreeDouble {
    WidgetTreeDouble::get_or_init(cx)
}

fn get_double_mut(ptr: *mut ()) -> &'static mut WidgetTreeDouble {
    unsafe { &mut *(ptr as *mut WidgetTreeDouble) }
}

impl CxWidgetExt for Cx {
    fn widget_tree(&self) -> &WidgetTree {
        if self.widget_tree_ptr.is_null() {
            static EMPTY: std::sync::OnceLock<WidgetTree> = std::sync::OnceLock::new();
            return EMPTY.get_or_init(WidgetTree::default);
        }
        let double = unsafe { &*(self.widget_tree_ptr as *const WidgetTreeDouble) };
        &double.front
    }

    fn with_node<F, R>(&mut self, uid: WidgetUid, name: LiveId, widget: WidgetRef, f: F) -> R
    where
        F: FnOnce(&mut Cx) -> R,
    {
        let double = get_or_init_double(self);
        let parent = double.cursor_stack.last().copied().unwrap_or(NONE);
        let idx = double.back.append(uid, name, widget, parent);
        double.cursor_stack.push(idx);

        let r = f(self);

        let double = get_double_mut(self.widget_tree_ptr);
        double.cursor_stack.pop();
        double.back.close_node(idx);
        r
    }

    fn with_widget_tree<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Cx) -> R,
    {
        let double = get_or_init_double(self);
        double.back.begin_frame();
        double.cursor_stack.clear();
        let r = f(self);
        let root_uid = {
            let double = get_double_mut(self.widget_tree_ptr);
            std::mem::swap(&mut double.front, &mut double.back);
            double.front.root_uid()
        };
        update_global_ui_handle(self, root_uid);
        r
    }
}

impl<'a, 'b> CxWidgetExt for Cx2d<'a, 'b> {
    fn widget_tree(&self) -> &WidgetTree {
        let ptr = widget_tree_ptr(self);
        if ptr.is_null() {
            static EMPTY: std::sync::OnceLock<WidgetTree> = std::sync::OnceLock::new();
            return EMPTY.get_or_init(WidgetTree::default);
        }
        let double = unsafe { &*(ptr as *const WidgetTreeDouble) };
        &double.front
    }

    fn with_node<F, R>(&mut self, uid: WidgetUid, name: LiveId, widget: WidgetRef, f: F) -> R
    where
        F: FnOnce(&mut Cx2d<'a, 'b>) -> R,
    {
        let ptr = {
            let cx: &mut Cx = self;
            let double = get_or_init_double(cx);
            let parent = double.cursor_stack.last().copied().unwrap_or(NONE);
            let idx = double.back.append(uid, name, widget, parent);
            double.cursor_stack.push(idx);
            (cx.widget_tree_ptr, idx)
        };

        let r = f(self);

        let double = get_double_mut(ptr.0);
        double.cursor_stack.pop();
        double.back.close_node(ptr.1);
        r
    }

    fn with_widget_tree<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Cx2d<'a, 'b>) -> R,
    {
        {
            let cx: &mut Cx = self;
            let double = get_or_init_double(cx);
            double.back.begin_frame();
            double.cursor_stack.clear();
        }
        let r = f(self);
        {
            let cx: &mut Cx = self;
            let double = get_double_mut(cx.widget_tree_ptr);
            std::mem::swap(&mut double.front, &mut double.back);
        }
        r
    }
}

impl<'a, 'b> CxWidgetExt for Cx3d<'a, 'b> {
    fn widget_tree(&self) -> &WidgetTree {
        let ptr = widget_tree_ptr(self);
        if ptr.is_null() {
            static EMPTY: std::sync::OnceLock<WidgetTree> = std::sync::OnceLock::new();
            return EMPTY.get_or_init(WidgetTree::default);
        }
        let double = unsafe { &*(ptr as *const WidgetTreeDouble) };
        &double.front
    }

    fn with_node<F, R>(&mut self, uid: WidgetUid, name: LiveId, widget: WidgetRef, f: F) -> R
    where
        F: FnOnce(&mut Cx3d<'a, 'b>) -> R,
    {
        let ptr = {
            let cx: &mut Cx = self;
            let double = get_or_init_double(cx);
            let parent = double.cursor_stack.last().copied().unwrap_or(NONE);
            let idx = double.back.append(uid, name, widget, parent);
            double.cursor_stack.push(idx);
            (cx.widget_tree_ptr, idx)
        };

        let r = f(self);

        let double = get_double_mut(ptr.0);
        double.cursor_stack.pop();
        double.back.close_node(ptr.1);
        r
    }

    fn with_widget_tree<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Cx3d<'a, 'b>) -> R,
    {
        {
            let cx: &mut Cx = self;
            let double = get_or_init_double(cx);
            double.back.begin_frame();
            double.cursor_stack.clear();
        }
        let r = f(self);
        {
            let cx: &mut Cx = self;
            let double = get_double_mut(cx.widget_tree_ptr);
            std::mem::swap(&mut double.front, &mut double.back);
        }
        r
    }
}
