use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let STUDIO_HEADER_HEIGHT = 36.0

    let AiChatMarkdown = StudioMarkdown {
        width: Fill
        height: Fit
        selectable: true
        padding: Inset {left: 0.0 right: 0.0 top: 0.0 bottom: 0.0}
        paragraph_spacing: 9.0
        pre_code_spacing: 4.0
        inline_code_padding: Inset {left: 4.0 right: 4.0 top: 1.0 bottom: 3.0}
        inline_code_margin: Inset {left: 2.0 right: 2.0 top: 0.0 bottom: 0.0}
        heading_base_scale: 1.45
        quote_layout: Layout {
            flow: Flow.Right {wrap: true}
            padding: Inset {left: 8.0 right: 8.0 top: 4.0 bottom: 5.0}
        }
        draw_block +: {
            quote_bg_color: theme.color_bg_highlight
            quote_fg_color: theme.color_label_inner_inactive
            code_color: theme.color_bg_highlight
        }
        splash_block := View {
            width: Fill
            height: 54.0
            flow: Overlay
            margin: Inset {left: 0.0 right: 0.0 top: 3.0 bottom: 1.0}
            splash_view := CodeView {
                keep_cursor_at_end: false
                editor +: {
                    height: 54.0
                    word_wrap: true
                    pad_left_top: vec2(8.0, 5.0)
                    draw_bg +: {
                        color: theme.color_bg_highlight
                    }
                }
            }
        }
        body: ""
    }

    mod.widgets.AiSkillMentionItem = RoundedView {
        width: Fill
        height: Fit
        flow: Down
        padding: Inset {left: 8.0 right: 8.0 top: 4.0 bottom: 4.0}
        draw_bg +: {
            border_radius: 5.0
            color: vec4(0.0, 0.0, 0.0, 0.0)
        }
        skill_label := Label {
            width: Fill
            height: Fit
            padding: Inset {}
            draw_text +: {
                color: theme.color_label_inner
                text_style: theme.font_bold {
                    font_size: 8.5
                }
            }
        }
        skill_description := Label {
            width: Fill
            height: Fit
            margin: Inset {top: 2.0}
            padding: Inset {}
            draw_text +: {
                color: theme.color_label_inner_inactive
                text_style: theme.font_regular {
                    font_size: 7.5
                }
            }
        }
    }

    let AiPromptInput = StudioCommandTextInput {
        width: Fill
        height: Fit
        inline_search: true
        margin: Inset {}
        popup +: {
            width: Fill
            padding: Inset {left: 4.0 right: 4.0 top: 4.0 bottom: 4.0}
            header_view +: {
                visible: false
            }
            draw_bg +: {
                color: theme.color_bg_highlight * 0.95
                border_color: vec4(1.0, 1.0, 1.0, 0.08)
                border_radius: 6.0
            }
        }
        persistent := RoundedView {
            width: Fill
            height: 80.0
            flow: Down
            draw_bg +: {
                color: vec4(0.0, 0.0, 0.0, 0.0)
                border_color: vec4(0.0, 0.0, 0.0, 0.0)
                border_radius: 7.0
            }
            top := View {height: 0.0}
            center := RoundedView {
                width: Fill
                height: Fill
                draw_bg +: {
                    color: vec4(0.0, 0.0, 0.0, 0.0)
                    border_color: vec4(0.0, 0.0, 0.0, 0.0)
                    border_radius: 7.0
                }
                text_input := TextInputFlat {
                    width: Fill
                    height: Fill
                    is_multiline: true
                    submit_on_enter: false
                    empty_text: "Ask AI"
                    margin: Inset {}
                    padding: Inset {left: 12.0 right: 12.0 top: 10.0 bottom: 10.0}
                        draw_bg +: {
                            border_radius: 7.0

                            color: vec4(0.045, 0.048, 0.052, 1.0)
                            color_hover: vec4(0.055, 0.058, 0.063, 1.0)
                            color_focus: vec4(0.055, 0.058, 0.063, 1.0)
                            color_down: vec4(0.050, 0.053, 0.058, 1.0)
                            color_empty: vec4(0.045, 0.048, 0.052, 1.0)

                        border_color: theme.color_u_hidden
                        border_color_hover: theme.color_u_hidden
                        border_color_focus: theme.color_bevel_focus
                        border_color_down: theme.color_u_hidden
                        border_color_empty: theme.color_u_hidden
                        border_color_disabled: theme.color_u_hidden

                        border_color_2: theme.color_u_hidden
                        border_color_2_hover: theme.color_u_hidden
                        border_color_2_focus: theme.color_u_hidden
                        border_color_2_down: theme.color_u_hidden
                        border_color_2_empty: theme.color_u_hidden
                        border_color_2_disabled: theme.color_u_hidden
                        }
                        draw_text +: {
                            color: theme.color_label_outer
                            color_hover: theme.color_label_outer
                            color_focus: theme.color_label_outer
                            color_down: theme.color_label_outer
                            color_empty: theme.color_label_inner_inactive
                            color_empty_hover: theme.color_label_inner_inactive
                            color_empty_focus: theme.color_label_inner_inactive
                        }
                }
            }
            bottom := View {height: 0.0}
        }
    }

    let AiRunButton = ButtonFlat {
        width: 42.0
        height: 42.0
        margin: Inset {}
        padding: Inset {left: 0.0 right: 0.0 top: 0.0 bottom: 0.0}
        text: "▶"
        draw_bg +: {
            border_radius: 12.0
            color: vec4(0.24, 0.44, 0.67, 0.25)
            color_hover: vec4(0.24, 0.44, 0.67, 0.45)
            color_down: vec4(0.24, 0.44, 0.67, 0.60)
            color_focus: vec4(0.24, 0.44, 0.67, 0.25)
            border_color: vec4(0.38, 0.69, 0.91, 0.3)
            border_size: 1.0
        }
        draw_text +: {
            color: #fff
            color_hover: #fff
            color_down: #fff
            color_focus: #fff
            color_disabled: theme.color_label_inner_inactive
            text_style: theme.font_bold {
                font_size: 15.0
            }
        }
    }

    let AiPaneDivider = View {
        width: Fill
        height: 1.0
        margin: Inset {left: 12.0 right: 12.0 top: 0.0 bottom: 0.0}
        show_bg: true
        draw_bg +: {
            color: vec4(1.0, 1.0, 1.0, 0.05)
        }
    }

    mod.widgets.AiPane = RectView {
        width: Fill
        height: Fill
        flow: Overlay
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                let color = vec4(theme.color_bg_container.rgb, 0.35)
                let highlight = 0.04 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.005
                sdf.fill(vec4(color.rgb + highlight + noise, color.a))
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.06), 1.0)
                return sdf.result
            }
        }

        ai_panel_content := View {
            width: Fill
            height: Fill
            flow: Down
            RectView {
                width: Fill
                height: STUDIO_HEADER_HEIGHT
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                padding: Inset {left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
                spacing: theme.space_2
                draw_bg +: {
                    color: vec4(1.0, 1.0, 1.0, 0.02)
                }

                View {
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: theme.space_2
                    align: Align {x: 0.0 y: 0.5}

                    Label {
                        text: "AI"
                    }

                    Filler {}

                    ai_status_label := Label {
                        width: Fit
                        text: "Loading AI..."
                    }

                    ai_status_spinner := LoadingSpinner {
                        width: 14.0
                        height: 14.0
                        margin: Inset {left: 4.0 right: 0.0 top: 1.0 bottom: 0.0}
                        draw_bg +: {
                            color: vec4(0.38, 0.69, 0.91, 1.0)
                            stroke_width: 2.0
                        }
                    }
                }
            }

            RectView {
                width: Fill
                height: Fit
                flow: Down
                spacing: theme.space_2
                padding: Inset {left: 12.0 right: 12.0 top: 12.0 bottom: 8.0}
                draw_bg +: {
                    color: vec4(0.0, 0.0, 0.0, 0.08)
                }

                View {
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: theme.space_2

                    ai_agent_dropdown := DropDown {
                        width: Fill
                        labels: ["Chat 1"]
                        draw_bg +: {
                            color: vec4(1.0, 1.0, 1.0, 0.04)
                            color_hover: vec4(1.0, 1.0, 1.0, 0.08)
                            color_focus: vec4(1.0, 1.0, 1.0, 0.08)
                            color_down: vec4(1.0, 1.0, 1.0, 0.04)
                            border_color: vec4(1.0, 1.0, 1.0, 0.06)
                            border_radius: 6.0
                            border_size: 1.0
                        }
                    }

                    ai_new_button := ButtonFlat {
                        width: 34.0
                        text: "+"
                        draw_bg +: {
                            color: vec4(1.0, 1.0, 1.0, 0.04)
                            color_hover: vec4(1.0, 1.0, 1.0, 0.08)
                            border_color: vec4(1.0, 1.0, 1.0, 0.06)
                            border_radius: 6.0
                            border_size: 1.0
                        }
                    }

                    ai_delete_button := ButtonFlat {
                        width: 34.0
                        text: "x"
                        draw_bg +: {
                            color: vec4(1.0, 1.0, 1.0, 0.04)
                            color_hover: vec4(1.0, 1.0, 1.0, 0.08)
                            border_color: vec4(1.0, 1.0, 1.0, 0.06)
                            border_radius: 6.0
                            border_size: 1.0
                        }
                    }
                }
            }

            ai_swarm_fold := FoldHeader {
                animator +: {
                    active +: {
                        default: @on
                    }
                }
                header: RectView {
                    width: Fill
                    height: 28.0
                    flow: Right
                    align: Align {x: 0.0 y: 0.5}
                    padding: Inset {left: 10.0 right: 10.0 top: 0.0 bottom: 0.0}
                    spacing: theme.space_1
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                            let color = vec4(1.0, 1.0, 1.0, 0.02)
                            let highlight = 0.02 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                            sdf.fill(vec4(color.rgb + highlight, color.a))

                            // Thin bottom border
                            if self.pos.y * self.rect_size.y >= self.rect_size.y - 1.0 {
                                sdf.clear(vec4(1.0, 1.0, 1.0, 0.04))
                            }
                            return sdf.result
                        }
                    }
                    fold_button := FoldButton {
                        animator +: {
                            active +: {
                                default: @on
                            }
                        }
                    }
                    Label {
                        text: "Task Board"
                        draw_text.color: theme.color_label_outer
                    }
                }
                body_walk: Walk {width: Fill, height: Fit}
                body: ScrollYView {
                    width: Fill
                    height: 92.0
                    flow: Down
                    show_bg: true
                    padding: Inset {left: 8.0 right: 8.0 top: 8.0 bottom: 8.0}
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                            let color = vec4(0.0, 0.0, 0.0, 0.10)
                            let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.003
                            sdf.fill(vec4(color.rgb + noise, color.a))
                            sdf.stroke(vec4(1.0, 1.0, 1.0, 0.03), 1.0)
                            return sdf.result
                        }
                    }
                    ai_swarm_markdown := AiChatMarkdown {}
                }
            }

            ai_live_fold := FoldHeader {
                animator +: {
                    active +: {
                        default: @on
                    }
                }
                header: RectView {
                    width: Fill
                    height: 28.0
                    flow: Right
                    align: Align {x: 0.0 y: 0.5}
                    padding: Inset {left: 10.0 right: 10.0 top: 0.0 bottom: 0.0}
                    spacing: theme.space_1
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                            let color = vec4(1.0, 1.0, 1.0, 0.02)
                            let highlight = 0.02 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                            sdf.fill(vec4(color.rgb + highlight, color.a))

                            // Thin bottom border
                            if self.pos.y * self.rect_size.y >= self.rect_size.y - 1.0 {
                                sdf.clear(vec4(1.0, 1.0, 1.0, 0.04))
                            }
                            return sdf.result
                        }
                    }
                    fold_button := FoldButton {
                        animator +: {
                            active +: {
                                default: @on
                            }
                        }
                    }
                    Label {
                        text: "Live Activity"
                        draw_text.color: theme.color_label_outer
                    }
                }
                body_walk: Walk {width: Fill, height: Fit}
                body: ScrollYView {
                    width: Fill
                    height: 108.0
                    flow: Down
                    show_bg: true
                    padding: Inset {left: 8.0 right: 8.0 top: 8.0 bottom: 8.0}
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                            let color = vec4(0.0, 0.0, 0.0, 0.10)
                            let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.003
                            sdf.fill(vec4(color.rgb + noise, color.a))
                            sdf.stroke(vec4(1.0, 1.0, 1.0, 0.03), 1.0)
                            return sdf.result
                        }
                    }
                    ai_live_markdown := AiChatMarkdown {}
                }
            }

            ai_files_fold := FoldHeader {
                animator +: {
                    active +: {
                        default: @on
                    }
                }
                header: RectView {
                    width: Fill
                    height: 28.0
                    flow: Right
                    align: Align {x: 0.0 y: 0.5}
                    padding: Inset {left: 10.0 right: 10.0 top: 0.0 bottom: 0.0}
                    spacing: theme.space_1
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                            let color = vec4(1.0, 1.0, 1.0, 0.02)
                            let highlight = 0.02 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                            sdf.fill(vec4(color.rgb + highlight, color.a))

                            // Thin bottom border
                            if self.pos.y * self.rect_size.y >= self.rect_size.y - 1.0 {
                                sdf.clear(vec4(1.0, 1.0, 1.0, 0.04))
                            }
                            return sdf.result
                        }
                    }
                    fold_button := FoldButton {
                        animator +: {
                            active +: {
                                default: @on
                            }
                        }
                    }
                    Label {
                        text: "Changed Files"
                        draw_text.color: theme.color_label_outer
                    }
                }
                body_walk: Walk {width: Fill, height: Fit}
                body: ScrollYView {
                    width: Fill
                    height: 78.0
                    flow: Down
                    show_bg: true
                    padding: Inset {left: 8.0 right: 8.0 top: 8.0 bottom: 8.0}
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                            let color = vec4(0.0, 0.0, 0.0, 0.10)
                            let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.003
                            sdf.fill(vec4(color.rgb + noise, color.a))
                            sdf.stroke(vec4(1.0, 1.0, 1.0, 0.03), 1.0)
                            return sdf.result
                        }
                    }
                    ai_files_markdown := AiChatMarkdown {}
                }
            }

            AiPaneDivider {}

            chat_scroll := ScrollYView {
                width: Fill
                height: Fill
                flow: Down
                show_bg: true
                padding: Inset {left: 12.0 right: 12.0 top: 10.0 bottom: 14.0}
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                        let color = vec4((theme.color_bg_container * 1.02).rgb, 0.25)
                        let highlight = 0.03 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                        let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.005
                        sdf.fill(vec4(color.rgb + highlight + noise, color.a))
                        sdf.stroke(vec4(1.0, 1.0, 1.0, 0.04), 1.0)
                        return sdf.result
                    }
                }
                ai_chat_markdown := AiChatMarkdown {}
            }

            AiPaneDivider {}

            RectView {
                width: Fill
                height: Fit
                flow: Overlay
                padding: Inset {left: 12.0 right: 12.0 top: 8.0 bottom: 12.0}
                draw_bg +: {
                    color: vec4(0.0, 0.0, 0.0, 0.0)
                }

                composer_anchor := View {
                    width: Fill
                    height: Fit
                    flow: Down

                    prompt_card := RoundedView {
                        width: Fill
                        height: Fit
                        flow: Down
                        padding: Inset {left: 1.0 right: 1.0 top: 1.0 bottom: 1.0}
                        draw_bg +: {
                            color: vec4(0.0, 0.0, 0.0, 0.15)
                            border_radius: 6.0
                            border_size: 1.0
                            border_color: vec4(1.0, 1.0, 1.0, 0.06)
                        }

                        ai_prompt_input := AiPromptInput {
                            width: Fill
                            height: Fit
                            padding: Inset {left: 12.0 right: 12.0 top: 10.0 bottom: 8.0}
                            draw_bg +: {
                                color: #0000
                                color_hover: #0000
                                color_focus: #0000
                                color_down: #0000
                                border_color: #0000
                            }
                        }

                        actions_row := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            align: Align {x: 0.0 y: 0.5}
                            padding: Inset {left: 12.0 right: 12.0 top: 4.0 bottom: 8.0}

                            ai_model_picker := DropDown {
                                width: 76.0
                                height: 24.0
                                margin: Inset {}
                                padding: Inset {left: 6.0 right: 14.0 top: 0.0 bottom: 0.0}
                                align: Align {x: 0.0 y: 0.5}
                                labels: ["local"]
                                draw_bg +: {
                                    color: vec4(1.0, 1.0, 1.0, 0.04)
                                    color_hover: vec4(1.0, 1.0, 1.0, 0.08)
                                    color_focus: vec4(1.0, 1.0, 1.0, 0.08)
                                    color_down: vec4(1.0, 1.0, 1.0, 0.04)
                                    border_color: vec4(1.0, 1.0, 1.0, 0.06)
                                    border_color_hover: vec4(1.0, 1.0, 1.0, 0.12)
                                    border_color_focus: vec4(1.0, 1.0, 1.0, 0.12)
                                    border_color_down: vec4(1.0, 1.0, 1.0, 0.06)
                                    border_color_disabled: theme.color_u_hidden
                                    border_color_2: theme.color_u_hidden
                                    border_color_2_hover: theme.color_u_hidden
                                    border_color_2_focus: theme.color_u_hidden
                                    border_color_2_down: theme.color_u_hidden
                                    border_color_2_disabled: theme.color_u_hidden
                                    border_radius: 12.0
                                    border_size: 1.0
                                }
                                draw_text +: {
                                    color: theme.color_label_inner
                                    text_style: theme.font_regular {
                                        font_size: 9.0
                                    }
                                }
                            }

                            ai_native_run_label := Label {
                                width: 58.0
                                margin: Inset {left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
                                text: "orchestrator"
                                draw_text +: {
                                    color: vec4(1.0, 1.0, 1.0, 0.38)
                                    text_style: theme.font_regular {
                                        font_size: 8.5
                                    }
                                }
                            }

                            ai_configure_button := ButtonFlat {
                                width: 24.0
                                height: 24.0
                                margin: Inset {}
                                padding: Inset {left: 0.0 right: 0.0 top: 0.0 bottom: 0.0}
                                text: "⚙"
                                draw_bg +: {
                                    color: vec4(1.0, 1.0, 1.0, 0.04)
                                    color_hover: vec4(1.0, 1.0, 1.0, 0.08)
                                    color_focus: vec4(1.0, 1.0, 1.0, 0.08)
                                    color_down: vec4(1.0, 1.0, 1.0, 0.04)
                                    border_color: vec4(1.0, 1.0, 1.0, 0.06)
                                    border_color_hover: vec4(1.0, 1.0, 1.0, 0.12)
                                    border_color_focus: vec4(1.0, 1.0, 1.0, 0.12)
                                    border_color_down: vec4(1.0, 1.0, 1.0, 0.06)
                                    border_color_disabled: theme.color_u_hidden
                                    border_color_2: theme.color_u_hidden
                                    border_color_2_hover: theme.color_u_hidden
                                    border_color_2_focus: theme.color_u_hidden
                                    border_color_2_down: theme.color_u_hidden
                                    border_color_2_disabled: theme.color_u_hidden
                                    border_radius: 12.0
                                    border_size: 1.0
                                }
                                draw_text +: {
                                    color: theme.color_label_inner
                                    font_size: 11.0
                                }
                            }

                            ai_run_button := AiRunButton {
                                width: 28.0
                                height: 24.0
                                draw_bg +: {
                                    radius: 12.0
                                }
                                draw_text +: {
                                    text_style: theme.font_bold {
                                        font_size: 9.5
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
