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
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
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
        // Its own frame only. Taking any frame that passed hid the missing
        // start below: the drawing moved once something else animated.
        if let Some(ne) = self.next_frame.is_event(event) {
            if self.animating {
                self.time = ne.time;
                // The draw this redraw causes asks for the next frame, so the
                // loop runs exactly while the drawing is drawn.
                self.draw_svg.redraw(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // The loop starts when the drawing is drawn. Asked for only at
        // Startup, an Svg built after launch stood still until an input
        // event set some other frame going.
        if self.animating {
            self.next_frame = cx.new_next_frame();
        }
        self.draw_svg.draw_walk_time(cx, walk, self.time as f32);
        DrawStep::done()
    }
}
