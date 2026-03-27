#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

use std::ffi::{c_char, c_void};
use std::os::raw::c_int;

pub type cef_dictionary_value_t = c_void;
pub type cef_preference_registrar_t = c_void;
pub type cef_preferences_type_t = c_int;
pub type cef_request_context_t = c_void;
pub type cef_request_context_handler_t = c_void;
pub type cef_render_process_handler_t = c_void;
pub type cef_resource_bundle_handler_t = c_void;
pub type cef_scheme_registrar_t = c_void;
pub type cef_window_handle_t = *mut c_void;
pub type cef_string_userfree_t = *mut cef_string_t;
pub type cef_string_list_t = *mut c_void;
pub type cef_string_map_t = *mut c_void;
pub type cef_accessibility_handler_t = c_void;
pub type cef_color_t = u32;
pub type cef_drag_data_t = c_void;
pub type cef_drag_operations_mask_t = c_int;
pub type cef_event_flags_t = u32;
pub type cef_horizontal_alignment_t = c_int;
pub type cef_key_event_type_t = c_int;
pub type cef_log_severity_t = c_int;
pub type cef_log_items_t = c_int;
pub type cef_mouse_button_type_t = c_int;
pub type cef_pointer_type_t = c_int;
pub type cef_process_id_t = c_int;
pub type cef_process_message_t = c_void;
pub type cef_runtime_style_t = c_int;
pub type cef_paint_element_type_t = c_int;
pub type cef_size_t = c_void;
pub type cef_state_t = c_int;
pub type cef_text_input_mode_t = c_int;
pub type cef_touch_event_type_t = c_int;
pub type cef_touch_handle_state_t = c_void;
pub type cef_composition_underline_t = c_void;

pub const LOGSEVERITY_DEFAULT: cef_log_severity_t = 0;
pub const LOGSEVERITY_INFO: cef_log_severity_t = 2;
pub const LOGSEVERITY_DISABLE: cef_log_severity_t = 99;
pub const LOG_ITEMS_DEFAULT: cef_log_items_t = 0;
pub const PET_VIEW: cef_paint_element_type_t = 0;
pub const MBT_LEFT: cef_mouse_button_type_t = 0;
pub const MBT_MIDDLE: cef_mouse_button_type_t = 1;
pub const MBT_RIGHT: cef_mouse_button_type_t = 2;
pub const EVENTFLAG_NONE: cef_event_flags_t = 0;
pub const EVENTFLAG_CAPS_LOCK_ON: cef_event_flags_t = 1 << 0;
pub const EVENTFLAG_SHIFT_DOWN: cef_event_flags_t = 1 << 1;
pub const EVENTFLAG_CONTROL_DOWN: cef_event_flags_t = 1 << 2;
pub const EVENTFLAG_ALT_DOWN: cef_event_flags_t = 1 << 3;
pub const EVENTFLAG_LEFT_MOUSE_BUTTON: cef_event_flags_t = 1 << 4;
pub const EVENTFLAG_MIDDLE_MOUSE_BUTTON: cef_event_flags_t = 1 << 5;
pub const EVENTFLAG_RIGHT_MOUSE_BUTTON: cef_event_flags_t = 1 << 6;
pub const EVENTFLAG_COMMAND_DOWN: cef_event_flags_t = 1 << 7;
pub const EVENTFLAG_NUM_LOCK_ON: cef_event_flags_t = 1 << 8;
pub const EVENTFLAG_IS_KEY_PAD: cef_event_flags_t = 1 << 9;
pub const EVENTFLAG_IS_REPEAT: cef_event_flags_t = 1 << 13;
pub const EVENTFLAG_PRECISION_SCROLLING_DELTA: cef_event_flags_t = 1 << 14;
pub const KEYEVENT_RAWKEYDOWN: cef_key_event_type_t = 0;
pub const KEYEVENT_KEYDOWN: cef_key_event_type_t = 1;
pub const KEYEVENT_KEYUP: cef_key_event_type_t = 2;
pub const KEYEVENT_CHAR: cef_key_event_type_t = 3;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_string_t {
    pub str_: *mut u16,
    pub length: usize,
    pub dtor: Option<unsafe extern "C" fn(*mut u16)>,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_base_ref_counted_t {
    pub size: usize,
    pub add_ref: Option<unsafe extern "system" fn(self_: *mut cef_base_ref_counted_t)>,
    pub release: Option<unsafe extern "system" fn(self_: *mut cef_base_ref_counted_t) -> c_int>,
    pub has_one_ref: Option<unsafe extern "system" fn(self_: *mut cef_base_ref_counted_t) -> c_int>,
    pub has_at_least_one_ref:
        Option<unsafe extern "system" fn(self_: *mut cef_base_ref_counted_t) -> c_int>,
}

#[repr(C)]
pub struct cef_command_line_t {
    pub base: cef_base_ref_counted_t,
    pub is_valid: Option<unsafe extern "system" fn(self_: *mut cef_command_line_t) -> c_int>,
    pub is_read_only: Option<unsafe extern "system" fn(self_: *mut cef_command_line_t) -> c_int>,
    pub copy: Option<
        unsafe extern "system" fn(self_: *mut cef_command_line_t) -> *mut cef_command_line_t,
    >,
    pub init_from_argv: Option<
        unsafe extern "system" fn(
            self_: *mut cef_command_line_t,
            argc: c_int,
            argv: *const *const c_char,
        ),
    >,
    pub init_from_string: Option<
        unsafe extern "system" fn(
            self_: *mut cef_command_line_t,
            command_line: *const cef_string_t,
        ),
    >,
    pub reset: Option<unsafe extern "system" fn(self_: *mut cef_command_line_t)>,
    pub get_argv:
        Option<unsafe extern "system" fn(self_: *mut cef_command_line_t, argv: cef_string_list_t)>,
    pub get_command_line_string:
        Option<unsafe extern "system" fn(self_: *mut cef_command_line_t) -> cef_string_userfree_t>,
    pub get_program:
        Option<unsafe extern "system" fn(self_: *mut cef_command_line_t) -> cef_string_userfree_t>,
    pub set_program: Option<
        unsafe extern "system" fn(self_: *mut cef_command_line_t, program: *const cef_string_t),
    >,
    pub has_switches: Option<unsafe extern "system" fn(self_: *mut cef_command_line_t) -> c_int>,
    pub has_switch: Option<
        unsafe extern "system" fn(
            self_: *mut cef_command_line_t,
            name: *const cef_string_t,
        ) -> c_int,
    >,
    pub get_switch_value: Option<
        unsafe extern "system" fn(
            self_: *mut cef_command_line_t,
            name: *const cef_string_t,
        ) -> cef_string_userfree_t,
    >,
    pub get_switches: Option<
        unsafe extern "system" fn(self_: *mut cef_command_line_t, switches: cef_string_map_t),
    >,
    pub append_switch: Option<
        unsafe extern "system" fn(self_: *mut cef_command_line_t, name: *const cef_string_t),
    >,
    pub append_switch_with_value: Option<
        unsafe extern "system" fn(
            self_: *mut cef_command_line_t,
            name: *const cef_string_t,
            value: *const cef_string_t,
        ),
    >,
    pub has_arguments: Option<unsafe extern "system" fn(self_: *mut cef_command_line_t) -> c_int>,
    pub get_arguments: Option<
        unsafe extern "system" fn(self_: *mut cef_command_line_t, arguments: cef_string_list_t),
    >,
    pub append_argument: Option<
        unsafe extern "system" fn(self_: *mut cef_command_line_t, argument: *const cef_string_t),
    >,
    pub prepend_wrapper: Option<
        unsafe extern "system" fn(self_: *mut cef_command_line_t, wrapper: *const cef_string_t),
    >,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_main_args_t {
    pub argc: c_int,
    pub argv: *mut *mut c_char,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_rect_t {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_range_t {
    pub from: u32,
    pub to: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_screen_info_t {
    pub size: usize,
    pub device_scale_factor: f32,
    pub depth: c_int,
    pub depth_per_component: c_int,
    pub is_monochrome: c_int,
    pub rect: cef_rect_t,
    pub available_rect: cef_rect_t,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_settings_t {
    pub size: usize,
    pub no_sandbox: c_int,
    pub browser_subprocess_path: cef_string_t,
    pub framework_dir_path: cef_string_t,
    pub main_bundle_path: cef_string_t,
    pub multi_threaded_message_loop: c_int,
    pub external_message_pump: c_int,
    pub windowless_rendering_enabled: c_int,
    pub command_line_args_disabled: c_int,
    pub cache_path: cef_string_t,
    pub root_cache_path: cef_string_t,
    pub persist_session_cookies: c_int,
    pub user_agent: cef_string_t,
    pub user_agent_product: cef_string_t,
    pub locale: cef_string_t,
    pub log_file: cef_string_t,
    pub log_severity: cef_log_severity_t,
    pub log_items: cef_log_items_t,
    pub javascript_flags: cef_string_t,
    pub resources_dir_path: cef_string_t,
    pub locales_dir_path: cef_string_t,
    pub remote_debugging_port: c_int,
    pub uncaught_exception_stack_size: c_int,
    pub background_color: cef_color_t,
    pub accept_language_list: cef_string_t,
    pub cookieable_schemes_list: cef_string_t,
    pub cookieable_schemes_exclude_defaults: c_int,
    pub chrome_policy_id: cef_string_t,
    pub chrome_app_icon_id: c_int,
    pub disable_signal_handlers: c_int,
    #[cfg(makepad_cef_api_ge_14600)]
    pub use_views_default_popup: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_browser_settings_t {
    pub size: usize,
    pub windowless_frame_rate: c_int,
    pub standard_font_family: cef_string_t,
    pub fixed_font_family: cef_string_t,
    pub serif_font_family: cef_string_t,
    pub sans_serif_font_family: cef_string_t,
    pub cursive_font_family: cef_string_t,
    pub fantasy_font_family: cef_string_t,
    pub default_font_size: c_int,
    pub default_fixed_font_size: c_int,
    pub minimum_font_size: c_int,
    pub minimum_logical_font_size: c_int,
    pub default_encoding: cef_string_t,
    pub remote_fonts: cef_state_t,
    pub javascript: cef_state_t,
    pub javascript_close_windows: cef_state_t,
    pub javascript_access_clipboard: cef_state_t,
    pub javascript_dom_paste: cef_state_t,
    pub image_loading: cef_state_t,
    pub image_shrink_standalone_to_fit: cef_state_t,
    pub text_area_resize: cef_state_t,
    pub tab_to_links: cef_state_t,
    pub local_storage: cef_state_t,
    pub databases_deprecated: cef_state_t,
    pub webgl: cef_state_t,
    pub background_color: cef_color_t,
    pub chrome_status_bubble: cef_state_t,
    pub chrome_zoom_bubble: cef_state_t,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_window_info_t {
    pub size: usize,
    pub window_name: cef_string_t,
    pub bounds: cef_rect_t,
    pub hidden: c_int,
    pub parent_view: cef_window_handle_t,
    pub windowless_rendering_enabled: c_int,
    pub shared_texture_enabled: c_int,
    pub external_begin_frame_enabled: c_int,
    pub view: cef_window_handle_t,
    pub runtime_style: cef_runtime_style_t,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_mouse_event_t {
    pub x: c_int,
    pub y: c_int,
    pub modifiers: cef_event_flags_t,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_touch_event_t {
    pub id: c_int,
    pub x: f32,
    pub y: f32,
    pub radius_x: f32,
    pub radius_y: f32,
    pub rotation_angle: f32,
    pub pressure: f32,
    pub type_: cef_touch_event_type_t,
    pub modifiers: cef_event_flags_t,
    pub pointer_type: cef_pointer_type_t,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_key_event_t {
    pub size: usize,
    pub type_: cef_key_event_type_t,
    pub modifiers: cef_event_flags_t,
    pub windows_key_code: c_int,
    pub native_key_code: c_int,
    pub is_system_key: c_int,
    pub character: u16,
    pub unmodified_character: u16,
    pub focus_on_editable_field: c_int,
}

pub type cef_unused_callback_t = Option<unsafe extern "system" fn()>;

#[repr(C)]
pub struct cef_render_handler_t {
    pub base: cef_base_ref_counted_t,
    pub get_accessibility_handler: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
        ) -> *mut cef_accessibility_handler_t,
    >,
    pub get_root_screen_rect: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            rect: *mut cef_rect_t,
        ) -> c_int,
    >,
    pub get_view_rect: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            rect: *mut cef_rect_t,
        ),
    >,
    pub get_screen_point: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            view_x: c_int,
            view_y: c_int,
            screen_x: *mut c_int,
            screen_y: *mut c_int,
        ) -> c_int,
    >,
    pub get_screen_info: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            screen_info: *mut cef_screen_info_t,
        ) -> c_int,
    >,
    pub on_popup_show: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            show: c_int,
        ),
    >,
    pub on_popup_size: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            rect: *const cef_rect_t,
        ),
    >,
    pub on_paint: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            type_: cef_paint_element_type_t,
            dirty_rects_count: usize,
            dirty_rects: *const cef_rect_t,
            buffer: *const c_void,
            width: c_int,
            height: c_int,
        ),
    >,
    pub on_accelerated_paint: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            type_: cef_paint_element_type_t,
            dirty_rects_count: usize,
            dirty_rects: *const cef_rect_t,
            info: *const c_void,
        ),
    >,
    pub get_touch_handle_size: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            orientation: cef_horizontal_alignment_t,
            size: *mut cef_size_t,
        ),
    >,
    pub on_touch_handle_state_changed: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            state: *const cef_touch_handle_state_t,
        ),
    >,
    pub start_dragging: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            drag_data: *mut cef_drag_data_t,
            allowed_ops: cef_drag_operations_mask_t,
            x: c_int,
            y: c_int,
        ) -> c_int,
    >,
    pub update_drag_cursor: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            operation: cef_drag_operations_mask_t,
        ),
    >,
    pub on_scroll_offset_changed: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            x: f64,
            y: f64,
        ),
    >,
    pub on_ime_composition_range_changed: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            selected_range: *const cef_range_t,
            character_bounds_count: usize,
            character_bounds: *const cef_rect_t,
        ),
    >,
    pub on_text_selection_changed: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            selected_text: *const cef_string_t,
            selected_range: *const cef_range_t,
        ),
    >,
    pub on_virtual_keyboard_requested: Option<
        unsafe extern "system" fn(
            self_: *mut cef_render_handler_t,
            browser: *mut cef_browser_t,
            input_mode: cef_text_input_mode_t,
        ),
    >,
}

#[repr(C)]
pub struct cef_client_t {
    pub base: cef_base_ref_counted_t,
    pub get_audio_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_command_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_context_menu_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_dialog_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_display_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_download_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_drag_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_find_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_focus_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_frame_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_permission_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_jsdialog_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_keyboard_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_life_span_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_load_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_print_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub get_render_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut cef_render_handler_t>,
    pub get_request_handler:
        Option<unsafe extern "system" fn(self_: *mut cef_client_t) -> *mut c_void>,
    pub on_process_message_received: Option<
        unsafe extern "system" fn(
            self_: *mut cef_client_t,
            browser: *mut cef_browser_t,
            frame: *mut cef_frame_t,
            source_process: cef_process_id_t,
            message: *mut cef_process_message_t,
        ) -> c_int,
    >,
}

#[repr(C)]
pub struct cef_browser_process_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_register_custom_preferences: Option<
        unsafe extern "system" fn(
            self_: *mut cef_browser_process_handler_t,
            type_: cef_preferences_type_t,
            registrar: *mut cef_preference_registrar_t,
        ),
    >,
    pub on_context_initialized:
        Option<unsafe extern "system" fn(self_: *mut cef_browser_process_handler_t)>,
    pub on_before_child_process_launch: Option<
        unsafe extern "system" fn(
            self_: *mut cef_browser_process_handler_t,
            command_line: *mut cef_command_line_t,
        ),
    >,
    pub on_already_running_app_relaunch: Option<
        unsafe extern "system" fn(
            self_: *mut cef_browser_process_handler_t,
            command_line: *mut cef_command_line_t,
            current_directory: *const cef_string_t,
        ) -> c_int,
    >,
    pub on_schedule_message_pump_work:
        Option<unsafe extern "system" fn(self_: *mut cef_browser_process_handler_t, delay_ms: i64)>,
    pub get_default_client: Option<
        unsafe extern "system" fn(self_: *mut cef_browser_process_handler_t) -> *mut cef_client_t,
    >,
    pub get_default_request_context_handler: Option<
        unsafe extern "system" fn(
            self_: *mut cef_browser_process_handler_t,
        ) -> *mut cef_request_context_handler_t,
    >,
}

#[repr(C)]
pub struct cef_app_t {
    pub base: cef_base_ref_counted_t,
    pub on_before_command_line_processing: Option<
        unsafe extern "system" fn(
            self_: *mut cef_app_t,
            process_type: *const cef_string_t,
            command_line: *mut cef_command_line_t,
        ),
    >,
    pub on_register_custom_schemes: Option<
        unsafe extern "system" fn(self_: *mut cef_app_t, registrar: *mut cef_scheme_registrar_t),
    >,
    pub get_resource_bundle_handler: Option<
        unsafe extern "system" fn(self_: *mut cef_app_t) -> *mut cef_resource_bundle_handler_t,
    >,
    pub get_browser_process_handler: Option<
        unsafe extern "system" fn(self_: *mut cef_app_t) -> *mut cef_browser_process_handler_t,
    >,
    pub get_render_process_handler: Option<
        unsafe extern "system" fn(self_: *mut cef_app_t) -> *mut cef_render_process_handler_t,
    >,
}

#[repr(C)]
pub struct cef_frame_t {
    pub base: cef_base_ref_counted_t,
    pub is_valid: cef_unused_callback_t,
    pub undo: cef_unused_callback_t,
    pub redo: cef_unused_callback_t,
    pub cut: cef_unused_callback_t,
    pub copy: cef_unused_callback_t,
    pub paste: cef_unused_callback_t,
    pub paste_and_match_style: cef_unused_callback_t,
    pub del: cef_unused_callback_t,
    pub select_all: cef_unused_callback_t,
    pub view_source: cef_unused_callback_t,
    pub get_source: cef_unused_callback_t,
    pub get_text: cef_unused_callback_t,
    pub load_request: cef_unused_callback_t,
    pub load_url:
        Option<unsafe extern "system" fn(self_: *mut cef_frame_t, url: *const cef_string_t)>,
}

#[repr(C)]
pub struct cef_browser_t {
    pub base: cef_base_ref_counted_t,
    pub is_valid: cef_unused_callback_t,
    pub get_host:
        Option<unsafe extern "system" fn(self_: *mut cef_browser_t) -> *mut cef_browser_host_t>,
    pub can_go_back: cef_unused_callback_t,
    pub go_back: cef_unused_callback_t,
    pub can_go_forward: cef_unused_callback_t,
    pub go_forward: cef_unused_callback_t,
    pub is_loading: cef_unused_callback_t,
    pub reload: cef_unused_callback_t,
    pub reload_ignore_cache: cef_unused_callback_t,
    pub stop_load: cef_unused_callback_t,
    pub get_identifier: cef_unused_callback_t,
    pub is_same: cef_unused_callback_t,
    pub is_popup: cef_unused_callback_t,
    pub has_document: cef_unused_callback_t,
    pub get_main_frame:
        Option<unsafe extern "system" fn(self_: *mut cef_browser_t) -> *mut cef_frame_t>,
}

#[repr(C)]
pub struct cef_browser_host_t {
    pub base: cef_base_ref_counted_t,
    pub get_browser: cef_unused_callback_t,
    pub close_browser:
        Option<unsafe extern "system" fn(self_: *mut cef_browser_host_t, force_close: c_int)>,
    pub try_close_browser: cef_unused_callback_t,
    pub is_ready_to_be_closed: cef_unused_callback_t,
    pub set_focus: Option<unsafe extern "system" fn(self_: *mut cef_browser_host_t, focus: c_int)>,
    pub get_window_handle: cef_unused_callback_t,
    pub get_opener_window_handle: cef_unused_callback_t,
    pub get_opener_identifier: cef_unused_callback_t,
    pub has_view: cef_unused_callback_t,
    pub get_client: cef_unused_callback_t,
    pub get_request_context: cef_unused_callback_t,
    pub can_zoom: cef_unused_callback_t,
    pub zoom: cef_unused_callback_t,
    pub get_default_zoom_level: cef_unused_callback_t,
    pub get_zoom_level: cef_unused_callback_t,
    pub set_zoom_level: cef_unused_callback_t,
    pub run_file_dialog: cef_unused_callback_t,
    pub start_download: cef_unused_callback_t,
    pub download_image: cef_unused_callback_t,
    pub print: cef_unused_callback_t,
    pub print_to_pdf: cef_unused_callback_t,
    pub find: cef_unused_callback_t,
    pub stop_finding: cef_unused_callback_t,
    pub show_dev_tools: cef_unused_callback_t,
    pub close_dev_tools: cef_unused_callback_t,
    pub has_dev_tools: cef_unused_callback_t,
    pub send_dev_tools_message: cef_unused_callback_t,
    pub execute_dev_tools_method: cef_unused_callback_t,
    pub add_dev_tools_message_observer: cef_unused_callback_t,
    pub get_navigation_entries: cef_unused_callback_t,
    pub replace_misspelling: cef_unused_callback_t,
    pub add_word_to_dictionary: cef_unused_callback_t,
    pub is_window_rendering_disabled: cef_unused_callback_t,
    pub was_resized: Option<unsafe extern "system" fn(self_: *mut cef_browser_host_t)>,
    pub was_hidden:
        Option<unsafe extern "system" fn(self_: *mut cef_browser_host_t, hidden: c_int)>,
    pub notify_screen_info_changed:
        Option<unsafe extern "system" fn(self_: *mut cef_browser_host_t)>,
    pub invalidate: Option<
        unsafe extern "system" fn(self_: *mut cef_browser_host_t, type_: cef_paint_element_type_t),
    >,
    pub send_external_begin_frame: cef_unused_callback_t,
    pub send_key_event: Option<
        unsafe extern "system" fn(self_: *mut cef_browser_host_t, event: *const cef_key_event_t),
    >,
    pub send_mouse_click_event: Option<
        unsafe extern "system" fn(
            self_: *mut cef_browser_host_t,
            event: *const cef_mouse_event_t,
            type_: cef_mouse_button_type_t,
            mouse_up: c_int,
            click_count: c_int,
        ),
    >,
    pub send_mouse_move_event: Option<
        unsafe extern "system" fn(
            self_: *mut cef_browser_host_t,
            event: *const cef_mouse_event_t,
            mouse_leave: c_int,
        ),
    >,
    pub send_mouse_wheel_event: Option<
        unsafe extern "system" fn(
            self_: *mut cef_browser_host_t,
            event: *const cef_mouse_event_t,
            delta_x: c_int,
            delta_y: c_int,
        ),
    >,
    pub send_touch_event: Option<
        unsafe extern "system" fn(self_: *mut cef_browser_host_t, event: *const cef_touch_event_t),
    >,
    pub send_capture_lost_event: Option<unsafe extern "system" fn(self_: *mut cef_browser_host_t)>,
    pub notify_move_or_resize_started: cef_unused_callback_t,
    pub get_windowless_frame_rate: cef_unused_callback_t,
    pub set_windowless_frame_rate: cef_unused_callback_t,
    pub ime_set_composition: Option<
        unsafe extern "system" fn(
            self_: *mut cef_browser_host_t,
            text: *const cef_string_t,
            underlines_count: usize,
            underlines: *const cef_composition_underline_t,
            replacement_range: *const cef_range_t,
            selection_range: *const cef_range_t,
        ),
    >,
    pub ime_commit_text: Option<
        unsafe extern "system" fn(
            self_: *mut cef_browser_host_t,
            text: *const cef_string_t,
            replacement_range: *const cef_range_t,
            relative_cursor_pos: c_int,
        ),
    >,
    pub ime_finish_composing_text:
        Option<unsafe extern "system" fn(self_: *mut cef_browser_host_t, keep_selection: c_int)>,
    pub ime_cancel_composition: Option<unsafe extern "system" fn(self_: *mut cef_browser_host_t)>,
}
