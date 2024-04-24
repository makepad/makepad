
use crate::{
  //  makepad_derive_widget::*,
    makepad_draw::*,
   // widget_match_event::*,
   // outline_tree::*,
   makepad_widgets::makepad_derive_widget::*,
   makepad_widgets::widget::*, 
};


live_design!{
    DiffuseThing = {{DiffuseThing}} 
    {
        height: 100,
        width: 100,
        draw_bg:{
            width: Fill,
            height: Fill,
            texture image: texture2d,
            fn pixel(self) -> vec4{

                let samp = sample2d(self.image, self.pos);
                let A = samp.y + samp.x * 10.0;
                let B = samp.w + samp.z * 10.0;
     
             //   return vec4(0.0, 0.0, 1.0, 1.0);
                return vec4(A/10.0, B/10.0, 0.0, 1.0);
            }
        }

        draw_thing:{
            width: Fill,
            height: Fill,
            texture image: texture2d,
            fn pixel(self) -> vec4{

                let samp = sample2d(self.image, self.pos);
                let A = samp.y + samp.x * 10.0;
                let B = samp.w + samp.z * 10.0;
     
                return vec4(1., 0., 0.0, 1.0);
            }
        }

        draw_reaction:{
            width: Fill,
            height: Fill,
            texture image: texture2d,
            fn pixel(self) -> vec4{
                 let samp = sample2d(self.image, self.pos);
                let A: float = samp.y + samp.x * 10.0;
                let B: float = samp.w + samp.z * 10.0;


                let bNew = B-A;
                let aNew = A-B;

                let Bh = floor(bNew);
                let Ah = floor(aNew);
                let Bl = (B-Bh);
                let Al = (A-Ah);
                
                return vec4(5., 1., 5.0, 1.0);
      
      //          return vec4(Ah, Al, Bh, Bl);
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
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                },
            ));
        }

        if self.target_1.is_none(){
            self.target_1 = Some(Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
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
                    target = self.target_0.as_ref().unwrap();
                    source = self.target_1.as_ref().unwrap();

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
            let rect = cx.walk_turtle_with_area(&mut self.area, walk);
            self.draw_reaction.draw_abs(cx, rect);

            let rect2 = rect.scale_and_shift(rect.center(), 0.3, DVec2{x: 0.0, y: 0.0});
            self.draw_thing.draw_abs(cx, rect2);


            cx.end_turtle();
             self.draw_list.end(cx);
            cx.end_pass(&self.pass);


            self.draw_bg.draw_vars.set_texture(0, target);    
            let rect = cx.walk_turtle_with_area(&mut self.area, walk);
            self.draw_bg.draw_abs(cx, rect);
        
        }
        DrawStep::done()

    }
}
