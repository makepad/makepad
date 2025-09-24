//! XKB (X Keyboard Extension) system bindings for Wayland keyboard input handling
//!
//! This module provides FFI bindings to libxkbcommon, which is used to:
//! - Parse keyboard layouts and keymaps
//! - Convert hardware key codes to key symbols (keysyms)
//! - Handle modifier states (Shift, Control, Alt, etc.)
//! - Support internationalization and complex input methods
//!
//! The XKB library is essential for proper keyboard handling in Wayland applications,
//! as it provides the mapping between physical key presses and their semantic meaning.

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(dead_code)]

use std::os::raw::{c_char, c_int, c_void};

// XKB Context
pub type xkb_context = c_void;
pub type xkb_keymap = c_void;
pub type xkb_state = c_void;

pub type xkb_keycode_t = u32;
pub type xkb_keysym_t = u32;
pub type xkb_layout_index_t = u32;
pub type xkb_layout_mask_t = u32;
pub type xkb_level_index_t = u32;
pub type xkb_mod_index_t = u32;
pub type xkb_mod_mask_t = u32;
pub type xkb_led_index_t = u32;
pub type xkb_led_mask_t = u32;

// XKB Context flags
pub const XKB_CONTEXT_NO_FLAGS: u32 = 0;
pub const XKB_CONTEXT_NO_DEFAULT_INCLUDES: u32 = 1 << 0;
pub const XKB_CONTEXT_NO_ENVIRONMENT_NAMES: u32 = 1 << 1;

// XKB Keymap compile flags
pub const XKB_KEYMAP_COMPILE_NO_FLAGS: u32 = 0;

// XKB Keymap format
#[repr(C)]
pub enum xkb_keymap_format {
    XKB_KEYMAP_FORMAT_TEXT_V1 = 1,
}

// XKB State component
pub const XKB_STATE_MODS_DEPRESSED: u32 = 1 << 0;
pub const XKB_STATE_MODS_LATCHED: u32 = 1 << 1;
pub const XKB_STATE_MODS_LOCKED: u32 = 1 << 2;
pub const XKB_STATE_MODS_EFFECTIVE: u32 = 1 << 3;
pub const XKB_STATE_LAYOUT_DEPRESSED: u32 = 1 << 4;
pub const XKB_STATE_LAYOUT_LATCHED: u32 = 1 << 5;
pub const XKB_STATE_LAYOUT_LOCKED: u32 = 1 << 6;
pub const XKB_STATE_LAYOUT_EFFECTIVE: u32 = 1 << 7;
pub const XKB_STATE_LEDS: u32 = 1 << 8;

// XKB Key direction
#[repr(C)]
pub enum xkb_key_direction {
    XKB_KEY_UP,
    XKB_KEY_DOWN,
}

// Common keysyms (subset of what we need)
pub const XKB_KEY_NoSymbol: xkb_keysym_t = 0x000000;
pub const XKB_KEY_VoidSymbol: xkb_keysym_t = 0xffffff;

// Latin 1 characters
pub const XKB_KEY_space: xkb_keysym_t = 0x0020;
pub const XKB_KEY_exclam: xkb_keysym_t = 0x0021;
pub const XKB_KEY_quotedbl: xkb_keysym_t = 0x0022;
pub const XKB_KEY_numbersign: xkb_keysym_t = 0x0023;
pub const XKB_KEY_dollar: xkb_keysym_t = 0x0024;
pub const XKB_KEY_percent: xkb_keysym_t = 0x0025;
pub const XKB_KEY_ampersand: xkb_keysym_t = 0x0026;
pub const XKB_KEY_apostrophe: xkb_keysym_t = 0x0027;
pub const XKB_KEY_parenleft: xkb_keysym_t = 0x0028;
pub const XKB_KEY_parenright: xkb_keysym_t = 0x0029;
pub const XKB_KEY_asterisk: xkb_keysym_t = 0x002a;
pub const XKB_KEY_plus: xkb_keysym_t = 0x002b;
pub const XKB_KEY_comma: xkb_keysym_t = 0x002c;
pub const XKB_KEY_minus: xkb_keysym_t = 0x002d;
pub const XKB_KEY_period: xkb_keysym_t = 0x002e;
pub const XKB_KEY_slash: xkb_keysym_t = 0x002f;

// Numbers
pub const XKB_KEY_0: xkb_keysym_t = 0x0030;
pub const XKB_KEY_1: xkb_keysym_t = 0x0031;
pub const XKB_KEY_2: xkb_keysym_t = 0x0032;
pub const XKB_KEY_3: xkb_keysym_t = 0x0033;
pub const XKB_KEY_4: xkb_keysym_t = 0x0034;
pub const XKB_KEY_5: xkb_keysym_t = 0x0035;
pub const XKB_KEY_6: xkb_keysym_t = 0x0036;
pub const XKB_KEY_7: xkb_keysym_t = 0x0037;
pub const XKB_KEY_8: xkb_keysym_t = 0x0038;
pub const XKB_KEY_9: xkb_keysym_t = 0x0039;

pub const XKB_KEY_colon: xkb_keysym_t = 0x003a;
pub const XKB_KEY_semicolon: xkb_keysym_t = 0x003b;
pub const XKB_KEY_less: xkb_keysym_t = 0x003c;
pub const XKB_KEY_equal: xkb_keysym_t = 0x003d;
pub const XKB_KEY_greater: xkb_keysym_t = 0x003e;
pub const XKB_KEY_question: xkb_keysym_t = 0x003f;
pub const XKB_KEY_at: xkb_keysym_t = 0x0040;

// Uppercase letters
pub const XKB_KEY_A: xkb_keysym_t = 0x0041;
pub const XKB_KEY_B: xkb_keysym_t = 0x0042;
pub const XKB_KEY_C: xkb_keysym_t = 0x0043;
pub const XKB_KEY_D: xkb_keysym_t = 0x0044;
pub const XKB_KEY_E: xkb_keysym_t = 0x0045;
pub const XKB_KEY_F: xkb_keysym_t = 0x0046;
pub const XKB_KEY_G: xkb_keysym_t = 0x0047;
pub const XKB_KEY_H: xkb_keysym_t = 0x0048;
pub const XKB_KEY_I: xkb_keysym_t = 0x0049;
pub const XKB_KEY_J: xkb_keysym_t = 0x004a;
pub const XKB_KEY_K: xkb_keysym_t = 0x004b;
pub const XKB_KEY_L: xkb_keysym_t = 0x004c;
pub const XKB_KEY_M: xkb_keysym_t = 0x004d;
pub const XKB_KEY_N: xkb_keysym_t = 0x004e;
pub const XKB_KEY_O: xkb_keysym_t = 0x004f;
pub const XKB_KEY_P: xkb_keysym_t = 0x0050;
pub const XKB_KEY_Q: xkb_keysym_t = 0x0051;
pub const XKB_KEY_R: xkb_keysym_t = 0x0052;
pub const XKB_KEY_S: xkb_keysym_t = 0x0053;
pub const XKB_KEY_T: xkb_keysym_t = 0x0054;
pub const XKB_KEY_U: xkb_keysym_t = 0x0055;
pub const XKB_KEY_V: xkb_keysym_t = 0x0056;
pub const XKB_KEY_W: xkb_keysym_t = 0x0057;
pub const XKB_KEY_X: xkb_keysym_t = 0x0058;
pub const XKB_KEY_Y: xkb_keysym_t = 0x0059;
pub const XKB_KEY_Z: xkb_keysym_t = 0x005a;

pub const XKB_KEY_bracketleft: xkb_keysym_t = 0x005b;
pub const XKB_KEY_backslash: xkb_keysym_t = 0x005c;
pub const XKB_KEY_bracketright: xkb_keysym_t = 0x005d;
pub const XKB_KEY_asciicircum: xkb_keysym_t = 0x005e;
pub const XKB_KEY_underscore: xkb_keysym_t = 0x005f;
pub const XKB_KEY_grave: xkb_keysym_t = 0x0060;
pub const XKB_KEY_quoteleft: xkb_keysym_t = 0x0060;

// Lowercase letters
pub const XKB_KEY_a: xkb_keysym_t = 0x0061;
pub const XKB_KEY_b: xkb_keysym_t = 0x0062;
pub const XKB_KEY_c: xkb_keysym_t = 0x0063;
pub const XKB_KEY_d: xkb_keysym_t = 0x0064;
pub const XKB_KEY_e: xkb_keysym_t = 0x0065;
pub const XKB_KEY_f: xkb_keysym_t = 0x0066;
pub const XKB_KEY_g: xkb_keysym_t = 0x0067;
pub const XKB_KEY_h: xkb_keysym_t = 0x0068;
pub const XKB_KEY_i: xkb_keysym_t = 0x0069;
pub const XKB_KEY_j: xkb_keysym_t = 0x006a;
pub const XKB_KEY_k: xkb_keysym_t = 0x006b;
pub const XKB_KEY_l: xkb_keysym_t = 0x006c;
pub const XKB_KEY_m: xkb_keysym_t = 0x006d;
pub const XKB_KEY_n: xkb_keysym_t = 0x006e;
pub const XKB_KEY_o: xkb_keysym_t = 0x006f;
pub const XKB_KEY_p: xkb_keysym_t = 0x0070;
pub const XKB_KEY_q: xkb_keysym_t = 0x0071;
pub const XKB_KEY_r: xkb_keysym_t = 0x0072;
pub const XKB_KEY_s: xkb_keysym_t = 0x0073;
pub const XKB_KEY_t: xkb_keysym_t = 0x0074;
pub const XKB_KEY_u: xkb_keysym_t = 0x0075;
pub const XKB_KEY_v: xkb_keysym_t = 0x0076;
pub const XKB_KEY_w: xkb_keysym_t = 0x0077;
pub const XKB_KEY_x: xkb_keysym_t = 0x0078;
pub const XKB_KEY_y: xkb_keysym_t = 0x0079;
pub const XKB_KEY_z: xkb_keysym_t = 0x007a;

pub const XKB_KEY_braceleft: xkb_keysym_t = 0x007b;
pub const XKB_KEY_bar: xkb_keysym_t = 0x007c;
pub const XKB_KEY_braceright: xkb_keysym_t = 0x007d;
pub const XKB_KEY_asciitilde: xkb_keysym_t = 0x007e;

// TTY function keys
pub const XKB_KEY_BackSpace: xkb_keysym_t = 0xff08;
pub const XKB_KEY_Tab: xkb_keysym_t = 0xff09;
pub const XKB_KEY_Linefeed: xkb_keysym_t = 0xff0a;
pub const XKB_KEY_Clear: xkb_keysym_t = 0xff0b;
pub const XKB_KEY_Return: xkb_keysym_t = 0xff0d;
pub const XKB_KEY_Pause: xkb_keysym_t = 0xff13;
pub const XKB_KEY_Scroll_Lock: xkb_keysym_t = 0xff14;
pub const XKB_KEY_Sys_Req: xkb_keysym_t = 0xff15;
pub const XKB_KEY_Escape: xkb_keysym_t = 0xff1b;
pub const XKB_KEY_Delete: xkb_keysym_t = 0xffff;

// Cursor control & motion
pub const XKB_KEY_Home: xkb_keysym_t = 0xff50;
pub const XKB_KEY_Left: xkb_keysym_t = 0xff51;
pub const XKB_KEY_Up: xkb_keysym_t = 0xff52;
pub const XKB_KEY_Right: xkb_keysym_t = 0xff53;
pub const XKB_KEY_Down: xkb_keysym_t = 0xff54;
pub const XKB_KEY_Prior: xkb_keysym_t = 0xff55;
pub const XKB_KEY_Page_Up: xkb_keysym_t = 0xff55;
pub const XKB_KEY_Next: xkb_keysym_t = 0xff56;
pub const XKB_KEY_Page_Down: xkb_keysym_t = 0xff56;
pub const XKB_KEY_End: xkb_keysym_t = 0xff57;
pub const XKB_KEY_Begin: xkb_keysym_t = 0xff58;

// Misc functions
pub const XKB_KEY_Select: xkb_keysym_t = 0xff60;
pub const XKB_KEY_Print: xkb_keysym_t = 0xff61;
pub const XKB_KEY_Execute: xkb_keysym_t = 0xff62;
pub const XKB_KEY_Insert: xkb_keysym_t = 0xff63;
pub const XKB_KEY_Undo: xkb_keysym_t = 0xff65;
pub const XKB_KEY_Redo: xkb_keysym_t = 0xff66;
pub const XKB_KEY_Menu: xkb_keysym_t = 0xff67;
pub const XKB_KEY_Find: xkb_keysym_t = 0xff68;
pub const XKB_KEY_Cancel: xkb_keysym_t = 0xff69;
pub const XKB_KEY_Help: xkb_keysym_t = 0xff6a;
pub const XKB_KEY_Break: xkb_keysym_t = 0xff6b;
pub const XKB_KEY_Mode_switch: xkb_keysym_t = 0xff7e;
pub const XKB_KEY_script_switch: xkb_keysym_t = 0xff7e;
pub const XKB_KEY_Num_Lock: xkb_keysym_t = 0xff7f;

// Keypad
pub const XKB_KEY_KP_Space: xkb_keysym_t = 0xff80;
pub const XKB_KEY_KP_Tab: xkb_keysym_t = 0xff89;
pub const XKB_KEY_KP_Enter: xkb_keysym_t = 0xff8d;
pub const XKB_KEY_KP_F1: xkb_keysym_t = 0xff91;
pub const XKB_KEY_KP_F2: xkb_keysym_t = 0xff92;
pub const XKB_KEY_KP_F3: xkb_keysym_t = 0xff93;
pub const XKB_KEY_KP_F4: xkb_keysym_t = 0xff94;
pub const XKB_KEY_KP_Home: xkb_keysym_t = 0xff95;
pub const XKB_KEY_KP_Left: xkb_keysym_t = 0xff96;
pub const XKB_KEY_KP_Up: xkb_keysym_t = 0xff97;
pub const XKB_KEY_KP_Right: xkb_keysym_t = 0xff98;
pub const XKB_KEY_KP_Down: xkb_keysym_t = 0xff99;
pub const XKB_KEY_KP_Prior: xkb_keysym_t = 0xff9a;
pub const XKB_KEY_KP_Page_Up: xkb_keysym_t = 0xff9a;
pub const XKB_KEY_KP_Next: xkb_keysym_t = 0xff9b;
pub const XKB_KEY_KP_Page_Down: xkb_keysym_t = 0xff9b;
pub const XKB_KEY_KP_End: xkb_keysym_t = 0xff9c;
pub const XKB_KEY_KP_Begin: xkb_keysym_t = 0xff9d;
pub const XKB_KEY_KP_Insert: xkb_keysym_t = 0xff9e;
pub const XKB_KEY_KP_Delete: xkb_keysym_t = 0xff9f;
pub const XKB_KEY_KP_Equal: xkb_keysym_t = 0xffbd;
pub const XKB_KEY_KP_Multiply: xkb_keysym_t = 0xffaa;
pub const XKB_KEY_KP_Add: xkb_keysym_t = 0xffab;
pub const XKB_KEY_KP_Separator: xkb_keysym_t = 0xffac;
pub const XKB_KEY_KP_Subtract: xkb_keysym_t = 0xffad;
pub const XKB_KEY_KP_Decimal: xkb_keysym_t = 0xffae;
pub const XKB_KEY_KP_Divide: xkb_keysym_t = 0xffaf;

pub const XKB_KEY_KP_0: xkb_keysym_t = 0xffb0;
pub const XKB_KEY_KP_1: xkb_keysym_t = 0xffb1;
pub const XKB_KEY_KP_2: xkb_keysym_t = 0xffb2;
pub const XKB_KEY_KP_3: xkb_keysym_t = 0xffb3;
pub const XKB_KEY_KP_4: xkb_keysym_t = 0xffb4;
pub const XKB_KEY_KP_5: xkb_keysym_t = 0xffb5;
pub const XKB_KEY_KP_6: xkb_keysym_t = 0xffb6;
pub const XKB_KEY_KP_7: xkb_keysym_t = 0xffb7;
pub const XKB_KEY_KP_8: xkb_keysym_t = 0xffb8;
pub const XKB_KEY_KP_9: xkb_keysym_t = 0xffb9;

// Function keys
pub const XKB_KEY_F1: xkb_keysym_t = 0xffbe;
pub const XKB_KEY_F2: xkb_keysym_t = 0xffbf;
pub const XKB_KEY_F3: xkb_keysym_t = 0xffc0;
pub const XKB_KEY_F4: xkb_keysym_t = 0xffc1;
pub const XKB_KEY_F5: xkb_keysym_t = 0xffc2;
pub const XKB_KEY_F6: xkb_keysym_t = 0xffc3;
pub const XKB_KEY_F7: xkb_keysym_t = 0xffc4;
pub const XKB_KEY_F8: xkb_keysym_t = 0xffc5;
pub const XKB_KEY_F9: xkb_keysym_t = 0xffc6;
pub const XKB_KEY_F10: xkb_keysym_t = 0xffc7;
pub const XKB_KEY_F11: xkb_keysym_t = 0xffc8;
pub const XKB_KEY_L1: xkb_keysym_t = 0xffc8;
pub const XKB_KEY_F12: xkb_keysym_t = 0xffc9;
pub const XKB_KEY_L2: xkb_keysym_t = 0xffc9;
pub const XKB_KEY_F13: xkb_keysym_t = 0xffca;
pub const XKB_KEY_L3: xkb_keysym_t = 0xffca;
pub const XKB_KEY_F14: xkb_keysym_t = 0xffcb;
pub const XKB_KEY_L4: xkb_keysym_t = 0xffcb;
pub const XKB_KEY_F15: xkb_keysym_t = 0xffcc;
pub const XKB_KEY_L5: xkb_keysym_t = 0xffcc;

// Modifiers
pub const XKB_KEY_Shift_L: xkb_keysym_t = 0xffe1;
pub const XKB_KEY_Shift_R: xkb_keysym_t = 0xffe2;
pub const XKB_KEY_Control_L: xkb_keysym_t = 0xffe3;
pub const XKB_KEY_Control_R: xkb_keysym_t = 0xffe4;
pub const XKB_KEY_Caps_Lock: xkb_keysym_t = 0xffe5;
pub const XKB_KEY_Shift_Lock: xkb_keysym_t = 0xffe6;
pub const XKB_KEY_Meta_L: xkb_keysym_t = 0xffe7;
pub const XKB_KEY_Meta_R: xkb_keysym_t = 0xffe8;
pub const XKB_KEY_Alt_L: xkb_keysym_t = 0xffe9;
pub const XKB_KEY_Alt_R: xkb_keysym_t = 0xffea;
pub const XKB_KEY_Super_L: xkb_keysym_t = 0xffeb;
pub const XKB_KEY_Super_R: xkb_keysym_t = 0xffec;
pub const XKB_KEY_Hyper_L: xkb_keysym_t = 0xffed;
pub const XKB_KEY_Hyper_R: xkb_keysym_t = 0xffee;

// ISO 9995 function and modifier keys
pub const XKB_KEY_ISO_Left_Tab: xkb_keysym_t = 0xfe20;

#[link(name = "xkbcommon")]
extern "C" {
    // Context management
    pub fn xkb_context_new(flags: u32) -> *mut xkb_context;
    pub fn xkb_context_unref(context: *mut xkb_context);
    pub fn xkb_context_ref(context: *mut xkb_context) -> *mut xkb_context;

    // Keymap management
    pub fn xkb_keymap_new_from_string(
        context: *mut xkb_context,
        string: *const c_char,
        format: xkb_keymap_format,
        flags: u32,
    ) -> *mut xkb_keymap;

    pub fn xkb_keymap_unref(keymap: *mut xkb_keymap);
    pub fn xkb_keymap_ref(keymap: *mut xkb_keymap) -> *mut xkb_keymap;

    // State management
    pub fn xkb_state_new(keymap: *mut xkb_keymap) -> *mut xkb_state;
    pub fn xkb_state_unref(state: *mut xkb_state);
    pub fn xkb_state_ref(state: *mut xkb_state) -> *mut xkb_state;

    // State updates
    pub fn xkb_state_update_mask(
        state: *mut xkb_state,
        depressed_mods: xkb_mod_mask_t,
        latched_mods: xkb_mod_mask_t,
        locked_mods: xkb_mod_mask_t,
        depressed_layout: xkb_layout_index_t,
        latched_layout: xkb_layout_index_t,
        locked_layout: xkb_layout_index_t,
    ) -> u32;

    pub fn xkb_state_update_key(
        state: *mut xkb_state,
        key: xkb_keycode_t,
        direction: xkb_key_direction,
    ) -> u32;

    // Key symbol queries
    pub fn xkb_state_key_get_syms(
        state: *mut xkb_state,
        key: xkb_keycode_t,
        syms_out: *mut *const xkb_keysym_t,
    ) -> c_int;

    pub fn xkb_state_key_get_one_sym(
        state: *mut xkb_state,
        key: xkb_keycode_t,
    ) -> xkb_keysym_t;

    // UTF-8 string queries
    pub fn xkb_state_key_get_utf8(
        state: *mut xkb_state,
        key: xkb_keycode_t,
        buffer: *mut c_char,
        size: usize,
    ) -> c_int;

    // Modifier state queries
    pub fn xkb_state_mod_name_is_active(
        state: *mut xkb_state,
        name: *const c_char,
        type_: u32,
    ) -> c_int;

    pub fn xkb_state_mod_names_are_active(
        state: *mut xkb_state,
        type_: u32,
        match_: u32,
        names: *const *const c_char,
    ) -> c_int;

    // Layout queries
    pub fn xkb_state_layout_name_is_active(
        state: *mut xkb_state,
        name: *const c_char,
        type_: u32,
    ) -> c_int;

    pub fn xkb_state_layout_index_is_active(
        state: *mut xkb_state,
        idx: xkb_layout_index_t,
        type_: u32,
    ) -> c_int;

    // LED state queries
    pub fn xkb_state_led_name_is_active(
        state: *mut xkb_state,
        name: *const c_char,
    ) -> c_int;

    pub fn xkb_state_led_index_is_active(
        state: *mut xkb_state,
        idx: xkb_led_index_t,
    ) -> c_int;

    // Keymap queries
    pub fn xkb_keymap_mod_get_index(
        keymap: *mut xkb_keymap,
        name: *const c_char,
    ) -> xkb_mod_index_t;

    pub fn xkb_keymap_layout_get_index(
        keymap: *mut xkb_keymap,
        name: *const c_char,
    ) -> xkb_layout_index_t;

    pub fn xkb_keymap_led_get_index(
        keymap: *mut xkb_keymap,
        name: *const c_char,
    ) -> xkb_led_index_t;

    // Key repeat
    pub fn xkb_keymap_key_repeats(
        keymap: *mut xkb_keymap,
        key: xkb_keycode_t,
    ) -> c_int;

    // Keysym utilities
    pub fn xkb_keysym_get_name(
        keysym: xkb_keysym_t,
        buffer: *mut c_char,
        size: usize,
    ) -> c_int;

    pub fn xkb_keysym_from_name(
        name: *const c_char,
        flags: u32,
    ) -> xkb_keysym_t;

    pub fn xkb_keysym_to_utf8(
        keysym: xkb_keysym_t,
        buffer: *mut c_char,
        size: usize,
    ) -> c_int;

    pub fn xkb_keysym_to_utf32(keysym: xkb_keysym_t) -> u32;

    // Compose support (for dead keys)
    pub fn xkb_compose_table_new_from_locale(
        context: *mut xkb_context,
        locale: *const c_char,
        flags: u32,
    ) -> *mut c_void; // xkb_compose_table

    pub fn xkb_compose_state_new(
        table: *mut c_void, // xkb_compose_table
        flags: u32,
    ) -> *mut c_void; // xkb_compose_state

    pub fn xkb_compose_state_feed(
        state: *mut c_void, // xkb_compose_state
        keysym: xkb_keysym_t,
    ) -> u32; // xkb_compose_feed_result

    pub fn xkb_compose_state_get_utf8(
        state: *mut c_void, // xkb_compose_state
        buffer: *mut c_char,
        size: usize,
    ) -> c_int;

    pub fn xkb_compose_state_get_one_sym(
        state: *mut c_void, // xkb_compose_state
    ) -> xkb_keysym_t;

    pub fn xkb_compose_state_get_status(
        state: *mut c_void, // xkb_compose_state
    ) -> u32; // xkb_compose_status

    pub fn xkb_compose_state_reset(
        state: *mut c_void, // xkb_compose_state
    );

    pub fn xkb_compose_state_unref(
        state: *mut c_void, // xkb_compose_state
    );

    pub fn xkb_compose_table_unref(
        table: *mut c_void, // xkb_compose_table
    );
}

// Helper constants for common modifier names
pub const MOD_NAME_SHIFT: &'static [u8] = b"Shift\0";
pub const MOD_NAME_CAPS: &'static [u8] = b"Lock\0";
pub const MOD_NAME_CTRL: &'static [u8] = b"Control\0";
pub const MOD_NAME_ALT: &'static [u8] = b"Mod1\0";
pub const MOD_NAME_NUM: &'static [u8] = b"Mod2\0";
pub const MOD_NAME_LOGO: &'static [u8] = b"Mod4\0";

// LED names
pub const LED_NAME_CAPS: &'static [u8] = b"Caps Lock\0";
pub const LED_NAME_NUM: &'static [u8] = b"Num Lock\0";
pub const LED_NAME_SCROLL: &'static [u8] = b"Scroll Lock\0";

// Helper function to convert XKB keysyms to Makepad KeyCodes
// This follows the same pattern as the X11 implementation
pub fn xkb_keysym_to_keycode(keysym: xkb_keysym_t) -> crate::event::keyboard::KeyCode {
    use crate::event::keyboard::KeyCode;

    match keysym {
        XKB_KEY_a | XKB_KEY_A => KeyCode::KeyA,
        XKB_KEY_b | XKB_KEY_B => KeyCode::KeyB,
        XKB_KEY_c | XKB_KEY_C => KeyCode::KeyC,
        XKB_KEY_d | XKB_KEY_D => KeyCode::KeyD,
        XKB_KEY_e | XKB_KEY_E => KeyCode::KeyE,
        XKB_KEY_f | XKB_KEY_F => KeyCode::KeyF,
        XKB_KEY_g | XKB_KEY_G => KeyCode::KeyG,
        XKB_KEY_h | XKB_KEY_H => KeyCode::KeyH,
        XKB_KEY_i | XKB_KEY_I => KeyCode::KeyI,
        XKB_KEY_j | XKB_KEY_J => KeyCode::KeyJ,
        XKB_KEY_k | XKB_KEY_K => KeyCode::KeyK,
        XKB_KEY_l | XKB_KEY_L => KeyCode::KeyL,
        XKB_KEY_m | XKB_KEY_M => KeyCode::KeyM,
        XKB_KEY_n | XKB_KEY_N => KeyCode::KeyN,
        XKB_KEY_o | XKB_KEY_O => KeyCode::KeyO,
        XKB_KEY_p | XKB_KEY_P => KeyCode::KeyP,
        XKB_KEY_q | XKB_KEY_Q => KeyCode::KeyQ,
        XKB_KEY_r | XKB_KEY_R => KeyCode::KeyR,
        XKB_KEY_s | XKB_KEY_S => KeyCode::KeyS,
        XKB_KEY_t | XKB_KEY_T => KeyCode::KeyT,
        XKB_KEY_u | XKB_KEY_U => KeyCode::KeyU,
        XKB_KEY_v | XKB_KEY_V => KeyCode::KeyV,
        XKB_KEY_w | XKB_KEY_W => KeyCode::KeyW,
        XKB_KEY_x | XKB_KEY_X => KeyCode::KeyX,
        XKB_KEY_y | XKB_KEY_Y => KeyCode::KeyY,
        XKB_KEY_z | XKB_KEY_Z => KeyCode::KeyZ,

        XKB_KEY_0 => KeyCode::Key0,
        XKB_KEY_1 => KeyCode::Key1,
        XKB_KEY_2 => KeyCode::Key2,
        XKB_KEY_3 => KeyCode::Key3,
        XKB_KEY_4 => KeyCode::Key4,
        XKB_KEY_5 => KeyCode::Key5,
        XKB_KEY_6 => KeyCode::Key6,
        XKB_KEY_7 => KeyCode::Key7,
        XKB_KEY_8 => KeyCode::Key8,
        XKB_KEY_9 => KeyCode::Key9,

        XKB_KEY_Alt_L | XKB_KEY_Alt_R => KeyCode::Alt,
        XKB_KEY_Meta_L | XKB_KEY_Meta_R | XKB_KEY_Super_L | XKB_KEY_Super_R => KeyCode::Logo,
        XKB_KEY_Shift_L | XKB_KEY_Shift_R => KeyCode::Shift,
        XKB_KEY_Control_L | XKB_KEY_Control_R => KeyCode::Control,

        XKB_KEY_equal => KeyCode::Equals,
        XKB_KEY_minus => KeyCode::Minus,
        XKB_KEY_bracketright => KeyCode::RBracket,
        XKB_KEY_bracketleft => KeyCode::LBracket,
        XKB_KEY_Return => KeyCode::ReturnKey,
        XKB_KEY_grave => KeyCode::Backtick,
        XKB_KEY_semicolon => KeyCode::Semicolon,
        XKB_KEY_backslash => KeyCode::Backslash,
        XKB_KEY_comma => KeyCode::Comma,
        XKB_KEY_slash => KeyCode::Slash,
        XKB_KEY_period => KeyCode::Period,
        XKB_KEY_Tab | XKB_KEY_ISO_Left_Tab => KeyCode::Tab,
        XKB_KEY_space => KeyCode::Space,
        XKB_KEY_BackSpace => KeyCode::Backspace,
        XKB_KEY_Escape => KeyCode::Escape,
        XKB_KEY_Caps_Lock => KeyCode::Capslock,
        XKB_KEY_KP_Decimal => KeyCode::NumpadDecimal,
        XKB_KEY_KP_Multiply => KeyCode::NumpadMultiply,
        XKB_KEY_KP_Add => KeyCode::NumpadAdd,
        XKB_KEY_Num_Lock => KeyCode::Numlock,
        XKB_KEY_KP_Divide => KeyCode::NumpadDivide,
        XKB_KEY_KP_Enter => KeyCode::NumpadEnter,
        XKB_KEY_KP_Subtract => KeyCode::NumpadSubtract,
        XKB_KEY_KP_Equal => KeyCode::NumpadEquals,
        XKB_KEY_KP_0 => KeyCode::Numpad0,
        XKB_KEY_KP_1 => KeyCode::Numpad1,
        XKB_KEY_KP_2 => KeyCode::Numpad2,
        XKB_KEY_KP_3 => KeyCode::Numpad3,
        XKB_KEY_KP_4 => KeyCode::Numpad4,
        XKB_KEY_KP_5 => KeyCode::Numpad5,
        XKB_KEY_KP_6 => KeyCode::Numpad6,
        XKB_KEY_KP_7 => KeyCode::Numpad7,
        XKB_KEY_KP_8 => KeyCode::Numpad8,
        XKB_KEY_KP_9 => KeyCode::Numpad9,

        XKB_KEY_F1 => KeyCode::F1,
        XKB_KEY_F2 => KeyCode::F2,
        XKB_KEY_F3 => KeyCode::F3,
        XKB_KEY_F4 => KeyCode::F4,
        XKB_KEY_F5 => KeyCode::F5,
        XKB_KEY_F6 => KeyCode::F6,
        XKB_KEY_F7 => KeyCode::F7,
        XKB_KEY_F8 => KeyCode::F8,
        XKB_KEY_F9 => KeyCode::F9,
        XKB_KEY_F10 => KeyCode::F10,
        XKB_KEY_F11 => KeyCode::F11,
        XKB_KEY_F12 => KeyCode::F12,

        XKB_KEY_Print => KeyCode::PrintScreen,
        XKB_KEY_Scroll_Lock => KeyCode::ScrollLock,
        XKB_KEY_Pause => KeyCode::Pause,
        XKB_KEY_Home => KeyCode::Home,
        XKB_KEY_Page_Up => KeyCode::PageUp,
        XKB_KEY_Delete => KeyCode::Delete,
        XKB_KEY_End => KeyCode::End,
        XKB_KEY_Page_Down => KeyCode::PageDown,
        XKB_KEY_Left => KeyCode::ArrowLeft,
        XKB_KEY_Right => KeyCode::ArrowRight,
        XKB_KEY_Down => KeyCode::ArrowDown,
        XKB_KEY_Up => KeyCode::ArrowUp,
        XKB_KEY_Insert => KeyCode::Insert,
        XKB_KEY_apostrophe => KeyCode::Quote,

        _ => KeyCode::Unknown,
    }
}

// Safe wrapper layer for XKB functionality
use std::ptr;
use std::ffi::CString;

/// Safe wrapper around XKB context
pub struct XkbContext {
    ptr: *mut xkb_context,
}

impl XkbContext {
    /// Create a new XKB context
    pub fn new() -> Option<Self> {
        let ptr = unsafe { xkb_context_new(XKB_CONTEXT_NO_FLAGS) };
        if ptr.is_null() {
            None
        } else {
            Some(XkbContext { ptr })
        }
    }

    /// Get the raw pointer (for internal use)
    pub(crate) fn as_ptr(&self) -> *mut xkb_context {
        self.ptr
    }
}

impl Drop for XkbContext {
    fn drop(&mut self) {
        unsafe {
            xkb_context_unref(self.ptr);
        }
    }
}

unsafe impl Send for XkbContext {}
unsafe impl Sync for XkbContext {}

/// Safe wrapper around XKB keymap
pub struct XkbKeymap {
    ptr: *mut xkb_keymap,
}

impl XkbKeymap {
    /// Create a keymap from a string
    pub fn from_cstr(context: &XkbContext, keymap_string: *mut c_void) -> Option<Self> {
        let ptr = unsafe {
            xkb_keymap_new_from_string(
                context.as_ptr(),
                keymap_string as *const c_char,
                xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1,
                XKB_KEYMAP_COMPILE_NO_FLAGS,
            )
        };

        if ptr.is_null() {
            println!("failed to create xkb state");
            None
        } else {
            Some(XkbKeymap { ptr })
        }
    }

    /// Check if a key repeats
    pub fn key_repeats(&self, keycode: u32) -> bool {
        unsafe { xkb_keymap_key_repeats(self.ptr, keycode) != 0 }
    }

    /// Get modifier index by name
    pub fn mod_get_index(&self, name: &str) -> Option<u32> {
        let c_name = CString::new(name).ok()?;
        let index = unsafe { xkb_keymap_mod_get_index(self.ptr, c_name.as_ptr()) };
        if index == u32::MAX {
            None
        } else {
            Some(index)
        }
    }

    /// Get layout index by name
    pub fn layout_get_index(&self, name: &str) -> Option<u32> {
        let c_name = CString::new(name).ok()?;
        let index = unsafe { xkb_keymap_layout_get_index(self.ptr, c_name.as_ptr()) };
        if index == u32::MAX {
            None
        } else {
            Some(index)
        }
    }

    /// Get the raw pointer (for internal use)
    pub(crate) fn as_ptr(&self) -> *mut xkb_keymap {
        self.ptr
    }
}

impl Drop for XkbKeymap {
    fn drop(&mut self) {
        unsafe {
            xkb_keymap_unref(self.ptr);
        }
    }
}

unsafe impl Send for XkbKeymap {}
unsafe impl Sync for XkbKeymap {}

/// Safe wrapper around XKB state
pub struct XkbState {
    ptr: *mut xkb_state,
}

impl XkbState {
    /// Create a new XKB state from a keymap
    pub fn new(keymap: &XkbKeymap) -> Option<Self> {
        let ptr = unsafe { xkb_state_new(keymap.as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(XkbState { ptr })
        }
    }

    /// Update the state with modifier mask
    pub fn update_mask(
        &mut self,
        depressed_mods: u32,
        latched_mods: u32,
        locked_mods: u32,
        depressed_layout: u32,
        latched_layout: u32,
        locked_layout: u32,
    ) -> u32 {
        unsafe {
            xkb_state_update_mask(
                self.ptr,
                depressed_mods,
                latched_mods,
                locked_mods,
                depressed_layout,
                latched_layout,
                locked_layout,
            )
        }
    }

    /// Update the state with a key press/release
    pub fn update_key(&mut self, keycode: u32, direction: XkbKeyDirection) -> u32 {
        let dir = match direction {
            XkbKeyDirection::Down => xkb_key_direction::XKB_KEY_DOWN,
            XkbKeyDirection::Up => xkb_key_direction::XKB_KEY_UP,
        };
        unsafe { xkb_state_update_key(self.ptr, keycode, dir) }
    }

    /// Get the primary keysym for a key
    pub fn key_get_one_sym(&self, keycode: u32) -> u32 {
        unsafe { xkb_state_key_get_one_sym(self.ptr, keycode) }
    }

    /// Get all keysyms for a key
    pub fn key_get_syms(&self, keycode: u32) -> Vec<u32> {
        let mut syms_ptr: *const u32 = ptr::null();
        let count = unsafe { xkb_state_key_get_syms(self.ptr, keycode, &mut syms_ptr) };

        if count <= 0 || syms_ptr.is_null() {
            return Vec::new();
        }

        let syms_slice = unsafe { std::slice::from_raw_parts(syms_ptr, count as usize) };
        syms_slice.to_vec()
    }

    /// Get UTF-8 string for a key
    pub fn key_get_utf8(&self, keycode: u32) -> String {
        // First, get the required buffer size
        let size = unsafe { xkb_state_key_get_utf8(self.ptr, keycode, ptr::null_mut(), 0) };

        if size <= 0 {
            return String::new();
        }

        // Allocate buffer and get the actual string, including the null terminator
        let mut buffer = vec![0u8; (size + 1) as usize];
        let actual_size = unsafe {
            xkb_state_key_get_utf8(
                self.ptr,
                keycode,
                buffer.as_mut_ptr() as *mut c_char,
                buffer.len(),
            )
        };

        if actual_size > 0 {
            // Remove the null terminator if present
            if buffer.last() == Some(&0) {
                buffer.pop();
            }
            String::from_utf8(buffer).unwrap_or_default()
        } else {
            String::new()
        }
    }

    /// Check if a modifier is active by name
    pub fn mod_name_is_active(&self, name: &str, state_type: XkbStateComponent) -> bool {
        let c_name = match CString::new(name) {
            Ok(s) => s,
            Err(_) => return false,
        };

        let type_flags = state_type.to_flags();
        unsafe { xkb_state_mod_name_is_active(self.ptr, c_name.as_ptr(), type_flags) != 0 }
    }

    /// Check if a layout is active by index
    pub fn layout_index_is_active(&self, index: u32, state_type: XkbStateComponent) -> bool {
        let type_flags = state_type.to_flags();
        unsafe { xkb_state_layout_index_is_active(self.ptr, index, type_flags) != 0 }
    }

    /// Check if an LED is active by name
    pub fn led_name_is_active(&self, name: &str) -> bool {
        let c_name = match CString::new(name) {
            Ok(s) => s,
            Err(_) => return false,
        };

        unsafe { xkb_state_led_name_is_active(self.ptr, c_name.as_ptr()) != 0 }
    }

    /// Convert a keycode to Makepad KeyCode using the current state
    pub fn keycode_to_makepad_keycode(&self, keycode: u32) -> crate::event::keyboard::KeyCode {
        let keysym = self.key_get_one_sym(keycode);
        xkb_keysym_to_keycode(keysym)
    }

    /// Get the raw pointer (for internal use)
    pub(crate) fn as_ptr(&self) -> *mut xkb_state {
        self.ptr
    }
}

impl Drop for XkbState {
    fn drop(&mut self) {
        unsafe {
            xkb_state_unref(self.ptr);
        }
    }
}

unsafe impl Send for XkbState {}
unsafe impl Sync for XkbState {}

/// Key direction for state updates
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XkbKeyDirection {
    Up,
    Down,
}

/// State component flags for querying modifier/layout state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XkbStateComponent {
    ModsDepressed,
    ModsLatched,
    ModsLocked,
    ModsEffective,
    LayoutDepressed,
    LayoutLatched,
    LayoutLocked,
    LayoutEffective,
    Leds,
}

impl XkbStateComponent {
    fn to_flags(self) -> u32 {
        match self {
            XkbStateComponent::ModsDepressed => XKB_STATE_MODS_DEPRESSED,
            XkbStateComponent::ModsLatched => XKB_STATE_MODS_LATCHED,
            XkbStateComponent::ModsLocked => XKB_STATE_MODS_LOCKED,
            XkbStateComponent::ModsEffective => XKB_STATE_MODS_EFFECTIVE,
            XkbStateComponent::LayoutDepressed => XKB_STATE_LAYOUT_DEPRESSED,
            XkbStateComponent::LayoutLatched => XKB_STATE_LAYOUT_LATCHED,
            XkbStateComponent::LayoutLocked => XKB_STATE_LAYOUT_LOCKED,
            XkbStateComponent::LayoutEffective => XKB_STATE_LAYOUT_EFFECTIVE,
            XkbStateComponent::Leds => XKB_STATE_LEDS,
        }
    }
}

/// Compose table and state for dead key handling
pub struct XkbCompose {
    table: *mut c_void,
    state: *mut c_void,
}

impl XkbCompose {
    /// Create a new compose table and state for the given locale
    pub fn new(context: &XkbContext, locale: &str) -> Option<Self> {
        let c_locale = CString::new(locale).ok()?;
        let table = unsafe {
            xkb_compose_table_new_from_locale(context.as_ptr(), c_locale.as_ptr(), 0)
        };

        if table.is_null() {
            return None;
        }

        let state = unsafe { xkb_compose_state_new(table, 0) };
        if state.is_null() {
            unsafe { xkb_compose_table_unref(table) };
            return None;
        }

        Some(XkbCompose { table, state })
    }

    /// Feed a keysym to the compose state
    pub fn feed(&mut self, keysym: u32) -> XkbComposeFeedResult {
        let result = unsafe { xkb_compose_state_feed(self.state, keysym) };
        match result {
            0 => XkbComposeFeedResult::Ignored,
            1 => XkbComposeFeedResult::Accepted,
            2 => XkbComposeFeedResult::Nothing,
            _ => XkbComposeFeedResult::Nothing,
        }
    }

    /// Get the current compose status
    pub fn get_status(&self) -> XkbComposeStatus {
        let status = unsafe { xkb_compose_state_get_status(self.state) };
        match status {
            0 => XkbComposeStatus::Nothing,
            1 => XkbComposeStatus::Composing,
            2 => XkbComposeStatus::Composed,
            3 => XkbComposeStatus::Cancelled,
            _ => XkbComposeStatus::Nothing,
        }
    }

    /// Get the composed UTF-8 string
    pub fn get_utf8(&self) -> String {
        let size = unsafe { xkb_compose_state_get_utf8(self.state, ptr::null_mut(), 0) };

        if size <= 0 {
            return String::new();
        }

        let mut buffer = vec![0u8; size as usize];
        let actual_size = unsafe {
            xkb_compose_state_get_utf8(
                self.state,
                buffer.as_mut_ptr() as *mut c_char,
                buffer.len(),
            )
        };

        if actual_size > 0 {
            if buffer.last() == Some(&0) {
                buffer.pop();
            }
            String::from_utf8(buffer).unwrap_or_default()
        } else {
            String::new()
        }
    }

    /// Get the composed keysym
    pub fn get_one_sym(&self) -> u32 {
        unsafe { xkb_compose_state_get_one_sym(self.state) }
    }

    /// Reset the compose state
    pub fn reset(&mut self) {
        unsafe { xkb_compose_state_reset(self.state) };
    }
}

impl Drop for XkbCompose {
    fn drop(&mut self) {
        unsafe {
            xkb_compose_state_unref(self.state);
            xkb_compose_table_unref(self.table);
        }
    }
}

unsafe impl Send for XkbCompose {}
unsafe impl Sync for XkbCompose {}

/// Result of feeding a keysym to the compose state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XkbComposeFeedResult {
    Ignored,
    Accepted,
    Nothing,
}

/// Status of the compose state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XkbComposeStatus {
    Nothing,
    Composing,
    Composed,
    Cancelled,
}

/// Utility functions for common XKB operations
impl XkbState {
    /// Check if Shift is pressed
    pub fn shift_active(&self) -> bool {
        self.mod_name_is_active("Shift", XkbStateComponent::ModsEffective)
    }

    /// Check if Control is pressed
    pub fn control_active(&self) -> bool {
        self.mod_name_is_active("Control", XkbStateComponent::ModsEffective)
    }

    /// Check if Alt is pressed
    pub fn alt_active(&self) -> bool {
        self.mod_name_is_active("Mod1", XkbStateComponent::ModsEffective)
    }

    /// Check if Logo/Super is pressed
    pub fn logo_active(&self) -> bool {
        self.mod_name_is_active("Mod4", XkbStateComponent::ModsEffective)
    }

    /// Get current key modifiers as Makepad KeyModifiers
    pub fn get_key_modifiers(&self) -> crate::KeyModifiers {
        crate::KeyModifiers {
            shift: self.shift_active(),
            control: self.control_active(),
            alt: self.alt_active(),
            logo: self.logo_active(),
        }
    }

    /// Check if Caps Lock is active
    pub fn caps_lock_active(&self) -> bool {
        self.led_name_is_active("Caps Lock")
    }

    /// Check if Num Lock is active
    pub fn num_lock_active(&self) -> bool {
        self.led_name_is_active("Num Lock")
    }

    /// Check if Scroll Lock is active
    pub fn scroll_lock_active(&self) -> bool {
        self.led_name_is_active("Scroll Lock")
    }
}

/// Helper function to convert keysym name to keysym value
pub fn keysym_from_name(name: &str) -> Option<u32> {
    let c_name = CString::new(name).ok()?;
    let keysym = unsafe { xkb_keysym_from_name(c_name.as_ptr(), 0) };
    if keysym == XKB_KEY_NoSymbol {
        None
    } else {
        Some(keysym)
    }
}

/// Helper function to convert keysym value to name
pub fn keysym_get_name(keysym: u32) -> String {
    let mut buffer = [0u8; 64];
    let size = unsafe {
        xkb_keysym_get_name(
            keysym,
            buffer.as_mut_ptr() as *mut c_char,
            buffer.len(),
        )
    };

    if size > 0 && size < buffer.len() as i32 {
        let name_bytes = &buffer[..size as usize];
        String::from_utf8_lossy(name_bytes).to_string()
    } else {
        String::new()
    }
}

/// Helper function to convert keysym to UTF-8 string
pub fn keysym_to_utf8(keysym: u32) -> String {
    let mut buffer = [0u8; 8]; // UTF-8 character can be at most 4 bytes, plus null terminator
    let size = unsafe {
        xkb_keysym_to_utf8(
            keysym,
            buffer.as_mut_ptr() as *mut c_char,
            buffer.len(),
        )
    };

    if size > 0 && size < buffer.len() as i32 {
        let utf8_bytes = &buffer[..size as usize];
        // Remove null terminator if present
        let end = utf8_bytes.iter().position(|&b| b == 0).unwrap_or(utf8_bytes.len());
        String::from_utf8_lossy(&utf8_bytes[..end]).to_string()
    } else {
        String::new()
    }
}

/// Helper function to convert keysym to UTF-32 codepoint
pub fn keysym_to_utf32(keysym: u32) -> Option<char> {
    let codepoint = unsafe { xkb_keysym_to_utf32(keysym) };
    if codepoint == 0 {
        None
    } else {
        char::from_u32(codepoint)
    }
}

/* Example usage of the safe wrapper API:

use crate::os::linux::wayland::xkb_sys::{XkbContext, XkbKeymap, XkbState, XkbKeyDirection};

// Create XKB context
let context = XkbContext::new().expect("Failed to create XKB context");

// Create keymap from string (received from Wayland compositor)
let keymap_string = "..."; // This would come from the Wayland keymap event
let keymap = XkbKeymap::from_string(&context, keymap_string)
    .expect("Failed to create keymap");

// Create state
let mut state = XkbState::new(&keymap).expect("Failed to create XKB state");

// Update state with modifier changes (from Wayland keyboard events)
state.update_mask(
    depressed_mods,  // From wl_keyboard::Event::Modifiers
    latched_mods,
    locked_mods,
    depressed_layout,
    latched_layout,
    locked_layout
);

// Handle key press
let keycode = 38; // From wl_keyboard::Event::Key
state.update_key(keycode, XkbKeyDirection::Down);

// Convert to Makepad KeyCode
let makepad_keycode = state.keycode_to_makepad_keycode(keycode);

// Get modifiers for the event
let modifiers = state.get_key_modifiers();

// Get UTF-8 text (useful for text input)
let text = state.key_get_utf8(keycode);

// Create KeyEvent for Makepad
let key_event = crate::event::keyboard::KeyEvent {
    key_code: makepad_keycode,
    is_repeat: keymap.key_repeats(keycode),
    modifiers: modifiers,
    time: current_time(),
};

// Handle compose sequences (dead keys, etc.)
let mut compose = XkbCompose::new(&context, "en_US.UTF-8")
    .expect("Failed to create compose state");

let keysym = state.key_get_one_sym(keycode);
match compose.feed(keysym) {
    XkbComposeFeedResult::Accepted => {
        match compose.get_status() {
            XkbComposeStatus::Composed => {
                let composed_text = compose.get_utf8();
                // Use composed_text for text input
            }
            XkbComposeStatus::Composing => {
                // Still composing, wait for more input
            }
            _ => {}
        }
    }
    _ => {
        // Use regular keysym/text
    }
}
*/
