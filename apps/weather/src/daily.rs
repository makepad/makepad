//! The ten-day list: 44 pt rows of day, condition glyph with its chance,
//! low, the temperature range on one shared domain, and high; today alone
//! carries the current-temperature dot. The range bar is a small shader
//! (track, palette-coloured segment, the dot with its contrast outline).
use crate::model::{glyph, range_color, range_domain, range_fraction, temp, Day, Glyph};
use makepad_widgets::*;

pub const ROW_H: f64 = 44.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*
    use mod.widgets.*

    mod.widgets.DrawRangeBar = set_type_default() do #(DrawRangeBar::script_shader(vm)) {
        ..mod.draw.DrawQuad
        lo: 0.0
        hi: 1.0
        color_lo: #6baeff
        color_hi: #f5a35b
        dot: -1.0
        track: #ffffff33
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            // The 4 pt track through the middle of the 8 pt row band.
            let ty = (h - 4.0) * 0.5
            sdf.box(0.0, ty, w, 4.0, 1.0)
            sdf.fill(self.track)
            // The coloured segment, at least a 4 pt capsule when the ends meet.
            let x0 = self.lo * w
            let x1 = max(self.hi * w, x0 + 4.0)
            sdf.box(x0, ty, x1 - x0, 4.0, 1.0)
            let f = clamp((self.pos.x * w - x0) / max(x1 - x0, 1.0), 0.0, 1.0)
            sdf.fill(mix(self.color_lo, self.color_hi, f))
            // Today's observation: a 4 pt dot with a 1 pt contrast outline.
            if self.dot >= 0.0 {
                let cx = clamp(self.dot * w, 3.0, w - 3.0)
                sdf.circle(cx, h * 0.5, 3.0)
                sdf.fill(vec4(0.10, 0.12, 0.16, 0.9))
                sdf.circle(cx, h * 0.5, 2.0)
                sdf.fill(vec4(1.0, 1.0, 1.0, 1.0))
            }
            return sdf.result
        }
    }

    mod.widgets.ForecastDaysBase = #(ForecastDays::register_widget(vm))
    mod.widgets.ForecastDays = set_type_default() do mod.widgets.ForecastDaysBase{
        width: Fill height: Fit
        day_text +: {text_style: theme.font_bold{font_size: 12.75} color: #ffffff}
        chance_text +: {text_style: theme.font_regular{font_size: 8.25} color: #a8d8ff}
        low_text +: {text_style: theme.font_regular{font_size: 12.75} color: #ffffffbf}
        high_text +: {text_style: theme.font_regular{font_size: 12.75} color: #ffffff}
        line +: {color: #ffffff1f}
        bar: mod.widgets.DrawRangeBar{}
        sun +: {svg: crate_resource("self:resources/icons/sun.svg")}
        moon +: {svg: crate_resource("self:resources/icons/moon.svg")}
        partly_day +: {svg: crate_resource("self:resources/icons/partly-day.svg")}
        partly_night +: {svg: crate_resource("self:resources/icons/partly-night.svg")}
        cloud +: {svg: crate_resource("self:resources/icons/cloud.svg")}
        drizzle +: {svg: crate_resource("self:resources/icons/drizzle.svg")}
        rain +: {svg: crate_resource("self:resources/icons/rain.svg")}
        snow +: {svg: crate_resource("self:resources/icons/snow.svg")}
        fog +: {svg: crate_resource("self:resources/icons/fog.svg")}
        thunder +: {svg: crate_resource("self:resources/icons/thunder.svg")}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRangeBar {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    lo: f32,
    #[live(1.0)]
    hi: f32,
    #[live]
    color_lo: Vec4f,
    #[live]
    color_hi: Vec4f,
    #[live(-1.0)]
    dot: f32,
    #[live]
    track: Vec4f,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ForecastDays {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    day_text: DrawText,
    #[live]
    chance_text: DrawText,
    #[live]
    low_text: DrawText,
    #[live]
    high_text: DrawText,
    #[live]
    line: DrawColor,
    #[live]
    bar: DrawRangeBar,
    #[live]
    sun: DrawSvg,
    #[live]
    moon: DrawSvg,
    #[live]
    partly_day: DrawSvg,
    #[live]
    partly_night: DrawSvg,
    #[live]
    cloud: DrawSvg,
    #[live]
    drizzle: DrawSvg,
    #[live]
    rain: DrawSvg,
    #[live]
    snow: DrawSvg,
    #[live]
    fog: DrawSvg,
    #[live]
    thunder: DrawSvg,
    #[rust]
    days: Vec<Day>,
    #[rust]
    observed: Option<f64>,
}

impl ForecastDays {
    pub fn set_days(&mut self, cx: &mut Cx, days: Vec<Day>, observed: Option<f64>) {
        if days != self.days || observed != self.observed {
            self.days = days;
            self.observed = observed;
            self.redraw(cx);
        }
    }

    fn glyph(&mut self, kind: Glyph) -> &mut DrawSvg {
        match kind {
            Glyph::Sun => &mut self.sun,
            Glyph::Moon => &mut self.moon,
            Glyph::PartlyDay => &mut self.partly_day,
            Glyph::PartlyNight => &mut self.partly_night,
            Glyph::Cloud => &mut self.cloud,
            Glyph::Drizzle => &mut self.drizzle,
            Glyph::Rain => &mut self.rain,
            Glyph::Snow => &mut self.snow,
            Glyph::Fog => &mut self.fog,
            Glyph::Thunder => &mut self.thunder,
        }
    }
}

fn draw_text_at(text: &mut DrawText, cx: &mut Cx2d, x: f64, y: f64, h: f64, right_edge: Option<f64>, s: &str) {
    if let Some(run) = text.prepare_single_line_run(cx, s) {
        let ink = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
        let x = right_edge.map(|e| e - run.width_in_lpxs as f64).unwrap_or(x);
        text.draw_abs(cx, dvec2(x, y + (h - ink) * 0.5), s);
    }
}

impl Widget for ForecastDays {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rows = self.days.len().max(1) as f64;
        let walk = Walk { height: Size::Fixed(rows * ROW_H), ..walk };
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        let w = r.size.x;
        // Columns relative to the inner width: day 0/58, glyph 68/28, low
        // 104/40 right-aligned, range from 154 to the high column, high
        // 40 wide against the right edge.
        let range_x = r.pos.x + 154.0;
        let range_w = (w - 154.0 - 40.0 - 10.0).max(72.0);
        let domain = range_domain(&self.days, self.observed);
        let days = self.days.clone();
        for (i, day) in days.iter().enumerate() {
            let y = r.pos.y + i as f64 * ROW_H;
            let name = if i == 0 { "Today" } else if day.weekday.is_empty() { day.date.as_str() } else { day.weekday };
            draw_text_at(&mut self.day_text, cx, r.pos.x, y, ROW_H, None, name);
            if let Some(code) = day.code {
                let g = glyph(code, true);
                let chance = day.precip_chance.filter(|c| *c >= 1.0);
                let icon_y = if chance.is_some() { y + 4.0 } else { y + (ROW_H - 22.0) * 0.5 };
                self.glyph(g).draw_abs(cx, Rect { pos: dvec2(r.pos.x + 71.0, icon_y), size: dvec2(22.0, 22.0) });
                if let Some(c) = chance {
                    let text = format!("{}%", c.round() as i64);
                    if let Some(run) = self.chance_text.prepare_single_line_run(cx, &text) {
                        let x = r.pos.x + 82.0 - run.width_in_lpxs as f64 * 0.5;
                        self.chance_text.draw_abs(cx, dvec2(x, y + 28.0), &text);
                    }
                }
            }
            draw_text_at(&mut self.low_text, cx, 0.0, y, ROW_H, Some(r.pos.x + 144.0), &temp(day.low));
            draw_text_at(&mut self.high_text, cx, 0.0, y, ROW_H, Some(r.pos.x + w), &temp(day.high));
            // The range bar: only the coloured segment needs both ends.
            let (lo, hi) = (day.low, day.high);
            let (lo_f, hi_f) = match (lo, hi) {
                (Some(lo), Some(hi)) => (range_fraction(lo, domain), range_fraction(hi, domain)),
                _ => (-1.0, -1.0),
            };
            self.bar.lo = lo_f as f32;
            self.bar.hi = hi_f as f32;
            self.bar.color_lo = range_color(lo.unwrap_or(domain.0));
            self.bar.color_hi = range_color(hi.unwrap_or(domain.1));
            self.bar.dot = if i == 0 { self.observed.map(|t| range_fraction(t, domain) as f32).unwrap_or(-1.0) } else { -1.0 };
            if lo_f < 0.0 {
                // Missing endpoint: the track alone.
                self.bar.lo = 0.0;
                self.bar.hi = 0.0;
                self.bar.color_lo = self.bar.track;
                self.bar.color_hi = self.bar.track;
            }
            self.bar.draw_abs(cx, Rect { pos: dvec2(range_x, y + (ROW_H - 8.0) * 0.5), size: dvec2(range_w, 8.0) });
            if i + 1 < days.len() {
                self.line.draw_abs(cx, Rect { pos: dvec2(r.pos.x, y + ROW_H - 0.5), size: dvec2(w, 0.5) });
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
