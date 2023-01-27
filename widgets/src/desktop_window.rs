use crate::{
    debug_view::DebugView,
    makepad_draw::*,
    nav_control::NavControl,
    window_menu::*,
    button::*,
    widget::*,
    frame::*,
};

live_design!{
    import crate::theme::*;
    registry Widget::*;
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
   
     DesktopWindow = {{DesktopWindow}} {
        pass: {clear_color: (COLOR_CLEAR)}
        var caption = "Makepad"
        ui: {
            layout: {
                flow: Down
            },
            caption_bar = <Solid> {
                layout: {
                    flow: Right
                },
                draw_bg: {color: (COLOR_BG_APP)}
                walk: {height: 29},
                caption_label = <Frame> {
                    walk: {width: Fill, height: Fill}
                    layout: {align: {x: 0.5, y: 0.5}},
                    <Label> {text: (caption), walk: {margin: {left: 100}}}
                }
                windows_buttons = <Frame> {
                    visible: false,
                    walk: {width: Fit, height: Fit}
                    min = <DesktopButton> {button_type: WindowsMin}
                    max = <DesktopButton> {button_type: WindowsMax}
                    close = <DesktopButton> {button_type: WindowsClose}
                }
                web_fullscreen = <Frame> {
                    visible: false,
                    walk: {width: Fit, height: Fit}
                    fullscreen = <DesktopButton> {button_type: Fullscreen}
                }
                web_xr = <Frame> {
                    visible: false,
                    walk: {width: Fit, height: Fit}
                    xr_on = <DesktopButton> {button_type: XRMode}
                }
            }
            inner_view = <Frame> {user_draw: true}
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
                sdf.move_to(1.0,1.0);
                sdf.line_to(self.rect_size.x-1.0, self.rect_size.y*0.5)
                sdf.line_to(self.rect_size.x*0.5, self.rect_size.y-1.0)
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
    #[rust] pub caption_size: DVec2,
    last_mouse_pos: DVec2,
    mouse_cursor_size: DVec2,
    draw_cursor: DrawQuad,
    debug_view: DebugView,
    nav_control: NavControl,
    window: Window,
    overlay: Overlay,
    main_view: View,
    pass: Pass,
    depth_texture: Texture,
    
    ui: FrameRef,
    
    #[rust(WindowMenu::new(cx))] pub window_menu: WindowMenu,
    #[rust(Menu::main(vec![
        Menu::sub("App", vec![
            //Menu::item("Quit App", Cx::command_quit()),
        ]),
    ]))]
    
    _default_menu: Menu,
    
    #[rust] pub last_menu: Option<Menu>,
    
    // testing
    #[rust] pub inner_over_chrome: bool,
}

impl LiveHook for DesktopWindow {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.window.set_pass(cx, &self.pass);
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
        // check if we are ar/vr capable
        if cx.xr_capabilities().vr_supported {
            // lets show a VR button
            self.ui.get_frame(id!(web_xr)).set_visible(true);
            log!("VR IS SUPPORTED");
        }
        if let OsType::Windows = cx.platform_type() {
            self.ui.get_frame(id!(caption_bar)).set_visible(true);
            self.ui.get_frame(id!(windows_buttons)).set_visible(true);
        }
        if let OsType::LinuxDirect = cx.platform_type() {
            self.ui.get_frame(id!(caption_bar)).set_visible(false);
        }
        if let OsType::LinuxWindow {custom_window_chrome: false} = cx.platform_type() {
            self.ui.get_frame(id!(caption_bar)).set_visible(false);
        }
    }
}

#[derive(Clone)]
pub enum DesktopWindowEvent {
    EventForOtherWindow,
    WindowClosed,
    WindowGeomChange(WindowGeomChangeEvent),
    None
}

impl DesktopWindow {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<DesktopWindowEvent> {
        let mut a = Vec::new();
        self.handle_event_fn(cx, event, &mut | _, v | a.push(v));
        a
    }
    
    pub fn handle_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, DesktopWindowEvent)) {
        
        self.debug_view.handle_event(cx, event);
        self.nav_control.handle_event(cx, event, self.main_view.draw_list_id());
        self.overlay.handle_event(cx, event);
        let actions = self.ui.handle_event(cx, event);
        if actions.not_empty() {
            if self.ui.get_button(id!(min)).clicked(&actions) {
                self.window.minimize(cx);
            }
            if self.ui.get_button(id!(max)).clicked(&actions) {
                if self.window.is_fullscreen(cx) {
                    self.window.restore(cx);
                }
                else {
                    self.window.maximize(cx);
                }
            }
            if self.ui.get_button(id!(close)).clicked(&actions) {
                self.window.close(cx);
            }
            if self.ui.get_button(id!(xr_on)).clicked(&actions) {
                cx.xr_start_presenting();
            }
        }
        /*
        if self.frame.get_button(ids!(min_btn)).clicked(&actions){
            
        }
        
        for item in self.frame.handle_event(cx, event) {
            if let ButtonAction::Click = item.action.cast() {match item.id() {
                live_id!(min_btn) => {
                    self.window.minimize(cx);
                }
                live_id!(max_btn) => {
                    if self.window.is_fullscreen(cx) {
                        self.window.restore(cx);
                    }
                    else {
                        self.window.maximize(cx);
                    }
                }
                live_id!(close_btn) => {
                    self.window.close(cx);
                }
                _ => ()
            }}
        }*/
        
        let is_for_other_window = match event {
            Event::WindowCloseRequested(ev) => ev.window_id != self.window.window_id(),
            Event::WindowClosed(ev) => {
                if ev.window_id == self.window.window_id() {
                    return dispatch_action(cx, DesktopWindowEvent::WindowClosed)
                }
                true
            }
            Event::WindowGeomChange(ev) => {
                if ev.window_id == self.window.window_id() {
                    return dispatch_action(cx, DesktopWindowEvent::WindowGeomChange(ev.clone()))
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
            Event::MouseDown(ev) => ev.window_id != self.window.window_id(),
            Event::MouseMove(ev) => ev.window_id != self.window.window_id(),
            Event::MouseUp(ev) => ev.window_id != self.window.window_id(),
            Event::Scroll(ev) => ev.window_id != self.window.window_id(),
            _ => false
        };
        if is_for_other_window {
            return dispatch_action(cx, DesktopWindowEvent::EventForOtherWindow)
        }
        if let Event::MouseMove(ev) = event {
            if let OsType::LinuxDirect = cx.platform_type() {
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
        if !cx.view_will_redraw(&self.main_view) {
            return ViewRedrawing::no()
        }
        
        cx.begin_pass(&self.pass);
        
        self.main_view.begin_always(cx);
        
        let pass_size = cx.current_pass_size();
        
        cx.begin_turtle(Walk::fixed_size(pass_size), Layout::flow_down());
        
        self.overlay.begin(cx);
        
        //while self.frame.draw(cx).is_ok(){}
        if self.ui.draw(cx).is_done() {
            self.end(cx);
            return ViewRedrawing::no()
        }
        ViewRedrawing::yes()
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        while self.ui.draw(cx).is_not_done() {}
        self.debug_view.draw(cx);
        // lets draw our cursor
        if let OsType::LinuxDirect = cx.platform_type() {
            self.draw_cursor.draw_abs(cx, Rect {
                pos: self.last_mouse_pos,
                size: self.mouse_cursor_size
            });
        }
        self.overlay.end(cx);
        
        cx.end_overlay_turtle();
        
        self.main_view.end(cx);
        cx.end_pass(&self.pass);
    }
}

