//! The halted view: compile errors, a parked panic or a failed run, in the Ignition Field's own
//! world. The field stops turning and cools (ember core and frame); a HUD header names what
//! halted; the errors list beside (wide) or below (narrow) the galaxy, root causes first with
//! repeated errors folded into groups. Logical points (dp on Android).
use crate::{
    document::{Line, Role},
    caption::Caption, groups, Action, Card, Environment, Layout, Mode, Renderer, State, TextLine,
};

use makepad_script::{id, LiveId};

/// The HUD's margin inside the frame (the field's corner brackets sit at 10).
const MARGIN: f32 = 22.0;
/// Member rows of a group shown before "+N more".
const ROWS: usize = 4;

const EMBER: [f32; 4] = [1.0, 0.478, 0.361, 1.0];
const DIM: [f32; 4] = [0.431, 0.396, 0.576, 1.0];
const SOFT: [f32; 4] = [0.706, 0.671, 0.839, 1.0];

/// The document's label of a phase.
pub(super) fn phase_key(phase: relax_base::run_state::Phase) -> LiveId {
    match phase {
        relax_base::run_state::Phase::Resolve => id!(resolve),
        relax_base::run_state::Phase::Parse => id!(parse),
        relax_base::run_state::Phase::Typecheck => id!(typecheck),
        relax_base::run_state::Phase::Codegen => id!(codegen),
        relax_base::run_state::Phase::Link => id!(link),
        relax_base::run_state::Phase::FirstFrame => id!(first_frame),
    }
}

fn style(base: Line, size: f32, color: [f32; 4]) -> Line {
    Line {
        size,
        height: (size * 1.4).round(),
        color,
        ..base
    }
}

/// The last two parts of a path (`src/main.rs`).
fn short_path(file: &str) -> &str {
    let mut cut = file.len();
    for _ in 0..2 {
        match file[..cut].rfind('/') {
            Some(i) => cut = i,
            None => return file,
        }
    }
    &file[cut + 1..]
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Renderer {
    fn label(&mut self, value: &str, style: Line, rect: [f32; 4], fixed: bool) -> TextLine {
        let run = self.text.shape(value, style.size, rect[2], style.height);
        let x = rect[0] + (rect[2] - run.width).max(0.0) * style.align;
        TextLine {
            run,
            position: [x, rect[1]],
            style,
            fixed,
        }
    }

    /// Lays out `value` at `y` in the scrolled list and answers its height.
    fn flow(&mut self, layout: &mut Layout, value: &str, style: Line, x: f32, y: f32, width: f32) -> f32 {
        let line = self.label(value, style, [x, y, width.max(1.0), 0.0], false);
        let height = line.run.rows as f32 * style.height;
        layout.text.push(line);
        height
    }

    pub(super) fn layout(&mut self, state: &State, size: [f32; 2], env: Environment, caption: &Caption) {
        let [left, top, right, bottom] = env.insets;
        // the caption strip is the window's: the view starts below it
        let top = top.max(caption.height);
        let program = self.active.as_ref().unwrap();
        let sizes = program.layout(state.mode);
        let app_style = program.line(Role::App);
        let status_style = program.line(Role::Status);
        let button_base = program.line(Role::Current);
        let headline_style = program.line(Role::Domain);
        let location_style = program.line(Role::Location);
        let error_style = program.line(Role::Errors);
        let panic_style = program.line(Role::Panic);
        let hint_style = program.line(Role::Hints);
        let hints = program.hints(state, self.input.as_deref());
        let labels = [
            id!(tap_restart),
            id!(tap_copy),
            id!(tap_open),
            id!(tap_dismiss),
            id!(tap_submit),
            id!(tap_cancel),
        ]
        .map(|key| program.label(key));
        let paused_label = program.label(id!(paused));
        let halted_label = program.label(id!(halted));
        let root_label = program.label(id!(root_cause));
        let follow_label = program.label(id!(follow_on));
        let resume_label = program.label(id!(tap_resume));
        let input_label = program.label(id!(input_label));
        let has_icon = self.icon_aspect.is_some() && program.icon;
        let overlay = state.mode == Mode::Overlay;

        // ---- the frame: the whole window, over the app too (Overlay), the HUD inside the
        // system's insets
        let safe = [
            left.min(size[0]),
            top.min(size[1]),
            (size[0] - left - right).max(1.0),
            (size[1] - top - bottom).max(1.0),
        ];
        let frame = [0.0, 0.0, size[0], size[1]];
        let inner_box = safe;
        let m = MARGIN.min((inner_box[2].min(inner_box[3]) * 0.25).max(4.0));
        let x0 = inner_box[0] + m;
        let width = (inner_box[2] - 2.0 * m).max(1.0);
        let y_top = inner_box[1] + m - 2.0;
        let y_end = inner_box[1] + inner_box[3] - m;
        let mut layout = Layout {
            panel: frame,
            frame,
            ..Layout::default()
        };
        self.text.begin_layout();
        let tiny = width < 260.0 || inner_box[3] < 240.0;

        // ---- the caption's title (the app, what halted) and window buttons
        let status = if state.parked.is_some() {
            paused_label.clone()
        } else if !state.diagnostics.is_empty() {
            halted_label.clone()
        } else {
            state.domain.clone()
        };
        if caption.height > 0.0 {
            let app = Line { size: 10.0, height: 13.0, color: crate::caption::TITLE, ..app_style };
            let what = Line { size: 10.0, height: 13.0, ..status_style };
            let dot = Line { color: crate::caption::TITLE, ..what };
            let parts = [(state.app.clone(), app), ("·".to_string(), dot), (status.clone(), what)];
            layout.fixed_text.extend(crate::caption::title(&mut self.text, caption, size[0], &parts));
            for (action, rect) in &caption.buttons {
                layout.cards.push(Card {
                    rect: *rect,
                    action: *action,
                    accent: crate::CAPTION_BUTTON,
                    members: 0..0,
                    fixed: true,
                });
            }
        }

        // ---- header (fixed): app and what halted (without a caption), the headline, the
        // count line
        let mut y = y_top;
        // the app's icon heads the name; with a caption the name is the caption's title
        let icon_w = if has_icon && !tiny && caption.height <= 0.0 { 22.0 } else { 0.0 };
        if icon_w > 0.0 {
            layout.icon = Some([x0, y, icon_w, icon_w]);
            layout.icon_fixed = true;
        }
        let tx = x0 + if icon_w > 0.0 { icon_w + 8.0 } else { 0.0 };
        let tw = (width - (tx - x0)).max(1.0);
        if caption.height <= 0.0 {
            let app = self.label(&state.app.to_uppercase(), app_style, [tx, y, tw, 0.0], true);
            y += app.run.rows as f32 * app_style.height;
            layout.fixed_text.push(app);
            let line = self.label(&status.to_uppercase(), status_style, [tx, y, tw, 0.0], true);
            y = y.max(y_top + icon_w) + line.run.rows as f32 * status_style.height + status_style.gap;
            layout.fixed_text.push(line);
        }
        let errors = state.diagnostics.len();
        let groups = groups::groups(&state.diagnostics);
        let headline = if let Some(panic) = &state.parked {
            format!("Panicked at {}", short_path(&panic.location))
        } else if errors > 0 {
            plural(errors, "error", "errors")
        } else {
            state.app.clone()
        };
        let mut headline_style = headline_style;
        if tiny {
            headline_style = style_size(headline_style, 16.0);
        }
        let line = self.label(&headline, headline_style, [x0, y, width, 0.0], true);
        let rows = line.run.rows.min(2);
        y += rows as f32 * headline_style.height;
        layout.fixed_text.push(line);
        let sub = if state.parked.is_some() {
            String::new()
        } else if errors > 0 {
            let roots: usize = groups.iter().filter(|g| g.root).map(|g| g.members.len()).sum();
            let mut files: Vec<&str> = state.diagnostics.iter().map(|d| d.file.as_str()).filter(|f| !f.is_empty()).collect();
            files.sort_unstable();
            files.dedup();
            let mut parts = vec![format!("{roots} {root_label}{}", if roots == 1 { "" } else { "s" })];
            if errors > roots {
                parts.push(format!("{} {follow_label}", errors - roots));
            }
            if groups.len() < errors {
                parts.push(plural(groups.len(), "group", "groups"));
            }
            if !files.is_empty() {
                parts.push(plural(files.len(), "file", "files"));
            }
            parts.join(" · ")
        } else {
            String::new()
        };
        if !sub.is_empty() && !tiny {
            let s = style(location_style, 11.0, if state.parked.is_some() { location_style.color } else { SOFT });
            let line = self.label(&sub, s, [x0, y + 2.0, width, 0.0], true);
            y += 2.0 + line.run.rows.min(2) as f32 * s.height;
            layout.fixed_text.push(line);
        }
        let header_end = y + 14.0;

        // ---- footer (fixed): key hints on desktop, buttons on touch
        let mut buttons = Vec::new();
        if env.touch {
            if self.input.is_some() {
                buttons.push((Action::Submit, labels[4].clone()));
                buttons.push((Action::Cancel, labels[5].clone()));
            } else {
                if state.can_retry || state.can_restart {
                    buttons.push((Action::Restart, labels[0].clone()));
                }
                if errors > 0 || state.parked.is_some() {
                    buttons.push((Action::Copy, labels[1].clone()));
                }
                if state.diagnostics.get(self.selected).is_some_and(|d| !d.file.is_empty()) {
                    buttons.push((Action::Open, labels[2].clone()));
                }
                if overlay {
                    buttons.push((Action::Dismiss, labels[3].clone()));
                }
            }
        }
        let gap = sizes.action_gap;
        let columns = (((width + gap) / (sizes.action_width + gap)).floor() as usize).max(1);
        let button_width = ((width - gap * (columns - 1) as f32) / columns as f32).max(1.0);
        let mut button_style = style(button_base, 12.0, button_base.color);
        button_style.align = 0.5;
        let button_rows = buttons.len().div_ceil(columns);
        let footer_height = if !buttons.is_empty() {
            button_rows as f32 * (sizes.action_height + gap)
        } else if !env.touch {
            let hint = self.label(&hints.to_uppercase(), hint_style, [x0, 0.0, width, 0.0], true);
            let h = hint.run.rows as f32 * hint_style.height + 10.0;
            let mut hint = hint;
            hint.position[1] = y_end - h + 6.0;
            layout.fixed_text.push(hint);
            h
        } else {
            0.0
        };
        let footer_top = y_end - footer_height;
        for (i, (action, label)) in buttons.iter().enumerate() {
            let row = i / columns;
            let at = x0 + (i % columns) as f32 * (button_width + gap);
            let by = footer_top + gap + row as f32 * (sizes.action_height + gap);
            let mut line = self.label(&label.to_uppercase(), button_style, [at, by, button_width, 0.0], true);
            line.position[1] = by + (sizes.action_height - line.run.rows as f32 * button_style.height) * 0.5;
            layout.fixed_text.push(line);
            layout.cards.push(Card {
                rect: [at, by, button_width, sizes.action_height],
                action: *action,
                accent: 3.0,
                members: 0..0,
                fixed: true,
            });
        }

        // ---- the galaxy's region and the list's column
        let body = (footer_top - header_end).max(1.0);
        let wide = width >= 700.0;
        let (galaxy, list_x, list_w, list_top) = if wide {
            let gw = (width * 0.36).min(360.0);
            (
                [frame[0], header_end, x0 - frame[0] + gw, body],
                x0 + gw + 20.0,
                width - gw - 20.0,
                header_end,
            )
        } else if body >= 320.0 {
            let gh = (body * 0.28).min(170.0);
            ([frame[0], header_end, frame[2], gh], x0, width, header_end + gh + 6.0)
        } else {
            // no room for both: the galaxy stays behind the list
            ([frame[0], header_end, frame[2], body], x0, width, header_end)
        };
        layout.galaxy = galaxy;
        let clip = [list_x, list_top, list_w.max(1.0), (footer_top - list_top - 4.0).max(1.0)];

        // ---- the list
        let pad = sizes.card_padding.min((list_w * 0.25).max(4.0));
        let text_x = list_x + pad + 4.0;
        let text_w = (list_w - 2.0 * pad - 4.0).max(1.0);
        let mut y = list_top;
        for group in &groups {
            let start = y;
            let n = group.members.len();
            let selected = group.members.contains(&self.selected);
            let shown = if selected { self.selected } else { group.members.start };
            let d = &state.diagnostics[shown];
            y += pad - 2.0;
            // kind line: ROOT CAUSE · RR0001 UNRESOLVED IMPORT ×4
            let kind = format!(
                "{} · {}{}{}",
                if group.root { &root_label } else { &follow_label }.to_uppercase(),
                group.code,
                if group.title.is_empty() { String::new() } else { format!(" {}", group.title.to_uppercase()) },
                if n > 1 { format!("  ×{n}") } else { String::new() },
            );
            let s = style(status_style, 9.5, if group.root { EMBER } else { DIM });
            y += self.flow(&mut layout, &kind, s, text_x, y, text_w) + 3.0;
            y += self.flow(&mut layout, &group.summary, error_style, text_x, y, text_w) + error_style.gap;
            if !d.file.is_empty() {
                let at = format!("{}:{}:{}", d.file, d.line, d.col);
                y += self.flow(&mut layout, &at, location_style, text_x, y, text_w) + location_style.gap;
            }
            let mut detail = Vec::new();
            if let Some(item) = &d.item {
                detail.push(item.clone());
            }
            for note in &d.notes {
                detail.push(format!("note: {note}"));
            }
            if let Some(help) = &d.suggestion {
                detail.push(format!("help: {}", help.message));
            }
            if !detail.is_empty() {
                let s = style(location_style, 11.0, SOFT);
                y += self.flow(&mut layout, &detail.join("\n"), s, text_x, y, text_w) + 4.0;
            }
            // the members of a folded group, one row each: the first ROWS and the selected one
            let mut rows = Vec::new();
            if n > 1 {
                y += 4.0;
                let row_style = style(location_style, 10.5, SOFT);
                let mut listed = 0;
                for i in group.members.clone() {
                    let first = i - group.members.start < ROWS || n <= ROWS + 1;
                    if !first && i != self.selected {
                        continue;
                    }
                    listed += 1;
                    let m = &state.diagnostics[i];
                    let what = if m.code == "RR0001" {
                        m.message.trim_start_matches("unresolved import ").trim_matches('`').to_string()
                    } else {
                        m.message.lines().next().unwrap_or("").to_string()
                    };
                    let text = if m.file.is_empty() {
                        what
                    } else {
                        format!("{}:{}   {what}", short_path(&m.file), m.line)
                    };
                    let mut s = row_style;
                    if i == self.selected {
                        s.color = error_style.color;
                    }
                    let h = self.flow(&mut layout, &text, s, text_x + 8.0, y + 2.0, text_w - 8.0) + 4.0;
                    rows.push(Card {
                        rect: [text_x - 2.0, y, text_w + 2.0, h],
                        action: Action::Select(i),
                        accent: 2.0,
                        members: i..i + 1,
                        fixed: false,
                    });
                    y += h;
                }
                if listed < n {
                    let s = style(location_style, 10.0, DIM);
                    let more = format!("+{} more · ↑/↓ steps through them", n - listed);
                    y += self.flow(&mut layout, &more, s, text_x + 8.0, y + 2.0, text_w - 8.0) + 2.0;
                }
            }
            y += pad - 2.0;
            layout.cards.push(Card {
                rect: [list_x, start, list_w, y - start],
                action: Action::Select(group.members.start),
                accent: if group.root { 1.0 } else { 0.0 },
                members: group.members.clone(),
                fixed: false,
            });
            layout.cards.extend(rows);
            y += 8.0;
        }
        if let Some(panic) = &state.parked {
            let start = y;
            y += pad - 2.0;
            let s = style(status_style, 9.5, EMBER);
            y += self.flow(&mut layout, "PANIC", s, text_x, y, text_w) + 3.0;
            y += self.flow(&mut layout, &panic.location, location_style, text_x, y, text_w) + location_style.gap;
            y += self.flow(&mut layout, &panic.message, panic_style, text_x, y, text_w) + panic_style.gap;
            if !env.touch && !panic.choices.is_empty() {
                let choices: Vec<String> = panic
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}  {}", i + 1, c.to_uppercase()))
                    .collect();
                let s = style(status_style, 10.0, error_style.color);
                let text = format!("{}   {}", resume_label.to_uppercase(), choices.join("     "));
                y += 4.0 + self.flow(&mut layout, &text, s, text_x, y + 4.0, text_w);
            }
            y += pad - 2.0;
            let index = state.diagnostics.len();
            layout.cards.push(Card {
                rect: [list_x, start, list_w, y - start],
                action: Action::Select(index),
                accent: 1.0,
                members: index..index + 1,
                fixed: false,
            });
            y += 8.0;
            if env.touch && self.input.is_none() {
                for (i, choice) in panic.choices.iter().enumerate() {
                    let label = format!("{resume_label} · {choice}").to_uppercase();
                    let height = sizes.action_height;
                    let mut line = self.label(&label, button_style, [list_x, y, list_w, 0.0], false);
                    line.position[1] = y + (height - line.run.rows as f32 * button_style.height) * 0.5;
                    layout.text.push(line);
                    layout.cards.push(Card {
                        rect: [list_x, y, list_w, height],
                        action: Action::Resume(i),
                        accent: 3.0,
                        members: 0..0,
                        fixed: false,
                    });
                    y += height + gap;
                }
            }
        }
        if let Some(input) = self.input.clone() {
            let text = format!("{input_label}\n{input}│");
            y += self.flow(&mut layout, &text, error_style, text_x, y, text_w) + 16.0;
        }
        layout.clip = clip;
        layout.content_height = (y - list_top).max(0.0);
        // fixed cards last; the list's cards in order of their tops (the frame's binary search)
        layout.cards.sort_by(|a, b| a.fixed.cmp(&b.fixed).then(a.rect[1].total_cmp(&b.rect[1])));
        layout
            .text
            .sort_by(|a, b| a.position[1].total_cmp(&b.position[1]));
        let mut end = 0.0_f32;
        layout.text_ends = layout
            .text
            .iter()
            .map(|line| {
                end = end.max(line.position[1] + line.run.rows as f32 * line.style.height);
                end
            })
            .collect();
        if self.reveal_selection {
            if let Some(card) = layout
                .cards
                .iter()
                .rev()
                .find(|c| !c.fixed && c.members.contains(&self.selected))
            {
                self.scroll = (card.rect[1] - list_top - 8.0).max(0.0);
            }
            self.reveal_selection = false;
        }
        if self.input.is_some() {
            self.scroll = (layout.content_height - layout.clip[3]).max(0.0);
        }
        self.text.end_layout();
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.pressed = None;
        self.layout = layout;
    }
}

fn style_size(base: Line, size: f32) -> Line {
    style(base, size, base.color)
}
