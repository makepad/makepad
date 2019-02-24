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
    pub anim_states:Vec<AnimState>,
    pub label:String
}

impl Style for Button{
    fn style(cx:&mut Cx)->Self{
        Self{
            view:View::new(),
            area:Area::zero(),
            layout:Layout{
                w:Fixed(50.0),
                h:Fixed(50.0),
                ..Layout::new()
            },
            bg_layout:Layout{
                w:Computed,
                h:Computed,
                margin:Margin::i32(1),
                ..Layout::filled_padded(10.0)
            },
            label:"OK".to_string(),
            anim_states:vec![
                AnimState::new("over", 0.1, AnimStart::Interrupt,vec![
                    AnimKey::new(1.0,vec![
                        AnimValue::color("bg", "color", "red")
                    ])
                ]) 
            ],
            bg:Quad{
                color:color("gray"),
                ..Style::style(cx)
            },
            text:Text{..Style::style(cx)}
        }
    }
}

pub enum ButtonEvent{
    None,
    Clicked
}

impl Button{
    pub fn handle(&mut self, cx:&mut Cx, event:&Event)->ButtonEvent{
        match event.hits(&self.area, cx){
            Event::Animate=>{
                cx.compute_animation("bg", &self.area, &self.anim_states, &self.area);
            },
            Event::FingerDown(_fe)=>{
                return ButtonEvent::Clicked
            },
            Event::FingerMove(_fe)=>{
                // lets start an animation.
                cx.start_animation("over", &self.area, &self.anim_states);
                //cx.start_animation(&self.area, ANIM_OVER);
                //println!("MOVE OVER");
            },
            _=>()
        }
        ButtonEvent::None
   }

    pub fn draw_with_label(&mut self, cx:&mut Cx, label: &str){
        // this marks a tree node.
        if self.view.begin(cx, &self.layout){return};

        // however our turtle stack needs to remain independent
        self.bg.begin(cx, &self.bg_layout);

        self.text.draw_text(cx, Computed, Computed, label);
        
        self.area = self.bg.end(cx);

        self.view.end(cx);
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
 