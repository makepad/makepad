//! Explicit agenda ownership: every nonempty result has a rendered occurrence row.
use crate::components::*;
use crate::model::*;
use crate::presentation::*;
use makepad_widgets::*;
use std::collections::BTreeMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.calendar.*
    mod.widgets.CalendarAgendaListBase = #(CalendarAgendaList::register_widget(vm))
    mod.widgets.CalendarAgendaList = set_type_default() do mod.widgets.CalendarAgendaListBase{
        width: Fill height: Fill flow: Down show_bg: false padding: 0 margin: 0
        scrolling: ScrollBars{show_scroll_x: false show_scroll_y: true}
    }
    mod.widgets.CalendarAgendaPage = Plain{flow: Down
        agenda_heading := Ink{height: 52 width: Fill padding: Inset{left: 20} text: "Upcoming" draw_text.text_style: theme.font_bold{font_size: 21}}
        agenda_range := Ink{height: 32 width: Fill padding: Inset{left: 20} draw_text +: {color: secondary text_style: theme.font_regular{font_size: 9.0}}}
        upcoming_agenda := mod.widgets.CalendarAgendaList{}
    }
    mod.widgets.CalendarAgendaHeading = Ink{width: Fill height: 52 padding: Inset{left: 20} text: "Upcoming" draw_text.text_style: theme.font_bold{font_size: 25.5}}
}
#[derive(Clone, Debug, Default)]
pub struct AgendaFrame {
    pub events: Vec<PresentedOccurrence>,
    pub grouped: bool,
    pub empty: String,
    pub allow_add: bool,
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarAgendaList {
    #[deref]
    view: View,
    #[live]
    #[area]
    scrolling: ScrollBars,
    #[rust]
    pub frame: AgendaFrame,
    #[rust]
    rows: BTreeMap<LiveId, WidgetRef>,
    #[rust]
    content_height: f64,
}
impl CalendarAgendaList {
    pub fn scroll(&self) -> f64 {
        self.scrolling.get_scroll_pos().y
    }
    pub fn set_scroll(&mut self, cx: &mut Cx, y: f64) {
        self.scrolling.set_scroll_pos_no_clip(cx, dvec2(0.0, y));
    }
    fn populate(&mut self, cx: &mut Cx) {
        let c = cx.with_vm(CalendarColors::resolve);
        self.frame.events.sort_by_cached_key(|e| occurrence_sort_key(&e.occurrence, &e.title, e.occurrence.key.event_id));
        let mut children = Vec::new();
        let mut last_day = None;
        let mut height = 0.0;
        for event in &self.frame.events {
            let day = event.occurrence.timing.start_day();
            if self.frame.grouped && last_day != Some(day) {
                let id = LiveId::from_str(&format!("date_{day}"));
                let row=self.rows.entry(id).or_insert_with(||cx.with_vm(|vm|{
                    let v=script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* use mod.calendar.* Ink{width:Fill height:32 padding:Inset{left:20 right:20} draw_text.text_style:theme.font_bold{font_size:11.25}}});
                    WidgetRef::script_from_value(vm,v)
                })).clone();
                row.as_label()
                    .set_text(cx, &format_heading_compact_day(day));
                row.as_label().set_text_color(cx, c.ink);
                children.push((id, row));
                height += 32.0;
                last_day = Some(day);
            }
            let id = occurrence_id("occurrence", event.occurrence.key);
            let row = self
                .rows
                .entry(id)
                .or_insert_with(|| {
                    cx.with_vm(|vm| {
                        let v = script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* mod.widgets.EventRow{}});
                        WidgetRef::script_from_value(vm, v)
                    })
                })
                .clone();
            target(&row, CalendarAction::Event(event.occurrence.key));
            let (start, end) = match event.occurrence.timing {
                Timing::AllDay { .. } => ("All-day".to_string(), String::new()),
                Timing::Timed { start, end } => (start.format_hm(), end.format_hm()),
            };
            label(&row, cx, ids!(starts), &start, c.ink);
            label(&row, cx, ids!(ends), &end, c.secondary);
            label(&row, cx, ids!(title), &event.title, c.ink);
            label(
                &row,
                cx,
                ids!(calendar_name),
                &event.calendar_name,
                c.secondary,
            );
            let color = c.category(event.colour);
            let mut stripe = row.widget(cx, ids!(category_stripe));
            script_apply_eval!(cx,stripe,{draw_bg.color: #(color)});
            children.push((id, row));
            height += 72.0;
        }
        if children.is_empty() {
            let id = live_id!(empty_state);
            let row=self.rows.entry(id).or_insert_with(||cx.with_vm(|vm|{
                let v=script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* use mod.calendar.* Plain{height:120 flow:Down padding:20 spacing:12
                    empty_title := Ink{width:Fill height:Fit flow:Right{wrap:true} max_lines:0 draw_text.text_style:theme.font_regular{font_size:12.75}}
                    empty_action := mod.widgets.CalendarHitRow{height:44
                        text := Ink{text:"Add Event" height:Fill draw_text.color:action}
                    }
                }});WidgetRef::script_from_value(vm,v)
            })).clone();
            label(&row, cx, ids!(empty_title), &self.frame.empty, c.secondary);
            row.widget(cx, ids!(empty_action))
                .set_visible(cx, self.frame.allow_add);
            target(&row.widget(cx, ids!(empty_action)), CalendarAction::Add);
            children.push((id, row));
            height = 120.0;
        }
        self.rows
            .retain(|id, _| children.iter().any(|(key, _)| key == id));
        let changed = self.view.children.len() != children.len()
            || self
                .view
                .children
                .iter()
                .zip(children.iter())
                .any(|(a, b)| a.0 != b.0);
        self.view.children.clear();
        self.view.children.extend(children);
        self.content_height = height;
        if changed {
            cx.widget_tree_mark_dirty(self.widget_uid());
        }
    }
}
impl Widget for CalendarAgendaList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.scrolling.handle_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.populate(cx);
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
                height: Size::Fixed(self.content_height),
                ..Walk::default()
            },
        )?;
        self.scrolling.end(cx);
        DrawStep::done()
    }
}
