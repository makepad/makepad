//! A small text/pass layout, in HAL logical points (dp on Android).
use crate::{
    document::{Line, Role},
    Action, Card, Environment, Layout, Mode, Renderer, State, TextLine,
};

use makepad_script::{id, LiveId};

/// The close button's side, in points.
const CLOSE_SIZE: f32 = 28.0;

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

    pub(super) fn layout(&mut self, state: &State, size: [f32; 2], env: Environment) {
        let dark = env.dark.unwrap_or(false);
        let [left, top, right, bottom] = env.insets;
        let safe = [
            left.min(size[0]),
            top.min(size[1]),
            (size[0] - left - right).max(1.0),
            (size[1] - top - bottom).max(1.0),
        ];
        let program = self.active.as_ref().unwrap();
        let style = program.layout(state.mode, [safe[2], safe[3]]);
        let app_style = program.line(Role::App, dark);
        let status_style = program.line(Role::Status, dark);
        let current_style = program.line(Role::Current, dark);
        let location_style = program.line(Role::Location, dark);
        let error_style = program.line(Role::Errors, dark);
        let domain_style = program.line(Role::Domain, dark);
        let hint_style = program.line(Role::Hints, dark);
        let panic_style = program.line(Role::Panic, dark);
        let progress_style = program.line(Role::Progress, dark);
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
        let resume_label = program.label(id!(tap_resume));
        let input_label = program.label(id!(input_label));
        let phase_label = program.label(match state.phase {
            relax_base::run_state::Phase::Resolve => id!(resolve),
            relax_base::run_state::Phase::Parse => id!(parse),
            relax_base::run_state::Phase::Typecheck => id!(typecheck),
            relax_base::run_state::Phase::Codegen => id!(codegen),
            relax_base::run_state::Phase::Link => id!(link),
            relax_base::run_state::Phase::FirstFrame => id!(first_frame),
        });
        let has_icon = self.icon_aspect.is_some() && program.pipes.len() > crate::document::ICON;
        let overlay = state.mode == Mode::Overlay;
        let detail = overlay || !state.diagnostics.is_empty() || state.parked.is_some();
        let margin = style
            .margin
            .min((safe[2].min(safe[3]) * 0.5 - 1.0).max(0.0));
        let mut panel = [
            safe[0] + margin,
            safe[1] + margin,
            (safe[2] - 2.0 * margin).max(1.0),
            (safe[3] - 2.0 * margin).max(1.0),
        ];
        if style.max_width > 0.0 {
            panel[2] = panel[2].min(style.max_width);
        }
        if style.max_height > 0.0 {
            panel[3] = panel[3].min(style.max_height);
        }
        panel[0] = safe[0] + (safe[2] - panel[2]) * 0.5;
        let padding = style
            .padding
            .min((panel[2].min(panel[3]) * 0.5 - 1.0).max(0.0));
        let x = panel[0] + padding;
        let width = (panel[2] - padding * 2.0).max(1.0);
        let mut y = panel[1] + padding + style.top;
        let end = panel[1] + panel[3] - padding;
        let mut layout = Layout {
            panel,
            ..Layout::default()
        };
        self.text.begin_layout();

        // A fixed footer on touch; panic choices stay with the scrollable panic.
        // Desktop keeps the original key bindings and their document-owned hints.
        let mut buttons = Vec::new();
        if env.touch && detail {
            if self.input.is_some() {
                buttons.push((Action::Submit, labels[4].clone()));
                buttons.push((Action::Cancel, labels[5].clone()));
            } else {
                if state.can_retry || state.can_restart {
                    buttons.push((Action::Restart, labels[0].clone()));
                }
                if !state.diagnostics.is_empty() || state.parked.is_some() {
                    buttons.push((Action::Copy, labels[1].clone()));
                }
                if state
                    .diagnostics
                    .get(self.selected)
                    .is_some_and(|d| !d.file.is_empty())
                {
                    buttons.push((Action::Open, labels[2].clone()));
                }
                if overlay {
                    buttons.push((Action::Dismiss, labels[3].clone()));
                }
            }
        }
        let gap = style.action_gap;
        let action_height = style.action_height;
        let columns = (((width + gap) / (style.action_width + gap)).floor() as usize).max(1);
        let button_width = ((width - gap * (columns - 1) as f32) / columns as f32).max(1.0);
        let mut button_style = current_style;
        button_style.align = 0.5;
        let mut button_runs = Vec::new();
        let mut row_heights = Vec::<f32>::new();
        for (i, (action, label)) in buttons.iter().enumerate() {
            let text = self.label(
                label,
                button_style,
                [0.0, 0.0, (button_width - 24.0).max(1.0), 0.0],
                true,
            );
            let row = i / columns;
            if row == row_heights.len() {
                row_heights.push(style.action_height);
            }
            row_heights[row] =
                row_heights[row].max(text.run.rows as f32 * button_style.height + 16.0);
            button_runs.push((*action, text));
        }
        let footer_height = if !row_heights.is_empty() {
            row_heights.iter().sum::<f32>() + gap * row_heights.len() as f32
        } else if !env.touch && detail {
            let text = self.label(&hints, hint_style, [x, 0.0, width, 0.0], true);
            let height = text.run.rows as f32 * hint_style.height + 16.0;
            layout.fixed_text.push(text);
            height
        } else {
            0.0
        };
        // If the keyboard or a very short window leaves too little space, move
        // the footer into the scroll flow, retaining every action and every line.
        let fixed_footer = footer_height + 120.0 <= end - y;
        let content_end = if fixed_footer {
            end - footer_height
        } else {
            end
        };

        let mut title_style = app_style;
        if detail {
            title_style.size = title_style.size.min(24.0);
            title_style.height = title_style.height.min(32.0);
            title_style.align = 0.0;
        }
        let title_x = if detail && has_icon { x + 44.0 } else { x };
        // the close button's corner stays clear of a left-aligned title
        let title_width = (width - (title_x - x) - if detail && !env.touch { CLOSE_SIZE } else { 0.0 }).max(1.0);
        let title = self.label(
            &state.app,
            title_style,
            [title_x, y, title_width, 0.0],
            detail,
        );
        let title_height = title.run.rows as f32 * title_style.height;
        // A long name also remains reachable on very small windows.
        let fixed_header = detail && title_height + 96.0 < content_end - y;
        let mut title = title;
        title.fixed = fixed_header;
        if detail {
            if has_icon {
                layout.icon = Some([x, y, 32.0, 32.0]);
                layout.icon_fixed = fixed_header;
            }
            if fixed_header {
                layout.fixed_text.push(title);
            } else {
                layout.text.push(title);
            }
            y += title_height.max(if has_icon { 32.0 } else { 0.0 }) + 12.0;
            let value = if state.parked.is_some() {
                &paused_label
            } else {
                &state.domain
            };
            let mut style = domain_style;
            style.align = 0.0;
            let line = self.label(value, style, [x, y, width, 0.0], fixed_header);
            y += line.run.rows as f32 * style.height + 16.0;
            if fixed_header {
                layout.fixed_text.push(line);
            } else {
                layout.text.push(line);
            }
        } else {
            // Portrait/desktop: one centred group. Short landscape: identity
            // on the left, progress and current work on the right.
            let split = style.columns.clamp(0.0, 0.8);
            let hero_width = if split > 0.0 { width * split } else { width };
            let group_top = y;
            if has_icon {
                let icon_size = style.icon_size.min(hero_width);
                let aspect = self.icon_aspect.unwrap();
                let w = icon_size.min(icon_size * aspect);
                let h = icon_size.min(icon_size / aspect);
                layout.icon = Some([x + (hero_width - w) * 0.5, y, w, h]);
                y += h + app_style.gap;
            }
            let line = self.label(&state.app, title_style, [x, y, hero_width, 0.0], false);
            let hero_end = y + line.run.rows as f32 * title_style.height + title_style.gap;
            layout.text.push(line);
            let (flow_x, flow_width) = if split > 0.0 {
                y = group_top + 16.0;
                (x + hero_width + 32.0, (width - hero_width - 32.0).max(1.0))
            } else {
                y = hero_end;
                (x, width)
            };
            layout.progress = Some([flow_x, y, flow_width, progress_style.height]);
            y += progress_style.height + progress_style.gap;
            let line = self.label(
                &phase_label,
                status_style,
                [flow_x, y, flow_width, 0.0],
                false,
            );
            y += line.run.rows as f32 * status_style.height + status_style.gap;
            layout.text.push(line);
            if !state.current.is_empty() {
                let line = self.label(
                    &state.current,
                    current_style,
                    [flow_x, y, flow_width, 0.0],
                    false,
                );
                y += line.run.rows.max(2) as f32 * current_style.height + current_style.gap;
                layout.text.push(line);
            } else {
                y += 2.0 * current_style.height + current_style.gap;
            }
            y = y.max(hero_end);
            let offset =
                ((content_end - group_top) - (y - group_top)).max(0.0) * style.center.min(1.0);
            for line in &mut layout.text {
                line.position[1] += offset;
            }
            if let Some(rect) = &mut layout.icon {
                rect[1] += offset;
            }
            if let Some(rect) = &mut layout.progress {
                rect[1] += offset;
            }
            y += offset;
        }
        let content_top = if fixed_header {
            y
        } else {
            panel[1] + padding + style.top
        };
        let card_padding = style.card_padding.min((width * 0.5 - 1.0).max(0.0));
        for (i, diagnostic) in state.diagnostics.iter().enumerate() {
            let start = y;
            y += card_padding;
            if !diagnostic.file.is_empty() {
                let location =
                    format!("{}:{}:{}", diagnostic.file, diagnostic.line, diagnostic.col);
                let mut style = location_style;
                style.align = 0.0;
                let line = self.label(
                    &location,
                    style,
                    [x + card_padding, y, width - 2.0 * card_padding, 0.0],
                    false,
                );
                y += line.run.rows as f32 * style.height + style.gap;
                layout.text.push(line);
            }
            let mut diagnostic = diagnostic.clone();
            diagnostic.file.clear();
            let mut style = error_style;
            style.align = 0.0;
            let line = self.label(
                &crate::diagnostic_text(&diagnostic),
                style,
                [x + card_padding, y, width - 2.0 * card_padding, 0.0],
                false,
            );
            y += line.run.rows as f32 * style.height + card_padding;
            layout.text.push(line);
            layout.cards.push(Card {
                rect: [x, start, width, y - start],
                action: Action::Select(i),
                button: false,
                fixed: false,
            });
            y += error_style.gap;
        }
        if let Some(panic) = &state.parked {
            let mut style = panic_style;
            style.align = 0.0;
            let start = y;
            y += card_padding;
            let line = self.label(
                &format!("{}\n{}", panic.location, panic.message),
                style,
                [x + card_padding, y, width - card_padding * 2.0, 0.0],
                false,
            );
            y += line.run.rows as f32 * style.height + card_padding;
            layout.text.push(line);
            // The panic is selectable after the diagnostics; Copy then uses it.
            layout.cards.push(Card {
                rect: [x, start, width, y - start],
                action: Action::Select(state.diagnostics.len()),
                button: false,
                fixed: false,
            });
            y += style.gap;
            if env.touch && self.input.is_none() {
                for (i, choice) in panic.choices.iter().enumerate() {
                    let label = format!("{resume_label} · {choice}");
                    let mut line = self.label(
                        &label,
                        button_style,
                        [x + 12.0, y + 8.0, (width - 24.0).max(1.0), 0.0],
                        false,
                    );
                    let height =
                        (line.run.rows as f32 * button_style.height + 16.0).max(action_height);
                    line.position[1] =
                        y + (height - line.run.rows as f32 * button_style.height) * 0.5;
                    layout.text.push(line);
                    layout.cards.push(Card {
                        rect: [x, y, width, height],
                        action: Action::Resume(i),
                        button: true,
                        fixed: false,
                    });
                    y += height + gap;
                }
            }
        }
        if let Some(input) = self.input.clone() {
            let mut style = error_style;
            style.align = 0.0;
            let line = self.label(
                &format!("{input_label}\n{input}│"),
                style,
                [x, y, width, 0.0],
                false,
            );
            y += line.run.rows as f32 * style.height + 16.0;
            layout.text.push(line);
        }
        let mut footer_y = if fixed_footer {
            end - footer_height + gap
        } else {
            y + gap
        };
        for (i, (action, mut line)) in button_runs.into_iter().enumerate() {
            let row = i / columns;
            if i > 0 && i % columns == 0 {
                footer_y += row_heights[row - 1] + gap;
            }
            let at = x + (i % columns) as f32 * (button_width + gap);
            let height = row_heights[row];
            line.position = [
                at + (button_width - line.run.width).max(0.0) * 0.5,
                footer_y + (height - line.run.rows as f32 * button_style.height) * 0.5,
            ];
            line.fixed = fixed_footer;
            if fixed_footer {
                layout.fixed_text.push(line);
            } else {
                layout.text.push(line);
            }
            layout.cards.push(Card {
                rect: [at, footer_y, button_width, height],
                action,
                button: true,
                fixed: fixed_footer,
            });
        }
        if !buttons.is_empty() && !fixed_footer {
            y = footer_y + row_heights.last().copied().unwrap_or(0.0);
        }
        if !env.touch && detail {
            // The hint was shaped before the header to reserve its space.
            let mut hint = layout.fixed_text.remove(0);
            hint.position[1] = if fixed_footer {
                end - footer_height + 8.0
            } else {
                y + 8.0
            };
            hint.fixed = fixed_footer;
            if fixed_footer {
                layout.fixed_text.push(hint);
            } else {
                y = hint.position[1] + hint.run.rows as f32 * hint.style.height;
                layout.text.push(hint);
            }
        }
        if !env.touch {
            // The close button in the panel's top right corner: the window has no caption of
            // its own (it drags from anywhere else, `Boot::drag_answer`). Fixed cards come last.
            let rect = [panel[0] + panel[2] - CLOSE_SIZE - 8.0, panel[1] + 8.0, CLOSE_SIZE, CLOSE_SIZE];
            let mut style = current_style;
            style.align = 0.5;
            style.size = style.size.max(16.0);
            style.height = CLOSE_SIZE;
            let mut line = self.label("\u{00d7}", style, [rect[0], rect[1], rect[2], 0.0], true);
            line.position[1] = rect[1];
            layout.fixed_text.push(line);
            layout.cards.push(Card { rect, action: Action::Close, button: true, fixed: true });
        }
        layout.clip = [x, content_top, width, (content_end - content_top).max(1.0)];
        layout.content_height = (y - content_top).max(0.0);
        // Landscape has two independent columns. Sort once when laying out so
        // the frame's visible-line binary search remains valid.
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
                .find(|c| c.action == Action::Select(self.selected))
            {
                self.scroll = (card.rect[1] - content_top).max(0.0);
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
