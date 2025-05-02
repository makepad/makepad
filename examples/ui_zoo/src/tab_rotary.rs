use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoRotary = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/rotary.md") } 
        }
        demos = {
            <H4> { text: "<Rotary>"}
            <UIZooRowH> {
                align: { x: 0. , y: 0.}
                <Rotary> {
                    text: "Label",
                }
                <Rotary> {
                    text: "Label",
                    draw_bg: {
                        // border_size: 3.
                        gap: 0.,
                    }
                }
                <Rotary> {
                    width: 300, height: 200
                    text: "Gap",
                    draw_bg: {
                        // border_size: 3.
                        gap: 180.,
                    }
                }
                <Rotary> {
                    text: "Label",
                    draw_bg: {
                        val_size: 30.
                    }
                }
                <Rotary> {
                    width: Fill,
                    height: 300
                    text: "Label",
                    draw_bg: {
                        // border_size: 5.
                        val_size: 30.
                        val_padding: 20.,
                    }
                }
            }

            <H4> { text: "<RotaryGradientY>"}
            <UIZooRowH> {
                align: { x: 0. , y: 0.}
                <RotaryGradientY> {
                    text: "Label",
                }
                <RotaryGradientY> {
                    text: "Label",
                    draw_bg: {
                        // border_size: 3.
                        gap: 0.,
                    }
                }
                <RotaryGradientY> {
                    width: 300, height: 200
                    text: "Gap",
                    draw_bg: {
                        // border_size: 3.
                        gap: 180.,
                    }
                }
                <RotaryGradientY> {
                    text: "Label",
                    draw_bg: {
                        val_size: 30.
                    }
                }
                <RotaryGradientY> {
                    width: Fill,
                    height: 300
                    text: "Label",
                    draw_bg: {
                        // border_size: 5.
                        val_size: 30.
                        val_padding: 20.,
                    }
                }
            }

            <Hr> {}
            <H4> { text: "Rotary styled" }
            <Rotary> {
                text: "Solid",

                label_walk: {
                    width: Fit, height: Fit,
                    margin: {bottom: (THEME_SPACE_1)},
                }

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
                    text: "Label",
                }
                <RotaryFlat> {
                    text: "Gap",
                    draw_bg: {
                        gap: 180.,
                    }
                }
                <RotaryFlat> {
                    text: "Label",
                    draw_bg: {
                        val_size: 30.
                    }
                }
                <RotaryFlat> {
                    text: "Label",
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
                        val_size: 30.
                        val_padding: 20.,
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
                        val_size: 20.,
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
                        val_size: 20.,
                        val_padding: 2.,
                    }
                }
                <RotaryFlat> {
                    text: "Solid",
                    draw_bg: {
                        val_color_1: #8;
                        val_color_2: #ff0;
                        gap: 75.,
                        val_size: 30.0,
                        val_padding: 4.,
                    }
                }
            }

            <Hr> {}
            <H4> { text: "RotaryFlatter" }
            <UIZooRowH> {
                <RotaryFlatter> { text: "RotaryFlatter" }
            }

        }
    }
}