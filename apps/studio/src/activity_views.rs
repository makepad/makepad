//! Three spatial views over one deterministic activity replay. These previews
//! exercise the oversight UI; they do not impersonate live agent telemetry.

use crate::activity_demo::{
    self, AgentSnapshot, AgentState, AppSnapshot, AppStage, DemoEvent, DemoSnapshot, EventKind,
    Layout as DemoLayout,
};
use crate::canvas_draw::DrawCanvasCard;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.StudioActivityViews = #(StudioActivityViews::register_widget(vm)){
        width: Fill height: Fill
        background: theme.color_bg_app
        surface: mix(theme.color_bg_app, theme.color_text, 0.035)
        raised: mix(theme.color_bg_app, theme.color_text, 0.075)
        edge: mix(theme.color_bg_app, theme.color_text, 0.18)
        ink: theme.color_text
        muted: mix(theme.color_bg_app, theme.color_text, 0.57)
        accent: theme.color_focus
        draw_card +: {shadow_color: #0002}
        draw_title +: {text_style: theme.font_bold{font_size: 14} color: theme.color_text}
        draw_text +: {text_style: theme.font_regular{font_size: 12} color: theme.color_text}
        draw_code +: {text_style: theme.font_code{font_size: 12} color: theme.color_text}
    }
}

#[derive(Clone, Debug, Default)]
pub enum ActivityViewAction {
    Selected(String),
    #[default]
    None,
}

struct Target {
    id: String,
    rect: Rect,
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct StudioActivityViews {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_card: DrawCanvasCard,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_code: DrawText,
    #[live]
    draw_links: DrawVector,
    #[live]
    background: Vec4f,
    #[live]
    surface: Vec4f,
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
    area: Area,
    #[rust]
    viewport: Rect,
    #[rust]
    layout: DemoLayout,
    #[rust]
    at: f64,
    #[rust]
    zoom: f64,
    #[rust]
    pan: DVec2,
    #[rust]
    needs_fit: bool,
    #[rust]
    fitted: bool,
    #[rust]
    selected: Option<String>,
    #[rust]
    hovered: Option<String>,
    #[rust]
    targets: Vec<Target>,
    #[rust]
    drag: Option<(DVec2, DVec2)>,
    #[rust]
    pressed: Option<String>,
}

const AGENTS: [&str; 5] = ["astra", "fable", "a11y", "api", "qa"];

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        pos: dvec2(x, y),
        size: dvec2(w, h),
    }
}

fn blend(a: Vec4f, b: Vec4f, amount: f32) -> Vec4f {
    vec4(
        a.x + (b.x - a.x) * amount,
        a.y + (b.y - a.y) * amount,
        a.z + (b.z - a.z) * amount,
        1.0,
    )
}

fn agent_color(id: &str) -> Vec4f {
    match id {
        "astra" => vec4(0.56, 0.53, 0.91, 1.0),
        "fable" => vec4(0.22, 0.69, 0.69, 1.0),
        "a11y" => vec4(0.36, 0.70, 0.48, 1.0),
        "api" => vec4(0.79, 0.59, 0.30, 1.0),
        _ => vec4(0.47, 0.63, 0.88, 1.0),
    }
}

fn state_label(state: AgentState) -> &'static str {
    match state {
        AgentState::Waiting => "WAITING",
        AgentState::Working => "WORKING",
        AgentState::Testing => "TESTING",
        AgentState::Blocked => "BLOCKED",
        AgentState::Done => "DONE",
    }
}

fn stage_label(stage: AppStage) -> &'static str {
    match stage {
        AppStage::Starting => "STARTING",
        AppStage::Loading => "LOADING",
        AppStage::Broken => "FAILING",
        AppStage::Fixed => "FIX APPLIED",
        AppStage::Verified => "VERIFIED",
    }
}

fn clock_label(at: f64) -> String {
    let seconds = at.max(0.0) as u64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn short(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let mut out: String = value.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

impl StudioActivityViews {
    pub fn set_view(&mut self, cx: &mut Cx, layout: DemoLayout, at: f64) {
        let at = if at.is_finite() {
            at.clamp(0.0, activity_demo::DURATION)
        } else {
            0.0
        };
        if self.layout != layout {
            self.layout = layout;
            self.needs_fit = true;
        }
        if self.at != at || self.needs_fit {
            self.at = at;
            self.area.redraw(cx);
        }
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn fit(&mut self, cx: &mut Cx) {
        self.needs_fit = true;
        self.area.redraw(cx);
    }

    fn scene_size(&self) -> DVec2 {
        match self.layout {
            DemoLayout::AgentLanes => dvec2(1400.0, 842.0),
            DemoLayout::Timeline => dvec2(1500.0, 816.0),
            DemoLayout::SystemLanes => dvec2(1440.0, 864.0),
        }
    }

    fn screen(&self, r: Rect) -> Rect {
        Rect {
            pos: self.viewport.pos + self.pan + r.pos * self.zoom,
            size: r.size * self.zoom,
        }
    }

    fn point(&self, p: DVec2) -> DVec2 {
        self.viewport.pos + self.pan + p * self.zoom
    }

    fn color(&self, color: Vec4f) -> Vec4f {
        vec4(color.x, color.y, color.z, 1.0)
    }

    fn fill(&mut self, cx: &mut Cx2d, r: Rect, color: Vec4f) {
        self.draw_shape.color = self.color(color);
        let screen = self.screen(r);
        self.draw_shape.draw_abs(cx, screen);
    }

    fn card(&mut self, cx: &mut Cx2d, r: Rect, id: Option<&str>, color: Vec4f) {
        let selected = id.is_some() && id == self.selected.as_deref();
        let hovered = id.is_some() && id == self.hovered.as_deref();
        self.draw_card.color = if hovered {
            blend(color, self.ink, 0.035)
        } else {
            self.color(color)
        };
        self.draw_card.border_color = if selected { self.accent } else { self.edge };
        self.draw_card.border_size = if selected { 1.7 } else { 0.8 };
        self.draw_card.border_radius = (4.0 * self.zoom) as f32;
        self.draw_card.outline_size = 0.0;
        self.draw_card.shadow_radius = (8.0 * self.zoom) as f32;
        self.draw_card.shadow_offset = vec2(0.0, (2.0 * self.zoom) as f32);
        let screen = self.screen(r);
        self.draw_card.draw_abs(cx, screen);
        if let Some(id) = id {
            self.targets.push(Target {
                id: id.to_owned(),
                rect: r,
            });
        }
    }

    fn text(&mut self, cx: &mut Cx2d, r: Rect, text: &str, size: f64, color: Vec4f, weight: u8) {
        if r.size.x <= 0.0 {
            return;
        }
        let screen = self.screen(r);
        if screen.pos.y + screen.size.y < self.viewport.pos.y
            || screen.pos.y > self.viewport.pos.y + self.viewport.size.y
        {
            return;
        }
        let text = short(
            text,
            (r.size.x / (size * if weight == 2 { 0.61 } else { 0.57 }))
                .floor()
                .max(1.0) as usize,
        );
        let draw = match weight {
            1 => &mut self.draw_title,
            2 => &mut self.draw_code,
            _ => &mut self.draw_text,
        };
        draw.color = color;
        draw.text_style.font_size = (size * self.zoom) as f32;
        draw.font_scale = 1.0;
        cx.push_clip_rect(screen);
        draw.draw_abs(cx, screen.pos, &text);
        cx.pop_clip_rect();
    }

    fn wrapped(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        text: &str,
        size: f64,
        color: Vec4f,
        lines: usize,
    ) {
        let limit = (r.size.x / (size * 0.56)).floor().max(1.0) as usize;
        let words: Vec<_> = text.split_whitespace().collect();
        let mut at = 0;
        for line in 0..lines {
            if at >= words.len() {
                break;
            }
            let mut text = String::new();
            while at < words.len() && (text.len() + words[at].len() + 1 <= limit || text.is_empty())
            {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(words[at]);
                at += 1;
            }
            if line + 1 == lines && at < words.len() {
                text.push('…');
            }
            self.text(
                cx,
                rect(
                    r.pos.x,
                    r.pos.y + line as f64 * (size + 4.0),
                    r.size.x,
                    size + 5.0,
                ),
                &text,
                size,
                color,
                0,
            );
        }
    }

    fn badge(&mut self, cx: &mut Cx2d, x: f64, y: f64, label: &str, color: Vec4f) {
        // Font sizes are points; character counts are not layout pixels. Use
        // the same measured run for the badge bounds and the visible label.
        self.draw_title.text_style.font_size = (10.0 * self.zoom) as f32;
        self.draw_title.font_scale = 1.0;
        self.draw_title.color = blend(self.ink, color, 0.38);
        let measured = self
            .draw_title
            .layout(cx, 0.0, 0.0, None, false, Align::default(), label);
        let text_size = dvec2(
            measured.size_in_lpxs.width as f64,
            measured.size_in_lpxs.height as f64,
        );
        let w = (text_size.x / self.zoom + 16.0).max(32.0);
        let h = (text_size.y / self.zoom + 6.0).max(20.0);
        let bounds = rect(x, y, w, h);
        self.fill(cx, bounds, blend(self.surface, color, 0.15));
        let screen = self.screen(bounds);
        let origin = screen.pos + dvec2(8.0 * self.zoom, (screen.size.y - text_size.y) * 0.5);
        cx.push_clip_rect(screen);
        self.draw_title.draw_abs(cx, origin, label);
        cx.pop_clip_rect();
    }

    fn role_label(&mut self, cx: &mut Cx2d, r: Rect, label: &str) {
        let screen = self.screen(r);
        self.draw_text.text_style.font_size = (10.5 * self.zoom) as f32;
        self.draw_text.font_scale = 1.0;
        self.draw_text.color = self.muted;
        let measure = |draw: &DrawText, cx: &mut Cx2d, text: &str| {
            draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text)
                .size_in_lpxs
                .width as f64
        };
        let mut label = label.to_owned();
        if measure(&self.draw_text, cx, &label) > screen.size.x {
            let chars: Vec<_> = label.chars().collect();
            let (mut lo, mut hi) = (0, chars.len());
            while lo < hi {
                let mid = (lo + hi + 1) / 2;
                let candidate = chars[..mid].iter().collect::<String>() + "…";
                if measure(&self.draw_text, cx, &candidate) <= screen.size.x {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            label = chars[..lo].iter().collect::<String>() + "…";
        }
        cx.push_clip_rect(screen);
        self.draw_text.draw_abs(cx, screen.pos, &label);
        cx.pop_clip_rect();
    }

    fn state_color(&self, state: AgentState) -> Vec4f {
        match state {
            AgentState::Blocked => vec4(0.85, 0.36, 0.36, 1.0),
            AgentState::Done => vec4(0.30, 0.69, 0.45, 1.0),
            AgentState::Testing => vec4(0.80, 0.60, 0.29, 1.0),
            AgentState::Working => self.accent,
            AgentState::Waiting => self.muted,
        }
    }

    fn event_color(&self, event: &DemoEvent) -> Vec4f {
        match event.kind {
            EventKind::Issue => vec4(0.85, 0.36, 0.36, 1.0),
            EventKind::Test if event.title.starts_with("FAIL") => vec4(0.85, 0.36, 0.36, 1.0),
            EventKind::Finish => vec4(0.30, 0.69, 0.45, 1.0),
            _ => agent_color(event.agent),
        }
    }

    fn diamond(&mut self, cx: &mut Cx2d, center: DVec2, radius: f64, color: Vec4f) {
        let center = self.point(center);
        let radius = (radius * self.zoom) as f32;
        let x = center.x as f32;
        let y = center.y as f32;
        self.draw_links.begin();
        self.draw_links.set_color(color.x, color.y, color.z, 1.0);
        self.draw_links.move_to(x, y - radius);
        self.draw_links.line_to(x + radius, y);
        self.draw_links.line_to(x, y + radius);
        self.draw_links.line_to(x - radius, y);
        self.draw_links.close();
        self.draw_links.fill();
        self.draw_links.end(cx);
    }

    fn connector(&mut self, cx: &mut Cx2d, from: DVec2, to: DVec2, color: Vec4f, vertical: bool) {
        let a = self.point(from);
        let b = self.point(to);
        let bend = if vertical {
            (b.y - a.y) * 0.5
        } else {
            (b.x - a.x) * 0.5
        };
        self.draw_links.begin();
        self.draw_links.set_color(color.x, color.y, color.z, 0.62);
        self.draw_links.move_to(a.x as f32, a.y as f32);
        if vertical {
            self.draw_links.bezier_to(
                a.x as f32,
                (a.y + bend) as f32,
                b.x as f32,
                (b.y - bend) as f32,
                b.x as f32,
                b.y as f32,
            );
        } else {
            self.draw_links.bezier_to(
                (a.x + bend) as f32,
                a.y as f32,
                (b.x - bend) as f32,
                b.y as f32,
                b.x as f32,
                b.y as f32,
            );
        }
        self.draw_links.stroke((1.2 * self.zoom).max(0.8) as f32);
        self.draw_links.end(cx);
    }

    fn header(&mut self, cx: &mut Cx2d, subtitle: &str) {
        self.text(
            cx,
            rect(20.0, 14.0, 650.0, 32.0),
            self.layout.label(),
            23.0,
            self.ink,
            1,
        );
        self.text(
            cx,
            rect(20.0, 48.0, 1080.0, 22.0),
            subtitle,
            12.0,
            self.muted,
            0,
        );
        let width = self.scene_size().x;
        self.badge(
            cx,
            width - 168.0,
            21.0,
            &format!("REPLAY {}", clock_label(self.at)),
            self.accent,
        );
    }

    fn agent<'a>(&self, snapshot: &'a DemoSnapshot, id: &str) -> Option<&'a AgentSnapshot> {
        snapshot.agents.iter().find(|agent| agent.id == id)
    }

    fn selected_agent<'a>(&self, snapshot: &'a DemoSnapshot) -> Option<&'a AgentSnapshot> {
        let id = self.selected.as_deref().unwrap_or("fable");
        let owner = snapshot
            .events
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.agent)
            .or_else(|| snapshot.apps.iter().find(|a| a.id == id).map(|a| a.owner))
            .unwrap_or(id);
        self.agent(snapshot, owner)
            .or_else(|| self.agent(snapshot, "fable"))
            .or_else(|| snapshot.agents.first())
    }

    fn code(&mut self, cx: &mut Cx2d, r: Rect, agent: &AgentSnapshot, snapshot: &DemoSnapshot) {
        let event_id = snapshot
            .events
            .iter()
            .rev()
            .find(|e| e.agent == agent.id && matches!(e.kind, EventKind::Edit))
            .map(|e| e.id)
            .unwrap_or(agent.id);
        self.card(cx, r, Some(event_id), self.surface);
        self.fill(
            cx,
            rect(r.pos.x + 1.0, r.pos.y + 1.0, r.size.x - 2.0, 26.0),
            self.raised,
        );
        self.text(
            cx,
            rect(r.pos.x + 11.0, r.pos.y + 8.0, r.size.x - 22.0, 17.0),
            agent.file.unwrap_or("Task plan / delegation"),
            11.5,
            self.ink,
            2,
        );
        if agent.diff.is_empty() {
            self.wrapped(
                cx,
                rect(
                    r.pos.x + 12.0,
                    r.pos.y + 39.0,
                    r.size.x - 24.0,
                    r.size.y - 42.0,
                ),
                agent.task,
                12.0,
                self.muted,
                3,
            );
            return;
        }
        let rows = ((r.size.y - 35.0) / 17.0).floor().max(1.0) as usize;
        for (i, line) in agent.diff.iter().take(rows).enumerate() {
            let y = r.pos.y + 34.0 + i as f64 * 17.0;
            let color = if line.starts_with('+') {
                vec4(0.33, 0.75, 0.48, 1.0)
            } else if line.starts_with('-') {
                vec4(0.89, 0.41, 0.42, 1.0)
            } else {
                self.muted
            };
            if line.starts_with('+') || line.starts_with('-') {
                self.fill(
                    cx,
                    rect(r.pos.x + 2.0, y - 1.0, r.size.x - 4.0, 17.0),
                    blend(self.surface, color, 0.13),
                );
            }
            self.text(
                cx,
                rect(r.pos.x + 11.0, y + 1.0, r.size.x - 22.0, 16.0),
                line,
                11.5,
                blend(self.ink, color, 0.40),
                2,
            );
        }
    }

    fn terminal(&mut self, cx: &mut Cx2d, r: Rect, agent: &AgentSnapshot) {
        self.card(
            cx,
            r,
            Some(agent.id),
            blend(self.background, self.surface, 0.3),
        );
        self.fill(
            cx,
            rect(r.pos.x + 10.0, r.pos.y + 11.0, 5.0, 5.0),
            agent_color(agent.id),
        );
        self.text(
            cx,
            rect(r.pos.x + 22.0, r.pos.y + 8.0, r.size.x - 33.0, 18.0),
            &format!("{} / terminal", agent.name),
            11.5,
            self.muted,
            1,
        );
        let rows = ((r.size.y - 36.0) / 17.0).floor().max(1.0) as usize;
        for (i, line) in agent
            .terminal
            .iter()
            .rev()
            .take(rows)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .enumerate()
        {
            let color =
                if line.contains("FAIL") || line.contains("Error") || line.contains("error:") {
                    vec4(0.89, 0.41, 0.42, 1.0)
                } else if line.starts_with('$') || line.starts_with('>') || line.contains("PASS") {
                    blend(self.ink, agent_color(agent.id), 0.45)
                } else {
                    self.muted
                };
            self.text(
                cx,
                rect(
                    r.pos.x + 11.0,
                    r.pos.y + 33.0 + i as f64 * 17.0,
                    r.size.x - 22.0,
                    17.0,
                ),
                line,
                11.5,
                color,
                2,
            );
        }
    }

    fn app(&mut self, cx: &mut Cx2d, r: Rect, app: &AppSnapshot) {
        self.card(cx, r, Some(app.id), self.surface);
        let bad = matches!(app.stage, AppStage::Broken);
        let ready = matches!(app.stage, AppStage::Fixed | AppStage::Verified);
        let stage = if bad {
            vec4(0.87, 0.37, 0.38, 1.0)
        } else if ready {
            vec4(0.30, 0.69, 0.45, 1.0)
        } else {
            self.muted
        };
        self.fill(
            cx,
            rect(r.pos.x + 1.0, r.pos.y + 1.0, r.size.x - 2.0, 25.0),
            self.raised,
        );
        for i in 0..3 {
            self.fill(
                cx,
                rect(r.pos.x + 10.0 + i as f64 * 7.0, r.pos.y + 11.0, 3.0, 3.0),
                self.muted,
            );
        }
        self.text(
            cx,
            rect(r.pos.x + 38.0, r.pos.y + 7.0, r.size.x - 132.0, 17.0),
            app.route,
            10.0,
            self.muted,
            2,
        );
        self.text(
            cx,
            rect(r.pos.x + r.size.x - 96.0, r.pos.y + 7.0, 88.0, 17.0),
            stage_label(app.stage),
            9.0,
            stage,
            1,
        );
        let compact = r.size.y < 150.0;
        let body = rect(
            r.pos.x + 11.0,
            r.pos.y + 33.0,
            r.size.x - 22.0,
            r.size.y - if compact { 43.0 } else { 70.0 },
        );
        cx.push_clip_rect(self.screen(body));
        if matches!(app.stage, AppStage::Starting)
            || (matches!(app.stage, AppStage::Loading) && app.frame < 2)
        {
            self.text(
                cx,
                rect(body.pos.x, body.pos.y, body.size.x, 22.0),
                app.title,
                14.0,
                self.ink,
                1,
            );
            for i in 0..3 {
                self.fill(
                    cx,
                    rect(
                        body.pos.x,
                        body.pos.y + 30.0 + i as f64 * 13.0,
                        body.size.x * (0.80 - i as f64 * 0.16),
                        5.0,
                    ),
                    blend(self.surface, self.muted, 0.18),
                );
            }
            if !compact {
                self.wrapped(
                    cx,
                    rect(body.pos.x, body.pos.y + 87.0, body.size.x, 44.0),
                    app.caption,
                    11.0,
                    self.muted,
                    2,
                );
            }
            cx.pop_clip_rect();
            return;
        }
        match app.id {
            "checkout" => self.checkout(cx, body, compact, app),
            "admin" => self.admin(cx, body, compact, app),
            _ => self.test_runner(cx, body, compact, app),
        }
        cx.pop_clip_rect();
        if !compact {
            self.fill(
                cx,
                rect(
                    r.pos.x + 10.0,
                    r.pos.y + r.size.y - 30.0,
                    r.size.x - 20.0,
                    1.0,
                ),
                self.edge,
            );
            self.text(
                cx,
                rect(
                    r.pos.x + 11.0,
                    r.pos.y + r.size.y - 22.0,
                    r.size.x - 22.0,
                    17.0,
                ),
                &format!("frame {} · {}", app.frame, app.caption),
                9.5,
                self.muted,
                0,
            );
        }
    }

    fn checkout(&mut self, cx: &mut Cx2d, b: Rect, compact: bool, app: &AppSnapshot) {
        let product = vec4(0.79, 0.61, 0.34, 1.0);
        let bad = matches!(app.stage, AppStage::Broken);
        let pending = app.frame == 5;
        let confirmed = app.frame >= 6;
        let total = if app.frame == 2 {
            "€91.00"
        } else {
            "€84.00"
        };
        self.text(
            cx,
            rect(b.pos.x, b.pos.y, b.size.x, 22.0),
            "ORBIT / checkout",
            if compact { 12.0 } else { 16.0 },
            self.ink,
            1,
        );
        let y = b.pos.y + if compact { 22.0 } else { 29.0 };
        self.fill(
            cx,
            rect(b.pos.x, y, 37.0, 34.0),
            blend(self.surface, product, 0.17),
        );
        // A small lamp silhouette, drawn as application content.
        self.fill(cx, rect(b.pos.x + 9.0, y + 6.0, 19.0, 10.0), product);
        self.fill(cx, rect(b.pos.x + 17.0, y + 16.0, 3.0, 12.0), product);
        self.fill(cx, rect(b.pos.x + 11.0, y + 27.0, 15.0, 3.0), product);
        self.text(
            cx,
            rect(b.pos.x + 48.0, y + 1.0, b.size.x - 132.0, 18.0),
            "Orbit lamp",
            11.5,
            self.ink,
            1,
        );
        self.text(
            cx,
            rect(b.pos.x + 48.0, y + 18.0, b.size.x - 132.0, 17.0),
            "Amsterdam · Qty 1",
            10.0,
            self.muted,
            0,
        );
        self.text(
            cx,
            rect(b.pos.x + b.size.x - 80.0, y + 1.0, 80.0, 20.0),
            total,
            12.0,
            if bad {
                vec4(0.89, 0.40, 0.40, 1.0)
            } else {
                self.ink
            },
            1,
        );
        if compact {
            self.fill(
                cx,
                rect(b.pos.x + b.size.x - 115.0, y + 22.0, 115.0, 22.0),
                if pending {
                    self.raised
                } else {
                    agent_color("fable")
                },
            );
            self.text(
                cx,
                rect(b.pos.x + b.size.x - 108.0, y + 27.0, 102.0, 15.0),
                if confirmed {
                    "Order confirmed"
                } else if pending {
                    "Updating total…"
                } else {
                    "Pay now →"
                },
                9.5,
                if pending {
                    self.muted
                } else {
                    vec4(1.0, 1.0, 1.0, 1.0)
                },
                1,
            );
            if bad {
                self.text(
                    cx,
                    rect(b.pos.x, y + 39.0, b.size.x - 125.0, 16.0),
                    if app.frame == 2 {
                        "Stale Utrecht quote"
                    } else {
                        "Pay enabled too early"
                    },
                    9.5,
                    vec4(0.89, 0.40, 0.40, 1.0),
                    0,
                );
            }
            return;
        }
        self.fill(
            cx,
            rect(b.pos.x, b.pos.y + 75.0, b.size.x, 24.0),
            self.raised,
        );
        self.text(
            cx,
            rect(b.pos.x + 8.0, b.pos.y + 82.0, b.size.x - 16.0, 16.0),
            if confirmed {
                "Order orbit-1042 · one payment"
            } else if pending {
                "Shipping address · updating quote…"
            } else {
                "Shipping to Amsterdam  ·  Edit address"
            },
            10.0,
            self.muted,
            0,
        );
        self.fill(
            cx,
            rect(b.pos.x, b.pos.y + 108.0, b.size.x, 27.0),
            if pending {
                self.raised
            } else {
                agent_color("fable")
            },
        );
        let button = if confirmed {
            "Order confirmed · €84.00"
        } else if pending {
            "Pay unavailable · quote pending"
        } else if app.frame == 2 {
            "Pay €91.00 →"
        } else {
            "Pay €84.00 →"
        };
        self.text(
            cx,
            rect(b.pos.x + 10.0, b.pos.y + 115.0, b.size.x - 20.0, 18.0),
            button,
            11.0,
            if pending {
                self.muted
            } else {
                vec4(1.0, 1.0, 1.0, 1.0)
            },
            1,
        );
        if b.size.y > 156.0 {
            self.text(
                cx,
                rect(b.pos.x, b.pos.y + 144.0, b.size.x, 18.0),
                if bad {
                    "Address and payment state disagree"
                } else if confirmed {
                    "Confirmation matches the order inspector"
                } else {
                    "Latest address quote · protected payment"
                },
                9.5,
                if bad {
                    vec4(0.89, 0.40, 0.40, 1.0)
                } else {
                    self.muted
                },
                0,
            );
        }
    }

    fn admin(&mut self, cx: &mut Cx2d, b: Rect, compact: bool, app: &AppSnapshot) {
        let bad = matches!(app.stage, AppStage::Broken);
        let verified = matches!(app.stage, AppStage::Verified);
        self.text(
            cx,
            rect(b.pos.x, b.pos.y, b.size.x, 22.0),
            "Orders / payment attempts",
            if compact { 12.0 } else { 16.0 },
            self.ink,
            1,
        );
        let y = b.pos.y + if compact { 25.0 } else { 34.0 };
        self.fill(cx, rect(b.pos.x, y, b.size.x, 19.0), self.raised);
        self.text(
            cx,
            rect(b.pos.x + 6.0, y + 4.0, b.size.x * 0.58, 15.0),
            "BASKET / ATTEMPT",
            8.5,
            self.muted,
            1,
        );
        self.text(
            cx,
            rect(b.pos.x + b.size.x * 0.63, y + 4.0, b.size.x * 0.36, 15.0),
            "PAYMENT",
            8.5,
            self.muted,
            1,
        );
        let rows = if compact {
            1
        } else if bad {
            2
        } else {
            1
        };
        for i in 0..rows {
            let row_y = y + 25.0 + i as f64 * 28.0;
            self.text(
                cx,
                rect(b.pos.x + 6.0, row_y, b.size.x * 0.61, 19.0),
                if verified {
                    "orbit-1042 / payment-17"
                } else if i == 0 {
                    "orbit-42 / payment-17"
                } else {
                    "orbit-42 / payment-18"
                },
                10.0,
                self.ink,
                0,
            );
            self.text(
                cx,
                rect(b.pos.x + b.size.x * 0.63, row_y, b.size.x * 0.35, 19.0),
                if verified { "Paid €84" } else { "Pending" },
                10.0,
                if bad {
                    vec4(0.89, 0.40, 0.40, 1.0)
                } else {
                    agent_color("a11y")
                },
                1,
            );
            if !compact {
                self.fill(cx, rect(b.pos.x, row_y + 20.0, b.size.x, 0.7), self.edge);
            }
        }
        if !compact {
            self.wrapped(
                cx,
                rect(b.pos.x, b.pos.y + 118.0, b.size.x, 44.0),
                if bad {
                    "Duplicate: two attempts for basket revision 7"
                } else if verified {
                    "One settled order · €84.00 · verified"
                } else if app.frame == 3 {
                    "Repeated submission reuses payment-17; retest pending"
                } else {
                    "Applying atomic attempt creation…"
                },
                10.5,
                self.muted,
                2,
            );
        }
    }

    fn test_runner(&mut self, cx: &mut Cx2d, b: Rect, compact: bool, app: &AppSnapshot) {
        self.text(
            cx,
            rect(b.pos.x, b.pos.y, b.size.x, 22.0),
            "Checks / checkout",
            if compact { 12.0 } else { 16.0 },
            self.ink,
            1,
        );
        let tests = [
            "latest address total",
            "Pay waits for quote",
            "keyboard recovery",
            "one payment attempt",
        ];
        for (i, name) in tests.iter().take(if compact { 2 } else { 4 }).enumerate() {
            let y = b.pos.y
                + if compact { 25.0 } else { 33.0 }
                + i as f64 * if compact { 18.0 } else { 23.0 };
            let failed = (app.frame == 1 && i == 0) || (app.frame == 3 && i == 1);
            let passed = app.frame >= 6
                || (app.frame >= 5 && i < 3)
                || (app.frame >= 4 && i == 2)
                || (app.frame >= 3 && i == 0);
            let color = if failed {
                vec4(0.89, 0.40, 0.40, 1.0)
            } else if passed {
                agent_color("a11y")
            } else {
                self.muted
            };
            self.fill(cx, rect(b.pos.x, y + 2.0, 7.0, 7.0), color);
            self.text(
                cx,
                rect(b.pos.x + 15.0, y, b.size.x - 15.0, 18.0),
                &format!(
                    "{}  {}",
                    if failed {
                        "FAIL"
                    } else if passed {
                        "PASS"
                    } else {
                        "WAIT"
                    },
                    name
                ),
                10.0,
                if failed { color } else { self.ink },
                2,
            );
        }
        if !compact {
            self.text(
                cx,
                rect(b.pos.x, b.pos.y + 135.0, b.size.x, 20.0),
                if app.frame == 1 {
                    "expected €84.00 · received €91.00"
                } else if app.frame == 3 {
                    "Pay must be disabled while quote is pending"
                } else if app.frame >= 7 {
                    "6 browser + 12 API + keyboard · verified"
                } else if app.frame >= 6 {
                    "6 browser + 12 API · smoke test pending"
                } else if app.frame >= 5 {
                    "6/6 browser · service checks pending"
                } else {
                    "Regression run in progress"
                },
                9.5,
                self.muted,
                2,
            );
        }
    }

    fn history_strip(
        &mut self,
        cx: &mut Cx2d,
        snapshot: &DemoSnapshot,
        agent: &AgentSnapshot,
        r: Rect,
    ) {
        self.fill(cx, rect(r.pos.x, r.pos.y + 5.0, r.size.x, 1.0), self.edge);
        for event in snapshot
            .events
            .iter()
            .filter(|event| event.agent == agent.id)
        {
            let x = r.pos.x + event.at / activity_demo::DURATION * r.size.x;
            let marker = rect(x - 3.0, r.pos.y + 2.0, 6.0, 7.0);
            self.fill(cx, marker, self.event_color(event));
            self.targets.push(Target {
                id: event.id.into(),
                rect: rect(x - 5.0, r.pos.y, 10.0, 12.0),
            });
        }
        let x = r.pos.x + self.at / activity_demo::DURATION * r.size.x;
        self.fill(cx, rect(x, r.pos.y, 1.0, 11.0), self.accent);
    }

    fn agent_lanes(&mut self, cx: &mut Cx2d, snapshot: &DemoSnapshot) {
        self.header(
            cx,
            "Stable delegation rows · follow commands, edits, and the application they affect",
        );
        for (x, w, label) in [
            (20.0, 180.0, "AGENT / ASSIGNMENT"),
            (214.0, 338.0, "TERMINAL ACTIVITY"),
            (568.0, 398.0, "FILES / LIVE DIFF"),
            (982.0, 398.0, "APP & TEST EVIDENCE"),
        ] {
            self.text(cx, rect(x, 83.0, w, 19.0), label, 10.0, self.muted, 1);
        }
        for (i, id) in AGENTS.into_iter().enumerate() {
            let y = 111.0 + i as f64 * 139.0;
            let lane = rect(14.0, y - 6.0, 1372.0, 130.0);
            self.fill(
                cx,
                lane,
                blend(
                    self.background,
                    self.raised,
                    if i % 2 == 0 { 0.3 } else { 0.1 },
                ),
            );
            let Some(agent) = self.agent(snapshot, id) else {
                self.text(
                    cx,
                    rect(30.0, y + 29.0, 450.0, 25.0),
                    "Delegation slot · waiting for an agent",
                    12.0,
                    self.muted,
                    0,
                );
                continue;
            };
            let inset = if id == "a11y" {
                28.0
            } else if agent.parent.is_some() {
                14.0
            } else {
                0.0
            };
            if let Some(parent) = agent.parent {
                if let Some(parent_row) = AGENTS.iter().position(|p| *p == parent) {
                    let x = 24.0 + inset;
                    self.fill(
                        cx,
                        rect(
                            x - 10.0,
                            111.0 + parent_row as f64 * 139.0 + 17.0,
                            1.0,
                            y - (111.0 + parent_row as f64 * 139.0),
                        ),
                        blend(self.background, agent_color(parent), 0.50),
                    );
                    self.fill(cx, rect(x - 10.0, y + 16.0, 9.0, 1.0), agent_color(parent));
                }
            }
            let x = 24.0 + inset;
            self.card(
                cx,
                rect(x, y, 174.0 - inset, 112.0),
                Some(agent.id),
                self.surface,
            );
            self.fill(cx, rect(x, y + 4.0, 3.0, 103.0), agent_color(id));
            self.text(
                cx,
                rect(x + 12.0, y + 11.0, 151.0 - inset, 23.0),
                agent.name,
                15.0,
                self.ink,
                1,
            );
            self.role_label(
                cx,
                rect(x + 12.0, y + 35.0, 151.0 - inset, 17.0),
                agent.role,
            );
            self.badge(
                cx,
                x + 12.0,
                y + 58.0,
                state_label(agent.state),
                self.state_color(agent.state),
            );
            self.text(
                cx,
                rect(x + 12.0, y + 87.0, 151.0 - inset, 17.0),
                agent.system,
                10.0,
                self.muted,
                0,
            );
            self.terminal(cx, rect(214.0, y, 338.0, 112.0), agent);
            self.code(cx, rect(568.0, y, 398.0, 112.0), agent, snapshot);
            let app_id = match id {
                "fable" | "a11y" => Some("checkout"),
                "api" => Some("admin"),
                "qa" => Some("test_runner"),
                _ => None,
            };
            if let Some(app) = app_id.and_then(|id| snapshot.apps.iter().find(|app| app.id == id)) {
                self.app(cx, rect(982.0, y, 398.0, 112.0), app);
            } else {
                self.card(
                    cx,
                    rect(982.0, y, 398.0, 112.0),
                    Some(agent.id),
                    self.surface,
                );
                self.text(
                    cx,
                    rect(995.0, y + 12.0, 370.0, 22.0),
                    if id == "astra" {
                        "Delegation / current objective"
                    } else {
                        "Awaiting an application run"
                    },
                    12.0,
                    self.ink,
                    1,
                );
                self.wrapped(
                    cx,
                    rect(995.0, y + 43.0, 370.0, 58.0),
                    agent.task,
                    12.0,
                    self.muted,
                    3,
                );
            }
            self.history_strip(cx, snapshot, agent, rect(214.0, y + 118.0, 1166.0, 12.0));
        }
    }

    fn timeline(&mut self, cx: &mut Cx2d, snapshot: &DemoSnapshot) {
        self.header(
            cx,
            "Time is the spine · delegation and failures connect the work to its evidence",
        );
        let start = 373.0;
        let span = 744.0;
        self.text(
            cx,
            rect(20.0, 85.0, 115.0, 20.0),
            "AGENT",
            10.0,
            self.muted,
            1,
        );
        self.text(
            cx,
            rect(145.0, 85.0, 210.0, 20.0),
            "DOING NOW",
            10.0,
            self.muted,
            1,
        );
        self.text(
            cx,
            rect(1153.0, 85.0, 325.0, 20.0),
            "PINNED APPLICATION FRAMES",
            10.0,
            self.muted,
            1,
        );
        let now = start + self.at / activity_demo::DURATION * span;
        for i in 0..AGENTS.len() {
            let y = 113.0 + i as f64 * 65.0;
            self.fill(
                cx,
                rect(15.0, y, 1117.0, 58.0),
                blend(
                    self.background,
                    self.raised,
                    if i % 2 == 0 { 0.28 } else { 0.07 },
                ),
            );
            if now < start + span {
                self.fill(
                    cx,
                    rect(now, y, start + span - now, 58.0),
                    blend(self.background, self.surface, 0.18),
                );
            }
        }
        for seconds in (0..=180).step_by(30) {
            let x = start + seconds as f64 / 180.0 * span;
            self.fill(
                cx,
                rect(x, 112.0, 0.8, 349.0),
                blend(self.background, self.edge, 0.65),
            );
            self.text(
                cx,
                rect(x - 13.0, 86.0, 48.0, 19.0),
                &clock_label(seconds as f64),
                10.0,
                self.muted,
                2,
            );
        }
        // Parent-to-child links are tied to observed spawn events, not OS PIDs.
        for event in snapshot
            .events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::Spawn))
        {
            let Some(agent) = self.agent(snapshot, event.agent) else {
                continue;
            };
            let Some(parent) = agent.parent else {
                continue;
            };
            let Some(child_row) = AGENTS.iter().position(|id| *id == agent.id) else {
                continue;
            };
            let Some(parent_row) = AGENTS.iter().position(|id| *id == parent) else {
                continue;
            };
            let x = start + event.at / 180.0 * span;
            self.connector(
                cx,
                dvec2(x - 12.0, 139.0 + parent_row as f64 * 65.0),
                dvec2(x, 139.0 + child_row as f64 * 65.0),
                agent_color(parent),
                true,
            );
        }
        let issue = snapshot.events.iter().rev().find(|e| {
            matches!(e.kind, EventKind::Issue)
                || (matches!(e.kind, EventKind::Test) && e.title.starts_with("FAIL"))
        });
        let edit = issue.and_then(|issue| {
            snapshot.events.iter().find(|e| {
                e.at > issue.at && e.agent == "fable" && matches!(e.kind, EventKind::Edit)
            })
        });
        let retest = edit.and_then(|edit| {
            snapshot.events.iter().find(|e| {
                e.at > edit.at
                    && e.agent == "qa"
                    && matches!(e.kind, EventKind::Test | EventKind::Finish)
                    && !e.title.starts_with("FAIL")
            })
        });
        for (a, b) in [(issue, edit), (edit, retest)] {
            if let (Some(a), Some(b)) = (a, b) {
                let row = |id: &str| AGENTS.iter().position(|s| *s == id).unwrap_or(0) as f64;
                self.connector(
                    cx,
                    dvec2(start + a.at / 180.0 * span, 139.0 + row(a.agent) * 65.0),
                    dvec2(start + b.at / 180.0 * span, 139.0 + row(b.agent) * 65.0),
                    self.event_color(a),
                    false,
                );
            }
        }
        for (i, id) in AGENTS.into_iter().enumerate() {
            let y = 113.0 + i as f64 * 65.0;
            let Some(agent) = self.agent(snapshot, id) else {
                self.text(
                    cx,
                    rect(26.0, y + 18.0, 310.0, 21.0),
                    "Waiting for delegation",
                    11.0,
                    self.muted,
                    0,
                );
                continue;
            };
            let indent = if id == "a11y" {
                18.0
            } else if agent.parent.is_some() {
                9.0
            } else {
                0.0
            };
            self.card(
                cx,
                rect(20.0 + indent, y + 3.0, 112.0 - indent, 51.0),
                Some(id),
                self.surface,
            );
            self.text(
                cx,
                rect(29.0 + indent, y + 12.0, 96.0 - indent, 19.0),
                agent.name,
                12.0,
                agent_color(id),
                1,
            );
            self.text(
                cx,
                rect(29.0 + indent, y + 33.0, 96.0 - indent, 15.0),
                state_label(agent.state),
                8.5,
                self.muted,
                1,
            );
            self.wrapped(
                cx,
                rect(145.0, y + 12.0, 205.0, 42.0),
                agent.task,
                11.0,
                self.ink,
                2,
            );
            let events: Vec<_> = snapshot.events.iter().filter(|e| e.agent == id).collect();
            for (n, event) in events.iter().enumerate() {
                let x = start + event.at / 180.0 * span;
                let end = events.get(n + 1).map(|e| e.at).unwrap_or(self.at);
                let width = ((end - event.at) / 180.0 * span).max(3.0);
                self.fill(
                    cx,
                    rect(x, y + 21.0, width, 12.0),
                    blend(self.surface, self.event_color(event), 0.30),
                );
                let selected = self.selected.as_deref() == Some(event.id);
                let marker_color = if selected {
                    self.accent
                } else {
                    self.event_color(event)
                };
                if matches!(event.kind, EventKind::Issue) || event.title.starts_with("FAIL") {
                    self.diamond(cx, dvec2(x, y + 27.0), 7.0, marker_color);
                } else {
                    self.fill(cx, rect(x - 2.5, y + 18.0, 5.0, 18.0), marker_color);
                }
                self.targets.push(Target {
                    id: event.id.into(),
                    rect: rect(x - 5.0, y + 10.0, width.max(10.0), 38.0),
                });
                if n + 1 == events.len() || selected {
                    let label_x = x.min(start + span - 150.0).max(start);
                    self.text(
                        cx,
                        rect(label_x, y + 39.0, 152.0, 16.0),
                        event.title,
                        9.0,
                        self.muted,
                        0,
                    );
                }
            }
        }
        let now = start + self.at / 180.0 * span;
        self.fill(cx, rect(now, 109.0, 1.4, 334.0), self.accent);
        self.badge(
            cx,
            now.min(1027.0),
            447.0,
            &format!("NOW {}", clock_label(self.at)),
            self.accent,
        );
        if let Some(agent) = self.selected_agent(snapshot) {
            self.text(
                cx,
                rect(20.0, 484.0, 1095.0, 23.0),
                &format!(
                    "Following {}  /  click an event to inspect its evidence",
                    agent.name
                ),
                12.0,
                self.muted,
                1,
            );
            self.code(cx, rect(20.0, 515.0, 543.0, 269.0), agent, snapshot);
            self.terminal(cx, rect(581.0, 515.0, 550.0, 269.0), agent);
        }
        for (i, id) in ["checkout", "admin", "test_runner"].into_iter().enumerate() {
            let r = rect(1153.0, 112.0 + i as f64 * 225.0, 327.0, 213.0);
            if let Some(app) = snapshot.apps.iter().find(|a| a.id == id) {
                self.app(cx, r, app);
            } else {
                self.card(cx, r, None, self.surface);
                self.text(
                    cx,
                    rect(r.pos.x + 14.0, r.pos.y + 17.0, r.size.x - 28.0, 22.0),
                    "Application slot",
                    13.0,
                    self.muted,
                    1,
                );
                self.wrapped(
                    cx,
                    rect(r.pos.x + 14.0, r.pos.y + 53.0, r.size.x - 28.0, 50.0),
                    "The next launched application appears here with its current rendered frame.",
                    11.0,
                    self.muted,
                    3,
                );
            }
        }
    }

    fn system_lanes(&mut self, cx: &mut Cx2d, snapshot: &DemoSnapshot) {
        self.header(
            cx,
            "The product is the map · stable system boundaries with explicit agent ownership",
        );
        if let Some(root) = self.agent(snapshot, "astra") {
            self.card(
                cx,
                rect(20.0, 83.0, 1400.0, 68.0),
                Some(root.id),
                self.surface,
            );
            self.fill(cx, rect(20.0, 87.0, 3.0, 60.0), agent_color(root.id));
            self.text(
                cx,
                rect(35.0, 97.0, 217.0, 23.0),
                "Astra / orchestration",
                15.0,
                self.ink,
                1,
            );
            self.text(
                cx,
                rect(35.0, 123.0, 217.0, 17.0),
                state_label(root.state),
                9.5,
                agent_color(root.id),
                1,
            );
            self.wrapped(
                cx,
                rect(278.0, 98.0, 789.0, 42.0),
                root.task,
                12.0,
                self.ink,
                2,
            );
            self.text(
                cx,
                rect(1115.0, 99.0, 280.0, 18.0),
                &format!(
                    "{} agents · {} observed events",
                    snapshot.agents.len(),
                    snapshot.events.len()
                ),
                11.0,
                self.muted,
                0,
            );
            self.text(
                cx,
                rect(1115.0, 122.0, 280.0, 17.0),
                "Delegates work across system boundaries",
                9.5,
                self.muted,
                0,
            );
        }
        for (i, (title, subtitle, owner, app_id)) in [
            ("Checkout", "Interface / purchase flow", "fable", "checkout"),
            ("Orders API", "Contract / payment integrity", "api", "admin"),
            (
                "Verification",
                "Browser / API / accessibility",
                "qa",
                "test_runner",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let x = 20.0 + i as f64 * 474.0;
            let color = agent_color(owner);
            self.fill(
                cx,
                rect(x, 168.0, 452.0, 667.0),
                blend(self.background, color, 0.035),
            );
            self.fill(cx, rect(x, 168.0, 452.0, 3.0), color);
            self.text(
                cx,
                rect(x + 12.0, 184.0, 315.0, 29.0),
                title,
                20.0,
                self.ink,
                1,
            );
            self.text(
                cx,
                rect(x + 12.0, 214.0, 426.0, 21.0),
                subtitle,
                11.5,
                self.muted,
                0,
            );
            if i < 2 {
                self.connector(
                    cx,
                    dvec2(x + 431.0, 195.0),
                    dvec2(x + 485.0, 195.0),
                    self.edge,
                    false,
                );
            }
            if let Some(agent) = self.agent(snapshot, owner) {
                self.badge(
                    cx,
                    x + 12.0,
                    243.0,
                    &format!("{} · {}", agent.name, state_label(agent.state)),
                    color,
                );
                if owner == "fable" && self.agent(snapshot, "a11y").is_some() {
                    self.badge(cx, x + 240.0, 243.0, "+ accessibility", agent_color("a11y"));
                }
                self.code(cx, rect(x + 11.0, 532.0, 430.0, 149.0), agent, snapshot);
                self.terminal(cx, rect(x + 11.0, 693.0, 430.0, 126.0), agent);
            } else {
                self.text(
                    cx,
                    rect(x + 12.0, 246.0, 423.0, 22.0),
                    "No agent assigned yet",
                    11.0,
                    self.muted,
                    0,
                );
                self.card(cx, rect(x + 11.0, 532.0, 430.0, 287.0), None, self.surface);
                self.wrapped(cx,rect(x+26.0,554.0,400.0,70.0),"Agent commands and code changes will appear in this system lane when work starts.",13.0,self.muted,3);
            }
            let app_rect = rect(x + 11.0, 278.0, 430.0, 240.0);
            if let Some(app) = snapshot.apps.iter().find(|a| a.id == app_id) {
                self.app(cx, app_rect, app);
            } else {
                self.card(cx, app_rect, None, self.surface);
                self.text(
                    cx,
                    rect(x + 26.0, 295.0, 400.0, 23.0),
                    "Runtime / waiting for launch",
                    13.0,
                    self.muted,
                    1,
                );
                for n in 0..4 {
                    self.fill(
                        cx,
                        rect(
                            x + 26.0,
                            341.0 + n as f64 * 28.0,
                            350.0 - n as f64 * 50.0,
                            7.0,
                        ),
                        blend(self.surface, self.muted, 0.12),
                    );
                }
            }
        }
    }

    fn hit_at(&self, abs: DVec2) -> Option<String> {
        if self.zoom <= 0.0 || !self.viewport.contains(abs) {
            return None;
        }
        let world = (abs - self.viewport.pos - self.pan) / self.zoom;
        self.targets
            .iter()
            .rev()
            .find(|target| target.rect.contains(world))
            .map(|target| target.id.clone())
    }
}

impl WidgetNode for StudioActivityViews {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for StudioActivityViews {
    fn draw_walk(&mut self, cx: &mut Cx2d, _: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        let resized = self.viewport.size != view.size;
        self.viewport = view;
        cx.add_rect_area(&mut self.area, view);
        if self.needs_fit || self.zoom <= 0.0 || (resized && self.fitted) {
            let scene = self.scene_size();
            self.zoom = ((view.size.x - 24.0) / scene.x)
                .min((view.size.y - 24.0) / scene.y)
                .clamp(0.18, 1.5);
            self.pan = (view.size - scene * self.zoom) * 0.5;
            self.needs_fit = false;
            self.fitted = true;
        }
        cx.push_clip_rect(view);
        self.draw_shape.color = self.color(self.background);
        self.draw_shape.draw_abs(cx, view);
        self.targets.clear();
        let snapshot = activity_demo::snapshot(self.at);
        match self.layout {
            DemoLayout::AgentLanes => self.agent_lanes(cx, &snapshot),
            DemoLayout::Timeline => self.timeline(cx, &snapshot),
            DemoLayout::SystemLanes => self.system_lanes(cx, &snapshot),
        }
        if self.zoom < 0.55 {
            let hint = Rect {
                pos: view.pos + dvec2(10.0, (view.size.y - 23.0).max(0.0)),
                size: dvec2((view.size.x - 20.0).max(1.0), 20.0),
            };
            self.draw_shape.color = self.color(self.background);
            self.draw_shape.draw_abs(cx, hint);
            self.draw_text.text_style.font_size = 10.0;
            self.draw_text.color = self.muted;
            self.draw_text.draw_abs(
                cx,
                hint.pos + dvec2(4.0, 4.0),
                "Overview · scroll to zoom into code and app frames · drag to pan · F to fit",
            );
        }
        cx.pop_clip_rect();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(e) => {
                cx.set_key_focus(self.area);
                self.drag = Some((e.abs, self.pan));
                self.pressed = self.hit_at(e.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(e) => {
                if let Some((start, pan)) = self.drag {
                    if (e.abs - start).length() > 3.0 {
                        self.pan = pan + e.abs - start;
                        self.fitted = false;
                        self.area.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(e) => {
                let click = self
                    .drag
                    .take()
                    .is_some_and(|(start, _)| (e.abs - start).length() < 4.0);
                if click {
                    if let Some(id) = self
                        .pressed
                        .take()
                        .filter(|id| self.hit_at(e.abs).as_ref() == Some(id))
                    {
                        self.selected = Some(id.clone());
                        cx.widget_action(self.uid, ActivityViewAction::Selected(id));
                        self.area.redraw(cx);
                    }
                }
                self.pressed = None;
            }
            Hit::FingerScroll(e) => {
                let delta = if e.scroll.y.abs() > 0.0 {
                    e.scroll.y
                } else {
                    e.scroll.x
                };
                let world = (e.abs - self.viewport.pos - self.pan) / self.zoom.max(0.001);
                self.zoom = (self.zoom * (-delta * 0.005).exp()).clamp(0.18, 3.0);
                self.pan = e.abs - self.viewport.pos - world * self.zoom;
                self.fitted = false;
                self.area.redraw(cx);
            }
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let hovered = self.hit_at(e.abs);
                cx.set_cursor(if hovered.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Grab
                });
                if self.hovered != hovered {
                    self.hovered = hovered;
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hovered.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            Hit::KeyDown(e) if e.key_code == KeyCode::KeyF => self.fit(cx),
            Hit::KeyDown(e) if e.key_code == KeyCode::Escape => {
                self.drag = None;
                self.pressed = None;
            }
            _ => {}
        }
    }
}
