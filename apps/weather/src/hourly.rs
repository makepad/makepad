//! The hourly strip: "Now" and the following hours, sunrise and sunset
//! inserted where they fall, in 56 pt cells 8 apart that scroll sideways
//! under the finger (release decays at 6/s). Cells are drawn here, not
//! instantiated: the strip is one widget with its own texts and glyphs.
use crate::model::{glyph, temp, Glyph, HourCell};
use makepad_widgets::*;

pub const CELL_W: f64 = 56.0;
pub const CELL_GAP: f64 = 8.0;
pub const STRIP_H: f64 = 88.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.GlyphIconBase = #(GlyphIcon::register_widget(vm))
    mod.widgets.GlyphIcon = set_type_default() do mod.widgets.GlyphIconBase{
        width: 24 height: 24
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

    mod.widgets.HourlyStripBase = #(HourlyStrip::register_widget(vm))
    mod.widgets.HourlyStrip = set_type_default() do mod.widgets.HourlyStripBase{
        width: Fill height: 88
        time_text +: {text_style: theme.font_bold{font_size: 9.75} color: #ffffff}
        chance_text +: {text_style: theme.font_regular{font_size: 8.25} color: #a8d8ff}
        temp_text +: {text_style: theme.font_bold{font_size: 12.75} color: #ffffff}
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
        sunrise +: {svg: crate_resource("self:resources/icons/sunrise.svg")}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct HourlyStrip {
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
    time_text: DrawText,
    #[live]
    chance_text: DrawText,
    #[live]
    temp_text: DrawText,
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
    #[live]
    sunrise: DrawSvg,
    #[rust]
    cells: Vec<HourCell>,
    #[rust]
    scroll: f64,
    #[rust]
    velocity: f64,
    #[rust]
    drag: Option<(f64, f64, f64)>,
    #[rust]
    next: NextFrame,
    #[rust]
    last: f64,
}

impl HourlyStrip {
    pub fn set_cells(&mut self, cx: &mut Cx, cells: Vec<HourCell>) {
        if cells != self.cells {
            self.cells = cells;
            self.scroll = 0.0;
            self.velocity = 0.0;
            self.redraw(cx);
        }
    }

    fn content_width(&self) -> f64 {
        self.cells.len() as f64 * (CELL_W + CELL_GAP) - CELL_GAP
    }

    fn max_scroll(&self, cx: &Cx) -> f64 {
        (self.content_width() - self.area.rect(cx).size.x).max(0.0)
    }

    fn animate(&mut self, cx: &mut Cx) {
        self.next = cx.new_next_frame();
        self.redraw(cx);
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

fn draw_centered(text: &mut DrawText, cx: &mut Cx2d, cell: Rect, y: f64, h: f64, s: &str) {
    if let Some(run) = text.prepare_single_line_run(cx, s) {
        let x = cell.pos.x + (cell.size.x - run.width_in_lpxs as f64) * 0.5;
        let ink = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
        text.draw_abs(cx, dvec2(x, cell.pos.y + y + (h - ink) * 0.5), s);
    }
}

impl Widget for HourlyStrip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(frame) = self.next.is_event(event) {
            let dt = if self.last == 0.0 { 1.0 / 60.0 } else { (frame.time - self.last).clamp(0.001, 0.05) };
            self.last = frame.time;
            if self.drag.is_none() && self.velocity.abs() > 4.0 {
                self.scroll = (self.scroll - self.velocity * dt).clamp(0.0, self.max_scroll(cx));
                self.velocity *= (-dt * 6.0).exp();
                self.animate(cx);
            } else {
                self.velocity = 0.0;
                self.redraw(cx);
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(e) if e.is_primary_hit() => {
                self.velocity = 0.0;
                self.last = 0.0;
                self.drag = Some((e.abs.x, self.scroll, e.time));
            }
            Hit::FingerMove(e) => {
                if let Some((x, start, time)) = self.drag {
                    let previous = self.scroll;
                    self.scroll = (start - (e.abs.x - x)).clamp(0.0, self.max_scroll(cx));
                    let dt = (e.time - time).max(0.008);
                    self.velocity = ((previous - self.scroll) / dt).clamp(-3000.0, 3000.0);
                    self.drag = Some((e.abs.x, self.scroll, e.time));
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(e) if e.is_primary_hit() => {
                self.drag = None;
                self.animate(cx);
            }
            Hit::FingerScroll(e) => {
                self.scroll = (self.scroll + e.scroll.x + e.scroll.y).clamp(0.0, self.max_scroll(cx));
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout { clip_x: true, clip_y: true, ..Default::default() });
        let r = cx.turtle().rect();
        self.scroll = self.scroll.clamp(0.0, (self.content_width() - r.size.x).max(0.0));
        let cells = self.cells.clone();
        for (i, cell) in cells.iter().enumerate() {
            let x = r.pos.x + i as f64 * (CELL_W + CELL_GAP) - self.scroll;
            if x + CELL_W < r.pos.x || x > r.pos.x + r.size.x {
                continue;
            }
            let rect = Rect { pos: dvec2(x, r.pos.y), size: dvec2(CELL_W, STRIP_H) };
            match cell {
                HourCell::Hour { label, code, is_day, chance, temp: t, .. } => {
                    draw_centered(&mut self.time_text, cx, rect, 0.0, 18.0, label);
                    let icon = self.glyph(glyph(*code, *is_day));
                    icon.draw_abs(cx, Rect { pos: dvec2(x + (CELL_W - 24.0) * 0.5, r.pos.y + 24.0), size: dvec2(24.0, 24.0) });
                    // The precipitation line is reserved even when there is none.
                    if let Some(c) = chance.filter(|c| *c >= 1.0) {
                        draw_centered(&mut self.chance_text, cx, rect, 51.0, 14.0, &format!("{}%", c.round() as i64));
                    }
                    draw_centered(&mut self.temp_text, cx, rect, 67.0, 21.0, &temp(*t));
                }
                HourCell::Sunrise { label } | HourCell::Sunset { label } => {
                    draw_centered(&mut self.time_text, cx, rect, 0.0, 18.0, label);
                    self.sunrise.draw_abs(cx, Rect { pos: dvec2(x + (CELL_W - 24.0) * 0.5, r.pos.y + 24.0), size: dvec2(24.0, 24.0) });
                    let word = if matches!(cell, HourCell::Sunrise { .. }) { "Sunrise" } else { "Sunset" };
                    draw_centered(&mut self.chance_text, cx, rect, 67.0, 21.0, word);
                }
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

/// One condition glyph, chosen at runtime from the crate's own set (an
/// SVG resource cannot be re-pointed by a runtime apply, so every glyph
/// is resident and one is drawn).
#[derive(Script, ScriptHook, Widget)]
pub struct GlyphIcon {
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
    glyph: Option<Glyph>,
}

impl GlyphIcon {
    pub fn set_glyph(&mut self, cx: &mut Cx, glyph: Option<Glyph>) {
        if self.glyph != glyph {
            self.glyph = glyph;
            self.redraw(cx);
        }
    }
}

impl Widget for GlyphIcon {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        if let Some(glyph) = self.glyph {
            let icon = match glyph {
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
            };
            icon.draw_abs(cx, r);
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
