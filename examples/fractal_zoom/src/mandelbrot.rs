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

#[cfg(any(not(target_arch = "wasm32"), all(target_arch = "wasm32", target_feature = "simd128")))]
use crate::mandelbrot_simd::*;

live_register!{
    use makepad_platform::shader::std::*;
    
    // the shader to draw the texturte tiles
    DrawTile: {{DrawTile}} {
        texture tex: texture2d
        fn pixel(self) -> vec4 {
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

// the shader struct used to draw

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawTile {
    draw_super: DrawQuad,
    max_iter: f32,
    alpha: f32,
    cycle: f32
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

fn mandelbrot_f64(tile: &mut Tile, max_iter: usize) {
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

pub struct Tile {
    pub buffer: Vec<u32>,
    pub texture: Texture,
    pub fractal: RectF64,
}

impl Tile {
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
    
    fn swap_buffer_with_texture(&mut self, cx: &mut Cx) {
        self.texture.swap_image_u32(cx, &mut self.buffer);
    }
}

#[derive(Default)]
pub struct TileCache {
    current: Vec<Tile>,
    next: Vec<Tile>,
    empty: Vec<Tile>,
    current_zoom: f64,
    next_zoom: f64,
    renders_in_queue: usize,
}

impl TileCache {
    fn new(cx: &mut Cx) -> Self {
        let mut empty = Vec::new();
        for _ in 0..CACHE_MAX {
            empty.push(Tile::new(cx));
        }
        Self {
            empty,
            ..Default::default()
        }
    }
    
    fn discard_next_layer(&mut self, cx: &mut Cx) {
        while let Some(mut item) = self.next.pop() {
            item.swap_buffer_with_texture(cx);
            self.empty.push(item);
        }
    }
    
    fn discard_current_layer(&mut self, cx: &mut Cx) {
        while let Some(mut item) = self.current.pop() {
            item.swap_buffer_with_texture(cx);
            self.empty.push(item);
        }
        self.current_zoom = self.next_zoom;
        std::mem::swap(&mut self.current, &mut self.next);
    }
}

pub enum ToUI {
    TileDone {tile: Tile},
    TileBailed {tile: Tile},
}

#[derive(Default)]
pub struct FractalSpace {
    view_rect: Rect,
    tile_size: Vec2F64,
    center: Vec2F64,
    zoom: f64,
}

// Space transforms from view (screen) to fractal and back
impl FractalSpace {
    fn new(center: Vec2F64, zoom: f64)->Self{
        Self{
            center,
            zoom,
            ..Self::default()
        }
    }
    fn fractal_to_screen(&self, other_zoom: f64, other_center: Vec2F64, pos: Vec2F64) -> Vec2 {
        let view_center = self.view_rect.pos + self.view_rect.size * 0.5;
        return (((pos - other_center) / other_zoom) * self.tile_size).into_vec2() + view_center;
    }
    
    fn screen_to_fractal(&self, other_zoom: f64, other_center: Vec2F64, pos: Vec2) -> Vec2F64 {
        let view_center = self.view_rect.pos + self.view_rect.size * 0.5;
        return (((pos - view_center).into_vec2f64() / self.tile_size) * other_zoom) + other_center;
    }
    
    fn fractal_to_screen_rect(&self, other_zoom: f64, other_center: Vec2F64, rect: RectF64) -> Rect {
        let pos1 = self.fractal_to_screen(other_zoom, other_center, rect.pos);
        let pos2 = self.fractal_to_screen(other_zoom, other_center, rect.pos + rect.size);
        Rect {
            pos: pos1,
            size: pos2 - pos1
        }
    }
    
    // transform a rect in view space to fractal space
    fn screen_to_fractal_rect(&self, other_zoom: f64, other_center: Vec2F64, rect: Rect) -> RectF64 {
        let pos1 = self.screen_to_fractal(other_zoom, other_center, rect.pos);
        let pos2 = self.screen_to_fractal(other_zoom, other_center, rect.pos + rect.size);
        RectF64 {
            pos: pos1,
            size: pos2 - pos1
        }
    }
    
    fn zoom_around(&mut self, factor: f64, around: Vec2) {
        let fpos1 = self.screen_to_fractal(self.zoom, self.center, around);
        self.zoom *= factor;
        if self.zoom < 5e-14f64 {
            self.zoom = 5e-14f64
        }
        if self.zoom > 2.0 {
            self.zoom = 2.0;
        }
        let fpos2 = self.screen_to_fractal(self.zoom, self.center, around);
        self.center += fpos1 - fpos2;
    }
    
    // self.view_rect in fractal space
    fn view_rect_to_fractal(&self, fractal_zoom: f64, fractal_center: Vec2F64) -> RectF64 {
        self.screen_to_fractal_rect(fractal_zoom, fractal_center, self.view_rect)
    }
}

#[derive(Clone, Default)]
pub struct BailWindow{
    window: RectF64,
    is_zoom_in: bool
}

#[derive(Live, FrameComponent)]
#[live_register(frame_component!(Mandelbrot))]
pub struct Mandelbrot {
    draw_tile: DrawTile,
    max_iter: usize,
    #[rust] next_frame: NextFrame,
    
    //#[rust(vec2f64(-0.5, 0.0))] fractal_center: Vec2F64,
    //#[rust(0.5)] fractal_zoom: f64,
    
    #[rust] finger_abs: Vec2,
    #[rust] is_zooming: bool,
    #[rust(true)] is_zoom_in: bool,
    #[rust] cycle: f32,
    // cross thread checkable window to see if a tile needs to be abandoned (outside the view)
    #[rust] bail_window: Arc<Mutex<RefCell<BailWindow>> >,
    #[rust(FractalSpace::new(vec2f64(-0.5, 0.0), 0.5))] space: FractalSpace,
    
    view: View,
    state: State,
    walk: Walk,
    #[rust(TileCache::new(cx))] tile_cache: TileCache,
    
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
    
    pub fn check_bail_window(tile: Tile, to_ui:&ToUISender<ToUI>, bail_window:Arc<Mutex<RefCell<BailWindow>> >)->Option<Tile>{
        let bail = bail_window.lock().unwrap().borrow().clone();
        if bail.is_zoom_in {
            if !tile.fractal.intersects(bail.window) {
                to_ui.send(ToUI::TileBailed {tile}).unwrap();
                return None
            }
        }
        else { // compare the size of the bail window against the tile
            if tile.fractal.size.x * tile.fractal.size.y < bail.window.size.x * bail.window.size.y * 0.007 {
                to_ui.send(ToUI::TileBailed {tile}).unwrap();
                return None
            }
        }
        Some(tile)
    }
    
    #[cfg(any(not(target_arch = "wasm32"), target_feature = "simd128"))]
    pub fn render_tile(&mut self, mut tile: Tile, fractal_zoom: f64) {
        // lets swap our texture to the tile
        let max_iter = self.max_iter;
        let to_ui = self.to_ui.sender();
        let bail_window = self.bail_window.clone();
        self.pool.execute(move || {
            if let Some(mut tile) = Self::check_bail_window(tile, &to_ui, bail_window){
                if fractal_zoom >2e-5 {
                    mandelbrot_f32_simd(&mut tile, max_iter);
                }
                else {
                    mandelbrot_f64_simd(&mut tile, max_iter);
                }
                to_ui.send(ToUI::TileDone {tile}).unwrap();
            }
        })
    }
 
    #[cfg(all(target_arch = "wasm32", not(target_feature = "simd128")))]
    pub fn render_tile(&mut self, mut tile: TextureTile, fractal_zoom: f64) {
        // lets swap our texture to the tile
        let max_iter = self.max_iter;
        let to_ui = self.to_ui.sender();
        let bail_window = self.bail_window.clone();
        self.pool.execute(move || {
            if let Some(mut tile) = Self::check_bail_window(tile, &to_ui, bail_window){
                mandelbrot_f64(&mut tile, max_iter);
                to_ui.send(ToUI::TileDone {tile}).unwrap();
            }
        })
    }
    
    pub fn generate_tiles_around(&mut self, cx: &mut Cx, zoom: f64, around: Vec2) {
        self.generate_tiles(
            cx,
            zoom,
            self.space.screen_to_fractal(zoom, self.space.center, around),
            self.space.view_rect_to_fractal(zoom, self.space.center),
            self.is_zoom_in
        );
    }
    
    pub fn generate_tiles(&mut self, cx: &mut Cx, zoom: f64, center: Vec2F64, space: RectF64, is_zoom_in: bool) {
        let size = vec2f64(zoom, zoom);
        
        let tile_cache = &mut self.tile_cache;
        
        if is_zoom_in && !tile_cache.next.is_empty() && tile_cache.next[0].fractal.size.x < 0.8 * zoom{
            tile_cache.discard_next_layer(cx);
        }
        else{
            tile_cache.discard_current_layer(cx);
        }
        
        tile_cache.next_zoom = zoom;
        
        let mut render_queue = Vec::new();
        let space = space.add_margin(size);
        // create a spiralling walk around the center point, usually your mouse
        Self::spiral_walk( | _step, i, j | {
            let fractal = RectF64 {
                pos: center + size * vec2f64(i as f64, j as f64) - 0.5 * size,
                size: size
            };
            if space.intersects(fractal) {
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
        
        if is_zoom_in {
            for mut tile in render_queue {
                self.render_tile(tile, zoom)
            }
        }
        else { // on zoom out reverse the spiral
            for mut tile in render_queue.into_iter().rev() {
                self.render_tile(tile, zoom)
            }
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> MandelbrotAction {
        self.state_handle_event(cx, event);
        
        if let Event::Signal(_) = event {
            // this batches up all the input signals
            self.next_frame = cx.new_next_frame();
        }
        
        if let Some(ne) = self.next_frame.triggered(event) {
            let mut too_many_textures = 0;
            while let Ok(msg) = self.to_ui.receiver.try_recv() {
                self.tile_cache.renders_in_queue -= 1;
                match msg {
                    ToUI::TileDone {mut tile} => {
                        tile.swap_buffer_with_texture(cx);
                        self.tile_cache.next.push(tile);
                        
                        // trigger a new tile render if we didn't render pixel accurate already
                        if self.tile_cache.renders_in_queue == 0 && self.tile_cache.next_zoom != self.space.zoom {
                            let zoom = self.space.zoom * if self.is_zooming {if self.is_zoom_in {0.8}else {2.0}}else {1.0};
                            self.generate_tiles_around(cx, zoom, self.finger_abs);
                        }
                        
                        too_many_textures += 1;
                        if too_many_textures > 10 {
                            break;
                        }
                    }
                    ToUI::TileBailed {mut tile} => {
                        self.tile_cache.empty.push(tile);
                    }
                }
            }
            // initial tile render
            if self.tile_cache.renders_in_queue == 0 && self.tile_cache.current.is_empty() {
                self.generate_tiles_around(cx, self.space.zoom, self.space.view_rect.center());
            }
            
            if self.is_zooming { // this only fires once the zoom is starting and the queue is emptying
                self.space.zoom_around(if self.is_zoom_in {0.98} else {1.02}, self.finger_abs);
            }
            
            // ok now the cycle.
            self.cycle = (ne.time * 0.2).fract() as f32;
            self.view.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
        
        match event.hits_with_options(cx, self.view.area(), HitOptions {use_multi_touch: true, margin: None}) {
            HitEvent::FingerDown(fe) => {
                self.is_zooming = true;
                if !fe.input_type.is_touch() || fe.digit == 0 {
                    self.finger_abs = fe.abs;
                }
                if fe.digit == 0 {
                    self.is_zoom_in = true;
                }
                else {
                    self.is_zoom_in = false;
                }
                // see if we need to kickstart a render
                if self.tile_cache.renders_in_queue == 0 {
                    let zoom = self.space.zoom * if self.is_zoom_in {0.8} else {2.0};
                    self.generate_tiles_around(cx, zoom, self.finger_abs);
                }
                self.view.redraw(cx);
                
                self.next_frame = cx.new_next_frame();
            },
            HitEvent::FingerMove(fe) => {
                if !fe.input_type.is_touch() || fe.digit == 0 {
                    self.finger_abs = fe.abs;
                }
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
        self.space.view_rect = cx.turtle().rect();
        
        *self.bail_window.lock().unwrap().borrow_mut() = BailWindow{
            is_zoom_in: self.is_zoom_in, 
            window:self.space.view_rect_to_fractal(self.space.zoom, self.space.center)
        };
        
        self.draw_tile.alpha = 1.0;
        self.draw_tile.max_iter = self.max_iter as f32;
        self.draw_tile.cycle = self.cycle;
        
        let tc = &mut self.tile_cache;
        
        for tile in tc.current.iter().chain(tc.next.iter()) {
            let rect = self.space.fractal_to_screen_rect(self.space.zoom, self.space.center, tile.fractal);
            self.draw_tile.draw_vars.set_texture(0, &tile.texture);
            self.draw_tile.draw_abs(cx, rect);
        }
        
        self.view.end(cx);
        
        Ok(())
    }
}