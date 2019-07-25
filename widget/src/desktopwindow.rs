use render::*;
use crate::scrollbar::*;
use crate::button::*;
use crate::desktopbutton::*;

#[derive(Clone)]
pub struct DesktopWindow {
    pub window: Window,
    pub pass: Pass,
    pub color_texture: Texture,
    pub depth_texture: Texture,
    pub main_view: View<ScrollBar>, // we have a root view otherwise is_overlay subviews can't attach topmost
    pub inner_view: View<ScrollBar>,
    
    pub min_btn: DesktopButton,
    pub max_btn: DesktopButton,
    pub close_btn: DesktopButton,
    pub caption_text: Text,
    pub caption_bg: Quad,
    pub caption_size: Vec2,
    pub caption: String,
    
    // testing
    pub inner_over_chrome: bool,
    pub test_rtt: bool,
    pub blit: Blit,
    pub sub_pass: Pass,
    pub sub_view: View<ScrollBar>, // we have a root view otherwise is_overlay subviews can't attach topmost
    pub blitbuffer: Texture,
}

#[derive(Clone, PartialEq)]
pub enum DesktopWindowEvent {
    EventForOtherWindow,
    WindowClosed,
    WindowGeomChange(WindowGeomChangeEvent),
    None
}

impl Style for DesktopWindow {
    fn style(cx: &mut Cx) -> Self {
        Self {
            window: Window::style(cx),
            pass: Pass::default(),
            color_texture: Texture::default(),
            depth_texture: Texture::default(),
            main_view: View::style(cx),
            inner_view: View::style(cx),
            
            min_btn: DesktopButton::style(cx),
            max_btn: DesktopButton::style(cx),
            close_btn: DesktopButton::style(cx),
            caption_text: Text::style(cx),
            caption_bg: Quad {
                color: cx.color("bg_selected"),
                ..Style::style(cx)
            },
            caption_size: Vec2::zero(),
            caption: "Makepad".to_string(),
            inner_over_chrome:false,
            test_rtt: false,
            sub_pass: Pass::default(),
            sub_view: View::style(cx),
            blit: Blit::style(cx),
            blitbuffer: Texture::default()
        }
    }
}

impl DesktopWindow {
    pub fn handle_desktop_window(&mut self, cx: &mut Cx, event: &mut Event) -> DesktopWindowEvent {
        //self.main_view.handle_scroll_bars(cx, event);
        //self.inner_view.handle_scroll_bars(cx, event);
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
    
    pub fn begin_desktop_window(&mut self, cx: &mut Cx) -> ViewRedraw {
        
        if !self.main_view.view_will_redraw(cx) || !self.inner_view.view_will_redraw(cx) || !self.sub_view.view_will_redraw(cx) {
            return Err(())
        }
        
        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, &mut self.color_texture, Some(color256(30,30,30)));
        
        // for z-buffering add a depth texture here
        
        if self.test_rtt {
            // ok so how does subpass know its size.
            self.sub_pass.begin_pass(cx);
            self.sub_pass.add_color_texture(cx, &mut self.blitbuffer, Some(Color::zero()));
        }
        
        let _ = self.main_view.begin_view(cx, Layout::default());
        
        // alright here we draw our platform buttons.
        match cx.platform_type {
            PlatformType::Windows => {
                let bg_inst = self.caption_bg.begin_quad(cx, &Layout {
                    align: Align::right_center(),
                    width: Bounds::Fill,
                    height: Bounds::Compute,
                    ..Default::default()
                });
                
                self.min_btn.draw_desktop_button(cx, DesktopButtonType::WindowsMin);
                if self.window.is_fullscreen(cx) {self.max_btn.draw_desktop_button(cx, DesktopButtonType::WindowsMaxToggled);}
                else {self.max_btn.draw_desktop_button(cx, DesktopButtonType::WindowsMax);}
                self.close_btn.draw_desktop_button(cx, DesktopButtonType::WindowsClose);
                
                // change alignment
                cx.realign_turtle(Align::center());
                cx.compute_turtle_height();
                cx.reset_turtle_walk();
                cx.move_turtle(50., 0.);
                // we need to store our caption rect somewhere.
                self.caption_size = Vec2 {x: cx.get_width_left(), y: cx.get_height_left()};
                self.caption_text.draw_text(cx, &self.caption);
                self.caption_bg.end_quad(cx, &bg_inst);
                cx.turtle_new_line();
            },
            PlatformType::OSX => { // mac still uses the built in buttons, TODO, replace that.
                let bg_inst = self.caption_bg.begin_quad(cx, &Layout {
                    align: Align::center(),
                    width: Bounds::Fill,
                    height: Bounds::Fix(22.),
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
        
        if self.inner_over_chrome{
            let _ = self.inner_view.begin_view(cx, Layout{abs_origin: Some(Vec2::zero()),..Layout::default()});
        }
        else{
            let _ = self.inner_view.begin_view(cx, Layout::default());
        }
        Ok(())
    }
    
    pub fn end_desktop_window(&mut self, cx: &mut Cx) {
        
        self.inner_view.end_view(cx);
        self.main_view.end_view(cx);
        if self.test_rtt {
            self.sub_pass.end_pass(cx);
            // alright so sub_pass rendered a texture, now we blit it inside the outer pass
            let _ = self.sub_view.begin_view(cx, Layout::default());
            self.blit.draw_blit_walk(cx, &self.blitbuffer, Bounds::Fill, Bounds::Fill, Margin::zero());
            self.sub_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        
        self.window.end_window(cx);
    }
}

