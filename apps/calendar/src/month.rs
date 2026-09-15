//! Month and mini-month presentations, with geometry shared by drawing and hits.
use crate::components::*;
use crate::engine::{all_day_lanes_for_days, AllDayRect};
use crate::model::*;
use crate::presentation::rect;
use crate::presentation::*;
use makepad_civil_time::{self as civil, Day};
use makepad_widgets::makepad_platform::event::TouchState;
use makepad_widgets::*;
use std::collections::{BTreeMap, BTreeSet};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.calendar.*
    use mod.shader.*
    mod.widgets.CalendarMonthCanvasBase = #(CalendarMonthCanvas::register_widget(vm))
    mod.widgets.CalendarMonthCanvas = set_type_default() do mod.widgets.CalendarMonthCanvasBase{
        width: Fill height: Fill flow: Overlay show_bg: false
        draw_indicator +: {color: action pixel:fn(){let sdf=Sdf2d.viewport(self.pos*self.rect_size) sdf.box(0.0,0.0,self.rect_size.x,self.rect_size.y,0.75) sdf.fill(self.color) return sdf.result}}
        draw_weekend +: {color: ink}
    }
    mod.widgets.CalendarPhoneMonth = mod.widgets.CalendarBodyDeck{active:@dates
        dates := mod.widgets.CalendarMonthCanvas{phone:true}
    }
    mod.widgets.CalendarWeekdaysBase = #(CalendarWeekdays::register_widget(vm))
    mod.widgets.CalendarWeekdays = set_type_default() do mod.widgets.CalendarWeekdaysBase{
        width: Fill height: 24
        draw_text +: {color: secondary text_style: theme.font_regular{font_size: 8.25} max_lines: 1 text_overflow: Ellipsis}
    }
    mod.widgets.CalendarMiniDatesBase = #(CalendarMiniDates::register_widget(vm))
    mod.widgets.CalendarMiniDates = set_type_default() do mod.widgets.CalendarMiniDatesBase{
        width: Fill height: 144 flow: Overlay show_bg: false
    }
    mod.widgets.CalendarMiniMonthBase = #(CalendarMiniMonth::register_widget(vm))
    mod.widgets.CalendarMiniMonth = set_type_default() do mod.widgets.CalendarMiniMonthBase{
        width: Fill height: 208 flow: Down show_bg: false padding: 0 margin: Inset{left: 12 right: 12 bottom: 16}
        mini_header := Plain{height: 44 flow: Right align: Align{y: 0.5}
            mini_previous := QuietAction{width: 24 text: "‹"}
            mini_title := Ink{width: Fill height: Fill align: Align{x: 0.5 y: 0.5} draw_text.text_style: theme.font_bold{font_size: 9.75}}
            mini_next := QuietAction{width: 24 text: "›"}
        }
        mini_weekdays := mod.widgets.CalendarWeekdays{height: 20 abbreviated: true sidebar: true}
        mini_dates := mod.widgets.CalendarMiniDates{}
    }
    mod.widgets.CalendarMonthPage = Plain{flow: Down show_bg:true draw_bg.color:paper
        weekdays := mod.widgets.CalendarWeekdays{height: 24}
        month_canvas := mod.widgets.CalendarMonthCanvas{}
    }
    mod.widgets.CalendarShortMonth = Plain{flow: Down show_bg:true draw_bg.color:paper
        weekdays := mod.widgets.CalendarWeekdays{height: 24}
        month_scroll := ScrollYView{show_bg: false padding: 0 margin: 0 flow: Down
            month_canvas := mod.widgets.CalendarMonthCanvas{height: 384}
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MonthFrame {
    pub displayed: Day,
    pub selected: Day,
    pub today: Day,
    pub events: Vec<PresentedOccurrence>,
}
pub fn month_slots(cell_height: f64) -> usize {
    ((cell_height - 6.0 - 28.0 - 4.0 - 6.0 + 2.0) / 20.0)
        .floor()
        .max(0.0) as usize
}

#[derive(Debug)]
struct WeekSlots {
    ribbons: Vec<AllDayRect>,
    timed: Vec<(usize, usize, usize)>, // column, occurrence index, slot
    overflow: Vec<Option<(usize, usize)>>, // slot, hidden count
}
fn week_slots(occs: &[Occurrence], days: &[Day], slots: usize) -> WeekSlots {
    let counts: Vec<_> = days.iter().map(|d| occs.iter().filter(|o| o.timing.intersects_day(*d)).count()).collect();
    let limits: Vec<_> = counts.iter().map(|n| if *n > slots { slots.saturating_sub(1) } else { slots }).collect();
    let mut occupied = vec![vec![false; slots]; days.len()];
    let mut shown = vec![0; days.len()];
    let mut ribbons = Vec::new();
    for lane in all_day_lanes_for_days(occs, days) {
        let covered = lane.day_index..lane.day_index + lane.day_span;
        if covered.clone().any(|d| lane.lane >= limits[d]) { continue; }
        for d in covered {
            occupied[d][lane.lane] = true;
            shown[d] += 1;
        }
        ribbons.push(lane);
    }
    let mut timed = Vec::new();
    for (col, day) in days.iter().enumerate() {
        for (index, occ) in occs.iter().enumerate().filter(|(_, o)| matches!(o.timing, Timing::Timed { .. }) && o.timing.intersects_day(*day)) {
            let Some(slot) = (0..limits[col]).find(|slot| !occupied[col][*slot]) else { break; };
            let _ = occ;
            occupied[col][slot] = true;
            shown[col] += 1;
            timed.push((col, index, slot));
        }
    }
    let overflow = counts.iter().enumerate().map(|(col, count)| {
        let hidden = count - shown[col];
        if hidden == 0 { None } else {
            (0..slots).rev().find(|slot| !occupied[col][*slot]).map(|slot| (slot, hidden))
        }
    }).collect();
    WeekSlots { ribbons, timed, overflow }
}

fn month_hit(hits: &[(Rect, CalendarAction)], point: Vec2d, min_hit: f64, day: Day) -> CalendarAction {
    let mut matched = hits.iter().filter(|(r, _)| {
        let w = r.size.x.max(min_hit);
        let h = r.size.y.max(min_hit);
        rect(r.pos.x - (w-r.size.x)*0.5, r.pos.y - (h-r.size.y)*0.5, w, h).contains(point)
    });
    let Some((_, action)) = matched.next() else { return CalendarAction::None; };
    if matched.next().is_some() { CalendarAction::Agenda(day) } else { *action }
}

#[derive(Script, ScriptHook, Widget)]
pub struct CalendarWeekdays {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live(false)]
    abbreviated: bool,
    #[live(0.0)]
    inset: f64,
    #[live(false)]
    sidebar: bool,
    #[redraw]
    #[rust]
    area: Area,
}
impl Widget for CalendarWeekdays {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        let c = cx.with_vm(CalendarColors::resolve);
        self.draw_text.color = if self.sidebar { c.on_surface(c.sidebar).secondary } else { c.secondary };
        let w = (r.size.x - self.inset * 2.0) / 7.0;
        for i in 0..7 {
            let day = civil::WEEKDAY_ABBR[i];
            let day = if self.abbreviated { &day[..1] } else { day };
            text_at(
                cx,
                &mut self.draw_text,
                rect(r.pos.x + self.inset + i as f64 * w, r.pos.y, w, r.size.y),
                day,
                Align { x: 0.5, y: 0.5 },
            );
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct CalendarMonthCanvas {
    #[deref]
    view: View,
    #[live(false)]
    phone: bool,
    #[live]
    draw_indicator: DrawColor,
    #[live]
    draw_weekend: DrawColor,
    #[rust]
    pub frame: MonthFrame,
    #[rust]
    swipe_origin: Option<Vec2d>,
    #[rust]
    event_widgets: BTreeMap<LiveId, WidgetRef>,
    #[rust]
    hits: Vec<(Rect, CalendarAction)>,
}
impl CalendarMonthCanvas {
    fn ensure_children(&mut self, cx: &mut Cx) {
        if self.view.children.len() >= 53 {
            return;
        }
        for i in 0..42 {
            let child = cx.with_vm(|vm| {
                let v = script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* mod.widgets.CalendarDayPresentation{}});
                WidgetRef::script_from_value(vm, v)
            });
            add_child(
                &mut self.view,
                cx,
                LiveId::from_str(&format!("day_{i:02}")),
                child,
            );
        }
        for i in 0..5 {
            let child = cx.with_vm(|vm| {
                let v = script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* mod.widgets.AppRule{}});
                WidgetRef::script_from_value(vm, v)
            });
            add_child(
                &mut self.view,
                cx,
                LiveId::from_str(&format!("week_rule_{i}")),
                child,
            );
        }
        for i in 0..6 {
            let child = cx.with_vm(|vm| {
                let v = script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* mod.widgets.AppRule{vertical:true strength:0.5}});
                WidgetRef::script_from_value(vm, v)
            });
            add_child(
                &mut self.view,
                cx,
                LiveId::from_str(&format!("day_rule_{i}")),
                child,
            );
        }
    }
    fn event_widget(&mut self, cx: &mut Cx, key: LiveId, ribbon: bool) -> WidgetRef {
        if let Some(w) = self.event_widgets.get(&key) {
            return w.clone();
        }
        let w = cx.with_vm(|vm| {
            let value = if ribbon {
                script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* mod.widgets.CalendarRibbon{}})
            } else {
                script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* mod.widgets.CalendarEventPresentation{}})
            };
            WidgetRef::script_from_value(vm, value)
        });
        self.event_widgets.insert(key, w.clone());
        w
    }
    fn populate(&mut self, cx: &mut Cx, r: Rect) {
        self.ensure_children(cx);
        let colors = cx.with_vm(CalendarColors::resolve);
        let (year, month, _) = civil::to_ymd(self.frame.displayed);
        let days = month_grid_monday(year, month);
        let w = r.size.x / 7.0;
        let h = r.size.y / 6.0;
        let mut used = BTreeSet::new();
        self.hits.clear();
        // Reparent event presentations by occurrence key; dates and their labels never change identity.
        for i in 0..42 {
            let day = days[i / 7][i % 7];
            let child = self.view.children[i].1.clone();
            place(
                &child,
                cx,
                rect(
                    r.pos.x + (i % 7) as f64 * w,
                    r.pos.y + (i / 7) as f64 * h,
                    w,
                    h,
                ),
            );
            target(&child, CalendarAction::SelectDay(day.day));
            let size = if self.phone { 32.0 } else { 26.0 };
            let x = if self.phone {
                (w - size) * 0.5
            } else {
                w - 6.0 - size
            };
            let y = if self.phone {
                (h - 44.0) * 0.5 + 2.0
            } else {
                6.0
            };
            let mark = child.widget(cx, ids!(date_mark));
            // Child positions are absolute in the current allocation, never last frame's bounds.
            let origin = r.pos + dvec2((i % 7) as f64 * w, (i / 7) as f64 * h);
            place(&mark, cx, rect(origin.x + x, origin.y + y, size, size));
            let color = if day.day == self.frame.today {
                colors.action
            } else if day.day == self.frame.selected {
                colors.selection
            } else {
                Vec4f::default()
            };
            if let Some(mut mark) = mark.borrow_mut::<CalendarDateMark>() {
                mark.set_color(cx, color);
                mark.focused = cx.has_key_focus(child.area());
            }
            let numeral = child.widget(cx, ids!(date_number));
            place(&numeral, cx, rect(origin.x + x, origin.y + y, size, size));
            let mut numeral_style = numeral.clone();
            let font_size = if self.phone { 15.0 } else { 9.75 };
            script_apply_eval!(cx,numeral_style,{draw_text +: {text_style +: {font_size: #(font_size)}}});
            label(
                &child,
                cx,
                ids!(date_number),
                &civil::to_ymd(day.day).2.to_string(),
                if day.day == self.frame.today {
                    colors.on_action()
                } else if day.in_month {
                    colors.ink
                } else {
                    colors.secondary
                },
            );
            if let Some(mut events) = child.widget(cx, ids!(events)).borrow_mut::<View>() {
                events.children.clear();
            }
            child.widget(cx, ids!(overflow)).set_visible(cx, false);
        }
        // Horizontal/vertical rules belong to the canvas, with no outer border.
        for i in 0..11 {
            let rule = self.view.children[42 + i].1.clone();
            rule.set_visible(cx, !self.phone);
            if i < 5 {
                place(
                    &rule,
                    cx,
                    rect(r.pos.x, r.pos.y + (i + 1) as f64 * h, r.size.x, 0.5),
                );
            } else {
                place(
                    &rule,
                    cx,
                    rect(r.pos.x + (i - 4) as f64 * w, r.pos.y, 1.0, r.size.y),
                );
            }
        }
        // Drop last allocation's direct ribbon children before adding this allocation's ribbons.
        self.view.children.truncate(53);
        if !self.phone {
            let slots = month_slots(h);
            for week in 0..6 {
                let week_days: Vec<Day> = days[week].iter().map(|d| d.day).collect();
                let events: Vec<_> = self
                    .frame
                    .events
                    .iter()
                    .filter(|e| {
                        e.occurrence
                            .timing
                            .intersects_range(week_days[0], week_days[0] + 7)
                    })
                    .cloned()
                    .collect();
                let occs: Vec<_> = events.iter().map(|e| e.occurrence).collect();
                let layout = week_slots(&occs, &week_days, slots);
                for lane in &layout.ribbons {
                    let Some(event) = events.iter().find(|e| e.occurrence.key == lane.key) else {
                        continue;
                    };
                    let start = lane.day_index;
                    let key =
                        LiveId::from_str(&format!("ribbon_{}_week_{week}", lane.key.as_tool_id()));
                    let widget = self.event_widget(cx, key, true);
                    used.insert(key);
                    target(&widget, CalendarAction::Event(lane.key));
                    set_event(&widget, cx, event, colors, true);
                    let left = if event.occurrence.timing.start_day() < week_days[0] {
                        0.0
                    } else {
                        6.0
                    };
                    let right = if event.occurrence.timing.end_day_exclusive() > week_days[6] + 1 {
                        0.0
                    } else {
                        6.0
                    };
                    let mut style = widget.clone();
                    let continuation_left = if left == 0.0 { 1.0 } else { 0.0 };
                    let continuation_right = if right == 0.0 { 1.0 } else { 0.0 };
                    script_apply_eval!(cx,style,{draw_bg +: {continuation_left:#(continuation_left) continuation_right:#(continuation_right)}});
                    place(
                        &widget,
                        cx,
                        rect(
                            r.pos.x + start as f64 * w + left,
                            r.pos.y + week as f64 * h + 38.0 + lane.lane as f64 * 20.0,
                            lane.day_span as f64 * w - left - right,
                            18.0,
                        ),
                    );
                    add_child(&mut self.view, cx, key, widget);
                    self.hits.push((rect(
                        r.pos.x + start as f64 * w + left,
                        r.pos.y + week as f64 * h + 38.0 + lane.lane as f64 * 20.0,
                        lane.day_span as f64 * w - left - right, 18.0,
                    ), CalendarAction::Event(lane.key)));
                }
                for col in 0..7 {
                    let child = self.view.children[week * 7 + col].1.clone();
                    let day = week_days[col];
                    for &(_, index, slot) in layout.timed.iter().filter(|(d, _, _)| *d == col) {
                        let event = &events[index];
                        let key = LiveId::from_str(&format!(
                            "event_{}_day_{day}",
                            event.occurrence.key.as_tool_id()
                        ));
                        let widget = self.event_widget(cx, key, false);
                        used.insert(key);
                        target(&widget, CalendarAction::Event(event.occurrence.key));
                        set_event(&widget, cx, event, colors, w > 110.0);
                        place(
                            &widget,
                            cx,
                            rect(
                                r.pos.x + col as f64 * w + 6.0,
                                r.pos.y + week as f64 * h + 38.0 + slot as f64 * 20.0,
                                w - 12.0,
                                18.0,
                            ),
                        );
                        if let Some(mut event_view) =
                            child.widget(cx, ids!(events)).borrow_mut::<View>()
                        {
                            add_child(&mut event_view, cx, key, widget);
                        }
                        self.hits.push((rect(
                            r.pos.x + col as f64 * w + 6.0,
                            r.pos.y + week as f64 * h + 38.0 + slot as f64 * 20.0,
                            w - 12.0, 18.0,
                        ), CalendarAction::Event(event.occurrence.key)));
                    }
                    if let Some((slot, more)) = layout.overflow[col] {
                        let overflow = child.widget(cx, ids!(overflow));
                        overflow.set_visible(cx, true);
                        target(&overflow, CalendarAction::Agenda(day));
                        label(
                            &overflow,
                            cx,
                            ids!(label),
                            &format!("+{more} more"),
                            colors.secondary,
                        );
                        place(
                            &overflow,
                            cx,
                            rect(
                                r.pos.x + col as f64 * w + 6.0,
                                r.pos.y
                                    + week as f64 * h
                                    + 38.0
                                    + slot as f64 * 20.0,
                                w - 12.0,
                                18.0,
                            ),
                        );
                        self.hits.push((rect(
                            r.pos.x + col as f64 * w + 6.0,
                            r.pos.y + week as f64 * h + 38.0 + slot as f64 * 20.0,
                            w - 12.0, 18.0,
                        ), CalendarAction::Agenda(day)));
                    }
                }
            }
        }
        self.event_widgets.retain(|id, _| used.contains(id));
        cx.widget_tree_mark_dirty(self.widget_uid());
    }
}
pub fn set_event(
    widget: &WidgetRef,
    cx: &mut Cx,
    event: &PresentedOccurrence,
    colors: CalendarColors,
    show_time: bool,
) {
    label(widget, cx, ids!(event_title), &event.title, colors.ink);
    let time = match event.occurrence.timing {
        Timing::Timed { start, .. } => start.format_hm(),
        _ => String::new(),
    };
    label(widget, cx, ids!(event_time), &time, colors.secondary);
    widget
        .widget(cx, ids!(event_time))
        .set_visible(cx, show_time && !time.is_empty());
    let mut stripe = widget.widget(cx, ids!(category_mark));
    let color = colors.category(event.colour);
    script_apply_eval!(cx,stripe,{draw_bg.color: #(color)});
    if matches!(event.occurrence.timing, Timing::AllDay { .. }) {
        let mut widget = widget.clone();
        let color = colors.tint(event.colour);
        script_apply_eval!(cx,widget,{draw_bg.color: #(color)});
    }
}
impl Widget for CalendarMonthCanvas {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Claim the canvas before child rows, so expanded touch regions are resolved together.
        match event.hits(cx, self.view.area()) {
            Hit::FingerDown(_) => cx.set_key_focus(self.view.area()),
            Hit::FingerUp(e) if e.is_primary_hit() && e.was_tap() => {
                let r = self.view.area().rect(cx);
                let (year, month, _) = civil::to_ymd(self.frame.displayed);
                let grid = month_grid_monday(year, month);
                let col = ((e.abs.x - r.pos.x) / (r.size.x / 7.0)).floor().clamp(0.0, 6.0) as usize;
                let row = ((e.abs.y - r.pos.y) / (r.size.y / 6.0)).floor().clamp(0.0, 5.0) as usize;
                let day = grid[row][col].day;
                let action = month_hit(&self.hits, e.abs, if e.device.is_touch() {44.0} else {0.0}, day);
                cx.widget_action(self.widget_uid(), if action != CalendarAction::None { action }
                    else if e.tap_count >= 2 { CalendarAction::OpenDay(day) }
                    else { CalendarAction::SelectDay(day) });
            }
            _ => {}
        }
        self.view.handle_event(cx, event, scope);
        if !self.phone {
            return;
        }
        let r = self.view.area().rect(cx);
        let mut finish = None;
        match event {
            Event::MouseDown(e) if r.contains(e.abs) => self.swipe_origin = Some(e.abs),
            Event::MouseUp(e) => finish = Some(e.abs),
            Event::TouchUpdate(e) => {
                for t in &e.touches {
                    match t.state {
                        TouchState::Start if r.contains(t.abs) => self.swipe_origin = Some(t.abs),
                        TouchState::Stop => finish = Some(t.abs),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if let Some(p) = finish {
            if let Some(origin) = self.swipe_origin.take() {
                let delta = p - origin;
                if delta.x.abs() > 60.0 && delta.x.abs() > 1.5 * delta.y.abs() {
                    cx.widget_action(
                        self.widget_uid(),
                        CalendarAction::Period(if delta.x < 0.0 { 1 } else { -1 }),
                    );
                }
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let r = cx.peek_walk_turtle(walk);
        self.populate(cx, r);
        let c = cx.with_vm(CalendarColors::resolve);
        let w = r.size.x / 7.0;
        let h = r.size.y / 6.0;
        if !self.phone {
            self.draw_weekend.color = Vec4f { w: 0.02, ..c.ink };
            self.draw_weekend
                .draw_abs(cx, rect(r.pos.x + 5.0 * w, r.pos.y, 2.0 * w, r.size.y));
        }
        self.view.draw_walk(cx, scope, walk)?;
        if self.phone {
            let (y, m, _) = civil::to_ymd(self.frame.displayed);
            let days = month_grid_monday(y, m);
            for i in 0..42 {
                let day = days[i / 7][i % 7].day;
                let mut categories = Vec::new();
                let mut count = 0;
                for e in self
                    .frame
                    .events
                    .iter()
                    .filter(|e| e.occurrence.timing.intersects_day(day))
                {
                    count += 1;
                    if !categories.contains(&e.colour) {
                        categories.push(e.colour);
                    }
                }
                let visible = categories.len().min(3);
                let width = visible as f64 * 8.0 - 2.0;
                for (j, cat) in categories.iter().take(3).enumerate() {
                    self.draw_indicator.color = c.category(*cat);
                    self.draw_indicator.draw_abs(
                        cx,
                        rect(
                            r.pos.x + (i % 7) as f64 * w + (w - width) * 0.5 + j as f64 * 8.0,
                            r.pos.y + (i / 7) as f64 * h + (h - 44.0) * 0.5 + 37.0,
                            6.0,
                            3.0,
                        ),
                    );
                }
                if count > 3 {
                    self.draw_indicator.color = c.secondary;
                    self.draw_indicator.draw_abs(
                        cx,
                        rect(
                            r.pos.x + (i % 7) as f64 * w + (w + width) * 0.5 + 3.0,
                            r.pos.y + (i / 7) as f64 * h + (h - 44.0) * 0.5 + 37.0,
                            2.0,
                            3.0,
                        ),
                    );
                }
            }
        }
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct CalendarMiniDates {
    #[deref]
    view: View,
    #[rust]
    pub frame: MonthFrame,
}
impl Widget for CalendarMiniDates {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.view.children.is_empty() {
            for i in 0..42 {
                let w = cx.with_vm(|vm| {
                    let v = script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* mod.widgets.CalendarMiniDate{}});
                    WidgetRef::script_from_value(vm, v)
                });
                add_child(
                    &mut self.view,
                    cx,
                    LiveId::from_str(&format!("date_{i:02}")),
                    w,
                );
            }
        }
        let r = cx.peek_walk_turtle(walk);
        let c = cx.with_vm(CalendarColors::resolve);
        let c = c.on_surface(c.sidebar);
        let (y, m, _) = civil::to_ymd(self.frame.displayed);
        let days = month_grid_monday(y, m);
        for i in 0..42 {
            let gd = days[i / 7][i % 7];
            let w = self.view.children[i].1.clone();
            let width = r.size.x / 7.0;
            let origin = r.pos + dvec2((i % 7) as f64 * width, (i / 7) as f64 * 24.0);
            place(&w, cx, rect(origin.x, origin.y, width, 24.0));
            target(&w, CalendarAction::SelectDay(gd.day));
            let mark = w.widget(cx, ids!(selection_mark));
            place(
                &mark,
                cx,
                rect(origin.x + (width - 22.0) * 0.5, origin.y + 1.0, 22.0, 22.0),
            );
            let color = if gd.day == self.frame.today {
                c.action
            } else if gd.day == self.frame.selected {
                c.selection
            } else {
                Vec4f::default()
            };
            if let Some(mut mark) = mark.borrow_mut::<CalendarDateMark>() {
                mark.set_color(cx, color);
            }
            label(
                &w,
                cx,
                ids!(numeral),
                &civil::to_ymd(gd.day).2.to_string(),
                if gd.day == self.frame.today {
                    c.on_action()
                } else if gd.in_month {
                    c.ink
                } else {
                    c.secondary
                },
            );
        }
        self.view.draw_walk(cx, scope, walk)
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarMiniMonth {
    #[deref]
    view: View,
    #[rust]
    pub frame: MonthFrame,
}
impl Widget for CalendarMiniMonth {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(a) = event {
            if self.view.button(cx, ids!(mini_previous)).clicked(a) {
                cx.widget_action(self.widget_uid(), CalendarAction::Period(-1));
            }
            if self.view.button(cx, ids!(mini_next)).clicked(a) {
                cx.widget_action(self.widget_uid(), CalendarAction::Period(1));
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view
            .label(cx, ids!(mini_title))
            .set_text(cx, &format_month_year(self.frame.displayed));
        if let Some(mut dates) = self
            .view
            .widget(cx, ids!(mini_dates))
            .borrow_mut::<CalendarMiniDates>()
        {
            dates.frame = self.frame.clone();
        }
        self.view.draw_walk(cx, scope, walk)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slots_respect_cell_geometry_and_overflow_reservation() {
        assert_eq!(month_slots(112.0), 3);
        assert_eq!(month_slots(64.0), 1);
        assert_eq!(month_slots(44.0), 0);
        let d = civil::from_ymd(2026, 9, 9);
        let doc = crate::seed::seed(d);
        let events = crate::engine::occurrences_for_day(&doc, d, true, UI_EXPAND_CAP);
        let (shown, extra) = crate::engine::day_chips(&events.items, month_slots(112.0));
        assert_eq!(shown.len() + extra, events.items.len());
        assert!(shown.len() + usize::from(extra > 0) <= 3);
    }
    #[test]
    fn sparse_ribbon_lanes_leave_timed_and_overflow_slots_free() {
        let monday = civil::from_ymd(2026,9,7);
        let days: Vec<_> = (0..7).map(|i| monday+i).collect();
        let mut occs = Vec::new();
        for (id, start, end) in [(1,0,2),(2,0,4),(3,1,7)] {
            occs.push(Occurrence {key:OccurrenceKey{event_id:EventId(id),start_day:monday+start},timing:Timing::AllDay{start:monday+start,end_exclusive:monday+end}});
        }
        let day = monday+2;
        occs.push(Occurrence { key:OccurrenceKey{event_id:EventId(4),start_day:day},timing:Timing::Timed{start:LocalMinute::new(day,540).unwrap(),end:LocalMinute::new(day,600).unwrap()} });
        let layout = week_slots(&occs,&days,3);
        assert!(layout.timed.contains(&(2,3,1)), "timed event fills the gap between ribbon lanes 0 and 2");
        assert_eq!(layout.overflow[2],None);
        occs.push(Occurrence{key:OccurrenceKey{event_id:EventId(5),start_day:day},..occs[3]});
        for slots in 0..=4 {
            let layout = week_slots(&occs,&days,slots);
            for col in 0..7 {
                let mut taken = std::collections::BTreeSet::new();
                for ribbon in &layout.ribbons {
                    if (ribbon.day_index..ribbon.day_index+ribbon.day_span).contains(&col) {assert!(taken.insert(ribbon.lane));}
                }
                for &(d,_,slot) in &layout.timed {if d==col {assert!(taken.insert(slot));}}
                let count = occs.iter().filter(|o| o.timing.intersects_day(days[col])).count();
                if let Some((slot,hidden)) = layout.overflow[col] {
                    assert_eq!(taken.len()+hidden,count);
                    assert!(taken.insert(slot), "overflow must never overlap a ribbon");
                } else if slots>0 {assert_eq!(taken.len(),count);}
                assert!(taken.iter().all(|slot| *slot<slots));
            }
        }
    }
    #[test]
    fn month_touch_targets_expand_and_ambiguous_hits_open_the_day_agenda() {
        let day=20;
        let key=OccurrenceKey{event_id:EventId(1),start_day:day};
        let mut hits=vec![(rect(100.0,38.0,100.0,18.0),CalendarAction::Event(key))];
        assert_eq!(month_hit(&hits,dvec2(110.0,25.0),44.0,day),CalendarAction::Event(key));
        assert_eq!(month_hit(&hits,dvec2(110.0,25.0),0.0,day),CalendarAction::None);
        hits.push((rect(100.0,58.0,100.0,18.0),CalendarAction::Agenda(day)));
        assert_eq!(month_hit(&hits,dvec2(110.0,55.0),44.0,day),CalendarAction::Agenda(day));
        assert_eq!(month_hit(&hits,dvec2(110.0,55.0),0.0,day),CalendarAction::Event(key));
        assert_eq!(month_hit(&hits,dvec2(110.0,88.0),44.0,day),CalendarAction::Agenda(day));
    }

}
