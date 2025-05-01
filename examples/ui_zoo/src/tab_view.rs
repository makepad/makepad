use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoView = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/view.md") } 
        }
        demos = {
            <H4> { text: "View" }
            <View> {
                width: Fit, height: Fit, 
                padding: <THEME_MSPACE_2> {},
                align: { x: 0.5, y: 0.5 }
                <Label> { text: "<View>" }
            }
            
            <Hr> {}
            <H4> { text: "Style Templates" }
            <UIZooRowH> {
                flow: RightWrap
                <SolidView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }
                    draw_bg: { color: #0 }
                    <Label> { text: "<SolidView>" }
                }

                <Vr> {
                    draw_bg: {
                        color_1: #000,
                        color_2: #000,
                    }
                }

                <Hr> {
                    width: 25.,
                    draw_bg: {
                        color_1: #0,
                        color_2: #5,
                    }
                }


                <RectView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }
                    draw_bg: {
                        color: #000,
                        border_size: 2.,
                        border_color: #8,
                        border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                    }
                    <Label> { text: "<RectView>" }
                }

                <RectShadowView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }

                    draw_bg: {
                        color: #000,
                        border_size: 2.0
                        border_color: #8
                        shadow_color: #0007
                        shadow_offset: vec2(5.0,5.0)
                        shadow_radius: 10.0
                    }

                    <Label> { text: "<RectShadowView>" }
                }

                <RoundedShadowView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }

                    draw_bg: {
                        color: #000,
                        border_radius: 5.,
                        border_color: #8
                        border_size: 2.0
                        shadow_color: #0007
                        shadow_radius: 10.0
                        shadow_offset: vec2(5.0,5.0)
                    }

                    <Label> { text: "<RoundedShadowView>" }
                }

                <RoundedView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }

                    draw_bg: {
                        color: #000,
                        border_radius: 5.,
                        border_size: 2.0
                        border_color: #8
                        border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                    }

                    <Label> { text: "<RoundedView>" }
                }

                <RoundedXView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }

                    draw_bg: {
                        color: #000,
                        border_radius: vec2(1.0, 5.0),
                        border_size: 2.0
                        border_color: #8
                        border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                    }

                    <Label> { text: "<RoundedXView>" }
                }

                <RoundedYView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }

                    draw_bg: {
                        color: #000,
                        border_size: 2.0
                        border_color: #8
                        border_radius: vec2(1.0, 5.0),
                        border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                    }

                    <Label> { text: "<RoundedYView>" }
                }

                <RoundedAllView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }

                    draw_bg: {
                        color: #000,
                        border_size: 2.0
                        border_color: #8
                        border_radius: vec4(1.0, 5.0, 2.0, 3.0),
                        border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                    }

                    <Label> { text: "<RoundedAllView>" }
                }

                <GradientXView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }

                    draw_bg: {
                        color_1: #f00,
                        color_2: #f80,
                        color_dither: 2.0
                    }

                    <Label> { text: "<GradientXView>" }
                }

                <GradientYView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }

                    draw_bg: {
                        color_1: #f00,
                        color_2: #f80,
                        color_dither: 2.0
                    }

                    <Label> { text: "<GradientYView>" }
                }
            }

            <Hr> {}
            <H4> { text: "Alternative Shapes" }
            <UIZooRowH> {
                flow: RightWrap
                <CircleView> {
                    width: Fit, height: Fit, 
                    padding: 15.,
                    align: { x: 0.5, y: 0.5 }

                    draw_bg: {
                        color: #0,
                        border_size: 2.0
                        border_color: #8
                        border_radius: 30.,
                        border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                    }

                    <Label> { text: "<CircleView>" }
                }

                <HexagonView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }

                    draw_bg: {
                        color: #0,
                        border_size: 2.0
                        border_color: #8
                        border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                        border_radius: vec2(0.0, 0.0)
        
                    }

                    <Label> { text: "<HexagonView>" }
                }
            }

            <Hr> {}
            <H4> { text: "Special functions" }
            <UIZooRowH> {
                flow: RightWrap
                <CachedView> {
                    width: Fit, height: Fit, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0.5, y: 0.5 }

                    <View> {
                        width: Fit, height: Fit,
                        show_bg: true, 
                        draw_bg: { color: #0 }

                        <Label> { text: "<CachedView>" }
                    }

                }

                <CachedRoundedView> {
                    width: Fit, height: Fit, 
                    padding: 0.,
                    align: { x: 0.5, y: 0.5 }
                    draw_bg: {
                        border_size: 2.0
                        border_color: #8
                        border_inset: vec4(0., 0., 0., 0.)
                        border_radius: 2.5
                    }

                    <View> {
                        width: Fit, height: Fit,
                        padding: <THEME_MSPACE_2> {},
                        show_bg: true, 
                        draw_bg: { color: #0 }

                        <Label> { text: "<CachedRoundedView>" }
                    }

                }

                <CachedScrollXY> {
                    width: 100, height: 100, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0., y: 0. }

                    <View> {
                        width: 400., height: 400.,
                        flow: Down,
                        show_bg: true, 
                        draw_bg: { color: #0 }

                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                        <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                    }
                }

                <CachedScrollX> {
                    width: 100, height: 100, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0., y: 0. }

                    <View> {
                        width: 400., height: 400.,
                        flow: Down,
                        show_bg: true, 
                        draw_bg: { color: #0 }

                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                        <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                    }
                }

                <CachedScrollY> {
                    width: 100, height: 100, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0., y: 0. }

                    <View> {
                        width: 400., height: 400.,
                        flow: Down,
                        show_bg: true, 
                        draw_bg: { color: #0 }

                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                        <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                    }
                }

                <ScrollXYView> {
                    width: 100, height: 100, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0., y: 0. }
                    show_bg: true,
                    draw_bg: {
                        color: #8
                    }

                    <View> {
                        width: 400., height: 400.,
                        flow: Down,
                        show_bg: true, 
                        draw_bg: { color: #0 }

                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                        <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                    }
                }

                <ScrollXView> {
                    width: 100, height: 100, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0., y: 0. }
                    show_bg: true,
                    draw_bg: {
                        color: #8
                    }

                    <View> {
                        width: 400., height: 400.,
                        flow: Down,
                        show_bg: true, 
                        draw_bg: { color: #0 }

                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                        <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                    }
                }

                <ScrollYView> {
                    width: 100, height: 100, 
                    padding: <THEME_MSPACE_2> {},
                    align: { x: 0., y: 0. }
                    show_bg: true,
                    draw_bg: {
                        color: #8
                    }

                    <View> {
                        width: 400., height: 400.,
                        flow: Down,
                        show_bg: true, 
                        draw_bg: { color: #0 }

                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                        <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                    }
                }

            }
        }
    }
}