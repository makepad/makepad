//! The floating navigation capsule's four targets: an icon over a label in
//! four equal cells, the selected cell's capsule sliding under them
//! (260 ms, ease-out) while the labels stay put. The glass surface is a
//! sibling drawn under this widget; this only paints crisp foreground.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.PhoneTabsBase = #(PhoneTabs::register_widget(vm))
    mod.widgets.PhoneTabs = set_type_default() do mod.widgets.PhoneTabsBase {
        width: 370 height: 64
        label +: {text_style: theme.font_bold{font_size: 8.25} color: theme.color_text}
        capsule +: {
            color: theme.color_inset
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 14.0)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        accent: #c86400
        secondary: theme.color_text_disabled
        capsule_alpha: 0.35
        icon_clock +: {svg: crate_resource("self:resources/icons/clock.svg") color: theme.color_text_disabled}
        icon_alarm +: {svg: crate_resource("self:resources/icons/alarm.svg") color: theme.color_text_disabled}
        icon_stopwatch +: {svg: crate_resource("self:resources/icons/stopwatch.svg") color: theme.color_text_disabled}
        icon_timer +: {svg: crate_resource("self:resources/icons/timer.svg") color: theme.color_text_disabled}
        on_clock +: {svg: crate_resource("self:resources/icons/clock.svg") color: #c86400}
        on_alarm +: {svg: crate_resource("self:resources/icons/alarm.svg") color: #c86400}
        on_stopwatch +: {svg: crate_resource("self:resources/icons/stopwatch.svg") color: #c86400}
        on_timer +: {svg: crate_resource("self:resources/icons/timer.svg") color: #c86400}
    }
}

#[derive(Clone, Debug, Default)]
pub enum PhoneTabsAction {
    Selected(usize),
    #[default]
    None,
}

const LABELS: [&str; 4] = ["Clock", "Alarms", "Stopwatch", "Timer"];
const SLIDE_SECS: f64 = 0.26;

#[derive(Script, ScriptHook, Widget)]
pub struct PhoneTabs {
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
    label: DrawText,
    #[live]
    capsule: DrawColor,
    #[live]
    icon_clock: DrawSvg,
    #[live]
    icon_alarm: DrawSvg,
    #[live]
    icon_stopwatch: DrawSvg,
    #[live]
    icon_timer: DrawSvg,
    #[live]
    on_clock: DrawSvg,
    #[live]
    on_alarm: DrawSvg,
    #[live]
    on_stopwatch: DrawSvg,
    #[live]
    on_timer: DrawSvg,
    #[live]
    accent: Vec4f,
    #[live]
    secondary: Vec4f,
    #[live(0.35)]
    capsule_alpha: f32,
    #[rust]
    active: usize,
    /// The capsule's animated position in cells, and where it is going.
    #[rust]
    capsule_pos: f64,
    #[rust]
    slide_from: f64,
    #[rust]
    slide_start: f64,
    #[rust]
    next: NextFrame,
    #[rust]
    pressed: Option<usize>,
}

fn ease_out(t: f64) -> f64 {
    // cubic-bezier(0.22, 1, 0.36, 1), close enough as 1 - (1-t)^3.
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

impl PhoneTabs {
    pub fn active(&self) -> usize {
        self.active
    }

    pub fn set_active(&mut self, cx: &mut Cx, index: usize, animate: bool) {
        let index = index.min(3);
        if index == self.active && self.capsule_pos == index as f64 {
            return;
        }
        self.active = index;
        if animate {
            self.slide_from = self.capsule_pos;
            self.slide_start = Cx::monotonic_now();
            self.next = cx.new_next_frame();
        } else {
            self.capsule_pos = index as f64;
        }
        self.redraw(cx);
    }

    fn cell_at(&self, cx: &Cx, p: Vec2d) -> Option<usize> {
        let r = self.area.rect(cx);
        if !r.contains(p) {
            return None;
        }
        Some((((p.x - r.pos.x) / r.size.x) * 4.0).floor().clamp(0.0, 3.0) as usize)
    }
}

impl Widget for PhoneTabs {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(_frame) = self.next.is_event(event) {
            let t = ((Cx::monotonic_now() - self.slide_start) / SLIDE_SECS).clamp(0.0, 1.0);
            self.capsule_pos = self.slide_from + (self.active as f64 - self.slide_from) * ease_out(t);
            if t < 1.0 {
                self.next = cx.new_next_frame();
            } else {
                self.capsule_pos = self.active as f64;
            }
            self.redraw(cx);
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(e) if e.is_primary_hit() => {
                self.pressed = self.cell_at(cx, e.abs);
            }
            Hit::FingerUp(e) if e.is_primary_hit() => {
                if let (Some(pressed), Some(up)) = (self.pressed.take(), self.cell_at(cx, e.abs)) {
                    if pressed == up {
                        self.set_active(cx, up, true);
                        cx.widget_action(self.uid, PhoneTabsAction::Selected(up));
                    }
                }
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => cx.set_cursor(MouseCursor::Hand),
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        // Four equal cells inside 4 pt padding; the capsule fills the cell.
        let inner = Rect { pos: r.pos + dvec2(4.0, 4.0), size: r.size - dvec2(8.0, 8.0) };
        let cell_w = inner.size.x / 4.0;
        let base = self.capsule.color;
        self.capsule.color = vec4(base.x, base.y, base.z, self.capsule_alpha);
        self.capsule.draw_abs(cx, Rect { pos: dvec2(inner.pos.x + cell_w * self.capsule_pos, inner.pos.y), size: dvec2(cell_w, inner.size.y) });
        self.capsule.color = base;
        for i in 0..4 {
            let cell = Rect { pos: dvec2(inner.pos.x + cell_w * i as f64, inner.pos.y), size: dvec2(cell_w, inner.size.y) };
            let ink = if i == self.active { self.accent } else { self.secondary };
            // Two resident tints per glyph: the SVG's colour is bound at
            // evaluation, so the active one is a sibling, not a re-tint.
            let icon = match (i, i == self.active) {
                (0, false) => &mut self.icon_clock,
                (1, false) => &mut self.icon_alarm,
                (2, false) => &mut self.icon_stopwatch,
                (_, false) => &mut self.icon_timer,
                (0, true) => &mut self.on_clock,
                (1, true) => &mut self.on_alarm,
                (2, true) => &mut self.on_stopwatch,
                (_, true) => &mut self.on_timer,
            };
            // Icon 24 above the 11 pt label, 3 apart, the pair centred.
            let stack_h = 24.0 + 3.0 + 13.0;
            let top = cell.pos.y + (cell.size.y - stack_h) * 0.5;
            icon.draw_abs(cx, Rect { pos: dvec2(cell.pos.x + (cell_w - 24.0) * 0.5, top), size: dvec2(24.0, 24.0) });
            self.label.color = ink;
            if let Some(run) = self.label.prepare_single_line_run(cx, LABELS[i]) {
                let ink_h = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
                let x = cell.pos.x + (cell_w - run.width_in_lpxs as f64) * 0.5;
                self.label.draw_abs(cx, dvec2(x, top + 27.0 + (13.0 - ink_h) * 0.5), LABELS[i]);
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
