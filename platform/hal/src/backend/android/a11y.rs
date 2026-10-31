//! Android's virtual view tree. JNI reads copy a compact record out of the mirror; no Java or
//! HAL call runs under the mirror/identity locks. Android focus is separate from keyboard focus.
use super::{jhelp, shell::{lock, Shared}};
use crate::a11y;
use makepad_a11y::{consumer, mirror::{text, Tree}, ActionArg, ActionRequest, Actions, Node, NodeId, Notification, Role, States, Toggle};
use makepad_jni_sys::{registration, *};
use std::{collections::HashMap, ffi::c_void, sync::{mpsc::{self, RecvTimeoutError, SyncSender, TrySendError}, Mutex, OnceLock}, time::{Duration, Instant}};

const HOST: i32 = -1;
const NONE: i32 = i32::MIN;
const CUSTOM: i32 = 0x7f010000;
const HEADER: usize = 28;
const STRINGS: usize = 6;

#[derive(Default)]
struct Ids {
    window: Option<u64>,
    next: i32,
    forward: HashMap<NodeId, i32>,
    reverse: HashMap<i32, NodeId>,
    focus: NodeId,
}
impl Ids {
    fn window(&mut self, key: u64) {
        if self.window != Some(key) {
            self.window = Some(key);
            self.forward.clear();
            self.reverse.clear();
            self.focus = NodeId::NONE;
        }
    }
    fn virtual_id(&mut self, tree: &Tree, node: NodeId) -> i32 {
        if node.is_none() { return NONE; }
        if node == tree.root { return HOST; }
        if tree.node(node).is_none() { return NONE; }
        if let Some(id) = self.forward.get(&node) { return *id; }
        // The finite Android ID space is an OS constraint, not a tree size limit. Never recycle
        // a removed node's ID into a different node while Android might still cache it.
        let Some(next) = self.next.checked_add(1) else {
            eprintln!("[a11y-android] Android virtual ID space exhausted");
            return NONE;
        };
        self.next = next;
        self.forward.insert(node, next);
        self.reverse.insert(next, node);
        next
    }
    fn node_id(&self, tree: &Tree, id: i32) -> Option<NodeId> {
        if id == HOST { Some(tree.root) } else { self.reverse.get(&id).copied() }
    }
}
fn ids() -> &'static Mutex<Ids> {
    static IDS: OnceLock<Mutex<Ids>> = OnceLock::new();
    IDS.get_or_init(|| Mutex::new(Ids::default()))
}
fn window() -> Option<u64> {
    lock(&Shared::global().state).recipient.map(|(sid, id)| a11y::window_key(sid, id.index))
}

#[derive(Debug)]
struct Record {
    ints: Vec<i32>,
    strings: Vec<String>,
}

fn text_value(n: &Node) -> &str {
    n.text.as_ref().map_or_else(|| {
        if !n.value.is_empty() { n.value.as_str() } else { n.name.as_str() }
    }, |t| t.content.as_str())
}
fn description(n: &Node) -> String {
    // Text fields keep their label separate from their editable contents.
    let label = if n.text.is_some() || !n.value.is_empty() { n.name.as_str() } else { "" };
    match (label.is_empty(), n.description.is_empty()) {
        (false, false) => format!("{label}, {}", n.description),
        (false, true) => label.into(),
        _ => n.description.clone(),
    }
}
fn state_description(n: &Node) -> String {
    let mut parts = Vec::new();
    if n.toggle == Toggle::Mixed { parts.push("mixed"); }
    if n.states.contains(States::EXPANDED) { parts.push("expanded"); }
    if n.states.contains(States::COLLAPSED) { parts.push("collapsed"); }
    if n.states.contains(States::BUSY) { parts.push("busy"); }
    if n.states.contains(States::REQUIRED) { parts.push("required"); }
    if n.states.contains(States::INVALID) { parts.push("invalid"); }
    parts.join(", ")
}
fn available_actions(n: &Node, focused: bool) -> Vec<(i32, String)> {
    let mut out = vec![(if focused { 128 } else { 64 }, String::new())];
    if n.states.contains(States::DISABLED) { return out; }
    let mut add = |id, supported| { if supported { out.push((id, String::new())); } };
    add(1, n.actions.contains(Actions::FOCUS));
    add(16, n.actions.intersects(Actions::PRESS | Actions::TOGGLE));
    add(4, n.actions.intersects(Actions::SELECT | Actions::ADD_SELECTION));
    add(8, n.actions.contains(Actions::REMOVE_SELECTION));
    add(4096, n.actions.intersects(Actions::SCROLL | Actions::INCREMENT));
    add(8192, n.actions.intersects(Actions::SCROLL | Actions::DECREMENT));
    add(131072, n.actions.contains(Actions::SET_TEXT_SELECTION));
    add(262144, n.actions.contains(Actions::EXPAND));
    add(524288, n.actions.contains(Actions::COLLAPSE));
    add(1048576, n.actions.contains(Actions::DISMISS));
    add(2097152, n.actions.contains(Actions::REPLACE_TEXT) || (n.range.is_none() && n.actions.contains(Actions::SET_VALUE)));
    add(16908342, n.actions.contains(Actions::SCROLL_INTO_VIEW));
    add(16908348, n.actions.contains(Actions::SHOW_MENU));
    add(16908349, n.range.is_some() && n.actions.contains(Actions::SET_VALUE));
    if n.actions.contains(Actions::CUSTOM) {
        for (i, name) in n.custom_actions.iter().enumerate() {
            if let Some(id) = i32::try_from(i).ok().and_then(|i| CUSTOM.checked_add(i)) {
                out.push((id, name.clone()));
            }
        }
    }
    out
}
fn pixel_bounds(n: &Node, dpi: f32) -> [i32; 4] {
    [(n.bounds.x * dpi).floor() as i32, (n.bounds.y * dpi).floor() as i32,
        ((n.bounds.x + n.bounds.w) * dpi).ceil() as i32, ((n.bounds.y + n.bounds.h) * dpi).ceil() as i32]
}

/// Object[]{int[], String[]} v1. Int header: version; parent; parent and surface rects; flags;
/// selection; range type/min/max/now (float bits); collection rows/cols/hierarchical/selection;
/// item row/span/col/span; live; child count; action count. Tail: children, then Android action IDs.
/// Strings: class, text, description, hint, state description, resource path; then action labels.
/// Java adds the live SurfaceView screen offset, so rotations/insets do not stale cached bounds.
fn record(key: u64, virtual_id: i32, dpi: f32) -> Option<Record> {
    a11y::with_tree(key, |tree| {
        let mut ids = lock(ids());
        ids.window(key);
        let id = ids.node_id(tree, virtual_id)?;
        let n = tree.node(id)?;
        let parent = tree.parent(id).and_then(|p| tree.node(p));
        let parent_id = parent.map_or(HOST, |p| ids.virtual_id(tree, p.id));
        let rect = pixel_bounds(n, dpi);
        let origin = parent.filter(|p| p.id != tree.root).map_or([0, 0], |p| {
            let r = pixel_bounds(p, dpi); [r[0], r[1]]
        });
        let focused = tree.focus == id;
        let a11y_focused = ids.focus == id;
        let flags = [
            !n.states.contains(States::DISABLED),
            !n.states.intersects(States::HIDDEN | States::OFFSCREEN) && !n.bounds.is_empty(),
            n.states.contains(States::FOCUSABLE), focused, a11y_focused,
            n.toggle != Toggle::None, n.toggle == Toggle::On,
            n.states.contains(States::SELECTED), n.states.contains(States::PROTECTED),
            n.states.contains(States::EDITABLE) && !n.states.contains(States::READ_ONLY),
            n.actions.contains(Actions::SCROLL), n.states.contains(States::MULTILINE),
            matches!(n.role, Role::Heading | Role::ColumnHeader | Role::RowHeader),
            !n.name.is_empty() || !n.actions.is_empty() || n.text.is_some(),
        ].into_iter().enumerate().fold(0, |v, (i, b)| v | ((b as i32) << i));
        let selection = n.text.as_ref().map_or([-1, -1], |t| [
            text::byte_to_utf16(&t.content, t.selection.0) as i32,
            text::byte_to_utf16(&t.content, t.selection.1) as i32,
        ]);
        let range = n.range.map_or([-1, 0, 0, 0], |r| [1, (r.min as f32).to_bits() as i32,
            (r.max as f32).to_bits() as i32, (r.now as f32).to_bits() as i32]);
        let collection = matches!(n.role, Role::List | Role::ListBox | Role::Tree | Role::Grid | Role::Table | Role::TabList);
        let item = matches!(n.role, Role::ListItem | Role::TreeItem | Role::Row | Role::Cell | Role::ColumnHeader | Role::RowHeader | Role::Option | Role::Tab);
        let rows = if collection { n.table.map_or(n.set_size.max(n.children.len() as u32), |t| t.rows) as i32 } else { -1 };
        let cols = n.table.map_or(1, |t| t.cols.max(1)) as i32;
        let row = if item { n.table.map_or(n.pos_in_set.saturating_sub(1), |t| t.row) as i32 } else { -1 };
        let children: Vec<_> = tree.children(id).filter(|c| !c.states.contains(States::HIDDEN))
            .map(|c| ids.virtual_id(tree, c.id)).collect();
        let actions = available_actions(n, a11y_focused);
        let mut ints = vec![1, parent_id, rect[0] - origin[0], rect[1] - origin[1], rect[2] - origin[0], rect[3] - origin[1],
            rect[0], rect[1], rect[2], rect[3], flags, selection[0], selection[1],
            range[0], range[1], range[2], range[3], rows, cols,
            matches!(n.role, Role::Tree) as i32,
            if n.states.contains(States::MULTISELECTABLE) { 2 } else if n.actions.intersects(Actions::SELECT | Actions::ADD_SELECTION) || matches!(n.role, Role::ListBox | Role::TabList) { 1 } else { 0 },
            row, n.table.map_or(1, |t| t.row_span.max(1)) as i32,
            n.table.map_or(0, |t| t.col) as i32, n.table.map_or(1, |t| t.col_span.max(1)) as i32,
            n.live as i32, children.len() as i32, actions.len() as i32];
        ints.extend(children);
        let mut strings = vec![n.role.android_class().into(), text_value(n).into(), description(n),
            n.placeholder.clone(), state_description(n), n.path.clone()];
        for (id, label) in actions { ints.push(id); strings.push(label); }
        Some(Record { ints, strings })
    }).flatten()
}

unsafe fn java_string(env: *mut JNIEnv, value: &str) -> jstring {
    let utf16: Vec<u16> = value.encode_utf16().collect();
    ((**env).NewString.unwrap())(env, utf16.as_ptr(), utf16.len() as i32)
}
unsafe fn java_record(env: *mut JNIEnv, record: Record) -> jobjectArray {
    // Local frame size is only a JNI reference reservation; strings are released in the loop.
    if ((**env).PushLocalFrame.unwrap())(env, 8) != 0 { return std::ptr::null_mut(); }
    let out = (|| {
        let objects = ((**env).FindClass.unwrap())(env, c"java/lang/Object".as_ptr());
        let strings_class = ((**env).FindClass.unwrap())(env, c"java/lang/String".as_ptr());
        if objects.is_null() || strings_class.is_null() { return std::ptr::null_mut(); }
        let out = ((**env).NewObjectArray.unwrap())(env, 2, objects, std::ptr::null_mut());
        let ints = ((**env).NewIntArray.unwrap())(env, record.ints.len() as i32);
        let strings = ((**env).NewObjectArray.unwrap())(env, record.strings.len() as i32, strings_class, std::ptr::null_mut());
        if out.is_null() || ints.is_null() || strings.is_null() { return std::ptr::null_mut(); }
        ((**env).SetIntArrayRegion.unwrap())(env, ints, 0, record.ints.len() as i32, record.ints.as_ptr());
        for (i, s) in record.strings.iter().enumerate() {
            let s = java_string(env, s);
            if s.is_null() { return std::ptr::null_mut(); }
            ((**env).SetObjectArrayElement.unwrap())(env, strings, i as i32, s);
            ((**env).DeleteLocalRef.unwrap())(env, s);
            if ((**env).ExceptionCheck.unwrap())(env) != 0 { return std::ptr::null_mut(); }
        }
        ((**env).SetObjectArrayElement.unwrap())(env, out, 0, ints);
        ((**env).SetObjectArrayElement.unwrap())(env, out, 1, strings);
        out
    })();
    ((**env).PopLocalFrame.unwrap())(env, out)
}
unsafe extern "system" fn node(env: *mut JNIEnv, _: jclass, id: jint) -> jobjectArray {
    window().and_then(|key| record(key, id, Shared::global().dpi()))
        .map_or(std::ptr::null_mut(), |r| java_record(env, r))
}

fn action_request(n: &Node, action: i32, value: &str, start: i32, end: i32, number: f32) -> Option<ActionRequest> {
    if n.states.intersects(States::HIDDEN | States::DISABLED) { return None; }
    if !available_actions(n, false).iter().any(|(id, _)| *id == action) { return None; }
    let mut arg = ActionArg::None;
    let action = match action {
        1 => Actions::FOCUS,
        16 => if n.actions.contains(Actions::PRESS) { Actions::PRESS } else { Actions::TOGGLE },
        4 => if n.actions.contains(Actions::SELECT) { Actions::SELECT } else { Actions::ADD_SELECTION },
        8 => Actions::REMOVE_SELECTION,
        4096 | 8192 => {
            if n.actions.contains(Actions::SCROLL) {
                let direction = if action == 4096 { 1. } else { -1. };
                arg = if n.states.contains(States::HORIZONTAL) { ActionArg::Scroll(n.bounds.w * direction, 0.) }
                    else { ActionArg::Scroll(0., n.bounds.h * direction) };
                Actions::SCROLL
            } else if action == 4096 { Actions::INCREMENT } else { Actions::DECREMENT }
        }
        131072 => {
            let t = n.text.as_ref()?;
            let length = t.content.encode_utf16().count() as i32;
            if start < 0 || end < 0 || start > length || end > length { return None; }
            arg = ActionArg::Range(text::utf16_to_byte(&t.content, start as u32), text::utf16_to_byte(&t.content, end as u32));
            Actions::SET_TEXT_SELECTION
        }
        262144 => Actions::EXPAND,
        524288 => Actions::COLLAPSE,
        1048576 => Actions::DISMISS,
        2097152 => {
            if n.actions.contains(Actions::REPLACE_TEXT) {
                arg = ActionArg::Replace(0, n.text.as_ref()?.content.len() as u32, value.into());
                Actions::REPLACE_TEXT
            } else { arg = ActionArg::Text(value.into()); Actions::SET_VALUE }
        }
        16908342 => Actions::SCROLL_INTO_VIEW,
        16908348 => Actions::SHOW_MENU,
        16908349 => { if !number.is_finite() { return None; } arg = ActionArg::Number(number as f64); Actions::SET_VALUE }
        id if id >= CUSTOM => {
            arg = ActionArg::Custom(n.custom_actions.get((id - CUSTOM) as usize)?.clone());
            Actions::CUSTOM
        }
        _ => return None,
    };
    Some(ActionRequest { req: 0, node: n.id, action, arg })
}
unsafe extern "system" fn action(env: *mut JNIEnv, _: jclass, id: jint, action: jint, value: jstring, start: jint, end: jint, number: jfloat, wait: jboolean) -> jlong {
    let Some(key) = window() else { return 0; };
    let value = if value.is_null() { String::new() } else { registration::string(env, value) };
    let result = a11y::with_tree(key, |tree| {
        let mut ids = lock(ids());
        ids.window(key);
        let node_id = ids.node_id(tree, id)?;
        let n = tree.node(node_id)?;
        if action == 64 || action == 128 {
            if action == 64 {
                if n.states.intersects(States::HIDDEN | States::OFFSCREEN) || ids.focus == node_id { return None; }
                ids.focus = node_id;
            } else {
                if ids.focus != node_id { return None; }
                ids.focus = NodeId::NONE;
            }
            return Some(None);
        }
        action_request(n, action, &value, start, end, number).map(Some)
    }).flatten();
    match result {
        Some(Some(req)) => {
            let req = a11y::inject_action(key, req);
            if wait == 0 {
                let out = outbox();
                lock(&out.requests).push((key, req));
                out.signal();
            }
            req as i64
        }
        Some(None) => -1, // Android accessibility focus changes only the provider, never key focus.
        None => 0,
    }
}
unsafe extern "system" fn hit(_: *mut JNIEnv, _: jclass, x: jfloat, y: jfloat) -> jint {
    let dpi = Shared::global().dpi();
    window().and_then(|key| a11y::with_tree(key, |tree| {
        let mut ids = lock(ids()); ids.window(key);
        tree.hit_test(x / dpi, y / dpi).filter(|id| tree.node(*id).is_some_and(|n| !n.states.contains(States::OFFSCREEN)))
            .map_or(NONE, |id| ids.virtual_id(tree, id))
    })).unwrap_or(NONE)
}
unsafe extern "system" fn find_focus(_: *mut JNIEnv, _: jclass, focus: jint) -> jint {
    window().and_then(|key| a11y::with_tree(key, |tree| {
        let mut ids = lock(ids()); ids.window(key);
        let id = match focus { 1 => tree.focus, 2 => ids.focus, _ => return NONE };
        ids.virtual_id(tree, id)
    })).unwrap_or(NONE)
}
unsafe extern "system" fn state(_: *mut JNIEnv, _: jclass, enabled: jboolean, _: jboolean) {
    a11y::set_os_selfcheck(self_check);
    a11y::request_consumer(consumer::OS, enabled != 0);
    if enabled == 0 { lock(ids()).focus = NodeId::NONE; }
}

// The only native ABI table. It generates both registration entries and a two-way reflection
// check (including absent declarations, which ordinary RegisterNatives would silently miss).
macro_rules! natives {
    ($($name:literal, $sig:literal => $function:ident;)*) => {
        pub(super) fn native_methods() -> Vec<(&'static str, &'static str, *mut c_void)> {
            vec![$(($name, $sig, $function as *mut c_void)),*]
        }
        pub(super) fn check_natives(reflected: &str) -> Result<(), String> {
            let expected = [$(($name, $sig)),*];
            for (name, signature) in expected {
                let found: Vec<_> = reflected.lines().filter_map(|l| l.split_once(':')).filter(|(n, _)| *n == name).collect();
                if found.len() != 1 || found[0].1 != signature {
                    return Err(format!("MakepadNative.{name}: expected exactly {signature}, reflected {found:?}"));
                }
            }
            for (name, signature) in reflected.lines().filter_map(|l| l.split_once(':')) {
                if name.starts_with("a11y") && !expected.contains(&(name, signature)) {
                    return Err(format!("unexpected accessibility native {name}:{signature}"));
                }
            }
            Ok(())
        }
    };
}
natives! {
    "a11yNode", "(I)[Ljava/lang/Object;" => node;
    "a11yAction", "(IILjava/lang/String;IIFZ)J" => action;
    "a11yHitTest", "(FF)I" => hit;
    "a11yFindFocus", "(I)I" => find_focus;
    "a11yState", "(ZZ)V" => state;
}

struct Outbox {
    pending: Mutex<Vec<(u64, Vec<Notification>)>>,
    requests: Mutex<Vec<(u64, u64)>>,
    wake: SyncSender<()>,
}
impl Outbox {
    fn signal(&self) {
        match self.wake.try_send(()) {
            Ok(()) => {}
            Err(TrySendError::Full(())) => eprintln!("[a11y-android] wake pending; retaining work"),
            Err(TrySendError::Disconnected(())) => eprintln!("[a11y-android] worker unavailable; retaining work"),
        }
    }
}
fn outbox() -> &'static Outbox {
    static OUTBOX: OnceLock<Outbox> = OnceLock::new();
    OUTBOX.get_or_init(|| {
        // One wake token covers all retained batches. A full signal queue never drops an event.
        let (wake, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new().name("android-a11y".into()).spawn(move || {
            let mut requests = Vec::new();
            loop {
                // Completed ordinary actions must be consumed too. Poll only while one is in
                // flight; a stalled app retains its requests and never delays notification delivery.
                if requests.is_empty() {
                    if rx.recv().is_err() { break; }
                } else if matches!(rx.recv_timeout(Duration::from_millis(100)), Err(RecvTimeoutError::Disconnected)) { break; }
                let batches = std::mem::take(&mut *lock(&outbox().pending));
                let added_requests = std::mem::take(&mut *lock(&outbox().requests));
                requests.extend(added_requests);
                for (key, events) in batches { send_notifications(key, events); }
                requests.retain(|(key, req)| {
                    match a11y::wait_done(*req, Duration::ZERO) {
                        Some(result) => {
                            if result > 1 { eprintln!("[a11y-android] action {req}: {}", makepad_a11y::ActionResult::from_u8(result).short()); }
                            false
                        }
                        None => a11y::with_tree(*key, |_| ()).is_some(),
                    }
                });
            }
        }).expect("Android accessibility notification worker");
        Outbox { pending: Mutex::new(Vec::new()), requests: Mutex::new(Vec::new()), wake }
    })
}
/// Called under the HAL world lock. Only copies the diff and signals a persistent worker.
pub(crate) fn notify(key: u64, events: &[Notification]) {
    if a11y::consumers() & consumer::OS == 0 { return; }
    let out = outbox();
    let batch = (key, events.to_vec());
    lock(&out.pending).push(batch);
    out.signal();
}
fn send_notifications(key: u64, events: Vec<Notification>) {
    if window() != Some(key) { return; }
    let Some(events) = a11y::with_tree(key, |tree| {
        let mut ids = lock(ids()); ids.window(key);
        // Reclaim removed identities on structural changes; never reuse the numeric IDs.
        if events.iter().any(|e| matches!(e, Notification::ChildrenChanged(_))) {
            ids.forward.retain(|id, _| tree.node(*id).is_some());
            ids.reverse.retain(|_, id| tree.node(*id).is_some());
        }
        if tree.node(ids.focus).is_none_or(|n| n.states.intersects(States::HIDDEN | States::OFFSCREEN)) { ids.focus = NodeId::NONE; }
        let mut out = Vec::new();
        // Even an initial empty notification list must invalidate Android's cached host children.
        if events.is_empty() { out.push((HOST, 2048, 1, String::new())); }
        for e in events {
            let (node, kind, change, text) = match e {
                Notification::FocusChanged(n) => (n, 8, 0, String::new()),
                Notification::ChildrenChanged(n) => (n, 2048, 1, String::new()),
                Notification::NameChanged(n) => {
                    let change = if tree.node(n).is_some_and(|n| n.text.is_none() && n.value.is_empty()) { 2 } else { 4 };
                    (n, 2048, change, String::new())
                }
                Notification::ValueChanged(n) | Notification::TextChanged { node: n, .. } => (n, 2048, 2, String::new()),
                Notification::StatesChanged { node: n, .. } => (n, 2048, 64, String::new()),
                Notification::CaretMoved(n) => (n, 8192, 0, String::new()),
                Notification::LiveRegionChanged(n) => (n, 2048, 2, String::new()),
                Notification::Announce { text, .. } => (tree.root, 16384, 0, text),
            };
            let id = ids.virtual_id(tree, node);
            out.push((if id == NONE { HOST } else { id }, kind, change, text));
        }
        out
    }) else { return; };
    // JNI work happens here, outside both locks, and Java posts to its main looper.
    if let Err(error) = jhelp::activity(|env, activity| unsafe {
        let method = jhelp::method(env, activity, c"a11yEvent", c"(IIILjava/lang/String;)V")?;
        for (id, kind, change, text) in events {
            let text = java_string(env, &text);
            jhelp::check(env)?;
            ((**env).CallVoidMethodA.unwrap())(env, activity, method, [jvalue { i: id }, jvalue { i: kind }, jvalue { i: change }, jvalue { l: text }].as_ptr());
            ((**env).DeleteLocalRef.unwrap())(env, text);
            jhelp::check(env)?;
        }
        Ok(())
    }) { eprintln!("[a11y-android] notification: {error}"); }
}

fn self_check(key: u64, act: bool) -> String {
    let started = Instant::now();
    if window() != Some(key) { return "error: window is not the Android surface's owner".into(); }
    let Some(tree) = a11y::with_tree(key, Clone::clone) else { return "error: no mirror".into(); };
    if tree.nodes.is_empty() { return "error: empty mirror; retry after activation".into(); }
    let dpi = Shared::global().dpi();
    let mut stack = vec![(HOST, tree.root)];
    let mut seen = std::collections::HashSet::new();
    let mut errors = Vec::new();
    let mut action_target = None;
    while let Some((virtual_id, node)) = stack.pop() {
        if !seen.insert(node) { errors.push(format!("cycle/duplicate #{}", node.short())); continue; }
        let Some(n) = tree.node(node) else { errors.push(format!("missing #{}", node.short())); continue; };
        // This is the actual native entry point's encoder, not a second adapter model.
        let Some(r) = record(key, virtual_id, dpi) else { errors.push(format!("no record #{}", node.short())); continue; };
        let children: Vec<_> = tree.children(node).filter(|c| !c.states.contains(States::HIDDEN)).collect();
        let a = &r.ints;
        let s = &r.strings;
        if a.len() < HEADER || a[0] != 1 || a[26] as usize != children.len() || a.len() != HEADER + a[26] as usize + a[27] as usize || s.len() != STRINGS + a[27] as usize {
            errors.push(format!("record shape #{}", node.short())); continue;
        }
        let b = n.bounds;
        let expected_bounds = [(b.x * dpi).floor() as i32, (b.y * dpi).floor() as i32, ((b.x + b.w) * dpi).ceil() as i32, ((b.y + b.h) * dpi).ceil() as i32];
        if a[6..10] != expected_bounds || s[0] != n.role.android_class() || s[1] != text_value(n) || s[2] != description(n) || s[3] != n.placeholder || s[4] != state_description(n) {
            errors.push(format!("bounds/strings #{}", node.short()));
        }
        let flags = a[10];
        if (flags & 1 != 0) == n.states.contains(States::DISABLED) || (flags & 32 != 0) != (n.toggle != Toggle::None) || (flags & 64 != 0) != (n.toggle == Toggle::On) || (flags & 8 != 0) != (tree.focus == node) {
            errors.push(format!("states #{}", node.short()));
        }
        if let Some(t) = &n.text {
            if a[11] != text::byte_to_utf16(&t.content, t.selection.0) as i32 || a[12] != text::byte_to_utf16(&t.content, t.selection.1) as i32 { errors.push(format!("selection #{}", node.short())); }
        }
        if let Some(range) = n.range {
            if a[13] != 1 || f32::from_bits(a[14] as u32) != range.min as f32 || f32::from_bits(a[15] as u32) != range.max as f32 || f32::from_bits(a[16] as u32) != range.now as f32 { errors.push(format!("range #{}", node.short())); }
        }
        let map = lock(ids());
        if node != tree.root && map.node_id(&tree, a[1]) != tree.parent(node) { errors.push(format!("parent #{}", node.short())); }
        for (i, c) in children.iter().enumerate() {
            if map.node_id(&tree, a[HEADER + i]) != Some(c.id) { errors.push(format!("child #{}", c.id.short())); }
            stack.push((a[HEADER + i], c.id));
        }
        drop(map);
        if action_target.is_none() && a[HEADER + children.len()..].contains(&16) { action_target = Some(virtual_id); }
    }
    if a11y::with_tree(key, |t| t.updates) != Some(tree.updates) { errors.push("mirror changed during walk; retry".into()); }
    let java = jhelp::activity(|env, activity| unsafe {
        let method = jhelp::method(env, activity, c"a11ySelfCheck", c"()Ljava/lang/String;")?;
        let value = ((**env).CallObjectMethodA.unwrap())(env, activity, method, std::ptr::null());
        jhelp::check(env)?;
        Ok(registration::string(env, value))
    });
    let mut out = if errors.is_empty() { format!("ok: Android native records nodes={} elapsed_us={}", seen.len(), started.elapsed().as_micros()) }
        else { format!("error: {}\nAndroid records nodes={}", errors.join("\n"), seen.len()) };
    let java_ok = java.as_ref().is_ok_and(|result| result.starts_with("ok:"));
    match java { Ok(result) => { out.push('\n'); out.push_str(&result); }, Err(error) => out.push_str(&format!("\nerror: Java provider check: {error}")) }
    if act {
        if !errors.is_empty() || !java_ok { out.push_str("\naction: skipped because the provider walk failed"); return out; }
        match action_target {
            Some(id) => {
                // Java marshals this provider call to the UI thread; only this remote check waits.
                let req = jhelp::activity(|env, activity| unsafe {
                    let method = jhelp::method(env, activity, c"a11ySelfCheckAction", c"(II)J")?;
                    let req = ((**env).CallLongMethodA.unwrap())(env, activity, method, [jvalue { i: id }, jvalue { i: 16 }].as_ptr());
                    jhelp::check(env)?; Ok(req)
                });
                match req {
                    Ok(req) if req > 0 => {
                        match a11y::wait_done(req as u64, Duration::from_secs(5)) {
                            Some(result) if result <= 1 => out.push_str(&format!("\naction: provider ACTION_CLICK id={id} req={req} result={}", makepad_a11y::ActionResult::from_u8(result).short())),
                            Some(result) => out.push_str(&format!("\nerror: provider click result={}", makepad_a11y::ActionResult::from_u8(result).short())),
                            None => {
                                out.push_str(&format!("\nerror: provider click req={req} completion pending"));
                                let pending = outbox();
                                lock(&pending.requests).push((key, req as u64));
                                pending.signal();
                            }
                        }
                    }
                    other => out.push_str(&format!("\nerror: provider click {other:?}")),
                }
            }
            None => out.push_str("\naction: no clickable node"),
        }
    }
    out
}
