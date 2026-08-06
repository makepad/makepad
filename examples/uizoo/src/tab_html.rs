use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // Mirrors a chat client's room-list message preview: bold sender, text,
    // and a mention pill whose rounded background overdraws its layout rect
    // through negative padding, all inside a RowAlign.Center wrapping flow.
    let ReproPreview = View{
        height: Fit, show_bg: true, draw_bg +: {color: #333}
        Html{
            width: Fill, height: Fit
            max_lines: 2
            text_overflow: Ellipsis
            font_size: 9.3
            flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
            align: Align{ y: 0.5 }
            text_style_normal +: { font_size: 9.3, line_spacing: 1.32 }
            text_style_bold +: { font_size: 9.3, line_spacing: 1.32 }
            pill := View {
                width: Fit, height: Fit,
                RoundedView {
                    width: Fit, height: Fit,
                    flow: Right,
                    align: Align{ y: 0.5 }
                    spacing: 1,
                    padding: Inset{ left: 4.5, right: 3.0, bottom: -3.5, top: -3.5 }
                    margin: Inset{ top: 1, right: 1 }
                    show_bg: true,
                    draw_bg +: { color: #000, border_radius: 4.5 }
                    RoundedView {
                        width: 13, height: 13,
                        show_bg: true,
                        draw_bg +: { color: #1fc7a8, border_radius: 6.5 }
                    }
                    Label {
                        flow: Right,
                        draw_text +: {
                            color: #f,
                            text_style +: { font_size: 8.5, line_spacing: 1.0 }
                        }
                        text: "Sam Carter",
                    }
                }
            }
            body: "<b>Riley Hayes</b>: a message with one pill <pill></pill>"
        }
    }

    // Mirrors a chat timeline message: pills and short text runs mixed in a
    // RowAlign.Center wrapping flow with no line clamp. The trailing "hello"
    // follows a pill, so at narrow widths it wraps as a continuation whose
    // first row is empty — the case where a row starts with regular text.
    let TimelineRepro = View{
        height: Fit, show_bg: true, draw_bg +: {color: #333}
        Html{
            width: Fill, height: Fit
            font_size: 9.3
            flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
            align: Align{ y: 0.5 }
            text_style_normal +: { font_size: 9.3, line_spacing: 1.32 }
            text_style_bold +: { font_size: 9.3, line_spacing: 1.32 }
            pill := View {
                width: Fit, height: Fit,
                RoundedView {
                    width: Fit, height: Fit,
                    flow: Right,
                    align: Align{ y: 0.5 }
                    spacing: 1,
                    padding: Inset{ left: 4.5, right: 3.0, bottom: -3.5, top: -3.5 }
                    margin: Inset{ top: 1, right: 1 }
                    show_bg: true,
                    draw_bg +: { color: #000, border_radius: 4.5 }
                    RoundedView {
                        width: 13, height: 13,
                        show_bg: true,
                        draw_bg +: { color: #1fc7a8, border_radius: 6.5 }
                    }
                    Label {
                        flow: Right,
                        draw_text +: {
                            color: #f,
                            text_style +: { font_size: 8.5, line_spacing: 1.0 }
                        }
                        text: "Jordan Lee",
                    }
                }
            }
            body: "<pill></pill> @ <pill></pill> <pill></pill> <pill></pill> hello <pill></pill> <pill></pill>"
        }
    }

    // Same shape at chat-timeline metrics: font 11 with 1.3 line spacing
    // and the default pill geometry (avatar 16, padding -3/-3, no top margin).
    // At these metrics the text advance and the pill's outer height differ,
    // unlike the 9.3pt preview metrics where they coincide.
    let TimelineRepro11 = View{
        height: Fit, show_bg: true, draw_bg +: {color: #333}
        Html{
            width: Fill, height: Fit
            font_size: 11
            flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
            align: Align{ y: 0.5 }
            text_style_normal +: { font_size: 11, line_spacing: 1.3 }
            text_style_bold +: { font_size: 11, line_spacing: 1.3 }
            pill := View {
                width: Fit, height: Fit,
                RoundedView {
                    width: Fit, height: Fit,
                    flow: Right,
                    align: Align{ y: 0.5 }
                    spacing: 1,
                    padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                    margin: Inset{ right: 1 }
                    show_bg: true,
                    draw_bg +: { color: #000, border_radius: 6.0 }
                    RoundedView {
                        width: 16, height: 16,
                        show_bg: true,
                        draw_bg +: { color: #1fc7a8, border_radius: 8.0 }
                    }
                    Label {
                        flow: Right,
                        draw_text +: {
                            color: #f,
                            text_style +: { font_size: 11, line_spacing: 1.0 }
                        }
                        text: "Jordan Lee",
                    }
                }
            }
            // A pill whose title falls back to the CJK font, whose line
            // metrics differ from the Latin font's.
            cjk := View {
                width: Fit, height: Fit,
                RoundedView {
                    width: Fit, height: Fit,
                    flow: Right,
                    align: Align{ y: 0.5 }
                    spacing: 1,
                    padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                    margin: Inset{ right: 1 }
                    show_bg: true,
                    draw_bg +: { color: #000, border_radius: 6.0 }
                    RoundedView {
                        width: 16, height: 16,
                        show_bg: true,
                        draw_bg +: { color: #cccccc, border_radius: 8.0 }
                    }
                    Label {
                        flow: Right,
                        draw_text +: {
                            color: #f,
                            text_style +: { font_size: 11, line_spacing: 1.0 }
                        }
                        text: "王小明",
                    }
                }
            }
            red := View {
                width: Fit, height: Fit,
                RoundedView {
                    width: Fit, height: Fit,
                    flow: Right,
                    align: Align{ y: 0.5 }
                    spacing: 1,
                    padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                    margin: Inset{ right: 1 }
                    show_bg: true,
                    draw_bg +: { color: #e4335a, border_radius: 6.0 }
                    RoundedView {
                        width: 16, height: 16,
                        show_bg: true,
                        draw_bg +: { color: #1fc7a8, border_radius: 8.0 }
                    }
                    Label {
                        flow: Right,
                        draw_text +: {
                            color: #f,
                            text_style +: { font_size: 11, line_spacing: 1.0 }
                        }
                        text: "Riley Hayes",
                    }
                }
            }
            body: "<cjk></cjk> @ <pill></pill> <pill></pill> <red></red> hello <pill></pill> <pill></pill>"
        }
    }

    // D1 repro: zero content padding (a chat timeline's message body) and a
    // FIRST row holds both a pill and the anchored first row of a wrapped text
    // run. Centering the pill on the anchor lifts it above the widget's clip
    // top unless the up-shift is clamped, which renders the pill's rounded top
    // as a flat cut edge.
    let TimelineReproD1 = View{
        height: Fit, show_bg: true, draw_bg +: {color: #333}
        Html{
            width: Fill, height: Fit
            font_size: 11
            padding: 0.0
            flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
            align: Align{ y: 0.5 }
            text_style_normal +: { font_size: 11, line_spacing: 1.3 }
            text_style_bold +: { font_size: 11, line_spacing: 1.3 }
            cjk := View {
                width: Fit, height: Fit,
                RoundedView {
                    width: Fit, height: Fit,
                    flow: Right,
                    align: Align{ y: 0.5 }
                    spacing: 1,
                    padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                    margin: Inset{ right: 1 }
                    show_bg: true,
                    draw_bg +: { color: #000, border_radius: 6.0 }
                    RoundedView {
                        width: 16, height: 16,
                        show_bg: true,
                        draw_bg +: { color: #cccccc, border_radius: 8.0 }
                    }
                    Label {
                        flow: Right,
                        draw_text +: {
                            color: #f,
                            text_style +: { font_size: 11, line_spacing: 1.0 }
                        }
                        text: "王小明",
                    }
                }
            }
            body: "<cjk></cjk> hello there this is a longer message that wraps"
        }
    }

    mod.widgets.DemoHtml = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Html\n\nThe Html widget renders HTML content."}
        }
        demos +: {
            H4{text: "Fit-max bounded pill labels at widths across the cap"}
            P{text: "A pill-like label bounded to 60% of the enclosing width, at widths walking across the cap. Truncation must always show a trailing ellipsis, never a bare cut."}
            View{
                width: Fill, height: Fit, flow: Down, spacing: 6
                View{ width: 300, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    View{ width: Fit, height: Fit,
                        View{ width: Fit, height: Fit,
                            RoundedView { width: Fit, height: Fit, flow: Right, align: Align{ y: 0.5 }, spacing: 1,
                                padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                                margin: Inset{ right: 1 }
                                show_bg: true, draw_bg +: { color: #000, border_radius: 6.0 }
                                RoundedView { width: 16, height: 16, show_bg: true, draw_bg +: { color: #1fc7a8, border_radius: 8.0 } }
                                Label {
                                    width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.6}},
                                    max_lines: 1, text_overflow: Ellipsis,
                                    draw_text +: { color: #f, text_style +: { font_size: 11, line_spacing: 1.0 } }
                                    text: "@somebody-with-a-long-name:example.org" }
                            }
                        }
                    }
                }
                View{ width: 220, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    View{ width: Fit, height: Fit,
                        View{ width: Fit, height: Fit,
                            RoundedView { width: Fit, height: Fit, flow: Right, align: Align{ y: 0.5 }, spacing: 1,
                                padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                                margin: Inset{ right: 1 }
                                show_bg: true, draw_bg +: { color: #000, border_radius: 6.0 }
                                RoundedView { width: 16, height: 16, show_bg: true, draw_bg +: { color: #1fc7a8, border_radius: 8.0 } }
                                Label {
                                    width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.6}},
                                    max_lines: 1, text_overflow: Ellipsis,
                                    draw_text +: { color: #f, text_style +: { font_size: 11, line_spacing: 1.0 } }
                                    text: "@somebody-with-a-long-name:example.org" }
                            }
                        }
                    }
                }
                View{ width: 160, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    View{ width: Fit, height: Fit,
                        View{ width: Fit, height: Fit,
                            RoundedView { width: Fit, height: Fit, flow: Right, align: Align{ y: 0.5 }, spacing: 1,
                                padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                                margin: Inset{ right: 1 }
                                show_bg: true, draw_bg +: { color: #000, border_radius: 6.0 }
                                RoundedView { width: 16, height: 16, show_bg: true, draw_bg +: { color: #1fc7a8, border_radius: 8.0 } }
                                Label {
                                    width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.6}},
                                    max_lines: 1, text_overflow: Ellipsis,
                                    draw_text +: { color: #f, text_style +: { font_size: 11, line_spacing: 1.0 } }
                                    text: "@quokka:example.org" }
                            }
                        }
                    }
                }
                View{ width: 160, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill, height: Fit, font_size: 11
                        flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
                        text_style_normal +: { font_size: 11, line_spacing: 1.3 }
                        pill := View { width: Fit, height: Fit,
                            RoundedView { width: Fit, height: Fit, flow: Right, align: Align{ y: 0.5 }, spacing: 1,
                                padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                                margin: Inset{ right: 1 }
                                show_bg: true, draw_bg +: { color: #000, border_radius: 6.0 }
                                RoundedView { width: 16, height: 16, show_bg: true, draw_bg +: { color: #1fc7a8, border_radius: 8.0 } }
                                Label {
                                    width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.6}},
                                    max_lines: 1, text_overflow: Ellipsis,
                                    draw_text +: { color: #f, text_style +: { font_size: 11, line_spacing: 1.0 } }
                                    text: "@somebody-with-a-long-name:example.org" }
                            }
                        }
                        body: "hi <pill></pill> ok"
                    }
                }
            }
            Hr{}

            H4{text: "Line-bounded pill labels (Base.Line)"}
            P{text: "Pill labels bounded by the line width actually available to the pill. A long name ellipsizes at the line edge; a mid-line pill relocates whole to the next row; a pill held on the last clamped row squeezes into the remnant and stays visible."}
            View{
                width: Fill, height: Fit, flow: Down, spacing: 6
                // A long name in a narrow container: ellipsis lands near the
                // container's right edge, not at a fixed fraction of it.
                View{ width: 160, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    View{ width: Fit, height: Fit,
                        View{ width: Fit, height: Fit,
                            RoundedView { width: Fit, height: Fit, flow: Right, align: Align{ y: 0.5 }, spacing: 1,
                                padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                                margin: Inset{ right: 1 }
                                show_bg: true, draw_bg +: { color: #000, border_radius: 6.0 }
                                RoundedView { width: 16, height: 16, show_bg: true, draw_bg +: { color: #1fc7a8, border_radius: 8.0 } }
                                Label {
                                    width: Fit{max: FitBound.Rel{base: Base.Line, factor: 1.0}},
                                    max_lines: 1, text_overflow: Ellipsis,
                                    draw_text +: { color: #f, text_style +: { font_size: 11, line_spacing: 1.0 } }
                                    text: "@somebody-with-a-long-name:example.org" }
                            }
                        }
                    }
                }
                // A long-name pill mid-line in a wrapping flow: the pill
                // relocates whole to its own row and ellipsizes at the row
                // edge; the text before and after stays intact.
                View{ width: 200, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill, height: Fit, font_size: 11
                        flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
                        text_style_normal +: { font_size: 11, line_spacing: 1.3 }
                        pill := View { width: Fit, height: Fit,
                            RoundedView { width: Fit, height: Fit, flow: Right, align: Align{ y: 0.5 }, spacing: 1,
                                padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                                margin: Inset{ right: 1 }
                                show_bg: true, draw_bg +: { color: #000, border_radius: 6.0 }
                                RoundedView { width: 16, height: 16, show_bg: true, draw_bg +: { color: #1fc7a8, border_radius: 8.0 } }
                                Label {
                                    width: Fit{max: FitBound.Rel{base: Base.Line, factor: 1.0}},
                                    max_lines: 1, text_overflow: Ellipsis,
                                    draw_text +: { color: #f, text_style +: { font_size: 11, line_spacing: 1.0 } }
                                    text: "@somebody-with-a-long-name:example.org" }
                            }
                        }
                        body: "hi <pill></pill> ok"
                    }
                }
                // A long-name pill landing on the LAST permitted row of a
                // clamped flow: the line clamp holds it in place, so the
                // title squeezes into the row remnant and the pill remains
                // visible with its own ellipsis, not hidden behind the
                // flow's.
                View{ width: 210, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill, height: Fit, font_size: 11
                        max_lines: 2
                        text_overflow: Ellipsis
                        flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
                        text_style_normal +: { font_size: 11, line_spacing: 1.3 }
                        pill := View { width: Fit, height: Fit,
                            RoundedView { width: Fit, height: Fit, flow: Right, align: Align{ y: 0.5 }, spacing: 1,
                                padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                                margin: Inset{ right: 1 }
                                show_bg: true, draw_bg +: { color: #000, border_radius: 6.0 }
                                RoundedView { width: 16, height: 16, show_bg: true, draw_bg +: { color: #1fc7a8, border_radius: 8.0 } }
                                Label {
                                    width: Fit{max: FitBound.Rel{base: Base.Line, factor: 1.0}},
                                    max_lines: 1, text_overflow: Ellipsis,
                                    draw_text +: { color: #f, text_style +: { font_size: 11, line_spacing: 1.0 } }
                                    text: "@somebody-with-a-long-name:example.org" }
                            }
                        }
                        body: "one two three four five six seven <pill></pill>"
                    }
                }
                // A short name in a wide container fits fully inline: the
                // line bound must not truncate a name the line can hold.
                View{ width: 400, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill, height: Fit, font_size: 11
                        flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
                        text_style_normal +: { font_size: 11, line_spacing: 1.3 }
                        pill := View { width: Fit, height: Fit,
                            RoundedView { width: Fit, height: Fit, flow: Right, align: Align{ y: 0.5 }, spacing: 1,
                                padding: Inset{ left: 6, right: 4, bottom: -3, top: -3 }
                                margin: Inset{ right: 1 }
                                show_bg: true, draw_bg +: { color: #000, border_radius: 6.0 }
                                RoundedView { width: 16, height: 16, show_bg: true, draw_bg +: { color: #1fc7a8, border_radius: 8.0 } }
                                Label {
                                    width: Fit{max: FitBound.Rel{base: Base.Line, factor: 1.0}},
                                    max_lines: 1, text_overflow: Ellipsis,
                                    draw_text +: { color: #f, text_style +: { font_size: 11, line_spacing: 1.0 } }
                                    text: "@quokka:example.org" }
                            }
                        }
                        body: "hi <pill></pill> ok"
                    }
                }
            }
            Hr{}

            H4{text: "REPRO: timeline message, pills + text, no line clamp"}
            P{text: "Rows starting with regular text after a pill-heavy row must keep the text and the pills on one center line, pill tops must never be cut, and inter-line spacing must be uniform."}
            View{
                width: Fill, height: Fit, flow: Down, spacing: 6
                TimelineRepro11 { width: 420 }
                TimelineRepro11 { width: 310 }
                TimelineRepro11 { width: 240 }
                TimelineRepro { width: 260 }
                TimelineRepro { width: 200 }
                TimelineReproD1 { width: 240 }
            }
            Hr{}

            H4{text: "REPRO: sender + wrapped text + trailing pill, chat-preview style"}
            P{text: "Bold sender, small text with 1.32 line spacing, RowAlign.Center, and a pill whose background overdraws its layout rect via negative padding. Five widths walk the pill from inline, to wrapped, to sharing a row with wrapped text, to overrunning the last line."}
            View{
                width: Fill, height: Fit, flow: Down, spacing: 6
                ReproPreview { width: 280 }
                ReproPreview { width: 230 }
                ReproPreview { width: 190 }
                ReproPreview { width: 170 }
                ReproPreview { width: 150 }
                ReproPreview { width: 130 }
            }
            Hr{}

            H4{text: "Html with ellipsis, inline code, and a narrow width (2 lines)"}
            P{text: "An inline code span too wide for the remaining room wraps to its own line. That wrap must consume one of the two allowed lines, so the text below must never exceed two rows at any window width."}
            View{
                width: Fill, height: Fit, flow: Down, spacing: 4
                View{ width: 230, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                        body: "Sam Carter: and <code>&lt;details&gt;</code> / <code>&lt;summary&gt;</code> is fully working (see: https://example.com/doc/inline-code-demo for the full writeup)" } }
                View{ width: 260, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                        body: "Sam Carter: and <code>&lt;details&gt;</code> / <code>&lt;summary&gt;</code> is fully working (see: https://example.com/doc/inline-code-demo for the full writeup)" } }
                View{ width: 320, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                        body: "Sam Carter: and <code>&lt;details&gt;</code> / <code>&lt;summary&gt;</code> is fully working (see: https://example.com/doc/inline-code-demo for the full writeup)" } }
                View{ width: 410, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                        body: "Sam Carter: and <code>&lt;details&gt;</code> / <code>&lt;summary&gt;</code> is fully working (see: https://example.com/doc/inline-code-demo for the full writeup)" } }
            }
            H4{text: "Html with an atomic inline widget at max_lines 2"}
            P{text: "An inline widget too wide for the room left on its row is relocated whole onto a new row. That row counts against max_lines like any other, and once the budget is spent no further widget may draw."}
            View{
                width: Fill, height: Fit, flow: Down, spacing: 4
                View{ width: 250, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                        pill := RoundedView{
                            width: Fit, height: Fit, flow: Right
                            padding: Inset{left: 6, right: 6, top: 1, bottom: 1}
                            margin: Inset{left: 2, right: 2}
                            show_bg: true, draw_bg +: { color: #4488cc, border_radius: 6.0 }
                            Label{ flow: Right, draw_text +: { color: #ffffff }, text: "@quill:example.org" }
                        }
                        body: "hey <pill></pill> and <pill></pill> plus a good deal more text that has to be clamped" }
                }
                View{ width: 300, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                        pill := RoundedView{
                            width: Fit, height: Fit, flow: Right
                            padding: Inset{left: 6, right: 6, top: 1, bottom: 1}
                            margin: Inset{left: 2, right: 2}
                            show_bg: true, draw_bg +: { color: #4488cc, border_radius: 6.0 }
                            Label{ flow: Right, draw_text +: { color: #ffffff }, text: "@quill:example.org" }
                        }
                        body: "hey <pill></pill> and <pill></pill> plus a good deal more text that has to be clamped" }
                }
            }

            H4{text: "Text that fills both lines, then a mention pill"}
            P{text: "The worst realistic case: the text before the pill ends on the last allowed line without being cut, so nothing has truncated yet, and the pill then does not fit in what is left of that line."}
            View{
                width: Fill, height: Fit, flow: Down, spacing: 4
                View{ width: 250, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                        pill := RoundedView{
                            width: Fit, height: Fit, flow: Right
                            padding: Inset{left: 6, right: 6, top: 1, bottom: 1}
                            margin: Inset{left: 2, right: 2}
                            show_bg: true, draw_bg +: { color: #4488cc, border_radius: 6.0 }
                            Label{ flow: Right, draw_text +: { color: #ffffff }, text: "@quill:example.org" }
                        }
                        body: "Thanks for the review, that all makes sense, so I am assigning it to <pill></pill>" }
                }
                View{ width: 250, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                        pill := RoundedView{
                            width: Fit, height: Fit, flow: Right
                            padding: Inset{left: 6, right: 6, top: 1, bottom: 1}
                            margin: Inset{left: 2, right: 2}
                            show_bg: true, draw_bg +: { color: #4488cc, border_radius: 6.0 }
                            Label{ flow: Right, draw_text +: { color: #ffffff }, text: "@quill:example.org" }
                        }
                        body: "Thanks for the review and all of your detailed comments, that all makes sense to me, so I am going to assign it over to <pill></pill> later today" }
                }
            }

            H4{text: "A mention pill inside a <summary>"}
            P{text: "The summary line opens with a fold button, so the same overrun applies there: the pill charges its row like any other inline widget."}
            View{
                width: 250, height: Fit, show_bg: true, draw_bg +: {color: #333}
                Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                    pill := RoundedView{
                        width: Fit, height: Fit, flow: Right
                        padding: Inset{left: 6, right: 6, top: 1, bottom: 1}
                        margin: Inset{left: 2, right: 2}
                        show_bg: true, draw_bg +: { color: #4488cc, border_radius: 6.0 }
                        Label{ flow: Right, draw_text +: { color: #ffffff }, text: "@quill:example.org" }
                    }
                    body: "<details><summary>a pill inside a summary <pill></pill></summary>hidden body content</details>" }
            }

            H4{text: "Consecutive inline widgets with no text between them"}
            P{text: "Nothing but widgets, so no text run follows to notice the rows they open; each widget charges its own row instead. On the last allowed row wrapping is switched off, so the widget that no longer fits is held there and clipped rather than opening a third row, and the ones after it are dropped."}
            View{
                width: 250, height: Fit, show_bg: true, draw_bg +: {color: #333}
                Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                    pill := RoundedView{
                        width: Fit, height: Fit, flow: Right
                        padding: Inset{left: 6, right: 6, top: 1, bottom: 1}
                        margin: Inset{left: 2, right: 2}
                        show_bg: true, draw_bg +: { color: #4488cc, border_radius: 6.0 }
                        Label{ flow: Right, draw_text +: { color: #ffffff }, text: "@quill:example.org" }
                    }
                    body: "<pill></pill><pill></pill><pill></pill><pill></pill><pill></pill><pill></pill>" }
            }

            H4{text: "Inline widget wider than the whole line"}
            P{text: "Left: an unbounded pill overflows the container, because relocating it to a fresh row still does not make it fit. Right: bounding its label to a fraction of the enclosing width keeps the pill inside the line and ellipsizes the name."}
            View{
                width: Fill, height: Fit, flow: Down, spacing: 4
                View{ width: 250, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                        pill := RoundedView{
                            width: Fit, height: Fit, flow: Right
                            padding: Inset{left: 6, right: 6, top: 1, bottom: 1}
                            margin: Inset{left: 2, right: 2}
                            show_bg: true, draw_bg +: { color: #4488cc, border_radius: 6.0 }
                            Label{ flow: Right, draw_text +: { color: #ffffff }
                                text: "@a-really-long-display-name-that-cannot-fit:example.org" }
                        }
                        body: "hey <pill></pill> and some trailing words" }
                }
                View{ width: 250, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Html{ width: Fill height: Fit max_lines: 2 text_overflow: Ellipsis
                        pill := RoundedView{
                            width: Fit, height: Fit, flow: Right
                            padding: Inset{left: 6, right: 6, top: 1, bottom: 1}
                            margin: Inset{left: 2, right: 2}
                            show_bg: true, draw_bg +: { color: #4488cc, border_radius: 6.0 }
                            Label{ flow: Right, draw_text +: { color: #ffffff }
                                width: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.6}}
                                max_lines: 1, text_overflow: Ellipsis
                                text: "@a-really-long-display-name-that-cannot-fit:example.org" }
                        }
                        body: "hey <pill></pill> and some trailing words" }
                }
            }

            H4{text: "Html list with max_lines 3"}
            P{text: "A list item's bullet and its first line of text share one visual line, so three allowed lines must show three items."}
            View{
                width: 320, height: Fit, show_bg: true, draw_bg +: {color: #333}
                Html{ width: Fill height: Fit max_lines: 3 text_overflow: Ellipsis
                    body: "<ul><li>alpha</li><li>beta</li><li>gamma</li><li>delta</li></ul>" }
            }

            H4{text: "Non-wrapping Label: max_lines 1 + Ellipsis"}
            P{text: "A single-line, non-wrapping Label overflows sideways rather than onto extra rows, so its ellipsis must still appear when max_lines is also set."}
            View{
                width: Fill, height: Fit, flow: Down, spacing: 4
                View{ width: 230, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Label{ width: Fill, height: Fit, flow: Flow.Right{wrap: false}, padding: 0
                        max_lines: 1, text_overflow: Ellipsis
                        text: "A label whose text is far too long to fit on one line" } }
                View{ width: 230, height: Fit, show_bg: true, draw_bg +: {color: #333}
                    Label{ width: Fill, height: Fit, flow: Flow.Right{wrap: false}, padding: 0
                        text_overflow: Ellipsis
                        text: "Same label with ellipsis but no max_lines set" } }
            }
            Hr{}

            Html{
                width: Fill height: Fit
                body: "<H1>H1 Headline</H1><H2>H2 Headline</H2><H3>H3 Headline</H3><H4>H4 Headline</H4><H5>H5 Headline</H5><H6>H6 Headline</H6>This is <b>bold</b>&nbsp;and <i>italic text</i>.<sep><b><i>Bold italic</i></b>, <u>underlined</u>, and <s>strike through</s> text. <p>This is a paragraph</p> <code>A code block</code>. <br/> And this is a <a href='https://www.google.com/'>link</a><br/><ul><li>lorem</li><li>ipsum</li><li>dolor</li></ul><ol><li>lorem</li><li>ipsum</li><li>dolor</li></ol><br/> <blockquote>Blockquote</blockquote> <pre>pre</pre><sub>sub</sub><del>del</del>"
            }

            Hr{}
            H4{text: "Html with ellipsis (1 line)"}
            P{text: "Html content truncated to a single line. Resize the window to see the ellipsis move."}
            Html{
                width: Fill height: Fit
                max_lines: 1
                text_overflow: Ellipsis
                body: "This is <b>bold</b> and <i>italic</i> and <code>inline code</code> and regular text that goes on long enough to be truncated with an ellipsis at the end of the line."
            }

            Hr{}
            H4{text: "Html with ellipsis (2 lines)"}
            P{text: "Styled Html wrapping to 2 lines before truncating."}
            Html{
                width: Fill height: Fit
                max_lines: 2
                text_overflow: Ellipsis
                body: "The <b>quick brown fox</b> jumps over the <i>lazy dog</i>. Pack my box with <b><i>five dozen</i></b> liquor jugs. How <u>vexingly quick</u> daft zebras jump! The five boxing wizards jump quickly. Sphinx of black quartz, judge my vow. Two driven jocks help fax my big quiz."
            }

            Hr{}
            H4{text: "Html with ellipsis, inline code, and a narrow width (2 lines)"}
            P{text: "An inline code span too wide for the remaining room wraps to its own line. That wrap must consume one of the two allowed lines, so the text below must never exceed two rows at any window width."}
            View{
                width: 320, height: Fit
                Html{
                    width: Fill height: Fit
                    max_lines: 2
                    text_overflow: Ellipsis
                    body: "Sam Carter: and <code>&lt;details&gt;</code> / <code>&lt;summary&gt;</code> is fully working (see: https://example.com/doc/inline-code-demo for the full writeup)"
                }
            }

            Hr{}
            H4{text: "Html with ellipsis and emoji (1 line)"}
            P{text: "Html with multi-byte emoji mixed into styled text."}
            Html{
                width: Fill height: Fit
                max_lines: 1
                text_overflow: Ellipsis
                body: "Stars \u{2B50}\u{2B50}\u{2B50} with <b>bold rockets \u{1F680}\u{1F680}\u{1F680}</b> and <i>italic flags \u{1F3C1}\u{1F3C1}\u{1F3C1}</i> and more content to overflow"
            }

            Hr{}
            H4{text: "Plain HTML table (no alignment)"}
            P{text: "Default left-aligned cells. Exercises bold, italic, code, links, sub/sup, emoji, entities, and strikethrough inside cells."}
            Html{
                width: Fill height: Fit
                body: "<table><thead><tr><th>Element</th><th>Symbol</th><th>Notes</th></tr></thead><tbody><tr><td>Hydrogen</td><td><b>H</b></td><td><i>Lightest</i> gas</td></tr><tr><td>Water</td><td>H<sub>2</sub>O</td><td>Covers ~71% of Earth \u{1F30A}</td></tr><tr><td>Carbon-14</td><td><sup>14</sup>C</td><td>Used in <code>dating</code>; see <a href='https://en.wikipedia.org/wiki/Carbon-14'>wiki</a></td></tr><tr><td>Caffeine</td><td>C<sub>8</sub>H<sub>10</sub>N<sub>4</sub>O<sub>2</sub></td><td>\u{2615} keeps you awake</td></tr><tr><td>Specials</td><td>a &amp; b &lt; c</td><td><s>struck</s> / <b>bold</b> / <i>em</i></td></tr></tbody></table>"
            }

            Hr{}
            H4{text: "Aligned HTML table (align attr + style='text-align:')"}
            P{text: "Left / center / right columns set via both the align attribute and inline style. Cells carry mixed formatting, links, sub/sup."}
            Html{
                width: Fill height: Fit
                body: "<table><thead><tr><th align='left'>Task</th><th align='center'>Status</th><th style='text-align: right'>Due</th></tr></thead><tbody><tr><td align='left'>Ship feature <b>X</b></td><td align='center'><code>WIP</code></td><td style='text-align: right'><b>Fri</b></td></tr><tr><td align='left'>Review <a href='https://example.com'>PR #42</a></td><td align='center'><i>Pending</i></td><td style='text-align: right'>Mon</td></tr><tr><td align='left'>Fix <s>critical</s> bug</td><td align='center'>Done \u{2705}</td><td style='text-align: right'>Yesterday</td></tr><tr><td align='left'>Write spec for H<sub>2</sub>O sync</td><td align='center'>50%</td><td style='text-align: right'>2026-05-15</td></tr><tr><td align='left'>Ship Claude<sup>TM</sup> release</td><td align='center'>Blocked</td><td style='text-align: right'>TBD</td></tr></tbody></table>"
            }

            Hr{}
            H4{text: "Collapsible sections (<details> / <summary>)"}
            P{text: "Click the triangle to expand or collapse. The first <details> uses the open attribute to start expanded; the others start collapsed. Nested <details> are supported, and summaries can contain styled text."}
            Html{
                width: Fill height: Fit
                body: "<details open><summary>What is this widget?</summary><p>This is a collapsible section. Each <code>&lt;details&gt;</code> tag wraps a <code>&lt;summary&gt;</code> followed by hidden content that is shown when expanded. Use the <code>open</code> attribute to make it expanded by default.</p></details><details><summary><b>Keyboard shortcuts</b></summary><ul><li><code>Cmd</code> + <code>S</code> — save</li><li><code>Cmd</code> + <code>Z</code> — undo</li><li><code>Cmd</code> + <code>Shift</code> + <code>Z</code> — redo</li></ul></details><details><summary>Nested <i>details</i> with H<sub>2</sub>O inside</summary><p>The outer summary holds a subscript and italics. Inside, you can nest more <code>&lt;details&gt;</code>:</p><details><summary>Level 2: click me</summary><p>Hidden level 2 content. Links still work: <a href='https://example.com'>example.com</a>.</p><details><summary>Level 3: click me too</summary><p>Deeply nested content. Blockquotes work here:</p><blockquote>A quote inside a collapsed-by-default section.</blockquote></details></details></details>"
            }

            Hr{}
            H4{text: "Numeric HTML table (all columns right-aligned)"}
            P{text: "Typical numeric layout using align='right' on every cell. Header, body rows, and a bold totals row."}
            Html{
                width: Fill height: Fit
                body: "<table><thead><tr><th align='left'>Region</th><th align='right'>Q1</th><th align='right'>Q2</th><th align='right'>Q3</th><th align='right'>YoY</th></tr></thead><tbody><tr><td align='left'><b>North America</b></td><td align='right'>$1.2M</td><td align='right'>$1.5M</td><td align='right'>$1.8M</td><td align='right'>+12%</td></tr><tr><td align='left'><i>Europe</i></td><td align='right'>$0.9M</td><td align='right'>$1.1M</td><td align='right'>$1.3M</td><td align='right'>+8%</td></tr><tr><td align='left'>Asia Pacific</td><td align='right'>$0.7M</td><td align='right'>$0.8M</td><td align='right'>$1.0M</td><td align='right'>+15%</td></tr><tr><td align='left'>LATAM</td><td align='right'>$0.2M</td><td align='right'>$0.3M</td><td align='right'>$0.4M</td><td align='right'>+22%</td></tr><tr><td align='left'><b>Total</b></td><td align='right'><b>$3.0M</b></td><td align='right'><b>$3.7M</b></td><td align='right'><b>$4.5M</b></td><td align='right'><b>+13%</b></td></tr></tbody></table>"
            }
        }
    }
}
