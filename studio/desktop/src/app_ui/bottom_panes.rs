use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let STUDIO_HEADER_HEIGHT = 36.0

    let LogToolbarToggle = Toggle {
        margin: Inset {}
        padding: Inset {left: 0.0 right: 0.0 top: 0.0 bottom: 0.0}
        label_walk: Walk {width: Fit height: Fit margin: Inset {left: 24.0 right: 0.0 top: 0.0 bottom: 0.0}}
        draw_bg +: {
            size: 13.0
        }
        draw_text +: {
            color: theme.color_label_outer_off
            color_hover: theme.color_label_outer
            color_active: theme.color_label_outer
        }
    }

    let LogToolbarFilterInput = TextInputFlat {
        height: 26.0
        margin: Inset {}
        padding: Inset {left: 10.0 right: 10.0 top: 5.0 bottom: 1.0}
        draw_bg +: {
            border_radius: 4.0

            color: theme.color_bg_app * 0.84
            color_hover: theme.color_bg_app * 0.9
            color_focus: theme.color_bg_app * 0.94
            color_down: theme.color_bg_app * 0.87
            color_empty: theme.color_bg_app * 0.84

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_focus: theme.color_u_hidden
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
            color_empty: theme.color_label_inner_inactive
            color_empty_hover: theme.color_label_inner_inactive
            color_empty_focus: theme.color_label_outer
        }
    }

    let LogToolbarButton = ButtonFlatter {
        margin: Inset {}
        padding: Inset {left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
        draw_text +: {
            color: theme.color_label_outer_off
            color_hover: theme.color_label_outer
            color_down: theme.color_label_outer
            color_focus: theme.color_label_outer
        }
    }

    let LogToolbarIconButton = ButtonFlatterIcon {
        width: 22.0
        height: 22.0
        margin: Inset {}
        icon_walk: Walk {width: 13.0 height: 13.0}
        draw_icon +: {
            color: theme.color_label_outer_off
            color_hover: theme.color_label_outer
            color_down: theme.color_label_outer
            color_focus: theme.color_label_outer
        }
    }

    mod.widgets.LogPane = View {
        width: Fill
        height: Fill
        flow: Down
        PaneToolbar {
            View {
                width: Fit
                height: Fit
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                spacing: theme.space_1

                log_tail_toggle := LogToolbarToggle {
                    text: "Tail"
                    active: true
                }
            }
            Filler {}
            View {
                width: Fit
                height: Fit
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                spacing: 4.0

                log_filter := LogToolbarFilterInput {
                    width: 216.0
                    empty_text: "Filter"
                }
                clear_log_filter := LogToolbarButton {
                    width: 20.0
                    height: 20.0
                    text: "x"
                    padding: Inset {left: 0.0 right: 0.0 top: 0.0 bottom: 0.0}
                }
            }
            View {width: 10.0 height: Fit}
            View {
                width: Fit
                height: Fit
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                spacing: 8.0

                clear_log := LogToolbarButton {
                    text: "Clear"
                }
                log_open_profiler := LogToolbarIconButton {
                    draw_icon +: {
                        svg: crate_resource("self://resources/icons/icon_profiler.svg")
                    }
                }
            }
        }
        log_view := DesktopLogView {}
    }

    mod.widgets.ProfilerPane = View {
        width: Fill
        height: Fill
        flow: Down
        profiler_view := DesktopProfilerView {}
    }

    mod.widgets.LogFirstPane = mod.widgets.LogPane {}

    let StudioTerminalView = DesktopTerminalView {
        pad_x: 6.0
        pad_y: 4.0
    }

    mod.widgets.TerminalPane = View {
        width: Fill
        height: Fill
        flow: Down
        terminal_view := StudioTerminalView {}
    }

    mod.widgets.TerminalFirstPane = RectView {
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                let color = vec4(theme.color_bg_container.rgb, 0.3)
                let highlight = 0.04 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.005
                sdf.fill(vec4(color.rgb + highlight + noise, color.a))
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.05), 1.0)
                return sdf.result
            }
        }
        View {
            width: Fill
            height: Fill
            flow: Down
            align: Center
            spacing: theme.space_3
            placeholder := Label {
                text: "Terminal tabs live here"
                draw_text.color: theme.color_label_outer
            }
            terminal_add_button := ButtonFlat {
                width: 136.0
                text: "Add Terminal"
            }
        }
    }
}
