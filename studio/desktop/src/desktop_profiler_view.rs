use crate::{
    app_data::{AppData, UiProfilerSamples},
    makepad_widgets::*,
};
use makepad_studio_protocol::hub_protocol::QueryId;
use std::fmt::Write;

#[path = "desktop_profiler_view/event_chart.rs"]
pub mod event_chart;
pub use event_chart::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DesktopProfilerEventChartBase = #(DesktopProfilerEventChart::register_widget(vm))
    mod.widgets.DesktopProfilerViewBase = #(DesktopProfilerView::register_widget(vm))

    mod.widgets.DesktopProfilerEventChart = set_type_default() do mod.widgets.DesktopProfilerEventChartBase {
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

    mod.widgets.DesktopProfilerView = set_type_default() do mod.widgets.DesktopProfilerViewBase {
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
                    svg: crate_resource("self://resources/icons/icon_profiler_clear.svg")
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
                    text: "App E: 0 G: 0 C: 0"
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
        chart_scroll := ScrollYView {
            width: Fill
            height: Fill
            scroll_bars.ignore_scroll_input: true
            flow: Down
            chart := mod.widgets.DesktopProfilerEventChart {}
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DesktopProfilerViewAction {
    SetRunning {
        build_id: QueryId,
        running: bool,
    },
    Clear {
        build_id: QueryId,
    },
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct DesktopProfilerView {
    #[deref]
    view: View,
    #[rust]
    tmp_status_label: String,
    #[rust]
    tmp_sample_count_label: String,
}

impl ScriptHook for DesktopProfilerView {}

impl DesktopProfilerView {
    fn profiler_build_id_from_context(&self, cx: &Cx, data: &AppData) -> Option<QueryId> {
        let view_path = cx.widget_tree().path_to(self.view.widget_uid());
        view_path.iter().rev().copied().find_map(|tab_id| {
            data.profiler_tab_state
                .get(&tab_id)
                .map(|state| state.build_id)
        })
    }
}

impl WidgetMatchEvent for DesktopProfilerView {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, scope: &mut Scope) {
        let build_id = {
            let Some(data) = scope.data.get::<AppData>() else {
                return;
            };
            let Some(build_id) = self.profiler_build_id_from_context(cx, data) else {
                return;
            };
            build_id
        };

        if self.view.button(cx, ids!(clear_button)).clicked(&actions) {
            cx.widget_action(
                self.widget_uid(),
                DesktopProfilerViewAction::Clear { build_id },
            );
        }

        if let Some(running) = self
            .view
            .check_box(cx, ids!(running_button))
            .changed(actions)
        {
            if let Some(mut chart) = self
                .view
                .widget(cx, ids!(chart))
                .borrow_mut::<DesktopProfilerEventChart>()
            {
                chart.set_follow_live(cx, running);
                if running {
                    chart.reset_for_new_session(cx);
                }
            }
            cx.widget_action(
                self.widget_uid(),
                DesktopProfilerViewAction::SetRunning { build_id, running },
            );
        }
    }
}

impl Widget for DesktopProfilerView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Some(data) = scope.data.get::<AppData>() else {
            self.view.draw_walk_all(cx, scope, walk);
            return DrawStep::done();
        };

        let build_id = self.profiler_build_id_from_context(cx, data);
        let running = build_id
            .and_then(|id| data.profiler_running_by_build.get(&id).copied())
            .unwrap_or(true);

        self.view
            .check_box(cx, ids!(running_button))
            .set_active(cx, running, Animate::Yes);
        if let Some(mut chart) = self
            .view
            .widget(cx, ids!(chart))
            .borrow_mut::<DesktopProfilerEventChart>()
        {
            chart.set_follow_live(cx, running);
        }

        self.tmp_status_label.clear();
        self.tmp_sample_count_label.clear();

        if let Some(build_id) = build_id {
            let empty_samples = UiProfilerSamples::default();
            let samples = data
                .profiler_samples_by_build
                .get(&build_id)
                .unwrap_or(&empty_samples);
            let title = data
                .build_package
                .get(&build_id)
                .cloned()
                .unwrap_or_else(|| format!("build {}", build_id.0));
            let _ = write!(
                &mut self.tmp_status_label,
                "Build: {} ({})",
                title, build_id.0
            );
            let _ = write!(
                &mut self.tmp_sample_count_label,
                "App E: {} G: {} C: {}",
                samples.event_samples.len(),
                samples.gpu_samples.len(),
                samples.gc_samples.len(),
            );
        } else {
            self.tmp_status_label.push_str("Build: -");
            self.tmp_sample_count_label.push_str("App E: 0 G: 0 C: 0");
        }

        self.view.label(cx, ids!(status_label)).set_text_with(|v| {
            v.clear();
            v.push_str(&self.tmp_status_label);
        });
        self.view
            .label(cx, ids!(sample_count_label))
            .set_text_with(|v| {
                v.clear();
                v.push_str(&self.tmp_sample_count_label);
            });
        self.view.label(cx, ids!(window_label)).set_text_with(|v| {
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
