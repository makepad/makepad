use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoSlider = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "Slider"}
        }
        demos = {
            <H4> { text: "Slider"}
            <Slider> { text: "Default" }
            <Slider> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <Slider> { text: "min/max", min: 0., max: 100. }
            <Slider> { text: "precision", precision: 20 }
            <Slider> { text: "stepped", step: 0.1 }

            <Hr> {}
            <H4> { text: "SliderFlat"}
            <SliderFlat> { text: "Default" }
            <SliderFlat> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <SliderFlat> { text: "min/max", min: 0., max: 100. }
            <SliderFlat> { text: "precision", precision: 20 }
            <SliderFlat> { text: "stepped", step: 0.1 }

            <Hr> {}
            <H4> { text: "SliderFlatter"}
            <SliderFlatter> { text: "Default" }
            <SliderFlatter> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <SliderFlatter> { text: "min/max", min: 0., max: 100. }
            <SliderFlatter> { text: "precision", precision: 20 }
            <SliderFlatter> { text: "stepped", step: 0.1 }

            <H4> { text: "SliderMinimal"}
            <SliderMinimal> { text: "Default" }
            <SliderMinimal> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <SliderMinimal> { text: "min/max", min: 0., max: 100. }
            <SliderMinimal> { text: "precision", precision: 20 }
            <SliderMinimal> { text: "stepped", step: 0.1 }

            <H4> { text: "SliderMinimalFlat"}
            <SliderMinimalFlat> { text: "Default" }
            <SliderMinimalFlat> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <SliderMinimalFlat> { text: "min/max", min: 0., max: 100. }
            <SliderMinimalFlat> { text: "precision", precision: 20 }
            <SliderMinimalFlat> { text: "stepped", step: 0.1 }

            <Hr> {}
            <H4> { text: "SliderRound"}
            <SliderRound> {
                text: "Colored",
                draw_bg: {
                    val_color_1: #FFCC00
                    val_color_1_hover: #FF9944
                    val_color_1_focus: #FFCC44
                    val_color_1_drag: #FFAA00

                    val_color_2: #F00
                    val_color_2_hover: #F00
                    val_color_2_focus: #F00
                    val_color_2_drag: #F00

                    handle_color: #0000
                    handle_color_hover: #0008
                    handle_color_focus: #000C
                    handle_color_drag: #000F
                }
            }
            <SliderRound> {
                text: "Solid",
                draw_text: {
                    color: #0ff;
                }
                draw_bg: {
                    val_color_1: #F08
                    val_color_1_hover: #F4A
                    val_color_1_focus: #C04
                    val_color_1_drag: #F08

                    val_color_2: #F08
                    val_color_2_hover: #F4A
                    val_color_2_focus: #C04
                    val_color_2_drag: #F08

                    handle_color: #F
                    handle_color_hover: #F
                    handle_color_focus: #F
                    handle_color_drag: #F
                }
            }
            <SliderRound> {
                text: "Solid",
                draw_bg: {
                    val_color_1: #6,
                    val_color_2: #6,
                    handle_color: #0,
                }
            }
            <SliderRound> { text: "min/max", min: 0., max: 100. }
            <SliderRound> { text: "precision", precision: 20 }
            <SliderRound> { text: "stepped", step: 0.1 }
            <SliderRound> {
                text: "label_size",
                draw_bg: {label_size: 150. },
            }

            <Hr> {}
            <H4> { text: "SliderRoundFlat"}
            <SliderRoundFlat> { text: "min/max", min: 0., max: 100. }
            <SliderRoundFlat> { text: "precision", precision: 20 }
            <SliderRoundFlat> { text: "stepped", step: 0.1 }

            <Hr> {}
            <H4> { text: "SliderRoundFlatter"}
            <SliderRoundFlatter> { text: "min/max", min: 0., max: 100. }
            <SliderRoundFlatter> { text: "precision", precision: 20 }
            <SliderRoundFlatter> { text: "stepped", step: 0.1 }
        }
    }
}