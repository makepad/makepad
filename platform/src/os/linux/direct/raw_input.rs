use {
    self::super::super::{
        libc_sys,
    },
    self::super::{
        direct_event::*,
    },
    crate::{
        makepad_math::*,
        window::{WindowId},
        event::*,
        area::Area,
    },
    std::{
        cell::Cell,
        fs::File,
        io::Read,
        sync::mpsc, 
    }
};

#[allow(unused,non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum InputEventType {
    EV_SYN = 0x00,
    EV_KEY = 0x01,
    EV_REL = 0x02,
    EV_ABS = 0x03,
    EV_MSC = 0x04,
    EV_SW = 0x05,
    EV_LED = 0x11,
    EV_SND = 0x12,
    EV_REP = 0x14,
    EV_FF = 0x15,
    EV_PWR = 0x16,
    EV_FF_STATUS = 0x17,
    EV_MAX = 0x1f,
    EV_CNT = InputEventType::EV_MAX as u16 + 1,
}

impl Default for InputEventType {
    fn default() -> Self {
        InputEventType::EV_SYN
    }
}

#[allow(unused,non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvAbsCodes {
    ABS_X			    =0x00,
    ABS_Y			    =0x01,
    ABS_Z			    =0x02,
    ABS_RX			    =0x03,
    ABS_RY			    =0x04,
    ABS_RZ			    =0x05,
    ABS_THROTTLE	    =0x06,
    ABS_RUDDER		    =0x07,
    ABS_WHEEL		    =0x08,
    ABS_GAS			    =0x09,
    ABS_BRAKE		    =0x0a,
    ABS_HAT0X		    =0x10,
    ABS_HAT0Y		    =0x11,
    ABS_HAT1X		    =0x12,
    ABS_HAT1Y		    =0x13,
    ABS_HAT2X		    =0x14,
    ABS_HAT2Y		    =0x15,
    ABS_HAT3X		    =0x16,
    ABS_HAT3Y		    =0x17,
    ABS_PRESSURE	    =0x18,
    ABS_DISTANCE	    =0x19,
    ABS_TILT_X		    =0x1a,
    ABS_TILT_Y		    =0x1b,
    ABS_TOOL_WIDTH	    =0x1c,
    ABS_VOLUME		    =0x20,
    ABS_PROFILE		    =0x21,
    ABS_MISC		    =0x28,
    ABS_RESERVED		=0x2e,
    ABS_MT_SLOT		    =0x2f,
    ABS_MT_TOUCH_MAJOR	=0x30,
    ABS_MT_TOUCH_MINOR	=0x31,
    ABS_MT_WIDTH_MAJOR	=0x32,
    ABS_MT_WIDTH_MINOR	=0x33,
    ABS_MT_ORIENTATION	=0x34,
    ABS_MT_POSITION_X	=0x35,
    ABS_MT_POSITION_Y	=0x36,
    ABS_MT_TOOL_TYPE	=0x37,
    ABS_MT_BLOB_ID		=0x38,
    ABS_MT_TRACKING_ID	=0x39,
    ABS_MT_PRESSURE		=0x3a,
    ABS_MT_DISTANCE		=0x3b,
    ABS_MT_TOOL_X		=0x3c,
    ABS_MT_TOOL_Y		=0x3d,
    ABS_MAX			    =0x3f,
    ABS_CNT			    =EvAbsCodes::ABS_MAX as u16 + 1
}

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
struct InputEvent {
    time: libc_sys::timeval,
    ty: InputEventType,
    code: u16,
    value: i32,
}

pub struct RawInput {
    pub modifiers: KeyModifiers,
    receiver: mpsc::Receiver<InputEvent>,
    width: f64,
    height: f64,
    abs: DVec2,
}


impl RawInput {
    pub fn new(width: f64, height: f64) -> Self {
        let (send, receiver) = mpsc::channel();
        for i in 0..12 {
            let device = format!("/dev/input/event{}", i);
            let send = send.clone();
            if let Ok(mut kb) = File::open(&device) {
                std::thread::spawn(move || loop {
                    let mut buf = [0u8; std::mem::size_of::<InputEvent>()];
                    if let Ok(len) = kb.read(&mut buf) {
                        if len == std::mem::size_of::<InputEvent>() {
                            let buf = unsafe {std::mem::transmute(buf)};
                            send.send(buf).unwrap();
                        }
                    }
                    else{
                        return
                    }
                });
            }
        }
        
        Self {
            receiver,
            width,
            height,
            abs: dvec2(0.0, 0.0),
            modifiers: Default::default(),
        }
    }
    
    pub fn poll_raw_input(&mut self, time: f64, window_id: WindowId) -> Vec<DirectEvent> {
        let mut evts = Vec::new();
        let mut mouse_moved = false;
        while let Ok(new) = self.receiver.try_recv() {
            match new.ty {
                InputEventType::EV_REL => { // relative input
                    if new.code == 0{
                        self.abs.x += new.value as f64;
                        if self.abs.x < 0.0{ self.abs.x = 0.0}
                        if self.abs.x > self.width{ self.abs.x = self.width}
                        mouse_moved = true;
                    }
                    else if new.code == 1{
                        self.abs.y += new.value as f64;
                        if self.abs.y < 0.0{ self.abs.y = 0.0}
                        if self.abs.y > self.height{ self.abs.y = self.height}
                        mouse_moved = true;
                    }
                }
                InputEventType::EV_ABS => { // absolute input
                    let code: EvAbsCodes = unsafe { std::mem::transmute(new.code) };
                    match code {
                        EvAbsCodes::ABS_X => {
                            self.abs.x = (new.value as f64 / 32767.0) * self.width;
                            mouse_moved = true;
                        },
                        EvAbsCodes::ABS_Y => {
                            self.abs.y = (new.value as f64 / 32767.0) * self.height;
                            mouse_moved = true;
                        },
                        EvAbsCodes::ABS_MT_POSITION_X => {
                            //TODO relate to dpi factor to correct position
                            self.abs.x = new.value as f64; 
                            mouse_moved = true;
                        },
                        EvAbsCodes::ABS_MT_POSITION_Y => {
                            //TODO relate to dpi factor to correct position
                            self.abs.y = new.value as f64;
                            mouse_moved = true;
                        },
                        _=> ()
                    }
                }
                InputEventType::EV_KEY => { // key press
                    let key_up = new.value == 0;
                    let key_down = new.value == 1;
                    let key_repeat = new.value == 2;
                    let key_code = match new.code {
                        30 => KeyCode::KeyA,
                        48 => KeyCode::KeyB,
                        46 => KeyCode::KeyC,
                        32 => KeyCode::KeyD,
                        18 => KeyCode::KeyE,
                        33 => KeyCode::KeyF,
                        34 => KeyCode::KeyG,
                        35 => KeyCode::KeyH,
                        23 => KeyCode::KeyI,
                        36 => KeyCode::KeyJ,
                        37 => KeyCode::KeyK,
                        38 => KeyCode::KeyL,
                        50 => KeyCode::KeyM,
                        49 => KeyCode::KeyN,
                        24 => KeyCode::KeyO,
                        25 => KeyCode::KeyP,
                        16 => KeyCode::KeyQ,
                        19 => KeyCode::KeyR,
                        31 => KeyCode::KeyS,
                        20 => KeyCode::KeyT,
                        22 => KeyCode::KeyU,
                        47 => KeyCode::KeyV,
                        17 => KeyCode::KeyW,
                        45 => KeyCode::KeyX,
                        21 => KeyCode::KeyY,
                        44 => KeyCode::KeyZ,
                        11 => KeyCode::Key0,
                        2 => KeyCode::Key1,
                        3 => KeyCode::Key2,
                        4 => KeyCode::Key3,
                        5 => KeyCode::Key4,
                        6 => KeyCode::Key5,
                        7 => KeyCode::Key6,
                        8 => KeyCode::Key7,
                        9 => KeyCode::Key8,
                        10 => KeyCode::Key9,
                        56 => KeyCode::Alt,
                        100 => KeyCode::Alt,
                        125 => KeyCode::Logo,
                        126 => KeyCode::Logo,
                        42 => KeyCode::Shift,
                        54 => KeyCode::Shift,
                        29 => KeyCode::Control,
                        97 => KeyCode::Control,
                        13 => KeyCode::Equals,
                        12 => KeyCode::Minus,
                        27 => KeyCode::RBracket,
                        26 => KeyCode::LBracket,
                        28 => KeyCode::ReturnKey,
                        86 => KeyCode::Backtick,
                        39 => KeyCode::Semicolon,
                        43 => KeyCode::Backslash,
                        51 => KeyCode::Comma,
                        53 => KeyCode::Slash,
                        52 => KeyCode::Period,
                        15 => KeyCode::Tab,
                        57 => KeyCode::Space,
                        14 => KeyCode::Backspace,
                        1 => KeyCode::Escape,
                        58 => KeyCode::Capslock,
                        83 => KeyCode::NumpadDecimal,
                        55 => KeyCode::NumpadMultiply,
                        78 => KeyCode::NumpadAdd,
                        69 => KeyCode::Numlock,
                        89 => KeyCode::NumpadDivide,
                        96 => KeyCode::NumpadEnter,
                        74 => KeyCode::NumpadSubtract,
                        //0 => KeyCode::NumpadEquals,
                        82 => KeyCode::Numpad0,
                        79 => KeyCode::Numpad1,
                        80 => KeyCode::Numpad2,
                        81 => KeyCode::Numpad3,
                        75 => KeyCode::Numpad4,
                        76 => KeyCode::Numpad5,
                        77 => KeyCode::Numpad6,
                        71 => KeyCode::Numpad7,
                        72 => KeyCode::Numpad8,
                        73 => KeyCode::Numpad9,
                        59 => KeyCode::F1,
                        60 => KeyCode::F2,
                        61 => KeyCode::F3,
                        62 => KeyCode::F4,
                        63 => KeyCode::F5,
                        64 => KeyCode::F6,
                        65 => KeyCode::F7,
                        66 => KeyCode::F8,
                        67 => KeyCode::F9,
                        68 => KeyCode::F10,
                        87 => KeyCode::F11,
                        88 => KeyCode::F12,
                        99 => KeyCode::PrintScreen,
                        102 => KeyCode::Home,
                        104 => KeyCode::PageUp,
                        111 => KeyCode::Delete,
                        107 => KeyCode::End,
                        109 => KeyCode::PageDown,
                        105 => KeyCode::ArrowLeft,
                        106 => KeyCode::ArrowRight,
                        108 => KeyCode::ArrowDown,
                        103 => KeyCode::ArrowUp,
                        _ => KeyCode::Unknown,
                    };
                    match key_code {
                        KeyCode::Shift => self.modifiers.shift = key_down,
                        KeyCode::Control => self.modifiers.control = key_down,
                        KeyCode::Logo => self.modifiers.logo = key_down,
                        KeyCode::Alt => self.modifiers.alt = key_down,
                        _ => ()
                    };
                    if key_down && !self.modifiers.control && !self.modifiers.alt && !self.modifiers.logo {
                        let uc = self.modifiers.shift;
                        let inp = key_code.to_char(uc);
                        if let Some(inp) = inp {
                            evts.push(DirectEvent::TextInput(TextInputEvent {
                                input: format!("{}", inp),
                                was_paste: false,
                                replace_last: false
                            }));
                        }
                    }
                    if new.code == 272 || new.code == 273 || new.code == 274 { // mouse
                        if mouse_moved {
                            mouse_moved = false;
                            evts.push(DirectEvent::MouseMove(MouseMoveEvent {
                                abs: self.abs,
                                window_id,
                                modifiers: self.modifiers,
                                time,
                                handled: Cell::new(Area::Empty),
                            }));
                        }

                        if key_down{
                            evts.push(DirectEvent::MouseDown(MouseDownEvent {
                                button: (new.code - 272) as usize,
                                abs: self.abs,
                                window_id,
                                modifiers: self.modifiers,
                                time,
                                handled: Cell::new(Area::Empty),
                            }));
                        }
                        else if key_up{
                            evts.push(DirectEvent::MouseUp(MouseUpEvent {
                                button: (new.code - 272) as usize,
                                abs: self.abs,
                                window_id,
                                modifiers: self.modifiers,
                                time,
                            }));
                        }
                    }
                    else {
                        if key_down || key_repeat{
                            evts.push(DirectEvent::KeyDown(KeyEvent {
                                key_code,
                                is_repeat: key_repeat,
                                modifiers: self.modifiers,
                                time
                            }));
                        }
                        else if key_up{
                            evts.push(DirectEvent::KeyUp(KeyEvent {
                                key_code,
                                is_repeat: false,
                                modifiers: self.modifiers,
                                time
                            }));
                        }
                    }
                }
                _ => ()
            }
        }
        if mouse_moved {
            evts.push(DirectEvent::MouseMove(MouseMoveEvent {
                abs: self.abs,
                window_id,
                modifiers: self.modifiers,
                time,
                handled: Cell::new(Area::Empty),
            }));
        }
        
        evts
    }
}
