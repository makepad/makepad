use render::*;
use crate::scrollbar::*;

#[derive(Clone)]
pub struct DesktopWindow {
    pub window: Window,
    pub pass: Pass,
    pub color_texture: Texture,
    pub depth_texture: Texture,
    pub main_view: View<ScrollBar>, // we have a root view otherwise is_overlay subviews can't attach topmost
    pub inner_view: View<ScrollBar>,
    
    // testing
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
        self.main_view.handle_scroll_bars(cx, event);
        self.inner_view.handle_scroll_bars(cx, event);
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
        
        if !self.main_view.view_will_redraw(cx) || !self.inner_view.view_will_redraw(cx) || !self.sub_view.view_will_redraw(cx){
            return Err(())
        }
        
        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, &mut self.color_texture, Some(Color::zero()));
        
        // for z-buffering add a depth texture here
        
        if self.test_rtt {
            // ok so how does subpass know its size.
            self.sub_pass.begin_pass(cx);
            self.sub_pass.add_color_texture(cx, &mut self.blitbuffer, Some(Color::zero()));
        }
        
        let _ = self.main_view.begin_view(cx, Layout::default());
        let _ = self.inner_view.begin_view(cx, Layout::default());
        
        Ok(())
    }
    
    pub fn end_desktop_window(&mut self, cx: &mut Cx) {
        
        self.inner_view.end_view(cx);
        self.main_view.end_view(cx);
        if self.test_rtt {
            self.sub_pass.end_pass(cx);
            // alright so sub_pass rendered a texture, now we blit it inside the outer pass
            let _ = self.sub_view.begin_view(cx, Layout::default());
            self.blit.draw_blit_abs(cx, &self.blitbuffer, Rect {x: 0., y: 0., w: 512., h: 512.});
            self.sub_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        
        self.window.end_window(cx);
    }
}

