use makepad_render::*;

live_register!{
    use makepad_render::shader_std::*;
    
    TabButton: {{TabButton}}{
        tab_close_button:{
            fn pixel(self) -> vec4 {
                let cx = Sdf2d::viewport(self.pos * self.rect_size);
                let mid = self.rect_size / 2.0;
                let size = 0.5 * length(self.rect_size) / 2.0;
                let min = mid - vec2(size);
                let max = mid + vec2(size);
                cx.move_to(min.x, min.y);
                cx.line_to(max.x, max.y);
                cx.move_to(min.x, max.y);
                cx.line_to(max.x, min.y);
                return cx.stroke(vec4(1.0), 1.0);
            }
        }
        walk:{
            height: Height::Fixed(10.0),
            width: Width::Fixed(10.0),
            margin: Margin {
                l: 10.0,
                t: 1.0,
                r: 0.0,
                b: 0.0,
            },
        },
    }
}

#[derive(Live, LiveHook)]
pub struct TabButton {
    tab_close_button: DrawColor,
    walk: Walk
}

impl TabButton {

    pub fn draw(&mut self, cx: &mut Cx) {
        self.tab_close_button.draw_walk(
            cx,
            self.walk
        );
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, Action),
    ) {
        match event.hits(cx, self.tab_close_button.draw_vars.area, HitOpt::default()) {
            Event::FingerDown(_) => dispatch_action(cx, Action::WasPressed),
            _ => {}
        }
    }
}

pub enum Action {
    WasPressed,
}
