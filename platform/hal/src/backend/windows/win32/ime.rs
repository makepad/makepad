//! Text composition through IMM (the input method manager every Windows IME speaks). Moved from
//! os/windows/win32_window.rs (composition reader, WM_IME_* handlers, candidate placement, the
//! consumed result that keeps DefWindowProc from typing it a second time, context activation);
//! the Cx callbacks became `Ime` / `Text` events and `ImeSync` is no longer a no-op.
//!
//! A window starts with its input context detached: the IME only wakes while the app shows a text
//! field (`ImeShow`). The composition preview goes to the app as `Ime` START / UPDATE, the result as
//! `Ime` COMMIT replacing the preview (or plain typed `Text` when there was none), an abandoned
//! composition as CANCEL. Caret offsets IMM reports in UTF-16 units become char offsets.

use super::window::Win;
use crate::types::input;
use makepad_windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use makepad_windows_sys::Win32::UI::Input::Ime::*;
use std::cell::{Cell, RefCell};

/// The text field's composition range the app last synced (`ImeSync`), in chars.
#[derive(Default, Clone)]
struct Synced {
    composition: [u32; 2],
}

#[derive(Default)]
pub(super) struct ImeState {
    /// the app shows a text field: the input context is attached
    active: Cell<bool>,
    /// the caret line, window points (x, y, w, h)
    rect: Cell<[f64; 4]>,
    /// the context detached while no text field shows
    saved: Cell<usize>,
    /// IMM is composing (between WM_IME_STARTCOMPOSITION and WM_IME_ENDCOMPOSITION)
    composing: Cell<bool>,
    /// the app holds a preview of the composition (a START went out)
    preview: Cell<bool>,
    /// a cancel the shell asked IMM for: its messages are not the user's
    silenced: Cell<bool>,
    synced: RefCell<Option<Synced>>,
}

fn context(hwnd: HWND) -> Option<HIMC> {
    let himc = unsafe { ImmGetContext(hwnd) };
    (!himc.is_invalid()).then_some(himc)
}

fn release(hwnd: HWND, himc: HIMC) {
    unsafe {
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

/// A composition string (`GCS_COMPSTR` the preview, `GCS_RESULTSTR` the result) as UTF-16.
fn composition_string(himc: HIMC, index: IME_COMPOSITION_STRING) -> Vec<u16> {
    unsafe {
        // a null buffer answers the byte length (the W variant counts UTF-16 units, 2 bytes each)
        let bytes = ImmGetCompositionStringW(himc, index, None, 0);
        if bytes <= 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; bytes as usize / 2];
        let written = ImmGetCompositionStringW(himc, index, Some(buf.as_mut_ptr() as *mut _), bytes as u32);
        buf.truncate((written.max(0) as usize / 2).min(buf.len()));
        buf
    }
}

/// The char offset of UTF-16 offset `units` in `text`.
fn chars_at(text: &[u16], units: usize) -> u32 {
    char::decode_utf16(text[..units.min(text.len())].iter().copied()).count() as u32
}

/// The composition and candidate windows follow the caret line: the candidate list keeps out of
/// the line (above or below it, with a gap) instead of covering the text.
fn place(w: &Win, himc: HIMC) {
    let r = w.ime.rect.get();
    if r[3] <= 0.0 {
        return;
    }
    let dpi = w.dpi.get();
    let (left, top) = ((r[0] * dpi) as i32, (r[1] * dpi) as i32);
    let (right, bottom) = (((r[0] + r[2]) * dpi) as i32, ((r[1] + r[3]) * dpi) as i32);
    let clearance = (r[3] * dpi * 0.6) as i32;
    let caret = POINT { x: left, y: bottom };
    unsafe {
        let _ = ImmSetCompositionWindow(himc, &COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: caret, rcArea: RECT::default() });
        let area = RECT { left, top: top - clearance, right: right.max(left + 1), bottom: bottom + clearance };
        let _ = ImmSetCandidateWindow(himc, &CANDIDATEFORM { dwIndex: 0, dwStyle: CFS_EXCLUDE, ptCurrentPos: caret, rcArea: area });
    }
}

/// A new window: no IME until a text field shows.
pub(super) fn window_created(w: &Win) {
    detach(w);
}

fn attach(w: &Win) {
    let saved = w.ime.saved.replace(0);
    if saved != 0 {
        unsafe {
            ImmAssociateContext(w.hwnd.get(), HIMC(saved as _));
        }
    }
}

fn detach(w: &Win) {
    if w.ime.saved.get() == 0 {
        let old = unsafe { ImmAssociateContext(w.hwnd.get(), HIMC::default()) };
        w.ime.saved.set(old.0 as usize);
    }
}

/// `ImeShow`: the field's caret line (window points).
pub(super) fn show(w: &Win, area: [f32; 4]) {
    let rect = area.map(|v| v as f64);
    w.ime.active.set(true);
    attach(w);
    if w.ime.rect.replace(rect) != rect && w.ime.composing.get() {
        // the caret line moved during a composition: its windows follow at once
        if let Some(himc) = context(w.hwnd.get()) {
            place(w, himc);
            release(w.hwnd.get(), himc);
        }
    }
}

/// `ImeHide`: the field lost focus. The app keeps its preview as text (as on macOS); only IMM's
/// composition is dropped, without telling the app.
pub(super) fn hide(w: &Win) {
    w.ime.active.set(false);
    if w.ime.composing.get() {
        cancel_silently(w);
    }
    w.ime.preview.set(false);
    *w.ime.synced.borrow_mut() = None;
    detach(w);
}

fn cancel_silently(w: &Win) {
    let hwnd = w.hwnd.get();
    if let Some(himc) = context(hwnd) {
        w.ime.silenced.set(true);
        unsafe {
            let _ = ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
        }
        w.ime.silenced.set(false);
        release(hwnd, himc);
    }
    w.ime.composing.set(false);
    w.ime.preview.set(false);
}

/// `ImeSync`: the field's state (char offsets). When the app ended its preview itself, the caret
/// leaving the old composition range (a click elsewhere in the field), IMM's composition is
/// cancelled so the next keystroke starts a fresh one. A sync that still reflects an older state
/// (sent before the app saw the latest START) never cancels: only the transition from a synced
/// composition to none counts.
pub(super) fn sync(w: &Win, selection: [u32; 2], composition: [u32; 2]) {
    let new = Synced { composition };
    let old = w.ime.synced.borrow_mut().replace(new);
    let Some(old) = old else { return };
    let had = old.composition[0] != old.composition[1];
    let has = composition[0] != composition[1];
    if had && !has && w.ime.preview.get() {
        let caret = selection[0].min(selection[1]);
        if caret < old.composition[0] || caret > old.composition[1] {
            if super::log_enabled() {
                eprintln!("[hal-win32] ime: the app ended the composition (caret {caret} outside {:?}); cancelling IMM's", old.composition);
            }
            cancel_silently(w);
        }
    }
}

/// The WM_IME_* messages; `Some` when the message is consumed (DefWindowProc would draw its own
/// composition window and type the result again as WM_IME_CHAR / WM_CHAR).
pub(super) fn message(w: &Win, msg: u32, lparam: isize) -> Option<()> {
    use makepad_windows_sys::Win32::UI::WindowsAndMessaging::{WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION};
    let hwnd = w.hwnd.get();
    match msg {
        WM_IME_STARTCOMPOSITION => {
            w.ime.composing.set(true);
            if let Some(himc) = context(hwnd) {
                place(w, himc);
                release(hwnd, himc);
            }
            Some(())
        }
        WM_IME_COMPOSITION => {
            if w.ime.silenced.get() {
                return Some(());
            }
            let Some(himc) = context(hwnd) else { return Some(()) };
            let flags = lparam as u32;
            if flags & GCS_RESULTSTR.0 != 0 {
                let result = String::from_utf16_lossy(&composition_string(himc, GCS_RESULTSTR));
                if !result.is_empty() {
                    if w.ime.preview.replace(false) {
                        // replaces the preview (start != end: a replaced range)
                        w.push(|q, id| q.ime(id, input::ime::COMMIT, 0, 1, result.as_bytes()));
                    } else {
                        w.push(|q, id| q.text(id, input::text::TYPED, result.as_bytes()));
                    }
                }
            }
            if flags & GCS_COMPSTR.0 != 0 {
                let units = composition_string(himc, GCS_COMPSTR);
                if units.is_empty() {
                    if w.ime.preview.replace(false) {
                        w.push(|q, id| q.ime(id, input::ime::CANCEL, 0, 0, b""));
                    }
                } else {
                    let cursor = if flags & GCS_CURSORPOS.0 != 0 {
                        let at = unsafe { ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0) };
                        chars_at(&units, at.max(0) as usize)
                    } else {
                        chars_at(&units, units.len())
                    };
                    let text = String::from_utf16_lossy(&units);
                    let kind = if w.ime.preview.replace(true) { input::ime::UPDATE } else { input::ime::START };
                    w.push(|q, id| q.ime(id, kind, cursor, cursor, text.as_bytes()));
                }
            }
            release(hwnd, himc);
            Some(())
        }
        WM_IME_ENDCOMPOSITION => {
            w.ime.composing.set(false);
            // an IME that ends without an empty preview first (some cancel paths)
            if !w.ime.silenced.get() && w.ime.preview.replace(false) {
                w.push(|q, id| q.ime(id, input::ime::CANCEL, 0, 0, b""));
            }
            Some(())
        }
        _ => None,
    }
}

/// The window was activated again: a field that shows gets its caret placement back.
pub(super) fn focus(w: &Win) {
    if w.ime.active.get() {
        attach(w);
    }
}
