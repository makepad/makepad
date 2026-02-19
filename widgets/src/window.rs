use crate::{
    desktop_button::DesktopButtonWidgetExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    nav_control::NavControl,
    view::*,
    widget::*,
};
#[cfg(feature = "voice")]
use crate::{
    voice_wave::VoiceWaveWidgetExt,
    window_voice_input::{VoiceInjectEvent, WindowVoiceInput},
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

    mod.widgets.WindowBase = #(Window::register_widget(vm))
    mod.widgets.Window = set_type_default() do mod.widgets.WindowBase{
        demo: false
        pass +: { clear_color: theme.color_bg_app }
        flow: Down
        nav_control: NavControl {}
        caption_bar := SolidView {
            visible: false

            flow: Right

            draw_bg.color: theme.color_app_caption_bar
            height: 27
            caption_label := View {
                width: Fill height: Fill
                align: Center
                label := Label {text: "Makepad" margin: Inset{left: 100}}
            }
            voice_wave := VoiceWave {}
            windows_buttons := View {
                visible: false
                width: Fit height: Fit
                min := DesktopButton {draw_bg.button_type: DesktopButtonType.WindowsMin width: 46 height: 29}
                max := DesktopButton {draw_bg.button_type: DesktopButtonType.WindowsMax width: 46 height: 29}
                close := DesktopButton {draw_bg.button_type: DesktopButtonType.WindowsClose width: 46 height: 29}
            }
            web_fullscreen := View {
                visible: false
                width: Fit height: Fit
                fullscreen := DesktopButton {draw_bg.button_type: DesktopButtonType.Fullscreen width: 50 height: 36}
            }
            web_xr := View {
                visible: false
                width: Fit height: Fit
                xr_on := DesktopButton {draw_bg.button_type: DesktopButtonType.XRMode width: 50 height: 36}
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
    #[uid]
    uid: WidgetUid,
    //#[rust] caption_size: Vec2d,
    #[live]
    last_mouse_pos: Vec2d,
    #[live]
    mouse_cursor_size: Vec2d,
    #[live]
    demo: bool,
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
    #[rust]
    last_known_area: Area,
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
    #[cfg(feature = "voice")]
    #[rust]
    voice_input: WindowVoiceInput,
    #[rust]
    has_focus: bool,
    #[cfg(feature = "voice")]
    #[rust]
    ptt_f1_down: bool,
    #[cfg(feature = "voice")]
    #[rust]
    ptt_owns_capture: bool,
    #[rust(Mat4f::nonuniform_scaled_translation(vec3(0.0004,-0.0004,-0.0004),vec3(-0.25,0.25,-0.5)))]
    xr_view_matrix: Mat4f,
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
    #[cfg(feature = "voice")]
    RecordVoice(bool),
    #[default]
    None,
}

impl Window {
    #[cfg(feature = "voice")]
    fn voice_callback_index(&self) -> usize {
        self.window.window_id().id() % MAX_AUDIO_DEVICE_INDEX
    }

    #[cfg(feature = "voice")]
    fn key_focus_in_this_window(&self, cx: &Cx) -> bool {
        let Some(draw_list_id) = cx.key_focus().draw_list_id() else {
            return false;
        };
        let Some(draw_list) = cx.draw_lists.checked_index(draw_list_id) else {
            return false;
        };
        let Some(draw_pass_id) = draw_list.draw_pass_id else {
            return false;
        };
        cx.get_pass_window_id(draw_pass_id) == Some(self.window.window_id())
    }

    #[cfg(feature = "voice")]
    fn dispatch_voice_inject_events(
        &mut self,
        cx: &mut Cx,
        scope: &mut Scope,
        events: Vec<VoiceInjectEvent>,
    ) {
        for event in events {
            match event {
                VoiceInjectEvent::Text(chunk) => {
                    let text_input = Event::TextInput(TextInputEvent {
                        input: chunk,
                        replace_last: false,
                        was_paste: false,
                    });
                    self.view.handle_event(cx, &text_input, scope);
                }
                VoiceInjectEvent::Enter => {
                    let key = KeyEvent {
                        key_code: KeyCode::ReturnKey,
                        is_repeat: false,
                        modifiers: KeyModifiers::default(),
                        time: 0.0,
                    };
                    self.view.handle_event(cx, &Event::KeyDown(key), scope);
                    self.view.handle_event(cx, &Event::KeyUp(key), scope);
                }
            }
        }
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        #[cfg(feature = "voice")]
        self.voice_input
            .ensure_audio_callback(cx, self.voice_callback_index());

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

        // check if we are ar/vr capable
        if cx.xr_capabilities().vr_supported {
            // lets show a VR button
            self.view(cx, ids!(web_xr)).set_visible(cx, true);
        }

        // OS-specific caption bar setup
        if self.demo {
            self.demo_next_frame = cx.new_next_frame();
        }
        let linux_custom_window_chrome =
            matches!(cx.os_type(), OsType::LinuxWindow(params) if params.custom_window_chrome);

        match cx.os_type() {
            OsType::Windows => {
                self.view(cx, ids!(caption_bar)).set_visible(cx, true);
                self.view(cx, ids!(windows_buttons)).set_visible(cx, true);
            }
            OsType::Macos => {
                self.view(cx, ids!(caption_bar)).set_visible(cx, true);
            }
            OsType::LinuxWindow(_) => {
                self.view(cx, ids!(caption_bar)).set_visible(cx, true);
                if linux_custom_window_chrome {
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
        #[cfg(feature = "voice")]
        {
            let wave = self.voice_wave(cx, ids!(voice_wave));
            self.voice_input.sync_voice_wave_mic_state(cx, &wave);
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
        cx.begin_root_turtle(size, Layout::flow_down());

        self.overlay.begin(cx);

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

        self.overlay.end(cx);
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
    #[cfg(feature = "voice")]
    pub fn set_record_voice(&mut self, cx: &mut Cx, enabled: bool) {
        self.voice_input
            .ensure_audio_callback(cx, self.voice_callback_index());
        self.voice_input.set_enabled(cx, enabled);
        let wave = self.voice_wave(cx, ids!(voice_wave));
        self.voice_input.sync_voice_wave_mic_state(cx, &wave);
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
}

impl WindowRef {
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
    #[cfg(feature = "voice")]
    pub fn set_record_voice(&self, cx: &mut Cx, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_record_voice(cx, enabled);
        }
    }
    #[cfg(feature = "voice")]
    pub fn record_voice_toggled(&self, actions: &Actions) -> Option<bool> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let WindowAction::RecordVoice(v) = item.cast() {
                return Some(v);
            }
        }
        None
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

        #[cfg(feature = "voice")]
        {
            if let Event::AudioDevices(devices) = event {
                self.voice_input.handle_audio_devices(cx, devices);
                let wave = self.voice_wave(cx, ids!(voice_wave));
                self.voice_input.sync_voice_wave_mic_state(cx, &wave);
            }
            if let Event::PermissionResult(result) = event {
                if self.voice_input.handle_permission_result(cx, result) {
                    let wave = self.voice_wave(cx, ids!(voice_wave));
                    self.voice_input.sync_voice_wave_mic_state(cx, &wave);
                    cx.widget_action(uid, WindowAction::RecordVoice(self.voice_input.is_enabled()));
                }
            }
            if let Event::Signal = event {
                let wave = self.voice_wave(cx, ids!(voice_wave));
                let events = self.voice_input.process_signal(cx, &wave);
                self.dispatch_voice_inject_events(cx, scope, events);
            }
            {
                let wave = self.voice_wave(cx, ids!(voice_wave));
                let events = self.voice_input.handle_timers(cx, event, &wave);
                self.dispatch_voice_inject_events(cx, scope, events);
            }
            if let Event::Shutdown = event {
                self.voice_input.shutdown(cx);
            }
        }
        #[cfg(feature = "voice")]
        {
            if let Event::KeyDown(key_event) = event {
                let has_focus = self.has_focus || self.key_focus_in_this_window(cx);
                if key_event.key_code == KeyCode::F1 && !key_event.is_repeat && has_focus {
                    if !self.ptt_f1_down {
                        self.ptt_f1_down = true;
                        self.ptt_owns_capture = !self.voice_input.is_enabled();
                        if self.ptt_owns_capture {
                            self.set_record_voice(cx, true);
                            cx.widget_action(uid, WindowAction::RecordVoice(true));
                        }
                    }
                    return;
                }
                let is_hotkey = !key_event.is_repeat
                    && (key_event.modifiers.logo || key_event.modifiers.control)
                    && key_event.key_code == KeyCode::Key1;
                if is_hotkey && has_focus {
                    let enabled = !self.voice_input.is_enabled();
                    self.set_record_voice(cx, enabled);
                    cx.widget_action(uid, WindowAction::RecordVoice(enabled));
                    return;
                }
            }
            if let Event::KeyUp(key_event) = event {
                if key_event.key_code == KeyCode::F1 && self.ptt_f1_down {
                    self.ptt_f1_down = false;
                    if self.ptt_owns_capture {
                        self.ptt_owns_capture = false;
                        self.set_record_voice(cx, false);
                        self.voice_input.arm_enter_after_next_transcript();
                        cx.widget_action(uid, WindowAction::RecordVoice(false));
                    }
                    return;
                }
            }
        }

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
                    #[cfg(feature = "voice")]
                    self.voice_input.shutdown(cx);
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
                                    self.view(cx, ids!(caption_bar)).set_visible(cx, true);
                                };
                            }
                        }
                        _ => (),
                    }

                    // Update the display context if the screen size has changed
                    cx.display_context.screen_size = ev.new_geom.inner_size;
                    cx.display_context.updated_on_event_id = cx.event_id();

                    cx.widget_action(uid, WindowAction::WindowGeomChange(ev.clone()));
                    return;
                }
                true
            }
            Event::WindowDragQuery(dq) => {
                if dq.window_id == self.window.window_id() {
                    if self.view(cx, ids!(caption_bar)).visible() {
                        let size = self.window.get_inner_size(cx);

                        if dq.abs.y < 25. {
                            if dq.abs.x < size.x - 250.0 {
                                dq.response.set(WindowDragQueryResponse::Caption);
                            } else {
                                dq.response.set(WindowDragQueryResponse::Client);
                            }
                            cx.set_cursor(MouseCursor::Default);
                        }
                        /*
                        if dq.abs.x < self.caption_size.x && dq.abs.y < self.caption_size.y {
                        }*/
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
            // lets store our inverse matrix
            if cx.in_xr_mode() {
                if let Event::XrUpdate(e) = &event {
                    let event =
                        Event::XrLocal(XrLocalEvent::from_update_event(e, &self.xr_view_matrix));
                    self.view.handle_event(cx, &event, scope);
                } else {
                    self.view.handle_event(cx, event, scope);
                }
            } else {
                self.view.handle_event(cx, event, scope);
            }
        }

        if let Event::Actions(actions) = event {
            #[cfg(feature = "voice")]
            if self.voice_wave(cx, ids!(voice_wave)).clicked(&actions) {
                let enabled = !self.voice_input.is_enabled();
                self.set_record_voice(cx, enabled);
                cx.widget_action(uid, WindowAction::RecordVoice(enabled));
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
            if self
                .desktop_button(cx, ids!(web_xr.xr_on))
                .clicked(&actions)
            {
                cx.xr_start_presenting();
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

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        // lets create a Cx2d in which we can draw. we dont support stepping here
        let cx = &mut Cx2d::new(cx.cx);

        self.main_draw_list.begin_always(cx);

        let size = dvec2(1500.0, 1200.0);
        cx.begin_root_turtle(size, Layout::flow_down());

        self.overlay.begin(cx);

        self.view.draw_walk_all(cx, scope, Walk::default());

        //self.debug_view.draw(cx);

        self.main_draw_list
            .set_view_transform(cx, &self.xr_view_matrix);

        cx.end_pass_sized_turtle();

        self.main_draw_list.end(cx);

        DrawStep::done()
    }
}
