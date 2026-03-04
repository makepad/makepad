//! Custom button widgets extracted from Robrix.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // Customized button widget for Robrix-style icon buttons
    mod.widgets.RobrixIconButton = Button {
        width: Fit,
        height: Fit,
        spacing: 10,
        padding: 10,
        align: Align{x: 0, y: 0.5}

        draw_bg +: {
            color: (COLOR_PRIMARY)
            color_hover: #A
            border_size: 0.0
            border_color: #D0D5DD
            border_radius: 4.0

            get_color: fn() -> vec4 {
                return mix(self.color, mix(self.color, self.color_hover, 0.2), self.hover)
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - (self.border_size * 2.0),
                    self.rect_size.y - (self.border_size * 2.0),
                    max(1.0, self.border_radius)
                )
                sdf.fill_keep(self.get_color())
                if self.border_size > 0.0 {
                    sdf.stroke(self.border_color, self.border_size)
                }
                return sdf.result;
            }
        }

        draw_icon.color: #F00
        icon_walk: Walk{width: 16, height: 16}

        draw_text +: {
            color: #000
            color_hover: #999
            color_down: #555
            text_style: mod.widgets.REGULAR_TEXT {font_size: 10},
        }
        text: ""
    }
}
