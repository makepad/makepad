//! The control panel: a header (how the master is reached, the fleet in numbers, the window
//! and follow controls), the occupancy timeline, and at the right what needs a look, the
//! detail of what was picked, and the queue; the landing strip at the bottom. It starts its
//! data source when it first sees an event, judges each new fleet's needs once, and hands the
//! fleet and its needs to its views; apps/devhub can embed it as it is.
//!
//! A pick (a need, or a lease clicked on the timeline) frames its lease on the timeline, opens
//! its detail, and asks the master for its job's output while the detail stays open.
//!
//! A hidden panel (its window closed to a tray) keeps taking fleets for its [`Glance`] but hands
//! none to its views, asks for no job output and has its source ask less often; shown again,
//! its views get the newest fleet.

use crate::detail::{DetailPane, DetailPaneAction, Subject};
use crate::fmt::{clock, dur, local_offset_s};
use crate::glance::Glance;
use crate::model::{Fleet, Link};
use crate::needs::{needs, Need, Sev};
use crate::needs_list::{NeedsList, NeedsListAction};
use crate::queue::FleetQueue;
use crate::source::{Mode, Source};
use crate::strip::LandStrip;
use crate::timeline::{FleetTimeline, FleetTimelineAction};
use makepad_widgets::*;
use std::collections::HashMap;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    let HeadButton = ButtonFlatter{
        height: 22 margin: 0 padding: Inset{left: 9 right: 9 top: 2 bottom: 2}
        draw_text +: {text_style: theme.font_regular{font_size: 8} color: theme.color_chart_ink}
    }
    let HeadText = Label{
        padding: Inset{left: 4 right: 4}
        draw_text +: {color: theme.color_chart_ink_dim text_style: theme.font_regular{font_size: 8}}
    }
    let Rule = View{width: Fill height: 1 show_bg: true draw_bg +: {color: mix(theme.color_chart_ground, theme.color_chart_ink, 0.12)}}

    mod.widgets.DevhubPanelBase = #(DevhubPanel::register_widget(vm))
    mod.widgets.DevhubPanel = set_type_default() do mod.widgets.DevhubPanelBase{
        width: Fill height: Fill flow: Down spacing: 0
        show_bg: true
        draw_bg +: {color: theme.color_chart_ground}
        View{
            width: Fill height: 32 flow: Right spacing: 2 align: Align{y: 0.5} padding: Inset{left: 10 right: 8}
            panel_title := Label{
                padding: Inset{right: 8}
                text: "Fleet"
                draw_text +: {color: theme.color_chart_ink text_style: theme.font_bold{font_size: 10}}
            }
            panel_link := Label{
                padding: Inset{right: 6}
                text: "connecting to the master\u{2026}"
                draw_text +: {color: theme.color_chart_ink text_style: theme.font_regular{font_size: 8.5}}
            }
            panel_counts := HeadText{text: ""}
            View{width: Fill height: 1}
            win_15 := HeadButton{text: "15 min"}
            win_60 := HeadButton{text: "1 h"}
            win_180 := HeadButton{text: "3 h"}
            View{width: 6 height: 1}
            panel_follow := HeadButton{text: "Following now"}
        }
        Rule{}
        View{
            width: Fill height: Fill flow: Right spacing: 0
            panel_timeline := mod.widgets.FleetTimeline{}
            View{width: 1 height: Fill show_bg: true draw_bg +: {color: mix(theme.color_chart_ground, theme.color_chart_ink, 0.12)}}
            View{
                width: 340 height: Fill flow: Down spacing: 0
                panel_needs := mod.widgets.NeedsList{}
                panel_detail := mod.widgets.DetailPane{}
                panel_queue := mod.widgets.FleetQueue{width: Fill height: Fill}
            }
        }
        panel_strip := mod.widgets.LandStrip{}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DevhubPanel {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    data: Option<Source>,
    #[rust]
    fleet: Option<Arc<Fleet>>,
    #[rust]
    needs: Arc<Vec<Need>>,
    #[rust]
    started: bool,
    #[rust]
    hidden: bool,
}

impl DevhubPanel {
    fn start(&mut self, cx: &mut Cx) {
        self.started = true;
        match Source::start(&cx.thread_spawner(), Mode::from_env()) {
            Ok(s) => self.data = Some(s),
            Err(e) => self.view.label(cx, ids!(panel_link)).set_text(cx, &format!("cannot start: {e}")),
        }
    }

    fn take(&mut self, cx: &mut Cx) {
        let Some(f) = self.data.as_ref().and_then(Source::take) else { return };
        // the needs, once per fleet: the list shows them, the timeline makes their leases glow
        self.needs = Arc::new(if f.now_ms > 0 { needs(&f) } else { Vec::new() });
        self.fleet = Some(f);
        if !self.hidden {
            self.show_fleet(cx);
        }
    }

    /// Hands the newest fleet and its needs to the views.
    fn show_fleet(&mut self, cx: &mut Cx) {
        let Some(f) = self.fleet.clone() else { return };
        let live = f.now_ms > 0;
        let needs = self.needs.clone();
        let mut flags: HashMap<String, (Sev, &'static str)> = HashMap::new();
        for n in needs.iter() {
            if let Some(l) = &n.lease {
                // the worst first: a lease keeps its first flag
                flags.entry(l.clone()).or_insert((n.sev, n.what.tag()));
            }
        }
        if let Some(mut t) = self.view.widget(cx, ids!(panel_timeline)).borrow_mut::<FleetTimeline>() {
            t.set_fleet(cx, f.clone());
            t.set_flags(cx, flags);
        }
        if let Some(mut n) = self.view.widget(cx, ids!(panel_needs)).borrow_mut::<NeedsList>() {
            n.set_needs(cx, needs.clone(), live);
        }
        if let Some(mut d) = self.view.widget(cx, ids!(panel_detail)).borrow_mut::<DetailPane>() {
            d.set_fleet(cx, f.clone());
        }
        if let Some(mut q) = self.view.widget(cx, ids!(panel_queue)).borrow_mut::<FleetQueue>() {
            q.set_fleet(cx, f.clone());
        }
        if let Some(mut s) = self.view.widget(cx, ids!(panel_strip)).borrow_mut::<LandStrip>() {
            s.set_fleet(cx, f);
        }
        self.want_output(cx);
        self.header(cx);
    }

    /// The fleet at a glance, for a tray (None: nothing read yet).
    pub fn glance(&self) -> Option<Glance> {
        self.fleet.as_ref().map(|f| Glance::of(f, &self.needs))
    }

    /// The panel's window was closed to a tray (false) or shown again (true).
    pub fn set_shown(&mut self, cx: &mut Cx, shown: bool) {
        let hidden = !shown;
        if self.hidden == hidden {
            return;
        }
        self.hidden = hidden;
        if let Some(d) = &self.data {
            d.set_hidden(hidden);
        }
        if shown {
            self.show_fleet(cx);
        } else if let Some(d) = &self.data {
            d.want_job(None);
        }
    }

    /// Asks for the open detail's job output (its job can appear after the pick: jobs are
    /// asked for every 5 s), or for none.
    fn want_output(&mut self, cx: &mut Cx) {
        if self.hidden {
            return;
        }
        let subject = self.view.widget(cx, ids!(panel_detail)).borrow::<DetailPane>().and_then(|d| d.subject().cloned());
        let job = match (subject, self.fleet.as_ref()) {
            (Some(s), Some(f)) => s.lease.as_deref().and_then(|l| f.job_of(l)).map(|j| j.id.clone()).or_else(|| s.need.and_then(|n| n.job)),
            _ => None,
        };
        if let Some(d) = &self.data {
            d.want_job(job);
        }
    }

    fn header(&mut self, cx: &mut Cx) {
        let Some(f) = self.fleet.clone() else { return };
        let now = f.now_ms.max(1);
        let offset = local_offset_s(now);
        let link = match &f.link {
            Link::Connecting => "connecting to the master\u{2026}".to_string(),
            Link::Live => match f.reconnected_ms {
                Some(at) => format!("\u{25cf} live \u{b7} master back since {}", clock(at, offset, false)),
                None => "\u{25cf} live".to_string(),
            },
            Link::Lost { since_ms, why } => format!("master not answering for {} ({why}): redialling, the view holds", dur(wall_ms().saturating_sub(*since_ms))),
            Link::Older => "this master has no panel service yet: it comes with its next restart".to_string(),
            Link::Demo => "\u{25cf} demo fleet (synthetic, DEVHUB_PANEL_DEMO=1)".to_string(),
        };
        self.view.label(cx, ids!(panel_link)).set_text(cx, &link);
        let up = f.nodes.iter().filter(|n| n.up).count();
        let counts = if f.now_ms == 0 {
            String::new()
        } else {
            format!("{} nodes ({up} up) \u{b7} {} running \u{b7} {} waiting \u{b7} {}", f.nodes.len(), f.running(), f.queued().len(), clock(f.now_ms, offset, true))
        };
        self.view.label(cx, ids!(panel_counts)).set_text(cx, &counts);
        let follow = self.view.widget(cx, ids!(panel_timeline)).borrow::<FleetTimeline>().is_some_and(|t| t.follow());
        self.view.button(cx, ids!(panel_follow)).set_text(cx, if follow { "Following now" } else { "Follow now" });
    }

    fn timeline(&self, cx: &mut Cx, f: impl FnOnce(&mut FleetTimeline, &mut Cx)) {
        if let Some(mut t) = self.view.widget(cx, ids!(panel_timeline)).borrow_mut::<FleetTimeline>() {
            f(&mut t, cx);
        }
    }

    /// Opens the detail of `s` (None: closes it) and marks its need in the list.
    fn open_detail(&mut self, cx: &mut Cx, s: Option<Subject>) {
        let key = s.as_ref().and_then(|s| s.need.as_ref().map(Need::key));
        if let Some(mut n) = self.view.widget(cx, ids!(panel_needs)).borrow_mut::<NeedsList>() {
            n.select_need(cx, key);
        }
        if let Some(mut d) = self.view.widget(cx, ids!(panel_detail)).borrow_mut::<DetailPane>() {
            d.show_subject(cx, s);
        }
        self.want_output(cx);
        // the right column's layout changes: the whole panel draws again
        self.view.redraw(cx);
    }
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Widget for DevhubPanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.started {
            self.start(cx);
        }
        if let Event::Signal = event {
            self.take(cx);
        }
        self.widget_match_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
    }
}

impl WidgetMatchEvent for DevhubPanel {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        let tl = self.view.widget(cx, ids!(panel_timeline)).widget_uid();
        let nl = self.view.widget(cx, ids!(panel_needs)).widget_uid();
        let dp = self.view.widget(cx, ids!(panel_detail)).widget_uid();
        for a in actions.iter().filter_map(|a| a.as_widget_action()) {
            if a.widget_uid == tl {
                match a.cast() {
                    FleetTimelineAction::Unfollowed => self.header(cx),
                    FleetTimelineAction::Selected(Some(id)) => {
                        let need = self.needs.iter().find(|n| n.lease.as_deref() == Some(id.as_str())).cloned();
                        self.open_detail(cx, Some(Subject { lease: Some(id), need }));
                    }
                    FleetTimelineAction::Selected(None) => self.open_detail(cx, None),
                    FleetTimelineAction::None => {}
                }
            } else if a.widget_uid == nl {
                if let NeedsListAction::Picked(n) = a.cast() {
                    match (&n.lease, &n.node) {
                        (Some(l), _) => {
                            let l = l.clone();
                            self.timeline(cx, |t, cx| t.frame_lease(cx, &l));
                        }
                        (None, Some(node)) => {
                            let node = node.clone();
                            self.timeline(cx, |t, cx| {
                                t.clear_selection(cx);
                                t.reveal_node(cx, &node)
                            });
                        }
                        (None, None) => self.timeline(cx, |t, cx| t.clear_selection(cx)),
                    }
                    self.open_detail(cx, Some(Subject { lease: n.lease.clone(), need: Some(n) }));
                    self.header(cx);
                }
            } else if a.widget_uid == dp {
                if let DetailPaneAction::Closed = a.cast() {
                    self.timeline(cx, |t, cx| t.clear_selection(cx));
                    self.open_detail(cx, None);
                }
            }
        }
        for (id, ms) in [(ids!(win_15), 15.0), (ids!(win_60), 60.0), (ids!(win_180), 180.0)] {
            if self.view.button(cx, id).clicked(actions) {
                self.timeline(cx, |t, cx| t.set_window(cx, ms * 60_000.0));
                self.header(cx);
            }
        }
        if self.view.button(cx, ids!(panel_follow)).clicked(actions) {
            self.timeline(cx, |t, cx| t.set_follow(cx, true));
            self.header(cx);
        }
    }
}
