use shui::*;

#[derive(Clone, Element)]
pub struct Button{
    pub view:View,
    pub bg_area:Area,
    pub layout:Layout,
    pub bg: Quad,
    pub bg_layout:Layout,
    pub text: Text,
    pub anim:Animation<ButtonState>,
    pub label:String,
    pub event:ButtonEvent
}

#[derive(Clone, PartialEq)]
pub enum ButtonState{
    Default,
    Over
}

impl Style for Button{
    fn style(cx:&mut Cx)->Self{
/*
        let mut sh = Shader::def(); 
        Quad::def_shader(&mut sh);
        sh.add_ast(shader_ast!(||{
            fn pixel()->vec4{
                df_viewport(pos*vec2(w,h));   
                df_circle(w*0.5,h*0.5,0.5*min(w,h));
                return df_fill(color); 
            }
        }));*/

        Self{
            view:View::new(),
            bg_area:Area::Empty,
            layout:Layout{
                w:Computed,
                h:Computed,
                ..Layout::new()
            },
            bg_layout:Layout{
                align:Align::center(),
                w:Computed,
                h:Computed,
                margin:Margin::i32(1),
                ..Layout::padded(5.0)
            },
            label:"OK".to_string(),
            anim:Animation::new(
                ButtonState::Default,
                vec![
                    AnimState::new(
                        ButtonState::Default,
                        AnimMode::Single{speed:1.0, len:1.0, cut:true}, 
                        vec![
                            AnimTrack::vec4("bg.color", vec![ (1.0,color("gray")) ])
                        ]
                    ),
                    AnimState::new(
                        ButtonState::Over,
                        AnimMode::Single{speed:1.0, len:1.0, cut:true}, 
                        vec![
                            AnimTrack::vec4("bg.color", vec![ (1.0,color("red")) ])
                        ]
                    ) 
                ]
            ),
            bg:Quad{
                ..Style::style(cx)
            },
            text:Text{..Style::style(cx)},
            event:ButtonEvent::None
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum ButtonEvent{
    None,
    Clicked
}

impl Button{
    pub fn handle(&mut self, cx:&mut Cx, event:&Event)->ButtonEvent{
        match event.hits(&self.bg_area, cx){
            Event::Animate(ae)=>{
                let color = self.anim.calc_vec4(cx, "bg.color", ae.time, self.bg_area.read_vec4(cx, "color"));
                self.bg_area.write_vec4(cx, "color", color);
            },
            Event::FingerDown(_fe)=>{
                self.event = ButtonEvent::Clicked;
                self.anim.change_state(cx, ButtonState::Over);
            },
            Event::FingerMove(_fe)=>{
            },
            _=>{
                 self.event = ButtonEvent::None
            }
        };
        self.event.clone()
   }

    pub fn draw_with_label(&mut self, cx:&mut Cx, label: &str){
        // pull the bg color from our animation system, uses 'default' value otherwise
        self.bg.color = self.anim.last_vec4("bg.color");

        self.bg.begin(cx, &self.bg_layout);

        self.text.draw_text(cx, Computed, Computed, label);
        
        self.bg_area = self.bg.end(cx);

        self.anim.set_area(cx, &self.bg_area); // if our area changed, update animation
    }
}
