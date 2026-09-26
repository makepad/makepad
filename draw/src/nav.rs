#![allow(dead_code)]
use {
    crate::{
        cx_draw::CxDraw, makepad_platform::Area, makepad_platform::Cx,
        makepad_platform::DrawListId, makepad_platform::Inset,
    },
    std::cell::RefCell,
    std::rc::Rc,
};

#[derive(Default)]
pub struct CxNavTree {
    nav_lists: Vec<CxNavList>,
    /// Draw lists that keep Tab inside themselves while they are drawn (an
    /// open modal): the last one reachable from the root holds the focus.
    traps: Vec<DrawListId>,
}

#[derive(Clone)]
pub struct CxNavTreeRc(pub Rc<RefCell<CxNavTree>>);

#[derive(Debug, Default, Clone)]
pub struct CxNavList {
    pub nav_list: Vec<NavItem>,
}

impl std::ops::Index<DrawListId> for CxNavTree {
    type Output = CxNavList;
    fn index(&self, index: DrawListId) -> &Self::Output {
        &self.nav_lists[index.index()]
    }
}

impl std::ops::IndexMut<DrawListId> for CxNavTree {
    fn index_mut(&mut self, index: DrawListId) -> &mut Self::Output {
        &mut self.nav_lists[index.index()]
    }
}

#[derive(Debug, Clone)]
pub enum NavOrder {
    Default,
    Top(u64),
    Middle(u64),
    Bottom(u64),
}

#[derive(Debug, Clone)]
pub struct NavStop {
    pub role: NavRole,
    pub order: NavOrder,
    pub margin: Inset,
    pub area: Area,
}

#[derive(Debug, Clone)]
pub enum NavItem {
    Child(DrawListId),
    Stop(NavStop),
    BeginScroll(Area),
    EndScroll(Area),
}

#[derive(Debug, Clone)]
pub enum NavRole {
    TextInput,
    DropDown,
    Slider,
}

impl<'a> CxDraw<'a> {
    pub fn lazy_construct_nav_tree(cx: &mut Cx) {
        // ok lets fetch/instance our CxFontsAtlasRc
        if !cx.has_global::<CxNavTreeRc>() {
            cx.set_global(CxNavTreeRc(Rc::new(RefCell::new(CxNavTree::default()))));
        }
    }

    pub fn iterate_nav_stops<F>(
        cx: &mut Cx,
        root: DrawListId,
        mut callback: F,
    ) -> Option<(Area, Vec<Area>)>
    where
        F: FnMut(&Cx, &NavStop) -> Option<Area>,
    {
        let nav_tree_rc = cx.get_global::<CxNavTreeRc>().clone();
        let nav_tree = &*nav_tree_rc.0.borrow();
        let mut scroll_stack = Vec::new();
        fn iterate_nav_stops<F>(
            cx: &Cx,
            scroll_stack: &mut Vec<Area>,
            nav_tree: &CxNavTree,
            draw_list_id: DrawListId,
            callback: &mut F,
        ) -> Option<Area>
        where
            F: FnMut(&Cx, &NavStop) -> Option<Area>,
        {
            for i in 0..nav_tree[draw_list_id].nav_list.len() {
                let nav_item = &nav_tree[draw_list_id].nav_list[i];
                match nav_item {
                    NavItem::Child(draw_list_id) => {
                        if let Some(area) =
                            iterate_nav_stops(cx, scroll_stack, nav_tree, *draw_list_id, callback)
                        {
                            return Some(area);
                        }
                    }
                    NavItem::Stop(stop) => {
                        if let Some(area) = callback(cx, stop) {
                            scroll_stack.push(area);
                            return Some(area);
                        }
                    }
                    NavItem::BeginScroll(area) => {
                        scroll_stack.push(*area);
                    }
                    NavItem::EndScroll(area) => {
                        let popped = scroll_stack.pop()?;
                        if *area != popped {
                            return None;
                        }
                    }
                }
            }
            None
        }
        iterate_nav_stops(cx, &mut scroll_stack, nav_tree, root, &mut callback)
            .map(|area| (area, scroll_stack))
    }

    /// Keep Tab inside `draw_list_id` (and what it contains) while `on`.
    pub fn set_nav_trap(cx: &mut Cx, draw_list_id: DrawListId, on: bool) {
        Self::lazy_construct_nav_tree(cx);
        let nav_tree_rc = cx.get_global::<CxNavTreeRc>().clone();
        let mut nav_tree = nav_tree_rc.0.borrow_mut();
        nav_tree.traps.retain(|id| *id != draw_list_id);
        if on {
            nav_tree.traps.push(draw_list_id);
        }
    }

    /// The tab stops Tab moves through, in order: those under the innermost
    /// trap reachable from `root` (an open modal), else all under `root`.
    pub fn nav_scope_stops(cx: &mut Cx, root: DrawListId) -> Vec<Area> {
        Self::lazy_construct_nav_tree(cx);
        let nav_tree_rc = cx.get_global::<CxNavTreeRc>().clone();
        let nav_tree = &*nav_tree_rc.0.borrow();
        fn reachable(nav_tree: &CxNavTree, id: DrawListId, found: &mut Option<DrawListId>, depth: usize) {
            if depth > 64 || id.index() >= nav_tree.nav_lists.len() {
                return;
            }
            if nav_tree.traps.contains(&id) {
                *found = Some(id);
            }
            for item in &nav_tree[id].nav_list {
                if let NavItem::Child(child) = item {
                    reachable(nav_tree, *child, found, depth + 1);
                }
            }
        }
        fn collect(nav_tree: &CxNavTree, id: DrawListId, out: &mut Vec<Area>, depth: usize) {
            if depth > 64 || id.index() >= nav_tree.nav_lists.len() {
                return;
            }
            for item in &nav_tree[id].nav_list {
                match item {
                    NavItem::Child(child) => collect(nav_tree, *child, out, depth + 1),
                    NavItem::Stop(stop) if !stop.area.is_empty() => out.push(stop.area),
                    _ => {}
                }
            }
        }
        let mut trap = None;
        reachable(nav_tree, root, &mut trap, 0);
        let mut out = Vec::new();
        collect(nav_tree, trap.unwrap_or(root), &mut out, 0);
        out
    }

    pub fn nav_list_clear(&mut self, draw_list_id: DrawListId) {
        let mut nav_tree = self.nav_tree_rc.0.borrow_mut();
        if draw_list_id.index() >= nav_tree.nav_lists.len() {
            nav_tree
                .nav_lists
                .resize(draw_list_id.index() + 1, Default::default());
        }
        nav_tree[draw_list_id].nav_list.clear();
    }

    pub fn nav_list_item_push(&mut self, draw_list_id: DrawListId, item: NavItem) {
        let mut nav_tree = self.nav_tree_rc.0.borrow_mut();
        nav_tree[draw_list_id].nav_list.push(item);
    }

    pub fn add_nav_stop(&mut self, area: Area, role: NavRole, margin: Inset) {
        let draw_list_id = *self.draw_list_stack.last().unwrap();
        self.nav_list_item_push(
            draw_list_id,
            NavItem::Stop(NavStop {
                role,
                area,
                order: NavOrder::Default,
                margin,
            }),
        );
    }

    pub fn add_begin_scroll(&mut self) -> NavScrollIndex {
        let mut nav_tree = self.nav_tree_rc.0.borrow_mut();
        let draw_list_id = *self.draw_list_stack.last().unwrap();
        let id = NavScrollIndex(nav_tree[draw_list_id].nav_list.len());
        nav_tree[draw_list_id]
            .nav_list
            .push(NavItem::BeginScroll(Area::Empty));
        id
    }

    pub fn add_end_scroll(&mut self, index: NavScrollIndex, area: Area) {
        let mut nav_tree = self.nav_tree_rc.0.borrow_mut();
        let draw_list_id = *self.draw_list_stack.last().unwrap();
        nav_tree[draw_list_id].nav_list[index.0] = NavItem::BeginScroll(area);
        nav_tree[draw_list_id]
            .nav_list
            .push(NavItem::EndScroll(area));
    }
}

pub struct NavScrollIndex(usize);
