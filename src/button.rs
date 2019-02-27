//use crate::math::*;
//use crate::shader::*;
use shui::*;

#[derive(Clone)]
pub struct Button{
    pub view:View,
    pub area:Area,
    pub layout:Layout,
    pub bg: Quad,
    pub bg_layout:Layout,
    pub text: Text,
    pub states:AnimStates<ButtonState>,
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
            area:Area::zero(),
            layout:Layout{
                w:Computed,
                h:Computed,
                //w:Fixed(50.0),
                //h:Fixed(50.0),
                ..Layout::new()
            },
            bg_layout:Layout{
                //w:Fixed(50.0),
                align:Align::center(),
                w:Computed,
                h:Computed,
                margin:Margin::i32(1),
                ..Layout::padded(5.0)
            },
            label:"OK".to_string(),
            states:AnimStates::new(
                ButtonState::Default,
                vec![
                    AnimState::new(
                        ButtonState::Over,
                        AnimMode::Single{speed:1.0, len:1.0, cut:true}, 
                        vec![
                            AnimTrack::vec4("bg", "color", vec![ (1.0,color("red")) ])
                        ]
                    ) 
                ]
            ),
            bg:Quad{
                //shader_id:cx.shaders.add(sh),
                color:color("gray"),
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
        match event.hits(&self.area, cx){
            Event::Animate(ae)=>{
                self.states.animate(cx, "bg", &self.area, &self.area, ae);
            },
            Event::FingerDown(_fe)=>{
                self.event = ButtonEvent::Clicked;
                self.states.change(cx, ButtonState::Over, &self.area);
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
        // this marks a tree node.
        self.label = label.to_string();
        //self.view.begin(cx, &self.layout);//{return};

        // however our turtle stack needs to remain independent
        self.bg.begin(cx, &self.bg_layout);

        self.text.draw_text(cx, Computed, Computed, label);
        
        self.area = self.bg.end(cx);

       // self.view.end(cx);
    }
}



/*
//self.bg.draw_sized(cx, Fixed(40.0),Fixed(140.0),Margin::zero());
self.time = self.time + 0.01;
for i in 0..200000{
    self.bg.color.x = 0.5+0.5*f32::sin(i as f32 / 10.0+self.time);
    self.bg.color.y = 0.5+0.5*f32::cos(i as f32 / 10.0+self.time);
    self.bg.color.z = 0.5+0.5*f32::cos(i as f32 / 10.0+1.5+self.time);
    self.bg.draw_at(cx, 
        f32::sin(i as f32 / 5.0+self.time), 
        f32::cos(i as f32 / 3.2+self.time),
        0.01, 0.01);
self.text.draw_text(cx, "HELLO WORLD");
}*/
 