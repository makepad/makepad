use makepad_widgets::{
    makepad_draw::{Area, Cx, LiveId, Walk},
    makepad_script::ScriptApply,
    widget::{Widget, WidgetNode, WidgetRef, WidgetUid},
    widget_tree::WidgetTree,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant};

fn id(name: &str) -> LiveId {
    LiveId::from_str(name)
}

#[derive(Clone)]
struct MockHandle {
    uid: WidgetUid,
    widget: WidgetRef,
    children: Rc<RefCell<Vec<(LiveId, WidgetRef)>>>,
}

impl MockHandle {
    fn new(uid: u64) -> Self {
        Self::new_with_skip(uid, false)
    }

    fn new_with_skip(uid: u64, skip_widget_tree_search: bool) -> Self {
        let children = Rc::new(RefCell::new(Vec::new()));
        let widget = WidgetRef::new_with_inner(Box::new(MockWidget {
            uid: WidgetUid(uid),
            children: children.clone(),
            skip_widget_tree_search,
        }));
        Self {
            uid: WidgetUid(uid),
            widget,
            children,
        }
    }

    fn set_children(&self, children: Vec<(LiveId, WidgetRef)>) {
        *self.children.borrow_mut() = children;
    }

    fn visit(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (name, child) in self.children.borrow().iter() {
            visit(*name, child.clone());
        }
    }
}

struct MockWidget {
    uid: WidgetUid,
    children: Rc<RefCell<Vec<(LiveId, WidgetRef)>>>,
    skip_widget_tree_search: bool,
}

impl ScriptApply for MockWidget {}

impl WidgetNode for MockWidget {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (name, child) in self.children.borrow().iter() {
            visit(*name, child.clone());
        }
    }

    fn skip_widget_tree_search(&self) -> bool {
        self.skip_widget_tree_search
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        Walk::default()
    }

    fn area(&self) -> Area {
        Area::Empty
    }

    fn redraw(&mut self, _cx: &mut Cx) {}
}

impl Widget for MockWidget {}

fn assert_found(tree: &WidgetTree, uid: WidgetUid, path: &[LiveId], label: &str) {
    assert!(
        !tree.find_within(uid, path).is_empty(),
        "lookup failed: {}",
        label
    );
}

fn basic_lookup_test() {
    let tree = WidgetTree::default();
    let root = MockHandle::new(1);
    let dock = MockHandle::new(2);
    let file_tree = MockHandle::new(3);

    dock.set_children(vec![(id("file_tree"), file_tree.widget.clone())]);
    root.set_children(vec![(id("dock"), dock.widget.clone())]);

    tree.set_root_widget(root.widget.clone());
    tree.refresh_from_borrowed(root.uid, |visit| root.visit(visit));
    tree.refresh_from_borrowed(dock.uid, |visit| dock.visit(visit));

    assert_found(&tree, root.uid, &[id("dock")], "root -> dock");
    assert_found(&tree, root.uid, &[id("file_tree")], "root -> file_tree");
    assert_found(
        &tree,
        root.uid,
        &[id("dock"), id("file_tree")],
        "root -> dock/file_tree",
    );
}

fn immediate_portal_item_lookup_test() {
    let tree = WidgetTree::default();
    let root = MockHandle::new(10);
    let portal = MockHandle::new(11);
    let item = MockHandle::new(12);
    let button = MockHandle::new(13);

    item.set_children(vec![(id("button"), button.widget.clone())]);
    portal.set_children(vec![(id("item"), item.widget.clone())]);
    root.set_children(vec![(id("portal"), portal.widget.clone())]);

    tree.set_root_widget(root.widget.clone());
    tree.refresh_from_borrowed(root.uid, |visit| root.visit(visit));
    tree.insert_child(portal.uid, id("item"), item.widget.clone());

    let found = tree.find_within_from_borrowed(item.uid, &[id("button")], |visit| item.visit(visit));
    assert!(!found.is_empty(), "item -> button should resolve immediately");
}

fn hammer_insert_and_lookup(iterations: usize) {
    let tree = WidgetTree::default();
    let root = MockHandle::new(100);
    let portal = MockHandle::new(101);

    root.set_children(vec![(id("portal"), portal.widget.clone())]);
    tree.set_root_widget(root.widget.clone());
    tree.refresh_from_borrowed(root.uid, |visit| root.visit(visit));

    let start = Instant::now();
    for i in 0..iterations {
        let item = MockHandle::new(200 + (i as u64) * 2);
        let button = MockHandle::new(201 + (i as u64) * 2);
        item.set_children(vec![(id("button"), button.widget.clone())]);

        // Replace the same portal slot repeatedly to stress stale-node pruning.
        portal.set_children(vec![(id("item"), item.widget.clone())]);
        tree.insert_child(portal.uid, id("item"), item.widget.clone());

        let found =
            tree.find_within_from_borrowed(item.uid, &[id("button")], |visit| item.visit(visit));
        assert!(
            !found.is_empty(),
            "iteration {}: immediate item button lookup failed",
            i
        );

        if i % 64 == 0 {
            tree.refresh_from_borrowed(portal.uid, |visit| portal.visit(visit));
            assert_found(&tree, root.uid, &[id("button")], "root -> button during hammer");
        }
    }

    println!(
        "hammer completed: {} iterations in {:?}",
        iterations,
        start.elapsed()
    );
}

fn dock_like_hammer(iterations: usize) {
    let tree = WidgetTree::default();

    // Root -> AppUI(Window) -> dock -> file_tree_tab -> file_tree_view -> page_flip -> file_tree(page) -> file_tree(field)
    let root = MockHandle::new(1000);
    let app_ui = MockHandle::new(1001);
    let dock = MockHandle::new(1002);
    let file_tree_tab = MockHandle::new(1003);
    let file_tree_view = MockHandle::new(1004);
    let page_flip = MockHandle::new(1005);
    let file_tree_page = MockHandle::new(1006);
    let file_tree = MockHandle::new(1007);
    let edit_first = MockHandle::new(1008);

    file_tree_page.set_children(vec![(id("file_tree"), file_tree.widget.clone())]);
    page_flip.set_children(vec![(id("file_tree"), file_tree_page.widget.clone())]);
    file_tree_view.set_children(vec![(id("page_flip"), page_flip.widget.clone())]);
    file_tree_tab.set_children(vec![(id("file_tree_view"), file_tree_view.widget.clone())]);
    dock.set_children(vec![
        (id("file_tree_tab"), file_tree_tab.widget.clone()),
        (id("edit_first"), edit_first.widget.clone()),
    ]);
    // Root often has an anonymous child in studio startup (`AppUI{}`), keep that shape.
    app_ui.set_children(vec![(id("dock"), dock.widget.clone())]);
    root.set_children(vec![(LiveId(0), app_ui.widget.clone())]);

    tree.set_root_widget(root.widget.clone());
    tree.refresh_from_borrowed(root.uid, |visit| root.visit(visit));

    assert_found(&tree, root.uid, &[id("dock")], "dock from root");
    assert_found(
        &tree,
        root.uid,
        &[id("file_tree")],
        "file_tree from root in dock-like tree",
    );

    let start = Instant::now();
    for i in 0..iterations {
        // Simulate dock page replacement/recreation while lookups continue.
        let new_page = MockHandle::new(2000 + (i as u64) * 2);
        let new_file_tree = MockHandle::new(2001 + (i as u64) * 2);
        new_page.set_children(vec![(id("file_tree"), new_file_tree.widget.clone())]);
        page_flip.set_children(vec![(id("file_tree"), new_page.widget.clone())]);
        tree.refresh_from_borrowed(page_flip.uid, |visit| page_flip.visit(visit));

        assert_found(
            &tree,
            root.uid,
            &[id("file_tree")],
            "file_tree should stay discoverable while dock content mutates",
        );
    }

    println!(
        "dock-like hammer completed: {} iterations in {:?}",
        iterations,
        start.elapsed()
    );
}

fn placeholder_root_survives_dirty_test() {
    let tree = WidgetTree::default();
    let root = MockHandle::new(3000);
    let dock = MockHandle::new(3001);
    root.set_children(vec![(id("dock"), dock.widget.clone())]);

    // Build from borrowed traversal first (creates placeholder root node without WidgetRef).
    tree.refresh_from_borrowed(root.uid, |visit| root.visit(visit));
    assert_found(
        &tree,
        root.uid,
        &[id("dock")],
        "placeholder root should expose dock",
    );

    // Dirtying placeholder root must not erase its children.
    tree.mark_dirty(root.uid);
    assert_found(
        &tree,
        root.uid,
        &[id("dock")],
        "dirty placeholder root should retain dock",
    );

    // Seeding concrete WidgetRef should upgrade placeholder and remain stable.
    tree.seed_from_widget(root.widget.clone());
    tree.mark_dirty(root.uid);
    assert_found(
        &tree,
        root.uid,
        &[id("dock")],
        "seeded root should retain dock after dirty",
    );
}

fn benchmark_lookup_1000() {
    fn make_tree(skip_heavy_subtree: bool) -> (WidgetTree, WidgetUid) {
        let tree = WidgetTree::default();
        let root = MockHandle::new(4000);
        let heavy = MockHandle::new_with_skip(4001, skip_heavy_subtree);
        let app_ui = MockHandle::new(4002);
        let dock = MockHandle::new(4003);
        let file_tree = MockHandle::new(4004);

        let mut heavy_children = Vec::with_capacity(1000);
        for i in 0..1000 {
            let leaf = MockHandle::new(5000 + i as u64);
            heavy_children.push((LiveId::from_str_num("portal_item", i as u64), leaf.widget));
        }
        heavy.set_children(heavy_children);

        dock.set_children(vec![(id("file_tree"), file_tree.widget.clone())]);
        app_ui.set_children(vec![(id("dock"), dock.widget.clone())]);
        root.set_children(vec![
            (id("heavy_before"), heavy.widget.clone()),
            (LiveId(0), app_ui.widget.clone()),
        ]);

        tree.set_root_widget(root.widget.clone());
        tree.refresh_from_borrowed(root.uid, |visit| root.visit(visit));
        tree.refresh_from_borrowed(heavy.uid, |visit| heavy.visit(visit));
        tree.refresh_from_borrowed(app_ui.uid, |visit| app_ui.visit(visit));
        tree.refresh_from_borrowed(dock.uid, |visit| dock.visit(visit));

        assert_found(&tree, root.uid, &[id("file_tree")], "benchmark seed");
        (tree, root.uid)
    }

    fn bench(tree: &WidgetTree, root_uid: WidgetUid, label: &str) -> f64 {
        let iterations = 200_000usize;
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = tree.find_within(root_uid, &[id("file_tree")]);
        }
        let elapsed = start.elapsed();
        let ns_per_lookup = elapsed.as_nanos() as f64 / iterations as f64;
        println!(
            "{}: {} lookups in {:?} ({:.1} ns/lookup)",
            label, iterations, elapsed, ns_per_lookup
        );
        ns_per_lookup
    }

    fn bench_hashmap(label: &str) -> f64 {
        let iterations = 200_000usize;
        let mut map = HashMap::<LiveId, WidgetRef>::new();
        map.insert(id("file_tree"), WidgetRef::empty());
        let key = id("file_tree");

        let start = Instant::now();
        for _ in 0..iterations {
            let _ = map.get(&key);
        }
        let elapsed = start.elapsed();
        let ns_per_lookup = elapsed.as_nanos() as f64 / iterations as f64;
        println!(
            "{}: {} lookups in {:?} ({:.1} ns/lookup)",
            label, iterations, elapsed, ns_per_lookup
        );
        ns_per_lookup
    }

    let (tree_no_skip, root_no_skip) = make_tree(false);
    let baseline = bench(&tree_no_skip, root_no_skip, "benchmark(no-skip)");

    let (tree_skip, root_skip) = make_tree(true);
    let pruned = bench(&tree_skip, root_skip, "benchmark(skip-heavy-subtree)");
    let direct_hash = bench_hashmap("benchmark(direct-hashmap)");

    if pruned > 0.0 {
        println!("benchmark speedup: {:.2}x", baseline / pruned);
    }
    if direct_hash > 0.0 {
        println!(
            "benchmark index-vs-hashmap (pruned): {:.2}x slower",
            pruned / direct_hash
        );
    }
}

fn benchmark_dirty_mutation_bounds() {
    fn make_tree(skip_heavy_subtree: bool) -> (WidgetTree, WidgetUid, WidgetUid) {
        let tree = WidgetTree::default();
        let root = MockHandle::new(7000);
        let heavy = MockHandle::new_with_skip(7001, skip_heavy_subtree);
        let dock = MockHandle::new(7002);
        let file_tree = MockHandle::new(7003);

        let mut heavy_children = Vec::with_capacity(1000);
        for i in 0..1000 {
            let leaf = MockHandle::new(8000 + i as u64);
            heavy_children.push((LiveId::from_str_num("portal_item", i as u64), leaf.widget));
        }
        heavy.set_children(heavy_children);
        dock.set_children(vec![(id("file_tree"), file_tree.widget.clone())]);
        root.set_children(vec![
            (id("heavy_before"), heavy.widget.clone()),
            (id("dock"), dock.widget.clone()),
        ]);

        tree.set_root_widget(root.widget.clone());
        tree.refresh_from_borrowed(root.uid, |visit| root.visit(visit));
        tree.refresh_from_borrowed(heavy.uid, |visit| heavy.visit(visit));
        tree.refresh_from_borrowed(dock.uid, |visit| dock.visit(visit));
        (tree, root.uid, heavy.uid)
    }

    fn bench(label: &str, tree: &WidgetTree, root_uid: WidgetUid, dirty_uid: WidgetUid) -> f64 {
        let iterations = 1_000usize;
        // Warm cache
        let _ = tree.find_within(root_uid, &[id("file_tree")]);
        let start = Instant::now();
        for _ in 0..iterations {
            tree.mark_dirty(dirty_uid);
            let _ = tree.find_within(root_uid, &[id("file_tree")]);
        }
        let elapsed = start.elapsed();
        let ns_per_iter = elapsed.as_nanos() as f64 / iterations as f64;
        println!(
            "{}: {} dirty+lookup iterations in {:?} ({:.1} ns/iter)",
            label, iterations, elapsed, ns_per_iter
        );
        ns_per_iter
    }

    let (tree_no_skip, root_no_skip, heavy_no_skip) = make_tree(false);
    let cost_no_skip = bench(
        "benchmark(dirty-no-skip)",
        &tree_no_skip,
        root_no_skip,
        heavy_no_skip,
    );

    let (tree_skip, root_skip, heavy_skip) = make_tree(true);
    let cost_skip = bench("benchmark(dirty-skip)", &tree_skip, root_skip, heavy_skip);

    if cost_skip > 0.0 {
        println!(
            "benchmark dirty bound speedup (skip vs no-skip): {:.2}x",
            cost_no_skip / cost_skip
        );
    }
}

fn main() {
    basic_lookup_test();
    immediate_portal_item_lookup_test();
    hammer_insert_and_lookup(2000);
    dock_like_hammer(2000);
    placeholder_root_survives_dirty_test();
    benchmark_lookup_1000();
    benchmark_dirty_mutation_bounds();
    println!("widget_tree tests passed");
}
