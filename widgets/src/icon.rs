use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*
};

live_design! {
    link widgets
    use link::shaders::*;

    pub IconBase = {{Icon}} {}

    pub Icon = <IconBase> {
        width: Fit,
        height: Fit,
        
        icon_walk: {
            width: 17.5,
            height: Fit,
        }
        
        draw_bg: {
            uniform color_dither: 1.0
            uniform color: #0000,
            uniform color_2: vec4(-1.0, -1.0,-1.0,-1.0)
            uniform bg_gradient_horizontal: 0.0; 

            fn pixel(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let color_2 = self.color_2;

                let gradient_fill_dir = self.pos.y + dither;
                if (self.bg_gradient_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither;
                }

                if (self.color_2.x < -0.5) {
                    color_2 = self.color;
                }

                return mix(self.color, color_2, gradient_fill_dir)
            }
        }

        draw_icon: {
            uniform color_dither: 1.0
            uniform color: #f00
            uniform color_2: vec4(-1.0, -1.0,-1.0,-1.0)
            uniform bg_gradient_horizontal: 0.0; 

            fn get_color(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let color_2 = self.color_2;

                let gradient_fill_dir = self.pos.y;
                if (self.bg_gradient_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither;
                }

                if (self.color_2.x < -0.5) {
                    color_2 = self.color;
                }

                return mix(self.color, color_2, gradient_fill_dir)
            }
        }
    }

    pub IconGradientX = <Icon> {
        draw_icon: {
            uniform color: (#f00)
            uniform color_2: (#00f)
            uniform bg_gradient_horizontal: 1.0; 
        }
    }

    pub IconGradientY = <Icon> {
        draw_icon: {
            color: (#f00)
            color_2: (#00f)
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