use {
    crate::{
        makepad_objc_sys::runtime::{ObjcId,nil},
        os::{
            apple::apple_sys::*,
        },
        event::{
            KeyCode,
            KeyModifiers
        },
        cursor::MouseCursor
    }
};


pub const fn four_char_as_u32(s: &str) -> u32 {
    let b = s.as_bytes();
    ((b[0] as u32) << 24)
        | ((b[1] as u32) << 16)
        | ((b[2] as u32) << 8)
        | ((b[3] as u32))
}

pub fn nsstring_to_string(string: ObjcId) -> String {
    unsafe {
        let utf8_string: *const std::os::raw::c_uchar = msg_send![string, UTF8String];
        let utf8_len: usize = msg_send![string, lengthOfBytesUsingEncoding: UTF8_ENCODING];
        let slice = std::slice::from_raw_parts(
            utf8_string,
            utf8_len,
        );
        std::str::from_utf8_unchecked(slice).to_owned()
    }
}

pub fn str_to_nsstring(str: &str) -> ObjcId {
    unsafe {
        let ns_string: ObjcId = msg_send![class!(NSString), alloc];
        let ns_string: ObjcId = msg_send![
            ns_string,
            initWithBytes: str.as_ptr()
            length: str.len()
            encoding: UTF8_ENCODING as ObjcId
        ];
        let _: () = msg_send![ns_string, autorelease];
        ns_string
    }
}

pub fn load_native_cursor(cursor_name: &str) -> ObjcId {
    let sel = Sel::register(cursor_name);
    let id: ObjcId = unsafe {msg_send![class!(NSCursor), performSelector: sel]};
    id
}

pub fn load_undocumented_cursor(cursor_name: &str) -> ObjcId {
    unsafe {
        let class = class!(NSCursor);
        let sel = Sel::register(cursor_name);
        let sel: ObjcId = msg_send![class, respondsToSelector: sel];
        let id: ObjcId = msg_send![class, performSelector: sel];
        id
    }
}

pub unsafe fn ccfstr_from_str(inp: &str) -> CFStringRef {
    let null = format!("{}\0", inp);
    __CFStringMakeConstantString(null.as_ptr() as *const ::std::os::raw::c_char)
}

pub unsafe fn cfstring_ref_to_string(cfstring: CFStringRef) -> String {
    let length = CFStringGetLength(cfstring);
    let range = CFRange {location: 0, length};
    let mut num_bytes = 0u64;
    let converted = CFStringGetBytes(cfstring, range, kCFStringEncodingUTF8, 0, false, 0 as *mut u8, 0, &mut num_bytes);
    if converted == 0 || num_bytes == 0 {return String::new()}
    let mut buffer = Vec::new();
    buffer.resize(num_bytes as usize, 0u8);
    CFStringGetBytes(cfstring, range, kCFStringEncodingUTF8, 0, false, buffer.as_mut_ptr() as *mut u8, num_bytes, 0 as *mut u64);
    if let Ok(val) = String::from_utf8(buffer) {
        val
    }
    else {
        String::new()
    }
}


pub fn load_webkit_cursor(cursor_name_str: &str) -> ObjcId {
    unsafe {
        static CURSOR_ROOT: &'static str = "/System/Library/Frameworks/ApplicationServices.framework/Versions/A/Frameworks/HIServices.framework/Versions/A/Resources/cursors";
        let cursor_root = str_to_nsstring(CURSOR_ROOT);
        let cursor_name = str_to_nsstring(cursor_name_str);
        let cursor_pdf = str_to_nsstring("cursor.pdf");
        let cursor_plist = str_to_nsstring("info.plist");
        let key_x = str_to_nsstring("hotx");
        let key_y = str_to_nsstring("hoty");
        
        let cursor_path: ObjcId = msg_send![cursor_root, stringByAppendingPathComponent: cursor_name];
        let pdf_path: ObjcId = msg_send![cursor_path, stringByAppendingPathComponent: cursor_pdf];
        let info_path: ObjcId = msg_send![cursor_path, stringByAppendingPathComponent: cursor_plist];
        
        let ns_image: ObjcId = msg_send![class!(NSImage), alloc];
        let () = msg_send![ns_image, initByReferencingFile: pdf_path];
        let info: ObjcId = msg_send![class!(NSDictionary), dictionaryWithContentsOfFile: info_path];
        //let image = NSImage::alloc(nil).initByReferencingFile_(pdf_path);
        // let info = NSDictionary::dictionaryWithContentsOfFile_(nil, info_path);
        
        let x: ObjcId = msg_send![info, valueForKey: key_x]; //info.valueForKey_(key_x);
        let y: ObjcId = msg_send![info, valueForKey: key_y]; //info.valueForKey_(key_y);
        let point = NSPoint {
            x: msg_send![x, doubleValue],
            y: msg_send![y, doubleValue],
        };
        let cursor: ObjcId = msg_send![class!(NSCursor), alloc];
        msg_send![cursor, initWithImage: ns_image hotSpot: point]
    }
}


pub fn get_event_char(event: ObjcId) -> char {
    unsafe {
        let characters: ObjcId = msg_send![event, characters];
        if characters == nil {
            return '\0'
        }
        let chars = nsstring_to_string(characters);
        
        if chars.len() == 0 {
            return '\0'
        }
        chars.chars().next().unwrap()
    }
}

pub fn get_event_key_modifier(event: ObjcId) -> KeyModifiers {
    let flags: u64 = unsafe {msg_send![event, modifierFlags]};
    KeyModifiers {
        shift: flags & NSEventModifierFlags::NSShiftKeyMask as u64 != 0,
        control: flags & NSEventModifierFlags::NSControlKeyMask as u64 != 0,
        alt: flags & NSEventModifierFlags::NSAlternateKeyMask as u64 != 0,
        logo: flags & NSEventModifierFlags::NSCommandKeyMask as u64 != 0,
    }
}

pub fn get_event_keycode(event: ObjcId) -> Option<KeyCode> {
    let scan_code: std::os::raw::c_ushort = unsafe {
        msg_send![event, keyCode]
    };
    
    Some(match scan_code {
        0x00 => KeyCode::KeyA,
        0x01 => KeyCode::KeyS,
        0x02 => KeyCode::KeyD,
        0x03 => KeyCode::KeyF,
        0x04 => KeyCode::KeyH,
        0x05 => KeyCode::KeyG,
        0x06 => KeyCode::KeyZ,
        0x07 => KeyCode::KeyX,
        0x08 => KeyCode::KeyC,
        0x09 => KeyCode::KeyV,
        //0x0a => World 1,
        0x0b => KeyCode::KeyB,
        0x0c => KeyCode::KeyQ,
        0x0d => KeyCode::KeyW,
        0x0e => KeyCode::KeyE,
        0x0f => KeyCode::KeyR,
        0x10 => KeyCode::KeyY,
        0x11 => KeyCode::KeyT,
        0x12 => KeyCode::Key1,
        0x13 => KeyCode::Key2,
        0x14 => KeyCode::Key3,
        0x15 => KeyCode::Key4,
        0x16 => KeyCode::Key6,
        0x17 => KeyCode::Key5,
        0x18 => KeyCode::Equals,
        0x19 => KeyCode::Key9,
        0x1a => KeyCode::Key7,
        0x1b => KeyCode::Minus,
        0x1c => KeyCode::Key8,
        0x1d => KeyCode::Key0,
        0x1e => KeyCode::RBracket,
        0x1f => KeyCode::KeyO,
        0x20 => KeyCode::KeyU,
        0x21 => KeyCode::LBracket,
        0x22 => KeyCode::KeyI,
        0x23 => KeyCode::KeyP,
        0x24 => KeyCode::ReturnKey,
        0x25 => KeyCode::KeyL,
        0x26 => KeyCode::KeyJ,
        0x27 => KeyCode::Quote,
        0x28 => KeyCode::KeyK,
        0x29 => KeyCode::Semicolon,
        0x2a => KeyCode::Backslash,
        0x2b => KeyCode::Comma,
        0x2c => KeyCode::Slash,
        0x2d => KeyCode::KeyN,
        0x2e => KeyCode::KeyM,
        0x2f => KeyCode::Period,
        0x30 => KeyCode::Tab,
        0x31 => KeyCode::Space,
        0x32 => KeyCode::Backtick,
        0x33 => KeyCode::Backspace,
        //0x34 => unkown,
        0x35 => KeyCode::Escape,
        //0x36 => KeyCode::RLogo,
        //0x37 => KeyCode::LLogo,
        //0x38 => KeyCode::LShift,
        0x39 => KeyCode::Capslock,
        //0x3a => KeyCode::LAlt,
        //0x3b => KeyCode::LControl,
        //0x3c => KeyCode::RShift,
        //0x3d => KeyCode::RAlt,
        //0x3e => KeyCode::RControl,
        //0x3f => Fn key,
        //0x40 => KeyCode::F17,
        0x41 => KeyCode::NumpadDecimal,
        //0x42 -> unkown,
        0x43 => KeyCode::NumpadMultiply,
        //0x44 => unkown,
        0x45 => KeyCode::NumpadAdd,
        //0x46 => unkown,
        0x47 => KeyCode::Numlock,
        //0x48 => KeypadClear,
        //0x49 => KeyCode::VolumeUp,
        //0x4a => KeyCode::VolumeDown,
        0x4b => KeyCode::NumpadDivide,
        0x4c => KeyCode::NumpadEnter,
        0x4e => KeyCode::NumpadSubtract,
        //0x4d => unkown,
        //0x4e => KeyCode::Subtract,
        //0x4f => KeyCode::F18,
        //0x50 => KeyCode::F19,
        0x51 => KeyCode::NumpadEquals,
        0x52 => KeyCode::Numpad0,
        0x53 => KeyCode::Numpad1,
        0x54 => KeyCode::Numpad2,
        0x55 => KeyCode::Numpad3,
        0x56 => KeyCode::Numpad4,
        0x57 => KeyCode::Numpad5,
        0x58 => KeyCode::Numpad6,
        0x59 => KeyCode::Numpad7,
        //0x5a => KeyCode::F20,
        0x5b => KeyCode::Numpad8,
        0x5c => KeyCode::Numpad9,
        //0x5d => KeyCode::Yen,
        //0x5e => JIS Ro,
        //0x5f => unkown,
        0x60 => KeyCode::F5,
        0x61 => KeyCode::F6,
        0x62 => KeyCode::F7,
        0x63 => KeyCode::F3,
        0x64 => KeyCode::F8,
        0x65 => KeyCode::F9,
        //0x66 => JIS Eisuu (macOS),
        0x67 => KeyCode::F11,
        //0x68 => JIS Kana (macOS),
        0x69 => KeyCode::PrintScreen,
        //0x6a => KeyCode::F16,
        //0x6b => KeyCode::F14,
        //0x6c => unkown,
        0x6d => KeyCode::F10,
        //0x6e => unkown,
        0x6f => KeyCode::F12,
        //0x70 => unkown,
        //0x71 => KeyCode::F15,
        0x72 => KeyCode::Insert,
        0x73 => KeyCode::Home,
        0x74 => KeyCode::PageUp,
        0x75 => KeyCode::Delete,
        0x76 => KeyCode::F4,
        0x77 => KeyCode::End,
        0x78 => KeyCode::F2,
        0x79 => KeyCode::PageDown,
        0x7a => KeyCode::F1,
        0x7b => KeyCode::ArrowLeft,
        0x7c => KeyCode::ArrowRight,
        0x7d => KeyCode::ArrowDown,
        0x7e => KeyCode::ArrowUp,
        //0x7f =>  unkown,
        //0xa => KeyCode::Caret,
        _ => return None,
    })
}

pub fn keycode_to_menu_key(keycode: KeyCode, shift: bool) -> &'static str {
    if !shift {
        match keycode {
            KeyCode::Backtick => "`",
            KeyCode::Key0 => "0",
            KeyCode::Key1 => "1",
            KeyCode::Key2 => "2",
            KeyCode::Key3 => "3",
            KeyCode::Key4 => "4",
            KeyCode::Key5 => "5",
            KeyCode::Key6 => "6",
            KeyCode::Key7 => "7",
            KeyCode::Key8 => "8",
            KeyCode::Key9 => "9",
            KeyCode::Minus => "-",
            KeyCode::Equals => "=",
            
            KeyCode::KeyQ => "q",
            KeyCode::KeyW => "w",
            KeyCode::KeyE => "e",
            KeyCode::KeyR => "r",
            KeyCode::KeyT => "t",
            KeyCode::KeyY => "y",
            KeyCode::KeyU => "u",
            KeyCode::KeyI => "i",
            KeyCode::KeyO => "o",
            KeyCode::KeyP => "p",
            KeyCode::LBracket => "[",
            KeyCode::RBracket => "]",
            
            KeyCode::KeyA => "a",
            KeyCode::KeyS => "s",
            KeyCode::KeyD => "d",
            KeyCode::KeyF => "f",
            KeyCode::KeyG => "g",
            KeyCode::KeyH => "h",
            KeyCode::KeyJ => "j",
            KeyCode::KeyK => "k",
            KeyCode::KeyL => "l",
            KeyCode::Semicolon => ";",
            KeyCode::Quote => "'",
            KeyCode::Backslash => "\\",
            
            KeyCode::KeyZ => "z",
            KeyCode::KeyX => "x",
            KeyCode::KeyC => "c",
            KeyCode::KeyV => "v",
            KeyCode::KeyB => "b",
            KeyCode::KeyN => "n",
            KeyCode::KeyM => "m",
            KeyCode::Comma => ",",
            KeyCode::Period => ".",
            KeyCode::Slash => "/",
            _ => ""
        }
    }
    else {
        match keycode {
            KeyCode::Backtick => "~",
            KeyCode::Key0 => "!",
            KeyCode::Key1 => "@",
            KeyCode::Key2 => "#",
            KeyCode::Key3 => "$",
            KeyCode::Key4 => "%",
            KeyCode::Key5 => "^",
            KeyCode::Key6 => "&",
            KeyCode::Key7 => "*",
            KeyCode::Key8 => "(",
            KeyCode::Key9 => ")",
            KeyCode::Minus => "_",
            KeyCode::Equals => "+",
            
            KeyCode::KeyQ => "Q",
            KeyCode::KeyW => "W",
            KeyCode::KeyE => "E",
            KeyCode::KeyR => "R",
            KeyCode::KeyT => "T",
            KeyCode::KeyY => "Y",
            KeyCode::KeyU => "U",
            KeyCode::KeyI => "I",
            KeyCode::KeyO => "O",
            KeyCode::KeyP => "P",
            KeyCode::LBracket => "{",
            KeyCode::RBracket => "}",
            
            KeyCode::KeyA => "A",
            KeyCode::KeyS => "S",
            KeyCode::KeyD => "D",
            KeyCode::KeyF => "F",
            KeyCode::KeyG => "G",
            KeyCode::KeyH => "H",
            KeyCode::KeyJ => "J",
            KeyCode::KeyK => "K",
            KeyCode::KeyL => "L",
            KeyCode::Semicolon => ":",
            KeyCode::Quote => "\"",
            KeyCode::Backslash => "|",
            
            KeyCode::KeyZ => "Z",
            KeyCode::KeyX => "X",
            KeyCode::KeyC => "C",
            KeyCode::KeyV => "V",
            KeyCode::KeyB => "B",
            KeyCode::KeyN => "N",
            KeyCode::KeyM => "M",
            KeyCode::Comma => "<",
            KeyCode::Period => ">",
            KeyCode::Slash => "?",
            _ => ""
        }
    }
}

pub unsafe fn superclass<'a>(this: &'a Object) -> &'a Class {
    let superclass: ObjcId = msg_send![this, superclass];
    &*(superclass as *const _)
}

pub fn bottom_left_to_top_left(rect: NSRect) -> f64 {
    let height = unsafe {CGDisplayPixelsHigh(CGMainDisplayID())};
    height as f64 - (rect.origin.y + rect.size.height)
}

pub fn load_mouse_cursor(cursor: MouseCursor) -> ObjcId {
    match cursor {
        MouseCursor::Arrow | MouseCursor::Default | MouseCursor::Hidden => load_native_cursor("arrowCursor"),
        MouseCursor::Hand => load_native_cursor("pointingHandCursor"),
        MouseCursor::Text => load_native_cursor("IBeamCursor"),
        MouseCursor::NotAllowed /*| MouseCursor::NoDrop*/ => load_native_cursor("operationNotAllowedCursor"),
        MouseCursor::Crosshair => load_native_cursor("crosshairCursor"),
        /*
        MouseCursor::Grabbing | MouseCursor::Grab => load_native_cursor("closedHandCursor"),
        MouseCursor::VerticalText => load_native_cursor("IBeamCursorForVerticalLayout"),
        MouseCursor::Copy => load_native_cursor("dragCopyCursor"),
        MouseCursor::Alias => load_native_cursor("dragLinkCursor"),
        MouseCursor::ContextMenu => load_native_cursor("contextualMenuCursor"),
        */
        MouseCursor::EResize => load_native_cursor("resizeRightCursor"),
        MouseCursor::NResize => load_native_cursor("resizeUpCursor"),
        MouseCursor::WResize => load_native_cursor("resizeLeftCursor"),
        MouseCursor::SResize => load_native_cursor("resizeDownCursor"),
        MouseCursor::NeResize => load_undocumented_cursor("_windowResizeNorthEastCursor"),
        MouseCursor::NwResize => load_undocumented_cursor("_windowResizeNorthWestCursor"),
        MouseCursor::SeResize => load_undocumented_cursor("_windowResizeSouthEastCursor"),
        MouseCursor::SwResize => load_undocumented_cursor("_windowResizeSouthWestCursor"),
        
        MouseCursor::EwResize | MouseCursor::ColResize => load_native_cursor("resizeLeftRightCursor"),
        MouseCursor::NsResize | MouseCursor::RowResize => load_native_cursor("resizeUpDownCursor"),
        
        // Undocumented cursors: https://stackoverflow.com/a/46635398/5435443
        MouseCursor::Help => load_undocumented_cursor("_helpCursor"),
        //MouseCursor::ZoomIn => load_undocumented_cursor("_zoomInCursor"),
        //MouseCursor::ZoomOut => load_undocumented_cursor("_zoomOutCursor"),
        
        MouseCursor::NeswResize => load_undocumented_cursor("_windowResizeNorthEastSouthWestCursor"),
        MouseCursor::NwseResize => load_undocumented_cursor("_windowResizeNorthWestSouthEastCursor"),
        
        // While these are available, the former just loads a white arrow,
        // and the latter loads an ugly deflated beachball!
        // MouseCursor::Move => Cursor::Undocumented("_moveCursor"),
        // MouseCursor::Wait => Cursor::Undocumented("_waitCursor"),
        // An even more undocumented cursor...
        // https://bugs.eclipse.org/bugs/show_bug.cgi?id=522349
        // This is the wrong semantics for `Wait`, but it's the same as
        // what's used in Safari and Chrome.
        MouseCursor::Wait/* | MouseCursor::Progress*/ => load_undocumented_cursor("busyButClickableCursor"),
        
        // For the rest, we can just snatch the cursors from WebKit...
        // They fit the style of the native cursors, and will seem
        // completely standard to macOS users.
        // https://stackoverflow.com/a/21786835/5435443
        MouseCursor::Move /*| MouseCursor::AllScroll*/ => load_webkit_cursor("move"),
        // MouseCursor::Cell => load_webkit_cursor("cell"),
    }
}


#[derive(Debug)]
#[repr(i32)]
pub enum OSError {
    
    Unimplemented = -4,
    FileNotFound = -43,
    FilePermission = -54,
    TooManyFilesOpen = -42,
    
    Unspecified = -1500,
    SystemSoundClientMessageTimeout = -1501,
    
    BadFilePath = 561017960,
    Param = -50,
    MemFull = -108,
    
    FormatUnspecified = 2003329396,
    UnknownProperty = 2003332927,
    BadPropertySize = 561211770,
    IllegalOperation = 1852797029,
    UnsupportedFormat = 560226676,
    State = 561214580,
    NotEnoughBufferSpace = 560100710,
    
    UnsupportedDataFormat = 1718449215,
    
    InvalidProperty = -10879,
    InvalidParameter = -10878,
    InvalidElement = -10877,
    NoConnection = -10876,
    FailedInitialization = -10875,
    TooManyFramesToProcess = -10874,
    InvalidFile = -10871,
    FormatNotSupported = -10868,
    Uninitialized = -10867,
    InvalidScope = -10866,
    PropertyNotWritable = -10865,
    CannotDoInCurrentContext = -10863,
    InvalidPropertyValue = -10851,
    PropertyNotInUse = -10850,
    Initialized = -10849,
    InvalidOfflineRender = -10848,
    Unauthorized = -10847,
    
    NoMatchingDefaultAudioUnitFound,
    
    Unknown,
}

impl OSError {
    pub fn from(result: i32) -> Result<(), Self> {
        Err(match result {
            0 => return Ok(()),
            x if x == Self::Unimplemented as i32 => Self::Unimplemented,
            x if x == Self::FileNotFound as i32 => Self::FileNotFound,
            x if x == Self::FilePermission as i32 => Self::FilePermission,
            x if x == Self::TooManyFilesOpen as i32 => Self::TooManyFilesOpen,
            
            x if x == Self::Unspecified as i32 => Self::Unspecified,
            x if x == Self::SystemSoundClientMessageTimeout as i32 => Self::SystemSoundClientMessageTimeout,
            
            x if x == Self::BadFilePath as i32 => Self::BadFilePath,
            x if x == Self::Param as i32 => Self::Param,
            x if x == Self::MemFull as i32 => Self::MemFull,
            
            x if x == Self::FormatUnspecified as i32 => Self::FormatUnspecified,
            x if x == Self::UnknownProperty as i32 => Self::UnknownProperty,
            x if x == Self::BadPropertySize as i32 => Self::BadPropertySize,
            x if x == Self::IllegalOperation as i32 => Self::IllegalOperation,
            x if x == Self::UnsupportedFormat as i32 => Self::UnsupportedFormat,
            x if x == Self::State as i32 => Self::State,
            x if x == Self::NotEnoughBufferSpace as i32 => Self::NotEnoughBufferSpace,
            
            x if x == Self::UnsupportedDataFormat as i32 => Self::UnsupportedDataFormat,
            
            x if x == Self::InvalidProperty as i32 => Self::InvalidProperty,
            x if x == Self::InvalidParameter as i32 => Self::InvalidParameter,
            x if x == Self::InvalidElement as i32 => Self::InvalidElement,
            x if x == Self::NoConnection as i32 => Self::NoConnection,
            x if x == Self::FailedInitialization as i32 => Self::FailedInitialization,
            x if x == Self::TooManyFramesToProcess as i32 => Self::TooManyFramesToProcess,
            x if x == Self::InvalidFile as i32 => Self::InvalidFile,
            x if x == Self::FormatNotSupported as i32 => Self::FormatNotSupported,
            x if x == Self::Uninitialized as i32 => Self::Uninitialized,
            x if x == Self::InvalidScope as i32 => Self::InvalidScope,
            x if x == Self::PropertyNotWritable as i32 => Self::PropertyNotWritable,
            x if x == Self::CannotDoInCurrentContext as i32 => Self::CannotDoInCurrentContext,
            x if x == Self::InvalidPropertyValue as i32 => Self::InvalidPropertyValue,
            x if x == Self::PropertyNotInUse as i32 => Self::PropertyNotInUse,
            x if x == Self::Initialized as i32 => Self::Initialized,
            x if x == Self::InvalidOfflineRender as i32 => Self::InvalidOfflineRender,
            x if x == Self::Unauthorized as i32 => Self::Unauthorized,
            _ => Self::Unknown
        })
    }
    
    pub fn from_nserror(ns_error: ObjcId) -> Result<(), Self> {
        if ns_error != nil {
            let code: i32 = unsafe {msg_send![ns_error, code]};
            Self::from(code)
        }
        else {
            Ok(())
        }
    }
}

