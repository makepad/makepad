pub mod os;

#[macro_use]
mod live_prims;

#[macro_use]
mod cx;
mod cx_api;

pub mod live_traits;
pub mod live_cx;
pub mod live_atomic;

pub mod thread;
pub mod audio;
pub mod midi;
pub mod video;

mod draw_matrix;
mod draw_shader; 
mod draw_list;
mod draw_vars;

mod id_pool;
mod event;
mod area;
mod window;
mod pass;
mod texture;
mod cursor;
mod menu;
mod live_state;
mod gpu_info;
mod geometry;
mod debug;
mod component_map;
mod network;

pub mod audio_stream;

mod media_api;

#[macro_use]
mod app_main;

#[cfg(target_arch = "wasm32")]
pub use makepad_wasm_bridge;

#[cfg(target_os = "macos")]
pub use makepad_objc_sys;

#[cfg(target_os = "windows")]
pub use makepad_windows as windows_crate;
 
pub use {
    makepad_shader_compiler,
    makepad_shader_compiler::makepad_derive_live,
    makepad_shader_compiler::makepad_math,
    makepad_shader_compiler::makepad_live_tokenizer,
    makepad_shader_compiler::makepad_micro_serde,
    makepad_shader_compiler::makepad_live_compiler,
    makepad_shader_compiler::makepad_live_id,
    makepad_shader_compiler::makepad_error_log,
    makepad_derive_live::*,
    makepad_error_log::*,
    makepad_math::*,
    makepad_live_id::*,
    app_main::AppMain,
    makepad_live_compiler::{
        vec4_ext::*,
        live_error_origin,
        live_eval,
        LiveEval,
        LiveErrorOrigin,
        LiveNodeOrigin,
        LiveRegistry,
        LiveId,
        LiveIdMap,
        LiveFileId,
        LivePtr,
        LiveRef,
        LiveNode,
        LiveType,
        LiveTypeInfo,
        LiveTypeField,
        LiveFieldKind,
        LiveComponentInfo,
        LiveComponentRegistry,
        LivePropType,
        LiveProp,
        LiveIdAsProp,
        LiveValue,
        InlineString,
        LiveBinding,
        LiveIdPath,
        LiveNodeSliceToCbor,
        LiveNodeVecFromCbor,
        LiveModuleId,
        LiveNodeSlice,
        LiveNodeVec,
        LiveNodeSliceApi,
        LiveNodeVecApi,
    },
    component_map::{
        ComponentMap
    },
    makepad_shader_compiler::{
        ShaderRegistry,
        ShaderEnum,
        DrawShaderPtr,
        ShaderTy,
    },
    crate::{
        os::*,
        cx_api::{
            CxOsApi,
        },
        media_api::{
            CxMediaApi
        },
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
        menu::{
            MenuCommand,
        },
        thread::Signal,
        event::{
            Margin,
            KeyCode,
            Event,
            Hit,
            DragHit,
            Trigger,
            //MidiInputListEvent,
            WebSocket,
            WebSocketAutoReconnect,
            Timer,
            NextFrame,
            KeyModifiers,
            DrawEvent,
            DigitDevice,
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
            TextCopyEvent,
            WindowCloseRequestedEvent,
            WindowClosedEvent,
            WindowDragQueryResponse,
            WindowDragQueryEvent,
            XRButton,
            XRInput,
            XRUpdateEvent,
            DragEvent,
            DropEvent,
            DragState,
            DragAction,
            DraggedItem,
            HitOptions,
            DragHitEvent,
            DropHitEvent,
        },
        cursor::MouseCursor,
        menu::Menu,
        draw_matrix::DrawMatrix,
        window::Window,
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
            TextureDesc
        },
        live_prims::{
            LiveDependency,
        },
        live_traits::{
            LiveHookDeref,
            LiveBody,
            LiveNew,
            LiveApply,
            LiveHook,
            LiveApplyValue,
            LiveRead,
            ToLiveValue,
            ApplyFrom,
        },
        live_state::{
            Ease,
            Play,
            Animate,
            LiveState,
            LiveStateImpl,
            StateAction,
            StatePair
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
        gpu_info::{
            GpuPerformance
        },
        
    },
};

