
use crate::{
  //  makepad_derive_widget::*,
    makepad_draw::*,
   // widget_match_event::*,
   // outline_tree::*,
   makepad_widgets::makepad_derive_widget::*,
   makepad_widgets::widget::*, 
};


live_design!{
    pub DiffuseThing = {{DiffuseThing}} 
    {
        height: 100,
        width: 100,
        draw_bg:{
            width: Fill,
            height: Fill,
            texture image: texture2d,
            fn samp(self, pos:vec2) -> vec2{
                let asamp = sample2d(self.image, pos);
                
                let A: float = asamp.x ;
                let B: float = asamp.y ;

                return vec2(A,B);
            },
            fn  pal(self,   t: float,  a:vec3,  b:vec3, c:vec3, d:vec3 ) -> vec3            {
                return a + b*cos( 6.28318*(c*t+d) );
            },
            fn pixel(self) -> vec4{

                let S = self.samp(self.pos);
                let C = self.pal( S.y, vec3(0.5,0.5,0.5),vec3(0.5,0.5,0.5),vec3(1.0,1.0,0.5),vec3(0.8,0.90,0.30) );
             //   return vec4(0.0, 0.0, 1.0, 1.0);
                return vec4(C.xyz, 1.0);
            }
        }

        draw_thing:{
            width: Fill,
            height: Fill,
            texture image: texture2d,

            fn tovec(self, inp:vec2) -> vec4{
                let aNew = inp.x;
                let bNew = inp.y;
               

                return vec4(aNew, bNew, 0, 1.0);
            },

            fn pixel(self) -> vec4{  
               // return self.tovec(vec2(1.0,0.0));
                return self.tovec(vec2(sin(self.time*10.)*0.5+0.5,sin(self.time*10.123145)*0.5+0.5));
            }
        }

        draw_reaction:{
            width: Fill,
            height: Fill,
            texture image: texture2d,

           
            fn tovec(self, inp:vec2) -> vec4{
                let aNew = inp.x;
                let bNew = inp.y;
               

                return vec4(aNew, bNew, 0, 1.0);
            },


            fn samp(self, pos:vec2) -> vec2{
                let asamp = sample2d(self.image, pos);
                
                let A =   asamp.x;
                let B = asamp.y;

                return vec2(A,B);
            },

            fn pixel(self) -> vec4{
                
                let offset = vec2(0.0,0.0556);
                let pos2 = self.pos + offset;
                let Dx = 0.0025;
                let Dy = 0.002;


                let S1 = self.samp(pos2 );
               
                let S2 = self.samp(pos2 + vec2(Dx,0.0));
                let S3 = self.samp(pos2 + vec2(-Dx,0.0));
                let S4 = self.samp(pos2 + vec2(0.0,Dy));
                let S5 = self.samp(pos2 + vec2(0.0,-Dy));
               
                let S6 = self.samp(pos2 + vec2(Dx,Dy));
                let S7 = self.samp(pos2 + vec2(-Dx,Dy));
                let S8 = self.samp(pos2 + vec2(Dx,-Dy));
                let S9 = self.samp(pos2 + vec2(-Dx,-Dy));
             
             
                let avga = ((S2 + S3 + S4 + S5)*0.2) + ((S6 + S7 + S8 + S9)*0.05) -S1; 
                
                let a = S1.x;
                let b = S1.y;

                let DA = 1.0;
                let DB = 0.5;
                let f = 0.07 - self.pos.x * 0.035;
                let k = 0.07 - self.pos.y * 0.023;

                let speed = 1.0;
//                let aNew = S1.x + (avga.x - S1.x )  * 0.98;
  //              let bNew =  S1.y + (avga.y - S1.y )  * 0.98;
                let aNew = a + (DA * (avga.x) - (a * b * b) + f * (1-a)) *speed;
                let bNew = b + (DB * (avga.y) + (a * b * b) - (k+f)*b) * speed;
                
               // return vec4(0., 0., 0.0, 0.0);
      
                return self.tovec(vec2(aNew,bNew));
            }
        }
    }
}

#[derive(Live, Widget)]
pub struct DiffuseThing{

    #[redraw] #[live] draw: DrawQuad,
    #[rust(Pass::new(cx))] pass: Pass,
    #[layout] layout: Layout,
    #[walk] walk: Walk,
    #[live] time: f32,
    #[rust] next_frame: NextFrame,

    #[live] draw_bg: DrawQuad,
    #[live] draw_thing: DrawQuad,
    #[live] draw_reaction: DrawQuad,
    #[rust] area:Area,

    #[rust] target_0: Option<Texture>,
    #[rust] target_1: Option<Texture>,
    #[rust] current_target: i32,

    #[redraw] #[rust(DrawList2d::new(cx))] draw_list: DrawList2d,

}

impl LiveHook for DiffuseThing{
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        // starts the animation cycle on startup
        self.next_frame = cx.new_next_frame();
    }
}

impl Widget for DiffuseThing{
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _scope: &mut Scope
    ){
        if let Some(ne) = self.next_frame.is_event(event) {
            // update time to use for animation
            self.time = (ne.time * 0.001).fract() as f32;
            // force updates, so that we can animate in the absence of user-generated events
            self.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {


        if self.target_0.is_none(){
            self.target_0 = Some(Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8   {
                    size: TextureSize::Auto,
                    initial: true,
                },
            ));
        }

        if self.target_1.is_none(){
            self.target_1 = Some(Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            ));
        }

        if cx.will_redraw(&mut self.draw_list, walk) 
        {
            self.pass.clear_color_textures(cx);

            let mut target:&Texture = self.target_0.as_ref().unwrap();
            let mut source:&Texture = self.target_1.as_ref().unwrap();
            
            match self.current_target {
                0 => 
                {
                    self.current_target = 1;
                }
                1 => 
                {
                    target = self.target_1.as_ref().unwrap();
                    source = self.target_0.as_ref().unwrap();

                    self.current_target = 0;
                }
                _ =>
                {
                    self.current_target = 0;
                }
            }
            self.pass.add_color_texture(
                cx, 
                target,
                PassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0))
            );        

            cx.make_child_pass(&self.pass);
            
            cx.begin_pass(&self.pass, None);
            self.draw_list.begin_always(cx);
           
          
            cx.begin_turtle(walk, Layout::flow_down());
            

            self.draw_reaction.draw_vars.set_texture(0, source);    
            
            let rect:Rect = cx.walk_turtle_with_area(&mut self.area, walk);
           // rect.pos.y = 0.;
            self.draw_reaction.draw_abs(cx, rect);

            let rect2 = rect.scale_and_shift(rect.center(), 0.1, DVec2{x: 0.0, y: 0.0});
            self.draw_thing.draw_abs(cx, rect2);


            cx.end_turtle();
            self.draw_list.end(cx);
            cx.end_pass(&self.pass);

          

            self.draw_bg.draw_vars.set_texture(0, target);    
            let rect = cx.walk_turtle_with_area(&mut self.area, walk);
            self.draw_bg.draw_abs(cx, rect);

            
            cx.set_pass_area_with_origin(
                &self.pass,
                self.area,
                dvec2(0.0,0.0)
            );

        }
        DrawStep::done()

    }
}
