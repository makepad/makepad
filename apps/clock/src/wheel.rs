//! A pair of cyclic, inertial time wheels. The selected value remains resident
//! across layout changes and theme reapplication.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.TimeWheel = set_type_default() do #(TimeWheel::register_widget(vm)) {
        width: Fill height: 220
        text +: {text_style: theme.font_regular{font_size: 30} color: theme.color_text}
        band +: {color: theme.color_bg_highlight}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct TimeWheel {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[redraw] #[rust] area: Area,
    #[live] text: DrawText,
    #[live] band: DrawColor,
    #[rust] positions: [f64; 2],
    #[rust] velocity: f64,
    #[rust] active: usize,
    #[rust] drag: Option<(f64, f64, f64)>,
    #[rust] next: NextFrame,
    #[rust] last: f64,
}
impl TimeWheel {
    pub fn set_time(&mut self, cx: &mut Cx, hour: u32, minute: u32) {
        self.positions = [hour as f64, minute as f64]; self.velocity = 0.0; self.drag = None; self.redraw(cx);
    }
    pub fn time(&self) -> (u32, u32) { (wheel_value(self.positions[0], 24), wheel_value(self.positions[1], 60)) }
    fn animate(&mut self, cx: &mut Cx) { self.next = cx.new_next_frame(); self.redraw(cx); }
    fn row_height(&self, cx: &Cx) -> f64 { (self.area.rect(cx).size.y / 5.0).clamp(32.0, 46.0) }
}
fn wheel_value(position: f64, count: i64) -> u32 { (position.round() as i64).rem_euclid(count) as u32 }
impl Widget for TimeWheel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(frame) = self.next.is_event(event) {
            let dt = if self.last == 0.0 {1.0 / 60.0} else {(frame.time - self.last).clamp(0.001, 0.04)};
            self.last = frame.time;
            if self.drag.is_none() {
                let p = &mut self.positions[self.active];
                if self.velocity.abs() > 0.6 {
                    *p += self.velocity * dt; self.velocity *= (-dt * 8.0).exp();
                } else {
                    self.velocity = 0.0;
                    *p += (p.round() - *p) * (1.0 - (-dt * 22.0).exp());
                }
                if self.velocity != 0.0 || (*p - p.round()).abs() > 0.002 { self.animate(cx); }
                else { *p = p.round(); self.redraw(cx); }
            }
        }
        let row = self.row_height(cx);
        match event.hits(cx, self.area) {
            Hit::FingerDown(e) if e.is_primary_hit() => {
                self.active = usize::from(e.abs.x > self.area.rect(cx).center().x);
                self.velocity = 0.0; self.last = 0.0; self.drag = Some((e.abs.y, self.positions[self.active], e.time));
                cx.set_key_focus(self.area);
            }
            Hit::FingerMove(e) => {
                if let Some((y, start, time)) = self.drag {
                    let previous = self.positions[self.active];
                    let current = start - (e.abs.y - y) / row;
                    let dt = (e.time - time).max(0.008);
                    self.velocity = ((current - previous) / dt).clamp(-28.0, 28.0);
                    self.positions[self.active] = current;
                    self.drag = Some((e.abs.y, current, e.time)); self.redraw(cx);
                }
            }
            Hit::FingerUp(e) if e.is_primary_hit() => { self.drag = None; self.animate(cx); }
            Hit::FingerScroll(e) => {
                self.active = usize::from(e.abs.x > self.area.rect(cx).center().x);
                self.positions[self.active] += e.scroll.y / row;
                self.velocity = 0.0; self.animate(cx);
            }
            Hit::KeyDown(e) => match e.key_code {
                KeyCode::ArrowUp => {self.positions[self.active] -= 1.0; self.redraw(cx);}
                KeyCode::ArrowDown => {self.positions[self.active] += 1.0; self.redraw(cx);}
                KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::Tab => {self.active = 1 - self.active;}
                _ => {}
            },
            _ => {}
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        let row = (r.size.y / 5.0).clamp(32.0, 46.0);
        self.band.draw_abs(cx, Rect {pos: dvec2(r.pos.x, r.center().y - row * 0.5), size: dvec2(r.size.x, row)});
        let color = self.text.color;
        let size = self.text.text_style.font_size;
        for dial in 0..2 {
            let p = self.positions[dial];
            let base = p.floor();
            for offset in -3..=3 {
                let distance = base + offset as f64 - p;
                let angle = distance * 0.52;
                if angle.abs() > 1.5 { continue; }
                let depth = angle.cos();
                self.text.text_style.font_size = size * (0.7 + depth * 0.3) as f32;
                self.text.color = color * (depth * depth * 0.75 + 0.25) as f32;
                let label = format!("{:02}", wheel_value(base + offset as f64, if dial == 0 {24} else {60}));
                let run = self.text.layout(cx, 0.0, 0.0, None, false, Align::default(), &label);
                if !run.rows.is_empty() {
                    let x = r.pos.x + r.size.x * (0.25 + dial as f64 * 0.5) - run.size_in_lpxs.width as f64 * 0.5;
                    // Center the visible digits, including the font's cap-height
                    // correction. A font-size estimate leaves the ink below the band.
                    let y = r.center().y + angle.sin() * row / 0.52
                        - run.size_in_lpxs.height as f64 * 0.5 + run.ink_center_offset_in_lpxs() as f64;
                    self.text.draw_abs(cx, dvec2(x, y), &label);
                }
            }
        }
        self.text.color = color; self.text.text_style.font_size = size;
        cx.end_turtle_with_area(&mut self.area); DrawStep::done()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn cyclic_wheels_snap_and_wrap_both_directions() {
        assert_eq!(wheel_value(-0.9, 24), 23); assert_eq!(wheel_value(24.2, 24), 0);
        assert_eq!(wheel_value(59.8, 60), 0); assert_eq!(wheel_value(7.49, 24), 7);
    }
}
