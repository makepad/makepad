use crate::makepad_draw_2d::*;


live_register!{
    import makepad_draw_2d::shader::std::*;
    
    DrawFocusRect: {{DrawFocusRect}} {
        fn pixel(self) -> vec4 {
            return #000f
        }
    }
    
    NavControl: {{NavControl}} {
        label: {
            text_style: {
                font_size: 6
            },
            color: #a
        }
        view: {
            unclipped: true
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawFocusRect {
    draw_super: DrawQuad,
}


#[derive(Live, LiveHook)]
pub struct NavControl {
    view: View,
    focus: DrawFocusRect,
    label: DrawText,
    #[rust] _recent_focus: Area,
}

impl NavControl {
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, root: DrawListId) {
        match event {
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::Tab => {
                    if ke.modifiers.shift {
                        let mut prev_area = Area::Empty;
                        if let Some(prev_area) = cx.iterate_nav_stops(root, | cx, stop | {
                            if cx.has_key_focus(stop.area) {
                                return Some(prev_area);
                            }
                            prev_area = stop.area;
                            None
                        }) {
                            if !prev_area.is_empty() {
                                cx.set_key_focus(prev_area);
                            }
                        }
                    }
                    else {
                        // ok lets walk and dump the nav item tree
                        let mut next_stop = false;
                        if let Some(next_area) = cx.iterate_nav_stops(root, | cx, stop | {
                            if next_stop {
                                return Some(stop.area)
                            }
                            if cx.has_key_focus(stop.area) {
                                next_stop = true;
                            }
                            None
                        }) {
                            cx.set_key_focus(next_area);
                        }
                    }
                }
                _ => ()
            },
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if !self.view.begin(cx, Walk::default(), Layout::default()).is_redrawing() {
            return
        }
        
        self.view.end(cx);
        
    }
}


