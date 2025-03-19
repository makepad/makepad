use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoAdaptiveView = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<AdaptiveView>"}
            <P> { text: "Resizing the window will make the layout block switch between a desktop and a mobile version."}
        }
        demos = {
            <AdaptiveView> {
                Desktop = {
                    flow: Down,
                    spacing: 10.

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
                        spacing: 10.
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
                    spacing: 5.

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