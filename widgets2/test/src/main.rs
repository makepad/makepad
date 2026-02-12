use makepad_widgets2::makepad_draw::live_id::LiveId;
use makepad_widgets2::widget::{WidgetRef, WidgetUid};
use makepad_widgets2::widget_tree::WidgetTree;

const NONE: u32 = u32::MAX;

/// Helper to build a tree node, run a closure for children, then close it.
fn node(
    tree: &mut WidgetTree,
    uid: u64,
    name: &str,
    parent: u32,
    f: impl FnOnce(&mut WidgetTree, u32),
) {
    let id = LiveId::from_str(name);
    let idx = tree.append(WidgetUid(uid), id, WidgetRef::empty(), parent);
    f(tree, idx);
    tree.close_node(idx);
}

/// Use find_all_within().len() to check whether a search finds results,
/// since all our WidgetRefs are empty shells and is_empty() won't distinguish them.
fn found(tree: &WidgetTree, root: WidgetUid, path: &[LiveId]) -> bool {
    tree.find_all_within(root, path).len() > 0
}

/// Build a tree mimicking the studio layout:
///
/// root (uid=1)
///   main_window (uid=2)
///     body (uid=3)
///       dock (uid=4)
///         edit_tabs (uid=5)
///           run_first (uid=6)
///           design_first (uid=7)
///           outline_first (uid=8)
///         log_tabs (uid=9)
///           log_list_tab (uid=10)
///         file_tree_tabs (uid=11)
///           file_tree_tab (uid=12)
///           search (uid=13)
///       my_button (uid=14)
///     other_view (uid=15)
///       nested (uid=16)
///         deep_label (uid=17)
///
fn build_test_tree() -> WidgetTree {
    let mut tree = WidgetTree::default();
    tree.begin_frame();

    node(&mut tree, 1, "root", NONE, |tree, root| {
        node(tree, 2, "main_window", root, |tree, win| {
            node(tree, 3, "body", win, |tree, body| {
                node(tree, 4, "dock", body, |tree, dock| {
                    node(tree, 5, "edit_tabs", dock, |tree, tabs| {
                        node(tree, 6, "run_first", tabs, |_, _| {});
                        node(tree, 7, "design_first", tabs, |_, _| {});
                        node(tree, 8, "outline_first", tabs, |_, _| {});
                    });
                    node(tree, 9, "log_tabs", dock, |tree, tabs| {
                        node(tree, 10, "log_list_tab", tabs, |_, _| {});
                    });
                    node(tree, 11, "file_tree_tabs", dock, |tree, tabs| {
                        node(tree, 12, "file_tree_tab", tabs, |_, _| {});
                        node(tree, 13, "search", tabs, |_, _| {});
                    });
                });
                node(tree, 14, "my_button", body, |_, _| {});
            });
            node(tree, 15, "other_view", win, |tree, ov| {
                node(tree, 16, "nested", ov, |tree, nested| {
                    node(tree, 17, "deep_label", nested, |_, _| {});
                });
            });
        });
    });

    tree
}

/// Build a tree the way the code ACTUALLY works at runtime:
/// Root registers its children but NOT itself.
/// This reproduces the real bug.
fn build_realistic_tree() -> WidgetTree {
    let mut tree = WidgetTree::default();
    tree.begin_frame();

    // Root (uid=1) is NOT registered - it only registers its children.
    // This is what Root::handle_event does:
    //   for (id, component) in self.components.iter_mut() {
    //       cx.with_node(component.widget_uid(), *id, component.clone(), |cx| { ... });
    //   }
    let root_parent = NONE; // no parent because root isn't in the tree
    node(&mut tree, 2, "main_window", root_parent, |tree, win| {
        node(tree, 3, "body", win, |tree, body| {
            node(tree, 4, "dock", body, |tree, dock| {
                node(tree, 5, "edit_tabs", dock, |tree, tabs| {
                    node(tree, 6, "run_first", tabs, |_, _| {});
                    node(tree, 7, "design_first", tabs, |_, _| {});
                    node(tree, 8, "outline_first", tabs, |_, _| {});
                });
            });
            node(tree, 14, "my_button", body, |_, _| {});
        });
    });

    tree
}

fn id(name: &str) -> LiveId {
    LiveId::from_str(name)
}

fn main() {
    let tree = build_test_tree();
    let mut pass = 0;
    let mut fail = 0;

    macro_rules! check {
        ($name:expr, $cond:expr) => {
            if $cond {
                pass += 1;
                println!("  PASS: {}", $name);
            } else {
                fail += 1;
                println!("  FAIL: {}", $name);
            }
        };
    }

    // === Test 1: find direct child by single name ===
    println!("\n--- find_within: single-name lookups from root ---");
    {
        let root = WidgetUid(1);

        check!(
            "root -> main_window",
            found(&tree, root, &[id("main_window")])
        );
        check!("root -> dock", found(&tree, root, &[id("dock")]));
        check!("root -> run_first", found(&tree, root, &[id("run_first")]));
        check!(
            "root -> nonexistent (should miss)",
            !found(&tree, root, &[id("nonexistent")])
        );
    }

    // === Test 2: find from a subtree root ===
    println!("\n--- find_within: subtree-scoped lookups ---");
    {
        let dock = WidgetUid(4);

        check!("dock -> run_first", found(&tree, dock, &[id("run_first")]));
        check!(
            "dock -> my_button (should miss)",
            !found(&tree, dock, &[id("my_button")])
        );
        check!(
            "dock -> deep_label (should miss)",
            !found(&tree, dock, &[id("deep_label")])
        );
    }

    // === Test 3: multi-segment path lookup ===
    println!("\n--- find_within: multi-segment paths ---");
    {
        let root = WidgetUid(1);

        check!(
            "root -> [dock, run_first]",
            found(&tree, root, &[id("dock"), id("run_first")])
        );
        check!(
            "root -> [edit_tabs, run_first]",
            found(&tree, root, &[id("edit_tabs"), id("run_first")])
        );
        check!(
            "root -> [log_tabs, run_first] (should miss)",
            !found(&tree, root, &[id("log_tabs"), id("run_first")])
        );
        check!(
            "root -> [other_view, nested, deep_label]",
            found(
                &tree,
                root,
                &[id("other_view"), id("nested"), id("deep_label")]
            )
        );
        check!(
            "root -> [body, deep_label] (should miss)",
            !found(&tree, root, &[id("body"), id("deep_label")])
        );
    }

    // === Test 4: find_within from non-root widget ===
    println!(
        "\n--- find_within: from non-root widgets (simulates self.ui.widget(cx, ids!(x))) ---"
    );
    {
        check!(
            "main_window -> dock",
            found(&tree, WidgetUid(2), &[id("dock")])
        );
        check!(
            "main_window -> run_first",
            found(&tree, WidgetUid(2), &[id("run_first")])
        );
        check!(
            "body -> my_button",
            found(&tree, WidgetUid(3), &[id("my_button")])
        );
        check!(
            "other_view -> dock (should miss)",
            !found(&tree, WidgetUid(15), &[id("dock")])
        );
    }

    // === Test 5: find_all_within counts ===
    println!("\n--- find_all_within ---");
    {
        let root = WidgetUid(1);
        let ws = tree.find_all_within(root, &[id("run_first")]);
        check!("find_all root -> run_first (count=1)", ws.len() == 1);

        let ws = tree.find_all_within(root, &[id("nonexistent")]);
        check!("find_all root -> nonexistent (count=0)", ws.len() == 0);
    }

    // === Test 6: uid lookup ===
    println!("\n--- uid_map lookup ---");
    {
        // uid_map should contain all 17 nodes
        let path = tree.path_to(WidgetUid(6));
        check!("uid 6 in uid_map", !path.is_empty());

        let path = tree.path_to(WidgetUid(999));
        check!("uid 999 not in uid_map", path.is_empty());
    }

    // === Test 7: path_to ===
    println!("\n--- path_to ---");
    {
        let path = tree.path_to(WidgetUid(6));
        let expected = vec![
            id("root"),
            id("main_window"),
            id("body"),
            id("dock"),
            id("edit_tabs"),
            id("run_first"),
        ];
        check!("path_to(run_first)", path == expected);

        let path = tree.path_to(WidgetUid(17));
        let expected = vec![
            id("root"),
            id("main_window"),
            id("other_view"),
            id("nested"),
            id("deep_label"),
        ];
        check!("path_to(deep_label)", path == expected);
    }

    // === Test 8: edge cases ===
    println!("\n--- edge cases ---");
    {
        // Unknown root_uid now falls back to searching entire tree (needed for Root widget)
        check!(
            "unknown root_uid -> searches whole tree",
            found(&tree, WidgetUid(999), &[id("run_first")])
        );
        check!("empty path -> miss", !found(&tree, WidgetUid(1), &[]));

        // Self-lookup: searching for "root" from root should find root itself
        check!(
            "root -> root (self)",
            found(&tree, WidgetUid(1), &[id("root")])
        );

        // Leaf as root: search from a leaf node
        check!(
            "leaf -> self name",
            found(&tree, WidgetUid(6), &[id("run_first")])
        );
        check!(
            "leaf -> any child (should miss)",
            !found(&tree, WidgetUid(6), &[id("dock")])
        );
    }

    // === Test 9: REALISTIC tree (root not registered) ===
    // This reproduces the actual runtime structure where Root doesn't register itself.
    // self.ui.widget_uid() returns Root's UID (1), but Root is NOT in the tree.
    println!("\n--- REALISTIC: root not in tree (reproduces runtime bug) ---");
    {
        let real_tree = build_realistic_tree();
        let root_uid = WidgetUid(1); // Root's UID, but it's NOT in the tree

        // This is what self.ui.dock(cx, ids!(dock)) does:
        // cx.widget_tree().find_within(self.ui.widget_uid(), &[id("dock")])
        // Since root_uid=1 is NOT in uid_map, find_within returns empty immediately.
        let found_dock = real_tree.find_all_within(root_uid, &[id("dock")]).len() > 0;
        check!(
            "REALISTIC: root(uid=1) -> dock (ROOT NOT IN TREE)",
            found_dock
        );

        let found_run = real_tree
            .find_all_within(root_uid, &[id("run_first")])
            .len()
            > 0;
        check!(
            "REALISTIC: root(uid=1) -> run_first (ROOT NOT IN TREE)",
            found_run
        );

        // But if we search from main_window (uid=2), which IS in the tree, it works:
        let found_dock2 = real_tree.find_all_within(WidgetUid(2), &[id("dock")]).len() > 0;
        check!("REALISTIC: main_window(uid=2) -> dock (WORKS)", found_dock2);

        let found_run2 = real_tree
            .find_all_within(WidgetUid(2), &[id("run_first")])
            .len()
            > 0;
        check!(
            "REALISTIC: main_window(uid=2) -> run_first (WORKS)",
            found_run2
        );
    }

    // === Summary ===
    println!("\n========================================");
    println!("  {} passed, {} failed", pass, fail);
    println!("========================================");

    if fail > 0 {
        std::process::exit(1);
    }
}
