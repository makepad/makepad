use makepad_render::*;
use crate::desktopbutton::*;
use crate::windowmenu::*;
use crate::buttonlogic::*;

live_register!{
    DesktopWindow: {{DesktopWindow}} {
        clear_color: #x1e
        caption_bg: {color: #3d}
        caption: "Desktop Window",
        main_view:{},
        inner_view:{},
        caption_view: {
            layout: {
                walk: {
                    width: Width::Filled,
                    height: Height::Computed
                },
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct DesktopWindow {
    #[rust] pub caption_size: Vec2,

    pass: Pass,
    color_texture: Texture,
    depth_texture: Texture,
    
    window: Window,
    main_view: View, // we have a root view otherwise is_overlay subviews can't attach topmost
    caption_view: View, // we have a root view otherwise is_overlay subviews can't attach topmost
    inner_view: View,
    
    clear_color: Vec4,
    
    min_btn: DesktopButton,
    max_btn: DesktopButton,
    close_btn: DesktopButton,
    xr_btn: DesktopButton,
    fullscreen_btn: DesktopButton,
    
    caption_text: DrawText,
    caption_bg: DrawColor,
    caption: String,
    
    #[rust(WindowMenu::new(cx))] pub window_menu: WindowMenu,
    #[rust(Menu::main(vec![ 
        Menu::sub("App", vec![
            //Menu::item("Quit App", Cx::command_quit()),
        ]),
    ]))]
    
    default_menu: Menu,
    
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
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> DesktopWindowEvent {
        //self.main_view.handle_scroll_bars(cx, event);
        //self.inner_view.handle_scroll_bars(cx, event);
        
        if let ButtonAction::WasClicked = self.xr_btn.handle_desktop_button(cx, event) {
            if self.window.xr_is_presenting(cx) {
                self.window.xr_stop_presenting(cx);
            }
            else {
                self.window.xr_start_presenting(cx);
            }
        }
        
        if let ButtonAction::WasClicked = self.fullscreen_btn.handle_desktop_button(cx, event) {
            if self.window.is_fullscreen(cx) {
                self.window.normal(cx);
            }
            else {
                self.window.fullscreen(cx);
            }
        }
        if let ButtonAction::WasClicked = self.min_btn.handle_desktop_button(cx, event) {
            self.window.minimize(cx);
        }
        if let ButtonAction::WasClicked = self.max_btn.handle_desktop_button(cx, event) {
            if self.window.is_fullscreen(cx) {
                self.window.restore(cx);
            }
            else {
                self.window.maximize(cx);
            }
        }
        if let ButtonAction::WasClicked = self.close_btn.handle_desktop_button(cx, event) {
            self.window.close(cx);
        }
        let is_for_other_window = match event {
            Event::WindowCloseRequested(ev) => ev.window_id != self.window.window_id,
            Event::WindowClosed(ev) => {
                if ev.window_id == self.window.window_id {
                    return DesktopWindowEvent::WindowClosed
                }
                true
            }
            Event::WindowGeomChange(ev) => {
                if ev.window_id == self.window.window_id {
                    return DesktopWindowEvent::WindowGeomChange(ev.clone())
                }
                true
            },
            Event::WindowDragQuery(dq) => {
                if dq.window_id == self.window.window_id {
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
            Event::FingerDown(ev) => ev.window_id != self.window.window_id,
            Event::FingerMove(ev) => ev.window_id != self.window.window_id,
            Event::FingerHover(ev) => ev.window_id != self.window.window_id,
            Event::FingerUp(ev) => ev.window_id != self.window.window_id,
            Event::FingerScroll(ev) => ev.window_id != self.window.window_id,
            _ => false
        };
        if is_for_other_window {
            DesktopWindowEvent::EventForOtherWindow
        }
        else {
            DesktopWindowEvent::None
        }
    }
    
    pub fn begin(&mut self, cx: &mut Cx, menu: Option<&Menu>) -> ViewRedraw {
        
        if !self.main_view.view_will_redraw(cx) {
            return Err(())
        }

        self.window.begin(cx);
        
        self.pass.begin(cx);
        self.pass.add_color_texture(cx, &self.color_texture, PassClearColor::ClearWith(self.clear_color));
        self.pass.set_depth_texture(cx, &self.depth_texture, PassClearDepth::ClearWith(1.0));
        
        self.main_view.begin(cx).unwrap();
        
        /*self.caption_view.set_layout(cx, Layout {
            walk: Walk::wh(Width::Filled, Height::Computed),
            ..Layout::default()
        });*/
        
        if self.caption_view.begin(cx).is_ok() {
            // alright here we draw our platform buttons.
            let process_chrome = match cx.platform_type {
                PlatformType::Linux {custom_window_chrome} => custom_window_chrome,
                _ => true
            }; 
            if process_chrome {
                match PlatformType::Windows { //cx.platform_type {
                    PlatformType::Windows | PlatformType::Unknown | PlatformType::Linux {..} => {
                        
                        self.caption_bg.begin(cx, Layout {
                            align: Align {fx: 1.0, fy: 0.0},
                            walk: Walk::wh(Width::Filled, Height::Computed),
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
                        self.caption_text.draw_walk(cx, &self.caption);
                        self.caption_bg.end(cx);
                        cx.turtle_new_line();
                    },
                    
                    PlatformType::OSX => { // mac still uses the built in buttons, TODO, replace that.
                        if let Some(menu) = menu {
                            cx.update_menu(menu);
                        }
                        else {
                            cx.update_menu(&self.default_menu);
                        }
                        self.caption_bg.begin(cx, Layout {
                            align: Align {fx: 0.5, fy: 0.5},
                            walk: Walk::wh(Width::Filled, Height::Fixed(26.)),
                            ..Default::default()
                        });
                        self.caption_size = Vec2 {x: cx.get_width_left(), y: cx.get_height_left()};
                        self.caption_text.draw_walk(cx, &self.caption);
                        self.caption_bg.end(cx);
                        cx.turtle_new_line();
                    },
                    PlatformType::Web {..} => {
                        if self.window.is_fullscreen(cx) { // put a bar at the top
                            self.caption_bg.begin(cx, Layout {
                                align: Align {fx: 0.5, fy: 0.5},
                                walk: Walk::wh(Width::Filled, Height::Fixed(22.)),
                                ..Default::default()
                            });
                            self.caption_bg.end(cx);
                            cx.turtle_new_line();
                        }
                    }
                }
            }
            self.caption_view.end(cx);
        }
        cx.turtle_new_line();
        
        self.inner_view.begin(cx).unwrap();
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx) {
        self.inner_view.end(cx);
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
        
        self.main_view.end(cx);
        
        self.pass.end(cx);
        
        self.window.end(cx);
    }
}

