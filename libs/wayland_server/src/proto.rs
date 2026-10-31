//! Generated from protocol XML by `tools/wayland_codegen --server`.
//! Do not edit manually; regenerate instead.
#![allow(dead_code)]

pub mod wl_display {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_display", version: 1,
    requests: &[
        Msg { name: "sync", since: 1, destructor: false, args: &[ArgType::NewId("wl_callback")] },
        Msg { name: "get_registry", since: 1, destructor: false, args: &[ArgType::NewId("wl_registry")] },
    ],
    events: &[
        Msg { name: "error", since: 1, destructor: false, args: &[ArgType::Object(""), ArgType::Uint, ArgType::Str] },
        Msg { name: "delete_id", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_SYNC: u16 = 0;
    pub const REQ_GET_REGISTRY: u16 = 1;
    pub const EVT_ERROR: u16 = 0;
    pub const EVT_DELETE_ID: u16 = 1;
    pub const ERROR_INVALID_OBJECT: u32 = 0;
    pub const ERROR_INVALID_METHOD: u32 = 1;
    pub const ERROR_NO_MEMORY: u32 = 2;
    pub const ERROR_IMPLEMENTATION: u32 = 3;
}
pub mod wl_registry {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_registry", version: 1,
    requests: &[
        Msg { name: "bind", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::NewIdDyn] },
    ],
    events: &[
        Msg { name: "global", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Str, ArgType::Uint] },
        Msg { name: "global_remove", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_BIND: u16 = 0;
    pub const EVT_GLOBAL: u16 = 0;
    pub const EVT_GLOBAL_REMOVE: u16 = 1;
}
pub mod wl_callback {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_callback", version: 1,
    requests: &[
    ],
    events: &[
        Msg { name: "done", since: 1, destructor: true, args: &[ArgType::Uint] },
    ],
    };
    pub const EVT_DONE: u16 = 0;
}
pub mod wl_compositor {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_compositor", version: 6,
    requests: &[
        Msg { name: "create_surface", since: 1, destructor: false, args: &[ArgType::NewId("wl_surface")] },
        Msg { name: "create_region", since: 1, destructor: false, args: &[ArgType::NewId("wl_region")] },
    ],
    events: &[
    ],
    };
    pub const REQ_CREATE_SURFACE: u16 = 0;
    pub const REQ_CREATE_REGION: u16 = 1;
}
pub mod wl_shm_pool {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_shm_pool", version: 2,
    requests: &[
        Msg { name: "create_buffer", since: 1, destructor: false, args: &[ArgType::NewId("wl_buffer"), ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Uint] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "resize", since: 1, destructor: false, args: &[ArgType::Int] },
    ],
    events: &[
    ],
    };
    pub const REQ_CREATE_BUFFER: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
    pub const REQ_RESIZE: u16 = 2;
}
pub mod wl_shm {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_shm", version: 2,
    requests: &[
        Msg { name: "create_pool", since: 1, destructor: false, args: &[ArgType::NewId("wl_shm_pool"), ArgType::Fd, ArgType::Int] },
        Msg { name: "release", since: 2, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "format", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_CREATE_POOL: u16 = 0;
    pub const REQ_RELEASE: u16 = 1;
    pub const EVT_FORMAT: u16 = 0;
    pub const ERROR_INVALID_FORMAT: u32 = 0;
    pub const ERROR_INVALID_STRIDE: u32 = 1;
    pub const ERROR_INVALID_FD: u32 = 2;
    pub const FORMAT_ARGB8888: u32 = 0;
    pub const FORMAT_XRGB8888: u32 = 1;
    pub const FORMAT_C8: u32 = 538982467;
    pub const FORMAT_RGB332: u32 = 943867730;
    pub const FORMAT_BGR233: u32 = 944916290;
    pub const FORMAT_XRGB4444: u32 = 842093144;
    pub const FORMAT_XBGR4444: u32 = 842089048;
    pub const FORMAT_RGBX4444: u32 = 842094674;
    pub const FORMAT_BGRX4444: u32 = 842094658;
    pub const FORMAT_ARGB4444: u32 = 842093121;
    pub const FORMAT_ABGR4444: u32 = 842089025;
    pub const FORMAT_RGBA4444: u32 = 842088786;
    pub const FORMAT_BGRA4444: u32 = 842088770;
    pub const FORMAT_XRGB1555: u32 = 892424792;
    pub const FORMAT_XBGR1555: u32 = 892420696;
    pub const FORMAT_RGBX5551: u32 = 892426322;
    pub const FORMAT_BGRX5551: u32 = 892426306;
    pub const FORMAT_ARGB1555: u32 = 892424769;
    pub const FORMAT_ABGR1555: u32 = 892420673;
    pub const FORMAT_RGBA5551: u32 = 892420434;
    pub const FORMAT_BGRA5551: u32 = 892420418;
    pub const FORMAT_RGB565: u32 = 909199186;
    pub const FORMAT_BGR565: u32 = 909199170;
    pub const FORMAT_RGB888: u32 = 875710290;
    pub const FORMAT_BGR888: u32 = 875710274;
    pub const FORMAT_XBGR8888: u32 = 875709016;
    pub const FORMAT_RGBX8888: u32 = 875714642;
    pub const FORMAT_BGRX8888: u32 = 875714626;
    pub const FORMAT_ABGR8888: u32 = 875708993;
    pub const FORMAT_RGBA8888: u32 = 875708754;
    pub const FORMAT_BGRA8888: u32 = 875708738;
    pub const FORMAT_XRGB2101010: u32 = 808669784;
    pub const FORMAT_XBGR2101010: u32 = 808665688;
    pub const FORMAT_RGBX1010102: u32 = 808671314;
    pub const FORMAT_BGRX1010102: u32 = 808671298;
    pub const FORMAT_ARGB2101010: u32 = 808669761;
    pub const FORMAT_ABGR2101010: u32 = 808665665;
    pub const FORMAT_RGBA1010102: u32 = 808665426;
    pub const FORMAT_BGRA1010102: u32 = 808665410;
    pub const FORMAT_YUYV: u32 = 1448695129;
    pub const FORMAT_YVYU: u32 = 1431918169;
    pub const FORMAT_UYVY: u32 = 1498831189;
    pub const FORMAT_VYUY: u32 = 1498765654;
    pub const FORMAT_AYUV: u32 = 1448433985;
    pub const FORMAT_NV12: u32 = 842094158;
    pub const FORMAT_NV21: u32 = 825382478;
    pub const FORMAT_NV16: u32 = 909203022;
    pub const FORMAT_NV61: u32 = 825644622;
    pub const FORMAT_YUV410: u32 = 961959257;
    pub const FORMAT_YVU410: u32 = 961893977;
    pub const FORMAT_YUV411: u32 = 825316697;
    pub const FORMAT_YVU411: u32 = 825316953;
    pub const FORMAT_YUV420: u32 = 842093913;
    pub const FORMAT_YVU420: u32 = 842094169;
    pub const FORMAT_YUV422: u32 = 909202777;
    pub const FORMAT_YVU422: u32 = 909203033;
    pub const FORMAT_YUV444: u32 = 875713881;
    pub const FORMAT_YVU444: u32 = 875714137;
    pub const FORMAT_R8: u32 = 538982482;
    pub const FORMAT_R16: u32 = 540422482;
    pub const FORMAT_RG88: u32 = 943212370;
    pub const FORMAT_GR88: u32 = 943215175;
    pub const FORMAT_RG1616: u32 = 842221394;
    pub const FORMAT_GR1616: u32 = 842224199;
    pub const FORMAT_XRGB16161616F: u32 = 1211388504;
    pub const FORMAT_XBGR16161616F: u32 = 1211384408;
    pub const FORMAT_ARGB16161616F: u32 = 1211388481;
    pub const FORMAT_ABGR16161616F: u32 = 1211384385;
    pub const FORMAT_XYUV8888: u32 = 1448434008;
    pub const FORMAT_VUY888: u32 = 875713878;
    pub const FORMAT_VUY101010: u32 = 808670550;
    pub const FORMAT_Y210: u32 = 808530521;
    pub const FORMAT_Y212: u32 = 842084953;
    pub const FORMAT_Y216: u32 = 909193817;
    pub const FORMAT_Y410: u32 = 808531033;
    pub const FORMAT_Y412: u32 = 842085465;
    pub const FORMAT_Y416: u32 = 909194329;
    pub const FORMAT_XVYU2101010: u32 = 808670808;
    pub const FORMAT_XVYU12_16161616: u32 = 909334104;
    pub const FORMAT_XVYU16161616: u32 = 942954072;
    pub const FORMAT_Y0L0: u32 = 810299481;
    pub const FORMAT_X0L0: u32 = 810299480;
    pub const FORMAT_Y0L2: u32 = 843853913;
    pub const FORMAT_X0L2: u32 = 843853912;
    pub const FORMAT_YUV420_8BIT: u32 = 942691673;
    pub const FORMAT_YUV420_10BIT: u32 = 808539481;
    pub const FORMAT_XRGB8888_A8: u32 = 943805016;
    pub const FORMAT_XBGR8888_A8: u32 = 943800920;
    pub const FORMAT_RGBX8888_A8: u32 = 943806546;
    pub const FORMAT_BGRX8888_A8: u32 = 943806530;
    pub const FORMAT_RGB888_A8: u32 = 943798354;
    pub const FORMAT_BGR888_A8: u32 = 943798338;
    pub const FORMAT_RGB565_A8: u32 = 943797586;
    pub const FORMAT_BGR565_A8: u32 = 943797570;
    pub const FORMAT_NV24: u32 = 875714126;
    pub const FORMAT_NV42: u32 = 842290766;
    pub const FORMAT_P210: u32 = 808530512;
    pub const FORMAT_P010: u32 = 808530000;
    pub const FORMAT_P012: u32 = 842084432;
    pub const FORMAT_P016: u32 = 909193296;
    pub const FORMAT_AXBXGXRX106106106106: u32 = 808534593;
    pub const FORMAT_NV15: u32 = 892425806;
    pub const FORMAT_Q410: u32 = 808531025;
    pub const FORMAT_Q401: u32 = 825242705;
    pub const FORMAT_XRGB16161616: u32 = 942953048;
    pub const FORMAT_XBGR16161616: u32 = 942948952;
    pub const FORMAT_ARGB16161616: u32 = 942953025;
    pub const FORMAT_ABGR16161616: u32 = 942948929;
    pub const FORMAT_C1: u32 = 538980675;
    pub const FORMAT_C2: u32 = 538980931;
    pub const FORMAT_C4: u32 = 538981443;
    pub const FORMAT_D1: u32 = 538980676;
    pub const FORMAT_D2: u32 = 538980932;
    pub const FORMAT_D4: u32 = 538981444;
    pub const FORMAT_D8: u32 = 538982468;
    pub const FORMAT_R1: u32 = 538980690;
    pub const FORMAT_R2: u32 = 538980946;
    pub const FORMAT_R4: u32 = 538981458;
    pub const FORMAT_R10: u32 = 540029266;
    pub const FORMAT_R12: u32 = 540160338;
    pub const FORMAT_AVUY8888: u32 = 1498764865;
    pub const FORMAT_XVUY8888: u32 = 1498764888;
    pub const FORMAT_P030: u32 = 808661072;
}
pub mod wl_buffer {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_buffer", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "release", since: 1, destructor: false, args: &[] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const EVT_RELEASE: u16 = 0;
}
pub mod wl_data_offer {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_data_offer", version: 3,
    requests: &[
        Msg { name: "accept", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::StrOpt] },
        Msg { name: "receive", since: 1, destructor: false, args: &[ArgType::Str, ArgType::Fd] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "finish", since: 3, destructor: false, args: &[] },
        Msg { name: "set_actions", since: 3, destructor: false, args: &[ArgType::Uint, ArgType::Uint] },
    ],
    events: &[
        Msg { name: "offer", since: 1, destructor: false, args: &[ArgType::Str] },
        Msg { name: "source_actions", since: 3, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "action", since: 3, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_ACCEPT: u16 = 0;
    pub const REQ_RECEIVE: u16 = 1;
    pub const REQ_DESTROY: u16 = 2;
    pub const REQ_FINISH: u16 = 3;
    pub const REQ_SET_ACTIONS: u16 = 4;
    pub const EVT_OFFER: u16 = 0;
    pub const EVT_SOURCE_ACTIONS: u16 = 1;
    pub const EVT_ACTION: u16 = 2;
    pub const ERROR_INVALID_FINISH: u32 = 0;
    pub const ERROR_INVALID_ACTION_MASK: u32 = 1;
    pub const ERROR_INVALID_ACTION: u32 = 2;
    pub const ERROR_INVALID_OFFER: u32 = 3;
}
pub mod wl_data_source {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_data_source", version: 3,
    requests: &[
        Msg { name: "offer", since: 1, destructor: false, args: &[ArgType::Str] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "set_actions", since: 3, destructor: false, args: &[ArgType::Uint] },
    ],
    events: &[
        Msg { name: "target", since: 1, destructor: false, args: &[ArgType::StrOpt] },
        Msg { name: "send", since: 1, destructor: false, args: &[ArgType::Str, ArgType::Fd] },
        Msg { name: "cancelled", since: 1, destructor: false, args: &[] },
        Msg { name: "dnd_drop_performed", since: 3, destructor: false, args: &[] },
        Msg { name: "dnd_finished", since: 3, destructor: false, args: &[] },
        Msg { name: "action", since: 3, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_OFFER: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
    pub const REQ_SET_ACTIONS: u16 = 2;
    pub const EVT_TARGET: u16 = 0;
    pub const EVT_SEND: u16 = 1;
    pub const EVT_CANCELLED: u16 = 2;
    pub const EVT_DND_DROP_PERFORMED: u16 = 3;
    pub const EVT_DND_FINISHED: u16 = 4;
    pub const EVT_ACTION: u16 = 5;
    pub const ERROR_INVALID_ACTION_MASK: u32 = 0;
    pub const ERROR_INVALID_SOURCE: u32 = 1;
}
pub mod wl_data_device {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_data_device", version: 3,
    requests: &[
        Msg { name: "start_drag", since: 1, destructor: false, args: &[ArgType::ObjectOpt("wl_data_source"), ArgType::Object("wl_surface"), ArgType::ObjectOpt("wl_surface"), ArgType::Uint] },
        Msg { name: "set_selection", since: 1, destructor: false, args: &[ArgType::ObjectOpt("wl_data_source"), ArgType::Uint] },
        Msg { name: "release", since: 2, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "data_offer", since: 1, destructor: false, args: &[ArgType::NewId("wl_data_offer")] },
        Msg { name: "enter", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Object("wl_surface"), ArgType::Fixed, ArgType::Fixed, ArgType::ObjectOpt("wl_data_offer")] },
        Msg { name: "leave", since: 1, destructor: false, args: &[] },
        Msg { name: "motion", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "drop", since: 1, destructor: false, args: &[] },
        Msg { name: "selection", since: 1, destructor: false, args: &[ArgType::ObjectOpt("wl_data_offer")] },
    ],
    };
    pub const REQ_START_DRAG: u16 = 0;
    pub const REQ_SET_SELECTION: u16 = 1;
    pub const REQ_RELEASE: u16 = 2;
    pub const EVT_DATA_OFFER: u16 = 0;
    pub const EVT_ENTER: u16 = 1;
    pub const EVT_LEAVE: u16 = 2;
    pub const EVT_MOTION: u16 = 3;
    pub const EVT_DROP: u16 = 4;
    pub const EVT_SELECTION: u16 = 5;
    pub const ERROR_ROLE: u32 = 0;
    pub const ERROR_USED_SOURCE: u32 = 1;
}
pub mod wl_data_device_manager {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_data_device_manager", version: 3,
    requests: &[
        Msg { name: "create_data_source", since: 1, destructor: false, args: &[ArgType::NewId("wl_data_source")] },
        Msg { name: "get_data_device", since: 1, destructor: false, args: &[ArgType::NewId("wl_data_device"), ArgType::Object("wl_seat")] },
    ],
    events: &[
    ],
    };
    pub const REQ_CREATE_DATA_SOURCE: u16 = 0;
    pub const REQ_GET_DATA_DEVICE: u16 = 1;
    pub const DND_ACTION_NONE: u32 = 0;
    pub const DND_ACTION_COPY: u32 = 1;
    pub const DND_ACTION_MOVE: u32 = 2;
    pub const DND_ACTION_ASK: u32 = 4;
}
pub mod wl_shell {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_shell", version: 1,
    requests: &[
        Msg { name: "get_shell_surface", since: 1, destructor: false, args: &[ArgType::NewId("wl_shell_surface"), ArgType::Object("wl_surface")] },
    ],
    events: &[
    ],
    };
    pub const REQ_GET_SHELL_SURFACE: u16 = 0;
    pub const ERROR_ROLE: u32 = 0;
}
pub mod wl_shell_surface {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_shell_surface", version: 1,
    requests: &[
        Msg { name: "pong", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "move", since: 1, destructor: false, args: &[ArgType::Object("wl_seat"), ArgType::Uint] },
        Msg { name: "resize", since: 1, destructor: false, args: &[ArgType::Object("wl_seat"), ArgType::Uint, ArgType::Uint] },
        Msg { name: "set_toplevel", since: 1, destructor: false, args: &[] },
        Msg { name: "set_transient", since: 1, destructor: false, args: &[ArgType::Object("wl_surface"), ArgType::Int, ArgType::Int, ArgType::Uint] },
        Msg { name: "set_fullscreen", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::ObjectOpt("wl_output")] },
        Msg { name: "set_popup", since: 1, destructor: false, args: &[ArgType::Object("wl_seat"), ArgType::Uint, ArgType::Object("wl_surface"), ArgType::Int, ArgType::Int, ArgType::Uint] },
        Msg { name: "set_maximized", since: 1, destructor: false, args: &[ArgType::ObjectOpt("wl_output")] },
        Msg { name: "set_title", since: 1, destructor: false, args: &[ArgType::Str] },
        Msg { name: "set_class", since: 1, destructor: false, args: &[ArgType::Str] },
    ],
    events: &[
        Msg { name: "ping", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "configure", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Int, ArgType::Int] },
        Msg { name: "popup_done", since: 1, destructor: false, args: &[] },
    ],
    };
    pub const REQ_PONG: u16 = 0;
    pub const REQ_MOVE: u16 = 1;
    pub const REQ_RESIZE: u16 = 2;
    pub const REQ_SET_TOPLEVEL: u16 = 3;
    pub const REQ_SET_TRANSIENT: u16 = 4;
    pub const REQ_SET_FULLSCREEN: u16 = 5;
    pub const REQ_SET_POPUP: u16 = 6;
    pub const REQ_SET_MAXIMIZED: u16 = 7;
    pub const REQ_SET_TITLE: u16 = 8;
    pub const REQ_SET_CLASS: u16 = 9;
    pub const EVT_PING: u16 = 0;
    pub const EVT_CONFIGURE: u16 = 1;
    pub const EVT_POPUP_DONE: u16 = 2;
    pub const RESIZE_NONE: u32 = 0;
    pub const RESIZE_TOP: u32 = 1;
    pub const RESIZE_BOTTOM: u32 = 2;
    pub const RESIZE_LEFT: u32 = 4;
    pub const RESIZE_TOP_LEFT: u32 = 5;
    pub const RESIZE_BOTTOM_LEFT: u32 = 6;
    pub const RESIZE_RIGHT: u32 = 8;
    pub const RESIZE_TOP_RIGHT: u32 = 9;
    pub const RESIZE_BOTTOM_RIGHT: u32 = 10;
    pub const TRANSIENT_INACTIVE: u32 = 1;
    pub const FULLSCREEN_METHOD_DEFAULT: u32 = 0;
    pub const FULLSCREEN_METHOD_SCALE: u32 = 1;
    pub const FULLSCREEN_METHOD_DRIVER: u32 = 2;
    pub const FULLSCREEN_METHOD_FILL: u32 = 3;
}
pub mod wl_surface {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_surface", version: 6,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "attach", since: 1, destructor: false, args: &[ArgType::ObjectOpt("wl_buffer"), ArgType::Int, ArgType::Int] },
        Msg { name: "damage", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int] },
        Msg { name: "frame", since: 1, destructor: false, args: &[ArgType::NewId("wl_callback")] },
        Msg { name: "set_opaque_region", since: 1, destructor: false, args: &[ArgType::ObjectOpt("wl_region")] },
        Msg { name: "set_input_region", since: 1, destructor: false, args: &[ArgType::ObjectOpt("wl_region")] },
        Msg { name: "commit", since: 1, destructor: false, args: &[] },
        Msg { name: "set_buffer_transform", since: 2, destructor: false, args: &[ArgType::Int] },
        Msg { name: "set_buffer_scale", since: 3, destructor: false, args: &[ArgType::Int] },
        Msg { name: "damage_buffer", since: 4, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int] },
        Msg { name: "offset", since: 5, destructor: false, args: &[ArgType::Int, ArgType::Int] },
    ],
    events: &[
        Msg { name: "enter", since: 1, destructor: false, args: &[ArgType::Object("wl_output")] },
        Msg { name: "leave", since: 1, destructor: false, args: &[ArgType::Object("wl_output")] },
        Msg { name: "preferred_buffer_scale", since: 6, destructor: false, args: &[ArgType::Int] },
        Msg { name: "preferred_buffer_transform", since: 6, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_ATTACH: u16 = 1;
    pub const REQ_DAMAGE: u16 = 2;
    pub const REQ_FRAME: u16 = 3;
    pub const REQ_SET_OPAQUE_REGION: u16 = 4;
    pub const REQ_SET_INPUT_REGION: u16 = 5;
    pub const REQ_COMMIT: u16 = 6;
    pub const REQ_SET_BUFFER_TRANSFORM: u16 = 7;
    pub const REQ_SET_BUFFER_SCALE: u16 = 8;
    pub const REQ_DAMAGE_BUFFER: u16 = 9;
    pub const REQ_OFFSET: u16 = 10;
    pub const EVT_ENTER: u16 = 0;
    pub const EVT_LEAVE: u16 = 1;
    pub const EVT_PREFERRED_BUFFER_SCALE: u16 = 2;
    pub const EVT_PREFERRED_BUFFER_TRANSFORM: u16 = 3;
    pub const ERROR_INVALID_SCALE: u32 = 0;
    pub const ERROR_INVALID_TRANSFORM: u32 = 1;
    pub const ERROR_INVALID_SIZE: u32 = 2;
    pub const ERROR_INVALID_OFFSET: u32 = 3;
    pub const ERROR_DEFUNCT_ROLE_OBJECT: u32 = 4;
}
pub mod wl_seat {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_seat", version: 10,
    requests: &[
        Msg { name: "get_pointer", since: 1, destructor: false, args: &[ArgType::NewId("wl_pointer")] },
        Msg { name: "get_keyboard", since: 1, destructor: false, args: &[ArgType::NewId("wl_keyboard")] },
        Msg { name: "get_touch", since: 1, destructor: false, args: &[ArgType::NewId("wl_touch")] },
        Msg { name: "release", since: 5, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "capabilities", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "name", since: 2, destructor: false, args: &[ArgType::Str] },
    ],
    };
    pub const REQ_GET_POINTER: u16 = 0;
    pub const REQ_GET_KEYBOARD: u16 = 1;
    pub const REQ_GET_TOUCH: u16 = 2;
    pub const REQ_RELEASE: u16 = 3;
    pub const EVT_CAPABILITIES: u16 = 0;
    pub const EVT_NAME: u16 = 1;
    pub const CAPABILITY_POINTER: u32 = 1;
    pub const CAPABILITY_KEYBOARD: u32 = 2;
    pub const CAPABILITY_TOUCH: u32 = 4;
    pub const ERROR_MISSING_CAPABILITY: u32 = 0;
}
pub mod wl_pointer {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_pointer", version: 10,
    requests: &[
        Msg { name: "set_cursor", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::ObjectOpt("wl_surface"), ArgType::Int, ArgType::Int] },
        Msg { name: "release", since: 3, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "enter", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Object("wl_surface"), ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "leave", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Object("wl_surface")] },
        Msg { name: "motion", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "button", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Uint, ArgType::Uint] },
        Msg { name: "axis", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Fixed] },
        Msg { name: "frame", since: 5, destructor: false, args: &[] },
        Msg { name: "axis_source", since: 5, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "axis_stop", since: 5, destructor: false, args: &[ArgType::Uint, ArgType::Uint] },
        Msg { name: "axis_discrete", since: 5, destructor: false, args: &[ArgType::Uint, ArgType::Int] },
        Msg { name: "axis_value120", since: 8, destructor: false, args: &[ArgType::Uint, ArgType::Int] },
        Msg { name: "axis_relative_direction", since: 9, destructor: false, args: &[ArgType::Uint, ArgType::Uint] },
    ],
    };
    pub const REQ_SET_CURSOR: u16 = 0;
    pub const REQ_RELEASE: u16 = 1;
    pub const EVT_ENTER: u16 = 0;
    pub const EVT_LEAVE: u16 = 1;
    pub const EVT_MOTION: u16 = 2;
    pub const EVT_BUTTON: u16 = 3;
    pub const EVT_AXIS: u16 = 4;
    pub const EVT_FRAME: u16 = 5;
    pub const EVT_AXIS_SOURCE: u16 = 6;
    pub const EVT_AXIS_STOP: u16 = 7;
    pub const EVT_AXIS_DISCRETE: u16 = 8;
    pub const EVT_AXIS_VALUE120: u16 = 9;
    pub const EVT_AXIS_RELATIVE_DIRECTION: u16 = 10;
    pub const ERROR_ROLE: u32 = 0;
    pub const BUTTON_STATE_RELEASED: u32 = 0;
    pub const BUTTON_STATE_PRESSED: u32 = 1;
    pub const AXIS_VERTICAL_SCROLL: u32 = 0;
    pub const AXIS_HORIZONTAL_SCROLL: u32 = 1;
    pub const AXIS_SOURCE_WHEEL: u32 = 0;
    pub const AXIS_SOURCE_FINGER: u32 = 1;
    pub const AXIS_SOURCE_CONTINUOUS: u32 = 2;
    pub const AXIS_SOURCE_WHEEL_TILT: u32 = 3;
    pub const AXIS_RELATIVE_DIRECTION_IDENTICAL: u32 = 0;
    pub const AXIS_RELATIVE_DIRECTION_INVERTED: u32 = 1;
}
pub mod wl_keyboard {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_keyboard", version: 10,
    requests: &[
        Msg { name: "release", since: 3, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "keymap", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Fd, ArgType::Uint] },
        Msg { name: "enter", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Object("wl_surface"), ArgType::Array] },
        Msg { name: "leave", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Object("wl_surface")] },
        Msg { name: "key", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Uint, ArgType::Uint] },
        Msg { name: "modifiers", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Uint, ArgType::Uint, ArgType::Uint] },
        Msg { name: "repeat_info", since: 4, destructor: false, args: &[ArgType::Int, ArgType::Int] },
    ],
    };
    pub const REQ_RELEASE: u16 = 0;
    pub const EVT_KEYMAP: u16 = 0;
    pub const EVT_ENTER: u16 = 1;
    pub const EVT_LEAVE: u16 = 2;
    pub const EVT_KEY: u16 = 3;
    pub const EVT_MODIFIERS: u16 = 4;
    pub const EVT_REPEAT_INFO: u16 = 5;
    pub const KEYMAP_FORMAT_NO_KEYMAP: u32 = 0;
    pub const KEYMAP_FORMAT_XKB_V1: u32 = 1;
    pub const KEY_STATE_RELEASED: u32 = 0;
    pub const KEY_STATE_PRESSED: u32 = 1;
    pub const KEY_STATE_REPEATED: u32 = 2;
}
pub mod wl_touch {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_touch", version: 10,
    requests: &[
        Msg { name: "release", since: 3, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "down", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Object("wl_surface"), ArgType::Int, ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "up", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Int] },
        Msg { name: "motion", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Int, ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "frame", since: 1, destructor: false, args: &[] },
        Msg { name: "cancel", since: 1, destructor: false, args: &[] },
        Msg { name: "shape", since: 6, destructor: false, args: &[ArgType::Int, ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "orientation", since: 6, destructor: false, args: &[ArgType::Int, ArgType::Fixed] },
    ],
    };
    pub const REQ_RELEASE: u16 = 0;
    pub const EVT_DOWN: u16 = 0;
    pub const EVT_UP: u16 = 1;
    pub const EVT_MOTION: u16 = 2;
    pub const EVT_FRAME: u16 = 3;
    pub const EVT_CANCEL: u16 = 4;
    pub const EVT_SHAPE: u16 = 5;
    pub const EVT_ORIENTATION: u16 = 6;
}
pub mod wl_output {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_output", version: 4,
    requests: &[
        Msg { name: "release", since: 3, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "geometry", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Str, ArgType::Str, ArgType::Int] },
        Msg { name: "mode", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Int, ArgType::Int, ArgType::Int] },
        Msg { name: "done", since: 2, destructor: false, args: &[] },
        Msg { name: "scale", since: 2, destructor: false, args: &[ArgType::Int] },
        Msg { name: "name", since: 4, destructor: false, args: &[ArgType::Str] },
        Msg { name: "description", since: 4, destructor: false, args: &[ArgType::Str] },
    ],
    };
    pub const REQ_RELEASE: u16 = 0;
    pub const EVT_GEOMETRY: u16 = 0;
    pub const EVT_MODE: u16 = 1;
    pub const EVT_DONE: u16 = 2;
    pub const EVT_SCALE: u16 = 3;
    pub const EVT_NAME: u16 = 4;
    pub const EVT_DESCRIPTION: u16 = 5;
    pub const SUBPIXEL_UNKNOWN: u32 = 0;
    pub const SUBPIXEL_NONE: u32 = 1;
    pub const SUBPIXEL_HORIZONTAL_RGB: u32 = 2;
    pub const SUBPIXEL_HORIZONTAL_BGR: u32 = 3;
    pub const SUBPIXEL_VERTICAL_RGB: u32 = 4;
    pub const SUBPIXEL_VERTICAL_BGR: u32 = 5;
    pub const TRANSFORM_NORMAL: u32 = 0;
    pub const TRANSFORM_90: u32 = 1;
    pub const TRANSFORM_180: u32 = 2;
    pub const TRANSFORM_270: u32 = 3;
    pub const TRANSFORM_FLIPPED: u32 = 4;
    pub const TRANSFORM_FLIPPED_90: u32 = 5;
    pub const TRANSFORM_FLIPPED_180: u32 = 6;
    pub const TRANSFORM_FLIPPED_270: u32 = 7;
    pub const MODE_CURRENT: u32 = 1;
    pub const MODE_PREFERRED: u32 = 2;
}
pub mod wl_region {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_region", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "add", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int] },
        Msg { name: "subtract", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_ADD: u16 = 1;
    pub const REQ_SUBTRACT: u16 = 2;
}
pub mod wl_subcompositor {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_subcompositor", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "get_subsurface", since: 1, destructor: false, args: &[ArgType::NewId("wl_subsurface"), ArgType::Object("wl_surface"), ArgType::Object("wl_surface")] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_GET_SUBSURFACE: u16 = 1;
    pub const ERROR_BAD_SURFACE: u32 = 0;
    pub const ERROR_BAD_PARENT: u32 = 1;
}
pub mod wl_subsurface {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_subsurface", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "set_position", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int] },
        Msg { name: "place_above", since: 1, destructor: false, args: &[ArgType::Object("wl_surface")] },
        Msg { name: "place_below", since: 1, destructor: false, args: &[ArgType::Object("wl_surface")] },
        Msg { name: "set_sync", since: 1, destructor: false, args: &[] },
        Msg { name: "set_desync", since: 1, destructor: false, args: &[] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_SET_POSITION: u16 = 1;
    pub const REQ_PLACE_ABOVE: u16 = 2;
    pub const REQ_PLACE_BELOW: u16 = 3;
    pub const REQ_SET_SYNC: u16 = 4;
    pub const REQ_SET_DESYNC: u16 = 5;
    pub const ERROR_BAD_SURFACE: u32 = 0;
}
pub mod wl_fixes {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wl_fixes", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "destroy_registry", since: 1, destructor: false, args: &[ArgType::Object("wl_registry")] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_DESTROY_REGISTRY: u16 = 1;
}
pub mod xdg_wm_base {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "xdg_wm_base", version: 7,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "create_positioner", since: 1, destructor: false, args: &[ArgType::NewId("xdg_positioner")] },
        Msg { name: "get_xdg_surface", since: 1, destructor: false, args: &[ArgType::NewId("xdg_surface"), ArgType::Object("wl_surface")] },
        Msg { name: "pong", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    events: &[
        Msg { name: "ping", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_CREATE_POSITIONER: u16 = 1;
    pub const REQ_GET_XDG_SURFACE: u16 = 2;
    pub const REQ_PONG: u16 = 3;
    pub const EVT_PING: u16 = 0;
    pub const ERROR_ROLE: u32 = 0;
    pub const ERROR_DEFUNCT_SURFACES: u32 = 1;
    pub const ERROR_NOT_THE_TOPMOST_POPUP: u32 = 2;
    pub const ERROR_INVALID_POPUP_PARENT: u32 = 3;
    pub const ERROR_INVALID_SURFACE_STATE: u32 = 4;
    pub const ERROR_INVALID_POSITIONER: u32 = 5;
    pub const ERROR_UNRESPONSIVE: u32 = 6;
}
pub mod xdg_positioner {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "xdg_positioner", version: 7,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "set_size", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int] },
        Msg { name: "set_anchor_rect", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int] },
        Msg { name: "set_anchor", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "set_gravity", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "set_constraint_adjustment", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "set_offset", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int] },
        Msg { name: "set_reactive", since: 3, destructor: false, args: &[] },
        Msg { name: "set_parent_size", since: 3, destructor: false, args: &[ArgType::Int, ArgType::Int] },
        Msg { name: "set_parent_configure", since: 3, destructor: false, args: &[ArgType::Uint] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_SET_SIZE: u16 = 1;
    pub const REQ_SET_ANCHOR_RECT: u16 = 2;
    pub const REQ_SET_ANCHOR: u16 = 3;
    pub const REQ_SET_GRAVITY: u16 = 4;
    pub const REQ_SET_CONSTRAINT_ADJUSTMENT: u16 = 5;
    pub const REQ_SET_OFFSET: u16 = 6;
    pub const REQ_SET_REACTIVE: u16 = 7;
    pub const REQ_SET_PARENT_SIZE: u16 = 8;
    pub const REQ_SET_PARENT_CONFIGURE: u16 = 9;
    pub const ERROR_INVALID_INPUT: u32 = 0;
    pub const ANCHOR_NONE: u32 = 0;
    pub const ANCHOR_TOP: u32 = 1;
    pub const ANCHOR_BOTTOM: u32 = 2;
    pub const ANCHOR_LEFT: u32 = 3;
    pub const ANCHOR_RIGHT: u32 = 4;
    pub const ANCHOR_TOP_LEFT: u32 = 5;
    pub const ANCHOR_BOTTOM_LEFT: u32 = 6;
    pub const ANCHOR_TOP_RIGHT: u32 = 7;
    pub const ANCHOR_BOTTOM_RIGHT: u32 = 8;
    pub const GRAVITY_NONE: u32 = 0;
    pub const GRAVITY_TOP: u32 = 1;
    pub const GRAVITY_BOTTOM: u32 = 2;
    pub const GRAVITY_LEFT: u32 = 3;
    pub const GRAVITY_RIGHT: u32 = 4;
    pub const GRAVITY_TOP_LEFT: u32 = 5;
    pub const GRAVITY_BOTTOM_LEFT: u32 = 6;
    pub const GRAVITY_TOP_RIGHT: u32 = 7;
    pub const GRAVITY_BOTTOM_RIGHT: u32 = 8;
    pub const CONSTRAINT_ADJUSTMENT_NONE: u32 = 0;
    pub const CONSTRAINT_ADJUSTMENT_SLIDE_X: u32 = 1;
    pub const CONSTRAINT_ADJUSTMENT_SLIDE_Y: u32 = 2;
    pub const CONSTRAINT_ADJUSTMENT_FLIP_X: u32 = 4;
    pub const CONSTRAINT_ADJUSTMENT_FLIP_Y: u32 = 8;
    pub const CONSTRAINT_ADJUSTMENT_RESIZE_X: u32 = 16;
    pub const CONSTRAINT_ADJUSTMENT_RESIZE_Y: u32 = 32;
}
pub mod xdg_surface {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "xdg_surface", version: 7,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "get_toplevel", since: 1, destructor: false, args: &[ArgType::NewId("xdg_toplevel")] },
        Msg { name: "get_popup", since: 1, destructor: false, args: &[ArgType::NewId("xdg_popup"), ArgType::ObjectOpt("xdg_surface"), ArgType::Object("xdg_positioner")] },
        Msg { name: "set_window_geometry", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int] },
        Msg { name: "ack_configure", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    events: &[
        Msg { name: "configure", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_GET_TOPLEVEL: u16 = 1;
    pub const REQ_GET_POPUP: u16 = 2;
    pub const REQ_SET_WINDOW_GEOMETRY: u16 = 3;
    pub const REQ_ACK_CONFIGURE: u16 = 4;
    pub const EVT_CONFIGURE: u16 = 0;
    pub const ERROR_NOT_CONSTRUCTED: u32 = 1;
    pub const ERROR_ALREADY_CONSTRUCTED: u32 = 2;
    pub const ERROR_UNCONFIGURED_BUFFER: u32 = 3;
    pub const ERROR_INVALID_SERIAL: u32 = 4;
    pub const ERROR_INVALID_SIZE: u32 = 5;
    pub const ERROR_DEFUNCT_ROLE_OBJECT: u32 = 6;
}
pub mod xdg_toplevel {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "xdg_toplevel", version: 7,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "set_parent", since: 1, destructor: false, args: &[ArgType::ObjectOpt("xdg_toplevel")] },
        Msg { name: "set_title", since: 1, destructor: false, args: &[ArgType::Str] },
        Msg { name: "set_app_id", since: 1, destructor: false, args: &[ArgType::Str] },
        Msg { name: "show_window_menu", since: 1, destructor: false, args: &[ArgType::Object("wl_seat"), ArgType::Uint, ArgType::Int, ArgType::Int] },
        Msg { name: "move", since: 1, destructor: false, args: &[ArgType::Object("wl_seat"), ArgType::Uint] },
        Msg { name: "resize", since: 1, destructor: false, args: &[ArgType::Object("wl_seat"), ArgType::Uint, ArgType::Uint] },
        Msg { name: "set_max_size", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int] },
        Msg { name: "set_min_size", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int] },
        Msg { name: "set_maximized", since: 1, destructor: false, args: &[] },
        Msg { name: "unset_maximized", since: 1, destructor: false, args: &[] },
        Msg { name: "set_fullscreen", since: 1, destructor: false, args: &[ArgType::ObjectOpt("wl_output")] },
        Msg { name: "unset_fullscreen", since: 1, destructor: false, args: &[] },
        Msg { name: "set_minimized", since: 1, destructor: false, args: &[] },
    ],
    events: &[
        Msg { name: "configure", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Array] },
        Msg { name: "close", since: 1, destructor: false, args: &[] },
        Msg { name: "configure_bounds", since: 4, destructor: false, args: &[ArgType::Int, ArgType::Int] },
        Msg { name: "wm_capabilities", since: 5, destructor: false, args: &[ArgType::Array] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_SET_PARENT: u16 = 1;
    pub const REQ_SET_TITLE: u16 = 2;
    pub const REQ_SET_APP_ID: u16 = 3;
    pub const REQ_SHOW_WINDOW_MENU: u16 = 4;
    pub const REQ_MOVE: u16 = 5;
    pub const REQ_RESIZE: u16 = 6;
    pub const REQ_SET_MAX_SIZE: u16 = 7;
    pub const REQ_SET_MIN_SIZE: u16 = 8;
    pub const REQ_SET_MAXIMIZED: u16 = 9;
    pub const REQ_UNSET_MAXIMIZED: u16 = 10;
    pub const REQ_SET_FULLSCREEN: u16 = 11;
    pub const REQ_UNSET_FULLSCREEN: u16 = 12;
    pub const REQ_SET_MINIMIZED: u16 = 13;
    pub const EVT_CONFIGURE: u16 = 0;
    pub const EVT_CLOSE: u16 = 1;
    pub const EVT_CONFIGURE_BOUNDS: u16 = 2;
    pub const EVT_WM_CAPABILITIES: u16 = 3;
    pub const ERROR_INVALID_RESIZE_EDGE: u32 = 0;
    pub const ERROR_INVALID_PARENT: u32 = 1;
    pub const ERROR_INVALID_SIZE: u32 = 2;
    pub const RESIZE_EDGE_NONE: u32 = 0;
    pub const RESIZE_EDGE_TOP: u32 = 1;
    pub const RESIZE_EDGE_BOTTOM: u32 = 2;
    pub const RESIZE_EDGE_LEFT: u32 = 4;
    pub const RESIZE_EDGE_TOP_LEFT: u32 = 5;
    pub const RESIZE_EDGE_BOTTOM_LEFT: u32 = 6;
    pub const RESIZE_EDGE_RIGHT: u32 = 8;
    pub const RESIZE_EDGE_TOP_RIGHT: u32 = 9;
    pub const RESIZE_EDGE_BOTTOM_RIGHT: u32 = 10;
    pub const STATE_MAXIMIZED: u32 = 1;
    pub const STATE_FULLSCREEN: u32 = 2;
    pub const STATE_RESIZING: u32 = 3;
    pub const STATE_ACTIVATED: u32 = 4;
    pub const STATE_TILED_LEFT: u32 = 5;
    pub const STATE_TILED_RIGHT: u32 = 6;
    pub const STATE_TILED_TOP: u32 = 7;
    pub const STATE_TILED_BOTTOM: u32 = 8;
    pub const STATE_SUSPENDED: u32 = 9;
    pub const STATE_CONSTRAINED_LEFT: u32 = 10;
    pub const STATE_CONSTRAINED_RIGHT: u32 = 11;
    pub const STATE_CONSTRAINED_TOP: u32 = 12;
    pub const STATE_CONSTRAINED_BOTTOM: u32 = 13;
    pub const WM_CAPABILITIES_WINDOW_MENU: u32 = 1;
    pub const WM_CAPABILITIES_MAXIMIZE: u32 = 2;
    pub const WM_CAPABILITIES_FULLSCREEN: u32 = 3;
    pub const WM_CAPABILITIES_MINIMIZE: u32 = 4;
}
pub mod xdg_popup {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "xdg_popup", version: 7,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "grab", since: 1, destructor: false, args: &[ArgType::Object("wl_seat"), ArgType::Uint] },
        Msg { name: "reposition", since: 3, destructor: false, args: &[ArgType::Object("xdg_positioner"), ArgType::Uint] },
    ],
    events: &[
        Msg { name: "configure", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int] },
        Msg { name: "popup_done", since: 1, destructor: false, args: &[] },
        Msg { name: "repositioned", since: 3, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_GRAB: u16 = 1;
    pub const REQ_REPOSITION: u16 = 2;
    pub const EVT_CONFIGURE: u16 = 0;
    pub const EVT_POPUP_DONE: u16 = 1;
    pub const EVT_REPOSITIONED: u16 = 2;
    pub const ERROR_INVALID_GRAB: u32 = 0;
}
pub mod wp_viewporter {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wp_viewporter", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "get_viewport", since: 1, destructor: false, args: &[ArgType::NewId("wp_viewport"), ArgType::Object("wl_surface")] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_GET_VIEWPORT: u16 = 1;
    pub const ERROR_VIEWPORT_EXISTS: u32 = 0;
}
pub mod wp_viewport {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wp_viewport", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "set_source", since: 1, destructor: false, args: &[ArgType::Fixed, ArgType::Fixed, ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "set_destination", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_SET_SOURCE: u16 = 1;
    pub const REQ_SET_DESTINATION: u16 = 2;
    pub const ERROR_BAD_VALUE: u32 = 0;
    pub const ERROR_BAD_SIZE: u32 = 1;
    pub const ERROR_OUT_OF_BUFFER: u32 = 2;
    pub const ERROR_NO_SURFACE: u32 = 3;
}
pub mod zwp_linux_dmabuf_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_linux_dmabuf_v1", version: 4,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "create_params", since: 1, destructor: false, args: &[ArgType::NewId("zwp_linux_buffer_params_v1")] },
        Msg { name: "get_default_feedback", since: 4, destructor: false, args: &[ArgType::NewId("zwp_linux_dmabuf_feedback_v1")] },
        Msg { name: "get_surface_feedback", since: 4, destructor: false, args: &[ArgType::NewId("zwp_linux_dmabuf_feedback_v1"), ArgType::Object("wl_surface")] },
    ],
    events: &[
        Msg { name: "format", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "modifier", since: 3, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Uint] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_CREATE_PARAMS: u16 = 1;
    pub const REQ_GET_DEFAULT_FEEDBACK: u16 = 2;
    pub const REQ_GET_SURFACE_FEEDBACK: u16 = 3;
    pub const EVT_FORMAT: u16 = 0;
    pub const EVT_MODIFIER: u16 = 1;
}
pub mod zwp_linux_buffer_params_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_linux_buffer_params_v1", version: 4,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "add", since: 1, destructor: false, args: &[ArgType::Fd, ArgType::Uint, ArgType::Uint, ArgType::Uint, ArgType::Uint, ArgType::Uint] },
        Msg { name: "create", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Uint, ArgType::Uint] },
        Msg { name: "create_immed", since: 2, destructor: false, args: &[ArgType::NewId("wl_buffer"), ArgType::Int, ArgType::Int, ArgType::Uint, ArgType::Uint] },
    ],
    events: &[
        Msg { name: "created", since: 1, destructor: false, args: &[ArgType::NewId("wl_buffer")] },
        Msg { name: "failed", since: 1, destructor: false, args: &[] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_ADD: u16 = 1;
    pub const REQ_CREATE: u16 = 2;
    pub const REQ_CREATE_IMMED: u16 = 3;
    pub const EVT_CREATED: u16 = 0;
    pub const EVT_FAILED: u16 = 1;
    pub const ERROR_ALREADY_USED: u32 = 0;
    pub const ERROR_PLANE_IDX: u32 = 1;
    pub const ERROR_PLANE_SET: u32 = 2;
    pub const ERROR_INCOMPLETE: u32 = 3;
    pub const ERROR_INVALID_FORMAT: u32 = 4;
    pub const ERROR_INVALID_DIMENSIONS: u32 = 5;
    pub const ERROR_OUT_OF_BOUNDS: u32 = 6;
    pub const ERROR_INVALID_WL_BUFFER: u32 = 7;
    pub const FLAGS_Y_INVERT: u32 = 1;
    pub const FLAGS_INTERLACED: u32 = 2;
    pub const FLAGS_BOTTOM_FIRST: u32 = 4;
}
pub mod zwp_linux_dmabuf_feedback_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_linux_dmabuf_feedback_v1", version: 4,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "done", since: 1, destructor: false, args: &[] },
        Msg { name: "format_table", since: 1, destructor: false, args: &[ArgType::Fd, ArgType::Uint] },
        Msg { name: "main_device", since: 1, destructor: false, args: &[ArgType::Array] },
        Msg { name: "tranche_done", since: 1, destructor: false, args: &[] },
        Msg { name: "tranche_target_device", since: 1, destructor: false, args: &[ArgType::Array] },
        Msg { name: "tranche_formats", since: 1, destructor: false, args: &[ArgType::Array] },
        Msg { name: "tranche_flags", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const EVT_DONE: u16 = 0;
    pub const EVT_FORMAT_TABLE: u16 = 1;
    pub const EVT_MAIN_DEVICE: u16 = 2;
    pub const EVT_TRANCHE_DONE: u16 = 3;
    pub const EVT_TRANCHE_TARGET_DEVICE: u16 = 4;
    pub const EVT_TRANCHE_FORMATS: u16 = 5;
    pub const EVT_TRANCHE_FLAGS: u16 = 6;
    pub const TRANCHE_FLAGS_SCANOUT: u32 = 1;
}
pub mod wp_linux_drm_syncobj_manager_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wp_linux_drm_syncobj_manager_v1", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "get_surface", since: 1, destructor: false, args: &[ArgType::NewId("wp_linux_drm_syncobj_surface_v1"), ArgType::Object("wl_surface")] },
        Msg { name: "import_timeline", since: 1, destructor: false, args: &[ArgType::NewId("wp_linux_drm_syncobj_timeline_v1"), ArgType::Fd] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_GET_SURFACE: u16 = 1;
    pub const REQ_IMPORT_TIMELINE: u16 = 2;
    pub const ERROR_SURFACE_EXISTS: u32 = 0;
    pub const ERROR_INVALID_TIMELINE: u32 = 1;
}
pub mod wp_linux_drm_syncobj_timeline_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wp_linux_drm_syncobj_timeline_v1", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
}
pub mod wp_linux_drm_syncobj_surface_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wp_linux_drm_syncobj_surface_v1", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "set_acquire_point", since: 1, destructor: false, args: &[ArgType::Object("wp_linux_drm_syncobj_timeline_v1"), ArgType::Uint, ArgType::Uint] },
        Msg { name: "set_release_point", since: 1, destructor: false, args: &[ArgType::Object("wp_linux_drm_syncobj_timeline_v1"), ArgType::Uint, ArgType::Uint] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_SET_ACQUIRE_POINT: u16 = 1;
    pub const REQ_SET_RELEASE_POINT: u16 = 2;
    pub const ERROR_NO_SURFACE: u32 = 1;
    pub const ERROR_UNSUPPORTED_BUFFER: u32 = 2;
    pub const ERROR_NO_BUFFER: u32 = 3;
    pub const ERROR_NO_ACQUIRE_POINT: u32 = 4;
    pub const ERROR_NO_RELEASE_POINT: u32 = 5;
    pub const ERROR_CONFLICTING_POINTS: u32 = 6;
}
pub mod zwp_tablet_manager_v2 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_tablet_manager_v2", version: 2,
    requests: &[
        Msg { name: "get_tablet_seat", since: 1, destructor: false, args: &[ArgType::NewId("zwp_tablet_seat_v2"), ArgType::Object("wl_seat")] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
    ],
    };
    pub const REQ_GET_TABLET_SEAT: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
}
pub mod zwp_tablet_seat_v2 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_tablet_seat_v2", version: 2,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "tablet_added", since: 1, destructor: false, args: &[ArgType::NewId("zwp_tablet_v2")] },
        Msg { name: "tool_added", since: 1, destructor: false, args: &[ArgType::NewId("zwp_tablet_tool_v2")] },
        Msg { name: "pad_added", since: 1, destructor: false, args: &[ArgType::NewId("zwp_tablet_pad_v2")] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const EVT_TABLET_ADDED: u16 = 0;
    pub const EVT_TOOL_ADDED: u16 = 1;
    pub const EVT_PAD_ADDED: u16 = 2;
}
pub mod zwp_tablet_tool_v2 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_tablet_tool_v2", version: 2,
    requests: &[
        Msg { name: "set_cursor", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::ObjectOpt("wl_surface"), ArgType::Int, ArgType::Int] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "type", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "hardware_serial", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint] },
        Msg { name: "hardware_id_wacom", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint] },
        Msg { name: "capability", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "done", since: 1, destructor: false, args: &[] },
        Msg { name: "removed", since: 1, destructor: false, args: &[] },
        Msg { name: "proximity_in", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Object("zwp_tablet_v2"), ArgType::Object("wl_surface")] },
        Msg { name: "proximity_out", since: 1, destructor: false, args: &[] },
        Msg { name: "down", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "up", since: 1, destructor: false, args: &[] },
        Msg { name: "motion", since: 1, destructor: false, args: &[ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "pressure", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "distance", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "tilt", since: 1, destructor: false, args: &[ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "rotation", since: 1, destructor: false, args: &[ArgType::Fixed] },
        Msg { name: "slider", since: 1, destructor: false, args: &[ArgType::Int] },
        Msg { name: "wheel", since: 1, destructor: false, args: &[ArgType::Fixed, ArgType::Int] },
        Msg { name: "button", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Uint] },
        Msg { name: "frame", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_SET_CURSOR: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
    pub const EVT_TYPE: u16 = 0;
    pub const EVT_HARDWARE_SERIAL: u16 = 1;
    pub const EVT_HARDWARE_ID_WACOM: u16 = 2;
    pub const EVT_CAPABILITY: u16 = 3;
    pub const EVT_DONE: u16 = 4;
    pub const EVT_REMOVED: u16 = 5;
    pub const EVT_PROXIMITY_IN: u16 = 6;
    pub const EVT_PROXIMITY_OUT: u16 = 7;
    pub const EVT_DOWN: u16 = 8;
    pub const EVT_UP: u16 = 9;
    pub const EVT_MOTION: u16 = 10;
    pub const EVT_PRESSURE: u16 = 11;
    pub const EVT_DISTANCE: u16 = 12;
    pub const EVT_TILT: u16 = 13;
    pub const EVT_ROTATION: u16 = 14;
    pub const EVT_SLIDER: u16 = 15;
    pub const EVT_WHEEL: u16 = 16;
    pub const EVT_BUTTON: u16 = 17;
    pub const EVT_FRAME: u16 = 18;
    pub const TYPE_PEN: u32 = 320;
    pub const TYPE_ERASER: u32 = 321;
    pub const TYPE_BRUSH: u32 = 322;
    pub const TYPE_PENCIL: u32 = 323;
    pub const TYPE_AIRBRUSH: u32 = 324;
    pub const TYPE_FINGER: u32 = 325;
    pub const TYPE_MOUSE: u32 = 326;
    pub const TYPE_LENS: u32 = 327;
    pub const CAPABILITY_TILT: u32 = 1;
    pub const CAPABILITY_PRESSURE: u32 = 2;
    pub const CAPABILITY_DISTANCE: u32 = 3;
    pub const CAPABILITY_ROTATION: u32 = 4;
    pub const CAPABILITY_SLIDER: u32 = 5;
    pub const CAPABILITY_WHEEL: u32 = 6;
    pub const BUTTON_STATE_RELEASED: u32 = 0;
    pub const BUTTON_STATE_PRESSED: u32 = 1;
    pub const ERROR_ROLE: u32 = 0;
}
pub mod zwp_tablet_v2 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_tablet_v2", version: 2,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "name", since: 1, destructor: false, args: &[ArgType::Str] },
        Msg { name: "id", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint] },
        Msg { name: "path", since: 1, destructor: false, args: &[ArgType::Str] },
        Msg { name: "done", since: 1, destructor: false, args: &[] },
        Msg { name: "removed", since: 1, destructor: false, args: &[] },
        Msg { name: "bustype", since: 2, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const EVT_NAME: u16 = 0;
    pub const EVT_ID: u16 = 1;
    pub const EVT_PATH: u16 = 2;
    pub const EVT_DONE: u16 = 3;
    pub const EVT_REMOVED: u16 = 4;
    pub const EVT_BUSTYPE: u16 = 5;
    pub const BUSTYPE_USB: u32 = 3;
    pub const BUSTYPE_BLUETOOTH: u32 = 5;
    pub const BUSTYPE_VIRTUAL: u32 = 6;
    pub const BUSTYPE_SERIAL: u32 = 17;
    pub const BUSTYPE_I2C: u32 = 24;
}
pub mod zwp_tablet_pad_ring_v2 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_tablet_pad_ring_v2", version: 2,
    requests: &[
        Msg { name: "set_feedback", since: 1, destructor: false, args: &[ArgType::Str, ArgType::Uint] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "source", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "angle", since: 1, destructor: false, args: &[ArgType::Fixed] },
        Msg { name: "stop", since: 1, destructor: false, args: &[] },
        Msg { name: "frame", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_SET_FEEDBACK: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
    pub const EVT_SOURCE: u16 = 0;
    pub const EVT_ANGLE: u16 = 1;
    pub const EVT_STOP: u16 = 2;
    pub const EVT_FRAME: u16 = 3;
    pub const SOURCE_FINGER: u32 = 1;
}
pub mod zwp_tablet_pad_strip_v2 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_tablet_pad_strip_v2", version: 2,
    requests: &[
        Msg { name: "set_feedback", since: 1, destructor: false, args: &[ArgType::Str, ArgType::Uint] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "source", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "position", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "stop", since: 1, destructor: false, args: &[] },
        Msg { name: "frame", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_SET_FEEDBACK: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
    pub const EVT_SOURCE: u16 = 0;
    pub const EVT_POSITION: u16 = 1;
    pub const EVT_STOP: u16 = 2;
    pub const EVT_FRAME: u16 = 3;
    pub const SOURCE_FINGER: u32 = 1;
}
pub mod zwp_tablet_pad_group_v2 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_tablet_pad_group_v2", version: 2,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "buttons", since: 1, destructor: false, args: &[ArgType::Array] },
        Msg { name: "ring", since: 1, destructor: false, args: &[ArgType::NewId("zwp_tablet_pad_ring_v2")] },
        Msg { name: "strip", since: 1, destructor: false, args: &[ArgType::NewId("zwp_tablet_pad_strip_v2")] },
        Msg { name: "modes", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "done", since: 1, destructor: false, args: &[] },
        Msg { name: "mode_switch", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Uint] },
        Msg { name: "dial", since: 2, destructor: false, args: &[ArgType::NewId("zwp_tablet_pad_dial_v2")] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const EVT_BUTTONS: u16 = 0;
    pub const EVT_RING: u16 = 1;
    pub const EVT_STRIP: u16 = 2;
    pub const EVT_MODES: u16 = 3;
    pub const EVT_DONE: u16 = 4;
    pub const EVT_MODE_SWITCH: u16 = 5;
    pub const EVT_DIAL: u16 = 6;
}
pub mod zwp_tablet_pad_v2 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_tablet_pad_v2", version: 2,
    requests: &[
        Msg { name: "set_feedback", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Str, ArgType::Uint] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "group", since: 1, destructor: false, args: &[ArgType::NewId("zwp_tablet_pad_group_v2")] },
        Msg { name: "path", since: 1, destructor: false, args: &[ArgType::Str] },
        Msg { name: "buttons", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "done", since: 1, destructor: false, args: &[] },
        Msg { name: "button", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Uint] },
        Msg { name: "enter", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Object("zwp_tablet_v2"), ArgType::Object("wl_surface")] },
        Msg { name: "leave", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Object("wl_surface")] },
        Msg { name: "removed", since: 1, destructor: false, args: &[] },
    ],
    };
    pub const REQ_SET_FEEDBACK: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
    pub const EVT_GROUP: u16 = 0;
    pub const EVT_PATH: u16 = 1;
    pub const EVT_BUTTONS: u16 = 2;
    pub const EVT_DONE: u16 = 3;
    pub const EVT_BUTTON: u16 = 4;
    pub const EVT_ENTER: u16 = 5;
    pub const EVT_LEAVE: u16 = 6;
    pub const EVT_REMOVED: u16 = 7;
    pub const BUTTON_STATE_RELEASED: u32 = 0;
    pub const BUTTON_STATE_PRESSED: u32 = 1;
}
pub mod zwp_tablet_pad_dial_v2 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_tablet_pad_dial_v2", version: 2,
    requests: &[
        Msg { name: "set_feedback", since: 1, destructor: false, args: &[ArgType::Str, ArgType::Uint] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "delta", since: 1, destructor: false, args: &[ArgType::Int] },
        Msg { name: "frame", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_SET_FEEDBACK: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
    pub const EVT_DELTA: u16 = 0;
    pub const EVT_FRAME: u16 = 1;
}
pub mod wp_cursor_shape_manager_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wp_cursor_shape_manager_v1", version: 2,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "get_pointer", since: 1, destructor: false, args: &[ArgType::NewId("wp_cursor_shape_device_v1"), ArgType::Object("wl_pointer")] },
        Msg { name: "get_tablet_tool_v2", since: 1, destructor: false, args: &[ArgType::NewId("wp_cursor_shape_device_v1"), ArgType::Object("zwp_tablet_tool_v2")] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_GET_POINTER: u16 = 1;
    pub const REQ_GET_TABLET_TOOL_V2: u16 = 2;
}
pub mod wp_cursor_shape_device_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wp_cursor_shape_device_v1", version: 2,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "set_shape", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_SET_SHAPE: u16 = 1;
    pub const SHAPE_DEFAULT: u32 = 1;
    pub const SHAPE_CONTEXT_MENU: u32 = 2;
    pub const SHAPE_HELP: u32 = 3;
    pub const SHAPE_POINTER: u32 = 4;
    pub const SHAPE_PROGRESS: u32 = 5;
    pub const SHAPE_WAIT: u32 = 6;
    pub const SHAPE_CELL: u32 = 7;
    pub const SHAPE_CROSSHAIR: u32 = 8;
    pub const SHAPE_TEXT: u32 = 9;
    pub const SHAPE_VERTICAL_TEXT: u32 = 10;
    pub const SHAPE_ALIAS: u32 = 11;
    pub const SHAPE_COPY: u32 = 12;
    pub const SHAPE_MOVE: u32 = 13;
    pub const SHAPE_NO_DROP: u32 = 14;
    pub const SHAPE_NOT_ALLOWED: u32 = 15;
    pub const SHAPE_GRAB: u32 = 16;
    pub const SHAPE_GRABBING: u32 = 17;
    pub const SHAPE_E_RESIZE: u32 = 18;
    pub const SHAPE_N_RESIZE: u32 = 19;
    pub const SHAPE_NE_RESIZE: u32 = 20;
    pub const SHAPE_NW_RESIZE: u32 = 21;
    pub const SHAPE_S_RESIZE: u32 = 22;
    pub const SHAPE_SE_RESIZE: u32 = 23;
    pub const SHAPE_SW_RESIZE: u32 = 24;
    pub const SHAPE_W_RESIZE: u32 = 25;
    pub const SHAPE_EW_RESIZE: u32 = 26;
    pub const SHAPE_NS_RESIZE: u32 = 27;
    pub const SHAPE_NESW_RESIZE: u32 = 28;
    pub const SHAPE_NWSE_RESIZE: u32 = 29;
    pub const SHAPE_COL_RESIZE: u32 = 30;
    pub const SHAPE_ROW_RESIZE: u32 = 31;
    pub const SHAPE_ALL_SCROLL: u32 = 32;
    pub const SHAPE_ZOOM_IN: u32 = 33;
    pub const SHAPE_ZOOM_OUT: u32 = 34;
    pub const SHAPE_DND_ASK: u32 = 35;
    pub const SHAPE_ALL_RESIZE: u32 = 36;
    pub const ERROR_INVALID_SHAPE: u32 = 1;
}
pub mod wp_fractional_scale_manager_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wp_fractional_scale_manager_v1", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "get_fractional_scale", since: 1, destructor: false, args: &[ArgType::NewId("wp_fractional_scale_v1"), ArgType::Object("wl_surface")] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_GET_FRACTIONAL_SCALE: u16 = 1;
    pub const ERROR_FRACTIONAL_SCALE_EXISTS: u32 = 0;
}
pub mod wp_fractional_scale_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "wp_fractional_scale_v1", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "preferred_scale", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const EVT_PREFERRED_SCALE: u16 = 0;
}
pub mod xdg_activation_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "xdg_activation_v1", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "get_activation_token", since: 1, destructor: false, args: &[ArgType::NewId("xdg_activation_token_v1")] },
        Msg { name: "activate", since: 1, destructor: false, args: &[ArgType::Str, ArgType::Object("wl_surface")] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_GET_ACTIVATION_TOKEN: u16 = 1;
    pub const REQ_ACTIVATE: u16 = 2;
}
pub mod xdg_activation_token_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "xdg_activation_token_v1", version: 1,
    requests: &[
        Msg { name: "set_serial", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Object("wl_seat")] },
        Msg { name: "set_app_id", since: 1, destructor: false, args: &[ArgType::Str] },
        Msg { name: "set_surface", since: 1, destructor: false, args: &[ArgType::Object("wl_surface")] },
        Msg { name: "commit", since: 1, destructor: false, args: &[] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "done", since: 1, destructor: false, args: &[ArgType::Str] },
    ],
    };
    pub const REQ_SET_SERIAL: u16 = 0;
    pub const REQ_SET_APP_ID: u16 = 1;
    pub const REQ_SET_SURFACE: u16 = 2;
    pub const REQ_COMMIT: u16 = 3;
    pub const REQ_DESTROY: u16 = 4;
    pub const EVT_DONE: u16 = 0;
    pub const ERROR_ALREADY_USED: u32 = 0;
}
pub mod zxdg_decoration_manager_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zxdg_decoration_manager_v1", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "get_toplevel_decoration", since: 1, destructor: false, args: &[ArgType::NewId("zxdg_toplevel_decoration_v1"), ArgType::Object("xdg_toplevel")] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_GET_TOPLEVEL_DECORATION: u16 = 1;
}
pub mod zxdg_toplevel_decoration_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zxdg_toplevel_decoration_v1", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "set_mode", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "unset_mode", since: 1, destructor: false, args: &[] },
    ],
    events: &[
        Msg { name: "configure", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_SET_MODE: u16 = 1;
    pub const REQ_UNSET_MODE: u16 = 2;
    pub const EVT_CONFIGURE: u16 = 0;
    pub const ERROR_UNCONFIGURED_BUFFER: u32 = 0;
    pub const ERROR_ALREADY_CONSTRUCTED: u32 = 1;
    pub const ERROR_ORPHANED: u32 = 2;
    pub const ERROR_INVALID_MODE: u32 = 3;
    pub const MODE_CLIENT_SIDE: u32 = 1;
    pub const MODE_SERVER_SIDE: u32 = 2;
}
pub mod zwp_text_input_v3 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_text_input_v3", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "enable", since: 1, destructor: false, args: &[] },
        Msg { name: "disable", since: 1, destructor: false, args: &[] },
        Msg { name: "set_surrounding_text", since: 1, destructor: false, args: &[ArgType::Str, ArgType::Int, ArgType::Int] },
        Msg { name: "set_text_change_cause", since: 1, destructor: false, args: &[ArgType::Uint] },
        Msg { name: "set_content_type", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint] },
        Msg { name: "set_cursor_rectangle", since: 1, destructor: false, args: &[ArgType::Int, ArgType::Int, ArgType::Int, ArgType::Int] },
        Msg { name: "commit", since: 1, destructor: false, args: &[] },
    ],
    events: &[
        Msg { name: "enter", since: 1, destructor: false, args: &[ArgType::Object("wl_surface")] },
        Msg { name: "leave", since: 1, destructor: false, args: &[ArgType::Object("wl_surface")] },
        Msg { name: "preedit_string", since: 1, destructor: false, args: &[ArgType::StrOpt, ArgType::Int, ArgType::Int] },
        Msg { name: "commit_string", since: 1, destructor: false, args: &[ArgType::StrOpt] },
        Msg { name: "delete_surrounding_text", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint] },
        Msg { name: "done", since: 1, destructor: false, args: &[ArgType::Uint] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_ENABLE: u16 = 1;
    pub const REQ_DISABLE: u16 = 2;
    pub const REQ_SET_SURROUNDING_TEXT: u16 = 3;
    pub const REQ_SET_TEXT_CHANGE_CAUSE: u16 = 4;
    pub const REQ_SET_CONTENT_TYPE: u16 = 5;
    pub const REQ_SET_CURSOR_RECTANGLE: u16 = 6;
    pub const REQ_COMMIT: u16 = 7;
    pub const EVT_ENTER: u16 = 0;
    pub const EVT_LEAVE: u16 = 1;
    pub const EVT_PREEDIT_STRING: u16 = 2;
    pub const EVT_COMMIT_STRING: u16 = 3;
    pub const EVT_DELETE_SURROUNDING_TEXT: u16 = 4;
    pub const EVT_DONE: u16 = 5;
    pub const CHANGE_CAUSE_INPUT_METHOD: u32 = 0;
    pub const CHANGE_CAUSE_OTHER: u32 = 1;
    pub const CONTENT_HINT_NONE: u32 = 0;
    pub const CONTENT_HINT_COMPLETION: u32 = 1;
    pub const CONTENT_HINT_SPELLCHECK: u32 = 2;
    pub const CONTENT_HINT_AUTO_CAPITALIZATION: u32 = 4;
    pub const CONTENT_HINT_LOWERCASE: u32 = 8;
    pub const CONTENT_HINT_UPPERCASE: u32 = 16;
    pub const CONTENT_HINT_TITLECASE: u32 = 32;
    pub const CONTENT_HINT_HIDDEN_TEXT: u32 = 64;
    pub const CONTENT_HINT_SENSITIVE_DATA: u32 = 128;
    pub const CONTENT_HINT_LATIN: u32 = 256;
    pub const CONTENT_HINT_MULTILINE: u32 = 512;
    pub const CONTENT_PURPOSE_NORMAL: u32 = 0;
    pub const CONTENT_PURPOSE_ALPHA: u32 = 1;
    pub const CONTENT_PURPOSE_DIGITS: u32 = 2;
    pub const CONTENT_PURPOSE_NUMBER: u32 = 3;
    pub const CONTENT_PURPOSE_PHONE: u32 = 4;
    pub const CONTENT_PURPOSE_URL: u32 = 5;
    pub const CONTENT_PURPOSE_EMAIL: u32 = 6;
    pub const CONTENT_PURPOSE_NAME: u32 = 7;
    pub const CONTENT_PURPOSE_PASSWORD: u32 = 8;
    pub const CONTENT_PURPOSE_PIN: u32 = 9;
    pub const CONTENT_PURPOSE_DATE: u32 = 10;
    pub const CONTENT_PURPOSE_TIME: u32 = 11;
    pub const CONTENT_PURPOSE_DATETIME: u32 = 12;
    pub const CONTENT_PURPOSE_TERMINAL: u32 = 13;
}
pub mod zwp_text_input_manager_v3 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_text_input_manager_v3", version: 1,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
        Msg { name: "get_text_input", since: 1, destructor: false, args: &[ArgType::NewId("zwp_text_input_v3"), ArgType::Object("wl_seat")] },
    ],
    events: &[
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const REQ_GET_TEXT_INPUT: u16 = 1;
}
pub mod zwp_pointer_gestures_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_pointer_gestures_v1", version: 3,
    requests: &[
        Msg { name: "get_swipe_gesture", since: 1, destructor: false, args: &[ArgType::NewId("zwp_pointer_gesture_swipe_v1"), ArgType::Object("wl_pointer")] },
        Msg { name: "get_pinch_gesture", since: 1, destructor: false, args: &[ArgType::NewId("zwp_pointer_gesture_pinch_v1"), ArgType::Object("wl_pointer")] },
        Msg { name: "release", since: 2, destructor: true, args: &[] },
        Msg { name: "get_hold_gesture", since: 3, destructor: false, args: &[ArgType::NewId("zwp_pointer_gesture_hold_v1"), ArgType::Object("wl_pointer")] },
    ],
    events: &[
    ],
    };
    pub const REQ_GET_SWIPE_GESTURE: u16 = 0;
    pub const REQ_GET_PINCH_GESTURE: u16 = 1;
    pub const REQ_RELEASE: u16 = 2;
    pub const REQ_GET_HOLD_GESTURE: u16 = 3;
}
pub mod zwp_pointer_gesture_swipe_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_pointer_gesture_swipe_v1", version: 2,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "begin", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Object("wl_surface"), ArgType::Uint] },
        Msg { name: "update", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "end", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Int] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const EVT_BEGIN: u16 = 0;
    pub const EVT_UPDATE: u16 = 1;
    pub const EVT_END: u16 = 2;
}
pub mod zwp_pointer_gesture_pinch_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_pointer_gesture_pinch_v1", version: 2,
    requests: &[
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "begin", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Object("wl_surface"), ArgType::Uint] },
        Msg { name: "update", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Fixed, ArgType::Fixed, ArgType::Fixed, ArgType::Fixed] },
        Msg { name: "end", since: 1, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Int] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const EVT_BEGIN: u16 = 0;
    pub const EVT_UPDATE: u16 = 1;
    pub const EVT_END: u16 = 2;
}
pub mod zwp_pointer_gesture_hold_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_pointer_gesture_hold_v1", version: 3,
    requests: &[
        Msg { name: "destroy", since: 3, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "begin", since: 3, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Object("wl_surface"), ArgType::Uint] },
        Msg { name: "end", since: 3, destructor: false, args: &[ArgType::Uint, ArgType::Uint, ArgType::Int] },
    ],
    };
    pub const REQ_DESTROY: u16 = 0;
    pub const EVT_BEGIN: u16 = 0;
    pub const EVT_END: u16 = 1;
}
pub mod zwp_primary_selection_device_manager_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_primary_selection_device_manager_v1", version: 1,
    requests: &[
        Msg { name: "create_source", since: 1, destructor: false, args: &[ArgType::NewId("zwp_primary_selection_source_v1")] },
        Msg { name: "get_device", since: 1, destructor: false, args: &[ArgType::NewId("zwp_primary_selection_device_v1"), ArgType::Object("wl_seat")] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
    ],
    };
    pub const REQ_CREATE_SOURCE: u16 = 0;
    pub const REQ_GET_DEVICE: u16 = 1;
    pub const REQ_DESTROY: u16 = 2;
}
pub mod zwp_primary_selection_device_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_primary_selection_device_v1", version: 1,
    requests: &[
        Msg { name: "set_selection", since: 1, destructor: false, args: &[ArgType::ObjectOpt("zwp_primary_selection_source_v1"), ArgType::Uint] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "data_offer", since: 1, destructor: false, args: &[ArgType::NewId("zwp_primary_selection_offer_v1")] },
        Msg { name: "selection", since: 1, destructor: false, args: &[ArgType::ObjectOpt("zwp_primary_selection_offer_v1")] },
    ],
    };
    pub const REQ_SET_SELECTION: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
    pub const EVT_DATA_OFFER: u16 = 0;
    pub const EVT_SELECTION: u16 = 1;
}
pub mod zwp_primary_selection_offer_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_primary_selection_offer_v1", version: 1,
    requests: &[
        Msg { name: "receive", since: 1, destructor: false, args: &[ArgType::Str, ArgType::Fd] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "offer", since: 1, destructor: false, args: &[ArgType::Str] },
    ],
    };
    pub const REQ_RECEIVE: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
    pub const EVT_OFFER: u16 = 0;
}
pub mod zwp_primary_selection_source_v1 {
    use crate::schema::*;
    pub static INTERFACE: Interface = Interface {
    name: "zwp_primary_selection_source_v1", version: 1,
    requests: &[
        Msg { name: "offer", since: 1, destructor: false, args: &[ArgType::Str] },
        Msg { name: "destroy", since: 1, destructor: true, args: &[] },
    ],
    events: &[
        Msg { name: "send", since: 1, destructor: false, args: &[ArgType::Str, ArgType::Fd] },
        Msg { name: "cancelled", since: 1, destructor: false, args: &[] },
    ],
    };
    pub const REQ_OFFER: u16 = 0;
    pub const REQ_DESTROY: u16 = 1;
    pub const EVT_SEND: u16 = 0;
    pub const EVT_CANCELLED: u16 = 1;
}

pub static ALL: &[&crate::schema::Interface] = &[
    &wl_display::INTERFACE,
    &wl_registry::INTERFACE,
    &wl_callback::INTERFACE,
    &wl_compositor::INTERFACE,
    &wl_shm_pool::INTERFACE,
    &wl_shm::INTERFACE,
    &wl_buffer::INTERFACE,
    &wl_data_offer::INTERFACE,
    &wl_data_source::INTERFACE,
    &wl_data_device::INTERFACE,
    &wl_data_device_manager::INTERFACE,
    &wl_shell::INTERFACE,
    &wl_shell_surface::INTERFACE,
    &wl_surface::INTERFACE,
    &wl_seat::INTERFACE,
    &wl_pointer::INTERFACE,
    &wl_keyboard::INTERFACE,
    &wl_touch::INTERFACE,
    &wl_output::INTERFACE,
    &wl_region::INTERFACE,
    &wl_subcompositor::INTERFACE,
    &wl_subsurface::INTERFACE,
    &wl_fixes::INTERFACE,
    &xdg_wm_base::INTERFACE,
    &xdg_positioner::INTERFACE,
    &xdg_surface::INTERFACE,
    &xdg_toplevel::INTERFACE,
    &xdg_popup::INTERFACE,
    &wp_viewporter::INTERFACE,
    &wp_viewport::INTERFACE,
    &zwp_linux_dmabuf_v1::INTERFACE,
    &zwp_linux_buffer_params_v1::INTERFACE,
    &zwp_linux_dmabuf_feedback_v1::INTERFACE,
    &wp_linux_drm_syncobj_manager_v1::INTERFACE,
    &wp_linux_drm_syncobj_timeline_v1::INTERFACE,
    &wp_linux_drm_syncobj_surface_v1::INTERFACE,
    &zwp_tablet_manager_v2::INTERFACE,
    &zwp_tablet_seat_v2::INTERFACE,
    &zwp_tablet_tool_v2::INTERFACE,
    &zwp_tablet_v2::INTERFACE,
    &zwp_tablet_pad_ring_v2::INTERFACE,
    &zwp_tablet_pad_strip_v2::INTERFACE,
    &zwp_tablet_pad_group_v2::INTERFACE,
    &zwp_tablet_pad_v2::INTERFACE,
    &zwp_tablet_pad_dial_v2::INTERFACE,
    &wp_cursor_shape_manager_v1::INTERFACE,
    &wp_cursor_shape_device_v1::INTERFACE,
    &wp_fractional_scale_manager_v1::INTERFACE,
    &wp_fractional_scale_v1::INTERFACE,
    &xdg_activation_v1::INTERFACE,
    &xdg_activation_token_v1::INTERFACE,
    &zxdg_decoration_manager_v1::INTERFACE,
    &zxdg_toplevel_decoration_v1::INTERFACE,
    &zwp_text_input_v3::INTERFACE,
    &zwp_text_input_manager_v3::INTERFACE,
    &zwp_pointer_gestures_v1::INTERFACE,
    &zwp_pointer_gesture_swipe_v1::INTERFACE,
    &zwp_pointer_gesture_pinch_v1::INTERFACE,
    &zwp_pointer_gesture_hold_v1::INTERFACE,
    &zwp_primary_selection_device_manager_v1::INTERFACE,
    &zwp_primary_selection_device_v1::INTERFACE,
    &zwp_primary_selection_offer_v1::INTERFACE,
    &zwp_primary_selection_source_v1::INTERFACE,
];
