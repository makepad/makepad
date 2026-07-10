//! PerfGraph — a small always-on-top frame profiler panel.
//!
//! Drop one anywhere (typically hovering a corner) and it enables the
//! platform's `Cx::perf_monitor` and plots its history live:
//!
//!   - top strip: frame-to-frame gap (pacing — spikes here are visible
//!     hiccups), colored against the 120Hz/60Hz budgets
//!   - bottom strip: stacked per-channel main-thread CPU time (event
//!     dispatch, script exec, GC, pass encode, drawable wait, plus any
//!     channels the app registered via `cx.perf_monitor.channel(...)`)
//!   - legend with per-channel averages
//!
//! The widget redraws itself every frame while visible; hide it (or don't
//! draw it) and it stops scheduling frames.

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PerfGraphBase = #(PerfGraph::register_widget(vm))

    mod.widgets.PerfGraph = set_type_default() do mod.widgets.PerfGraphBase{
        width: Fill
        height: Fill
        panel_width: 330.0
        panel_height: 150.0
        panel_margin: 10.0
        draw_bg +: {
            color: #x10141ada
        }
        draw_text +: {
            color: #xa8bacc
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct PerfGraph {
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
    draw_bg: DrawColor,
    #[live]
    draw_vector: DrawVector,
    #[live]
    draw_text: DrawText,
    /// The plotted panel corner-pins bottom-right inside the widget's walk
    /// rect — self-positioned so it works under Overlay flow without an
    /// aligning parent (deferred alignment would displace the vector layer).
    #[live(330.0)]
    panel_width: f64,
    #[live(150.0)]
    panel_height: f64,
    #[live(10.0)]
    panel_margin: f64,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    frames: Vec<PerfMonitorFrame>,
}

fn channel_color(rgb: u32) -> Vec4f {
    vec4(
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
        1.0,
    )
}

impl Widget for PerfGraph {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let pane = cx.walk_turtle(walk);
        if pane.size.x < 40.0 || pane.size.y < 40.0 {
            return DrawStep::done();
        }
        // Corner-pin the panel bottom-right within the walk rect.
        let m = self.panel_margin;
        let size = dvec2(
            self.panel_width.min(pane.size.x - m * 2.0),
            self.panel_height.min(pane.size.y - m * 2.0),
        );
        let rect = Rect {
            pos: pane.pos + pane.size - size - dvec2(m, m),
            size,
        };
        cx.cx.perf_monitor.set_enabled(true);
        cx.cx.perf_monitor.read(&mut self.frames);
        let channels: Vec<PerfChannelInfo> = cx.cx.perf_monitor.channels().to_vec();
        // Only plot channels that actually carry data.
        let used: Vec<usize> = (0..channels.len())
            .filter(|&i| self.frames.iter().any(|f| f.channel_us[i] > 0))
            .collect();

        self.draw_bg.draw_abs(cx, rect);

        let pad = 8.0f64;
        let x0 = rect.pos.x + pad;
        let plot_w = rect.size.x - pad * 2.0;
        let header_h = 14.0f64;
        let legend_rows = (used.len() as f64 / 3.0).ceil().max(1.0);
        let legend_h = legend_rows * 11.0 + 2.0;
        let strips_h = rect.size.y - pad * 2.0 - header_h - legend_h - 8.0;
        let gap_h = (strips_h * 0.55).max(10.0);
        let cpu_h = (strips_h - gap_h - 4.0).max(8.0);
        let gap_y0 = rect.pos.y + pad + header_h;
        let cpu_y0 = gap_y0 + gap_h + 4.0;

        // Averages over the newest 120 frames (2s), worst over the window.
        let recent = &self.frames[self.frames.len().saturating_sub(120)..];
        let (mut gap_sum, mut gap_n, mut gap_worst) = (0.0f64, 0u32, 0.0f64);
        for f in recent {
            if f.gap_ms > 0.0 {
                gap_sum += f.gap_ms as f64;
                gap_n += 1;
                gap_worst = gap_worst.max(f.gap_ms as f64);
            }
        }
        let fps = if gap_n > 0 && gap_sum > 0.0 {
            1000.0 / (gap_sum / gap_n as f64)
        } else {
            0.0
        };

        self.draw_text.text_style.font_size = 8.0;
        self.draw_text.draw_abs(
            cx,
            dvec2(x0, rect.pos.y + pad),
            &format!("{:.0} fps   worst {:.1}ms", fps, gap_worst),
        );

        // DrawVector geometry maps through the current turtle (see ChartView
        // begin()): open one pinned to our rect or the paths land wherever
        // the parent turtle happens to be.
        cx.begin_turtle(
            Walk {
                abs_pos: Some(rect.pos),
                width: Size::Fixed(rect.size.x),
                height: Size::Fixed(rect.size.y),
                margin: Inset::default(),
                metrics: Metrics::default(),
            },
            Layout {
                clip_x: true,
                clip_y: true,
                ..Layout::default()
            },
        );
        self.draw_vector.begin();

        // ── gap strip: one bar per frame, scaled to 0..33ms ──
        let n = self.frames.len().max(1);
        let bar_w = (plot_w / n as f64) as f32;
        let gap_scale = 33.3f32;
        for (i, f) in self.frames.iter().enumerate() {
            if f.gap_ms <= 0.0 {
                continue;
            }
            let frac = (f.gap_ms / gap_scale).min(1.0);
            // green under the 120Hz budget, amber under 60Hz, red past it
            let c = if f.gap_ms <= 9.0 {
                vec4(0.30, 0.78, 0.42, 1.0)
            } else if f.gap_ms <= 17.5 {
                vec4(0.88, 0.72, 0.28, 1.0)
            } else {
                vec4(0.90, 0.30, 0.28, 1.0)
            };
            self.draw_vector.set_color(c.x, c.y, c.z, c.w);
            let h = frac as f64 * gap_h;
            let bx = x0 + i as f64 * bar_w as f64;
            self.draw_vector
                .rect(bx as f32, (gap_y0 + gap_h - h) as f32, bar_w.max(1.0), h as f32);
            self.draw_vector.fill();
        }
        // budget guides at 8.33ms (120Hz) and 16.7ms (60Hz)
        for guide_ms in [8.33f32, 16.7] {
            let gy = gap_y0 + (1.0 - (guide_ms / gap_scale) as f64) * gap_h;
            self.draw_vector.set_color(1.0, 1.0, 1.0, 0.18);
            self.draw_vector.move_to(x0 as f32, gy as f32);
            self.draw_vector.line_to((x0 + plot_w) as f32, gy as f32);
            self.draw_vector.stroke(1.0);
        }

        // ── cpu strip: stacked channels, auto-scaled (min 4ms full-scale) ──
        let mut cpu_max = 4000u32;
        for f in &self.frames {
            let total: u32 = used.iter().map(|&i| f.channel_us[i]).sum();
            cpu_max = cpu_max.max(total);
        }
        for (i, f) in self.frames.iter().enumerate() {
            let bx = (x0 + i as f64 * bar_w as f64) as f32;
            let mut y = (cpu_y0 + cpu_h) as f32;
            for &ch in &used {
                let us = f.channel_us[ch];
                if us == 0 {
                    continue;
                }
                let h = (us as f64 / cpu_max as f64 * cpu_h) as f32;
                let c = channel_color(channels[ch].color);
                self.draw_vector.set_color(c.x, c.y, c.z, 1.0);
                self.draw_vector.rect(bx, y - h, bar_w.max(1.0), h);
                self.draw_vector.fill();
                y -= h;
            }
        }

        self.draw_vector.end(cx);
        cx.end_turtle();

        // strip captions + budget-guide labels
        self.draw_text.text_style.font_size = 6.5;
        self.draw_text.draw_abs(cx, dvec2(x0 + 1.0, gap_y0 - 1.0), "frame gap  (guides: 8.3ms = 120Hz, 16.7ms = 60Hz)");
        self.draw_text.draw_abs(cx, dvec2(x0 + 1.0, cpu_y0 - 1.0), "cpu stacked + gpu (violet), ms");

        // ── legend: swatch + name + avg ms, three per row ──
        self.draw_text.text_style.font_size = 7.0;
        let bg_color = self.draw_bg.color;
        let legend_y = cpu_y0 + cpu_h + 5.0;
        for (slot, &ch) in used.iter().enumerate() {
            let col = slot % 3;
            let row = slot / 3;
            let lx = x0 + col as f64 * (plot_w / 3.0);
            let ly = legend_y + row as f64 * 11.0;
            let avg_us: f64 = recent.iter().map(|f| f.channel_us[ch] as f64).sum::<f64>()
                / recent.len().max(1) as f64;
            let c = channel_color(channels[ch].color);
            self.draw_bg.color = c;
            self.draw_bg.draw_abs(
                cx,
                Rect {
                    pos: dvec2(lx, ly + 2.0),
                    size: dvec2(6.0, 6.0),
                },
            );
            self.draw_text.draw_abs(
                cx,
                dvec2(lx + 9.0, ly),
                &format!("{} {:.2}", channels[ch].name, avg_us / 1000.0),
            );
        }
        // restore the bg color mutated for the swatches
        self.draw_bg.color = bg_color;

        // live graph: keep frames coming while we're being drawn
        self.next_frame = cx.new_next_frame();
        DrawStep::done()
    }
}
