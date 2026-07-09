use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.CodeEditorPane = View {
        width: Fill
        height: Fill
        flow: Down
        code_editor := DesktopCodeEditor {
            editor +: {
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                        let color = vec4(theme.color_bg_container.rgb, 0.25)
                        let highlight = 0.03 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                        let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.005
                        sdf.fill(vec4(color.rgb + highlight + noise, color.a))
                        sdf.stroke(vec4(1.0, 1.0, 1.0, 0.04), 1.0)
                        return sdf.result
                    }
                }
                draw_cursor_bg +: {
                    pixel: fn() {
                        let color = theme.color_u_hidden.mix(vec4(1.0, 1.0, 1.0, 0.035), self.focus)
                        return vec4(color.rgb * color.a, color.a)
                    }
                }
            }
        }
    }

    mod.widgets.EditorFirstPane = RectView {
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
    }

    mod.widgets.RunningAppPane = View {
        width: Fill
        height: Fill
        flow: Down
        run_view := DesktopRunView {}
    }

    mod.widgets.RunFirstPane = RectView {
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
            align: Align {x: 0.5 y: 0.5}
            placeholder := Label {
                text: "Click play in Run to launch"
                draw_text.color: theme.color_label_outer
            }
        }
    }
}
