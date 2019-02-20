//use crate::math::*;
//use crate::shader::*;
use crate::cx::*;
use crate::cxdrawing::*;
use crate::rect::*;
use crate::text::*;

pub struct Button{
    pub dn:DrawNode,
    pub time:f32,
    pub bg: Rect,
    pub text: Text,
    pub label:String,
    pub did_click: bool
}

impl Style for Button{
    fn style(cx:&mut Cx)->Self{
        Self{
            time:0.0,
            dn:DrawNode{..Default::default()},
            label:"OK".to_string(),
            did_click:false,
            bg:Rect{..Style::style(cx)},
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

    pub fn draw_with_label(&mut self, cx:&mut Cx, _label: &str){
        self.dn.begin(cx);
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
        }*/
        self.text.draw_text(cx, "HELLO WORLD");

        self.dn.end(cx);
    }
}
