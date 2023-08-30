use {
    crate::{
        makepad_derive_widget::*,
        widget::*,
        makepad_draw::*,
        button::{Button}
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import crate::theme::*;
    
    const THICKNESS = 0.8
    LinkLabel = {{LinkLabel}} {
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let offset_y = 1.0
                sdf.move_to(0., self.rect_size.y - offset_y);
                sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);
                return sdf.stroke(mix(
                    COLOR_TEXT_DEFAULT,
                    COLOR_TEXT_META,
                    self.pressed
                ), mix(0.0, THICKNESS, self.hover));
            }
        }
        draw_label: {
            text_style: <FONT_META> {}
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        COLOR_TEXT_META,
                        COLOR_TEXT_DEFAULT,
                        self.hover
                    ),
                    COLOR_TEXT_META,
                    self.pressed
                )
            }
        }
        
        
            width: Fit,
            height: Fit,
            margin: {left: 5.0, top: 0.0, right: 0.0
        }
        
        
            padding: {left: 1.0, top: 1.0, right: 1.0, bottom: 1.0
        }
    }
}

#[derive(Live)]
pub struct LinkLabel {
    #[deref] button: Button
}

impl LiveHook for LinkLabel{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx,LinkLabel)
    }
}

impl Widget for LinkLabel {
    fn redraw(&mut self, cx:&mut Cx){
        self.button.redraw(cx)
    }
    
    fn walk(&self)->Walk{
        self.button.walk()
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk:Walk)->WidgetDraw{
        self.button.draw_walk_widget(cx, walk)
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct LinkLabelRef(WidgetRef); 

impl LinkLabelRef {
    pub fn set_label(&self, text:&str){
        if let Some(mut inner) = self.borrow_mut(){
            let s = inner.button.label.as_mut();
            s.clear();
            s.push_str(text);
        }
    }
}
