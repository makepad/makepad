//! Exercise the native cursor loaders without a window or changing the user's
//! cursor. Hidden-window tests do not reliably invoke AppKit resetCursorRects.

#[cfg(all(target_os = "macos", not(gpusim)))]
fn main() {
    use makepad_platform::{
        os::apple::{apple_sys::*, apple_util::*},
        MouseCursor,
    };

    unsafe {
        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
        let app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let _: BOOL = msg_send![app, setActivationPolicy: 2i64];
        let cursor_class = class!(NSCursor);
        let assert_cursor = |id: ObjcId| {
            assert!(!id.is_null());
            let valid: BOOL = msg_send![id, isKindOfClass: cursor_class];
            assert_eq!(valid, YES);
            // The cursor cache retains these returned objects before AppKit
            // uses them, so exercise that ownership boundary as well.
            let retained: ObjcId = msg_send![id, retain];
            let _: NSPoint = msg_send![retained, hotSpot];
            let _: () = msg_send![retained, release];
        };
        // Both supported and missing selector paths previously passed a BOOL
        // as a selector and could terminate the process with SIGSEGV.
        assert_cursor(load_undocumented_cursor("arrowCursor"));
        assert_cursor(load_undocumented_cursor(
            "_makepadMissingCursorForRegression",
        ));
        for cursor in [
            MouseCursor::Default,
            MouseCursor::Text,
            MouseCursor::Hand,
            MouseCursor::Grab,
            MouseCursor::Grabbing,
            MouseCursor::Move,
            MouseCursor::NResize,
            MouseCursor::SResize,
            MouseCursor::EResize,
            MouseCursor::WResize,
            MouseCursor::NeResize,
            MouseCursor::NwResize,
            MouseCursor::SeResize,
            MouseCursor::SwResize,
            MouseCursor::NeswResize,
            MouseCursor::NwseResize,
            MouseCursor::ColResize,
            MouseCursor::RowResize,
            MouseCursor::Help,
            MouseCursor::Wait,
        ] {
            assert_cursor(load_mouse_cursor(cursor));
        }
        let _: () = msg_send![pool, drain];
    }
    println!("Native cursor regression passed (supported/missing selectors and 20 cursor kinds)");
}

#[cfg(not(all(target_os = "macos", not(gpusim))))]
fn main() {}
