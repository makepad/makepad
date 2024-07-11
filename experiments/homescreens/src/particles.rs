

use crate::{
    //  makepad_derive_widget::*,
      makepad_draw::*,
     // widget_match_event::*,
     // outline_tree::*,
     makepad_widgets::makepad_derive_widget::*,
     makepad_widgets::widget::*, 
  };
  
  
live_design! {
    ParticleSystem = {{ParticleSystem}} {
        width: Fill,
        height: Fill,
        
       
        draw_quad: {
            fn pixel(self) -> vec4{
                return vec4(0.05,0.34,1.0,1.)
            }
        }   
    }
}

pub struct Particle
{
    pub position: DVec2,
    pub velocity: DVec2,
    pub acceleration: DVec2,
    pub life: f64
}

impl Particle {
    pub fn update(&mut self, deltatime: f64) {
        self.velocity += self.acceleration * deltatime;        
        self.position += self.velocity * deltatime;
        self.life -= deltatime;
        if self.life < 0. {
            self.life = 0.;
        }
    }
}

#[derive(Live,  Widget)]
pub struct ParticleSystem {
    #[animator]
    animator: Animator,
    #[walk]
    walk: Walk,
    #[live]
    #[redraw] 
    draw_quad: DrawQuad,
    #[redraw] #[rust]
    area: Area,
    #[rust] particles: Vec<Particle>,
    #[rust] next_frame: NextFrame,
    #[rust] deltatime: f64,
}


impl LiveHook for ParticleSystem {
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        // starts the animation cycle on startup
        self.next_frame = cx.new_next_frame();
    }
}



impl Widget for ParticleSystem {
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _scope: &mut Scope,
    ) {
        self.animator_handle_event(cx, event);

        if let Some(ne) = self.next_frame.is_event(event) {
            self.deltatime = ne.time;
            self.area.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }

    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // lets draw a bunch of quads
        let fullrect = cx.walk_turtle_with_area(&mut self.area, walk);

        
        // self.line_width = 10.5;
        for particle in &mut self.particles {
            particle.update(self.deltatime);
        
            let mut rect = fullrect;

            rect.pos = particle.position;
            rect.size = dvec2(1.,1.);
            self.draw_quad.draw_abs(cx, rect);
        
        }

      
        DrawStep::done()
    }
}
