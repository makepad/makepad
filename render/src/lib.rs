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
mod cx_cocoa_util;
#[cfg(target_os = "macos")]
mod cx_cocoa_delegate;
#[cfg(target_os = "macos")]
mod cx_cocoa_app;
#[cfg(target_os = "macos")]
mod cx_cocoa_window;
#[cfg(target_os = "macos")]
mod cx_macos;
#[cfg(target_os = "macos")]
mod cx_apple;

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

mod cx_style;
mod turtle;
mod font;
mod cursor;
mod window;
mod view;
mod pass;
mod texture;
mod animation;
mod area;
mod geometrygen;

mod drawquad;
mod drawtext;
mod drawcolor;
mod events;
mod menu; 
mod geometry;
mod drawvars;
mod shader_std;
mod gpuinfo;

pub use {
    makepad_derive_live::*,
    makepad_microserde::*,
    makepad_live_compiler::{
        live_error_origin,
        LiveErrorOrigin,
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
        cursor::MouseCursor,
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
        events::{
            KeyCode,
            Event,
            HitOpt,
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
            TriggersEvent,
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
        animation::{
            Ease,
            Play,
            Animator,
        },
        area::{
            Area,
            ViewArea,
            InstanceArea
        },
        menu::{
            Menu
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

