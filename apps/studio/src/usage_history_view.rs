use crate::usage::{now, AccountUsageHistory, AccountUsageSample, UsageProvider, UsageSnapshot, UsageWindow};
use makepad_widgets::{scroll_bars::ScrollBars, *};
use std::{collections::BTreeSet, sync::Arc};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.StudioUsageHistoryView = #(StudioUsageHistoryView::register_widget(vm)) {
        width: Fill height: Fill
        scroll_bars: mod.widgets.ScrollBars {show_scroll_x: false show_scroll_y: true}
        draw_text +: {text_style: theme.font_regular{font_size: 9.5} color: theme.color_text}
        background: theme.color_bg_container
        raised: theme.color_outset
        edge: theme.color_bevel_inset_2
        ink: theme.color_text
        muted: theme.color_text_disabled
        accent: theme.color_focus
    }
}

#[derive(Clone, Debug, Default)]
pub enum UsageHistoryAction {
    Changed,
    #[default]
    None,
}

struct AccountTarget {
    email: String,
    rect: Rect,
}

/// Read-only observations for known provider accounts. The selected CLI
/// identity comes from the latest query, never from an old history record.
#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct StudioUsageHistoryView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_marks: DrawVector,
    #[live]
    draw_text: DrawText,
    #[live]
    background: Vec4f,
    #[live]
    raised: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    muted: Vec4f,
    #[live]
    accent: Vec4f,
    #[rust]
    snapshot: Option<Arc<UsageSnapshot>>,
    #[rust]
    provider: UsageProvider,
    #[rust]
    expanded: BTreeSet<String>,
    #[rust]
    targets: Vec<AccountTarget>,
    #[rust]
    hovered: Option<String>,
    #[rust]
    pressed: Option<(String, DVec2)>,
}

const ACCOUNT_HEIGHT: f64 = 78.0;
const SAMPLE_HEIGHT: f64 = 46.0;
const SAMPLES_HEADING: f64 = 24.0;
const NOTE_HEIGHT: f64 = 34.0;
const MAX_ACCOUNTS: usize = 32;
const MAX_SAMPLES: usize = 32;

impl StudioUsageHistoryView {
    pub fn set_snapshot(&mut self, cx: &mut Cx, snapshot: Arc<UsageSnapshot>, provider: UsageProvider) {
        let provider_changed = self.provider != provider;
        if !provider_changed && self.snapshot.as_ref().is_some_and(|old| Arc::ptr_eq(old, &snapshot)) {
            return;
        }
        if provider_changed {
            self.expanded.clear();
            self.scroll_bars.set_scroll_pos(cx, dvec2(0.0, 0.0));
        }
        self.provider = provider;
        self.expanded.retain(|email| snapshot.account_history.iter()
            .any(|account| account.provider == provider && &account.email == email));
        self.snapshot = Some(snapshot);
        self.hovered = None;
        self.pressed = None;
        self.scroll_bars.redraw(cx);
    }

    /// Natural body height, excluding the popup title supplied by the host.
    pub fn preferred_height(&self) -> f64 {
        let Some(snapshot) = &self.snapshot else { return ACCOUNT_HEIGHT + NOTE_HEIGHT; };
        let mut count = 0;
        let mut height = 0.0;
        for account in snapshot.account_history.iter().filter(|account| account.provider == self.provider).take(MAX_ACCOUNTS) {
            count += 1;
            height += ACCOUNT_HEIGHT;
            if self.expanded.contains(&account.email) {
                height += SAMPLES_HEADING + account.samples.len().min(MAX_SAMPLES).max(1) as f64 * SAMPLE_HEIGHT;
            }
        }
        if count == 0 { height = ACCOUNT_HEIGHT; }
        height + NOTE_HEIGHT + if snapshot.history_error.is_some() { NOTE_HEIGHT } else { 0.0 }
    }

    fn text(&mut self, cx: &mut Cx2d, rect: Rect, value: &str, color: Vec4f, size: f32) {
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 { return; }
        self.draw_text.color = color;
        self.draw_text.text_style.font_size = size;
        let limit = (rect.size.x / (size as f64 * 0.45)).max(1.0) as usize;
        let mut chars = value.chars();
        let mut label: String = chars.by_ref().take(limit).collect();
        if chars.next().is_some() { label.push('…'); }
        while label.chars().count() > 1
            && self.draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), &label).size_in_lpxs.width as f64 > rect.size.x {
            if label.ends_with('…') { label.pop(); }
            label.pop();
            label.push('…');
        }
        cx.push_clip_rect(rect);
        self.draw_text.draw_abs(cx, rect.pos, &label);
        cx.pop_clip_rect();
    }

    fn quota(&mut self, cx: &mut Cx2d, rect: Rect, scope: &str, window: Option<&UsageWindow>, at: u64) {
        let used = window.and_then(|window| window.used_percent
            .or_else(|| window.remaining_percent.map(|remaining| 100.0 - remaining)))
            .filter(|value| value.is_finite() && (0.0..=100.0).contains(value));
        let color = match used {
            Some(value) if value >= 90.0 => vec4(0.95, 0.36, 0.32, 1.0),
            Some(value) if value >= 80.0 => vec4(0.91, 0.58, 0.22, 1.0),
            Some(_) => self.ink,
            None => self.muted,
        };
        self.text(cx, box_at(rect, 0.0, 0.0, 47.0, 16.0), scope, self.muted, 9.0);
        let value = used.map(|value| format!("{value:.0}% used")).unwrap_or_else(|| "Unknown".into());
        self.text(cx, box_at(rect, 48.0, 0.0, rect.size.x - 48.0, 16.0), &value, color, 9.5);
        let reset = window.map(|window| {
            let label = window.reset_label();
            let label = label.strip_prefix("Resets ").or_else(|| label.strip_prefix("resets "))
                .or_else(|| label.strip_prefix("Reset ")).or_else(|| label.strip_prefix("reset ")).unwrap_or(&label);
            if window.reset_at.is_some_and(|reset| reset <= at) {
                format!("Elapsed · {label}")
            } else if window.reset_at.is_some() || window.reset_text.is_some() {
                format!("Reset {label}")
            } else { "Reset unknown".into() }
        }).unwrap_or_else(|| "Reset unknown".into());
        self.text(cx, box_at(rect, 0.0, 17.0, rect.size.x, 16.0), &reset, self.muted, 8.5);
    }

    fn columns(&mut self, cx: &mut Cx2d, rect: Rect, sample: Option<&AccountUsageSample>, at: u64) {
        if self.provider == UsageProvider::Claude {
            let width = (rect.size.x - 12.0).max(0.0) * 0.5;
            self.quota(cx, box_at(rect, 0.0, 0.0, width, 34.0), "Session", sample.and_then(|sample| sample.session.as_ref()), at);
            self.quota(cx, box_at(rect, width + 12.0, 0.0, width, 34.0), "Week", sample.and_then(|sample| sample.week.as_ref()), at);
        } else {
            self.quota(cx, rect, "Week", sample.and_then(|sample| sample.week.as_ref()), at);
        }
    }

    fn account(&mut self, cx: &mut Cx2d, row: Rect, account: &AccountUsageHistory, current: bool, stale: bool, at: u64) {
        let open = self.expanded.contains(&account.email);
        if self.hovered.as_ref() == Some(&account.email) {
            self.draw_shape.color = self.raised;
            self.draw_shape.draw_abs(cx, row);
        }
        let color = if current { self.accent } else { self.muted };
        let center = row.pos + dvec2(12.0, 13.0);
        self.draw_marks.begin();
        self.draw_marks.set_color(color.x, color.y, color.z, color.w);
        if open {
            self.draw_marks.move_to((center.x - 3.0) as f32, (center.y - 1.5) as f32);
            self.draw_marks.line_to(center.x as f32, (center.y + 1.5) as f32);
            self.draw_marks.line_to((center.x + 3.0) as f32, (center.y - 1.5) as f32);
        } else {
            self.draw_marks.move_to((center.x - 1.5) as f32, (center.y - 3.0) as f32);
            self.draw_marks.line_to((center.x + 1.5) as f32, center.y as f32);
            self.draw_marks.line_to((center.x - 1.5) as f32, (center.y + 3.0) as f32);
        }
        self.draw_marks.stroke(1.2);
        self.draw_marks.end(cx);
        self.text(cx, box_at(row, 25.0, 5.0, row.size.x - 105.0, 17.0), &account.email, self.ink, 10.0);
        self.text(cx, box_at(row, row.size.x - 75.0, 6.0, 68.0, 16.0), if current { "Current" } else { "Last known" }, color, 9.0);
        self.columns(cx, box_at(row, 25.0, 27.0, row.size.x - 35.0, 34.0), account.latest(), at);
        let status = if !account.last_check_succeeded {
            " · check unavailable; last known"
        } else if stale { " · last known" } else { "" };
        let checked = format!("Checked {} · {}{}", timestamp(account.last_checked_at), age(account.last_checked_at, at), status);
        self.text(cx, box_at(row, 25.0, 61.0, row.size.x - 35.0, 15.0), &checked, self.muted, 8.5);
        self.draw_shape.color = self.edge;
        self.draw_shape.draw_abs(cx, box_at(row, 8.0, row.size.y - 1.0, row.size.x - 16.0, 0.5));
    }
}

impl WidgetNode for StudioUsageHistoryView {
    fn widget_uid(&self) -> WidgetUid { self.uid }
    fn walk(&mut self, _: &mut Cx) -> Walk { self.walk }
    fn area(&self) -> Area { self.scroll_bars.area() }
    fn redraw(&mut self, cx: &mut Cx) { self.scroll_bars.redraw(cx); }
}

impl Widget for StudioUsageHistoryView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.scroll_bars.catch_fling_on_press(cx, event) { self.pressed = None; return; }
        if !self.scroll_bars.handle_event(cx, event, scope).is_empty() {
            self.pressed = None;
            self.hovered = None;
        }
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(event) => {
                self.pressed = self.targets.iter().find(|target| target.rect.contains(event.abs))
                    .map(|target| (target.email.clone(), event.abs));
            }
            Hit::FingerUp(event) => {
                if let Some((email, start)) = self.pressed.take() {
                    if (event.abs - start).length() < 5.0 && self.targets.iter().any(|target| target.email == email && target.rect.contains(event.abs)) {
                        if !self.expanded.remove(&email) { self.expanded.insert(email); }
                        self.scroll_bars.redraw(cx);
                        cx.widget_action(self.uid, UsageHistoryAction::Changed);
                    }
                }
            }
            Hit::FingerHoverIn(event) | Hit::FingerHoverOver(event) => {
                let hovered = self.targets.iter().find(|target| target.rect.contains(event.abs)).map(|target| target.email.clone());
                cx.set_cursor(if hovered.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
                if hovered != self.hovered { self.hovered = hovered; self.scroll_bars.redraw(cx); }
            }
            Hit::FingerHoverOut(_) => {
                if self.hovered.take().is_some() { self.scroll_bars.redraw(cx); }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _: &mut Scope, walk: Walk) -> DrawStep {
        self.scroll_bars.begin(cx, walk, Layout::flow_down());
        let viewport = cx.turtle().rect_unscrolled();
        let width = (viewport.size.x - 10.0).max(0.0);
        self.draw_shape.color = self.background;
        self.draw_shape.draw_abs(cx, viewport);
        self.targets.clear();
        cx.push_clip_rect(viewport);
        let at = now();
        let snapshot = self.snapshot.clone();
        let provider = self.provider;
        let current = snapshot.as_ref().and_then(|snapshot| snapshot.provider(provider));
        let mut count = 0;
        if let Some(snapshot) = &snapshot {
            for account in snapshot.account_history.iter().filter(|account| account.provider == provider).take(MAX_ACCOUNTS) {
                count += 1;
                let row = cx.walk_turtle(Walk::new(Size::Fixed(width), Size::Fixed(ACCOUNT_HEIGHT)));
                if row.pos.y < viewport.pos.y + viewport.size.y && row.pos.y + row.size.y > viewport.pos.y {
                    let is_current = current.and_then(|provider| provider.account_email.as_deref()) == Some(account.email.as_str());
                    let stale = !is_current || current.is_none_or(|provider| provider.is_stale(at));
                    self.account(cx, row, account, is_current, stale, at);
                    self.targets.push(AccountTarget { email: account.email.clone(), rect: row });
                }
                if self.expanded.contains(&account.email) {
                    let heading = cx.walk_turtle(Walk::new(Size::Fixed(width), Size::Fixed(SAMPLES_HEADING)));
                    self.text(cx, box_at(heading, 25.0, 6.0, width - 35.0, 16.0), "Recent observations · newest first", self.muted, 9.0);
                    if account.samples.is_empty() {
                        let row = cx.walk_turtle(Walk::new(Size::Fixed(width), Size::Fixed(SAMPLE_HEIGHT)));
                        self.text(cx, box_at(row, 25.0, 6.0, width - 35.0, 16.0), "No successful quota observation", self.muted, 9.0);
                    }
                    for sample in account.samples.iter().rev().take(MAX_SAMPLES) {
                        let row = cx.walk_turtle(Walk::new(Size::Fixed(width), Size::Fixed(SAMPLE_HEIGHT)));
                        if row.pos.y >= viewport.pos.y + viewport.size.y || row.pos.y + row.size.y <= viewport.pos.y { continue; }
                        // Timestamp remains separate from both quota scopes.
                        let stamp_width = (width * 0.27).clamp(100.0, 140.0);
                        let stamp = timestamp(sample.observed_at);
                        let (date, time) = stamp.split_once(' ').unwrap_or((&stamp, ""));
                        self.text(cx, box_at(row, 25.0, 6.0, stamp_width - 30.0, 16.0), date, self.muted, 8.5);
                        self.text(cx, box_at(row, 25.0, 23.0, stamp_width - 30.0, 16.0), time, self.muted, 8.5);
                        self.columns(cx, box_at(row, stamp_width, 6.0, width - stamp_width - 10.0, 34.0), Some(sample), at);
                    }
                }
            }
        }
        if count == 0 {
            let row = cx.walk_turtle(Walk::new(Size::Fixed(width), Size::Fixed(ACCOUNT_HEIGHT)));
            let label = if current.and_then(|provider| provider.account_email.as_ref()).is_some() {
                "No stored quota observations for this provider yet"
            } else { "No known account history yet" };
            self.text(cx, box_at(row, 12.0, 16.0, width - 24.0, 18.0), label, self.ink, 10.0);
        }
        if let Some(error) = snapshot.as_ref().and_then(|snapshot| snapshot.history_error.as_deref()) {
            let row = cx.walk_turtle(Walk::new(Size::Fixed(width), Size::Fixed(NOTE_HEIGHT)));
            self.text(cx, box_at(row, 12.0, 7.0, width - 24.0, 16.0), &format!("History unavailable: {error}"), self.muted, 9.0);
        }
        let note = cx.walk_turtle(Walk::new(Size::Fixed(width), Size::Fixed(NOTE_HEIGHT)));
        self.text(cx, box_at(note, 12.0, 7.0, width - 24.0, 16.0), "Elapsed resets keep their last-known reading until checked again.", self.muted, 8.5);
        cx.pop_clip_rect();
        self.scroll_bars.end(cx);
        DrawStep::done()
    }
}

fn box_at(parent: Rect, x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect { pos: parent.pos + dvec2(x, y), size: dvec2(width.max(0.0), height.max(0.0)) }
}

fn timestamp(at: u64) -> String {
    if at == 0 || at > 253_402_300_799 { "Unknown".into() }
    else { crate::usage::format_reset_at(at) }
}

fn age(checked: u64, at: u64) -> String {
    if checked == 0 { return "not checked".into(); }
    if checked > at { return "timestamp ahead".into(); }
    let seconds = at.saturating_sub(checked);
    if seconds < 60 { "just now".into() }
    else if seconds < 3600 { format!("{}m ago", seconds / 60) }
    else if seconds < 86_400 { format!("{}h ago", seconds / 3600) }
    else { format!("{}d ago", seconds / 86_400) }
}
