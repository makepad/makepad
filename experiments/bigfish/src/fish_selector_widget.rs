#[allow(dead_code)]
use crate::{makepad_draw::*, makepad_widgets::widget::*, makepad_widgets::*};

live_design! {
    FishSelectorWidget = {{FishSelectorWidget}} {
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


        draw_bg: {
            fn pixel(self) -> vec4 {
                
                return vec4(0.3,0.3,0.3,0.3);
            }
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, DefaultNone)]
pub enum FishSelectorWidgetAction {
    None,
    Clicked,
    Pressed,
    Released,
}

#[derive(Live, LiveHook, Widget)]
pub struct FishSelectorWidget {
    #[live]
    start_pos: DVec2,
    #[live]
    end_pos: DVec2,
    #[animator]
    animator: Animator,
    #[redraw]
    #[live]
    draw_line: DrawLine,
    #[live]
    draw_bg: DrawQuad,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live(true)]
    grab_key_focus: bool,

    #[live]
    pub color: Vec4,

    #[live(10.0)]
    pub line_width: f64,

}

impl Widget for FishSelectorWidget {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        //let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        return;
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let _ = self.draw_walk_fishselector(cx, walk);
        DrawStep::done()
    }  
}

impl FishSelectorWidget {
    pub fn draw_walk_fishselector(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_line.begin(cx, walk, self.layout);
        self.draw_line.end(cx);

        let delta = self.end_pos - self.start_pos;
        let rect  = Rect {pos:  self.start_pos, size: delta};
        self.draw_bg.draw_abs(cx, rect);
        self.draw_line.draw_line_abs(cx, self.start_pos, self.start_pos + dvec2(delta.x, 0.), self.color, self.line_width);
        self.draw_line.draw_line_abs(cx, self.start_pos, self.start_pos + dvec2(0., delta.y), self.color, self.line_width);
        self.draw_line.draw_line_abs(cx, self.end_pos, self.end_pos - dvec2(delta.x, 0.), self.color, self.line_width);
        self.draw_line.draw_line_abs(cx, self.end_pos, self.end_pos - dvec2(0., delta.y), self.color, self.line_width);

        //   self.draw_line.draw_abs(cx, cx.turtle().unscrolled_rect());
    }
}
