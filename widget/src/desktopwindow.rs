use makepad_render::*;
use crate::desktopbutton::*;
use crate::windowmenu::*;
use crate::buttonlogic::*;

live_register!{
    use makepad_render::drawcolor::DrawColor;
    use makepad_render::drawtext::DrawText;
    use makepad_widget::desktopbutton::DesktopButton;
    
    DesktopWindow: {
        rust_type: {{DesktopWindow}}
        clear_color: #f00
        inner_layout: Layout {}
        caption_text: DrawText {}
        min_btn: DesktopButton {}
        max_btn: DesktopButton {}
        close_btn: DesktopButton {}
        xr_btn: DesktopButton {}
        fullscreen_btn: DesktopButton {}
        caption_bg: DrawColor {color: #0f0}
        caption: "Hello World"
    }
}

#[derive(LiveComponent, LiveApply)]
pub struct DesktopWindow {

    #[rust] pub pass: Pass,
    #[rust(Texture::new(cx))] pub color_texture: Texture,
    #[rust(Texture::new(cx))] pub depth_texture: Texture,
    #[rust] pub caption_size: Vec2,
    
    #[rust(Window::new(cx))] pub window: Window,
    #[rust(View::new(cx))] pub caption_view: View, // we have a root view otherwise is_overlay subviews can't attach topmost
    #[rust(View::new(cx))] pub main_view: View, // we have a root view otherwise is_overlay subviews can't attach topmost
    #[rust(View::new(cx))] pub inner_view: View,
    #[live] pub inner_layout: Layout,
    
    #[live] pub clear_color: Vec4,

    //pub caption_bg_color: ColorId,
    #[live] pub min_btn: DesktopButton,
    #[live] pub max_btn: DesktopButton,
    #[live] pub close_btn: DesktopButton,
    #[live] pub xr_btn: DesktopButton,
    #[live] pub fullscreen_btn: DesktopButton,
    
    #[live] pub caption_text: DrawText,
    #[live] pub caption_bg: DrawColor,
    #[live] pub caption: String,
    
    #[rust(WindowMenu::new(cx))] pub window_menu: WindowMenu,
    #[rust(Menu::main(vec![
        Menu::sub("App", vec![
            Menu::item("Quit App", Cx::command_quit()),
        ]),
    ]))]
    pub default_menu: Menu,
    
    #[rust] pub last_menu: Option<Menu>,
    
    // testing
    #[rust] pub inner_over_chrome: bool,
}

#[derive(Clone, PartialEq)]
pub enum DesktopWindowEvent {
    EventForOtherWindow,
    WindowClosed,
    WindowGeomChange(WindowGeomChangeEvent),
    None
}

impl DesktopWindow {
    
    pub fn handle_desktop_window(&mut self, cx: &mut Cx, event: &mut Event) -> DesktopWindowEvent {
        //self.main_view.handle_scroll_bars(cx, event);
        //self.inner_view.handle_scroll_bars(cx, event);
        
        if let ButtonAction::Clicked = self.xr_btn.handle_desktop_button(cx, event) {
            if self.window.xr_is_presenting(cx) {
                self.window.xr_stop_presenting(cx);
            }
            else {
                self.window.xr_start_presenting(cx);
            }
        }
        
        if let ButtonAction::Clicked = self.fullscreen_btn.handle_desktop_button(cx, event) {
            if self.window.is_fullscreen(cx) {
                self.window.normal_window(cx);
            }
            else {
                self.window.fullscreen_window(cx);
            }
        }
        if let ButtonAction::Clicked = self.min_btn.handle_desktop_button(cx, event) {
            self.window.minimize_window(cx);
        }
        if let ButtonAction::Clicked = self.max_btn.handle_desktop_button(cx, event) {
            if self.window.is_fullscreen(cx) {
                self.window.restore_window(cx);
            }
            else {
                self.window.maximize_window(cx);
            }
        }
        if let ButtonAction::Clicked = self.close_btn.handle_desktop_button(cx, event) {
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
    
    pub fn draw_desktop_window<F>(&mut self, cx: &mut Cx, menu: Option<&Menu>, f: F)
    where
    F: FnOnce(&mut Cx)
    {
        if self.begin_desktop_window(cx, menu).is_ok() {
            f(cx);
            self.end_desktop_window(cx);
        }
    }
    
    pub fn begin_desktop_window(&mut self, cx: &mut Cx, menu: Option<&Menu>) -> ViewRedraw {
        
        if !self.main_view.view_will_redraw(cx) {
            return Err(())
        }
        
        self.window.begin_window(cx);
        
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, self.color_texture, ClearColor::ClearWith(self.clear_color));
        self.pass.set_depth_texture(cx, self.depth_texture, ClearDepth::ClearWith(1.0));
        
        let _ = self.main_view.begin_view(cx, Layout::default());
        
        if self.caption_view.begin_view(cx, Layout {
            walk: Walk::wh(Width::Fill, Height::Compute),
            ..Layout::default()
        }).is_ok() {
            
            // alright here we draw our platform buttons.
            let process_chrome = match cx.platform_type {
                PlatformType::Linux {custom_window_chrome} => custom_window_chrome,
                _ => true
            };
            if process_chrome {
                match cx.platform_type {
                    PlatformType::Windows | PlatformType::Unknown | PlatformType::Linux {..} => {
                        
                        self.caption_bg.begin_quad(cx, Layout {
                            align: Align {fx: 1.0, fy: 0.0},
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
                        cx.change_turtle_align_x_cab(0.5); //Align::center());
                        cx.compute_turtle_height();
                        cx.change_turtle_align_y_cab(0.5); //Align::center());
                        cx.reset_turtle_pos();
                        cx.move_turtle(50., 0.);
                        // we need to store our caption rect somewhere.
                        self.caption_size = Vec2 {x: cx.get_width_left(), y: cx.get_height_left()};
                        self.caption_text.draw_text_walk(cx, &self.caption);
                        self.caption_bg.end_quad(cx);
                        cx.turtle_new_line();
                    },
                    
                    PlatformType::OSX => { // mac still uses the built in buttons, TODO, replace that.
                        if let Some(menu) = menu {
                            cx.update_menu(menu);
                        }
                        else {
                            cx.update_menu(&self.default_menu);
                        }
                        self.caption_bg.begin_quad(cx, Layout {
                            align: Align {fx: 0.5, fy: 0.5},
                            walk: Walk::wh(Width::Fill, Height::Fix(22.)),
                            ..Default::default()
                        });
                        self.caption_size = Vec2 {x: cx.get_width_left(), y: cx.get_height_left()};
                        self.caption_text.draw_text_walk(cx, &self.caption);
                        self.caption_bg.end_quad(cx);
                        cx.turtle_new_line();
                    },
                    PlatformType::Web {..} => {
                        if self.window.is_fullscreen(cx) { // put a bar at the top
                            self.caption_bg.begin_quad(cx, Layout {
                                align: Align {fx: 0.5, fy: 0.5},
                                walk: Walk::wh(Width::Fill, Height::Fix(22.)),
                                ..Default::default()
                            });
                            self.caption_bg.end_quad(cx);
                            cx.turtle_new_line();
                        }
                    }
                }
            }
            self.caption_view.end_view(cx);
        }
        cx.turtle_new_line();
        
        if self.inner_over_chrome {
            let _ = self.inner_view.begin_view(cx, Layout {abs_origin: Some(Vec2::default()), ..self.inner_layout});
        }
        else {
            let _ = self.inner_view.begin_view(cx, self.inner_layout);
        }
        Ok(())
    }
    
    pub fn end_desktop_window(&mut self, cx: &mut Cx) {
        self.inner_view.end_view(cx);
        // lets draw a VR button top right over the UI.
        // window fullscreen?
        
        // only support fullscreen on web atm
        if !cx.platform_type.is_desktop() && !self.window.is_fullscreen(cx) {
            cx.reset_turtle_pos();
            cx.move_turtle(cx.get_width_total() - 50.0, 0.);
            self.fullscreen_btn.draw_desktop_button(cx, DesktopButtonType::Fullscreen);
        }
        
        if self.window.xr_can_present(cx) { // show a switch-to-VRMode button
            cx.reset_turtle_pos();
            cx.move_turtle(cx.get_width_total() - 100.0, 0.);
            self.xr_btn.draw_desktop_button(cx, DesktopButtonType::XRMode);
        }
        
        self.main_view.end_view(cx);
        
        self.pass.end_pass(cx);
        
        self.window.end_window(cx);
    }
}

