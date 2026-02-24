#[allow(unused_imports)]
use {
    crate::{
        cursor::MouseCursor,
        //menu::{
        //    CxCommandSetting
        //},
        //turtle::{
        //    Rect
        //},
        event::{
            DragItem, KeyCode, KeyEvent, KeyModifiers, TextClipboardEvent, TextInputEvent,
            TimerEvent,
        },
        macos_menu::MacosMenu,
        makepad_math::Vec2d,
        makepad_objc_sys::runtime::{nil, ObjcId},
        os::{
            apple::apple_sys::*,
            apple_util::{
                get_event_key_modifier, get_event_keycode, keycode_to_menu_key, nsstring_to_string,
                str_to_nsstring,
            },
            audio_unit::define_key_value_observing_delegate,
            av_capture::define_av_video_callback_delegate,
            cx_native::EventFlow,
        },
        makepad_network::backend::apple::{
            http::{define_url_session_data_delegate, define_url_session_delegate},
            web_socket::define_web_socket_delegate,
        },
    },
    std::{cell::RefCell, collections::HashMap, os::raw::c_void, rc::Rc, time::Instant},
};

#[cfg(target_os = "macos")]
use crate::os::audio_tap::define_sc_stream_output_delegate;

// this is unsafe, however we don't have much choice since the system calls into
// the objective C entrypoints we need to enter our eventloop
// So wherever we put this boundary, it will be unsafe

// this value will be fetched from multiple threads (post signal uses it)
pub static mut APPLE_CLASSES: *const AppleClasses = 0 as *const _;

pub fn init_apple_classes_global() {
    unsafe {
        APPLE_CLASSES = Box::into_raw(Box::new(AppleClasses::new()));
    }
}

pub fn get_apple_class_global() -> &'static AppleClasses {
    unsafe { &*(APPLE_CLASSES) }
}

pub struct AppleClasses {
    pub key_value_observing_delegate: *const Class,
    pub video_callback_delegate: *const Class,
    pub web_socket_delegate: *const Class,
    pub url_session_delegate: *const Class,
    pub url_session_data_delegate: *const Class,
    #[cfg(target_os = "macos")]
    pub sc_stream_output_delegate: *const Class,
    pub const_attributes_for_marked_text: ObjcId,
    pub const_empty_string: RcObjcId,
}

impl AppleClasses {
    pub fn new() -> Self {
        let const_attributes = vec![
            RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSMarkedClauseSegment")).unwrap())
                .forget(),
            RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSGlyphInfo")).unwrap()).forget(),
        ];
        Self {
            web_socket_delegate: define_web_socket_delegate(),
            url_session_delegate: define_url_session_delegate(),
            url_session_data_delegate: define_url_session_data_delegate(),
            video_callback_delegate: define_av_video_callback_delegate(),
            key_value_observing_delegate: define_key_value_observing_delegate(),
            #[cfg(target_os = "macos")]
            sc_stream_output_delegate: define_sc_stream_output_delegate(),
            const_attributes_for_marked_text: unsafe {
                msg_send![
                    class!(NSArray),
                    arrayWithObjects: const_attributes.as_ptr()
                    count: const_attributes.len()
                ]
            },
            const_empty_string: RcObjcId::from_unowned(NonNull::new(str_to_nsstring("")).unwrap()),
        }
    }
}
