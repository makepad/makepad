use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let SidebarFilterInput = TextInputFlat {
        height: 26.0
        margin: Inset {}
        padding: Inset {left: 12.0 right: 12.0 top: 5.0 bottom: 1.0}
        draw_bg +: {
            border_radius: 4.0

            color: theme.color_bg_app * 0.82
            color_hover: theme.color_bg_app * 0.88
            color_focus: theme.color_bg_app * 0.92
            color_down: theme.color_bg_app * 0.85
            color_empty: theme.color_bg_app * 0.82

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

    mod.widgets.FileTreePane = View {
        width: Fill
        height: Fill
        flow: Down
        PaneToolbar {
            file_tree_filter := SidebarFilterInput {
                width: Fill
                empty_text: "Filter"
            }
        }
        file_tree := DesktopFileTree {}
    }

    mod.widgets.RunListPane = View {
        width: Fill
        height: Fill
        flow: Down
        PaneToolbar {
            padding: Inset {left: 10.0 right: 4.0 top: 0.0 bottom: 0.0}
            align: Align {x: 0.0 y: 0.5}
            Label {
                text: "Run Targets"
                draw_text +: {
                    font_size: theme.font_size_p - 1.0
                    color: theme.color_label_inner
                    text_style: theme.font_bold
                }
            }
            Filler {}
            run_stop_all := ButtonFlatter {
                width: 24.0
                height: 24.0
                text: ""
                padding: 0.0
                margin: 0.0
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.box(8.0, 8.0, 8.0, 8.0, 1.0)
                        let color = #xef596f
                        sdf.fill(color.mix(#xFFFFFF, self.hover * 0.2))
                        return sdf.result
                    }
                }
            }
        }
        run_list := DesktopRunList {}
    }
}
