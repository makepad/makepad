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
            let fractal = sample2d(self.tex, vec2(self.pos.x, 1.0 - self.pos.y))
            let iter = fractal.y * 65535 + fractal.x * 255;
            let dist = (fractal.w * 256 + fractal.z - 127);
            let index = abs(8.0 * iter / 256 - 0.2 * log(dist));
            if iter > 255 {
                return vec4(0, 0, 0, self.alpha)
            }
            return vec4(Pal::iq2(index) * self.alpha, self.alpha);
        }
    }
    
    Mandelbrot: {{Mandelbrot}} {
        max_iter: 256,
        tile_size: vec2(128, 128),
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawMandelbrot {
    draw_super: DrawQuad,
    alpha: f32,
}

pub enum ToUI {
    TileDone {tile: TextureTile, into_current: bool},
}

pub struct TextureTile {
    buffer: Vec<u32>,
    texture: Texture,
    fractal: RectF64,
    display: Rect
}

const TILE_SIZE_X: usize = 256;
const TILE_SIZE_Y: usize = 256;
const CACHE_MAX: usize = 1024;

pub struct TileCache {
    current: Vec<TextureTile>,
    next: Vec<TextureTile>,
    empty: Vec<TextureTile>,
    current_zoom: f64,
    next_zoom: f64,
}

impl TileCache {
    fn new(cx: &mut Cx) -> Self {
        let mut empty = Vec::new();
        for _ in 0..CACHE_MAX {
            empty.push(TextureTile::new(cx));
        }
        Self {
            current: Vec::new(),
            next: Vec::new(),
            empty,
            current_zoom: 0.5,
            next_zoom: 0.5
        }
    }
    fn cycle(&mut self){
        self.current_zoom = self.next_zoom;
        while let Some(item) = self.current.pop(){
            self.empty.push(item);
        }
        std::mem::swap(&mut self.current, &mut self.next);
    }
}

impl TextureTile {
    fn new(cx: &mut Cx) -> Self {
        let texture = Texture::new(cx);
        texture.set_desc(cx, TextureDesc {
            format: TextureFormat::ImageBGRA,
            width: Some(TILE_SIZE_X),
            height: Some(TILE_SIZE_Y),
            multisample: None
        });
        Self {
            buffer: Vec::new(),
            texture,
            fractal: Default::default(),
            display: Default::default()
        }
    }
}

pub enum ZoomMode {
    In(f64),
    Out(f64),
}

#[derive(Live, FrameComponent)]
#[live_register(frame_component!(Mandelbrot))]
pub struct Mandelbrot {
    draw_mandelbrot: DrawMandelbrot,
    max_iter: usize,
    #[rust] next_frame: NextFrame,
    
    #[rust(vec2f64(-1.5, 0.0))] fractal_center: Vec2F64,
    #[rust(0.5)] fractal_zoom: f64,
    
    view: View,
    state: State,
    walk: Walk,
    #[rust(TileCache::new(cx))] tile_cache: TileCache,

    #[rust] zoom_mode: Option<ZoomMode>,
    #[rust] view_rect: Rect,
    #[rust] view_center: Vec2,
    #[rust] view_shift: Vec2,
    #[rust] tile_size: Vec2,
    
    #[rust(ThreadPool::new(cx, 4))] pool: ThreadPool,
    #[rust] to_ui: ToUIReceiver<ToUI>,
}

impl LiveHook for Mandelbrot {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.next_frame = cx.new_next_frame();
    }
}

#[derive(Clone, FrameComponentAction)]
pub enum MandelbrotAction {
    None
}

impl Mandelbrot {
    
    // creates a nice spiral ordering to the tile rendering
    fn spiral_walk<F: FnMut(usize, isize, isize) -> bool>(mut f: F) {
        let mut di = 1;
        let mut dj = 0;
        let mut seg_len = 1;
        let mut i = 0;
        let mut j = 0;
        let mut seg_pass = 0;
        let mut any_intersect = false;
        for step in 0..1000 {
            if f(step, i, j) {
                any_intersect = true;
            }
            i += di;
            j += dj;
            seg_pass += 1;
            if seg_len == seg_pass {
                seg_pass = 0;
                let t = di;
                di = -dj;
                dj = t;
                if dj == 0 { // check if we had any intersections
                    if !any_intersect {
                        return
                    }
                    any_intersect = false;
                    seg_len += 1;
                }
            }
        }
    }
    
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
    
    pub fn render_tiles(&mut self, fractal_zoom: f64, fractal_center: Vec2F64) {

        let max_iter = self.max_iter;
        
        let tile_size = self.tile_size;
        let view_rect = self.view_rect;
        let view_center = self.view_center;
        
        let fractal_size = vec2f64(fractal_zoom, fractal_zoom);
        
        let tile_cache = &mut self.tile_cache;
        let mut into_current = false;

        if tile_cache.current.is_empty(){
            into_current = true;
            tile_cache.current_zoom = fractal_zoom;
        }
        else{
             if !tile_cache.next.is_empty(){
                tile_cache.cycle();
            }
            tile_cache.next_zoom = fractal_zoom;
        }
        
        let mut to_render = Vec::new();
        
        Self::spiral_walk( | _step, i, j | {
            let display = Rect {
                pos: view_rect.pos + view_center + tile_size * vec2(i as f32, j as f32),
                size: tile_size
            };
            if view_rect.intersects(display) {
                let fractal = RectF64 {
                    pos: fractal_center + fractal_size * vec2f64(i as f64, j as f64),
                    size: fractal_size
                };
                let mut tile = tile_cache.empty.pop().unwrap();
                tile.fractal = fractal;
                tile.display = display;
                to_render.push(tile);
                true
            }
            else {
                false
            }
        });
        
        // ok now we should iterate all filled tilecache items
        for mut tile in to_render {
            let to_ui = self.to_ui.sender();
            self.pool.execute(move || {
                
                tile.buffer.resize(TILE_SIZE_X * TILE_SIZE_Y, 0);
                let tile_size = vec2f64(TILE_SIZE_X as f64, TILE_SIZE_Y as f64);
                // ok lets draw our mandelbrot f64
                for y in 0..TILE_SIZE_Y {
                    for x in 0..TILE_SIZE_X {
                        // ok lets get our in-fractal pos
                        let fp = tile.fractal.pos + tile.fractal.size * (vec2f64(x as f64, y as f64) / tile_size);
                        let (iter, dist) = Self::mandelbrot_f64(max_iter, fp.x, fp.y);
                        let dist = (dist * 256.0 + 127.0 * 255.0).max(0.0).min(65535.0) as u32;
                        let data = iter as u32 | (dist << 16);
                        tile.buffer[y * TILE_SIZE_X + x] = data;
                    }
                }
                to_ui.send(ToUI::TileDone {tile, into_current}).unwrap();
            })
        }
    }
    
    pub fn zoom_around(&mut self, abs: Vec2) {
        let scale = (self.tile_cache.current_zoom / self.fractal_zoom) as f32;
        
        // this is the new pos
        let real = (abs - self.view_shift - self.view_center) / scale + self.view_center;
        let p_old = self.view_center - self.view_center * scale;
        let p_new = real - real * scale;
        
        // shift to keep the point in the same place
        self.view_center = real;
        self.view_shift += p_old - p_new;
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> MandelbrotAction {
        self.state_handle_event(cx, event);
        if let Some(_ne) = self.next_frame.triggered(event) {
            
            if self.tile_cache.current.is_empty() {
                self.view_center = self.view_rect.size * 0.5 - self.tile_size * 0.5;
                self.render_tiles(self.fractal_zoom, self.fractal_center);
            }
            match self.zoom_mode {
                Some(ZoomMode::In(f)) => {
                    self.fractal_zoom *= f;
                    self.next_frame = cx.new_next_frame();
                    self.view.redraw(cx);
                    // when do we trigger the rendering of the next batch of tiles?
                    if self.fractal_zoom < self.tile_cache.next_zoom{
                        // we should swap next with current and current thrown out
                        // get a next batch
                        self.render_tiles(self.fractal_zoom * 0.5, self.fractal_center);
                    }
                    // if fractal_zoom
                    // we generally have 2 layers. a current layer
                    // and a next layer.
                    // once we 'pass' the next layer we can throw away the current.
                    // if our 'current layer'
                }
                Some(ZoomMode::Out(f)) => {
                    self.fractal_zoom *= f;
                    self.next_frame = cx.new_next_frame();
                    self.view.redraw(cx);
                    if self.fractal_zoom > self.tile_cache.next_zoom{
                        // we should swap next with current and current thrown out
                        // get a next batch
                        self.render_tiles(self.fractal_zoom * 2.0, self.fractal_center);
                    }
                }
                None => ()
            }
        }
        
        while let Ok(msg) = self.to_ui.try_recv(event) {
            let ToUI::TileDone {mut tile, into_current} = msg;
            tile.texture.swap_image_u32(cx, &mut tile.buffer);
            if into_current {
                self.tile_cache.current.push(tile);
            }
            else {
                self.tile_cache.next.push(tile);
            }
            self.view.redraw(cx);
        }
        
        match event.hits(cx, self.view.area()) {
            HitEvent::FingerDown(fe) => {
                self.zoom_around(fe.abs);
                if fe.digit == 0 {
                    self.zoom_mode = Some(ZoomMode::In(0.98));
                }
                else {
                    self.zoom_mode = Some(ZoomMode::Out(1.02));
                }
                self.next_frame = cx.new_next_frame();
            },
            HitEvent::FingerMove(fe) => {
                self.zoom_around(fe.abs);
            }
            HitEvent::FingerUp(_) => {
                self.zoom_mode = None;
            }
            _ => ()
        }
        MandelbrotAction::None
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> ViewRedraw {
        self.view.begin(cx, walk, Layout::flow_right()) ?;
        
        self.view_rect = cx.turtle().rect();
        self.tile_size = vec2(TILE_SIZE_X as f32, TILE_SIZE_Y as f32) / cx.current_dpi_factor;
        
        for tile in &self.tile_cache.current {
            self.draw_mandelbrot.draw_vars.set_texture(0, &tile.texture);
            let scale = (self.tile_cache.current_zoom / self.fractal_zoom) as f32;
            let display = tile.display.scale_and_shift(self.view_center, scale as f32, self.view_shift);
            self.draw_mandelbrot.alpha = 1.0;
            self.draw_mandelbrot.draw_abs(cx, display);
        }
        for tile in &self.tile_cache.next {
            self.draw_mandelbrot.draw_vars.set_texture(0, &tile.texture);
            let scale = (self.tile_cache.next_zoom / self.fractal_zoom) as f32;
            let display = tile.display.scale_and_shift(self.view_center, scale as f32, self.view_shift);
            // ok so we have current zoom, and next zoom
            // and the fractal zoom needs to 'blend' between them
            let blend = (self.tile_cache.current_zoom - self.fractal_zoom) / (self.tile_cache.current_zoom - self.tile_cache.next_zoom);
            self.draw_mandelbrot.alpha = blend as f32;
            self.draw_mandelbrot.draw_abs(cx, display);
        }


        // ok now render the 'next' tiles blended
        
        
        self.view.end(cx);
        
        Ok(())
    }
}