use render::*;
use crate::buttonlogic::*;
use crate::desktopbutton::*;
use crate::windowmenu::*;
use crate::widgettheme::*;

#[derive(Clone)]
pub struct DesktopWindow {
    pub window: Window,
    pub pass: Pass,
    pub color_texture: Texture,
    pub depth_texture: Texture,
    pub caption_view: View, // we have a root view otherwise is_overlay subviews can't attach topmost
    pub main_view: View, // we have a root view otherwise is_overlay subviews can't attach topmost
    pub inner_view: View,
    //pub caption_bg_color: ColorId,
    pub min_btn: DesktopButton,
    pub max_btn: DesktopButton,
    pub close_btn: DesktopButton,
    pub vr_btn: DesktopButton,
    pub caption_text: Text,
    pub caption_bg: Quad,
    pub caption_size: Vec2,
    pub caption: String,
    
    pub window_menu: WindowMenu,
    
    pub _last_menu: Option<Menu>,
    
    // testing
    pub inner_over_chrome: bool,
}

#[derive(Clone, PartialEq)]
pub enum DesktopWindowEvent {
    EventForOtherWindow,
    WindowClosed,
    WindowGeomChange(WindowGeomChangeEvent),
    None
}

impl DesktopWindow {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            window: Window::proto(cx),
            pass: Pass::default(),
            color_texture: Texture::default(),
            depth_texture: Texture::default(),
            main_view: View::proto(cx),
            caption_view: View::proto(cx),
            inner_view: View::proto(cx),
            
            min_btn: DesktopButton::proto(cx),
            max_btn: DesktopButton::proto(cx),
            close_btn: DesktopButton::proto(cx),
            vr_btn: DesktopButton::proto(cx),
            
            window_menu: WindowMenu::proto(cx),
            
            caption_text: Text::proto(cx),
            //caption_bg_color: Color_bg_selected_over::id(cx),
            caption_bg: Quad::proto(cx),
            caption_size: Vec2::zero(),
            caption: "Makepad".to_string(),
            inner_over_chrome: false,
            _last_menu: None
        }
    }
    
    pub fn text_style_window_caption() ->TextStyleId{uid!()}
    
    pub fn theme(cx:&mut Cx){
        Self::text_style_window_caption().set_base(cx, Theme::text_style_normal().base(cx));
    }
    
    pub fn handle_desktop_window(&mut self, cx: &mut Cx, event: &mut Event) -> DesktopWindowEvent {
        //self.main_view.handle_scroll_bars(cx, event);
        //self.inner_view.handle_scroll_bars(cx, event);
        if let ButtonEvent::Clicked = self.vr_btn.handle_button(cx, event) {
            if self.window.vr_is_presenting(cx) {
                self.window.vr_stop_presenting(cx);
            }
            else {
                self.window.vr_start_presenting(cx);
            }
        }
        if let ButtonEvent::Clicked = self.min_btn.handle_button(cx, event) {
            self.window.minimize_window(cx);
        }
        if let ButtonEvent::Clicked = self.max_btn.handle_button(cx, event) {
            if self.window.is_fullscreen(cx) {
                self.window.restore_window(cx);
            }
            else {
                self.window.maximize_window(cx);
            }
        }
        if let ButtonEvent::Clicked = self.close_btn.handle_button(cx, event) {
            self.window.close_window(cx);
        }
        if let Some(window_id) = self.window.window_id {
            let is_for_other_window = match event {
                Event::WindowCloseRequested(ev) => ev.window_id != window_id,
                Event::WindowClosed(ev) => {
                    if ev.window_id == window_id {
                        return DesktopWindowEvent::WindowClosed
                    }
                    true
                }
                Event::WindowGeomChange(ev) => {
                    if ev.window_id == window_id {
                        return DesktopWindowEvent::WindowGeomChange(ev.clone())
                    }
                    true
                },
                Event::WindowDragQuery(dq) => {
                    if dq.window_id == window_id {
                        if dq.abs.x < self.caption_size.x && dq.abs.y < self.caption_size.y {
                            if dq.abs.x < 50. {
                                dq.response = WindowDragQueryResponse::SysMenu;
                            }
                            else {
                                dq.response = WindowDragQueryResponse::Caption;
                            }
                        }
                    }
                    true
                }
                Event::FingerDown(ev) => ev.window_id != window_id,
                Event::FingerMove(ev) => ev.window_id != window_id,
                Event::FingerHover(ev) => ev.window_id != window_id,
                Event::FingerUp(ev) => ev.window_id != window_id,
                Event::FingerScroll(ev) => ev.window_id != window_id,
                _ => false
            };
            if is_for_other_window {
                DesktopWindowEvent::EventForOtherWindow
            }
            else {
                DesktopWindowEvent::None
            }
        }
        else {
            DesktopWindowEvent::None
        }
    }
    
    pub fn begin_desktop_window(&mut self, cx: &mut Cx, menu: Option<&Menu>) -> ViewRedraw {
        
        if !self.main_view.view_will_redraw(cx) {
            return Err(())
        }
        
        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, &mut self.color_texture, ClearColor::ClearWith(color256(30, 30, 30)));
        self.pass.set_depth_texture(cx, &mut self.depth_texture, ClearDepth::ClearWith(1.0));
        
        let _ = self.main_view.begin_view(cx, Layout::default());
        
        if self.caption_view.begin_view(cx, Layout {
            walk:Walk::wh(Width::Fill, Height::Compute),
            ..Layout::default()
        }).is_ok() {
            self.caption_text.text_style = Self::text_style_window_caption().base(cx);
            self.caption_bg.color = Theme::color_bg_selected_over().base(cx);//cx.colors[self.caption_bg_color];
            // alright here we draw our platform buttons.
            match cx.platform_type {
                PlatformType::Linux | PlatformType::Windows => {
                    
                    let bg_inst = self.caption_bg.begin_quad(cx, Layout {
                        align: Align::right_center(),
                        walk: Walk::wh(Width::Fill, Height::Compute),
                        ..Default::default()
                    });
                    
                    // we need to draw the window menu here.
                    if let Some(_menu) = menu {
                        // lets draw the thing, check with the clone if it changed
                        // then draw it
                    }
                    
                    self.min_btn.draw_desktop_button(cx, DesktopButtonType::WindowsMin);
                    if self.window.is_fullscreen(cx) {
                        self.max_btn.draw_desktop_button(cx, DesktopButtonType::WindowsMaxToggled);
                    }
                    else {
                        self.max_btn.draw_desktop_button(cx, DesktopButtonType::WindowsMax);
                    }
                    self.close_btn.draw_desktop_button(cx, DesktopButtonType::WindowsClose);
                    
                    // change alignment
                    cx.change_turtle_align_x(0.5); //Align::center());
                    cx.compute_turtle_height();
                    cx.reset_turtle_pos();
                    cx.move_turtle(50., 0.);
                    // we need to store our caption rect somewhere.
                    self.caption_size = Vec2 {x: cx.get_width_left(), y: cx.get_height_left()};
                    self.caption_text.draw_text(cx, &self.caption);
                    self.caption_bg.end_quad(cx, &bg_inst);
                    cx.turtle_new_line();
                },
                
                PlatformType::OSX => { // mac still uses the built in buttons, TODO, replace that.
                    if let Some(menu) = menu {
                        cx.update_menu(menu);
                    }
                    let bg_inst = self.caption_bg.begin_quad(cx, Layout {
                        align: Align::center(),
                        walk: Walk::wh(Width::Fill, Height::Fix(22.)),
                        ..Default::default()
                    });
                    self.caption_size = Vec2 {x: cx.get_width_left(), y: cx.get_height_left()};
                    self.caption_text.draw_text(cx, &self.caption);
                    self.caption_bg.end_quad(cx, &bg_inst);
                    cx.turtle_new_line();
                },
                _ => {
                    
                }
            }
            self.caption_view.end_view(cx);
        }
        cx.turtle_new_line();
        
        if self.inner_over_chrome {
            let _ = self.inner_view.begin_view(cx, Layout {abs_origin: Some(Vec2::zero()), ..Layout::default()});
        }
        else {
            let _ = self.inner_view.begin_view(cx, Layout::default());
        }
        Ok(())
    }
    
    pub fn end_desktop_window(&mut self, cx: &mut Cx) {
        self.inner_view.end_view(cx);
        // lets draw a VR button top right over the UI.
        if cx.vr_can_present { // show a switch-to-VRMode button
            cx.reset_turtle_pos();
            cx.move_turtle(cx.get_width_total() - 50.0, 0.);
            self.vr_btn.draw_desktop_button(cx, DesktopButtonType::VRMode);
        }
        self.main_view.end_view(cx);
        
        self.pass.end_pass(cx);
        
        self.window.end_window(cx);
    }
}

