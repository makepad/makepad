use crate::alarm::AlarmSpec;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.AlarmList = set_type_default() do #(AlarmList::register_widget(vm)) {
        width: Fill height: Fill
        time_text +: {text_style: theme.font_regular{font_size: 34} color: theme.color_text}
        caption +: {text_style: theme.font_regular{font_size: 11} color: theme.color_text_disabled}
        background: theme.color_bg_container
        accent: theme.color_focus
        muted: theme.color_bevel_outset_2
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
pub enum AlarmListAction { Edit(u64), Toggle(u64), Remove(u64), #[default] None }
#[derive(Clone, Copy, PartialEq, Default)]
enum DragAxis { #[default] Pending, Horizontal, Vertical }
#[derive(Clone, Copy)]
struct Drag { start: Vec2d, scroll: f64, reveal: f64, row: Option<u64>, axis: DragAxis }

#[derive(Script, ScriptHook, Widget)]
pub struct AlarmList {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[redraw] #[rust] area: Area,
    #[live] shape: DrawColor,
    #[live] time_text: DrawText,
    #[live] caption: DrawText,
    #[live] background: Vec4f,
    #[live] accent: Vec4f,
    #[live] muted: Vec4f,
    #[rust] rows: Vec<AlarmSpec>,
    #[rust] scroll: f64,
    #[rust] reveal: f64,
    #[rust] revealed: Option<u64>,
    #[rust] drag: Option<Drag>,
}
const ROW: f64 = 106.0;
const DELETE: f64 = 76.0;
fn drag_axis(delta: Vec2d) -> DragAxis {
    if delta.x.abs().max(delta.y.abs()) < 8.0 { DragAxis::Pending }
    else if delta.x.abs() > delta.y.abs() * 1.2 { DragAxis::Horizontal }
    else { DragAxis::Vertical }
}
impl AlarmList {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<AlarmSpec>) {
        if rows != self.rows {
            self.rows = rows;
            if !self.rows.iter().any(|a| Some(a.id) == self.revealed) { self.revealed = None; self.reveal = 0.0; }
            self.clamp_scroll(cx); self.redraw(cx);
        }
    }
    fn clamp_scroll(&mut self, cx: &Cx) { self.scroll = self.scroll.clamp(0.0, (self.rows.len() as f64 * ROW - self.area.rect(cx).size.y).max(0.0)); }
    fn at(&self, cx: &Cx, p: Vec2d) -> Option<u64> {
        if !self.area.rect(cx).contains(p) { return None; }
        let row = ((p.y - self.area.rect(cx).pos.y + self.scroll) / ROW).floor() as usize;
        self.rows.get(row).map(|a| a.id)
    }
    fn rounded(&mut self, cx: &mut Cx2d, r: Rect, radius: f32, color: Vec4f) {
        self.shape.color = color; self.shape.draw_vars.set_dyn_instance(cx, live_id!(radius), &[radius]); self.shape.draw_abs(cx, r);
    }
}
impl Widget for AlarmList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(e) if e.device.is_primary_hit() => {
                let row = self.at(cx, e.abs);
                if row != self.revealed { self.revealed = None; self.reveal = 0.0; }
                self.drag = Some(Drag {start:e.abs, scroll:self.scroll, reveal:self.reveal, row, axis:DragAxis::Pending});
            }
            Hit::FingerMove(e) => if let Some(mut drag) = self.drag {
                let delta = e.abs - drag.start;
                if drag.axis == DragAxis::Pending { drag.axis = drag_axis(delta); }
                match drag.axis {
                    DragAxis::Horizontal => { self.revealed = drag.row; self.reveal = (drag.reveal - delta.x).clamp(0.0, DELETE); }
                    DragAxis::Vertical => { self.scroll = drag.scroll - delta.y; self.clamp_scroll(cx); }
                    _ => {}
                }
                self.drag = Some(drag); self.redraw(cx);
            },
            Hit::FingerUp(e) if e.is_primary_hit() => if let Some(drag) = self.drag.take() {
                if drag.axis == DragAxis::Horizontal {
                    self.reveal = if self.reveal > DELETE * 0.35 { DELETE } else { 0.0 };
                    if self.reveal == 0.0 { self.revealed = None; }
                } else if drag.axis == DragAxis::Pending && self.at(cx,e.abs) == drag.row {
                    if let Some(id) = drag.row {
                        let r = self.area.rect(cx);
                        let action = if self.revealed == Some(id) && e.abs.x > r.pos.x + r.size.x - DELETE {
                            AlarmListAction::Remove(id)
                        } else if self.reveal > 0.0 { self.revealed = None; self.reveal = 0.0; AlarmListAction::None }
                        else if e.abs.x > r.pos.x + r.size.x - 80.0 { AlarmListAction::Toggle(id) }
                        else { AlarmListAction::Edit(id) };
                        cx.widget_action(self.uid, action);
                    }
                }
                self.redraw(cx);
            },
            Hit::FingerScroll(e) => { self.scroll += e.scroll.y; self.clamp_scroll(cx); self.redraw(cx); }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => cx.set_cursor(MouseCursor::Hand),
            _ => {}
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout {clip_x:true, clip_y:true, ..Default::default()});
        let r = cx.turtle().rect();
        self.scroll = self.scroll.clamp(0.0, (self.rows.len() as f64 * ROW - r.size.y).max(0.0));
        if self.rows.is_empty() {
            self.time_text.text_style.font_size = 25.0;
            self.time_text.draw_abs(cx, r.pos + dvec2(20.0, 40.0), "No alarms yet");
            self.caption.draw_abs(cx, r.pos + dvec2(20.0, 85.0), "Add an alarm for your next early start.");
            self.time_text.text_style.font_size = 34.0;
        }
        let color = self.time_text.color;
        let from = (self.scroll / ROW).floor() as usize;
        let to = ((self.scroll + r.size.y) / ROW).ceil() as usize;
        for index in from..to.min(self.rows.len()) {
            let row = self.rows[index].clone();
            let y = r.pos.y + index as f64 * ROW - self.scroll;
            let offset = if self.revealed == Some(row.id) { self.reveal } else { 0.0 };
            if offset > 0.0 {
                self.rounded(cx, Rect {pos:dvec2(r.pos.x + r.size.x - DELETE,y),size:dvec2(DELETE,ROW-10.0)},12.0,vec4(0.88,0.12,0.17,1.0));
                let old = self.caption.color; self.caption.color = vec4(1.0,1.0,1.0,1.0);
                self.caption.draw_abs(cx,dvec2(r.pos.x+r.size.x-DELETE+16.0,y+38.0),"Delete"); self.caption.color=old;
            }
            let x = r.pos.x - offset;
            self.rounded(cx, Rect {pos:dvec2(x,y),size:dvec2(r.size.x,ROW-10.0)},14.0,self.background);
            self.time_text.color = if row.enabled {color} else {self.caption.color};
            self.time_text.draw_abs(cx, dvec2(x+18.0,y+9.0), &row.time());
            let title = if row.label.trim().is_empty() {"Alarm"} else {&row.label};
            self.caption.draw_abs(cx, dvec2(x+20.0,y+62.0), &format!("{} · Every day",title));
            let toggle = Rect {pos:dvec2(x+r.size.x-69.0,y+29.0),size:dvec2(51.0,31.0)};
            self.rounded(cx,toggle,15.5,if row.enabled {self.accent} else {self.muted});
            self.rounded(cx,Rect{pos:toggle.pos+dvec2(if row.enabled {23.0}else{3.0},3.0),size:dvec2(25.0,25.0)},12.5,vec4(1.0,1.0,1.0,1.0));
        }
        self.time_text.color = color;
        cx.end_turtle_with_area(&mut self.area); DrawStep::done()
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn scrolling_does_not_reveal_delete() {
        assert!(drag_axis(dvec2(4.0,30.0)) == DragAxis::Vertical);
        assert!(drag_axis(dvec2(-30.0,4.0)) == DragAxis::Horizontal);
        assert!(drag_axis(dvec2(-4.0,2.0)) == DragAxis::Pending);
    }
}
