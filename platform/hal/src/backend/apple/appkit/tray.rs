//! The system tray as status items (`TraySet` / `TrayRemove`): one NSStatusItem per app tray in
//! the menu bar, its button showing the app's monochrome glyph as a template image (so it follows
//! the light / dark menu bar), its tooltip, and its NSMenu. A click opens the menu (AppKit has no
//! separate primary action for a status item with a menu); a chosen item goes to the session as
//! `TrayMenu`. Main thread only.
//!
//! `MAKEPAD_TRAY_SELFTEST=1`: after a tray is first made, the shell logs its button frame and
//! menu item count and performs the first enabled menu item itself (`performActionForItemAtIndex:`),
//! as a click would, so a test sees the whole path to the `TrayMenu` event without a user.

use super::app::app;
use crate::types::*;
use makepad_apple_sys::*;
use std::cell::RefCell;

struct Tray {
    sid: u32,
    id: u64,
    item: ObjcId,
    menu: ObjcId,
    /// the menu items' targets (an NSMenuItem does not retain its target)
    targets: Vec<ObjcId>,
}

thread_local! {
    static TRAYS: RefCell<Vec<Tray>> = const { RefCell::new(Vec::new()) };
}

/// `NSVariableStatusItemLength`
const VARIABLE_LENGTH: f64 = -1.0;
const STATE_ON: i64 = 1;
const STATE_OFF: i64 = 0;
/// the menu bar glyph height in points
const GLYPH_POINTS: f64 = 18.0;

pub(super) fn set(sid: u32, id: u64, w: u32, h: u32, alpha: &[u8], tooltip: &str, items: &[(MenuItem, String)]) {
    unsafe {
        let created = !TRAYS.with_borrow(|t| t.iter().any(|t| t.sid == sid && t.id == id));
        if created {
            let bar: ObjcId = msg_send![class!(NSStatusBar), systemStatusBar];
            let item: ObjcId = msg_send![bar, statusItemWithLength: VARIABLE_LENGTH];
            let () = msg_send![item, retain];
            TRAYS.with_borrow_mut(|t| t.push(Tray { sid, id, item, menu: nil, targets: Vec::new() }));
        }
        let item = TRAYS.with_borrow(|t| t.iter().find(|t| t.sid == sid && t.id == id).map(|t| t.item)).unwrap_or(nil);
        if item == nil {
            return;
        }
        let button: ObjcId = msg_send![item, button];
        if button != nil {
            let image = template_image(w, h, alpha);
            let () = msg_send![button, setImage: image];
            if image != nil {
                let () = msg_send![image, release];
            }
            let () = msg_send![button, setToolTip: str_to_nsstring(tooltip)];
        }
        let (menu, targets) = build_menu(sid, id, items);
        let () = msg_send![item, setMenu: menu];
        let old = TRAYS.with_borrow_mut(|t| {
            let t = t.iter_mut().find(|t| t.sid == sid && t.id == id)?;
            Some((std::mem::replace(&mut t.menu, menu), std::mem::replace(&mut t.targets, targets)))
        });
        if let Some((old_menu, old_targets)) = old {
            release(old_menu, &old_targets);
        }
        if created && std::env::var_os("MAKEPAD_TRAY_SELFTEST").is_some() {
            selftest(item, menu);
        }
    }
}

pub(super) fn remove(sid: u32, id: u64) {
    let gone: Vec<Tray> = TRAYS.with_borrow_mut(|t| {
        let (gone, keep) = std::mem::take(t).into_iter().partition(|t| t.sid == sid && t.id == id);
        *t = keep;
        gone
    });
    gone.into_iter().for_each(destroy);
}

pub(super) fn session_closed(sid: u32) {
    let gone: Vec<Tray> = TRAYS.with_borrow_mut(|t| {
        let (gone, keep) = std::mem::take(t).into_iter().partition(|t| t.sid == sid);
        *t = keep;
        gone
    });
    gone.into_iter().for_each(destroy);
}

/// The sessions that have a tray (they are asked to quit like those with windows).
pub(super) fn sessions() -> Vec<u32> {
    TRAYS.with_borrow(|t| t.iter().map(|t| t.sid).collect())
}

/// A tray menu item's target fired.
pub(super) fn action(sid: u32, tray: u64, item: u64) {
    super::Shared::global().push(sid, |q| q.tray_menu(tray, item));
}

fn destroy(tray: Tray) {
    unsafe {
        let bar: ObjcId = msg_send![class!(NSStatusBar), systemStatusBar];
        let () = msg_send![bar, removeStatusItem: tray.item];
        let () = msg_send![tray.item, release];
    }
    release(tray.menu, &tray.targets);
}

fn release(menu: ObjcId, targets: &[ObjcId]) {
    unsafe {
        if menu != nil {
            let () = msg_send![menu, release];
        }
        for target in targets {
            let () = msg_send![*target, release];
        }
    }
}

unsafe fn build_menu(sid: u32, tray: u64, items: &[(MenuItem, String)]) -> (ObjcId, Vec<ObjcId>) {
    let menu: ObjcId = msg_send![class!(NSMenu), new];
    let () = msg_send![menu, setAutoenablesItems: NO];
    let mut targets = Vec::new();
    for (item, label) in items {
        if item.kind & 0xff != menu_kind::ITEM {
            let separator: ObjcId = msg_send![class!(NSMenuItem), separatorItem];
            let () = msg_send![menu, addItem: separator];
            continue;
        }
        let entry: ObjcId = msg_send![menu, addItemWithTitle: str_to_nsstring(label) action: sel!(trayAction:) keyEquivalent: str_to_nsstring("")];
        let enabled = item.kind & menu_kind::DISABLED == 0;
        let () = msg_send![entry, setEnabled: if enabled { YES } else { NO }];
        if item.kind & menu_kind::CHECKABLE != 0 {
            let state = if item.kind & menu_kind::CHECKED != 0 { STATE_ON } else { STATE_OFF };
            let () = msg_send![entry, setState: state];
        }
        let target: ObjcId = msg_send![app().classes().tray_target, new];
        (*target).set_ivar("sid", sid as usize);
        (*target).set_ivar("tray_u64", tray as usize);
        (*target).set_ivar("item_u64", item.cmd as usize);
        let () = msg_send![entry, setTarget: target];
        targets.push(target);
    }
    (menu, targets)
}

/// The glyph as a template NSImage (black with the app's alpha; AppKit tints templates),
/// `GLYPH_POINTS` high. Retained (+1); nil for an empty glyph.
unsafe fn template_image(w: u32, h: u32, alpha: &[u8]) -> ObjcId {
    let (width, height) = (w as usize, h as usize);
    if width == 0 || height == 0 || alpha.len() < width * height {
        return nil;
    }
    let rep: ObjcId = msg_send![class!(NSBitmapImageRep), alloc];
    let rep: ObjcId = msg_send![rep,
        initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>()
        pixelsWide: width as i64
        pixelsHigh: height as i64
        bitsPerSample: 8i64
        samplesPerPixel: 4i64
        hasAlpha: YES
        isPlanar: NO
        colorSpaceName: str_to_nsstring("NSDeviceRGBColorSpace")
        bytesPerRow: (width * 4) as i64
        bitsPerPixel: 32i64
    ];
    if rep == nil {
        return nil;
    }
    let data: *mut u8 = msg_send![rep, bitmapData];
    if !data.is_null() {
        // premultiplied black: only the alpha channel carries the glyph
        let out = std::slice::from_raw_parts_mut(data, width * height * 4);
        for (px, a) in out.chunks_exact_mut(4).zip(alpha) {
            px.copy_from_slice(&[0, 0, 0, *a]);
        }
    }
    let size = NSSize { width: GLYPH_POINTS * width as f64 / height as f64, height: GLYPH_POINTS };
    let () = msg_send![rep, setSize: size];
    let image: ObjcId = msg_send![class!(NSImage), alloc];
    let image: ObjcId = msg_send![image, initWithSize: size];
    let () = msg_send![image, addRepresentation: rep];
    let () = msg_send![rep, release];
    let () = msg_send![image, setTemplate: YES];
    image
}

unsafe fn selftest(item: ObjcId, menu: ObjcId) {
    let button: ObjcId = msg_send![item, button];
    let frame: NSRect = if button != nil { msg_send![button, frame] } else { NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: 0.0, height: 0.0 } } };
    let count: i64 = msg_send![menu, numberOfItems];
    eprintln!(
        "tray selftest: status item button frame {}x{} at ({}, {}), {count} menu items",
        frame.size.width, frame.size.height, frame.origin.x, frame.origin.y
    );
    for index in 0..count {
        let entry: ObjcId = msg_send![menu, itemAtIndex: index];
        let separator: BOOL = msg_send![entry, isSeparatorItem];
        let enabled: BOOL = msg_send![entry, isEnabled];
        if separator == NO && enabled != NO {
            eprintln!("tray selftest: performing menu item {index}");
            let () = msg_send![menu, performActionForItemAtIndex: index];
            return;
        }
    }
}
