use crate::{
    widget::*,
    makepad_draw::*,
    view::*,
    label::*
};

live_design!{
    import makepad_draw::shader::std::*;  
    import makepad_widgets::label::LabelBase;
    import makepad_widgets::view::ViewBase;

    REGULAR_TEXT = {
        font_size: 10,
        font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
    }

    PerformanceView = {{PerformanceView}} {  
        width: 160,
        height: 90,

        abs_pos: vec2(20, 40),

        show_bg: true,
        draw_bg: {
            color: #f00
        }

        <ViewBase> {
            visible: true
            flow: Down
            width: Fill,
            height: Fill,

            margin: 10,

            <LabelBase> {
                draw_text: {
                    text_style: <REGULAR_TEXT>{},
                    color: #111
                }
                text: "Frame time"
            }
            <LabelBase> {
                draw_text: {
                    text_style: <REGULAR_TEXT>{},
                    color: #111
                }
                text: "(max in last second)"
            }
            frame_time = <LabelBase> {
                align: {x: 1}
                draw_text: {
                    text_style: <REGULAR_TEXT>{font_size: 16},
                    color: #111
                }
                text: "-"
            }
        }
    }
}

#[derive(Live)]
pub struct PerformanceView {
    #[deref] view: View,
    #[rust] next_frame: NextFrame,
    #[rust] next_refresh_in: f64,
}

impl LiveHook for PerformanceView {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, PerformanceView);
    }

    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.next_frame = cx.new_next_frame();
        self.next_refresh_in = 2.0;
    }
}

impl Widget for PerformanceView {
    fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
    }

    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.view.walk(cx)
    }

    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.view.find_widgets(path, cached, results);
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.view.draw_walk_widget(cx, walk)
    }
}

impl PerformanceView {
    pub fn handle_widget(&mut self, cx: &mut Cx, event: &Event) {
        if let Some(ne) = self.next_frame.is_event(event) {
            if self.next_refresh_in < ne.time {
                let prev_refresh_in = self.next_refresh_in;
                self.next_refresh_in = ne.time + 0.5;
                
                if cx.performance_stats.frame_times.len() > 0 {
                    let last_sec_data = cx.performance_stats.frame_times.iter().filter_map(|f|
                        if f.occurred_at >= prev_refresh_in - 0.5 {
                            Some((f.time_spent * 1000.) as u32)
                        } else {
                            None
                        }
                    ).collect::<Vec<u32>>();

                    let fps = last_sec_data.iter().max().unwrap();
                    self.label(id!(frame_time)).set_text(&format!("{} ms", fps));
                    
                    let color = match fps {
                        0..=16 => vec4(0.4, 1.0, 0.4, 1.0),
                        17..=33 => vec4(0.9, 0.9, 0.4, 1.0),
                        _ => vec4(1.0, 0.2, 0.2, 1.0)
                    };
                    self.view.apply_over(cx, live!{
                        draw_bg: {
                            color: (color)
                        }
                    });
                    self.redraw(cx);
                }
            }

            self.next_frame = cx.new_next_frame();
        }
    }
}