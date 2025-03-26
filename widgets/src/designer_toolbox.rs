use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    designer_data::*,
    view::View,
    widget::*,
};

live_design!{
    use link::theme::*;
    use makepad_draw::shader::std::*;
    use link::widgets::*;
    use crate::designer_theme::*;
        
    pub DesignerToolboxBase = {{DesignerToolbox}}{
    }
    
    Vr = <View> {
        width: Fit, height: Fill,
        flow: Right,
        spacing: 0.,
        margin: <THEME_MSPACE_V_2> {}
        <View> {
            width: (THEME_BEVELING * 2.0), height: Fill
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_BEVEL_SHADOW) }
        }
        <View> {
            width: (THEME_BEVELING), height: Fill,
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_BEVEL_LIGHT) }
        }
    }
    
    pub DesignerToolbox = <DesignerToolboxBase>{
        width: Fill,
        height: Fill
        show_bg: false
        
        <DockToolbar> {
            content = {
                align: { x: 0., y: 0.5 }
                spacing: (THEME_SPACE_3 * 1.5)
                <ButtonFlat> {
                    width: 32.
                    text: ""
                    margin: { right: -10. }
                    icon_walk: { width: 11. }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_typography.svg"),
                    }
                }
                <Vr> {}
                <View> {
                    width: Fit,
                    flow: Right,
                    spacing: (THEME_SPACE_1)
                    <Pbold> {
                        width: Fit,
                        text: "Font",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                    <P> {
                        width: Fit,
                        text: "Noto Sans",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                }
                <View> {
                    width: Fit,
                    spacing: (THEME_SPACE_1)
                    flow: Right,
                    <Pbold> {
                        width: Fit,
                        text: "Weight",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                    <P> {
                        width: Fit,
                        text: "bold",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                }
                <View> {
                    width: Fit,
                    spacing: (THEME_SPACE_1)
                    flow: Right,
                    <Pbold> {
                        width: Fit,
                        text: "Size",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                    <P> {
                        width: Fit,
                        text: "11 pt",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                } 
                <View> {
                    width: Fit,
                    spacing: (THEME_SPACE_1)
                    flow: Right,
                    <Pbold> {
                        width: Fit,
                        text: "Line height",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                    <P> {
                        width: Fit,
                        text: "1.2",
                        margin: 0.,
                        padding: <THEME_MSPACE_V_1> {}
                    }
                } 
                <Vr> {}
                <View> {
                    width: Fit,
                    flow: Right,
                    spacing: 0,
                    <ButtonFlat> {
                        width: 25.
                        text: ""
                        icon_walk: { width: 11. }
                        draw_icon: {
                            svg_file: dep("crate://self/resources/icons/icon_text_align_left.svg"),
                        }
                    }
                    <ButtonFlat> {
                        width: 25.
                        text: ""
                        icon_walk: { width: 11. }
                        draw_icon: {
                            color: (THEME_COLOR_D_3),
                            svg_file: dep("crate://self/resources/icons/icon_text_align_justify.svg"),
                        }
                    }
                    <ButtonFlat> {
                        width: 25.
                        text: ""
                        icon_walk: { width: 11. }
                        draw_icon: {
                            color: (THEME_COLOR_D_3),
                            svg_file: dep("crate://self/resources/icons/icon_text_align_right.svg"),
                        }
                    }
                }
                <Vr> {}
                <P> { width: Fit, text: "Stroke" }
                <RoundedView> {
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_5),
                        border_radius: 5.0
                    }
                }
                <P> { width: Fit, text: "Fill" }
                <RoundedView> {
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_2),
                        border_radius: 5.0
                    }
                }
                <Filler> {}
                <Vr> {}
                <P> { width: Fit, text: "Canvas" }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (THEME_COLOR_D_3),
                        border_radius: 5.0
                    }
                }
            }
        }
        
        <RoundedShadowView>{
            abs_pos: vec2(25., 65.)
            padding: 0.
            width: 36., height: Fit,
            spacing: 0.,
            align: { x: 0.5, y: 0.0 }
            flow: Down,
            clip_x: false, clip_y: false,
            
            draw_bg: {
                border_size: 1.0
                border_color: (THEME_COLOR_BEVEL_LIGHT)
                shadow_color: (THEME_COLOR_D_4)
                shadow_radius: 10.0,
                shadow_offset: vec2(0.0, 5.0)
                border_radius: 2.5
                color: (THEME_COLOR_FG_APP),
            }
            
            <View> {
                width: Fit, height: 36.,
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    icon_walk: { width: 9. }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_select.svg"),
                    }
                    text: ""
                }
            }
            <Hr> { margin: 0. }
            <View> {
                width: Fit, height: 36.,
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    icon_walk: { width: 14.5 }
                    align: { x: 0.5, y: 0.5 }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_draw.svg"),
                    }
                    text: ""
                }
            }
            <Hr> { margin: 0. }
            <View> {
                width: Fit, height: 36.,
                
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    icon_walk: { width: 12. }
                    align: { x: 0.5, y: 0.5 }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_text.svg"),
                    }
                    text: ""
                }
            }
            <Hr> { margin: 0. }
            <View> {
                width: Fit, height: 36.,
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    icon_walk: { width: 13.5 }
                    align: { x: 0.5, y: 0.5 }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_layout.svg"),
                    }
                    text: ""
                }
            }
            <Hr> { margin: 0. }
            <View> {
                width: Fit, height: 36.,
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    flow: Down,
                    icon_walk: { width: 15.5 }
                    align: { x: 0.5, y: 0.5 }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_widget.svg"),
                    }
                    text: ""
                }
            }
            <Hr> { margin: 0. }
            <View> {
                width: Fit, height: 36.,
                align: { x: 0.5, y: 0.5}
                <ButtonFlatter> {
                    flow: Down,
                    icon_walk: { width: 15.5 }
                    align: { x: 0.5, y: 0.5 }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/icons/icon_image.svg"),
                    }
                    text: ""
                }
            }
        }
        /*
        <RoundedShadowView>{
            width: 250., height: 350.,
            abs_pos: vec2(25., 325.)
            padding: <THEME_MSPACE_2> {}
            spacing: (THEME_SPACE_1)
            align: { x: 0.5, y: 0.0 }
            flow: Down,
            clip_x: false, clip_y: false,
            
            draw_bg: {
                border_size: 1.0
                border_color: (THEME_COLOR_BEVEL_LIGHT)
                shadow_color: (THEME_COLOR_D_4)
                shadow_radius: 10.0,
                shadow_offset: vec2(0.0, 5.0)
                border_radius: 2.5
                color: (THEME_COLOR_FG_APP),
            }
                        
            <View> {
                flow: Right,
                width: Fill, height: Fit, 
                align: { x: 0.0, y: 0.5 }
                <RoundedView> {
                    margin: { left: (THEME_SPACE_2), right: (THEME_SPACE_1), top: 5. }
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (#f00),
                        border_radius: 5.0
                    }
                }
                <Pbold> { width: Fit, margin: {left: 3.}, text: "Canvas" }
            }
            <Hr> { margin: <THEME_MSPACE_1> {} }
            <ColorPicker>{}
            <View> {
                width: Fill, height: Fit, 
                spacing: (THEME_SPACE_2)
                align: { x: 0.5, y: 0.5 }
                flow: Right,
                <Pbold> { width: Fit, text: "RGBA" }
                <P> { width: Fit, text: "0 / 255 / 0 / 255" }
                <P> { width: Fit, text: "#83741AFF" }
            }
            <View> {
                align: { x: 0.5, y: 0.5 }
                width: Fill, height: Fit, 
                flow: Right,
                spacing: (THEME_SPACE_1),
                margin: { bottom: 10. }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_1),
                        border_radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_2),
                        border_radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_3),
                        border_radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_4),
                        border_radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_5),
                        border_radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_6),
                        border_radius: 5.0
                    }
                }
                <RoundedView> {
                    margin: { right: (THEME_SPACE_1)}
                    width: 15., height: 15.,
                    draw_bg: {
                        color: (STUDIO_PALETTE_7),
                        border_radius: 5.0
                    }
                }
            }
        }*/
    }
}

#[derive(Live, Widget, LiveHook)]
pub struct DesignerToolbox {
    #[deref] view: View
}

impl Widget for DesignerToolbox {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.view.handle_event(cx, event, scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, _walk: Walk) -> DrawStep {
        let _data = scope.data.get::<DesignerData>().unwrap();
        while let Some(_next) = self.view.draw(cx, &mut Scope::empty()).step() {
        }
        DrawStep::done()
    }
}