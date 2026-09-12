//! Fixed date/all-day headers over one scrollable 24-hour presentation.
use crate::components::*;
use crate::engine::{
    all_day_lanes_for_days, hit_timed, initial_scroll_for_day, timed_rects_for_days, HitResult,
    TimedRect, HOUR_HEIGHT, TIMELINE_CONTENT_HEIGHT,
};
use crate::model::*;
use crate::presentation::rect;
use crate::presentation::*;
use makepad_civil_time::{self as civil, Day};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.calendar.*
    use mod.shader.*
    mod.widgets.TimelineGridBase = #(TimelineGrid::register_widget(vm))
    mod.widgets.TimelineGrid = set_type_default() do mod.widgets.TimelineGridBase{
        width: Fill height: 1536 flow: Overlay show_bg: false
        draw_ground.color: paper
        draw_rule.color: rule
        draw_now +: {color: action}
        draw_now_dot +: {
            color: action
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.circle(2.5,2.5,2.5)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_meta +: {color: secondary text_style: theme.font_regular{font_size: 8.25} max_lines:1 text_overflow:Ellipsis}
    }
    mod.widgets.CalendarTimelineBlock = mod.widgets.CalendarHitRow{
        width: Fill height: Fill flow: Down padding: Inset{left:6 right:6 top:2 bottom:2} spacing: 2
        show_bg: true
        draw_bg +: {
            color: instance(selection)
            stripe: instance(action)
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0,0.0,self.rect_size.x,self.rect_size.y,2.0)
                sdf.fill(self.color)
                sdf.rect(0.0,0.0,2.0,self.rect_size.y)
                sdf.fill(self.stripe)
                return sdf.result
            }
        }
        event_title := Ink{height:16 width:Fill draw_text.text_style:theme.font_bold{font_size:9.0}}
        event_time := Ink{height:14 width:Fill draw_text.text_style:theme.font_regular{font_size:8.25}}
    }
    mod.widgets.CalendarTimelineHeadersBase = #(CalendarTimelineHeaders::register_widget(vm))
    mod.widgets.CalendarTimelineHeaders = set_type_default() do mod.widgets.CalendarTimelineHeadersBase{
        height:44 width:Fill flow:Overlay show_bg:false
    }
    mod.widgets.CalendarDateStrip = mod.widgets.CalendarTimelineHeaders{height:64 strip:true}
    mod.widgets.CalendarAllDayBandBase = #(CalendarAllDayBand::register_widget(vm))
    mod.widgets.CalendarAllDayBand = set_type_default() do mod.widgets.CalendarAllDayBandBase{
        height:44 width:Fill flow:Overlay show_bg:false
        caption := Ink{width:52 height:44 text:"All-day" align:Align{x:0.5 y:0.5} draw_text +: {color:secondary text_style:theme.font_regular{font_size:8.25}}}
    }
    mod.widgets.CalendarTimelineScrollBase = #(CalendarTimelineScroll::register_widget(vm))
    mod.widgets.CalendarTimelineScroll = set_type_default() do mod.widgets.CalendarTimelineScrollBase{
        height:Fill width:Fill flow:Down show_bg:false
        scrolling: ScrollBars{show_scroll_x:false show_scroll_y:true}
        grid := mod.widgets.TimelineGrid{}
    }
    mod.widgets.CalendarTimelineBase = #(CalendarTimeline::register_widget(vm))
    mod.widgets.CalendarTimeline = set_type_default() do mod.widgets.CalendarTimelineBase{
        height:Fill width:Fill flow:Down show_bg:true draw_bg.color:paper padding:0 margin:0 spacing:0
        headers := mod.widgets.CalendarTimelineHeaders{height:44}
        all_day := mod.widgets.CalendarAllDayBand{height:44}
        body := mod.widgets.CalendarTimelineScroll{}
    }
}
#[derive(Clone, Debug, Default)]
pub struct TimelineFrame {
    pub days: Vec<Day>,
    pub events: Vec<PresentedOccurrence>,
    pub clock: ClockSnapshot,
    pub selected: Day,
    pub gutter: f64,
    pub compact: bool,
    pub short: bool,
}
impl TimelineFrame {
    fn occurrences(&self) -> Vec<Occurrence> {
        self.events.iter().map(|e| e.occurrence).collect()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct TimelineGrid {
    #[deref]
    view: View,
    #[live]
    draw_ground: DrawColor,
    #[live]
    draw_rule: DrawColor,
    #[live]
    draw_now: DrawColor,
    #[live]
    draw_now_dot: DrawColor,
    #[live]
    draw_meta: DrawText,
    #[rust]
    frame: TimelineFrame,
    #[rust]
    rects: Vec<TimedRect>,
}
impl Widget for TimelineGrid {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Blocks are presentational; the shared engine resolves overlapping touch regions.
        self.view.handle_event(cx, event, scope);
        match event.hits(cx, self.view.area()) {
            Hit::FingerDown(_) => cx.set_key_focus(self.view.area()),
            Hit::FingerUp(e) if e.is_primary_hit() && e.was_tap() => {
                let r = self.view.area().rect(cx);
                let p = e.abs - r.pos;
                let hit = hit_timed(
                    &self.rects,
                    &self.frame.days,
                    p.x,
                    p.y,
                    if e.device.is_touch() { 44.0 } else { 0.0 },
                );
                let action = match hit {
                    HitResult::Event(k) => CalendarAction::Event(k),
                    HitResult::Ambiguous(d) => CalendarAction::Agenda(d),
                    HitResult::Empty => {
                        if p.x < self.frame.gutter {
                            return;
                        }
                        let n = self.frame.days.len().max(1);
                        let w = (r.size.x - self.frame.gutter) / n as f64;
                        let i = ((p.x - self.frame.gutter) / w)
                            .floor()
                            .clamp(0.0, (n - 1) as f64) as usize;
                        let day = self
                            .frame
                            .days
                            .get(i)
                            .copied()
                            .unwrap_or(self.frame.selected);
                        CalendarAction::Create {
                            day,
                            minute: snap_minute_15(
                                (p.y / HOUR_HEIGHT * 60.0).clamp(0.0, 1439.0) as u16
                            ),
                        }
                    }
                };
                cx.widget_action(self.widget_uid(), action);
            }
            _ => {}
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let r = cx.peek_walk_turtle(walk);
        let c = cx.with_vm(CalendarColors::resolve);
        self.draw_ground.color = c.paper;
        self.draw_ground.draw_abs(cx, r);
        let width = r.size.x - if self.frame.compact { 12.0 } else { 0.0 };
        let col_w = (width - self.frame.gutter) / self.frame.days.len().max(1) as f64;
        self.draw_meta.color = c.secondary;
        let line = hairline(cx);
        for hour in 0..24 {
            let y = r.pos.y + hour as f64 * HOUR_HEIGHT;
            self.draw_rule.color = c.rule;
            self.draw_rule.draw_abs(
                cx,
                rect(
                    r.pos.x + self.frame.gutter,
                    y,
                    width - self.frame.gutter,
                    line,
                ),
            );
            self.draw_rule.color = mix(c.paper, c.rule, 0.5);
            self.draw_rule.draw_abs(
                cx,
                rect(
                    r.pos.x + self.frame.gutter,
                    y + 32.0,
                    width - self.frame.gutter,
                    line,
                ),
            );
            text_at(
                cx,
                &mut self.draw_meta,
                rect(r.pos.x + 4.0, y + 2.0, self.frame.gutter - 12.0, 16.0),
                &format!("{hour:02}:00"),
                Align { x: 1.0, y: 0.5 },
            );
        }
        for i in 0..self.frame.days.len() {
            self.draw_rule.color = mix(c.paper, c.rule, 0.5);
            self.draw_rule.draw_abs(
                cx,
                rect(
                    r.pos.x + self.frame.gutter + i as f64 * col_w,
                    r.pos.y,
                    line,
                    r.size.y,
                ),
            );
        }
        self.rects = timed_rects_for_days(
            &self.frame.occurrences(),
            &self.frame.days,
            width,
            self.frame.gutter,
        );
        let mut children = Vec::new();
        for tr in &self.rects {
            let Some(event) = self
                .frame
                .events
                .iter()
                .find(|e| e.occurrence.key == tr.key)
            else {
                continue;
            };
            let id = LiveId::from_str(&format!("block_{}_{}", tr.key.as_tool_id(), tr.day_index));
            let widget = self
                .view
                .children
                .iter()
                .find(|(key, _)| *key == id)
                .map(|(_, w)| w.clone())
                .unwrap_or_else(|| {
                    cx.with_vm(|vm| {
                        let v = script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* mod.widgets.CalendarTimelineBlock{}});
                        WidgetRef::script_from_value(vm, v)
                    })
                });
            place(
                &widget,
                cx,
                rect(
                    r.pos.x + tr.x,
                    r.pos.y + tr.y,
                    tr.w,
                    tr.h.min(TIMELINE_CONTENT_HEIGHT - tr.y),
                ),
            );
            let mut style = widget.clone();
            let color = c.tint(event.colour);
            let stripe = c.category(event.colour);
            script_apply_eval!(cx,style,{draw_bg +: {color:#(color) stripe:#(stripe)}});
            label(&widget, cx, ids!(event_title), &event.title, c.ink);
            let time = match event.occurrence.timing {
                Timing::Timed { start, end } => {
                    format!("{}–{}", start.format_hm(), end.format_hm())
                }
                _ => String::new(),
            };
            label(&widget, cx, ids!(event_time), &time, c.secondary);
            widget
                .widget(cx, ids!(event_time))
                .set_visible(cx, tr.h >= 36.0);
            children.push((id, widget));
        }
        self.view.children.clear();
        self.view.children.extend(children);
        cx.widget_tree_mark_dirty(self.widget_uid());
        self.view.draw_walk(cx, scope, walk)?;
        if let Some(i) = self
            .frame
            .days
            .iter()
            .position(|d| *d == self.frame.clock.today)
        {
            let y = r.pos.y + self.frame.clock.minute as f64 / 60.0 * HOUR_HEIGHT;
            let x = r.pos.x + self.frame.gutter + i as f64 * col_w;
            self.draw_now.color = c.action;
            self.draw_now.draw_abs(cx, rect(x, y, col_w, 1.0));
            self.draw_now_dot.color = c.action;
            self.draw_now_dot
                .draw_abs(cx, rect(x - 2.5, y - 2.0, 5.0, 5.0));
        }
        DrawStep::done()
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarTimelineHeaders {
    #[deref]
    view: View,
    #[live(false)]
    strip: bool,
    #[rust]
    pub frame: TimelineFrame,
}
impl Widget for CalendarTimelineHeaders {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let r = cx.peek_walk_turtle(walk);
        let c = cx.with_vm(CalendarColors::resolve);
        let days = if self.strip {
            (0..7).map(|i| monday_of(self.frame.selected) + i).collect()
        } else {
            self.frame.days.clone()
        };
        let gutter = if self.strip { 0.0 } else { self.frame.gutter };
        let w = (r.size.x - gutter) / days.len().max(1) as f64;
        let mut children = Vec::new();
        for (i, day) in days.iter().copied().enumerate() {
            let id = LiveId::from_str(&format!("date_{i}"));
            let child=self.view.children.iter().find(|(key,_)|*key==id).map(|(_,w)|w.clone()).unwrap_or_else(||cx.with_vm(|vm|{
                let v=script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* use mod.calendar.*
    use mod.shader.* mod.widgets.CalendarHitRow{flow:Down align:Align{x:0.5} spacing:0
                    weekday := Ink{height:16 width:Fill align:Align{x:0.5 y:0.5} draw_text.text_style:theme.font_regular{font_size:8.25}}
                    numeral := Ink{height:28 width:Fill align:Align{x:0.5 y:0.5} draw_text.text_style:theme.font_regular{font_size:15}}
                }});WidgetRef::script_from_value(vm,v)
            }));
            target(
                &child,
                if self.strip {
                    CalendarAction::SelectDay(day)
                } else {
                    CalendarAction::OpenDay(day)
                },
            );
            place(
                &child,
                cx,
                rect(r.pos.x + gutter + i as f64 * w, r.pos.y, w, r.size.y),
            );
            label(
                &child,
                cx,
                ids!(weekday),
                civil::WEEKDAY_ABBR[civil::weekday(day) as usize],
                c.secondary,
            );
            label(
                &child,
                cx,
                ids!(numeral),
                &civil::to_ymd(day).2.to_string(),
                if day == self.frame.clock.today {
                    c.action
                } else {
                    c.ink
                },
            );
            if let Some(mut row) = child.borrow_mut::<CalendarHitRow>() {
                row.selected = self.strip && day == self.frame.selected;
            }
            children.push((id, child));
        }
        self.view.children.clear();
        self.view.children.extend(children);
        cx.widget_tree_mark_dirty(self.widget_uid());
        self.view.draw_walk(cx, scope, walk)
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarAllDayBand {
    #[deref]
    view: View,
    #[rust]
    frame: TimelineFrame,
    #[rust]
    hits: Vec<(Rect, OccurrenceKey)>,
}
impl Widget for CalendarAllDayBand {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        match event.hits(cx, self.view.area()) {
            Hit::FingerDown(_) => {}
            Hit::FingerUp(e) if e.is_primary_hit() && e.was_tap() => {
                let r = self.view.area().rect(cx);
                let p = e.abs;
                let hits: Vec<_> = self
                    .hits
                    .iter()
                    .filter(|(r, _)| {
                        let w = r.size.x.max(44.0);
                        let h = r.size.y.max(44.0);
                        crate::presentation::rect(
                            r.pos.x - (w - r.size.x) * 0.5,
                            r.pos.y - (h - r.size.y) * 0.5,
                            w,
                            h,
                        )
                        .contains(p)
                    })
                    .collect();
                let action = if hits.len() == 1 {
                    CalendarAction::Event(hits[0].1)
                } else {
                    let column = ((p.x - r.pos.x - self.frame.gutter)
                        / ((r.size.x - self.frame.gutter) / self.frame.days.len().max(1) as f64))
                        .floor()
                        .max(0.0) as usize;
                    CalendarAction::Agenda(
                        self.frame
                            .days
                            .get(column)
                            .copied()
                            .unwrap_or(self.frame.selected),
                    )
                };
                cx.widget_action(self.widget_uid(), action);
            }
            _ => {}
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let r = cx.peek_walk_turtle(walk);
        let c = cx.with_vm(CalendarColors::resolve);
        let n = self.frame.days.len().max(1);
        let w = (r.size.x - self.frame.gutter) / n as f64;
        let lanes = all_day_lanes_for_days(&self.frame.occurrences(), &self.frame.days);
        let max_rows = ((r.size.y - 4.0) / 20.0).floor() as usize;
        let mut children = Vec::new();
        self.hits.clear();
        if let Some(caption) = self
            .view
            .children
            .iter()
            .find(|(id, _)| *id == live_id!(caption))
        {
            children.push(caption.clone());
        }
        for lane in lanes.iter().filter(|l| l.lane < max_rows) {
            let Some(event) = self
                .frame
                .events
                .iter()
                .find(|e| e.occurrence.key == lane.key)
            else {
                continue;
            };
            let id = occurrence_id("ribbon", lane.key);
            let child = self
                .view
                .children
                .iter()
                .find(|(key, _)| *key == id)
                .map(|(_, w)| w.clone())
                .unwrap_or_else(|| {
                    cx.with_vm(|vm| {
                        let v = script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* mod.widgets.CalendarRibbon{}});
                        WidgetRef::script_from_value(vm, v)
                    })
                });
            target(&child, CalendarAction::None);
            self.hits.push((
                rect(
                    r.pos.x + self.frame.gutter + lane.day_index as f64 * w + 4.0,
                    r.pos.y + 2.0 + lane.lane as f64 * 20.0,
                    lane.day_span as f64 * w - 8.0,
                    18.0,
                ),
                lane.key,
            ));
            crate::month::set_event(&child, cx, event, c, false);
            let mut style = child.clone();
            let first = self
                .frame
                .days
                .first()
                .copied()
                .unwrap_or(self.frame.selected);
            let last = self.frame.days.last().copied().unwrap_or(first) + 1;
            let continuation_left = if event.occurrence.timing.start_day() < first {
                1.0
            } else {
                0.0
            };
            let continuation_right = if event.occurrence.timing.end_day_exclusive() > last {
                1.0
            } else {
                0.0
            };
            script_apply_eval!(cx,style,{draw_bg +: {continuation_left:#(continuation_left) continuation_right:#(continuation_right)}});
            place(
                &child,
                cx,
                rect(
                    r.pos.x + self.frame.gutter + lane.day_index as f64 * w + 4.0,
                    r.pos.y + 2.0 + lane.lane as f64 * 20.0,
                    lane.day_span as f64 * w - 8.0,
                    18.0,
                ),
            );
            children.push((id, child));
        }
        if lanes.iter().any(|l| l.lane >= max_rows) {
            let id = live_id!(overflow);
            let child = self
                .view
                .children
                .iter()
                .find(|(key, _)| *key == id)
                .map(|(_, w)| w.clone())
                .unwrap_or_else(|| {
                    cx.with_vm(|vm| {
                        let v = script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* use mod.calendar.*
    use mod.shader.* mod.widgets.CalendarHitRow{label := Ink{text:"More" width:Fill height:Fill}}});
                        WidgetRef::script_from_value(vm, v)
                    })
                });
            target(&child, CalendarAction::Agenda(self.frame.selected));
            place(&child, cx, rect(r.pos.x, r.pos.y, self.frame.gutter, 44.0));
            children.retain(|(id, _)| *id != live_id!(caption));
            children.push((id, child));
        }
        self.view.children.clear();
        self.view.children.extend(children);
        cx.widget_tree_mark_dirty(self.widget_uid());
        self.view.draw_walk(cx, scope, walk)
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarTimelineScroll {
    #[deref]
    view: View,
    #[live]
    #[area]
    scrolling: ScrollBars,
    #[rust]
    frame: TimelineFrame,
    #[rust]
    initialized: bool,
    #[rust]
    pending: Option<f64>,
}
impl Widget for CalendarTimelineScroll {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.scrolling.handle_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let r = cx.peek_walk_turtle(walk);
        if !self.initialized {
            self.pending = Some(initial_scroll_for_day(
                self.frame.clock,
                self.frame.selected,
                r.size.y,
            ));
            self.initialized = true;
        }
        if let Some(y) = self.pending.take() {
            self.scrolling.set_scroll_pos_no_clip(cx, dvec2(0.0, y));
        }
        if let Some(mut grid) = self
            .view
            .widget(cx, ids!(grid))
            .borrow_mut::<TimelineGrid>()
        {
            grid.frame = self.frame.clone();
        }
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
                height: Size::Fixed(TIMELINE_CONTENT_HEIGHT),
                ..Walk::default()
            },
        )?;
        self.scrolling.end(cx);
        DrawStep::done()
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarTimeline {
    #[deref]
    view: View,
    #[rust]
    pub frame: TimelineFrame,
    #[live(true)]
    show_headers: bool,
}
impl CalendarTimeline {
    pub fn scroll_state(&self, cx: &mut Cx) -> Option<f64> {
        self.view
            .widget(cx, ids!(body))
            .borrow::<CalendarTimelineScroll>()
            .and_then(|b| b.initialized.then(|| b.scrolling.get_scroll_pos().y))
    }

    pub fn scroll(&self, cx: &mut Cx) -> f64 {
        self.view
            .widget(cx, ids!(body))
            .borrow::<CalendarTimelineScroll>()
            .map(|b| b.scrolling.get_scroll_pos().y)
            .unwrap_or(0.0)
    }
    pub fn set_scroll_y(&self, cx: &mut Cx, y: f64) {
        if let Some(mut b) = self
            .view
            .widget(cx, ids!(body))
            .borrow_mut::<CalendarTimelineScroll>()
        {
            b.pending = Some(y);
            b.initialized = true;
        }
    }
    pub fn scroll_to_today(&self, cx: &mut Cx) {
        if let Some(mut b) = self
            .view
            .widget(cx, ids!(body))
            .borrow_mut::<CalendarTimelineScroll>()
        {
            b.initialized = false;
        }
    }
}
impl Widget for CalendarTimeline {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view
            .widget(cx, ids!(headers))
            .set_visible(cx, self.show_headers);
        if let Some(mut h) = self
            .view
            .widget(cx, ids!(headers))
            .borrow_mut::<CalendarTimelineHeaders>()
        {
            h.frame = self.frame.clone();
        }
        let rows = all_day_lanes_for_days(&self.frame.occurrences(), &self.frame.days)
            .iter()
            .map(|r| r.lane + 1)
            .max()
            .unwrap_or(0);
        let height = if !self.frame.short && rows > 2 {
            88.0
        } else {
            44.0
        };
        let mut band = self.view.widget(cx, ids!(all_day));
        script_apply_eval!(cx,band,{height:#(height)});
        if let Some(mut b) = band.borrow_mut::<CalendarAllDayBand>() {
            b.frame = self.frame.clone();
        }
        if let Some(mut b) = self
            .view
            .widget(cx, ids!(body))
            .borrow_mut::<CalendarTimelineScroll>()
        {
            b.frame = self.frame.clone();
        }
        self.view.draw_walk(cx, scope, walk)
    }
}
