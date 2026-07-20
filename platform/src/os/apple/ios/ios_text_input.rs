// =============================================================================
// iOS text input: MakepadTextView (a real UITextView subclass)
// =============================================================================
//
// Using a real UITextView as the hidden editing client gives us the native
// language HUD and the full input-mode cycle (a hand-rolled UITextInput view
// never gets them). The view is the input/nav/selection authority; makepad
// mirrors it via full_state_sync and renders the text + caret itself. This
// module also holds the shared hardware arrow/doc-nav UIKeyCommand machinery.

use crate::{
    event::keyboard::KeyCode,
    os::{
        apple::apple_sys::*,
        apple::apple_util::nsstring_to_string,
        apple::ios_app::IosApp,
        apple::ios_app::{IOS_TEXT_INPUT_CARET_HEIGHT, IOS_TEXT_INPUT_TARGET_HEIGHT},
    },
};

// Import shared iOS helpers from ios_delegates.
use super::ios_delegates::dispatch_makepad_key_code;

/// Convert two UTF-16 indices to character offsets in a single pass.
/// Assumes end >= start.
fn utf16_indices_to_char_offsets(
    text: &str,
    utf16_start: usize,
    utf16_end: usize,
) -> (usize, usize) {
    let mut utf16_count = 0;
    let char_len = text.chars().count();
    let mut char_start = char_len;
    let mut found_start = false;
    for (char_idx, c) in text.chars().enumerate() {
        if !found_start && utf16_count >= utf16_start {
            char_start = char_idx;
            found_start = true;
        }
        if utf16_count >= utf16_end {
            return (char_start, char_idx);
        }
        utf16_count += c.len_utf16();
    }
    if !found_start {
        char_start = char_len;
    }
    (char_start, char_len)
}

// iOS 13.4+ UIKeyInput constants for Home/End/PageUp/PageDown. The iOS 11
// deployment floor can't link them (missing symbols crash at launch on older
// systems), so resolve them at runtime via dlsym; each is nil if unavailable.
struct DocNavInputs {
    home: ObjcId,
    end: ObjcId,
    page_up: ObjcId,
    page_down: ObjcId,
}
// The values are immortal, thread-safe NSString singletons.
unsafe impl Send for DocNavInputs {}
unsafe impl Sync for DocNavInputs {}

unsafe fn lookup_uikit_string_const(name: &[u8]) -> ObjcId {
    unsafe extern "C" {
        fn dlsym(handle: *mut std::ffi::c_void, symbol: *const i8) -> *mut std::ffi::c_void;
    }
    // RTLD_DEFAULT (-2) searches every loaded image; UIKit is already linked.
    let rtld_default = -2isize as *mut std::ffi::c_void;
    let sym = dlsym(rtld_default, name.as_ptr() as *const i8);
    if sym.is_null() {
        nil
    } else {
        // The symbol holds an NSString *const; deref once to read the pointer.
        *(sym as *const ObjcId)
    }
}

fn doc_nav_inputs() -> &'static DocNavInputs {
    static CACHE: std::sync::OnceLock<DocNavInputs> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| unsafe {
        DocNavInputs {
            home: lookup_uikit_string_const(b"UIKeyInputHome\0"),
            end: lookup_uikit_string_const(b"UIKeyInputEnd\0"),
            page_up: lookup_uikit_string_const(b"UIKeyInputPageUp\0"),
            page_down: lookup_uikit_string_const(b"UIKeyInputPageDown\0"),
        }
    })
}

// True once the iOS 13.4+ doc-nav constants resolve (so their UIKeyCommands are
// registered). ios_delegates gates its focused-skip on this so that on older iOS
// these keys keep navigating via the one-shot pressesBegan path.
pub(crate) fn doc_nav_keycommands_available() -> bool {
    doc_nav_inputs().home != nil
}

// Hardware-keyboard nav-key UIKeyCommand machinery, shared by both makepad text
// views. UIKeyModifierFlags share bit positions with NSEventModifierFlags.
const NAV_KEY_MOD_SHIFT: u64 = 1 << 17;
const NAV_KEY_MOD_ALT: u64 = 1 << 19;
const NAV_KEY_MOD_CMD: u64 = 1 << 20;

unsafe fn add_nav_key_command(array: ObjcId, input: ObjcId, modifier_flags: u64) {
    if input == nil {
        return;
    }
    let cmd: ObjcId = msg_send![
        class!(UIKeyCommand),
        keyCommandWithInput: input
        modifierFlags: modifier_flags
        action: sel!(handleMakepadKeyCommand:)
    ];
    // iOS 15+ only; harmless (and skipped) on older systems.
    let responds: BOOL =
        msg_send![cmd, respondsToSelector: sel!(setWantsPriorityOverSystemBehavior:)];
    if responds == YES {
        let () = msg_send![cmd, setWantsPriorityOverSystemBehavior: YES];
    }
    let () = msg_send![array, addObject: cmd];
}

unsafe fn nav_key_code_for_input(input: ObjcId) -> KeyCode {
    let doc = doc_nav_inputs();
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
    for (constant, key_code) in pairs {
        if constant != nil {
            let eq: BOOL = msg_send![input, isEqualToString: constant];
            if eq == YES {
                return key_code;
            }
        }
    }
    KeyCode::Unknown
}

// Reclaim hardware arrow/doc-nav keys from UIKit's built-in text navigation:
// wantsPriorityOverSystemBehavior wins the key AND makes UIKit auto-repeat it while held.
unsafe fn build_nav_key_commands() -> ObjcId {
    let array: ObjcId = msg_send![class!(NSMutableArray), array];
    let arrow_inputs = [
        UIKeyInputLeftArrow,
        UIKeyInputRightArrow,
        UIKeyInputUpArrow,
        UIKeyInputDownArrow,
    ];
    let arrow_masks = [
        0,
        NAV_KEY_MOD_SHIFT,
        NAV_KEY_MOD_ALT,
        NAV_KEY_MOD_CMD,
        NAV_KEY_MOD_SHIFT | NAV_KEY_MOD_ALT,
        NAV_KEY_MOD_SHIFT | NAV_KEY_MOD_CMD,
    ];
    for input in arrow_inputs {
        for mask in arrow_masks {
            add_nav_key_command(array, input, mask);
        }
    }
    // Home/End also take the Cmd text-boundary move; Page keys are bare/Shift only.
    let doc = doc_nav_inputs();
    let line_edge_masks = [
        0,
        NAV_KEY_MOD_SHIFT,
        NAV_KEY_MOD_CMD,
        NAV_KEY_MOD_SHIFT | NAV_KEY_MOD_CMD,
    ];
    for input in [doc.home, doc.end] {
        for mask in line_edge_masks {
            add_nav_key_command(array, input, mask);
        }
    }
    for input in [doc.page_up, doc.page_down] {
        for mask in [0, NAV_KEY_MOD_SHIFT] {
            add_nav_key_command(array, input, mask);
        }
    }
    array
}

unsafe fn handle_nav_key_command(sender: ObjcId) {
    let input: ObjcId = msg_send![sender, input];
    if input == nil {
        return;
    }
    let key_code = nav_key_code_for_input(input);
    if key_code.is_unknown() {
        return;
    }
    let modifier_flags: u64 = msg_send![sender, modifierFlags];
    let modifiers = crate::os::apple::apple_util::key_modifiers_from_flags(modifier_flags);
    // Balanced down/up keeps keys_down clean (mirrors the floating cursor).
    dispatch_makepad_key_code(key_code, modifiers, true);
    dispatch_makepad_key_code(key_code, modifiers, false);
}

/// Defines the main text input view conforming to UITextInput protocol.
/// This replaces the hidden UITextField and provides full IME support.
/// Defines `MakepadTextView`: a real `UITextView` subclass used as the iOS editing
/// client. A real text view IS a full system keyboard client, so iOS gives us the
/// language HUD and the full input-mode cycle (which a hand-rolled UITextInput view
/// never gets). We keep it invisible (clear text/tint) and region-framed; makepad
/// still renders the text + caret. The text view is the input/nav/selection
/// authority and makepad mirrors it via `full_state_sync` (see the delegate methods).
pub fn define_makepad_text_view() -> *const Class {
    let mut decl = ClassDecl::new("MakepadTextView", class!(UITextView)).unwrap();

    // Caret geometry (view-local), kept current by IosApp::set_ime_position so the
    // IME candidate window / accent popup anchor at makepad's caret rather than the
    // text view's own (invisible) internal layout.
    decl.add_ivar::<f64>("ime_pos_x");
    decl.add_ivar::<f64>("ime_pos_y");
    // Real caret-line height (native points) so the IME candidate clearance can
    // scale with the font size instead of using a fixed proxy.
    decl.add_ivar::<f64>("ime_line_height");
    // Whether the focused makepad field is multiline (set by configure_keyboard).
    decl.add_ivar::<bool>("_is_multiline");
    decl.add_ivar::<bool>("_submit_on_enter");
    decl.add_ivar::<bool>("_is_read_only");
    // Set true while makepad is pushing text in (set_ime_text); the delegate
    // callbacks that push triggers must not echo straight back to makepad.
    decl.add_ivar::<BOOL>("programmatic_update");

    extern "C" fn can_become_focused(_: &Object, _: Sel) -> BOOL {
        YES
    }
    extern "C" fn focus_effect(_: &Object, _: Sel) -> ObjcId {
        nil
    }
    // Empty focus geometry so Full Keyboard Access has nothing to draw its cursor over.
    extern "C" fn accessibility_path(_: &Object, _: Sel) -> ObjcId {
        unsafe {
            let zero = NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize { width: 0.0, height: 0.0 },
            };
            msg_send![class!(UIBezierPath), bezierPathWithRect: zero]
        }
    }
    // Never intercept touches: makepad renders the text and owns hit-testing and
    // caret placement; the text view only drives the keyboard + IME.
    extern "C" fn point_inside(_: &Object, _: Sel, _: NSPoint, _: ObjcId) -> BOOL {
        NO
    }

    // Override UITextView's geometry (which would point at its invisible internal
    // layout) so IME popups anchor at makepad's caret.
    extern "C" fn caret_rect_for_position(this: &Object, _: Sel, _pos: ObjcId) -> NSRect {
        unsafe {
            let x = *this.get_ivar::<f64>("ime_pos_x");
            let y = *this.get_ivar::<f64>("ime_pos_y");
            NSRect {
                origin: NSPoint {
                    x,
                    y: y - IOS_TEXT_INPUT_CARET_HEIGHT,
                },
                size: NSSize {
                    // Zero size: no iOS caret drawn (makepad draws its own); the origin
                    // still anchors the accent popup. iOS clamps zero width alone to a
                    // thin line, so collapse the height too.
                    width: 0.0,
                    height: 0.0,
                },
            }
        }
    }
    extern "C" fn first_rect_for_range(this: &Object, _: Sel, _range: ObjcId) -> NSRect {
        unsafe {
            let x = *this.get_ivar::<f64>("ime_pos_x");
            // ime_pos_y is the caret-line BOTTOM in this view's local (y-down) space.
            let y = *this.get_ivar::<f64>("ime_pos_y");
            let line_height = *this.get_ivar::<f64>("ime_line_height");
            // Fall back to the sliver height if no real line height was supplied.
            let height = if line_height > 1.0 {
                line_height
            } else {
                IOS_TEXT_INPUT_TARGET_HEIGHT
            };

            // Only hand iOS a real line box while actually COMPOSING marked text
            // (when the CJK candidate window is up). The same method drives iOS's
            // autocorrect/suggestion highlight when NOT composing, and a real
            // height there makes iOS render an oversized highlight / stray caret
            // over English text. So outside composition we return the degenerate
            // rect, exactly like before the IME-positioning rework.
            let marked_range: ObjcId = msg_send![this, markedTextRange];
            if marked_range == nil {
                return NSRect {
                    origin: NSPoint {
                        x,
                        y: y - 1.5 * IOS_TEXT_INPUT_TARGET_HEIGHT,
                    },
                    size: NSSize {
                        width: 1.0,
                        height: 0.01,
                    },
                };
            }

            // Composing: return the TRUE composing-line box (top-left at the line
            // top, real height) so iOS flips the candidate window around the real
            // top/bottom edges - consistent spacing at any screen position, unlike
            // the old degenerate point (huge gap at the bottom, overlap mid-screen).
            // Inflate it by a fraction of the line height on both edges so the
            // candidate sits a bit further from the text than iOS's tight native
            // default (tune via the multiplier). This does NOT draw a caret - only
            // caret_rect_for_position does, and it stays zero-size.
            let clearance = height * 0.3;
            NSRect {
                origin: NSPoint {
                    x,
                    y: y - height - clearance,
                },
                size: NSSize {
                    width: 1.0,
                    height: height + 2.0 * clearance,
                },
            }
        }
    }

    // Report no selection rects: the system uses these to place the selection
    // highlight + grab handles, and makepad renders the real selection.
    extern "C" fn selection_rects_for_range(_: &Object, _: Sel, _range: ObjcId) -> ObjcId {
        unsafe { msg_send![class!(NSArray), array] }
    }

    // The text view is the authority; on any change push its full state to makepad
    // as one `full_state_sync` (text + selection, char offsets). Reuses the existing
    // SelectionChanged queue path, which maps to TextInputEvent.full_state_sync.
    unsafe fn forward_state_to_makepad(this: &Object) {
        let programmatic: BOOL = *this.get_ivar::<BOOL>("programmatic_update");
        if programmatic == YES {
            return;
        }
        let view = this as *const _ as ObjcId;
        let ns_text: ObjcId = msg_send![view, text];
        let text = if ns_text == nil {
            String::new()
        } else {
            nsstring_to_string(ns_text)
        };
        let sel: NSRange = msg_send![view, selectedRange];
        let (char_start, char_end) = utf16_indices_to_char_offsets(
            &text,
            sel.location as usize,
            (sel.location + sel.length) as usize,
        );
        IosApp::send_text_selection_changed(text, char_start, char_end);
    }

    extern "C" fn text_view_did_change(this: &Object, _: Sel, _tv: ObjcId) {
        unsafe { forward_state_to_makepad(this) }
    }
    extern "C" fn text_view_did_change_selection(this: &Object, _: Sel, _tv: ObjcId) {
        unsafe { forward_state_to_makepad(this) }
    }
    // Route Return through makepad's submit/newline logic (should_submit) rather
    // than letting the text view insert a raw newline; if makepad decides newline,
    // it comes back via set_ime_text.
    extern "C" fn should_change_text(
        this: &Object,
        _: Sel,
        _tv: ObjcId,
        _range: NSRange,
        text: ObjcId,
    ) -> BOOL {
        unsafe {
            if text != nil {
                let s = nsstring_to_string(text);
                if s == "\n" {
                    // Multiline newlines insert into the view (soft keyboard here,
                    // hardware via insertText) so they sync in-order; single-line submits.
                    let is_multiline: bool = *this.get_ivar::<bool>("_is_multiline");
                    if is_multiline {
                        return YES;
                    }
                    IosApp::send_return_key();
                    return NO;
                }
                if s == "\t" {
                    // Hardware Tab moves focus (handled by makepad's nav_control via pressesBegan),
                    // so never insert a tab character into the field.
                    return NO;
                }
            }
            YES
        }
    }

    // Hardware arrow/doc-nav auto-repeat: these UIKeyCommands win over the
    // UITextView's native nav, so arrows route through makepad and UIKit repeats them.
    extern "C" fn key_commands(_this: &Object, _: Sel) -> ObjcId {
        unsafe { build_nav_key_commands() }
    }

    extern "C" fn handle_makepad_key_command(_this: &Object, _: Sel, sender: ObjcId) {
        unsafe { handle_nav_key_command(sender) }
    }

    unsafe {
        decl.add_method(
            sel!(canBecomeFocused),
            can_become_focused as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(focusEffect),
            focus_effect as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(accessibilityPath),
            accessibility_path as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(pointInside:withEvent:),
            point_inside as extern "C" fn(&Object, Sel, NSPoint, ObjcId) -> BOOL,
        );
        decl.add_method(
            sel!(caretRectForPosition:),
            caret_rect_for_position as extern "C" fn(&Object, Sel, ObjcId) -> NSRect,
        );
        decl.add_method(
            sel!(firstRectForRange:),
            first_rect_for_range as extern "C" fn(&Object, Sel, ObjcId) -> NSRect,
        );
        decl.add_method(
            sel!(selectionRectsForRange:),
            selection_rects_for_range as extern "C" fn(&Object, Sel, ObjcId) -> ObjcId,
        );
        decl.add_method(
            sel!(textViewDidChange:),
            text_view_did_change as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(textViewDidChangeSelection:),
            text_view_did_change_selection as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(textView:shouldChangeTextInRange:replacementText:),
            should_change_text as extern "C" fn(&Object, Sel, ObjcId, NSRange, ObjcId) -> BOOL,
        );
        decl.add_method(
            sel!(keyCommands),
            key_commands as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(handleMakepadKeyCommand:),
            handle_makepad_key_command as extern "C" fn(&Object, Sel, ObjcId),
        );
    }

    decl.register()
}
