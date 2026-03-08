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
                get_ios_class_global, UI_TEXT_AUTOCORRECTION_DEFAULT, UI_TEXT_AUTOCORRECTION_NO,
            },
        },
    },
    makepad_objc_sys::runtime::Protocol,
};

// Import try_with_ios_app from ios_delegates (shared utility)
use super::ios_delegates::try_with_ios_app;

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

/// Defines the main text input view conforming to UITextInput protocol.
/// This replaces the hidden UITextField and provides full IME support.
pub fn define_text_input_view() -> *const Class {
    let mut decl = ClassDecl::new("MakepadTextInputView", class!(UIView)).unwrap();

    // Instance variables for text input state
    decl.add_ivar::<ObjcId>("markedText"); // NSMutableAttributedString
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

    // ==========================================================================
    // UIResponder methods
    // ==========================================================================

    extern "C" fn can_become_first_responder(_: &Object, _: Sel) -> BOOL {
        YES
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

    extern "C" fn insert_text(this: &Object, _: Sel, text: ObjcId) {
        unsafe {
            let string = nsstring_to_string(text);

            // Handle Enter/Return key specially
            if string == "\n" {
                // Get inputDelegate for notifications
                let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");

                // Notify that text will change
                if input_delegate != nil {
                    let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
                    let () =
                        msg_send![input_delegate, selectionWillChange: this as *const _ as ObjcId];
                }

                // Send the newline as TextInput to Makepad so buffers stay synchronized
                // This is critical for multiline editing, without it, iOS's buffer has "\n"
                // but Makepad's doesn't, causing cursor position desyncs for autocorrect
                IosApp::send_text_input(string.clone(), false);

                // Insert newline at cursor position (not append!)
                let buffer = get_text_buffer(this);
                let cursor: i64 = *this.get_ivar("cursorPosition");
                let buffer_len: u64 = msg_send![buffer, length];
                let insert_pos = (cursor.max(0) as u64).min(buffer_len);
                let () = msg_send![buffer, insertString: text atIndex: insert_pos];

                let new_cursor = cursor + 1; // newline is 1 UTF-16 code unit
                (*(this as *const _ as *mut Object)).set_ivar("cursorPosition", new_cursor);

                // Notify that text did change
                if input_delegate != nil {
                    let () =
                        msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                    let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
                }

                // Also send Return key event for widgets that need to know Enter was pressed
                IosApp::send_return_key();
                return;
            }

            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");

            // Notify that text and selection will change
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, selectionWillChange: this as *const _ as ObjcId];
            }

            // Clear marked text BEFORE sending to Makepad
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text != nil {
                let len: u64 = msg_send![marked_text, length];
                if len > 0 {
                    let mutable_string: ObjcId = msg_send![marked_text, mutableString];
                    let empty = str_to_nsstring("");
                    let () = msg_send![mutable_string, setString: empty];
                }
            }

            // Send the text input event to Makepad
            IosApp::send_text_input(string.clone(), false);

            // Update text buffer - insert at cursor position, not append
            let buffer = get_text_buffer(this);
            let cursor: i64 = *this.get_ivar("cursorPosition");
            let buffer_len: u64 = msg_send![buffer, length];
            let insert_pos = (cursor.max(0) as u64).min(buffer_len);
            let () = msg_send![buffer, insertString: text atIndex: insert_pos];

            // Update cursor position using UTF-16 code units (matches iOS NSString.length)
            let new_cursor = cursor + utf16_len(&string);
            (*(this as *const _ as *mut Object)).set_ivar("cursorPosition", new_cursor);

            // Notify that text and selection did change
            if input_delegate != nil {
                let () = msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
        }
    }

    extern "C" fn delete_backward(this: &Object, _: Sel) {
        unsafe {
            let buffer = get_text_buffer(this);
            let buffer_len: u64 = msg_send![buffer, length];
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
                        (*(this as *const _ as *mut Object))
                            .set_ivar("cursorPosition", prev_char_utf16_start as i64);
                    }
                }
            } else if cursor != 0 {
                // Reset cursor if buffer is empty but cursor wasn't at 0
                (*(this as *const _ as *mut Object)).set_ivar("cursorPosition", 0i64);
            }
        }

        // Send backspace event immediately (not queued) for held delete support
        let time = try_with_ios_app(|app| app.time_now()).unwrap_or(0.0);
        IosApp::do_callback(IosEvent::KeyDown(KeyEvent {
            key_code: KeyCode::Backspace,
            is_repeat: false,
            modifiers: Default::default(),
            time,
        }));
        IosApp::do_callback(IosEvent::KeyUp(KeyEvent {
            key_code: KeyCode::Backspace,
            is_repeat: false,
            modifiers: Default::default(),
            time,
        }));
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
                let cursor: i64 = *this.get_ivar("cursorPosition");
                let range_class = get_ios_class_global().text_range;
                msg_send![range_class, rangeWithStart: cursor end: cursor + (len as i64)]
            } else {
                nil
            }
        }
    }

    extern "C" fn set_marked_text(
        this: &mut Object,
        _: Sel,
        marked_text_input: ObjcId,
        _selected_range: NSRange,
    ) {
        unsafe {
            // Notify inputDelegate that text will change
            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
            }

            let marked_text_ref: &mut ObjcId = this.get_mut_ivar("markedText");

            if *marked_text_ref != nil {
                let () = msg_send![*marked_text_ref, release];
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

            *marked_text_ref = new_marked;

            // Send marked text to Makepad for inline display
            let text_string: ObjcId = msg_send![new_marked, string];
            let marked_string = nsstring_to_string(text_string);

            // Always send with replace_last=true - empty string clears composition preview
            IosApp::send_text_input(marked_string, true);

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

            // Notify that text will change
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, selectionWillChange: this as *const _ as ObjcId];
            }

            // Commit the marked text to Makepad
            IosApp::send_text_input(string.clone(), false);

            // Update text buffer
            let buffer = get_text_buffer(this);
            let cursor: i64 = *this.get_ivar("cursorPosition");
            let buffer_len: u64 = msg_send![buffer, length];
            let insert_pos = (cursor.max(0) as u64).min(buffer_len);
            let () = msg_send![buffer, insertString: text_string atIndex: insert_pos];

            // Update cursor position using UTF-16 code units (matches iOS NSString.length)
            let new_cursor = cursor + utf16_len(&string);
            (*(this as *const _ as *mut Object)).set_ivar("cursorPosition", new_cursor);

            // Clear the marked text
            let mutable_string: ObjcId = msg_send![marked_text, mutableString];
            let empty = str_to_nsstring("");
            let () = msg_send![mutable_string, setString: empty];

            // Notify that text did change
            if input_delegate != nil {
                let () = msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
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
            let start: ObjcId = msg_send![range, start];
            let end: ObjcId = msg_send![range, end];
            if start != nil && end != nil {
                let start_offset: i64 = msg_send![start, offset];
                let end_offset: i64 = msg_send![end, offset];
                this.set_ivar("selectionStart", start_offset);
                this.set_ivar("selectionEnd", end_offset);
                this.set_ivar("cursorPosition", end_offset);
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

            let buffer = get_text_buffer(this);
            let buffer_len: u64 = msg_send![buffer, length];
            let cursor: i64 = *this.get_ivar("cursorPosition");

            // Check if this is querying the marked text range
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text != nil {
                let marked_len: u64 = msg_send![marked_text, length];
                if marked_len > 0 {
                    // Marked text is at cursor position
                    let marked_start = cursor;
                    let marked_end = cursor + marked_len as i64;

                    // If query overlaps with marked text range, return marked text
                    if start_offset >= marked_start && end_offset <= marked_end {
                        let text_string: ObjcId = msg_send![marked_text, string];
                        return text_string;
                    }
                }
            }

            // Otherwise return from buffer
            let start_idx = (start_offset.max(0) as u64).min(buffer_len);
            let end_idx = (end_offset.max(0) as u64).min(buffer_len);

            if start_idx >= end_idx {
                return str_to_nsstring("");
            }

            let range = NSRange {
                location: start_idx,
                length: end_idx - start_idx,
            };
            msg_send![buffer, substringWithRange: range]
        }
    }

    extern "C" fn replace_range_with_text(this: &Object, _: Sel, range: ObjcId, text: ObjcId) {
        unsafe {
            let new_string = nsstring_to_string(text);

            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");

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

            // Send the range replacement event to Makepad
            IosApp::send_text_range_replace(char_start, char_end, new_string.clone());

            // Update cursor position to end of inserted text (using UTF-16 code units)
            let new_cursor = range_start as i64 + utf16_len(&new_string);
            (*(this as *const _ as *mut Object)).set_ivar("cursorPosition", new_cursor);

            if input_delegate != nil {
                let () = msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
        }
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
            // Return the actual buffer length. The iOS text buffer is kept in sync
            // with text input operations, so this should match reality.
            let buffer = get_text_buffer(this);
            let buffer_len: i64 = msg_send![buffer, length];
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: buffer_len]
        }
    }

    extern "C" fn position_from_position_offset(
        _: &Object,
        _: Sel,
        position: ObjcId,
        offset: i64,
    ) -> ObjcId {
        if position == nil {
            return nil;
        }
        unsafe {
            let pos: i64 = msg_send![position, offset];
            let new_pos = pos + offset;
            if new_pos < 0 {
                return nil;
            }
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: new_pos]
        }
    }

    extern "C" fn position_from_position_in_direction_offset(
        _: &Object,
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
            let new_pos = pos + actual_offset;
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

    extern "C" fn character_range_by_extending_position_in_direction(
        _: &Object,
        _: Sel,
        position: ObjcId,
        _direction: i64,
    ) -> ObjcId {
        if position == nil {
            return nil;
        }
        // Return a zero-width range at the position
        unsafe {
            let range_class = get_ios_class_global().text_range;
            msg_send![range_class, rangeWithStartPosition: position endPosition: position]
        }
    }

    // ==========================================================================
    // UITextInput protocol - Geometry methods
    // ==========================================================================

    extern "C" fn first_rect_for_range(this: &Object, _: Sel, _range: ObjcId) -> NSRect {
        // Return a rect at the IME position for candidate window placement.
        // When iOS 16+ native selection display is active, return the bounding
        // rect for the explicit selection handle anchors provided by Rust.
        unsafe {
            let view = this as *const _ as ObjcId;
            let view_frame: NSRect = msg_send![view, frame];
            let selection_visible: BOOL = *this.get_ivar("selection_handles_visible");

            if selection_visible == YES {
                let sx: f64 = *this.get_ivar("selection_handle_start_x");
                let sy: f64 = *this.get_ivar("selection_handle_start_y");
                let ex: f64 = *this.get_ivar("selection_handle_end_x");
                let ey: f64 = *this.get_ivar("selection_handle_end_y");
                let y0 = view_frame.size.height - sy;
                let y1 = view_frame.size.height - ey;
                return NSRect {
                    origin: NSPoint {
                        x: sx.min(ex),
                        y: y0.min(y1),
                    },
                    size: NSSize {
                        width: (sx - ex).abs().max(1.0),
                        height: (y0 - y1).abs().max(1.0),
                    },
                };
            }

            let x: f64 = *this.get_ivar("ime_pos_x");
            let y: f64 = *this.get_ivar("ime_pos_y");

            NSRect {
                origin: NSPoint {
                    x,
                    y: view_frame.size.height - y,
                },
                size: NSSize {
                    width: 1.0,
                    height: 20.0,
                },
            }
        }
    }

    extern "C" fn caret_rect_for_position(this: &Object, sel: Sel, position: ObjcId) -> NSRect {
        unsafe {
            let selection_visible: BOOL = *this.get_ivar("selection_handles_visible");
            if selection_visible == YES {
                let view = this as *const _ as ObjcId;
                let view_frame: NSRect = msg_send![view, frame];
                let offset: i64 = if position != nil {
                    msg_send![position, offset]
                } else {
                    0
                };

                let (x, y) = if offset <= 0 {
                    (
                        *this.get_ivar::<f64>("selection_handle_start_x"),
                        *this.get_ivar::<f64>("selection_handle_start_y"),
                    )
                } else {
                    (
                        *this.get_ivar::<f64>("selection_handle_end_x"),
                        *this.get_ivar::<f64>("selection_handle_end_y"),
                    )
                };

                return NSRect {
                    origin: NSPoint {
                        x,
                        y: view_frame.size.height - y,
                    },
                    size: NSSize {
                        width: 1.0,
                        height: 20.0,
                    },
                };
            }
        }

        first_rect_for_range(this, sel, nil)
    }

    extern "C" fn selection_rects_for_range(this: &Object, _: Sel, _range: ObjcId) -> ObjcId {
        unsafe {
            let selection_visible: BOOL = *this.get_ivar("selection_handles_visible");
            if selection_visible != YES {
                return msg_send![class!(NSArray), array];
            }

            let view = this as *const _ as ObjcId;
            let view_frame: NSRect = msg_send![view, frame];
            let sx: f64 = *this.get_ivar("selection_handle_start_x");
            let sy: f64 = *this.get_ivar("selection_handle_start_y");
            let ex: f64 = *this.get_ivar("selection_handle_end_x");
            let ey: f64 = *this.get_ivar("selection_handle_end_y");
            let y0 = view_frame.size.height - sy;
            let y1 = view_frame.size.height - ey;

            let rect_cls = get_ios_class_global().text_selection_rect;
            let selection_rect: ObjcId = msg_send![
                rect_cls,
                rectWithX: sx.min(ex)
                y: y0.min(y1)
                w: (sx - ex).abs().max(1.0)
                h: (y0 - y1).abs().max(1.0)
                containsStart: YES
                containsEnd: YES
            ];
            msg_send![class!(NSArray), arrayWithObject: selection_rect]
        }
    }

    /// Returns position 0 (no hit testing). Proper implementation would require
    /// access to text layout from the Rust side, which lives in the widget layer,
    /// not the platform layer. As a result, system text cursor drag (moving the
    /// cursor by dragging the iOS loupe/magnifier) won't work.
    extern "C" fn closest_position_to_point(_: &Object, _: Sel, _point: NSPoint) -> ObjcId {
        unsafe {
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: 0i64]
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

    // ==========================================================================
    // Register all methods
    // ==========================================================================

    unsafe {
        // UIResponder
        decl.add_method(
            sel!(canBecomeFirstResponder),
            can_become_first_responder as extern "C" fn(&Object, Sel) -> BOOL,
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
            sel!(replaceRange:withText:),
            replace_range_with_text as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
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
