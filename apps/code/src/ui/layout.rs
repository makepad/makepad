//! The code-chat's declarative layout and its whole look:
//! the conversation with its HUD, plan and composer (reference chips, the @
//! file picker), the status bar and the command palette. Behaviour is in `view`; colours
//! and type come from `theme` (`mod.look.*`, `theme.*`).
//!
//! The look is a terminal's: one monospaced face, rows on a 4-point rhythm,
//! hairlines and chamfered frames instead of cards, a 22-point gutter at the
//! left of the transcript holding each row's mark (› you, ∴ reasoning, the
//! tool's state), and glow kept for the one thing that is live.

use super::view::AgentView;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.sdf.Finish

    mod.widgets.AgentViewBase = #(AgentView::register_widget(vm))

    // ---- type ------------------------------------------------------------

    let Mono = Label{
        width: Fit
        height: Fit
        padding: 0
        text: ""
        draw_selection +: { color: mod.look.agent_select }
        // File paths in the text are links (the view finds them).
        link_color: mod.look.agent
        draw_link +: { color: mod.look.agent }
        draw_text +: {
            color: mod.look.text
            text_style: mod.look.mono{font_size: 9.0}
        }
    }

    let Dim = Mono{
        width: Fill
        draw_text +: {
            color: mod.look.meta
            text_style: mod.look.mono{font_size: 9.0}
        }
    }

    /// A section head: `SESSIONS ───────`, small caps-weight mono.
    let Caption = Mono{
        draw_text +: {
            color: mod.look.dim
            text_style: mod.look.mono_bold{font_size: 7.5}
        }
    }

    let Rule = View{
        width: Fill
        height: 1
        show_bg: true
        draw_bg +: {
            color: mod.look.hair_hi
            pixel: fn() {
                return vec4(self.color.rgb * self.color.a, self.color.a)
            }
        }
    }

    /// A short upright rule between status fields.
    let Sep = Rule{ width: 1 height: 12 }

    let Section = View{
        width: Fill
        height: 20
        flow: Right
        spacing: 8
        align: Align{y: 0.5}
        padding: Inset{left: 2 right: 2}
    }

    /// The mark every transcript row starts with: a 12-point column 4 in
    /// from the edge and 6 before the text, so text starts on the 22-point
    /// gutter and the mark's centre (x = 10) lines up with the hairlines
    /// under running commands.
    let Glyph = Mono{
        selectable: false
        width: 12
        margin: Inset{right: 6}
        align: Align{x: 0.5}
        draw_text +: {
            color: mod.look.meta
            text_style: mod.look.mono_bold{font_size: 9.0}
        }
    }

    // ---- controls --------------------------------------------------------

    /// A key button: a hairline frame with chamfered corners, the label in
    /// mono, a wash of its ink on hover. `ink` is the frame and label ink.
    let KeyBtn = ButtonFlat{
        height: 22
        padding: Inset{left: 10 right: 10 top: 0 bottom: 0}
        margin: 0
        draw_text +: {
            color: mod.look.text
            color_hover: mod.look.text_hi
            color_down: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            text_style: mod.look.mono{font_size: 8.5}
        }
        draw_bg +: {
            ink: uniform(mod.look.meta)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                let d = Finish.sd_chamfer(p, c, c - vec2(0.5, 0.5), 4.0)
                let edge = Finish.band(d, 0.0, px, px)
                let fill = Finish.cover(d, px)
                let wash = 0.05 + 0.12 * self.hover + 0.22 * self.down
                let a = clamp(fill * wash + edge * (0.55 + 0.45 * max(self.hover, self.focus)), 0.0, 1.0)
                return vec4(self.ink.rgb * a, a)
            }
        }
    }

    let Btn = KeyBtn{}
    let BtnFlat = KeyBtn{}

    let Field = TextInput{
        draw_text +: {
            color: mod.look.text_hi
            color_hover: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            color_down: uniform(mod.look.text_hi)
            color_empty: uniform(mod.look.dim)
            color_empty_hover: uniform(mod.look.dim)
            color_empty_focus: uniform(mod.look.dim)
            text_style: mod.look.mono{font_size: 9.5}
        }
        draw_cursor +: { color: mod.look.agent }
        draw_selection +: {
            color: mod.look.agent_select
            color_hover: mod.look.agent_select
            color_focus: mod.look.agent_select
        }
        draw_bg +: {
            color: #0000
            color_hover: #0000
            color_focus: #0000
            color_empty: #0000
            color_empty_hover: #0000
            color_empty_focus: #0000
            border_size: 0.0
        }
    }

    /// The code views (answers' code blocks, the editor pane): mono, no
    /// fill of their own, a syntax palette tuned to the neon pair.
    let CodeInk = CodeView{
        editor +: {
            draw_bg +: { color: #0000 }
            draw_text +: { text_style: mod.look.mono{font_size: 8.5} }
            draw_gutter +: {
                text_style: mod.look.mono{font_size: 8.0}
                color: mod.look.dim
            }
            token_colors +: {
                whitespace: #x2a3440
                delimiter: #x8a97a8
                delimiter_highlight: #xeaf2f8
                unknown: #xc3cdd8
                branch_keyword: #xff7ab4
                constant: #xc6ef8e
                identifier: #xc3cdd8
                macro_identifier: #xb39dff
                attribute: #xb39dff
                loop_keyword: #xff7ab4
                number: #xc6ef8e
                other_keyword: #x5fdcea
                punctuator: #x7d8a9a
                string: #xffcf73
                function: #xeaf2f8
                typename: #x7fe6c8
                comment: #x506273
            }
        }
    }

    // ---- surfaces --------------------------------------------------------

    /// A pane: a translucent wash over the window's ground, so the texture
    /// shows through, with one hairline on the edge facing the centre.
    let Pane = View{
        show_bg: true
        draw_bg +: {
            color: mod.look.glass
            /** which edge carries the hairline: 0 right, 1 left, 2 top */
            edge: uniform(0.0)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                var d = self.rect_size.x - p.x
                if self.edge > 0.5 {
                    d = p.x
                }
                if self.edge > 1.5 {
                    d = p.y
                }
                let line = 1.0 - step(px, d)
                let c = mix(self.color, mod.look.hair, line)
                return vec4(c.rgb * c.a, c.a)
            }
        }
    }

    // ---- small parts -------------------------------------------------------

    let PalRow = View{
        visible: false
        width: Fill
        height: 24
        flow: Right
        spacing: 10
        align: Align{y: 0.5}
        padding: Inset{left: 12 right: 12}
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {
            color: #0000
            pixel: fn() {
                let p = self.pos * self.rect_size
                // The selected row's wash carries a bar on its left.
                let bar = (1.0 - step(2.0, p.x)) * step(0.001, self.color.a)
                let c = mix(self.color, mod.look.agent, bar)
                return vec4(c.rgb * c.a, c.a)
            }
        }
        palette_title := Mono{
                                max_lines: 1
                                text_overflow: Ellipsis
            width: Fill
            draw_text +: {
                color: mod.look.text_hi
                text_style: mod.look.mono{font_size: 9.0}
            }
        }
        palette_hint := Mono{
            draw_text +: {
                color: mod.look.meta
                text_style: mod.look.mono{font_size: 8.0}
            }
        }
    }

    /// A reference chip: a hairline chamfered frame, the file glyph, the
    /// name in small mono, an × that takes it off.
    let Chip = View{
        visible: false
        width: Fit
        height: 18
        flow: Right
        spacing: 5
        align: Align{y: 0.5}
        padding: Inset{left: 6 right: 2}
        show_bg: true
        draw_bg +: {
            color: mod.look.agent_wash
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c0 = self.rect_size * 0.5
                let d = Finish.sd_chamfer(p, c0, c0 - vec2(0.5, 0.5), 4.0)
                let inside = Finish.cover(d, px)
                let edge = Finish.band(d, 0.0, px, px)
                var c = self.color * inside
                c = mix(c, vec4(mod.look.agent.rgb, 1.0), edge * 0.55)
                return vec4(c.rgb * c.a, c.a)
            }
        }
        chip_icon := Mono{
            selectable: false
            draw_text +: {
                color: mod.look.agent
                text_style: theme.font_icons{font_size: 6.5}
            }
        }
        chip_name := Mono{
            selectable: false
            draw_text +: {
                color: mod.look.text_hi
                text_style: mod.look.mono{font_size: 8.0}
            }
        }
        chip_x := View{
            width: 14
            height: Fill
            align: Align{x: 0.5 y: 0.5}
            cursor: MouseCursor.Hand
            Mono{
                selectable: false
                text: "×"
                draw_text +: {
                    color: mod.look.meta
                    text_style: mod.look.mono{font_size: 8.5}
                }
            }
        }
    }

    mod.widgets.AgentView = set_type_default() do mod.widgets.AgentViewBase{
        width: Fill
        height: Fill
        flow: Down

        // The @ file picker: rows like the palette's, floating over the
        // transcript above the composer (its own draw list: it never moves
        // the transcript or the composer).
        at_popup: View{
            width: 420
            height: Fit
            flow: Down
            padding: Inset{left: 4 right: 4 top: 4 bottom: 4}
            show_bg: true
            draw_bg +: {
                color: mod.look.rail
                pixel: fn() {
                    let p = self.pos * self.rect_size
                    let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                    let c0 = self.rect_size * 0.5
                    let d = Finish.sd_chamfer(p, c0, c0 - vec2(0.5, 0.5), 6.0)
                    let inside = Finish.cover(d, px)
                    let edge = Finish.band(d, 0.0, px, px)
                    var c = vec4(self.color.rgb, 1.0) * inside
                    c = mix(c, vec4(mod.look.agent.rgb, 1.0), edge * 0.7)
                    return vec4(c.rgb * c.a, c.a)
                }
            }
            at_row_0 := PalRow{}
            at_row_1 := PalRow{}
            at_row_2 := PalRow{}
            at_row_3 := PalRow{}
            at_row_4 := PalRow{}
            at_row_5 := PalRow{}
            at_row_6 := PalRow{}
            at_row_7 := PalRow{}
        }

        // The window's ground: ink, fine scanlines, a faint point grid, a
        // breath of the agent's colour from the top and a soft vignette.
        // Static: it is drawn with the window and costs nothing at rest.
        show_bg: true
        draw_bg +: {
            color: mod.look.bg
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                var c = self.color.rgb
                // A point every 24 points, one device pixel, barely there.
                let g = abs(fract((p + vec2(12.0, 12.0)) / 24.0) - vec2(0.5, 0.5)) * 24.0
                let dot = (1.0 - step(px, g.x)) * (1.0 - step(px, g.y))
                c = c + vec3(0.07, 0.10, 0.13) * dot
                // Haze from the top edge.
                let haze = exp(-p.y / 220.0) * 0.035
                c = c + mod.look.agent.rgb * haze * (1.0 - abs(self.pos.x - 0.5))
                // Scanlines: a 3-point pitch, 7 percent deep.
                c = c * Finish.scan(p.y, px, 3.0, 0.07)
                // Vignette toward the corners.
                let v = length((self.pos - vec2(0.5, 0.5)) * vec2(1.0, 1.15))
                c = c * (1.0 - 0.32 * smoothstep(0.3, 0.85, v))
                return vec4(c, 1.0)
            }
        }

        body := View{
            width: Fill
            height: Fill
            flow: Right

            centre := View{
                width: Fill
                height: Fill
                flow: Down
                padding: Inset{left: 20 right: 20 top: 4 bottom: 0}

                stage := View{
                width: Fill
                height: Fill
                flow: Overlay

                // Before the first message: the mark, where you are, keys.
                empty_state := View{
                    visible: false
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 4
                    align: Align{x: 0.5 y: 0.4}
                    Mono{
                        text: "▚▞"
                        draw_text +: {
                            color: mod.look.agent
                            text_style: mod.look.mono_bold{font_size: 22.0}
                        }
                    }
                    View{ width: 1 height: 6 }
                    Mono{
                        text: "M A K E P A D   C O D E"
                        draw_text +: {
                            color: mod.look.text_hi
                            text_style: mod.look.mono_bold{font_size: 10.0}
                        }
                    }
                    empty_where := Mono{
                        draw_text +: {
                            color: mod.look.meta
                            text_style: mod.look.mono{font_size: 8.5}
                        }
                    }
                    View{ width: 1 height: 10 }
                    Rule{ width: 260 }
                    View{ width: 1 height: 8 }
                    EmptyKeys := View{
                        width: 260
                        height: Fit
                        flow: Down
                        spacing: 4
                        Mono{ text: "⌘K  commands        ⌘N  new chat" draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.5} } }
                        Mono{ text: "⌘P  open file       ⌘⇧F search" draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.5} } }
                        Mono{ text: "@   attach a file" draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.5} } }
                        Mono{ text: "esc interrupt       ↑   last prompt" draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.5} } }
                    }
                }

                // Selectable like a terminal: a mouse drag selects across
                // rows (never scrolls), Cmd+A takes the whole transcript.
                transcript := PortalList{
                    width: Fill
                    height: Fill
                    auto_tail: true
                    selectable: true

                    // You: a hairline closes the previous turn, a faint wash
                    // of your colour, the prompt glyph, your words bright.
                    UserRow := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        padding: Inset{left: 0 right: 8 top: 12 bottom: 4}
                        show_bg: true
                        draw_bg +: {
                            color: mod.look.human_wash
                            pixel: fn() {
                                let p = self.pos * self.rect_size
                                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                                // The rule sits at the top of the padding; the
                                // wash covers the text below it.
                                let rule = (1.0 - step(px, p.y)) * 0.9
                                let wash = step(8.0, p.y)
                                let bar = (1.0 - step(2.0, p.x)) * wash
                                var c = vec4(0.0, 0.0, 0.0, 0.0)
                                c = mix(c, self.color, wash)
                                c = mix(c, vec4(mod.look.human.rgb, 0.85), bar)
                                c = mix(c, mod.look.hair, rule)
                                return vec4(c.rgb * c.a, c.a)
                            }
                        }
                        Glyph{
                            margin: Inset{left: 4 right: 6}
                            text: "›"
                            draw_text +: { color: mod.look.human }
                        }
                        View{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 3
                            user_text := Mono{
                                width: Fill
                                draw_text +: {
                                    color: mod.look.text_hi
                                    text_style: mod.look.mono{font_size: 9.0}
                                }
                            }
                            // The files sent with it.
                            user_refs := Mono{
                                visible: false
                                width: Fill
                                draw_text +: {
                                    color: mod.look.agent
                                    text_style: mod.look.mono{font_size: 8.0}
                                }
                            }
                        }
                    }

                    AssistantRow := View{
                        width: Fill
                        height: Fit
                        flow: Down
                        padding: Inset{left: 22 right: 8 top: 2 bottom: 2}
                        assistant_md := Markdown{
                            width: Fill
                            height: Fit
                            padding: 0
                            // Selectable so it tracks its text; inside the
                            // selectable list the list drives the selection.
                            selectable: true
                            use_code_block_widget: true
                            body: ""
                            font_size: 9.0
                            font_color: mod.look.text
                            paragraph_spacing: 8
                            fade_chars: 24.0
                            pre_code_spacing: 6
                            heading_base_scale: 1.12
                            text_style_normal: mod.look.mono{font_size: 9.0}
                            text_style_italic: mod.look.mono{font_size: 9.0}
                            text_style_bold: mod.look.mono_bold{font_size: 9.0}
                            text_style_bold_italic: mod.look.mono_bold{font_size: 9.0}
                            text_style_fixed: mod.look.mono{font_size: 8.5}
                            // Paths are links the view hit-tests.
                            inline_links: true
                            link_color: mod.look.agent
                            inline_code_padding: Inset{left: 3 right: 3 top: 1 bottom: 1}
                            inline_code_margin: Inset{left: 1 right: 1}
                            draw_selection +: { color: mod.look.agent_select }
                            draw_block +: {
                                selection_color: mod.look.agent_select
                                line_color: mod.look.hair_hi
                                sep_color: mod.look.hair
                                quote_bg_color: mod.look.agent_wash
                                quote_fg_color: mod.look.agent
                                code_color: mod.look.agent_wash
                                table_header_bg_color: mod.look.raised
                                table_border_color: mod.look.hair
                            }
                            draw_text +: {
                                color: mod.look.text
                                // Streaming text: the newest characters arrive
                                // as phosphor, hot and tinted, and cool to the
                                // body ink over 24 chars (= fade_chars).
                                get_color: fn() {
                                    let dist = self.total_chars - self.char_index
                                    let fade = clamp(dist / 24.0, 0.0, 1.0)
                                    let hot = 1.0 - clamp(dist / 6.0, 0.0, 1.0)
                                    let rgb = mix(self.color.rgb, mix(mod.look.agent.rgb, vec3(1.0, 1.0, 1.0), 0.55), hot * 0.85)
                                    return vec4(rgb, self.color.a * pow(fade, 0.5))
                                }
                            }
                            code_block := View{
                                width: Fill
                                height: Fit
                                flow: Overlay
                                margin: Inset{top: 2 bottom: 2}
                                padding: Inset{left: 10 right: 6 top: 4 bottom: 4}
                                show_bg: true
                                draw_bg +: {
                                    color: mod.look.well
                                    pixel: fn() {
                                        let p = self.pos * self.rect_size
                                        let bar = 1.0 - step(1.0, p.x)
                                        let c = mix(self.color, mod.look.hair_hi, bar)
                                        return vec4(c.rgb * c.a, c.a)
                                    }
                                }
                                code_view := CodeInk{
                                    keep_cursor_at_end: false
                                    editor +: {
                                        height: Fit
                                        pad_left_top: vec2(0.0, 0.0)
                                    }
                                }
                            }
                        }
                    }

                    ThinkingRow := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        padding: Inset{left: 4 right: 8 top: 2 bottom: 2}
                        Glyph{
                            text: "∴"
                            draw_text +: { color: mod.look.lane_text }
                        }
                        thinking_text := Dim{
                            draw_text +: {
                                color: mod.look.lane_text
                                text_style: mod.look.mono{font_size: 9.0}
                            }
                        }
                    }

                    // A sub-agent: violet bar, its name, its state at the right.
                    LaneRow := View{
                        width: Fill
                        height: 20
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}
                        margin: Inset{top: 4 bottom: 0}
                        padding: Inset{left: 8 right: 8}
                        show_bg: true
                        draw_bg +: {
                            color: mod.look.lane_wash
                            border_color: mod.look.lane
                            pixel: fn() {
                                let p = self.pos * self.rect_size
                                let bar = 1.0 - step(2.0, p.x)
                                let c = mix(self.color, vec4(self.border_color.rgb, max(self.border_color.a, 0.5)), bar)
                                return vec4(c.rgb * c.a, c.a)
                            }
                        }
                        lane_row_dot := Mono{
                            text: "●"
                            draw_text +: {
                                color: mod.look.lane
                                text_style: mod.look.mono{font_size: 7.0}
                            }
                        }
                        lane_row_title := Mono{
                                max_lines: 1
                                text_overflow: Ellipsis
                            width: Fill
                            draw_text +: {
                                color: mod.look.text_hi
                                text_style: mod.look.mono{font_size: 9.0}
                            }
                        }
                        lane_row_state := Mono{
                                max_lines: 1
                                text_overflow: Ellipsis
                            draw_text +: {
                                color: mod.look.meta
                                text_style: mod.look.mono{font_size: 9.0}
                            }
                        }
                    }

                    // A tool call: one terminal line. State mark, the command
                    // as typed, the result summary dim after it.
                    // The state mark is the row's button (fold/unfold); the
                    // command and its summary are text, selectable like any.
                    ToolRow := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        padding: Inset{left: 4 right: 8 top: 2 bottom: 2}
                        show_bg: true
                        draw_bg +: {
                            /** resolve scan progress, 1 = at rest 0..1 step 0.01 */
                            scan: instance(1.0)
                            /** the outcome's ink */
                            ink: instance(vec4(0.0, 0.0, 0.0, 0.0))
                            pixel: fn() {
                                if self.scan >= 1.0 {
                                    return vec4(0.0, 0.0, 0.0, 0.0)
                                }
                                let p = self.pos * self.rect_size
                                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                                // A front sweeping right with a trail behind
                                // it, scanlines in the trail, all fading out.
                                let t = self.scan
                                let d = t * 1.3 - self.pos.x
                                let trail = smoothstep(0.0, 0.015, d) * (1.0 - smoothstep(0.015, 0.45, d))
                                let front = 1.0 - smoothstep(0.0, 1.5 * px / self.rect_size.x, abs(d))
                                let lines = Finish.scan(p.y, px, 2.0, 0.6)
                                let a = (0.16 * trail * lines + 0.7 * front) * (1.0 - t) * (1.0 - t)
                                return vec4(self.ink.rgb * a, a)
                            }
                        }
                        tool_head := View{
                            width: Fit
                            height: Fit
                            cursor: MouseCursor.Hand
                            tool_glyph := Glyph{
                                margin: Inset{top: 1 right: 6}
                                draw_text +: { text_style: theme.font_icons{font_size: 7.0} }
                            }
                        }
                            tool_title := Mono{
                                width: Fill
                                draw_text +: {
                                    color: mod.look.text_hi
                                    text_style: mod.look.mono{font_size: 9.0}
                                }
                            }
                            View{ width: 12 height: 1 }
                            tool_summary := Mono{
                                width: Fit
                                draw_text +: {
                                    color: mod.look.meta
                                    text_style: mod.look.mono{font_size: 9.0}
                                }
                            }
                    }

                    // Waiting on you: calm, unmistakable. Your colour as a
                    // bar and a faint field, a chamfered corner, the request
                    // in bright mono, keys as framed buttons.
                    ApprovalRow := View{
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 6
                        margin: Inset{top: 2 bottom: 4}
                        padding: Inset{left: 22 right: 12 top: 8 bottom: 8}
                        show_bg: true
                        draw_bg +: {
                            color: mod.look.human_field
                            pixel: fn() {
                                let p = self.pos * self.rect_size
                                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                                let c0 = self.rect_size * 0.5
                                let d = Finish.sd_notch(p, c0, c0, vec4(0.0, 10.0, 0.0, 0.0))
                                let inside = Finish.cover(d, px)
                                let edge = Finish.band(d, 0.0, px, px) * 0.45
                                let bar = (1.0 - step(2.0, p.x)) * inside
                                var c = self.color * inside
                                c = mix(c, vec4(mod.look.human.rgb, 1.0), max(bar, edge))
                                return vec4(c.rgb * c.a, c.a)
                            }
                        }
                        Mono{
                            text: "◆ WAITING ON YOU"
                            draw_text +: {
                                color: mod.look.human
                                text_style: mod.look.mono_bold{font_size: 7.5}
                            }
                        }
                        approval_text := Mono{
                            width: Fill
                            draw_text +: {
                                color: mod.look.text_hi
                                text_style: mod.look.mono{font_size: 9.0}
                            }
                        }
                        approval_keys := Mono{
                            width: Fill
                            draw_text +: {
                                color: mod.look.meta
                                text_style: mod.look.mono{font_size: 8.0}
                            }
                        }
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8
                            allow_button := KeyBtn{
                                text: "Allow  (y)"
                                draw_bg +: { ink: mod.look.human }
                                draw_text +: { color: mod.look.text_hi }
                            }
                            always_button := KeyBtn{ text: "This session  (a)" }
                            repo_button := KeyBtn{ text: "This repo  (r)" }
                            deny_button := KeyBtn{ text: "Deny  (n)" }
                        }
                    }

                    QuestionRow := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        margin: Inset{top: 2 bottom: 4}
                        padding: Inset{left: 4 right: 12 top: 6 bottom: 6}
                        show_bg: true
                        draw_bg +: {
                            color: mod.look.human_field
                            pixel: fn() {
                                let p = self.pos * self.rect_size
                                let bar = 1.0 - step(2.0, p.x)
                                let c = mix(self.color, vec4(mod.look.human.rgb, 1.0), bar)
                                return vec4(c.rgb * c.a, c.a)
                            }
                        }
                        Glyph{
                            text: "?"
                            draw_text +: { color: mod.look.human }
                        }
                        question_text := Mono{
                            width: Fill
                            draw_text +: {
                                color: mod.look.text_hi
                                text_style: mod.look.mono{font_size: 9.0}
                            }
                        }
                    }

                    // A hunk head: `@@` line in the lane ink over a rule.
                    HunkRow := View{
                        width: Fill
                        height: Fit
                        padding: Inset{top: 2}
                        hunk_head := View{
                            width: Fill
                            height: 20
                            flow: Right
                            spacing: 0
                            align: Align{y: 0.5}
                            padding: Inset{left: 22 right: 12}
                            cursor: MouseCursor.Hand
                            show_bg: true
                            draw_bg +: {
                                color: mod.look.lane_wash
                                pixel: fn() {
                                    let p = self.pos * self.rect_size
                                    let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                                    let rule = 1.0 - step(px, p.y)
                                    let c = mix(self.color, mod.look.hair_hi, rule)
                                    return vec4(c.rgb * c.a, c.a)
                                }
                            }
                            hunk_check := Mono{
                                width: 16
                                draw_text +: {
                                    color: mod.look.human
                                    text_style: mod.look.mono{font_size: 8.5}
                                }
                            }
                            hunk_text := Mono{
                                width: Fill
                                draw_text +: {
                                    color: mod.look.lane
                                    text_style: mod.look.mono{font_size: 8.5}
                                }
                            }
                            undo_button := KeyBtn{ visible: false height: 18 text: "undo" }
                        }
                    }

                    // A diff line: the wash `view` picks, a two-point bar of
                    // the same ink at full strength on the left, mono text.
                    DiffRow := View{
                        width: Fill
                        height: Fit
                        padding: Inset{left: 22 right: 12 top: 0 bottom: 0}
                        show_bg: true
                        draw_bg +: {
                            color: #0000
                            pixel: fn() {
                                let p = self.pos * self.rect_size
                                let bar = (1.0 - step(11.0, p.x)) * step(9.0, p.x) * step(0.001, self.color.a)
                                let c = mix(self.color, vec4(self.color.rgb, 0.85), bar)
                                return vec4(c.rgb * c.a, c.a)
                            }
                        }
                        diff_text := Mono{
                            width: Fill
                            draw_text +: {
                                color: mod.look.text
                                text_style: mod.look.mono{font_size: 8.5}
                            }
                        }
                    }

                    // A running command's live tail: two lines under it on
                    // the same hairline the full output uses.
                    TailRow := View{
                        width: Fill
                        height: Fit
                        padding: Inset{left: 22 right: 12 top: 0 bottom: 2}
                        show_bg: true
                        draw_bg +: {
                            color: #0000
                            pixel: fn() {
                                let p = self.pos * self.rect_size
                                let line = 1.0 - step(1.0, abs(p.x - 10.0))
                                let c = vec4(mod.look.agent.rgb, 0.45) * line
                                return vec4(c.rgb * c.a, c.a)
                            }
                        }
                        tail_text := Mono{
                            width: Fill
                            draw_text +: {
                                color: mod.look.terminal_text
                                text_style: mod.look.mono{font_size: 8.5}
                            }
                        }
                    }

                    // Command output: a quiet block under its command, a
                    // hairline down its left, text a step dimmer than prose.
                    OutputRow := View{
                        width: Fill
                        height: Fit
                        padding: Inset{left: 22 right: 12 top: 3 bottom: 6}
                        show_bg: true
                        draw_bg +: {
                            color: mod.look.well
                            border_color: mod.look.hair_hi
                            pixel: fn() {
                                let p = self.pos * self.rect_size
                                let line = (1.0 - step(1.0, abs(p.x - 10.0)))
                                let c = mix(self.color, vec4(self.border_color.rgb, max(self.border_color.a, 0.6)), line)
                                return vec4(c.rgb * c.a, c.a)
                            }
                        }
                        output_text := Mono{
                            width: Fill
                            draw_text +: {
                                color: mod.look.terminal_text
                                text_style: mod.look.mono{font_size: 8.5}
                            }
                        }
                    }

                    NoticeRow := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        padding: Inset{left: 4 right: 8 top: 2 bottom: 2}
                        Glyph{ text: "·" draw_text +: { color: mod.look.dim } }
                        notice_text := Dim{}
                    }

                    ErrorRow := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        padding: Inset{left: 4 right: 8 top: 2 bottom: 2}
                        Glyph{ text: "✗" draw_text +: { color: mod.look.bad } }
                        error_text := Dim{
                            draw_text +: { color: mod.look.bad }
                        }
                    }
                }
                }

                plan := View{
                    visible: false
                    width: Fill
                    height: Fit
                    flow: Down
                    margin: Inset{top: 4}
                    plan_head := View{
                        width: Fill
                        height: 20
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}
                        cursor: MouseCursor.Hand
                        plan_title := Mono{
                            text: "PLAN"
                            draw_text +: {
                                color: mod.look.meta
                                text_style: mod.look.mono_bold{font_size: 7.5}
                            }
                        }
                        Rule{}
                    }
                    todos := PortalList{
                        width: Fill
                        height: 0
                        TodoRow := View{
                            width: Fill
                            height: 20
                            flow: Right
                            spacing: 0
                            align: Align{y: 0.5}
                            padding: Inset{left: 4}
                            todo_glyph := Glyph{
                                draw_text +: { text_style: theme.font_icons{font_size: 7.0} }
                            }
                            todo_text := Mono{
                                width: Fill
                                draw_text +: {
                                    color: mod.look.text
                                    text_style: mod.look.mono{font_size: 8.5}
                                }
                            }
                        }
                    }
                }

                hud := CodeHud{ margin: Inset{top: 4} }

                // The prompt: your glyph, the field, a hairline frame that
                // glows the agent's colour while a turn runs.
                composer := View{
                    width: Fill
                    height: Fit
                    flow: Down
                    margin: Inset{top: 2 bottom: 10}
                    padding: Inset{left: 8 right: 10 top: 7 bottom: 7}
                    show_bg: true
                    draw_bg +: {
                        color: mod.look.surface
                        border_color: mod.look.hair_hi
                        /** files are dragged over the chat: a wash of the agent's ink */
                        drop: uniform(0.0)
                        pixel: fn() {
                            let p = self.pos * self.rect_size
                            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                            let c0 = self.rect_size * 0.5
                            let d = Finish.sd_chamfer(p, c0, c0 - vec2(0.5, 0.5), 6.0)
                            let inside = Finish.cover(d, px)
                            let edge = Finish.band(d, 0.0, px, px)
                            // The frame takes the border ink: `view` breathes
                            // it while a turn runs, at rest it is a line.
                            var c = mix(self.color, vec4(mod.look.agent.rgb, 1.0), 0.10 * self.drop) * inside
                            c = mix(c, vec4(self.border_color.rgb, 1.0), edge * max(self.border_color.a, 0.35))
                            return vec4(c.rgb * c.a, c.a)
                        }
                    }
                    // Files attached to the next message.
                    chips := View{
                        visible: false
                        width: Fill
                        height: Fit
                        flow: Flow.Right{wrap: true}
                        spacing: 6
                        margin: Inset{left: 18 bottom: 5}
                        chip_0 := Chip{}
                        chip_1 := Chip{}
                        chip_2 := Chip{}
                        chip_3 := Chip{}
                        chip_4 := Chip{}
                        chip_5 := Chip{}
                        chip_6 := Chip{}
                        chip_7 := Chip{}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        align: Align{y: 0.0}
                        Glyph{
                            margin: Inset{top: 3}
                            text: "›"
                            draw_text +: {
                                color: mod.look.human
                                text_style: mod.look.mono_bold{font_size: 10.0}
                            }
                        }
                        input := Field{
                            width: Fill
                            height: Fit
                            is_multiline: true
                            submit_on_enter: true
                            empty_text: "ask, or describe a change   ⏎ send  ⇧⏎ newline  @ file"
                        }
                    }
                }
            }

        }

        // The status bar is a HUD line: dark, a hairline above it, every
        // field mono, fields parted by thin rules.
        status_bar := View{
            width: Fill
            height: 24
            flow: Right
            spacing: 12
            padding: Inset{left: 12 right: 12}
            align: Align{y: 0.5}
            show_bg: true
            draw_bg +: {
                color: mod.look.well
                pixel: fn() {
                    let p = self.pos * self.rect_size
                    let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                    let rule = 1.0 - step(px, p.y)
                    let c = mix(self.color, mod.look.hair_hi, rule)
                    return vec4(c.rgb * c.a, c.a)
                }
            }
            status_dot := Mono{
                text: "●"
                draw_text +: {
                    color: mod.look.dim
                    text_style: mod.look.mono{font_size: 7.0}
                }
            }
            status_route := Mono{
                text: "starting"
                draw_text +: {
                    color: mod.look.text
                    text_style: mod.look.mono{font_size: 8.0}
                }
            }
            status_state := Mono{
                width: Fill
                draw_text +: {
                    color: mod.look.meta
                    text_style: mod.look.mono{font_size: 8.0}
                }
            }
            status_mode := Mono{
                draw_text +: {
                    color: mod.look.meta
                    text_style: mod.look.mono{font_size: 8.0}
                }
            }
            Sep{}
            rate_spark := Sparkline{}
            status_rate := Mono{
                width: 52
                draw_text +: {
                    color: mod.look.agent
                    text_style: mod.look.mono{font_size: 8.0}
                }
            }
            Sep{}
            heat := HeatStrip{}
            ctx_chip := View{
                width: Fit
                height: Fit
                cursor: MouseCursor.Hand
                status_context := Mono{
                    draw_text +: {
                        color: mod.look.text
                        text_style: mod.look.mono{font_size: 8.0}
                    }
                }
            }
        }

        palette := Modal{
            align: Align{x: 0.5 y: 0.14}
            bg_view +: { draw_bg.color: #x020306b8 }
            content +: {
                View{
                    width: 620
                    height: Fit
                    flow: Down
                    spacing: 4
                    padding: Inset{left: 8 right: 8 top: 8 bottom: 8}
                    show_bg: true
                    draw_bg +: {
                        color: mod.look.rail
                        pixel: fn() {
                            let p = self.pos * self.rect_size
                            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                            let c0 = self.rect_size * 0.5
                            let d = Finish.sd_chamfer(p, c0, c0 - vec2(0.5, 0.5), 8.0)
                            let inside = Finish.cover(d, px)
                            let edge = Finish.band(d, 0.0, px, px)
                            var c = vec4(self.color.rgb, 1.0) * inside
                            c = mix(c, vec4(mod.look.agent.rgb, 1.0), edge * 0.7)
                            return vec4(c.rgb * c.a, c.a)
                        }
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        padding: Inset{left: 6 right: 6 top: 4 bottom: 4}
                        Glyph{
                            margin: Inset{top: 3}
                            text: "▸"
                            draw_text +: { color: mod.look.agent }
                        }
                        palette_input := Field{
                            width: Fill
                            height: Fit
                            empty_text: "command"
                        }
                    }
                    Rule{}
                    palette_rows := View{
                        width: Fill
                        height: Fit
                        flow: Down
                        pal_row_0 := PalRow{}
                        pal_row_1 := PalRow{}
                        pal_row_2 := PalRow{}
                        pal_row_3 := PalRow{}
                        pal_row_4 := PalRow{}
                        pal_row_5 := PalRow{}
                        pal_row_6 := PalRow{}
                        pal_row_7 := PalRow{}
                        pal_row_8 := PalRow{}
                    }
                }
            }
        }
    }
}
