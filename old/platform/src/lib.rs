//#![cfg_attr(all(unix), feature(unix_socket_ancillary_data))]

pub mod os;

#[macro_use]
pub mod log;

#[macro_use]
mod live_prims;

#[macro_use]
mod cx;
mod cx_api;
mod ime;

pub mod action;

pub mod live_atomic;
pub mod live_cx;
pub mod live_traits;

pub mod audio;
pub mod midi;
pub mod scope;
pub mod thread;
pub mod video;
//pub mod script;

mod draw_list;
mod draw_matrix;
mod draw_shader;
mod draw_vars;

mod animator;
mod area;
mod component_list;
mod component_map;
mod cursor;
mod debug;
pub mod event;
mod geometry;
mod gpu_info;
mod id_pool;
mod macos_menu;
mod pass;
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

pub use {
    crate::{
        action::{
            Action, ActionCast, ActionCastRef, ActionDefaultRef, ActionTrait, Actions, ActionsBuf,
        },
        animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Ease, Play},
        area::{Area, InstanceArea, RectArea},
        audio::*,
        cursor::MouseCursor,
        cx::{Cx, CxRef, OsType},
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        draw_list::{CxDrawCall, CxDrawItem, CxDrawListPool, CxRectArea, DrawList, DrawListId},
        draw_matrix::DrawMatrix,
        draw_vars::{shader_enum, DrawVars},
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
            Hit,
            HitDesigner,
            HitOptions,
            HoverState,
            HttpError,
            HttpMethod,
            HttpProgress,
            HttpRequest,
            HttpResponse,
            KeyCode,
            KeyEvent,
            KeyFocusEvent,
            KeyModifiers,
            Margin,
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
        geometry::{
            Geometry, GeometryField, GeometryFields, GeometryFingerprint, GeometryId, GeometryRef,
        },
        gpu_info::GpuPerformance,
        ime::{
            AutoCapitalize, AutoCorrect, InputMode, ReturnKeyType, SoftKeyboardConfig,
            TextInputConfig,
        },
        live_prims::{ArcStringMut, LiveDependency},
        live_traits::{
            Apply, ApplyFrom, LiveApply, LiveApplyReset, LiveApplyValue, LiveBody, LiveHook,
            LiveHookDeref, LiveNew, LiveRead, LiveRegister, ToLiveValue,
        },
        macos_menu::MacosMenu,
        media_api::CxMediaApi,
        midi::*,
        os::*,
        pass::{CxPassParent, CxPassRect, Pass, PassClearColor, PassClearDepth, PassId},
        scope::*,
        texture::{
            Texture, TextureAnimation, TextureFormat, TextureId, TextureSize, TextureUpdated,
        },
        thread::*,
        ui_runner::*,
        video::*,
        web_socket::{WebSocket, WebSocketMessage},
        window::{CxWindowPool, WindowHandle, WindowId},
    },
    app_main::*,
    component_list::ComponentList,
    component_map::ComponentMap,
    //makepad_script::vm::*,
    //makepad_script::traits::*,
    //makepad_script::script,
    log::*,
    //makepad_image_formats::image,
    makepad_derive_live::*,

    makepad_error_log,
    makepad_network,
    //makepad_script::vm,
    makepad_live_compiler::{
        live_error_origin, vec4_ext::*, InlineString, LiveBinding, LiveComponentInfo,
        LiveComponentRegistry, LiveErrorOrigin, LiveFieldKind, LiveFileId, LiveId, LiveIdAsProp,
        LiveIdMap, LiveIdPath, LiveModuleId, LiveNode, LiveNodeOrigin, LiveNodeSlice,
        LiveNodeSliceApi, LiveNodeSliceToCbor, LiveNodeVec, LiveNodeVecApi, LiveNodeVecFromCbor,
        LiveProp, LivePropType, LivePtr, LiveRef, LiveRegistry, LiveType, LiveTypeField,
        LiveTypeInfo, LiveValue,
    },
    makepad_live_id::*,
    makepad_math::*,
    makepad_shader_compiler,
    makepad_shader_compiler::makepad_derive_live,
    makepad_shader_compiler::makepad_live_compiler,
    makepad_shader_compiler::makepad_live_id,
    makepad_shader_compiler::makepad_live_tokenizer,
    makepad_shader_compiler::makepad_math,
    makepad_shader_compiler::makepad_micro_serde,
    makepad_shader_compiler::{DrawShaderPtr, ShaderEnum, ShaderRegistry, ShaderTy},
    smallvec,
    smallvec::SmallVec,
};
