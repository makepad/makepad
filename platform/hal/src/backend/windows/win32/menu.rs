//! The application menu (`MenuSet`) as a native HMENU tree. Makepad windows draw their own caption,
//! so the tree is not a menu bar: it opens as a popup at the window's top-left when Alt is tapped
//! alone (the keyboard convention of a menu bar), and its key equivalents (Ctrl, plus Shift when
//! the item asks) answer as `MenuCmd` instead of key events, as AppKit's do. A chosen item reaches
//! the window as WM_COMMAND and goes to the session that set the menu.
//!
//! Replacement is safe while the menu is open: the tree being tracked is destroyed only after
//! TrackPopupMenuEx returned; a `MenuSet` arriving meanwhile is applied then.

use super::keys;
use super::window::Win;
use crate::types::*;
use crate::Id;
use makepad_key_code::KeyCode;
use makepad_windows_sys::core::PWSTR;
use makepad_windows_sys::Win32::Foundation::{HWND, POINT};
use makepad_windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::*;
use std::cell::{Cell, RefCell};

enum Action {
    Cmd(u64),
    Quit,
}

struct Menu {
    sid: u32,
    root: HMENU,
    /// WM_COMMAND id - 1 -> action
    actions: Vec<Action>,
    /// key equivalents: key index, shift -> action index
    keys: Vec<(u32, bool, usize)>,
}

thread_local! {
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<Option<(u32, Vec<(MenuItem, String)>)>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The label of a key equivalent ("Ctrl+Shift+S").
fn shortcut(key: KeyCode, shift: bool) -> Option<String> {
    let vk = (1u16..255).find(|vk| keys::key_code(*vk, false) == key)?;
    let name = match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => {
            let n = format!("{key:?}");
            n.strip_prefix("Key").map(str::to_string).unwrap_or(n)
        }
    };
    Some(if shift { format!("Ctrl+Shift+{name}") } else { format!("Ctrl+{name}") })
}

fn insert(parent: HMENU, info: &MENUITEMINFOW) {
    unsafe {
        // position -1: appended
        if let Err(e) = InsertMenuItemW(parent, u32::MAX, true, info) {
            eprintln!("[hal-win32] menu item insert failed: {e:?}");
        }
    }
}

fn build(sid: u32, items: &[(MenuItem, String)]) -> Option<Menu> {
    let root = unsafe { CreatePopupMenu() }.ok()?;
    let mut menu = Menu { sid, root, actions: Vec::new(), keys: Vec::new() };
    // menus[0] is the root, menus[i] the submenu of node i (None for items and lines)
    let mut menus: Vec<Option<HMENU>> = vec![Some(root)];
    for (item, name) in items {
        let parent = menus.get(item.parent as usize).copied().flatten().unwrap_or(root);
        let mut info = MENUITEMINFOW { cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32, ..Default::default() };
        match item.kind & 0xff {
            menu_kind::SUB => {
                let Ok(sub) = (unsafe { CreatePopupMenu() }) else {
                    menus.push(None);
                    continue;
                };
                let mut text = wide(name);
                info.fMask = MIIM_SUBMENU | MIIM_STRING;
                info.hSubMenu = sub;
                info.dwTypeData = PWSTR(text.as_mut_ptr());
                insert(parent, &info);
                menus.push(Some(sub));
            }
            menu_kind::ITEM => {
                menus.push(None);
                let action = if item.kind & menu_kind::QUIT != 0 { Action::Quit } else { Action::Cmd(item.cmd) };
                menu.actions.push(action);
                let id = menu.actions.len();
                if id > u16::MAX as usize {
                    // WM_COMMAND carries 16-bit ids
                    eprintln!("[hal-win32] menu has more than {} items; the rest are not shown", u16::MAX);
                    break;
                }
                let key = KeyCode::from_index(item.key);
                let shift = item.kind & menu_kind::SHIFT != 0;
                let label = match (key != KeyCode::Unknown).then(|| shortcut(key, shift)).flatten() {
                    Some(s) => {
                        menu.keys.push((item.key, shift, id - 1));
                        format!("{name}\t{s}")
                    }
                    None => name.clone(),
                };
                let mut text = wide(&label);
                info.fMask = MIIM_ID | MIIM_STRING | MIIM_STATE | MIIM_FTYPE;
                info.fType = MFT_STRING;
                info.wID = id as u32;
                if item.kind & menu_kind::DISABLED != 0 {
                    info.fState = MFS_DISABLED;
                }
                info.dwTypeData = PWSTR(text.as_mut_ptr());
                insert(parent, &info);
            }
            _ => {
                menus.push(None);
                info.fMask = MIIM_FTYPE;
                info.fType = MFT_SEPARATOR;
                insert(parent, &info);
            }
        }
    }
    Some(menu)
}

fn destroy(menu: Menu) {
    // destroys the submenus with it
    unsafe {
        let _ = DestroyMenu(menu.root);
    }
}

/// `MenuSet` of session `sid`: the session's menu replaces the app's menu.
pub(super) fn set(sid: u32, items: Vec<(MenuItem, String)>) {
    if TRACKING.get() {
        PENDING.set(Some((sid, items)));
        return;
    }
    let new = build(sid, &items);
    if super::log_enabled() {
        eprintln!("[hal-win32] session {sid}: menu of {} nodes, {} key equivalents", items.len(), new.as_ref().map_or(0, |m| m.keys.len()));
    }
    let old = MENU.replace(new);
    if let Some(old) = old {
        destroy(old);
    }
}

pub(super) fn session_closed(sid: u32) {
    if PENDING.with_borrow(|p| p.as_ref().is_some_and(|p| p.0 == sid)) {
        PENDING.set(None);
    }
    if TRACKING.get() {
        return;
    }
    let old = MENU.with_borrow_mut(|m| if m.as_ref().is_some_and(|m| m.sid == sid) { m.take() } else { None });
    if let Some(old) = old {
        destroy(old);
    }
}

fn run(sid: u32, index: usize) {
    let action = MENU.with_borrow(|m| {
        let m = m.as_ref()?;
        (m.sid == sid).then_some(())?;
        Some(match m.actions.get(index)? {
            Action::Cmd(c) => Some(*c),
            Action::Quit => None,
        })
    });
    let key = (sid, Id::NONE);
    match action {
        Some(Some(cmd)) => super::Shared::global().push(key, |q, _| q.menu_cmd(cmd)),
        Some(None) => super::Shared::global().push(key, |q, _| q.lifecycle(lifecycle::QUIT_REQUESTED)),
        None => {}
    }
}

/// A key press with Control held (and no Alt): true when it was a menu's key equivalent, which
/// then answered as the menu command instead of a key event.
pub(super) fn key_equivalent(w: &Win, key: KeyCode, mods: u32) -> bool {
    if mods & input::MOD_CONTROL == 0 || mods & input::MOD_ALT != 0 {
        return false;
    }
    let shift = mods & input::MOD_SHIFT != 0;
    let index = key.to_index();
    let hit = MENU.with_borrow(|m| {
        let m = m.as_ref()?;
        if m.sid != w.recipient.get().0 {
            return None;
        }
        m.keys.iter().find(|(k, s, _)| *k == index && *s == shift).map(|(_, _, a)| (m.sid, *a))
    });
    match hit {
        Some((sid, a)) => {
            run(sid, a);
            true
        }
        None => false,
    }
}

/// Opens the menu at the window's top-left (Alt tapped alone). The choice comes back as WM_COMMAND.
pub(super) fn show(w: &Win) {
    let sid = w.recipient.get().0;
    let Some(root) = MENU.with_borrow(|m| m.as_ref().filter(|m| m.sid == sid).map(|m| m.root)) else { return };
    let hwnd: HWND = w.hwnd.get();
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut pt);
    }
    TRACKING.set(true);
    unsafe {
        // the tracking loop is modal on this thread; HAL ops keep arriving through it
        let _ = TrackPopupMenuEx(root, (TPM_LEFTALIGN | TPM_TOPALIGN).0, pt.x, pt.y, hwnd, None);
    }
    TRACKING.set(false);
    if let Some((sid, items)) = PENDING.take() {
        set(sid, items);
    }
}

/// WM_COMMAND from the menu (`id` is the item's).
pub(super) fn command(w: &Win, id: u16) {
    if id == 0 {
        return;
    }
    run(w.recipient.get().0, id as usize - 1);
}

/// Whether the window's session has a menu to open.
pub(super) fn has_menu(w: &Win) -> bool {
    let sid = w.recipient.get().0;
    MENU.with_borrow(|m| m.as_ref().is_some_and(|m| m.sid == sid))
}
