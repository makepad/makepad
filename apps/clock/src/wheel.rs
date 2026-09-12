//! Cyclic, inertial value wheels: two for a time of day (hours, minutes),
//! three for a duration (hours, minutes, seconds) with their unit words
//! beside the digits. Five 44 pt rows, digits 30/400, units 15/400, the
//! selection band a Surface capsule (visible radius 10). Momentum decays
//! at 8/s and the final snap is critically damped (≈180 ms). The selected
//! values stay resident across layout changes and theme reapplication.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.TimeWheel = set_type_default() do #(TimeWheel::register_widget(vm)) {
        width: Fill height: 220
        columns: 2
        text +: {text_style: theme.font_regular{font_size: 22.5} color: theme.color_text}
        unit +: {text_style: theme.font_regular{font_size: 11.25} color: theme.color_text}
        band +: {
            color: theme.color_inset
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 5.0)
                sdf.fill(self.color)
                return sdf.result
            }
        }
    }
    mod.widgets.DurationWheel = mod.widgets.TimeWheel{columns: 3}
}

const ROW: f64 = 44.0;
const UNITS: [&str; 3] = ["hours", "min", "sec"];

#[derive(Script, ScriptHook, Widget)]
pub struct TimeWheel {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[redraw] #[rust] area: Area,
    #[live] text: DrawText,
    #[live] unit: DrawText,
    #[live] band: DrawColor,
    /// 2 = hours and minutes (a time of day), 3 = hours, minutes, seconds.
    #[live(2)] columns: usize,
    #[rust] positions: [f64; 3],
    #[rust] velocity: f64,
    #[rust] active: usize,
    #[rust] drag: Option<(f64, f64, f64)>,
    #[rust] next: NextFrame,
    #[rust] last: f64,
}

impl TimeWheel {
    pub fn set_time(&mut self, cx: &mut Cx, hour: u32, minute: u32) {
        self.positions = [hour as f64, minute as f64, 0.0];
        self.velocity = 0.0;
        self.drag = None;
        self.redraw(cx);
    }
    pub fn time(&self) -> (u32, u32) {
        (wheel_value(self.positions[0], self.count(0)), wheel_value(self.positions[1], 60))
    }
    /// A duration in seconds, for the three-column wheel.
    pub fn set_duration(&mut self, cx: &mut Cx, seconds: u64) {
        let seconds = seconds.min(23 * 3600 + 59 * 60 + 59);
        self.positions = [(seconds / 3600) as f64, ((seconds / 60) % 60) as f64, (seconds % 60) as f64];
        self.velocity = 0.0;
        self.drag = None;
        self.redraw(cx);
    }
    pub fn duration(&self) -> u64 {
        let (h, m, s) = (wheel_value(self.positions[0], 24), wheel_value(self.positions[1], 60), wheel_value(self.positions[2], 60));
        (h as u64) * 3600 + (m as u64) * 60 + s as u64
    }
    fn count(&self, column: usize) -> i64 {
        if column == 0 { 24 } else { 60 }
    }
    fn column_at(&self, cx: &Cx, x: f64) -> usize {
        let r = self.area.rect(cx);
        let n = self.columns.max(1) as f64;
        (((x - r.pos.x) / r.size.x * n).floor().max(0.0) as usize).min(self.columns.max(1) - 1)
    }
    fn animate(&mut self, cx: &mut Cx) {
        self.next = cx.new_next_frame();
        self.redraw(cx);
    }
}

fn wheel_value(position: f64, count: i64) -> u32 {
    (position.round() as i64).rem_euclid(count) as u32
}

impl Widget for TimeWheel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(frame) = self.next.is_event(event) {
            let dt = if self.last == 0.0 { 1.0 / 60.0 } else { (frame.time - self.last).clamp(0.001, 0.04) };
            self.last = frame.time;
            if self.drag.is_none() {
                let p = &mut self.positions[self.active];
                if self.velocity.abs() > 0.6 {
                    *p += self.velocity * dt;
                    self.velocity *= (-dt * 8.0).exp();
                } else {
                    self.velocity = 0.0;
                    // Critically damped settle: about 180 ms to rest.
                    *p += (p.round() - *p) * (1.0 - (-dt * 22.0).exp());
                }
                if self.velocity != 0.0 || (*p - p.round()).abs() > 0.002 {
                    self.animate(cx);
                } else {
                    *p = p.round();
                    self.redraw(cx);
                }
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(e) if e.is_primary_hit() => {
                self.active = self.column_at(cx, e.abs.x);
                self.velocity = 0.0;
                self.last = 0.0;
                self.drag = Some((e.abs.y, self.positions[self.active], e.time));
                cx.set_key_focus(self.area);
            }
            Hit::FingerMove(e) => {
                if let Some((y, start, time)) = self.drag {
                    let previous = self.positions[self.active];
                    let current = start - (e.abs.y - y) / ROW;
                    let dt = (e.time - time).max(0.008);
                    self.velocity = ((current - previous) / dt).clamp(-28.0, 28.0);
                    self.positions[self.active] = current;
                    self.drag = Some((e.abs.y, current, e.time));
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(e) if e.is_primary_hit() => {
                self.drag = None;
                self.animate(cx);
            }
            Hit::FingerScroll(e) => {
                self.active = self.column_at(cx, e.abs.x);
                self.positions[self.active] += e.scroll.y / ROW;
                self.velocity = 0.0;
                self.animate(cx);
            }
            Hit::KeyDown(e) => match e.key_code {
                KeyCode::ArrowUp => {
                    self.positions[self.active] -= 1.0;
                    self.redraw(cx);
                }
                KeyCode::ArrowDown => {
                    self.positions[self.active] += 1.0;
                    self.redraw(cx);
                }
                KeyCode::ArrowLeft => self.active = self.active.saturating_sub(1),
                KeyCode::ArrowRight | KeyCode::Tab => self.active = (self.active + 1).min(self.columns.max(1) - 1),
                _ => {}
            },
            _ => {}
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        let n = self.columns.max(1);
        let col_w = r.size.x / n as f64;
        self.band.draw_abs(cx, Rect { pos: dvec2(r.pos.x, r.center().y - ROW * 0.5), size: dvec2(r.size.x, ROW) });
        let color = self.text.color;
        let size = self.text.text_style.font_size;
        for dial in 0..n {
            let p = self.positions[dial];
            let base = p.floor();
            let count = self.count(dial);
            // The digits sit left of the column's centre, the unit word to the right.
            let digits_x = r.pos.x + col_w * (dial as f64 + 0.5) - if n == 3 { 14.0 } else { 0.0 };
            for offset in -3..=3 {
                let distance = base + offset as f64 - p;
                let angle = distance * 0.52;
                if angle.abs() > 1.5 {
                    continue;
                }
                let depth = angle.cos();
                self.text.text_style.font_size = size * (0.72 + depth * 0.28) as f32;
                self.text.color = color * (depth * depth * 0.75 + 0.25) as f32;
                let label = format!("{:02}", wheel_value(base + offset as f64, count));
                let run = self.text.layout(cx, 0.0, 0.0, None, false, Align::default(), &label);
                if !run.rows.is_empty() {
                    let x = digits_x - run.size_in_lpxs.width as f64 * 0.5;
                    // Centre the visible digits, including the font's cap-height
                    // correction. A font-size estimate leaves the ink below the band.
                    let y = r.center().y + angle.sin() * ROW / 0.52 - run.size_in_lpxs.height as f64 * 0.5 + run.ink_center_offset_in_lpxs() as f64;
                    self.text.draw_abs(cx, dvec2(x, y), &label);
                }
            }
            if n == 3 {
                let word = UNITS[dial];
                if let Some(run) = self.unit.prepare_single_line_run(cx, word) {
                    let ink = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
                    self.unit.draw_abs(cx, dvec2(digits_x + 24.0, r.center().y - ink * 0.5), word);
                }
            }
        }
        self.text.color = color;
        self.text.text_style.font_size = size;
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cyclic_wheels_snap_and_wrap_both_directions() {
        assert_eq!(wheel_value(-0.9, 24), 23);
        assert_eq!(wheel_value(24.2, 24), 0);
        assert_eq!(wheel_value(59.8, 60), 0);
        assert_eq!(wheel_value(7.49, 24), 7);
    }
}
