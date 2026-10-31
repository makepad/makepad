//! WKWebView's content state and responder ownership, delivered through the shell inbox.
use crate::{native_layer::state, types::Id};
use makepad_apple_sys::*;

fn emit(web: &Object, what: u32, a: u64, b: u64, data: &[u8]) {
    unsafe {
        let sid: u32 = *web.get_ivar("native_sid");
        let id = Id::new(*web.get_ivar("native_index"), *web.get_ivar("native_gen"));
        super::super::shell::Shared::global().push(sid, |q| q.native_layer_state(id, what, a, b, data));
    }
}

pub(super) fn class() -> &'static Class {
    static CLASS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let ptr = *CLASS.get_or_init(|| {
        let mut decl = ClassDecl::new("HalNativeWebView", class!(WKWebView)).unwrap();
        for field in ["native_sid", "native_index", "native_gen", "native_observing"] { decl.add_ivar::<u32>(field); }
        extern "C" fn observe(this: &Object, _: Sel, key: ObjcId, _: ObjcId, _: ObjcId, _: *mut c_void) {
            unsafe {
                match nsstring_to_string(key).as_str() {
                    "title" => {
                        let text: ObjcId = msg_send![this, title];
                        let text = if text == nil { String::new() } else { nsstring_to_string(text) };
                        emit(this, state::TITLE, 0, 0, text.as_bytes());
                    }
                    "URL" => {
                        let url: ObjcId = msg_send![this, URL];
                        let text: ObjcId = msg_send![url, absoluteString];
                        let text = if text == nil { String::new() } else { nsstring_to_string(text) };
                        emit(this, state::URL, 0, 0, text.as_bytes());
                    }
                    "loading" => {
                        let loading: BOOL = msg_send![this, isLoading];
                        emit(this, state::LOADING, (loading != NO) as u64, 0, b"");
                    }
                    "canGoBack" | "canGoForward" => {
                        let back: BOOL = msg_send![this, canGoBack];
                        let forward: BOOL = msg_send![this, canGoForward];
                        emit(this, state::NAV, (back != NO) as u64, (forward != NO) as u64, b"");
                    }
                    _ => {}
                }
            }
        }
        #[cfg(target_os = "ios")]
        extern "C" fn became_responder(this: &Object, _: Sel) -> BOOL {
            let ok: BOOL = unsafe { msg_send![super(this, class!(WKWebView)), becomeFirstResponder] };
            if ok != NO { emit(this, state::FOCUS_IN, 0, 0, b""); }
            ok
        }
        #[cfg(target_os = "ios")]
        extern "C" fn resign(this: &Object, _: Sel) -> BOOL {
            let ok: BOOL = unsafe { msg_send![super(this, class!(WKWebView)), resignFirstResponder] };
            if ok != NO { emit(this, state::FOCUS_OUT, 0, 0, b""); }
            ok
        }
        unsafe {
            decl.add_method(sel!(observeValueForKeyPath:ofObject:change:context:), observe as extern "C" fn(&Object, Sel, ObjcId, ObjcId, ObjcId, *mut c_void));
            #[cfg(target_os = "ios")]
            {
                decl.add_method(sel!(becomeFirstResponder), became_responder as extern "C" fn(&Object, Sel) -> BOOL);
                decl.add_method(sel!(resignFirstResponder), resign as extern "C" fn(&Object, Sel) -> BOOL);
            }
        }
        decl.register() as *const Class as usize
    });
    unsafe { &*(ptr as *const Class) }
}

const KEYS: [&str; 5] = ["title", "URL", "loading", "canGoBack", "canGoForward"];
pub(super) fn watch(web: ObjcId, sid: u32, id: Id) {
    unsafe {
        (*web).set_ivar("native_sid", sid);
        (*web).set_ivar("native_index", id.index);
        (*web).set_ivar("native_gen", id.gen);
        (*web).set_ivar("native_observing", 1u32);
        for key in KEYS {
            let () = msg_send![web, addObserver: web forKeyPath: str_to_nsstring(key) options: 5u64 context: std::ptr::null_mut::<c_void>()];
        }
    }
}
pub(super) fn unwatch(web: ObjcId) {
    unsafe {
        if *(*web).get_ivar::<u32>("native_observing") == 0 { return; }
        (*web).set_ivar("native_observing", 0u32);
        for key in KEYS { let () = msg_send![web, removeObserver: web forKeyPath: str_to_nsstring(key)]; }
    }
}

#[cfg(target_os = "macos")]
unsafe fn owner(mut responder: ObjcId) -> ObjcId {
    while responder != nil {
        let web: BOOL = msg_send![responder, isKindOfClass: class()];
        if web != NO { return responder; }
        let has_parent: BOOL = msg_send![responder, respondsToSelector: sel!(superview)];
        if has_parent == NO { break; }
        responder = msg_send![responder, superview];
    }
    nil
}
#[cfg(target_os = "macos")]
pub(crate) fn window_owns_focus(window: ObjcId) -> bool {
    unsafe { owner(msg_send![window, firstResponder]) != nil }
}

#[cfg(target_os = "macos")]
pub(super) fn is_focused(web: ObjcId) -> bool {
    unsafe { let window: ObjcId = msg_send![web, window]; owner(msg_send![window, firstResponder]) == web }
}

/// AppKit makes a private descendant of WKWebView first responder. Observe the window's
/// successful transition, including keyboard traversal, instead of only explicit focus ops.
#[cfg(target_os = "macos")]
pub(crate) extern "C" fn make_first_responder(this: &Object, _: Sel, next: ObjcId) -> BOOL {
    unsafe {
        let before = owner(msg_send![this, firstResponder]);
        let base: ObjcId = msg_send![this, superclass];
        let ok: BOOL = msg_send![super(this, &*(base as *const Class)), makeFirstResponder: next];
        let after = owner(msg_send![this, firstResponder]);
        if ok != NO && before != after {
            if before != nil { emit(&*before, state::FOCUS_OUT, 0, 0, b""); }
            if after != nil { emit(&*after, state::FOCUS_IN, 0, 0, b""); }
        }
        ok
    }
}
