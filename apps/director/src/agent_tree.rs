//! App-local sidebar of the agent tree: the synthetic Director root and the
//! real agents beneath it, keyed by stable terminal origin. Rows come from the
//! worker's immutable snapshot and are indexed once per engine revision.
//! Clicking a row selects its LEVEL (the canvas then shows that agent's own
//! lane followed by its direct children); the chevron only toggles disclosure.
//! Neither touches terminal focus, a Dock tab or a lane's lifecycle; the app
//! keeps the actionable lane among the lanes the selected level shows.
use crate::iteration::{AgentTerminalState, Engine, FlowLifecycle};
use makepad_widgets::*;
use std::{collections::BTreeSet, sync::Arc};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.StudioAgentTree = #(StudioAgentTree::register_widget(vm)){
        width: Fill height: Fill
        background: theme.color_bg_app
        surface: mix(theme.color_bg_app, theme.color_text, 0.05)
        edge: mix(theme.color_bg_app, theme.color_text, 0.12)
        ink: theme.color_text
        muted: mix(theme.color_bg_app, theme.color_text, 0.57)
        accent: theme.color_focus
        attention: #e8a33d
        draw_title +: {text_style: theme.font_regular{font_size: 9.5} color: theme.color_text}
        draw_text +: {text_style: theme.font_regular{font_size: 8.5} color: theme.color_text}
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum AgentTreeAction {
    /// The level projection changed; None is the Director root.
    SelectLevel(Option<String>),
    /// Disclosure changed; the host persists the expanded set.
    Expanded(BTreeSet<String>),
    #[default]
    None,
}

struct Row {
    /// None is the synthetic Director root.
    node: Option<String>,
    depth: usize,
    title: String,
    detail: String,
    expandable: bool,
    expanded: bool,
    live: bool,
    attention: bool,
}

const ROW_HEIGHT: f64 = 24.0;
const INDENT: f64 = 14.0;
const CHEVRON: f64 = 18.0;
const MAX_ROWS: usize = crate::iteration::MAX_AGENT_NODES + 1;

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct StudioAgentTree {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_marks: DrawVector,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_text: DrawText,
    #[live]
    background: Vec4f,
    #[live]
    surface: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    muted: Vec4f,
    #[live]
    accent: Vec4f,
    #[live]
    attention: Vec4f,
    #[rust]
    area: Area,
    #[rust]
    viewport: Rect,
    #[rust]
    engine: Option<Arc<Engine>>,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    expanded: BTreeSet<String>,
    #[rust]
    level: Option<String>,
    #[rust]
    hovered: Option<usize>,
    #[rust]
    pressed: Option<(usize, bool)>,
    #[rust]
    scroll: f64,
}

fn short(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut value: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        value.push('…');
    }
    value
}

impl StudioAgentTree {
    /// A new snapshot: rows are rebuilt once here, never during draw.
    pub fn set_engine(&mut self, cx: &mut Cx, engine: Arc<Engine>) {
        if self
            .engine
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &engine))
        {
            return;
        }
        self.engine = Some(engine);
        self.rebuild();
        self.area.redraw(cx);
    }

    /// Restored presentation: selected level and disclosure.
    pub fn set_state(&mut self, cx: &mut Cx, level: Option<String>, expanded: BTreeSet<String>) {
        if self.level == level && self.expanded == expanded {
            return;
        }
        self.level = level;
        self.expanded = expanded;
        self.rebuild();
        self.area.redraw(cx);
    }

    pub fn set_level(&mut self, cx: &mut Cx, level: Option<String>) {
        if self.level == level {
            return;
        }
        self.level = level;
        self.rebuild();
        self.area.redraw(cx);
    }

    pub fn level(&self) -> Option<&str> {
        self.level.as_deref()
    }

    pub fn expanded(&self) -> &BTreeSet<String> {
        &self.expanded
    }

    /// Expand every ancestor so a level chosen elsewhere is visible.
    pub fn reveal(&mut self, cx: &mut Cx, node: &str) {
        let Some(engine) = &self.engine else {
            return;
        };
        let mut changed = false;
        for ancestor in engine.agent_ancestors(node) {
            changed |= self.expanded.insert(ancestor);
        }
        if changed {
            self.rebuild();
            self.area.redraw(cx);
        }
    }

    fn rebuild(&mut self) {
        let Some(engine) = self.engine.clone() else {
            self.rows.clear();
            return;
        };
        let roots = engine.agent_children(None);
        let mut rows = vec![Row {
            node: None,
            depth: 0,
            title: "Director".into(),
            detail: format!(
                "{} root lane{}",
                roots.len(),
                if roots.len() == 1 { "" } else { "s" }
            ),
            expandable: false,
            expanded: true,
            live: true,
            attention: false,
        }];
        let mut stack: Vec<(String, usize)> = roots
            .iter()
            .rev()
            .map(|node| (node.id.clone(), 1))
            .collect();
        while let Some((id, depth)) = stack.pop() {
            if rows.len() >= MAX_ROWS {
                break;
            }
            let children = engine.agent_children(Some(&id));
            let expanded = self.expanded.contains(&id);
            let flow = engine.agent_current_flow(&id);
            let title = flow
                .map(|flow| flow.title.clone())
                .unwrap_or_else(|| format!("{id} · deleted"));
            let unread = engine.agent_unread(&id).len();
            // A fact restored from disk is the previous run's metadata, not
            // an observation: it shows as unverified, as agent status does,
            // and raises no attention until the terminal was seen again.
            let fact = engine.agent_terminal(&id);
            let unverified = fact.is_some_and(|fact| !fact.live);
            let terminal = fact.filter(|fact| fact.live).map(|fact| fact.state);
            let mut detail = match flow {
                Some(flow) if flow.lifecycle == FlowLifecycle::Archived => "archived".to_owned(),
                Some(flow) if flow.lifecycle == FlowLifecycle::Stopped => "stopped".to_owned(),
                Some(_) if unverified => "unverified".to_owned(),
                Some(_) => match terminal {
                    Some(AgentTerminalState::Attached) | Some(AgentTerminalState::Detached) => {
                        "running".to_owned()
                    }
                    Some(AgentTerminalState::Ended) => "ended".to_owned(),
                    Some(AgentTerminalState::Unavailable) => "failed".to_owned(),
                    Some(AgentTerminalState::Stopping) => "stopping".to_owned(),
                    Some(AgentTerminalState::Connecting) => "starting".to_owned(),
                    Some(AgentTerminalState::WaitingForBinding) | None => "pending".to_owned(),
                },
                None => "deleted".to_owned(),
            };
            if !children.is_empty() {
                detail = format!("{detail} · {}", children.len());
            }
            if unread != 0 {
                detail = format!("{detail} · {unread} new");
            }
            let live = flow.is_some_and(|flow| flow.lifecycle != FlowLifecycle::Archived);
            rows.push(Row {
                node: Some(id.clone()),
                depth,
                title,
                detail,
                expandable: !children.is_empty(),
                expanded,
                live,
                attention: unread != 0 || matches!(terminal, Some(AgentTerminalState::Unavailable)),
            });
            if expanded {
                for child in children.iter().rev() {
                    stack.push((child.id.clone(), depth + 1));
                }
            }
        }
        self.rows = rows;
        let content = self.rows.len() as f64 * ROW_HEIGHT;
        self.scroll = self
            .scroll
            .clamp(0.0, (content - self.viewport.size.y).max(0.0));
    }

    fn row_at(&self, abs: DVec2) -> Option<(usize, bool)> {
        if !self.viewport.contains(abs) {
            return None;
        }
        let y = abs.y - self.viewport.pos.y + self.scroll;
        let index = (y / ROW_HEIGHT).floor();
        if index < 0.0 {
            return None;
        }
        let index = index as usize;
        let row = self.rows.get(index)?;
        let chevron_right = self.viewport.pos.x + 6.0 + row.depth as f64 * INDENT + CHEVRON;
        Some((index, row.expandable && abs.x < chevron_right))
    }

    fn toggle(&mut self, cx: &mut Cx, index: usize) {
        let Some(node) = self.rows.get(index).and_then(|row| row.node.clone()) else {
            return;
        };
        if !self.expanded.remove(&node) {
            self.expanded.insert(node);
        }
        self.rebuild();
        cx.widget_action(self.uid, AgentTreeAction::Expanded(self.expanded.clone()));
        self.area.redraw(cx);
    }

    fn select(&mut self, cx: &mut Cx, index: usize) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let level = row.node.clone();
        if self.level != level {
            self.level = level.clone();
            self.rebuild();
            cx.widget_action(self.uid, AgentTreeAction::SelectLevel(level));
        }
        self.area.redraw(cx);
    }

    fn text(&mut self, cx: &mut Cx2d, rect: Rect, value: &str, title: bool, color: Vec4f) {
        let draw = if title {
            &mut self.draw_title
        } else {
            &mut self.draw_text
        };
        draw.color = color;
        let size = if title { 9.5 } else { 8.5 };
        let mut value = short(value, (rect.size.x / (size * 0.52)).max(1.0) as usize);
        while value.chars().count() > 1
            && draw
                .layout(cx, 0.0, 0.0, None, false, Align::default(), &value)
                .size_in_lpxs
                .width as f64
                > rect.size.x
        {
            if value.ends_with('…') {
                value.pop();
            }
            value.pop();
            value.push('…');
        }
        cx.push_clip_rect(rect);
        draw.draw_abs(cx, rect.pos, &value);
        cx.pop_clip_rect();
    }

    fn chevron(&mut self, cx: &mut Cx2d, center: DVec2, open: bool, hovered: bool) {
        let angle = if open {
            std::f64::consts::FRAC_PI_2
        } else {
            0.0
        };
        let (sin, cos) = angle.sin_cos();
        let color = if hovered { self.ink } else { self.muted };
        self.draw_marks.begin();
        self.draw_marks
            .set_color(color.x, color.y, color.z, color.w);
        for (index, (px, py)) in [(-2.5, -4.0), (2.0, 0.0), (-2.5, 4.0)]
            .into_iter()
            .enumerate()
        {
            let x = (center.x + px * cos - py * sin) as f32;
            let y = (center.y + px * sin + py * cos) as f32;
            if index == 0 {
                self.draw_marks.move_to(x, y);
            } else {
                self.draw_marks.line_to(x, y);
            }
        }
        self.draw_marks.stroke(1.15);
        self.draw_marks.end(cx);
    }
}

impl WidgetNode for StudioAgentTree {
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
    fn children(&self, _visit: &mut dyn FnMut(LiveId, WidgetRef)) {}
    fn find_widgets_from_point(&self, _cx: &Cx, _p: DVec2, _found: &mut dyn FnMut(&WidgetRef)) {}
}

impl Widget for StudioAgentTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        self.viewport = view;
        cx.add_rect_area(&mut self.area, view);
        cx.push_clip_rect(view);
        self.draw_shape.color = self.background;
        self.draw_shape.draw_abs(cx, view);
        let content = self.rows.len() as f64 * ROW_HEIGHT;
        self.scroll = self.scroll.clamp(0.0, (content - view.size.y).max(0.0));
        for index in 0..self.rows.len() {
            let top = view.pos.y + index as f64 * ROW_HEIGHT - self.scroll;
            if top + ROW_HEIGHT < view.pos.y || top > view.pos.y + view.size.y {
                continue;
            }
            let row_rect = Rect {
                pos: dvec2(view.pos.x, top),
                size: dvec2(view.size.x, ROW_HEIGHT),
            };
            let selected = self.rows[index].node == self.level;
            let hovered = self.hovered == Some(index);
            if selected || hovered {
                self.draw_shape.color = if selected {
                    vec4(self.accent.x, self.accent.y, self.accent.z, 0.22)
                } else {
                    self.surface
                };
                self.draw_shape.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(row_rect.pos.x + 2.0, row_rect.pos.y + 1.0),
                        size: dvec2(row_rect.size.x - 4.0, row_rect.size.y - 2.0),
                    },
                );
            }
            let depth = self.rows[index].depth;
            let x = view.pos.x + 6.0 + depth as f64 * INDENT;
            if self.rows[index].expandable {
                let open = self.rows[index].expanded;
                self.chevron(
                    cx,
                    dvec2(x + CHEVRON * 0.5, top + ROW_HEIGHT * 0.5),
                    open,
                    hovered,
                );
            } else if index != 0 {
                let color = if self.rows[index].live {
                    self.muted
                } else {
                    self.edge
                };
                self.draw_shape.color = color;
                self.draw_shape.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x + CHEVRON * 0.5 - 2.0, top + ROW_HEIGHT * 0.5 - 2.0),
                        size: dvec2(4.0, 4.0),
                    },
                );
            }
            let label_x = x + CHEVRON + 2.0;
            let detail_width = (view.size.x * 0.34).clamp(48.0, 120.0);
            let label_width = (view.pos.x + view.size.x - 8.0 - detail_width - label_x).max(24.0);
            let title = self.rows[index].title.clone();
            let detail = self.rows[index].detail.clone();
            let live = self.rows[index].live;
            let attention = self.rows[index].attention;
            let ink = if live { self.ink } else { self.muted };
            self.text(
                cx,
                Rect {
                    pos: dvec2(label_x, top + 5.0),
                    size: dvec2(label_width, 16.0),
                },
                &title,
                true,
                ink,
            );
            let detail_color = if attention {
                self.attention
            } else {
                self.muted
            };
            self.text(
                cx,
                Rect {
                    pos: dvec2(view.pos.x + view.size.x - 6.0 - detail_width, top + 6.5),
                    size: dvec2(detail_width, 14.0),
                },
                &detail,
                false,
                detail_color,
            );
        }
        self.draw_shape.color = self.edge;
        self.draw_shape.draw_abs(
            cx,
            Rect {
                pos: dvec2(view.pos.x + view.size.x - 1.0, view.pos.y),
                size: dvec2(1.0, view.size.y),
            },
        );
        cx.pop_clip_rect();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(event) => {
                self.pressed = self.row_at(event.abs);
            }
            Hit::FingerUp(event) => {
                let pressed = self.pressed.take();
                if let (Some((index, on_chevron)), Some((up, _))) =
                    (pressed, self.row_at(event.abs))
                {
                    if index == up {
                        if on_chevron {
                            self.toggle(cx, index);
                        } else {
                            self.select(cx, index);
                        }
                    }
                }
            }
            Hit::FingerScroll(event) => {
                let content = self.rows.len() as f64 * ROW_HEIGHT;
                self.scroll = (self.scroll + event.scroll.y)
                    .clamp(0.0, (content - self.viewport.size.y).max(0.0));
                self.area.redraw(cx);
            }
            Hit::FingerHoverIn(event) | Hit::FingerHoverOver(event) => {
                let hovered = self.row_at(event.abs).map(|(index, _)| index);
                if hovered != self.hovered {
                    self.hovered = hovered;
                    self.area.redraw(cx);
                }
                cx.set_cursor(if hovered.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
            }
            Hit::FingerHoverOut(_) => {
                if self.hovered.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            _ => {}
        }
    }
}
