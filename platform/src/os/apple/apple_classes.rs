#[allow(unused_imports)]
use {
    std::{
        rc::Rc,
        cell::{RefCell},
        time::Instant,
        collections::{HashMap},
        os::raw::{c_void},
    },
    crate::{
        makepad_objc_sys::runtime::{ObjcId, nil},
        makepad_math::{
            DVec2,
        },
        os::{
            apple::apple_sys::*,
            ns_url_session::define_web_socket_delegate,
            av_capture::define_av_video_callback_delegate,
            audio_unit::define_key_value_observing_delegate,
            apple_util::{
                nsstring_to_string,
                str_to_nsstring,
                keycode_to_menu_key,
                get_event_keycode,
                get_event_key_modifier
            },
            cx_native::EventFlow,
        },
        menu::{
            CxCommandSetting
        },
        //turtle::{
        //    Rect
        //},
        event::{
            KeyCode,
            KeyEvent,
            TextInputEvent,
            TextClipboardEvent,
            TimerEvent,
            DragItem,
            KeyModifiers,
        },
        cursor::MouseCursor,
        menu::{
            Menu,
            MenuCommand
        },
    }
};

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
    unsafe {
        &*(APPLE_CLASSES)
    }
}

pub struct AppleClasses {
    pub key_value_observing_delegate: *const Class,
    pub video_callback_delegate: *const Class,
    pub web_socket_delegate: *const Class,
    pub const_attributes_for_marked_text: ObjcId,
    pub const_empty_string: RcObjcId,
}

impl AppleClasses {
    pub fn new() -> Self {
        let const_attributes = vec![
            RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSMarkedClauseSegment")).unwrap()).forget(),
            RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSGlyphInfo")).unwrap()).forget(),
        ];
        Self {
            web_socket_delegate: define_web_socket_delegate(),
            video_callback_delegate: define_av_video_callback_delegate(),
            key_value_observing_delegate: define_key_value_observing_delegate(),
            const_attributes_for_marked_text: unsafe {msg_send![
                class!(NSArray),
                arrayWithObjects: const_attributes.as_ptr()
                count: const_attributes.len()
            ]},
            const_empty_string: RcObjcId::from_unowned(NonNull::new(str_to_nsstring("")).unwrap()),
        }
    }
}

