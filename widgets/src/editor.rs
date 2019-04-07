use render::*;
use crate::textbuffer::*;
use crate::scrollbar::*;

#[derive(Clone)]
pub struct Editor{
    pub path:String,
    pub view:View<ScrollBar>,
    pub bg_layout:Layout,
    pub bg: Quad,
    pub text: Text,
    pub _hit_state:HitState,
    pub _bg_area:Area,
}

impl ElementLife for Editor{
    fn construct(&mut self, _cx:&mut Cx){}
    fn destruct(&mut self, _cx:&mut Cx){}
}

impl Style for Editor{
    fn style(cx:&mut Cx)->Self{
        let tab = Self{
            path:"".to_string(),
            view:View{
                scroll_h:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                scroll_v:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                ..Style::style(cx)
            },
            bg:Quad{
                color:color("#2"),
                ..Style::style(cx)
            },
            bg_layout:Layout{
                width:Bounds::Compute,
                height:Bounds::Compute,
                margin:Margin::all(0.),
                padding:Padding{l:4.0,t:4.0,r:4.0,b:4.0},
                ..Default::default()
            },
            text:Text{
                font_id:cx.load_font(&cx.font("mono_font")),
                font_size:12.0,
                wrapping:Wrapping::Line,
                ..Style::style(cx)
            },
            _hit_state:HitState{..Default::default()},
            _bg_area:Area::Empty
        };
        //tab.animator.default = tab.anim_default(cx);
        tab
    }
}

#[derive(Clone, PartialEq)]
pub enum EditorEvent{
    None,
    Change
}

impl Editor{

    pub fn handle_editor(&mut self, cx:&mut Cx, event:&mut Event, text_buffer:&mut TextBuffer)->EditorEvent{
        self.view.handle_scroll_bars(cx, event);
        match event.hits(cx, self._bg_area, &mut self._hit_state){
            Event::Animate(_ae)=>{
            },
            Event::FingerDown(_fe)=>{
            },
            Event::FingerHover(_fe)=>{
            },
            Event::FingerUp(_fe)=>{
            },
            Event::FingerMove(_fe)=>{
            },
            _=>()
        };
        EditorEvent::None
   }

    pub fn draw_editor(&mut self, cx:&mut Cx, text_buffer:&mut TextBuffer){
        // pull the bg color from our animation system, uses 'default' value otherwise
       // self.bg.color = self.animator.last_vec4("bg.color");
        // push the 2 vars we added to bg shader
        //self.text.color = self.animator.last_vec4("text.color");
        self.view.begin_view(cx, &Layout{..Default::default()});
        if text_buffer.load_id != 0{
            self._bg_area = self.bg.begin_quad(cx, &Layout{
                align:Align::center(),
                ..self.bg_layout.clone()
            });
            self.text.draw_text(cx, "Loading ...");
        }
        else{
            self._bg_area = self.bg.begin_quad(cx, &self.bg_layout);
            self.text.draw_text(cx, &text_buffer.text);
        }

        self.bg.end_quad(cx);
        self.view.end_view(cx);
         //self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
    }

}