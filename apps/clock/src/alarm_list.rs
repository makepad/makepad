//! The alarms as continuous 108 pt rows: the time large and light, the
//! label and repeat beneath, a hairline between rows, a green/grey toggle
//! at the trailing edge (its knob slides 220 ms, ease-out), a 76 pt Delete
//! reveal on a leftward swipe, and the empty state with its Add action.
//! Ids stay stable across edits; the view owns the book, this draws it.
use crate::alarm::AlarmSpec;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.AlarmList = set_type_default() do #(AlarmList::register_widget(vm)) {
        width: Fill height: Fill
        time_text +: {text_style: theme.font_regular{font_size: 42} color: theme.color_text}
        caption +: {text_style: theme.font_regular{font_size: 11.25} color: theme.color_text}
        empty_title +: {text_style: theme.font_bold{font_size: 16.5} color: theme.color_text}
        secondary: theme.color_text_disabled
        toggle_on: #34c759
        toggle_off: #d1d1d6
        delete: #c52f35
        hairline_alpha: 0.12
        shape +: {
            radius: instance(12.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.radius)
                sdf.fill(self.color)
                return sdf.result
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum AlarmListAction {
    Edit(u64),
    Toggle(u64),
    Remove(u64),
    Add,
    #[default]
    None,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum DragAxis {
    #[default]
    Pending,
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy)]
struct Drag {
    start: Vec2d,
    scroll: f64,
    reveal: f64,
    row: Option<u64>,
    axis: DragAxis,
}

#[derive(Script, ScriptHook, Widget)]
pub struct AlarmList {
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
    shape: DrawColor,
    #[live]
    time_text: DrawText,
    #[live]
    caption: DrawText,
    #[live]
    empty_title: DrawText,
    #[live]
    secondary: Vec4f,
    #[live]
    toggle_on: Vec4f,
    #[live]
    toggle_off: Vec4f,
    #[live]
    delete: Vec4f,
    #[live(0.12)]
    hairline_alpha: f32,
    #[rust]
    rows: Vec<AlarmSpec>,
    #[rust]
    scroll: f64,
    #[rust]
    reveal: f64,
    #[rust]
    revealed: Option<u64>,
    #[rust]
    drag: Option<Drag>,
    /// Each toggle's knob position, 0..1, animated toward its state.
    #[rust]
    knobs: Vec<(u64, f64)>,
    #[rust]
    next: NextFrame,
    #[rust]
    last: f64,
    /// The empty state's Add button rect, for hits.
    #[rust]
    add_rect: Option<Rect>,
    /// Footer text drawn under the rows (the runtime note).
    #[live]
    footer: String,
}

pub const ROW: f64 = 108.0;
const DELETE: f64 = 76.0;
const KNOB_SECS: f64 = 0.22;

fn drag_axis(delta: Vec2d) -> DragAxis {
    if delta.x.abs().max(delta.y.abs()) < 8.0 {
        DragAxis::Pending
    } else if delta.x.abs() > delta.y.abs() * 1.2 {
        DragAxis::Horizontal
    } else {
        DragAxis::Vertical
    }
}

impl AlarmList {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<AlarmSpec>) {
        if rows != self.rows {
            // A toggle that changed state animates from where it is; a new
            // row starts at its state.
            let mut animate = false;
            for row in &rows {
                let target = if row.enabled { 1.0 } else { 0.0 };
                match self.knobs.iter_mut().find(|(id, _)| *id == row.id) {
                    Some((_, pos)) => {
                        if (*pos - target).abs() > 0.001 {
                            animate = true;
                        }
                    }
                    None => self.knobs.push((row.id, target)),
                }
            }
            self.knobs.retain(|(id, _)| rows.iter().any(|r| r.id == *id));
            self.rows = rows;
            if !self.rows.iter().any(|a| Some(a.id) == self.revealed) {
                self.revealed = None;
                self.reveal = 0.0;
            }
            self.clamp_scroll(cx);
            if animate {
                self.last = 0.0;
                self.next = cx.new_next_frame();
            }
            self.redraw(cx);
        }
    }

    pub fn set_footer(&mut self, cx: &mut Cx, footer: &str) {
        if self.footer != footer {
            self.footer = footer.to_string();
            self.redraw(cx);
        }
    }

    fn content_height(&self) -> f64 {
        self.rows.len() as f64 * ROW + 40.0
    }

    fn clamp_scroll(&mut self, cx: &Cx) {
        self.scroll = self.scroll.clamp(0.0, (self.content_height() - self.area.rect(cx).size.y).max(0.0));
    }

    fn at(&self, cx: &Cx, p: Vec2d) -> Option<u64> {
        if !self.area.rect(cx).contains(p) {
            return None;
        }
        let row = ((p.y - self.area.rect(cx).pos.y + self.scroll) / ROW).floor();
        if row < 0.0 {
            return None;
        }
        self.rows.get(row as usize).map(|a| a.id)
    }

    fn rounded(&mut self, cx: &mut Cx2d, r: Rect, radius: f32, color: Vec4f) {
        self.shape.color = color;
        self.shape.draw_vars.set_dyn_instance(cx, live_id!(radius), &[radius]);
        self.shape.draw_abs(cx, r);
    }
}

impl Widget for AlarmList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(frame) = self.next.is_event(event) {
            let dt = if self.last == 0.0 { 1.0 / 60.0 } else { (frame.time - self.last).clamp(0.001, 0.05) };
            self.last = frame.time;
            let mut moving = false;
            for row in &self.rows {
                let target = if row.enabled { 1.0 } else { 0.0 };
                if let Some((_, pos)) = self.knobs.iter_mut().find(|(id, _)| *id == row.id) {
                    let step = dt / KNOB_SECS;
                    if (*pos - target).abs() > step {
                        *pos += step * (target - *pos).signum();
                        moving = true;
                    } else {
                        *pos = target;
                    }
                }
            }
            if moving {
                self.next = cx.new_next_frame();
            }
            self.redraw(cx);
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(e) if e.device.is_primary_hit() => {
                let row = self.at(cx, e.abs);
                if row != self.revealed {
                    self.revealed = None;
                    self.reveal = 0.0;
                }
                self.drag = Some(Drag { start: e.abs, scroll: self.scroll, reveal: self.reveal, row, axis: DragAxis::Pending });
            }
            Hit::FingerMove(e) => {
                if let Some(mut drag) = self.drag {
                    let delta = e.abs - drag.start;
                    if drag.axis == DragAxis::Pending {
                        drag.axis = drag_axis(delta);
                    }
                    match drag.axis {
                        DragAxis::Horizontal => {
                            if drag.row.is_some() {
                                self.revealed = drag.row;
                                self.reveal = (drag.reveal - delta.x).clamp(0.0, DELETE);
                            }
                        }
                        DragAxis::Vertical => {
                            self.scroll = drag.scroll - delta.y;
                            self.clamp_scroll(cx);
                        }
                        _ => {}
                    }
                    self.drag = Some(drag);
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(e) if e.is_primary_hit() => {
                if let Some(drag) = self.drag.take() {
                    if drag.axis == DragAxis::Horizontal {
                        self.reveal = if self.reveal > DELETE * 0.35 { DELETE } else { 0.0 };
                        if self.reveal == 0.0 {
                            self.revealed = None;
                        }
                    } else if drag.axis == DragAxis::Pending {
                        let r = self.area.rect(cx);
                        if self.rows.is_empty() {
                            if self.add_rect.is_some_and(|a| a.contains(e.abs)) {
                                cx.widget_action(self.uid, AlarmListAction::Add);
                            }
                        } else if self.at(cx, e.abs) == drag.row {
                            if let Some(id) = drag.row {
                                let row_top = r.pos.y + (e.abs.y - r.pos.y + self.scroll).div_euclid(ROW) * ROW - self.scroll;
                                let local = e.abs - dvec2(r.pos.x, row_top);
                                let action = if self.revealed == Some(id) && e.abs.x > r.pos.x + r.size.x - DELETE {
                                    AlarmListAction::Remove(id)
                                } else if self.reveal > 0.0 {
                                    self.revealed = None;
                                    self.reveal = 0.0;
                                    AlarmListAction::None
                                } else if local.x >= r.size.x - 100.0 && local.y >= 26.0 && local.y <= 82.0 {
                                    // The toggle's 60×56 hit box around its 51×31 track.
                                    AlarmListAction::Toggle(id)
                                } else {
                                    AlarmListAction::Edit(id)
                                };
                                cx.widget_action(self.uid, action);
                            }
                        }
                    }
                    self.redraw(cx);
                }
            }
            Hit::FingerScroll(e) => {
                self.scroll += e.scroll.y;
                self.clamp_scroll(cx);
                self.redraw(cx);
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => cx.set_cursor(MouseCursor::Hand),
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout { clip_x: true, clip_y: true, ..Default::default() });
        let r = cx.turtle().rect();
        self.scroll = self.scroll.clamp(0.0, (self.content_height() - r.size.y).max(0.0));
        let color = self.time_text.color;
        let caption_color = self.caption.color;
        if self.rows.is_empty() {
            // Centred around y 250: title, explanation, then a 144×48 Add.
            let cy = r.pos.y + 250.0 - 118.0;
            if let Some(run) = self.empty_title.prepare_single_line_run(cx, "No alarms") {
                self.empty_title.draw_abs(cx, dvec2(r.pos.x + (r.size.x - run.width_in_lpxs as f64) * 0.5, cy - 40.0), "No alarms");
            }
            self.caption.color = self.secondary;
            let line = "Add one and it rings while Clock is running.";
            if let Some(run) = self.caption.prepare_single_line_run(cx, line) {
                self.caption.draw_abs(cx, dvec2(r.pos.x + (r.size.x - run.width_in_lpxs as f64) * 0.5, cy - 6.0), line);
            }
            let add = Rect { pos: dvec2(r.pos.x + (r.size.x - 144.0) * 0.5, cy + 34.0), size: dvec2(144.0, 48.0) };
            self.rounded(cx, add, 12.0, self.toggle_on);
            self.caption.color = vec4(1.0, 1.0, 1.0, 1.0);
            if let Some(run) = self.caption.prepare_single_line_run(cx, "Add alarm") {
                let ink = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
                self.caption.draw_abs(cx, dvec2(add.pos.x + (add.size.x - run.width_in_lpxs as f64) * 0.5, add.pos.y + (add.size.y - ink) * 0.5), "Add alarm");
            }
            self.add_rect = Some(add);
            self.caption.color = caption_color;
        } else {
            self.add_rect = None;
        }
        let from = (self.scroll / ROW).floor() as usize;
        let to = ((self.scroll + r.size.y) / ROW).ceil() as usize;
        let rows = self.rows.clone();
        for index in from..to.min(rows.len()) {
            let row = &rows[index];
            let y = r.pos.y + index as f64 * ROW - self.scroll;
            let offset = if self.revealed == Some(row.id) { self.reveal } else { 0.0 };
            if offset > 0.0 {
                self.rounded(cx, Rect { pos: dvec2(r.pos.x + r.size.x - DELETE, y + 8.0), size: dvec2(DELETE, ROW - 16.0) }, 10.0, self.delete);
                self.caption.color = vec4(1.0, 1.0, 1.0, 1.0);
                if let Some(run) = self.caption.prepare_single_line_run(cx, "Delete") {
                    let ink = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
                    self.caption.draw_abs(cx, dvec2(r.pos.x + r.size.x - DELETE + (DELETE - run.width_in_lpxs as f64) * 0.5, y + (ROW - ink) * 0.5), "Delete");
                }
                self.caption.color = caption_color;
            }
            let x = r.pos.x - offset;
            // The time: 56/300, disabled in Secondary; the label/repeat beneath.
            self.time_text.color = if row.enabled { color } else { self.secondary };
            if let Some(run) = self.time_text.prepare_single_line_run(cx, &row.time()) {
                let ink = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
                self.time_text.draw_abs(cx, dvec2(x, y + 8.0 + (62.0 - ink) * 0.5), &row.time());
            }
            let title = if row.label.trim().is_empty() { "Alarm" } else { row.label.as_str() };
            self.caption.color = if row.enabled { caption_color } else { self.secondary };
            let line = format!("{} · Every day", title);
            if let Some(run) = self.caption.prepare_single_line_run(cx, &line) {
                let ink = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
                self.caption.draw_abs(cx, dvec2(x, y + 75.0 + (21.0 - ink) * 0.5), &line);
            }
            self.caption.color = caption_color;
            // The toggle: a 51×31 track at (w−51, 38), the 27 pt knob travelling 20.
            let knob_pos = self.knobs.iter().find(|(id, _)| *id == row.id).map(|(_, p)| *p).unwrap_or(if row.enabled { 1.0 } else { 0.0 });
            let track = Rect { pos: dvec2(x + r.size.x - 51.0, y + 38.0), size: dvec2(51.0, 31.0) };
            let track_color = self.toggle_off + (self.toggle_on - self.toggle_off) * knob_pos as f32;
            self.rounded(cx, track, 7.75, track_color);
            self.rounded(cx, Rect { pos: track.pos + dvec2(2.0 + 20.0 * knob_pos, 2.0), size: dvec2(27.0, 27.0) }, 6.75, vec4(1.0, 1.0, 1.0, 1.0));
            // The hairline under the row.
            let hair = vec4(color.x, color.y, color.z, self.hairline_alpha);
            self.rounded(cx, Rect { pos: dvec2(x, y + ROW - 0.5), size: dvec2(r.size.x, 0.5) }, 0.0, hair);
        }
        if !rows.is_empty() && !self.footer.is_empty() {
            let y = r.pos.y + rows.len() as f64 * ROW - self.scroll + 12.0;
            self.caption.color = self.secondary;
            let size = self.caption.text_style.font_size;
            self.caption.text_style.font_size = 9.75;
            let footer = self.footer.clone();
            self.caption.draw_abs(cx, dvec2(r.pos.x, y), &footer);
            self.caption.text_style.font_size = size;
            self.caption.color = caption_color;
        }
        self.time_text.color = color;
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scrolling_does_not_reveal_delete() {
        assert!(drag_axis(dvec2(4.0, 30.0)) == DragAxis::Vertical);
        assert!(drag_axis(dvec2(-30.0, 4.0)) == DragAxis::Horizontal);
        assert!(drag_axis(dvec2(-4.0, 2.0)) == DragAxis::Pending);
    }
}
