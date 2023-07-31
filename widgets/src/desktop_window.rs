use crate::{
    makepad_derive_widget::*,
    debug_view::DebugView,
    makepad_draw::*,
    nav_control::NavControl,
   // window_menu::*,
    button::*,
    widget::*,
    frame::*,
};

live_design!{
    import makepad_widgets::theme::*;
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    import makepad_widgets::label::Label;
    import makepad_widgets::desktop_button::DesktopButton;
    
    DesktopWindow = {{DesktopWindow}} {
        pass: {clear_color: (COLOR_CLEAR)}
        
        layout: {
            flow: Down
        },
        caption_bar = <Solid> {
            visible: false,
            layout: {
                flow: Right
            },
            draw_bg: {color: (COLOR_BG_APP)}
            walk: {height: 29},
            caption_label = <Frame> {
                walk: {width: Fill, height: Fill}
                layout: {align: {x: 0.5, y: 0.5}},
                label = <Label> {label: "Makepad", walk: {margin: {left: 100}}}
            }
            windows_buttons = <Frame> {
                visible: false,
                walk: {width: Fit, height: Fit}
                min = <DesktopButton> {draw_bg:{button_type: WindowsMin}}
                max = <DesktopButton> {draw_bg:{button_type: WindowsMax}}
                close = <DesktopButton> {draw_bg:{button_type: WindowsClose}}
            }
            web_fullscreen = <Frame> {
                visible: false,
                walk: {width: Fit, height: Fit}
                fullscreen = <DesktopButton> {draw_bg:{button_type: Fullscreen}}
            }
            web_xr = <Frame> {
                visible: false,
                walk: {width: Fit, height: Fit}
                xr_on = <DesktopButton> {draw_bg:{button_type: XRMode}}
            }
        }
        
        mouse_cursor_size: vec2(20, 20),
        draw_cursor: {
            instance border_width: 1.5
            instance color: #000
            instance border_color: #fff
            
            fn get_color(self) -> vec4 {
                return self.color
            }
            
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.move_to(1.0, 1.0);
                sdf.line_to(self.rect_size.x - 1.0, self.rect_size.y * 0.5)
                sdf.line_to(self.rect_size.x * 0.5, self.rect_size.y - 1.0)
                sdf.close_path();
                sdf.fill_keep(self.get_color())
                if self.border_width > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_width)
                }
                return sdf.result
            }
        }
        window: {
            inner_size: vec2(1024, 768)
        }
    }
}

#[derive(Live)]
pub struct DesktopWindow {
    #[rust] caption_size: DVec2,
    #[live] last_mouse_pos: DVec2,
    #[live] mouse_cursor_size: DVec2,
    
    #[live] cursor_view: View,
    #[live] draw_cursor: DrawQuad,
    
    #[live] debug_view: DebugView,
    #[live] nav_control: NavControl,
    #[live] window: Window,
    #[live] overlay: Overlay,
    #[live] main_view: View,
    #[live] pass: Pass,
    #[live] depth_texture: Texture,
    
    #[deref] frame: Frame,
    
   // #[rust(WindowMenu::new(cx))] _window_menu: WindowMenu,
    /*#[rust(Menu::main(vec![
        Menu::sub("App", vec![
            //Menu::item("Quit App", Cx::command_quit()),
        ]),
    ]))]*/
    
    //#[live] _default_menu: Menu,
    
    //#[rust] last_menu: Option<Menu>,
    
    // testing
    #[rust] draw_state: DrawStateWrap<DrawState>,
    
}

#[derive(Clone)]
enum DrawState {
    Drawing,
}

impl LiveHook for DesktopWindow {
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, DesktopWindow)
    }

    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.window.set_pass(cx, &self.pass);
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
        // check if we are ar/vr capable
        if cx.xr_capabilities().vr_supported {
            // lets show a VR button
            self.frame.get_frame(id!(web_xr)).set_visible(true);
            log!("VR IS SUPPORTED");
        }
        match cx.os_type() {
            OsType::Windows => {
                self.frame.get_frame(id!(caption_bar)).set_visible(true);
                self.frame.get_frame(id!(windows_buttons)).set_visible(true);
            }
            OsType::Macos => {
               // self.frame.get_frame(id!(caption_bar)).set_visible(false);
            }
            OsType::LinuxWindow(_) |
            OsType::LinuxDirect |
            OsType::Android(_) => {
                //self.frame.get_frame(id!(caption_bar)).set_visible(false);
            }
            OsType::Web(_) => {
               // self.frame.get_frame(id!(caption_bar)).set_visible(false);
            }
            _ => ()
        }
    }
}

#[derive(Clone, WidgetAction)]
pub enum DesktopWindowAction {
    EventForOtherWindow,
    WindowClosed,
    WindowGeomChange(WindowGeomChangeEvent),
    FrameActions(Vec<WidgetActionItem>),
    None
}

impl DesktopWindow {
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, DesktopWindowAction)) {
        
        self.debug_view.handle_event(cx, event);
        self.nav_control.handle_event(cx, event, self.main_view.draw_list_id());
        self.overlay.handle_event(cx, event);
        
        let is_for_other_window = match event {
            Event::WindowCloseRequested(ev) => ev.window_id != self.window.window_id(),
            Event::WindowClosed(ev) => {
                if ev.window_id == self.window.window_id() {
                    return dispatch_action(cx, DesktopWindowAction::WindowClosed)
                }
                true
            }
            Event::WindowGeomChange(ev) => {
                if ev.window_id == self.window.window_id() {
                    return dispatch_action(cx, DesktopWindowAction::WindowGeomChange(ev.clone()))
                }
                true
            },
            Event::WindowDragQuery(dq) => {
                if dq.window_id == self.window.window_id() {
                    // alright we should query the caption area.
                    // we should build an api for that
                    if dq.abs.x < self.caption_size.x && dq.abs.y < self.caption_size.y {
                        if dq.abs.x < 50. {
                            dq.response.set(WindowDragQueryResponse::SysMenu);
                        }
                        else {
                            dq.response.set(WindowDragQueryResponse::Caption);
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
            _ => false
        };
        
        if is_for_other_window {
            return dispatch_action(cx, DesktopWindowAction::EventForOtherWindow)
        }
        else {
            let actions = self.frame.handle_widget_event(cx, event);
            if actions.not_empty() {
                if self.frame.get_button(id!(min)).clicked(&actions) {
                    self.window.minimize(cx);
                }
                if self.frame.get_button(id!(max)).clicked(&actions) {
                    if self.window.is_fullscreen(cx) {
                        self.window.restore(cx);
                    }
                    else {
                        self.window.maximize(cx);
                    }
                }
                if self.frame.get_button(id!(close)).clicked(&actions) {
                    self.window.close(cx);
                }
                if self.frame.get_button(id!(xr_on)).clicked(&actions) {
                    cx.xr_start_presenting();
                }
                dispatch_action(cx, DesktopWindowAction::FrameActions(actions));
            }
        }
        
        if let Event::Resume = event {
            Cx2d::reset_fonts_atlas(cx);
            Cx2d::reset_icon_atlas(cx);
        }
        if let Event::MouseMove(ev) = event {
            if let OsType::LinuxDirect = cx.os_type() {
                // ok move our mouse cursor
                self.last_mouse_pos = ev.abs;
                self.draw_cursor.update_abs(cx, Rect {
                    pos: ev.abs,
                    size: self.mouse_cursor_size
                })
            }
        }
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d) -> ViewRedrawing {
        if !cx.view_will_redraw(&mut self.main_view, Walk::default()) {
            return ViewRedrawing::no()
        }
        
        cx.begin_pass(&self.pass, None);
        
        self.main_view.begin_always(cx);
        
        cx.begin_pass_sized_turtle(Layout::flow_down());
        
        self.overlay.begin(cx);
        
        ViewRedrawing::yes()
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        //while self.frame.draw_widget_continue(cx).is_not_done() {}
        self.debug_view.draw(cx);
        
        // lets draw our cursor
        if let OsType::LinuxDirect = cx.os_type() {
            self.cursor_view.begin_overlay_last(cx);
            self.draw_cursor.draw_abs(cx, Rect {
                pos: self.last_mouse_pos,
                size: self.mouse_cursor_size
            });
            self.cursor_view.end(cx);
        }
        
        self.overlay.end(cx);
        cx.end_pass_sized_turtle();

        self.main_view.end(cx);
        cx.end_pass(&self.pass);
    }
}

impl Widget for DesktopWindow{
   fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            if let DesktopWindowAction::FrameActions(actions) = action{
               for action in actions{
                   dispatch_action(cx, action)
               } 
            }
            else{
                dispatch_action(cx, WidgetActionItem::new(action.into(),uid));
            }
        });
    }

    fn get_walk(&self)->Walk{Walk::default()}
    
    fn redraw(&mut self, cx:&mut Cx){
        self.frame.redraw(cx)
    }
        
    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.frame.find_widgets(path, cached, results);
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, _walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, DrawState::Drawing) {
            if self.begin(cx).is_not_redrawing() {
                self.draw_state.end();
                return WidgetDraw::done();
            }
        }
        
        if let Some(DrawState::Drawing) = self.draw_state.get(){
            self.frame.draw_widget(cx)?;
            self.draw_state.end();
            self.end(cx);
        }        
        
        WidgetDraw::done()
    }
}

