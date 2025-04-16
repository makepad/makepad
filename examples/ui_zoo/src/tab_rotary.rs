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
            <H3> { text: "Rotary"}
        }
        demos = {
            <H4> { text: "<Rotary>"}
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

            <Hr> {}
            <H4> { text: "Rotary Solid"}
            <UIZooRowH> {
                <RotarySolidFlat> {
                    text: "Colored",
                    draw_bg: {
                        gap: 90.,
                    }
                }
                <RotarySolidFlat> {
                    text: "Colored",
                    draw_bg: {
                        gap: 180.,
                    }
                }
                <RotarySolidFlat> {
                    text: "Colored",
                    draw_bg: {
                        gap: 60.,
                    }
                }
            }

        }
    }
}