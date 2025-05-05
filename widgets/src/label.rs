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
        padding: <THEME_MSPACE_1> {}
        draw_text: {
            color: (THEME_COLOR_LABEL_OUTER),
            text_style: <THEME_FONT_REGULAR> {
                line_spacing: (THEME_FONT_WDGT_LINE_SPACING),
            },
            wrap: Word
        }
    }

    pub Labelbold = <Label> {
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                font_size: (THEME_FONT_SIZE_P)
            }
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
    
    pub LabelGradientY = <LabelGradientX> {
        draw_text: {
            fn get_color(self) ->vec4{
                return mix(self.color_1, self.color_2, self.pos.x)
            }
        }
    }
    
    pub TextBox = <Label> {
        width: Fill, height: Fit,
        padding: { left: 0., right: 0., top: (THEME_SPACE_1), bottom: 0. }
        draw_text: {
            text_style: <THEME_FONT_REGULAR> {
                line_spacing: (THEME_FONT_LONGFORM_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_TEXT)
        }
        text: "TextBox"
    }

    pub H1 = <Label> {
        width: Fill,
        padding: 0.
        draw_text: {
            wrap: Word
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_HL_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_1)
            }
            color: (THEME_COLOR_TEXT_HL)
        }
        text: "H1"
    }
    
    pub H1italic = <H1> {
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_HL_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_1)
            }
        }
        text: "H1 italic"
    }
    
    pub H2 = <H1> {
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_HL_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_2)
            }
        }
        text: "H2"
    }
    
    pub H2italic = <H1> {
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_HL_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_2)
            }
        }
        text: "H2 italic"
    }
    
    pub H3 = <H1> {
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_HL_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_3)
            }
        }
        text: "H3"
    }
    
    pub H3italic = <H1> {
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_HL_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_3)
            }
        }
        text: "H3 italic"
    }
    
    pub H4 = <H1> {
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                line_spacing: (THEME_FONT_HL_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_4)
            }
        }
        text: "H4"
    }
    
    pub H4italic = <H1> {
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                line_spacing: (THEME_FONT_HL_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_4)
            }
        }
        text: "H4 italic"
    }
 
    pub P = <TextBox> {
        text: "Paragraph"
    }
    
    pub Pbold = <TextBox> {
        draw_text: {
            text_style: <THEME_FONT_BOLD> {
                font_size: (THEME_FONT_SIZE_P)
            }
        }
        text: "Paragraph"
    }
    
    pub Pitalic = <TextBox> {
        draw_text: {
            text_style: <THEME_FONT_ITALIC> {
                font_size: (THEME_FONT_SIZE_P)
            }
        }
        text: "Paragraph"
    }
    
    pub Pbolditalic = <TextBox> {
        draw_text: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                font_size: (THEME_FONT_SIZE_P)
            }
        }
        text: "Paragraph"
    }

    pub IconSet = <Label> {
        width: Fit,
        draw_text: {
            text_style: <THEME_FONT_ICONS> {
                line_spacing: (THEME_FONT_WDGT_LINE_SPACING),
                font_size: 100.
            }
            color: (THEME_COLOR_TEXT)
        }
        text: "Car"
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
