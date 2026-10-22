//! The event table: ONE line per event (tag, name, constructor, fields). The macro generates the
//! tag constants, `RawEvent::<ctor>(..)`, `RawEvent::decode` -> [`HalEvent`] (what Cx matches on),
//! the `EventQueue::<ctor>(..)` push method the HAL calls, and the round-trip samples for tests.
//!
//! Field kinds are in `codec::k`. `window: obj` is `RawEvent::window`, `rng` the arena payload
//! (`RawEvent::data`); `f64` fields use the four float lanes (`RawEvent::a`), everything else the
//! six words of `RawEvent::b`. The time of an event (`RawEvent::time_ns`) is not a field: the queue
//! stamps it when the event is pushed.

use crate::buffers::EventQueue;
use crate::codec::{k, Field, Packer, Reader, EVENT_FLOATS, EVENT_WORDS};
use crate::types::RawEvent;

macro_rules! hal_events {
    ($($(#[doc = $doc:literal])* $name:ident = $tag:literal, $ctor:ident { $($f:ident : $kind:ident),* $(,)? };)*) => {
        /// Event tags.
        #[allow(non_upper_case_globals)]
        pub mod event_tag {
            $( pub const $name: u16 = $tag; )*
        }

        /// (tag, name, field names) of every event.
        pub const EVENT_TABLE: &[(u16, &str, &[&str])] = &[ $( ($tag, stringify!($name), &[$(stringify!($f)),*]) ),* ];

        /// An event decoded for Cx. Borrowed payloads point into the arena it was read with.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub enum HalEvent<'a> {
            $( $(#[doc = $doc])* $name { $( $f: <k::$kind as Field>::Out<'a> ),* } ),*
        }

        impl RawEvent {
            $(
                $(#[doc = $doc])*
                #[allow(clippy::too_many_arguments, unused_mut)]
                #[inline]
                pub fn $ctor($( $f: <k::$kind as Field>::T ),*) -> RawEvent {
                    #[allow(unused_comparisons)]
                    const _: () = assert!(0 $(+ <k::$kind as Field>::UW)* <= EVENT_WORDS);
                    #[allow(unused_comparisons)]
                    const _: () = assert!(0 $(+ <k::$kind as Field>::FW)* <= EVENT_FLOATS);
                    let mut p = Packer::new();
                    $( <k::$kind as Field>::pack($f, &mut p); )*
                    RawEvent { tag: $tag, window: p.id, time_ns: 0, a: p.f, b: p.words_b3(), data: p.data }
                }
            )*

            /// The typed view of this event; `None` for a tag this build does not know.
            #[allow(unused_mut, unused_variables)]
            pub fn decode<'a>(&self, arena: &'a [u8]) -> Option<HalEvent<'a>> {
                match self.tag {
                    $( $tag => {
                        let mut r = Reader::from_event(self, arena);
                        Some(HalEvent::$name { $( $f: <k::$kind as Field>::read(&mut r) ),* })
                    } )*
                    _ => None,
                }
            }
        }

        impl EventQueue {
            $(
                $(#[doc = $doc])*
                #[allow(clippy::too_many_arguments)]
                pub fn $ctor(&mut self, $( $f: <k::$kind as Field>::In<'_> ),*) {
                    $( let $f = <k::$kind as Field>::lower(self, $f); )*
                    self.push(RawEvent::$ctor($( $f ),*));
                }
            )*
        }

        /// Pushes every event once with deterministic sample values; returns what the decoder must
        /// yield for each (its `Debug` text).
        #[doc(hidden)]
        pub fn sample_events(queue: &mut EventQueue) -> Vec<String> {
            let mut expected = Vec::new();
            $( {
                #[allow(unused_mut, unused_assignments, unused_variables)]
                let mut n = $tag as u32;
                $(
                    n += 1;
                    let $f = <k::$kind as Field>::sample(n);
                )*
                expected.push(format!("{:?}", HalEvent::$name { $( $f: <k::$kind as Field>::expect($f) ),* }));
                queue.$ctor($( $f ),*);
            } )*
            expected
        }
    };
}

hal_events! {
    // ---- input (positions are window-local points)
    MouseDown = 1, mouse_down { window: obj, x: f64, y: f64, button: u32, mods: u32 };
    /// `dx`, `dy`: the pointer's motion while it is locked (otherwise zero).
    MouseMove = 2, mouse_move { window: obj, x: f64, y: f64, dx: f64, dy: f64, mods: u32 };
    MouseUp = 3, mouse_up { window: obj, x: f64, y: f64, button: u32, mods: u32 };
    MouseLeave = 4, mouse_leave { window: obj, x: f64, y: f64 };
    Scroll = 5, scroll { window: obj, x: f64, y: f64, dx: f64, dy: f64, mods: u32, precise: bool, phase: u32 };
    Pinch = 6, pinch { window: obj, x: f64, y: f64, scale: f64, phase: u32 };
    /// `touches`: `count` flat `TouchPoint`s.
    Touch = 7, touch { window: obj, count: u32, touches: rng };
    KeyDown = 8, key_down { window: obj, key: u32, repeat: bool, mods: u32 };
    KeyUp = 9, key_up { window: obj, key: u32, mods: u32 };
    /// `kind` is `input::text`: typed text, or the pasteboard's text on the paste chord.
    Text = 10, text { window: obj, kind: u32, text: rng };
    /// Composition changes: `kind` start / update / commit / cancel, text in the payload.
    Ime = 11, ime { window: obj, kind: u32, start: u32, end: u32, text: rng };
    /// A controller's state changed (`media::game::pack_state`); polled while the app asked for
    /// game input (`GameOut` ENABLE).
    GameInput = 12, game_input { device: u64, state: rng };

    /// Cancels a captured pointer press without activating the pressed control.
    MouseCancel = 13, mouse_cancel { window: obj, x: f64, y: f64, button: u32, mods: u32 };

    /// An application message forwarded by the embedding parent.
    AppMessage = 14, app_message { window: obj, message: rng };

    /// Long press in window points, with the original pointer id and event timestamp.
    LongPress = 15, long_press { window: obj, x: f64, y: f64, pointer: u32 };
    /// Complete editor state; offsets are UTF-16 code units, u32::MAX means no composition.
    ImeState = 16, ime_state { window: obj, sel_start: u32, sel_end: u32, comp_start: u32, comp_end: u32, text: rng };
    ImeAction = 17, ime_action { window: obj, action: u32 };
    ImeResize = 18, ime_resize { window: obj, height: f64, open: bool };
    PhysicalKeyboard = 19, physical_keyboard { window: obj, connected: bool };
    SelectionHandle = 33, selection_handle { window: obj, handle: u32, phase: u32, x: f64, y: f64 };

    /// The OS cancelled these pointers; never interpret this as an ordinary release/drop.
    TouchCancel = 34, touch_cancel { window: obj, count: u32, touches: rng };



    // ---- windows
    Focus = 20, focus { window: obj, focused: bool };
    /// Position and size in points, dpi factor, window state (`window_state`).
    Geom = 21, geom { window: obj, x: f64, y: f64, w: f64, h: f64, dpi: f32, state: u32 };
    CloseRequested = 22, close_requested { window: obj };
    Closed = 23, closed { window: obj };
    PopupDismissed = 24, popup_dismissed { window: obj };
    /// Asks what the window has at the point (client, caption, system menu); answered by a
    /// `DragReply` command.
    DragQuery = 25, drag_query { window: obj, x: f64, y: f64 };
    /// `phase`: 0 enter, 1 over; `items` are `count` flat `DragItem`s with their strings (see
    /// `DragItem`). Answered by a `DragReply` command.
    DragOver = 26, drag_over { window: obj, x: f64, y: f64, phase: u32, count: u32, items: rng };
    /// `dropped`: false = the drag ended without a drop.
    DragDrop = 27, drag_drop { window: obj, x: f64, y: f64, dropped: bool, count: u32, items: rng };
    ResizeLoop = 28, resize_loop { window: obj, start: bool };
    MenuCmd = 29, menu_cmd { cmd: u64 };
    /// A dpi / refresh-rate change.
    Display = 30, display { window: obj, dpi: f32, refresh: f32 };
    /// The window's chrome: the rect of the window buttons (x, y, w, h in window points) and
    /// whether the window floats above others. Sent ahead of a `Geom`.
    Chrome = 31, chrome { window: obj, x: f64, y: f64, w: f64, h: f64, topmost: bool };
    /// Safe-area insets in window points, accompanying Display and Geom notifications.
    SafeArea = 32, safe_area { window: obj, top: f64, right: f64, bottom: f64, left: f64 };

    // ---- app
    Startup = 40, startup {};
    /// `lifecycle::*`
    Lifecycle = 41, lifecycle { kind: u32 };
    Timer = 42, timer { timer: u32 };
    Wake = 43, wake {};
    /// The OS asks the app to open files and URLs: `count` drag items (`FILE` a path, `STRING`
    /// a URL) in the payload, as `pack_drag_items` writes them.
    AppOpen = 44, app_open { count: u32, items: rng };

    // ---- GPU
    /// The window can take a frame (vsync / display link); `next_ns` is the predicted next vsync.
    FrameReady = 50, frame_ready { window: obj, next_ns: u64 };
    /// The packet finished: GPU busy ns, completion ns, and the GPU span in host seconds.
    /// Zero span means this backend has no GPU timestamps. Cx submits one pass per packet.
    FrameDone = 51, frame_done { window: obj, frame: u64, gpu_ns: u64, present_ns: u64, gpu_start: f64, gpu_end: f64 };
    PipelineReady = 52, pipeline_ready { pipeline: id };
    /// The packet left draws out: a pipeline they use was still being built for its target's
    /// layout (they come with a later packet, after `PipelineReady`). Posted before its `FrameDone`.
    FramePartial = 68, frame_partial { window: obj, frame: u64 };
    /// The HAL wants a complete frame of every window (`window` NONE) or of one: an inspector
    /// is about to record it (frame capture). The app redraws everything, cached or not; with
    /// `owners` it also records which widget draws each instance range (the `draw_owners` query).
    Redraw = 70, redraw { window: obj, owners: bool };
    PipelineFailed = 53, pipeline_failed { pipeline: id, log: rng };
    BufferEvicted = 54, buffer_evicted { buffer: id };
    TextureShared = 55, texture_shared { texture: id, handle: u64 };
    /// `status`: `readback::*`; `rect` is x, y, w, h of the pixels (RGBA8 rows).
    Readback = 56, readback { req: u32, status: u32, rect: urect, pixels: rng };
    PickResult = 57, pick_result { req: u32, ids: rng };
    /// The GPU device `generation` was lost (removed, reset, driver update, adapter switch). Every
    /// event the old device still owed comes before the matching `DeviceReset`; GPU commands sent
    /// meanwhile are dropped. The app stops submitting until the reset.
    DeviceLost = 58, device_lost { generation: u32 };
    /// The HAL gave a transient texture's storage back after it sat unused; the object is gone
    /// and the app produces its content again when it is next needed.
    TextureEvicted = 59, texture_evicted { texture: id };

    // ---- results
    /// `paths`: `count` strings (u32 length + bytes).
    DialogResult = 60, dialog_result { req: u32, count: u32, paths: rng };
    /// Automation asks the app; answered by a `QueryReply` command.
    Query = 61, query { req: u32, bytes: rng };
    /// Whether a `TextureImport` acquired the shared image. A failed import can be retried.
    TextureImported = 62, texture_imported { texture: id, ok: bool };
    /// `clipboard::ClipboardData`; unsupported backends still answer the request.
    Clipboard = 63, clipboard { req: u64, supported: bool, content: rng };
    /// Device working set (0 = unknown), actual bytes, and the acknowledged reservation epoch.
    GpuBudget = 64, gpu_budget { epoch: u64, device_bytes: u64, used_bytes: u64 };
    /// Device `generation` replaced the lost one. Every object id the session holds is still
    /// valid but its contents are gone: the app uploads its textures and buffers again and paints
    /// every target. Pipelines are rebuilt by the HAL (a `PipelineReady` per pipeline follows),
    /// the font atlases are restored by the HAL, a shared texture answers with a new
    /// `TextureShared` handle and an imported one with `TextureImported { ok: false }`.
    DeviceReset = 66, device_reset { generation: u32 };
    /// The device is short of memory for the UI: free memory fell under the reserve, or a
    /// create failed for lack of room (`failed`). `wanted_mb` brings the free memory back to
    /// the reserve (plus the failed allocation); 0 = the shortage is over. Sent to every
    /// session; the one owning the desktop asks other GPU users (the AI hub) for room.
    GpuPressure = 67, gpu_pressure { wanted_mb: u32, free_mb: u32, budget_mb: u32, reserve_mb: u32, failed: bool };
    /// Announced immediately BEFORE a result whose size the client could not predict.
    /// Grow the next pump's arena to at least `bytes`. The queue never drains an oversized
    /// payload after another event in the same pump, so the announcement arrives first.
    PayloadSize = 65, payload_size { bytes: u32 };

    // ---- embedded windows and domains (to a parent session)
    /// A child's window became (or changed): `window` is its id in the child, `texture` the id
    /// the parent samples it by (valid until `ChildWindowClosed` or the next `ChildWindow` of the
    /// same window), `w`/`h` its pixels, the payload its title (utf8).
    ChildWindow = 80, child_window { window: obj, child: u32, texture: id, w: u32, h: u32, dpi: f32, title: rng };
    ChildWindowClosed = 81, child_window_closed { window: obj, child: u32 };
    /// A domain changed state (`domain_state`); `req` is the `DomainStart` it answers (0 = none).
    /// The payload is `buffers::pack_strs(&[app, message])`.
    DomainState = 82, domain_state { domain: u32, state: u32, phase: u32, pct: f32, req: u32, text: rng };
    /// The answer to `DomainList`: `count` records, see `buffers::read_domains`.
    DomainListed = 83, domain_listed { req: u32, count: u32, list: rng };
    /// A child's window presented `frame`: `window` is its id in the child, `texture` the lent
    /// id the parent samples. Only the latest pending frame per child window is delivered.
    ChildFrame = 84, child_frame { window: obj, child: u32, texture: id, frame: u64 };

    /// Forwarded by an embedding parent; answered with ChildDragReply.
    ParentDragQuery = 85, parent_drag_query { window: obj, request: u32, x: f64, y: f64 };
    ChildChrome = 86, child_chrome { window: obj, child: u32, own: bool };
    ChildDragAnswer = 87, child_drag_answer { window: obj, child: u32, request: u32, answer: u32 };
    ChildWindowState = 88, child_window_state { window: obj, child: u32, state: u32 };

    ChildMessage = 89, child_message { child: u32, message: rng };

    // ---- media (backend/apple/media; see `media`)
    /// The player `video` is ready: the picture size, the duration, whether it seeks; the payload
    /// is `media::pack_tracks`.
    VideoPrepared = 120, video_prepared { video: u64, size: u16x2, duration_ms: u64, seekable: bool, tracks: rng };
    /// A new frame is in the player's textures: the position and how it is in them (`media::yuv`).
    VideoFrame = 121, video_frame { video: u64, pos_ms: u64, yuv: u32, matrix: f32 };
    /// `video_status::*`; the text says why for an error.
    VideoStatus = 122, video_status { video: u64, kind: u32, text: rng };
    /// `ranges::*` of the player, `media::pack_ranges`.
    VideoRanges = 123, video_ranges { video: u64, kind: u32, ranges: rng };
    /// The audio devices (`media::pack_audio_devices`), on the first use of audio and whenever the
    /// system's set changes.
    AudioDevices = 124, audio_devices { devices: rng };
    /// The MIDI ports (`media::pack_midi_ports`), when the client starts and the set changes.
    MidiPorts = 125, midi_ports { ports: rng };
    /// A channel message from a connected input port (status << 16 | d1 << 8 | d2).
    MidiData = 126, midi_data { port: u64, data: u32 };
    /// The ordered controller list (`media::game::pack_devices`), whenever it changes.
    GameDevices = 127, game_devices { devices: rng };
    /// The location service: a fix or why there is none (`location_kind::*`).
    Location = 128, location { kind: u32, lon: f64, lat: f64, accuracy: f64, time: f64, alt: f32, speed: f32, heading: f32, text: rng };
    /// The answer to a `Permission` command (`permission_status::*`).
    Permission = 129, permission { req: u32, kind: u32, status: u32 };
    /// The cameras and their formats (`media::pack_video_inputs`), on the first use of capture and
    /// whenever the system's set changes.
    VideoInputs = 130, video_inputs { inputs: rng };
    /// A frame of the camera the slots in `slots` read (`media::pack_camera_frame`).
    CameraFrame = 131, camera_frame { slots: u32, frame: rng };
    /// A recorded window frame (`target::CAPTURE`): the window, its size in pixels (even), and a
    /// platform surface handle: a retained `CVPixelBuffer` on macOS, or a session-scoped
    /// completed mapped-buffer ticket on Linux. The receiving app takes the reference.
    ScreenSurface = 132, screen_surface { window: obj, size: u16x2, handle: u64 };

    // ---- fonts (to the session that asked)
    /// The answer to `FontOpen`: `status` is `font::font_status`; `len` and `hash` identify the
    /// file's content (the sfnt table directory's checksums), the payload is the path the host
    /// mapped (empty for a font given as bytes).
    FontOpened = 100, font_opened { font: u32, status: u32, len: u64, hash: u64, path: rng };
    /// An atlas the session may sample: `kind` is `font::atlas`, `texture` the lent id to
    /// draw with (sample it like any texture), `generation` changes when the atlas is made again
    /// (every glyph entry of the older generation is void).
    AtlasInfo = 101, atlas_info { kind: u32, texture: id, w: u32, h: u32, generation: u32 };
    /// Glyphs of the session's font `font` are in the atlas: `count` entries (`font::read_entries`).
    GlyphsReady = 102, glyphs_ready { font: u32, count: u32, entries: rng };
    /// The HAL took `count` slots (`font::read_slots`) back; the app drops its entries for them and
    /// asks again for what it still draws. Storage stays reserved until every session's highest
    /// frame submitted before this eviction has completed; only then may a new glyph reuse it.
    AtlasEvicted = 103, atlas_evicted { kind: u32, generation: u32, count: u32, slots: rng };

    // ---- direct display (to the session owning the desktop window)
    /// Initial inventory and every change; `direct::DisplayInventory` includes desktop-point bounds.
    /// `window = NONE` and an empty inventory revoke the previous owner's cached state.
    DirectDisplays = 133, direct_displays { window: obj, generation: u64, inventory: rng };
    /// Output index in `generation`'s inventory. Zero present time clears its phase.
    /// Presentation is in the HAL clock; the refresh period is seconds. No frame is requested.
    DirectFrameTiming = 134, direct_frame_timing { window: obj, generation: u64, output: u32, last_present_ns: u64, refresh_interval: f64 };
    /// Host acceptance of a display command (its tag in `command`), not completion of a modeset.
    /// Later device failures are inventory statuses. `message` is UTF-8 on failure.
    DirectDisplayResult = 135, direct_display_result { req: u64, command: u32, ok: bool, message: rng };
    /// Initial host preferences and changes applied by this owner.
    DirectInput = 136, direct_input { window: obj, mouse_speed: u32, touchpad_speed: u32, natural_scroll: bool, tap_to_click: bool };

    /// The soft keyboard starts to move: it will cover `height` window points of the window's
    /// bottom (`open`), or go away, over `duration` seconds along `curve` (`ime_curve::*`). The
    /// settled state follows as `ImeResize`.
    ImeAnimate = 137, ime_animate { window: obj, height: f64, open: bool, duration: f64, curve: u32 };

    // ---- GL interop (the GLES backend, at session open)
    /// The EGL display, config and context of the HAL's GLES context: a context the app makes
    /// with this share context shares its texture and sync names (`TextureAdoptGl`, `GlFence`).
    GlShare = 138, gl_share { display: u64, config: u64, context: u64 };

    // ---- system back (Android), appended
    /// A step of the system back gesture (`input::back_state::*`) at `x`, `y` window points
    /// (NaN when it is not a touch: the back button), `progress` 0..1 and the screen `edge`
    /// (`input::back_edge::*`).
    BackGesture = 146, back_gesture { window: obj, state: u32, x: f64, y: f64, progress: f64, edge: u32 };
    /// The system back press or completed back gesture. Unless the app takes it, it answers
    /// `BackUnhandled`.
    BackPressed = 147, back_pressed { window: obj };
}
