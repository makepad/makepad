use render::*;

#[derive(Clone, Element)]
pub struct Editor{
    pub bg_layout:Layout,
    pub bg: Quad,
    pub text: Text,
    pub _hit_state:HitState,
    pub _bg_area:Area,
}

impl Style for Editor{
    fn style(cx:&mut Cx)->Self{
        let tab = Self{
            bg:Quad{
                ..Style::style(cx)
            },
            bg_layout:Layout{
                align:Align::center(),
                width:Bounds::Compute,
                height:Bounds::Compute,
                margin:Margin::all(0.),
                padding:Padding{l:16.0,t:12.0,r:16.0,b:12.0},
                ..Default::default()
            },
            text:Text{..Style::style(cx)},
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

    pub fn handle_editor(&mut self, cx:&mut Cx, event:&mut Event)->EditorEvent{
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

    pub fn draw_editor(&mut self, cx:&mut Cx){
        // pull the bg color from our animation system, uses 'default' value otherwise
       // self.bg.color = self.animator.last_vec4("bg.color");
        self._bg_area = self.bg.begin_quad(cx, &self.bg_layout);
        // push the 2 vars we added to bg shader
        //self.text.color = self.animator.last_vec4("text.color");
        
        self.text.draw_text(cx, "HELLO WORLD");

        self.bg.end_quad(cx);

        //self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
    }

}