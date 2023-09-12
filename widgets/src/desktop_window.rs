use crate::{
    makepad_derive_widget::*,
    debug_view::DebugView,
    makepad_draw::*,
    nav_control::NavControl,
    // window_menu::*,
    button::*,
    widget::*,
    view::*,
};

live_design!{
    DesktopWindowBase = {{DesktopWindow}} {}
}

#[derive(Live)]
pub struct DesktopWindow {
    #[rust] caption_size: DVec2,
    #[live] last_mouse_pos: DVec2,
    #[live] mouse_cursor_size: DVec2,
    
    #[live] cursor_draw_list: DrawList2d,
    #[live] draw_cursor: DrawQuad,
    
    #[live] debug_view: DebugView,
    #[live] nav_control: NavControl,
    #[live] window: Window,
    #[live] overlay: Overlay,
    #[live] main_draw_list: DrawList2d,
    #[live] pass: Pass,
    #[live] depth_texture: Texture,
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

impl LiveHook for DesktopWindow {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, DesktopWindow)
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.window.set_pass(cx, &self.pass);
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
        // check if we are ar/vr capable
        if cx.xr_capabilities().vr_supported {
            // lets show a VR button
            self.view.view(id!(web_xr)).set_visible(true);
            log!("VR IS SUPPORTED");
        }
        match cx.os_type() {
            OsType::Windows => {
                self.view.view(id!(caption_bar)).set_visible(true);
                self.view.view(id!(windows_buttons)).set_visible(true);
            }
            OsType::Macos => {
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

#[derive(Clone, WidgetAction)]
pub enum DesktopWindowAction {
    EventForOtherWindow,
    WindowClosed,
    WindowGeomChange(WindowGeomChangeEvent),
    ViewActions(Vec<WidgetActionItem>),
    None
}

impl DesktopWindow {
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, DesktopWindowAction)) {
        
        self.debug_view.handle_event(cx, event);
        self.nav_control.handle_event(cx, event, self.main_draw_list.draw_list_id());
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
                    match cx.os_type() {
                        OsType::Macos => {
                            if self.hide_caption_on_fullscreen{
                                if ev.new_geom.is_fullscreen && !ev.old_geom.is_fullscreen {
                                    self.view.view(id!(caption_bar)).set_visible(false);
                                    self.view.redraw(cx);
                                }
                                else if !ev.new_geom.is_fullscreen && ev.old_geom.is_fullscreen {
                                    self.view.view(id!(caption_bar)).set_visible(true);
                                    self.view.redraw(cx);
                                };
                            }
                        }
                        _ => ()
                    }
                    
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
            let actions = self.view.handle_widget_event(cx, event);
            if actions.not_empty() {
                if self.view.button(id!(min)).clicked(&actions) {
                    self.window.minimize(cx);
                }
                if self.view.button(id!(max)).clicked(&actions) {
                    if self.window.is_fullscreen(cx) {
                        self.window.restore(cx);
                    }
                    else {
                        self.window.maximize(cx);
                    }
                }
                if self.view.button(id!(close)).clicked(&actions) {
                    self.window.close(cx);
                }
                if self.view.button(id!(xr_on)).clicked(&actions) {
                    cx.xr_start_presenting();
                }
                dispatch_action(cx, DesktopWindowAction::ViewActions(actions));
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
        cx.end_pass_sized_turtle();
        
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass);
    }
}

impl Widget for DesktopWindow {
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            if let DesktopWindowAction::ViewActions(actions) = action {
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
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, _walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, DrawState::Drawing) {
            if self.begin(cx).is_not_redrawing() {
                self.draw_state.end();
                return WidgetDraw::done();
            }
        }
        
        if let Some(DrawState::Drawing) = self.draw_state.get() {
            self.view.draw_widget(cx) ?;
            self.draw_state.end();
            self.end(cx);
        }
        
        WidgetDraw::done()
    }
}

