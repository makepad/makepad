
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
    pub BirdSystem = {{BirdSystem}} {
        width: Fill,
        height: Fill,
        
       spawnrate: 10.,
        draw_bird: {
            texture image: texture2d,
            uniform thetime: 0.0,
            fn pixel(self) -> vec4{
                
                let frame = mod(floor(self.thetime*6.0) , 16.0);
                let x = mod(frame ,4.0);
                let y = floor(frame / 4 );
                
                let col = sample2d(self.image, self.pos*0.25 + vec2(x,y)*0.25);
                let alpha = 1 - cos(self.thetime*0.10)*0.5;
                //return vec4(0.0,0.02,0.02,pow(self.pos.y,5.0)*0.3);
                return col*alpha*0.2;
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
pub enum BirdType {
    Stroky
} 
    

pub struct Bird
{
    pub position: Vec3,
    pub startposition: Vec3,
    pub velocity: Vec3,
    pub acceleration: Vec3,
    pub life: f64,
    pub pathtime: f64,
    pub scale: f64,
    pub bird_type: BirdType
}

impl Bird {
    pub fn update(&mut self, deltatime: f64) {
        self.velocity += self.acceleration * deltatime as f32 ;    
        self.pathtime += deltatime*0.3;    
        self.position = self.startposition + vec3(sin(self.pathtime as f32)*0.3,sin(self.pathtime as f32*0.2)*0.1,0.);
       self.life -= deltatime;
        match self.bird_type{
            BirdType::Stroky => {
                self.life -= deltatime;
              
            }
           
        }
       
        if self.life < 0. {
            self.life = 0.;
        }
    }
}

#[derive(Live,  Widget)]
pub struct BirdSystem {
    #[animator]
    animator: Animator,
    #[walk]
    walk: Walk,
    #[live]
    #[redraw] 
    draw_bird: DrawQuad,
    #[live]
    #[redraw] 
    draw_splash: DrawQuad,
    #[redraw] #[rust]
    area: Area,
    #[rust] birds: Vec<Bird>,
    #[rust] next_frame: NextFrame,
    #[rust] lasttime: f64,
    #[rust] deltatime: f64,
    #[rust] spawncounter: f64,
    
    #[live(10.0)] spawnrate: f64,
    #[live(6.0) ] bird_width: f64,
    #[live(64.0) ] bird_height: f64,
    #[live(100)] max_birds: i32,

    #[live] birdtexture: Image,
    #[rust] seed: u32,
    #[live(0.0)] time: f64
}

fn perspective(input: Vec3) -> DVec2{
    let zdif = input.z.max(0.0001);
    dvec2((input.x / zdif) as f64, (input.y/zdif)as f64)
}

impl LiveHook for BirdSystem {
    
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
impl Widget for BirdSystem {
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
        self.draw_bird.set_texture(0,&self.birdtexture.get_texture(0).clone().unwrap());
        self.spawncounter+= self.deltatime;
        while self.spawncounter * self.spawnrate > 1.0 && self.birds.len() < self.max_birds as usize
        {
            self.spawncounter -= 1.0/self.spawnrate.max(0.00001);

            let inp = random_f32(&mut self.seed);
            let newscale = (1.0-(inp*inp)) *2.0+ 0.1;
            self.birds.push(
                Bird
                {
                    bird_type: BirdType::Stroky,  
                    scale: newscale as f64,
                    startposition: vec3(random_f32(&mut self.seed) ,random_f32(&mut self.seed)*0.2,random_f32(&mut self.seed)),
                    position: vec3(0.0,0.0,0.0),
                    velocity: vec3(0.0,0.1500 * newscale,0.), acceleration: vec3(0.0,0.0,0.0), life: 10.0, pathtime: 0.
                });
        }

        if self.spawncounter > 1.0 {
            self.spawncounter = 1.0
        }
        // self.line_width = 10.5;
        for bird in &mut self.birds {
            bird.update(self.deltatime);
        
            let mut rect = fullrect;

            match bird.bird_type {
                
                BirdType::Stroky => {
                    rect.size = dvec2(self.bird_width*bird.scale,self.bird_height*bird.scale);
                    let perspective_pos = perspective(bird.position);
                    rect.pos = perspective_pos * fullrect.size + fullrect.pos - rect.size/2.0;
                    self.draw_bird.set_uniform(cx, id!(thetime), &[self.time as f32 + bird.pathtime as f32]);
                    self.draw_bird.draw_abs(cx, rect);
                }   
                
            }
      
        
        }

        self.birds.retain(|x| x.life > 0.0);

      
        DrawStep::done()
    }
}
