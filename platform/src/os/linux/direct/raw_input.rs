use {
    self::super::super::libc_sys,
    self::super::direct_event::*,
    crate::{area::Area, event::*, makepad_math::*, window::WindowId},
    std::{
        cell::Cell,
        collections::HashSet,
        fs::{File, OpenOptions},
        io::{self, Read},
        os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
        path::PathBuf,
    },
};

// Kernel event codes are extensible integers, not Rust enum discriminants.
// Unknown codes must be ignored without constructing an invalid enum value.
macro_rules! input_codes {
    ($name:ident, $ty:ty, {$($key:ident = $value:expr,)*}) => {
        #[allow(dead_code, non_snake_case)]
        mod $name {
            $(pub const $key: $ty = $value;)*
        }
    };
}

input_codes! { InputEventType, u16, {
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
    EV_CNT = EV_MAX as u16 + 1,
}}


input_codes! { EvSynCodes, u16, {
    SYN_REPORT = 0x00,
    SYN_CONFIG = 0x01,
    SYN_MT_REPORT = 0x02,
    SYN_DROPPED = 0x03,
    SYN_MAX = 0x0f,
    SYN_CNT = SYN_MAX as u16 + 1,
}}

input_codes! { EvKeyCodes, u16, {
    KEY_RESERVED = 0,
    KEY_ESC = 1,
    KEY_1 = 2,
    KEY_2 = 3,
    KEY_3 = 4,
    KEY_4 = 5,
    KEY_5 = 6,
    KEY_6 = 7,
    KEY_7 = 8,
    KEY_8 = 9,
    KEY_9 = 10,
    KEY_0 = 11,
    KEY_MINUS = 12,
    KEY_EQUAL = 13,
    KEY_BACKSPACE = 14,
    KEY_TAB = 15,
    KEY_Q = 16,
    KEY_W = 17,
    KEY_E = 18,
    KEY_R = 19,
    KEY_T = 20,
    KEY_Y = 21,
    KEY_U = 22,
    KEY_I = 23,
    KEY_O = 24,
    KEY_P = 25,
    KEY_LEFTBRACE = 26,
    KEY_RIGHTBRACE = 27,
    KEY_ENTER = 28,
    KEY_LEFTCTRL = 29,
    KEY_A = 30,
    KEY_S = 31,
    KEY_D = 32,
    KEY_F = 33,
    KEY_G = 34,
    KEY_H = 35,
    KEY_J = 36,
    KEY_K = 37,
    KEY_L = 38,
    KEY_SEMICOLON = 39,
    KEY_APOSTROPHE = 40,
    KEY_GRAVE = 41,
    KEY_LEFTSHIFT = 42,
    KEY_BACKSLASH = 43,
    KEY_Z = 44,
    KEY_X = 45,
    KEY_C = 46,
    KEY_V = 47,
    KEY_B = 48,
    KEY_N = 49,
    KEY_M = 50,
    KEY_COMMA = 51,
    KEY_DOT = 52,
    KEY_SLASH = 53,
    KEY_RIGHTSHIFT = 54,
    KEY_KPASTERISK = 55,
    KEY_LEFTALT = 56,
    KEY_SPACE = 57,
    KEY_CAPSLOCK = 58,
    KEY_F1 = 59,
    KEY_F2 = 60,
    KEY_F3 = 61,
    KEY_F4 = 62,
    KEY_F5 = 63,
    KEY_F6 = 64,
    KEY_F7 = 65,
    KEY_F8 = 66,
    KEY_F9 = 67,
    KEY_F10 = 68,
    KEY_NUMLOCK = 69,
    KEY_SCROLLLOCK = 70,
    KEY_KP7 = 71,
    KEY_KP8 = 72,
    KEY_KP9 = 73,
    KEY_KPMINUS = 74,
    KEY_KP4 = 75,
    KEY_KP5 = 76,
    KEY_KP6 = 77,
    KEY_KPPLUS = 78,
    KEY_KP1 = 79,
    KEY_KP2 = 80,
    KEY_KP3 = 81,
    KEY_KP0 = 82,
    KEY_KPDOT = 83,
    KEY_ZENKAKUHANKAKU = 85,
    KEY_102ND = 86,
    KEY_F11 = 87,
    KEY_F12 = 88,
    KEY_RO = 89,
    KEY_KATAKANA = 90,
    KEY_HIRAGANA = 91,
    KEY_HENKAN = 92,
    KEY_KATAKANAHIRAGANA = 93,
    KEY_MUHENKAN = 94,
    KEY_KPJPCOMMA = 95,
    KEY_KPENTER = 96,
    KEY_RIGHTCTRL = 97,
    KEY_KPSLASH = 98,
    KEY_SYSRQ = 99,
    KEY_RIGHTALT = 100,
    KEY_LINEFEED = 101,
    KEY_HOME = 102,
    KEY_UP = 103,
    KEY_PAGEUP = 104,
    KEY_LEFT = 105,
    KEY_RIGHT = 106,
    KEY_END = 107,
    KEY_DOWN = 108,
    KEY_PAGEDOWN = 109,
    KEY_INSERT = 110,
    KEY_DELETE = 111,
    KEY_MACRO = 112,
    KEY_MUTE = 113,
    KEY_VOLUMEDOWN = 114,
    KEY_VOLUMEUP = 115,
    KEY_POWER = 116,
    KEY_KPEQUAL = 117,
    KEY_KPPLUSMINUS = 118,
    KEY_PAUSE = 119,
    KEY_SCALE = 120,
    KEY_KPCOMMA = 121,
    KEY_HANGEUL_HANGUEL = 122,
    KEY_HANJA = 123,
    KEY_YEN = 124,
    KEY_LEFTMETA = 125,
    KEY_RIGHTMETA = 126,
    KEY_COMPOSE = 127,
    KEY_STOP = 128,
    KEY_AGAIN = 129,
    KEY_PROPS = 130,
    KEY_UNDO = 131,
    KEY_FRONT = 132,
    KEY_COPY = 133,
    KEY_OPEN = 134,
    KEY_PASTE = 135,
    KEY_FIND = 136,
    KEY_CUT = 137,
    KEY_HELP = 138,
    KEY_MENU = 139,
    KEY_CALC = 140,
    KEY_SETUP = 141,
    KEY_SLEEP = 142,
    KEY_WAKEUP = 143,
    KEY_FILE = 144,
    KEY_SENDFILE = 145,
    KEY_DELETEFILE = 146,
    KEY_XFER = 147,
    KEY_PROG1 = 148,
    KEY_PROG2 = 149,
    KEY_WWW = 150,
    KEY_MSDOS = 151,
    KEY_COFFEE_SCREENLOCK = 152,
    KEY_ROTATE_DISPLAY_DIRECTION = 153,
    KEY_CYCLEWINDOWS = 154,
    KEY_MAIL = 155,
    KEY_BOOKMARKS = 156,
    KEY_COMPUTER = 157,
    KEY_BACK = 158,
    KEY_FORWARD = 159,
    KEY_CLOSECD = 160,
    KEY_EJECTCD = 161,
    KEY_EJECTCLOSECD = 162,
    KEY_NEXTSONG = 163,
    KEY_PLAYPAUSE = 164,
    KEY_PREVIOUSSONG = 165,
    KEY_STOPCD = 166,
    KEY_RECORD = 167,
    KEY_REWIND = 168,
    KEY_PHONE = 169,
    KEY_ISO = 170,
    KEY_CONFIG = 171,
    KEY_HOMEPAGE = 172,
    KEY_REFRESH = 173,
    KEY_EXIT = 174,
    KEY_MOVE = 175,
    KEY_EDIT = 176,
    KEY_SCROLLUP = 177,
    KEY_SCROLLDOWN = 178,
    KEY_KPLEFTPAREN = 179,
    KEY_KPRIGHTPAREN = 180,
    KEY_NEW = 181,
    KEY_REDO = 182,
    KEY_F13 = 183,
    KEY_F14 = 184,
    KEY_F15 = 185,
    KEY_F16 = 186,
    KEY_F17 = 187,
    KEY_F18 = 188,
    KEY_F19 = 189,
    KEY_F20 = 190,
    KEY_F21 = 191,
    KEY_F22 = 192,
    KEY_F23 = 193,
    KEY_F24 = 194,
    KEY_PLAYCD = 200,
    KEY_PAUSECD = 201,
    KEY_PROG3 = 202,
    KEY_PROG4 = 203,
    KEY_ALL_APPLICATIONS_DASHBOARD = 204,
    KEY_SUSPEND = 205,
    KEY_CLOSE = 206,
    KEY_PLAY = 207,
    KEY_FASTFORWARD = 208,
    KEY_BASSBOOST = 209,
    KEY_PRINT = 210,
    KEY_HP = 211,
    KEY_CAMERA = 212,
    KEY_SOUND = 213,
    KEY_QUESTION = 214,
    KEY_EMAIL = 215,
    KEY_CHAT = 216,
    KEY_SEARCH = 217,
    KEY_CONNECT = 218,
    KEY_FINANCE = 219,
    KEY_SPORT = 220,
    KEY_SHOP = 221,
    KEY_ALTERASE = 222,
    KEY_CANCEL = 223,
    KEY_BRIGHTNESSDOWN = 224,
    KEY_BRIGHTNESSUP = 225,
    KEY_MEDIA = 226,
    KEY_SWITCHVIDEOMODE = 227,
    KEY_KBDILLUMTOGGLE = 228,
    KEY_KBDILLUMDOWN = 229,
    KEY_KBDILLUMUP = 230,
    KEY_SEND = 231,
    KEY_REPLY = 232,
    KEY_FORWARDMAIL = 233,
    KEY_SAVE = 234,
    KEY_DOCUMENTS = 235,
    KEY_BATTERY = 236,
    KEY_BLUETOOTH = 237,
    KEY_WLAN = 238,
    KEY_UWB = 239,
    KEY_UNKNOWN = 240,
    KEY_VIDEO_NEXT = 241,
    KEY_VIDEO_PREV = 242,
    KEY_BRIGHTNESS_CYCLE = 243,
    KEY_BRIGHTNESS_ZERO_AUTO = 244,
    KEY_DISPLAY_OFF = 245,
    KEY_WWAN_WIMAX = 246,
    KEY_RFKILL = 247,
    KEY_MICMUTE = 248,
    BTN_0 = 0x100,
    BTN_1 = 0x101,
    BTN_2 = 0x102,
    BTN_3 = 0x103,
    BTN_4 = 0x104,
    BTN_5 = 0x105,
    BTN_6 = 0x106,
    BTN_7 = 0x107,
    BTN_8 = 0x108,
    BTN_9 = 0x109,
    BTN_LEFT = 0x110,
    BTN_RIGHT = 0x111,
    BTN_MIDDLE = 0x112,
    BTN_SIDE = 0x113,
    BTN_EXTRA = 0x114,
    BTN_FORWARD = 0x115,
    BTN_BACK = 0x116,
    BTN_TASK = 0x117,
    BTN_JOYSTICK = 0x120,
    BTN_THUMB = 0x121,
    BTN_THUMB2 = 0x122,
    BTN_TOP = 0x123,
    BTN_TOP2 = 0x124,
    BTN_BASE = 0x126,
    BTN_PINKIE = 0x125,
    BTN_BASE2 = 0x127,
    BTN_BASE3 = 0x128,
    BTN_BASE4 = 0x129,
    BTN_BASE5 = 0x12a,
    BTN_BASE6 = 0x12b,
    BTN_DEAD = 0x12f,
    BTN_SOUTH_A = 0x130,
    BTN_EAST_B = 0x131,
    BTN_C = 0x132,
    BTN_NORTH_X = 0x133,
    BTN_WEST_Y = 0x134,
    BTN_Z = 0x135,
    BTN_TL = 0x136,
    BTN_TR = 0x137,
    BTN_TL2 = 0x138,
    BTN_TR2 = 0x139,
    BTN_SELECT = 0x13a,
    BTN_START = 0x13b,
    BTN_MODE = 0x13c,
    BTN_THUMBL = 0x13d,
    BTN_THUMBR = 0x13e,
    BTN_TOOL_PEN = 0x140,
    BTN_TOOL_RUBBER = 0x141,
    BTN_TOOL_BRUSH = 0x142,
    BTN_TOOL_PENCIL = 0x143,
    BTN_TOOL_AIRBRUSH = 0x144,
    BTN_TOOL_FINGER = 0x145,
    BTN_TOOL_MOUSE = 0x146,
    BTN_TOOL_LENS = 0x147,
    BTN_TOOL_QUINTTAP = 0x148,
    BTN_STYLUS3 = 0x149,
    BTN_TOUCH = 0x14a,
    BTN_STYLUS = 0x14b,
    BTN_STYLUS2 = 0x14c,
    BTN_TOOL_DOUBLETAP = 0x14d,
    BTN_TOOL_TRIPLETAP = 0x14e,
    BTN_TOOL_QUADTAP = 0x14f,
    BTN_GEAR_DOWN = 0x150,
    BTN_GEAR_UP = 0x151,
    KEY_OK = 0x160,
    KEY_SELECT = 0x161,
    KEY_GOTO = 0x162,
    KEY_CLEAR = 0x163,
    KEY_POWER2 = 0x164,
    KEY_OPTION = 0x165,
    KEY_INFO = 0x166,
    KEY_TIME = 0x167,
    KEY_VENDOR = 0x168,
    KEY_ARCHIVE = 0x169,
    KEY_PROGRAM = 0x16a,
    KEY_CHANNEL = 0x16b,
    KEY_FAVORITES = 0x16c,
    KEY_EPG = 0x16d,
    KEY_PVR = 0x16e,
    KEY_MHP = 0x16f,
    KEY_LANGUAGE = 0x170,
    KEY_TITLE = 0x171,
    KEY_SUBTITLE = 0x172,
    KEY_ANGLE = 0x173,
    KEY_FULL_SCREEN = 0x174,
    KEY_MODE = 0x175,
    KEY_KEYBOARD = 0x176,
    KEY_ASPECT_RATIO = 0x177,
    KEY_PC = 0x178,
    KEY_TV = 0x179,
    KEY_TV2 = 0x17a,
    KEY_VCR = 0x17b,
    KEY_VCR2 = 0x17c,
    KEY_SAT = 0x17d,
    KEY_SAT2 = 0x17e,
    KEY_CD = 0x17f,
    KEY_TAPE = 0x180,
    KEY_RADIO = 0x181,
    KEY_TUNER = 0x182,
    KEY_PLAYER = 0x183,
    KEY_TEXT = 0x184,
    KEY_DVD = 0x185,
    KEY_AUX = 0x186,
    KEY_MP3 = 0x187,
    KEY_AUDIO = 0x188,
    KEY_VIDEO = 0x189,
    KEY_DIRECTORY = 0x18a,
    KEY_LIST = 0x18b,
    KEY_MEMO = 0x18c,
    KEY_CALENDAR = 0x18d,
    KEY_RED = 0x18e,
    KEY_GREEN = 0x18f,
    KEY_YELLOW = 0x190,
    KEY_BLUE = 0x191,
    KEY_CHANNELUP = 0x192,
    KEY_CHANNELDOWN = 0x193,
    KEY_FIRST = 0x194,
    KEY_LAST = 0x195,
    KEY_AB = 0x196,
    KEY_NEXT = 0x197,
    KEY_RESTART = 0x198,
    KEY_SLOW = 0x199,
    KEY_SHUFFLE = 0x19a,
    KEY_BREAK = 0x19b,
    KEY_PREVIOUS = 0x19c,
    KEY_DIGITS = 0x19d,
    KEY_TEEN = 0x19e,
    KEY_TWEN = 0x19f,
    KEY_VIDEOPHONE = 0x1a0,
    KEY_GAMES = 0x1a1,
    KEY_ZOOMIN = 0x1a2,
    KEY_ZOOMOUT = 0x1a3,
    KEY_ZOOMRESET = 0x1a4,
    KEY_WORDPROCESSOR = 0x1a5,
    KEY_EDITOR = 0x1a6,
    KEY_SPREADSHEET = 0x1a7,
    KEY_GRAPHICSEDITOR = 0x1a8,
    KEY_PRESENTATION = 0x1a9,
    KEY_DATABASE = 0x1aa,
    KEY_NEWS = 0x1ab,
    KEY_VOICEMAIL = 0x1ac,
    KEY_ADDRESSBOOK = 0x1ad,
    KEY_MESSENGER = 0x1ae,
    KEY_DISPLAYTOGGLE = 0x1af,
    KEY_SPELLCHECK = 0x1b0,
    KEY_LOGOFF = 0x1b1,
    KEY_DOLLAR = 0x1b2,
    KEY_EURO = 0x1b3,
}}

input_codes! { EvRelCodes, u16, {
    REL_X = 0x00,
    REL_Y = 0x01,
    REL_Z = 0x02,
    REL_RX = 0x03,
    REL_RY = 0x04,
    REL_RZ = 0x05,
    REL_HWHEEL = 0x06,
    REL_DIAL = 0x07,
    REL_WHEEL = 0x08,
    REL_MISC = 0x09,
    REL_RESERVED = 0x0a,
    REL_WHEEL_HI_RES = 0x0b,
    REL_HWHEEL_HI_RES = 0x0c,
    REL_MAX = 0x0f,
    REL_CNT = REL_MAX as u16 + 1,
}}

input_codes! { EvAbsCodes, u16, {
    ABS_X = 0x00,
    ABS_Y = 0x01,
    ABS_Z = 0x02,
    ABS_RX = 0x03,
    ABS_RY = 0x04,
    ABS_RZ = 0x05,
    ABS_THROTTLE = 0x06,
    ABS_RUDDER = 0x07,
    ABS_WHEEL = 0x08,
    ABS_GAS = 0x09,
    ABS_BRAKE = 0x0a,
    ABS_HAT0X = 0x10,
    ABS_HAT0Y = 0x11,
    ABS_HAT1X = 0x12,
    ABS_HAT1Y = 0x13,
    ABS_HAT2X = 0x14,
    ABS_HAT2Y = 0x15,
    ABS_HAT3X = 0x16,
    ABS_HAT3Y = 0x17,
    ABS_PRESSURE = 0x18,
    ABS_DISTANCE = 0x19,
    ABS_TILT_X = 0x1a,
    ABS_TILT_Y = 0x1b,
    ABS_TOOL_WIDTH = 0x1c,
    ABS_VOLUME = 0x20,
    ABS_PROFILE = 0x21,
    ABS_MISC = 0x28,
    ABS_RESERVED = 0x2e,
    ABS_MT_SLOT = 0x2f,
    ABS_MT_TOUCH_MAJOR = 0x30,
    ABS_MT_TOUCH_MINOR = 0x31,
    ABS_MT_WIDTH_MAJOR = 0x32,
    ABS_MT_WIDTH_MINOR = 0x33,
    ABS_MT_ORIENTATION = 0x34,
    ABS_MT_POSITION_X = 0x35,
    ABS_MT_POSITION_Y = 0x36,
    ABS_MT_TOOL_TYPE = 0x37,
    ABS_MT_BLOB_ID = 0x38,
    ABS_MT_TRACKING_ID = 0x39,
    ABS_MT_PRESSURE = 0x3a,
    ABS_MT_DISTANCE = 0x3b,
    ABS_MT_TOOL_X = 0x3c,
    ABS_MT_TOOL_Y = 0x3d,
    ABS_MAX = 0x3f,
    ABS_CNT = ABS_MAX as u16 + 1,
}}

#[allow(unused, non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvMscCodes {
    MSC_SERIAL = 0x00,
    MSC_PULSELED = 0x01,
    MSC_GESTURE = 0x02,
    MSC_RAW = 0x03,
    MSC_SCAN = 0x04,
    MSC_TIMESTAMP = 0x05,
    MSC_MAX = 0x07,
    MSC_CNT = EvMscCodes::MSC_MAX as u16 + 1,
}

#[allow(unused, non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvSwCodes {
    SW_LID = 0x00,                  /* set = lid shut */
    SW_TABLET_MODE = 0x01,          /* set = tablet mode */
    SW_HEADPHONE_INSERT = 0x02,     /* set = inserted */
    SW_RFKILL_ALL_RADIO = 0x03,     /* rfkill master switch, type "any" set = radio enabled */
    SW_MICROPHONE_INSERT = 0x04,    /* set = inserted */
    SW_DOCK = 0x05,                 /* set = plugged into dock */
    SW_LINEOUT_INSERT = 0x06,       /* set = inserted */
    SW_JACK_PHYSICAL_INSERT = 0x07, /* set = mechanical switch set */
    SW_VIDEOOUT_INSERT = 0x08,      /* set = inserted */
    SW_CAMERA_LENS_COVER = 0x09,    /* set = lens covered */
    SW_KEYPAD_SLIDE = 0x0a,         /* set = keypad slide out */
    SW_FRONT_PROXIMITY = 0x0b,      /* set = front proximity sensor active */
    SW_ROTATE_LOCK = 0x0c,          /* set = rotate locked/disabled */
    SW_LINEIN_INSERT = 0x0d,        /* set = inserted */
    SW_MUTE_DEVICE = 0x0e,          /* set = device disabled */
    SW_PEN_INSERTED = 0x0f,         /* set = pen inserted */
    SW_MACHINE_COVER = 0x10,        /* set = cover closed */
    SW_CNT = EvSwCodes::SW_MACHINE_COVER as u16 + 1,
}

#[allow(unused, non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvLedCodes {
    LED_NUML = 0x00,
    LED_CAPSL = 0x01,
    LED_SCROLLL = 0x02,
    LED_COMPOSE = 0x03,
    LED_KANA = 0x04,
    LED_SLEEP = 0x05,
    LED_SUSPEND = 0x06,
    LED_MUTE = 0x07,
    LED_MISC = 0x08,
    LED_MAIL = 0x09,
    LED_CHARGING = 0x0a,
    LED_MAX = 0x0f,
    LED_CNT = EvLedCodes::LED_MAX as u16 + 1,
}

#[allow(unused, non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvSndCodes {
    SND_CLICK = 0x00,
    SND_BELL = 0x01,
    SND_TONE = 0x02,
    SND_MAX = 0x07,
    SND_CNT = EvSndCodes::SND_MAX as u16 + 1,
}

#[allow(unused, non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvRepCodes {
    REP_DELAY = 0x00,
    REP_PERIOD_MAX = 0x01,
    REP_CNT = EvRepCodes::REP_PERIOD_MAX as u16 + 1,
}

input_codes! { KeyAction, i32, {
    KEY_UP = 0x00,
    KEY_DOWN = 0x01,
    KEY_REPEAT = 0x02,
}}

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
struct InputEvent {
    time: libc_sys::timeval,
    ty: u16,
    code: u16,
    value: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

// Linux evdev's _IOR('E', nr, size), using the existing in-house ioctl FFI.
fn evdev_read(file: &File, nr: u32, bytes: &mut [u8]) -> bool {
    let request = 0x8000_0000u32 | ((bytes.len() as u32) << 16) | (b'E' as u32) << 8 | nr;
    unsafe { libc_sys::ioctl(file.as_raw_fd(), request as std::os::raw::c_ulong, bytes.as_mut_ptr()) >= 0 }
}

fn abs_info(file: &File, axis: u16) -> Option<AbsInfo> {
    let mut supported = [0u8; 8];
    if !evdev_read(file, 0x23, &mut supported) || !has_bit(&supported, axis) {
        return None;
    }
    let mut bytes = [0u8; std::mem::size_of::<AbsInfo>()];
    evdev_read(file, 0x40 + axis as u32, &mut bytes)
        .then(|| unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<AbsInfo>()) })
}

fn has_bit(bytes: &[u8], bit: u16) -> bool {
    bytes.get(bit as usize / 8).is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
}

#[derive(Default, Clone)]
struct Contact {
    id: Option<i32>,
    x: Option<i32>,
    y: Option<i32>,
}

struct Touchpad {
    axes: [AbsInfo; 2],
    slots: Vec<Contact>,
    slot_base: i32,
    slot: Option<usize>,
    has_touch: bool,
    contact: bool,
    tools: [bool; 5],
    // Exact slot identities: a new finger or a change in contact count must
    // establish a new baseline, never jump the pointer/scroll position.
    previous_ids: Vec<(usize, i32)>,
    previous: Option<Vec2d>,
    suppress_until_lift: bool,
}

impl Touchpad {
    fn open(file: &File, props: &[u8], keys: &[u8]) -> Option<Self> {
        if has_bit(props, 1) || !has_bit(keys, EvKeyCodes::BTN_TOOL_FINGER) {
            return None;
        }
        let mt = abs_info(file, EvAbsCodes::ABS_MT_SLOT).filter(|axis| {
            axis.minimum == 0 && (0..64).contains(&axis.maximum)
        });
        let codes = if mt.is_some() {
            [EvAbsCodes::ABS_MT_POSITION_X, EvAbsCodes::ABS_MT_POSITION_Y]
        } else {
            [EvAbsCodes::ABS_X, EvAbsCodes::ABS_Y]
        };
        let axes = [abs_info(file, codes[0])?, abs_info(file, codes[1])?];
        if axes.iter().any(|axis| axis.maximum <= axis.minimum) { return None; }
        let mut pad = Self {
            axes,
            slots: vec![Contact::default(); mt.map_or(0, |axis| (axis.maximum + 1) as usize)],
            slot_base: mt.map_or(0, |axis| axis.minimum),
            slot: None,
            has_touch: has_bit(keys, EvKeyCodes::BTN_TOUCH),
            contact: false,
            tools: [false; 5],
            previous_ids: Vec::new(),
            previous: None,
            suppress_until_lift: false,
        };
        pad.resync(file);
        Some(pad)
    }

    fn tool_index(code: u16) -> Option<usize> {
        match code {
            EvKeyCodes::BTN_TOOL_FINGER => Some(0),
            EvKeyCodes::BTN_TOOL_DOUBLETAP => Some(1),
            EvKeyCodes::BTN_TOOL_TRIPLETAP => Some(2),
            EvKeyCodes::BTN_TOOL_QUADTAP => Some(3),
            EvKeyCodes::BTN_TOOL_QUINTTAP => Some(4),
            _ => None,
        }
    }

    fn resync(&mut self, file: &File) {
        self.previous = None;
        self.previous_ids.clear();
        self.suppress_until_lift = true;
        let mut keys = [0u8; 96];
        evdev_read(file, 0x18, &mut keys); // EVIOCGKEY
        self.contact = has_bit(&keys, EvKeyCodes::BTN_TOUCH);
        for (index, code) in [EvKeyCodes::BTN_TOOL_FINGER, EvKeyCodes::BTN_TOOL_DOUBLETAP,
            EvKeyCodes::BTN_TOOL_TRIPLETAP, EvKeyCodes::BTN_TOOL_QUADTAP,
            EvKeyCodes::BTN_TOOL_QUINTTAP].into_iter().enumerate() {
            self.tools[index] = has_bit(&keys, code);
        }
        if self.slots.is_empty() {
            for (index, code) in [EvAbsCodes::ABS_X, EvAbsCodes::ABS_Y].into_iter().enumerate() {
                if let Some(axis) = abs_info(file, code) { self.axes[index] = axis; }
            }
        } else {
            self.slots.fill(Contact::default());
            self.slot = abs_info(file, EvAbsCodes::ABS_MT_SLOT).and_then(|axis| self.slot_index(axis.value));
            for code in [EvAbsCodes::ABS_MT_TRACKING_ID, EvAbsCodes::ABS_MT_POSITION_X, EvAbsCodes::ABS_MT_POSITION_Y] {
                // EVIOCGMTSLOTS takes the desired axis followed by one i32 per slot.
                let mut bytes = vec![0u8; (1 + self.slots.len()) * 4];
                bytes[..4].copy_from_slice(&(code as i32).to_ne_bytes());
                if !evdev_read(file, 0x0a, &mut bytes) { continue; }
                for (slot, value) in self.slots.iter_mut().zip(bytes[4..].chunks_exact(4)) {
                    let value = i32::from_ne_bytes(value.try_into().unwrap());
                    match code {
                        EvAbsCodes::ABS_MT_TRACKING_ID => slot.id = (value >= 0).then_some(value),
                        EvAbsCodes::ABS_MT_POSITION_X => slot.x = Some(value),
                        EvAbsCodes::ABS_MT_POSITION_Y => slot.y = Some(value),
                        _ => {}
                    }
                }
            }
        }
        // A pad opened while untouched is ready for the first contact.
        if !self.contact && !self.tools.iter().any(|tool| *tool)
            && self.slots.iter().all(|slot| slot.id.is_none()) {
            self.suppress_until_lift = false;
        }
    }

    fn slot_index(&self, value: i32) -> Option<usize> {
        let index = value.checked_sub(self.slot_base)?;
        (index >= 0 && (index as usize) < self.slots.len()).then_some(index as usize)
    }

    // Consume only touch coordinates/contact keys. Physical button events
    // still take the ordinary mouse path. In particular BTN_TOUCH is NOT a
    // click on an indirect touchpad.
    fn consume(&mut self, event: &InputEvent) -> bool {
        if event.ty == InputEventType::EV_REL
            && matches!(event.code, EvRelCodes::REL_X | EvRelCodes::REL_Y) {
            return true;
        }
        if event.ty == InputEventType::EV_KEY {
            if event.code == EvKeyCodes::BTN_TOUCH {
                if self.contact != (event.value != 0) { self.previous = None; }
                self.contact = event.value != 0;
                return true;
            }
            if let Some(index) = Self::tool_index(event.code) {
                self.tools[index] = event.value != 0;
                return true;
            }
        }
        if event.ty != InputEventType::EV_ABS { return false; }
        if self.slots.is_empty() {
            match event.code {
                EvAbsCodes::ABS_X => self.axes[0].value = event.value,
                EvAbsCodes::ABS_Y => self.axes[1].value = event.value,
                _ => {}
            }
        } else if event.code == EvAbsCodes::ABS_MT_SLOT {
            self.slot = self.slot_index(event.value);
        } else if let Some(slot) = self.slot.and_then(|index| self.slots.get_mut(index)) {
            match event.code {
                EvAbsCodes::ABS_MT_TRACKING_ID => {
                    slot.id = (event.value >= 0).then_some(event.value);
                }
                EvAbsCodes::ABS_MT_POSITION_X => slot.x = Some(event.value),
                EvAbsCodes::ABS_MT_POSITION_Y => slot.y = Some(event.value),
                _ => {}
            }
        }
        true // Never process both legacy ABS_X/Y and the MT coordinates.
    }

    fn finish_report(&mut self) -> Option<(Vec2d, bool)> {
        // Some pads keep reporting proximity positions while a finger is
        // hovering. Only actual surface contact may move the pointer.
        if self.has_touch && !self.contact {
            self.previous = None;
            self.previous_ids.clear();
            self.suppress_until_lift = false;
            return None;
        }
        let mut position = dvec2(0.0, 0.0);
        let mut ids = Vec::new();
        if self.slots.is_empty() {
            let count = self.tools.iter().rposition(|tool| *tool).map_or(self.contact as usize, |i| i + 1);
            if count > 0 {
                position = dvec2(self.axes[0].value as f64, self.axes[1].value as f64);
                for i in 0..count { ids.push((i, 0)); }
            }
        } else {
            for (index, slot) in self.slots.iter().enumerate() {
                if let Some(id) = slot.id {
                    let (Some(x), Some(y)) = (slot.x, slot.y) else {
                        self.previous = None;
                        return None;
                    };
                    ids.push((index, id));
                    position += dvec2(x as f64, y as f64);
                }
            }
            if !ids.is_empty() { position = position / ids.len() as f64; }
        }
        if ids.is_empty() { self.suppress_until_lift = false; }
        if self.suppress_until_lift || !(1..=2).contains(&ids.len()) {
            self.previous = None;
            self.previous_ids = ids;
            return None;
        }
        let previous = self.previous.replace(position);
        let same_contacts = self.previous_ids == ids;
        let scroll = ids.len() == 2;
        self.previous_ids = ids;
        if !same_contacts { return None; }
        let delta = position - previous?;
        // Kernel resolution is units/mm. Old pads without it use their axis
        // extent as a conservative physical-size estimate, not screen size.
        let resolution = |index: usize, size_mm: f64| {
            let axis = self.axes[index];
            if axis.resolution > 0 { axis.resolution as f64 }
            else { (axis.maximum - axis.minimum) as f64 / size_mm }
        };
        let delta = dvec2(delta.x / resolution(0, 100.0), delta.y / resolution(1, 65.0));
        (delta.x != 0.0 || delta.y != 0.0).then_some((delta, scroll))
    }
}

enum DeviceEvent {
    Raw(InputEvent),
    TouchpadMotion { millimeters: Vec2d, scroll: bool },
}

struct InputDevice {
    id: u64,
    path: PathBuf,
    file: File,
    pending: Vec<InputEvent>,
    dropped: bool,
    reported_input: bool,
    touchpad: Option<Touchpad>,
    absolute_axes: Option<[AbsInfo; 2]>,
    absolute_mt: bool,
}

pub struct RawInput {
    pub modifiers: KeyModifiers,
    devices: Vec<InputDevice>,
    next_device_id: u64,
    modifier_keys: HashSet<(u64, u16)>,
    denied: HashSet<PathBuf>,
    next_scan: f64,
    width: f64,
    height: f64,
    dpi_factor: f64,
    abs: Vec2d,
}

impl RawInput {
    pub fn new(width: f64, height: f64, dpi_factor: f64) -> Self {
        let mut input = Self {
            devices: Vec::new(),
            next_device_id: 0,
            modifier_keys: HashSet::new(),
            denied: HashSet::new(),
            next_scan: 1.0,
            width,
            height,
            dpi_factor,
            abs: dvec2(0.0, 0.0),
            modifiers: Default::default(),
        };
        input.scan_devices();
        input
    }

    /// The pointer's clamp rectangle follows the primary display. Called when a
    /// hotplug reconcile changes the desktop size; the current pointer position
    /// is kept and clamped so it stays on screen.
    pub fn set_bounds(&mut self, width: f64, height: f64, dpi_factor: f64) {
        self.width = width;
        self.height = height;
        if dpi_factor.is_finite() && dpi_factor > 0.0 {
            self.dpi_factor = dpi_factor;
        }
        self.abs.x = self.abs.x.clamp(0.0, width.max(0.0));
        self.abs.y = self.abs.y.clamp(0.0, height.max(0.0));
    }

    fn scan_devices(&mut self) {
        let entries = match std::fs::read_dir("/dev/input") {
            Ok(entries) => entries,
            Err(error) => {
                if self.denied.insert(PathBuf::from("/dev/input")) {
                    crate::warning!("Direct input: enumerate /dev/input: {error}");
                }
                return;
            }
        };
        let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.file_name().and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("event"))
                .is_some_and(|number| !number.is_empty() && number.bytes().all(|c| c.is_ascii_digit()))
        }).collect();
        paths.sort();
        self.denied.retain(|path| paths.contains(path));
        for path in paths {
            if self.devices.iter().any(|device| device.path == path) {
                continue;
            }
            match OpenOptions::new().read(true).custom_flags(libc_sys::O_NONBLOCK).open(&path) {
                Ok(file) => {
                    self.denied.remove(&path);
                    let mut props = [0u8; 8];
                    let mut keys = [0u8; 96];
                    evdev_read(&file, 0x09, &mut props); // EVIOCGPROP
                    evdev_read(&file, 0x21, &mut keys); // EVIOCGBIT(EV_KEY)
                    let touchpad = Touchpad::open(&file, &props, &keys);
                    let mut absolute_mt = false;
                    let absolute_axes = if touchpad.is_none() && (has_bit(&props, 1)
                        || has_bit(&keys, EvKeyCodes::BTN_TOOL_PEN) || has_bit(&keys, EvKeyCodes::BTN_TOUCH)) {
                        abs_info(&file, EvAbsCodes::ABS_X).zip(abs_info(&file, EvAbsCodes::ABS_Y))
                            .or_else(|| {
                                absolute_mt = true;
                                abs_info(&file, EvAbsCodes::ABS_MT_POSITION_X)
                                    .zip(abs_info(&file, EvAbsCodes::ABS_MT_POSITION_Y))
                            })
                            .filter(|(x, y)| x.maximum > x.minimum && y.maximum > y.minimum)
                            .map(|(x, y)| [x, y])
                    } else { None };
                    crate::log!("Direct input: opened {}{}", path.display(),
                        if touchpad.is_some() { " (relative touchpad, two-finger scroll)" } else { "" });
                    self.next_device_id += 1;
                    self.devices.push(InputDevice { id: self.next_device_id, path, file,
                        pending: Vec::new(), dropped: false, reported_input: false, touchpad, absolute_axes, absolute_mt });
                }
                Err(error) => {
                    if self.denied.insert(path.clone()) {
                        crate::warning!("Direct input: open {}: {error}", path.display());
                    }
                }
            }
        }
    }

    pub fn fds(&self) -> impl Iterator<Item = i32> + '_ {
        self.devices.iter().map(|device| device.file.as_raw_fd())
    }

    pub fn poll_raw_input(&mut self, time: f64, window_id: WindowId) -> Vec<DirectEvent> {
        if time >= self.next_scan {
            self.next_scan = time + 1.0;
            self.scan_devices();
        }
        let mut dir_evts = Vec::new();
        let mut packets = Vec::new();
        // Poll nonblocking descriptors directly. Packet state belongs to each
        // device, so a partial report never spins the UI or mixes two devices.
        self.devices.retain_mut(|device| {
            const EVENT_SIZE: usize = std::mem::size_of::<InputEvent>();
            let mut buffer = [0u8; EVENT_SIZE * 64];
            // Bound work per device so an input flood cannot starve rendering.
            for _ in 0..16 {
                let len = match device.file.read(&mut buffer) {
                    Ok(0) => return false,
                    Ok(len) => len,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        crate::log!("Direct input: disconnected {}: {error}", device.path.display());
                        return false;
                    }
                };
                for bytes in buffer[..len].chunks_exact(EVENT_SIZE) {
                    // input_event consists only of integers; read_unaligned
                    // also works when the byte buffer has no struct alignment.
                    let event = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<InputEvent>()) };
                    if event.ty == InputEventType::EV_SYN {
                        if event.code == EvSynCodes::SYN_DROPPED {
                            device.pending.clear();
                            device.dropped = true;
                            crate::warning!("Direct input: event queue overrun on {}", device.path.display());
                        } else if event.code == EvSynCodes::SYN_REPORT {
                            if !device.dropped {
                                if !device.reported_input && device.pending.iter().any(|event| {
                                    matches!(event.ty, InputEventType::EV_KEY | InputEventType::EV_REL)
                                }) {
                                    crate::log!("Direct input: receiving keyboard/pointer reports from {}", device.path.display());
                                    device.reported_input = true;
                                }
                                if let Some(pad) = device.touchpad.as_mut() {
                                    device.pending.retain(|event| !pad.consume(event));
                                    if let Some((millimeters, scroll)) = pad.finish_report() {
                                        packets.push((device.id, event.time, DeviceEvent::TouchpadMotion { millimeters, scroll }));
                                    }
                                }
                                packets.extend(device.pending.drain(..)
                                    .map(|event| (device.id, event.time, DeviceEvent::Raw(event))));
                            } else if let Some(pad) = device.touchpad.as_mut() {
                                // The kernel requires a state query after SYN_DROPPED.
                                // Suppress a held gesture until lift instead of jumping.
                                pad.resync(&device.file);
                            }
                            device.dropped = false;
                        }
                    } else if !device.dropped {
                        if device.pending.len() < 1024 {
                            device.pending.push(event);
                        } else {
                            device.pending.clear();
                            device.dropped = true;
                        }
                    }
                }
            }
            true
        });
        // Composite keyboards can report modifiers and keys on different
        // event nodes. Preserve kernel order, not device enumeration order.
        packets.sort_by_key(|(_, time, _)| (time.tv_sec, time.tv_usec));
        self.modifier_keys.retain(|(id, _)| self.devices.iter().any(|device| device.id == *id));
        self.update_modifiers();
        self.process_event(&mut packets, &mut dir_evts, time, window_id);
        dir_evts
    }

    fn process_event(
        &mut self,
        evts: &mut Vec<(u64, libc_sys::timeval, DeviceEvent)>,
        dir_evts: &mut Vec<DirectEvent>,
        time: f64,
        window_id: WindowId,
    ) {
        for (device_id, _, event) in evts.drain(..) {
            let evt = match event {
                DeviceEvent::Raw(evt) => evt,
                DeviceEvent::TouchpadMotion { millimeters, scroll } => {
                    let delta = if scroll {
                        millimeters * (12.0 / self.dpi_factor)
                    } else {
                        millimeters * (20.0 * crate::linux_input::pointer_speed(true) as f64 / 100.0 / self.dpi_factor)
                    };
                    if scroll {
                        dir_evts.push(DirectEvent::Scroll(ScrollEvent {
                            window_id, scroll: delta, abs: self.abs, modifiers: self.modifiers,
                            handled_x: Cell::new(false), handled_y: Cell::new(false),
                            is_mouse: false, time, phase: Default::default(),
                        }));
                    } else {
                        self.abs.x = (self.abs.x + delta.x).clamp(0.0, self.width.max(0.0));
                        self.abs.y = (self.abs.y + delta.y).clamp(0.0, self.height.max(0.0));
                        dir_evts.push(DirectEvent::MouseMove(MouseMoveEvent {
                            lock_delta: Default::default(), abs: self.abs, window_id,
                            modifiers: self.modifiers, time, handled: Cell::new(Area::Empty),
                        }));
                    }
                    continue;
                }
            };
            match evt.ty {
                InputEventType::EV_REL => {
                    // relative input
                    self.process_rel_event(evt, dir_evts, time, window_id)
                }
                InputEventType::EV_ABS => {
                    // absolute input
                    self.process_abs_event(device_id, evt, dir_evts, time, window_id)
                }
                InputEventType::EV_KEY => {
                    // key press
                    self.process_key_event(device_id, evt, dir_evts, time, window_id)
                }
                _ => (),
            }
        }
    }

    fn process_rel_event(
        &mut self,
        evt: InputEvent,
        dir_evts: &mut Vec<DirectEvent>,
        time: f64,
        window_id: WindowId,
    ) {
        let code = evt.code;
        match code {
            EvRelCodes::REL_X => {
                self.abs.x += evt.value as f64 * crate::linux_input::pointer_speed(false) as f64 / 100.0 / self.dpi_factor;
                if self.abs.x < 0.0 {
                    self.abs.x = 0.0
                }
                if self.abs.x > self.width {
                    self.abs.x = self.width
                }
            }
            EvRelCodes::REL_Y => {
                self.abs.y += evt.value as f64 * crate::linux_input::pointer_speed(false) as f64 / 100.0 / self.dpi_factor;
                if self.abs.y < 0.0 {
                    self.abs.y = 0.0
                }
                if self.abs.y > self.height {
                    self.abs.y = self.height
                }
            }
            EvRelCodes::REL_WHEEL | EvRelCodes::REL_HWHEEL => {
                let delta = evt.value as f64 * 40.0;
                dir_evts.push(DirectEvent::Scroll(ScrollEvent {
                    window_id,
                    scroll: if code == EvRelCodes::REL_WHEEL {
                        dvec2(0.0, -delta)
                    } else {
                        dvec2(delta, 0.0)
                    },
                    abs: self.abs,
                    modifiers: self.modifiers,
                    handled_x: Cell::new(false),
                    handled_y: Cell::new(false),
                    is_mouse: true,
                    time,
                    phase: Default::default(),
                }));
                return;
            }
            _ => return (),
        }
        dir_evts.push(DirectEvent::MouseMove(MouseMoveEvent {
                lock_delta: Default::default(),
            abs: self.abs,
            window_id,
            modifiers: self.modifiers,
            time,
            handled: Cell::new(Area::Empty),
        }))
    }

    fn process_abs_event(
        &mut self,
        device_id: u64,
        evt: InputEvent,
        dir_evts: &mut Vec<DirectEvent>,
        time: f64,
        window_id: WindowId,
    ) {
        // Wheels, pedals and gamepad axes are not absolute pointing devices.
        let Some((axes, mt)) = self.devices.iter().find(|device| device.id == device_id)
            .and_then(|device| device.absolute_axes.map(|axes| (axes, device.absolute_mt))) else { return; };
        let normalized = |axis: AbsInfo| {
            ((evt.value as f64 - axis.minimum as f64) / (axis.maximum as f64 - axis.minimum as f64)).clamp(0.0, 1.0)
        };
        match (evt.code, mt) {
            (EvAbsCodes::ABS_X, false) | (EvAbsCodes::ABS_MT_POSITION_X, true) => {
                self.abs.x = normalized(axes[0]) * self.width;
            }
            (EvAbsCodes::ABS_Y, false) | (EvAbsCodes::ABS_MT_POSITION_Y, true) => {
                self.abs.y = normalized(axes[1]) * self.height;
            }
            _ => return (),
        }
        dir_evts.push(DirectEvent::MouseMove(MouseMoveEvent {
                lock_delta: Default::default(),
            abs: self.abs,
            window_id,
            modifiers: self.modifiers,
            time,
            handled: Cell::new(Area::Empty),
        }))
    }

    fn update_modifiers(&mut self) {
        let held = |left, right| self.modifier_keys.iter().any(|(_, key)| *key == left || *key == right);
        self.modifiers = KeyModifiers {
            shift: held(EvKeyCodes::KEY_LEFTSHIFT, EvKeyCodes::KEY_RIGHTSHIFT),
            control: held(EvKeyCodes::KEY_LEFTCTRL, EvKeyCodes::KEY_RIGHTCTRL),
            alt: held(EvKeyCodes::KEY_LEFTALT, EvKeyCodes::KEY_RIGHTALT),
            logo: held(EvKeyCodes::KEY_LEFTMETA, EvKeyCodes::KEY_RIGHTMETA),
        };
    }

    fn process_key_event(
        &mut self,
        device_id: u64,
        evt: InputEvent,
        dir_evts: &mut Vec<DirectEvent>,
        time: f64,
        window_id: WindowId,
    ) {
        let code = evt.code;
        let key_action = evt.value;
        if !(KeyAction::KEY_UP..=KeyAction::KEY_REPEAT).contains(&key_action) {
            return;
        }
        let key_code = match code {
            EvKeyCodes::KEY_ESC => KeyCode::Escape,
            EvKeyCodes::KEY_1 => KeyCode::Key1,
            EvKeyCodes::KEY_2 => KeyCode::Key2,
            EvKeyCodes::KEY_3 => KeyCode::Key3,
            EvKeyCodes::KEY_4 => KeyCode::Key4,
            EvKeyCodes::KEY_5 => KeyCode::Key5,
            EvKeyCodes::KEY_6 => KeyCode::Key6,
            EvKeyCodes::KEY_7 => KeyCode::Key7,
            EvKeyCodes::KEY_8 => KeyCode::Key8,
            EvKeyCodes::KEY_9 => KeyCode::Key9,
            EvKeyCodes::KEY_0 => KeyCode::Key0,
            EvKeyCodes::KEY_MINUS => KeyCode::Minus,
            EvKeyCodes::KEY_EQUAL => KeyCode::Equals,
            EvKeyCodes::KEY_BACKSPACE => KeyCode::Backspace,
            EvKeyCodes::KEY_TAB => KeyCode::Tab,
            EvKeyCodes::KEY_Q => KeyCode::KeyQ,
            EvKeyCodes::KEY_W => KeyCode::KeyW,
            EvKeyCodes::KEY_E => KeyCode::KeyE,
            EvKeyCodes::KEY_R => KeyCode::KeyR,
            EvKeyCodes::KEY_T => KeyCode::KeyT,
            EvKeyCodes::KEY_Y => KeyCode::KeyY,
            EvKeyCodes::KEY_U => KeyCode::KeyU,
            EvKeyCodes::KEY_I => KeyCode::KeyI,
            EvKeyCodes::KEY_O => KeyCode::KeyO,
            EvKeyCodes::KEY_P => KeyCode::KeyP,
            EvKeyCodes::KEY_LEFTBRACE => KeyCode::LBracket,
            EvKeyCodes::KEY_RIGHTBRACE => KeyCode::RBracket,
            EvKeyCodes::KEY_ENTER => KeyCode::ReturnKey,
            EvKeyCodes::KEY_LEFTCTRL => KeyCode::Control,
            EvKeyCodes::KEY_A => KeyCode::KeyA,
            EvKeyCodes::KEY_S => KeyCode::KeyS,
            EvKeyCodes::KEY_D => KeyCode::KeyD,
            EvKeyCodes::KEY_F => KeyCode::KeyF,
            EvKeyCodes::KEY_G => KeyCode::KeyG,
            EvKeyCodes::KEY_H => KeyCode::KeyH,
            EvKeyCodes::KEY_J => KeyCode::KeyJ,
            EvKeyCodes::KEY_K => KeyCode::KeyK,
            EvKeyCodes::KEY_L => KeyCode::KeyL,
            EvKeyCodes::KEY_SEMICOLON => KeyCode::Semicolon,
            EvKeyCodes::KEY_APOSTROPHE => KeyCode::Quote,
            EvKeyCodes::KEY_GRAVE => KeyCode::Backtick,
            EvKeyCodes::KEY_LEFTSHIFT => KeyCode::Shift,
            EvKeyCodes::KEY_BACKSLASH => KeyCode::Backslash,
            EvKeyCodes::KEY_Z => KeyCode::KeyZ,
            EvKeyCodes::KEY_X => KeyCode::KeyX,
            EvKeyCodes::KEY_C => KeyCode::KeyC,
            EvKeyCodes::KEY_V => KeyCode::KeyV,
            EvKeyCodes::KEY_B => KeyCode::KeyB,
            EvKeyCodes::KEY_N => KeyCode::KeyN,
            EvKeyCodes::KEY_M => KeyCode::KeyM,
            EvKeyCodes::KEY_COMMA => KeyCode::Comma,
            EvKeyCodes::KEY_DOT => KeyCode::Period,
            EvKeyCodes::KEY_SLASH => KeyCode::Slash,
            EvKeyCodes::KEY_RIGHTSHIFT => KeyCode::Shift,
            EvKeyCodes::KEY_KPASTERISK => KeyCode::NumpadMultiply,
            EvKeyCodes::KEY_LEFTALT => KeyCode::Alt,
            EvKeyCodes::KEY_SPACE => KeyCode::Space,
            EvKeyCodes::KEY_CAPSLOCK => KeyCode::Capslock,
            EvKeyCodes::KEY_F1 => KeyCode::F1,
            EvKeyCodes::KEY_F2 => KeyCode::F2,
            EvKeyCodes::KEY_F3 => KeyCode::F3,
            EvKeyCodes::KEY_F4 => KeyCode::F4,
            EvKeyCodes::KEY_F5 => KeyCode::F5,
            EvKeyCodes::KEY_F6 => KeyCode::F6,
            EvKeyCodes::KEY_F7 => KeyCode::F7,
            EvKeyCodes::KEY_F8 => KeyCode::F8,
            EvKeyCodes::KEY_F9 => KeyCode::F9,
            EvKeyCodes::KEY_F10 => KeyCode::F10,
            EvKeyCodes::KEY_NUMLOCK => KeyCode::Numlock,
            EvKeyCodes::KEY_SCROLLLOCK => KeyCode::ScrollLock,
            EvKeyCodes::KEY_KP7 => KeyCode::Numpad7,
            EvKeyCodes::KEY_KP8 => KeyCode::Numpad8,
            EvKeyCodes::KEY_KP9 => KeyCode::Numpad9,
            EvKeyCodes::KEY_KPMINUS => KeyCode::NumpadSubtract,
            EvKeyCodes::KEY_KP4 => KeyCode::Numpad4,
            EvKeyCodes::KEY_KP5 => KeyCode::Numpad5,
            EvKeyCodes::KEY_KP6 => KeyCode::Numpad6,
            EvKeyCodes::KEY_KPPLUS => KeyCode::NumpadAdd,
            EvKeyCodes::KEY_KP1 => KeyCode::Numpad1,
            EvKeyCodes::KEY_KP2 => KeyCode::Numpad2,
            EvKeyCodes::KEY_KP3 => KeyCode::Numpad3,
            EvKeyCodes::KEY_KP0 => KeyCode::Numpad0,
            EvKeyCodes::KEY_KPDOT => KeyCode::NumpadDecimal,
            EvKeyCodes::KEY_ZENKAKUHANKAKU => KeyCode::Unknown,
            EvKeyCodes::KEY_102ND => KeyCode::Backtick, //Seems odd but this was in the code this replaced
            EvKeyCodes::KEY_F11 => KeyCode::F11,
            EvKeyCodes::KEY_F12 => KeyCode::F12,
            EvKeyCodes::KEY_RO => KeyCode::NumpadDivide, //Seems odd but this was in the code this replaced
            EvKeyCodes::KEY_KPENTER => KeyCode::NumpadEnter,
            EvKeyCodes::KEY_RIGHTCTRL => KeyCode::Control,
            EvKeyCodes::KEY_KPSLASH => KeyCode::NumpadDivide,
            EvKeyCodes::KEY_SYSRQ => KeyCode::PrintScreen,
            EvKeyCodes::KEY_RIGHTALT => KeyCode::Alt,
            EvKeyCodes::KEY_HOME => KeyCode::Home,
            EvKeyCodes::KEY_UP => KeyCode::ArrowUp,
            EvKeyCodes::KEY_PAGEUP => KeyCode::PageUp,
            EvKeyCodes::KEY_LEFT => KeyCode::ArrowLeft,
            EvKeyCodes::KEY_RIGHT => KeyCode::ArrowRight,
            EvKeyCodes::KEY_END => KeyCode::End,
            EvKeyCodes::KEY_DOWN => KeyCode::ArrowDown,
            EvKeyCodes::KEY_PAGEDOWN => KeyCode::PageDown,
            EvKeyCodes::KEY_INSERT => KeyCode::Insert,
            EvKeyCodes::KEY_DELETE => KeyCode::Delete,
            EvKeyCodes::KEY_LEFTMETA => KeyCode::Logo,
            EvKeyCodes::KEY_RIGHTMETA => KeyCode::Logo,
            _ => KeyCode::Unknown,
        };
        if matches!(code, EvKeyCodes::KEY_LEFTSHIFT | EvKeyCodes::KEY_RIGHTSHIFT
            | EvKeyCodes::KEY_LEFTCTRL | EvKeyCodes::KEY_RIGHTCTRL
            | EvKeyCodes::KEY_LEFTALT | EvKeyCodes::KEY_RIGHTALT
            | EvKeyCodes::KEY_LEFTMETA | EvKeyCodes::KEY_RIGHTMETA) {
            if key_action == KeyAction::KEY_UP { self.modifier_keys.remove(&(device_id, code)); }
            else { self.modifier_keys.insert((device_id, code)); }
            self.update_modifiers();
        }
        if matches!(key_code, KeyCode::Logo | KeyCode::Space) {
            crate::trace!("input.keys", "direct key device={} code={} key={:?} action={} modifiers={:?}", device_id, code, key_code, key_action, self.modifiers);
        }
        match key_action {
            KeyAction::KEY_DOWN => {
                match code {
                    EvKeyCodes::BTN_LEFT
                    | EvKeyCodes::BTN_RIGHT
                    | EvKeyCodes::BTN_MIDDLE
                    | EvKeyCodes::BTN_SIDE
                    | EvKeyCodes::BTN_EXTRA => {
                        dir_evts.push(DirectEvent::MouseDown(MouseDownEvent {
                            button: MouseButton::from_raw_button(
                                (evt.code - EvKeyCodes::BTN_LEFT as u16) as usize,
                            ),
                            abs: self.abs,
                            window_id,
                            modifiers: self.modifiers,
                            time,
                            handled: Cell::new(Area::Empty),
                        }))
                    }
                    EvKeyCodes::BTN_TOUCH => {
                        dir_evts.push(DirectEvent::MouseDown(MouseDownEvent {
                            button: MouseButton::PRIMARY,
                            abs: self.abs,
                            window_id,
                            modifiers: self.modifiers,
                            time,
                            handled: Cell::new(Area::Empty),
                        }))
                    }
                    _ => {
                        if !self.modifiers.control && !self.modifiers.alt && !self.modifiers.logo {
                            let uc = self.modifiers.shift;
                            let inp = key_code.to_char(uc);
                            if let Some(inp) = inp {
                                dir_evts.push(DirectEvent::TextInput(TextInputEvent {
                                    input: format!("{}", inp),
                                    was_paste: false,
                                    replace_last: false,
                                    ..Default::default()
                                }));
                            }
                        }
                        dir_evts.push(DirectEvent::KeyDown(KeyEvent {
                            key_code,
                            is_repeat: false,
                            modifiers: self.modifiers,
                            time,
                        }))
                    }
                }
            }
            KeyAction::KEY_UP => {
                match code {
                    EvKeyCodes::BTN_LEFT
                    | EvKeyCodes::BTN_RIGHT
                    | EvKeyCodes::BTN_MIDDLE
                    | EvKeyCodes::BTN_SIDE
                    | EvKeyCodes::BTN_EXTRA => dir_evts.push(DirectEvent::MouseUp(MouseUpEvent {
                        button: MouseButton::from_raw_button(
                            (evt.code - EvKeyCodes::BTN_LEFT as u16) as usize,
                        ),
                        abs: self.abs,
                        window_id,
                        modifiers: self.modifiers,
                        time,
                    })),
                    EvKeyCodes::BTN_TOUCH => dir_evts.push(DirectEvent::MouseUp(MouseUpEvent {
                        button: MouseButton::PRIMARY,
                        abs: self.abs,
                        window_id,
                        modifiers: self.modifiers,
                        time,
                    })),
                    _ => dir_evts.push(DirectEvent::KeyUp(KeyEvent {
                        key_code,
                        is_repeat: false,
                        modifiers: self.modifiers,
                        time,
                    })),
                }
            }
            KeyAction::KEY_REPEAT => {
                if !self.modifiers.control && !self.modifiers.alt && !self.modifiers.logo {
                    let uc = self.modifiers.shift;
                    let inp = key_code.to_char(uc);
                    if let Some(inp) = inp {
                        dir_evts.push(DirectEvent::TextInput(TextInputEvent {
                            input: format!("{}", inp),
                            was_paste: false,
                            replace_last: false,
                            ..Default::default()
                        }));
                    }
                }
                dir_evts.push(DirectEvent::KeyDown(KeyEvent {
                    key_code,
                    is_repeat: true,
                    modifiers: self.modifiers,
                    time,
                }))
            }
            _ => (),
        }
    }
}
