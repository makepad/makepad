use crate::{
    makepad_platform::*,
    button_logic::*,
    window_menu::*,
    frame::*,
    frame_traits::*
};

live_register!{
    import crate::theme::*;
    registry FrameComponent::*;
    import makepad_component::frame::*;
    
    DesktopWindow: {{DesktopWindow}} {
        pass: {clear_color: (COLOR_CLEAR)}
        var caption: "Makepad"
        frame: {
            layout: {
                flow: Flow::Down
            },
            windows_buttons =? Solid {
                bg: {color: (COLOR_BG_APP)}
                height: 29
                caption_label = Frame {
                    layout: {align: {x: 0.5, y: 0.5}},
                    Label {text: (caption), walk: {margin: {left: 100}}}
                }
                //min_btn:= DesktopButton {button_type: DesktopButtonType::WindowsMin}
                //max_btn:= DesktopButton {button_type: DesktopButtonType::WindowsMax}
                //close_btn:= DesktopButton {button_type: DesktopButtonType::WindowsClose}
                
            }
            inner_view = Frame {user_draw: true}
        }
        
        window: {
            inner_size: vec2(1024, 768)
        }
    }
}

#[derive(Live)]
pub struct DesktopWindow {
    #[rust] pub caption_size: Vec2,
    
    window: Window,
    main_view: View,
    pass: Pass,
    depth_texture: Texture,
    
    frame: Frame,
    
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
            self.frame.template(cx, ids!(windows_buttons), id!(my_instrument), live!{});
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
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> DesktopWindowEvent {
        
        for item in self.frame.handle_event_iter(cx, event) {
            if let ButtonAction::WasClicked = item.action.cast() {match item.id() {
                id!(min_btn) => {
                    self.window.minimize(cx);
                }
                id!(max_btn) => {
                    if self.window.is_fullscreen(cx) {
                        self.window.restore(cx);
                    }
                    else {
                        self.window.maximize(cx);
                    }
                }
                id!(close_btn) => {
                    self.window.close(cx);
                }
                _ => ()
            }}
        }
        
        let is_for_other_window = match event {
            Event::WindowCloseRequested(ev) => ev.window_id != self.window.window_id(),
            Event::WindowClosed(ev) => {
                if ev.window_id == self.window.window_id() {
                    return DesktopWindowEvent::WindowClosed
                }
                true
            }
            Event::WindowGeomChange(ev) => {
                if ev.window_id == self.window.window_id() {
                    return DesktopWindowEvent::WindowGeomChange(ev.clone())
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
            DesktopWindowEvent::EventForOtherWindow
        }
        else {
            DesktopWindowEvent::None
        }
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d, _menu: Option<&Menu>) -> ViewRedrawing {
        if !cx.view_will_redraw(&self.main_view) {
            return ViewRedrawing::no()
        }
        
        cx.begin_pass(&self.pass);
        
        self.main_view.begin(cx, Walk::default(), Layout::flow_right()).assume_redrawing();
        
        //while self.frame.draw(cx).is_ok(){}
        if self.frame.draw(cx).is_done() {
            self.main_view.end(cx);
            cx.end_pass(&self.pass);
            return ViewRedrawing::no()
        }
        
        ViewRedrawing::yes()
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        while self.frame.draw(cx).is_not_done() {}
        self.main_view.end(cx);
        cx.end_pass(&self.pass);
    }
}

