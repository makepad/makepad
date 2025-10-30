use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    Box = <RoundedView> {
        show_bg: true,
        draw_bg: { color: #0F02 }
        padding: 3.
        align: { x: 0.5, y: 0.5 }
        draw_bg: {
            border_size: 1.
            border_radius: 0.
            border_color: #fff8
        }
    }

    BoxLabel = <P> {
        width: Fit,
        align: { x: 0.5 }
    }


    pub DemoLayout = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/layout.md") } 
        }
        demos = {
            <H4> { text: "Width & Height"}
            <UIZooRowH> {
                flow: Right,
                height: 100.
                <Box> {
                    width: 100., height: 60.
                    <BoxLabel> { text: "width: 100.\nheight: 60" }
                }
                <Box> {
                    width: 100., height: Fill
                    <BoxLabel> { text: "width: 100.\nheight: Fill" }
                }
                <Box> {
                    width: 150., height: Fit
                    <BoxLabel> { text: "width: 150.\nheight: Fit" }
                }
            }

            <Hr> {}
            <H4> { text: "Margin"}
            <UIZooRowH> {
                align: { x: 0., y: 0. }
                flow: Right
                spacing: 0.
                <Box> {
                    width: Fit, height: Fit,
                    margin: 0.,
                    <BoxLabel> { text: "margin: 0." }
                }
                <Box> {
                    width: Fit, height: Fit,
                    margin: 0.,
                    <BoxLabel> { text: "margin: 0." }
                }
                <Box> {
                    width: Fit, height: Fit,
                    margin: 10.,
                    <BoxLabel> { text: "margin: 10." }
                }
                <Box> {
                    width: Fit, height: Fit,
                    margin: {top: 0., left: 40, right: 0, bottom: 0.},
                    <BoxLabel> { text: "margin: {top: 0., left: 40, right: 0, bottom: 0.}" }
                }
            }

            <Hr> {}
            <H4> { text: "Padding"}
            <UIZooRowH> {
                <Box> {
                    width: Fit, height: Fit,
                    padding: 20.
                    <BoxLabel> { text: "padding: 20." }
                }
                <Box> {
                    width: Fit, height: Fit,
                    padding: { left: 40., right: 10. }
                    <BoxLabel> { text: "padding: { left: 40., right: 10. }" }
                }
                <Box> {
                    align: {x: 0., y: 0.}
                    width: Fit, height: Fit,
                    padding: { left: 40., bottom: 20., right: 25., top: 0. }
                    <BoxLabel> { text: "padding: { left: 40., bottom: 20., right: 25., top: 0. }" }
                }
            }

            <Hr> {}
            <H4> { text: "Spacing"}
            <Pbold> { text: "spacing: 10." }
            <UIZooRowH> {
                spacing: 10.
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
            }
            <Pbold> { text: "spacing: 30." }
            <UIZooRowH> {
                spacing: 30.
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
            }

            <Hr> {}
            <H4> { text: "Flow Direction"}
            <Pbold> { text: "flow: Right" }
            <UIZooRowH> {
                spacing: 10.
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
            }
            <Pbold> { text: "flow: Down" }
            <UIZooRowH> {
                flow: Down,
                spacing: 10.
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
                <Box> { height: 50, width: 50.}
            }


            <Hr> {}
            <H4> { text: "Align"}
            <Pbold> { text: "align: { x: 0., y: 0.}" }
            <UIZooRowH> {
                align: { x: 0., y: 0.}
                <Box> { height: 100, width: 50.}
                <Box> { height: 20, width: 50.}
                <Box> { height: 50, width: 50.}
            }
            <Pbold> { text: "align: { x: 0.0, y: 0.5}" }
            <UIZooRowH> {
                align: { x: 0.0, y: 0.5}
                <Box> { height: 100, width: 50.}
                <Box> { height: 20, width: 50.}
                <Box> { height: 50, width: 50.}
            }
            <Pbold> { text: "align: { x: 0., y: 1.}" }
            <UIZooRowH> {
                align: { x: 0.0, y: 1.0}
                <Box> { height: 100, width: 50.}
                <Box> { height: 20, width: 50.}
                <Box> { height: 50, width: 50.}
            }
            <Pbold> { text: "align: { x: 0.5, y: 0.}" }
            <UIZooRowH> {
                align: { x: .5, y: 0.}
                <Box> { height: 100, width: 50.}
                <Box> { height: 20, width: 50.}
                <Box> { height: 50, width: 50.}
            }
            <Pbold> { text: "align: { x: 0.0, y: 0.5}" }
            <UIZooRowH> {
                align: { x: 1.0, y: 1.}
                <Box> { height: 100, width: 50.}
                <Box> { height: 20, width: 50.}
                <Box> { height: 50, width: 50.}
            }


            <Hr> {}
            <H4> { text: "AdaptiveView"}
            <UIZooRowH> {
                width: Fill, height: 450.
                flow: Down,
                <P> { text: "Resize the window to make the UI update on lower resolutions."}
                <AdaptiveView> {
                    Desktop = {
                        flow: Down,
                        spacing: (THEME_SPACE_2)

                        <View> {
                            show_bg: true,
                            draw_bg: {color: #800}

                            width: Fill,
                            height: Fit,
                            padding: 10.

                            <H2> { width: Fit, height: Fit, text: "Desktop"}
                        }
                        <View> {
                            flow: Right,
                            spacing: (THEME_SPACE_2)
                            <View> {
                                show_bg: true,
                                draw_bg: {color: #008}

                                width: 200,
                                height: Fill,
                                padding: 10.

                                <H3> { width: Fit, height: Fit, text: "Menu"}
                            }
                            <View> {
                                show_bg: true,
                                draw_bg: {color: #080}

                                width: Fill,
                                height: Fill,
                                padding: 10.

                                <H3> { width: Fit, height: Fit, text: "Content"}
                            }
                        }
                    } 

                    Mobile = {
                        flow: Down,
                        show_bg: true,
                        draw_bg: {color: #008}
                        spacing: (THEME_SPACE_1)

                        <View> {
                            show_bg: true,
                            draw_bg: {color: #800}

                            width: Fill,
                            height: Fit,
                            padding: 10.

                            <H2> { width: Fit, height: Fit, text: "Mobile"}
                        }

                        <View> {
                            show_bg: true,
                            draw_bg: {color: #008}

                            width: 200,
                            height: 100,
                            padding: 10.

                            <H3> { width: Fill, height: Fit, text: "Menu"}
                        }

                        <View> {
                            show_bg: true,
                            draw_bg: {color: #080}

                            width: Fill,
                            height: Fill,
                            padding: 10.

                            <H3> { width: Fit, height: Fit, text: "Content"}
                        }
                    } 

                }
            }
        }
    }
}