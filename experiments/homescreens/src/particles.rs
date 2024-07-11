
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
        
       spawnrate: 10.,
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
    #[rust] lasttime: f64,
    #[rust] deltatime: f64,
    #[rust] spawncounter: f64,
    #[live(10.0)] spawnrate: f64,
    #[live(100)] maxparticles: i32,
    #[rust] seed: u32
}


impl LiveHook for ParticleSystem {
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        // starts the animation cycle on startup
        self.next_frame = cx.new_next_frame();
        self.lasttime = 0.0;
        
    }
}

fn random_bit(seed: &mut u32) -> u32 {
    *seed = seed.overflowing_add((seed.overflowing_mul(*seed)).0 | 5).0;
    return *seed >> 31;
}

fn random_f32(seed: &mut u32) -> f32 {
    let mut out = 0;
    for _ in 0..32 {
        out |= random_bit(seed);
        out <<= 1;
    }
    out as f32 / std::u32::MAX as f32
}

fn random_f64(seed: &mut u32) -> f64 {
    let mut out = 0;
    for _ in 0..32 {
        out |= random_bit(seed);
        out <<= 1;
    }
    out as f64 / std::u32::MAX as f64
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
            self.deltatime = ne.time - self.lasttime;
            self.lasttime= ne.time;
            self.area.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }

    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // lets draw a bunch of quads
        let fullrect = cx.walk_turtle_with_area(&mut self.area, walk);

     
     
        self.spawncounter+= self.deltatime;
        while self.spawncounter * self.spawnrate > 1.0 && self.particles.len() < self.maxparticles as usize
        {
            self.spawncounter -= 1.0/self.spawnrate.max(0.00001);
            self.particles.push(Particle{position: dvec2(random_f64(&mut self.seed) *  fullrect.size.x,-10.0), velocity: dvec2(0.0,random_f64(&mut self.seed) * 100.0), acceleration: dvec2(0.0,0.0), life: 10.0});
        }

        if (self.spawncounter > 1.0) {
            self.spawncounter = 1.0
        }
        // self.line_width = 10.5;
        for particle in &mut self.particles {
            particle.update(self.deltatime);
        
            let mut rect = fullrect;

            rect.pos = particle.position + fullrect.pos;
            rect.size = dvec2(10.,10.);
            self.draw_quad.draw_abs(cx, rect);
        
        }

        self.particles.retain(|x| x.life > 0.0);

      
        DrawStep::done()
    }
}
