use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let STUDIO_HEADER_HEIGHT = 36.0

    mod.widgets.PaneToolbar = RectView {
        width: Fill
        height: STUDIO_HEADER_HEIGHT
        flow: Right
        align: Align {x: 0.0 y: 0.5}
        padding: Inset {left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
        spacing: theme.space_2
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                let color = vec4(theme.color_bg_container.rgb, 0.15)
                let highlight = 0.03 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.005
                sdf.fill(vec4(color.rgb + highlight + noise, color.a))

                // Bottom separator line
                let thickness = 1.0
                if self.pos.y * self.rect_size.y >= self.rect_size.y - thickness {
                    sdf.clear(vec4(1.0, 1.0, 1.0, 0.05))
                }
                return sdf.result
            }
        }
    }
}
