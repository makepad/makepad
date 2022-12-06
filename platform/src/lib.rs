pub mod os;

#[macro_use]
mod live_prims;

#[macro_use]
mod cx;
mod cx_api;
mod cx_draw_shaders;

pub mod live_traits;
pub mod live_cx;
pub mod live_atomic;

pub mod thread;
pub mod audio;
pub mod midi;

mod draw_matrix;
mod id_pool;
mod event;
mod area;
mod window;
mod pass;
mod texture;
mod cursor;
mod menu;
mod state;
mod gpu_info;
mod draw_vars;
mod geometry;
mod draw_list;
mod debug;
mod component_map;

mod media_api;

#[macro_use]
mod main_app;

#[cfg(target_arch = "wasm32")]
pub use makepad_wasm_bridge;

#[cfg(target_os = "macos")]
pub use makepad_objc_sys;

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
    makepad_live_compiler::{
        vec4_ext::*,
        live_error_origin,
        live_eval,
        LiveEval,
        LiveErrorOrigin,
        LiveNodeOrigin,
        LiveRegistry,
        LiveDocNodes,
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
        FittedString,
        InlineString,
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
        event::{
            Margin,
            KeyCode,
            Event,
            Hit,
            DragHit,
            Signal,
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
            SignalEvent,
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
            LiveIdToEnum,
        },
        live_traits::{
            LiveBody,
            LiveNew,
            LiveApply,
            LiveHook,
            LiveApplyValue,
            LiveRead,
            ToLiveValue,
            ApplyFrom,
        },
        state::{
            Ease,
            Play,
            Animate,
            LiveState,
            State,
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

