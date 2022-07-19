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
            // unpack iteration and distance
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
    current_geom: TileLayerGeom,
    next_geom: TileLayerGeom,
}

#[derive(Default, Clone)]
pub struct TileLayerGeom {
    fractal_zoom: f64,
    fractal_center: Vec2F64,
    rect: Rect,
    center: Vec2,
    shift: Vec2,
}

impl TileLayerGeom{
    pub fn zoom_around(&mut self, abs: Vec2, fractal_zoom: f64) {
        let scale = (self.fractal_zoom / fractal_zoom) as f32;
        
        // this is the new pos
        let real = (abs - self.shift - self.center) / scale + self.center;
        let p_old = self.center - self.center * scale;
        let p_new = real - real * scale;
        
        // shift to keep the point in the same place
        self.center = real;
        self.shift += p_old - p_new;
    }
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
            current_geom: Default::default(),
            next_geom: Default::default()
        }
    }
    fn cycle(&mut self){
        self.current_geom = self.next_geom.clone();
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
    
    #[rust(0.5)] fractal_zoom: f64,
    
    view: View,
    state: State,
    walk: Walk,
    #[rust(TileCache::new(cx))] tile_cache: TileCache,

    #[rust] zoom_mode: Option<ZoomMode>,
    #[rust] view_rect2: Rect,
    #[rust] view_center2: Vec2,
    
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
    
    fn mandelbrot_pixel_f64(max_iter: usize, c_x: f64, c_y: f64) -> (usize, f64) {
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
    
    fn mandelbrot_f64(tile:&mut TextureTile, max_iter: usize) {
        tile.buffer.resize(TILE_SIZE_X * TILE_SIZE_Y, 0);
        let tile_size = vec2f64(TILE_SIZE_X as f64, TILE_SIZE_Y as f64);
        // ok lets draw our mandelbrot f64
        for y in 0..TILE_SIZE_Y {
            for x in 0..TILE_SIZE_X {
                // ok lets get our in-fractal pos
                let fp = tile.fractal.pos + tile.fractal.size * (vec2f64(x as f64, y as f64) / tile_size);
                let (iter, dist) = Self::mandelbrot_pixel_f64(max_iter, fp.x, fp.y);
                // pack iterations and distance into a u32 texture
                let dist = (dist * 256.0 + 127.0 * 255.0).max(0.0).min(65535.0) as u32;
                let data = iter as u32 | (dist << 16);
                tile.buffer[y * TILE_SIZE_X + x] = data;
            }
        }
    }
    
    pub fn mandelbrot_tile_generator(&mut self, fractal_zoom: f64, is_zoom_in: bool) {

        let max_iter = self.max_iter;
        
        let fractal_size = vec2f64(fractal_zoom, fractal_zoom);
        
        let tile_cache = &mut self.tile_cache;
        let mut into_current = false;
        
        let tile_geom = if tile_cache.current.is_empty(){
            into_current = true;
            tile_cache.next_geom.fractal_zoom = fractal_zoom;
            &mut tile_cache.current_geom
        }
        else{
            // lets jump the fractal center.
            // question is by how much
            tile_cache.next_geom.fractal_center = tile_cache.current_geom.fractal_center;
            
            
            if !tile_cache.next.is_empty(){
                tile_cache.cycle();
            }
            &mut tile_cache.next_geom
        };
        
        // ok so fractal zoom jumps from this to that
        // and our center jumped from X to Y
        // that should give us a shift for fractal center
        //let zoom_jump = tile_geom.fractal_zoom / fractal_zoom;
        //let 
        
        let tile_size = self.tile_size;
        tile_geom.fractal_zoom = fractal_zoom;
        tile_geom.rect = self.view_rect2;
        tile_geom.center = self.view_center2;
        tile_geom.shift = vec2(0.0,0.0); 
        
        // shift has to be negative?
        // ok so. rethink it.
        // first off lets keep our tiles in 'fractal' space
        // lets compute our display window in fractal space.
        // we need to compute a new fractal center for these tiles
        // so we have our old layer at center X
        // and then our new layer at zoom level Y, but a new 'center'
        
        let tile_geom = tile_geom.clone();
        
        let mut to_render = Vec::new();
        
        Self::spiral_walk( | _step, i, j | {
            let display = Rect {
                pos: tile_geom.rect.pos + tile_geom.center + tile_size * vec2(i as f32, j as f32),
                size: tile_size
            };
            if tile_geom.rect.intersects(display) {
                let fractal = RectF64 {
                    pos: tile_geom.fractal_center + fractal_size * vec2f64(i as f64, j as f64),
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
        
        if is_zoom_in{
            for mut tile in to_render {
                let to_ui = self.to_ui.sender();
                self.pool.execute(move || {
                    Self::mandelbrot_f64(&mut tile, max_iter);
                    to_ui.send(ToUI::TileDone {tile, into_current}).unwrap();
                })
            }
        }
        else{ // on zoom out reverse the spiral
            for mut tile in to_render.into_iter().rev() {
                let to_ui = self.to_ui.sender();
                self.pool.execute(move || {
                    Self::mandelbrot_f64(&mut tile, max_iter);
                    to_ui.send(ToUI::TileDone {tile, into_current}).unwrap();
                })
            }
        }
    }
    
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> MandelbrotAction {
        self.state_handle_event(cx, event);
        if let Some(_ne) = self.next_frame.triggered(event) {
            
            if self.tile_cache.current.is_empty() {
                self.view_center2 = self.view_rect2.size * 0.5 - self.tile_size * 0.5;
                self.tile_cache.current_geom.fractal_center = vec2f64(-1.5, 0.0);
                self.mandelbrot_tile_generator(self.fractal_zoom, true);
            }
            match self.zoom_mode {
                Some(ZoomMode::In(f)) => {
                    self.fractal_zoom *= f;
                    self.next_frame = cx.new_next_frame();
                    self.view.redraw(cx);
                    // when do we trigger the rendering of the next batch of tiles?
                    if self.fractal_zoom < self.tile_cache.next_geom.fractal_zoom{
                        // we should swap next with current and current thrown out
                        // get a next batch
                       self.mandelbrot_tile_generator(self.fractal_zoom * 0.5, true);
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
                    if self.fractal_zoom > self.tile_cache.next_geom.fractal_zoom{
                        // we should swap next with current and current thrown out
                        // get a next batch
                        self.mandelbrot_tile_generator(self.fractal_zoom * 2.0, false);
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
                self.view_center2 = fe.abs;
                self.tile_cache.current_geom.zoom_around(fe.abs, self.fractal_zoom);
                self.tile_cache.next_geom.zoom_around(fe.abs, self.fractal_zoom);
                if fe.digit == 0 {
                    self.zoom_mode = Some(ZoomMode::In(0.98));
                }
                else {
                    self.zoom_mode = Some(ZoomMode::Out(1.02));
                }
                self.next_frame = cx.new_next_frame();
            },
            HitEvent::FingerMove(fe) => {
                self.view_center2 = fe.abs;
                self.tile_cache.current_geom.zoom_around(fe.abs, self.fractal_zoom);
                self.tile_cache.next_geom.zoom_around(fe.abs, self.fractal_zoom);
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
        
        self.view_rect2 = cx.turtle().rect();
        self.tile_size = vec2(TILE_SIZE_X as f32, TILE_SIZE_Y as f32) / cx.current_dpi_factor;
        let tc = &mut self.tile_cache;
        let cg = &tc.current_geom;
        let ng = &tc.next_geom;
    
        for tile in &tc.current {
            let scale = (cg.fractal_zoom / self.fractal_zoom) as f32;
            self.draw_mandelbrot.draw_vars.set_texture(0, &tile.texture);
            let display = tile.display.scale_and_shift(cg.center, scale as f32, cg.shift);
            self.draw_mandelbrot.alpha = 1.0;
            self.draw_mandelbrot.draw_abs(cx, display);
        }
        for tile in &tc.next {
            let scale = (ng.fractal_zoom / self.fractal_zoom) as f32;
            self.draw_mandelbrot.draw_vars.set_texture(0, &tile.texture);
            let display = tile.display.scale_and_shift(ng.center, scale as f32, ng.shift);
            let blend = 1.0-(self.fractal_zoom - ng.fractal_zoom) / (cg.fractal_zoom - ng.fractal_zoom);
            self.draw_mandelbrot.alpha = blend as f32;
            self.draw_mandelbrot.draw_abs(cx, display);
        }
        
        self.view.end(cx);
        
        Ok(())
    }
}