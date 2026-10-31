//! UIKit consumes the HAL mirror. Cache ownership and every UIKit call stay on the main
//! thread; mirror closures only copy data, since Objective-C calls may reenter the adapter.
use super::{app, classes::win_of};
use crate::a11y as hal;
use crate::backend::apple::objc_class::{objc_class, shielded};
use makepad_a11y::{
    consumer, ActionArg, ActionRequest, Actions, Live, Node, NodeId, Notification, Rect, Role,
    States, Toggle,
};
use makepad_apple_sys::{ui_accessibility::*, *};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
};

mod check;

thread_local! {
    // One owned retain per element. Arrays and AT clients can retain elements past removal;
    // their getters then return empty values and their actions are rejected.
    static ELEMENTS: RefCell<HashMap<u64, HashMap<NodeId, ObjcId>>> = RefCell::new(HashMap::new());
    static ELEMENT_CLASS: Cell<*const Class> = const { Cell::new(std::ptr::null()) };
    static QUERIED: Cell<bool> = const { Cell::new(false) };
    static FOCUS: RefCell<HashMap<u64, NodeId>> = RefCell::new(HashMap::new());
    static DIALOGS: RefCell<HashMap<u64, HashSet<NodeId>>> = RefCell::new(HashMap::new());
    static CHECK_ACTION: Cell<bool> = const { Cell::new(false) };
    static CHECK_REQUEST: Cell<Option<u64>> = const { Cell::new(None) };
}

fn activate() {
    if !QUERIED.with(|q| q.replace(true)) {
        hal::request_consumer(consumer::OS, true);
    }
}
fn status_changed(_: &Object, _: ObjcId) {
    // A status transition ends a query-only activation; either service still keeps OS on.
    QUERIED.with(|q| q.set(false));
    unsafe {
        hal::request_consumer(
            consumer::OS,
            UIAccessibilityIsVoiceOverRunning() != NO
                || UIAccessibilityIsSwitchControlRunning() != NO,
        );
    }
}
pub(super) fn init() {
    let mut decl = ClassDecl::new("HalA11yObserverUI", class!(NSObject)).unwrap();
    objc_class!(decl;
        (statusChanged:) => status_changed(note: ObjcId) -> () = ();
    );
    unsafe {
        // Process-lifetime observer, like UIKit's keyboard observer.
        let observer: ObjcId = msg_send![decl.register(), new];
        let center: ObjcId = msg_send![class!(NSNotificationCenter), defaultCenter];
        for name in [
            UIAccessibilityVoiceOverStatusDidChangeNotification,
            UIAccessibilitySwitchControlStatusDidChangeNotification,
        ] {
            let () = msg_send![center, addObserver: observer selector: sel!(statusChanged:) name: name object: nil];
        }
        if UIAccessibilityIsVoiceOverRunning() != NO
            || UIAccessibilityIsSwitchControlRunning() != NO
        {
            hal::request_consumer(consumer::OS, true);
        }
    }
}

fn key_of(this: &Object) -> (u64, NodeId) {
    unsafe { (*this.get_ivar("window_key"), NodeId(*this.get_ivar("node"))) }
}
fn read<R>(this: &Object, f: impl FnOnce(&Node) -> R) -> Option<R> {
    let (key, id) = key_of(this);
    hal::with_tree(key, |t| t.node(id).map(f)).flatten()
}
fn view(key: u64) -> ObjcId {
    app::app().a11y_view(key).unwrap_or(nil)
}
fn element(key: u64, id: NodeId) -> ObjcId {
    if id.is_none() || view(key) == nil {
        return nil;
    }
    match hal::with_tree(key, |t| (t.root, t.node(id).is_some())) {
        Some((root, true)) if root == id => return view(key),
        Some((_, true)) => (),
        _ => return nil,
    }
    if let Some(obj) = ELEMENTS.with(|c| c.borrow().get(&key).and_then(|c| c.get(&id)).copied()) {
        return obj;
    }
    let cls = ELEMENT_CLASS.with(|c| {
        if c.get().is_null() {
            c.set(define_element_class());
        }
        c.get()
    });
    let obj: ObjcId = unsafe {
        let obj: ObjcId = msg_send![cls, alloc];
        let obj: ObjcId = msg_send![obj, initWithAccessibilityContainer: view(key)];
        (*obj).set_ivar("window_key", key);
        (*obj).set_ivar("node", id.0);
        obj
    };
    ELEMENTS.with(|c| c.borrow_mut().entry(key).or_default().insert(id, obj));
    obj
}
fn array(objects: &[ObjcId]) -> ObjcId {
    unsafe { msg_send![class!(NSArray), arrayWithObjects: objects.as_ptr() count: objects.len()] }
}
fn child_ids(key: u64, id: NodeId) -> Vec<NodeId> {
    hal::with_tree(key, |t| t.node(id).map(|n| n.children.clone()))
        .flatten()
        .unwrap_or_default()
}
fn children_for(key: u64, id: NodeId) -> ObjcId {
    let objects: Vec<_> = child_ids(key, id)
        .into_iter()
        .map(|id| element(key, id))
        .filter(|o| *o != nil)
        .collect();
    array(&objects)
}
fn at_index(key: u64, id: NodeId, index: isize) -> ObjcId {
    if index < 0 {
        return nil;
    }
    hal::with_tree(key, |t| {
        t.node(id)
            .and_then(|n| n.children.get(index as usize).copied())
    })
    .flatten()
    .map(|id| element(key, id))
    .unwrap_or(nil)
}
fn index_of(key: u64, id: NodeId, obj: ObjcId) -> isize {
    let cls = ELEMENT_CLASS.with(Cell::get);
    if obj == nil || cls.is_null() {
        return isize::MAX;
    }
    let ours: BOOL = unsafe { msg_send![obj, isKindOfClass: cls] };
    if ours == NO {
        return isize::MAX;
    }
    let (object_key, node) = key_of(unsafe { &*obj });
    if object_key != key {
        return isize::MAX;
    }
    hal::with_tree(key, |t| {
        t.node(id)
            .and_then(|n| n.children.iter().position(|n| *n == node))
    })
    .flatten()
    .map(|i| i as isize)
    .unwrap_or(isize::MAX)
}
fn count_for(key: u64, id: NodeId) -> isize {
    hal::with_tree(key, |t| t.node(id).map(|n| n.children.len() as isize))
        .flatten()
        .unwrap_or(0)
}
fn view_root(this: &Object) -> Option<(u64, NodeId)> {
    activate();
    let w = win_of(this)?;
    let key = hal::window_key(w.sid.get(), w.id.get().index);
    Some((key, hal::with_tree(key, |t| t.root).unwrap_or(NodeId::NONE)))
}
fn view_is_element(this: &Object) -> BOOL {
    view_root(this);
    NO
}
// UIKit hosts additional HAL windows as subviews. Keep their containers reachable when
// overriding UIView's automatic traversal; the hidden UITextView is never included here.
fn child_views(this: &Object) -> Vec<ObjcId> {
    let this = this as *const Object as ObjcId;
    app::app()
        .live_windows()
        .into_iter()
        .filter_map(|w| unsafe {
            let parent: ObjcId = msg_send![w.view, superview];
            let hidden: BOOL = msg_send![w.view, isHidden];
            (parent == this && hidden == NO).then_some(w.view)
        })
        .collect()
}
fn view_children(this: &Object) -> ObjcId {
    let mut objects: Vec<_> = view_root(this)
        .map(|(k, n)| {
            child_ids(k, n)
                .into_iter()
                .map(|n| element(k, n))
                .filter(|o| *o != nil)
                .collect()
        })
        .unwrap_or_default();
    objects.extend(child_views(this));
    array(&objects)
}
fn view_count(this: &Object) -> isize {
    view_root(this).map(|(k, n)| count_for(k, n)).unwrap_or(0) + child_views(this).len() as isize
}
fn view_at(this: &Object, index: isize) -> ObjcId {
    if index < 0 {
        return nil;
    }
    let root = view_root(this);
    let count = root.map(|(k, n)| count_for(k, n)).unwrap_or(0);
    if index < count {
        return root.map(|(k, n)| at_index(k, n, index)).unwrap_or(nil);
    }
    child_views(this)
        .get((index - count) as usize)
        .copied()
        .unwrap_or(nil)
}
fn view_index(this: &Object, obj: ObjcId) -> isize {
    let root = view_root(this);
    let index = root.map(|(k, n)| index_of(k, n, obj)).unwrap_or(isize::MAX);
    if index != isize::MAX {
        return index;
    }
    child_views(this)
        .iter()
        .position(|o| *o == obj)
        .map(|i| i as isize + root.map(|(k, n)| count_for(k, n)).unwrap_or(0))
        .unwrap_or(isize::MAX)
}
pub(super) fn add_view_methods(decl: &mut ClassDecl) {
    hal::set_os_selfcheck(check::oscheck);
    objc_class!(decl;
        (isAccessibilityElement) => view_is_element() -> BOOL = NO;
        (accessibilityElements) => view_children() -> ObjcId = nil;
        (accessibilityElementCount) => view_count() -> isize = 0;
        (accessibilityElementAtIndex:) => view_at(index: isize) -> ObjcId = nil;
        (indexOfAccessibilityElement:) => view_index(obj: ObjcId) -> isize = isize::MAX;
    );
}
fn yes(b: bool) -> BOOL {
    if b {
        YES
    } else {
        NO
    }
}
fn zero_rect() -> NSRect {
    NSRect {
        origin: NSPoint { x: 0., y: 0. },
        size: NSSize {
            width: 0.,
            height: 0.,
        },
    }
}
fn screen_rect(key: u64, r: Rect) -> NSRect {
    let v = view(key);
    if v == nil {
        return zero_rect();
    }
    unsafe {
        let bounds: NSRect = msg_send![v, bounds];
        UIAccessibilityConvertFrameToScreenCoordinates(
            NSRect {
                origin: NSPoint {
                    x: bounds.origin.x + r.x as f64,
                    y: bounds.origin.y + r.y as f64,
                },
                size: NSSize {
                    width: r.w as f64,
                    height: r.h as f64,
                },
            },
            v,
        )
    }
}
fn exposed(n: &Node) -> bool {
    !n.states.contains(States::HIDDEN)
        && n.role != Role::TabList
        && (n.children.is_empty() || n.role.is_interactive() || !n.actions.is_empty())
}
fn is_element(this: &Object) -> BOOL {
    yes(read(this, exposed).unwrap_or(false))
}
fn label_text(n: &Node) -> String {
    if n.name.is_empty() && n.role.is_text_input() {
        n.placeholder.clone()
    } else {
        n.name.clone()
    }
}
fn value_text(n: &Node) -> String {
    if !n.value.is_empty() {
        n.value.clone()
    } else if let Some(r) = n.range {
        r.now.to_string()
    } else {
        n.toggle.short().to_owned()
    }
}
fn label(this: &Object) -> ObjcId {
    read(this, label_text)
        .map(|s| str_to_nsstring(&s))
        .unwrap_or(nil)
}
fn value(this: &Object) -> ObjcId {
    read(this, value_text)
        .map(|s| str_to_nsstring(&s))
        .unwrap_or(nil)
}
fn hint(this: &Object) -> ObjcId {
    read(this, |n| n.description.clone())
        .map(|s| str_to_nsstring(&s))
        .unwrap_or(nil)
}
fn frame(this: &Object) -> NSRect {
    read(this, |n| n.bounds)
        .map(|r| screen_rect(key_of(this).0, r))
        .unwrap_or_else(zero_rect)
}
fn semantic_traits(n: &Node) -> u64 {
    unsafe {
        let mut bits = match n.role {
            Role::Button
            | Role::ToggleButton
            | Role::CheckBox
            | Role::Radio
            | Role::Switch
            | Role::MenuItem
            | Role::MenuItemCheck
            | Role::MenuItemRadio
            | Role::Tab
            | Role::ComboBox
            | Role::ColorWell
            | Role::DisclosureButton => UIAccessibilityTraitButton,
            Role::Link => UIAccessibilityTraitLink,
            Role::Heading | Role::Header | Role::ColumnHeader | Role::RowHeader => {
                UIAccessibilityTraitHeader
            }
            Role::StaticText | Role::Paragraph => UIAccessibilityTraitStaticText,
            Role::Image | Role::Canvas => UIAccessibilityTraitImage,
            Role::SearchField => UIAccessibilityTraitSearchField,
            Role::TabList => UIAccessibilityTraitTabBar,
            Role::KeyboardKey => UIAccessibilityTraitKeyboardKey,
            _ => 0,
        };
        if n.states.contains(States::SELECTED) || n.toggle == Toggle::On {
            bits |= UIAccessibilityTraitSelected;
        }
        if n.states.contains(States::DISABLED) {
            bits |= UIAccessibilityTraitNotEnabled;
        }
        // Read-only progress/meter ranges must not advertise an adjustment they cannot perform.
        if n.range.is_some()
            && n.actions
                .intersects(Actions::INCREMENT | Actions::DECREMENT)
        {
            bits |= UIAccessibilityTraitAdjustable;
        }
        if n.live != Live::Off {
            bits |= UIAccessibilityTraitUpdatesFrequently;
        }
        bits
    }
}
fn traits(this: &Object) -> u64 {
    read(this, semantic_traits).unwrap_or(0)
}
fn parent(this: &Object) -> ObjcId {
    let (k, n) = key_of(this);
    hal::with_tree(k, |t| t.parent(n))
        .flatten()
        .map(|n| element(k, n))
        .unwrap_or(nil)
}
fn children(this: &Object) -> ObjcId {
    let (k, n) = key_of(this);
    children_for(k, n)
}
fn count(this: &Object) -> isize {
    let (k, n) = key_of(this);
    count_for(k, n)
}
fn child_at(this: &Object, index: isize) -> ObjcId {
    let (k, n) = key_of(this);
    at_index(k, n, index)
}
fn child_index(this: &Object, obj: ObjcId) -> isize {
    let (k, n) = key_of(this);
    index_of(k, n, obj)
}
fn hidden(this: &Object) -> BOOL {
    yes(read(this, |n| n.states.contains(States::HIDDEN)).unwrap_or(true))
}
fn modal(this: &Object) -> BOOL {
    yes(read(this, |n| n.states.contains(States::MODAL)).unwrap_or(false))
}
fn did_focus(this: &Object) {
    let (k, n) = key_of(this);
    if read(this, |_| ()).is_some() {
        FOCUS.with(|f| f.borrow_mut().insert(k, n));
    }
}
fn lost_focus(this: &Object) {
    let (k, n) = key_of(this);
    FOCUS.with(|f| {
        let mut f = f.borrow_mut();
        if f.get(&k) == Some(&n) {
            f.remove(&k);
        }
    });
}
fn focused(this: &Object) -> BOOL {
    let (k, n) = key_of(this);
    yes(FOCUS.with(|f| f.borrow().get(&k) == Some(&n)) && read(this, |_| ()).is_some())
}
fn send(this: &Object, action: Actions, arg: ActionArg) -> BOOL {
    if !read(this, |n| {
        n.actions.contains(action)
            && !n.states.intersects(States::DISABLED | States::HIDDEN)
            && match &arg {
                ActionArg::Custom(s) => n.custom_actions.contains(s),
                _ => true,
            }
    })
    .unwrap_or(false)
        || view(key_of(this).0) == nil
    {
        return NO;
    }
    let (key, node) = key_of(this);
    let req = hal::inject_action(
        key,
        ActionRequest {
            req: 0,
            node,
            action,
            arg,
        },
    );
    if CHECK_ACTION.with(Cell::get) {
        CHECK_REQUEST.with(|c| c.set(Some(req)));
    } else {
        hal::forget_action(req);
    }
    YES
}
fn activation_action(n: &Node) -> Actions {
    if n.role.is_text_input() && n.states.contains(States::EDITABLE) {
        Actions::FOCUS
    } else if n.actions.contains(Actions::TOGGLE) {
        Actions::TOGGLE
    } else {
        Actions::PRESS
    }
}
fn press(this: &Object) -> BOOL {
    let action = read(this, activation_action).unwrap_or(Actions::PRESS);
    let accepted = send(this, action, ActionArg::None);
    // FOCUS uses the ordinary widget/IME path. If it is already the active field, keep
    // the real UITextInput as first responder; otherwise ImeShow will do so after focus.
    #[cfg(target_os = "ios")]
    if accepted != NO && action == Actions::FOCUS {
        let (key, node) = key_of(this);
        super::text::accessibility_activate(key, node);
    }
    accepted
}
fn increment(this: &Object) {
    send(this, Actions::INCREMENT, ActionArg::None);
}
fn decrement(this: &Object) {
    send(this, Actions::DECREMENT, ActionArg::None);
}
fn escape(this: &Object) -> BOOL {
    send(this, Actions::DISMISS, ActionArg::None)
}
fn scroll(this: &Object, direction: isize) -> BOOL {
    let Some((r, horizontal)) = read(this, |n| (n.bounds, n.states.contains(States::HORIZONTAL)))
    else {
        return NO;
    };
    let (x, y) = match direction {
        1 => (r.w, 0.),
        2 => (-r.w, 0.),
        3 => (0., -r.h),
        4 => (0., r.h),
        5 if horizontal => (r.w, 0.),
        6 if horizontal => (-r.w, 0.),
        5 => (0., r.h),
        6 => (0., -r.h),
        _ => return NO,
    };
    send(this, Actions::SCROLL, ActionArg::Scroll(x, y))
}
fn custom_actions(this: &Object) -> ObjcId {
    let names = read(this, |n| {
        if n.actions.contains(Actions::CUSTOM) && !n.states.contains(States::DISABLED) {
            n.custom_actions.clone()
        } else {
            Vec::new()
        }
    })
    .unwrap_or_default();
    let mut objects = Vec::with_capacity(names.len());
    for name in names {
        unsafe {
            let obj: ObjcId = msg_send![class!(UIAccessibilityCustomAction), alloc];
            let obj: ObjcId = msg_send![obj, initWithName: str_to_nsstring(&name) target: this selector: sel!(performCustomAction:)];
            let obj: ObjcId = msg_send![obj, autorelease];
            objects.push(obj);
        }
    }
    array(&objects)
}
fn perform_custom(this: &Object, action: ObjcId) -> BOOL {
    if action == nil {
        return NO;
    }
    let name: ObjcId = unsafe { msg_send![action, name] };
    if name == nil {
        return NO;
    }
    send(
        this,
        Actions::CUSTOM,
        ActionArg::Custom(nsstring_to_string(name)),
    )
}
#[cfg(target_os = "ios")]
fn text_responder(this: &Object) -> ObjcId {
    let (key, node) = key_of(this);
    super::text::accessibility_responder(key, node)
}

fn page_content(this: &Object) -> ObjcId {
    read(this, |n| {
        n.text
            .as_ref()
            .map_or_else(|| n.name.clone(), |t| t.content.clone())
    })
    .map(|s| str_to_nsstring(&s))
    .unwrap_or(nil)
}
fn line_content(this: &Object, line: isize) -> ObjcId {
    if line < 0 {
        return nil;
    }
    read(this, |n| {
        if let Some(t) = &n.text {
            if !t.lines.is_empty() {
                t.lines.get(line as usize)?;
                let (a, b) = makepad_a11y::mirror::text::line_range(t, line as usize);
                return t.content.get(a as usize..b as usize).map(str::to_owned);
            }
            return (line == 0).then(|| t.content.clone());
        }
        (line == 0).then(|| n.name.clone())
    })
    .flatten()
    .map(|s| str_to_nsstring(&s))
    .unwrap_or(nil)
}
fn line_frame(this: &Object, line: isize) -> NSRect {
    if line < 0 {
        return zero_rect();
    }
    read(this, |n| {
        if let Some(t) = &n.text {
            if !t.lines.is_empty() {
                return t.lines.get(line as usize).map(|l| l.rect);
            }
        }
        (line == 0).then_some(n.bounds)
    })
    .flatten()
    .map(|r| screen_rect(key_of(this).0, r))
    .unwrap_or_else(zero_rect)
}
fn line_for_point(this: &Object, point: NSPoint) -> isize {
    let v = view(key_of(this).0);
    if v == nil {
        return -1;
    }
    let bounds: NSRect = unsafe { msg_send![v, bounds] };
    let (x, y) = (
        (point.x - bounds.origin.x) as f32,
        (point.y - bounds.origin.y) as f32,
    );
    read(this, |n| {
        if let Some(t) = &n.text {
            if !t.lines.is_empty() {
                return t
                    .lines
                    .iter()
                    .position(|l| l.rect.contains(x, y))
                    .map(|i| i as isize)
                    .unwrap_or(-1);
            }
        }
        if n.bounds.contains(x, y) {
            0
        } else {
            -1
        }
    })
    .unwrap_or(-1)
}

fn define_element_class() -> *const Class {
    let mut decl = ClassDecl::new("HalA11yElementUI", class!(UIAccessibilityElement)).unwrap();
    decl.add_ivar::<u64>("window_key");
    decl.add_ivar::<u64>("node");
    if let Some(protocol) = Protocol::get("UIAccessibilityReadingContent") {
        decl.add_protocol(protocol);
    }
    objc_class!(decl;
        (isAccessibilityElement) => is_element() -> BOOL = NO;
        (accessibilityLabel) => label() -> ObjcId = nil;
        (accessibilityValue) => value() -> ObjcId = nil;
        (accessibilityHint) => hint() -> ObjcId = nil;
        (accessibilityTraits) => traits() -> u64 = 0;
        (accessibilityFrame) => frame() -> NSRect = zero_rect();
        (accessibilityContainer) => parent() -> ObjcId = nil;
        (accessibilityElements) => children() -> ObjcId = nil;
        (accessibilityElementCount) => count() -> isize = 0;
        (accessibilityElementAtIndex:) => child_at(index: isize) -> ObjcId = nil;
        (indexOfAccessibilityElement:) => child_index(obj: ObjcId) -> isize = isize::MAX;
        (accessibilityElementsHidden) => hidden() -> BOOL = YES;
        (accessibilityViewIsModal) => modal() -> BOOL = NO;
        (accessibilityElementDidBecomeFocused) => did_focus() -> () = ();
        (accessibilityElementDidLoseFocus) => lost_focus() -> () = ();
        (accessibilityElementIsFocused) => focused() -> BOOL = NO;
        (accessibilityActivate) => press() -> BOOL = NO;
        (accessibilityIncrement) => increment() -> () = ();
        (accessibilityDecrement) => decrement() -> () = ();
        (accessibilityScroll:) => scroll(direction: isize) -> BOOL = NO;
        (accessibilityPerformEscape) => escape() -> BOOL = NO;
        (accessibilityCustomActions) => custom_actions() -> ObjcId = nil;
        (performCustomAction:) => perform_custom(action: ObjcId) -> BOOL = NO;
        (accessibilityLineNumberForPoint:) => line_for_point(point: NSPoint) -> isize = -1;
        (accessibilityContentForLineNumber:) => line_content(line: isize) -> ObjcId = nil;
        (accessibilityFrameForLineNumber:) => line_frame(line: isize) -> NSRect = zero_rect();
        (accessibilityPageContent) => page_content() -> ObjcId = nil;
    );
    #[cfg(target_os = "ios")]
    objc_class!(decl;
        // Public since iOS 18.1. Older systems simply do not query this selector.
        (accessibilityTextInputResponder) => text_responder() -> ObjcId = nil;
    );
    decl.register()
}
fn release(objects: Vec<ObjcId>) {
    for obj in objects {
        unsafe {
            let () = msg_send![obj, release];
        }
    }
}
pub(super) fn drop_window(key: u64) {
    let objects = ELEMENTS.with(|c| {
        c.borrow_mut()
            .remove(&key)
            .unwrap_or_default()
            .into_values()
            .collect()
    });
    FOCUS.with(|c| c.borrow_mut().remove(&key));
    DIALOGS.with(|c| c.borrow_mut().remove(&key));
    release(objects);
}
fn prune(key: u64) {
    let ids: Vec<_> = ELEMENTS.with(|c| {
        c.borrow()
            .get(&key)
            .map(|c| c.keys().copied().collect())
            .unwrap_or_default()
    });
    let removed = hal::with_tree(key, |t| {
        ids.iter()
            .copied()
            .filter(|id| t.node(*id).is_none())
            .collect::<Vec<_>>()
    })
    .unwrap_or(ids);
    let objects = ELEMENTS.with(|c| {
        let mut c = c.borrow_mut();
        let Some(c) = c.get_mut(&key) else {
            return Vec::new();
        };
        removed.iter().filter_map(|id| c.remove(id)).collect()
    });
    FOCUS.with(|f| {
        let mut f = f.borrow_mut();
        if f.get(&key).is_some_and(|n| removed.contains(n)) {
            f.remove(&key);
        }
    });
    release(objects);
}
/// HAL submission holds its own state lock: always enqueue UIKit work, even from main.
pub(crate) fn notify(key: u64, notifications: &[Notification]) {
    let notifications = notifications.to_vec();
    // There is no NodeAdded notification. Capture dialog identity from this exact update,
    // including empty-notification updates, before a later update can replace the mirror.
    let dialogs = hal::with_tree(key, |t| {
        let mut ids = HashSet::new();
        let mut pending = vec![t.root];
        let mut seen = HashSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(n) = t.node(id).filter(|n| !n.states.contains(States::HIDDEN)) else {
                continue;
            };
            if matches!(n.role, Role::Dialog | Role::AlertDialog) {
                ids.insert(id);
            }
            pending.extend(n.children.iter().copied());
        }
        ids
    })
    .unwrap_or_default();
    app::run_on_main(move || {
        shielded(
            "UIKit notifications",
            || (),
            || {
                if view(key) == nil {
                    drop_window(key);
                    return;
                }
                prune(key);
                let added = DIALOGS.with(|d| {
                    let mut d = d.borrow_mut();
                    let previous = d.entry(key).or_default();
                    let mut added: Vec<_> = dialogs.difference(previous).copied().collect();
                    added.sort();
                    *previous = dialogs.clone();
                    added
                });
                unsafe {
                    for id in added {
                        let obj = element(key, id);
                        if obj != nil {
                            UIAccessibilityPostNotification(
                                UIAccessibilityScreenChangedNotification,
                                obj,
                            );
                        }
                    }
                    for note in &notifications {
                        match note {
                            Notification::FocusChanged(n) => {
                                let obj = element(key, *n);
                                UIAccessibilityPostNotification(
                                    UIAccessibilityLayoutChangedNotification,
                                    obj,
                                );
                            }
                            Notification::ChildrenChanged(_) => UIAccessibilityPostNotification(
                                UIAccessibilityLayoutChangedNotification,
                                nil,
                            ),
                            Notification::ValueChanged(n) => {
                                let ax_focus = FOCUS.with(|f| f.borrow().get(&key) == Some(n));
                                let adjustable = hal::with_tree(key, |t| {
                                    (ax_focus || t.focus == *n)
                                        && t.node(*n).is_some_and(|n| {
                                            semantic_traits(n) & UIAccessibilityTraitAdjustable != 0
                                        })
                                })
                                .unwrap_or(false);
                                if adjustable {
                                    UIAccessibilityPostNotification(
                                        UIAccessibilityLayoutChangedNotification,
                                        nil,
                                    );
                                }
                            }
                            Notification::Announce { text, live } if *live != Live::Off => {
                                UIAccessibilityPostNotification(
                                    UIAccessibilityAnnouncementNotification,
                                    str_to_nsstring(text),
                                );
                            }
                            Notification::LiveRegionChanged(n) => {
                                if let Some(s) = hal::with_tree(key, |t| {
                                    t.node(*n).filter(|n| n.live != Live::Off).map(|n| {
                                        if n.value.is_empty() {
                                            n.name.clone()
                                        } else {
                                            n.value.clone()
                                        }
                                    })
                                })
                                .flatten()
                                {
                                    UIAccessibilityPostNotification(
                                        UIAccessibilityAnnouncementNotification,
                                        str_to_nsstring(&s),
                                    );
                                }
                            }
                            _ => (),
                        }
                    }
                }
            },
        )
    });
}
