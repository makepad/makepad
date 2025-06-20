use crate::{
    makepad_derive_widget::*,
    debug_view::DebugView,
    performance_view::PerformanceView,
    makepad_draw::*,
    nav_control::NavControl,
    desktop_button::*,
    view::*,
    widget::*,
};

live_design!{
    link widgets;
    use link::widgets::*;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    pub WindowBase = {{Window}} {demo:false}
    pub Window = <WindowBase> {
        pass: { clear_color: (THEME_COLOR_BG_APP) }
        flow: Down
        nav_control: <NavControl> {}
        caption_bar = <SolidView> {
            visible: false,
            
            flow: Right
            
            draw_bg: {color: (THEME_COLOR_APP_CAPTION_BAR)}
            height: 27,
            caption_label = <View> {
                width: Fill, height: Fill,
                align: {x: 0.5, y: 0.5},
                label = <Label> {text: "Makepad", margin: {left: 100}}
            }
            windows_buttons = <View> {
                visible: false,
                width: Fit, height: Fit,
                min = <DesktopButton> {draw_bg: {button_type: WindowsMin}}
                max = <DesktopButton> {draw_bg: {button_type: WindowsMax}}
                close = <DesktopButton> {draw_bg: {button_type: WindowsClose}}
            }
            web_fullscreen = <View> {
                visible: false,
                width: Fit, height: Fit,
                fullscreen = <DesktopButton> {draw_bg: {button_type: Fullscreen}}
            }
            web_xr = <View> {
                visible: false,
                width: Fit, height: Fit,
                xr_on = <DesktopButton> {draw_bg: {button_type: XRMode}}
            }
        }
        
        window_menu = <WindowMenu> {
            main = Main{items:[app]}
            app = Sub { name:"Makepad", items:[quit] }
            quit = Item {
                name:"Quit",
                shift: false,
                key: KeyQ,
                enabled: true
            }
        }
        body = <KeyboardView> {
            width: Fill, height: Fill,
            keyboard_min_shift: 30,
        }
        
        cursor: Default
        mouse_cursor_size: vec2(20, 20),
        draw_cursor: {
            uniform border_size: 1.5
            uniform color: (THEME_COLOR_CURSOR)
            uniform border_color: (THEME_COLOR_CURSOR_BORDER)
            
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
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result
            }
        }
        window: {
            inner_size: vec2(1024, 768)
        }
    }
    
}

#[derive(Live, Widget)]
pub struct Window {
    //#[rust] caption_size: DVec2,
    #[live] last_mouse_pos: DVec2,
    #[live] mouse_cursor_size: DVec2,
    #[live] demo: bool,
    #[rust] demo_next_frame: NextFrame,
    #[live] cursor_draw_list: DrawList2d,
    #[live] draw_cursor: DrawQuad,
    #[live] debug_view: DebugView,
    #[live] performance_view: PerformanceView,
    #[live] nav_control: NavControl,
    #[live] window: WindowHandle,
    #[live] stdin_size: DrawColor,
    #[rust(Overlay::new(cx))] overlay: Overlay,
    #[rust(DrawList2d::new(cx))] main_draw_list: DrawList2d,
    #[live] pass: Pass,
    #[rust(Texture::new(cx))] depth_texture: Texture,
    #[live] hide_caption_on_fullscreen: bool, 
    #[live] show_performance_view: bool,
    #[rust(Mat4::nonuniform_scaled_translation(vec3(0.0004,-0.0004,-0.0004),vec3(-0.25,0.25,-0.5)))] xr_view_matrix: Mat4,
    #[deref] view: View,
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

impl LiveHook for Window {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.window.set_pass(cx, &self.pass);
        //self.pass.set_window_clear_color(cx, vec4(0.0,0.0,0.0,0.0));
        self.depth_texture = Texture::new_with_format(cx, TextureFormat::DepthD32{
            size:TextureSize::Auto,
            initial: true,
        });
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
        // check if we are ar/vr capable
        if cx.xr_capabilities().vr_supported {
            // lets show a VR button
            self.view(id!(web_xr)).set_visible(cx, true);
            log!("VR IS SUPPORTED");
        }
       
    }
    
    fn after_apply(&mut self, cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        if self.demo{
            self.demo_next_frame = cx.new_next_frame();
        }
        match cx.os_type() {
            OsType::Windows => {
                if !cx.in_makepad_studio(){
                    self.view(id!(caption_bar)).set_visible(cx, true);
                    self.view(id!(windows_buttons)).set_visible(cx, true);
                }
            }
            OsType::Macos => {
                //self.view(id!(caption_bar)).set_visible(true);
                //self.view(id!(windows_buttons)).set_visible(true);
                //self.view(id!(caption_bar)).set_visible(true);
                //self.view(id!(windows_buttons)).set_visible(true);
                /*if std::env::args().find(|v| v == "--message-format=json").is_some(){
                    self.apply_over(cx, live!{
                        caption_bar={draw_bg:{color:(vec4(0.,0.2,0.2,1.0))}}
                    }); 
                }*/
                                
                //draw_bg: {color: (THEME_COLOR_BG_APP)}  
                // self.frame.get_view(id!(caption_bar)).set_visible(false);
            }
            OsType::LinuxWindow(_) |
            OsType::LinuxDirect |
            OsType::Android(_) => {
                //self.frame.get_view(id!(caption_bar)).set_visible(false);
            }
            OsType::Web(_) => {
                // self.frame.get_view(id!(caption_bar)).set_visible(false);
            }
            _ => ()
        }
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum WindowAction {
    EventForOtherWindow,
    WindowClosed,
    WindowGeomChange(WindowGeomChangeEvent),
    None
}

impl Window {

    pub fn begin(&mut self, cx: &mut Cx2d) -> Redrawing {

        if !cx.will_redraw(&mut self.main_draw_list, Walk::default()) {
            return Redrawing::no()
        }
        
        cx.begin_pass(&self.pass, None);

        self.main_draw_list.begin_always(cx);
        
        let size = cx.current_pass_size();
        cx.begin_sized_turtle(size, Layout::flow_down());
        
        self.overlay.begin(cx);
        
        Redrawing::yes()
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        //while self.frame.draw_widget_continue(cx).is_not_done() {}
        self.debug_view.draw(cx);
        
        // lets draw our cursor
        if let OsType::LinuxDirect = cx.os_type() {
            self.cursor_draw_list.begin_overlay_last(cx);
            self.draw_cursor.draw_abs(cx, Rect {
                pos: self.last_mouse_pos,
                size: self.mouse_cursor_size
            });
            self.cursor_draw_list.end(cx);
        }
        
        self.overlay.end(cx);
        // lets get te pass size
        fn encode_size(x: f64)->Vec4{
            let x = x as usize;
            let r = ((x >> 8)&0xff) as f32 / 255.0;
            let b = ((x >> 0)&0xff) as f32 / 255.0;
            vec4(r,0.0,b,1.0)
        }
        
        // if we are running in stdin mode, write a tracking pixel with the pass size
        if cx.in_makepad_studio(){
            let df = cx.current_dpi_factor();
            let size = self.pass.size(cx).unwrap() * df;
            self.stdin_size.color = encode_size(size.x);
            self.stdin_size.draw_abs(cx, Rect{pos:dvec2(0.0,0.0),size:dvec2(1.0/df,1.0/df)});
            self.stdin_size.color = encode_size(size.y);
            self.stdin_size.draw_abs(cx, Rect{pos:dvec2(1.0/df,0.0),size:dvec2(1.0/df,1.0/df)});
        }

        if self.show_performance_view {
            self.performance_view.draw_all(cx, &mut Scope::empty());
        }

        cx.end_pass_sized_turtle();
        
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass);
    }
    pub fn resize(&self, cx: &mut Cx, size: DVec2) {
        self.window.resize(cx, size);
    }
    pub fn reposition(&self, cx: &mut Cx, size: DVec2) {
        self.window.reposition(cx, size);
    }
    pub fn set_fullscreen(&mut self, cx: &mut Cx) {
        self.window.fullscreen(cx);
    }
    pub fn configure_window(&mut self, cx: &mut Cx, inner_size: DVec2, position: DVec2, is_fullscreen: bool, title: String) {
        self.window.configure_window(cx, inner_size, position, is_fullscreen, title);
    }
}

impl WindowRef{
    pub fn get_inner_size(&self, cx:&Cx)->DVec2{
        if let Some(inner) = self.borrow(){
            inner.window.get_inner_size(cx)
        }
        else{
            dvec2(0.0,0.0)
        }
    }

    pub fn get_position(&self, cx:&Cx)->DVec2{
        if let Some(inner) = self.borrow(){
            inner.window.get_position(cx)
        }
        else{
            dvec2(0.0,0.0)
        }
    }
    pub fn is_fullscreen(&self, cx:&Cx)->bool{
        if let Some(inner) = self.borrow(){
            inner.window.is_fullscreen(cx)
        }
        else{
            false
        }
    }
    pub fn resize(&self, cx: &mut Cx, size: DVec2) {
        if let Some(inner) = self.borrow() {
            inner.resize(cx, size);
        }
    }

    pub fn reposition(&self, cx: &mut Cx, size: DVec2) {
        if let Some(inner) = self.borrow() {
            inner.reposition(cx, size);
        }
    }
    pub fn fullscreen(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_fullscreen(cx);
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
    pub fn configure_window(&self, cx: &mut Cx, inner_size: DVec2, position: DVec2, fullscreen: bool, title: String) {
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
            return
        }
        
        let uid = self.widget_uid();
        
        self.debug_view.handle_event(cx, event);
        if self.show_performance_view {
            self.performance_view.handle_widget(cx, event);
        }
        
        self.nav_control.handle_event(cx, event, self.main_draw_list.draw_list_id());
        self.overlay.handle_event(cx, event);
        if self.demo_next_frame.is_event(event).is_some(){
            if self.demo{
                self.demo_next_frame = cx.new_next_frame();
            }
            cx.repaint_pass_and_child_passes(self.pass.pass_id());
        }
        let is_for_other_window = match event {
            Event::WindowCloseRequested(ev) => ev.window_id != self.window.window_id(),
            Event::WindowClosed(ev) => {
                if ev.window_id == self.window.window_id() {
                    cx.widget_action(uid, &scope.path, WindowAction::WindowClosed)
                }
                true
            }
            Event::WindowGeomChange(ev) => {
                if ev.window_id == self.window.window_id() {
                    match cx.os_type() {
                        OsType::Windows | OsType::Macos => {
                            if self.hide_caption_on_fullscreen{
                                if ev.new_geom.is_fullscreen && !ev.old_geom.is_fullscreen {
                                    self.view(id!(caption_bar)).set_visible(cx, false);
                                }
                                else if !ev.new_geom.is_fullscreen && ev.old_geom.is_fullscreen {
                                    self.view(id!(caption_bar)).set_visible(cx, true);
                                };
                            }
                        }
                        _ => ()
                    }

                    // Update the display context if the screen size has changed
                    cx.display_context.screen_size = ev.new_geom.inner_size;
                    cx.display_context.updated_on_event_id = cx.event_id();

                    cx.widget_action(uid, &scope.path, WindowAction::WindowGeomChange(ev.clone()));
                    return
                }
                true
            },
            Event::WindowDragQuery(dq) => {
                if dq.window_id == self.window.window_id() {

                    if self.view(id!(caption_bar)).visible() {
                        let size = self.window.get_inner_size(cx);
                    
                        if dq.abs.y < 25. {
                            if dq.abs.x < size.x - 135.0 {
                                dq.response.set(WindowDragQueryResponse::Caption);
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
            _ => false
        };
        
        if is_for_other_window {
            cx.widget_action(uid, &scope.path, WindowAction::EventForOtherWindow);
            return
        }
        else {
            // lets store our inverse matrix
            if cx.in_xr_mode(){
                if let Event::XrUpdate(e) = &event{
                    let event = Event::XrLocal(XrLocalEvent::from_update_event(e, &self.xr_view_matrix));
                    self.view.handle_event(cx, &event, scope);
                }
                else{
                    self.view.handle_event(cx, event, scope);
                }
            }
            else{
                self.view.handle_event(cx, event, scope);
            }
        }
        
        if let Event::Actions(actions) = event{
            if self.desktop_button(id!(windows_buttons.min)).clicked(&actions) {
                self.window.minimize(cx);
            }
            if self.desktop_button(id!(windows_buttons.max)).clicked(&actions) {
                if self.window.is_fullscreen(cx) {
                    self.window.restore(cx);
                }
                else {
                    self.window.maximize(cx);
                }
            }
            if self.desktop_button(id!(windows_buttons.close)).clicked(&actions) {
                self.window.close(cx);
            }
            if self.desktop_button(id!(web_xr.xr_on)).clicked(&actions) {
                cx.xr_start_presenting();
            }
        }
                
        if let Event::ClearAtlasses = event {
            CxDraw::reset_icon_atlas(cx);
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
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk: Walk) -> DrawStep {
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
    
    fn draw_3d(&mut self, cx: &mut Cx3d, scope:&mut Scope)->DrawStep{
        // lets create a Cx2d in which we can draw. we dont support stepping here
        let cx = &mut Cx2d::new(cx.cx);
        
        self.main_draw_list.begin_always(cx);
        
        let size = dvec2(1500.0,1200.0);
        cx.begin_sized_turtle(size, Layout::flow_down());
                
        self.overlay.begin(cx);
        
        self.view.draw_walk_all(cx, scope, Walk::default());
        
        self.debug_view.draw(cx);
                        
        self.main_draw_list.set_view_transform(cx, &self.xr_view_matrix);
        
        cx.end_pass_sized_turtle();
                
        self.main_draw_list.end(cx);
        
        DrawStep::done()
    }
}
