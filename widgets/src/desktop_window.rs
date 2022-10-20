use crate::{
    debug_view::DebugView,
    makepad_draw_2d::*,
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
    
    DesktopWindow= {{DesktopWindow}} {
        pass: {clear_color: (COLOR_CLEAR)}
        var caption =  "Makepad"
        frame: {
            layout: {
                flow: Down
            },
            windows_buttons = <Solid> {
                bg: {color: (COLOR_BG_APP)}
                walk:{height: 29},
                caption_label = <Frame> {
                    layout: {align: {x: 0.5, y: 0.5}},
                    <Label> {text: (caption), walk: {margin: {left: 100}}}
                }
                //min_btn:= DesktopButton {button_type: DesktopButtonType::WindowsMin}
                //max_btn:= DesktopButton {button_type: DesktopButtonType::WindowsMax}
                //close_btn:= DesktopButton {button_type: DesktopButtonType::WindowsClose}
                
            }
            inner_view = <Frame> {user_draw: true}
        }
        
        window: {
            inner_size: vec2(1024, 768)
        }
    }
}

#[derive(Live)]
pub struct DesktopWindow {
    #[rust] pub caption_size: DVec2,
    
    debug_view: DebugView,
    nav_control: NavControl,
    window: Window,
    overlay: Overlay,
    main_view: View,
    pass: Pass,
    depth_texture: Texture,
    
    frame: FrameRef,
    
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
        if cx.platform_type().is_desktop(){
            //self.frame.template(cx, id!(windows_buttons), id!(my_instrument), live!{});
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
    
    pub fn handle_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, DesktopWindowEvent)){
        
        self.debug_view.handle_event(cx,event);
        self.nav_control.handle_event(cx, event, self.main_view.draw_list_id());
        self.overlay.handle_event(cx, event);
        let actions = self.frame.handle_event(cx, event);
        if actions.not_empty(){
            if self.frame.get_button(id!(min_btn)).clicked(&actions){
            
            }
            if self.frame.get_button(id!(max_btn)).clicked(&actions){
            
            }
            if self.frame.get_button(id!(close_btn)).clicked(&actions){
            
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
            Event::FingerDown(ev) => ev.window_id != self.window.window_id(),
            Event::FingerMove(ev) => ev.window_id != self.window.window_id(),
            Event::FingerHover(ev) => ev.window_id != self.window.window_id(),
            Event::FingerUp(ev) => ev.window_id != self.window.window_id(),
            Event::FingerScroll(ev) => ev.window_id != self.window.window_id(),
            _ => false
        };
        if is_for_other_window {
            return dispatch_action(cx, DesktopWindowEvent::EventForOtherWindow)
        }
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d, _menu: Option<&Menu>) -> ViewRedrawing {
        if !cx.view_will_redraw(&self.main_view) {
            return ViewRedrawing::no()
        }
        
        cx.begin_pass(&self.pass);
        
        self.main_view.begin_always(cx);

        let pass_size = cx.current_pass_size();

        cx.begin_turtle(Walk::fixed_size(pass_size), Layout::flow_down());
        
        self.overlay.begin(cx);

        //while self.frame.draw(cx).is_ok(){}
        if self.frame.draw(cx).is_done() {
            self.end(cx);
            return ViewRedrawing::no()
        }
        ViewRedrawing::yes() 
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        while self.frame.draw(cx).is_not_done() {}
        self.debug_view.draw(cx);
        cx.end_turtle();
        self.main_view.end(cx);
        cx.end_pass(&self.pass);
    }
}

