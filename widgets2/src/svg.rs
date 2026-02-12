use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use crate::makepad_draw::DrawSvg;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.SvgBase = #(Svg::register_widget(vm))

    mod.widgets.Svg = set_type_default() do mod.widgets.SvgBase{
        width: Fit
        height: Fit
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Svg {
    #[uid] uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    pub draw_svg: DrawSvg,
    #[live(true)]
    pub animating: bool,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time: f64,
}

impl Widget for Svg {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.animating {
            if let Event::NextFrame(ne) = event {
                self.time = ne.time;
                self.draw_svg.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
            if let Event::Startup = event {
                self.next_frame = cx.new_next_frame();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_svg.draw_walk_time(cx, walk, self.time as f32);
        DrawStep::done()
    }
}
