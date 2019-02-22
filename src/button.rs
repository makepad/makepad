//use crate::math::*;
//use crate::shader::*;
use shui::*;

pub struct Button{
    pub view:View,
    pub layout:Layout,
    pub time:f32,
    pub bg: Quad,
    pub bg_layout:Layout,
    pub text: Text,
    pub label:String,
    pub did_click: bool
}

impl Style for Button{
    fn style(cx:&mut Cx)->Self{
        Self{
            time:0.0,
            view:View::new(),
            layout:Layout{
                w:Computed,
                h:Computed,
                ..Layout::new()
            },
            bg_layout:Layout{
                ..Layout::paddedf(10.0,10.0,10.0,10.0)
            },
            label:"OK".to_string(),
            did_click:false,
            bg:Quad{
                color:color("gray"),
                ..Style::style(cx)
            },
            text:Text{..Style::style(cx)}
        }
    }
}

impl Button{
    pub fn handle(&mut self, _cx:&mut Cx, _ev:&Ev){
        // handle event and figure out if we got clicked
    }

    pub fn handle_click(&mut self, cx:&mut Cx, ev:&Ev)->bool{
        self.handle(cx, ev);
        self.did_click()
    }

    pub fn did_click(&self)->bool{
        self.did_click
    }

    pub fn draw_with_label(&mut self, cx:&mut Cx, label: &str){
        // this marks a tree node.
        self.view.begin(cx, &self.layout);

        // however our turtle stack needs to remain independent
        self.bg.begin(cx, &self.bg_layout);

        //self.bg.draw_sized(cx, Fixed(40.0),Fixed(140.0),Margin::zero());

        self.text.draw_text(cx, Computed, Computed, label);
        
        self.bg.end(cx);

        self.view.end(cx);
    }
}



        /*
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
 