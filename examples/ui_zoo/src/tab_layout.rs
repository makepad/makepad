use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    BoxA = <RoundedView> {
        height: Fill, width: Fill,
        padding: 10., margin: 0.,
        show_bg: true,
        draw_bg: { color: #0F02 }
        flow: Down,
        align: { x: 0.5, y: 0.5}
        <P> { width: Fit, text: "width: Fill\nheight: Fill\nflow: Down"}
    }

    BoxB = <RoundedView> {
        height: Fit, width: Fit,
        padding: 10., margin: 0.,
        show_bg: true,
        draw_bg: { color: #F002 }
        flow: Down,
        align: { x: 0.5, y: 0.5}
        <P> { width: Fit, text: "width: Fit\nheight: Fit\nflow: Down"}
    }

    BoxC = <RoundedView> {
        height: 200, width: Fill,
        padding: 10., margin: 0.,
        spacing: 10.
        show_bg: true,
        draw_bg: { color: #0002 }
        flow: Right,
        align: { x: 0.5, y: 0.5}
        <P> { width: Fit, text: "width: Fill\nheight: 200\nflow: Right,\n spacing: 10."}
    }



    pub DemoLayout = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/layout.md") } 
        }
        demos = {
            <BoxA> {}
            <BoxB> {}
            <BoxB> {}
            <BoxC> {
                <BoxB> {}
                <BoxB> {}
                <BoxA> {}
                <Filler> {}
                <BoxB> {}
            }
        }
    }
}