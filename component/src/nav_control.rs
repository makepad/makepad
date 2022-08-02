use crate::makepad_platform::*;


live_register!{
    import makepad_platform::shader::std::*;
    
    DrawFocusRect: {{DrawFocusRect}} {
        fn pixel(self) -> vec4 {
            return #000f
        }
    }
    
    NavControl: {{NavControl}} {
        label: {
            text_style:{
                font_size: 6
            },
            color: #a
        }
        view:{
            unclipped: true
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawFocusRect {
    draw_super: DrawQuad,
}


#[derive(Live, LiveHook)]
pub struct NavControl {
    view: View,
    focus: DrawFocusRect,
    label: DrawText,
    #[rust] _recent_focus: Area,
}

impl NavControl {
    pub fn handle_event(&mut self, _cx: &mut Cx, event: &Event) {
       match event{
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::Tab => {
                    if ke.modifiers.shift{
                        
                    }
                    else{
                        
                    }
                }
                _=>()
            },
            _=>()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if !self.view.begin(cx, Walk::default(), Layout::default()).is_redrawing(){
            return
        }
        
        self.view.end(cx);
        
    }
}


