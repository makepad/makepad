#![allow(dead_code)]
#![allow(unused)]

use {
    std::sync::Arc,
    std::sync::Mutex,
    std::cell::RefCell,
    std::simd::*,
    crate::{
        makepad_platform::*,
        frame_component::*,
    }
};

#[cfg(any(not(target_arch = "wasm32"),all(target_arch = "wasm32",target_feature = "simd128")))]
use crate::mandelbrot_simd::*;

live_register!{
    use makepad_platform::shader::std::*;
    
    DrawMandelbrot: {{DrawMandelbrot}} {
        texture tex: texture2d
        fn pixel(self) -> vec4 {
            //let tile_size = vec2(255.0,255.0);
            //let texpos = floor(vec2(self.pos.x, 1.0 - self.pos.y) * tile_size + vec2(0.5,0.5)) / tile_size;
            let fractal = sample2d(self.tex, vec2(self.pos.x, 1.0 - self.pos.y))
            // unpack iteration and distance
            let iter = fractal.y * 65535 + fractal.x * 255;
            let dist = (fractal.w * 256 + fractal.z - 127);
            
            let index = abs((6.0 * iter / self.max_iter) - 0.1 * log(dist));
            if iter > self.max_iter {
                return vec4(0, 0, 0, self.alpha);
            }
            return vec4(Pal::iq2(index + self.cycle) * self.alpha, self.alpha);
        }
    }
    
    Mandelbrot: {{Mandelbrot}} {
        max_iter: 320,
        tile_size: vec2(128, 128),
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawMandelbrot {
    draw_super: DrawQuad,
    max_iter: f32,
    alpha: f32,
    cycle: f32
}

pub enum ToUI {
    TileDone {tile: TextureTile, into_current: bool},
    TileBailed {tile: TextureTile},
}

pub struct TextureTile {
    pub buffer: Vec<u32>,
    pub texture: Texture,
    pub fractal: RectF64,
}

pub const TILE_SIZE_X: usize = 256;
pub const TILE_SIZE_Y: usize = 256;
pub const CACHE_MAX: usize = 500;

    
    
// basic plain f64 loop
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

fn mandelbrot_f64(tile: &mut TextureTile, max_iter: usize) {
    let tile_size = vec2f64(TILE_SIZE_X as f64, TILE_SIZE_Y as f64);
    for y in 0..TILE_SIZE_Y {
        for x in 0..TILE_SIZE_X {
            let fp = tile.fractal.pos + tile.fractal.size * (vec2f64(x as f64, y as f64) / tile_size);
            let (iter, dist) = mandelbrot_pixel_f64(max_iter, fp.x, fp.y);
            let dist = (dist * 256.0 + 127.0 * 255.0).max(0.0).min(65535.0) as u32;
            tile.buffer[y * TILE_SIZE_X + x] = iter as u32 | (dist << 16);
        }
    }
}

#[derive(Default)]
pub struct TileCache {
    current: Vec<TextureTile>,
    next: Vec<TextureTile>,
    empty: Vec<TextureTile>,
    current_zoom: f64,
    next_zoom: f64,
    renders_in_queue: usize,
}

impl TileCache {
    fn new(cx: &mut Cx) -> Self {
        let mut empty = Vec::new();
        for _ in 0..CACHE_MAX {
            empty.push(TextureTile::new(cx));
        }
        Self {
            empty,
            ..Default::default()
        }
    }
    
    fn discard_layer(&mut self, cx:&mut Cx, discard_next:bool) {
        if discard_next{
            while let Some(mut item) = self.next.pop() {
                item.swap_buffer_with_texture(cx);
                self.empty.push(item);
            }
        }
        else{
            while let Some(mut item) = self.current.pop() {
                item.swap_buffer_with_texture(cx);
                self.empty.push(item);
            }
            self.current_zoom = self.next_zoom;
            std::mem::swap(&mut self.current, &mut self.next);
        }
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
        // preallocate buffers otherwise safari barfs in the worker
        let mut buffer = Vec::new();
        buffer.resize(TILE_SIZE_X * TILE_SIZE_Y, 0);
        Self {
            buffer,
            texture,
            fractal: RectF64::default()
        }
    }
    
    fn swap_buffer_with_texture(&mut self, cx: &mut Cx){
        self.texture.swap_image_u32(cx, &mut self.buffer);
    }
}

#[derive(Default)]
pub struct FractalSpace {
    view_space: Rect,
    tile_size: Vec2F64,
}

impl FractalSpace {
    fn fractal_to_view(&self, fractal_zoom: f64, fractal_center: Vec2F64, pos: Vec2F64) -> Vec2 {
        let view_center = self.view_space.pos + self.view_space.size * 0.5;
        return (((pos - fractal_center) / fractal_zoom) * self.tile_size).into_vec2() + view_center;
    }
    
    fn view_to_fractal(&self, fractal_zoom: f64, fractal_center: Vec2F64, pos: Vec2) -> Vec2F64 {
        let view_center = self.view_space.pos + self.view_space.size * 0.5;
        return (((pos - view_center).into_vec2f64() / self.tile_size) * fractal_zoom) + fractal_center;
    }
    
    fn fractal_to_view_rect(&self, fractal_zoom: f64, fractal_center: Vec2F64, rect: RectF64) -> Rect {
        let pos1 = self.fractal_to_view(fractal_zoom, fractal_center, rect.pos);
        let pos2 = self.fractal_to_view(fractal_zoom, fractal_center, rect.pos + rect.size);
        Rect {
            pos: pos1,
            size: pos2 - pos1
        }
    }
    
    // transform a rect in view space to fractal space
    fn view_to_fractal_rect(&self, fractal_zoom: f64, fractal_center: Vec2F64, rect: Rect) -> RectF64 {
        let pos1 = self.view_to_fractal(fractal_zoom, fractal_center, rect.pos);
        let pos2 = self.view_to_fractal(fractal_zoom, fractal_center, rect.pos + rect.size);
        RectF64 {
            pos: pos1,
            size: pos2 - pos1
        }
    }
    
    // self.view_rect in fractal space
    fn view_fractal_rect(&self, fractal_zoom: f64, fractal_center: Vec2F64) -> RectF64 {
        self.view_to_fractal_rect(fractal_zoom, fractal_center, self.view_space)
    }
}

#[derive(Live, FrameComponent)]
#[live_register(frame_component!(Mandelbrot))]
pub struct Mandelbrot {
    draw_mandelbrot: DrawMandelbrot,
    max_iter: usize,
    #[rust] next_frame: NextFrame,
    
    #[rust(vec2f64(-0.5, 0.0))] fractal_center: Vec2F64,
    #[rust(0.5)] fractal_zoom: f64,
    
    #[rust] finger_abs: Vec2,
    #[rust] is_zooming: bool,
    #[rust(true)] is_zoom_in: bool,
    #[rust] cycle: f32,
    
    #[rust] bail_window: Arc<Mutex<RefCell<(bool,RectF64)>>>,
    
    #[rust] space: FractalSpace,
    
    view: View,
    state: State,
    walk: Walk,
    #[rust(TileCache::new(cx))] tile_cache: TileCache,
    
    #[rust(Some(ThreadPool::new(cx, 4)))] pool: Option<ThreadPool>,
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
        let mut intersect_step = 0;
        for step in 0..100000 {
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
                    intersect_step += 1;
                    if intersect_step > 2 {
                        if !any_intersect {
                            return
                        }
                        intersect_step = 0;
                        any_intersect = false;
                    }
                    seg_len += 1;
                }
            }
        }
    }
    
    

    
    pub fn zoom_around(&mut self, factor: f64, around: Vec2) {
        let fpos1 = self.space.view_to_fractal(self.fractal_zoom, self.fractal_center, around);
        self.fractal_zoom *= factor;
        if self.fractal_zoom < 5e-14f64 {
            self.fractal_zoom = 5e-14f64
        }
        if self.fractal_zoom > 2.0 {
            self.fractal_zoom = 2.0;
        }
        let fpos2 = self.space.view_to_fractal(self.fractal_zoom, self.fractal_center, around);
        self.fractal_center += fpos1 - fpos2;
    }
    
    #[cfg(any(not(target_arch = "wasm32"),all(target_arch = "wasm32",target_feature = "simd128")))]
    pub fn render_tile(&mut self, mut tile:TextureTile, fractal_zoom: f64, into_current:bool){
        // lets swap our texture to the tile
        let max_iter = self.max_iter;
        let to_ui = self.to_ui.sender();
        let bail_window = self.bail_window.clone();
        self.pool.as_mut().unwrap().execute(move || {
            // lets check our tile still intersects the view when we start to compute it
            // otherwise we abort
            let (is_zoom_in,bail_window) = bail_window.lock().unwrap().borrow().clone();
            if is_zoom_in && !tile.fractal.intersects(bail_window){
                return to_ui.send(ToUI::TileBailed {tile}).unwrap();
            }
            
            if fractal_zoom >2e-5 {
                mandelbrot_f32_simd(&mut tile, max_iter);
            }
            else {
                mandelbrot_f64_simd(&mut tile, max_iter);
            }
            to_ui.send(ToUI::TileDone {tile, into_current}).unwrap();
        })        
    }
    
    #[cfg(all(target_arch = "wasm32", not(target_feature = "simd128")))]
    pub fn render_tile(&mut self, mut tile:TextureTile, fractal_zoom: f64, into_current:bool){
        // lets swap our texture to the tile
        let max_iter = self.max_iter;
        let to_ui = self.to_ui.sender();
        self.pool.as_mut().unwrap().execute(move || {
            mandelbrot_f64(&mut tile, max_iter);
            to_ui.send(ToUI::TileDone {tile, into_current}).unwrap();
        })        
    }
    
    pub fn mandelbrot_tile_generator(
        &mut self,
        cx:&mut Cx,
        fractal_zoom: f64,
        fractal_center: Vec2F64,
        fractal_space: RectF64,
        is_zoom_in: bool
    ) {
        
        // lets widen the fractal rect with about 1 tile all around
        let fractal_size = vec2f64(fractal_zoom, fractal_zoom);
        
        let tile_cache = &mut self.tile_cache;
        
        let into_current = if tile_cache.current.is_empty() {
            tile_cache.current_zoom = fractal_zoom;
            tile_cache.next_zoom = fractal_zoom;
            true
        }
        else {
            if !tile_cache.next.is_empty() {
                // if we interrupted a zoom, then our layer doesn't fill the edges
                let discard_next = is_zoom_in && tile_cache.next[0].fractal.size.x < 0.8 * fractal_zoom;
                tile_cache.discard_layer(cx, discard_next);
                tile_cache.next_zoom = fractal_zoom;
            }
            false
        };
        
        let mut render_queue = Vec::new();
        let fractal_space = fractal_space.add_margin(fractal_size);
        Self::spiral_walk( | _step, i, j | {
            //let tile_pos =
            let fractal = RectF64 {
                pos: fractal_center + fractal_size * vec2f64(i as f64, j as f64) - 0.5 * fractal_size,
                size: fractal_size
            };
            if fractal_space.intersects(fractal) {
                if let Some(mut tile) = tile_cache.empty.pop() {
                    tile.fractal = fractal;
                    render_queue.push(tile);
                }
                true
            }
            else {
                false
            }
        });
        self.tile_cache.renders_in_queue = render_queue.len();
        
        if self.pool.is_none() {
            return;
        }
        if is_zoom_in {
            for mut tile in render_queue {
                self.render_tile(tile, fractal_zoom, into_current)
            }
        }
        else { // on zoom out reverse the spiral
            for mut tile in render_queue.into_iter().rev() {
                self.render_tile(tile, fractal_zoom, into_current)
            }
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> MandelbrotAction {
        self.state_handle_event(cx, event);
        
        if let Event::Signal(_) = event{
            self.next_frame = cx.new_next_frame();
        }
        
        if let Some(ne) = self.next_frame.triggered(event) {
            let mut too_many_textures = 0;
            while let Ok(msg) = self.to_ui.receiver.try_recv() {
                match msg{
                    ToUI::TileDone {mut tile, into_current}=>{
                        self.tile_cache.renders_in_queue -= 1;
                        tile.swap_buffer_with_texture(cx);
                        if into_current {
                            self.tile_cache.current.push(tile);
                        }
                        else {
                            self.tile_cache.next.push(tile);
                        }
                        
                        if self.tile_cache.renders_in_queue == 0 && self.tile_cache.next_zoom != self.fractal_zoom {
                            let zoom = self.fractal_zoom * if self.is_zooming{if self.is_zoom_in{0.8}else{3.0}}else{1.0};
                            self.mandelbrot_tile_generator(
                                cx,
                                zoom,
                                self.space.view_to_fractal(zoom, self.fractal_center, self.finger_abs),
                                self.space.view_fractal_rect(zoom, self.fractal_center),
                                self.is_zoom_in
                            );
                        }
                        too_many_textures += 1;
                        if too_many_textures > 10{
                            break;
                        }
                    }
                    ToUI::TileBailed{mut tile}=>{
                        self.tile_cache.renders_in_queue -= 1;
                        self.tile_cache.empty.push(tile);
                    }
                }

            }
            // initial tile render
            if self.tile_cache.renders_in_queue == 0 && self.tile_cache.current.is_empty() {
                self.mandelbrot_tile_generator(
                    cx,
                    self.fractal_zoom,
                    self.fractal_center,
                    self.space.view_fractal_rect(self.fractal_zoom, self.fractal_center),
                    true
                );
            }
            if self.is_zooming { // this only fires once 
                self.zoom_around(if self.is_zoom_in {0.98} else {1.02}, self.finger_abs);
                if self.tile_cache.renders_in_queue == 0{
                    let zoom = self.fractal_zoom * if self.is_zoom_in {0.8} else {3.0};
                    self.mandelbrot_tile_generator(
                        cx,
                        zoom,
                        self.space.view_to_fractal(zoom, self.fractal_center, self.finger_abs),
                        self.space.view_fractal_rect(zoom, self.fractal_center),
                        self.is_zoom_in
                    );
                }
                self.view.redraw(cx);
            }
            // ok now the cycle.
            self.cycle = (ne.time * 0.2).fract() as f32;
            self.view.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
        
        match event.hits_with_options(cx, self.view.area(), HitOptions {use_multi_touch: true, margin: None}) {
            HitEvent::FingerDown(fe) => {
                self.finger_abs = fe.abs;
                self.is_zooming = true;
                if fe.digit == 0 {
                    self.is_zoom_in = true;
                }
                else {
                    self.is_zoom_in = false;
                }
                self.next_frame = cx.new_next_frame();
            },
            HitEvent::FingerMove(fe) => {
                self.finger_abs = fe.abs;
            }
            HitEvent::FingerUp(fe) => {
                if fe.input_type.is_touch() && fe.digit == 1 {
                    self.is_zoom_in = true;
                }
                else {
                    self.is_zoom_in = true;
                    self.is_zooming = false;
                }
            }
            _ => ()
        }
        MandelbrotAction::None
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> ViewRedraw {
        self.view.begin(cx, walk, Layout::flow_right()) ?;
        self.space.tile_size = vec2f64(TILE_SIZE_X as f64, TILE_SIZE_Y as f64) / cx.current_dpi_factor as f64;
        self.space.view_space = cx.turtle().rect();
        
        *self.bail_window.lock().unwrap().borrow_mut() = (self.is_zoom_in,self.space.view_fractal_rect(self.fractal_zoom, self.fractal_center));
        
        self.draw_mandelbrot.alpha = 1.0;
        self.draw_mandelbrot.max_iter = self.max_iter as f32;
        self.draw_mandelbrot.cycle = self.cycle;

        let tc = &mut self.tile_cache;
        for tile in &tc.current {
            let rect = self.space.fractal_to_view_rect(self.fractal_zoom, self.fractal_center, tile.fractal);
            self.draw_mandelbrot.draw_vars.set_texture(0, &tile.texture);
            self.draw_mandelbrot.draw_abs(cx, rect);
        }
        for tile in &tc.next {
            let rect = self.space.fractal_to_view_rect(self.fractal_zoom, self.fractal_center, tile.fractal);
            self.draw_mandelbrot.draw_vars.set_texture(0, &tile.texture);
            self.draw_mandelbrot.draw_abs(cx, rect);
        }
        
        self.view.end(cx);
        
        Ok(())
    }
}