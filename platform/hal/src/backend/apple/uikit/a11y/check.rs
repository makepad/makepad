//! Main-thread loopback through the public UIKit container and element selectors.
use super::*;
use std::time::Duration;

#[derive(Default)]
struct Check {
    nodes: usize,
    texts: usize,
    actions: usize,
    mismatches: usize,
    errors: Vec<String>,
    samples: Vec<String>,
    request: Option<u64>,
    elapsed_us: u128,
}
impl Check {
    fn test(&mut self, ok: bool, id: NodeId, field: &str) {
        if !ok {
            self.mismatches += 1;
            self.errors.push(format!("#{} {field}", id.short()));
        }
    }
    fn report(&self) -> String {
        let mut out = format!(
            "oscheck uikit nodes={} text={} actions={} mismatches={} main_us={}\n",
            self.nodes, self.texts, self.actions, self.mismatches, self.elapsed_us
        );
        for line in self.errors.iter().chain(&self.samples) {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}
fn string(obj: ObjcId) -> String {
    if obj == nil {
        String::new()
    } else {
        nsstring_to_string(obj)
    }
}
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 0.01
}
unsafe fn object_children(check: &mut Check, obj: ObjcId, id: NodeId) -> Vec<ObjcId> {
    let array: ObjcId = msg_send![obj, accessibilityElements];
    let len: usize = msg_send![array, count];
    let count: isize = msg_send![obj, accessibilityElementCount];
    check.test(count == len as isize, id, "container count");
    let before: ObjcId = msg_send![obj, accessibilityElementAtIndex: -1isize];
    let after: ObjcId = msg_send![obj, accessibilityElementAtIndex: len as isize];
    let missing: isize = msg_send![obj, indexOfAccessibilityElement: nil];
    check.test(
        before == nil && after == nil && missing == isize::MAX,
        id,
        "container bounds",
    );
    (0..len)
        .map(|i| {
            let child: ObjcId = msg_send![array, objectAtIndex: i];
            let indexed: ObjcId = msg_send![obj, accessibilityElementAtIndex: i as isize];
            let index: isize = msg_send![obj, indexOfAccessibilityElement: child];
            check.test(
                child == indexed && index == i as isize,
                id,
                "container indexing",
            );
            child
        })
        .collect()
}
fn check_main(key: u64, act: bool, cancelled: &std::sync::atomic::AtomicBool) -> Check {
    let mut check = Check::default();
    let v = view(key);
    if v == nil {
        check.test(false, NodeId::NONE, "window unavailable");
        return check;
    }
    activate();
    let Some(tree) = hal::with_tree(key, Clone::clone) else {
        check.test(false, NodeId::NONE, "tree unavailable");
        return check;
    };
    let children_of = |id| tree.children(id).map(|n| n.id).collect::<Vec<_>>();
    let mut expected = HashSet::new();
    let mut pending = children_of(tree.root);
    while let Some(id) = pending.pop() {
        if expected.insert(id) {
            pending.extend(children_of(id));
        }
    }
    let mut seen = HashSet::new();
    let mut pressable = nil;
    unsafe {
        let is_element: BOOL = msg_send![v, isAccessibilityElement];
        check.test(is_element == NO, tree.root, "view is a container");
        let mut roots = object_children(&mut check, v, tree.root);
        let views = child_views(&*v);
        let count = children_of(tree.root).len();
        check.test(
            roots.get(count..) == Some(views.as_slice()),
            tree.root,
            "nested HAL windows",
        );
        roots.truncate(count);
        let root_ids: Vec<_> = roots
            .iter()
            .map(|o| NodeId(*(**o).get_ivar::<u64>("node")))
            .collect();
        check.test(
            root_ids == children_of(tree.root),
            tree.root,
            "root children",
        );
        let mut stack: Vec<_> = roots.into_iter().rev().map(|o| (o, v)).collect();
        while let Some((obj, expected_parent)) = stack.pop() {
            let id = NodeId(*(*obj).get_ivar::<u64>("node"));
            if !seen.insert(id) {
                check.test(false, id, "duplicate/cycle");
                continue;
            }
            let Some(n) = tree.node(id) else {
                check.test(false, id, "unexpected element");
                continue;
            };
            check.nodes += 1;
            let label: ObjcId = msg_send![obj, accessibilityLabel];
            let value: ObjcId = msg_send![obj, accessibilityValue];
            let hint: ObjcId = msg_send![obj, accessibilityHint];
            let parent: ObjcId = msg_send![obj, accessibilityContainer];
            let traits: u64 = msg_send![obj, accessibilityTraits];
            let is_element: BOOL = msg_send![obj, isAccessibilityElement];
            let modal: BOOL = msg_send![obj, accessibilityViewIsModal];
            let hidden: BOOL = msg_send![obj, accessibilityElementsHidden];
            check.test(
                (hidden != NO) == n.states.contains(States::HIDDEN),
                id,
                "hidden subtree",
            );
            check.test(string(label) == label_text(n), id, "label");
            check.test(string(value) == value_text(n), id, "value");
            check.test(string(hint) == n.description, id, "hint");
            check.test(parent == expected_parent, id, "parent");
            check.test(traits == semantic_traits(n), id, "traits");
            check.test((is_element != NO) == exposed(n), id, "element/container");
            check.test(
                (modal != NO) == n.states.contains(States::MODAL),
                id,
                "modal",
            );
            // Independent inverse of the UIKit screen conversion, in the view's coordinate space.
            let rect: NSRect = msg_send![obj, accessibilityFrame];
            let window: ObjcId = msg_send![v, window];
            let screen: ObjcId = msg_send![window, screen];
            let space: ObjcId = msg_send![screen, coordinateSpace];
            let local: NSRect = msg_send![v, convertRect: rect fromCoordinateSpace: space];
            let bounds: NSRect = msg_send![v, bounds];
            check.test(
                near(local.origin.x - bounds.origin.x, n.bounds.x as f64)
                    && near(local.origin.y - bounds.origin.y, n.bounds.y as f64)
                    && near(local.size.width, n.bounds.w as f64)
                    && near(local.size.height, n.bounds.h as f64),
                id,
                "frame",
            );
            let focused: BOOL = msg_send![obj, accessibilityElementIsFocused];
            check.test(
                (focused != NO) == FOCUS.with(|f| f.borrow().get(&key) == Some(&id)),
                id,
                "AT focus",
            );
            let custom: ObjcId = msg_send![obj, accessibilityCustomActions];
            let count: usize = msg_send![custom, count];
            let names =
                if n.actions.contains(Actions::CUSTOM) && !n.states.contains(States::DISABLED) {
                    n.custom_actions.as_slice()
                } else {
                    &[]
                };
            check.test(count == names.len(), id, "custom action count");
            for (i, name) in names.iter().take(count).enumerate() {
                let action: ObjcId = msg_send![custom, objectAtIndex: i];
                let actual: ObjcId = msg_send![action, name];
                check.test(string(actual) == *name, id, "custom action name");
            }
            for selector in [
                sel!(accessibilityActivate),
                sel!(accessibilityIncrement),
                sel!(accessibilityDecrement),
                sel!(accessibilityScroll:),
                sel!(accessibilityPerformEscape),
            ] {
                let responds: BOOL = msg_send![obj, respondsToSelector: selector];
                check.test(responds != NO, id, selector.name());
            }
            if n.actions.contains(activation_action(n)) && !n.states.contains(States::DISABLED) {
                check.actions += 1;
                // oscheck act=1 only presses/toggles; it does not summon the keyboard.
                if pressable == nil && activation_action(n) != Actions::FOCUS {
                    pressable = obj;
                }
            }
            if n.role.is_text_input() {
                check.texts += 1;
                #[cfg(target_os = "ios")]
                {
                    let responder: ObjcId = msg_send![obj, accessibilityTextInputResponder];
                    check.test(
                        responder == super::super::text::accessibility_responder(key, id),
                        id,
                        "text responder owner",
                    );
                    if responder != nil {
                        let exposed: BOOL = msg_send![responder, isAccessibilityElement];
                        let focusable: BOOL = msg_send![responder, canBecomeFocused];
                        let first: BOOL = msg_send![responder, isFirstResponder];
                        check.test(
                            exposed == NO && focusable == NO && first != NO,
                            id,
                            "hidden text client",
                        );
                    }
                }
            }
            if let Some(t) = &n.text {
                let page: ObjcId = msg_send![obj, accessibilityPageContent];
                check.test(string(page) == t.content, id, "reading content");
                for (i, line) in t.lines.iter().enumerate() {
                    let content: ObjcId =
                        msg_send![obj, accessibilityContentForLineNumber: i as isize];
                    let (a, b) = makepad_a11y::mirror::text::line_range(t, i);
                    check.test(
                        string(content) == t.content.get(a as usize..b as usize).unwrap_or(""),
                        id,
                        "reading line",
                    );
                    let rect: NSRect = msg_send![obj, accessibilityFrameForLineNumber: i as isize];
                    let local: NSRect = msg_send![v, convertRect: rect fromCoordinateSpace: space];
                    check.test(
                        near(local.origin.x - bounds.origin.x, line.rect.x as f64)
                            && near(local.origin.y - bounds.origin.y, line.rect.y as f64)
                            && near(local.size.width, line.rect.w as f64)
                            && near(local.size.height, line.rect.h as f64),
                        id,
                        "reading line frame",
                    );
                    if !line.rect.is_empty() {
                        let point = NSPoint {
                            x: bounds.origin.x + line.rect.x as f64,
                            y: bounds.origin.y + line.rect.y as f64,
                        };
                        let actual: isize = msg_send![obj, accessibilityLineNumberForPoint: point];
                        check.test(actual == i as isize, id, "reading line point");
                    }
                }
                let missing: ObjcId = msg_send![obj, accessibilityContentForLineNumber: -1isize];
                check.test(missing == nil, id, "invalid reading line");
            }
            check.samples.push(format!(
                "#{} {} {:?} traits={traits:#x}",
                id.short(),
                n.role.short(),
                n.name
            ));
            let children = object_children(&mut check, obj, id);
            let ids: Vec<_> = children
                .iter()
                .map(|o| NodeId(*(**o).get_ivar::<u64>("node")))
                .collect();
            check.test(ids == children_of(id), id, "children");
            stack.extend(children.into_iter().rev().map(|child| (child, obj)));
        }
        check.test(seen == expected, tree.root, "reachable nodes");
        check.test(
            hal::with_tree(key, |t| t.updates == tree.updates).unwrap_or(false),
            tree.root,
            "tree changed during check; retry on settled UI",
        );
        if act && !cancelled.load(std::sync::atomic::Ordering::Acquire) {
            if pressable == nil {
                check.test(false, tree.root, "no activatable element");
            } else {
                CHECK_REQUEST.with(|c| c.set(None));
                CHECK_ACTION.with(|c| c.set(true));
                let accepted: BOOL = msg_send![pressable, accessibilityActivate];
                CHECK_ACTION.with(|c| c.set(false));
                check.request = CHECK_REQUEST.with(Cell::take);
                check.test(
                    accepted != NO && check.request.is_some(),
                    tree.root,
                    "activation rejected",
                );
            }
        }
    }
    check
}

pub(super) fn oscheck(key: u64, act: bool) -> String {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    // Only the remote consumer waits. The UIKit block and its action callbacks never wait.
    let main: BOOL = unsafe { msg_send![class!(NSThread), isMainThread] };
    if main != NO {
        return "oscheck error: invoke from a remote thread".into();
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let main_cancelled = cancelled.clone();
    app::run_on_main(move || {
        if main_cancelled.load(Ordering::Acquire) {
            return;
        }
        let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };
        let start = std::time::Instant::now();
        let mut check = shielded(
            "oscheck",
            || {
                let mut c = Check::default();
                c.test(false, NodeId::NONE, "callback panic");
                c
            },
            || check_main(key, act, &main_cancelled),
        );
        check.elapsed_us = start.elapsed().as_micros();
        unsafe {
            let _: () = msg_send![pool, release];
        }
        if let Err(error) = tx.try_send(check) {
            let check = match error {
                std::sync::mpsc::TrySendError::Full(c)
                | std::sync::mpsc::TrySendError::Disconnected(c) => c,
            };
            if let Some(req) = check.request {
                hal::forget_action(req);
            }
        }
    });
    let mut check = match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(c) => c,
        Err(_) => {
            cancelled.store(true, Ordering::Release);
            return "oscheck error: main-thread timeout".into();
        }
    };
    let mut action = String::new();
    if let Some(req) = check.request {
        let result =
            hal::wait_done(req, Duration::from_secs(5)).map(makepad_a11y::ActionResult::from_u8);
        check.test(
            matches!(
                result,
                Some(makepad_a11y::ActionResult::Handled | makepad_a11y::ActionResult::Fallback)
            ),
            NodeId::NONE,
            "activation completion",
        );
        action = format!(
            "action activate accepted=1 result={}\n",
            result.map(|r| r.short()).unwrap_or("timeout")
        );
        hal::forget_action(req);
    }
    check.report() + &action
}
