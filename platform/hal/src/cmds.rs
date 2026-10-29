//! The command table: ONE line per command (tag, name, constructor, fields). The macro below
//! generates, from those lines alone: the tag constants, `Cmd::<ctor>(..)` (the packed command),
//! `Cmd::decode` -> [`HalCmd`] (the HAL side's typed view), the `CmdWriter::<ctor>(..)` method
//! Cx calls (payload bytes go into the writer's arena), and the round-trip samples the tests use.
//!
//! Field kinds are in `codec::k`. `id: obj` is the command's subject (`Cmd::id`), `rng` is its
//! arena payload (`Cmd::data`), `flags` the small enum in `Cmd::flags`; everything else is packed
//! into the eight words of `Cmd::a`.
//!
//! Subject kinds by command: texture / buffer / pipeline commands name an object of that kind,
//! window commands a window, `ImeShow..` / `ClipboardActions` / `DragStart` the window they act
//! in. Which id space a subject belongs to follows from the command.

use crate::buffers::CmdWriter;
use crate::codec::{k, Field, Packer, Reader, CMD_WORDS};
use crate::types::Cmd;

macro_rules! hal_commands {
    ($($(#[doc = $doc:literal])* $name:ident = $tag:literal, $ctor:ident { $($f:ident : $kind:ident),* $(,)? };)*) => {
        /// Command tags.
        #[allow(non_upper_case_globals)]
        pub mod cmd_tag {
            $( pub const $name: u16 = $tag; )*
        }

        /// (tag, name, field names) of every command.
        pub const CMD_TABLE: &[(u16, &str, &[&str])] = &[ $( ($tag, stringify!($name), &[$(stringify!($f)),*]) ),* ];

        /// A command decoded for the HAL side. Borrowed payloads (`rng`) point into the arena the
        /// command was decoded with; `slice` fields are the caller's raw [`Slice`] (read it with
        /// `Slice::as_bytes` during the `hal_submit` call).
        #[derive(Clone, Copy, Debug, PartialEq)]
        #[allow(clippy::large_enum_variant)]
        pub enum HalCmd<'a> {
            $( $(#[doc = $doc])* $name { $( $f: <k::$kind as Field>::Out<'a> ),* } ),*
        }

        impl Cmd {
            $(
                $(#[doc = $doc])*
                #[allow(clippy::too_many_arguments, unused_mut)]
                #[inline]
                pub fn $ctor($( $f: <k::$kind as Field>::T ),*) -> Cmd {
                    #[allow(unused_comparisons)]
                    const _: () = assert!(0 $(+ <k::$kind as Field>::UW)* <= CMD_WORDS);
                    #[allow(unused_comparisons)]
                    const _: () = assert!(0 $(+ <k::$kind as Field>::FW)* == 0);
                    let mut p = Packer::new();
                    $( <k::$kind as Field>::pack($f, &mut p); )*
                    Cmd { tag: $tag, flags: p.flags, id: p.id, a: p.words_a4(), data: p.data }
                }
            )*

            /// The typed view of this command; `None` for a tag this build does not know.
            #[allow(unused_mut, unused_variables)]
            pub fn decode<'a>(&self, arena: &'a [u8]) -> Option<HalCmd<'a>> {
                match self.tag {
                    $( $tag => {
                        let mut r = Reader::from_cmd(self, arena);
                        Some(HalCmd::$name { $( $f: <k::$kind as Field>::read(&mut r) ),* })
                    } )*
                    _ => None,
                }
            }
        }

        impl CmdWriter {
            $(
                $(#[doc = $doc])*
                #[allow(clippy::too_many_arguments)]
                pub fn $ctor(&mut self, $( $f: <k::$kind as Field>::In<'_> ),*) {
                    $( let $f = <k::$kind as Field>::lower(self, $f); )*
                    self.push(Cmd::$ctor($( $f ),*));
                }
            )*
        }

        /// Writes every command once with deterministic sample values; returns, in order, what the
        /// decoder must yield for each (its `Debug` text).
        #[doc(hidden)]
        pub fn sample_cmds(writer: &mut CmdWriter) -> Vec<String> {
            let mut expected = Vec::new();
            $( {
                #[allow(unused_mut, unused_assignments, unused_variables)]
                let mut n = $tag as u32;
                $(
                    n += 1;
                    let $f = <k::$kind as Field>::sample(n);
                )*
                expected.push(format!("{:?}", HalCmd::$name { $( $f: <k::$kind as Field>::expect($f) ),* }));
                writer.$ctor($( $f ),*);
            } )*
            expected
        }
    };
}

hal_commands! {
    // ---- session, strings, timers
    /// Defines string `sid` of the session string table (utf8 bytes).
    StrDef = 1, str_def { sid: sid, bytes: rng };
    /// The app asks to quit.
    Quit = 2, quit {};
    /// Starts (or restarts) timer `timer`: fires after `interval_ns` of HAL time, again every
    /// interval when `repeat`.
    TimerStart = 3, timer_start { timer: u32, interval_ns: u64, repeat: bool };
    TimerStop = 4, timer_stop { timer: u32 };

    // ---- windows
    /// Creates a window. `adopt`: take the host's adoptable window (the spinner window) instead,
    /// when there is one. `kind` is `window_kind`, `geom` is x, y, w, h.
    WindowCreate = 10, window_create { id: obj, kind: u32, parent: id, geom: rect, adopt: bool };
    /// Closes a window (an adopted one goes back to the host).
    WindowClose = 11, window_close { id: obj };
    WindowGeom = 12, window_geom { id: obj, geom: rect };
    /// `window_state`
    WindowState = 13, window_state { id: obj, state: u32 };
    WindowTitle = 14, window_title { id: obj, title: sid };
    WindowVisuals = 15, window_visuals { id: obj, visuals: u32, params: rect };
    WindowTopmost = 16, window_topmost { id: obj, on: bool };
    WindowButtons = 17, window_buttons { id: obj, show: bool };
    ChromelessWhenMax = 18, chromeless_when_max { id: obj, on: bool };
    Cursor = 19, cursor { id: obj, cursor: u32 };
    /// lock / pin / re-pin the pointer
    PointerLock = 20, pointer_lock { id: obj, mode: u32 };
    ShowInDock = 21, show_in_dock { on: bool };
    ActivateApp = 22, activate_app {};
    /// Geometry supplied by the compositor of a HOSTED window. The client consumes the
    /// host's geometry event itself; this updates HAL state without an OS request or echo.
    WindowHostedGeom = 23, window_hosted_geom { id: obj, geom: rect, dpi: f32 };
    /// Centers the native window buttons `y_center` points below the window's top edge.
    /// AppKit retains the center across geometry changes; other backends ignore it.
    WindowButtonsCenter = 24, window_buttons_center { id: obj, y_center: f32 };
    /// Session 0 over a lent window. With `layer` NONE: an opaque panel, in window-local
    /// points: while enabled session 0's window frames update a separate surface, composited
    /// after the borrower's passes, and input goes to session 0. With `layer` a texture of
    /// session 0 (BGRA8, the window's pixel size): the window layer -- the borrower's frames
    /// go to the retained app image instead of the window, session 0's frame is the window
    /// (it starts with the app image copied into `layer`, to sample) and input stays with the
    /// borrower; `rect` is ignored. Needs `info_flags::WINDOW_LAYERS`; it may be enabled on an
    /// adoptable window before it is lent (so no borrower frame reaches the window first).
    /// Disabling restores the retained app image.
    WindowOverlay = 25, window_overlay { id: obj, on: bool, rect: rect, layer: id };

    /// Whether the window draws its own caption, for an embedding parent.
    WindowChrome = 26, window_chrome { id: obj, own: bool };
    /// The caption the app draws in the window: the strip's top and height (window points; height
    /// zero: none), the window buttons in it (the app's own or the system's; zero: none), and
    /// whether the caption is dark. Sent when they change. Session 0 places what it draws in a
    /// lent window's caption (the JIT ring) by it; window systems ignore it.
    WindowCaption = 27, window_caption { id: obj, top: f32, height: f32, buttons: rect, dark: bool };

    // ---- text input
    ImeShow = 30, ime_show { id: obj, area: rect, config: u32 };
    ImeHide = 31, ime_hide { id: obj };
    /// The text field state: selection and composition ranges, text in the payload.
    ImeSync = 32, ime_sync { id: obj, sel_start: u32, sel_end: u32, comp_start: u32, comp_end: u32, text: rng };

    // ---- clipboard, drag, menu, dialogs
    ClipSet = 40, clip_set { text: rng };
    /// The Linux primary selection.
    SelectionSet = 41, selection_set { text: rng };
    /// A drag out of the app from window `id`: `items` holds `count` flat `DragItem`s, then the
    /// drag image, `image_w` x `image_h` RGBA8 rows (none when either is zero).
    DragStart = 42, drag_start { id: obj, count: u32, image_w: u32, image_h: u32, items: rng };
    /// `items`: `count` flat `MenuItem`s.
    MenuSet = 43, menu_set { count: u32, items: rng };
    /// `filters`: `count` flat strings (u32 length + bytes), a description and the comma-separated
    /// extensions of each filter in turn; `filename` the name a save dialog offers.
    FileDialog = 44, file_dialog { req: u32, kind: u32, title: sid, location: sid, filename: sid, count: u32, filters: rng };
    ShareText = 45, share_text { text: rng };
    /// The system's edit menu (Copy / Cut / Paste / Select All) at `area` (window points);
    /// `selection`: there is a selection to copy or cut.
    ClipboardActions = 46, clipboard_actions { id: obj, show: bool, area: rect, selection: bool };
    /// The application icon (the Dock's): `w` x `h` RGBA8 rows in the payload.
    AppIcon = 47, app_icon { w: u32, h: u32, rgba: rng };
    /// Read all clipboard representations; answered by `Clipboard` with this request id.
    /// `primary` selects Linux PRIMARY instead of CLIPBOARD; macOS ignores it.
    ClipRead = 48, clip_read { req: u64, primary: bool };
    /// A usage snapshot after the allocations preceding it; echoes the client's reservation epoch.
    GpuBudget = 49, gpu_budget { epoch: u64 };

    // ---- GPU resources
    /// `usage` (flags) is `usage::*`; `depth_mips` is [depth/layers, mip levels].
    TextureCreate = 50, texture_create { id: obj, usage: flags, format: u32, w: u32, h: u32, depth_mips: u16x2, attr: attr };
    /// Uploads `bytes` (borrowed for the submit call, tightly packed rows) into `rect` (x, y, w,
    /// h) of mip level `mip` of layer / cube face `layer`. A block format takes whole 4x4 blocks.
    TextureUpdate = 51, texture_update { id: obj, mip: flags, rect: urect, layer: u32, bytes: slice32 };
    TextureDestroy = 52, texture_destroy { id: obj };
    /// Answered by a `Readback` event with the same `req`.
    TextureReadback = 53, texture_readback { id: obj, req: u32, rect: urect };
    /// Exports the texture (IOSurface / dmabuf); answered by `TextureShared`.
    TextureShare = 54, texture_share { id: obj };
    /// A texture backed by a handle another process / the host owns (`texture_handle`).
    /// Answered by `TextureImported`; failed imports can be retried with the same id.
    TextureImport = 55, texture_import { id: obj, handle: u64, kind: u32 };
    /// `kind` (flags) is `buffer_kind::*`.
    BufferCreate = 56, buffer_create { id: obj, kind: flags, size: u64, attr: attr };
    BufferUpdate = 57, buffer_update { id: obj, off: u64, bytes: slice };
    BufferDestroy = 58, buffer_destroy { id: obj };
    /// `desc`: a `PipelineDesc` followed by the parts it names (see `buffers::PipelineSpec`).
    PipelineCreate = 59, pipeline_create { id: obj, dialect: u32, attr: attr, desc: rng };
    PipelineDestroy = 60, pipeline_destroy { id: obj };
    /// Retained instance data the HAL may drop under pressure (-> `BufferEvicted`).
    BufferEvictable = 61, buffer_evictable { id: obj };

    // ---- frames
    /// Render `packet` (index into `CmdBuf::frames`) to `target`; `kind` is `target::*`.
    Frame = 70, frame { target: obj, kind: u32, packet: u32 };
    /// A pick-mode frame request at `pos`; answered by `PickResult`.
    Pick = 71, pick { window: obj, req: u32, pos: v2 };
    /// Answered by a `Readback` event with the same `req`.
    Snapshot = 72, snapshot { window: obj, req: u32 };

    /// Answers a `DragQuery` (`to` = [`input::reply::QUERY`], `answer` = a `drag_query` value) or a
    /// `DragOver` (`to` = [`input::reply::OVER`], `answer` = a `drag_response` value).
    DragReply = 73, drag_reply { window: obj, to: u32, answer: u32 };

    /// The app has work for `window`: one `FrameReady` follows at the next vsync (a backend with
    /// a display clock; the others answer at once). Asked again after each frame while the app
    /// keeps animating, so an idle app costs no wake-ups.
    FrameRequest = 74, frame_request { id: obj };

    /// Copies the retained window frame to an existing BGRA8 texture, entirely on the GPU.
    /// Completion is a `Readback` with this `req`, status and dimensions, without pixel bytes.
    WindowCopy = 75, window_copy { window: obj, texture: id, req: u32 };

    // ---- automation
    /// The app's answer to a `Query` event.
    QueryReply = 80, query_reply { req: u32, bytes: rng };
    TraceMark = 81, trace_mark { text: sid };
    /// A budget the HAL checks against FrameDone / the ledger: `what` names it, `value` its limit.
    Budget = 82, budget { what: u32, value: u64 };

    // ---- embedded windows (a parent session and its child domains)
    /// Resizes the child's embedded window `window` (points) and sets its dpi; the child gets a
    /// `Geom`, the parent a `ChildWindow` with the new texture.
    ChildGeom = 90, child_geom { window: obj, child: u32, size: v2, dpi: f32 };
    /// Delivers an input event into the child's queue as if the OS had: `event` is
    /// `buffers::pack_forward` (the event and its payload); the window is `window`, in the
    /// child's window space.
    Forward = 91, forward { window: obj, child: u32, event: rng };
    /// Grants the child keyboard / IME / cursor / clipboard; zero releases all children.
    ChildFocus = 92, child_focus { child: u32 };
    /// Asks the child to close its embedded window (it gets `CloseRequested`).
    ChildClose = 93, child_close { window: obj, child: u32 };

    /// Answers a parent's correlated caption query.
    ChildDragReply = 94, child_drag_reply { window: obj, request: u32, answer: u32 };

    /// An application message to this embedded session's parent.
    ParentMessage = 95, parent_message { message: rng };

    // ---- domains (a privileged session only)
    /// Starts the app `app` (a string of this session's table) as a child domain; the host's
    /// registered `DomainHost` does it. `args` is a string of NUL-terminated argv entries.
    /// Progress comes back as `DomainState` events carrying `req`.
    DomainStart = 100, domain_start { req: u32, app: sid, args: sid };
    /// `domain_stop::QUIT` / `KILL`.
    DomainStop = 101, domain_stop { domain: u32, mode: u32 };
    /// Frees the domain's session and has the host start the app again under the same domain id.
    DomainRelaunch = 102, domain_relaunch { domain: u32 };
    /// Answered by a `DomainListed` event.
    DomainList = 103, domain_list { req: u32 };

    // ---- media (backend/apple/media; see `media`)
    /// Prepares a player for the source in `src` (`mode` is `video_mode::*`: autoplay, loop, audio
    /// only, the source kind). The picture lands in the textures named `tex` (BGRA) and `y` / `u` /
    /// `v` (planes) as frames arrive: the HAL makes their storage, the ids are the app's. An audio
    /// file names no textures. Answered by `VideoPrepared` or `VideoStatus` ERROR.
    VideoPrepare = 120, video_prepare { tex: obj, mode: flags, video: u64, y: id, u: id, v: id, src: rng };
    /// `video_op::*` on the player `video`; `arg` is the position (ms) of a seek, the f32 bits of
    /// a volume or rate, the track of a track selection.
    VideoControl = 121, video_control { video: u64, op: u32, arg: u64 };
    /// Opens (`options`: `audio_option::*`) the devices in `devices` (u64 ids, `media::pack_u64s`)
    /// for `dir` (`audio_dir::*`); the ones not listed are closed. The first call of a session
    /// makes its `AudioDevices` list.
    AudioUse = 122, audio_use { dir: u32, options: u32, devices: rng };
    /// Registers (or with `entry` 0 clears) the realtime entry of slot `index` of `dir`, see
    /// `media::AudioBlock`. When the command returns from `hal_submit` the HAL no longer calls the
    /// entry it replaced: the app may free that entry's `ctx` then.
    AudioEntry = 123, audio_entry { dir: u32, index: u32, entry: u64, ctx: u64 };
    /// `midi_op::*`. `ports` is `media::pack_u64s` of port ids.
    Midi = 124, midi { op: u32, port: u64, data: u32, ports: rng };
    /// `game_op::*`: game controller control. `id` names the device (a wheel's report, a pad's
    /// pulse), 0 for ENABLE.
    GameOut = 125, game_out { id: u64, op: u32, a: u32, b: u32, c: u32, d: u32, data: rng };
    /// `location_op::*`: the location service.
    Location = 126, location { op: u32 };
    /// `permission_op::*` for `permission_kind::*`; answered by a `Permission` event with `req`.
    Permission = 127, permission { op: u32, kind: u32, req: u32 };
    /// The cameras the slots read: `inputs` is `media::pack_u64s` of (camera, format) id pairs, one
    /// per slot; `consumers` the slots whose frames the app wants (`CameraFrame` events carry them).
    VideoInput = 129, video_input { consumers: u32, inputs: rng };

    // ---- fonts (the HAL's font service, HAL.md 13). Font ids are the app's own; glyph rasters
    // are shared by every session through the host's atlas.
    /// Opens a font under the app's id `font`: `kind` is `font::source` (a path the host maps,
    /// or the bytes), `desc` the `nvars` variation coordinates then the path or bytes (see
    /// `font::pack_font_open`). Answered by `FontOpened`; the first open of a session also
    /// brings `AtlasInfo` for the shared atlases.
    FontOpen = 130, font_open { font: u32, index: u32, kind: u32, nvars: u32, desc: rng };
    /// Asks for `count` glyphs (`font::pack_glyphs`) of an open font, `kind` is `font::kind`.
    /// Answered by `GlyphsReady`, in as many events as the raster takes.
    GlyphRequest = 131, glyph_request { font: u32, kind: u32, count: u32, glyphs: rng };
    /// The app no longer draws from `count` atlas slots (`font::pack_slots`): the HAL may take
    /// them back first when the atlas is full.
    GlyphRelease = 132, glyph_release { count: u32, slots: rng };

    // ---- direct display (only the session owning the desktop window)
    /// Set the primary/source connector (UTF-8). Answered by `DirectDisplayResult`.
    DirectDisplaySource = 133, direct_display_source { req: u64, name: rng };
    /// Connector names in desktop order (`buffers::pack_strs`). Answered by `DirectDisplayResult`.
    DirectDisplayOrder = 134, direct_display_order { req: u64, count: u32, names: rng };
    /// Two strings: connector and mode (empty = automatic). Answered by `DirectDisplayResult`.
    DirectDisplayMode = 135, direct_display_mode { req: u64, args: rng };
    /// Mouse or touchpad speed in hundredths; the host clamps it to the native preference range.
    DirectPointerSpeed = 136, direct_pointer_speed { touchpad: bool, hundredths: u32 };
    /// Natural scrolling for touchpads or mouse wheels (direct backend).
    DirectNaturalScroll = 137, direct_natural_scroll { touchpad: bool, on: bool };
    /// Touchpad tap to click (direct backend).
    DirectTapToClick = 149, direct_tap_to_click { on: bool };

    /// The window's UI dpi override, or 0 to restore its native dpi. AppKit uses the
    /// ratio to the backing scale to center native buttons in the app's zoomed caption.
    /// Other shells ignore this; layout scaling stays in Cx.
    WindowDpiOverride = 138, window_dpi_override { id: obj, dpi: f64w };

    // ---- the mobile system UI (the UIKit shell; other backends ignore them)
    /// `mobile_ui::*` and its value: the status bar's icon tone, the orientations the app
    /// allows, keeping the screen on, the screen edges whose system gestures wait for the app.
    MobileUi = 139, mobile_ui { op: u32, value: u32 };
    /// The text selection's drag handles in window `id` (window points): `op` is
    /// `selection_handles::*`, `start` / `end` the handles' anchor points (the bottom of the
    /// selection's first and last line).
    SelectionHandles = 140, selection_handles { id: obj, op: u32, start: v2, end: v2 };

    // ---- GL interop (the GLES backend; the app's GL context is in the HAL's share group, see
    // the `GlShare` event)
    /// A 2D GL texture of the share group named `name` stands for texture `id` (`format` RGBA8,
    /// R8 or RG8, `w` x `h`); the HAL deletes the name with the texture only when `owned`.
    /// Answered by `TextureImported`. A later adopt under the same id replaces the name.
    TextureAdoptGl = 141, texture_adopt_gl { id: obj, name: u32, format: u32, w: u32, h: u32, owned: bool };
    /// A fence (`GLsync`) the app's context made after writing adopted textures: the frames that
    /// follow wait for it on the GPU before they sample them. The HAL deletes it.
    GlFence = 142, gl_fence { sync: u64 };

    // ---- media, appended
    /// A system haptic (`media::haptic_kind::*`), the shell's click or tick feedback. Android only;
    /// the other backends ignore it.
    Haptic = 143, haptic { kind: u32 };
    /// The arrangement (`direct::DisplayArrangement`): mirror every direct output, optimised for
    /// the named connector (UTF-8), or empty = extended desktop. Answered by `DirectDisplayResult`.
    DirectDisplayArrangement = 144, direct_display_arrangement { req: u64, name: rng };
    /// The pacing setting (`direct::DisplayPacing`), packed strings: `auto`, `slowest`,
    /// `fastest` or `screen <name>`.
    DirectDisplayPacing = 145, direct_display_pacing { req: u64, args: rng };
    /// The app did not take a `BackPressed`: the host does what the system does by default
    /// (Android: finish the activity, or move its task to the back). Other backends ignore it.
    BackUnhandled = 148, back_unhandled {};
    /// The app drew from `count` atlas slots (`font::pack_slots`) since its last touch: when the
    /// atlas is full the HAL takes back the least recently DRAWN slots first (a slot's own use
    /// time is otherwise only its delivery).
    GlyphTouch = 150, glyph_touch { count: u32, slots: rng };

    // ---- system tray (status items)
    /// Creates or updates (in place) the app's tray `tray` (the app's own id): a `w` x `h`
    /// monochrome glyph, the `tooltip`, and its menu. `data` is `count` flat `MenuItem`s (all
    /// `parent` 0, `cmd` the item's id, `menu_kind::CHECKABLE`/`CHECKED` for check marks) followed
    /// by `w * h` alpha bytes (one per pixel, rows top-down). Chosen items answer as `TrayMenu`.
    /// `flags` (`tray_flags`): `DROPDOWN` = a primary click answers `TrayClick` (the app opens its
    /// dropdown window) and the menu opens on a secondary click.
    TraySet = 151, tray_set { tray: u64, w: u32, h: u32, tooltip: sid, count: u32, flags: u32, data: rng };
    /// Removes the app's tray `tray`.
    TrayRemove = 152, tray_remove { tray: u64 };
    /// The virtual display (`direct::VirtualDisplay`), packed strings: `WxH@Hz`
    /// (`VirtualDisplay::mode_string`) and `mirror` or `off` (what the panels show); no strings = the panels' own
    /// modes again. Answered by `DirectDisplayResult`.
    DirectDisplayVirtual = 153, direct_display_virtual { req: u64, count: u32, args: rng };
    /// Sizes window `id` (a `window_kind::DROPDOWN`) to `w` x `h` points, fitted into the visible
    /// area of the screen with tray `tray`'s icon, places it there (under the macOS status item,
    /// next to the Windows notification icon), shows it in front and gives it the focus. Its focus
    /// loss (a click anywhere else) answers `PopupDismissed`.
    WindowAtTray = 154, window_at_tray { id: obj, tray: u64, w: f32, h: f32 };
    /// The eyes XR window `id`'s next frame shows with, when they are not its `XrFrame`'s (a
    /// picture rendered elsewhere, for other eyes): `eyes` is two `XrEye` (`buffers::Wire`) and
    /// the depth (metres along the view, f32 little-endian; 0 none) of what the frame shows
    /// there. The shell gives them to the display (the runtime reprojects from them to the
    /// display's pose, positionally at that depth).
    XrFrameEyes = 155, xr_frame_eyes { id: obj, eyes: rng };
    /// XR window `id`'s `XrFrame`s are located `lead_ns` past their display time (head, eyes,
    /// hands, controllers): a picture rendered elsewhere from them shows that much later (a
    /// stream's measured latency). 0: at the display time.
    XrPoseLead = 156, xr_pose_lead { id: obj, lead_ns: u64 };
    /// XR window `id`'s headset runs its display at the supported refresh rate nearest to
    /// `millihertz` (0: the runtime's default).
    XrRefreshRate = 157, xr_refresh_rate { id: obj, millihertz: u32 };

    /// Creates native content (`native_layer::kind`). WEB: UTF-8 initial URL; CAMERA_PREVIEW:
    /// little-endian u64 input id followed by u64 format id. Placement comes from frame layers.
    NativeLayerCreate = 160, native_layer_create { id: obj, kind: u32, params: rng };
    /// Changes native content (`native_layer::op`); never changes its frame geometry.
    NativeLayerOp = 161, native_layer_op { id: obj, op: u32, arg: u64, data: rng };
    /// Releases the layer's content and its native resources.
    NativeLayerDestroy = 162, native_layer_destroy { id: obj };
}
