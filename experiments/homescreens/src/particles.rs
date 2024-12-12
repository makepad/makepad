
use crate::{
    //  makepad_derive_widget::*,
      makepad_draw::*,
     // widget_match_event::*,
     // outline_tree::*,
     makepad_widgets::makepad_derive_widget::*,
     makepad_widgets::widget::*, 
     makepad_widgets::*, 
      makepad_widgets::image_cache::ImageCacheImpl
    };
  
  
live_design! {
    pub ParticleSystem = {{ParticleSystem}} {
        width: Fill,
        height: Fill,
        
       spawnrate: 10.,
        draw_rain: {
            texture image: texture2d
            fn pixel(self) -> vec4{
                
                let col = sample2d(self.image, self.pos);
                col *= 0.12;
                return col;
                //return vec4(0.0,0.02,0.02,pow(self.pos.y,5.0)*0.3)
            }
        }   
        draw_splash: {
            uniform thetime: 0.0,
                    // Function to create ripple effect
            fn ripple(uv: vec2,  dropPos: vec2 ,  time: float) -> float{
                let dist = distance(uv, dropPos);
                return ((sin(dist * 50.0 - time * 5.0)*0.5+0.5) * exp(-dist * 10.20));
            }

            fn pixel(self) -> vec4 {
                let R = ripple(self.pos, vec2(0.5,0.5), self.thetime)*0.2;
                return vec4(0.*R,0.*R,0.*R,R)
            }
        }   
    }
}
pub enum ParticleType {
    Drop,
    Splash
} 
    

pub struct Particle
{
    pub position: Vec3,
    pub velocity: Vec3,
    pub acceleration: Vec3,
    pub life: f64,
    pub scale: f64,
    pub particle_type: ParticleType
}

impl Particle {
    pub fn update(&mut self, deltatime: f64) {
        self.velocity += self.acceleration * deltatime as f32 ;        
        self.position += self.velocity * deltatime as f32;
       
        match self.particle_type{
            ParticleType::Splash => {
                self.life -= deltatime;
              
            }
            ParticleType::Drop => {
                if self.position.y> 1.0{
                    self.velocity.zero();
                    self.life = 0.;
                    self.acceleration.zero();
                    self.particle_type = ParticleType::Splash;
                }
            }
        }
       
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
    draw_rain: DrawQuad,
    #[live]
    #[redraw] 
    draw_splash: DrawQuad,
    #[redraw] #[rust]
    area: Area,
    #[rust] particles: Vec<Particle>,
    #[rust] next_frame: NextFrame,
    #[rust] lasttime: f64,
    #[rust] deltatime: f64,
    #[rust] spawncounter: f64,
    
    #[live(10.0)] spawnrate: f64,
    #[live(6.0) ] drop_width: f64,
    #[live(64.0) ] drop_height: f64,
    #[live(100)] maxparticles: i32,

    #[live] particletexture: Image,
    #[rust] seed: u32,
    #[live(0.0)] time: f64
}

fn perspective(input: Vec3) -> DVec2{
    let zdif = input.z.max(0.0001);
    dvec2((input.x / zdif) as f64, (input.y/zdif)as f64)
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

fn _random_f64(seed: &mut u32) -> f64 {
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
            self.time += self.deltatime;
           // self.draw_splash.time;// = self.time;
            self.area.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }

    }
  

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // lets draw a bunch of quads
        let fullrect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.draw_rain.set_texture(0,&self.particletexture.get_texture(0).clone().unwrap());
        self.spawncounter+= self.deltatime;
        while self.spawncounter * self.spawnrate > 1.0 && self.particles.len() < self.maxparticles as usize
        {
            self.spawncounter -= 1.0/self.spawnrate.max(0.00001);

            let inp = random_f32(&mut self.seed);
            let newscale = (1.0-(inp*inp)) *2.0+ 0.1;
            self.particles.push(
                Particle
                {
                    particle_type: ParticleType::Drop,  
                    scale: newscale as f64,
                    position: vec3(random_f32(&mut self.seed) ,-1.0,random_f32(&mut self.seed)),
                    velocity: vec3(0.0,0.1500 * newscale,0.), acceleration: vec3(0.0,0.0,0.0), life: 1.0});
        }

        if self.spawncounter > 1.0 {
            self.spawncounter = 1.0
        }
        // self.line_width = 10.5;
        for particle in &mut self.particles {
            particle.update(self.deltatime);
        
            let mut rect = fullrect;

            match particle.particle_type {
                
                ParticleType::Drop => {
                    rect.size = dvec2(self.drop_width*particle.scale,self.drop_height*particle.scale);
                    let perspective_pos = perspective(particle.position);
                    rect.pos = perspective_pos * fullrect.size + fullrect.pos - rect.size/2.0;
                    self.draw_rain.set_uniform(cx, id!(thetime), &[self.time as f32]);
                    self.draw_rain.draw_abs(cx, rect);
                }   
                ParticleType::Splash => {
                    rect.size = dvec2(80.*particle.scale,10.*particle.scale);
                    let perspective_pos = perspective(particle.position);
                    rect.pos = perspective_pos * fullrect.size + fullrect.pos - rect.size/2.0;
                    //self.draw_splash.set_uniform(cx, "time", self.time);
                    self.draw_splash.set_uniform(cx, id!(thetime), &[self.time as f32]);

                   self.draw_splash.draw_abs(cx, rect);
                }
            }
      
        
        }

        self.particles.retain(|x| x.life > 0.0);

      
        DrawStep::done()
    }
}
