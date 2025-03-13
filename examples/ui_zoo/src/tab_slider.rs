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
            flow: Right,
            <View> {
                flow: Down,
                <Slider> { text: "Default" }
                <Slider> { text: "label_align", label_align: { x: 0.5, y: 0. } }
                <Slider> { text: "min/max", min: 0., max: 100. }
                <Slider> { text: "precision", precision: 20 }
                <Slider> { text: "stepped", step: 0.1 }
                <SliderBig> { text: "Default" }
                <SliderBig> { text: "label_align", label_align: { x: 0.5, y: 0. } }
                <SliderBig> { text: "min/max", min: 0., max: 100. }
                <SliderBig> { text: "precision", precision: 20 }
                <SliderBig> { text: "stepped", step: 0.1 }
                <SliderAlt1> {
                    text: "Colored",
                    draw_slider: {
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
                    draw_slider: {
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
                    draw_slider: {
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
                    draw_slider: {label_size: 150. },
                }
            }
            <View> {
                flow: Down,
                <View> {
                    width: Fill, height: Fit,
                    flow: Right,
                    <Rotary> {
                        width: 100, height: 100,
                        text: "Colored",
                        draw_slider: {
                            gap: 90.,
                            val_size: 20.
                            val_padding: 2.,
                        }
                    }
                    <Rotary> {
                        width: 100, height: 200,
                        text: "Colored",
                        draw_slider: {
                            gap: 60.,
                            val_size: 10.,
                            val_padding: 4.,
                        }
                    }
                    <Rotary> {
                        width: 200, height: 100,
                        text: "Colored",
                        draw_slider: {
                            gap: 75.,
                            val_size: 20.
                            val_padding: 4,
                        }
                    }
                    <Rotary> {
                        width: 200, height: 150,
                        text: "Colored",
                        draw_slider: {
                            gap: 90.,
                            val_size: 20.
                            val_padding: 4.,
                        }
                    }
                    <Rotary> {
                        width: Fill, height: 150,
                        text: "Colored",
                        draw_slider: {
                            gap: 60.,
                            val_size: 20.
                            val_padding: 2.,
                        }
                    }
                }
                <View> {
                    width: Fill, height: Fit,
                    flow: Right,
                    <Rotary> {
                        width: 100., height: 100.,
                        text: "Colored",
                        draw_slider: {
                            gap: 0.,
                            val_size: 20.
                            val_padding: 0.,
                        }
                    }
                    <Rotary> {
                        width: 120., height: 120.,
                        text: "Solid",
                        draw_text: {
                            color: #0ff;
                        }
                        draw_slider: {
                            val_color_1: #ff0,
                            val_color_2: #f00,
                            handle_color: #f,
                            gap: 180.,
                            val_size: 20.,
                            val_padding: 2.,
                        }
                    }
                    <Rotary> {
                        width: 120., height: 120.,
                        text: "Solid",
                        draw_slider: {
                            val_color_1: #0ff,
                            val_color_2: #ff0,
                            handle_color: #f,
                            gap: 90.,
                            val_size: 20.,
                            val_padding: 2.,
                        }
                    }
                    <Rotary> {
                        width: 100., height: 90.,
                        text: "Solid",
                        draw_slider: {
                            gap: 90.,
                            val_padding: 10.,
                            val_size: 20.,
                            val_padding: 2.
                            handle_color: #f0f,
                        }
                    }
                    <Rotary> {
                        width: 150., height: 150.,
                        text: "Solid",
                        draw_slider: {
                            val_color_1: #0ff,
                            val_color_2: #0ff,
                            gap: 180.,
                            val_padding: 4.,
                            val_size: 6.,
                        }
                    }
                    <Rotary> {
                        width: 150., height: 150.,
                        text: "Solid",
                        draw_slider: {
                            gap: 0.,
                            val_size: 10.0,
                            val_padding: 4.,
                        }
                    }
                }
                
                <View> {
                    width: Fill, height: Fit,
                    flow: Right,
                    <RotaryFlat> {
                        width: 100., height: 100.,
                        text: "Colored",
                        draw_slider: {
                            gap: 0.,
                            width: 20.
                            val_padding: 0.,
                        }
                    }
                    <RotaryFlat> {
                        width: 120., height: 120.,
                        text: "Solid",
                        draw_text: {
                            color: #0ff;
                        }
                        draw_slider: {
                            val_color_1: #ff0,
                            val_color_2: #f00,
                            handle_color: #f,
                            gap: 180.,
                            width: 20.,
                            val_padding: 2.,
                        }
                    }
                    <RotaryFlat> {
                        width: 120., height: 120.,
                        text: "Solid",
                        draw_slider: {
                            val_color_1: #0ff,
                            val_color_2: #ff0,
                            handle_color: #f,
                            gap: 90.,
                            width: 20.,
                            val_padding: 2.,
                        }
                    }
                    <RotaryFlat> {
                        width: 100., height: 90.,
                        text: "Solid",
                        draw_slider: {
                            gap: 90.,
                            val_padding: 10.,
                            width: 20.,
                            handle_color: #f0f,
                        }
                    }
                    <RotaryFlat> {
                        width: 150., height: 150.,
                        text: "Solid",
                        draw_slider: {
                            val_color_1: #0ff,
                            val_color_2: #0ff,
                            gap: 180.,
                            val_padding: 4.,
                            width: 6.,
                        }
                    }
                    <RotaryFlat> {
                        width: Fill, height: 150.,
                        text: "Solid",
                        draw_slider: {
                            val_color_1: #8;
                            val_color_2: #ff0;
                            gap: 75.,
                            width: 40.0,
                            val_padding: 4.,
                        }
                    }
                }
                <View> {
                    width: Fill, height: Fit,
                    flow: Right,
                    <RotarySolid> {
                        width: 100, height: 100,
                        text: "Colored",
                        draw_slider: {
                            gap: 90.,
                        }
                    }
                    <RotarySolid> {
                        width: 200, height: 150,
                        text: "Colored",
                        draw_slider: {
                            gap: 180.,
                        }
                    }
                    <RotarySolid> {
                        width: Fill, height: 150,
                        text: "Colored",
                        draw_slider: {
                            gap: 60.,
                        }
                    }
                }
            }
        }
    }
}