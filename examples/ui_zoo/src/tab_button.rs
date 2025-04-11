use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoButton = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "Button"}
        }
        demos = {
            <H4> { text: "Standard"}
            <UIZooRowH> {
                basicbutton = <Button> {

                    draw_text: {
                        color: (THEME_COLOR_TEXT)
                        color_hover: (THEME_COLOR_TEXT_HOVER)
                        color_down: (THEME_COLOR_TEXT_DOWN)
                        text_style: <THEME_FONT_REGULAR> {
                            font_size: (THEME_FONT_SIZE_P)
                        }
                    }

                    icon_walk: {
                        width: (THEME_DATA_ICON_WIDTH), height: Fit,
                    }

                    draw_icon: {
                        color: (THEME_COLOR_TEXT)
                        color_hover: (THEME_COLOR_TEXT_HOVER)
                        color_down: (THEME_COLOR_TEXT_DOWN)
                    }

                    draw_bg: {
                        border_radius: (THEME_BEVELING)
                        border_radius: (THEME_CORNER_RADIUS)

                        color: (THEME_COLOR_OUTSET)
                        color_hover: (THEME_COLOR_OUTSET_HOVER)
                        color_down: (THEME_COLOR_OUTSET_DOWN)

                        border_color_1: (THEME_COLOR_BEVEL_LIGHT)
                        border_color_1_hover: (THEME_COLOR_BEVEL_LIGHT)
                        border_color_1_down: (THEME_COLOR_BEVEL_SHADOW)

                        border_color_2: (THEME_COLOR_BEVEL_SHADOW)
                        border_color_2_hover: (THEME_COLOR_BEVEL_SHADOW)
                        border_color_2_down: (THEME_COLOR_BEVEL_LIGHT)
                    }

                    text: "<Button>"
                }

                iconbutton = <ButtonIcon> {
                    draw_icon: {
                        color: #f00,
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                    text: "<ButtonIcon>"
                }
            }

            <Hr> {}
            <H4> { text: "GradientX"}
            <UIZooRowH> {
                <ButtonGradientX> { text: "<ButtonGradientX>" }
                <ButtonGradientX> {
                    draw_bg: {
                        border_radius: 1.0,
                        border_radius: 4.0

                        color_1: #C00
                        color_1_hover: #F0F
                        color_1_down: #800

                        color_2: #0CC
                        color_2_hover: #0FF
                        color_2_down: #088

                        border_color_1: #C
                        border_color_1_hover: #F
                        border_color_1_down: #0

                        border_color_2: #3
                        border_color_2_hover: #6
                        border_color_2_down: #8

                    }
                    text: "<ButtonGradientX>"
                }

            }

            <Hr> {}
            <H4> { text: "GradientY"}
            <UIZooRowH> {
                <ButtonGradientY> { text: "<ButtonGradientY>" }
                <ButtonGradientY> {
                    draw_bg: {
                        border_radius: 1.0,
                        border_radius: 4.0

                        color_1: #C00
                        color_1_hover: #F0F
                        color_1_down: #800

                        color_2: #0CC
                        color_2_hover: #0FF
                        color_2_down: #088

                        border_color_1: #C
                        border_color_1_hover: #F
                        border_color_1_down: #0

                        border_color_2: #3
                        border_color_2_hover: #6
                        border_color_2_down: #8

                    }
                    text: "<ButtonGradientY>"
                }

            }

            <Hr> {}
            <H4> { text: "Flat"}
            <UIZooRowH> {
                <ButtonFlat> {
                    draw_icon: {
                        color: #f00,
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                    text: "<ButtonFlat>"
                }

                <ButtonFlat> {
                    flow: Down,
                    icon_walk: { width: 15. }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                    text: "<ButtonFlat>"
                }
            }

            <Hr> {}
            <H4> { text: "Flatter"}
            <UIZooRowH> {
                <ButtonFlatter> {
                    draw_icon: {
                        color: #f00,
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                    text: "<ButtonFlatter>"
                }
            }

            <Hr> {}
            <H4> { text: "Custom"}
            styledbutton = <Button> {
            // Allows instantiation of customly styled elements as i.e. <MyButton> {}.

                // BUTTON SPECIFIC PROPERTIES

                draw_bg: { // Shader object that draws the bg.
                        fn pixel(self) -> vec4 {
                        return mix( // State transition animations.
                            mix(
                                #800,
                                mix(#800, #f, 0.5),
                                self.hover
                            ),
                            #00f,
                            self.down
                        )
                    }
                },

                draw_icon: { // Shader object that draws the icon.
                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    // Icon file dependency.

                    fn get_color(self) -> vec4 { // Overwrite the shader's fill method.
                        return mix( // State transition animations.
                            mix(
                                #f0f,
                                #fff,
                                self.hover
                            ),
                            #000,
                            self.down
                        )
                    }
                }

                grab_key_focus: true, // Keyboard gets focus when clicked.

                icon_walk: {
                    margin: 10.,
                    width: 16.,
                    height: Fit
                }

                label_walk: {
                    margin: 0.,
                    width: Fit,
                    height: Fit,
                }

                text: "Freely Styled <Button> clicked: 0", // Text label.

                animator: { // State change triggered animations.
                    hover = { // State
                        default: off // The state's starting point.
                        off = { // Behavior when the animation is started to the off-state
                            from: { // Behavior depending on the prior states
                                all: Forward {duration: 0.1}, // Default animation direction and speed in secs.
                                down: Forward {duration: 0.25} // Direction and speed for 'pressed' in secs.
                            }
                            apply: { // Shader methods to animate
                                draw_bg: { down: 0.0, hover: 0.0 } // Timeline target positions for the given states.
                                draw_icon: { down: 0.0, hover: 0.0 }
                                draw_text: { down: 0.0, hover: 0.0 }
                            }
                        }

                        on = { // Behavior when the animation is started to the on-state
                            from: {
                                all: Forward {duration: 0.1},
                                pressed: Forward {duration: 0.5}
                            }
                            apply: {
                                draw_bg: { down: 0.0, hover: [{time: 0.0, value: 1.0}] },
                                // pressed: 'pressed' timeline target position
                                // hover, time: Normalized timeline from 0.0 - 1.0. 'duration' then determines the actual playback duration of this animation in seconds.
                                // hover, value: target timeline position
                                draw_icon: { down: 0.0, hover: [{time: 0.0, value: 1.0}] },
                                draw_text: { down: 0.0, hover: [{time: 0.0, value: 1.0}] }
                            }
                        }
            
                        pressed = { // Behavior when the animation is started to the pressed-state
                            from: {all: Forward {duration: 0.2}}
                            apply: {
                                draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0}, 
                                draw_icon: {down: [{time: 0.0, value: 1.0}], hover: 1.0},
                                draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0}
                            }
                        }
                    }
                }

                // LAYOUT PROPERTIES

                height: Fit,
                // Element assumes the height of its children.

                width: Fill,
                // Element assumes the width of its children.

                margin: 5.0
                padding: { top: 3.0, right: 6.0, bottom: 3.0, left: 6.0 },
                // Individual space between the element's border and its content
                // for top and left.

                flow: Right,
                // Stacks children from left to right.

                spacing: 5.0,
                // A spacing of 10.0 between children.

                align: { x: 0.5, y: 0.5 },
                // Positions children at the left (x) bottom (y) corner of the parent.
            }
        }
    }
}