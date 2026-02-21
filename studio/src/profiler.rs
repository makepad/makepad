use {
    crate::{
        app::{AppAction, AppData},
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
        height: Fill
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
                active: true
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
                    text: "Events: 0 GPU: 0 GC: 0"
                    margin: 0.
                    padding: theme.mspace_v_1
                }
                window_label := Pbold {
                    width: Fit
                    text: "Live"
                    margin: 0.
                    padding: theme.mspace_v_1
                }
            }
        }
        chart := mod.widgets.ProfilerEventChart {}
    }
}

const DEFAULT_PROFILE_WINDOW_SECONDS: f64 = 0.5;
const MIN_PROFILE_WINDOW_SECONDS: f64 = 0.000_01;

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
    draw_label: DrawText,
    #[live]
    draw_time: DrawText,
    #[rust(TimeRange{start:0.0, end: 0.5})]
    time_range: TimeRange,
    #[rust]
    time_drag: Option<TimeRange>,
    #[rust(true)]
    follow_live: bool,
    #[rust]
    tmp_label: String,
}

impl ProfilerEventChart {
    fn set_follow_live(&mut self, cx: &mut Cx, follow_live: bool) {
        if self.follow_live != follow_live {
            self.follow_live = follow_live;
            self.time_drag = None;
            self.draw_bg.redraw(cx);
        }
    }

    fn current_window_seconds(&self) -> f64 {
        self.time_range.len().max(DEFAULT_PROFILE_WINDOW_SECONDS)
    }

    fn sync_live_window(&mut self, latest_sample_end: f64) {
        let window = self.current_window_seconds().max(MIN_PROFILE_WINDOW_SECONDS);
        self.time_range = TimeRange {
            start: latest_sample_end - window,
            end: latest_sample_end,
        };
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
        self.draw_bg.begin(cx, walk, Layout::default());
        let bm = &scope.data.get::<AppData>().unwrap().build_manager;
        let mut label = String::new();

        let rect = cx.turtle().rect();
        if let Some((_build_id, pss)) = bm.current_profile_store() {
            if self.follow_live {
                let latest_event_end = pss.event.last().map(|sample| sample.end);
                let latest_gpu_end = pss.gpu.last().map(|sample| sample.end);
                let latest_gc_end = pss.gc.last().map(|sample| sample.end);
                let latest_sample_end = [latest_event_end, latest_gpu_end, latest_gc_end]
                    .into_iter()
                    .flatten()
                    .max_by(|a, b| a.total_cmp(b));
                if let Some(latest_sample_end) = latest_sample_end {
                    self.sync_live_window(latest_sample_end);
                }
            }

            let range_len = self.time_range.len().max(MIN_PROFILE_WINDOW_SECONDS);
            let scale = rect.size.x / range_len;

            let mut step_size = 0.008;
            while range_len / step_size > rect.size.x / 80.0 {
                step_size *= 2.0;
            }

            while range_len / step_size < rect.size.x / 80.0 {
                step_size /= 2.0;
            }

            let mut iter =
                (self.time_range.start / step_size).floor() * step_size - self.time_range.start;
            while iter < range_len {
                let xpos = iter * scale;
                let pos = dvec2(xpos, 0.0) + rect.pos;
                self.draw_line.draw_abs(
                    cx,
                    Rect {
                        pos,
                        size: dvec2(3.0, rect.size.y),
                    },
                );
                label.clear();
                write!(&mut label, "{:.3}s", (iter + self.time_range.start)).unwrap();
                self.draw_time.draw_abs(cx, pos + dvec2(2.0, 2.0), &label);
                iter += step_size;
            }

            if let Some(first) = pss.event.iter().position(|v| v.end > self.time_range.start) {
                // lets draw the time lines and time text
                for i in first..pss.event.len() {
                    let sample = &pss.event[i];
                    if sample.start > self.time_range.end {
                        break;
                    }
                    let color = LiveId(0).bytes_append(&sample.event_u32.to_be_bytes()).0 as u32
                        | 0xff000000;
                    self.draw_item.color = Vec4f::from_u32(color);
                    self.draw_block(
                        cx,
                        &rect,
                        sample.start,
                        sample.end,
                        Event::name_from_u32(sample.event_u32),
                        sample.event_meta,
                    );
                }
            }

            self.draw_item.color = Vec4f::from_u32(0x7f7f7fff);
            if let Some(first) = pss.gpu.iter().position(|v| v.end > self.time_range.start) {
                // lets draw the time lines and time text
                for i in first..pss.gpu.len() {
                    let sample = &pss.gpu[i];
                    if sample.start > self.time_range.end {
                        break;
                    }
                    self.draw_block(
                        cx,
                        &Rect {
                            pos: rect.pos + dvec2(0.0, 25.0),
                            size: rect.size,
                        },
                        sample.start,
                        sample.end,
                        "GPU",
                        0,
                    );
                }
            }

            self.draw_item.color = Vec4f::from_u32(0x5eb27fff);
            if let Some(first) = pss.gc.iter().position(|v| v.end > self.time_range.start) {
                for i in first..pss.gc.len() {
                    let sample = &pss.gc[i];
                    if sample.start > self.time_range.end {
                        break;
                    }
                    self.draw_block(
                        cx,
                        &Rect {
                            pos: rect.pos + dvec2(0.0, 50.0),
                            size: rect.size,
                        },
                        sample.start,
                        sample.end,
                        "GC",
                        sample.heap_live,
                    );
                }
            }
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
                    let scale = self.time_range.len().max(MIN_PROFILE_WINDOW_SECONDS) / fe.rect.size.x;
                    let shift_time = moved * scale;
                    self.time_range = start.shifted(shift_time);
                    self.draw_bg.redraw(cx);
                }
                }
            }
            Hit::FingerScroll(e) => {
                if !self.follow_live && e.device.is_mouse() {
                    let zoom = (1.03).powf(e.scroll.y / 150.0);
                    let scale = self.time_range.len().max(MIN_PROFILE_WINDOW_SECONDS) / e.rect.size.x;
                    let time = scale * (e.abs.x - e.rect.pos.x) + self.time_range.start;
                    self.time_range = TimeRange {
                        start: (self.time_range.start - time) * zoom + time,
                        end: (self.time_range.end - time) * zoom + time,
                    };
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
            if let Some(mut chart) = self
                .view
                .widget(cx, ids!(chart))
                .borrow_mut::<ProfilerEventChart>()
            {
                chart.set_follow_live(cx, is_running);
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
        if let Some((build_id, samples)) = bm.current_profile_store() {
            let build_name = bm
                .process_name(build_id)
                .unwrap_or_else(|| format!("build {}", build_id.0));
            let _ = write!(
                &mut self.tmp_status_label,
                "Build: {} ({})",
                build_name, build_id.0
            );
            let _ = write!(
                &mut self.tmp_sample_count_label,
                "Events: {} GPU: {} GC: {}",
                samples.event.len(),
                samples.gpu.len(),
                samples.gc.len()
            );
        } else {
            self.tmp_status_label.push_str("Build: -");
            self.tmp_sample_count_label.push_str("Events: 0 GPU: 0 GC: 0");
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
