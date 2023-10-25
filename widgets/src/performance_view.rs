use std::collections::VecDeque;

use crate::{label::*, makepad_draw::*, view::*, widget::*};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::label::LabelBase;
    import makepad_widgets::view::ViewBase;

    REGULAR_TEXT = {
        font_size: 10,
        font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
    }

    PerformanceView = {{PerformanceView}} {
        width: 300,
        height: 65,

        abs_pos: vec2(0, 20),

        show_bg: true,
        draw_bg: {
            color: vec4(0.8, 0.8, 0.8, 0.8)
        }

        <ViewBase> {
            margin: 5,
            visible: true
            flow: Right
            width: Fill,
            height: Fill,


            <LabelBase> {
                width: Fit
                align: {x: 0., y: 1.}
                draw_text: {
                    text_style: <REGULAR_TEXT>{font_size: 8},
                    color: #111
                }
                text: "Frame time (max in last second)"
            }
            <View> { width: Fill, height: Fit }
            frame_time = <LabelBase> {
                width: Fit
                align: {x: 1., y: 1}
                draw_text: {
                    text_style: <REGULAR_TEXT>{font_size: 8},
                    color: #111
                }
                text: "-"
            }
        }
    }
}

const BAR_WIDTH: f64 = 3.;

#[derive(Live)]
pub struct PerformanceView {
    #[deref]
    view: View,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    next_refresh_at: f64,
    #[live]
    draw_graph: DrawColor,
    #[live]
    draw_bar: DrawColor,
    #[rust]
    frame_time_data: VecDeque<i64>,
}

impl LiveHook for PerformanceView {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, PerformanceView);
    }

    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.next_frame = cx.new_next_frame();
        self.next_refresh_at = 2.0;
    }
}

impl Widget for PerformanceView {
    fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
        self.draw_graph.redraw(cx);
        self.draw_bar.redraw(cx);
    }

    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.view.walk(cx)
    }

    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.view.find_widgets(path, cached, results);
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.view.draw_walk_widget(cx, walk);
        let _ = self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

impl PerformanceView {
    pub fn handle_widget(&mut self, cx: &mut Cx, event: &Event) {
        if let Some(ne) = self.next_frame.is_event(event) {
            if self.next_refresh_at < ne.time {
                self.next_refresh_at = ne.time + 0.5;

                if cx.performance_stats.max_frame_times.len() > 0 {
                    // Data is stored in 100ms buckets, so we take the max of the last 10 buckets
                    let last_sec_data = cx.performance_stats.max_frame_times.iter().take(10);
                    let time = (last_sec_data
                        .max_by(|a, b| a.time_spent.partial_cmp(&b.time_spent).unwrap())
                        .unwrap()
                        .time_spent
                        * 1000.0) as i64;

                    // Update the frame time data array
                    let frames_shown_count: usize =
                        (self.walk(cx).width.fixed_or_zero() / BAR_WIDTH) as usize;
                    self.frame_time_data.push_back(time);
                    while self.frame_time_data.len() >= frames_shown_count {
                        self.frame_time_data.pop_front();
                    }

                    self.label(id!(frame_time))
                        .set_text(&format!("{} ms", time));

                    self.redraw(cx);
                }
            }

            self.next_frame = cx.new_next_frame();
        }
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        // Draw graph
        let color_increment_lines = vec4(0.5, 0.5, 0.5, 1.);
        let color_ok = vec4(0.4, 1.0, 0.4, 1.0);
        let color_warning = vec4(0.9, 0.9, 0.4, 1.0);
        let color_fatal = vec4(1.0, 0.2, 0.2, 1.0);

        let graph_width = self.walk(cx).width.fixed_or_zero();
        let graph_height = self.walk(cx).height.fixed_or_zero();
        const MS_INCREMENTS: i64 = 16;

        self.draw_graph.begin(cx, walk, Layout::default());

        // draw increment lines
        for i in 0..(graph_height as i64 / MS_INCREMENTS) {
            self.draw_bar.color = color_increment_lines;
            let rect = Rect {
                pos: DVec2 {
                    x: 0.,
                    // + 1 to compensate the bar height
                    y: -(i * MS_INCREMENTS - graph_height as i64) as f64,
                },
                size: DVec2 {
                    x: graph_width,
                    y: 1.,
                },
            };
            self.draw_bar.draw_abs(cx, rect);
        }

        // graphing frame time data
        let frame_time_data_clone = &self.frame_time_data.clone();
        for (i, &time) in frame_time_data_clone.iter().enumerate() {
            self.draw_bar.color = match time {
                t if t < MS_INCREMENTS => color_ok,
                t if t >= MS_INCREMENTS && t < MS_INCREMENTS * 2 => color_warning,
                _ => color_fatal,
            };

            let rect = Rect {
                pos: DVec2 {
                    x: i as f64 * BAR_WIDTH,
                    y: graph_height,
                },
                size: DVec2 {
                    x: BAR_WIDTH,
                    // Negative so it draws to the positive side of the y axis.
                    y: -(time as f64) - 1.,
                },
            };
            self.draw_bar.draw_abs(cx, rect);
        }

        self.draw_graph.end(cx);
    }
}
