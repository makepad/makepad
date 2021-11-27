use {
    crate::tab_button::{self, TabButton},
    makepad_render::*,
};

live_register!{
    use makepad_render::shader_std::*;
        
    DrawTab: {{DrawTab}}{
        fn pixel(self) -> vec4 {
            let cx = Sdf2d::viewport(self.pos * self.rect_size);
            cx.clear(self.color);
            cx.move_to(0.0, 0.0);
            cx.line_to(0.0, self.rect_size.y);
            cx.move_to(self.rect_size.x, 0.0);
            cx.line_to(self.rect_size.x, self.rect_size.y);
            return cx.stroke(border_color, border_width);
        }
    }
    
    Tab: {{Tab}} {
        height: 40.0
        color: #34
        color_selected: #28
        border_width: 1.0
        border_color: #28
        name_color: #82
        name_color_selected: #FF
        drag_color: #FFFFFF80
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

#[derive(LiveComponent, LiveApply, LiveCast)]
pub struct Tab {
    #[rust] is_selected: bool,
    #[rust] is_dragged: bool,
    #[live] tab: DrawTab,
    #[live] close_button: TabButton,
    #[live] height: f32,
    #[live] layout: Layout,
    #[live] color: Vec4,
    #[live] color_selected: Vec4,
    #[live] name: DrawText,
    #[live] name_color: Vec4,
    #[live] name_color_selected: Vec4,
    #[live] drag: DrawColor,
}

impl Tab {

    pub fn is_selected(&self) -> bool {
        self.is_selected
    }

    pub fn set_is_selected(&mut self, is_selected: bool) {
        self.is_selected = is_selected;
    }

    pub fn draw(&mut self, cx: &mut Cx, name: &str) {

        self.tab.color = self.color(self.is_selected);
        self.tab.begin_quad(cx, self.layout);
        self.name.color = self.name_color(self.is_selected);
        self.name.draw_text_walk(cx, name);
        cx.turtle_align_y();
        self.close_button.draw(cx);
        self.tab.end_quad(cx);
        if self.is_dragged {
            self.drag.draw_quad_abs(cx, self.tab.draw_vars.area.get_rect(cx));
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
        dispatch_action: &mut dyn FnMut(&mut Cx, Action),
    ) {
        self.close_button
            .handle_event(cx, event, &mut |cx, action| match action {
                tab_button::Action::WasPressed => dispatch_action(cx, Action::ButtonWasPressed),
            });
        match event.hits(cx, self.tab.draw_vars.area, HitOpt::default()) {
            Event::FingerDown(_) => {
                dispatch_action(cx, Action::WasPressed);
            }
            _ => {}
        }
        match event.drag_hits(cx, self.tab.draw_vars.area, HitOpt::default()) {
            Event::FingerDrag(drag_event) => match drag_event.state {
                DragState::In => {
                    self.is_dragged = true;
                    self.tab.draw_vars.redraw_view(cx);
                    match event {
                        Event::FingerDrag(event) => {
                            event.action = DragAction::Copy;
                        }
                        _ => panic!(),
                    }
                }
                DragState::Out => {
                    self.is_dragged = false;
                    self.tab.draw_vars.redraw_view(cx);
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
                self.tab.draw_vars.redraw_view(cx);
                dispatch_action(cx, Action::ReceivedDraggedItem(event.dragged_item))
            }
            _ => {}
        }
    }
}

#[derive(LiveComponent, LiveApply, LiveCast)]
#[repr(C)]
struct DrawTab {
    #[live] deref_target: DrawColor,
    #[live] border_width: f32,
    #[live] border_color: Vec4,
}

pub enum Action {
    WasPressed,
    ButtonWasPressed,
    ReceivedDraggedItem(DraggedItem),
}
