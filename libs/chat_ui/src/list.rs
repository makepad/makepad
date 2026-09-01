//! The transcript on screen: one virtual list of bubbles, tool chips and
//! the think indicator, reading [`crate::transcript::CHAT`] during draw.
//!
//! One widget, two apps. Styling is data: a host that wants another palette
//! re-declares `mod.widgets.AssetChatList` in its own `script_mod` (after
//! this crate's) with its own templates — it never forks this file.

use crate::transcript::{ChatData, ChatRole, CHAT};
use makepad_widgets::*;

/// How much of the live reasoning the porthole shows. It is a WINDOW on
/// the thought, not a log: enough to read the current clause, no more.
const THOUGHT_TAIL_CHARS: usize = 220;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The think-phase indicator: three soft dots pulsing in sequence where
    // the reply will appear. The shader runs on the pass clock
    // (LoadingSpinner's pattern); the Rust widget only pumps frames while
    // it is on screen.
    mod.widgets.ThinkingDotsBase = #(ThinkingDots::register_widget(vm))
    mod.widgets.ThinkingDots = set_type_default() do mod.widgets.ThinkingDotsBase {
        width: 44
        height: 16
        draw_dots +: {
            color: uniform(#x9fb4d0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let r = min(self.rect_size.y * 0.22, 3.5)
                let cy = self.rect_size.y * 0.5
                let base = self.color
                let w = self.draw_pass.time * 5.2

                let p1 = 0.25 + 0.75 * max(0.0, sin(w))
                sdf.circle(self.rect_size.x * 0.2, cy, r * 2.0)
                sdf.fill(vec4(base.xyz * p1, p1))

                let p2 = 0.25 + 0.75 * max(0.0, sin(w - 1.1))
                sdf.circle(self.rect_size.x * 0.5, cy, r * 2.0)
                sdf.fill(vec4(base.xyz * p2, p2))

                let p3 = 0.25 + 0.75 * max(0.0, sin(w - 2.2))
                sdf.circle(self.rect_size.x * 0.8, cy, r * 2.0)
                sdf.fill(vec4(base.xyz * p3, p3))

                return sdf.result
            }
        }
    }

    mod.widgets.AssetChatListBase = #(AssetChatList::register_widget(vm))
    mod.widgets.AssetChatList = set_type_default() do mod.widgets.AssetChatListBase {
        width: Fill
        height: Fill

        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: false
            auto_tail: true
            smooth_tail: true
            selectable: true

            User := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 6 bottom: 6 left: 40 right: 4}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #x2a3a5a
                    radius: 8.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #xe8eef8
                    draw_text.text_style: theme.font_regular{font_size: 13}
                }
            }

            Assistant := RoundedView {
                width: Fill
                height: Fit
                flow: Down
                margin: Inset{top: 6 bottom: 6 left: 4 right: 40}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #x232330
                    radius: 8.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #xd8dee8
                    draw_text.text_style: theme.font_regular{font_size: 13}
                }
                // How fast this reply came out, pinned once it landed.
                // Same dim/small key as the tool chips' detail line — it
                // is a footnote, not a panel.
                meta := Label {
                    width: Fill
                    height: Fit
                    visible: false
                    margin: Inset{top: 2}
                    text: ""
                    draw_text.color: #x6f7c90
                    draw_text.text_style: theme.font_regular{font_size: 10}
                }
            }

            System := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 6 bottom: 6 left: 4 right: 4}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #x3a2a24
                    radius: 8.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #xe8c9a0
                    draw_text.text_style: theme.font_regular{font_size: 12}
                }
            }

            // The think-phase row: dots pulse where the reply will appear,
            // with the phase text muted beside them; the first streamed
            // token replaces the whole row (drawn as Assistant).
            Thinking := RoundedView {
                width: Fill
                height: Fit
                flow: Down
                spacing: 6
                margin: Inset{top: 6 bottom: 6 left: 4 right: 40}
                padding: Inset{left: 12 top: 10 right: 12 bottom: 10}
                show_bg: true
                draw_bg +: {
                    color: #x232330
                    radius: 8.0
                }
                View {
                    width: Fill
                    flow: Right
                    spacing: 8
                    align: Align{y: 0.5}
                    // Full path on purpose: a bare `ThinkingDots` does not
                    // resolve from the same script_mod block that registers
                    // it (the use-glob snapshot predates the assignment).
                    dots := mod.widgets.ThinkingDots {}
                    phase_text := Label {
                        width: Fit
                        height: Fit
                        text: ""
                        draw_text.color: #x6f7c90
                        draw_text.text_style: theme.font_regular{font_size: 10}
                    }
                }
                // The thoughts porthole: a FIXED-height window the live
                // reasoning tail scrolls through. Constant geometry is the
                // whole point — the bubble never judders, the text inside
                // it moves instead.
                thought_text := Label {
                    width: Fill
                    height: 46
                    text: ""
                    max_lines: 3
                    draw_text.color: #x525d70
                    draw_text.text_style: theme.font_regular{font_size: 9}
                }
            }

            // A tool call as a compact chip; clicking it unfolds the full
            // arguments and result under the summary line.
            Tool := RoundedView {
                width: Fill
                height: Fit
                flow: Down
                margin: Inset{top: 4 bottom: 4 left: 4 right: 40}
                padding: Inset{left: 6 top: 2 right: 6 bottom: 2}
                show_bg: true
                draw_bg +: {
                    color: #x1a2430
                    radius: 6.0
                }
                chip := ButtonFlatter {
                    width: Fill
                    height: Fit
                    align: Align{x: 0.0 y: 0.5}
                    text: ""
                    draw_text +: {
                        color: #x9fc4e8
                        text_style: theme.font_regular{font_size: 11}
                    }
                }
                detail := Label {
                    width: Fill
                    height: Fit
                    visible: false
                    margin: Inset{left: 8 bottom: 6}
                    text: ""
                    draw_text.color: #x8fa2b8
                    draw_text.text_style: theme.font_regular{font_size: 10}
                }
            }
        }
    }
}

/// The think indicator: the shader animates on the pass clock; this widget
/// only draws the quad and pumps NextFrame redraws while it is on screen,
/// so the pulse stays smooth even when nothing else repaints.
#[derive(Script, ScriptHook, Widget)]
pub struct ThinkingDots {
    #[uid]
    uid: WidgetUid,
    #[rust]
    next_frame: NextFrame,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_dots: DrawQuad,
}

impl Widget for ThinkingDots {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_dots.draw_abs(cx, rect);
        cx.end_turtle();
        self.next_frame = cx.new_next_frame();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.draw_dots.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct AssetChatList {
    #[deref]
    view: View,
}

impl Widget for AssetChatList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Ok(data) = CHAT.read() else {
            return DrawStep::done();
        };
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let portal = item.as_portal_list();
            let Some(mut list) = portal.borrow_mut() else {
                continue;
            };
            let msg_count = data.messages.len();
            list.set_item_range(cx, 0, msg_count + data.is_streaming as usize);

            while let Some(item_id) = list.next_visible_item(cx) {
                // The streaming reply is a virtual item past the end.
                if data.is_streaming && item_id == msg_count {
                    // Until the first token streams, the row is the pulsing
                    // dots with the phase text muted beside them (a
                    // full-size "waiting for the model..." bubble reads as
                    // a stalled reply). Tokens replace the row instantly.
                    // WHITESPACE is not text: `strip_marker` leaves a
                    // newline behind after a tool line, and drawing that as
                    // an assistant bubble put an empty grey box on screen
                    // that reads as broken.
                    if data.streaming_text.trim().is_empty() {
                        let widget = list.item(cx, item_id, id!(Thinking));
                        // Before the first serving facts arrive there is no
                        // phase to name and no reasoning yet — but a bubble
                        // with nothing in it reads as broken, not as
                        // waiting. It always says what it is doing.
                        let phase = if data.activity.is_empty() {
                            "thinking…"
                        } else {
                            data.activity.as_str()
                        };
                        widget.label(cx, ids!(phase_text)).set_text(cx, phase);
                        // Tail of the live reasoning: the last few clauses,
                        // single-spaced, so the porthole reads as thought
                        // scrolling by rather than a growing log.
                        let tail: String = {
                            let t = data.thinking_text.replace('\n', " ");
                            let chars: Vec<char> = t.chars().collect();
                            let keep = THOUGHT_TAIL_CHARS.min(chars.len());
                            chars[chars.len() - keep..].iter().collect()
                        };
                        widget.label(cx, ids!(thought_text)).set_text(cx, &tail);
                        widget.draw_all_unscoped(cx);
                        continue;
                    }
                    let mut text = data.streaming_text.clone();
                    if !data.activity.is_empty() {
                        text.push_str("\n\n");
                        text.push_str(&data.activity);
                    }
                    let widget = list.item(cx, item_id, id!(Assistant));
                    widget.label(cx, ids!(body)).set_text(cx, &text);
                    // While it streams, the rate lives in the host's status
                    // strip (one home for the live number); the footnote
                    // appears when the reply lands, carrying its average.
                    widget.label(cx, ids!(meta)).set_visible(cx, false);
                    widget.draw_all_unscoped(cx);
                    continue;
                }
                let Some(msg) = data.messages.get(item_id) else {
                    continue;
                };
                if msg.role == ChatRole::Tool {
                    let widget = list.item(cx, item_id, id!(Tool));
                    // ASCII on purpose: the bundled UI font has no triangle
                    // glyphs and renders them as tofu boxes.
                    let arrow = if msg.expanded { "[-]" } else { "[+]" };
                    widget
                        .button(cx, ids!(chip))
                        .set_text(cx, &format!("{arrow} {}", msg.text));
                    let detail = widget.label(cx, ids!(detail));
                    detail.set_visible(cx, msg.expanded);
                    if msg.expanded {
                        detail.set_text(cx, msg.detail.as_deref().unwrap_or(""));
                    }
                    widget.draw_all_unscoped(cx);
                    continue;
                }
                let template = match msg.role {
                    ChatRole::User => id!(User),
                    ChatRole::Assistant => id!(Assistant),
                    ChatRole::System => id!(System),
                    ChatRole::Tool => unreachable!("handled above"),
                };
                let widget = list.item(cx, item_id, template);
                widget.label(cx, ids!(body)).set_text(cx, &msg.text);
                if msg.role == ChatRole::Assistant {
                    let meta = widget.label(cx, ids!(meta));
                    match &msg.meta {
                        Some(text) => {
                            meta.set_visible(cx, true);
                            meta.set_text(cx, text);
                        }
                        None => meta.set_visible(cx, false),
                    }
                }
                widget.draw_all_unscoped(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl AssetChatList {
    /// Toggle tool chips on click. The host forwards its actions here.
    pub fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let list = self.portal_list(cx, ids!(list));
        let mut toggled = false;
        for (index, item) in list.items_with_actions(actions) {
            if item.button(cx, ids!(chip)).clicked(actions) {
                ChatData::toggle_tool(index);
                toggled = true;
            }
        }
        if toggled {
            self.view.redraw(cx);
        }
    }
}
