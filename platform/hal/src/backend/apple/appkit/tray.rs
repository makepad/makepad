//! The system tray as status items (`TraySet` / `TrayRemove`): one NSStatusItem per app tray in
//! the menu bar, its button showing the app's monochrome glyph as a template image (so it follows
//! the light / dark menu bar), its tooltip, and its NSMenu. A click opens the menu (AppKit has no
//! separate primary action for a status item with a menu); a chosen item goes to the session as
//! `TrayMenu`. A `tray_flags::DROPDOWN` tray has no menu on its item: the button's own action
//! sends a primary click as `TrayClick` (with the button's rect and its screen's visible frame)
//! and opens the menu on a right click or Ctrl-click. `WindowAtTray` (`window_at`) puts a
//! dropdown window under the button. Main thread only.
//!
//! `MAKEPAD_TRAY_SELFTEST=1`: after a tray is first made, the shell logs its button frame and
//! menu item count and performs the first enabled menu item itself (`performActionForItemAtIndex:`),
//! as a click would, so a test sees the whole path to the `TrayMenu` event without a user. A
//! dropdown tray instead gets a primary click, and the first `WindowAtTray` logs the dropdown's
//! frame against the button's; with `MAKEPAD_TRAY_SELFTEST_ITEM=<label>` (and
//! `MAKEPAD_TRAY_SELFTEST_DELAY=<s>`, default 20) the menu item with that label is chosen that
//! long after, through its own target, as a click on it would. `MAKEPAD_TRAY_SELFTEST_CLICKS=<s>,..`
//! replaces the immediate click by the button's own action fired at those delays.

use super::app::app;
use super::window::Win;
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
    /// `tray_flags::DROPDOWN`: the button's target (a `HalTrayTarget` for `trayClick:`)
    click_target: ObjcId,
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
/// the space between the menu bar and a dropdown, points
const DROPDOWN_GAP: f64 = 4.0;
/// `NSEventMaskLeftMouseDown | NSEventMaskRightMouseDown`
const CLICK_MASK: u64 = (1 << 1) | (1 << 3);
/// `NSEventTypeRightMouseDown`, `NSEventModifierFlagControl`
const RIGHT_MOUSE_DOWN: u64 = 3;
const CONTROL_FLAG: u64 = 1 << 18;

pub(super) fn set(sid: u32, id: u64, w: u32, h: u32, flags: u32, alpha: &[u8], tooltip: &str, items: &[(MenuItem, String)]) {
    unsafe {
        let created = !TRAYS.with_borrow(|t| t.iter().any(|t| t.sid == sid && t.id == id));
        if created {
            let bar: ObjcId = msg_send![class!(NSStatusBar), systemStatusBar];
            let item: ObjcId = msg_send![bar, statusItemWithLength: VARIABLE_LENGTH];
            let () = msg_send![item, retain];
            TRAYS.with_borrow_mut(|t| t.push(Tray { sid, id, item, menu: nil, targets: Vec::new(), click_target: nil }));
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
        let dropdown = flags & tray_flags::DROPDOWN != 0;
        // a dropdown tray's button acts itself; the menu is set only while it opens
        let click_target = if dropdown && button != nil {
            let target: ObjcId = msg_send![app().classes().tray_target, new];
            (*target).set_ivar("sid", sid as usize);
            (*target).set_ivar("tray_u64", id as usize);
            (*target).set_ivar("item_u64", 0usize);
            let () = msg_send![button, setTarget: target];
            let () = msg_send![button, setAction: sel!(trayClick:)];
            let _: i64 = msg_send![button, sendActionOn: CLICK_MASK];
            target
        } else {
            if button != nil {
                // the old target is released below
                let () = msg_send![button, setTarget: nil];
            }
            nil
        };
        let () = msg_send![item, setMenu: if dropdown { nil } else { menu }];
        let old = TRAYS.with_borrow_mut(|t| {
            let t = t.iter_mut().find(|t| t.sid == sid && t.id == id)?;
            Some((std::mem::replace(&mut t.menu, menu), std::mem::replace(&mut t.targets, targets), std::mem::replace(&mut t.click_target, click_target)))
        });
        if let Some((old_menu, old_targets, old_click)) = old {
            release(old_menu, &old_targets);
            release(nil, &[old_click]);
        }
        if created && selftest_on() {
            eprintln!("tray selftest: activation policy {}", super::app::activation_policy_name());
            if dropdown {
                if !selftest_clicks(click_target, button) {
                    selftest_click(sid, id, item);
                }
                selftest_later(menu);
            } else {
                selftest(item, menu);
            }
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

/// A dropdown tray's button was pressed: the menu on a right click or Ctrl-click, else a
/// `TrayClick` with where the button is.
pub(super) fn click(sid: u32, tray: u64) {
    let Some((item, menu)) = TRAYS.with_borrow(|t| t.iter().find(|t| t.sid == sid && t.id == tray).map(|t| (t.item, t.menu))) else { return };
    unsafe {
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let event: ObjcId = msg_send![ns_app, currentEvent];
        let (kind, mods): (u64, u64) = if event != nil { (msg_send![event, type], msg_send![event, modifierFlags]) } else { (0, 0) };
        if kind == RIGHT_MOUSE_DOWN || mods & CONTROL_FLAG != 0 {
            // the menu opens under the item as a menu tray's does, then the item is ours again
            let button: ObjcId = msg_send![item, button];
            let () = msg_send![item, setMenu: menu];
            let () = msg_send![button, performClick: nil];
            let () = msg_send![item, setMenu: nil];
            return;
        }
        push_click(sid, tray, item);
    }
}

unsafe fn push_click(sid: u32, tray: u64, item: ObjcId) {
    let (anchor, visible) = button_place(item);
    let primary = primary_height();
    let (a, screen) = (flip(anchor, primary), flip(visible, primary));
    super::Shared::global().push(sid, |q| q.tray_click(tray, a[0] as f64, a[1] as f64, a[2] as f64, a[3] as f64, screen));
}

/// The button's rect on screen and its screen's visible frame, AppKit coordinates (y up).
unsafe fn button_place(item: ObjcId) -> (NSRect, NSRect) {
    let zero = NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: 0.0, height: 0.0 } };
    let button: ObjcId = msg_send![item, button];
    if button == nil {
        return (zero, zero);
    }
    let window: ObjcId = msg_send![button, window];
    if window == nil {
        return (zero, zero);
    }
    let bounds: NSRect = msg_send![button, bounds];
    let in_window: NSRect = msg_send![button, convertRect: bounds toView: nil];
    let on_screen: NSRect = msg_send![window, convertRectToScreen: in_window];
    let mut screen: ObjcId = msg_send![window, screen];
    if screen == nil {
        screen = msg_send![class!(NSScreen), mainScreen];
    }
    let visible: NSRect = if screen != nil { msg_send![screen, visibleFrame] } else { zero };
    (on_screen, visible)
}

/// The primary screen's height: AppKit's y-up screen space flips about it.
unsafe fn primary_height() -> f64 {
    let screens: ObjcId = msg_send![class!(NSScreen), screens];
    let count: u64 = if screens != nil { msg_send![screens, count] } else { 0 };
    if count == 0 {
        return 0.0;
    }
    let first: ObjcId = msg_send![screens, objectAtIndex: 0u64];
    let frame: NSRect = msg_send![first, frame];
    frame.size.height
}

/// x, y, w, h from the primary screen's top left, y down.
fn flip(r: NSRect, primary: f64) -> [f32; 4] {
    [r.origin.x as f32, (primary - r.origin.y - r.size.height) as f32, r.size.width as f32, r.size.height as f32]
}

/// `WindowAtTray`: dropdown `win` becomes `w` x `h` points, under tray `tray`'s button (centred
/// on it, fitted into its screen's visible frame), in front and key. Without that tray it goes
/// under the menu bar of the main screen, centred.
pub(super) fn window_at(sid: u32, win: &Win, tray: u64, w: f32, h: f32) {
    unsafe {
        let item = TRAYS.with_borrow(|t| t.iter().find(|t| t.sid == sid && t.id == tray).map(|t| t.item));
        let (anchor, mut visible) = match item {
            Some(item) => button_place(item),
            None => {
                let zero = NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: 0.0, height: 0.0 } };
                (zero, zero)
            }
        };
        if visible.size.width <= 0.0 {
            let screen: ObjcId = msg_send![class!(NSScreen), mainScreen];
            if screen != nil {
                visible = msg_send![screen, visibleFrame];
            }
        }
        let (vx, vy, vw, vh) = (visible.origin.x, visible.origin.y, visible.size.width, visible.size.height);
        let width = (w as f64).min((vw - 2.0 * DROPDOWN_GAP).max(1.0));
        let height = (h as f64).min((vh - 2.0 * DROPDOWN_GAP).max(1.0));
        let centre = if anchor.size.width > 0.0 { anchor.origin.x + anchor.size.width * 0.5 } else { vx + vw * 0.5 };
        let x = (centre - width * 0.5).clamp(vx + DROPDOWN_GAP, (vx + vw - width - DROPDOWN_GAP).max(vx + DROPDOWN_GAP));
        // under the menu bar: the top of the visible frame (the button's bottom edge is the
        // menu bar's), the gap below it
        let top = if anchor.size.height > 0.0 { anchor.origin.y.min(vy + vh) } else { vy + vh };
        let y = (top - DROPDOWN_GAP - height).max(vy + DROPDOWN_GAP);
        let frame = NSRect { origin: NSPoint { x, y }, size: NSSize { width, height } };
        let () = msg_send![win.window, setFrame: frame display: YES];
        if super::window::focus_allowed() {
            // a non-activating panel: key without activating the app (`init_dropdown`)
            let () = msg_send![win.window, makeKeyAndOrderFront: nil];
        } else {
            let () = msg_send![win.window, orderFrontRegardless];
        }
        if selftest_on() {
            let key: BOOL = msg_send![win.window, isKeyWindow];
            let primary = primary_height();
            let (a, f) = (flip(anchor, primary), flip(frame, primary));
            eprintln!(
                "tray selftest: dropdown frame {}x{} at ({}, {}), status item {}x{} at ({}, {}): {} pt below it, centre offset {} pt, key {}",
                f[2], f[3], f[0], f[1], a[2], a[3], a[0], a[1],
                f[1] - (a[1] + a[3]),
                (f[0] + f[2] * 0.5) - (a[0] + a[2] * 0.5),
                key != NO
            );
        }
    }
    win.send_geom();
}

fn selftest_on() -> bool {
    std::env::var_os("MAKEPAD_TRAY_SELFTEST").is_some()
}

fn destroy(tray: Tray) {
    unsafe {
        let bar: ObjcId = msg_send![class!(NSStatusBar), systemStatusBar];
        let () = msg_send![bar, removeStatusItem: tray.item];
        let () = msg_send![tray.item, release];
    }
    release(tray.menu, &tray.targets);
    release(nil, &[tray.click_target]);
}

fn release(menu: ObjcId, targets: &[ObjcId]) {
    unsafe {
        if menu != nil {
            let () = msg_send![menu, release];
        }
        for target in targets.iter().filter(|t| **t != nil) {
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

/// A dropdown tray's selftest: a primary click as the button's action would send it.
unsafe fn selftest_click(sid: u32, tray: u64, item: ObjcId) {
    let (anchor, visible) = button_place(item);
    eprintln!(
        "tray selftest: dropdown status item button {}x{} at ({}, {}) on a screen with visible frame {}x{} at ({}, {}); performing a primary click",
        anchor.size.width, anchor.size.height, anchor.origin.x, anchor.origin.y, visible.size.width, visible.size.height, visible.origin.x, visible.origin.y
    );
    push_click(sid, tray, item);
}

/// `MAKEPAD_TRAY_SELFTEST_CLICKS=<s>,<s>,..`: instead of the immediate click, the button's own
/// action (`trayClick:`, what a real click runs) fires that many seconds after the tray was made,
/// once per entry, so a click arrives long after startup like a user's. False when not set.
unsafe fn selftest_clicks(target: ObjcId, button: ObjcId) -> bool {
    let Ok(list) = std::env::var("MAKEPAD_TRAY_SELFTEST_CLICKS") else { return false };
    if target == nil || button == nil {
        return false;
    }
    for delay in list.split(',').filter_map(|d| d.trim().parse::<f64>().ok()) {
        eprintln!("tray selftest: button click in {delay} s");
        let () = msg_send![target, performSelector: sel!(trayClick:) withObject: button afterDelay: delay];
    }
    true
}

/// `MAKEPAD_TRAY_SELFTEST_ITEM`: the item with that label is chosen after the delay.
unsafe fn selftest_later(menu: ObjcId) {
    let Ok(label) = std::env::var("MAKEPAD_TRAY_SELFTEST_ITEM") else { return };
    let delay: f64 = std::env::var("MAKEPAD_TRAY_SELFTEST_DELAY").ok().and_then(|d| d.trim().parse().ok()).unwrap_or(20.0);
    let count: i64 = msg_send![menu, numberOfItems];
    for index in 0..count {
        let entry: ObjcId = msg_send![menu, itemAtIndex: index];
        let title: ObjcId = msg_send![entry, title];
        if title != nil && nsstring_to_string(title) == label {
            let target: ObjcId = msg_send![entry, target];
            eprintln!("tray selftest: choosing menu item {index} {label:?} in {delay} s");
            // performSelector:afterDelay: keeps the target until it fires
            let () = msg_send![target, performSelector: sel!(trayAction:) withObject: entry afterDelay: delay];
            return;
        }
    }
    eprintln!("tray selftest: no menu item {label:?}");
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
