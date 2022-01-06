#![allow(dead_code)]

mod platform;

#[macro_use]
mod live_prims;

#[macro_use]
mod cx;
mod cx_api;
mod cx_registries;
mod cx_draw_shaders;

mod live_eval;
pub mod live_traits;
mod live_cx;

mod event;
mod area;
mod turtle;
mod font;
mod window;
mod view;
mod pass;
mod texture;
mod cursor;
mod menu;
mod animator;
mod gpu_info;
mod draw_vars;
mod geometry;

mod shader;
//mod gen_id;
pub use {
    makepad_shader_compiler::makepad_derive_live::*,
    makepad_shader_compiler::makepad_live_tokenizer,
    makepad_shader_compiler::makepad_micro_serde,
    //makepad_microserde::*,
    makepad_shader_compiler::makepad_live_compiler::{
        vec4_ext::*,
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
        LiveIdMap,
        LiveFileId,
        LivePtr,
        LiveNode,
        LiveType,
        LiveTypeInfo,
        LiveTypeField,
        LiveFieldKind,
        //LiveTypeKind,
        LiveValue,
        FittedString,
        InlineString,
        LiveModuleId,
        LiveNodeSlice,
        LiveNodeVec,
    },
    makepad_shader_compiler,
    makepad_shader_compiler::makepad_live_compiler,
    makepad_shader_compiler::{
        ShaderRegistry,
        ShaderEnum,
        DrawShaderPtr,
        ShaderTy,
    },
    crate::{
        cx_api::{
            CxPlatformApi
        },
        cx_registries::{
            CxRegistries,
            CxRegistryNew,
        },
        cx_draw_shaders::{
        },
        cx::{
            Cx,
            PlatformType
        },
        area::{
            Area,
            ViewArea,
            InstanceArea
        },
        event::{
            KeyCode,
            Event,
            HitEvent,
            DragEvent,
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
            NextFrameEvent,
            TimerEvent,
            SignalEvent,
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
            FingerDragEvent,
            FingerDropEvent,
            DragState,
            DragAction,
            DraggedItem,
            HitOptions,
            FingerScrollHitEvent,
            FingerMoveHitEvent,
            FingerHoverHitEvent,
            FingerDownHitEvent,
            FingerUpHitEvent,
            FingerDragHitEvent,
            FingerDropHitEvent
        },
        cursor::MouseCursor,
        menu::Menu,
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
        texture::{Texture, TextureFormat},
        live_traits::{
            LiveNew,
            LiveApply,
            LiveHook,
            LiveApplyValue,
            ToLiveValue,
            LiveAnimate,
            ApplyFrom,
            LiveBody,
        },
        animator::{
            Ease,
            Play,
            Animator,
            AnimatorAction
        },
        draw_vars::{
            DrawVars
        },
        geometry::{
            GeometryField,
            Geometry,
        },
        gpu_info::{
            GpuPerformance
        },
        shader::{
            draw_quad::DrawQuad,
            draw_text::DrawText,
            draw_color::DrawColor,
            geometry_gen::{
                GeometryGen,
                GeometryQuad2D,
            },
        },
    },
};

