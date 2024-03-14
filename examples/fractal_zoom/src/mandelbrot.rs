use {
    crate::{
        makepad_widgets::*,
        mandelbrot_simd::*
    }
};

// Our live DSL to define the shader and UI def
live_design!{
    // include shader standard library with the Pal object
    import makepad_draw::shader::std::*;
    
    // the shader to draw the texture tiles
    DrawTile = {{DrawTile}} {
        texture tex: texture2d
        fn pixel(self) -> vec4 {
            //return vec4(self.max_iter / 1000.0,0.0,0.0,1.0);
            let fractal = sample2d(self.tex, self.pos)
            
            // unpack iteration and magnitude squared from our u32 buffer
            let iter = fractal.y * 65535 + fractal.z * 255;
            let magsq = (fractal.w * 256 + fractal.x - 127);
            
            // create a nice palette index
            let index = abs((1.0 * iter / self.max_iter * 18) - .01 * log(magsq));
            // if the iter > max_iter we return black
            if iter > self.max_iter {
                return vec4(0, 0, 0, 1.0);
            }
            // fetch a color using iq2 (inigo quilez' shadertoy palette #2)
            
            return vec4(Pal::iq2(index - self.color_cycle*-1.0),1);
            
        }
    }
    
    Mandelbrot = {{Mandelbrot}} {
        max_iter: 256,
    }
}

pub const TILE_SIZE_X: usize = 256;
pub const TILE_SIZE_Y: usize = 256;
pub const TILE_CACHE_SIZE: usize = 3500;
// the shader struct used to draw

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawTile {
    // this shader structs inherits from the super class DrawQuad
    // the shader compiler allows a form of inheritance where you
    // define a 'draw_super' field, which projects all values in the chain
    // onto the 'self' property in the shader. This is useful to partially reuse shadercode.
    #[deref] draw_super: DrawQuad,
    // max iterations of the mandelbrot fractal
    #[live] max_iter: f32,
    // a value that cycles the color in the palette (0..1)
    #[live] color_cycle: f32
}

// basic plain f64 loop, not called in SIMD mode.
// Returns the iteration count when the loop goes to infinity,
// and the squared magnitude of the complex number at the time of exit
// you can use this number to create the nice color bands you see in the output
// For a more detailed description, see mandelbrot explanations online
#[allow(dead_code)]
fn mandelbrot_pixel_f64(max_iter: usize, c_x: f64, c_y: f64) -> (usize, f64) {
    let mut x = c_x;
    let mut y = c_y;
    let mut magsq = 0.0;
    for n in 0..max_iter {
        let xy = x * y;
        let xx = x * x;
        let yy = y * y;
        magsq = xx + yy;
        if magsq > 4.0 {
            return (n, magsq)
        }
        x = (xx - yy) + c_x;
        y = (xy + xy) + c_y;
    }
    return (max_iter, magsq)
}

#[allow(dead_code)]
fn mandelbrot_f64(tile: &mut Tile, max_iter: usize) {
    let tile_size = dvec2(TILE_SIZE_X as f64, TILE_SIZE_Y as f64);
    for y in 0..TILE_SIZE_Y {
        for x in 0..TILE_SIZE_X {
            let fp = tile.fractal.pos + tile.fractal.size * (dvec2(x as f64, y as f64) / tile_size);
            let (iter, dist) = mandelbrot_pixel_f64(max_iter, fp.x, fp.y);
            let dist = ((dist + 127.0) * 256.0).max(0.0).min(65535.0) as u32;
            tile.buffer[y * TILE_SIZE_X + x] = iter as u32 | (dist << 16);
        }
    }
}

pub struct Tile {
    // the memory buffer thats used when a tile is rendered
    pub buffer: Vec<u32>,
    // the makepad system texture backing the tile, when ready for drawing 'buffer' is swapped onto it
    pub texture_index: usize,
    // the fractal space rectangle that this tile represents
    pub fractal: Rect,
}

impl Tile {
    fn new(texture_index: usize) -> Self {
        let mut buffer = Vec::new();
        buffer.resize(TILE_SIZE_X * TILE_SIZE_Y, 0);
        Self {
            buffer,
            texture_index,
            fractal: Rect::default()
        }
    }
}

// used to last minute test if a tile is to be discarded by a worker
#[derive(Clone, Default)]
pub struct BailTest {
    // the position of our viewport in fractal space
    space: Rect,
    // if zooming in, the bail-test is wether a tile is outside of the view
    // if zooming out if a tile uses less than X percentage of the view is.
    is_zoom_in: bool
}

pub struct TileCache {
    textures: Vec<Texture>,
    // the current layer of tiles
    current: Vec<Tile>,
    // next layer of tiles
    next: Vec<Tile>,
    // the empty tilecache we can render to from workers
    empty: Vec<Tile>,
    current_zoom: f64,
    next_zoom: f64,
    tiles_in_flight: usize,
    
    // this holds a Wasm compatible threadpool
    thread_pool: MessageThreadPool<BailTest>,
}

impl TileCache {
    fn new(cx: &mut Cx) -> Self {
        let mut empty = Vec::new();
        let mut textures = Vec::new();
        for i in 0..TILE_CACHE_SIZE {
            empty.push(Tile::new(i));
            
            let texture = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
                data: vec![],
                width: TILE_SIZE_X,
                height: TILE_SIZE_Y,
            });
            textures.push(texture);
        }
        // preallocate buffers otherwise safari barfs in the worker
        let use_cores = cx.cpu_cores().max(3) - 2;
        Self {
            textures,
            current: Vec::new(),
            next: Vec::new(),
            empty,
            current_zoom: 0.0,
            next_zoom: 0.0,
            tiles_in_flight: 0,
            thread_pool: MessageThreadPool::new(cx, use_cores),
        }
    }
    
    fn tile_completed(&mut self, cx: &mut Cx, mut tile: Tile) {
        self.tiles_in_flight -= 1;
        self.textures[tile.texture_index].swap_vec_u32(cx, &mut tile.buffer);
        self.next.push(tile)
    }
    
    fn tile_bailed(&mut self, tile: Tile) {
        self.tiles_in_flight -= 1;
        self.empty.push(tile);
    }
    
    fn set_bail_test(&self, bail_test: BailTest) {
        self.thread_pool.send_msg(bail_test);
    }
    
    fn tile_needs_to_bail(tile: &Tile, bail_window: Option<BailTest >) -> bool {
        if let Some(bail) = bail_window {
            if bail.is_zoom_in {
                if !tile.fractal.intersects(bail.space) {
                    return true
                }
            }
            else { // compare the size of the bail window against the tile
                //if tile.fractal.size.x * tile.fractal.size.y < bail.space.size.x * bail.space.size.y * 0.007 {
                //    return true
                //}
            }
        }
        false
    }
    
    fn generate_completed(&self) -> bool {
        self.tiles_in_flight == 0
    }
    
    fn discard_next_layer(&mut self, cx: &mut Cx) {
        while let Some(mut tile) = self.next.pop() {
            self.textures[tile.texture_index].swap_vec_u32(cx, &mut tile.buffer);
            self.empty.push(tile);
        }
    }
    
    fn discard_current_layer(&mut self, cx: &mut Cx) {
        while let Some(mut tile) = self.current.pop() {
            self.textures[tile.texture_index].swap_vec_u32(cx, &mut tile.buffer);
            self.empty.push(tile);
        }
        self.current_zoom = self.next_zoom;
        std::mem::swap(&mut self.current, &mut self.next);
    }
    
    // generates a queue
    pub fn generate_tasks_and_flip_layers(&mut self, cx: &mut Cx, zoom: f64, center: DVec2, window: Rect, is_zoom_in: bool) -> Vec<Tile> {
        let size = dvec2(zoom, zoom);
        
        // discard the next layer if we don't fill the screen yet at this point and reuse old
        if is_zoom_in && !self.next.is_empty() && self.next[0].fractal.size.x < 0.8 * zoom {
            self.discard_next_layer(cx);
        }
        else {
            self.discard_current_layer(cx);
        }
        
        self.next_zoom = zoom;
        
        let mut render_tasks = Vec::new();
        let window = window.add_margin(size);
        
        Self::spiral_walk( | _step, i, j | {
            let fractal = Rect {
                pos: center + size * dvec2(i as f64, j as f64) - 0.5 * size,
                size: size
            };
            if window.intersects(fractal) {
                if let Some(mut tile) = self.empty.pop() {
                    tile.fractal = fractal;
                    render_tasks.push(tile);
                }
                true
            }
            else {
                false
            }
        });
        
        self.tiles_in_flight = render_tasks.len();
        render_tasks
    }
    
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
                if dj == 0 {
                    intersect_step += 1;
                    // cover the case that a spiral-edge step up does not match
                    // a complete circle
                    if intersect_step > 2 {
                        // at the end of a circular walk
                        // we check if we had any intersections with the viewport.
                        // (the closure returned true)
                        // ifso we keep spiralling
                        // otherwise we are done
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
}

// the worker threads send the UI thread this message
// the tile is passed by value, meaning the ownership returns to the UI thread via this path
pub enum ToUI {
    TileDone {tile: Tile},
    TileBailed {tile: Tile},
}

// Space transforms from view (screen) to fractal and back
#[derive(Default, Clone)]
pub struct FractalSpace {
    // the rectangle of the viewport on screen
    view_rect: Rect,
    // the size of the tile in fractal space
    tile_size: DVec2,
    // the center of the fractal space
    center: DVec2,
    // the zoomfactor in the fractal space
    zoom: f64,
}

impl FractalSpace {
    fn new(center: DVec2, zoom: f64) -> Self {
        Self {
            center,
            zoom,
            ..Self::default()
        }
    }
    
    // constructs a copy of self with other zoom/center values
    fn other(&self, other_zoom: f64, other_center: DVec2) -> Self {
        Self {
            center: other_center,
            zoom: other_zoom,
            ..self.clone()
        }
    }
    
    fn fractal_to_screen(&self, pos: DVec2) -> DVec2 {
        let view_center = self.view_rect.pos + self.view_rect.size * 0.5;
        return (((pos - self.center) / self.zoom) * self.tile_size) + view_center;
    }
    
    fn screen_to_fractal(&self, pos: DVec2) -> DVec2 {
        let view_center = self.view_rect.pos + self.view_rect.size * 0.5;
        return (((pos - view_center) / self.tile_size) * self.zoom) + self.center;
    }
    
    fn fractal_to_screen_rect(&self, rect: Rect) -> Rect {
        let pos1 = self.fractal_to_screen(rect.pos);
        let pos2 = self.fractal_to_screen(rect.pos + rect.size);
        Rect {
            pos: pos1,
            size: pos2 - pos1
        }
    }
    
    fn screen_to_fractal_rect(&self, rect: Rect) -> Rect {
        let pos1 = self.screen_to_fractal(rect.pos);
        let pos2 = self.screen_to_fractal(rect.pos + rect.size);
        Rect {
            pos: pos1,
            size: pos2 - pos1
        }
    }
    
    // this zooms the fractal space around a point on the screen
    fn zoom_around(&mut self, factor: f64, around: DVec2) {
        // hold on to the current position in fractal space
        let fpos1 = self.screen_to_fractal(around);
        self.zoom *= factor;
        if self.zoom < 5e-14f64 { // maximum zoom for f64
            self.zoom = 5e-14f64;
        }
        if self.zoom > 2.0 { // don't go too far out
            self.zoom = 2.0;
        }
        let fpos2 = self.screen_to_fractal(around);
        // by comparing the position in fractal space before and after the zoomstep
        // we can move the center so it stays in the same spot
        self.center += fpos1 - fpos2;
    }
    
    // self.view_rect in fractal space
    fn view_rect_to_fractal(&self) -> Rect {
        self.screen_to_fractal_rect(self.view_rect)
    }
}


#[derive(Live, Widget)]
pub struct Mandelbrot {
    // DSL accessible
    #[live] draw_tile: DrawTile,
    #[live] max_iter: usize,
    
    // thew view container that contains our mandelbrot UI
    #[redraw] #[rust] view_area: Area,
    
    #[walk] walk: Walk,
    // prepending #[rust] makes derive(Live) ignore these fields
    // and they dont get DSL accessors
    #[rust] next_frame: NextFrame,
    // where your finger/mouse was when moved
    #[rust] finger_abs: DVec2,
    // set to true when the fractal is actively zoom animating
    #[rust] is_zooming: bool,
    
    // this bool flips wether or not you were zooming in or out
    // used to decide tile generation strategy
    #[rust(true)] is_zoom_in: bool,
    
    // default fractal space for looking at a mandelbrot
    #[rust(FractalSpace::new(dvec2(-0.5, 0.0), 0.5))] space: FractalSpace,
    
    #[rust]had_first_draw: bool,
    
    // the tilecache holding all the tiles
    #[rust(TileCache::new(cx))]
    tile_cache: TileCache,
    
    // the channel that can transmit message to the UI from workers
    #[rust] to_ui: ToUIReceiver<ToUI>,
}

impl LiveHook for Mandelbrot {
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        // starts the animation cycle on startup
        self.next_frame = cx.new_next_frame();
    }
}

#[derive(Clone, DefaultNone)]
pub enum MandelbrotAction {
    None
}

impl Widget for Mandelbrot {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope){
        if let Some(ne) = self.next_frame.is_event(event) {
            // If we don't have a current layer, initiate the first tile render on the center of the screen
            if self.had_first_draw && self.tile_cache.generate_completed() && self.tile_cache.current.is_empty() {
                self.generate_tiles_around_finger(cx, self.space.zoom, self.space.view_rect.center());
            }
                        
            // try pulling tiles from our message channel from the worker threads
            let mut tiles_received = 0;
            while let Ok(msg) = self.to_ui.receiver.try_recv() {
                match msg {
                    ToUI::TileDone {tile} => {
                        self.tile_cache.tile_completed(cx, tile);
                                                
                        // when we have all the tiles, and aren't pixel accurate, fire a new tile render
                        // this is the common path for initiating a tile render
                        if self.tile_cache.generate_completed() && self.tile_cache.next_zoom != self.space.zoom {
                            let zoom = self.space.zoom * if self.is_zooming {if self.is_zoom_in {0.8}else {2.0}}else {1.0};
                            self.generate_tiles_around_finger(cx, zoom, self.finger_abs);
                        }
                                                
                        tiles_received += 1;
                        // dont process too many tiles at once as this hiccups the renderer
                        if tiles_received > 10 {
                            break;
                        }
                    }
                    ToUI::TileBailed {tile} => {
                        self.tile_cache.tile_bailed(tile);
                    }
                }
            }
                        
            // We are zooming, so animate the zoom
            if self.is_zooming {
                if let OsType::LinuxDirect = cx.os_type() {
                    self.space.zoom_around(if self.is_zoom_in {0.92} else {1.08}, self.finger_abs);
                }
                else {
                    self.space.zoom_around(if self.is_zoom_in {0.98} else {1.02}, self.finger_abs);
                }
                // this kickstarts the tile cache generation when zooming, only happens once per zoom
                if self.tile_cache.generate_completed() {
                    let zoom = self.space.zoom * if self.is_zoom_in {0.8} else {2.0};
                    self.generate_tiles_around_finger(cx, zoom, self.finger_abs);
                }
            }
                        
            // animate color cycle
            self.draw_tile.color_cycle = (ne.time * 0.05).fract() as f32;
            // this triggers a draw_walk call and another 'next frame' event
            self.view_area.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
                
        // check if we click/touch the mandelbrot view in multitouch mode
        // in this mode we get fingerdown events for each finger.
        
        match event.hits(cx, self.view_area) {
            Hit::FingerDown(fe) => {
                // ok so we get multiple finger downs
                self.is_zooming = true;
                self.finger_abs = fe.abs;
                self.is_zoom_in = true;
                cx.set_key_focus(self.view_area);
                self.view_area.redraw(cx);
                                            
                self.next_frame = cx.new_next_frame();
            },
            Hit::KeyDown(k)=>{
                if KeyCode::Space == k.key_code{
                    self.space.zoom = 0.5;
                    self.space.center = dvec2(-0.5,0.0);
                }
            }
            Hit::FingerMove(fe) => {
            //if fe.digit.index == 0 { // only respond to digit 0
                self.finger_abs = fe.abs;
                //}
            }
            Hit::FingerUp(_) => {
                self.is_zoom_in = true;
                self.is_zooming = false;
            }
            _ => ()
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::flow_right());
        // lets check our clip
                
        self.had_first_draw = true;
        // store the view information here as its the only place it's known in the codeflow
        self.space.tile_size = dvec2(TILE_SIZE_X as f64, TILE_SIZE_Y as f64) / cx.current_dpi_factor() as f64;
        self.space.view_rect = cx.turtle().rect();
                
        // update bail window the workers check to skip tiles that are no longer in view
        self.tile_cache.set_bail_test(BailTest {
            is_zoom_in: self.is_zoom_in,
            space: self.space.view_rect_to_fractal()
        });
                
        // pass the max_iter value to the shader
        self.draw_tile.max_iter = self.max_iter as f32;
                
        // iterate the current and next tile caches and draw the fractal tile
        for tile in self.tile_cache.current.iter().chain(self.tile_cache.next.iter()) {
            let rect = self.space.fractal_to_screen_rect(tile.fractal);
            // set texture by index.
            self.draw_tile.draw_vars.set_texture(0, &self.tile_cache.textures[tile.texture_index]);
                        
            // this emits the drawcall onto the drawlists that go to the renderbackend
            // By changing the texture every time we cause to emit multiple drawcalls.
            // if we wouldn't change the texture, it would batch all draws into one instanced array.
            self.draw_tile.draw_abs(cx, rect);
        }
                
        cx.end_turtle_with_area(&mut self.view_area);
        DrawStep::done()
    }
}

impl Mandelbrot {
    
    // the SIMD tile rendering, uses the threadpool to draw the tile
    
    //#[cfg(feature = "nightly")]
    pub fn render_tile(&mut self, mut tile: Tile, fractal_zoom: f64, is_zooming: bool) {
        let max_iter = self.max_iter;
        // we pull a cloneable sender from the to_ui message channel for the worker
        let to_ui = self.to_ui.sender();
        
        self.tile_cache.thread_pool.execute(move | bail_test | {
            if TileCache::tile_needs_to_bail(&tile, bail_test) {
                return to_ui.send(ToUI::TileBailed {tile}).unwrap();
            }
            
            if !is_zooming {
                mandelbrot_f64x2_4xaa(&mut tile, max_iter);
            }
            else
            if fractal_zoom >2e-5 {
                // we can use a f32x4 path when we aren't zoomed in far (2x faster)
                // as f32 has limited zoom-depth it can support
                mandelbrot_f32x4(&mut tile, max_iter);
            }
            else {
                // otherwise we use a higher resolution f64
                mandelbrot_f64x2(&mut tile, max_iter);
            }
            to_ui.send(ToUI::TileDone {tile}).unwrap();
        })
    }
    
    // Normal tile rendering, uses the threadpool to draw the tile
    /*#[cfg(not(feature = "nightly"))]
    pub fn render_tile(&mut self, mut tile: Tile, _fractal_zoom: f64, _is_zooming: bool) {
        let max_iter = self.max_iter;
        // we pull a cloneable sender from the to_ui message channel for the worker
        let to_ui = self.to_ui.sender();
        // this is run on any one of our worker threads that's free
        self.tile_cache.thread_pool.execute(move | bail_test | {
            if TileCache::tile_needs_to_bail(&tile, bail_test) {
                return to_ui.send(ToUI::TileBailed {tile}).unwrap();
            }
            // use the non SIMD mandelbrot. This path is used on safari
            mandelbrot_f64(&mut tile, max_iter);
            to_ui.send(ToUI::TileDone {tile}).unwrap();
        })
    }*/
    
    pub fn generate_tiles_around_finger(&mut self, cx: &mut Cx, zoom: f64, finger: DVec2) {
        self.generate_tiles(
            cx,
            zoom,
            self.space.other(zoom, self.space.center).screen_to_fractal(finger),
            self.space.other(zoom, self.space.center).view_rect_to_fractal(),
            self.is_zoom_in,
            self.is_zooming
        );
    }
    
    // generates the tiles and emits them in the right spiral order
    pub fn generate_tiles(&mut self, cx: &mut Cx, zoom: f64, center: DVec2, window: Rect, is_zoom_in: bool, is_zooming: bool) {
        let render_tasks = self.tile_cache.generate_tasks_and_flip_layers(cx, zoom, center, window, is_zoom_in);
        if is_zoom_in {
            for tile in render_tasks {
                self.render_tile(tile, zoom, is_zooming)
            }
        }
        else { // on zoom out reverse the spiral compared to zoom_in
            for tile in render_tasks.into_iter().rev() {
                self.render_tile(tile, zoom, is_zooming)
            }
        }
    }
}