//#![cfg_attr(all(unix), feature(unix_socket_ancillary_data))]
#![cfg_attr(feature = "nightly_simd", feature(portable_simd))]

pub mod os;

#[macro_use]
pub mod log;

#[macro_use]
mod cx;
mod arc_string_mut;
mod cx_api;

pub mod action;
pub mod game_input;

pub mod audio;
pub mod midi;
pub mod script;
pub mod thread;
pub mod video;

mod draw_list;
mod draw_matrix;
mod draw_pass;
mod draw_shader;
mod draw_vars;

mod area;
pub mod component;
mod component_list;
mod component_map;
mod cursor;
mod debug;
pub mod event;
mod geometry;
mod gpu_info;
mod id_pool;
mod macos_menu;
mod performance_stats;
pub mod permission;
pub mod studio;
mod texture;
mod window;

pub mod web_socket;

pub mod audio_stream;

pub mod file_dialogs;

mod media_api;

pub mod ui_runner;

pub mod display_context;

#[macro_use]
mod app_main;

#[cfg(target_arch = "wasm32")]
pub use makepad_wasm_bridge;

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
pub use makepad_objc_sys;

#[cfg(target_os = "windows")]
pub use ::windows;

pub use makepad_futures;

// Re-export trap module for Script derive macro error macros that use crate::trap::ScriptTrap
pub use makepad_script::trap;

pub use {
    crate::{
        action::{
            Action, ActionCast, ActionCastRef, ActionDefaultRef, ActionTrait, Actions, ActionsBuf,
        },
        area::{Area, InstanceArea, RectArea},
        audio::*,
        component::{ComponentInfo, ComponentRegistries, ComponentRegistry},
        cursor::MouseCursor,
        cx::{Cx, CxRef, OsType},
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        draw_list::{CxDrawCall, CxDrawItem, CxDrawListPool, CxRectArea, DrawList, DrawListId},
        draw_matrix::DrawMatrix,
        draw_pass::{
            CxDrawPassParent, CxDrawPassRect, DrawPass, DrawPassClearColor, DrawPassClearDepth,
            DrawPassId, ScriptDrawPass,
        },
        draw_vars::DrawVars,
        event::{
            DesignerPickEvent,
            DigitDevice,
            DragEvent,
            DragHit,
            DragHitEvent,
            DragItem,
            DragResponse,
            DragState,
            DrawEvent,
            DropEvent,
            DropHitEvent,
            Event,
            FingerDownEvent,
            FingerHoverEvent,
            FingerMoveEvent,
            FingerScrollEvent,
            FingerUpEvent,
            GameInputState,
            Hit,
            HitDesigner,
            HitOptions,
            HoverState,
            HttpError,
            HttpMethod,
            HttpProgress,
            HttpRequest,
            HttpResponse,
            Inset,
            KeyCode,
            KeyEvent,
            KeyFocusEvent,
            KeyModifiers,
            MouseButton,
            MouseDownEvent,
            MouseMoveEvent,
            MouseUpEvent,
            NetworkResponse,
            NetworkResponsesEvent,
            NextFrame,
            NextFrameEvent,
            TextClipboardEvent,
            TextInputEvent,
            //MidiInputListEvent,
            Timer,
            TimerEvent,
            Trigger,
            VirtualKeyboardEvent,
            WindowCloseRequestedEvent,
            WindowClosedEvent,
            WindowDragQueryEvent,
            WindowDragQueryResponse,
            WindowGeomChangeEvent,
            WindowMovedEvent,
            XrAnchor,
            XrController,
            XrHand,
            XrLocalEvent,
            XrState,
            XrUpdateEvent,
        },
        game_input::*,
        geometry::{Geometry, GeometryId},
        gpu_info::GpuPerformance,
        macos_menu::MacosMenu,
        media_api::CxMediaApi,
        midi::*,
        os::*,
        script::vm::*,
        texture::{
            Texture, TextureAnimation, TextureFormat, TextureId, TextureSize, TextureUpdated,
        },
        thread::*,
        ui_runner::*,
        video::*,
        web_socket::{WebSocket, WebSocketMessage},
        window::{CxWindowPool, ScriptWindowHandle, WindowHandle, WindowId},
    },
    app_main::*,
    arc_string_mut::ArcStringMut,
    component_list::ComponentList,
    component_map::ComponentMap,
    //makepad_image_formats::image,
    log::*,
    makepad_http,
    makepad_math::makepad_micro_serde,
    makepad_math::*,
    makepad_script,
    makepad_script::{
        apply::*, handle::*, heap::*, makepad_error_log, makepad_live_id, makepad_live_id::*,
        makepad_math, makepad_script_derive, makepad_script_derive::*, native::*, object::*,
        script_args, script_args_def, script_array_index, script_has_proto, script_is_fn,
        script_value, script_value_bool, script_value_f64, set_script_value,
        set_script_value_to_api, set_script_value_to_pod, string::*, traits::*, trap::*, value::*,
        vm::*,
    },
    smallvec,
    smallvec::SmallVec,
};
