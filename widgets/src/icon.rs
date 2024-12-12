use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*
};

live_design! {
    link widgets
    pub IconBase = {{Icon}} {}
    pub Icon = <IconBase> {
        width: Fit,
        height: Fit,
        
        icon_walk: {
            margin: {left: 5.0},
            width: Fit,
            height: Fit,
        }
        
        draw_bg: {
            instance color: #0000,
            fn pixel(self) -> vec4 {
                return self.color;
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct Icon {
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_icon: DrawIcon,
    #[live]
    icon_walk: Walk,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
}

impl Widget for Icon {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_bg.end(cx);
        DrawStep::done()
    }
}