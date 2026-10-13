//! Text input on iOS: an invisible `UITextView` subclass is the system keyboard's client. A
//! real text view gets the native language HUD and the full input-mode cycle (a hand-rolled
//! UITextInput view never does); it is the authority for typing, navigation and the keyboard's
//! composition, and the app mirrors it (`ImeState`, UTF-16 offsets) and draws the text and the
//! caret itself. The app pushes its own changes back with `ImeSync`. Also here: the soft
//! keyboard's frame (`ImeAnimate` / `ImeResize`), hardware keyboards coming and going
//! (`PhysicalKeyboard`), the pasteboard, and the text field's auto-repeating navigation keys.
//! Moved from os/apple/ios/ios_text_input.rs and the keyboard parts of ios_app.rs /
//! ios_delegates.rs.

use super::app::app;
use super::classes::shielded;
use super::keys;
use super::window::Win;
use crate::clipboard::ClipboardData;
use crate::types::input::ime_config as cfg;
use crate::types::*;
use makepad_apple_sys::*;
use makepad_key_code::KeyCode;
use std::cell::Cell;

/// The caret anchor the candidate window and the accent popup hang from.
const CARET_HEIGHT: f64 = 20.0;
/// The height of the sliver the text view occupies at the caret line.
const TARGET_HEIGHT: f64 = 32.0;

/// The text field's state, main thread only.
struct Text {
    class: Cell<*const Class>,
    root: Cell<ObjcId>,
    view: Cell<ObjcId>,
    observer: Cell<ObjcId>,
    /// the window whose field has the keyboard
    owner: Cell<Option<&'static Win>>,
    /// the last `ImeShow::config` applied
    config: Cell<Option<u32>>,
    /// where the view sits (the caret line's bottom left), to skip blink redraws
    position: Cell<Option<[f64; 2]>>,
    /// the number of the last `ImeState` pushed (see `shell::Op::Cmd`)
    forwarded: Cell<u64>,
    physical_keyboard: Cell<bool>,
}

struct MainOnly(Text);
unsafe impl Sync for MainOnly {}
unsafe impl Send for MainOnly {}

fn text() -> &'static Text {
    static TEXT: std::sync::OnceLock<MainOnly> = std::sync::OnceLock::new();
    &TEXT
        .get_or_init(|| {
            MainOnly(Text {
                class: Cell::new(std::ptr::null()),
                root: Cell::new(nil),
                view: Cell::new(nil),
                observer: Cell::new(nil),
                owner: Cell::new(None),
                config: Cell::new(None),
                position: Cell::new(None),
                forwarded: Cell::new(0),
                physical_keyboard: Cell::new(false),
            })
        })
        .0
}

/// At launch: the text view in the root view, the keyboard observers.
pub(super) fn init(root: ObjcId) {
    let t = text();
    t.class.set(define_text_view());
    t.root.set(root);
    unsafe {
        t.view.set(create_view(root));
        let observer: ObjcId = msg_send![define_observer(), new];
        t.observer.set(observer);
        let center: ObjcId = msg_send![class!(NSNotificationCenter), defaultCenter];
        let () = msg_send![center, addObserver: observer selector: sel!(keyboardWillChangeFrame:) name: UIKeyboardWillChangeFrameNotification object: nil];
        let () = msg_send![center, addObserver: observer selector: sel!(keyboardDidChangeFrame:) name: UIKeyboardDidChangeFrameNotification object: nil];
        let () = msg_send![center, addObserver: observer selector: sel!(inputModeDidChange:) name: UITextInputCurrentInputModeDidChangeNotification object: nil];
        let () = msg_send![center, addObserver: observer selector: sel!(physicalKeyboardChanged:) name: str_to_nsstring("GCKeyboardDidConnectNotification") object: nil];
        let () = msg_send![center, addObserver: observer selector: sel!(physicalKeyboardChanged:) name: str_to_nsstring("GCKeyboardDidDisconnectNotification") object: nil];
    }
    t.physical_keyboard.set(physical_keyboard_connected());
}

/// A fresh text view, invisible (the app draws the text and the caret), in `root`. Also made
/// again when a secure field gave up the keyboard: iOS keeps a sticky password / AutoFill tag on
/// the text-input object.
unsafe fn create_view(root: ObjcId) -> ObjcId {
    let bounds: NSRect = msg_send![root, bounds];
    let view: ObjcId = msg_send![text().class.get(), alloc];
    let view: ObjcId = msg_send![view, initWithFrame: bounds];
    (*view).set_ivar::<f64>("ime_pos_x", 0.0);
    (*view).set_ivar::<f64>("ime_pos_y", 0.0);
    (*view).set_ivar::<f64>("ime_line_height", 0.0);
    (*view).set_ivar::<bool>("is_multiline", false);
    (*view).set_ivar::<bool>("submit_on_enter", false);
    (*view).set_ivar::<bool>("is_read_only", false);
    (*view).set_ivar::<BOOL>("programmatic_update", NO);
    let clear: ObjcId = msg_send![class!(UIColor), clearColor];
    let () = msg_send![view, setBackgroundColor: clear];
    let () = msg_send![view, setTextColor: clear];
    // the tint also colours the CJK candidate bar's highlight: a clear tint would white it out
    // (the caret is hidden by its zero rect instead)
    let tint: ObjcId = msg_send![class!(UIColor), systemBlueColor];
    let () = msg_send![view, setTintColor: tint];
    let () = msg_send![view, setOpaque: NO];
    let () = msg_send![view, setScrollEnabled: NO];
    let () = msg_send![view, setClipsToBounds: YES];
    // no red misspelling underline: the app renders the text
    let () = msg_send![view, setSpellCheckingType: 1i64];
    let () = msg_send![view, setDelegate: view];
    let responds: BOOL = msg_send![view, respondsToSelector: sel!(setFocusEffect:)];
    if responds == YES {
        let () = msg_send![view, setFocusEffect: nil];
    }
    // Full Keyboard Access skips it (no forced visible focus caret); keys still arrive
    let () = msg_send![view, setIsAccessibilityElement: YES];
    let responds: BOOL = msg_send![view, respondsToSelector: sel!(setAccessibilityRespondsToUserInteraction:)];
    if responds == YES {
        let () = msg_send![view, setAccessibilityRespondsToUserInteraction: NO];
    }
    let () = msg_send![root, addSubview: view];
    view
}

/// The keyboard comes up for a field of `win`: its traits, its place (the caret line, `area`
/// x, y, w, h in window points), the focus.
pub(super) fn show(win: &'static Win, area: [f32; 4], config: u32) {
    let t = text();
    t.owner.set(Some(win));
    configure(config);
    let caret = [area[0] as f64, (area[1] + area[3]) as f64];
    let line_height = area[3] as f64;
    position(caret, line_height);
    let view = t.view.get();
    unsafe {
        let focused: BOOL = msg_send![view, isFirstResponder];
        if focused != YES {
            let () = msg_send![view, becomeFirstResponder];
        }
    }
}

/// The keyboard goes: the field's text (a typed password) is wiped from the shared view.
pub(super) fn hide() {
    let view = text().view.get();
    if view == nil {
        return;
    }
    unsafe {
        (*view).set_ivar::<BOOL>("programmatic_update", YES);
        let () = msg_send![view, setText: str_to_nsstring("")];
        (*view).set_ivar::<BOOL>("programmatic_update", NO);
        let () = msg_send![view, resignFirstResponder];
    }
}

/// The session is gone: its field gives up the keyboard.
pub(super) fn session_closed(sid: u32) {
    let t = text();
    if t.owner.get().is_some_and(|w| w.owner_sid == sid || w.sid.get() == sid) {
        hide();
        t.owner.set(None);
    }
}

/// The field's traits (`ImeShow::config`): keyboard type, capitalization, correction, return
/// key, secure entry and AutoFill content type. Applied only when they changed, since
/// `reloadInputViews` is not free and the app shows the keyboard every frame.
fn configure(config: u32) {
    let t = text();
    let before = t.config.replace(Some(config));
    if before == Some(config) {
        return;
    }
    // leaving a credential field: a fresh view for the next field (see `create_view`)
    let taints = |c: u32| c & cfg::SECURE != 0 || matches!(cfg::field(c, cfg::CONTENT_TYPE_SHIFT, 4), 1..=4);
    if before.is_some_and(taints) && !taints(config) {
        let old = t.view.get();
        unsafe {
            let () = msg_send![old, removeFromSuperview];
            let () = msg_send![old, release];
            t.view.set(create_view(t.root.get()));
        }
        t.position.set(None);
    }
    let view = t.view.get();
    // the Cx enums' variants in declaration order (see `ime_config`)
    let keyboard: i64 = match cfg::field(config, cfg::INPUT_MODE_SHIFT, 4) {
        // None, Text -> UIKeyboardTypeDefault
        2 => 1,  // Ascii -> ASCIICapable
        3 => 3,  // Url -> URL
        4 => 4,  // Numeric -> NumberPad
        5 => 5,  // Tel -> PhonePad
        6 => 7,  // Email -> EmailAddress
        7 => 8,  // Decimal -> DecimalPad
        8 => 10, // Search -> WebSearch
        _ => 0,
    };
    // None, Words, Sentences, AllCharacters = UITextAutocapitalizationType
    let capitalize = cfg::field(config, cfg::AUTOCAPITALIZE_SHIFT, 2) as i64;
    let correct: i64 = match cfg::field(config, cfg::AUTOCORRECT_SHIFT, 2) {
        1 => 2, // Enabled -> Yes
        2 => 1, // Disabled -> No
        _ => 0, // Default
    };
    // Default None Go Google Join Next Route Search Send Yahoo Done EmergencyCall Continue Previous
    let return_key: i64 = match cfg::field(config, cfg::RETURN_KEY_SHIFT, 4) {
        k @ 2..=12 => k as i64 - 1,
        _ => 0,
    };
    unsafe {
        let content = match cfg::field(config, cfg::CONTENT_TYPE_SHIFT, 4) {
            1 => UITextContentTypeUsername,
            2 => UITextContentTypePassword,
            3 => UITextContentTypeNewPassword,
            4 => UITextContentTypeEmailAddress,
            5 => UITextContentTypeURL,
            6 => UITextContentTypeFullStreetAddress,
            7 => UITextContentTypeTelephoneNumber,
            8 => UITextContentTypeOneTimeCode,
            _ => UITextContentTypeNone,
        };
        let () = msg_send![view, setKeyboardType: keyboard];
        let () = msg_send![view, setAutocapitalizationType: capitalize];
        let () = msg_send![view, setAutocorrectionType: correct];
        let () = msg_send![view, setReturnKeyType: return_key];
        let () = msg_send![view, setSecureTextEntry: if config & cfg::SECURE != 0 { YES } else { NO }];
        // a read-only field rejects every insert (a hardware Enter too), so the app and the
        // view stay in step
        let () = msg_send![view, setEditable: if config & cfg::READ_ONLY != 0 { NO } else { YES }];
        let () = msg_send![view, setTextContentType: content];
        (*view).set_ivar::<bool>("is_multiline", config & cfg::MULTILINE != 0);
        (*view).set_ivar::<bool>("submit_on_enter", config & cfg::SUBMIT_ON_ENTER != 0);
        (*view).set_ivar::<bool>("is_read_only", config & cfg::READ_ONLY != 0);
        let () = msg_send![view, reloadInputViews];
    }
}

/// The view moves to a sliver at the caret line (a tall frame drops the first candidate in
/// multiline fields); 1 point wide so the language HUD still shows.
fn position(caret: [f64; 2], line_height: f64) {
    let t = text();
    if t.position.replace(Some(caret)) == Some(caret) {
        return;
    }
    let view = t.view.get();
    let frame = NSRect { origin: NSPoint { x: caret[0], y: caret[1] - TARGET_HEIGHT }, size: NSSize { width: 1.0, height: TARGET_HEIGHT } };
    unsafe {
        let () = msg_send![view, setFrame: frame];
        (*view).set_ivar::<f64>("ime_pos_x", 0.0);
        (*view).set_ivar::<f64>("ime_pos_y", TARGET_HEIGHT);
        (*view).set_ivar::<f64>("ime_line_height", line_height);
    }
}

/// The app's text, selection and composition (char offsets) go into the view. Dropped when
/// the app sent it before it had seen the view's newest state: newer typing is on its way and
/// wins. The delegate callbacks the writes trigger do not echo back to the app.
pub(super) fn sync(text_now: &str, selection: [u32; 2], composition: [u32; 2], taken: u64) {
    let t = text();
    if taken < t.forwarded.get() {
        return;
    }
    let view = t.view.get();
    if view == nil {
        return;
    }
    let utf16 = |chars: u32| -> u64 { text_now.chars().take(chars as usize).map(|c| c.len_utf16() as u64).sum() };
    let (sel_start, sel_end) = (utf16(selection[0]), utf16(selection[1]));
    unsafe {
        let ns_text: ObjcId = msg_send![view, text];
        let live = if ns_text == nil { String::new() } else { nsstring_to_string(ns_text) };
        let live_selection: NSRange = msg_send![view, selectedRange];
        let range = NSRange { location: sel_start, length: sel_end.saturating_sub(sel_start) };
        (*view).set_ivar::<BOOL>("programmatic_update", YES);
        if live != text_now {
            if composition[1] > composition[0] {
                // the keyboard's composition goes back as marked text, so an edit beside it does
                // not commit it: the text around it first, then the composed part at its place
                let (start, end) = (composition[0] as usize, composition[1] as usize);
                let around: String = text_now.chars().take(start).chain(text_now.chars().skip(end)).collect();
                let composed: String = text_now.chars().skip(start).take(end - start).collect();
                let () = msg_send![view, setText: str_to_nsstring(&around)];
                let () = msg_send![view, setSelectedRange: NSRange { location: utf16(composition[0]), length: 0 }];
                let caret = NSRange { location: utf16(composition[1]) - utf16(composition[0]), length: 0 };
                let () = msg_send![view, setMarkedText: str_to_nsstring(&composed) selectedRange: caret];
            } else {
                let () = msg_send![view, setText: str_to_nsstring(text_now)];
                let () = msg_send![view, setSelectedRange: range];
            }
        } else if live_selection.location != range.location || live_selection.length != range.length {
            let () = msg_send![view, setSelectedRange: range];
        }
        (*view).set_ivar::<BOOL>("programmatic_update", NO);
    }
}

/// The view changed (typing, the keyboard's composition, navigation): its whole state goes to
/// the app, offsets in UTF-16 code units.
unsafe fn forward_state(view: ObjcId) {
    let t = text();
    let programmatic: BOOL = *(*view).get_ivar::<BOOL>("programmatic_update");
    if programmatic == YES {
        return;
    }
    let Some(win) = t.owner.get() else { return };
    let ns_text: ObjcId = msg_send![view, text];
    let text_now = if ns_text == nil { String::new() } else { nsstring_to_string(ns_text) };
    let selection: NSRange = msg_send![view, selectedRange];
    let marked: ObjcId = msg_send![view, markedTextRange];
    let (comp_start, comp_end) = if marked == nil {
        (u32::MAX, u32::MAX)
    } else {
        let begin: ObjcId = msg_send![view, beginningOfDocument];
        let start: ObjcId = msg_send![marked, start];
        let end: ObjcId = msg_send![marked, end];
        let start: i64 = msg_send![view, offsetFromPosition: begin toPosition: start];
        let end: i64 = msg_send![view, offsetFromPosition: begin toPosition: end];
        if start >= 0 && end > start { (start as u32, end as u32) } else { (u32::MAX, u32::MAX) }
    };
    let id = win.id.get();
    let start = selection.location as u32;
    let end = (selection.location + selection.length) as u32;
    let number = win.push(|q| q.ime_state(id, start, end, comp_start, comp_end, text_now.as_bytes()));
    t.forwarded.set(number);
}

/// A key the field's text path stands for (Return in a single-line field): down and up.
fn field_key(code: KeyCode) {
    if let Some(win) = text().owner.get() {
        keys::key(win, code, 0, true);
        keys::key(win, code, 0, false);
    }
}

// ------------------------------------------------------------------ what the keys ask

/// The text view has the keyboard focus.
pub(super) fn focused() -> bool {
    let view = text().view.get();
    view != nil && unsafe { msg_send![view, isFirstResponder] }
}

/// (multiline, submit on enter, read only) of the focused field, for a hardware Enter.
pub(super) fn enter_config() -> Option<(bool, bool, bool)> {
    let view = text().view.get();
    if view == nil {
        return None;
    }
    unsafe { Some((*(*view).get_ivar::<bool>("is_multiline"), *(*view).get_ivar::<bool>("submit_on_enter"), *(*view).get_ivar::<bool>("is_read_only"))) }
}

/// A hardware Enter as a newline: into the view, so it syncs in order like typed text.
pub(super) fn insert_newline() {
    let view = text().view.get();
    if view != nil {
        unsafe {
            let () = msg_send![view, insertText: str_to_nsstring("\n")];
        }
    }
}

/// Command V and the edit menu's Paste: the pasteboard's text, as pasted text.
pub(super) fn paste(win: &Win) {
    let content = unsafe {
        let pasteboard: ObjcId = msg_send![class!(UIPasteboard), generalPasteboard];
        let string: ObjcId = msg_send![pasteboard, string];
        if string == nil { String::new() } else { nsstring_to_string(string) }
    };
    if !content.is_empty() {
        let id = win.id.get();
        win.push(|q| q.text(id, input::text::PASTE, content.as_bytes()));
    }
}

pub(super) fn clipboard_set(content: &str) {
    unsafe {
        let pasteboard: ObjcId = msg_send![class!(UIPasteboard), generalPasteboard];
        let () = msg_send![pasteboard, setString: str_to_nsstring(content)];
    }
}

/// The pasteboard's text and its image (PNG), for `ClipRead`.
pub(super) fn clipboard_read() -> Vec<u8> {
    let mut content = ClipboardData::default();
    unsafe {
        let pasteboard: ObjcId = msg_send![class!(UIPasteboard), generalPasteboard];
        let string: ObjcId = msg_send![pasteboard, string];
        if string != nil {
            content.text = nsstring_to_string(string);
        }
        let data: ObjcId = msg_send![pasteboard, dataForPasteboardType: str_to_nsstring("public.png")];
        if data != nil {
            let len: u64 = msg_send![data, length];
            let bytes: *const u8 = msg_send![data, bytes];
            if len > 0 && !bytes.is_null() {
                content.png = std::slice::from_raw_parts(bytes, len as usize).to_vec();
            }
        }
    }
    content.pack()
}

fn physical_keyboard_connected() -> bool {
    unsafe {
        let class = makepad_objc_sys::runtime::objc_getClass(b"GCKeyboard\0".as_ptr() as *const _) as ObjcId;
        if class.is_null() {
            return false;
        }
        let responds: BOOL = msg_send![class, respondsToSelector: sel!(coalescedKeyboard)];
        if responds != YES {
            return false;
        }
        let keyboard: ObjcId = msg_send![class, coalescedKeyboard];
        keyboard != nil
    }
}

// ------------------------------------------------------------------ navigation key commands

/// The iOS 13.4+ `UIKeyInput` names of Home / End / PageUp / PageDown, looked up at run time
/// (each nil where the system lacks it).
struct DocKeys {
    home: ObjcId,
    end: ObjcId,
    page_up: ObjcId,
    page_down: ObjcId,
}
// immortal NSString constants
unsafe impl Send for DocKeys {}
unsafe impl Sync for DocKeys {}

unsafe fn uikit_string(name: &[u8]) -> ObjcId {
    extern "C" {
        fn dlsym(handle: *mut std::ffi::c_void, symbol: *const i8) -> *mut std::ffi::c_void;
    }
    // RTLD_DEFAULT (-2) searches every loaded image
    let symbol = dlsym(-2isize as *mut std::ffi::c_void, name.as_ptr() as *const i8);
    if symbol.is_null() {
        nil
    } else {
        *(symbol as *const ObjcId)
    }
}

fn doc_keys() -> &'static DocKeys {
    static KEYS: std::sync::OnceLock<DocKeys> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| unsafe {
        DocKeys { home: uikit_string(b"UIKeyInputHome\0"), end: uikit_string(b"UIKeyInputEnd\0"), page_up: uikit_string(b"UIKeyInputPageUp\0"), page_down: uikit_string(b"UIKeyInputPageDown\0") }
    })
}

/// Home / End / PageUp / PageDown have key commands (iOS 13.4+).
pub(super) fn doc_nav_key_commands() -> bool {
    doc_keys().home != nil
}

const MOD_SHIFT: u64 = 1 << 17;
const MOD_ALT: u64 = 1 << 19;
const MOD_CMD: u64 = 1 << 20;

unsafe fn add_key_command(array: ObjcId, input: ObjcId, flags: u64) {
    if input == nil {
        return;
    }
    let command: ObjcId = msg_send![class!(UIKeyCommand), keyCommandWithInput: input modifierFlags: flags action: sel!(navigationKey:)];
    // iOS 15+: the command wins over the text view's own navigation and auto-repeats
    let responds: BOOL = msg_send![command, respondsToSelector: sel!(setWantsPriorityOverSystemBehavior:)];
    if responds == YES {
        let () = msg_send![command, setWantsPriorityOverSystemBehavior: YES];
    }
    let () = msg_send![array, addObject: command];
}

unsafe fn navigation_key_commands() -> ObjcId {
    let array: ObjcId = msg_send![class!(NSMutableArray), array];
    let arrows = [UIKeyInputLeftArrow, UIKeyInputRightArrow, UIKeyInputUpArrow, UIKeyInputDownArrow];
    let arrow_mods = [0, MOD_SHIFT, MOD_ALT, MOD_CMD, MOD_SHIFT | MOD_ALT, MOD_SHIFT | MOD_CMD];
    for input in arrows {
        for flags in arrow_mods {
            add_key_command(array, input, flags);
        }
    }
    // Home / End also take the Command text-boundary move; Page keys bare or with Shift
    let doc = doc_keys();
    for input in [doc.home, doc.end] {
        for flags in [0, MOD_SHIFT, MOD_CMD, MOD_SHIFT | MOD_CMD] {
            add_key_command(array, input, flags);
        }
    }
    for input in [doc.page_up, doc.page_down] {
        for flags in [0, MOD_SHIFT] {
            add_key_command(array, input, flags);
        }
    }
    array
}

unsafe fn navigation_key(command: ObjcId) {
    let input: ObjcId = msg_send![command, input];
    if input == nil {
        return;
    }
    let doc = doc_keys();
    let pairs = [
        (UIKeyInputLeftArrow, KeyCode::ArrowLeft),
        (UIKeyInputRightArrow, KeyCode::ArrowRight),
        (UIKeyInputUpArrow, KeyCode::ArrowUp),
        (UIKeyInputDownArrow, KeyCode::ArrowDown),
        (doc.home, KeyCode::Home),
        (doc.end, KeyCode::End),
        (doc.page_up, KeyCode::PageUp),
        (doc.page_down, KeyCode::PageDown),
    ];
    let Some(code) = pairs.iter().find(|(name, _)| *name != nil && msg_send![input, isEqualToString: *name]).map(|(_, code)| *code) else { return };
    let flags: u64 = msg_send![command, modifierFlags];
    let mods = keys::modifier_bits(flags);
    let Some(win) = text().owner.get().or_else(|| app().top_window()) else { return };
    // down and up together: the key does not stay down for the app
    keys::key(win, code, mods, true);
    keys::key(win, code, mods, false);
}

// ------------------------------------------------------------------ the classes

/// `HalTextView`: the `UITextView` the keyboard talks to, transparent to touches, with its
/// geometry answers pointing at the app's caret.
fn define_text_view() -> *const Class {
    extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
        YES
    }
    extern "C" fn nil_object(_: &Object, _: Sel) -> ObjcId {
        nil
    }
    // empty focus geometry: Full Keyboard Access has nothing to draw its cursor over
    extern "C" fn accessibility_path(_: &Object, _: Sel) -> ObjcId {
        unsafe { msg_send![class!(UIBezierPath), bezierPathWithRect: NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: 0.0, height: 0.0 } }] }
    }
    // the app owns hit-testing and caret placement
    extern "C" fn point_inside(_: &Object, _: Sel, _: NSPoint, _: ObjcId) -> BOOL {
        NO
    }
    // zero size: no system caret (the app draws its own); the origin anchors the accent popup
    extern "C" fn caret_rect(this: &Object, _: Sel, _: ObjcId) -> NSRect {
        unsafe {
            let x = *this.get_ivar::<f64>("ime_pos_x");
            let y = *this.get_ivar::<f64>("ime_pos_y");
            NSRect { origin: NSPoint { x, y: y - CARET_HEIGHT }, size: NSSize { width: 0.0, height: 0.0 } }
        }
    }
    // While composing, the true composing-line box (with some clearance) so the candidate window
    // flips around the line's real edges; otherwise a degenerate rect, since the same answer
    // drives the autocorrect highlight, which a real height turns into a stray box.
    extern "C" fn first_rect(this: &Object, _: Sel, _: ObjcId) -> NSRect {
        unsafe {
            let x = *this.get_ivar::<f64>("ime_pos_x");
            let y = *this.get_ivar::<f64>("ime_pos_y");
            let line_height = *this.get_ivar::<f64>("ime_line_height");
            let height = if line_height > 1.0 { line_height } else { TARGET_HEIGHT };
            let marked: ObjcId = msg_send![this, markedTextRange];
            if marked == nil {
                return NSRect { origin: NSPoint { x, y: y - 1.5 * TARGET_HEIGHT }, size: NSSize { width: 1.0, height: 0.01 } };
            }
            let clearance = height * 0.3;
            NSRect { origin: NSPoint { x, y: y - height - clearance }, size: NSSize { width: 1.0, height: height + 2.0 * clearance } }
        }
    }
    // no selection rects: the app draws the selection and its handles
    extern "C" fn selection_rects(_: &Object, _: Sel, _: ObjcId) -> ObjcId {
        unsafe { msg_send![class!(NSArray), array] }
    }
    extern "C" fn did_change(this: &Object, _: Sel, _: ObjcId) {
        shielded("text", || unsafe { forward_state(this as *const Object as ObjcId) });
    }
    extern "C" fn did_change_selection(this: &Object, _: Sel, _: ObjcId) {
        shielded("text", || unsafe { forward_state(this as *const Object as ObjcId) });
    }
    // Return in a single-line field is the app's submit, not a newline; a multiline field takes
    // the newline in the view (it syncs in order). Tab moves the focus (the app's), never a tab.
    extern "C" fn should_change(this: &Object, _: Sel, _: ObjcId, _: NSRange, replacement: ObjcId) -> BOOL {
        unsafe {
            if replacement != nil {
                let s = nsstring_to_string(replacement);
                if s == "\n" {
                    if *this.get_ivar::<bool>("is_multiline") {
                        return YES;
                    }
                    shielded("return", || field_key(KeyCode::ReturnKey));
                    return NO;
                }
                if s == "\t" {
                    return NO;
                }
            }
            YES
        }
    }
    extern "C" fn key_commands(_: &Object, _: Sel) -> ObjcId {
        unsafe { navigation_key_commands() }
    }
    extern "C" fn navigation_key_command(_: &Object, _: Sel, command: ObjcId) {
        shielded("navigation-key", || unsafe { navigation_key(command) });
    }
    let mut decl = ClassDecl::new("HalTextView", class!(UITextView)).unwrap();
    // the caret line in view space (x, y = its bottom) and its height, kept by `position`
    decl.add_ivar::<f64>("ime_pos_x");
    decl.add_ivar::<f64>("ime_pos_y");
    decl.add_ivar::<f64>("ime_line_height");
    decl.add_ivar::<bool>("is_multiline");
    decl.add_ivar::<bool>("submit_on_enter");
    decl.add_ivar::<bool>("is_read_only");
    // the app is writing into the view: its callbacks do not echo back
    decl.add_ivar::<BOOL>("programmatic_update");
    unsafe {
        decl.add_method(sel!(canBecomeFocused), yes as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(focusEffect), nil_object as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(sel!(accessibilityPath), accessibility_path as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(sel!(pointInside:withEvent:), point_inside as extern "C" fn(&Object, Sel, NSPoint, ObjcId) -> BOOL);
        decl.add_method(sel!(caretRectForPosition:), caret_rect as extern "C" fn(&Object, Sel, ObjcId) -> NSRect);
        decl.add_method(sel!(firstRectForRange:), first_rect as extern "C" fn(&Object, Sel, ObjcId) -> NSRect);
        decl.add_method(sel!(selectionRectsForRange:), selection_rects as extern "C" fn(&Object, Sel, ObjcId) -> ObjcId);
        decl.add_method(sel!(textViewDidChange:), did_change as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(textViewDidChangeSelection:), did_change_selection as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(textView:shouldChangeTextInRange:replacementText:), should_change as extern "C" fn(&Object, Sel, ObjcId, NSRange, ObjcId) -> BOOL);
        decl.add_method(sel!(keyCommands), key_commands as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(sel!(navigationKey:), navigation_key_command as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.register()
}

/// The soft keyboard's frame in the owner window's view: whether it is up, and how much of the
/// view's bottom a docked keyboard covers (a floating iPad keyboard covers nothing). UIKit
/// reports the frame in screen coordinates; converting through the screen's coordinate space
/// keeps it right for Split View, Slide Over, Stage Manager and rotation.
unsafe fn keyboard_cover(note: ObjcId, view: ObjcId) -> (bool, f64) {
    let info: ObjcId = msg_send![note, userInfo];
    let value: ObjcId = msg_send![info, objectForKey: UIKeyboardFrameEndUserInfoKey];
    if value == nil {
        return (false, 0.0);
    }
    let end_in_screen: NSRect = msg_send![value, CGRectValue];
    let window: ObjcId = msg_send![view, window];
    let screen: ObjcId = if window == nil { nil } else { msg_send![window, screen] };
    let space: ObjcId = if screen == nil { nil } else { msg_send![screen, coordinateSpace] };
    let end: NSRect = if space == nil { msg_send![view, convertRect: end_in_screen fromView: nil] } else { msg_send![view, convertRect: end_in_screen fromCoordinateSpace: space] };
    let bounds: NSRect = msg_send![view, bounds];
    let (view_top, view_bottom) = (bounds.origin.y, bounds.origin.y + bounds.size.height);
    let (view_left, view_right) = (bounds.origin.x, bounds.origin.x + bounds.size.width);
    let (top, bottom) = (end.origin.y.min(end.origin.y + end.size.height), end.origin.y.max(end.origin.y + end.size.height));
    let (left, right) = (end.origin.x.min(end.origin.x + end.size.width), end.origin.x.max(end.origin.x + end.size.width));
    let visible_w = right.min(view_right) - left.max(view_left);
    let visible_h = bottom.min(view_bottom) - top.max(view_top);
    if visible_w <= 1.0 || visible_h <= 1.0 {
        return (false, 0.0);
    }
    let docked = bottom + 1.0 >= view_bottom;
    if !docked {
        return (true, 0.0);
    }
    let frame_h = [end.size.height.abs(), end_in_screen.size.height.abs()].into_iter().filter(|h| *h > 0.0).fold(f64::INFINITY, f64::min);
    let frame_h = if frame_h.is_finite() { frame_h } else { visible_h };
    (true, visible_h.max(0.0).min(frame_h).min((view_bottom - view_top).max(0.0)))
}

/// The keyboard animation's duration and curve (`ime_curve`).
unsafe fn keyboard_animation(note: ObjcId) -> (f64, u32) {
    let info: ObjcId = msg_send![note, userInfo];
    let duration: ObjcId = msg_send![info, objectForKey: UIKeyboardAnimationDurationUserInfoKey];
    let duration: f64 = if duration == nil { 0.0 } else { msg_send![duration, doubleValue] };
    let curve: ObjcId = msg_send![info, objectForKey: UIKeyboardAnimationCurveUserInfoKey];
    let curve: i64 = if curve == nil { 0 } else { msg_send![curve, intValue] };
    let curve = if curve > 3 { curve >> 16 } else { curve };
    // UIViewAnimationCurve: EaseInOut, EaseIn, EaseOut, Linear (the keyboard's private curve 7
    // falls through to linear, as before)
    (duration, if (0..=3).contains(&curve) { curve as u32 } else { input::ime_curve::LINEAR })
}

/// `HalKeyboardObserver`: the keyboard's frame (one source of truth: the change-frame
/// notifications, which also come for show / hide), input-mode and hardware-keyboard changes.
fn define_observer() -> *const Class {
    fn owner_view() -> Option<&'static Win> {
        text().owner.get().filter(|w| !w.retired.get()).or_else(|| app().top_window())
    }
    extern "C" fn will_change_frame(_: &Object, _: Sel, note: ObjcId) {
        shielded("keyboard", || unsafe {
            let Some(win) = owner_view() else { return };
            let (open, height) = keyboard_cover(note, win.view);
            let (duration, curve) = keyboard_animation(note);
            let id = win.id.get();
            win.push(|q| q.ime_animate(id, height, open, duration, curve));
        });
    }
    extern "C" fn did_change_frame(_: &Object, _: Sel, note: ObjcId) {
        shielded("keyboard", || unsafe {
            let Some(win) = owner_view() else { return };
            let (open, height) = keyboard_cover(note, win.view);
            let id = win.id.get();
            win.push(|q| q.ime_resize(id, height, open));
        });
    }
    // A new keyboard language: the view reloads its input views so iOS asks again for the
    // traits that depend on it (autocorrection for CJK).
    extern "C" fn input_mode_changed(_: &Object, _: Sel, _: ObjcId) {
        let view = text().view.get();
        if view != nil {
            unsafe {
                let () = msg_send![view, reloadInputViews];
            }
        }
    }
    extern "C" fn physical_keyboard_changed(_: &Object, _: Sel, _: ObjcId) {
        shielded("hardware-keyboard", || {
            let t = text();
            let connected = physical_keyboard_connected();
            if t.physical_keyboard.replace(connected) == connected {
                return;
            }
            for win in app().live_windows() {
                let id = win.id.get();
                win.push(|q| q.physical_keyboard(id, connected));
            }
        });
    }
    let mut decl = ClassDecl::new("HalKeyboardObserver", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(keyboardWillChangeFrame:), will_change_frame as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(keyboardDidChangeFrame:), did_change_frame as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(inputModeDidChange:), input_mode_changed as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(physicalKeyboardChanged:), physical_keyboard_changed as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.register()
}

/// The windows a new session's window joins hear whether a hardware keyboard is there.
pub(super) fn announce_keyboard(win: &Win) {
    let connected = text().physical_keyboard.get();
    let id = win.id.get();
    win.push(|q| q.physical_keyboard(id, connected));
}
