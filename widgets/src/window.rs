#[cfg(feature = "voice")]
use crate::voice_wave::VoiceWaveWidgetExt;
use crate::{
    desktop_button::DesktopButtonWidgetExt,
    gauss_view::{
        begin_window_gauss_frame, finish_window_gauss_frame, window_wants_gauss_capture,
        GaussBlurSnapshot, GAUSS_VIEW_LEVELS,
    },
    label::*,
    makepad_derive_widget::*,
    makepad_draw::shader::draw_sploded_hairline::DrawSplodedHairline,
    makepad_draw::*,
    nav_control::NavControl,
    screen_cap::ScreenCap,
    view::*,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.View
    use mod.widgets.SolidView
    use mod.widgets.Label
    use mod.widgets.DesktopButton
    use mod.widgets.DesktopButtonType
    use mod.widgets.KeyboardView
    use mod.widgets.WindowMenu
    use mod.widgets.NavControl
    use mod.widgets.ScreenCap
    use mod.widgets.Tweaker
    use mod.widgets.VoiceWave
    use mod.widgets.MenuItem
    use mod.draw.KeyCode

    set_type_default() do #(DrawGaussDownsample::script_shader(vm)){
        ..mod.draw.DrawQuad
        source_texture: texture_2d(float)

        sample_source: fn(uv: vec2) -> vec4 {
            return self.source_texture.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }

        pixel: fn() {
            let size = self.source_texture.size()
            let texel = vec2(
                1.0 / max(size.x, 1.0),
                1.0 / max(size.y, 1.0)
            )
            let uv = self.pos
            let color = self.sample_source(uv) * 0.125
                + (
                    self.sample_source(uv + texel * vec2(-2.0, 2.0))
                    + self.sample_source(uv + texel * vec2(2.0, 2.0))
                    + self.sample_source(uv + texel * vec2(-2.0, -2.0))
                    + self.sample_source(uv + texel * vec2(2.0, -2.0))
                ) * 0.03125
                + (
                    self.sample_source(uv + texel * vec2(0.0, 2.0))
                    + self.sample_source(uv + texel * vec2(-2.0, 0.0))
                    + self.sample_source(uv + texel * vec2(2.0, 0.0))
                    + self.sample_source(uv + texel * vec2(0.0, -2.0))
                ) * 0.0625
                + (
                    self.sample_source(uv + texel * vec2(-1.0, 1.0))
                    + self.sample_source(uv + texel * vec2(1.0, 1.0))
                    + self.sample_source(uv + texel * vec2(-1.0, -1.0))
                    + self.sample_source(uv + texel * vec2(1.0, -1.0))
                ) * 0.125
            return color
        }
    }

    set_type_default() do #(DrawGaussUpsample::script_shader(vm)){
        ..mod.draw.DrawQuad
        source_texture: texture_2d(float)

        sample_source: fn(uv: vec2) -> vec4 {
            return self.source_texture.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }

        pixel: fn() {
            // Tent offsets are HALF a source texel (one target texel at 2x upsample). The
            // exposed pyramid levels must keep a ratio-2 sigma ladder for the glass/tilt
            // log2(radius) mapping to stay smooth; full-texel offsets widen each re-home
            // stage enough to inflate deep levels ~35%, opening a visible blur jump at the
            // raw->re-homed level boundary.
            let size = self.source_texture.size()
            let texel = vec2(
                0.5 / max(size.x, 1.0),
                0.5 / max(size.y, 1.0)
            )
            let uv = self.pos
            return self.sample_source(uv) * 0.25
                + (
                    self.sample_source(uv + texel * vec2(1.0, 0.0))
                    + self.sample_source(uv + texel * vec2(-1.0, 0.0))
                    + self.sample_source(uv + texel * vec2(0.0, 1.0))
                    + self.sample_source(uv + texel * vec2(0.0, -1.0))
                ) * 0.125
                + (
                    self.sample_source(uv + texel * vec2(1.0, 1.0))
                    + self.sample_source(uv + texel * vec2(-1.0, 1.0))
                    + self.sample_source(uv + texel * vec2(1.0, -1.0))
                    + self.sample_source(uv + texel * vec2(-1.0, -1.0))
                ) * 0.0625
        }
    }

    set_type_default() do #(DrawGaussScene::script_shader(vm)){
        ..mod.draw.DrawQuad
        scene_texture: texture_2d(float)
        source_y_flip: uniform(0.0)

        pixel: fn() {
            let uv = vec2(self.pos.x, mix(self.pos.y, 1.0 - self.pos.y, self.source_y_flip))
            return self.scene_texture.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }
    }

    set_type_default() do #(DrawSsaaResolve::script_shader(vm)){
        ..mod.draw.DrawQuad
        scene_texture: texture_2d(float)
        source_y_flip: uniform(0.0)

        // Downscale-resolve into the window: one bilinear tap = a 2x2 box average at supersample 2.
        pixel: fn() {
            let uv = vec2(self.pos.x, mix(self.pos.y, 1.0 - self.pos.y, self.source_y_flip))
            return self.scene_texture.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }
    }

    mod.widgets.WindowBase = #(Window::register_widget(vm))
    mod.widgets.Window = set_type_default() do mod.widgets.WindowBase{
        demo: false
        show_caption_bar: true
        pass +: { clear_color: theme.color_bg_app }
        flow: Down
        nav_control: NavControl {}
        // SHIFT+F12 records this window to local/screencap/*.mp4, picture and
        // sound (widgets/src/screen_cap.rs). Hardcoded like the caption bar:
        // inert and free until the key is pressed.
        screen_cap: ScreenCap {}
        caption_bar := SolidView {
            visible: false

            flow: Right

            draw_bg.color: theme.color_app_caption_bar
            // Note: by default, the caption bar height is calculated at runtime
            // based on window chrome button geometry to ensure the buttons are vertically centered.
            // If you want to override this height with a fixed value, set the `caption_bar_height_override` on the Window itself.
            height: Fit
            caption_label := View {
                width: Fill height: Fill
                align: Center
                label := Label {text: "Makepad"}
            }
            voice_wave := VoiceWave {}
            windows_buttons := View {
                visible: false
                width: Fit height: Fit
                min := DesktopButton {
                    draw_bg.button_type: DesktopButtonType.WindowsMin
                    width: 46 height: 29
                    draw_bg +: {
                        color: theme.color_label_inner, color_hover: #000, color_down: #000
                        bg_color_hover: #E9E9E9, bg_color_down: #CCCCCC
                    }
                }
                max := DesktopButton {
                    draw_bg.button_type: DesktopButtonType.WindowsMax
                    width: 46 height: 29
                    draw_bg +: {
                        color: theme.color_label_inner, color_hover: #000, color_down: #000
                        bg_color_hover: #E9E9E9, bg_color_down: #CCCCCC
                    }
                }
                close := DesktopButton {
                    draw_bg.button_type: DesktopButtonType.WindowsClose
                    width: 46 height: 29
                    draw_bg +: {
                        color: theme.color_label_inner, color_hover: #FFF, color_down: #FFF
                        bg_color_hover: #E81123, bg_color_down: #F1707A
                    }
                }
            }
            web_fullscreen := View {
                visible: false
                width: Fit height: Fit
                fullscreen := DesktopButton {draw_bg.button_type: DesktopButtonType.Fullscreen width: 50 height: 36}
            }
        }
        window_menu := WindowMenu {
            main := MenuItem.Main{items:[@app_menu, @file_menu, @edit_menu, @selection_menu, @view_menu, @window_menu, @help_menu]}

            // App menu
            app_menu := MenuItem.Sub { name:"Makepad" items:[@about, @line1, @settings, @line2, @quit] }
            about := MenuItem.Item { name:"About Makepad" key: KeyCode.Escape enabled: true }
            line1 := MenuItem.Line {}
            settings := MenuItem.Item { name:"Settings..." key: KeyCode.Comma enabled: true }
            line2 := MenuItem.Line {}
            quit := MenuItem.Item { name:"Quit Makepad" key: KeyCode.KeyQ enabled: true }

            // File menu
            file_menu := MenuItem.Sub { name:"File" items:[@new_file, @new_window, @line3, @open, @line4, @save, @save_as, @line5, @close_editor, @close_window] }
            new_file := MenuItem.Item { name:"New File" key: KeyCode.KeyN enabled: true }
            new_window := MenuItem.Item { name:"New Window" shift: true key: KeyCode.KeyN enabled: true }
            line3 := MenuItem.Line {}
            open := MenuItem.Item { name:"Open..." key: KeyCode.KeyO enabled: true }
            line4 := MenuItem.Line {}
            save := MenuItem.Item { name:"Save" key: KeyCode.KeyS enabled: true }
            save_as := MenuItem.Item { name:"Save As..." shift: true key: KeyCode.KeyS enabled: true }
            line5 := MenuItem.Line {}
            close_editor := MenuItem.Item { name:"Close Editor" key: KeyCode.KeyW enabled: true }
            close_window := MenuItem.Item { name:"Close Window" shift: true key: KeyCode.KeyW enabled: true }

            // Edit menu
            edit_menu := MenuItem.Sub { name:"Edit" items:[@undo, @redo, @line6, @cut, @copy, @paste, @line7, @find, @replace, @line8, @find_in_files, @replace_in_files] }
            undo := MenuItem.Item { name:"Undo" key: KeyCode.KeyZ enabled: true }
            redo := MenuItem.Item { name:"Redo" shift: true key: KeyCode.KeyZ enabled: true }
            line6 := MenuItem.Line {}
            cut := MenuItem.Item { name:"Cut" key: KeyCode.KeyX enabled: true }
            copy := MenuItem.Item { name:"Copy" key: KeyCode.KeyC enabled: true }
            paste := MenuItem.Item { name:"Paste" key: KeyCode.KeyV enabled: true }
            line7 := MenuItem.Line {}
            find := MenuItem.Item { name:"Find" key: KeyCode.KeyF enabled: true }
            replace := MenuItem.Item { name:"Replace" key: KeyCode.KeyH enabled: true }
            line8 := MenuItem.Line {}
            find_in_files := MenuItem.Item { name:"Find in Files" shift: true key: KeyCode.KeyF enabled: true }
            replace_in_files := MenuItem.Item { name:"Replace in Files" shift: true key: KeyCode.KeyH enabled: true }

            // Selection menu
            selection_menu := MenuItem.Sub { name:"Selection" items:[@select_all] }
            select_all := MenuItem.Item { name:"Select All" key: KeyCode.KeyA enabled: true }

            // View menu
            view_menu := MenuItem.Sub { name:"View" items:[@zoom_in, @zoom_out, @line9, @fullscreen] }
            zoom_in := MenuItem.Item { name:"Zoom In" key: KeyCode.Equals enabled: true }
            zoom_out := MenuItem.Item { name:"Zoom Out" key: KeyCode.Minus enabled: true }
            line9 := MenuItem.Line {}
            fullscreen := MenuItem.Item { name:"Enter Full Screen" key: KeyCode.ReturnKey enabled: true }

            // Window menu
            window_menu := MenuItem.Sub { name:"Window" items:[@minimize, @zoom, @line10, @all_to_front] }
            minimize := MenuItem.Item { name:"Minimize" key: KeyCode.KeyM enabled: true }
            zoom := MenuItem.Item { name:"Zoom" key: KeyCode.Escape enabled: true }
            line10 := MenuItem.Line {}
            all_to_front := MenuItem.Item { name:"Bring All to Front" key: KeyCode.Escape enabled: true }

            // Help menu
            help_menu := MenuItem.Sub { name:"Help" items:[@help_about] }
            help_about := MenuItem.Item { name:"Makepad Help" key: KeyCode.Escape enabled: true }
        }
        body := KeyboardView {
            width: Fill height: Fill
            keyboard_min_shift: 30
        }
        // The design-feedback overlay (widgets/src/tweaker.rs): hardcoded
        // like the caption bar, inert unless --remote, zero cost while off.
        tweaker := Tweaker {}

        cursor: MouseCursor.Default
        mouse_cursor_size: vec2(20 20)
        draw_cursor +: {
            border_size: uniform(1.5)
            color: uniform(theme.color_cursor)
            border_color: uniform(theme.color_cursor_border)

            get_color: fn() {
                return self.color
            }

            get_border_color: fn() {
                return self.border_color
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.move_to(1.0, 1.0)
                sdf.line_to(self.rect_size.x - 1.0, self.rect_size.y * 0.5)
                sdf.line_to(self.rect_size.x * 0.5, self.rect_size.y - 1.0)
                sdf.close_path()
                sdf.fill_keep(self.get_color())
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result
            }
        }
        window +: {
            inner_size: vec2(1024 768)
        }
    }

}

#[derive(Script, ScriptHook, Widget)]
pub struct Window {
    #[source]
    source: ScriptObjectRef,
    //#[rust] caption_size: Vec2d,
    #[live]
    last_mouse_pos: Vec2d,
    #[live]
    mouse_cursor_size: Vec2d,
    #[live]
    demo: bool,
    #[live]
    show_caption_bar: bool,
    #[rust]
    demo_next_frame: NextFrame,
    #[live]
    cursor_draw_list: DrawList2d,
    #[live]
    draw_cursor: DrawQuad,
    //#[live] debug_view: DebugView,
    //#[live] performance_view: PerformanceView,
    #[live]
    nav_control: NavControl,
    /// Shift+F12 screen recorder. Hardcoded here so every app can record
    /// itself; Window owns it so the capture sink can be bound to THIS
    /// window rather than whichever one presents first.
    #[live]
    screen_cap: ScreenCap,
    #[live]
    window: ScriptWindowHandle,
    #[live]
    stdin_size: DrawColor,
    #[live]
    draw_gauss_downsample: DrawGaussDownsample,
    #[live]
    draw_gauss_upsample: DrawGaussUpsample,
    #[live]
    draw_gauss_scene: DrawGaussScene,
    #[live]
    draw_ssaa_resolve: DrawSsaaResolve,
    #[rust]
    use_gauss_capture: bool,
    #[rust]
    use_ssaa: bool,
    /// The exploded z-layer view is routing this window's content through its
    /// own body pass this frame.
    #[rust]
    use_sploded: bool,
    #[rust]
    last_known_area: Area,
    #[rust(GaussStack::new(vm.cx_mut()))]
    gauss_stack: GaussStack,
    #[rust(SsaaStack::new(vm.cx_mut()))]
    ssaa_stack: SsaaStack,
    #[rust(SplodedStack::new(vm.cx_mut()))]
    sploded_stack: SplodedStack,
    #[new]
    overlay: Overlay,
    #[new]
    main_draw_list: DrawList2d,
    #[live]
    pass: ScriptDrawPass,
    #[new]
    depth_texture: Texture,
    #[live]
    hide_caption_on_fullscreen: bool,
    #[live]
    show_performance_view: bool,
    #[rust]
    has_focus: bool,
    /// The calculated value of the caption bar height, a value that will result in
    /// the window chrome buttons being nicely vertically centered within the caption bar.
    /// `None` means no geometry has been reported by the platform yet.
    #[rust]
    system_caption_bar_height: Option<f64>,
    /// The last system-bar (status/navigation bar) icon tint pushed to the
    /// platform: `Some(true)` for dark icons, `Some(false)` for light icons.
    /// Used to only emit a platform op when the resolved value actually changes.
    #[rust]
    system_bar_dark_icons: Option<bool>,
    /// Cached `(caption_bar visible, caption rect, buttons rect)` for `WindowDragQuery`. It is
    /// refreshed only after layout finishes, so a synchronous native hit-test between configure
    /// and redraw cannot preserve rectangles from the previous window size.
    #[rust]
    drag_query_cache: Option<(bool, Rect, Rect)>,
    /// Whether a completed draw has made this frame's areas authoritative, so a geometry
    /// computed now may be cached. Between a configure and the redraw that answers it the
    /// areas still describe the previous size, and a query in that window is answered live
    /// without being stored.
    #[rust]
    drag_query_layout_valid: bool,
    /// The caption-layout inputs (show_caption_bar, height override, system caption height) that
    /// `drag_query_cache` was last computed against. When they change without a platform
    /// `WindowGeomChange` (e.g. a live/DSL reload toggling the caption), we drop the cache in
    /// `ensure_initialized`.
    #[rust]
    caption_query_sig: Option<(bool, Option<f64>, Option<f64>)>,
    /// The resolved title last pushed to the caption label. `sync_caption_title` runs on
    /// every event via `ensure_initialized`, so it uses this to skip the widget-tree label
    /// lookup and `set_text` when the title is unchanged.
    #[rust]
    last_synced_title: Option<String>,
    #[deref]
    view: View,

    // testing
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    #[rust]
    initialized: bool,
}

#[derive(Clone)]
enum DrawState {
    Drawing,
}

#[derive(Clone, Debug, Default)]
pub enum WindowAction {
    EventForOtherWindow,
    WindowClosed,
    WindowGeomChange(WindowGeomChangeEvent),
    #[default]
    None,
}

const GAUSS_STACK_LEVELS: usize = GAUSS_VIEW_LEVELS;
const GAUSS_SMOOTH_LEVEL_START: usize = 3;
/// Deep mips are re-homed (tent-upsampled back) to this level's resolution before the glass
/// samples them. Without this, blur level 5/6 samples a 1/64-res texture stretched over the
/// window — the texel lattice and clamp-to-edge bands are clearly visible. With the floor at
/// 1/8 res, no on-screen sample ever comes from a texture coarser than 8 device px per texel.
const GAUSS_FLOOR_LEVEL: usize = 2;

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGaussDownsample {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGaussUpsample {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGaussScene {
    #[deref]
    draw_super: DrawQuad,
}

/// Resolve (downscale) shader for full-window supersampling: samples the supersized scene
/// texture into the window framebuffer. See `DrawSsaaResolve` shader in the Window script.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSsaaResolve {
    #[deref]
    draw_super: DrawQuad,
}

/// Full-window supersampling (SSAA) factor: renders the whole UI at NxN device pixels and
/// downscales it — clean AA but costly, so it's off by default (the analytic AA covers most of
/// it for free). Opt in with `MAKEPAD_SUPERSAMPLE` = 2 (clamped <=4).
fn supersample_factor() -> f64 {
    // Read the env var once and cache it: begin()/end() query this every frame per window, and
    // std::env::var allocates a String each call. The factor can't change over a process's life.
    static FACTOR: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *FACTOR.get_or_init(|| {
        std::env::var("MAKEPAD_SUPERSAMPLE")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(1.0)
            .clamp(1.0, 4.0)
    })
}

struct GaussSmoothStage {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
}

struct GaussStackLevel {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
    // One tent-upsample per resolution doubling from this level's own size back up to the
    // floor size; the last stage's texture is what the snapshot exposes. Empty for levels
    // at or above the floor resolution.
    smooth_stages: Vec<GaussSmoothStage>,
}

struct GaussStack {
    scene_pass: DrawPass,
    scene_draw_list: DrawList2d,
    scene_texture: Texture,
    _scene_depth_texture: Texture,
    levels: Vec<GaussStackLevel>,
}

fn gauss_fast() -> bool {
    thread_local! { static ON: bool = std::env::var_os("MAKEPAD_GAUSS_FAST").is_some(); }
    ON.with(|v| *v)
}

fn gauss_render_texture_y_flip_for_os(os_type: &OsType) -> f32 {
    match os_type {
        OsType::Android(_) => 1.0,
        _ => 0.0,
    }
}

fn classify_window_drag_query(
    visible: bool,
    caption_rect: Rect,
    buttons_rect: Rect,
    transitional_buttons_rect: Rect,
    point: Vec2d,
) -> WindowDragQueryResponse {
    if !visible {
        return WindowDragQueryResponse::NoAnswer;
    }
    let hits_buttons = (buttons_rect.size != Vec2d::default() && buttons_rect.contains(point))
        || (transitional_buttons_rect.size != Vec2d::default()
            && transitional_buttons_rect.contains(point));
    if hits_buttons {
        WindowDragQueryResponse::Client
    } else if caption_rect.contains(point) {
        WindowDragQueryResponse::Caption
    } else {
        WindowDragQueryResponse::NoAnswer
    }
}

fn configured_window_buttons_rect(buttons_rect: Rect, configured_rect: Rect) -> Rect {
    if buttons_rect.size == Vec2d::default() || configured_rect.size == Vec2d::default() {
        return configured_rect;
    }
    Rect {
        pos: dvec2(
            configured_rect.pos.x + configured_rect.size.x - buttons_rect.size.x,
            buttons_rect.pos.y,
        ),
        size: buttons_rect.size,
    }
}

fn configured_window_caption_rect(caption_rect: Rect, configured_size: Vec2d) -> Rect {
    if caption_rect.size == Vec2d::default() || configured_size == Vec2d::default() {
        return caption_rect;
    }
    Rect {
        pos: caption_rect.pos,
        size: dvec2(
            (configured_size.x - caption_rect.pos.x).max(0.0),
            caption_rect.size.y,
        ),
    }
}

impl GaussStack {
    fn new(cx: &mut Cx) -> Self {
        let scene_pass = DrawPass::new_with_name(cx, "gauss_scene");
        let scene_draw_list = DrawList2d::new(cx);
        let scene_texture = Self::new_render_texture(cx);
        let scene_depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        scene_pass.set_color_texture(
            cx,
            &scene_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
        );
        scene_pass.set_depth_texture(cx, &scene_depth_texture, DrawPassClearDepth::ClearWith(1.0));
        scene_pass.set_live_with_parent(cx, true);

        let mut levels = Vec::with_capacity(GAUSS_STACK_LEVELS);
        for index in 0..GAUSS_STACK_LEVELS {
            let pass = DrawPass::new_with_name(cx, &format!("gauss_mip_{index}"));
            pass.set_live_with_parent(cx, true);
            let draw_list = DrawList2d::new(cx);
            let texture = Self::new_render_texture(cx);
            pass.set_color_texture(
                cx,
                &texture,
                DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
            );
            let stage_count = if index >= GAUSS_SMOOTH_LEVEL_START {
                index - GAUSS_FLOOR_LEVEL
            } else {
                0
            };
            let mut smooth_stages = Vec::with_capacity(stage_count);
            for stage in 0..stage_count {
                let smooth_pass =
                    DrawPass::new_with_name(cx, &format!("gauss_smooth_mip_{index}_{stage}"));
                smooth_pass.set_live_with_parent(cx, true);
                let smooth_draw_list = DrawList2d::new(cx);
                let smooth_texture = Self::new_render_texture(cx);
                smooth_pass.set_color_texture(
                    cx,
                    &smooth_texture,
                    DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
                );
                smooth_stages.push(GaussSmoothStage {
                    pass: smooth_pass,
                    draw_list: smooth_draw_list,
                    texture: smooth_texture,
                });
            }
            levels.push(GaussStackLevel {
                pass,
                draw_list,
                texture,
                smooth_stages,
            });
        }

        Self {
            scene_pass,
            scene_draw_list,
            scene_texture,
            _scene_depth_texture: scene_depth_texture,
            levels,
        }
    }

    fn new_render_texture(cx: &mut Cx) -> Texture {
        Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        )
    }

    fn begin_scene(&mut self, cx: &mut Cx2d) {
        cx.make_child_pass(&self.scene_pass);
        cx.begin_pass(&self.scene_pass, None);
        self.scene_draw_list.begin_always(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
    }

    fn end_scene(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.scene_draw_list.end(cx);
        cx.end_pass(&self.scene_pass);
    }

    fn snapshot(&self, root_size: Vec2d, source_y_flip: f32, dpi_factor: f64) -> GaussBlurSnapshot {
        GaussBlurSnapshot {
            scene_texture: self.scene_texture.clone(),
            mip_textures: self
                .levels
                .iter()
                .map(|level| {
                    if let Some(stage) = level.smooth_stages.last() {
                        stage.texture.clone()
                    } else {
                        level.texture.clone()
                    }
                })
                .collect(),
            source_size: root_size,
            source_y_flip,
            dpi_factor,
        }
    }

    fn level_size(root_size: Vec2d, dpi: f64, index: usize) -> Vec2d {
        let min_logical_size = 1.0 / dpi.max(1.0);
        let scale = (1usize << (index + 1)) as f64;
        dvec2(
            (root_size.x / scale).max(min_logical_size),
            (root_size.y / scale).max(min_logical_size),
        )
    }

    fn draw_mip_chain(
        &mut self,
        cx: &mut Cx2d,
        downsample: &mut DrawGaussDownsample,
        root_size: Vec2d,
    ) {
        let dpi = cx.current_dpi_factor();
        let mut source_texture = self.scene_texture.clone();

        for (index, level) in self.levels.iter_mut().enumerate() {
            // MAKEPAD_GAUSS_FAST=1: probe rig — stop the chain early to
            // measure how much of a frame the pass COUNT itself costs.
            if gauss_fast() && index > 3 {
                break;
            }
            let level_size = Self::level_size(root_size, dpi, index);

            level.pass.set_size(cx, level_size);
            cx.make_child_pass(&level.pass);
            cx.begin_pass(&level.pass, Some(dpi));
            level.draw_list.begin_always(cx);

            let pass_size = cx.current_pass_size();
            cx.begin_root_turtle(pass_size, Layout::flow_overlay());
            downsample.draw_vars.set_texture(0, &source_texture);
            downsample.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: pass_size,
                },
            );
            cx.end_pass_sized_turtle();

            level.draw_list.end(cx);
            cx.end_pass(&level.pass);
            source_texture = level.texture.clone();
        }
    }

    // Re-home each deep mip at the floor resolution: starting from the level's own raw mip,
    // tent-upsample one resolution doubling at a time until the floor size is reached. The
    // progressive doubling matters — a single stretch from 1/64 straight to 1/8 would keep the
    // source's texel lattice; each doubling convolves another tent on top and gaussianizes it.
    fn draw_high_blur_chain(
        &mut self,
        cx: &mut Cx2d,
        upsample: &mut DrawGaussUpsample,
        root_size: Vec2d,
    ) {
        if gauss_fast() {
            return;
        }
        let dpi = cx.current_dpi_factor();
        for index in GAUSS_SMOOTH_LEVEL_START..self.levels.len() {
            let level = &mut self.levels[index];
            let mut source_texture = level.texture.clone();
            for (stage_index, stage) in level.smooth_stages.iter_mut().enumerate() {
                let stage_size = Self::level_size(root_size, dpi, index - 1 - stage_index);
                stage.pass.set_size(cx, stage_size);
                cx.make_child_pass(&stage.pass);
                cx.begin_pass(&stage.pass, Some(dpi));
                stage.draw_list.begin_always(cx);

                let pass_size = cx.current_pass_size();
                cx.begin_root_turtle(pass_size, Layout::flow_overlay());
                upsample.draw_vars.set_texture(0, &source_texture);
                upsample.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(0.0, 0.0),
                        size: pass_size,
                    },
                );
                cx.end_pass_sized_turtle();

                stage.draw_list.end(cx);
                cx.end_pass(&stage.pass);
                source_texture = stage.texture.clone();
            }
        }
    }

    fn draw_scene(&mut self, cx: &mut Cx2d, scene: &mut DrawGaussScene, root_size: Vec2d) {
        let source_y_flip = gauss_render_texture_y_flip_for_os(cx.os_type());
        scene
            .draw_vars
            .set_uniform(cx, live_id!(source_y_flip), &[source_y_flip]);
        scene.draw_vars.set_texture(0, &self.scene_texture);
        scene.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: root_size,
            },
        );
    }
}

/// Body target for the exploded z-layer view.
///
/// The explode is a camera on a PASS, so which pass it goes on decides what
/// tilts. Putting it on the window pass tilts everything — including the
/// tweaker's panel, which then cannot be read or clicked. So while the mode is
/// up the window's own content renders into this child pass, that pass carries
/// the explode camera, and the resulting texture is composited back into the
/// window pass by a flat quad. Overlays bound to the window pass — the panel,
/// its popups, tooltips — draw over that composite completely untouched.
///
/// The pass and its textures are allocated once with the window (recycling a
/// `DrawPass` at runtime leaks its render target on Metal), but a render target
/// is only ever allocated on the first pass draw, so an app that never opens
/// the mode pays no GPU memory for this.
struct SplodedStack {
    scene_pass: DrawPass,
    scene_draw_list: DrawList2d,
    scene_texture: Texture,
    _scene_depth_texture: Texture,
    /// The tweaker's hover / pinned outlines, drawn INSIDE the exploded pass
    /// on their widgets' own planes. Its own list so a hover change redraws
    /// the marks alone, not the app.
    mark_draw_list: DrawList2d,
    mark_outline: Option<Box<DrawSplodedHairline>>,
}

impl SplodedStack {
    fn new(cx: &mut Cx) -> Self {
        let scene_pass = DrawPass::new_with_name(cx, "sploded_body");
        let scene_draw_list = DrawList2d::new(cx);
        let scene_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        let scene_depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        scene_pass.set_color_texture(
            cx,
            &scene_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
        );
        scene_pass.set_depth_texture(cx, &scene_depth_texture, DrawPassClearDepth::ClearWith(1.0));
        scene_pass.set_live_with_parent(cx, true);
        Self {
            scene_pass,
            scene_draw_list,
            scene_texture,
            _scene_depth_texture: scene_depth_texture,
            mark_draw_list: DrawList2d::new(cx),
            mark_outline: None,
        }
    }

    /// The tweaker's marks, last in the body pass so they paint over the
    /// app on their planes. Each mark's draw call is created with
    /// `nesting_depth` set to the mark's level — that is the only thing that
    /// decides which plane a call renders on while the mode is up.
    fn draw_marks(&mut self, cx: &mut Cx2d) {
        cx.sploded_set_mark_list(self.mark_draw_list.id());
        // `begin_always`, like the scene list: a walk here would register a
        // deferred Fill on the pass root turtle moments before that turtle
        // ends, and the window's own deferred walks then resolve against the
        // wrong turtle (an index-out-of-bounds in `resolve_fill`, contained
        // at the display-link boundary — which reads as the app hanging).
        self.mark_draw_list.begin_always(cx);
        let (hover, pinned) = cx.sploded_marks();
        if hover.is_some() || pinned.is_some() {
            if self.mark_outline.is_none() {
                let outline =
                    cx.with_vm(|vm| DrawSplodedHairline::script_new_with_default(vm));
                self.mark_outline = Some(Box::new(outline));
            }
            let mut outline = self.mark_outline.take().unwrap();
            let saved_depth = cx.nesting_depth;
            if let Some(mark) = pinned {
                cx.nesting_depth = mark.level as usize;
                outline.draw_mark(cx, mark.rect, mark.level, 2.0, 2.5);
            }
            if let Some(mark) = hover {
                if Some(mark) != pinned {
                    cx.nesting_depth = mark.level as usize;
                    outline.draw_mark(cx, mark.rect, mark.level, 1.0, 1.5);
                }
            }
            cx.nesting_depth = saved_depth;
            self.mark_outline = Some(outline);
        }
        self.mark_draw_list.end(cx);
    }

    fn begin_scene(&mut self, cx: &mut Cx2d) {
        let dpi = cx.current_dpi_factor();
        let size = cx.current_pass_size();
        self.scene_pass.set_size(cx, size);
        cx.make_child_pass(&self.scene_pass);
        cx.begin_pass(&self.scene_pass, Some(dpi));
        // The explode camera goes on THIS pass and nowhere else. Setting it
        // here — before any content is emitted — also means the CPU-side slug
        // text matrix, which bakes `camera_view` at draw time, is correct on
        // the very first exploded frame instead of one frame late.
        let pass_id = self.scene_pass.draw_pass_id();
        let params = cx.sploded_params(size);
        cx.passes[pass_id].sploded = params;
        cx.passes[pass_id].set_ortho_matrix(dvec2(0.0, 0.0), size);
        self.scene_draw_list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        // Only the body explodes: scope frames are emitted into lists bound
        // to this pass, nowhere else (the panel stays flat and frameless).
        cx.sploded_scene = Some(pass_id);
    }

    fn end_scene(&mut self, cx: &mut Cx2d) {
        cx.sploded_scene = None;
        self.draw_marks(cx);
        cx.end_pass_sized_turtle();
        self.scene_draw_list.end(cx);
        cx.end_pass(&self.scene_pass);
    }

    /// Composite the exploded body back into the window pass, flat and 1:1.
    fn draw_resolve(&mut self, cx: &mut Cx2d, resolve: &mut DrawSsaaResolve, root_size: Vec2d) {
        // Same orientation as the gauss compositor, NOT the SSAA one: this
        // pass renders at the window's own dpi, so its texture comes back
        // top-down (grab-verified — the inverted flag renders the UI mirrored).
        let source_y_flip = gauss_render_texture_y_flip_for_os(cx.os_type());
        resolve
            .draw_vars
            .set_uniform(cx, live_id!(source_y_flip), &[source_y_flip]);
        resolve.draw_vars.set_texture(0, &self.scene_texture);
        resolve.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: root_size,
            },
        );
    }
}

/// Full-window supersampling target: render the UI into a `supersample`x offscreen pass, then a resolve quad downscales it.
struct SsaaStack {
    scene_pass: DrawPass,
    scene_draw_list: DrawList2d,
    scene_texture: Texture,
    _scene_depth_texture: Texture,
}

impl SsaaStack {
    fn new(cx: &mut Cx) -> Self {
        let scene_pass = DrawPass::new_with_name(cx, "ssaa_scene");
        let scene_draw_list = DrawList2d::new(cx);
        let scene_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        let scene_depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        scene_pass.set_color_texture(
            cx,
            &scene_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
        );
        scene_pass.set_depth_texture(cx, &scene_depth_texture, DrawPassClearDepth::ClearWith(1.0));
        scene_pass.set_live_with_parent(cx, true);
        Self {
            scene_pass,
            scene_draw_list,
            scene_texture,
            _scene_depth_texture: scene_depth_texture,
        }
    }

    /// Begin rendering the whole UI into the supersized scene pass. The dpi override
    /// (dpi * supersample) inflates only the render-target pixel density + viewport; the pass
    /// rect (logical layout size) is copied from the parent window pass, so layout/hit-testing
    /// are unchanged — only rasterization happens at higher resolution.
    fn begin_scene(&mut self, cx: &mut Cx2d, supersample: f64) {
        let dpi = cx.current_dpi_factor();
        // Logical size of the window pass. Set it explicitly on the scene pass (like the gauss
        // mip passes do): begin_pass(Some(dpi)) does NOT auto-inherit the parent rect, so without
        // set_size get_pass_rect returns None and the GL backend panics. With the dpi override =
        // dpi*supersample, the same logical size rasterizes into a supersample-x device texture.
        let size = cx.current_pass_size();
        self.scene_pass.set_size(cx, size);
        cx.make_child_pass(&self.scene_pass);
        cx.begin_pass(&self.scene_pass, Some(dpi * supersample));
        self.scene_draw_list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
    }

    fn end_scene(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.scene_draw_list.end(cx);
        cx.end_pass(&self.scene_pass);
    }

    /// Draw the single fullscreen resolve quad into the (now-active) window pass, sampling the
    /// supersized scene texture with LINEAR (== a 2x2 box for supersample==2).
    fn draw_resolve(&mut self, cx: &mut Cx2d, resolve: &mut DrawSsaaResolve, root_size: Vec2d) {
        // Scene texture is bottom-up — flip opposite to the gauss compositor or the UI shows upside-down.
        let source_y_flip = 1.0 - gauss_render_texture_y_flip_for_os(cx.os_type());
        resolve
            .draw_vars
            .set_uniform(cx, live_id!(source_y_flip), &[source_y_flip]);
        resolve.draw_vars.set_texture(0, &self.scene_texture);
        resolve.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: root_size,
            },
        );
    }
}

impl Window {
    fn sync_caption_bar_state(&mut self, cx: &mut Cx) {
        // Hosted inside studio: the studio chrome owns the window, never
        // show our own caption bar (a DSL hot-reload re-runs this sync).
        if cx.in_makepad_studio() {
            self.view(cx, ids!(caption_bar)).set_visible(cx, false);
            return;
        }
        match cx.os_type() {
            OsType::Windows => {
                self.view(cx, ids!(caption_bar))
                    .set_visible(cx, self.show_caption_bar);
                self.view(cx, ids!(windows_buttons)).set_visible(cx, true);
            }
            OsType::Macos => {
                // In macOS fullscreen, the OS provides its own auto-hiding
                // toolbar with traffic-light buttons, so hide our caption bar.
                let is_fullscreen = self.window.handle.is_fullscreen(cx);
                self.view(cx, ids!(caption_bar))
                    .set_visible(cx, self.show_caption_bar && !is_fullscreen);
            }
            OsType::LinuxWindow(params) => {
                // X11 uses WM decorations. Wayland decides per window from the
                // compositor's xdg-decoration configure event.
                let custom_chrome = params.custom_window_chrome
                    && self
                        .window
                        .handle
                        .uses_wayland_client_side_decorations(cx);
                let visible = self.show_caption_bar
                    && custom_chrome
                    && !self.window.handle.is_wayland_fullscreen(cx);
                self.view(cx, ids!(caption_bar))
                    .set_visible(cx, visible);
                self.view(cx, ids!(windows_buttons))
                    .set_visible(cx, visible);
            }
            OsType::LinuxDirect | OsType::Android(_) => {
                //self.frame.get_view(ids!(caption_bar)).set_visible(false);
            }
            OsType::Web(_) => {
                // self.frame.get_view(ids!(caption_bar)).set_visible(false);
            }
            _ => (),
        }
    }

    fn caption_drag_geometry(&self, cx: &mut Cx) -> (bool, Rect, Rect) {
        // Each `self.view` is a widget-tree walk, so the caption bar is resolved once
        // rather than once per field read.
        let caption = self.view(cx, ids!(caption_bar));
        let visible = caption.visible();
        let caption_rect = caption.area().rect(cx);
        let buttons = self.view(cx, ids!(windows_buttons));
        let buttons_rect = if buttons.visible() {
            buttons.area().rect(cx)
        } else {
            Rect::default()
        };
        (visible, caption_rect, buttons_rect)
    }

    fn sync_caption_bar_height(&mut self, cx: &mut Cx) {
        // Explicit DSL override takes priority, then system-calculated.
        let height = self
            .window
            .caption_bar_height_override
            .or(self.system_caption_bar_height);
        if let Some(h) = height {
            let caption_bar = self.view(cx, ids!(caption_bar));
            if let Some(mut bar) = caption_bar.borrow_mut() {
                bar.walk.height = Size::Fixed(h);
            }
            drop(caption_bar);
        }
    }

    /// Adjusts the caption label's left padding so that the title text appears
    /// centered in the full caption bar width when there's enough room.
    /// When the window is too narrow, the padding gracefully reduces to 0,
    /// transitioning to a left-aligned title.
    fn sync_caption_centering(&mut self, cx: &mut Cx) {
        let bar_width = self.view(cx, ids!(caption_bar)).area().rect(cx).size.x;
        let buttons_width = self.view(cx, ids!(windows_buttons)).area().rect(cx).size.x;

        if bar_width <= 0.0 {
            return; // No area info yet (first frame)
        }

        let fill_width = bar_width - buttons_width;
        // At wide widths: padding = buttons_width, so the label's center
        // aligns with the bar's center (truly centered).
        // At narrow widths: padding shrinks toward 0, so the title
        // shifts left to maximize the available text space.
        let padding_left = buttons_width.min((fill_width - buttons_width).max(0.0));

        let caption_label = self.view(cx, ids!(caption_label));
        if let Some(mut inner) = caption_label.borrow_mut() {
            // Redraw when the padding actually changes; nothing else re-lays-out
            // the caption label now that the title sync skips unchanged titles.
            if (inner.layout.padding.left - padding_left).abs() > 0.1 {
                inner.layout.padding.left = padding_left;
                inner.redraw(cx);
            }
        }
        drop(caption_label);
    }

    fn sync_caption_title(&mut self, cx: &mut Cx) {
        let title = if self.window.title.is_empty() {
            cx.windows[self.window.handle.window_id()].create_title.clone()
        } else {
            self.window.title.clone()
        };
        // Under `--remote` the title carries a `[remote]` tag, so a human who
        // finds this window lingering can tell it belongs to an agent. Apps that
        // draw their own caption bar must show it too, not just the OS title bar.
        // No-op (and idempotent) when the remote server is not running.
        let title = crate::makepad_platform::remote::tag_window_title(title);
        // Bail out early when the resolved title was already synced: pushing an
        // unchanged title through `set_text` every event would still cost a
        // widget-tree lookup per event.
        if self.last_synced_title.as_deref() == Some(title.as_str()) {
            return;
        }
        if !title.is_empty() {
            let label = self.label(cx, ids!(caption_label.label));
            if label.borrow().is_none() {
                // No caption label in the tree yet; retry on a later event.
                return;
            }
            label.set_text(cx, &title);
        }
        self.last_synced_title = Some(title);
    }

    /// Resolves the desired system-bar (status/navigation bar) icon tint and,
    /// when it changes, asks the platform to apply it.
    ///
    /// In `Auto` mode the tint follows the window background luminance: a light
    /// background needs dark icons for contrast, and vice versa. `DarkIcons` /
    /// `LightIcons` force the choice. Honored on Android and iOS (iOS has no
    /// separate navigation bar, so only the status bar is affected).
    fn sync_system_bar_appearance(&mut self, cx: &mut Cx) {
        if !matches!(cx.os_type(), OsType::Android(_) | OsType::Ios(_)) {
            return;
        }
        let dark_icons = match cx.display_context.system_bar_appearance {
            SystemBarAppearance::DarkIcons => true,
            SystemBarAppearance::LightIcons => false,
            SystemBarAppearance::Auto => {
                let c = self.pass.clear_color;
                // Rec.709 luma as a perceptual brightness estimate.
                let luma = 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
                luma > 0.5
            }
        };
        if self.system_bar_dark_icons != Some(dark_icons) {
            self.system_bar_dark_icons = Some(dark_icons);
            cx.push_unique_platform_op(CxOsOp::SetSystemBarDarkIcons(dark_icons));
        }
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        // If the inputs that drive caption layout changed without a platform WindowGeomChange
        // (e.g. a live/DSL reload toggling the caption bar or changing its height), the cached
        // WindowDragQuery geometry is stale — drop it so the next hit-test recomputes.
        let caption_sig = (
            self.show_caption_bar,
            self.window.caption_bar_height_override,
            self.system_caption_bar_height,
        );
        if self.caption_query_sig != Some(caption_sig) {
            self.caption_query_sig = Some(caption_sig);
            self.drag_query_cache = None;
            self.drag_query_layout_valid = false;
        }

        self.sync_caption_bar_state(cx);
        self.sync_caption_bar_height(cx);
        self.sync_caption_title(cx);
        self.sync_caption_centering(cx);
        self.sync_system_bar_appearance(cx);

        if self.initialized {
            return;
        }
        self.initialized = true;

        self.window.handle.set_pass(cx, &self.pass.handle);
        //self.pass.set_window_clear_color(cx, vec4(0.0,0.0,0.0,0.0));
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.handle.set_depth_texture(
            cx,
            &self.depth_texture,
            DrawPassClearDepth::ClearWith(1.0),
        );

        if self.demo {
            self.demo_next_frame = cx.new_next_frame();
        }
    }

    pub fn begin(&mut self, cx: &mut Cx2d) -> Redrawing {
        self.ensure_initialized(cx);

        let will_redraw = cx.will_redraw(&mut self.main_draw_list, Walk::default());
        if !will_redraw {
            return Redrawing::no();
        }

        cx.begin_pass(&self.pass.handle, None);

        self.main_draw_list.begin_always(cx);

        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_overlay());
        let window_id = self.window.handle.window_id();
        self.use_gauss_capture = window_wants_gauss_capture(cx, window_id);
        let source_y_flip = gauss_render_texture_y_flip_for_os(cx.os_type());
        let gauss_snapshot = if self.use_gauss_capture {
            Some(
                self.gauss_stack
                    .snapshot(size, source_y_flip, cx.current_dpi_factor()),
            )
        } else {
            None
        };
        begin_window_gauss_frame(cx, window_id, self.use_gauss_capture, gauss_snapshot);

        // Full-window supersampling: render everything (incl. the overlay = tooltips/modals/
        // context-menus) into the supersized scene pass, then downscale in end(). Skip when
        // gauss capture is active for this window (avoid nesting the two scene mechanisms).
        self.use_ssaa = !self.use_gauss_capture && supersample_factor() > 1.0;
        // The exploded view owns the body pass; it does not nest inside the
        // other two scene mechanisms.
        self.use_sploded =
            cx.sploded_active() && !self.use_gauss_capture && !self.use_ssaa;

        if self.use_sploded {
            self.sploded_stack.begin_scene(cx);
            // Bind the overlay to the WINDOW pass, so the tweaker's panel and
            // every popup composite flat over the exploded body.
            self.overlay
                .begin_for_pass(cx, self.pass.handle.draw_pass_id());
        } else if self.use_gauss_capture {
            self.gauss_stack.begin_scene(cx);
            self.overlay
                .begin_for_pass(cx, self.pass.handle.draw_pass_id());
        } else if self.use_ssaa {
            self.ssaa_stack.begin_scene(cx, supersample_factor());
            self.overlay
                .begin_for_pass(cx, self.ssaa_stack.scene_pass.draw_pass_id());
        } else {
            self.overlay.begin(cx);
        }

        Redrawing::yes()
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        //while self.frame.draw_widget_continue(cx).is_not_done() {}
        //self.debug_view.draw(cx);

        // lets draw our cursor
        if let OsType::LinuxDirect = cx.os_type() {
            self.cursor_draw_list.begin_overlay_last(cx);
            self.draw_cursor.draw_abs(
                cx,
                Rect {
                    pos: self.last_mouse_pos,
                    size: self.mouse_cursor_size,
                },
            );
            self.cursor_draw_list.end(cx);
        }

        if self.use_sploded {
            // End the body pass first, then composite it, then let the overlay
            // (panel, popups) close into the window pass ON TOP of it — the
            // gauss ordering, for the same reason.
            self.sploded_stack.end_scene(cx);
            let root_size = cx.current_pass_size();
            if root_size.x >= 0.5 && root_size.y >= 0.5 {
                self.sploded_stack
                    .draw_resolve(cx, &mut self.draw_ssaa_resolve, root_size);
            }
            self.overlay.end(cx);
        } else if self.use_gauss_capture {
            self.gauss_stack.end_scene(cx);
            let root_size = cx.current_pass_size();
            if root_size.x >= 0.5 && root_size.y >= 0.5 {
                self.gauss_stack
                    .draw_mip_chain(cx, &mut self.draw_gauss_downsample, root_size);
                self.gauss_stack
                    .draw_high_blur_chain(cx, &mut self.draw_gauss_upsample, root_size);
                self.gauss_stack
                    .draw_scene(cx, &mut self.draw_gauss_scene, root_size);
            }
            self.overlay.end(cx);
        } else if self.use_ssaa {
            // The overlay was begun for the scene pass, so finalize it BEFORE ending that pass.
            self.overlay.end(cx);
            self.ssaa_stack.end_scene(cx);
            // Now the window pass is active again; downscale the supersized scene into it.
            let root_size = cx.current_pass_size();
            if root_size.x >= 0.5 && root_size.y >= 0.5 {
                self.ssaa_stack
                    .draw_resolve(cx, &mut self.draw_ssaa_resolve, root_size);
            }
        } else {
            self.overlay.end(cx);
        }
        let window_id = self.window.handle.window_id();
        if finish_window_gauss_frame(cx, window_id) {
            cx.repaint_pass_and_child_passes(self.pass.handle.draw_pass_id());
        }

        // lets get te pass size
        fn encode_size(x: f64) -> Vec4f {
            let x = x as usize;
            let r = ((x >> 8) & 0xff) as f32 / 255.0;
            let b = ((x >> 0) & 0xff) as f32 / 255.0;
            vec4(r, 0.0, b, 1.0)
        }

        // if we are running in stdin mode, write a tracking pixel with the pass size
        if cx.in_makepad_studio() {
            let df = cx.current_dpi_factor();
            let size = self.pass.handle.size(cx).unwrap() * df;
            self.stdin_size.color = encode_size(size.x);
            self.stdin_size.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(1.0 / df, 1.0 / df),
                },
            );
            self.stdin_size.color = encode_size(size.y);
            self.stdin_size.draw_abs(
                cx,
                Rect {
                    pos: dvec2(1.0 / df, 0.0),
                    size: dvec2(1.0 / df, 1.0 / df),
                },
            );
        }

        //if self.show_performance_view {
        //    self.performance_view.draw_all(cx, &mut Scope::empty());
        //}

        // The REC dot goes on last, in the WINDOW pass, so it sits over every
        // scene mechanism (gauss / ssaa / sploded) and over the overlay - and
        // so it lands in the recording, which reads back this same pass.
        let pass_size = cx.current_pass_size();
        self.screen_cap.draw_indicator(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: pass_size,
            },
        );

        cx.end_pass_sized_turtle();

        // Areas are authoritative only after this frame's layout has completed, so this is
        // where a cached answer becomes allowed. Computing it here instead would charge
        // every frame for three widget-tree walks that only a drag query ever reads, and
        // most frames never see one. Dropping last frame's answer rather than keeping it
        // means the first query after any relayout still recomputes, whether or not the
        // relayout was one of the two that invalidate explicitly.
        self.drag_query_cache = None;
        self.drag_query_layout_valid = true;
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass.handle);
    }
    pub fn resize(&self, cx: &mut Cx, size: Vec2d) {
        self.window.handle.resize(cx, size);
    }
    pub fn reposition(&self, cx: &mut Cx, size: Vec2d) {
        self.window.handle.reposition(cx, size);
    }
    pub fn set_fullscreen(&mut self, cx: &mut Cx) {
        self.window.handle.fullscreen(cx);
    }
    pub fn configure_window(
        &mut self,
        cx: &mut Cx,
        inner_size: Vec2d,
        position: Vec2d,
        is_fullscreen: bool,
        title: String,
    ) {
        self.window
            .handle
            .configure_window(cx, inner_size, position, is_fullscreen, title);
    }

    pub fn configure_macos_window(&mut self, cx: &mut Cx, config: MacosWindowConfig) {
        self.window.handle.configure_macos_window(cx, config);
    }

    pub fn configure_wayland_decorations(
        &mut self,
        cx: &mut Cx,
        preference: WaylandDecorationPreference,
    ) {
        self.window
            .handle
            .configure_wayland_decorations(cx, preference);
    }

    pub fn window_index(&self) -> usize {
        self.window.handle.window_id().id()
    }

    pub fn position(&self, cx: &Cx) -> Vec2d {
        self.window.handle.get_position(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauss_render_texture_y_flip_is_platform_specific() {
        assert_eq!(gauss_render_texture_y_flip_for_os(&OsType::Macos), 0.0);
        assert_eq!(
            gauss_render_texture_y_flip_for_os(&OsType::Android(Default::default())),
            1.0
        );
    }

    #[test]
    fn native_button_geometry_wins_during_configure_to_draw_transition() {
        let stale_caption = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(800.0, 29.0),
        };
        let stale_buttons = Rect {
            pos: dvec2(662.0, 0.0),
            size: dvec2(138.0, 29.0),
        };
        let configured_buttons = Rect {
            pos: dvec2(1782.0, 0.0),
            size: dvec2(138.0, 29.0),
        };
        assert!(matches!(
            classify_window_drag_query(
                true,
                stale_caption,
                stale_buttons,
                configured_buttons,
                dvec2(1851.0, 14.0),
            ),
            WindowDragQueryResponse::Client
        ));
        assert!(matches!(
            classify_window_drag_query(
                true,
                stale_caption,
                stale_buttons,
                Rect::default(),
                dvec2(400.0, 14.0),
            ),
            WindowDragQueryResponse::Caption
        ));

        let zoomed_buttons = Rect {
            pos: dvec2(708.0, 0.0),
            size: dvec2(92.0, 19.0),
        };
        assert_eq!(
            configured_window_buttons_rect(zoomed_buttons, configured_buttons),
            Rect {
                pos: dvec2(1828.0, 0.0),
                size: dvec2(92.0, 19.0),
            }
        );
        let configured_caption =
            configured_window_caption_rect(stale_caption, dvec2(1920.0, 1080.0));
        assert!(matches!(
            classify_window_drag_query(
                true,
                configured_caption,
                stale_buttons,
                configured_buttons,
                dvec2(1200.0, 14.0),
            ),
            WindowDragQueryResponse::Caption
        ));
    }
}

impl WindowRef {
    pub fn set_title(&self, cx: &mut Cx, title: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.window.title == title {
                return;
            }
            inner.window.title = title.to_string();
            inner.window.handle.set_title(cx, title.to_string());
            inner.last_synced_title = None;
            inner.sync_caption_title(cx);
        }
    }

    pub fn window_id(&self) -> Option<WindowId> {
        self.borrow().map(|inner| inner.window.handle.window_id())
    }

    pub fn get_inner_size(&self, cx: &Cx) -> Vec2d {
        if let Some(inner) = self.borrow() {
            inner.window.handle.get_inner_size(cx)
        } else {
            dvec2(0.0, 0.0)
        }
    }

    pub fn get_position(&self, cx: &Cx) -> Vec2d {
        if let Some(inner) = self.borrow() {
            inner.window.handle.get_position(cx)
        } else {
            dvec2(0.0, 0.0)
        }
    }
    pub fn is_fullscreen(&self, cx: &Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.window.handle.is_fullscreen(cx)
        } else {
            false
        }
    }
    /// OS-native maximize (Windows: `ShowWindow(SW_MAXIMIZE)`; macOS: zoom).
    /// Unlike `fullscreen()`/`disable_fullscreen()` (which push
    /// `FullscreenWindow`/`NormalizeWindow` — not handled by every
    /// backend), `maximize`/`restore` push the ops the Windows backend
    /// actually implements, and `is_fullscreen()` reflects this state
    /// there too (`window_geom.is_fullscreen` mirrors `get_is_maximized`).
    pub fn maximize(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.window.handle.maximize(cx);
        }
    }
    /// See `maximize()`.
    pub fn restore(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.window.handle.restore(cx);
        }
    }
    /// See `WindowHandle::set_chromeless_when_maximized` (Windows only;
    /// other backends ignore it).
    pub fn set_chromeless_when_maximized(&self, cx: &mut Cx, chromeless: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.window.handle.set_chromeless_when_maximized(cx, chromeless);
        }
    }
    pub fn resize(&self, cx: &mut Cx, size: Vec2d) {
        if let Some(inner) = self.borrow() {
            inner.resize(cx, size);
        }
    }

    pub fn reposition(&self, cx: &mut Cx, size: Vec2d) {
        if let Some(inner) = self.borrow() {
            inner.reposition(cx, size);
        }
    }
    pub fn fullscreen(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_fullscreen(cx);
        }
    }
    pub fn disable_fullscreen(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.window.handle.normal(cx);
        }
    }
    /// Configure the window's size and position, and whether it's fullscreen or not.
    ///
    /// If `fullscreen` is `true`, the window will be set to the monitor's size and the
    /// `inner_size` and `position` arguments will be ignored.
    ///
    /// If `fullscreen` is `false`, the window will be set to the specified `inner_size`
    /// and positioned at `position` on the screen.
    ///
    /// The `title` argument sets the window's title bar text.
    ///
    /// This only works in app startup.
    pub fn configure_window(
        &self,
        cx: &mut Cx,
        inner_size: Vec2d,
        position: Vec2d,
        fullscreen: bool,
        title: String,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.configure_window(cx, inner_size, position, fullscreen, title);
        }
    }

    pub fn configure_macos_window(&self, cx: &mut Cx, config: MacosWindowConfig) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.configure_macos_window(cx, config);
        }
    }

    pub fn configure_wayland_decorations(
        &self,
        cx: &mut Cx,
        preference: WaylandDecorationPreference,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.configure_wayland_decorations(cx, preference);
        }
    }
}

impl Widget for Window {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Draw(e) = event {
            let mut cx_draw = CxDraw::new(cx, e);
            let cx = &mut Cx2d::new(&mut cx_draw);
            self.draw_all(cx, scope);
            return;
        }
        if matches!(event, Event::LiveEdit) {
            // A live reload can re-apply DSL text over the caption label, so
            // force the next sync to push the title again.
            self.last_synced_title = None;
        }
        self.ensure_initialized(cx);

        let uid = self.widget_uid();

        //self.debug_view.handle_event(cx, event);
        //if self.show_performance_view {
        //    self.performance_view.handle_widget(cx, event);
        //}

        self.nav_control
            .handle_event(cx, event, self.main_draw_list.draw_list_id());
        self.overlay.handle_event(cx, event);
        // The recorder is fed the raw event before focus routing, so Shift+F12
        // works while a text input holds the caret, and is told which window
        // it is recording so its capture sink follows THIS window.
        self.screen_cap.set_window_id(self.window.window_id().id());
        self.screen_cap.handle_event(cx, event, scope);
        if self.screen_cap.take_redraw_request() {
            // The REC dot appearing or disappearing is a change to the draw
            // lists, so it needs a real redraw — twice per recording.
            self.view.redraw(cx);
        }
        if self.screen_cap.take_repaint_request() {
            // A still app presents no frames, and a recorder with no frames is
            // an empty file. A pass repaint re-presents the existing draw lists
            // at frame rate without re-running the widget tree.
            cx.repaint_pass_and_child_passes(self.pass.handle.draw_pass_id());
        }
        if self.demo_next_frame.is_event(event).is_some() {
            if self.demo {
                self.demo_next_frame = cx.new_next_frame();
            }
            cx.repaint_pass_and_child_passes(self.pass.handle.draw_pass_id());
        }
        let is_for_other_window = match event {
            Event::WindowCloseRequested(ev) => ev.window_id != self.window.window_id(),
            Event::WindowClosed(ev) => {
                if ev.window_id == self.window.window_id() {
                    cx.widget_action(uid, WindowAction::WindowClosed)
                }
                true
            }
            Event::WindowGeomChange(ev) => {
                if ev.window_id == self.window.window_id() {
                    // The caption / buttons may have been re-laid-out; drop the WindowDragQuery
                    // geometry cache so it is recomputed on the next hit-test, and mark the
                    // areas non-authoritative until the redraw that answers this configure.
                    self.drag_query_cache = None;
                    self.drag_query_layout_valid = false;
                    match cx.os_type() {
                        OsType::Windows | OsType::Macos => {
                            if self.hide_caption_on_fullscreen && !cx.in_makepad_studio() {
                                if ev.new_geom.is_fullscreen && !ev.old_geom.is_fullscreen {
                                    self.view(cx, ids!(caption_bar)).set_visible(cx, false);
                                } else if !ev.new_geom.is_fullscreen && ev.old_geom.is_fullscreen {
                                    self.view(cx, ids!(caption_bar))
                                        .set_visible(cx, self.show_caption_bar);
                                };
                            }
                        }
                        _ => (),
                    }

                    // Update the display context if the screen size has changed.
                    // Some platforms send spurious zero-size geometry at startup (notably macOS);
                    // don't let it clobber a good size and flip adaptive layouts to their fallback.
                    let old_insets = cx.display_context.safe_area_insets;
                    if ev.new_geom.inner_size.x > 0.0 && ev.new_geom.inner_size.y > 0.0 {
                        cx.display_context.screen_size = ev.new_geom.inner_size;
                    }
                    cx.display_context.safe_area_insets = ev.new_geom.safe_area_insets;
                    cx.display_context.updated_on_event_id = cx.event_id();

                    // Update safe area inset values on the script heap so
                    // Splash code can reference mod.widgets.SAFE_INSET_PAD_*.
                    cx.update_safe_inset_script_values(ev.new_geom.safe_area_insets);

                    // Only pin the caption height on macOS: the buttons there are OS traffic lights
                    // (fixed size, don't zoom) that the title lines up with. Elsewhere we draw the
                    // buttons ourselves, so height: Fit lets the bar zoom along with them instead.
                    if matches!(cx.os_type(), OsType::Macos) {
                        let new_buttons = ev.new_geom.window_chrome_buttons;
                        if new_buttons != Rect::default() {
                            let h = (new_buttons.pos.y * 2.0 + new_buttons.size.y).ceil();
                            if self.system_caption_bar_height != Some(h) {
                                self.system_caption_bar_height = Some(h);
                                self.view(cx, ids!(caption_bar)).redraw(cx);
                            }
                        }
                    }

                    // If safe area insets changed, request a `script_mod`
                    // re-run so widget definitions that reference these
                    // primitives via `mod.widgets.SAFE_INSET_PAD_*` get the
                    // new values re-baked. `request_script_reapply` would
                    // not work here: those expressions are evaluated when
                    // `script_mod` runs, and `Apply::ScriptReapply` does
                    // not re-evaluate them.
                    if old_insets != ev.new_geom.safe_area_insets {
                        cx.request_live_edit();
                    }

                    cx.widget_action(uid, WindowAction::WindowGeomChange(ev.clone()));
                    return;
                }
                true
            }
            Event::WindowDragQuery(dq) => {
                if dq.window_id == self.window.window_id() {
                    // A native query can arrive synchronously after configure but before redraw.
                    // Use live areas for that one query, but only a completed draw may cache them.
                    let cache_ready = self.drag_query_cache.is_some() || self.drag_query_layout_valid;
                    let geometry = match self.drag_query_cache {
                        Some(cached) => cached,
                        None => {
                            let live = self.caption_drag_geometry(cx);
                            if self.drag_query_layout_valid {
                                self.drag_query_cache = Some(live);
                            }
                            live
                        }
                    };
                    let (visible, mut caption_rect, buttons_rect) = geometry;
                    if !cache_ready {
                        caption_rect = configured_window_caption_rect(
                            caption_rect,
                            cx.windows[dq.window_id].window_geom.inner_size,
                        );
                    }
                    let transitional_buttons = if cache_ready {
                        Rect::default()
                    } else {
                        configured_window_buttons_rect(
                            buttons_rect,
                            cx.windows[dq.window_id].window_geom.window_chrome_buttons,
                        )
                    };
                    match classify_window_drag_query(
                        visible,
                        caption_rect,
                        buttons_rect,
                        transitional_buttons,
                        dq.abs,
                    ) {
                        WindowDragQueryResponse::Client => {
                            // Button geometry wins even if the stale caption rect still has the
                            // previous width, and therefore also blocks native top-edge resize.
                            dq.response.set(WindowDragQueryResponse::Client);
                        }
                        WindowDragQueryResponse::Caption => {
                            dq.response.set(WindowDragQueryResponse::Caption);
                            cx.set_cursor(MouseCursor::Default);
                        }
                        WindowDragQueryResponse::NoAnswer | WindowDragQueryResponse::SysMenu => {}
                    }
                }
                true
            }
            Event::TouchUpdate(ev) => ev.window_id != self.window.window_id(),
            Event::MouseDown(ev) => ev.window_id != self.window.window_id(),
            Event::MouseMove(ev) => ev.window_id != self.window.window_id(),
            Event::MouseUp(ev) => ev.window_id != self.window.window_id(),
            Event::Scroll(ev) => ev.window_id != self.window.window_id(),
            Event::WindowGotFocus(window_id) => {
                if *window_id == self.window.window_id() {
                    self.has_focus = true;
                    cx.set_key_focus(self.last_known_area);
                }

                *window_id != self.window.window_id()
            }
            Event::WindowLostFocus(window_id) => {
                if *window_id == self.window.window_id() {
                    self.has_focus = false;
                    self.last_known_area = cx.key_focus();
                    cx.set_key_focus(Area::Empty);
                }

                *window_id != self.window.window_id()
            }
            _ => false,
        };

        if is_for_other_window {
            cx.widget_action(uid, WindowAction::EventForOtherWindow);
            return;
        } else {
            // Tweak mode swallows pointer events over the body before
            // ordinary dispatch (picking must never fire a Button); all the
            // logic lives in widgets/src/tweaker.rs.
            if !crate::tweaker::window_intercept(cx, event, &mut self.view, self.window.window_id())
            {
                self.view.handle_event(cx, event, scope);
            }
        }

        if let Event::Actions(actions) = event {
            #[cfg(feature = "voice")]
            {
                let voice_wave = self.voice_wave(cx, ids!(voice_wave));
                voice_wave.handle_actions(cx, actions, &mut self.view, scope);
            }
            if self
                .desktop_button(cx, ids!(windows_buttons.min))
                .clicked(&actions)
            {
                self.window.handle.minimize(cx);
            }
            if self
                .desktop_button(cx, ids!(windows_buttons.max))
                .clicked(&actions)
            {
                if self.window.handle.is_fullscreen(cx) {
                    self.window.handle.restore(cx);
                } else {
                    self.window.handle.maximize(cx);
                }
            }
            if self
                .desktop_button(cx, ids!(windows_buttons.close))
                .clicked(&actions)
            {
                self.window.handle.close(cx);
            }
        }

        //if let Event::ClearAtlasses = event {
        //    CxDraw::reset_icon_atlas(cx);
        //}

        if let Event::MouseMove(ev) = event {
            if let OsType::LinuxDirect = cx.os_type() {
                // ok move our mouse cursor
                self.last_mouse_pos = ev.abs;
                self.draw_cursor.update_abs(
                    cx,
                    Rect {
                        pos: ev.abs,
                        size: self.mouse_cursor_size,
                    },
                )
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, DrawState::Drawing) {
            if self.begin(cx).is_not_redrawing() {
                self.draw_state.end();
                return DrawStep::done();
            }
        }

        if let Some(DrawState::Drawing) = self.draw_state.get() {
            self.view.draw_walk(cx, scope, walk)?;
            self.draw_state.end();
            self.end(cx);
        }

        DrawStep::done()
    }
}
