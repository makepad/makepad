//! ElevationGraph — route height profile panel, self-pinned bottom-center
//! above the route bar. DrawVector geometry is aligned-instance, so the
//! widget takes Fill/Fill in the overlay stack and positions its own panel
//! (never place it under an aligning parent).

use crate::dem::ElevationProfile;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ElevationGraphBase = #(ElevationGraph::register_widget(vm))
    mod.widgets.ElevationGraph = set_type_default() do mod.widgets.ElevationGraphBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: #xfffffff2
        }
        draw_text +: {
            color: #x51626e
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ElevationGraph {
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
    #[live(420.0)]
    panel_width: f64,
    #[live(92.0)]
    panel_height: f64,
    /// Gap under the panel so it floats above the route bar.
    #[live(64.0)]
    bottom_margin: f64,
    #[rust]
    profile: Option<ElevationProfile>,
}

impl ElevationGraph {
    pub fn set_profile(&mut self, cx: &mut Cx, profile: Option<ElevationProfile>) {
        self.profile = profile;
        // Own-area redraw is a no-op while we've never drawn (empty area
        // before the first profile arrives) — repaint the window instead.
        cx.redraw_all();
    }
}

fn fmt_m(m: f32) -> String {
    format!("{}m", m.round() as i64)
}

impl Widget for ElevationGraph {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let pane = cx.walk_turtle(walk);
        let Some(profile) = self.profile.clone() else {
            return DrawStep::done();
        };
        if profile.samples.len() < 2 || pane.size.x < 80.0 {
            return DrawStep::done();
        }
        let size = dvec2(
            self.panel_width.min(pane.size.x - 24.0),
            self.panel_height,
        );
        let rect = Rect {
            pos: dvec2(
                pane.pos.x + (pane.size.x - size.x) * 0.5,
                pane.pos.y + pane.size.y - size.y - self.bottom_margin,
            ),
            size,
        };
        self.draw_bg.draw_abs(cx, rect);

        let pad = 8.0f64;
        let header_h = 13.0;
        let x0 = rect.pos.x + pad;
        let plot_w = rect.size.x - pad * 2.0;
        let y0 = rect.pos.y + pad + header_h;
        let plot_h = rect.size.y - pad * 2.0 - header_h - 10.0;

        // Flat routes still get a sensible vertical scale.
        let span = ((profile.max_elev - profile.min_elev) as f64).max(10.0);
        let base = profile.min_elev as f64 - (span * 0.05);
        let scale = plot_h / (span * 1.1);
        let total = profile.total_m.max(1.0) as f64;

        // See PerfGraph: DrawVector maps through the current turtle, pin one.
        cx.begin_turtle(
            Walk {
                abs_pos: Some(rect.pos),
                width: Size::Fixed(rect.size.x),
                height: Size::Fixed(rect.size.y),
                margin: Inset::default(),
                metrics: Metrics::default(),
                ..Default::default()
            },
            Layout {
                clip_x: true,
                clip_y: true,
                ..Layout::default()
            },
        );
        self.draw_vector.begin();

        let px = |d: f32| (x0 + d as f64 / total * plot_w) as f32;
        let py = |e: f32| (y0 + plot_h - (e as f64 - base) * scale) as f32;

        // Filled area under the curve, then the profile line on top.
        self.draw_vector.set_color(0.30, 0.55, 0.92, 0.25);
        let (d0, e0) = profile.samples[0];
        self.draw_vector.move_to(px(d0), (y0 + plot_h) as f32);
        self.draw_vector.line_to(px(d0), py(e0));
        for &(d, e) in &profile.samples[1..] {
            self.draw_vector.line_to(px(d), py(e));
        }
        let (dl, _) = *profile.samples.last().unwrap();
        self.draw_vector.line_to(px(dl), (y0 + plot_h) as f32);
        self.draw_vector.fill();

        self.draw_vector.set_color(0.16, 0.42, 0.85, 1.0);
        self.draw_vector.move_to(px(d0), py(e0));
        for &(d, e) in &profile.samples[1..] {
            self.draw_vector.line_to(px(d), py(e));
        }
        self.draw_vector.stroke(1.6);

        self.draw_vector.end(cx);
        cx.end_turtle();

        self.draw_text.text_style.font_size = 7.5;
        self.draw_text.draw_abs(
            cx,
            dvec2(x0, rect.pos.y + pad - 2.0),
            &format!(
                "Elevation  ↗ {}  ↘ {}   {} – {}",
                fmt_m(profile.ascent_m),
                fmt_m(profile.descent_m),
                fmt_m(profile.min_elev),
                fmt_m(profile.max_elev),
            ),
        );
        DrawStep::done()
    }
}
