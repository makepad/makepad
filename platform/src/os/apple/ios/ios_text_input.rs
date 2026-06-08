// =============================================================================
// UITextInput Protocol Implementation for IME Support
// =============================================================================
//
// This module implements the UITextInput protocol for iOS IME (Input Method Editor)
// support. It provides:
//   - MakepadTextPosition: Custom UITextPosition subclass
//   - MakepadTextRange: Custom UITextRange subclass
//   - MakepadTextInputView: Main UIView subclass conforming to UITextInput
//
// The UITextInput protocol is iOS's interface for complex text input including:
//   - IME composition (marked text) for CJK languages
//   - Cursor positioning and selection
//   - Text replacement and autocorrect integration
//   - Floating cursor (keyboard trackpad) support

use {
    crate::{
        event::keyboard::KeyCode,
        event::KeyEvent,
        os::{
            apple::apple_sys::*,
            apple::apple_util::{nsstring_to_string, str_to_nsstring},
            apple::ios::ios_event::IosEvent,
            apple::ios_app::IosApp,
            apple::ios_app::{
                get_ios_class_global, IOS_TEXT_INPUT_CARET_HEIGHT,
                UI_TEXT_AUTOCORRECTION_DEFAULT, UI_TEXT_AUTOCORRECTION_NO,
            },
        },
    },
    makepad_objc_sys::runtime::Protocol,
};

// Import shared iOS helpers from ios_delegates.
use super::ios_delegates::{dispatch_makepad_key_code, try_with_ios_app};

/// Count UTF-16 code units in a Rust string.
/// This is needed because iOS NSString uses UTF-16 internally, and UITextInput
/// positions are measured in UTF-16 code units, not characters or bytes.
/// For ASCII: 1 char = 1 UTF-16 code unit
/// For most Unicode (including CJK): 1 char = 1 UTF-16 code unit
/// For emoji and other astral plane characters: 1 char = 2 UTF-16 code units (surrogate pair)
fn utf16_len(s: &str) -> i64 {
    s.encode_utf16().count() as i64
}

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

fn char_to_utf16_index(text: &str, char_index: usize) -> usize {
    text.chars().take(char_index).map(|c| c.len_utf16()).sum()
}

fn char_index_to_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

fn utf16_range_to_string(text: &str, start: usize, end: usize) -> String {
    let utf16_len = text.encode_utf16().count();
    let start = start.min(utf16_len);
    let end = end.min(utf16_len).max(start);
    let (char_start, char_end) = utf16_indices_to_char_offsets(text, start, end);
    let byte_start = char_index_to_byte_index(text, char_start);
    let byte_end = char_index_to_byte_index(text, char_end);
    text[byte_start..byte_end].to_string()
}

/// Defines a custom UITextPosition subclass.
/// UITextInput protocol requires custom position/range classes (token-based, not integer-based).
pub fn define_makepad_text_position() -> *const Class {
    let mut decl = ClassDecl::new("MakepadTextPosition", class!(UITextPosition)).unwrap();

    decl.add_ivar::<i64>("_offset");

    extern "C" fn get_offset(this: &Object, _: Sel) -> i64 {
        unsafe { *this.get_ivar::<i64>("_offset") }
    }

    extern "C" fn set_offset(this: &mut Object, _: Sel, offset: i64) {
        unsafe {
            this.set_ivar::<i64>("_offset", offset);
        }
    }

    extern "C" fn position_with_offset(cls: &Class, _: Sel, offset: i64) -> ObjcId {
        unsafe {
            let obj: ObjcId = msg_send![cls, alloc];
            let obj: ObjcId = msg_send![obj, init];
            (*obj).set_ivar::<i64>("_offset", offset);
            let obj: ObjcId = msg_send![obj, autorelease];
            obj
        }
    }

    unsafe {
        decl.add_method(
            sel!(offset),
            get_offset as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(setOffset:),
            set_offset as extern "C" fn(&mut Object, Sel, i64),
        );
        decl.add_class_method(
            sel!(positionWithOffset:),
            position_with_offset as extern "C" fn(&Class, Sel, i64) -> ObjcId,
        );
    }

    return decl.register();
}

/// Defines a custom UITextRange subclass.
/// Stores start and end offsets directly, creates positions on demand.
pub fn define_makepad_text_range() -> *const Class {
    let mut decl = ClassDecl::new("MakepadTextRange", class!(UITextRange)).unwrap();

    // Store offsets directly instead of position objects to avoid memory management issues
    decl.add_ivar::<i64>("_startOffset");
    decl.add_ivar::<i64>("_endOffset");

    extern "C" fn get_start(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let offset: i64 = *this.get_ivar::<i64>("_startOffset");
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: offset]
        }
    }

    extern "C" fn get_end(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let offset: i64 = *this.get_ivar::<i64>("_endOffset");
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: offset]
        }
    }

    extern "C" fn is_empty(this: &Object, _: Sel) -> BOOL {
        unsafe {
            let start: i64 = *this.get_ivar::<i64>("_startOffset");
            let end: i64 = *this.get_ivar::<i64>("_endOffset");
            if start == end {
                YES
            } else {
                NO
            }
        }
    }

    // Simplified class method using the class parameter directly
    extern "C" fn range_with_start_end(cls: &Class, _: Sel, start: i64, end: i64) -> ObjcId {
        unsafe {
            let obj: ObjcId = msg_send![cls, alloc];
            let obj: ObjcId = msg_send![obj, init];
            (*obj).set_ivar::<i64>("_startOffset", start);
            (*obj).set_ivar::<i64>("_endOffset", end);
            let obj: ObjcId = msg_send![obj, autorelease];
            obj
        }
    }

    extern "C" fn range_with_positions(cls: &Class, _: Sel, start: ObjcId, end: ObjcId) -> ObjcId {
        unsafe {
            let obj: ObjcId = msg_send![cls, alloc];
            let obj: ObjcId = msg_send![obj, init];
            let start_offset: i64 = if start != nil {
                msg_send![start, offset]
            } else {
                0
            };
            let end_offset: i64 = if end != nil {
                msg_send![end, offset]
            } else {
                0
            };
            (*obj).set_ivar::<i64>("_startOffset", start_offset);
            (*obj).set_ivar::<i64>("_endOffset", end_offset);
            let obj: ObjcId = msg_send![obj, autorelease];
            obj
        }
    }

    unsafe {
        decl.add_method(
            sel!(start),
            get_start as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(sel!(end), get_end as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(
            sel!(isEmpty),
            is_empty as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_class_method(
            sel!(rangeWithStart:end:),
            range_with_start_end as extern "C" fn(&Class, Sel, i64, i64) -> ObjcId,
        );
        decl.add_class_method(
            sel!(rangeWithStartPosition:endPosition:),
            range_with_positions as extern "C" fn(&Class, Sel, ObjcId, ObjcId) -> ObjcId,
        );
    }

    return decl.register();
}

/// Minimal UITextSelectionRect implementation used by iOS 16+
/// UITextSelectionDisplayInteraction runtime integration.
pub fn define_makepad_selection_rect() -> *const Class {
    let mut decl = ClassDecl::new("MakepadSelectionRect", class!(UITextSelectionRect)).unwrap();

    decl.add_ivar::<f64>("x");
    decl.add_ivar::<f64>("y");
    decl.add_ivar::<f64>("w");
    decl.add_ivar::<f64>("h");
    decl.add_ivar::<BOOL>("contains_start");
    decl.add_ivar::<BOOL>("contains_end");

    extern "C" fn rect(this: &Object, _: Sel) -> NSRect {
        unsafe {
            NSRect {
                origin: NSPoint {
                    x: *this.get_ivar::<f64>("x"),
                    y: *this.get_ivar::<f64>("y"),
                },
                size: NSSize {
                    width: *this.get_ivar::<f64>("w"),
                    height: *this.get_ivar::<f64>("h"),
                },
            }
        }
    }

    extern "C" fn writing_direction(_: &Object, _: Sel) -> i64 {
        0 // NSWritingDirectionNatural
    }

    extern "C" fn contains_start(this: &Object, _: Sel) -> BOOL {
        unsafe { *this.get_ivar::<BOOL>("contains_start") }
    }

    extern "C" fn contains_end(this: &Object, _: Sel) -> BOOL {
        unsafe { *this.get_ivar::<BOOL>("contains_end") }
    }

    extern "C" fn is_vertical(_: &Object, _: Sel) -> BOOL {
        NO
    }

    extern "C" fn rect_with_geometry(
        cls: &Class,
        _: Sel,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        contains_start: BOOL,
        contains_end: BOOL,
    ) -> ObjcId {
        unsafe {
            let obj: ObjcId = msg_send![cls, alloc];
            let obj: ObjcId = msg_send![obj, init];
            (*obj).set_ivar::<f64>("x", x);
            (*obj).set_ivar::<f64>("y", y);
            (*obj).set_ivar::<f64>("w", w);
            (*obj).set_ivar::<f64>("h", h);
            (*obj).set_ivar::<BOOL>("contains_start", contains_start);
            (*obj).set_ivar::<BOOL>("contains_end", contains_end);
            let obj: ObjcId = msg_send![obj, autorelease];
            obj
        }
    }

    unsafe {
        decl.add_method(sel!(rect), rect as extern "C" fn(&Object, Sel) -> NSRect);
        decl.add_method(
            sel!(writingDirection),
            writing_direction as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(containsStart),
            contains_start as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(containsEnd),
            contains_end as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(isVertical),
            is_vertical as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_class_method(
            sel!(rectWithX:y:w:h:containsStart:containsEnd:),
            rect_with_geometry
                as extern "C" fn(&Class, Sel, f64, f64, f64, f64, BOOL, BOOL) -> ObjcId,
        );
    }

    decl.register()
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

/// Defines the main text input view conforming to UITextInput protocol.
/// This replaces the hidden UITextField and provides full IME support.
pub fn define_text_input_view() -> *const Class {
    let mut decl = ClassDecl::new("MakepadTextInputView", class!(UIView)).unwrap();

    // Instance variables for text input state
    decl.add_ivar::<ObjcId>("markedText"); // NSMutableAttributedString
    decl.add_ivar::<i64>("markedTextStart"); // UTF-16 start offset of active marked text
    decl.add_ivar::<ObjcId>("textBuffer"); // NSMutableString - tracks text for iOS context
    decl.add_ivar::<i64>("cursorPosition"); // Current cursor position
    decl.add_ivar::<i64>("selectionStart"); // Selection start
    decl.add_ivar::<i64>("selectionEnd"); // Selection end
    decl.add_ivar::<ObjcId>("_inputDelegate"); // id<UITextInputDelegate>
    decl.add_ivar::<ObjcId>("_tokenizer"); // UITextInputStringTokenizer
                                           // IME position stored in ivars to avoid re-entrant borrow issues
    decl.add_ivar::<f64>("ime_pos_x");
    decl.add_ivar::<f64>("ime_pos_y");

    // Keyboard configuration ivars (UITextInputTraits)
    decl.add_ivar::<i64>("_keyboard_type"); // UIKeyboardType
    decl.add_ivar::<i64>("_autocapitalization_type"); // UITextAutocapitalizationType
    decl.add_ivar::<i64>("_autocorrection_type"); // UITextAutocorrectionType (-1 = use CJK logic)
    decl.add_ivar::<i64>("_return_key_type"); // UIReturnKeyType
    decl.add_ivar::<bool>("_secure_text_entry"); // isSecureTextEntry
    decl.add_ivar::<bool>("_is_multiline"); // whether the focused field is multiline

    // Floating cursor state (keyboard trackpad)
    decl.add_ivar::<BOOL>("floating_cursor_active");
    decl.add_ivar::<f64>("floating_cursor_last_x");
    decl.add_ivar::<f64>("floating_cursor_last_y");

    // Selection-handle anchor points for iOS 16+ UITextSelectionDisplayInteraction.
    decl.add_ivar::<f64>("selection_handle_start_x");
    decl.add_ivar::<f64>("selection_handle_start_y");
    decl.add_ivar::<f64>("selection_handle_end_x");
    decl.add_ivar::<f64>("selection_handle_end_y");
    decl.add_ivar::<BOOL>("selection_handles_visible");
    decl.add_ivar::<BOOL>("accept_system_selection_changes");
    decl.add_ivar::<BOOL>("suppress_system_selection_changes");

    // ==========================================================================
    // UIResponder methods
    // ==========================================================================

    extern "C" fn can_become_first_responder(_: &Object, _: Sel) -> BOOL {
        YES
    }

    extern "C" fn can_become_focused(_: &Object, _: Sel) -> BOOL {
        NO
    }

    extern "C" fn is_accessibility_element(_: &Object, _: Sel) -> BOOL {
        NO
    }

    extern "C" fn accessibility_elements_hidden(_: &Object, _: Sel) -> BOOL {
        YES
    }

    extern "C" fn accessibility_responds_to_user_interaction(_: &Object, _: Sel) -> BOOL {
        NO
    }

    extern "C" fn accessibility_frame(this: &Object, _: Sel) -> NSRect {
        unsafe {
            let view = this as *const _ as ObjcId;
            let bounds: NSRect = msg_send![view, bounds];
            msg_send![view, convertRect: bounds toView: nil as ObjcId]
        }
    }

    unsafe fn set_suppress_system_selection_changes(this: &Object, value: BOOL) {
        (*(this as *const _ as *mut Object))
            .set_ivar::<BOOL>("suppress_system_selection_changes", value);
    }

    extern "C" fn point_inside(_: &Object, _: Sel, _: NSPoint, _: ObjcId) -> BOOL {
        NO
    }

    // ==========================================================================
    // UIKeyInput protocol methods
    // ==========================================================================

    extern "C" fn has_text(this: &Object, _: Sel) -> BOOL {
        unsafe {
            let buffer: ObjcId = *this.get_ivar("textBuffer");
            if buffer != nil {
                let len: u64 = msg_send![buffer, length];
                if len > 0 {
                    return YES;
                }
            }
            let marked: ObjcId = *this.get_ivar("markedText");
            if marked != nil {
                let len: u64 = msg_send![marked, length];
                if len > 0 {
                    return YES;
                }
            }
            NO
        }
    }

    // Helper to get or create the text buffer
    unsafe fn get_text_buffer(this: &Object) -> ObjcId {
        let buffer: ObjcId = *this.get_ivar("textBuffer");
        if buffer != nil {
            return buffer;
        }
        let new_buffer: ObjcId = msg_send![class!(NSMutableString), alloc];
        let new_buffer: ObjcId = msg_send![new_buffer, init];
        (*(this as *const _ as *mut Object)).set_ivar("textBuffer", new_buffer);
        new_buffer
    }

    unsafe fn normalized_selection_range(this: &Object) -> (u64, u64) {
        let buffer = get_text_buffer(this);
        let buffer_len: u64 = msg_send![buffer, length];
        let cursor: i64 = *this.get_ivar("cursorPosition");
        let sel_start: i64 = *this.get_ivar("selectionStart");
        let sel_end: i64 = *this.get_ivar("selectionEnd");

        let start = if sel_start != sel_end {
            sel_start
        } else {
            cursor
        };
        let end = if sel_start != sel_end {
            sel_end
        } else {
            cursor
        };

        let start = (start.max(0) as u64).min(buffer_len);
        let end = (end.max(0) as u64).min(buffer_len);
        (start.min(end), start.max(end))
    }

    unsafe fn set_selection_utf16(this: &Object, start: u64, end: u64) {
        let this = this as *const _ as *mut Object;
        (*this).set_ivar("selectionStart", start as i64);
        (*this).set_ivar("selectionEnd", end as i64);
        (*this).set_ivar("cursorPosition", end as i64);
    }

    unsafe fn marked_text_utf16_len(this: &Object) -> u64 {
        let marked_text: ObjcId = *this.get_ivar("markedText");
        if marked_text == nil {
            return 0;
        }
        msg_send![marked_text, length]
    }

    unsafe fn visible_text_utf16_len(this: &Object) -> i64 {
        let buffer = get_text_buffer(this);
        let buffer_len: u64 = msg_send![buffer, length];
        (buffer_len + marked_text_utf16_len(this)) as i64
    }

    unsafe fn visible_text_string(this: &Object) -> String {
        let buffer = get_text_buffer(this);
        let buffer_string = nsstring_to_string(buffer);
        let mut visible_text = buffer_string.clone();

        let marked_text: ObjcId = *this.get_ivar("markedText");
        if marked_text != nil {
            let marked_len: u64 = msg_send![marked_text, length];
            if marked_len > 0 {
                let marked_start: i64 = *this.get_ivar("markedTextStart");
                let text_string: ObjcId = msg_send![marked_text, string];
                let marked_string = nsstring_to_string(text_string);
                let insert_utf16 =
                    (marked_start.max(0) as usize).min(buffer_string.encode_utf16().count());
                let (insert_char, _) =
                    utf16_indices_to_char_offsets(&buffer_string, insert_utf16, insert_utf16);
                let insert_byte = char_index_to_byte_index(&visible_text, insert_char);
                visible_text.insert_str(insert_byte, &marked_string);
            }
        }

        visible_text
    }

    unsafe fn replace_buffer_range(this: &Object, start: u64, end: u64, text: ObjcId) -> u64 {
        let buffer = get_text_buffer(this);
        let buffer_len: u64 = msg_send![buffer, length];
        let start = start.min(buffer_len);
        let end = end.min(buffer_len).max(start);

        if start < end {
            let delete_range = NSRange {
                location: start,
                length: end - start,
            };
            let () = msg_send![buffer, deleteCharactersInRange: delete_range];
        }

        let insert_len: u64 = if text != nil {
            msg_send![text, length]
        } else {
            0
        };
        if insert_len > 0 {
            let new_len: u64 = msg_send![buffer, length];
            let insert_pos = start.min(new_len);
            let () = msg_send![buffer, insertString: text atIndex: insert_pos];
        }

        let new_cursor = start + insert_len;
        set_selection_utf16(this, new_cursor, new_cursor);
        new_cursor
    }

    unsafe fn string_object_from_plain_or_attributed_text(text: ObjcId) -> ObjcId {
        if text == nil {
            return str_to_nsstring("");
        }
        let is_attributed: BOOL = msg_send![text, isKindOfClass: class!(NSAttributedString)];
        if is_attributed == YES {
            msg_send![text, string]
        } else {
            text
        }
    }

    unsafe fn make_placeholder_object() -> ObjcId {
        let obj: ObjcId = msg_send![class!(NSObject), new];
        let obj: ObjcId = msg_send![obj, autorelease];
        obj
    }

    extern "C" fn insert_text(this: &Object, _: Sel, text: ObjcId) {
        unsafe {
            let string = nsstring_to_string(text);

            // Return reaches here through UIKit text input. Single-line submits;
            // multiline inserts a literal newline.
            if string == "\n" {
                let is_multiline: bool = *this.get_ivar("_is_multiline");
                if !is_multiline {
                    IosApp::send_return_key();
                    return;
                }
            }

            set_suppress_system_selection_changes(this, YES);
            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");

            // Notify that text and selection will change
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, selectionWillChange: this as *const _ as ObjcId];
            }

            let buffer = get_text_buffer(this);
            let buffer_string = nsstring_to_string(buffer);
            let marked_text: ObjcId = *this.get_ivar("markedText");
            let had_marked = if marked_text != nil {
                let len: u64 = msg_send![marked_text, length];
                len > 0
            } else {
                false
            };
            let (range_start, range_end) = if had_marked {
                let marked_start: i64 = *this.get_ivar("markedTextStart");
                let marked_start =
                    (marked_start.max(0) as u64).min(buffer_string.encode_utf16().count() as u64);
                (marked_start, marked_start)
            } else {
                normalized_selection_range(this)
            };

            // Clear marked text BEFORE sending to Makepad
            if marked_text != nil {
                let len: u64 = msg_send![marked_text, length];
                if len > 0 {
                    let mutable_string: ObjcId = msg_send![marked_text, mutableString];
                    let empty = str_to_nsstring("");
                    let () = msg_send![mutable_string, setString: empty];
                    (*(this as *const _ as *mut Object)).set_ivar("markedTextStart", 0i64);
                }
            }

            if !had_marked && range_start != range_end {
                let replaced_text =
                    utf16_range_to_string(&buffer_string, range_start as usize, range_end as usize);
                let (char_start, char_end) = utf16_indices_to_char_offsets(
                    &buffer_string,
                    range_start as usize,
                    range_end as usize,
                );
                IosApp::send_text_range_replace(
                    char_start,
                    char_end,
                    string.clone(),
                    Some(replaced_text),
                    true,
                );
            } else {
                IosApp::send_text_input(string.clone(), false);
            }

            replace_buffer_range(this, range_start, range_end, text);

            // Notify that text and selection did change
            if input_delegate != nil {
                let () = msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
            set_suppress_system_selection_changes(this, NO);
        }
    }

    extern "C" fn insert_text_with_alternatives(
        this: &Object,
        _: Sel,
        text: ObjcId,
        _alternatives: ObjcId,
        _style: i64,
    ) {
        insert_text(this, sel!(insertText:), text);
    }

    extern "C" fn insert_input_suggestion(this: &Object, _: Sel, input_suggestion: ObjcId) {
        if input_suggestion == nil {
            return;
        }
        unsafe {
            let responds: BOOL =
                msg_send![input_suggestion, respondsToSelector: sel!(localizedSuggestion)];
            if responds == YES {
                let text: ObjcId = msg_send![input_suggestion, localizedSuggestion];
                if text != nil {
                    insert_text(this, sel!(insertText:), text);
                }
            }
        }
    }

    extern "C" fn insert_attributed_text(this: &Object, _: Sel, text: ObjcId) {
        unsafe {
            let string = string_object_from_plain_or_attributed_text(text);
            insert_text(this, sel!(insertText:), string);
        }
    }

    extern "C" fn delete_backward(this: &Object, _: Sel) {
        unsafe {
            set_suppress_system_selection_changes(this, YES);
            let buffer = get_text_buffer(this);
            let buffer_len: u64 = msg_send![buffer, length];
            let (sel_start, sel_end) = normalized_selection_range(this);

            if sel_start != sel_end {
                let buffer_string = nsstring_to_string(buffer);
                let (char_start, char_end) = utf16_indices_to_char_offsets(
                    &buffer_string,
                    sel_start as usize,
                    sel_end as usize,
                );
                let replaced_text =
                    utf16_range_to_string(&buffer_string, sel_start as usize, sel_end as usize);
                let empty = str_to_nsstring("");
                replace_buffer_range(this, sel_start, sel_end, empty);
                IosApp::send_text_range_replace(
                    char_start,
                    char_end,
                    String::new(),
                    Some(replaced_text),
                    false,
                );
                set_suppress_system_selection_changes(this, NO);
                return;
            }

            let cursor: i64 = *this.get_ivar("cursorPosition");

            // Clamp cursor to valid range to handle out-of-sync states
            let cursor = cursor.max(0).min(buffer_len as i64);

            if cursor > 0 && buffer_len > 0 {
                // Get buffer string to properly handle multi-code-unit characters (emoji)
                let buffer_string = nsstring_to_string(buffer);

                // Ensure cursor doesn't exceed string's UTF-16 length
                let cursor_clamped = (cursor as usize).min(buffer_string.encode_utf16().count());

                // Find character before cursor (handles emoji as single unit)
                let (_, cursor_char) =
                    utf16_indices_to_char_offsets(&buffer_string, cursor_clamped, cursor_clamped);
                if cursor_char > 0 {
                    // Get UTF-16 position of previous character
                    let prev_char_utf16_start =
                        char_to_utf16_index(&buffer_string, cursor_char - 1) as u64;
                    let delete_len = (cursor_clamped as u64).saturating_sub(prev_char_utf16_start);

                    if delete_len > 0 && prev_char_utf16_start + delete_len <= buffer_len {
                        // Delete the entire previous character (1 or 2 UTF-16 code units)
                        let range = NSRange {
                            location: prev_char_utf16_start,
                            length: delete_len,
                        };
                        let () = msg_send![buffer, deleteCharactersInRange: range];
                        set_selection_utf16(this, prev_char_utf16_start, prev_char_utf16_start);
                    }
                }
            } else if cursor != 0 {
                // Reset cursor if buffer is empty but cursor wasn't at 0
                set_selection_utf16(this, 0, 0);
            }
            set_suppress_system_selection_changes(this, NO);
        }

        // Queue the backspace so any preceding UIKit selection update in the
        // same callback burst is applied before Rust handles deletion.
        IosApp::send_backspace();
    }

    // ==========================================================================
    // UITextInput protocol - Marked text (composition) methods
    // ==========================================================================

    extern "C" fn has_marked_text(this: &Object, _: Sel) -> BOOL {
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text == nil {
                return NO;
            }
            let len: u64 = msg_send![marked_text, length];
            if len > 0 {
                YES
            } else {
                NO
            }
        }
    }

    extern "C" fn marked_text_range(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text == nil {
                return nil;
            }
            let len: u64 = msg_send![marked_text, length];
            if len > 0 {
                let marked_start: i64 = *this.get_ivar("markedTextStart");
                let range_class = get_ios_class_global().text_range;
                msg_send![range_class, rangeWithStart: marked_start end: marked_start + (len as i64)]
            } else {
                nil
            }
        }
    }

    extern "C" fn set_marked_text(
        this: &mut Object,
        _: Sel,
        marked_text_input: ObjcId,
        selected_range: NSRange,
    ) {
        unsafe {
            // Notify inputDelegate that text will change
            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
            }

            let previous_marked: ObjcId = *this.get_ivar("markedText");
            let had_marked = if previous_marked != nil {
                let len: u64 = msg_send![previous_marked, length];
                len > 0
            } else {
                false
            };

            if !had_marked {
                let (sel_start, sel_end) = normalized_selection_range(this);
                if sel_start != sel_end {
                    let empty = str_to_nsstring("");
                    replace_buffer_range(this, sel_start, sel_end, empty);
                    set_selection_utf16(this, sel_start, sel_start);
                }
                this.set_ivar("markedTextStart", sel_start as i64);
            }

            let old_marked: ObjcId = *this.get_ivar("markedText");
            if old_marked != nil {
                let () = msg_send![old_marked, release];
            }

            // Create new marked text storage
            let new_marked: ObjcId = msg_send![class!(NSMutableAttributedString), alloc];

            if marked_text_input != nil {
                let has_attr: BOOL =
                    msg_send![marked_text_input, isKindOfClass: class!(NSAttributedString)];
                if has_attr == YES {
                    let () = msg_send![new_marked, initWithAttributedString: marked_text_input];
                } else {
                    let () = msg_send![new_marked, initWithString: marked_text_input];
                }
            } else {
                let () = msg_send![new_marked, init];
            }

            this.set_ivar("markedText", new_marked);

            // Send marked text to Makepad for inline display
            let text_string: ObjcId = msg_send![new_marked, string];
            let marked_string = nsstring_to_string(text_string);

            // Always send with replace_last=true - empty string clears composition preview
            IosApp::send_text_input(marked_string, true);

            let marked_start: i64 = *this.get_ivar("markedTextStart");
            let marked_len: u64 = msg_send![new_marked, length];
            let sel_start = (selected_range.location).min(marked_len);
            let sel_end = (selected_range.location + selected_range.length).min(marked_len);
            set_selection_utf16(
                this,
                (marked_start.max(0) as u64) + sel_start,
                (marked_start.max(0) as u64) + sel_end,
            );

            // Notify inputDelegate that text did change
            if input_delegate != nil {
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
        }
    }

    extern "C" fn unmark_text(this: &Object, _: Sel) {
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text == nil {
                return;
            }

            let len: u64 = msg_send![marked_text, length];
            if len == 0 {
                return;
            }

            // Get the marked text string
            let text_string: ObjcId = msg_send![marked_text, string];
            let string = nsstring_to_string(text_string);

            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");

            set_suppress_system_selection_changes(this, YES);

            // Notify that text will change
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, selectionWillChange: this as *const _ as ObjcId];
            }

            // Commit the marked text to Makepad
            IosApp::send_text_input(string.clone(), false);

            // Update text buffer
            let buffer = get_text_buffer(this);
            let marked_start: i64 = *this.get_ivar("markedTextStart");
            let buffer_len: u64 = msg_send![buffer, length];
            let insert_pos = (marked_start.max(0) as u64).min(buffer_len);
            let () = msg_send![buffer, insertString: text_string atIndex: insert_pos];

            // Update cursor position using UTF-16 code units (matches iOS NSString.length)
            let new_cursor = insert_pos + utf16_len(&string) as u64;
            set_selection_utf16(this, new_cursor, new_cursor);

            // Clear the marked text
            let mutable_string: ObjcId = msg_send![marked_text, mutableString];
            let empty = str_to_nsstring("");
            let () = msg_send![mutable_string, setString: empty];
            (*(this as *const _ as *mut Object)).set_ivar("markedTextStart", 0i64);

            // Notify that text did change
            if input_delegate != nil {
                let () = msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
            set_suppress_system_selection_changes(this, NO);
        }
    }

    extern "C" fn marked_text_style(_: &Object, _: Sel) -> ObjcId {
        nil
    }

    extern "C" fn set_marked_text_style(_: &mut Object, _: Sel, _style: ObjcId) {
        // Not implemented - we don't use custom styling
    }

    // ==========================================================================
    // UITextInput protocol - Selection methods
    // ==========================================================================

    extern "C" fn selected_text_range(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let sel_start: i64 = *this.get_ivar("selectionStart");
            let sel_end: i64 = *this.get_ivar("selectionEnd");
            let cursor: i64 = *this.get_ivar("cursorPosition");
            // Use selection if set, otherwise use cursor
            let start = if sel_start != sel_end {
                sel_start
            } else {
                cursor
            };
            let end = if sel_start != sel_end {
                sel_end
            } else {
                cursor
            };
            let range_class = get_ios_class_global().text_range;
            msg_send![range_class, rangeWithStart: start end: end]
        }
    }

    extern "C" fn set_selected_text_range(this: &mut Object, _: Sel, range: ObjcId) {
        if range == nil {
            return;
        }
        unsafe {
            // Makepad owns the actual selection/cursor. Accept UIKit selection
            // changes while marked text is active, or when they are standalone
            // text-navigation updates. Suppress selection callbacks that UIKit
            // emits during insertion/replacement/deletion; those are derived
            // from the text mutation and can be stale by the time Makepad has
            // already applied more input.
            let accept_system_selection: BOOL = *this.get_ivar("accept_system_selection_changes");
            let suppress_system_selection: BOOL =
                *this.get_ivar("suppress_system_selection_changes");
            let has_marked_text = marked_text_utf16_len(this) > 0;
            if !has_marked_text
                && accept_system_selection != YES
                && suppress_system_selection == YES
            {
                return;
            }

            let start: ObjcId = msg_send![range, start];
            let end: ObjcId = msg_send![range, end];
            if start != nil && end != nil {
                let buffer_len = visible_text_utf16_len(this);
                let start_offset: i64 = msg_send![start, offset];
                let end_offset: i64 = msg_send![end, offset];
                let start_offset = start_offset.clamp(0, buffer_len);
                let end_offset = end_offset.clamp(0, buffer_len);
                let current_start: i64 = *this.get_ivar("selectionStart");
                let current_end: i64 = *this.get_ivar("selectionEnd");
                if current_start == start_offset && current_end == end_offset {
                    return;
                }

                let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");
                if input_delegate != nil {
                    let () =
                        msg_send![input_delegate, selectionWillChange: this as *const _ as ObjcId];
                }

                this.set_ivar("selectionStart", start_offset);
                this.set_ivar("selectionEnd", end_offset);
                this.set_ivar("cursorPosition", end_offset);

                if !has_marked_text
                    && (accept_system_selection == YES || suppress_system_selection != YES)
                {
                    let visible_text = visible_text_string(this);
                    let (char_start, char_end) = utf16_indices_to_char_offsets(
                        &visible_text,
                        start_offset as usize,
                        end_offset as usize,
                    );
                    IosApp::send_text_selection_changed(visible_text, char_start, char_end);
                }

                if input_delegate != nil {
                    let () =
                        msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                }
            }
        }
    }

    // ==========================================================================
    // UITextInput protocol - Text storage methods
    // ==========================================================================

    extern "C" fn text_in_range(this: &Object, _: Sel, range: ObjcId) -> ObjcId {
        if range == nil {
            return str_to_nsstring("");
        }
        unsafe {
            let start: ObjcId = msg_send![range, start];
            let end: ObjcId = msg_send![range, end];

            if start == nil || end == nil {
                return str_to_nsstring("");
            }

            let start_offset: i64 = msg_send![start, offset];
            let end_offset: i64 = msg_send![end, offset];

            if start_offset >= end_offset {
                return str_to_nsstring("");
            }

            // Include active marked text in the visible text model. UIKit often
            // queries ranges that cross the marked-text boundary while building
            // candidate lists or applying autocorrect.
            let visible_text = visible_text_string(this);

            str_to_nsstring(&utf16_range_to_string(
                &visible_text,
                start_offset.max(0) as usize,
                end_offset.max(0) as usize,
            ))
        }
    }

    extern "C" fn attributed_text_in_range(this: &Object, sel: Sel, range: ObjcId) -> ObjcId {
        unsafe {
            let string = text_in_range(this, sel, range);
            let attributed: ObjcId = msg_send![class!(NSAttributedString), alloc];
            let attributed: ObjcId = msg_send![attributed, initWithString: string];
            let attributed: ObjcId = msg_send![attributed, autorelease];
            attributed
        }
    }

    extern "C" fn replace_range_with_text(this: &Object, _: Sel, range: ObjcId, text: ObjcId) {
        unsafe {
            let new_string = nsstring_to_string(text);

            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");

            set_suppress_system_selection_changes(this, YES);

            // Notify that text will change
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, selectionWillChange: this as *const _ as ObjcId];
            }

            // Clear marked text first
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text != nil {
                let len: u64 = msg_send![marked_text, length];
                if len > 0 {
                    let mutable_string: ObjcId = msg_send![marked_text, mutableString];
                    let empty = str_to_nsstring("");
                    let () = msg_send![mutable_string, setString: empty];
                    (*(this as *const _ as *mut Object)).set_ivar("markedTextStart", 0i64);
                }
            }

            // Get range bounds by extracting offsets from our custom position objects
            let (range_start, range_end) = if range != nil {
                let start_pos: ObjcId = msg_send![range, start];
                let end_pos: ObjcId = msg_send![range, end];

                let start: i64 = if start_pos != nil {
                    let responds: BOOL = msg_send![start_pos, respondsToSelector: sel!(offset)];
                    if responds == YES {
                        msg_send![start_pos, offset]
                    } else {
                        0
                    }
                } else {
                    0
                };

                let end: i64 = if end_pos != nil {
                    let responds: BOOL = msg_send![end_pos, respondsToSelector: sel!(offset)];
                    if responds == YES {
                        msg_send![end_pos, offset]
                    } else {
                        start
                    }
                } else {
                    start
                };

                (start.max(0) as usize, end.max(0) as usize)
            } else {
                let cursor: i64 = *this.get_ivar("cursorPosition");
                let pos = cursor.max(0) as usize;
                (pos, pos)
            };

            // Update iOS text buffer directly
            let buffer = get_text_buffer(this);
            let buffer_len: u64 = msg_send![buffer, length];
            let buf_start = (range_start as u64).min(buffer_len);
            let buf_end = (range_end as u64).min(buffer_len);

            // Get buffer contents BEFORE modification for UTF-16 to char index conversion
            let buffer_string = nsstring_to_string(buffer);

            // Delete the range from buffer
            if buf_start < buf_end {
                let delete_range = NSRange {
                    location: buf_start,
                    length: buf_end - buf_start,
                };
                let () = msg_send![buffer, deleteCharactersInRange: delete_range];
            }

            // Insert new text at buf_start
            if !new_string.is_empty() {
                let new_buf_len: u64 = msg_send![buffer, length];
                let insert_pos = buf_start.min(new_buf_len);
                let () = msg_send![buffer, insertString: text atIndex: insert_pos];
            }

            // Convert UTF-16 indices to character indices for Makepad
            // iOS uses UTF-16 code units, but Makepad expects character indices
            let (char_start, char_end) =
                utf16_indices_to_char_offsets(&buffer_string, range_start, range_end);

            let replaced_text = utf16_range_to_string(&buffer_string, range_start, range_end);

            // Send the range replacement event to Makepad
            IosApp::send_text_range_replace(
                char_start,
                char_end,
                new_string.clone(),
                Some(replaced_text),
                false,
            );

            // Update cursor position to end of inserted text (using UTF-16 code units)
            let new_cursor = range_start as i64 + utf16_len(&new_string);
            set_selection_utf16(this, new_cursor as u64, new_cursor as u64);

            if input_delegate != nil {
                let () = msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
            set_suppress_system_selection_changes(this, NO);
        }
    }

    extern "C" fn replace_range_with_attributed_text(
        this: &Object,
        _: Sel,
        range: ObjcId,
        text: ObjcId,
    ) {
        unsafe {
            let string = string_object_from_plain_or_attributed_text(text);
            replace_range_with_text(this, sel!(replaceRange:withText:), range, string);
        }
    }

    extern "C" fn should_change_text_in_range(
        _: &Object,
        _: Sel,
        _range: ObjcId,
        _replacement_text: ObjcId,
    ) -> BOOL {
        YES
    }

    // ==========================================================================
    // UITextInput protocol - Position/Range methods
    // ==========================================================================

    extern "C" fn beginning_of_document(_: &Object, _: Sel) -> ObjcId {
        unsafe {
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: 0i64]
        }
    }

    extern "C" fn end_of_document(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            // Return the visible text length, including active marked text.
            let buffer_len = visible_text_utf16_len(this);
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: buffer_len]
        }
    }

    extern "C" fn position_from_position_offset(
        this: &Object,
        _: Sel,
        position: ObjcId,
        offset: i64,
    ) -> ObjcId {
        if position == nil {
            return nil;
        }
        unsafe {
            let pos: i64 = msg_send![position, offset];
            let buffer_len = visible_text_utf16_len(this);
            let new_pos = (pos + offset).clamp(0, buffer_len);
            if new_pos < 0 {
                return nil;
            }
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: new_pos]
        }
    }

    extern "C" fn position_from_position_in_direction_offset(
        this: &Object,
        _: Sel,
        position: ObjcId,
        direction: i64,
        offset: i64,
    ) -> ObjcId {
        if position == nil {
            return nil;
        }
        unsafe {
            let pos: i64 = msg_send![position, offset];
            // UITextLayoutDirection: 0=right, 1=left, 2=up, 3=down
            let actual_offset = if direction == 1 { -offset } else { offset };
            let buffer_len = visible_text_utf16_len(this);
            let new_pos = (pos + actual_offset).clamp(0, buffer_len);
            if new_pos < 0 {
                return nil;
            }
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: new_pos]
        }
    }

    extern "C" fn text_range_from_position_to_position(
        _: &Object,
        _: Sel,
        from_position: ObjcId,
        to_position: ObjcId,
    ) -> ObjcId {
        if from_position == nil || to_position == nil {
            return nil;
        }
        unsafe {
            let range_class = get_ios_class_global().text_range;
            msg_send![range_class, rangeWithStartPosition: from_position endPosition: to_position]
        }
    }

    extern "C" fn compare_position_to_position(
        _: &Object,
        _: Sel,
        position: ObjcId,
        other: ObjcId,
    ) -> i64 {
        if position == nil || other == nil {
            return 0; // NSOrderedSame
        }
        unsafe {
            let pos1: i64 = msg_send![position, offset];
            let pos2: i64 = msg_send![other, offset];
            if pos1 < pos2 {
                -1 // NSOrderedAscending
            } else if pos1 > pos2 {
                1 // NSOrderedDescending
            } else {
                0 // NSOrderedSame
            }
        }
    }

    extern "C" fn offset_from_position_to_position(
        _: &Object,
        _: Sel,
        from: ObjcId,
        to: ObjcId,
    ) -> i64 {
        if from == nil || to == nil {
            return 0;
        }
        unsafe {
            let from_pos: i64 = msg_send![from, offset];
            let to_pos: i64 = msg_send![to, offset];
            to_pos - from_pos
        }
    }

    extern "C" fn position_within_range_farthest_in_direction(
        _: &Object,
        _: Sel,
        range: ObjcId,
        direction: i64,
    ) -> ObjcId {
        if range == nil {
            return nil;
        }
        unsafe {
            // UITextLayoutDirection: 0=right, 1=left, 2=up, 3=down
            // For left/up, return start; for right/down, return end
            if direction == 1 || direction == 2 {
                msg_send![range, start]
            } else {
                msg_send![range, end]
            }
        }
    }

    extern "C" fn position_within_range_at_character_offset(
        _: &Object,
        _: Sel,
        range: ObjcId,
        offset: i64,
    ) -> ObjcId {
        if range == nil {
            return nil;
        }
        unsafe {
            let start: ObjcId = msg_send![range, start];
            let end: ObjcId = msg_send![range, end];
            if start == nil || end == nil {
                return nil;
            }
            let start_offset: i64 = msg_send![start, offset];
            let end_offset: i64 = msg_send![end, offset];
            let range_start = start_offset.min(end_offset);
            let range_end = start_offset.max(end_offset);
            let new_offset = (range_start + offset).clamp(range_start, range_end);
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: new_offset]
        }
    }

    extern "C" fn character_offset_of_position_within_range(
        _: &Object,
        _: Sel,
        position: ObjcId,
        range: ObjcId,
    ) -> i64 {
        if position == nil || range == nil {
            return 0;
        }
        unsafe {
            let start: ObjcId = msg_send![range, start];
            let end: ObjcId = msg_send![range, end];
            if start == nil || end == nil {
                return 0;
            }
            let pos_offset: i64 = msg_send![position, offset];
            let start_offset: i64 = msg_send![start, offset];
            let end_offset: i64 = msg_send![end, offset];
            let range_start = start_offset.min(end_offset);
            let range_end = start_offset.max(end_offset);
            pos_offset.clamp(range_start, range_end) - range_start
        }
    }

    extern "C" fn character_range_by_extending_position_in_direction(
        this: &Object,
        _: Sel,
        position: ObjcId,
        direction: i64,
    ) -> ObjcId {
        if position == nil {
            return nil;
        }
        unsafe {
            let pos: i64 = msg_send![position, offset];
            let buffer_string = visible_text_string(this);
            let utf16_len = buffer_string.encode_utf16().count() as i64;
            let pos = pos.clamp(0, utf16_len) as usize;
            let char_at_pos = utf16_indices_to_char_offsets(&buffer_string, pos, pos).0;
            let (start, end) = if direction == 1 {
                let char_start = char_at_pos.saturating_sub(1);
                (
                    char_to_utf16_index(&buffer_string, char_start) as i64,
                    char_to_utf16_index(&buffer_string, char_at_pos) as i64,
                )
            } else {
                let char_count = buffer_string.chars().count();
                let char_end = (char_at_pos + 1).min(char_count);
                (
                    char_to_utf16_index(&buffer_string, char_at_pos) as i64,
                    char_to_utf16_index(&buffer_string, char_end) as i64,
                )
            };
            let range_class = get_ios_class_global().text_range;
            msg_send![range_class, rangeWithStart: start end: end]
        }
    }

    // ==========================================================================
    // UITextInput protocol - Geometry methods
    // ==========================================================================

    const CANDIDATE_RECT_HEIGHT: f64 = 32.0;

    unsafe fn view_local_point(this: &Object, x: f64, y: f64) -> NSPoint {
        let view = this as *const _ as ObjcId;
        let view_frame: NSRect = msg_send![view, frame];
        NSPoint {
            x: x - view_frame.origin.x,
            y: y - view_frame.origin.y,
        }
    }

    unsafe fn ime_candidate_rect(this: &Object) -> NSRect {
        // ime_pos_x/y are view-local (the view is parked at the caret). Return a
        // small in-bounds rect over the glyph so UIKit's bounds-clamp is a no-op.
        let x: f64 = *this.get_ivar("ime_pos_x");
        let y: f64 = *this.get_ivar("ime_pos_y");

        NSRect {
            origin: NSPoint {
                x,
                y: y - CANDIDATE_RECT_HEIGHT,
            },
            size: NSSize {
                width: 1.0,
                height: CANDIDATE_RECT_HEIGHT,
            },
        }
    }

    unsafe fn hidden_caret_rect(this: &Object) -> NSRect {
        // ime_pos_x/y are view-local (the view is parked at the caret). A small
        // in-bounds rect over the glyph keeps UIKit from clamping the accent popup.
        let x: f64 = *this.get_ivar("ime_pos_x");
        let y: f64 = *this.get_ivar("ime_pos_y");

        NSRect {
            origin: NSPoint {
                x,
                y: y - IOS_TEXT_INPUT_CARET_HEIGHT,
            },
            size: NSSize {
                width: 1.0,
                height: IOS_TEXT_INPUT_CARET_HEIGHT,
            },
        }
    }

    extern "C" fn first_rect_for_range(this: &Object, _: Sel, _range: ObjcId) -> NSRect {
        // Return a rect at the IME position for candidate window placement.
        // For explicit selection handles, return the bounding rect provided by
        // Makepad's selection geometry.
        unsafe {
            let selection_visible: BOOL = *this.get_ivar("selection_handles_visible");

            if selection_visible == YES {
                let sx: f64 = *this.get_ivar("selection_handle_start_x");
                let sy: f64 = *this.get_ivar("selection_handle_start_y");
                let ex: f64 = *this.get_ivar("selection_handle_end_x");
                let ey: f64 = *this.get_ivar("selection_handle_end_y");
                let start = view_local_point(this, sx, sy);
                let end = view_local_point(this, ex, ey);
                return NSRect {
                    origin: NSPoint {
                        x: start.x.min(end.x),
                        y: start.y.min(end.y),
                    },
                    size: NSSize {
                        width: (start.x - end.x).abs().max(1.0),
                        height: (start.y - end.y).abs().max(1.0),
                    },
                };
            }

            ime_candidate_rect(this)
        }
    }

    extern "C" fn caret_rect_for_position(this: &Object, _sel: Sel, _position: ObjcId) -> NSRect {
        unsafe { hidden_caret_rect(this) }
    }

    extern "C" fn selection_rects_for_range(this: &Object, _: Sel, _range: ObjcId) -> ObjcId {
        unsafe {
            let selection_visible: BOOL = *this.get_ivar("selection_handles_visible");
            if selection_visible != YES {
                return msg_send![class!(NSArray), array];
            }

            let sx: f64 = *this.get_ivar("selection_handle_start_x");
            let sy: f64 = *this.get_ivar("selection_handle_start_y");
            let ex: f64 = *this.get_ivar("selection_handle_end_x");
            let ey: f64 = *this.get_ivar("selection_handle_end_y");
            let start = view_local_point(this, sx, sy);
            let end = view_local_point(this, ex, ey);

            let rect_cls = get_ios_class_global().text_selection_rect;
            let selection_rect: ObjcId = msg_send![
                rect_cls,
                rectWithX: start.x.min(end.x)
                y: start.y.min(end.y)
                w: (start.x - end.x).abs().max(1.0)
                h: (start.y - end.y).abs().max(1.0)
                containsStart: YES
                containsEnd: YES
            ];
            msg_send![class!(NSArray), arrayWithObject: selection_rect]
        }
    }

    /// Returns the current cursor position. Exact hit testing requires text
    /// layout from the widget layer; returning the active cursor is less
    /// disruptive than jumping UIKit-managed interactions to document start.
    extern "C" fn closest_position_to_point(this: &Object, _: Sel, _point: NSPoint) -> ObjcId {
        unsafe {
            let cursor: i64 = *this.get_ivar("cursorPosition");
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: cursor]
        }
    }

    extern "C" fn closest_position_to_point_within_range(
        _: &Object,
        _: Sel,
        _point: NSPoint,
        range: ObjcId,
    ) -> ObjcId {
        if range == nil {
            return nil;
        }
        // Return start of range
        unsafe { msg_send![range, start] }
    }

    extern "C" fn character_range_at_point(_: &Object, _: Sel, _point: NSPoint) -> ObjcId {
        nil
    }

    extern "C" fn unobscured_content_rect(this: &Object, _: Sel) -> NSRect {
        unsafe {
            let view = this as *const _ as ObjcId;
            msg_send![view, bounds]
        }
    }

    extern "C" fn text_input_view(this: &Object, _: Sel) -> ObjcId {
        this as *const _ as ObjcId
    }

    // ==========================================================================
    // UITextInput protocol - Writing direction (required methods)
    // ==========================================================================

    extern "C" fn base_writing_direction_for_position(
        _: &Object,
        _: Sel,
        _position: ObjcId,
        _direction: i64,
    ) -> i64 {
        0 // NSWritingDirectionNatural
    }

    extern "C" fn set_base_writing_direction(_: &Object, _: Sel, _direction: i64, _range: ObjcId) {
        // No-op - we don't support changing writing direction
    }

    // ==========================================================================
    // UITextInput protocol - Input delegate and tokenizer
    // ==========================================================================

    extern "C" fn input_delegate(this: &Object, _: Sel) -> ObjcId {
        unsafe { *this.get_ivar("_inputDelegate") }
    }

    extern "C" fn set_input_delegate(this: &mut Object, _: Sel, delegate: ObjcId) {
        unsafe {
            this.set_ivar("_inputDelegate", delegate);
        }
    }

    extern "C" fn tokenizer(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let tok: ObjcId = *this.get_ivar("_tokenizer");
            if tok != nil {
                return tok;
            }
            // Create tokenizer on first access
            let view = this as *const _ as ObjcId;
            let new_tok: ObjcId = msg_send![class!(UITextInputStringTokenizer), alloc];
            let new_tok: ObjcId = msg_send![new_tok, initWithTextInput: view];
            (*(this as *const _ as *mut Object)).set_ivar("_tokenizer", new_tok);
            new_tok
        }
    }

    // ==========================================================================
    // UITextInputTraits protocol
    // ==========================================================================

    extern "C" fn keyboard_type(this: &Object, _: Sel) -> i64 {
        unsafe { *this.get_ivar::<i64>("_keyboard_type") }
    }

    extern "C" fn autocorrection_type(this: &Object, _: Sel) -> i64 {
        unsafe {
            let stored: i64 = *this.get_ivar::<i64>("_autocorrection_type");
            // -1 means "use CJK detection logic" (Default behavior)
            if stored >= 0 {
                return stored;
            }
            // Try to get current input mode to check keyboard language
            let input_mode: ObjcId = msg_send![class!(UITextInputMode), currentInputMode];
            if input_mode != nil {
                let primary_lang: ObjcId = msg_send![input_mode, primaryLanguage];
                if primary_lang != nil {
                    let lang = nsstring_to_string(primary_lang);
                    // Disable autocorrect for CJK languages (composition-based IME)
                    if lang.starts_with("zh")      // Chinese
                        || lang.starts_with("ja")  // Japanese
                        || lang.starts_with("ko")
                    // Korean
                    {
                        return UI_TEXT_AUTOCORRECTION_NO;
                    }
                }
            }
            UI_TEXT_AUTOCORRECTION_DEFAULT
        }
    }

    extern "C" fn autocapitalization_type(this: &Object, _: Sel) -> i64 {
        unsafe { *this.get_ivar::<i64>("_autocapitalization_type") }
    }

    extern "C" fn spell_checking_type(_: &Object, _: Sel) -> i64 {
        0 // UITextSpellCheckingTypeDefault
    }

    extern "C" fn smart_quotes_type(_: &Object, _: Sel) -> i64 {
        1 // UITextSmartQuotesTypeNo - disable for code/text editing
    }

    extern "C" fn smart_dashes_type(_: &Object, _: Sel) -> i64 {
        1 // UITextSmartDashesTypeNo
    }

    extern "C" fn smart_insert_delete_type(_: &Object, _: Sel) -> i64 {
        1 // UITextSmartInsertDeleteTypeNo
    }

    extern "C" fn keyboard_appearance(_: &Object, _: Sel) -> i64 {
        0 // UIKeyboardAppearanceDefault
    }

    extern "C" fn return_key_type(this: &Object, _: Sel) -> i64 {
        unsafe { *this.get_ivar::<i64>("_return_key_type") }
    }

    extern "C" fn enables_return_key_automatically(_: &Object, _: Sel) -> BOOL {
        NO
    }

    extern "C" fn is_secure_text_entry(this: &Object, _: Sel) -> BOOL {
        unsafe {
            if *this.get_ivar::<bool>("_secure_text_entry") {
                YES
            } else {
                NO
            }
        }
    }

    extern "C" fn text_content_type(_: &Object, _: Sel) -> ObjcId {
        nil // No specific content type
    }

    extern "C" fn selection_affinity(_: &Object, _: Sel) -> i64 {
        0 // UITextStorageDirectionForward
    }

    extern "C" fn set_selection_affinity(_: &mut Object, _: Sel, _affinity: i64) {
        // We keep a direction-neutral selection model.
    }

    extern "C" fn text_styling_at_position(
        _: &Object,
        _: Sel,
        _position: ObjcId,
        _direction: i64,
    ) -> ObjcId {
        nil
    }

    extern "C" fn supports_adaptive_image_glyph(_: &Object, _: Sel) -> BOOL {
        NO
    }

    extern "C" fn insert_dictation_result(this: &Object, _: Sel, dictation_result: ObjcId) {
        if dictation_result == nil {
            return;
        }
        unsafe {
            let count: usize = msg_send![dictation_result, count];
            let mut result = String::new();
            for index in 0..count {
                let phrase: ObjcId = msg_send![dictation_result, objectAtIndex: index];
                if phrase == nil {
                    continue;
                }

                let is_string: BOOL = msg_send![phrase, isKindOfClass: class!(NSString)];
                if is_string == YES {
                    result.push_str(&nsstring_to_string(phrase));
                    continue;
                }

                let responds_to_text: BOOL = msg_send![phrase, respondsToSelector: sel!(text)];
                if responds_to_text == YES {
                    let text: ObjcId = msg_send![phrase, text];
                    if text != nil {
                        let text = nsstring_to_string(text);
                        if !text.is_empty() {
                            result.push_str(&text);
                            continue;
                        }
                    }
                }

                let responds_to_alternatives: BOOL =
                    msg_send![phrase, respondsToSelector: sel!(alternativeInterpretations)];
                if responds_to_alternatives == YES {
                    let alternatives: ObjcId = msg_send![phrase, alternativeInterpretations];
                    if alternatives != nil {
                        let alternative_count: usize = msg_send![alternatives, count];
                        if alternative_count > 0 {
                            let text: ObjcId = msg_send![alternatives, objectAtIndex: 0usize];
                            if text != nil {
                                result.push_str(&nsstring_to_string(text));
                            }
                        }
                    }
                }
            }

            if !result.is_empty() {
                let text = str_to_nsstring(&result);
                insert_text(this, sel!(insertText:), text);
            }
        }
    }

    extern "C" fn insert_dictation_result_placeholder(_: &Object, _: Sel) -> ObjcId {
        unsafe { make_placeholder_object() }
    }

    extern "C" fn frame_for_dictation_result_placeholder(
        this: &Object,
        sel: Sel,
        _placeholder: ObjcId,
    ) -> NSRect {
        first_rect_for_range(this, sel, nil)
    }

    extern "C" fn remove_dictation_result_placeholder(
        _: &Object,
        _: Sel,
        _placeholder: ObjcId,
        _will_insert_result: BOOL,
    ) {
    }

    extern "C" fn dictation_recognition_failed(_: &Object, _: Sel) {}

    extern "C" fn dictation_recording_did_end(_: &Object, _: Sel) {}

    extern "C" fn insert_text_placeholder_with_size(_: &Object, _: Sel, _size: NSSize) -> ObjcId {
        unsafe { make_placeholder_object() }
    }

    extern "C" fn remove_text_placeholder(_: &Object, _: Sel, _placeholder: ObjcId) {}

    // ==========================================================================
    // UITextInput protocol - Floating cursor methods (keyboard trackpad)
    // ==========================================================================

    extern "C" fn begin_floating_cursor(this: &mut Object, _: Sel, point: NSPoint) {
        unsafe {
            this.set_ivar::<BOOL>("floating_cursor_active", YES);
            this.set_ivar::<f64>("floating_cursor_last_x", point.x);
            this.set_ivar::<f64>("floating_cursor_last_y", point.y);
        }
    }

    extern "C" fn update_floating_cursor(this: &mut Object, _: Sel, point: NSPoint) {
        unsafe {
            let active: BOOL = *this.get_ivar("floating_cursor_active");
            if active == NO {
                return;
            }

            let last_x: f64 = *this.get_ivar("floating_cursor_last_x");
            let last_y: f64 = *this.get_ivar("floating_cursor_last_y");
            let delta_x = point.x - last_x;
            let delta_y = point.y - last_y;

            // Threshold in points (~character width for horizontal, ~line height for vertical)
            const MOVE_THRESHOLD_X: f64 = 10.0;
            const MOVE_THRESHOLD_Y: f64 = 20.0;

            let time = try_with_ios_app(|app| app.time_now()).unwrap_or(0.0);

            // Handle horizontal movement
            if delta_x.abs() >= MOVE_THRESHOLD_X {
                let steps = (delta_x / MOVE_THRESHOLD_X).trunc() as i32;
                let key_code = if steps > 0 {
                    KeyCode::ArrowRight
                } else {
                    KeyCode::ArrowLeft
                };

                for _ in 0..steps.abs() {
                    IosApp::do_callback(IosEvent::KeyDown(KeyEvent {
                        key_code,
                        is_repeat: false,
                        modifiers: Default::default(),
                        time,
                    }));
                    IosApp::do_callback(IosEvent::KeyUp(KeyEvent {
                        key_code,
                        is_repeat: false,
                        modifiers: Default::default(),
                        time,
                    }));
                }

                let consumed = (steps as f64) * MOVE_THRESHOLD_X;
                this.set_ivar::<f64>("floating_cursor_last_x", last_x + consumed);
            }

            // Handle vertical movement
            if delta_y.abs() >= MOVE_THRESHOLD_Y {
                let steps = (delta_y / MOVE_THRESHOLD_Y).trunc() as i32;
                // Positive Y is down on iOS, so positive delta = ArrowDown
                let key_code = if steps > 0 {
                    KeyCode::ArrowDown
                } else {
                    KeyCode::ArrowUp
                };

                for _ in 0..steps.abs() {
                    IosApp::do_callback(IosEvent::KeyDown(KeyEvent {
                        key_code,
                        is_repeat: false,
                        modifiers: Default::default(),
                        time,
                    }));
                    IosApp::do_callback(IosEvent::KeyUp(KeyEvent {
                        key_code,
                        is_repeat: false,
                        modifiers: Default::default(),
                        time,
                    }));
                }

                let consumed = (steps as f64) * MOVE_THRESHOLD_Y;
                this.set_ivar::<f64>("floating_cursor_last_y", last_y + consumed);
            }
        }
    }

    extern "C" fn end_floating_cursor(this: &mut Object, _: Sel) {
        unsafe {
            this.set_ivar::<BOOL>("floating_cursor_active", NO);
        }
    }

    // The four UITextLayoutDirection arrows plus the modifier combos the widget
    // maps to navigation. UIKeyModifierFlags share bit positions with the macOS
    // NSEventModifierFlags, so key_modifiers_from_flags decodes both.
    const KEY_MOD_SHIFT: u64 = 1 << 17;
    const KEY_MOD_ALT: u64 = 1 << 19;
    const KEY_MOD_CMD: u64 = 1 << 20;

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

    // Reclaim hardware arrow keys from UIKit's UITextInput navigation, which
    // otherwise moves a phantom caret in this proxy and never reaches the widget.
    // wantsPriorityOverSystemBehavior wins the key over built-in text nav, and
    // because these arrive as UIKeyCommands UIKit auto-repeats them at the system
    // rate while held. Each arrow is registered across the modifier combos the
    // widget maps (bare/Shift move-or-extend, Alt word, Cmd line/doc) so the
    // modified moves auto-repeat too.
    extern "C" fn key_commands(_this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let array: ObjcId = msg_send![class!(NSMutableArray), array];
            let arrow_inputs = [
                UIKeyInputLeftArrow,
                UIKeyInputRightArrow,
                UIKeyInputUpArrow,
                UIKeyInputDownArrow,
            ];
            let arrow_masks = [
                0,
                KEY_MOD_SHIFT,
                KEY_MOD_ALT,
                KEY_MOD_CMD,
                KEY_MOD_SHIFT | KEY_MOD_ALT,
                KEY_MOD_SHIFT | KEY_MOD_CMD,
            ];
            for input in arrow_inputs {
                for mask in arrow_masks {
                    add_nav_key_command(array, input, mask);
                }
            }

            // Home/End/PageUp/PageDown (iOS 13.4+, nil-guarded). Home/End also
            // take the Cmd text-boundary move; the Page keys are bare/Shift only.
            let doc = doc_nav_inputs();
            let line_edge_masks = [0, KEY_MOD_SHIFT, KEY_MOD_CMD, KEY_MOD_SHIFT | KEY_MOD_CMD];
            for input in [doc.home, doc.end] {
                for mask in line_edge_masks {
                    add_nav_key_command(array, input, mask);
                }
            }
            for input in [doc.page_up, doc.page_down] {
                for mask in [0, KEY_MOD_SHIFT] {
                    add_nav_key_command(array, input, mask);
                }
            }
            array
        }
    }

    extern "C" fn handle_makepad_key_command(_this: &Object, _: Sel, sender: ObjcId) {
        unsafe {
            let input: ObjcId = msg_send![sender, input];
            if input == nil {
                return;
            }
            let key_code = nav_key_code_for_input(input);
            if key_code.is_unknown() {
                return;
            }
            let modifier_flags: u64 = msg_send![sender, modifierFlags];
            let modifiers =
                crate::os::apple::apple_util::key_modifiers_from_flags(modifier_flags);
            // Balanced down/up keeps keys_down clean (mirrors the floating cursor).
            dispatch_makepad_key_code(key_code, modifiers, true);
            dispatch_makepad_key_code(key_code, modifiers, false);
        }
    }

    // ==========================================================================
    // Register all methods
    // ==========================================================================

    unsafe {
        // UIResponder
        decl.add_method(
            sel!(canBecomeFirstResponder),
            can_become_first_responder as extern "C" fn(&Object, Sel) -> BOOL,
        );
        // Hardware arrow-key navigation (see key_commands above).
        decl.add_method(
            sel!(keyCommands),
            key_commands as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(handleMakepadKeyCommand:),
            handle_makepad_key_command as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(canBecomeFocused),
            can_become_focused as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(isAccessibilityElement),
            is_accessibility_element as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(accessibilityElementsHidden),
            accessibility_elements_hidden as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(accessibilityRespondsToUserInteraction),
            accessibility_responds_to_user_interaction as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(accessibilityFrame),
            accessibility_frame as extern "C" fn(&Object, Sel) -> NSRect,
        );
        decl.add_method(
            sel!(pointInside:withEvent:),
            point_inside as extern "C" fn(&Object, Sel, NSPoint, ObjcId) -> BOOL,
        );
        // UIKeyInput
        decl.add_method(
            sel!(hasText),
            has_text as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(insertText:),
            insert_text as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(insertText:alternatives:style:),
            insert_text_with_alternatives as extern "C" fn(&Object, Sel, ObjcId, ObjcId, i64),
        );
        decl.add_method(
            sel!(insertInputSuggestion:),
            insert_input_suggestion as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(insertAttributedText:),
            insert_attributed_text as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(deleteBackward),
            delete_backward as extern "C" fn(&Object, Sel),
        );

        // UITextInput - Marked text
        decl.add_method(
            sel!(hasMarkedText),
            has_marked_text as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(markedTextRange),
            marked_text_range as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(setMarkedText:selectedRange:),
            set_marked_text as extern "C" fn(&mut Object, Sel, ObjcId, NSRange),
        );
        decl.add_method(
            sel!(setAttributedMarkedText:selectedRange:),
            set_marked_text as extern "C" fn(&mut Object, Sel, ObjcId, NSRange),
        );
        decl.add_method(sel!(unmarkText), unmark_text as extern "C" fn(&Object, Sel));
        decl.add_method(
            sel!(markedTextStyle),
            marked_text_style as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(setMarkedTextStyle:),
            set_marked_text_style as extern "C" fn(&mut Object, Sel, ObjcId),
        );

        // UITextInput - Selection
        decl.add_method(
            sel!(selectedTextRange),
            selected_text_range as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(setSelectedTextRange:),
            set_selected_text_range as extern "C" fn(&mut Object, Sel, ObjcId),
        );

        // UITextInput - Text storage
        decl.add_method(
            sel!(textInRange:),
            text_in_range as extern "C" fn(&Object, Sel, ObjcId) -> ObjcId,
        );
        decl.add_method(
            sel!(attributedTextInRange:),
            attributed_text_in_range as extern "C" fn(&Object, Sel, ObjcId) -> ObjcId,
        );
        decl.add_method(
            sel!(replaceRange:withText:),
            replace_range_with_text as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(replaceRange:withAttributedText:),
            replace_range_with_attributed_text as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(shouldChangeTextInRange:replacementText:),
            should_change_text_in_range as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> BOOL,
        );

        // UITextInput - Position/Range
        decl.add_method(
            sel!(beginningOfDocument),
            beginning_of_document as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(endOfDocument),
            end_of_document as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(positionFromPosition:offset:),
            position_from_position_offset as extern "C" fn(&Object, Sel, ObjcId, i64) -> ObjcId,
        );
        decl.add_method(
            sel!(positionFromPosition:inDirection:offset:),
            position_from_position_in_direction_offset
                as extern "C" fn(&Object, Sel, ObjcId, i64, i64) -> ObjcId,
        );
        decl.add_method(
            sel!(textRangeFromPosition:toPosition:),
            text_range_from_position_to_position
                as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> ObjcId,
        );
        decl.add_method(
            sel!(comparePosition:toPosition:),
            compare_position_to_position as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> i64,
        );
        decl.add_method(
            sel!(offsetFromPosition:toPosition:),
            offset_from_position_to_position as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> i64,
        );
        decl.add_method(
            sel!(positionWithinRange:farthestInDirection:),
            position_within_range_farthest_in_direction
                as extern "C" fn(&Object, Sel, ObjcId, i64) -> ObjcId,
        );
        decl.add_method(
            sel!(positionWithinRange:atCharacterOffset:),
            position_within_range_at_character_offset
                as extern "C" fn(&Object, Sel, ObjcId, i64) -> ObjcId,
        );
        decl.add_method(
            sel!(characterOffsetOfPosition:withinRange:),
            character_offset_of_position_within_range
                as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> i64,
        );
        decl.add_method(
            sel!(characterRangeByExtendingPosition:inDirection:),
            character_range_by_extending_position_in_direction
                as extern "C" fn(&Object, Sel, ObjcId, i64) -> ObjcId,
        );

        // UITextInput - Geometry
        decl.add_method(
            sel!(firstRectForRange:),
            first_rect_for_range as extern "C" fn(&Object, Sel, ObjcId) -> NSRect,
        );
        decl.add_method(
            sel!(caretRectForPosition:),
            caret_rect_for_position as extern "C" fn(&Object, Sel, ObjcId) -> NSRect,
        );
        decl.add_method(
            sel!(selectionRectsForRange:),
            selection_rects_for_range as extern "C" fn(&Object, Sel, ObjcId) -> ObjcId,
        );
        decl.add_method(
            sel!(closestPositionToPoint:),
            closest_position_to_point as extern "C" fn(&Object, Sel, NSPoint) -> ObjcId,
        );
        decl.add_method(
            sel!(closestPositionToPoint:withinRange:),
            closest_position_to_point_within_range
                as extern "C" fn(&Object, Sel, NSPoint, ObjcId) -> ObjcId,
        );
        decl.add_method(
            sel!(characterRangeAtPoint:),
            character_range_at_point as extern "C" fn(&Object, Sel, NSPoint) -> ObjcId,
        );
        decl.add_method(
            sel!(unobscuredContentRect),
            unobscured_content_rect as extern "C" fn(&Object, Sel) -> NSRect,
        );
        decl.add_method(
            sel!(textInputView),
            text_input_view as extern "C" fn(&Object, Sel) -> ObjcId,
        );

        // UITextInput - Writing direction
        decl.add_method(
            sel!(baseWritingDirectionForPosition:inDirection:),
            base_writing_direction_for_position as extern "C" fn(&Object, Sel, ObjcId, i64) -> i64,
        );
        decl.add_method(
            sel!(setBaseWritingDirection:forRange:),
            set_base_writing_direction as extern "C" fn(&Object, Sel, i64, ObjcId),
        );

        // UITextInput - Delegate and tokenizer
        decl.add_method(
            sel!(inputDelegate),
            input_delegate as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(setInputDelegate:),
            set_input_delegate as extern "C" fn(&mut Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(tokenizer),
            tokenizer as extern "C" fn(&Object, Sel) -> ObjcId,
        );

        // UITextInputTraits
        decl.add_method(
            sel!(keyboardType),
            keyboard_type as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(autocorrectionType),
            autocorrection_type as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(autocapitalizationType),
            autocapitalization_type as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(spellCheckingType),
            spell_checking_type as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(smartQuotesType),
            smart_quotes_type as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(smartDashesType),
            smart_dashes_type as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(smartInsertDeleteType),
            smart_insert_delete_type as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(keyboardAppearance),
            keyboard_appearance as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(returnKeyType),
            return_key_type as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(enablesReturnKeyAutomatically),
            enables_return_key_automatically as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(isSecureTextEntry),
            is_secure_text_entry as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(textContentType),
            text_content_type as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(selectionAffinity),
            selection_affinity as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(setSelectionAffinity:),
            set_selection_affinity as extern "C" fn(&mut Object, Sel, i64),
        );
        decl.add_method(
            sel!(textStylingAtPosition:inDirection:),
            text_styling_at_position as extern "C" fn(&Object, Sel, ObjcId, i64) -> ObjcId,
        );
        decl.add_method(
            sel!(supportsAdaptiveImageGlyph),
            supports_adaptive_image_glyph as extern "C" fn(&Object, Sel) -> BOOL,
        );

        // UITextInput - Dictation and placeholders
        decl.add_method(
            sel!(insertDictationResult:),
            insert_dictation_result as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(insertDictationResultPlaceholder),
            insert_dictation_result_placeholder as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(frameForDictationResultPlaceholder:),
            frame_for_dictation_result_placeholder as extern "C" fn(&Object, Sel, ObjcId) -> NSRect,
        );
        decl.add_method(
            sel!(removeDictationResultPlaceholder:willInsertResult:),
            remove_dictation_result_placeholder as extern "C" fn(&Object, Sel, ObjcId, BOOL),
        );
        decl.add_method(
            sel!(dictationRecognitionFailed),
            dictation_recognition_failed as extern "C" fn(&Object, Sel),
        );
        decl.add_method(
            sel!(dictationRecordingDidEnd),
            dictation_recording_did_end as extern "C" fn(&Object, Sel),
        );
        decl.add_method(
            sel!(insertTextPlaceholderWithSize:),
            insert_text_placeholder_with_size as extern "C" fn(&Object, Sel, NSSize) -> ObjcId,
        );
        decl.add_method(
            sel!(removeTextPlaceholder:),
            remove_text_placeholder as extern "C" fn(&Object, Sel, ObjcId),
        );

        // UITextInput - Floating cursor (keyboard trackpad)
        decl.add_method(
            sel!(beginFloatingCursorAtPoint:),
            begin_floating_cursor as extern "C" fn(&mut Object, Sel, NSPoint),
        );
        decl.add_method(
            sel!(updateFloatingCursorAtPoint:),
            update_floating_cursor as extern "C" fn(&mut Object, Sel, NSPoint),
        );
        decl.add_method(
            sel!(endFloatingCursor),
            end_floating_cursor as extern "C" fn(&mut Object, Sel),
        );
    }

    // Protocol conformance
    if let Some(protocol) = Protocol::get("UIKeyInput") {
        decl.add_protocol(protocol);
    }
    if let Some(protocol) = Protocol::get("UITextInput") {
        decl.add_protocol(protocol);
    }

    return decl.register();
}
