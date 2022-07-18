use {
    crate::{
        makepad_platform::*,
        frame_component::*,
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    
    DrawMandelbrot: {{DrawMandelbrot}} {
        texture tex: texture2d
        fn pixel(self) -> vec4 {
            let fractal = sample2d(self.tex, self.pos)
            let iter = fractal.y * 65535 + fractal.x*255;
            let dist = (fractal.w * 256 + fractal.z - 127) ;
            let index = abs(8.0*iter / 256 - 0.2 * log(dist));
            if iter > 255{
                return vec4(0,0,0,1)
            }
            return vec4(Pal::iq2(index),1.0);
        }
    }
    
    Mandelbrot: {{Mandelbrot}} {
        num_threads: 1,
        max_iter: 256,
        tile_width: 1024,
        tile_height: 1024,
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawMandelbrot {
    draw_super: DrawQuad,
}

#[derive(Clone, Debug)]
pub enum ToUI {
    TileDone(Vec<u32>),
}


#[derive(Live, FrameComponent)]
#[live_register(frame_component!(Mandelbrot))]
pub struct Mandelbrot {
    draw_mandelbrot: DrawMandelbrot,
    texture: Texture,
    num_threads: usize,
    max_iter: usize,
    tile_width: usize,
    tile_height: usize,
    state: State,
    walk: Walk,
    #[rust] pool: ThreadPool,
    #[rust] to_ui: ToUIReceiver<ToUI>,
}

impl LiveHook for Mandelbrot {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.texture.set_desc(cx, TextureDesc {
            format: TextureFormat::ImageBGRA,
            width: Some(self.tile_width),
            height: Some(self.tile_height),
            multisample: None
        });
        self.pool.add_threads(cx, self.num_threads);
        self.render_tile();
    }
}

#[derive(Clone, FrameComponentAction)]
pub enum MandelbrotAction {
    None
}

impl Mandelbrot {
    
    fn mandelbrot_f64(max_iter: usize, c_x: f64, c_y: f64) -> (usize, f64) {
        let mut x = c_x;
        let mut y = c_y;
        let mut dist = 0.0;
        for n in 0..max_iter {
            let xy = x * y;
            let xx = x * x;
            let yy = y * y;
            dist = xx + yy;
            if dist > 4.0 {
                return (n, dist)
            }
            x = (xx - yy) + c_x;
            y = (xy + xy) + c_y;
        }
        return (max_iter, dist)
    }
    
    pub fn render_tile(&mut self) {
        let max_iter = self.max_iter;
        // ok so how do we do this.
        // lets first draw a single tile
        let to_ui = self.to_ui.sender();
        
        let tile_width = self.tile_width;
        let tile_height = self.tile_height;
        
        let cx = -1.5;
        let cy = -1.0;
        let _zoom = 1.0;
        
        let mut image_u32 = Vec::new();
        self.pool.execute(move || {
            
            image_u32.resize(tile_width * tile_height, 0);
            // ok lets draw our mandelbrot f64
            for y in 0..tile_height {
                for x in 0..tile_width {
                    let fx = (x as f64 / tile_width as f64)*3.0 + cx;
                    let fy = (y as f64 / tile_height as f64)*2.0 + cy;
                    let (iter, dist) = Self::mandelbrot_f64(max_iter, fx, fy);
                    let dist = (dist * 256.0 + 127.0 * 255.0).max(0.0).min(65535.0) as u32;
                    let data = iter as u32 | (dist << 16);
                    image_u32[y * tile_width + x] =  data;
                }
            }
            
            to_ui.send(ToUI::TileDone(image_u32)).unwrap();
        })
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> MandelbrotAction {
        self.state_handle_event(cx, event);
        while let Ok(msg) = self.to_ui.try_recv(event) {
            let ToUI::TileDone(mut image_u32) = msg;
            self.texture.swap_image_u32(cx, &mut image_u32);
            self.draw_mandelbrot.area().redraw(cx);
        }
        
        match event.hits(cx, self.draw_mandelbrot.area()) {
            _ => ()
        }
        MandelbrotAction::None
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_mandelbrot.draw_vars.set_texture(0, &self.texture);
        self.draw_mandelbrot.draw_walk(cx, walk);
    }
}