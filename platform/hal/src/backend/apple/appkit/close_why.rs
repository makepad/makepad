//! Why a window closes or the app is asked to quit, in the log as it happens. An app's only
//! window closed twice under the person's hands (11-06, "who killed the window"), and the log
//! said only that the last window closed. Each close and quit request names what AppKit is
//! handling then: a key equivalent and its modifiers, a click (on the window's close button or
//! elsewhere), an Apple Event and the process that sent it, or none (a call from code:
//! `performClose:`, `terminate:`). A key or a click says where it came from: the hardware (the
//! window server's HID stream), or the process that posted it (a remote-desktop viewer, a script:
//! four apps closed within 40 s on clicks on their close buttons, 11-06, and the HAL could not
//! tell a person's from a posted one). All of it is read in-process; nothing is sent to another
//! app.

use makepad_apple_sys::*;

/// The NSEvent AppKit handles now, and the Apple Event it answers (if any).
pub(super) fn cause() -> String {
    unsafe {
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let event: ObjcId = msg_send![ns_app, currentEvent];
        let mut out = if event == nil { "no event (a call from code)".to_string() } else { describe_event(event) };
        out.push_str(&apple_event());
        out
    }
}

/// What sent `terminate:`: a menu item (its title), or the object's class.
pub(super) fn sender(obj: ObjcId) -> String {
    unsafe {
        if obj == nil {
            return "none".into();
        }
        let class: ObjcId = msg_send![obj, className];
        let is_item: bool = msg_send![obj, isKindOfClass: class!(NSMenuItem)];
        if is_item {
            let title: ObjcId = msg_send![obj, title];
            return format!("menu item {:?}", text(title));
        }
        text(class)
    }
}

/// An NSString's text ("" for nil).
fn text(s: ObjcId) -> String {
    if s == nil { String::new() } else { nsstring_to_string(s) }
}

unsafe fn describe_event(event: ObjcId) -> String {
    let kind: u64 = msg_send![event, type];
    let flags: u64 = msg_send![event, modifierFlags];
    let mods: String = [(1u64 << 20, "cmd"), (1 << 18, "ctrl"), (1 << 19, "option"), (1 << 17, "shift")]
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, name)| format!(" +{name}"))
        .collect();
    match kind {
        // key down / up
        10 | 11 => {
            let chars: ObjcId = msg_send![event, charactersIgnoringModifiers];
            let code: u16 = msg_send![event, keyCode];
            format!("key {} {:?} (code {code}){mods}, {}", if kind == 10 { "down" } else { "up" }, text(chars), source(event))
        }
        // the mouse buttons' downs, ups and drags
        1..=4 | 6 | 7 | 25..=27 => {
            let window: ObjcId = msg_send![event, window];
            let at: NSPoint = msg_send![event, locationInWindow];
            let place = if window == nil {
                "outside the app's windows".to_string()
            } else {
                let number: isize = msg_send![window, windowNumber];
                let close: ObjcId = msg_send![window, standardWindowButton: 0u64];
                let on_close = close != nil && {
                    let p: NSPoint = msg_send![close, convertPoint: at fromView: nil];
                    let b: NSRect = msg_send![close, bounds];
                    p.x >= b.origin.x && p.y >= b.origin.y && p.x < b.origin.x + b.size.width && p.y < b.origin.y + b.size.height
                };
                format!("window {number} at ({:.0}, {:.0}){}", at.x, at.y, if on_close { ", on its close button" } else { "" })
            };
            format!("mouse event {kind} in {place}{mods}, {}", source(event))
        }
        k => format!("event type {k}{mods}"),
    }
}

/// Where an input event came from, as its Quartz event says: `hardware` (the window server's HID
/// stream: no process made it), or `from pid P <name>, from <its parents>` for one a process made
/// and posted (`CGEventPost`, `CGEventPostToPid`, an `NSEvent` it queued: each carries its maker's
/// pid; `from this app itself, pid P ...` for its own), with its source's state and user data.
unsafe fn source(event: ObjcId) -> String {
    let cg: ObjcId = msg_send![event, CGEvent];
    if cg == nil {
        return "its source not known (no Quartz event)".into();
    }
    let pid = CGEventGetIntegerValueField(cg, kCGEventSourceUnixProcessID);
    let state = CGEventGetIntegerValueField(cg, kCGEventSourceStateID);
    if pid == 0 {
        return format!("hardware (source state {state})");
    }
    let data = CGEventGetIntegerValueField(cg, kCGEventSourceUserData);
    let own = if pid == std::process::id() as i64 { "this app itself, " } else { "" };
    format!("from {own}{} (source state {state}, user data {data})", makepad_remote::procs::said(pid as u32))
}

/// `, Apple Event 'aevtquit' from pid N (name)` while AppKit answers one (the Dock's Quit, a
/// logout, a script), else nothing.
unsafe fn apple_event() -> String {
    let manager: ObjcId = msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
    let event: ObjcId = msg_send![manager, currentAppleEvent];
    if event == nil {
        return String::new();
    }
    let class: u32 = msg_send![event, eventClass];
    let id: u32 = msg_send![event, eventID];
    // keySenderPIDAttr
    let pid_desc: ObjcId = msg_send![event, attributeDescriptorForKeyword: u32::from_be_bytes(*b"spid")];
    let pid: i32 = if pid_desc == nil { 0 } else { msg_send![pid_desc, int32Value] };
    let app: ObjcId = if pid > 0 { msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid] } else { nil };
    let name = if app == nil {
        String::new()
    } else {
        let name: ObjcId = msg_send![app, localizedName];
        format!(" ({})", text(name))
    };
    let code = |c: u32| String::from_utf8_lossy(&c.to_be_bytes()).into_owned();
    format!(", Apple Event '{}{}' from pid {pid}{name}", code(class), code(id))
}
