use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoIconSet = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<IconSet>"}
        }
        demos = {
            flow: RightWrap,
            spacing: 30.
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Home
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Circle-User
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Image
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // File
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Camera
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Calendar
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Cloud
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Truck
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Thumbs Up
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Face Smile
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Headphones
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Bell
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // User
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Comment
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Envelope
            <IconSet> {
                text: ""
                draw_text: { color: #0ff }
             } // Car
        }
    }
}