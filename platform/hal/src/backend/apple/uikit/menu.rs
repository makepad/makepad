//! The text editing chrome iOS draws for the app (iOS): the system edit menu (Copy / Cut /
//! Paste / Select All, `ClipboardActions`) and the selection's drag handles
//! (`SelectionHandles` -> `SelectionHandle` events). The menu's actions arrive at the window
//! view as a responder and become what the app already understands: the copy / cut chord, a
//! paste, Command A. Moved from os/apple/ios/ios_app.rs (`show_clipboard_actions`,
//! `*_selection_handles`) and ios_delegates.rs (the view's actions, the menu delegate).

use super::app::app;
use super::classes::{shielded, win_of};
use super::keys;
use super::text;
use super::window::Win;
use crate::types::input::MOD_LOGO;
use crate::types::*;
use makepad_apple_sys::*;
use makepad_key_code::KeyCode;
use std::cell::Cell;

/// Main thread only.
struct Menu {
    /// UIEditMenuInteraction (iOS 16+), its delegate and the view it is attached to
    interaction: Cell<ObjcId>,
    delegate: Cell<ObjcId>,
    host: Cell<ObjcId>,
    /// the menu's target (window points of `host`) and whether there is a selection to copy
    rect: Cell<NSRect>,
    selection: Cell<bool>,
    /// the two handle views and the window they are over
    handles: Cell<[ObjcId; 2]>,
    handle_owner: Cell<Option<&'static Win>>,
    handle_target: Cell<*const Class>,
}

struct MainOnly(Menu);
unsafe impl Sync for MainOnly {}
unsafe impl Send for MainOnly {}

fn menu() -> &'static Menu {
    static MENU: std::sync::OnceLock<MainOnly> = std::sync::OnceLock::new();
    &MENU
        .get_or_init(|| {
            MainOnly(Menu {
                interaction: Cell::new(nil),
                delegate: Cell::new(nil),
                host: Cell::new(nil),
                rect: Cell::new(NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: 1.0, height: 1.0 } }),
                selection: Cell::new(false),
                handles: Cell::new([nil, nil]),
                handle_owner: Cell::new(None),
                handle_target: Cell::new(std::ptr::null()),
            })
        })
        .0
}

/// At launch: the menu delegate and the edit-menu interaction where the system has it.
pub(super) fn init(_root: ObjcId) {
    let m = menu();
    unsafe {
        m.delegate.set(msg_send![define_menu_delegate(), new]);
        m.handle_target.set(define_handle_target());
        let class = makepad_objc_sys::runtime::objc_getClass(b"UIEditMenuInteraction\0".as_ptr() as *const _) as ObjcId;
        if !class.is_null() {
            let interaction: ObjcId = msg_send![class, alloc];
            let interaction: ObjcId = msg_send![interaction, initWithDelegate: m.delegate.get()];
            m.interaction.set(interaction);
        }
    }
}

pub(super) fn session_closed(sid: u32) {
    let m = menu();
    if m.handle_owner.get().is_some_and(|w| w.owner_sid == sid || w.sid.get() == sid) {
        show_handles(false);
        m.handle_owner.set(None);
    }
}

/// Shows (at `area`, window points) or hides the edit menu over `win`.
pub(super) fn clipboard_actions(win: &'static Win, show: bool, area: [f32; 4], selection: bool) {
    let m = menu();
    unsafe {
        if !show {
            let interaction = m.interaction.get();
            if interaction != nil {
                let () = msg_send![interaction, dismissMenu];
            } else {
                let controller: ObjcId = msg_send![class!(UIMenuController), sharedMenuController];
                let () = msg_send![controller, setMenuVisible: NO animated: YES];
            }
            return;
        }
        let rect = NSRect { origin: NSPoint { x: area[0] as f64, y: area[1] as f64 }, size: NSSize { width: (area[2] as f64).max(1.0), height: (area[3] as f64).max(1.0) } };
        m.rect.set(rect);
        m.selection.set(selection);
        let interaction = m.interaction.get();
        if interaction != nil {
            if m.host.get() != win.view {
                let host = m.host.replace(win.view);
                if host != nil {
                    let () = msg_send![host, removeInteraction: interaction];
                }
                let () = msg_send![win.view, addInteraction: interaction];
            }
            let source = NSPoint { x: rect.origin.x + rect.size.width / 2.0, y: rect.origin.y + rect.size.height / 2.0 };
            let config: ObjcId = msg_send![class!(UIEditMenuConfiguration), configurationWithIdentifier: nil sourcePoint: source];
            let () = msg_send![interaction, presentEditMenuWithConfiguration: config];
        } else {
            // iOS 15: UIMenuController
            let controller: ObjcId = msg_send![class!(UIMenuController), sharedMenuController];
            let () = msg_send![win.view, becomeFirstResponder];
            let () = msg_send![controller, setTargetRect: rect inView: win.view];
            let () = msg_send![controller, setMenuVisible: YES animated: YES];
        }
    }
}

/// The selection's handles over `win`: shown at, moved to (`start` / `end`, window points: the
/// handles' centres) or hidden.
pub(super) fn selection_handles(win: &'static Win, op: u32, start: [f32; 2], end: [f32; 2]) {
    let m = menu();
    if op == selection_handles::HIDE {
        show_handles(false);
        return;
    }
    let mut handles = m.handles.get();
    unsafe {
        if handles[0] == nil {
            handles = [make_handle(0), make_handle(1)];
            m.handles.set(handles);
        }
        if m.handle_owner.get().is_none_or(|w| !std::ptr::eq(w, win)) {
            for handle in handles {
                let () = msg_send![handle, removeFromSuperview];
                let () = msg_send![win.view, addSubview: handle];
            }
            m.handle_owner.set(Some(win));
        }
        for (handle, at) in handles.into_iter().zip([start, end]) {
            let () = msg_send![handle, setCenter: NSPoint { x: at[0] as f64, y: at[1] as f64 }];
        }
    }
    if op == selection_handles::SHOW {
        show_handles(true);
    }
}

fn show_handles(show: bool) {
    for handle in menu().handles.get() {
        if handle == nil {
            continue;
        }
        unsafe {
            let () = msg_send![handle, setHidden: if show { NO } else { YES }];
            if show {
                let parent: ObjcId = msg_send![handle, superview];
                if parent != nil {
                    let () = msg_send![parent, bringSubviewToFront: handle];
                }
            }
        }
    }
}

/// A 24 point round handle with its pan recognizer (`kind` 0 start, 1 end), hidden.
unsafe fn make_handle(kind: i64) -> ObjcId {
    const SIZE: f64 = 24.0;
    let handle: ObjcId = msg_send![class!(UIView), alloc];
    let handle: ObjcId = msg_send![handle, initWithFrame: NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: SIZE, height: SIZE } }];
    let color: ObjcId = msg_send![class!(UIColor), systemBlueColor];
    let () = msg_send![handle, setBackgroundColor: color];
    let layer: ObjcId = msg_send![handle, layer];
    let () = msg_send![layer, setCornerRadius: SIZE * 0.5];
    let () = msg_send![handle, setUserInteractionEnabled: YES];
    let () = msg_send![handle, setHidden: YES];
    let target: ObjcId = msg_send![menu().handle_target.get(), new];
    (*target).set_ivar::<i64>("kind", kind);
    let pan: ObjcId = msg_send![class!(UIPanGestureRecognizer), alloc];
    let pan: ObjcId = msg_send![pan, initWithTarget: target action: sel!(pan:)];
    let () = msg_send![handle, addGestureRecognizer: pan];
    handle
}

/// The handles' pan target: the dragged handle stays under the finger (the app sends the
/// authoritative places back with `SelectionHandles` MOVE); the app hears begin / move / end.
fn define_handle_target() -> *const Class {
    extern "C" fn pan(this: &Object, _: Sel, recognizer: ObjcId) {
        shielded("selection-handle", std::panic::AssertUnwindSafe(|| unsafe {
            let state: i64 = msg_send![recognizer, state];
            let phase = match state {
                1 => 0,         // began
                2 => 1,         // changed
                3 | 4 | 5 => 2, // ended, cancelled, failed
                _ => return,
            };
            let kind: i64 = *this.get_ivar("kind");
            let handle: ObjcId = msg_send![recognizer, view];
            let host: ObjcId = if handle == nil { nil } else { msg_send![handle, superview] };
            let Some(win) = app().win_of_view(host) else { return };
            let at: NSPoint = msg_send![recognizer, locationInView: host];
            let () = msg_send![handle, setCenter: at];
            let id = win.id.get();
            win.push(|q| q.selection_handle(id, kind as u32, phase, at.x, at.y));
        }));
    }
    let mut decl = ClassDecl::new("HalSelectionHandleTarget", class!(NSObject)).unwrap();
    decl.add_ivar::<i64>("kind");
    unsafe {
        decl.add_method(sel!(pan:), pan as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.register()
}

/// The edit menu's delegate: where the menu points (the app's selection).
fn define_menu_delegate() -> *const Class {
    extern "C" fn target_rect(_: &Object, _: Sel, _: ObjcId, _: ObjcId) -> NSRect {
        menu().rect.get()
    }
    let mut decl = ClassDecl::new("HalEditMenuDelegate", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(editMenuInteraction:targetRectForConfiguration:), target_rect as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> NSRect);
    }
    decl.register()
}

// ------------------------------------------------------------------ the window view's actions

fn action_window(this: &Object) -> Option<&'static Win> {
    win_of(this).or_else(|| app().top_window())
}

pub(super) fn add_view_methods(decl: &mut ClassDecl) {
    // Copy and Cut need a selection, Paste text on the pasteboard; Select All always works.
    extern "C" fn can_perform(_: &Object, _: Sel, action: Sel, _: ObjcId) -> BOOL {
        if action == sel!(copy:) || action == sel!(cut:) {
            return if menu().selection.get() { YES } else { NO };
        }
        if action == sel!(paste:) {
            return unsafe {
                let pasteboard: ObjcId = msg_send![class!(UIPasteboard), generalPasteboard];
                msg_send![pasteboard, hasStrings]
            };
        }
        if action == sel!(selectAll:) {
            YES
        } else {
            NO
        }
    }
    extern "C" fn copy(this: &Object, _: Sel, _: ObjcId) {
        if let Some(win) = action_window(this) {
            shielded("copy", || keys::key(win, KeyCode::KeyC, MOD_LOGO, true));
        }
    }
    extern "C" fn cut(this: &Object, _: Sel, _: ObjcId) {
        if let Some(win) = action_window(this) {
            shielded("cut", || keys::key(win, KeyCode::KeyX, MOD_LOGO, true));
        }
    }
    extern "C" fn paste(this: &Object, _: Sel, _: ObjcId) {
        if let Some(win) = action_window(this) {
            shielded("paste", || text::paste(win));
        }
    }
    extern "C" fn select_all(this: &Object, _: Sel, _: ObjcId) {
        if let Some(win) = action_window(this) {
            shielded(
                "select-all",
                std::panic::AssertUnwindSafe(|| {
                    keys::key(win, KeyCode::KeyA, MOD_LOGO, true);
                    keys::key(win, KeyCode::KeyA, MOD_LOGO, false);
                }),
            );
        }
    }
    unsafe {
        decl.add_method(sel!(canPerformAction:withSender:), can_perform as extern "C" fn(&Object, Sel, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(copy:), copy as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(cut:), cut as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(paste:), paste as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(selectAll:), select_all as extern "C" fn(&Object, Sel, ObjcId));
    }
}
