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
enum EvSynCodes {
    SYN_REPORT          =0x00,
    SYN_CONFIG          =0x01,
    SYN_MT_REPORT       =0x02,
    SYN_DROPPED         =0x03,
    SYN_MAX             =0x0f,
    SYN_CNT             =EvSynCodes::SYN_MAX as u16 + 1,
}

#[allow(unused,non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvKeyCodes {
    KEY_RESERVED		    =0,
    KEY_ESC			        =1,
    KEY_1			        =2,
    KEY_2			        =3,
    KEY_3			        =4,
    KEY_4			        =5,
    KEY_5			        =6,
    KEY_6			        =7,
    KEY_7			        =8,
    KEY_8			        =9,
    KEY_9			        =10,
    KEY_0			        =11,
    KEY_MINUS		        =12,
    KEY_EQUAL		        =13,
    KEY_BACKSPACE		    =14,
    KEY_TAB			        =15,
    KEY_Q			        =16,
    KEY_W			        =17,
    KEY_E			        =18,
    KEY_R			        =19,
    KEY_T			        =20,
    KEY_Y			        =21,
    KEY_U			        =22,
    KEY_I			        =23,
    KEY_O			        =24,
    KEY_P			        =25,
    KEY_LEFTBRACE		    =26,
    KEY_RIGHTBRACE		    =27,
    KEY_ENTER		        =28,
    KEY_LEFTCTRL		    =29,
    KEY_A			        =30,
    KEY_S			        =31,
    KEY_D			        =32,
    KEY_F			        =33,
    KEY_G			        =34,
    KEY_H			        =35,
    KEY_J			        =36,
    KEY_K			        =37,
    KEY_L			        =38,
    KEY_SEMICOLON		    =39,
    KEY_APOSTROPHE		    =40,
    KEY_GRAVE		        =41,
    KEY_LEFTSHIFT		    =42,
    KEY_BACKSLASH		    =43,
    KEY_Z			        =44,
    KEY_X			        =45,
    KEY_C			        =46,
    KEY_V			        =47,
    KEY_B			        =48,
    KEY_N			        =49,
    KEY_M			        =50,
    KEY_COMMA		        =51,
    KEY_DOT			        =52,
    KEY_SLASH		        =53,
    KEY_RIGHTSHIFT		    =54,
    KEY_KPASTERISK		    =55,
    KEY_LEFTALT		        =56,
    KEY_SPACE		        =57,
    KEY_CAPSLOCK		    =58,
    KEY_F1			        =59,
    KEY_F2			        =60,
    KEY_F3			        =61,
    KEY_F4			        =62,
    KEY_F5			        =63,
    KEY_F6			        =64,
    KEY_F7			        =65,
    KEY_F8			        =66,
    KEY_F9			        =67,
    KEY_F10			        =68,
    KEY_NUMLOCK		        =69,
    KEY_SCROLLLOCK		    =70,
    KEY_KP7			        =71,
    KEY_KP8			        =72,
    KEY_KP9			        =73,
    KEY_KPMINUS		        =74,
    KEY_KP4			        =75,
    KEY_KP5			        =76,
    KEY_KP6			        =77,
    KEY_KPPLUS		        =78,
    KEY_KP1			        =79,
    KEY_KP2			        =80,
    KEY_KP3			        =81,
    KEY_KP0			        =82,
    KEY_KPDOT		        =83,
    KEY_ZENKAKUHANKAKU	    =85,
    KEY_102ND		        =86,
    KEY_F11			        =87,
    KEY_F12			        =88,
    KEY_RO			        =89,
    KEY_KATAKANA		    =90,
    KEY_HIRAGANA		    =91,
    KEY_HENKAN		        =92,
    KEY_KATAKANAHIRAGANA	=93,
    KEY_MUHENKAN		    =94,
    KEY_KPJPCOMMA		    =95,
    KEY_KPENTER		        =96,
    KEY_RIGHTCTRL		    =97,
    KEY_KPSLASH		        =98,
    KEY_SYSRQ		        =99,
    KEY_RIGHTALT		    =100,
    KEY_LINEFEED		    =101,
    KEY_HOME		        =102,
    KEY_UP			        =103,
    KEY_PAGEUP		        =104,
    KEY_LEFT		        =105,
    KEY_RIGHT		        =106,
    KEY_END			        =107,
    KEY_DOWN		        =108,
    KEY_PAGEDOWN		    =109,
    KEY_INSERT		        =110,
    KEY_DELETE		        =111,
    KEY_MACRO		        =112,
    KEY_MUTE		        =113,
    KEY_VOLUMEDOWN		    =114,
    KEY_VOLUMEUP		    =115,
    KEY_POWER		        =116,
    KEY_KPEQUAL		        =117,
    KEY_KPPLUSMINUS		    =118,
    KEY_PAUSE		        =119,
    KEY_SCALE		        =120,
    KEY_KPCOMMA		        =121,
    KEY_HANGEUL_HANGUEL     =122,
    KEY_HANJA		        =123,
    KEY_YEN			        =124,
    KEY_LEFTMETA		    =125,
    KEY_RIGHTMETA		    =126,
    KEY_COMPOSE		        =127,
    KEY_STOP		        =128,
    KEY_AGAIN		        =129,
    KEY_PROPS		        =130,
    KEY_UNDO		        =131,
    KEY_FRONT		        =132,
    KEY_COPY		        =133,
    KEY_OPEN		        =134,
    KEY_PASTE		        =135,
    KEY_FIND		        =136,
    KEY_CUT			        =137,
    KEY_HELP		        =138,
    KEY_MENU		        =139,
    KEY_CALC		        =140,
    KEY_SETUP		        =141,
    KEY_SLEEP		        =142,
    KEY_WAKEUP		        =143,
    KEY_FILE		        =144,
    KEY_SENDFILE		    =145,
    KEY_DELETEFILE		    =146,
    KEY_XFER		        =147,
    KEY_PROG1		        =148,
    KEY_PROG2		        =149,
    KEY_WWW			        =150,
    KEY_MSDOS		        =151,
    KEY_COFFEE_SCREENLOCK   =152,
    KEY_ROTATE_DISPLAY_DIRECTION=153,
    KEY_CYCLEWINDOWS	    =154,
    KEY_MAIL		        =155,
    KEY_BOOKMARKS		    =156,
    KEY_COMPUTER		    =157,
    KEY_BACK		        =158,
    KEY_FORWARD		        =159,
    KEY_CLOSECD		        =160,
    KEY_EJECTCD		        =161,
    KEY_EJECTCLOSECD	    =162,
    KEY_NEXTSONG		    =163,
    KEY_PLAYPAUSE		    =164,
    KEY_PREVIOUSSONG	    =165,
    KEY_STOPCD		        =166,
    KEY_RECORD		        =167,
    KEY_REWIND		        =168,
    KEY_PHONE		        =169,
    KEY_ISO			        =170,
    KEY_CONFIG		        =171,
    KEY_HOMEPAGE		    =172,
    KEY_REFRESH		        =173,
    KEY_EXIT		        =174,
    KEY_MOVE		        =175,
    KEY_EDIT		        =176,
    KEY_SCROLLUP		    =177,
    KEY_SCROLLDOWN		    =178,
    KEY_KPLEFTPAREN		    =179,
    KEY_KPRIGHTPAREN	    =180,
    KEY_NEW			        =181,
    KEY_REDO		        =182,
    KEY_F13			        =183,
    KEY_F14			        =184,
    KEY_F15			        =185,
    KEY_F16			        =186,
    KEY_F17			        =187,
    KEY_F18			        =188,
    KEY_F19			        =189,
    KEY_F20			        =190,
    KEY_F21			        =191,
    KEY_F22			        =192,
    KEY_F23			        =193,
    KEY_F24			        =194,
    KEY_PLAYCD		        =200,
    KEY_PAUSECD		        =201,
    KEY_PROG3		        =202,
    KEY_PROG4		        =203,
    KEY_ALL_APPLICATIONS_DASHBOARD	=204,
    KEY_SUSPEND		        =205,
    KEY_CLOSE		        =206,
    KEY_PLAY		        =207,
    KEY_FASTFORWARD		    =208,
    KEY_BASSBOOST		    =209,
    KEY_PRINT		        =210,
    KEY_HP			        =211,
    KEY_CAMERA		        =212,
    KEY_SOUND		        =213,
    KEY_QUESTION		    =214,
    KEY_EMAIL		        =215,
    KEY_CHAT		        =216,
    KEY_SEARCH		        =217,
    KEY_CONNECT		        =218,
    KEY_FINANCE		        =219,
    KEY_SPORT		        =220,
    KEY_SHOP		        =221,
    KEY_ALTERASE		    =222,
    KEY_CANCEL		        =223,
    KEY_BRIGHTNESSDOWN	    =224,
    KEY_BRIGHTNESSUP	    =225,
    KEY_MEDIA		        =226,
    KEY_SWITCHVIDEOMODE	    =227,
    KEY_KBDILLUMTOGGLE	    =228,
    KEY_KBDILLUMDOWN	    =229,
    KEY_KBDILLUMUP		    =230,
    KEY_SEND		        =231,
    KEY_REPLY		        =232,
    KEY_FORWARDMAIL		    =233,
    KEY_SAVE		        =234,
    KEY_DOCUMENTS		    =235,
    KEY_BATTERY		        =236,
    KEY_BLUETOOTH		    =237,
    KEY_WLAN		        =238,
    KEY_UWB			        =239,
    KEY_UNKNOWN		        =240,
    KEY_VIDEO_NEXT		    =241,
    KEY_VIDEO_PREV		    =242,
    KEY_BRIGHTNESS_CYCLE	=243,
    KEY_BRIGHTNESS_ZERO_AUTO=244,
    KEY_DISPLAY_OFF		    =245,
    KEY_WWAN_WIMAX	        =246,
    KEY_RFKILL		        =247,
    KEY_MICMUTE		        =248,
    BTN_0			        =0x100,
    BTN_1			        =0x101,
    BTN_2			        =0x102,
    BTN_3			        =0x103,
    BTN_4			        =0x104,
    BTN_5			        =0x105,
    BTN_6			        =0x106,
    BTN_7			        =0x107,
    BTN_8			        =0x108,
    BTN_9			        =0x109,
    BTN_LEFT		        =0x110,
    BTN_RIGHT		        =0x111,
    BTN_MIDDLE		        =0x112,
    BTN_SIDE		        =0x113,
    BTN_EXTRA		        =0x114,
    BTN_FORWARD		        =0x115,
    BTN_BACK		        =0x116,
    BTN_TASK		        =0x117,
    BTN_JOYSTICK		    =0x120,
    BTN_THUMB		        =0x121,
    BTN_THUMB2		        =0x122,
    BTN_TOP			        =0x123,
    BTN_TOP2		        =0x124,
    BTN_BASE		        =0x126,
    BTN_PINKIE		        =0x125,
    BTN_BASE2		        =0x127,
    BTN_BASE3		        =0x128,
    BTN_BASE4		        =0x129,
    BTN_BASE5		        =0x12a,
    BTN_BASE6		        =0x12b,
    BTN_DEAD		        =0x12f,
    BTN_SOUTH_A		        =0x130,
    BTN_EAST_B		        =0x131,
    BTN_C			        =0x132,
    BTN_NORTH_X		        =0x133,
    BTN_WEST_Y		        =0x134,
    BTN_Z			        =0x135,
    BTN_TL			        =0x136,
    BTN_TR			        =0x137,
    BTN_TL2			        =0x138,
    BTN_TR2			        =0x139,
    BTN_SELECT		        =0x13a,
    BTN_START		        =0x13b,
    BTN_MODE		        =0x13c,
    BTN_THUMBL		        =0x13d,
    BTN_THUMBR		        =0x13e,
    BTN_TOOL_PEN	    	=0x140,
    BTN_TOOL_RUBBER		    =0x141,
    BTN_TOOL_BRUSH		    =0x142,
    BTN_TOOL_PENCIL		    =0x143,
    BTN_TOOL_AIRBRUSH	    =0x144,
    BTN_TOOL_FINGER		    =0x145,
    BTN_TOOL_MOUSE		    =0x146,
    BTN_TOOL_LENS		    =0x147,
    BTN_TOOL_QUINTTAP	    =0x148,
    BTN_STYLUS3		        =0x149,
    BTN_TOUCH		        =0x14a,
    BTN_STYLUS		        =0x14b,
    BTN_STYLUS2		        =0x14c,
    BTN_TOOL_DOUBLETAP	    =0x14d,
    BTN_TOOL_TRIPLETAP	    =0x14e,
    BTN_TOOL_QUADTAP	    =0x14f,
    BTN_GEAR_DOWN		    =0x150,
    BTN_GEAR_UP		        =0x151,
    KEY_OK			        =0x160,
    KEY_SELECT		        =0x161,
    KEY_GOTO		        =0x162,
    KEY_CLEAR		        =0x163,
    KEY_POWER2		        =0x164,
    KEY_OPTION		        =0x165,
    KEY_INFO		        =0x166,
    KEY_TIME		        =0x167,
    KEY_VENDOR		        =0x168,
    KEY_ARCHIVE		        =0x169,
    KEY_PROGRAM		        =0x16a,
    KEY_CHANNEL		        =0x16b,
    KEY_FAVORITES		    =0x16c,
    KEY_EPG			        =0x16d,
    KEY_PVR			        =0x16e,
    KEY_MHP			        =0x16f,
    KEY_LANGUAGE		    =0x170,
    KEY_TITLE		        =0x171,
    KEY_SUBTITLE		    =0x172,
    KEY_ANGLE		        =0x173,
    KEY_FULL_SCREEN		    =0x174,
    KEY_MODE		        =0x175,
    KEY_KEYBOARD		    =0x176,
    KEY_ASPECT_RATIO	    =0x177,
    KEY_PC			        =0x178,
    KEY_TV			        =0x179,
    KEY_TV2			        =0x17a,
    KEY_VCR			        =0x17b,
    KEY_VCR2		        =0x17c,
    KEY_SAT			        =0x17d,
    KEY_SAT2		        =0x17e,
    KEY_CD			        =0x17f,
    KEY_TAPE		        =0x180,
    KEY_RADIO		        =0x181,
    KEY_TUNER		        =0x182,
    KEY_PLAYER		        =0x183,
    KEY_TEXT		        =0x184,
    KEY_DVD			        =0x185,
    KEY_AUX			        =0x186,
    KEY_MP3			        =0x187,
    KEY_AUDIO		        =0x188,
    KEY_VIDEO		        =0x189,
    KEY_DIRECTORY		    =0x18a,
    KEY_LIST		        =0x18b,
    KEY_MEMO		        =0x18c,
    KEY_CALENDAR		    =0x18d,
    KEY_RED			        =0x18e,
    KEY_GREEN		        =0x18f,
    KEY_YELLOW		        =0x190,
    KEY_BLUE		        =0x191,
    KEY_CHANNELUP		    =0x192,
    KEY_CHANNELDOWN		    =0x193,
    KEY_FIRST		        =0x194,
    KEY_LAST		        =0x195,
    KEY_AB			        =0x196,
    KEY_NEXT		        =0x197,
    KEY_RESTART		        =0x198,
    KEY_SLOW		        =0x199,
    KEY_SHUFFLE		        =0x19a,
    KEY_BREAK		        =0x19b,
    KEY_PREVIOUS		    =0x19c,
    KEY_DIGITS		        =0x19d,
    KEY_TEEN		        =0x19e,
    KEY_TWEN		        =0x19f,
    KEY_VIDEOPHONE		    =0x1a0,
    KEY_GAMES		        =0x1a1,
    KEY_ZOOMIN		        =0x1a2,
    KEY_ZOOMOUT		        =0x1a3,
    KEY_ZOOMRESET		    =0x1a4,
    KEY_WORDPROCESSOR	    =0x1a5,
    KEY_EDITOR		        =0x1a6,
    KEY_SPREADSHEET		    =0x1a7,
    KEY_GRAPHICSEDITOR	    =0x1a8,
    KEY_PRESENTATION	    =0x1a9,
    KEY_DATABASE		    =0x1aa,
    KEY_NEWS		        =0x1ab,
    KEY_VOICEMAIL		    =0x1ac,
    KEY_ADDRESSBOOK		    =0x1ad,
    KEY_MESSENGER		    =0x1ae,
    KEY_DISPLAYTOGGLE	    =0x1af,
    KEY_SPELLCHECK		    =0x1b0,
    KEY_LOGOFF		        =0x1b1,
    KEY_DOLLAR		        =0x1b2,
    KEY_EURO		        =0x1b3,
}

#[allow(unused,non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvRelCodes {
    REL_X			    =0x00,
    REL_Y			    =0x01,
    REL_Z			    =0x02,
    REL_RX			    =0x03,
    REL_RY			    =0x04,
    REL_RZ			    =0x05,
    REL_HWHEEL		    =0x06,
    REL_DIAL		    =0x07,
    REL_WHEEL		    =0x08,
    REL_MISC		    =0x09,
    REL_RESERVED		=0x0a,
    REL_WHEEL_HI_RES	=0x0b,
    REL_HWHEEL_HI_RES	=0x0c,
    REL_MAX			    =0x0f,
    REL_CNT			    =EvRelCodes::REL_MAX as u16 + 1,
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

#[allow(unused,non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvMscCodes {
    MSC_SERIAL		=0x00,
    MSC_PULSELED	=0x01,
    MSC_GESTURE		=0x02,
    MSC_RAW			=0x03,
    MSC_SCAN		=0x04,
    MSC_TIMESTAMP	=0x05,
    MSC_MAX			=0x07,
    MSC_CNT			=EvMscCodes::MSC_MAX as u16 + 1,
}

#[allow(unused,non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvSwCodes {
    SW_LID			        =0x00,  /* set = lid shut */
    SW_TABLET_MODE		    =0x01,  /* set = tablet mode */
    SW_HEADPHONE_INSERT	    =0x02,  /* set = inserted */
    SW_RFKILL_ALL_RADIO	    =0x03,  /* rfkill master switch, type "any" set = radio enabled */
    SW_MICROPHONE_INSERT	=0x04,  /* set = inserted */
    SW_DOCK			        =0x05,  /* set = plugged into dock */
    SW_LINEOUT_INSERT	    =0x06,  /* set = inserted */
    SW_JACK_PHYSICAL_INSERT =0x07,  /* set = mechanical switch set */
    SW_VIDEOOUT_INSERT	    =0x08,  /* set = inserted */
    SW_CAMERA_LENS_COVER	=0x09,  /* set = lens covered */
    SW_KEYPAD_SLIDE		    =0x0a,  /* set = keypad slide out */
    SW_FRONT_PROXIMITY	    =0x0b,  /* set = front proximity sensor active */
    SW_ROTATE_LOCK		    =0x0c,  /* set = rotate locked/disabled */
    SW_LINEIN_INSERT	    =0x0d,  /* set = inserted */
    SW_MUTE_DEVICE		    =0x0e,  /* set = device disabled */
    SW_PEN_INSERTED		    =0x0f,  /* set = pen inserted */
    SW_MACHINE_COVER	    =0x10,  /* set = cover closed */
    SW_CNT			        =EvSwCodes::SW_MACHINE_COVER as u16 + 1,
}

#[allow(unused,non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvLedCodes {
    LED_NUML		=0x00,
    LED_CAPSL		=0x01,
    LED_SCROLLL		=0x02,
    LED_COMPOSE		=0x03,
    LED_KANA		=0x04,
    LED_SLEEP		=0x05,
    LED_SUSPEND		=0x06,
    LED_MUTE		=0x07,
    LED_MISC		=0x08,
    LED_MAIL		=0x09,
    LED_CHARGING	=0x0a,
    LED_MAX			=0x0f,
    LED_CNT			=EvLedCodes::LED_MAX as u16 + 1,
}

#[allow(unused,non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvSndCodes {
    SND_CLICK		=0x00,
    SND_BELL		=0x01,
    SND_TONE		=0x02,
    SND_MAX			=0x07,
    SND_CNT			=EvSndCodes::SND_MAX as u16 + 1,
}

#[allow(unused,non_camel_case_types)]
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
enum EvRepCodes {
    REP_DELAY		=0x00,
    REP_PERIOD_MAX	=0x01,
    REP_CNT			=EvRepCodes::REP_PERIOD_MAX as u16 + 1,
}

#[allow(unused)]
#[repr(i32)]
#[derive(Clone, Copy, Debug)]
enum KeyAction {
    KeyUp		=0x00,
    KeyDown 	=0x01,
    KeyRepeat   =0x02,
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
    dpi_factor: f64,
    abs: DVec2,
}


impl RawInput {
    pub fn new(width: f64, height: f64, dpi_factor: f64) -> Self {
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
            dpi_factor,
            abs: dvec2(0.0, 0.0),
            modifiers: Default::default(),
        }
    }
    
    pub fn poll_raw_input(&mut self, time: f64, window_id: WindowId) -> Vec<DirectEvent> {
        let mut dir_evts: Vec<DirectEvent> = Vec::new();
        let mut evts: Vec<InputEvent> = Vec::new();
        loop {
            let new = match self.receiver.try_recv() {
                Ok(new) => new, //new event
                Err(err) => {
                    match err {
                        mpsc::TryRecvError::Empty =>  {
                            if evts.len()>0 {
                                continue; //partial message read that hasnt been cleared out, keep reading
                            } else {
                                break; //nothing to read
                            }
                        },
                        mpsc::TryRecvError::Disconnected => break //no input devices?
                    }
                }
            };
            match new.ty {
                InputEventType::EV_SYN => {
                    let code: EvSynCodes = unsafe { std::mem::transmute_copy(&new.code) };
                    match code {
                        EvSynCodes::SYN_REPORT => { //end of event reached
                            self.process_event(&mut evts, &mut dir_evts, time, window_id);
                        },
                        EvSynCodes::SYN_DROPPED => { //evdev client buffer overrun, ignore event till now and up untill the next SYN_REPORT
                            evts.clear();
                            while let Ok(dropped) = self.receiver.try_recv() {
                                match dropped.ty {
                                    InputEventType::EV_SYN => {
                                        if dropped.code == EvSynCodes::SYN_REPORT as u16 {
                                            break;
                                        }
                                    },
                                    _ => continue
                                }
                            }
                            continue;
                        },
                        _ => evts.push(new)
                    }
                }
                _ => {
                    evts.push(new);
                }
            }
        }
        dir_evts
    }

    fn process_event(&mut self, evts: &mut Vec<InputEvent>, dir_evts: &mut Vec<DirectEvent>, time: f64, window_id: WindowId) {
        while let Some(evt) = evts.pop() {
            match evt.ty {
                InputEventType::EV_REL => { // relative input
                    self.process_rel_event(evt, dir_evts, time, window_id)
                }
                InputEventType::EV_ABS => { // absolute input
                    self.process_abs_event(evt, dir_evts, time, window_id)
                }
                InputEventType::EV_KEY => { // key press
                    self.process_key_event(evt, dir_evts, time, window_id)
                }
                    _ => ()
            }
        }
    }

    fn process_rel_event(&mut self, evt: InputEvent, dir_evts: &mut Vec<DirectEvent>, time: f64, window_id: WindowId){
        let code: EvRelCodes = unsafe { std::mem::transmute(evt.code) };
        match code {
            EvRelCodes::REL_X => {
                self.abs.x += evt.value as f64;
                if self.abs.x < 0.0{ self.abs.x = 0.0}
                if self.abs.x > self.width{ self.abs.x = self.width}
                
            },
            EvRelCodes::REL_Y => {
                self.abs.y += evt.value as f64;
                if self.abs.y < 0.0{ self.abs.y = 0.0}
                if self.abs.y > self.height{ self.abs.y = self.height}
            },
            _ => return ()
        }
        dir_evts.push(DirectEvent::MouseMove(MouseMoveEvent {
            abs: self.abs,
            window_id,
            modifiers: self.modifiers,
            time,
            handled: Cell::new(Area::Empty),
        }))
    }

    fn process_abs_event(&mut self, evt: InputEvent, dir_evts: &mut Vec<DirectEvent>, time: f64, window_id: WindowId){
        let code: EvAbsCodes = unsafe { std::mem::transmute(evt.code) };
        match code {
            EvAbsCodes::ABS_X => {
                self.abs.x = (evt.value as f64 / 32767.0) * self.width;
            },
            EvAbsCodes::ABS_Y => {
                self.abs.y = (evt.value as f64 / 32767.0) * self.height;
            },
            EvAbsCodes::ABS_MT_POSITION_X => {
                self.abs.x = evt.value as f64 / self.dpi_factor; 
            },
            EvAbsCodes::ABS_MT_POSITION_Y => {
                self.abs.y = evt.value as f64 / self.dpi_factor;
            },
            _=> return ()
        }
        dir_evts.push(DirectEvent::MouseMove(MouseMoveEvent {
            abs: self.abs,
            window_id,
            modifiers: self.modifiers,
            time,
            handled: Cell::new(Area::Empty),
        }))
    }

    fn process_key_event(&mut self, evt: InputEvent, dir_evts: &mut Vec<DirectEvent>, time: f64, window_id: WindowId){
        let code: EvKeyCodes = unsafe { std::mem::transmute(evt.code) };
        let key_action: KeyAction = unsafe {std::mem::transmute(evt.value) };
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
        match key_action {
            KeyAction::KeyDown => {
                match key_code {
                    KeyCode::Shift => self.modifiers.shift = true,
                    KeyCode::Control => self.modifiers.control = true,
                    KeyCode::Logo => self.modifiers.logo = true,
                    KeyCode::Alt => self.modifiers.alt = true,
                    _ => ()
                };
                match code {
                    EvKeyCodes::BTN_LEFT | EvKeyCodes::BTN_RIGHT | EvKeyCodes::BTN_MIDDLE | EvKeyCodes::BTN_SIDE | EvKeyCodes::BTN_EXTRA => {
                        dir_evts.push(DirectEvent::MouseDown(MouseDownEvent {
                            button: MouseButton::from_raw_button((evt.code - EvKeyCodes::BTN_LEFT as u16) as usize),
                            abs: self.abs,
                            window_id,
                            modifiers: self.modifiers,
                            time,
                            handled: Cell::new(Area::Empty),
                        }))
                    },
                    EvKeyCodes::BTN_TOUCH => {
                        dir_evts.push(DirectEvent::MouseDown(MouseDownEvent {
                            button: MouseButton::PRIMARY,
                            abs: self.abs,
                            window_id,
                            modifiers: self.modifiers,
                            time,
                            handled: Cell::new(Area::Empty),
                        }))
                    },
                    _ => {
                        if !self.modifiers.control && !self.modifiers.alt && !self.modifiers.logo {
                            let uc = self.modifiers.shift;
                            let inp = key_code.to_char(uc);
                            if let Some(inp) = inp {
                                dir_evts.push(DirectEvent::TextInput(TextInputEvent {
                                    input: format!("{}", inp),
                                    was_paste: false,
                                    replace_last: false
                                }));
                            }
                        }
                        dir_evts.push(DirectEvent::KeyDown(KeyEvent {
                            key_code,
                            is_repeat: false,
                            modifiers: self.modifiers,
                            time
                        }))
                    }
                }
                
            },
            KeyAction::KeyUp => {
                match key_code {
                    KeyCode::Shift => self.modifiers.shift = false,
                    KeyCode::Control => self.modifiers.control = false,
                    KeyCode::Logo => self.modifiers.logo = false,
                    KeyCode::Alt => self.modifiers.alt = false,
                    _ => ()
                };
                match code {
                    EvKeyCodes::BTN_LEFT | EvKeyCodes::BTN_RIGHT | EvKeyCodes::BTN_MIDDLE | EvKeyCodes::BTN_SIDE | EvKeyCodes::BTN_EXTRA => {
                        dir_evts.push(DirectEvent::MouseUp(MouseUpEvent {
                            button: MouseButton::from_raw_button((evt.code - EvKeyCodes::BTN_LEFT as u16) as usize),
                            abs: self.abs,
                            window_id,
                            modifiers: self.modifiers,
                            time,
                        }))
                    },
                    EvKeyCodes::BTN_TOUCH => {
                        dir_evts.push(DirectEvent::MouseUp(MouseUpEvent {
                            button: MouseButton::PRIMARY,
                            abs: self.abs,
                            window_id,
                            modifiers: self.modifiers,
                            time,
                        }))
                    },
                    _ => {
                        dir_evts.push(DirectEvent::KeyUp(KeyEvent {
                            key_code,
                            is_repeat: false,
                            modifiers: self.modifiers,
                            time
                        }))
                    }
                }
            },
            KeyAction::KeyRepeat => {
                dir_evts.push(DirectEvent::KeyDown(KeyEvent {
                    key_code,
                    is_repeat: false,
                    modifiers: self.modifiers,
                    time
                }))
            }
        }
    }
}

