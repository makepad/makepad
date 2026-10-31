//! AppKit is a consumer of the HAL mirror, never a second semantic tree. All Objective-C objects
//! and the cache live on the main thread. Copy mirror data before messaging AppKit (it can reenter).
use super::{app, classes::win_of};
use crate::a11y as hal;
use crate::backend::apple::objc_class::{objc_class, shielded};
use makepad_a11y::{
    consumer, mirror::text, ActionArg, ActionRequest, Actions, Live, Node, NodeId, Notification,
    Rect, States, TextInfo, Toggle,
};
use makepad_apple_sys::{accessibility::*, *};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    sync::Once,
};

mod check;

thread_local! {
    // The cache owns one retain per element; returned arrays retain their members independently.
    static ELEMENTS: RefCell<HashMap<u64, HashMap<NodeId, ObjcId>>> = RefCell::new(HashMap::new());
    static ELEMENT_CLASS: Cell<*const Class> = const { Cell::new(std::ptr::null()) };
    // Only an oscheck action asks to keep the completion for its remote thread.
    static CHECK_ACTION: Cell<bool> = const { Cell::new(false) };
    static CHECK_REQUEST: Cell<Option<u64>> = const { Cell::new(None) };
}

fn activate() {
    static START: Once = Once::new();
    START.call_once(|| hal::request_consumer(consumer::OS, true));
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
    let root = hal::with_tree(key, |t| (t.root, t.node(id).is_some()));
    match root {
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
        let obj: ObjcId = msg_send![cls, new];
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

fn children_for(key: u64, id: NodeId) -> ObjcId {
    let ids = hal::with_tree(key, |t| t.node(id).map(|n| n.children.clone()))
        .flatten()
        .unwrap_or_default();
    let objects: Vec<_> = ids
        .into_iter()
        .map(|id| element(key, id))
        .filter(|o| *o != nil)
        .collect();
    array(&objects)
}

fn view_key(this: &Object) -> Option<u64> {
    activate();
    // NSView initialization may query accessibility before initWithPtr: sets its owner.
    let ptr: *mut c_void = unsafe { *this.get_ivar("macos_window_ptr") };
    if ptr.is_null() {
        return None;
    }
    let w = win_of(this);
    (!w.retired.get()).then(|| hal::window_key(w.sid.get(), w.id.get().index))
}
fn view_is_element(this: &Object) -> BOOL {
    view_key(this);
    NO
}
fn view_role(this: &Object) -> ObjcId {
    view_key(this);
    str_to_nsstring("AXGroup")
}
fn view_children(this: &Object) -> ObjcId {
    let Some(key) = view_key(this) else {
        return array(&[]);
    };
    let root = hal::with_tree(key, |t| t.root).unwrap_or(NodeId::NONE);
    children_for(key, root)
}
fn view_focus(this: &Object) -> ObjcId {
    let Some(key) = view_key(this) else {
        return nil;
    };
    element(
        key,
        hal::with_tree(key, |t| t.focus).unwrap_or(NodeId::NONE),
    )
}
fn view_hit(this: &Object, point: NSPoint) -> ObjcId {
    let Some(key) = view_key(this) else {
        return nil;
    };
    let Some((x, y)) = local_point(key, point) else {
        return nil;
    };
    let id = hal::with_tree(key, |t| t.hit_test(x, y))
        .flatten()
        .unwrap_or(NodeId::NONE);
    element(key, id)
}

pub(super) fn add_view_methods(decl: &mut ClassDecl) {
    hal::set_os_selfcheck(check::oscheck);
    objc_class!(decl;
        (isAccessibilityElement) => view_is_element() -> BOOL = NO;
        (accessibilityRole) => view_role() -> ObjcId = nil;
        (accessibilityChildren) => view_children() -> ObjcId = nil;
        (accessibilityHitTest:) => view_hit(point: NSPoint) -> ObjcId = nil;
        (accessibilityFocusedUIElement) => view_focus() -> ObjcId = nil;
    );
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
fn missing_range() -> NSRange {
    NSRange {
        location: isize::MAX as u64,
        length: 0,
    }
}
fn yes(value: bool) -> BOOL {
    if value {
        YES
    } else {
        NO
    }
}

/// Mirror geometry is top-left, y-down, independent of NSView's flippedness or backing scale.
fn screen_rect(key: u64, r: Rect) -> NSRect {
    let v = view(key);
    if v == nil {
        return zero_rect();
    }
    unsafe {
        let bounds: NSRect = msg_send![v, bounds];
        let flipped: BOOL = msg_send![v, isFlipped];
        let rect = NSRect {
            origin: NSPoint {
                x: bounds.origin.x + r.x as f64,
                y: bounds.origin.y
                    + if flipped != NO {
                        r.y as f64
                    } else {
                        bounds.size.height - (r.y + r.h) as f64
                    },
            },
            size: NSSize {
                width: r.w as f64,
                height: r.h as f64,
            },
        };
        let rect: NSRect = msg_send![v, convertRect: rect toView: nil];
        let window: ObjcId = msg_send![v, window];
        msg_send![window, convertRectToScreen: rect]
    }
}
fn local_point(key: u64, point: NSPoint) -> Option<(f32, f32)> {
    let v = view(key);
    if v == nil {
        return None;
    }
    unsafe {
        let window: ObjcId = msg_send![v, window];
        let p: NSPoint = msg_send![window, convertPointFromScreen: point];
        let p: NSPoint = msg_send![v, convertPoint: p fromView: nil];
        let bounds: NSRect = msg_send![v, bounds];
        let flipped: BOOL = msg_send![v, isFlipped];
        Some((
            (p.x - bounds.origin.x) as f32,
            (if flipped != NO {
                p.y - bounds.origin.y
            } else {
                bounds.origin.y + bounds.size.height - p.y
            }) as f32,
        ))
    }
}

fn is_element(this: &Object) -> BOOL {
    yes(read(this, |_| true).unwrap_or(false))
}
fn role(this: &Object) -> ObjcId {
    read(this, |n| n.role.ax_role().0)
        .map(str_to_nsstring)
        .unwrap_or(nil)
}
fn subrole(this: &Object) -> ObjcId {
    read(this, |n| n.role.ax_role().1)
        .filter(|s| !s.is_empty())
        .map(str_to_nsstring)
        .unwrap_or(nil)
}
fn role_description(this: &Object) -> ObjcId {
    let Some((description, (role, subrole))) =
        read(this, |n| (n.role_description.clone(), n.role.ax_role()))
    else {
        return nil;
    };
    if !description.is_empty() {
        return str_to_nsstring(&description);
    }
    unsafe {
        NSAccessibilityRoleDescription(
            str_to_nsstring(role),
            if subrole.is_empty() {
                nil
            } else {
                str_to_nsstring(subrole)
            },
        )
    }
}
fn label(this: &Object) -> ObjcId {
    read(this, |n| n.name.clone())
        .map(|s| str_to_nsstring(&s))
        .unwrap_or(nil)
}
fn help(this: &Object) -> ObjcId {
    read(this, |n| n.description.clone())
        .map(|s| str_to_nsstring(&s))
        .unwrap_or(nil)
}
fn placeholder(this: &Object) -> ObjcId {
    read(this, |n| n.placeholder.clone())
        .map(|s| str_to_nsstring(&s))
        .unwrap_or(nil)
}
fn number(n: f64) -> ObjcId {
    unsafe { msg_send![class!(NSNumber), numberWithDouble: n] }
}
fn toggle_value(t: Toggle) -> f64 {
    match t {
        Toggle::On => 1.,
        Toggle::Mixed => 2.,
        _ => 0.,
    }
}
fn value(this: &Object) -> ObjcId {
    let Some((range, toggle, value)) = read(this, |n| (n.range, n.toggle, n.value.clone())) else {
        return nil;
    };
    if let Some(r) = range {
        number(r.now)
    } else if toggle != Toggle::None {
        number(toggle_value(toggle))
    } else {
        str_to_nsstring(&value)
    }
}
fn min_value(this: &Object) -> ObjcId {
    read(this, |n| n.range)
        .flatten()
        .map(|r| number(r.min))
        .unwrap_or(nil)
}
fn max_value(this: &Object) -> ObjcId {
    read(this, |n| n.range)
        .flatten()
        .map(|r| number(r.max))
        .unwrap_or(nil)
}
fn frame(this: &Object) -> NSRect {
    read(this, |n| n.bounds)
        .map(|r| screen_rect(key_of(this).0, r))
        .unwrap_or_else(zero_rect)
}
fn parent(this: &Object) -> ObjcId {
    let (key, id) = key_of(this);
    let parent = hal::with_tree(key, |t| t.parent(id)).flatten();
    parent.map(|p| element(key, p)).unwrap_or(nil)
}
fn children(this: &Object) -> ObjcId {
    let (k, n) = key_of(this);
    children_for(k, n)
}
fn enabled(this: &Object) -> BOOL {
    yes(read(this, |n| !n.states.contains(States::DISABLED)).unwrap_or(false))
}
fn focused(this: &Object) -> BOOL {
    let (k, n) = key_of(this);
    yes(hal::with_tree(k, |t| t.focus == n && t.node(n).is_some()).unwrap_or(false))
}
fn selected(this: &Object) -> BOOL {
    yes(read(this, |n| n.states.contains(States::SELECTED)).unwrap_or(false))
}
fn expanded(this: &Object) -> BOOL {
    yes(read(this, |n| n.states.contains(States::EXPANDED)).unwrap_or(false))
}
fn window(this: &Object) -> ObjcId {
    unsafe { msg_send![view(key_of(this).0), window] }
}
fn top_level(this: &Object) -> ObjcId {
    window(this)
}

fn send(this: &Object, action: Actions, arg: ActionArg) -> BOOL {
    let offered = read(this, |n| {
        n.actions.contains(action)
            && !n.states.contains(States::DISABLED)
            && match &arg {
                ActionArg::Custom(s) => n.custom_actions.contains(s),
                _ => true,
            }
    })
    .unwrap_or(false);
    if !offered {
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
fn press_action(n: &Node) -> Actions {
    if n.toggle != Toggle::None && n.actions.contains(Actions::TOGGLE) {
        Actions::TOGGLE
    } else {
        Actions::PRESS
    }
}
fn press(this: &Object) -> BOOL {
    let action = read(this, press_action).unwrap_or(Actions::PRESS);
    send(this, action, ActionArg::None)
}
fn increment(this: &Object) -> BOOL {
    send(this, Actions::INCREMENT, ActionArg::None)
}
fn decrement(this: &Object) -> BOOL {
    send(this, Actions::DECREMENT, ActionArg::None)
}
fn show_menu(this: &Object) -> BOOL {
    send(this, Actions::SHOW_MENU, ActionArg::None)
}
fn cancel(this: &Object) -> BOOL {
    send(this, Actions::DISMISS, ActionArg::None)
}
fn set_focused(this: &Object, on: BOOL) {
    if on != NO {
        send(this, Actions::FOCUS, ActionArg::None);
    }
}
fn set_value(this: &Object, value: ObjcId) {
    if value == nil {
        return;
    }
    let numeric = read(this, |n| n.range.is_some() || n.toggle != Toggle::None).unwrap_or(false);
    let arg = unsafe {
        if numeric {
            ActionArg::Number(msg_send![value, doubleValue])
        } else {
            let s: ObjcId = msg_send![value, description];
            ActionArg::Text(nsstring_to_string(s))
        }
    };
    send(this, Actions::SET_VALUE, arg);
}
fn custom_actions(this: &Object) -> ObjcId {
    let names = read(this, |n| n.custom_actions.clone()).unwrap_or_default();
    let mut actions = Vec::with_capacity(names.len());
    for name in names {
        unsafe {
            let obj: ObjcId = msg_send![class!(NSAccessibilityCustomAction), alloc];
            let obj: ObjcId = msg_send![obj, initWithName: str_to_nsstring(&name) target: this selector: sel!(performCustomAction:)];
            let obj: ObjcId = msg_send![obj, autorelease];
            actions.push(obj);
        }
    }
    array(&actions)
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

// Implementing an action selector must not advertise it on every element.
fn selector_allowed(this: &Object, selector: Sel) -> BOOL {
    let Some((actions, disabled, press_action, has_text)) = read(this, |n| {
        (
            n.actions,
            n.states.contains(States::DISABLED),
            press_action(n),
            n.text.is_some(),
        )
    }) else {
        return NO;
    };
    let action = match selector.name() {
        "accessibilityPerformPress" => Some(press_action),
        "accessibilityPerformIncrement" => Some(Actions::INCREMENT),
        "accessibilityPerformDecrement" => Some(Actions::DECREMENT),
        "accessibilityPerformShowMenu" => Some(Actions::SHOW_MENU),
        "accessibilityPerformCancel" => Some(Actions::DISMISS),
        "setAccessibilityValue:" => Some(Actions::SET_VALUE),
        "setAccessibilityFocused:" => Some(Actions::FOCUS),
        "setAccessibilitySelectedTextRange:" => Some(Actions::SET_TEXT_SELECTION),
        _ => None,
    };
    if let Some(action) = action {
        return yes(!disabled && actions.contains(action));
    }
    if matches!(
        selector.name(),
        "accessibilityNumberOfCharacters"
            | "accessibilitySelectedText"
            | "accessibilitySelectedTextRange"
            | "accessibilityStringForRange:"
            | "accessibilityFrameForRange:"
            | "accessibilityLineForIndex:"
            | "accessibilityRangeForLine:"
            | "accessibilityInsertionPointLineNumber"
            | "accessibilityVisibleCharacterRange"
            | "accessibilityRangeForPosition:"
    ) {
        return yes(has_text);
    }
    unsafe {
        msg_send![super(this, class!(NSAccessibilityElement)), isAccessibilitySelectorAllowed: selector]
    }
}

fn row_count(this: &Object) -> isize {
    read(this, |n| n.table.map(|t| t.rows as isize))
        .flatten()
        .unwrap_or(0)
}
fn column_count(this: &Object) -> isize {
    read(this, |n| n.table.map(|t| t.cols as isize))
        .flatten()
        .unwrap_or(0)
}
fn row_range(this: &Object) -> NSRange {
    read(this, |n| n.table)
        .flatten()
        .map(|t| NSRange {
            location: t.row as u64,
            length: t.row_span.max(1) as u64,
        })
        .unwrap_or_else(missing_range)
}
fn column_range(this: &Object) -> NSRange {
    read(this, |n| n.table)
        .flatten()
        .map(|t| NSRange {
            location: t.col as u64,
            length: t.col_span.max(1) as u64,
        })
        .unwrap_or_else(missing_range)
}
fn text_info(this: &Object) -> Option<TextInfo> {
    read(this, |n| n.text.clone()).flatten()
}
fn utf16_range(t: &TextInfo, a: u32, b: u32) -> NSRange {
    let start = text::byte_to_utf16(&t.content, a.min(b)) as u64;
    let end = text::byte_to_utf16(&t.content, a.max(b)) as u64;
    NSRange {
        location: start,
        length: end - start,
    }
}
fn byte_range(t: &TextInfo, range: &NSRange) -> Option<(u32, u32)> {
    let end = range.location.checked_add(range.length)?;
    if end > t.content.encode_utf16().count() as u64 {
        return None;
    }
    Some((
        text::utf16_to_byte(&t.content, range.location as u32),
        text::utf16_to_byte(&t.content, end as u32),
    ))
}
fn character_count(this: &Object) -> isize {
    text_info(this)
        .map(|t| t.content.encode_utf16().count() as isize)
        .unwrap_or(0)
}
fn selected_text(this: &Object) -> ObjcId {
    text_info(this)
        .map(|t| {
            let a = text::floor_boundary(&t.content, t.selection.0.min(t.selection.1) as usize);
            let b = text::floor_boundary(&t.content, t.selection.0.max(t.selection.1) as usize);
            str_to_nsstring(&t.content[a..b])
        })
        .unwrap_or(nil)
}
fn selected_range(this: &Object) -> NSRange {
    text_info(this)
        .map(|t| utf16_range(&t, t.selection.0, t.selection.1))
        .unwrap_or_else(missing_range)
}
fn set_selected_range(this: &Object, range: NSRange) {
    if let Some((a, b)) = text_info(this).and_then(|t| byte_range(&t, &range)) {
        send(this, Actions::SET_TEXT_SELECTION, ActionArg::Range(a, b));
    }
}
fn string_for_range(this: &Object, range: NSRange) -> ObjcId {
    text_info(this)
        .and_then(|t| {
            byte_range(&t, &range).map(|(a, b)| str_to_nsstring(&t.content[a as usize..b as usize]))
        })
        .unwrap_or(nil)
}
fn union(rects: &[Rect]) -> Option<Rect> {
    rects.iter().copied().reduce(|a, b| {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Rect {
            x,
            y,
            w: (a.x + a.w).max(b.x + b.w) - x,
            h: (a.y + a.h).max(b.y + b.h) - y,
        }
    })
}
fn frame_for_range(this: &Object, range: NSRange) -> NSRect {
    text_info(this)
        .and_then(|t| byte_range(&t, &range).and_then(|(a, b)| union(&text::range_rects(&t, a, b))))
        .map(|r| screen_rect(key_of(this).0, r))
        .unwrap_or_else(zero_rect)
}
fn line_for_index(this: &Object, index: isize) -> isize {
    text_info(this)
        .filter(|t| index >= 0 && index as usize <= t.content.encode_utf16().count())
        .map(|t| text::line_for_byte(&t, text::utf16_to_byte(&t.content, index as u32)) as isize)
        .unwrap_or(-1)
}
fn range_for_line(this: &Object, line: isize) -> NSRange {
    text_info(this)
        .filter(|t| line >= 0 && (line as usize) < t.lines.len())
        .map(|t| {
            let (a, b) = text::line_range(&t, line as usize);
            utf16_range(&t, a, b)
        })
        .unwrap_or_else(missing_range)
}
fn insertion_line(this: &Object) -> isize {
    text_info(this)
        .map(|t| text::line_for_byte(&t, t.selection.1) as isize)
        .unwrap_or(-1)
}
fn visible_range(this: &Object) -> NSRange {
    let Some(t) = text_info(this) else {
        return missing_range();
    };
    let v = view(key_of(this).0);
    if v == nil {
        return missing_range();
    }
    let visible: NSRect = unsafe { msg_send![v, visibleRect] };
    let bounds: NSRect = unsafe { msg_send![v, bounds] };
    let flipped: BOOL = unsafe { msg_send![v, isFlipped] };
    let top = if flipped != NO {
        visible.origin.y - bounds.origin.y
    } else {
        bounds.origin.y + bounds.size.height - visible.origin.y - visible.size.height
    } as f32;
    let bottom = top + visible.size.height as f32;
    let mut lines = t
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.rect.y + l.rect.h > top && l.rect.y < bottom)
        .map(|(i, _)| i);
    if let Some(first) = lines.next() {
        let last = lines.last().unwrap_or(first);
        utf16_range(
            &t,
            text::line_range(&t, first).0,
            text::line_range(&t, last).1,
        )
    } else {
        NSRange {
            location: 0,
            length: 0,
        }
    }
}
fn range_for_position(this: &Object, point: NSPoint) -> NSRange {
    let Some((x, y)) = local_point(key_of(this).0, point) else {
        return missing_range();
    };
    text_info(this)
        .map(|t| {
            let a = text::byte_at_point(&t, x, y);
            let b = t
                .content
                .get(a as usize..)
                .and_then(|s| s.chars().next())
                .map(|c| a + c.len_utf8() as u32)
                .unwrap_or(a);
            utf16_range(&t, a, b)
        })
        .unwrap_or_else(missing_range)
}

fn define_element_class() -> *const Class {
    let mut decl = ClassDecl::new("HalA11yElement", class!(NSAccessibilityElement)).unwrap();
    decl.add_ivar::<u64>("window_key");
    decl.add_ivar::<u64>("node");
    objc_class!(decl;
        (isAccessibilityElement) => is_element() -> BOOL = NO;
        (accessibilityRole) => role() -> ObjcId = nil;
        (accessibilitySubrole) => subrole() -> ObjcId = nil;
        (accessibilityRoleDescription) => role_description() -> ObjcId = nil;
        (accessibilityLabel) => label() -> ObjcId = nil;
        (accessibilityValue) => value() -> ObjcId = nil;
        (accessibilityHelp) => help() -> ObjcId = nil;
        (accessibilityPlaceholderValue) => placeholder() -> ObjcId = nil;
        (accessibilityFrame) => frame() -> NSRect = zero_rect();
        (accessibilityParent) => parent() -> ObjcId = nil;
        (accessibilityChildren) => children() -> ObjcId = nil;
        (accessibilityWindow) => window() -> ObjcId = nil;
        (accessibilityTopLevelUIElement) => top_level() -> ObjcId = nil;
        (isAccessibilityEnabled) => enabled() -> BOOL = NO;
        (isAccessibilityFocused) => focused() -> BOOL = NO;
        (setAccessibilityFocused:) => set_focused(on: BOOL) -> () = ();
        (isAccessibilitySelected) => selected() -> BOOL = NO;
        (isAccessibilityExpanded) => expanded() -> BOOL = NO;
        (accessibilityMinValue) => min_value() -> ObjcId = nil;
        (accessibilityMaxValue) => max_value() -> ObjcId = nil;
        (accessibilityPerformPress) => press() -> BOOL = NO;
        (accessibilityPerformIncrement) => increment() -> BOOL = NO;
        (accessibilityPerformDecrement) => decrement() -> BOOL = NO;
        (accessibilityPerformShowMenu) => show_menu() -> BOOL = NO;
        (accessibilityPerformCancel) => cancel() -> BOOL = NO;
        (setAccessibilityValue:) => set_value(value: ObjcId) -> () = ();
        (accessibilityCustomActions) => custom_actions() -> ObjcId = nil;
        (performCustomAction:) => perform_custom(action: ObjcId) -> BOOL = NO;
        (isAccessibilitySelectorAllowed:) => selector_allowed(selector: Sel) -> BOOL = NO;
        (accessibilityRowCount) => row_count() -> isize = 0;
        (accessibilityColumnCount) => column_count() -> isize = 0;
        (accessibilityRowIndexRange) => row_range() -> NSRange = missing_range();
        (accessibilityColumnIndexRange) => column_range() -> NSRange = missing_range();
        (accessibilityNumberOfCharacters) => character_count() -> isize = 0;
        (accessibilitySelectedText) => selected_text() -> ObjcId = nil;
        (accessibilitySelectedTextRange) => selected_range() -> NSRange = missing_range();
        (setAccessibilitySelectedTextRange:) => set_selected_range(range: NSRange) -> () = ();
        (accessibilityStringForRange:) => string_for_range(range: NSRange) -> ObjcId = nil;
        (accessibilityFrameForRange:) => frame_for_range(range: NSRange) -> NSRect = zero_rect();
        (accessibilityLineForIndex:) => line_for_index(index: isize) -> isize = -1;
        (accessibilityRangeForLine:) => range_for_line(line: isize) -> NSRange = missing_range();
        (accessibilityInsertionPointLineNumber) => insertion_line() -> isize = -1;
        (accessibilityVisibleCharacterRange) => visible_range() -> NSRange = missing_range();
        (accessibilityRangeForPosition:) => range_for_position(point: NSPoint) -> NSRange = missing_range();
    );
    decl.register()
}

fn destroy(objects: Vec<ObjcId>) {
    for obj in objects {
        unsafe {
            NSAccessibilityPostNotification(obj, NSAccessibilityUIElementDestroyedNotification);
            let _: () = msg_send![obj, release];
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
    destroy(objects);
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
        removed.into_iter().filter_map(|id| c.remove(&id)).collect()
    });
    destroy(objects);
}
fn post(key: u64, id: NodeId, notification: ObjcId) {
    let obj = element(key, id);
    if obj != nil {
        unsafe {
            NSAccessibilityPostNotification(obj, notification);
        }
    }
}
fn announce(key: u64, text: &str, live: Live) {
    let v = view(key);
    if v == nil || live == Live::Off {
        return;
    }
    unsafe {
        // NSAccessibilityPriorityLevel: low=10, medium=50, high=90.
        let keys = [NSAccessibilityAnnouncementKey, NSAccessibilityPriorityKey];
        let values = [
            str_to_nsstring(text),
            number(if live == Live::Assertive { 90. } else { 50. }),
        ];
        let info: ObjcId = msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr() forKeys: keys.as_ptr() count: keys.len()];
        NSAccessibilityPostNotificationWithUserInfo(
            v,
            NSAccessibilityAnnouncementRequestedNotification,
            info,
        );
    }
}
/// Called by Gpu::a11y_notify during HAL command submission, with the HAL state locked.
/// Always enqueue, even if the caller happens to be main: AppKit can reenter HAL callbacks.
pub(crate) fn notify(key: u64, notifications: &[Notification]) {
    let notifications = notifications.to_vec();
    app::run_on_main(move || {
        shielded(
            "a11y notifications",
            || (),
            || {
                prune(key);
                for note in &notifications {
                    unsafe {
                        match note {
                            Notification::FocusChanged(n) => {
                                post(key, *n, NSAccessibilityFocusedUIElementChangedNotification)
                            }
                            Notification::ValueChanged(n) => {
                                post(key, *n, NSAccessibilityValueChangedNotification)
                            }
                            Notification::NameChanged(n) => {
                                post(key, *n, NSAccessibilityTitleChangedNotification)
                            }
                            Notification::ChildrenChanged(n) => {
                                post(key, *n, NSAccessibilityLayoutChangedNotification)
                            }
                            Notification::CaretMoved(n) => {
                                post(key, *n, NSAccessibilitySelectedTextChangedNotification)
                            }
                            Notification::TextChanged { node, .. } => {
                                post(key, *node, NSAccessibilityValueChangedNotification)
                            }
                            Notification::StatesChanged { node, old, new } => {
                                if old.contains(States::EXPANDED) != new.contains(States::EXPANDED)
                                    || old.contains(States::COLLAPSED)
                                        != new.contains(States::COLLAPSED)
                                {
                                    post(
                                        key,
                                        *node,
                                        if new.contains(States::EXPANDED) {
                                            NSAccessibilityRowExpandedNotification
                                        } else {
                                            NSAccessibilityRowCollapsedNotification
                                        },
                                    );
                                }
                            }
                            Notification::Announce { text, live } => announce(key, text, *live),
                            Notification::LiveRegionChanged(n) => {
                                if let Some((s, live)) = hal::with_tree(key, |t| {
                                    t.node(*n).map(|n| {
                                        (
                                            if n.value.is_empty() {
                                                n.name.clone()
                                            } else {
                                                n.value.clone()
                                            },
                                            n.live,
                                        )
                                    })
                                })
                                .flatten()
                                {
                                    announce(key, &s, live);
                                }
                            }
                        }
                    }
                }
            },
        )
    });
}
