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
    makepad_draw::*,
    nav_control::NavControl,
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
        detail_texture: texture_2d(float)
        detail_mix: uniform(0.82)

        sample_source: fn(uv: vec2) -> vec4 {
            return self.source_texture.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }

        sample_detail: fn(uv: vec2) -> vec4 {
            return self.detail_texture.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }

        pixel: fn() {
            let size = self.source_texture.size()
            let texel = vec2(
                1.0 / max(size.x, 1.0),
                1.0 / max(size.y, 1.0)
            )
            let uv = self.pos
            let smooth =
                self.sample_source(uv) * 0.25
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
            return smooth.mix(self.sample_detail(uv), clamp(self.detail_mix, 0.0, 1.0))
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

    mod.widgets.WindowBase = #(Window::register_widget(vm))
    mod.widgets.Window = set_type_default() do mod.widgets.WindowBase{
        demo: false
        show_caption_bar: true
        pass +: { clear_color: theme.color_bg_app }
        flow: Down
        nav_control: NavControl {}
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
                        color: #000, color_hover: #000, color_down: #000
                        bg_color_hover: #E9E9E9, bg_color_down: #CCCCCC
                    }
                }
                max := DesktopButton {
                    draw_bg.button_type: DesktopButtonType.WindowsMax
                    width: 46 height: 29
                    draw_bg +: {
                        color: #000, color_hover: #000, color_down: #000
                        bg_color_hover: #E9E9E9, bg_color_down: #CCCCCC
                    }
                }
                close := DesktopButton {
                    draw_bg.button_type: DesktopButtonType.WindowsClose
                    width: 46 height: 29
                    draw_bg +: {
                        color: #000, color_hover: #FFF, color_down: #FFF
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
    #[rust]
    use_gauss_capture: bool,
    #[rust]
    last_known_area: Area,
    #[rust(GaussStack::new(vm.cx_mut()))]
    gauss_stack: GaussStack,
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

struct GaussStackLevel {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
    smooth_pass: DrawPass,
    smooth_draw_list: DrawList2d,
    smooth_texture: Texture,
}

struct GaussStack {
    scene_pass: DrawPass,
    scene_draw_list: DrawList2d,
    scene_texture: Texture,
    _scene_depth_texture: Texture,
    levels: Vec<GaussStackLevel>,
}

fn gauss_render_texture_y_flip_for_os(os_type: &OsType) -> f32 {
    match os_type {
        OsType::Android(_) => 1.0,
        _ => 0.0,
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

        let mut levels = Vec::with_capacity(GAUSS_STACK_LEVELS);
        for index in 0..GAUSS_STACK_LEVELS {
            let pass = DrawPass::new_with_name(cx, &format!("gauss_mip_{index}"));
            let draw_list = DrawList2d::new(cx);
            let texture = Self::new_render_texture(cx);
            pass.set_color_texture(
                cx,
                &texture,
                DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
            );
            let smooth_pass = DrawPass::new_with_name(cx, &format!("gauss_smooth_mip_{index}"));
            let smooth_draw_list = DrawList2d::new(cx);
            let smooth_texture = Self::new_render_texture(cx);
            smooth_pass.set_color_texture(
                cx,
                &smooth_texture,
                DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
            );
            levels.push(GaussStackLevel {
                pass,
                draw_list,
                texture,
                smooth_pass,
                smooth_draw_list,
                smooth_texture,
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
                .enumerate()
                .map(|(index, level)| {
                    if index >= GAUSS_SMOOTH_LEVEL_START {
                        level.smooth_texture.clone()
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

    fn draw_high_blur_chain(
        &mut self,
        cx: &mut Cx2d,
        upsample: &mut DrawGaussUpsample,
        root_size: Vec2d,
    ) {
        if self.levels.is_empty() || GAUSS_SMOOTH_LEVEL_START >= self.levels.len() {
            return;
        }

        let dpi = cx.current_dpi_factor();
        let mut source_texture = self.levels[self.levels.len() - 1].texture.clone();

        for index in (GAUSS_SMOOTH_LEVEL_START..self.levels.len()).rev() {
            let level_size = Self::level_size(root_size, dpi, index);
            let level = &mut self.levels[index];
            level.smooth_pass.set_size(cx, level_size);
            cx.make_child_pass(&level.smooth_pass);
            cx.begin_pass(&level.smooth_pass, Some(dpi));
            level.smooth_draw_list.begin_always(cx);

            let pass_size = cx.current_pass_size();
            cx.begin_root_turtle(pass_size, Layout::flow_overlay());
            upsample.draw_vars.set_texture(0, &source_texture);
            upsample.draw_vars.set_texture(1, &level.texture);
            let detail_mix = if index == GAUSS_SMOOTH_LEVEL_START {
                0.90
            } else {
                0.78
            };
            upsample
                .draw_vars
                .set_uniform(cx, live_id!(detail_mix), &[detail_mix]);
            upsample.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: pass_size,
                },
            );
            cx.end_pass_sized_turtle();

            level.smooth_draw_list.end(cx);
            cx.end_pass(&level.smooth_pass);
            source_texture = level.smooth_texture.clone();
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

impl Window {
    fn sync_caption_bar_state(&mut self, cx: &mut Cx) {
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
                // Only show the caption bar if we're drawing our own window chrome
                // (e.g. Wayland without server-side decorations). On X11 the WM
                // provides native decorations, so we hide the in-app caption bar.
                let custom_chrome = params.custom_window_chrome;
                self.view(cx, ids!(caption_bar))
                    .set_visible(cx, self.show_caption_bar && custom_chrome);
                if custom_chrome {
                    self.view(cx, ids!(windows_buttons)).set_visible(cx, true);
                }
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
            inner.layout.padding.left = padding_left;
        }
        drop(caption_label);
    }

    fn sync_caption_title(&mut self, cx: &mut Cx) {
        let title = if self.window.title.is_empty() {
            cx.windows[self.window.handle.window_id()]
                .create_title
                .clone()
        } else {
            self.window.title.clone()
        };
        if !title.is_empty() {
            self.label(cx, ids!(caption_label.label))
                .set_text(cx, &title);
        }
    }

    /// Resolves the desired system-bar (status/navigation bar) icon tint and,
    /// when it changes, asks the platform to apply it.
    ///
    /// In `Auto` mode the tint follows the window background luminance: a light
    /// background needs dark icons for contrast, and vice versa. `DarkIcons` /
    /// `LightIcons` force the choice. Currently only Android honors this.
    fn sync_system_bar_appearance(&mut self, cx: &mut Cx) {
        if !matches!(cx.os_type(), OsType::Android(_)) {
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

        if self.use_gauss_capture {
            self.gauss_stack.begin_scene(cx);
            self.overlay
                .begin_for_pass(cx, self.pass.handle.draw_pass_id());
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

        if self.use_gauss_capture {
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
        }
        self.overlay.end(cx);
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

        cx.end_pass_sized_turtle();

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
}

impl WindowRef {
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
}

impl Widget for Window {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Draw(e) = event {
            let mut cx_draw = CxDraw::new(cx, e);
            let cx = &mut Cx2d::new(&mut cx_draw);
            self.draw_all(cx, scope);
            return;
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

                    // Update the display context if the screen size has changed
                    let old_insets = cx.display_context.safe_area_insets;
                    cx.display_context.screen_size = ev.new_geom.inner_size;
                    cx.display_context.safe_area_insets = ev.new_geom.safe_area_insets;
                    cx.display_context.updated_on_event_id = cx.event_id();

                    // Update safe area inset values on the script heap so
                    // Splash code can reference mod.widgets.SAFE_INSET_PAD_*.
                    cx.update_safe_inset_script_values(ev.new_geom.safe_area_insets);

                    // If the platform reports native chrome button geometry, derive
                    // the caption bar height so the buttons are vertically centered:
                    // height = top_margin * 2 + button_height = pos.y * 2 + size.y.
                    let new_buttons = ev.new_geom.window_chrome_buttons;
                    if new_buttons != Rect::default() {
                        let h = (new_buttons.pos.y * 2.0 + new_buttons.size.y).ceil();
                        if self.system_caption_bar_height != Some(h) {
                            self.system_caption_bar_height = Some(h);
                            self.view(cx, ids!(caption_bar)).redraw(cx);
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
                    if self.view(cx, ids!(caption_bar)).visible() {
                        let caption_rect = self.view(cx, ids!(caption_bar)).area().rect(cx);
                        let buttons_rect = self.view(cx, ids!(windows_buttons)).area().rect(cx);

                        if caption_rect.contains(dq.abs) {
                            if buttons_rect.size != Vec2d::default()
                                && buttons_rect.contains(dq.abs)
                            {
                                dq.response.set(WindowDragQueryResponse::Client);
                            } else {
                                dq.response.set(WindowDragQueryResponse::Caption);
                            }
                            cx.set_cursor(MouseCursor::Default);
                        }
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
            self.view.handle_event(cx, event, scope);
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
