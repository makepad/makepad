use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    }
};

live_design!{
    link widgets;
    use link::theme::*;
    
    pub LabelBase = {{Label}} {}
    pub Label = <LabelBase> {
        width: Fit, height: Fit,
        draw_text: {
            color: (THEME_COLOR_TEXT),
            text_style: <THEME_FONT_REGULAR> {},
            wrap: Word
        }
    }

    pub LabelGradientX = <Label> {
        width: Fit, height: Fit,
        draw_text: {
            uniform color_1: #f00,
            uniform color_2: #ff0

            fn get_color(self) ->vec4{
                return mix(self.color_1, self.color_2, self.pos.y)
            }
        }
    }
    
    pub LabelGradientY = <Label> {
        width: Fit, height: Fit,
        draw_text: {
            uniform color_1: #f00,
            uniform color_2: #ff0

            fn get_color(self) ->vec4{
                return mix(self.color_1, self.color_2, self.pos.x)
            }
        }
    }
    
    pub H1 = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_1 * 0.25)}
        draw_text: {
            wrap: Word
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_1)
            }
            color: (THEME_COLOR_TEXT_HL)
        }
        text: "Headline H1"
    }
    
    pub H1italic = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_1 * 0.25)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_1)
            }
            color: (THEME_COLOR_TEXT_HL)
        }
        text: "Headline H1"
    }
    
    pub H2 = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_2 * 0.25)}
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_2)
            }
            color: (THEME_COLOR_TEXT_HL)
        }
        text: "Headline H2"
    }
    
    pub H2italic = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_2 * 0.25)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_2)
            }
            color: (THEME_COLOR_TEXT_HL)
        }
        text: "Headline H2"
    }
    
    pub H3 = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_3 * 0.25)}
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_3)
            }
            color: (THEME_COLOR_TEXT_HL)
        }
        text: "Headline H3"
    }
    
    pub H3italic = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_3 * 0.25)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_3)
            }
            color: (THEME_COLOR_TEXT_HL)
        }
        text: "Headline H3"
    }
    
    pub H4 = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_4 * 0.25)}
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_4)
            }
            color: (THEME_COLOR_TEXT_HL)
        }
        text: "Headline H4"
    }
    
    pub H4italic = <Label> {
        width: Fill,
        margin: {top: (THEME_FONT_SIZE_4 * 0.25)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_4)
            }
            color: (THEME_COLOR_TEXT_HL)
        }
        text: "Headline H4"
    }
    
    pub P = <Label> {
        width: Fill,
        margin: 0.,
        padding: 0.,
        margin: {top: (THEME_SPACE_2 * 0.25), bottom: (THEME_FONT_SIZE_P * 0.5)}
        draw_text: {
            text_style: <THEME_FONT_REGULAR> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT)
        }
        text: "Paragraph"
    }
    
    pub Pbold = <Label> {
        width: Fill,
        margin: {top: (THEME_SPACE_2 * 0.25), bottom: (THEME_FONT_SIZE_P * 0.5)}
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT)
        }
        text: "Paragraph"
    }
    
    pub Pitalic = <Label> {
        width: Fill,
        margin: {top: (THEME_SPACE_2 * 0.25), bottom: (THEME_FONT_SIZE_P * 0.5)}
        draw_text: {
            text_style: <THEME_FONT_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT)
        }
        text: "Paragraph"
    }
    
    pub Pbolditalic = <Label> {
        width: Fill,
        margin: {top: (THEME_SPACE_2 * 0.25), bottom: (THEME_FONT_SIZE_P * 0.5)}
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT)
        }
        text: "Paragraph"
    }

    
}

#[derive(Clone, Debug, DefaultNone)]
pub enum LabelAction {
    HoverIn(Rect),
    HoverOut,
    None
}


#[derive(Live, LiveHook, Widget)]
pub struct Label {
    #[redraw] #[live] draw_text: DrawText,
    
    #[walk] walk: Walk,
    #[live] align: Align,
    #[live(Flow::RightWrap)] flow: Flow,
    #[live] padding: Padding,
    
    #[rust] area: Area,
    //margin: Margin,
    #[live] text: ArcStringMut,

    // Indicates if this label responds to hover events
    // It is not turned on by default because it will consume finger events
    // and prevent other widgets from receiving them, if it is not considered with care
    // The primary use case for this kind of emitted actions is for tooltips displaying
    #[live(false)] hover_actions_enabled: bool
} 

impl Widget for Label {

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk:Walk)->DrawStep{
        let walk = walk.with_add_padding(self.padding);
        cx.begin_turtle(walk, Layout{
            flow: self.flow,
            ..Default::default()
        });
        // here we need to check if the text is empty, if so we need to set it to a space
        // or the text draw will not work(seems like lazy drawtext bug)
        let _ = self.text.as_ref().is_empty().then(|| {
            let _ = self.set_text(cx, " ");
        });
        self.draw_text.draw_walk(cx, walk, self.align, self.text.as_ref());
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
    
    fn text(&self)->String{
        self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, cx:&mut Cx, v:&str){
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
                
        match event.hit_designer(cx, self.area){
            HitDesigner::DesignerPick(_e)=>{
                cx.widget_action(uid, &scope.path, WidgetDesignAction::PickedBody)
            }
            _=>()
        }
        
        if self.hover_actions_enabled {
            
            match event.hits_with_capture_overload(cx, self.area, true) {
                Hit::FingerHoverIn(fh) => {
                    cx.widget_action(uid, &scope.path, LabelAction::HoverIn(fh.rect));
                }
                Hit::FingerHoverOut(_) => {
                    cx.widget_action(uid, &scope.path, LabelAction::HoverOut);
                },
                _ => ()
            }
        }
    }
}

impl LabelRef {
    pub fn hover_in(&self, actions:&Actions)->Option<Rect>{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                LabelAction::HoverIn(rect) => Some(rect),
                _=> None
            }
        } else {
            None
        }
    }

    pub fn hover_out(&self, actions:&Actions)->bool{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                LabelAction::HoverOut => true,
                _=> false
            }
        } else {
            false
        }
    }
    
    pub fn set_text_with<F:FnOnce(&mut String)>(&self, f:F) {
        if let Some(mut inner) = self.borrow_mut(){
            f(inner.text.as_mut())
        }
    }
}
