use crate::{
    makepad_derive_widget::*,
    debug_view::DebugView,
    makepad_draw::*,
    nav_control::NavControl,
    button::*,
    view::*,
    widget::*,
};

live_design!{
    WindowBase = {{Window}} {demo:false}
}

#[derive(Live)]
pub struct Window {
    //#[rust] caption_size: DVec2,
    #[live] last_mouse_pos: DVec2,
    #[live] mouse_cursor_size: DVec2,
    #[live] demo: bool,
    #[rust] demo_next_frame: NextFrame,
    #[live] cursor_draw_list: DrawList2d,
    #[live] draw_cursor: DrawQuad,
    #[live] debug_view: DebugView,
    #[live] nav_control: NavControl,
    #[live] window: WindowHandle,
    #[live] stdin_size: DrawColor,
    #[live] overlay: Overlay,
    #[live] main_draw_list: DrawList2d,
    #[live] pass: Pass,
    #[rust(Texture::new(cx))] depth_texture: Texture,
    #[live] hide_caption_on_fullscreen: bool, 
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
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, Window)
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.window.set_pass(cx, &self.pass);
        //self.pass.set_window_clear_color(cx, vec4(0.0,0.0,0.0,0.0));
        self.depth_texture.set_format(cx, TextureFormat::DepthD32{
            size:TextureSize::Auto
        });
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
        // check if we are ar/vr capable
        if cx.xr_capabilities().vr_supported {
            // lets show a VR button
            self.view(id!(web_xr)).set_visible(true);
            log!("VR IS SUPPORTED");
        }
        match cx.os_type() {
            OsType::Windows => {
                if !cx.in_makepad_studio(){
                    self.view(id!(caption_bar)).set_visible(true);
                    self.view(id!(windows_buttons)).set_visible(true);
                }
            }
            OsType::Macos => {
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
    
    fn after_apply(&mut self, cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if self.demo{
            self.demo_next_frame = cx.new_next_frame();
        }
    }
}

#[derive(Clone, WidgetAction)]
pub enum WindowAction {
    EventForOtherWindow,
    WindowClosed,
    WindowGeomChange(WindowGeomChangeEvent),
    ViewActions(Vec<WidgetActionItem>),
    None
}

impl Window {
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WindowAction)) {
        
        self.debug_view.handle_event(cx, event);
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
                    return dispatch_action(cx, WindowAction::WindowClosed)
                }
                true
            }
            Event::WindowGeomChange(ev) => {
                if ev.window_id == self.window.window_id() {
                    match cx.os_type() {
                        OsType::Macos => {
                            if self.hide_caption_on_fullscreen{
                                if ev.new_geom.is_fullscreen && !ev.old_geom.is_fullscreen {
                                    self.view(id!(caption_bar)).set_visible(false);
                                    self.redraw(cx);
                                }
                                else if !ev.new_geom.is_fullscreen && ev.old_geom.is_fullscreen {
                                    self.view(id!(caption_bar)).set_visible(true);
                                    self.redraw(cx);
                                };
                            }
                        }
                        _ => ()
                    }
                    
                    return dispatch_action(cx, WindowAction::WindowGeomChange(ev.clone()))
                }
                true
            },
            Event::WindowDragQuery(dq) => {
                if dq.window_id == self.window.window_id() {
                    let size = self.window.get_inner_size(cx);
                    
                    if dq.abs.y < 25.{
                        if dq.abs.x < 50. {
                            dq.response.set(WindowDragQueryResponse::SysMenu);
                        }
                        else if dq.abs.x < size.x - 135.0{
                            dq.response.set(WindowDragQueryResponse::Caption);
                        }
                        cx.set_cursor(MouseCursor::Default);
                    }
                    /*
                    if dq.abs.x < self.caption_size.x && dq.abs.y < self.caption_size.y {
                    }*/
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
            return dispatch_action(cx, WindowAction::EventForOtherWindow)
        }
        else {
            let actions = self.view.handle_widget_event(cx, event);
            if actions.not_empty() {
                if self.button(id!(min)).clicked(&actions) {
                    self.window.minimize(cx);
                }
                if self.button(id!(max)).clicked(&actions) {
                    if self.window.is_fullscreen(cx) {
                        self.window.restore(cx);
                    }
                    else {
                        self.window.maximize(cx);
                    }
                }
                if self.button(id!(close)).clicked(&actions) {
                    self.window.close(cx);
                }
                if self.button(id!(xr_on)).clicked(&actions) {
                    cx.xr_start_presenting();
                }
                
                dispatch_action(cx, WindowAction::ViewActions(actions));
            }
        }
        
        if let Event::ClearAtlasses = event {
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
    
    pub fn begin(&mut self, cx: &mut Cx2d) -> Redrawing {

        if !cx.will_redraw(&mut self.main_draw_list, Walk::default()) {
            return Redrawing::no()
        }
        
        cx.begin_pass(&self.pass, None);

        self.main_draw_list.begin_always(cx);
        
        cx.begin_pass_sized_turtle(Layout::flow_down());
        
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
        
        cx.end_pass_sized_turtle();
        
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass);
    }
}

impl Widget for Window {
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            if let WindowAction::ViewActions(actions) = action {
                for action in actions {
                    dispatch_action(cx, action)
                }
            }
            else {
                dispatch_action(cx, WidgetActionItem::new(action.into(), uid));
            }
        });
    }
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {Walk::default()}
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw(cx)
    }
    
    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.view.find_widgets(path, cached, results);
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, DrawState::Drawing) {
            if self.begin(cx).is_not_redrawing() {
                self.draw_state.end();
                return WidgetDraw::done();
            }
        }
        
        if let Some(DrawState::Drawing) = self.draw_state.get() {
            self.view.draw_walk_widget(cx, walk)?;
            self.draw_state.end();
            self.end(cx);
        }
        
        WidgetDraw::done()
    }
}