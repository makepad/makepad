use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoGlassPanel = UIZooTabLayout_B{
        desc +: {
            Markdown{
                body: "# GlassPanel\n\nReusable glass-style panel with tint, border, specular and noise controls. `use_scene_blur` and `blur_amount` are exposed for M2 visuals."
            }
        }
        demos +: {
            H4{text: "Default"}
            GlassPanel{
                width: 260
                height: 140
                padding: theme.mspace_3
                flow: Down
                spacing: theme.space_1
                Label{text: "GlassPanel"}
                Label{text: "Default material"}
            }

            Hr{}
            H4{text: "Custom tint + edge"}
            GlassPanel{
                width: 260
                height: 140
                padding: theme.mspace_3
                flow: Down
                spacing: theme.space_1
                draw_bg +: {
                    tint_color: #6af
                    tint_alpha: 0.23
                    border_color: #9cf
                    border_alpha: 0.6
                    border_width: 1.5
                    corner_radius: 18.0
                    specular_strength: 0.5
                    noise_strength: 0.025
                }
                Label{text: "Cool tint"}
                Label{text: "Rounded edge + stronger specular"}
            }

            Hr{}
            H4{text: "Scene blur path flag"}
            GlassPanel{
                width: 260
                height: 140
                padding: theme.mspace_3
                flow: Down
                spacing: theme.space_1
                draw_bg +: {
                    tint_color: #fff
                    tint_alpha: 0.18
                    use_scene_blur: 1.0
                    blur_amount: 0.75
                    specular_strength: 0.4
                    noise_strength: 0.04
                }
                Label{text: "use_scene_blur: true"}
                Label{text: "blur_amount: 0.75"}
            }
        }
    }
}
