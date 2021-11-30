use {
    crate::tab_button::{self, TabButton},
    makepad_render::*,
};

live_register!{
    use makepad_render::shader_std::*;
        
    DrawTab: {{DrawTab}}{
       //debug:true
        border_width: 1.0
        border_color: #28
     //   drag_color: #FFFFFF80
        fn pixel(self) -> vec4 {
            let cx = Sdf2d::viewport(self.pos * self.rect_size)
            cx.clear(self.color)
            cx.move_to(0.0, 0.0)
            cx.line_to(0.0, self.rect_size.y)
            cx.move_to(self.rect_size.x, 0.0)
            cx.line_to(self.rect_size.x, self.rect_size.y)
            return cx.stroke(self.border_color, self.border_width)
        }
    }
    
    Tab: {{Tab}} {
        height: 40.0
        color: #34
        color_selected: #28
        name_color: #82
        name_color_selected: #FF
        layout: Layout {
            align: Align { fx: 0.0, fy: 0.5 },
            walk: Walk {
                width: Width::Computed,
                height: Height::Fixed(40.0),
            }, 
            padding: Padding {
                l: 10.0,
                t: 0.0,
                r: 10.0,
                b: 0.0,
            },
        }
    }

}

#[derive(Live, LiveHook)]
pub struct Tab {
    #[rust] is_selected: bool,
    #[rust] is_dragged: bool,

    bg_quad: DrawTab,
    name_text: DrawText,
    drag_quad: DrawColor,

    close_button: TabButton,

    height: f32,
    layout: Layout,
    color: Vec4,
    color_selected: Vec4,
    name_color: Vec4,
    name_color_selected: Vec4,
}

impl Tab {

    pub fn is_selected(&self) -> bool {
        self.is_selected
    }

    pub fn set_is_selected(&mut self, is_selected: bool) {
        self.is_selected = is_selected;
    }

    pub fn draw(&mut self, cx: &mut Cx, name: &str) {
        self.bg_quad.color = self.color(self.is_selected);
        self.bg_quad.begin(cx, self.layout);
        self.name_text.color = self.name_color(self.is_selected);
        self.name_text.draw_walk(cx, name);
        cx.turtle_align_y();
        self.close_button.draw(cx);
        self.bg_quad.end(cx);
        if self.is_dragged {
            self.drag_quad.draw_abs(cx, self.bg_quad.draw_vars.area.get_rect(cx));
        }
    }


    fn color(&self, is_selected: bool) -> Vec4 {
        if is_selected {
            self.color_selected
        } else {
            self.color
        }
    }

    fn name_color(&self, is_selected: bool) -> Vec4 {
        if is_selected {
            self.name_color_selected
        } else {
            self.name_color
        }
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, TabAction),
    ) {
        self.close_button
            .handle_event(cx, event, &mut |cx, action| match action {
                tab_button::Action::WasPressed => dispatch_action(cx, TabAction::ButtonWasPressed),
            });
        match event.hits(cx, self.bg_quad.draw_vars.area, HitOpt::default()) {
            Event::FingerDown(_) => {
                dispatch_action(cx, TabAction::WasPressed);
            }
            _ => {}
        }
        match event.drag_hits(cx, self.bg_quad.draw_vars.area, HitOpt::default()) {
            Event::FingerDrag(drag_event) => match drag_event.state {
                DragState::In => {
                    self.is_dragged = true;
                    self.bg_quad.draw_vars.redraw_view(cx);
                    match event {
                        Event::FingerDrag(event) => {
                            event.action = DragAction::Copy;
                        }
                        _ => panic!(),
                    }
                }
                DragState::Out => {
                    self.is_dragged = false;
                    self.bg_quad.draw_vars.redraw_view(cx);
                }
                DragState::Over => match event {
                    Event::FingerDrag(event) => {
                        event.action = DragAction::Copy;
                    }
                    _ => panic!(),
                },
            },
            Event::FingerDrop(event) => {
                self.is_dragged = false;
                self.bg_quad.draw_vars.redraw_view(cx);
                dispatch_action(cx, TabAction::ReceivedDraggedItem(event.dragged_item))
            }
            _ => {}
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawTab {
    #[live] deref_target: DrawColor,
    #[live] border_width: f32,
    #[live] border_color: Vec4,
}

pub enum TabAction {
    WasPressed,
    ButtonWasPressed,
    ReceivedDraggedItem(DraggedItem),
}
