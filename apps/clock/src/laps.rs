//! The stopwatch's laps: 48 pt rows, the lap's label at the left and its
//! duration at the right, hairlines between, the current lap first. After
//! two completed laps the fastest reads in the start colour and the
//! slowest in the stop colour. Scrolls under the finger.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.LapListBase = #(LapList::register_widget(vm))
    mod.widgets.LapList = set_type_default() do mod.widgets.LapListBase {
        width: Fill height: Fill
        text +: {text_style: theme.font_regular{font_size: 12.75} color: theme.color_text}
        line +: {color: theme.color_text_disabled}
        color_fastest: #187b36
        color_slowest: #c52f35
    }
}

pub const ROW: f64 = 48.0;

#[derive(Script, ScriptHook, Widget)]
pub struct LapList {
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
    text: DrawText,
    #[live]
    line: DrawColor,
    #[live]
    color_fastest: Vec4f,
    #[live]
    color_slowest: Vec4f,
    /// Completed lap durations, oldest first.
    #[rust]
    laps: Vec<f64>,
    /// The current lap's running duration, when the stopwatch has laps.
    #[rust]
    current: Option<f64>,
    #[rust]
    scroll: f64,
    #[rust]
    drag: Option<(f64, f64)>,
}

impl LapList {
    pub fn set(&mut self, cx: &mut Cx, laps: &[f64], current: Option<f64>) {
        if self.laps != laps || self.current != current {
            self.laps = laps.to_vec();
            self.current = current;
            self.redraw(cx);
        }
    }

    fn rows(&self) -> usize {
        self.laps.len() + usize::from(self.current.is_some())
    }

    fn clamp_scroll(&mut self, cx: &Cx) {
        self.scroll = self.scroll.clamp(0.0, (self.rows() as f64 * ROW - self.area.rect(cx).size.y).max(0.0));
    }
}

fn mm_ss_hh(secs: f64) -> String {
    let hundredths = (secs * 100.0).floor() as u64;
    let (m, s, h) = (hundredths / 6000, (hundredths / 100) % 60, hundredths % 100);
    format!("{:02}:{:02}.{:02}", m, s, h)
}

impl Widget for LapList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(e) if e.is_primary_hit() => self.drag = Some((e.abs.y, self.scroll)),
            Hit::FingerMove(e) => {
                if let Some((y, start)) = self.drag {
                    self.scroll = start - (e.abs.y - y);
                    self.clamp_scroll(cx);
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(_) => self.drag = None,
            Hit::FingerScroll(e) => {
                self.scroll += e.scroll.y;
                self.clamp_scroll(cx);
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout { clip_x: true, clip_y: true, ..Default::default() });
        let r = cx.turtle().rect();
        self.scroll = self.scroll.clamp(0.0, (self.rows() as f64 * ROW - r.size.y).max(0.0));
        let (fastest, slowest) = if self.laps.len() >= 2 {
            let mut fast = 0;
            let mut slow = 0;
            for (i, v) in self.laps.iter().enumerate() {
                if *v < self.laps[fast] {
                    fast = i;
                }
                if *v > self.laps[slow] {
                    slow = i;
                }
            }
            (Some(fast), Some(slow))
        } else {
            (None, None)
        };
        let base = self.text.color;
        let base_line = self.line.color;
        self.line.color = vec4(base_line.x, base_line.y, base_line.z, 0.45);
        // Rows: the current lap first, then completed laps newest first.
        let mut entries: Vec<(String, f64, Vec4f)> = Vec::new();
        if let Some(cur) = self.current {
            entries.push((format!("Lap {}", self.laps.len() + 1), cur, base));
        }
        for (i, v) in self.laps.iter().enumerate().rev() {
            let color = if Some(i) == fastest { self.color_fastest } else if Some(i) == slowest { self.color_slowest } else { base };
            entries.push((format!("Lap {}", i + 1), *v, color));
        }
        for (row, (label, secs, color)) in entries.iter().enumerate() {
            let y = r.pos.y + row as f64 * ROW - self.scroll;
            if y + ROW < r.pos.y || y > r.pos.y + r.size.y {
                continue;
            }
            self.text.color = *color;
            if let Some(run) = self.text.prepare_single_line_run(cx, label) {
                let ink = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
                self.text.draw_abs(cx, dvec2(r.pos.x, y + (ROW - ink) * 0.5), label);
            }
            let time = mm_ss_hh(*secs);
            crate::ring::draw_tabular(&mut self.text, cx, dvec2(r.pos.x + r.size.x - 48.0, y + ROW * 0.5), &time);
            if row + 1 < entries.len() {
                self.line.draw_abs(cx, Rect { pos: dvec2(r.pos.x, y + ROW - 0.5), size: dvec2(r.size.x, 0.5) });
            }
        }
        self.text.color = base;
        self.line.color = base_line;
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lap_durations_format_with_hundredths() {
        assert_eq!(mm_ss_hh(0.0), "00:00.00");
        assert_eq!(mm_ss_hh(61.257), "01:01.25");
    }
}
