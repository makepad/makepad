#![allow(dead_code)]

#[macro_use]
mod cx;
#[macro_use]
mod liveprims;

mod liveeval;
mod livetraits;
mod livecx;

#[cfg(target_os = "linux")]
mod cx_opengl;
#[cfg(target_os = "linux")]
mod cx_xlib;
#[cfg(target_os = "linux")]
mod cx_linux;

#[cfg(target_os = "macos")]
mod cx_metal;
#[cfg(target_os = "macos")]
mod cx_macos;

#[cfg(target_os = "windows")]
mod cx_dx11;
#[cfg(target_os = "windows")]
mod cx_win32;
#[cfg(target_os = "windows")]
mod cx_windows;

#[cfg(target_arch = "wasm32")]
mod cx_webgl;
#[macro_use]
#[cfg(target_arch = "wasm32")]
mod cx_wasm32;

#[macro_use]
#[cfg(any(target_os = "linux", target_os="macos", target_os="windows"))]
mod cx_desktop;

mod turtle;
mod font;
mod window;
mod view;
mod pass;
mod texture;
mod animator;
mod area;
mod geometrygen;

mod drawquad;
mod drawtext;
mod drawcolor;
mod events;
//mod menu; 
mod geometry;
mod drawvars;
mod shader_std;
mod gpuinfo;

pub use {
    makepad_derive_live::*,
    //makepad_microserde::*,
    makepad_live_compiler::{
        live_error_origin,
        LiveErrorOrigin,
        LiveNodeOrigin,
        id,
        id_num,
        Mat4,
        Transform,
        vec2,
        vec3,
        vec4,
        Vec2,
        Vec3,
        Vec4,
        Plane,
        Quat,
        LiveRegistry,
        LiveDocNodes,
        LiveId,
        LiveFileId,
        LivePtr,
        LiveNode,
        LiveType,
        LiveTypeInfo,
        LiveTypeField,
        LiveFieldKind,
        LiveTypeKind,
        LiveValue,
        FittedString,
        InlineString,
        LiveModuleId,
        LiveNodeSlice,
        LiveNodeVec,
    },
    makepad_platform::{
        area::{
            Area,
            ViewArea,
            InstanceArea
        },
        events::{
            KeyCode,
            Event,
            Signal,
            Timer,
            NextFrame,
            KeyModifiers,
            FingerInputType,
            FingerDownEvent,
            FingerMoveEvent,
            FingerUpEvent,
            HoverState,
            FingerHoverEvent,
            FingerScrollEvent,
            WindowGeomChangeEvent,
            WindowMovedEvent,
            AnimateEvent,
            NextFrameEvent,
            FileReadEvent,
            TimerEvent,
            SignalEvent,
            FileWriteEvent,
            KeyEvent,
            KeyFocusEvent,
            TextInputEvent,
            TextCopyEvent,
            WindowCloseRequestedEvent,
            WindowClosedEvent,
            WindowResizeLoopEvent,
            WindowDragQueryResponse,
            WindowDragQueryEvent,
            XRButton,
            XRInput,
            XRUpdateEvent,
            WebSocketMessageEvent,
            FingerDragEvent,
            FingerDropEvent,
            DragState,
            DragAction,
            DraggedItem
        },
        cursor::MouseCursor,
        menu::Menu
    },
    makepad_shader_compiler::{
        ShaderRegistry, 
        ShaderEnum,
        DrawShaderPtr, 
        ShaderTy,
    },
    crate::{
        cx::{
            Cx,
            PlatformType
        },
        drawquad::DrawQuad,
        drawtext::DrawText,
        drawcolor::DrawColor,
        font::Font,
        events::{
            HitOpt,
            EventImpl
        },
        turtle::{
            LineWrap,
            Layout,
            Walk,
            Align,
            Margin,
            Padding,
            Direction,
            Axis,
            Width,
            Height,
            Rect
        },
        window::Window,
        view::{
            View,
            ManyInstances,
            ViewRedraw
        },
        pass::{
            Pass,
            PassClearColor,
            PassClearDepth
        },
        texture::{Texture,TextureFormat},
        livetraits::{
            OptionAnyAction,
            LiveFactory,
            LiveNew,
            LiveApply,
            LiveHook,
            LiveApplyValue,
            ToLiveValue,
            LiveAnimate,
            ApplyFrom,
            LiveBody,
            AnyAction,
            FrameComponent
        },
        animator::{
            Ease,
            Play,
            Animator,
            AnimatorAction
        },
        area::{
            AreaImpl,
        },
        drawvars::{
            DrawShader,
            DrawVars
        },
        geometry::{
            GeometryField,
            Geometry,
        },
        geometrygen::{
            GeometryGen,
            GeometryQuad2D,
        },
        gpuinfo::{
            GpuPerformance
        }
    }
};

