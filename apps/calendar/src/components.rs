//! Quiet controls and row-owned hit regions shared by Calendar presentations.
use crate::model::*;
use crate::presentation::*;
use crate::presentation::rect;
use makepad_civil_time::Day;
use makepad_widgets::animator::Ease;
use makepad_widgets::makepad_platform::event::TouchState;
use makepad_widgets::*;
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.shader.*
    use mod.calendar.*

    mod.calendar.Plain = View{
        width: Fill height: Fill show_bg: false
        padding: 0 margin: 0 spacing: 0
    }
    mod.calendar.Ink = Label{
        padding: 0 margin: 0
        flow: Right max_lines: 1 text_overflow: Ellipsis
        draw_text +: {color: ink text_style: theme.font_regular{font_size: 9.75} font_scale: 1.0}
    }
    mod.calendar.QuietAction = ButtonFlat{
        width: 44 height: 44 text: "" padding: 0 margin: 0
        draw_bg +: {pixel: fn(){return vec4(0.0,0.0,0.0,0.0)}}
        draw_text +: {
            color: ink color_hover: ink color_down: ink
            text_style: theme.font_regular{font_size: 9.75}
        }
        animator +: {hover: {default:@off
            off: AnimatorState{ease: Ease.OutCubic from: {all: Forward{duration: 0.12}}}
            on: AnimatorState{ease: Ease.OutCubic from: {all: Forward{duration: 0.1} down: Forward{duration: 0.12}}
                apply: {draw_bg: {down: 0.0 hover: 1.0} draw_text: {down: 0.0 hover: 1.0}}}
            down: AnimatorState{ease: Ease.OutCubic from: {all: Forward{duration: 0.07}}
                apply: {draw_bg: {down: 1.0 hover: 1.0} draw_text: {down: 1.0 hover: 1.0}}}
        }}
    }
    mod.calendar.QuietField = TextInput{
        width: Fill height: 44 padding: 0 margin: 0
        draw_bg +: {pixel: fn(){return vec4(0.0,0.0,0.0,0.0)}}
        draw_text +: {color: ink text_style: theme.font_regular{font_size: 12.75}}
    }
    mod.calendar.QuietChoice = DropDown{
        width: Fill height: 44 padding: Inset{left: 0 right: 16} margin: 0
        draw_bg +: {pixel: fn(){return vec4(0.0,0.0,0.0,0.0)}}
        draw_text +: {color: ink text_style: theme.font_regular{font_size: 12.75}}
    }
    mod.widgets.CalendarEditorScrollBase = #(CalendarEditorScroll::register_widget(vm))
    mod.widgets.CalendarEditorScroll = set_type_default() do mod.widgets.CalendarEditorScrollBase{
        width:Fill height:Fill flow:Down padding:16 spacing:16 show_bg:false
        scrolling:ScrollBars{show_scroll_x:false show_scroll_y:true}
    }
    mod.widgets.CalendarControlGroupBase = #(CalendarControlGroup::register_widget(vm))
    mod.widgets.CalendarControlGroup = set_type_default() do mod.widgets.CalendarControlGroupBase{
        width:Fill height:Fill show_bg:false padding:0 margin:0 spacing:0
        draw_wash.color:ink
    }
    mod.widgets.CalendarHitRowBase = #(CalendarHitRow::register_widget(vm))
    mod.widgets.CalendarHitRow = set_type_default() do mod.widgets.CalendarHitRowBase{
        width: Fill height: 44 show_bg: false padding: 0 margin: 0 spacing: 0
        draw_wash +: {color: ink}
    }
    mod.widgets.AppRuleBase = #(AppRule::register_widget(vm))
    mod.widgets.AppRule = set_type_default() do mod.widgets.AppRuleBase{
        width: Fill height: 0.5 draw_line.color: rule
    }
    mod.widgets.CalendarStripe = SolidView{width: 2 height: 40 margin: 0 padding: 0 draw_bg.color: action}
    mod.widgets.CalendarDateMarkBase = #(CalendarDateMark::register_widget(vm))
    mod.widgets.CalendarDateMark = set_type_default() do mod.widgets.CalendarDateMarkBase{
        width: 26 height: 26
        draw_mark +: {
            color: selection
            focus_color: uniform(focus)
            focus: instance(0.0)
            checked: instance(0.0)
            ring: instance(0.0)
            check_color: instance(ink)
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.rect_size.x * 0.5 - 1.0)
                sdf.fill(self.color * (1.0 - self.ring))
                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.rect_size.x * 0.5 - 1.0)
                sdf.stroke(self.color, self.ring)
                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.rect_size.x * 0.5 - 0.5)
                sdf.stroke(self.focus_color, self.focus)
                sdf.move_to(self.rect_size.x * 0.25, self.rect_size.y * 0.5)
                sdf.line_to(self.rect_size.x * 0.44, self.rect_size.y * 0.68)
                sdf.line_to(self.rect_size.x * 0.75, self.rect_size.y * 0.32)
                sdf.stroke(self.check_color, self.checked)
                return sdf.result
            }
        }
    }
    mod.widgets.CalendarDayPresentation = mod.widgets.CalendarHitRow{
        width: Fill height: Fill flow: Overlay
        date_mark := mod.widgets.CalendarDateMark{}
        date_number := mod.calendar.Ink{width: 26 height: 26 align: Align{x: 0.5 y: 0.5}}
        events := mod.calendar.Plain{flow: Overlay}
        overflow := mod.widgets.CalendarHitRow{visible: false height: 18
            label := mod.calendar.Ink{width: Fill height: Fill draw_text +: {color: secondary text_style: theme.font_regular{font_size: 9.0}}}
        }
    }
    mod.widgets.CalendarMiniDate = mod.widgets.CalendarHitRow{
        width: 28 height: 24 flow: Overlay
        selection_mark := mod.widgets.CalendarDateMark{width: 22 height: 22}
        numeral := mod.calendar.Ink{width: Fill height: Fill align: Align{x: 0.5 y: 0.5}}
    }
    mod.widgets.CalendarEventPresentation = mod.widgets.CalendarHitRow{
        height: 18 flow: Right spacing: 4 align: Align{y: 0.5}
        category_mark := RoundedView{width:4 height:4 padding:0 margin:0 draw_bg +: {color:action border_radius:1.0}}
        event_title := mod.calendar.Ink{width: Fill height: Fill draw_text.text_style: theme.font_regular{font_size: 9.0}}
        event_time := mod.calendar.Ink{width: 34 height: Fill align: Align{x: 1.0 y: 0.5} draw_text +: {color: secondary text_style: theme.font_regular{font_size: 8.25}}}
    }
    mod.widgets.CalendarRibbon = mod.widgets.CalendarHitRow{
        height: 18 flow: Right spacing: 6 show_bg: true
        draw_bg +: {
            color: instance(selection)
            radius: instance(1.5)
            continuation_left:instance(0.0) continuation_right:instance(0.0)
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(-6.0*self.continuation_left,0.0,self.rect_size.x+6.0*(self.continuation_left+self.continuation_right),self.rect_size.y,self.radius)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        category_mark := mod.widgets.CalendarStripe{width: 2 height: Fill}
        event_title := mod.calendar.Ink{width: Fill height: Fill draw_text.text_style: theme.font_regular{font_size: 9.0}}
        event_time := mod.calendar.Ink{visible: false}
    }
    mod.widgets.EventRow = mod.widgets.CalendarHitRow{
        width: Fill height: 72 flow: Down show_bg: false
        content := mod.calendar.Plain{
            flow: Right padding: Inset{left: 20 right: 20 top: 12 bottom: 12}
            times := mod.calendar.Plain{width: 56 flow: Down spacing: 4
                starts := mod.calendar.Ink{width: Fill height: 20 draw_text.text_style: theme.font_regular{font_size: 9.75}}
                ends := mod.calendar.Ink{width: Fill height: 18 draw_text +: {color: secondary text_style: theme.font_regular{font_size: 9.75}}}
            }
            time_gap := mod.calendar.Plain{width: 12}
            category_stripe := mod.widgets.CalendarStripe{width: 2 height: 40}
            title_gap := mod.calendar.Plain{width: 10}
            text := mod.calendar.Plain{flow: Down spacing: 4
                title := mod.calendar.Ink{width: Fill height: 24 draw_text.text_style: theme.font_bold{font_size: 12.75}}
                calendar_name := mod.calendar.Ink{width: Fill height: 18 draw_text.color: secondary}
            }
        }
        separator := mod.widgets.AppRule{height: 0.5 margin: Inset{left: 100 right: 20}}
    }
    mod.widgets.CalendarRows = mod.calendar.Plain{
        height: Fit flow: Down
        cal0 := mod.widgets.CalendarHitRow{height: 36 calendar_choice:true flow:Overlay
            indicator := mod.widgets.CalendarDateMark{width: 12 height: 12}
            calendar_name := mod.calendar.Ink{width: Fill height: Fill align: Align{x:0.0 y:0.5}}
        }
        cal1 := mod.widgets.CalendarHitRow{height: 36 calendar_choice:true flow:Overlay
            indicator := mod.widgets.CalendarDateMark{width: 12 height: 12}
            calendar_name := mod.calendar.Ink{width: Fill height: Fill align: Align{x:0.0 y:0.5}}
        }
        cal2 := mod.widgets.CalendarHitRow{height: 36 calendar_choice:true flow:Overlay
            indicator := mod.widgets.CalendarDateMark{width: 12 height: 12}
            calendar_name := mod.calendar.Ink{width: Fill height: Fill align: Align{x:0.0 y:0.5}}
        }
        cal3 := mod.widgets.CalendarHitRow{height: 36 calendar_choice:true flow:Overlay
            indicator := mod.widgets.CalendarDateMark{width: 12 height: 12}
            calendar_name := mod.calendar.Ink{width: Fill height: Fill align: Align{x:0.0 y:0.5}}
        }
    }
    mod.widgets.AppStorageStatus = mod.calendar.Plain{
        height: 28 flow: Right align: Align{y: 0.5} padding: Inset{left: 12 right: 12}
        status := mod.calendar.Ink{width: Fill draw_text +: {color: secondary text_style: theme.font_regular{font_size: 8.25}}}
        retry := mod.calendar.QuietAction{width: 52 visible: false text: "Retry"}
    }
}

fn calendar_row_rects(row: Rect) -> (Rect, Rect) {
    (rect(row.pos.x + 16.0, row.pos.y + (row.size.y - 12.0) * 0.5, 12.0, 12.0),
     rect(row.pos.x + 36.0, row.pos.y, row.size.x - 52.0, row.size.y))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CalendarAction {
    #[default]
    None,
    SelectDay(Day),
    OpenDay(Day),
    Event(OccurrenceKey),
    Agenda(Day),
    Calendar(usize),
    Period(i32),
    Create {
        day: Day,
        minute: u16,
    },
    Add,
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarHitRow {
    #[deref]
    pub view: View,
    #[live]
    draw_wash: DrawColor,
    #[rust]
    pub target: CalendarAction,
    #[live(false)]
    calendar_choice: bool,
    #[rust]
    hover: f64,
    #[rust]
    pressed: bool,
    #[rust]
    hover_target: f64,
    #[rust]
    motion: NextFrame,
    #[rust]
    last_time: f64,
    #[rust]
    duration: f64,
    #[rust]
    from: f64,
    #[rust]
    elapsed: f64,
    #[rust]
    pub selected: bool,
    #[live(false)]
    pub reduced_motion: bool,
}
impl CalendarHitRow {
    fn animate(&mut self, cx: &mut Cx, target: f64, duration: f64) {
        self.from = self.hover;
        self.hover_target = target;
        self.duration = if self.reduced_motion {
            duration.min(0.08)
        } else {
            duration
        };
        self.elapsed = 0.0;
        self.last_time = 0.0;
        self.motion = cx.new_next_frame();
        self.view.redraw(cx);
    }
}
impl Widget for CalendarHitRow {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && event.requires_visibility() {
            return;
        }
        self.view.handle_event(cx, event, scope);
        if let Some(e) = self.motion.is_event(event) {
            if self.last_time != 0.0 {
                self.elapsed += (e.time - self.last_time).max(0.0);
            }
            self.last_time = e.time;
            let p = (self.elapsed / self.duration.max(0.001)).min(1.0);
            self.hover = self.from + (self.hover_target - self.from) * Ease::OutCubic.map(p);
            if p < 1.0 {
                self.motion = cx.new_next_frame();
            }
            self.view.redraw(cx);
        }
        if self.target == CalendarAction::None {
            return;
        }
        match event.hits(cx, self.view.area()) {
            Hit::FingerDown(_) => {
                cx.set_key_focus(self.view.area());
                self.pressed = true;
                self.animate(cx, 1.0, 0.07);
            }
            Hit::FingerUp(e) => {
                self.pressed = false;
                self.animate(cx, 0.0, 0.12);
                if e.is_primary_hit() && e.was_tap() {
                    let target = match self.target {
                        CalendarAction::SelectDay(d) if e.tap_count >= 2 => {
                            CalendarAction::OpenDay(d)
                        }
                        other => other,
                    };
                    cx.widget_action(self.widget_uid(), target);
                }
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Hand);
                if self.hover_target == 0.0 {
                    self.animate(cx, 0.5, 0.1);
                }
            }
            Hit::FingerHoverOut(_) => self.animate(cx, 0.0, 0.1),
            Hit::KeyDown(e) if matches!(e.key_code, KeyCode::ReturnKey | KeyCode::Space) => {
                cx.widget_action(self.widget_uid(), self.target);
            }
            _ => {}
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.calendar_choice {
            let (indicator, mut name) = calendar_row_rects(cx.peek_walk_turtle(walk));
            let label = self.view.widget(cx, ids!(calendar_name));
            if let Some(label) = label.borrow::<Label>() {
                // DrawText aligns horizontally; center the measured line in this row.
                let text = label.text();
                let line = label.draw_text.layout(
                    cx, 0.0, 0.0, Some(name.size.x as f32), false, Align::default(), &text,
                );
                let height = line.size_in_lpxs.height as f64 * label.draw_text.font_scale as f64;
                name.pos.y += (name.size.y - height) * 0.5;
                name.size.y = height;
            }
            place(&self.view.widget(cx, ids!(indicator)), cx, indicator);
            place(&label, cx, name);
        }
        self.view.draw_walk(cx, scope, walk)?;
        if self.visible && (self.hover > 0.0 || self.selected) {
            let c = cx.with_vm(CalendarColors::resolve);
            self.draw_wash.color = Vec4f {
                w: (self.hover as f32 * 0.06).max(if self.selected { 0.06 } else { 0.0 }),
                ..c.ink
            };
            self.draw_wash.draw_abs(cx, self.view.area().rect(cx));
        }
        DrawStep::done()
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct AppRule {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    draw_line: DrawColor,
    #[live(false)]
    vertical: bool,
    #[live(1.0)]
    strength: f32,
    #[redraw]
    #[rust]
    area: Area,
}
impl Widget for AppRule {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let c = cx.with_vm(CalendarColors::resolve);
        cx.begin_turtle(walk, Layout::default());
        let mut r = cx.turtle().rect();
        self.draw_line.color = mix(c.paper, c.rule, self.strength);
        if self.vertical {
            r.size.x = hairline(cx);
        } else {
            r.size.y = hairline(cx);
        }
        self.draw_line.draw_abs(cx, r);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarDateMark {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    pub draw_mark: DrawColor,
    #[redraw]
    #[rust]
    area: Area,
    #[rust]
    color: Vec4f,
    #[rust]
    from: Vec4f,
    #[rust]
    target: Vec4f,
    #[rust]
    progress: f64,
    #[rust]
    last: f64,
    #[rust]
    next: NextFrame,
    #[rust]
    initialized: bool,
    #[rust]
    pub focused: bool,
    #[rust]
    checked: bool,
    #[rust]
    ring: bool,
    #[live(false)]
    pub reduced_motion: bool,
}
impl CalendarDateMark {
    pub fn set_checked(&mut self, cx: &mut Cx, checked: bool, color: Vec4f) {
        self.checked = checked;
        self.ring = !checked;
        self.set_color(cx, color);
    }
    pub fn set_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if !self.initialized {
            self.color = color;
            self.initialized = true;
        }
        if self.target != color {
            self.from = self.color;
            self.target = color;
            self.progress = 0.0;
            self.last = 0.0;
            self.next = cx.new_next_frame();
            self.redraw(cx);
        }
    }
}
impl Widget for CalendarDateMark {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(e) = self.next.is_event(event) {
            if self.last != 0.0 {
                self.progress = (self.progress
                    + (e.time - self.last) / if self.reduced_motion { 0.08 } else { 0.12 })
                .min(1.0);
            }
            self.last = e.time;
            self.color = mix(
                self.from,
                self.target,
                Ease::OutCubic.map(self.progress) as f32,
            );
            if self.progress < 1.0 {
                self.next = cx.new_next_frame();
            }
            self.redraw(cx);
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        self.draw_mark.color = self.color;
        let c = cx.with_vm(CalendarColors::resolve);
        let check = if contrast(c.paper, self.color) > contrast(c.ink, self.color) {
            c.paper
        } else {
            c.ink
        };
        self.draw_mark.draw_vars.set_dyn_instance(
            cx,
            live_id!(checked),
            &[if self.checked { 1.0 } else { 0.0 }],
        );
        self.draw_mark.draw_vars.set_dyn_instance(
            cx,
            live_id!(ring),
            &[if self.ring { 1.0 } else { 0.0 }],
        );
        self.draw_mark.draw_vars.set_dyn_instance(
            cx,
            live_id!(focus),
            &[if self.focused { 1.0 } else { 0.0 }],
        );
        self.draw_mark.draw_vars.set_dyn_instance(
            cx,
            live_id!(check_color),
            &[check.x, check.y, check.z, check.w],
        );
        self.draw_mark.draw_abs(cx, cx.turtle().rect());
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
pub fn target(widget: &WidgetRef, action: CalendarAction) {
    if let Some(mut row) = widget.borrow_mut::<CalendarHitRow>() {
        row.target = action;
    }
}
pub fn place(widget: &WidgetRef, cx: &mut Cx, r: Rect) {
    let mut widget = widget.clone();
    script_apply_eval!(cx, widget, {width: #(r.size.x) height: #(r.size.y) abs_pos: #(r.pos)});
}
pub fn add_child(view: &mut View, cx: &mut Cx, id: LiveId, child: WidgetRef) {
    view.children.push((id, child.clone()));
    cx.widget_tree_insert_child_deep(view.widget_uid(), id, child);
}
pub fn label(widget: &WidgetRef, cx: &mut Cx, id: &[LiveId], text: &str, color: Vec4f) {
    widget.label(cx, id).set_text(cx, text);
    widget.label(cx, id).set_text_color(cx, color);
}

#[derive(Default)]
struct ControlWash {
    value: f64,
    from: f64,
    target: f64,
    elapsed: f64,
    duration: f64,
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarControlGroup {
    #[deref]
    view: View,
    #[live]
    draw_wash: DrawColor,
    #[rust]
    controls: HashMap<WidgetUid, ControlWash>,
    #[rust]
    next: NextFrame,
    #[rust]
    last: f64,
    #[live(false)]
    pub reduced_motion: bool,
}
impl CalendarControlGroup {
    fn controls(&self) -> Vec<WidgetRef> {
        let mut nodes: Vec<_> = self.view.children.iter().map(|(_, w)| w.clone()).collect();
        let mut i = 0;
        while i < nodes.len() {
            let w = nodes[i].clone();
            w.try_children(&mut |_, child| nodes.push(child));
            i += 1;
        }
        nodes
            .into_iter()
            .filter(|w| {
                w.borrow::<Button>().is_some()
                    || w.borrow::<TextInput>().is_some()
                    || w.borrow::<DropDown>().is_some()
            })
            .collect()
    }
}
impl Widget for CalendarControlGroup {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Some(e) = self.next.is_event(event) {
            let dt = if self.last == 0.0 {
                0.0
            } else {
                e.time - self.last
            };
            self.last = e.time;
            let mut moving = false;
            for wash in self.controls.values_mut() {
                wash.elapsed += dt;
                let p = (wash.elapsed / wash.duration.max(0.001)).min(1.0);
                wash.value = wash.from + (wash.target - wash.from) * Ease::OutCubic.map(p);
                moving |= p < 1.0;
            }
            if moving {
                self.next = cx.new_next_frame();
            }
            self.view.redraw(cx);
        }
        let pointer = match event {
            Event::MouseMove(e) => Some((e.abs, 0.5, 0.1)),
            Event::MouseDown(e) => Some((e.abs, 1.0, 0.07)),
            Event::MouseUp(e) => Some((e.abs, 0.5, 0.12)),
            Event::TouchUpdate(e) => e.touches.first().map(|t| {
                (
                    t.abs,
                    if t.state == TouchState::Stop {
                        0.0
                    } else {
                        1.0
                    },
                    if t.state == TouchState::Stop {
                        0.12
                    } else {
                        0.07
                    },
                )
            }),
            _ => None,
        };
        if let Some((point, amount, duration)) = pointer {
            for widget in self.controls() {
                let area = widget.area().rect(cx);
                let target = if area.contains(point) { amount } else { 0.0 };
                let wash = self.controls.entry(widget.widget_uid()).or_default();
                if wash.target != target {
                    wash.from = wash.value;
                    wash.target = target;
                    wash.elapsed = 0.0;
                    wash.duration = if self.reduced_motion {
                        duration.min(0.08)
                    } else {
                        duration
                    };
                    self.last = 0.0;
                    self.next = cx.new_next_frame();
                }
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)?;
        let c = cx.with_vm(CalendarColors::resolve);
        for widget in self.controls() {
            let r = widget.area().rect(cx);
            if r.size.x <= 0.0 || r.size.y <= 0.0 {
                continue;
            }
            if let Some(wash) = self.controls.get(&widget.widget_uid()) {
                if wash.value > 0.0 {
                    self.draw_wash.color = Vec4f {
                        w: wash.value as f32 * 0.06,
                        ..c.ink
                    };
                    self.draw_wash.draw_abs(cx, r);
                }
            }
            if cx.has_key_focus(widget.area()) {
                self.draw_wash.color = c.focus;
                self.draw_wash.draw_abs(
                    cx,
                    crate::presentation::rect(r.pos.x, r.pos.y + r.size.y - 1.0, r.size.x, 1.0),
                );
            }
        }
        DrawStep::done()
    }
}

/// The editor owns one scroll viewport for its complete, persistent field tree.
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarEditorScroll {
    #[deref]
    view: View,
    #[live]
    #[area]
    scrolling: ScrollBars,
    #[rust]
    pub reveal_focus: bool,
}
impl Widget for CalendarEditorScroll {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.scrolling.handle_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::KeyFocus(_)) {
            self.reveal_focus = true;
            self.view.redraw(cx);
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let viewport = cx.peek_walk_turtle(walk);
        self.scrolling.begin(
            cx,
            walk,
            Layout {
                flow: Flow::Down,
                ..Layout::default()
            },
        );
        self.view.draw_walk(
            cx,
            scope,
            Walk {
                width: Size::fill(),
                height: Size::fit(),
                ..Walk::default()
            },
        )?;
        self.scrolling.end(cx);
        if self.reveal_focus {
            self.reveal_focus = false;
            let focus = cx.key_focus();
            if !focus.is_empty() {
                let r = focus.rect(cx);
                if r.size.y > 0.0
                    && (r.pos.y < viewport.pos.y + 8.0
                        || r.pos.y + r.size.y > viewport.pos.y + viewport.size.y - 8.0)
                {
                    self.scrolling.scroll_into_view_abs(
                        cx,
                        crate::presentation::rect(
                            r.pos.x,
                            r.pos.y - 8.0,
                            r.size.x,
                            r.size.y + 16.0,
                        ),
                    );
                    self.view.redraw(cx);
                }
            }
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calendar_check_leads_the_name_on_one_row() {
        for (width,height) in [(402.0,44.0),(320.0,44.0),(180.0,36.0)] {
            let row=rect(20.0,100.0,width,height);
            let (check,name)=calendar_row_rects(row);
            assert_eq!(check.pos.y+check.size.y*0.5,name.pos.y+name.size.y*0.5);
            assert_eq!(name.pos.x-(check.pos.x+check.size.x),8.0);
            assert!(check.pos.y>=row.pos.y && check.pos.y+check.size.y<=row.pos.y+row.size.y);
            assert!(name.pos.x+name.size.x<=row.pos.x+row.size.x);
        }
    }
}
