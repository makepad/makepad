//! OS notifications through UserNotifications (`NotifyPost` / `NotifyRemove`). The center and
//! session routes live on the main thread; delegate responses and authorization replies return
//! there before using them. A click answers `NotifyClick` for the session that posted the id.
//!
//! `MAKEPAD_NOTIFY_SELFTEST=1`: every post and removal is logged. One second after the first
//! post, a synthetic click takes the delegate's path to the app, even without an .app bundle.

use makepad_apple_sys::*;
use std::{cell::RefCell, collections::HashMap};

#[link(name = "UserNotifications", kind = "framework")]
extern "C" {}

#[derive(Default)]
struct Notifications {
    sessions: HashMap<u64, u32>,
    last_session: Option<u32>,
    /// A retained request's address distinguishes authorization replies after replacement.
    pending: HashMap<u64, usize>,
    bundle: Option<bool>,
    delegate: ObjcId,
    center: ObjcId,
    selftest_started: bool,
}

thread_local! {
    static NOTIFICATIONS: RefCell<Notifications> = RefCell::new(Notifications::default());
}

fn bundled() -> bool {
    NOTIFICATIONS.with_borrow_mut(|n| *n.bundle.get_or_insert_with(|| unsafe {
        let bundle: ObjcId = msg_send![class!(NSBundle), mainBundle];
        let identifier: ObjcId = msg_send![bundle, bundleIdentifier];
        let path: ObjcId = msg_send![bundle, bundlePath];
        let valid = identifier != nil && path != nil && nsstring_to_string(path).ends_with(".app");
        if !valid {
            eprintln!("notify: dropped: this process runs from no .app bundle with a bundle identifier (UNUserNotificationCenter needs one)");
        }
        valid
    }))
}

fn delegate() -> ObjcId {
    NOTIFICATIONS.with_borrow_mut(|n| {
        if n.delegate == nil {
            unsafe { n.delegate = msg_send![define_delegate(), new]; }
        }
        n.delegate
    })
}

fn center() -> ObjcId {
    let delegate = delegate();
    NOTIFICATIONS.with_borrow_mut(|n| unsafe {
        if n.center == nil {
            let center: ObjcId = msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
            n.center = msg_send![center, retain];
            // The center's delegate is weak, so its one instance stays owned for the process.
            let () = msg_send![center, setDelegate: delegate];
        }
        n.center
    })
}

fn define_delegate() -> *const Class {
    extern "C" fn response(_: &Object, _: Sel, _: ObjcId, response: ObjcId, completion: ObjcId) {
        unsafe {
            let notification: ObjcId = msg_send![response, notification];
            let request: ObjcId = msg_send![notification, request];
            let identifier: ObjcId = msg_send![request, identifier];
            let id = if identifier == nil { None } else { nsstring_to_string(identifier).parse::<u64>().ok() };
            let completion: ObjcId = msg_send![completion, copy];
            let completion = completion as usize;
            super::app::run_on_main(move || {
                if let Some(id) = id {
                    click(id);
                }
                objc_block_invoke!(completion as ObjcId, invoke());
                let () = msg_send![completion as ObjcId, release];
            });
        }
    }

    extern "C" fn present(_: &Object, _: Sel, _: ObjcId, _: ObjcId, completion: ObjcId) {
        unsafe { objc_block_invoke!(completion, invoke((16 | 8): usize)); }
    }

    extern "C" fn selftest_click(_: &Object, _: Sel, identifier: ObjcId) {
        if let Ok(id) = nsstring_to_string(identifier).parse::<u64>() {
            eprintln!("notify selftest: synthetic click {id}");
            click(id);
        }
    }

    let mut decl = ClassDecl::new("HalNotificationDelegate", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:), response as extern "C" fn(&Object, Sel, ObjcId, ObjcId, ObjcId));
        decl.add_method(sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:), present as extern "C" fn(&Object, Sel, ObjcId, ObjcId, ObjcId));
        decl.add_method(sel!(notificationSelftest:), selftest_click as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.register()
}

fn click(id: u64) {
    let sid = NOTIFICATIONS.with_borrow(|n| n.sessions.get(&id).copied().or(n.last_session));
    if let Some(sid) = sid {
        super::Shared::global().push(sid, |q| q.notify_click(id));
    } else {
        eprintln!("notify: dropped click {id}: no session has posted a notification");
    }
}

fn selftest() -> bool {
    relax_api::env::var("MAKEPAD_NOTIFY_SELFTEST").is_ok_and(|v| v == "1")
}

pub(super) fn post(sid: u32, id: u64, title: &str, body: &str) {
    NOTIFICATIONS.with_borrow_mut(|n| {
        n.sessions.insert(id, sid);
        n.last_session = Some(sid);
    });
    let bundle = bundled();
    if selftest() {
        eprintln!("notify selftest: post {id} bundle {}", if bundle { "yes" } else { "no" });
        if NOTIFICATIONS.with_borrow_mut(|n| !std::mem::replace(&mut n.selftest_started, true)) {
            unsafe {
                let target = delegate();
                // The delayed selector retains its target and argument until it fires
                // (str_to_nsstring's strings are autoreleased: never released here).
                let arg = str_to_nsstring(&id.to_string());
                let () = msg_send![target, performSelector: sel!(notificationSelftest:) withObject: arg afterDelay: 1.0f64];
            }
        }
    }
    if !bundle {
        return;
    }
    unsafe {
        let center = center();
        let content: ObjcId = msg_send![class!(UNMutableNotificationContent), new];
        // (str_to_nsstring's strings are autoreleased; the content and the request copy them)
        let (title, body, identifier) = (str_to_nsstring(title), str_to_nsstring(body), str_to_nsstring(&id.to_string()));
        let () = msg_send![content, setTitle: title];
        let () = msg_send![content, setBody: body];
        let sound: ObjcId = msg_send![class!(UNNotificationSound), defaultSound];
        let () = msg_send![content, setSound: sound];
        let request: ObjcId = msg_send![class!(UNNotificationRequest), requestWithIdentifier: identifier content: content trigger: nil];
        let () = msg_send![content, release];
        let request: ObjcId = msg_send![request, retain];
        let retained_center: ObjcId = msg_send![center, retain];
        let (request, retained_center) = (request as usize, retained_center as usize);
        NOTIFICATIONS.with_borrow_mut(|n| n.pending.insert(id, request));
        let authorized = objc_block!(move |granted: BOOL, error: ObjcId| {
            if error != nil {
                let description: ObjcId = msg_send![error, localizedDescription];
                eprintln!("notify: authorization failed: {}", nsstring_to_string(description));
            }
            // (a refusal with an error said why above; a post is one per waiting question, so rare)
            if granted == NO && error == nil {
                eprintln!("notify: the person has not allowed notifications for this app");
            }
            super::app::run_on_main(move || {
                // Removing or replacing a post while permission was pending cancels this request.
                let current = NOTIFICATIONS.with_borrow_mut(|n| {
                    if n.pending.get(&id) != Some(&request) {
                        return false;
                    }
                    n.pending.remove(&id);
                    true
                });
                if granted != NO && current {
                    let added = objc_block!(move |error: ObjcId| {
                        if error != nil {
                            let description: ObjcId = msg_send![error, localizedDescription];
                            eprintln!("notify: post failed: {}", nsstring_to_string(description));
                        }
                    });
                    let () = msg_send![retained_center as ObjcId, addNotificationRequest: request as ObjcId withCompletionHandler: &added];
                }
                let () = msg_send![request as ObjcId, release];
                let () = msg_send![retained_center as ObjcId, release];
            });
        });
        let () = msg_send![center, requestAuthorizationWithOptions: (4 | 2) as usize completionHandler: &authorized];
    }
}

pub(super) fn remove(_sid: u32, id: u64) {
    if selftest() {
        eprintln!("notify selftest: remove {id}");
    }
    NOTIFICATIONS.with_borrow_mut(|n| {
        n.sessions.remove(&id);
        n.pending.remove(&id);
    });
    if !bundled() {
        return;
    }
    unsafe {
        let center = center();
        let identifier = str_to_nsstring(&id.to_string());
        let identifiers: ObjcId = msg_send![class!(NSArray), arrayWithObject: identifier];
        let () = msg_send![center, removeDeliveredNotificationsWithIdentifiers: identifiers];
        let () = msg_send![center, removePendingNotificationRequestsWithIdentifiers: identifiers];
    }
}
