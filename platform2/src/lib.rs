//#![cfg_attr(all(unix), feature(unix_socket_ancillary_data))]

pub mod os;

#[macro_use]
pub mod log;


#[macro_use]
mod cx;
mod cx_api;

pub mod action;

pub mod thread;
pub mod audio;
pub mod midi;
pub mod video;
pub mod scope;
pub mod script;

mod draw_matrix;
mod draw_shader; 
mod draw_list;
mod draw_vars;

mod id_pool;
pub mod event;
pub mod permission;
mod area;
mod window;
mod pass;
mod texture;
mod cursor;
mod macos_menu;
mod gpu_info;
mod geometry;
mod debug;
mod component_map;
mod component_list;
mod performance_stats;
pub mod studio;

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

#[cfg(any(target_os = "macos", target_os = "ios", target_os="tvos"))]
pub use makepad_objc_sys;

#[cfg(target_os = "windows")]
pub use ::windows as windows;

pub use makepad_futures;
 
pub use {
    makepad_script,
    makepad_http,
    smallvec,
    smallvec::SmallVec,
    //makepad_image_formats::image,
    
    log::*,
    makepad_math::*,
    makepad_math::makepad_micro_serde,
    makepad_script::{
        heap::*,
        value::*,
        object::*,
        vm::*,
        traits::*,
        makepad_script_derive::*,
        makepad_math,
        makepad_error_log,
        makepad_live_id,
        makepad_live_id::*,
    },
    app_main::*,
    component_map::ComponentMap,
    component_list::ComponentList,
    crate::{
        os::*,
        cx_api::{CxOsApi,OpenUrlInPlace, CxOsOp},
        media_api::CxMediaApi,
        scope::*,
        draw_list::{
            CxDrawItem,
            CxRectArea,
            CxDrawCall,
            DrawList,
            DrawListId,
            CxDrawListPool
        },
        cx::{
            Cx,
            CxRef,
            OsType
        },
        area::{
            Area,
            RectArea,
            InstanceArea
        },
        midi::*,
        audio::*,
        thread::*,
        video::*,
        web_socket::{WebSocket,WebSocketMessage},
        event::{
            VirtualKeyboardEvent,
            HttpRequest,
            HttpResponse,
            HttpMethod,
            HttpProgress,
            HttpError,
            NetworkResponse,
            NetworkResponsesEvent,
            Margin,
            KeyCode,
            Event,
            Hit,
            DragHit,
            Trigger,
            //MidiInputListEvent,
            Timer,
            NextFrame,
            KeyModifiers,
            DrawEvent,
            DigitDevice,
            MouseButton,
            MouseDownEvent,
            MouseMoveEvent,
            MouseUpEvent,
            FingerDownEvent,
            FingerMoveEvent,
            FingerUpEvent,
            HoverState,
            FingerHoverEvent,
            FingerScrollEvent,
            WindowGeomChangeEvent,
            WindowMovedEvent,
            NextFrameEvent,
            TimerEvent,
            KeyEvent,
            KeyFocusEvent,
            TextInputEvent,
            TextClipboardEvent,
            WindowCloseRequestedEvent,
            WindowClosedEvent,
            WindowDragQueryResponse,
            WindowDragQueryEvent,
            XrController,
            XrHand,
            XrAnchor,
            XrState,
            XrUpdateEvent,
            XrLocalEvent,
            DragEvent,
            DropEvent,
            DragState,
            DragItem,
            DragResponse,
            HitOptions,
            DragHitEvent,
            DropHitEvent,
            DesignerPickEvent,
            HitDesigner,
        },
        action::{
            Action,
            Actions,
            ActionsBuf, 
            ActionCast,
            ActionCastRef,
            ActionTrait,
            ActionDefaultRef
        },
        cursor::MouseCursor,
        macos_menu::MacosMenu,
        draw_matrix::DrawMatrix,
        window::{WindowHandle,CxWindowPool, WindowId},
        pass::{
            PassId,
            CxPassParent,
            CxPassRect,
            Pass,
            PassClearColor,
            PassClearDepth
        },
        texture::{
            Texture,
            TextureId,
            TextureFormat,
            TextureSize,
            TextureUpdated,
            TextureAnimation,
        },
        draw_vars::{
            shader_enum,
            DrawVars
        },
        geometry::{
            GeometryFingerprint,
            GeometryField,
            GeometryFields,
            GeometryId,
            GeometryRef,
            Geometry,
        },
        gpu_info::GpuPerformance,     
        ui_runner::*,  
    },
};

