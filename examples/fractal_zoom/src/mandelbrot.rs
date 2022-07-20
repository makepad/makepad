#![allow(dead_code)]
#![allow(unused)]

use {
    std::simd::*,
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
}

pub struct TextureTile {
    buffer: Vec<u32>,
    texture: Texture,
    fractal: RectF64,
}

const TILE_SIZE_X: usize = 256;
const TILE_SIZE_Y: usize = 256;
const CACHE_MAX: usize = 300;

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
    
    fn discard_current(&mut self) {
        while let Some(item) = self.current.pop() {
            self.empty.push(item);
        }
        self.current_zoom = self.next_zoom;
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
        // preallocate buffers otherwise safari barfs in the worker
        let mut buffer = Vec::new();
        buffer.resize(TILE_SIZE_X * TILE_SIZE_Y, 0);
        texture.swap_image_u32(cx, &mut buffer);
        buffer.resize(TILE_SIZE_X * TILE_SIZE_Y, 0);
        Self {
            buffer,
            texture,
            fractal: RectF64::default()
        }
    }
}

#[derive(Default)]
pub struct FractalSpace {
    view_rect: Rect,
    tile_size: Vec2F64,
}

impl FractalSpace {
    fn fractal_to_view(&self, fractal_zoom: f64, fractal_center: Vec2F64, pos: Vec2F64) -> Vec2 {
        let view_center = self.view_rect.pos + self.view_rect.size * 0.5;
        return (((pos - fractal_center) / fractal_zoom) * self.tile_size).into_vec2() + view_center;
    }
    
    fn view_to_fractal(&self, fractal_zoom: f64, fractal_center: Vec2F64, pos: Vec2) -> Vec2F64 {
        let view_center = self.view_rect.pos + self.view_rect.size * 0.5;
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
    
    fn view_to_fractal_rect(&self, fractal_zoom: f64, fractal_center: Vec2F64, rect: Rect) -> RectF64 {
        let pos1 = self.view_to_fractal(fractal_zoom, fractal_center, rect.pos);
        let pos2 = self.view_to_fractal(fractal_zoom, fractal_center, rect.pos + rect.size);
        RectF64 {
            pos: pos1,
            size: pos2 - pos1
        }
    }
    
    fn view_fractal_rect(&self, fractal_zoom: f64, fractal_center: Vec2F64) -> RectF64 {
        self.view_to_fractal_rect(fractal_zoom, fractal_center, self.view_rect)
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

// simd constructor helpers to declog the code
fn f32x4v(a: f32, b: f32, c: f32, d: f32) -> f32x4 {f32x4::from_array([a, b, c, d])}
fn f32x4s(a: f32) -> f32x4 {f32x4::from_array([a; 4])}
fn m32x4s(a: bool) -> Mask::<i32, 4> {Mask::<i32, 4>::from_array([a; 4])}
fn u32x4v(a: u32, b: u32, c: u32, d: u32) -> u32x4 {u32x4::from_array([a, b, c, d])}
fn u32x4s(a: u32) -> u32x4 {u32x4::from_array([a; 4])}

fn f64x2v(a: f64, b: f64) -> f64x2 {f64x2::from_array([a, b])}
fn f64x2s(a: f64) -> f64x2 {f64x2::from_array([a; 2])}
fn m64x2s(a: bool) -> Mask::<i64, 2> {Mask::<i64, 2>::from_array([a; 2])}
fn u64x2s(a: u64) -> u64x2 {u64x2::from_array([a; 2])}
fn u64x2v(a: u64, b: u64) -> u64x2 {u64x2::from_array([a, b])}



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
    
    
    
    
    // Mandelbrot implementations
    
    
    
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
                let (iter, dist) = Self::mandelbrot_pixel_f64(max_iter, fp.x, fp.y);
                let dist = (dist * 256.0 + 127.0 * 255.0).max(0.0).min(65535.0) as u32;
                tile.buffer[y * TILE_SIZE_X + x] = iter as u32 | (dist << 16);
            }
        }
    }
    
    
    // 4 lane f32 
    fn mandelbrot_pixel_f32_simd(max_iter: u32, c_x: f32x4, c_y: f32x4) -> (u32x4, f32x4) {
        let mut x = c_x;
        let mut y = c_y;
        let mut dist_out = f32x4s(0.0);
        let mut iter_out = u32x4s(2);
        let mut exitted = m32x4s(false);
        for n in 0..max_iter {
            let xy = x * y;
            let xx = x * x;
            let yy = y * y;
            let dist = xx + yy;
            
            // using a mask, you can write parallel if/else code 
            let if_exit = dist.lanes_gt(f32x4s(4.0));
            let new_exit = (if_exit ^ exitted) & if_exit;
            exitted = exitted | new_exit;
            dist_out = new_exit.select(dist, dist_out);
            iter_out = new_exit.select(u32x4s(n), iter_out);
            if exitted.all() {
                return (iter_out, dist_out)
            }
            
            x = (xx - yy) + c_x;
            y = (xy + xy) + c_y;
        }
        return (iter_out, dist_out)
    }
    
    fn mandelbrot_f32_simd(tile: &mut TextureTile, max_iter: usize) {
        let tile_size = (f32x4s(TILE_SIZE_X as f32), f32x4s(TILE_SIZE_Y as f32));
        let fractal_pos = (f32x4s(tile.fractal.pos.x as f32), f32x4s(tile.fractal.pos.y as f32));
        let fractal_size = (f32x4s(tile.fractal.size.x as f32), f32x4s(tile.fractal.size.y as f32));
        
        for y in 0..TILE_SIZE_Y {
            for x in (0..TILE_SIZE_X).step_by(4) {
                let xf = x as f32;
                let tile_pos = (f32x4v(xf, xf + 1.0, xf + 2.0, xf + 3.0), f32x4s(y as f32));
                let fp_x = fractal_pos.0 + fractal_size.0 * tile_pos.0 / tile_size.0;
                let fp_y = fractal_pos.1 + fractal_size.1 * tile_pos.1 / tile_size.1;
                let (iter, dist) = Self::mandelbrot_pixel_f32_simd(max_iter as u32, fp_x, fp_y);
                let dist = (dist * f32x4s(255.0)) + f32x4s(127.0 * 255.0);
                let dist = dist.clamp(f32x4s(0.0), f32x4s(65535.0));
                let dist: u32x4 = dist.cast();
                for i in 0..4 {
                    tile.buffer[y * TILE_SIZE_X + x + i] = iter[i] as u32 | ((dist[i]) << 16);
                }
            }
        }
    }

    
    // 2 lane f64
    fn mandelbrot_pixel_f64_simd(max_iter: u64, c_x: f64x2, c_y: f64x2) -> (u64x2, f64x2) {
        let mut x = c_x;
        let mut y = c_y;
        let mut dist_out = f64x2s(0.0);
        let mut iter_out = u64x2s(2);
        let mut exitted = m64x2s(false);
        for n in 0..max_iter {
            let xy = x * y;
            let xx = x * x;
            let yy = y * y;
            let dist = xx + yy;
            
            let if_exit = dist.lanes_gt(f64x2s(4.0));
            let new_exit = (if_exit ^ exitted) & if_exit;
            exitted = exitted | new_exit;
            dist_out = new_exit.select(dist, dist_out);
            iter_out = new_exit.select(u64x2s(n), iter_out);
            if exitted.all() {
                return (iter_out, dist_out)
            }
            
            x = (xx - yy) + c_x;
            y = (xy + xy) + c_y;
        }
        return (iter_out, dist_out)
    }
    
    fn mandelbrot_f64_simd(tile: &mut TextureTile, max_iter: usize) {
        let tile_size = (f64x2s(TILE_SIZE_X as f64), f64x2s(TILE_SIZE_Y as f64));
        let fractal_pos = (f64x2s(tile.fractal.pos.x), f64x2s(tile.fractal.pos.y));
        let fractal_size = (f64x2s(tile.fractal.size.x), f64x2s(tile.fractal.size.y));
        // ok lets draw our mandelbrot f64
        for y in 0..TILE_SIZE_Y {
            for x in (0..TILE_SIZE_X).step_by(2) {
                let xf = x as f64;
                let tile_pos = (f64x2v(xf, xf + 1.0), f64x2s(y as f64));
                let fp_x = fractal_pos.0 + fractal_size.0 * tile_pos.0 / tile_size.0;
                let fp_y = fractal_pos.1 + fractal_size.1 * tile_pos.1 / tile_size.1;
                let (iter, dist) = Self::mandelbrot_pixel_f64_simd(max_iter as u64, fp_x, fp_y);
                let dist = (dist * f64x2s(255.0)) + f64x2s(127.0 * 255.0);
                let dist = dist.clamp(f64x2s(0.0), f64x2s(65535.0));
                let dist: u64x2 = dist.cast();
                for i in 0..2 {
                    tile.buffer[y * TILE_SIZE_X + x + i] = iter[i] as u32 | ((dist[i]) << 16) as u32;
                }
            }
        }
    }
    
    // 2 lane f64 antialiased
    fn mandelbrot_f64_simd_aa(tile: &mut TextureTile, max_iter: usize) {
        let tile_size = (f64x2s(TILE_SIZE_X as f64), f64x2s(TILE_SIZE_Y as f64));
        let fractal_pos = (f64x2s(tile.fractal.pos.x), f64x2s(tile.fractal.pos.y));
        let fractal_size = (f64x2s(tile.fractal.size.x), f64x2s(tile.fractal.size.y));
        // ok lets draw our mandelbrot f64
        for y in 0..TILE_SIZE_Y {
            for x in 0..TILE_SIZE_X {
                let xf = x as f64;
                let yf = y as f64;
                let tile_pos = (f64x2v(xf, xf + 0.5), f64x2s(yf));
                let fp_x = fractal_pos.0 + fractal_size.0 * tile_pos.0 / tile_size.0;
                let fp_y = fractal_pos.1 + fractal_size.1 * tile_pos.1 / tile_size.1;
                let (iter1, dist1) = Self::mandelbrot_pixel_f64_simd(max_iter as u64, fp_x, fp_y);
                let tile_pos = (f64x2v(xf, xf + 0.5), f64x2s(yf+0.5));
                let fp_x = fractal_pos.0 + fractal_size.0 * tile_pos.0 / tile_size.0;
                let fp_y = fractal_pos.1 + fractal_size.1 * tile_pos.1 / tile_size.1;
                let (iter2, dist2) = Self::mandelbrot_pixel_f64_simd(max_iter as u64, fp_x, fp_y);
                let iter = (iter1 + iter2).reduce_sum() / 4;
                let dist = (dist1 + dist2).reduce_sum() / 4.0;
                let dist = (dist * 256.0 + 127.0 * 255.0).max(0.0).min(65535.0) as u32;
                tile.buffer[y * TILE_SIZE_X + x] = iter as u32 | (dist << 16);
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
    
    pub fn mandelbrot_tile_generator(
        &mut self,
        fractal_zoom: f64,
        fractal_center: Vec2F64,
        fractal_rect: RectF64,
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
                tile_cache.discard_current();
                tile_cache.next_zoom = fractal_zoom;
            }
            false
        };
        
        let mut render_queue = Vec::new();
        
        Self::spiral_walk( | _step, i, j | {
            //let tile_pos =
            let fractal = RectF64 {
                pos: fractal_center + fractal_size * vec2f64(i as f64, j as f64) - 0.5 * fractal_size,
                size: fractal_size
            };
            if fractal_rect.intersects(fractal) {
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
        let max_iter = self.max_iter;
        if self.pool.is_none() {
            return;
        }
        if is_zoom_in {
            for mut tile in render_queue {
                let to_ui = self.to_ui.sender();
                self.pool.as_mut().unwrap().execute(move || {
                    if fractal_zoom >2e-5 {
                        Self::mandelbrot_f64_simd(&mut tile, max_iter);
                    }
                    else {
                        Self::mandelbrot_f64_simd(&mut tile, max_iter);
                    }
                    to_ui.send(ToUI::TileDone {tile, into_current}).unwrap();
                })
            }
        }
        else { // on zoom out reverse the spiral
            for mut tile in render_queue.into_iter().rev() {
                let to_ui = self.to_ui.sender();
                self.pool.as_mut().unwrap().execute(move || {
                    if fractal_zoom >2e-5 {
                        Self::mandelbrot_f32_simd(&mut tile, max_iter);
                    }
                    else {
                        Self::mandelbrot_f64_simd(&mut tile, max_iter);
                    }
                    to_ui.send(ToUI::TileDone {tile, into_current}).unwrap();
                })
            }
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> MandelbrotAction {
        self.state_handle_event(cx, event);
        if let Some(ne) = self.next_frame.triggered(event) {
            if self.tile_cache.renders_in_queue == 0 && self.tile_cache.current.is_empty() {
                self.mandelbrot_tile_generator(
                    self.fractal_zoom,
                    self.fractal_center,
                    self.space.view_fractal_rect(self.fractal_zoom, self.fractal_center),
                    true
                );
            }
            if self.is_zooming {
                self.zoom_around(if self.is_zoom_in {0.98} else {1.02}, self.finger_abs);
                if self.tile_cache.renders_in_queue == 0 && (
                    self.is_zoom_in && self.fractal_zoom < self.tile_cache.next_zoom ||
                    !self.is_zoom_in && self.fractal_zoom > self.tile_cache.next_zoom
                ) {
                    let zoom = self.fractal_zoom * if self.is_zoom_in {0.5} else {2.0};
                    self.mandelbrot_tile_generator(
                        zoom,
                        self.space.view_to_fractal(zoom, self.fractal_center, self.finger_abs),
                        self.space.view_fractal_rect(zoom, self.fractal_center),
                        self.is_zoom_in
                    );
                }
                self.view.redraw(cx);
            }
            // ok now the cycle.
            self.view.redraw(cx);
            self.cycle = (ne.time * 0.2).fract() as f32;
            self.next_frame = cx.new_next_frame();
        }
        
        while let Ok(msg) = self.to_ui.try_recv(event) {
            let ToUI::TileDone {mut tile, into_current} = msg;
            self.tile_cache.renders_in_queue -= 1;
            tile.texture.swap_image_u32(cx, &mut tile.buffer);
            if into_current {
                self.tile_cache.current.push(tile);
            }
            else {
                self.tile_cache.next.push(tile);
            }
            // ok so we should compute which tiles to retire
            // tiles that are outside of our viewport for instance
            // or tiles that are overlapped completely.
            if self.tile_cache.renders_in_queue == 0 && self.tile_cache.next_zoom != self.fractal_zoom {
                self.mandelbrot_tile_generator(
                    self.fractal_zoom,
                    self.space.view_to_fractal(self.fractal_zoom, self.fractal_center, self.finger_abs),
                    self.space.view_fractal_rect(self.fractal_zoom, self.fractal_center),
                    self.is_zoom_in
                );
            }
            self.view.redraw(cx);
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
        self.draw_mandelbrot.alpha = 1.0;
        self.draw_mandelbrot.max_iter = self.max_iter as f32;
        self.draw_mandelbrot.cycle = self.cycle;
        // alright so. we have
        let tc = &mut self.tile_cache;
        for tile in &tc.current {
            // transform fractalspace to screen space
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