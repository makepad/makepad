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
            <H3> { text: "<Slider>"}
        }
        demos = {
            <H4> { text: "Slider"}
            <Slider> { text: "Default" }
            <Slider> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <Slider> { text: "min/max", min: 0., max: 100. }
            <Slider> { text: "precision", precision: 20 }
            <Slider> { text: "stepped", step: 0.1 }

            <Hr> {}
            <H4> { text: "SliderBig"}
            <SliderBig> { text: "Default" }
            <SliderBig> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <SliderBig> { text: "min/max", min: 0., max: 100. }
            <SliderBig> { text: "precision", precision: 20 }
            <SliderBig> { text: "stepped", step: 0.1 }


            <Hr> {}
            <H4> { text: "SliderAlt1"}
            <SliderAlt1> {
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
            <SliderAlt1> {
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
            <SliderAlt1> {
                text: "Solid",
                draw_bg: {
                    val_color_1: #6,
                    val_color_2: #6,
                    handle_color: #0,
                }
            }
            <SliderAlt1> { text: "min/max", min: 0., max: 100. }
            <SliderAlt1> { text: "precision", precision: 20 }
            <SliderAlt1> { text: "stepped", step: 0.1 }
            <SliderAlt1> {
                text: "label_size",
                draw_bg: {label_size: 150. },
            }


            <Hr> {}
            <H4> { text: "Rotary"}
            <UIZooRowH> {
                <Rotary> {
                    text: "Default",
                }
                <Rotary> {
                    text: "Gap",
                    draw_bg: {
                        gap: 180.,
                    }
                }
                <Rotary> {
                    text: "ValSize",
                    draw_bg: {
                        val_size: 30.
                    }
                }
                <Rotary> {
                    text: "val_padding",
                    draw_bg: {
                        val_size: 30.
                        val_padding: 20.,
                    }
                }
            }

            <Hr> {}
            <H4> { text: "Rotary styled" }
            <Rotary> {
                text: "Solid",
                draw_text: {
                    color: #0f0;
                    color_hover: #0ff;
                    color_focus: #fff;
                    color_drag: #f00;
                }
                draw_bg: {
                    val_color_1: #80C,
                    val_color_1_hover: #88F,
                    val_color_1_focus: #80F,
                    val_color_1_drag: #F8F,

                    val_color_2: #C00,
                    val_color_2_hover: #F00,
                    val_color_2_focus: #F80,
                    val_color_2_drag: #F88,

                    handle_color: #f,
                    gap: 180.,
                    val_size: 20.,
                    val_padding: 2.,
                }
            }

            <Hr> {}
            <H4> { text: "RotaryFlat" }
            <UIZooRowH> {
                <RotaryFlat> {
                    text: "Default",
                }
                <RotaryFlat> {
                    text: "Gap",
                    draw_bg: {
                        gap: 180.,
                    }
                }
                <RotaryFlat> {
                    text: "ValSize",
                    draw_bg: {
                        val_size: 30.
                    }
                }
                <RotaryFlat> {
                    text: "val_padding",
                    draw_bg: {
                        val_size: 30.
                        val_padding: 20.,
                    }
                }
            }

            <Hr> {}
            <H4> { text: "RotaryFlat styled" }
            <UIZooRowH> {
                <RotaryFlat> {
                    text: "Gap",
                    draw_bg: {
                        gap: 0.,
                        width: 20.
                        val_padding: 0.,
                    }
                }

                <RotaryFlat> {
                    text: "Solid",
                    draw_text: {
                        color: #0ff;
                    }
                    draw_bg: {
                        val_color_1: #ff0,
                        val_color_2: #f00,
                        handle_color: #f,
                        gap: 180.,
                        width: 20.,
                        val_padding: 2.,
                    }
                }
                <RotaryFlat> {
                    text: "Solid",
                    draw_bg: {
                        val_color_1: #0ff,
                        val_color_2: #ff0,
                        handle_color: #f,
                        gap: 90.,
                        width: 20.,
                        val_padding: 2.,
                    }
                }
                <RotaryFlat> {
                    text: "Solid",
                    draw_bg: {
                        val_color_1: #8;
                        val_color_2: #ff0;
                        gap: 75.,
                        width: 30.0,
                        val_padding: 4.,
                    }
                }
            }

            <Hr> {}
            <H4> { text: "Rotary Solid"}
            <UIZooRowH> {
                <RotarySolid> {
                    text: "Colored",
                    draw_bg: {
                        gap: 90.,
                    }
                }
                <RotarySolid> {
                    text: "Colored",
                    draw_bg: {
                        gap: 180.,
                    }
                }
                <RotarySolid> {
                    text: "Colored",
                    draw_bg: {
                        gap: 60.,
                    }
                }
            }


        }
    }
}