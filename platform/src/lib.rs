//#![cfg_attr(all(unix), feature(unix_socket_ancillary_data))]
pub mod gl_render_bridge;
pub mod home;
pub mod os;

#[cfg(any(
    test,
    all(target_arch = "wasm32", target_feature = "atomics")
))]
#[path = "os/web/alloc.rs"]
mod web_alloc;

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[global_allocator]
static WEB_GLOBAL_ALLOCATOR: web_alloc::ThreadCachingAllocator =
    web_alloc::ThreadCachingAllocator::new();

#[macro_use]
pub mod log;

#[macro_use]
mod cx;
mod arc_string_mut;
mod cx_api;
mod shared_bytes;

pub mod action;
pub mod game_input;

pub mod audio;
pub mod midi;
pub mod script;
pub mod thread;
pub mod storage;
pub mod video;
pub mod gpu_texture;

#[cfg(not(target_arch = "wasm32"))]
pub mod video_decode;
#[cfg(not(target_arch = "wasm32"))]
pub mod video_encode;
pub mod video_file;

mod draw_list;
mod draw_matrix;
mod draw_pass;
mod draw_shader;
mod draw_vars;

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod app_icon;
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
pub mod ime;
mod live_reload;
mod macos_menu;
mod performance_stats;
pub mod memory_watchdog;
pub mod perf_monitor;
pub mod sploded;
pub mod permission;
mod screen;
mod texture;
mod uniform_buffer;
mod window;
mod xr_tsdf;

pub mod web_socket;

pub mod audio_stream;

pub mod file_dialogs;

mod media_api;
mod media_host;
mod media_plugin;
mod playback_session;
mod video_session;

pub mod ui_runner;

pub mod display_context;
pub mod font_policy;

#[macro_use]
mod app_main;
pub mod remote;
pub mod pixel_probe;
pub mod screen_capture;
pub mod audio_output_tap;
pub mod shader_error;
pub use crate::app_main::{
    new_cx_with_font_set, resolve_studio_http, should_run_stdin_loop_from_env,
};
// Working-tree startup instrumentation (`MAKEPAD_TRACE=startup`).
pub use crate::cx::{
    startup_acc, startup_since_exec_ms, startup_trace, startup_trace_enabled, startup_trace_flush,
};
pub use crate::cx_api::{can_play_type, CxSystemBrowser, SystemBrowserId};
pub use crate::xr_tsdf::{
    XrDepthAlignHeightMap, XrTsdfCooperativeStepResult, XrTsdfCooperativeStepStats,
};

#[cfg(target_arch = "wasm32")]
pub use makepad_wasm_bridge;

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
pub use makepad_objc_sys;

#[cfg(target_os = "windows")]
pub use ::windows;

pub use makepad_futures;
pub use makepad_script_std::makepad_network;
pub use makepad_script_std::makepad_script;
pub use makepad_studio_protocol as studio;

// Re-export trap module for Script derive macro error macros that use crate::trap::ScriptTrap
pub use makepad_script_std::makepad_script::trap;

pub use crate::gl_render_bridge::{GlApi, GlRenderBridge};

pub use {
    crate::{
        action::{
            Action, ActionCast, ActionCastRef, ActionDefaultRef, ActionTrait, Actions, ActionsBuf,
        },
        area::{Area, InstanceArea, RectArea},
        audio::*,
        component::{ComponentInfo, ComponentRegistries, ComponentRegistry},
        cursor::MouseCursor,
        cx::{Cx, CxRef, LinuxWindowParams, OsType},
        cx_api::{AccessibilityUpdatePayload, CxOsApi, CxOsOp, CxThreadPriority, OpenUrlInPlace},
        display_context::{DisplayContext, SystemBarAppearance},
        font_policy::{
            extend_font_asset_manifest, font_asset_manifest_len, FontAsset, FontChain, FontPolicy,
            FontRole, FontSet, FONT_ASSET_MANIFEST_SECTION, INTERNATIONAL_FONT_ASSET_MANIFEST,
            LATIN_FONT_ASSET_MANIFEST, MATH_VIEW_FONT_ASSET, UI_SYMBOL_FALLBACK,
        },
        draw_list::{CxDrawCall, CxDrawItem, CxDrawListPool, CxRectArea, DrawList, DrawListId},
        draw_matrix::DrawMatrix,
        draw_pass::{
            CxDrawPassParent, CxDrawPassRect, DrawPass, DrawPassClearColor, DrawPassClearDepth,
            DrawPassId, ScriptDrawPass,
        },
        draw_vars::DrawVars,
        sploded::{SplodedParams, SplodedView},
        event::{
            CharOffset,
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
            FullTextState,
            GameInputState,
            Hit,
            HitOptions,
            HoverState,
            ImeAction,
            ImeActionEvent,
            Inset,
            KeyCode,
            KeyEvent,
            KeyFocusEvent,
            KeyModifiers,
            MouseButton,
            LocationErrorEvent,
            LocationUpdateEvent,
            MouseDownEvent,
            MouseMoveEvent,
            MouseUpEvent,
            NetworkResponsesEvent,
            StorageResponsesEvent,
            NextFrame,
            NextFrameEvent,
            QuitReason,
            QuitRequestedEvent,
            SafeAreaInsets,
            SelectionHandleDragEvent,
            SelectionHandleKind,
            SelectionHandlePhase,
            TextClipboardEvent,
            TextInputEvent,
            TextRangeReplaceEvent,
            //MidiInputListEvent,
            Timer,
            TimerEvent,
            Trigger,
            VirtualKeyboardEvent,
            WindowCloseRequestedEvent,
            WindowClosedEvent,
            WindowDragQueryEvent,
            WindowDragQueryResponse,
            WindowGeom,
            WindowGeomChangeEvent,
            WindowMovedEvent,
            XrAnchor,
            XrController,
            XrHand,
            XrLocalEvent,
            XrState,
            XrUpdateEvent,
        },
        file_dialogs::{
            FileDialog, FileDialogAction, VirtualFile, VirtualFileLimits,
            DEFAULT_VIRTUAL_FILE_SIZE_LIMIT,
        },
        game_input::*,
        geometry::{CxGeometry, Geometry, GeometryId, IndexData, VertexData},
        draw_shader::{DrawShaderAttrFormat, DrawShaderInputPacking, DrawShaderInputs},
        gpu_info::GpuPerformance,
        ime::{
            AutoCapitalize, AutoCorrect, InputMode, ReturnKeyType, SoftKeyboardConfig,
            TextInputConfig,
        },
        macos_menu::MacosMenu,
        media_api::CxMediaApi,
        media_host::{MediaControlBridge, MediaEventBridge, MediaTextureBridge, MediaTextureInfo},
        media_plugin::{
            media_plugin, media_video_capabilities, merge_video_capabilities,
            register_media_plugin, FrameDecoderCodec, FrameDecoderConfig, MediaPlaybackSession,
            MediaPlugin, MediaVideoEncoder, MseAudioTrackInfo, MseDecodedAudioFrame,
            MseDecodedFrame, MseEngineOutput, MseInitMetadata, MsePlaybackEngine,
            MseVideoTrackInfo, PlaybackPrepared, VideoFrameDecoder,
        },
        memory_watchdog::*,
        midi::*,
        os::*,
        perf_monitor::*,
        playback_session::{
            mix_active_media_audio, register_active_media_audio, register_media_playback_session,
            take_registered_media_playback_session, unregister_active_media_audio,
            unregister_media_playback_session, MediaPlaybackSessionId,
        },
        script::vm::*,
        screen::{fit_window_rect_to_screens, ScreenGeom, MIN_WINDOW_SIZE},
        shared_bytes::{MappedBytes, SharedBytes, SharedBytesStats},
        storage::{
            StorageError, StorageHandle, StorageList, StorageOp, StorageRequestId,
            StorageEstimate, StorageResponse, StorageResult, StorageStat, DEFAULT_STORAGE_VALUE_CAP,
            MAX_STORAGE_KEY_BYTES, MAX_STORAGE_LIST_LIMIT, MAX_STORAGE_NAMESPACE_BYTES,
        },
        texture::{
            image_cache_use_mipmaps, Texture, TextureAnimation, TextureFormat, TextureId,
            TextureSize, TextureUpdated, TextureWrap,
        },
        thread::*,
        ui_runner::*,
        uniform_buffer::{UniformBuffer, UniformBufferId},
        video::*,
        video_session::{
            register_video_frame_session, take_registered_video_frame_session,
            unregister_video_frame_session, VideoFrameSession, VideoFrameSessionId,
            VideoSessionState,
        },
        web_socket::{WebSocket, WebSocketMessage},
        window::{
            CxWindowPool, MacosWindowChrome, MacosWindowConfig, MacosWindowKind, MacosWindowLevel,
            ScriptWindowHandle, WindowBackdrop, WindowHandle, WindowIcon, WindowIconBuffer,
            WindowId, WindowVisuals,
        },
        xr_tsdf::{
            ChunkKey, SparseTsdGridReadSnapshot, SparseTsdReadChunk, TsdfPublishedSnapshot,
            XrTsdfState, XrTsdfStats, XrTsdfStore,
        },
    },
    app_main::*,
    arc_string_mut::ArcStringMut,
    component_list::ComponentList,
    component_map::ComponentMap,
    //makepad_image_formats::image,
    log::*,
    makepad_math::makepad_micro_serde,
    makepad_math::*,
    makepad_script_std::makepad_network::{
        HttpError, HttpMethod, HttpProgress, HttpRequest, HttpResponse, NetworkResponse,
    },
    makepad_script_std::makepad_script::{
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

/// The compiled-shader handle the const-table API is keyed by
/// (`Cx::shader_const_table`, `shader_const_patch`, `shader_const_reset`).
pub use crate::draw_shader::DrawShaderId;
