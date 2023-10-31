use std::collections::VecDeque;

use makepad_derive_widget::WidgetRef;

use crate::{label::*, makepad_draw::*, view::*, widget::*};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::label::LabelBase;
    import makepad_widgets::view::ViewBase;

    REGULAR_TEXT = {
        font_size: 10,
        font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
    }

    PerformanceLiveGraph = {{PerformanceLiveGraph}} {
        data_increments: 16.
        bar_width: 3.
        graph_label: ""
        data_y_subfix: ""

        width: 300,
        height: 65,

        abs_pos: vec2(0, 0)

        show_bg: true,
        draw_bg: {
            color: vec4(0.8, 0.8, 0.8, 0.8)
        }

        view = <ViewBase> {
            margin: 5,
            visible: true
            flow: Right
            width: Fill,
            height: Fill,

            graph_label = <LabelBase> {
                width: Fit
                align: {x: 0., y: 1.}
                draw_text: {
                    text_style: <REGULAR_TEXT>{font_size: 8},
                    color: #111
                }
                text: ""
            }

            <ViewBase> { width: Fill, height: Fit }

            current_y_entry = <LabelBase> {
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

    PerformanceView = {{PerformanceView}} {
        width: Fit,
        height: Fit,

        abs_pos: vec2(0, 0)

        graph = <PerformanceLiveGraph> {
            abs_pos: vec2(0, 20)
            graph_label: "Frame time (max in last second)"
            data_y_subfix: "ms"
        }

    }
}

#[derive(Live)]
pub struct PerformanceView {
    #[deref]
    view: View,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    next_refresh_at: f64,
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
    }

    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.view.find_widgets(path, cached, results);
    }

    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.view.walk(cx)
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.view.draw_walk_widget(cx, walk);
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
                        * 1000.) as i64;

                    self.performance_live_graph(id!(graph))
                        .add_y_entry(cx, time);
                }
            }

            self.next_frame = cx.new_next_frame();
        }
    }
}

#[derive(Live)]
pub struct PerformanceLiveGraph {
    #[deref]
    view: View,
    #[live]
    draw_graph: DrawColor,
    #[live]
    draw_bar: DrawColor,
    #[rust]
    data: VecDeque<i64>,
    #[live]
    bar_width: f64,
    #[live]
    data_increments: i64,
    #[live]
    data_y_subfix: String,
    #[live]
    graph_label: String,
}

impl LiveHook for PerformanceLiveGraph {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, PerformanceLiveGraph);
    }

    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
        self.label(id!(graph_label))
            .set_text(&format!("{}", self.graph_label));
    }
}

impl Widget for PerformanceLiveGraph {
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

impl PerformanceLiveGraph {
    pub fn add_y_entry(&mut self, cx: &mut Cx, y_entry: i64) {
        let frames_shown_count: usize =
            (self.walk(cx).width.fixed_or_zero() / self.bar_width) as usize;
        self.data.push_back(y_entry);
        while self.data.len() >= frames_shown_count {
            self.data.pop_front();
        }

        self.label(id!(current_y_entry))
            .set_text(&format!("{}{}", y_entry, self.data_y_subfix));

        self.redraw(cx);
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        let color_increment_lines = vec4(0.5, 0.5, 0.5, 1.);
        let color_ok = vec4(0.4, 1.0, 0.4, 1.0);
        let color_warning = vec4(0.9, 0.9, 0.4, 1.0);
        let color_fatal = vec4(1.0, 0.2, 0.2, 1.0);

        let graph_width = self.walk(cx).width.fixed_or_zero();
        let graph_height = self.walk(cx).height.fixed_or_zero();
        let graph_zero_baseline = graph_height - 20.;

        self.label(id!(graph_label))
            .set_text(&format!("{}", self.graph_label));

        self.draw_graph.begin(cx, walk, Layout::default());

        // draw increment lines
        if self.data_increments > 0 {
            for i in 0..(graph_height as i64 / self.data_increments) {
                self.draw_bar.color = color_increment_lines;
                let rect = Rect {
                    pos: DVec2 {
                        x: 0.,
                        // Negative so it draws to the positive side of the y axis.
                        y: -(i * self.data_increments - graph_zero_baseline as i64) as f64,
                    },
                    size: DVec2 {
                        x: graph_width,
                        y: 1.,
                    },
                };
                self.draw_bar.draw_rel(cx, rect);
            }
        }

        // graphing data
        for (i, &y_entry) in self.data.iter().enumerate() {
            self.draw_bar.color = match y_entry {
                t if t < self.data_increments => color_ok,
                t if t >= self.data_increments && t < self.data_increments * 2 => color_warning,
                _ => color_fatal,
            };

            let rect = Rect {
                pos: DVec2 {
                    x: i as f64 * self.bar_width,
                    y: graph_zero_baseline,
                },
                size: DVec2 {
                    x: self.bar_width,
                    // Negative so it draws to the positive side of the y axis.
                    y: -(y_entry as f64) - 1.,
                },
            };

            self.draw_bar.draw_rel(cx, rect);
        }

        self.draw_graph.end(cx);
    }
}

#[derive(Debug, Clone, PartialEq, WidgetRef)]
pub struct PerformanceLiveGraphRef(WidgetRef);

impl PerformanceLiveGraphRef {
    pub fn add_y_entry(&mut self, cx: &mut Cx, y_entry: i64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.add_y_entry(cx, y_entry);
        }
    }
}
