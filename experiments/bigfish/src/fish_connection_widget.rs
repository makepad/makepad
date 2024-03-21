#[allow(dead_code)]
use crate::{makepad_draw::*, makepad_widgets::widget::*, makepad_widgets::*};

live_design! {
    FishConnectionWidget = {{FishConnectionWidget}} {
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                    }
                }
            }
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                    }
                }
            }
        }



    }
}

#[allow(dead_code)]
#[derive(Clone, DefaultNone)]
pub enum FishConnectionWidgetAction {
    None,
    Clicked,
    Pressed,
    Released,
}

#[derive(Live, LiveHook, Widget)]
pub struct FishConnectionWidget {
    #[live]
    start_pos: DVec2,
    #[live]
    end_pos: DVec2,
    #[animator]
    animator: Animator,
    #[redraw]
    #[live]
    draw_line: DrawLine,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live(true)]
    grab_key_focus: bool,
    #[live]
    pub text: RcStringMut,

    #[live]
    pub color: Vec4,

    #[live(0)]
    pub from_h: i32,
    #[live(0)]
    pub to_h: i32,

    #[live(10.0)]
    pub line_width: f64,

    #[live(-1)]
    pub from_top: i32,
    #[live(-1)]
    pub from_bottom: i32,
    #[live(-1)]
    pub to_top: i32,
    #[live(-1)]
    pub to_bottom: i32,

    #[live(false)]
    pub selected: bool,

}

impl Widget for FishConnectionWidget {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        //let uid = self.widget_uid();
        self.animator_handle_event(cx, event);

        return;
/*
        match event.hits(cx, self.draw_line.area()) {
            Hit::FingerDown(_fe) => {
                if self.grab_key_focus {
                    cx.set_key_focus(self.draw_line.area());
                }
                cx.widget_action(uid, &scope.path, ButtonAction::Pressed);
                self.animator_play(cx, id!(hover.pressed));
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerUp(fe) => {
                if fe.is_over {
                    cx.widget_action(uid, &scope.path, ButtonAction::Clicked);
                    if fe.device.has_hovers() {
                        self.animator_play(cx, id!(hover.on));
                    } else {
                        self.animator_play(cx, id!(hover.off));
                    }
                } else {
                    cx.widget_action(uid, &scope.path, ButtonAction::Released);
                    self.animator_play(cx, id!(hover.off));
                }
            }
            _ => (),
        }*/
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let _ = self.draw_walk_fishconnection(cx, walk);
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, v: &str) {
        self.text.as_mut_empty().push_str(v);
    }
}

impl FishConnectionWidget {
    pub fn draw_walk_fishconnection(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_line.begin(cx, walk, self.layout);
        self.draw_line.end(cx);

        let overshoot = 40.;

        if self.end_pos.x < self.start_pos.x + overshoot*2. {
            let mut midpoint = (self.end_pos + self.start_pos) * 0.5;

            if self.from_bottom > -1 {
                if self.from_bottom > self.to_top {
                    midpoint.y = (self.from_top + self.to_bottom) as f64 / 2.0;
                } else {
                    midpoint.y = (self.to_top + self.from_bottom) as f64 / 2.0;
                }
            }


            let points = vec![
                    self.start_pos, self.start_pos + dvec2(overshoot,0.), 
                    dvec2(self.start_pos.x + overshoot, midpoint.y),
                    dvec2(self.end_pos.x - overshoot, midpoint.y),
                    self.end_pos + dvec2(-overshoot, 0.),
                    self.end_pos             
                ];


          /*  for i in 0..points.len()-1
            {
                self.draw_line.draw_line_abs(
                    cx,
                    points[i],
                    points[i+1],
                    self.color,
                    self.line_width,
                );

    
            }*/
            if self.selected
            {
                self.draw_line.draw_bezier_abs(cx, &points, vec4(1.,1.,0.0,1.), self.line_width*1.3);
            }

                self.draw_line.draw_bezier_abs(
                    cx,
                    &points,
                    self.color,
                    self.line_width,
                );
    
            


        } else {
            let midpoint = (self.end_pos + self.start_pos) * 0.5;
            let deltatomid = midpoint - self.start_pos;

            let points = vec![
                self.start_pos, 
                self.start_pos + dvec2(deltatomid.x, 0.),                 
                self.end_pos - dvec2(deltatomid.x, 0.),
                self.end_pos             
            ];
            if self.selected
            {
                self.draw_line.draw_bezier_abs(cx, &points, vec4(1.,1.,0.0,1.), self.line_width*1.3);
            }
            self.draw_line.draw_bezier_abs(cx, &points, self.color, self.line_width);
            
        }

        //   self.draw_line.draw_abs(cx, cx.turtle().unscrolled_rect());
    }
}
