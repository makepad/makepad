use {
    crate::{
        app::{AppAction, AppData},
        build_manager::build_manager::ProfileSampleStore,
        makepad_widgets::*,
    },
    std::fmt::Write,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ProfilerEventChartBase = #(ProfilerEventChart::register_widget(vm))
    mod.widgets.ProfilerBase = #(Profiler::register_widget(vm))

    mod.widgets.ProfilerEventChart = set_type_default() do mod.widgets.ProfilerEventChartBase {
        height: Fit
        width: Fill
        draw_bg +: {
            pixel: fn() { return theme.color_bg_container }
        }
        draw_line +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0
                )
                sdf.fill_keep(theme.color_shadow)
                return sdf.result
            }
        }
        draw_item +: {
            pixel: fn() {
                return self.color
            }
        }
        draw_vector +: {
            draw_depth: 2.0
        }
        draw_time +: {
            text_style: theme.font_regular {
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_p
            }
            color: theme.color_label_outer
        }
        draw_label +: {
            text_style: theme.font_regular {
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_p
            }
            color: theme.color_label_outer_down
        }
    }

    mod.widgets.Profiler = set_type_default() do mod.widgets.ProfilerBase {
        height: Fill
        width: Fill
        flow: Down

        View {
            height: Fit
            width: Fill
            flow: Right
            padding: theme.mspace_2
            spacing: theme.space_2

            running_button := ToggleFlat {
                text: "Running"
                active: false
                icon_walk: Walk{ width: 8. }
            }
            clear_button := ButtonFlat {
                text: "Clear"
                icon_walk: Walk{ width: 12. }
                draw_icon +: {
                    svg: crate_resource("self:resources/icons/icon_profiler_clear.svg")
                }
            }
            Filler {}
            stats := View {
                width: Fit
                flow: Right
                spacing: theme.space_2
                status_label := P {
                    width: Fit
                    text: "Build: -"
                    margin: 0.
                    padding: theme.mspace_v_1
                }
                sample_count_label := P {
                    width: Fit
                    text: "App E: 0 G: 0 C: 0 | Self E: 0 G: 0 C: 0"
                    margin: 0.
                    padding: theme.mspace_v_1
                }
                window_label := Pbold {
                    width: Fit
                    text: "Paused"
                    margin: 0.
                    padding: theme.mspace_v_1
                }
            }
        }
        chart_scroll := ScrollYView {
            width: Fill
            height: Fill
            flow: Down
            chart := mod.widgets.ProfilerEventChart {}
        }
    }
}

const DEFAULT_PROFILE_WINDOW_SECONDS: f64 = 0.5;
const MIN_PROFILE_WINDOW_SECONDS: f64 = 0.000_01;
const PROFILE_ROW_Y_STEP: f64 = 25.0;
const PROFILE_GRAPH_START_Y: f64 = PROFILE_ROW_Y_STEP * 3.0 + 24.0;
const PROFILE_GRAPH_LANE_HEIGHT: f64 = 56.0;
const PROFILE_GRAPH_LANE_GAP: f64 = 10.0;
const PROFILE_FRAMETIME_GRAPH_OFFSET_Y: f64 = PROFILE_GRAPH_START_Y;
const PROFILE_COUNTS_GRAPH_OFFSET_Y: f64 =
    PROFILE_FRAMETIME_GRAPH_OFFSET_Y + PROFILE_GRAPH_LANE_HEIGHT + PROFILE_GRAPH_LANE_GAP;
const PROFILE_UPLOAD_GRAPH_OFFSET_Y: f64 =
    PROFILE_COUNTS_GRAPH_OFFSET_Y + PROFILE_GRAPH_LANE_HEIGHT + PROFILE_GRAPH_LANE_GAP;
const PROFILE_STORE_HEIGHT: f64 = PROFILE_UPLOAD_GRAPH_OFFSET_Y + PROFILE_GRAPH_LANE_HEIGHT + 12.0;
const SELF_PROFILE_ROW_OFFSET_Y: f64 = PROFILE_STORE_HEIGHT + 16.0;
const DRAW_EVENT_U32: u32 = 7;
const FRAME_BUDGET_SECONDS: f64 = 1.0 / 60.0;
const FRAME_BUDGET_120HZ_SECONDS: f64 = 1.0 / 120.0;
const HICCUP_GAP_SECONDS: f64 = FRAME_BUDGET_SECONDS * 1.5;

#[derive(Clone)]
struct TimeRange {
    start: f64,
    end: f64,
}

impl TimeRange {
    fn len(&self) -> f64 {
        self.end - self.start
    }
    fn shifted(&self, shift: f64) -> Self {
        Self {
            start: self.start + shift,
            end: self.end + shift,
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
struct ProfilerEventChart {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_line: DrawQuad,
    #[live]
    draw_item: DrawColor,
    #[live]
    draw_vector: DrawVector,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_time: DrawText,
    #[rust(TimeRange{start:0.0, end: 0.5})]
    time_range: TimeRange,
    #[rust]
    time_drag: Option<TimeRange>,
    #[rust(false)]
    follow_live: bool,
    #[rust]
    self_time_offset: Option<f64>,
    #[rust]
    tmp_label: String,
}

impl ProfilerEventChart {
    fn set_follow_live(&mut self, cx: &mut Cx, follow_live: bool) {
        if self.follow_live != follow_live {
            self.follow_live = follow_live;
            self.time_drag = None;
            if self.follow_live {
                self.self_time_offset = None;
            }
            self.draw_bg.redraw(cx);
        }
    }

    fn current_window_seconds(&self) -> f64 {
        self.time_range.len().max(DEFAULT_PROFILE_WINDOW_SECONDS)
    }

    fn sync_live_window(&mut self, latest_sample_end: f64) {
        let window = self.current_window_seconds().max(MIN_PROFILE_WINDOW_SECONDS);
        self.time_range = if latest_sample_end <= window {
            TimeRange {
                start: 0.0,
                end: window,
            }
        } else {
            TimeRange {
                start: latest_sample_end - window,
                end: latest_sample_end,
            }
        };
    }

    fn reset_for_new_session(&mut self, cx: &mut Cx) {
        self.time_drag = None;
        self.self_time_offset = None;
        self.time_range = TimeRange {
            start: 0.0,
            end: DEFAULT_PROFILE_WINDOW_SECONDS,
        };
        self.draw_bg.redraw(cx);
    }

    fn has_samples(samples: &ProfileSampleStore) -> bool {
        !(samples.event.is_empty() && samples.gpu.is_empty() && samples.gc.is_empty())
    }

    fn latest_sample_end(samples: &ProfileSampleStore) -> Option<f64> {
        [
            samples.event.last().map(|sample| sample.end),
            samples.gpu.last().map(|sample| sample.end),
            samples.gc.last().map(|sample| sample.end),
        ]
        .into_iter()
        .flatten()
        .max_by(|a, b| a.total_cmp(b))
    }

    fn format_time_to_now_label(label: &mut String, seconds_to_now: f64) {
        label.clear();
        if seconds_to_now.abs() < 0.000_5 {
            label.push_str("now");
            return;
        }
        let abs_seconds = seconds_to_now.abs();
        if abs_seconds < 1.0 {
            let _ = write!(label, "-{:.0}ms", abs_seconds * 1000.0);
        } else {
            let _ = write!(label, "-{:.2}s", abs_seconds);
        }
    }

    fn draw_time_grid(&mut self, cx: &mut Cx2d, rect: &Rect, label: &mut String) {
        let range_len = self.time_range.len().max(MIN_PROFILE_WINDOW_SECONDS);
        let scale = rect.size.x / range_len;
        let mut major_step = FRAME_BUDGET_SECONDS;
        while major_step * scale < 90.0 {
            major_step *= 2.0;
        }
        while major_step * scale > 180.0 && major_step > 0.001 {
            major_step *= 0.5;
        }
        let minor_step = major_step * 0.5;

        if minor_step * scale >= 28.0 {
            let mut t = (self.time_range.start / minor_step).floor() * minor_step;
            while t <= self.time_range.end {
                let major_index = (t / major_step).round();
                let major_t = major_index * major_step;
                let is_major = (t - major_t).abs() <= minor_step * 0.06;
                if !is_major {
                    let xpos = rect.pos.x + (t - self.time_range.start) * scale;
                    self.draw_line.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(xpos, rect.pos.y),
                            size: dvec2(1.0, rect.size.y),
                        },
                    );
                }
                t += minor_step;
            }
        }

        let mut t = (self.time_range.start / major_step).floor() * major_step;
        while t <= self.time_range.end {
            let xpos = rect.pos.x + (t - self.time_range.start) * scale;
            let pos = dvec2(xpos, rect.pos.y);
            self.draw_line.draw_abs(
                cx,
                Rect {
                    pos,
                    size: dvec2(2.0, rect.size.y),
                },
            );
            Self::format_time_to_now_label(label, self.time_range.end - t);
            self.draw_time.draw_abs(cx, pos + dvec2(2.0, 2.0), label);
            t += major_step;
        }
    }

    fn draw_frame_gap_markers(
        &mut self,
        cx: &mut Cx2d,
        rect: &Rect,
        samples: &ProfileSampleStore,
        time_offset: f64,
        label: &mut String,
    ) {
        let range_len = self.time_range.len().max(MIN_PROFILE_WINDOW_SECONDS);
        let scale = rect.size.x / range_len;
        let mut prev_draw_end = None;
        let mut last_label_x = f64::NEG_INFINITY;

        for sample in &samples.event {
            if sample.event_u32 != DRAW_EVENT_U32 {
                continue;
            }
            let draw_end = sample.end + time_offset;
            if draw_end < self.time_range.start {
                prev_draw_end = Some(draw_end);
                continue;
            }
            if draw_end > self.time_range.end {
                break;
            }
            if let Some(prev_end) = prev_draw_end {
                let gap = draw_end - prev_end;
                if gap >= HICCUP_GAP_SECONDS {
                    let xpos = rect.pos.x + (draw_end - self.time_range.start) * scale;
                    self.draw_item.color = Vec4f::from_u32(0xff5f5f50);
                    self.draw_item.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2((xpos - 1.0).max(rect.pos.x), rect.pos.y),
                            size: dvec2(2.0, rect.size.y),
                        },
                    );
                    if xpos - last_label_x > 84.0 {
                        label.clear();
                        let _ = write!(label, "{:.1}ms gap", gap * 1000.0);
                        self.draw_time.draw_abs(cx, dvec2(xpos + 3.0, rect.pos.y + 14.0), label);
                        last_label_x = xpos;
                    }
                }
            }
            prev_draw_end = Some(draw_end);
        }
    }

    fn draw_profile_store(
        &mut self,
        cx: &mut Cx2d,
        rect: &Rect,
        samples: &ProfileSampleStore,
        base_y: f64,
        label_prefix: &str,
        time_offset: f64,
    ) {
        if let Some(first) = samples
            .event
            .iter()
            .position(|sample| sample.end + time_offset > self.time_range.start)
        {
            let mut prefixed_label = String::new();
            for i in first..samples.event.len() {
                let sample = &samples.event[i];
                let sample_start = sample.start + time_offset;
                let sample_end = sample.end + time_offset;
                if sample_start > self.time_range.end {
                    break;
                }
                let color = LiveId(0).bytes_append(&sample.event_u32.to_be_bytes()).0 as u32
                    | 0xff000000;
                self.draw_item.color = Vec4f::from_u32(color);
                if label_prefix.is_empty() {
                    self.draw_block(
                        cx,
                        &Rect {
                            pos: rect.pos + dvec2(0.0, base_y),
                            size: rect.size,
                        },
                        sample_start,
                        sample_end,
                        Event::name_from_u32(sample.event_u32),
                        sample.event_meta,
                    );
                } else {
                    prefixed_label.clear();
                    prefixed_label.push_str(label_prefix);
                    prefixed_label.push_str(Event::name_from_u32(sample.event_u32));
                    self.draw_block(
                        cx,
                        &Rect {
                            pos: rect.pos + dvec2(0.0, base_y),
                            size: rect.size,
                        },
                        sample_start,
                        sample_end,
                        &prefixed_label,
                        sample.event_meta,
                    );
                }
            }
        }

        self.draw_item.color = Vec4f::from_u32(if label_prefix.is_empty() {
            0x7f7f7fff
        } else {
            0x9f5f5fff
        });
        if let Some(first) = samples
            .gpu
            .iter()
            .position(|sample| sample.end + time_offset > self.time_range.start)
        {
            let mut gpu_label = String::new();
            for i in first..samples.gpu.len() {
                let sample = &samples.gpu[i];
                let sample_start = sample.start + time_offset;
                let sample_end = sample.end + time_offset;
                if sample_start > self.time_range.end {
                    break;
                }
                gpu_label.clear();
                if label_prefix.is_empty() {
                    gpu_label.push_str("GPU");
                } else {
                    gpu_label.push_str("Self GPU");
                }
                let _ = write!(
                    &mut gpu_label,
                    " d:{} i:{} v*i:{} ib:{:.1}k ub:{:.1}k vb:{:.1}k tb:{:.1}k",
                    sample.draw_calls,
                    sample.instances,
                    sample.vertices,
                    sample.instance_bytes as f64 / 1024.0,
                    sample.uniform_bytes as f64 / 1024.0,
                    sample.vertex_buffer_bytes as f64 / 1024.0,
                    sample.texture_bytes as f64 / 1024.0,
                );
                self.draw_block(
                    cx,
                    &Rect {
                        pos: rect.pos + dvec2(0.0, base_y + PROFILE_ROW_Y_STEP),
                        size: rect.size,
                    },
                    sample_start,
                    sample_end,
                    &gpu_label,
                    0,
                );
            }
        }

        self.draw_item.color = Vec4f::from_u32(if label_prefix.is_empty() {
            0x5eb27fff
        } else {
            0x3f9c5fff
        });
        if let Some(first) = samples
            .gc
            .iter()
            .position(|sample| sample.end + time_offset > self.time_range.start)
        {
            for i in first..samples.gc.len() {
                let sample = &samples.gc[i];
                let sample_start = sample.start + time_offset;
                let sample_end = sample.end + time_offset;
                if sample_start > self.time_range.end {
                    break;
                }
                self.draw_block(
                    cx,
                    &Rect {
                        pos: rect.pos + dvec2(0.0, base_y + PROFILE_ROW_Y_STEP * 2.0),
                        size: rect.size,
                    },
                    sample_start,
                    sample_end,
                    if label_prefix.is_empty() {
                        "GC"
                    } else {
                        "Self GC"
                    },
                    sample.heap_live,
                );
            }
        }
    }

    fn draw_graph_lane_background(
        &mut self,
        cx: &mut Cx2d,
        rect: &Rect,
        base_y: f64,
        lane_offset_y: f64,
        label_prefix: &str,
    ) -> Option<Rect> {
        let graph_rect = Rect {
            pos: rect.pos + dvec2(0.0, base_y + lane_offset_y),
            size: dvec2(rect.size.x, PROFILE_GRAPH_LANE_HEIGHT),
        };
        if graph_rect.size.x <= 1.0
            || graph_rect.size.y <= 1.0
            || graph_rect.pos.y >= rect.pos.y + rect.size.y
        {
            return None;
        }

        self.draw_item.color = Vec4f::from_u32(if label_prefix.is_empty() {
            0x142a2a30
        } else {
            0x141a1a30
        });
        self.draw_item.draw_abs(cx, graph_rect);

        for i in 1..=3 {
            let ypos = graph_rect.pos.y + graph_rect.size.y * (i as f64 / 4.0);
            self.draw_line.draw_abs(
                cx,
                Rect {
                    pos: dvec2(graph_rect.pos.x, ypos),
                    size: dvec2(graph_rect.size.x, 1.0),
                },
            );
        }
        Some(graph_rect)
    }

    fn draw_gpu_frametime_graph(
        &mut self,
        cx: &mut Cx2d,
        rect: &Rect,
        samples: &ProfileSampleStore,
        base_y: f64,
        label_prefix: &str,
        time_offset: f64,
        label: &mut String,
    ) {
        let Some(graph_rect) = self.draw_graph_lane_background(
            cx,
            rect,
            base_y,
            PROFILE_FRAMETIME_GRAPH_OFFSET_Y,
            label_prefix,
        ) else {
            return;
        };

        let range_len = self.time_range.len().max(MIN_PROFILE_WINDOW_SECONDS);
        let x_scale = rect.size.x / range_len;
        let mut max_ms = 0.0_f64;
        let mut visible_count = 0usize;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            visible_count += 1;
            max_ms = max_ms.max((sample.end - sample.start).max(0.0) * 1000.0);
        }

        if visible_count == 0 {
            return;
        }

        let max_ms = max_ms.max(FRAME_BUDGET_120HZ_SECONDS * 1000.0);
        let metric_to_y = |value_ms: f64| -> f32 {
            let t = (value_ms / max_ms).clamp(0.0, 1.0);
            (graph_rect.pos.y + graph_rect.size.y - t * (graph_rect.size.y - 1.0)) as f32
        };

        label.clear();
        let _ = write!(
            label,
            "{} GPU frametime (max {:.2} ms, 120Hz {:.2} ms)",
            if label_prefix.is_empty() { "App" } else { "Self" },
            max_ms,
            FRAME_BUDGET_120HZ_SECONDS * 1000.0
        );
        self.draw_time
            .draw_abs(cx, graph_rect.pos + dvec2(4.0, 2.0), label);

        let budget_y = metric_to_y(FRAME_BUDGET_120HZ_SECONDS * 1000.0) as f64;
        self.draw_item.color = Vec4f::from_u32(0xffb05050);
        self.draw_item.draw_abs(
            cx,
            Rect {
                pos: dvec2(graph_rect.pos.x, budget_y),
                size: dvec2(graph_rect.size.x, 1.0),
            },
        );

        self.draw_vector.set_color(0.95, 0.64, 0.12, 1.0);
        let mut is_first = true;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            let xpos =
                rect.pos.x + (sample_end - self.time_range.start).clamp(0.0, range_len) * x_scale;
            let ypos = metric_to_y((sample.end - sample.start).max(0.0) * 1000.0);
            if is_first {
                self.draw_vector.move_to(xpos as f32, ypos);
                is_first = false;
            } else {
                self.draw_vector.line_to(xpos as f32, ypos);
            }
        }
        if !is_first {
            self.draw_vector.stroke(1.25);
        }
    }

    fn draw_gpu_counts_graph(
        &mut self,
        cx: &mut Cx2d,
        rect: &Rect,
        samples: &ProfileSampleStore,
        base_y: f64,
        label_prefix: &str,
        time_offset: f64,
        label: &mut String,
    ) {
        let Some(graph_rect) = self.draw_graph_lane_background(
            cx,
            rect,
            base_y,
            PROFILE_COUNTS_GRAPH_OFFSET_Y,
            label_prefix,
        ) else {
            return;
        };

        let range_len = self.time_range.len().max(MIN_PROFILE_WINDOW_SECONDS);
        let x_scale = rect.size.x / range_len;
        let mut max_count = 0u64;
        let mut visible_count = 0usize;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            visible_count += 1;
            max_count = max_count.max(sample.draw_calls);
            max_count = max_count.max(sample.instances);
            max_count = max_count.max(sample.vertices);
        }

        if visible_count == 0 {
            return;
        }

        let max_count = max_count.max(1);
        let metric_to_y = |value: u64| -> f32 {
            let t = (value as f64 / max_count as f64).clamp(0.0, 1.0);
            (graph_rect.pos.y + graph_rect.size.y - t * (graph_rect.size.y - 1.0)) as f32
        };

        label.clear();
        let _ = write!(
            label,
            "{} GPU counts (max {})",
            if label_prefix.is_empty() { "App" } else { "Self" },
            max_count
        );
        self.draw_time
            .draw_abs(cx, graph_rect.pos + dvec2(4.0, 2.0), label);

        self.draw_vector.set_color(0.95, 0.64, 0.12, 1.0);
        let mut is_first = true;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            let xpos =
                rect.pos.x + (sample_end - self.time_range.start).clamp(0.0, range_len) * x_scale;
            let ypos = metric_to_y(sample.draw_calls);
            if is_first {
                self.draw_vector.move_to(xpos as f32, ypos);
                is_first = false;
            } else {
                self.draw_vector.line_to(xpos as f32, ypos);
            }
        }
        if !is_first {
            self.draw_vector.stroke(1.25);
        }

        self.draw_vector.set_color(0.26, 0.65, 0.96, 1.0);
        is_first = true;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            let xpos =
                rect.pos.x + (sample_end - self.time_range.start).clamp(0.0, range_len) * x_scale;
            let ypos = metric_to_y(sample.instances);
            if is_first {
                self.draw_vector.move_to(xpos as f32, ypos);
                is_first = false;
            } else {
                self.draw_vector.line_to(xpos as f32, ypos);
            }
        }
        if !is_first {
            self.draw_vector.stroke(1.25);
        }

        // "vertices" already contains vertex_count * instance_count from backend.
        self.draw_vector.set_color(0.40, 0.73, 0.42, 1.0);
        is_first = true;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            let xpos =
                rect.pos.x + (sample_end - self.time_range.start).clamp(0.0, range_len) * x_scale;
            let ypos = metric_to_y(sample.vertices);
            if is_first {
                self.draw_vector.move_to(xpos as f32, ypos);
                is_first = false;
            } else {
                self.draw_vector.line_to(xpos as f32, ypos);
            }
        }
        if !is_first {
            self.draw_vector.stroke(1.25);
        }

        let legend_top = graph_rect.pos.y + 14.0;
        let legend_x = graph_rect.pos.x + 6.0;
        let legend = [
            (0xfff18f01, "D"),
            (0xff42a5f5, "I"),
            (0xff66bb6a, "VxI"),
        ];
        for (i, (color, ch)) in legend.iter().enumerate() {
            let x = legend_x + i as f64 * 28.0;
            self.draw_item.color = Vec4f::from_u32(*color);
            self.draw_item.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x, legend_top),
                    size: dvec2(6.0, 6.0),
                },
            );
            self.draw_time
                .draw_abs(cx, dvec2(x + 8.0, legend_top - 4.0), ch);
        }
    }

    fn draw_gpu_upload_graph(
        &mut self,
        cx: &mut Cx2d,
        rect: &Rect,
        samples: &ProfileSampleStore,
        base_y: f64,
        label_prefix: &str,
        time_offset: f64,
        label: &mut String,
    ) {
        let Some(graph_rect) = self.draw_graph_lane_background(
            cx,
            rect,
            base_y,
            PROFILE_UPLOAD_GRAPH_OFFSET_Y,
            label_prefix,
        ) else {
            return;
        };

        let range_len = self.time_range.len().max(MIN_PROFILE_WINDOW_SECONDS);
        let x_scale = rect.size.x / range_len;
        let mut max_bytes = 0u64;
        let mut visible_count = 0usize;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            visible_count += 1;
            max_bytes = max_bytes.max(sample.instance_bytes);
            max_bytes = max_bytes.max(sample.uniform_bytes);
            max_bytes = max_bytes.max(sample.vertex_buffer_bytes);
            max_bytes = max_bytes.max(sample.texture_bytes);
        }
        if visible_count == 0 {
            return;
        }

        let max_bytes = max_bytes.max(1);
        let metric_to_y = |value: u64| -> f32 {
            let t = (value as f64 / max_bytes as f64).clamp(0.0, 1.0);
            (graph_rect.pos.y + graph_rect.size.y - t * (graph_rect.size.y - 1.0)) as f32
        };

        label.clear();
        let _ = write!(
            label,
            "{} GPU upload bytes (max {:.1} KB)",
            if label_prefix.is_empty() { "App" } else { "Self" },
            max_bytes as f64 / 1024.0
        );
        self.draw_time
            .draw_abs(cx, graph_rect.pos + dvec2(4.0, 2.0), label);

        self.draw_vector.set_color(0.94, 0.56, 0.01, 1.0);
        let mut is_first = true;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            let xpos =
                rect.pos.x + (sample_end - self.time_range.start).clamp(0.0, range_len) * x_scale;
            let ypos = metric_to_y(sample.instance_bytes);
            if is_first {
                self.draw_vector.move_to(xpos as f32, ypos);
                is_first = false;
            } else {
                self.draw_vector.line_to(xpos as f32, ypos);
            }
        }
        if !is_first {
            self.draw_vector.stroke(1.25);
        }

        self.draw_vector.set_color(0.26, 0.65, 0.96, 1.0);
        is_first = true;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            let xpos =
                rect.pos.x + (sample_end - self.time_range.start).clamp(0.0, range_len) * x_scale;
            let ypos = metric_to_y(sample.uniform_bytes);
            if is_first {
                self.draw_vector.move_to(xpos as f32, ypos);
                is_first = false;
            } else {
                self.draw_vector.line_to(xpos as f32, ypos);
            }
        }
        if !is_first {
            self.draw_vector.stroke(1.25);
        }

        self.draw_vector.set_color(0.40, 0.73, 0.42, 1.0);
        is_first = true;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            let xpos =
                rect.pos.x + (sample_end - self.time_range.start).clamp(0.0, range_len) * x_scale;
            let ypos = metric_to_y(sample.vertex_buffer_bytes);
            if is_first {
                self.draw_vector.move_to(xpos as f32, ypos);
                is_first = false;
            } else {
                self.draw_vector.line_to(xpos as f32, ypos);
            }
        }
        if !is_first {
            self.draw_vector.stroke(1.25);
        }

        self.draw_vector.set_color(0.93, 0.25, 0.48, 1.0);
        is_first = true;
        for sample in &samples.gpu {
            let sample_end = sample.end + time_offset;
            if sample_end < self.time_range.start {
                continue;
            }
            if sample_end > self.time_range.end {
                break;
            }
            let xpos =
                rect.pos.x + (sample_end - self.time_range.start).clamp(0.0, range_len) * x_scale;
            let ypos = metric_to_y(sample.texture_bytes);
            if is_first {
                self.draw_vector.move_to(xpos as f32, ypos);
                is_first = false;
            } else {
                self.draw_vector.line_to(xpos as f32, ypos);
            }
        }
        if !is_first {
            self.draw_vector.stroke(1.25);
        }

        let legend_top = graph_rect.pos.y + 14.0;
        let legend_x = graph_rect.pos.x + 6.0;
        let legend = [
            (0xfff18f01, "I"),
            (0xff42a5f5, "U"),
            (0xff66bb6a, "V"),
            (0xffec407a, "T"),
        ];
        for (i, (color, ch)) in legend.iter().enumerate() {
            let x = legend_x + i as f64 * 18.0;
            self.draw_item.color = Vec4f::from_u32(*color);
            self.draw_item.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x, legend_top),
                    size: dvec2(6.0, 6.0),
                },
            );
            self.draw_time
                .draw_abs(cx, dvec2(x + 8.0, legend_top - 4.0), ch);
        }
    }

    fn draw_block(
        &mut self,
        cx: &mut Cx2d,
        rect: &Rect,
        sample_start: f64,
        sample_end: f64,
        label: &str,
        meta: u64,
    ) {
        let scale = rect.size.x / self.time_range.len();
        let xpos = rect.pos.x + (sample_start - self.time_range.start) * scale;
        let xsize = ((sample_end - sample_start) * scale).max(2.0);

        let pos = dvec2(xpos, rect.pos.y + 20.0);
        let size = dvec2(xsize, 20.0);
        let rect = Rect { pos, size };

        self.draw_item.draw_abs(cx, rect);
        self.tmp_label.clear();
        if meta > 0 {
            if sample_end - sample_start > 0.001 {
                write!(
                    &mut self.tmp_label,
                    "{}({meta}) {:.2} ms",
                    label,
                    (sample_end - sample_start) * 1000.0
                )
                .unwrap();
            } else {
                write!(
                    &mut self.tmp_label,
                    "{}({meta}) {:.0} µs",
                    label,
                    (sample_end - sample_start) * 1000_000.0
                )
                .unwrap();
            }
        } else {
            if sample_end - sample_start > 0.001 {
                write!(
                    &mut self.tmp_label,
                    "{} {:.2} ms",
                    label,
                    (sample_end - sample_start) * 1000.0
                )
                .unwrap();
            } else {
                write!(
                    &mut self.tmp_label,
                    "{} {:.0} µs",
                    label,
                    (sample_end - sample_start) * 1000_000.0
                )
                .unwrap();
            }
        }

        // if xsize > 10.0 lets draw a clipped piece of text
        if xsize > 10.0 {
            cx.begin_turtle(Walk::abs_rect(rect), Layout::default());
            self.draw_label
                .draw_abs(cx, pos + dvec2(2.0, 4.0), &self.tmp_label);
            cx.end_turtle();
        }
    }
}

impl Widget for ProfilerEventChart {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let bm = &scope.data.get::<AppData>().unwrap().build_manager;
        let mut label = String::new();

        let app_samples = bm.current_profile_store().map(|(_, samples)| samples);
        let self_samples = bm.self_profile_store();

        let has_app_samples = app_samples.map_or(false, Self::has_samples);
        let has_self_samples = Self::has_samples(self_samples);
        let chart_content_height = if has_app_samples && has_self_samples {
            SELF_PROFILE_ROW_OFFSET_Y + PROFILE_STORE_HEIGHT + 8.0
        } else {
            PROFILE_STORE_HEIGHT + 8.0
        }
        .max(220.0);

        let chart_walk = Walk {
            height: Size::Fixed(chart_content_height),
            ..walk
        };
        self.draw_bg.begin(cx, chart_walk, Layout::default());
        let rect = cx.turtle().rect();

        let latest_app_end = app_samples.and_then(Self::latest_sample_end);
        let latest_self_end = if has_self_samples {
            Self::latest_sample_end(self_samples)
        } else {
            None
        };

        // App and studio samples use different local time origins; align self to app's latest.
        // Keep this offset fixed while paused, so the self lane doesn't jitter.
        if self.follow_live {
            if let (Some(app_end), Some(self_end)) = (latest_app_end, latest_self_end) {
                self.self_time_offset = Some(app_end - self_end);
            }
        }
        let self_time_offset = self.self_time_offset.unwrap_or_else(|| {
            match (latest_app_end, latest_self_end) {
                (Some(app_end), Some(self_end)) => app_end - self_end,
                _ => 0.0,
            }
        });

        if has_app_samples || has_self_samples {
            if self.follow_live {
                let latest_sample_end = latest_app_end.or_else(|| {
                    latest_self_end.map(|self_end| self_end + self_time_offset)
                });
                if let Some(latest_sample_end) = latest_sample_end {
                    self.sync_live_window(latest_sample_end);
                }
            }

            self.draw_time_grid(cx, &rect, &mut label);
            self.draw_vector.begin();
            let self_base_y = if has_app_samples {
                SELF_PROFILE_ROW_OFFSET_Y
            } else {
                0.0
            };

            if has_app_samples {
                if let Some(app_samples) = app_samples {
                    self.draw_frame_gap_markers(cx, &rect, app_samples, 0.0, &mut label);
                    self.draw_time.draw_abs(cx, rect.pos + dvec2(4.0, 2.0), "App");
                    self.draw_profile_store(cx, &rect, app_samples, 0.0, "", 0.0);
                    self.draw_gpu_frametime_graph(
                        cx,
                        &rect,
                        app_samples,
                        0.0,
                        "",
                        0.0,
                        &mut label,
                    );
                    self.draw_gpu_counts_graph(cx, &rect, app_samples, 0.0, "", 0.0, &mut label);
                    self.draw_gpu_upload_graph(cx, &rect, app_samples, 0.0, "", 0.0, &mut label);
                }
            } else if has_self_samples {
                self.draw_frame_gap_markers(
                    cx,
                    &rect,
                    self_samples,
                    self_time_offset,
                    &mut label,
                );
            }
            if has_self_samples {
                self.draw_time
                    .draw_abs(cx, rect.pos + dvec2(4.0, self_base_y + 2.0), "Self");
                self.draw_profile_store(
                    cx,
                    &rect,
                    self_samples,
                    self_base_y,
                    "Self ",
                    self_time_offset,
                );
                self.draw_gpu_frametime_graph(
                    cx,
                    &rect,
                    self_samples,
                    self_base_y,
                    "Self ",
                    self_time_offset,
                    &mut label,
                );
                self.draw_gpu_counts_graph(
                    cx,
                    &rect,
                    self_samples,
                    self_base_y,
                    "Self ",
                    self_time_offset,
                    &mut label,
                );
                self.draw_gpu_upload_graph(
                    cx,
                    &rect,
                    self_samples,
                    self_base_y,
                    "Self ",
                    self_time_offset,
                    &mut label,
                );
            }
            self.draw_vector.end(cx);
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if !self.follow_live {
                    cx.set_key_focus(self.draw_bg.area());
                    self.time_drag = Some(self.time_range.clone());
                }
            }
            Hit::FingerMove(fe) => {
                if !self.follow_live {
                    if let Some(start) = &self.time_drag {
                        // ok so how much did we move?
                        let moved = fe.abs_start.x - fe.abs.x;
                        // scale this thing to the time window
                        let scale =
                            self.time_range.len().max(MIN_PROFILE_WINDOW_SECONDS) / fe.rect.size.x;
                        let shift_time = moved * scale;
                        self.time_range = start.shifted(shift_time);
                        self.draw_bg.redraw(cx);
                    }
                }
            }
            Hit::FingerScroll(e) => {
                if e.device.is_mouse() {
                    let zoom = (1.03).powf(e.scroll.y / 150.0);
                    if self.follow_live {
                        let window = self.current_window_seconds().max(MIN_PROFILE_WINDOW_SECONDS);
                        let next_window = (window * zoom).max(MIN_PROFILE_WINDOW_SECONDS);
                        self.time_range = TimeRange {
                            start: self.time_range.end - next_window,
                            end: self.time_range.end,
                        };
                    } else {
                        let scale = self.time_range.len().max(MIN_PROFILE_WINDOW_SECONDS) / e.rect.size.x;
                        let time = scale * (e.abs.x - e.rect.pos.x) + self.time_range.start;
                        self.time_range = TimeRange {
                            start: (self.time_range.start - time) * zoom + time,
                            end: (self.time_range.end - time) * zoom + time,
                        };
                    }
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(_) => {}
            _ => (),
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
struct Profiler {
    #[deref]
    view: View,
    #[rust]
    tmp_status_label: String,
    #[rust]
    tmp_sample_count_label: String,
}

impl WidgetMatchEvent for Profiler {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, scope: &mut Scope) {
        let data = scope.data.get_mut::<AppData>().unwrap();
        if self.button(cx, ids!(clear_button)).clicked(&actions) {
            data.build_manager.clear_profile_samples();
            cx.action(AppAction::RedrawProfiler);
        }

        if let Some(is_running) = self.check_box(cx, ids!(running_button)).changed(actions) {
            data.build_manager.set_profiler_running(is_running);
            if let Some(mut chart) = self
                .view
                .widget(cx, ids!(chart))
                .borrow_mut::<ProfilerEventChart>()
            {
                chart.set_follow_live(cx, is_running);
                if is_running {
                    chart.reset_for_new_session(cx);
                }
            }
            cx.action(AppAction::RedrawProfiler);
        }
    }
}

impl Widget for Profiler {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let bm = &scope.data.get::<AppData>().unwrap().build_manager;
        let running = self.check_box(cx, ids!(running_button)).active(cx);
        if let Some(mut chart) = self
            .view
            .widget(cx, ids!(chart))
            .borrow_mut::<ProfilerEventChart>()
        {
            chart.set_follow_live(cx, running);
        }

        self.tmp_status_label.clear();
        self.tmp_sample_count_label.clear();
        let self_samples = bm.self_profile_store();
        if let Some((build_id, samples)) = bm.current_profile_store() {
            let build_name = bm
                .process_name(build_id)
                .unwrap_or_else(|| format!("build {}", build_id.0));
            let _ = write!(
                &mut self.tmp_status_label,
                "Build: {} ({}) | Self: Studio",
                build_name, build_id.0
            );
            let _ = write!(
                &mut self.tmp_sample_count_label,
                "App E: {} G: {} C: {} | Self E: {} G: {} C: {}",
                samples.event.len(),
                samples.gpu.len(),
                samples.gc.len(),
                self_samples.event.len(),
                self_samples.gpu.len(),
                self_samples.gc.len()
            );
        } else {
            self.tmp_status_label.push_str("Build: - | Self: Studio");
            let _ = write!(
                &mut self.tmp_sample_count_label,
                "App E: 0 G: 0 C: 0 | Self E: {} G: {} C: {}",
                self_samples.event.len(),
                self_samples.gpu.len(),
                self_samples.gc.len()
            );
        }
        self.label(cx, ids!(status_label))
            .set_text_with(|v| {
                v.clear();
                v.push_str(&self.tmp_status_label);
            });
        self.label(cx, ids!(sample_count_label))
            .set_text_with(|v| {
                v.clear();
                v.push_str(&self.tmp_sample_count_label);
            });
        self.label(cx, ids!(window_label))
            .set_text_with(|v| {
                v.clear();
                v.push_str(if running { "Live" } else { "Paused" });
            });

        self.view.draw_walk_all(cx, scope, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope);
    }
}
