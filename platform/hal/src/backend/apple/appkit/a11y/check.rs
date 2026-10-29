//! In-process AppKit protocol walk, called only by the remote OS self-check.
use super::*;
use std::{collections::HashSet, time::Duration};

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
            if self.errors.len() < 20 {
                self.errors.push(format!("#{} {field}", id.short()));
            }
        }
    }
    fn report(&self) -> String {
        let mut out = format!(
            "oscheck macos nodes={} text={} actions={} mismatches={} main_us={}\n",
            self.nodes, self.texts, self.actions, self.mismatches, self.elapsed_us
        );
        for line in self.errors.iter().chain(self.samples.iter()).take(20) {
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
fn same_range(a: NSRange, b: NSRange) -> bool {
    a.location == b.location && a.length == b.length
}
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 0.01
}
fn same_rect(a: NSRect, b: NSRect) -> bool {
    near(a.origin.x, b.origin.x)
        && near(a.origin.y, b.origin.y)
        && near(a.size.width, b.size.width)
        && near(a.size.height, b.size.height)
}
unsafe fn object_children(obj: ObjcId) -> Vec<ObjcId> {
    let array: ObjcId = msg_send![obj, accessibilityChildren];
    if array == nil {
        return Vec::new();
    }
    let count: usize = msg_send![array, count];
    (0..count)
        .map(|i| msg_send![array, objectAtIndex: i])
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
    let mut expected = HashSet::new();
    tree.walk(|n, _| {
        if n.id != tree.root {
            expected.insert(n.id);
        }
    });
    let mut seen = HashSet::new();
    let mut pressable = nil;
    unsafe {
        let is_element: BOOL = msg_send![v, isAccessibilityElement];
        let role: ObjcId = msg_send![v, accessibilityRole];
        check.test(
            is_element == NO && string(role) == "AXGroup",
            tree.root,
            "container",
        );
        let focus: ObjcId = msg_send![v, accessibilityFocusedUIElement];
        check.test(focus == element(key, tree.focus), tree.root, "focus");
        let roots = object_children(v);
        let root_ids: Vec<_> = roots
            .iter()
            .map(|obj| NodeId(*(**obj).get_ivar::<u64>("node")))
            .collect();
        check.test(
            tree.node(tree.root).is_some_and(|n| n.children == root_ids),
            tree.root,
            "children",
        );
        let mut stack: Vec<_> = roots.into_iter().rev().map(|obj| (obj, v)).collect();
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
            let role: ObjcId = msg_send![obj, accessibilityRole];
            let subrole: ObjcId = msg_send![obj, accessibilitySubrole];
            let label: ObjcId = msg_send![obj, accessibilityLabel];
            let help: ObjcId = msg_send![obj, accessibilityHelp];
            let placeholder: ObjcId = msg_send![obj, accessibilityPlaceholderValue];
            let parent: ObjcId = msg_send![obj, accessibilityParent];
            check.test(string(role) == n.role.ax_role().0, id, "role");
            check.test(string(subrole) == n.role.ax_role().1, id, "subrole");
            check.test(string(label) == n.name, id, "label");
            check.test(string(help) == n.description, id, "help");
            check.test(string(placeholder) == n.placeholder, id, "placeholder");
            check.test(parent == expected_parent, id, "parent");
            let value: ObjcId = msg_send![obj, accessibilityValue];
            if let Some(r) = n.range {
                let is_number: BOOL = msg_send![value, isKindOfClass: class!(NSNumber)];
                let min: ObjcId = msg_send![obj, accessibilityMinValue];
                let max: ObjcId = msg_send![obj, accessibilityMaxValue];
                check.test(
                    is_number != NO
                        && near(msg_send![value, doubleValue], r.now)
                        && near(msg_send![min, doubleValue], r.min)
                        && near(msg_send![max, doubleValue], r.max),
                    id,
                    "range value",
                );
            } else if n.toggle != Toggle::None {
                let is_number: BOOL = msg_send![value, isKindOfClass: class!(NSNumber)];
                check.test(
                    is_number != NO && near(msg_send![value, doubleValue], toggle_value(n.toggle)),
                    id,
                    "toggle value",
                );
            } else {
                check.test(string(value) == n.value, id, "value");
            }
            let rect: NSRect = msg_send![obj, accessibilityFrame];
            // Invert the screen conversion, checking the top-left and size in mirror coordinates.
            let top_left = local_point(
                key,
                NSPoint {
                    x: rect.origin.x,
                    y: rect.origin.y + rect.size.height,
                },
            );
            check.test(
                top_left.is_some_and(|(x, y)| {
                    near(x as f64, n.bounds.x as f64) && near(y as f64, n.bounds.y as f64)
                }) && near(rect.size.width, n.bounds.w as f64)
                    && near(rect.size.height, n.bounds.h as f64),
                id,
                "frame",
            );
            let enabled: BOOL = msg_send![obj, isAccessibilityEnabled];
            let focused: BOOL = msg_send![obj, isAccessibilityFocused];
            let selected: BOOL = msg_send![obj, isAccessibilitySelected];
            let expanded: BOOL = msg_send![obj, isAccessibilityExpanded];
            check.test(
                (enabled != NO) != n.states.contains(States::DISABLED),
                id,
                "enabled",
            );
            check.test((focused != NO) == (tree.focus == id), id, "focused");
            check.test(
                (selected != NO) == n.states.contains(States::SELECTED),
                id,
                "selected",
            );
            check.test(
                (expanded != NO) == n.states.contains(States::EXPANDED),
                id,
                "expanded",
            );
            if !n.bounds.is_empty() {
                let (x, y) = n.bounds.center();
                let point = NSPoint {
                    x: rect.origin.x + rect.size.width * 0.5,
                    y: rect.origin.y + rect.size.height * 0.5,
                };
                let hit: ObjcId = msg_send![v, accessibilityHitTest: point];
                check.test(
                    hit == tree.hit_test(x, y).map(|n| element(key, n)).unwrap_or(nil),
                    id,
                    "hit test",
                );
            }
            for (selector, action) in [
                (sel!(accessibilityPerformPress), press_action(n)),
                (sel!(accessibilityPerformIncrement), Actions::INCREMENT),
                (sel!(accessibilityPerformDecrement), Actions::DECREMENT),
                (sel!(accessibilityPerformShowMenu), Actions::SHOW_MENU),
                (sel!(accessibilityPerformCancel), Actions::DISMISS),
                (sel!(setAccessibilityValue:), Actions::SET_VALUE),
                (sel!(setAccessibilityFocused:), Actions::FOCUS),
                (
                    sel!(setAccessibilitySelectedTextRange:),
                    Actions::SET_TEXT_SELECTION,
                ),
            ] {
                let allowed: BOOL = msg_send![obj, isAccessibilitySelectorAllowed: selector];
                let expected = n.actions.contains(action) && !n.states.contains(States::DISABLED);
                check.test((allowed != NO) == expected, id, selector.name());
                if allowed != NO {
                    check.actions += 1;
                }
            }
            if pressable == nil
                && n.actions.contains(press_action(n))
                && !n.states.contains(States::DISABLED)
            {
                pressable = obj;
            }
            let custom: ObjcId = msg_send![obj, accessibilityCustomActions];
            let count: usize = msg_send![custom, count];
            check.test(count == n.custom_actions.len(), id, "custom actions count");
            for (i, expected) in n.custom_actions.iter().take(count).enumerate() {
                let action: ObjcId = msg_send![custom, objectAtIndex: i];
                let name: ObjcId = msg_send![action, name];
                check.test(string(name) == *expected, id, "custom action name");
            }
            if let Some(table) = n.table {
                let rows: isize = msg_send![obj, accessibilityRowCount];
                let cols: isize = msg_send![obj, accessibilityColumnCount];
                let range: NSRange = msg_send![obj, accessibilityRowIndexRange];
                check.test(
                    rows == table.rows as isize
                        && cols == table.cols as isize
                        && range.location == table.row as u64
                        && range.length == table.row_span.max(1) as u64,
                    id,
                    "table",
                );
            }
            if let Some(t) = &n.text {
                check.texts += 1;
                let count: isize = msg_send![obj, accessibilityNumberOfCharacters];
                let selection: NSRange = msg_send![obj, accessibilitySelectedTextRange];
                let selected: ObjcId = msg_send![obj, accessibilitySelectedText];
                let full: ObjcId = msg_send![obj, accessibilityStringForRange: NSRange { location: 0, length: t.content.encode_utf16().count() as u64 }];
                check.test(
                    count == t.content.encode_utf16().count() as isize,
                    id,
                    "text length",
                );
                check.test(
                    same_range(selection, utf16_range(t, t.selection.0, t.selection.1)),
                    id,
                    "text selection",
                );
                check.test(
                    string(selected)
                        == t.content
                            .get(
                                t.selection.0.min(t.selection.1) as usize
                                    ..t.selection.0.max(t.selection.1) as usize,
                            )
                            .unwrap_or(""),
                    id,
                    "selected text",
                );
                check.test(string(full) == t.content, id, "text content");
                let caret: isize = msg_send![obj, accessibilityInsertionPointLineNumber];
                check.test(
                    caret == text::line_for_byte(t, t.selection.1) as isize,
                    id,
                    "caret line",
                );
                let visible: NSRange = msg_send![obj, accessibilityVisibleCharacterRange];
                check.test(byte_range(t, &visible).is_some(), id, "visible range");
                for (line, lb) in t.lines.iter().enumerate() {
                    let (a, b) = text::line_range(t, line);
                    let range: NSRange = msg_send![obj, accessibilityRangeForLine: line as isize];
                    let index = text::byte_to_utf16(&t.content, lb.start) as isize;
                    let actual_line: isize = msg_send![obj, accessibilityLineForIndex: index];
                    let rect: NSRect =
                        msg_send![obj, accessibilityFrameForRange: utf16_range(t, a, b)];
                    let expected = union(&text::range_rects(t, a, b))
                        .map(|r| screen_rect(key, r))
                        .unwrap_or_else(zero_rect);
                    check.test(
                        same_range(range, utf16_range(t, a, b))
                            && actual_line == line as isize
                            && same_rect(rect, expected),
                        id,
                        "text line/range/frame",
                    );
                    let point = screen_rect(
                        key,
                        Rect {
                            x: lb.rect.x,
                            y: lb.rect.y,
                            w: 0.,
                            h: 0.,
                        },
                    )
                    .origin;
                    let position: NSRange = msg_send![obj, accessibilityRangeForPosition: point];
                    let expected_byte = text::byte_at_point(t, lb.rect.x, lb.rect.y);
                    check.test(
                        position.location == text::byte_to_utf16(&t.content, expected_byte) as u64,
                        id,
                        "text position",
                    );
                }
            }
            if check.samples.len() < 20 {
                check.samples.push(format!(
                    "#{} {} {:?}",
                    id.short(),
                    n.role.ax_role().0,
                    n.name
                ));
            }
            let children = object_children(obj);
            let ids: Vec<_> = children
                .iter()
                .map(|obj| NodeId(*(**obj).get_ivar::<u64>("node")))
                .collect();
            check.test(ids == n.children, id, "children");
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
                check.test(false, tree.root, "no pressable element");
            } else {
                CHECK_REQUEST.with(|c| c.set(None));
                CHECK_ACTION.with(|c| c.set(true));
                let accepted: BOOL = msg_send![pressable, accessibilityPerformPress];
                CHECK_ACTION.with(|c| c.set(false));
                check.request = CHECK_REQUEST.with(|c| c.take());
                check.test(
                    accepted != NO && check.request.is_some(),
                    tree.root,
                    "press rejected",
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
    // Only the remote consumer waits. The AppKit block and its action callbacks never wait.
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
            "press completion",
        );
        action = format!(
            "action press accepted=1 result={}\n",
            result.map(|r| r.short()).unwrap_or("timeout")
        );
        hal::forget_action(req);
    }
    check.report() + &action
}
