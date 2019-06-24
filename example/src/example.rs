/// This is Makepad, a work-in-progress livecoding IDE for 2D Design.
// This application is nearly 100% Wasm running on webGL. NO HTML AT ALL.
// The vision is to build a livecoding / design hybrid program,
// here procedural design and code are fused in one environment.
// If you have missed 'learnable programming' please check this out:
// http://worrydream.com/LearnableProgramming/
// Makepad aims to fulfill (some) of these ideas using a completely
// from-scratch renderstack built on the GPU and Rust/wasm.
// It will be like an IDE meets a vector designtool, and had offspring.
// Direct manipulation of the vectors modifies the code, the code modifies the vectors.
// And the result can be lasercut, embroidered or drawn using a plotter.
// This means our first goal is 2D path CNC with booleans (hence the CAD),
// and not dropshadowy-gradients.

// Find the repo and more explanation at github.com/makepad/makepad.
// We are developing the UI kit and code-editor as MIT, but the full stack
// will be a cloud/native app product in a few months.

// However to get to this amazing mixed-mode code editing-designtool,
// we first have to build an actually nice code editor (what you are looking at)
// And a vector stack with booleans (in progress)
// Up next will be full multiplatform support and more visual UI.
// All of the code is written in Rust, and it compiles to native and Wasm
// Its built on a novel immediate-mode UI architecture
// The styling is done with shaders written in Rust, transpiled to metal/glsl

// for now enjoy how smooth a full GPU editor can scroll (try select scrolling the edge)
// Also the tree fold-animates and the docking panel system works.
// Multi cursor/grid cursor also works with ctrl+click / ctrl+shift+click
// press alt or escape for animated codefolding outline view!

use render::*;
use widget::*;
pub mod capture_winmf;
pub use crate::capture_winmf::*;
pub mod gamepad_winxinput;
pub use crate::gamepad_winxinput::*;
use core::arch::x86_64::*;
use std::sync::mpsc;

#[derive(Default, Clone, PartialEq)]
struct MandelbrotLoc {
    zoom: f64,
    center_x: f64,
    center_y: f64
}

#[derive(Default)]
struct Mandelbrot {
    start_loc: MandelbrotLoc,
    texture: Texture,
    num_threads: usize,
    width: usize,
    height: usize,
    frame_signal: Signal,
    sender: Option<mpsc::Sender<(usize, MandelbrotLoc)>>,
}

impl Style for Mandelbrot {
    fn style(_cx: &mut Cx) -> Self {
        Self {
            start_loc: MandelbrotLoc {
                zoom: 1.0,
                center_x: -1.5,
                center_y: 0.0
            },
            texture: Texture::default(),
            num_threads: 30,
            width: 3840,
            height: 2160,
            frame_signal: Signal::empty(),
            sender: None,
        }
    }
}
impl Mandelbrot {
    fn init(&mut self, cx: &mut Cx) {
        // lets start a mandelbrot thread that produces frames
        self.frame_signal = cx.new_signal();
        self.texture.set_desc(cx, Some(TextureDesc {
            format: TextureFormat::MappedRGf32,
            width: Some(self.width),
            height: Some(self.height),
            multisample: None
        }));
        
        unsafe fn calc_mandel_avx2(c_x: __m256d, c_y: __m256d, max_iter: u32) -> (__m256d, __m256d) {
            let mut x = c_x;
            let mut y = c_y;
            let mut count = _mm256_set1_pd(0.0);
            let mut merge_sum = _mm256_set1_pd(0.0);
            let add = _mm256_set1_pd(1.0);
            let max_dist = _mm256_set1_pd(4.0);
            
            for _ in 0..max_iter as usize {
                let xy = _mm256_mul_pd(x, y);
                let xx = _mm256_mul_pd(x, x);
                let yy = _mm256_mul_pd(y, y);
                let sum = _mm256_add_pd(xx, yy);
                let mask = _mm256_cmp_pd(sum, max_dist, _CMP_LT_OS);
                let mask_u32 = _mm256_movemask_pd(mask);
                if mask_u32 == 0 { // is a i8
                    return (_mm256_div_pd(count, _mm256_set1_pd(max_iter as f64)), _mm256_sqrt_pd(merge_sum));
                }
                merge_sum = _mm256_or_pd(_mm256_and_pd(sum, mask), _mm256_andnot_pd(mask, merge_sum));
                count = _mm256_add_pd(count, _mm256_and_pd(add, mask));
                x = _mm256_add_pd(_mm256_sub_pd(xx, yy), c_x);
                y = _mm256_add_pd(_mm256_add_pd(xy, xy), c_y);
            }
            return (_mm256_set1_pd(2.0), merge_sum);
        }
        
        // lets spawn fractal.height over 32 threads
        let num_threads = self.num_threads;
        let width = self.width;
        let height = self.height;
        let fwidth = self.width as f64;
        let fheight = self.height as f64;
        let chunk_height = height / num_threads;
        
        // stuff that goes into the threads
        let mut thread_pool = scoped_threadpool::Pool::new(self.num_threads as u32);
        let frame_signal = self.frame_signal.clone();
        let mut cxthread = cx.new_cxthread();
        let texture = self.texture.clone();
        let mut loc = self.start_loc.clone();
        let mut re_render = true;
        let (tx, rx) = mpsc::channel();
        self.sender = Some(tx);
        std::thread::spawn(move || {
            let mut user_data = 0;
            loop {
                while let Ok((recv_user_data, new_loc)) = rx.try_recv() {
                    user_data = recv_user_data;
                    if loc != new_loc {
                        re_render = true;
                    }
                    loc = new_loc;
                }
                //zoom = zoom / 1.003;
                //if zoom < 0.000000000002 {
                //    zoom = 1.0;
                // }
                //let time1 = Cx::profile_time_ns();
                if re_render {
                    thread_pool.scoped( | scope | {
                        if let Some(mapped_texture) = cxthread.lock_mapped_texture_f32(&texture, user_data) {
                            let mut iter = mapped_texture.chunks_mut((chunk_height * width * 2) as usize);
                            for i in 0..num_threads {
                                let thread_num = i;
                                let slice = iter.next().unwrap();
                                let loc = loc.clone();
                                //println!("{}", chunk_height);
                                scope.execute(move || {
                                    let it_v = [0f64, 0f64, 0f64, 0f64];
                                    let su_v = [0f64, 0f64, 0f64, 0f64];
                                    let start = thread_num * chunk_height as usize;
                                    for y in (start..(start + chunk_height)).step_by(2) {
                                        for x in (0..width).step_by(2) {
                                            unsafe {
                                                // TODO simd these things too
                                                let c_re = _mm256_set_pd(
                                                    ((x as f64 - fwidth * 0.5) * 4.0 / fwidth) * loc.zoom + loc.center_x,
                                                    (((x + 1) as f64 - fwidth * 0.5) * 4.0 / fwidth) * loc.zoom + loc.center_x,
                                                    ((x as f64 - fwidth * 0.5) * 4.0 / fwidth) * loc.zoom + loc.center_x,
                                                    (((x + 1) as f64 - fwidth * 0.5) * 4.0 / fwidth) * loc.zoom + loc.center_x
                                                );
                                                let c_im = _mm256_set_pd(
                                                    ((y as f64 - fheight * 0.5) * 3.0 / fheight) * loc.zoom + loc.center_y,
                                                    ((y as f64 - fheight * 0.5) * 3.0 / fheight) * loc.zoom + loc.center_y,
                                                    (((y + 1) as f64 - fheight * 0.5) * 3.0 / fheight) * loc.zoom + loc.center_y,
                                                    (((y + 1) as f64 - fheight * 0.5) * 3.0 / fheight) * loc.zoom + loc.center_y
                                                );
                                                let (it256, sum256) = calc_mandel_avx2(c_re, c_im, 2048);
                                                _mm256_store_pd(it_v.as_ptr(), it256);
                                                _mm256_store_pd(su_v.as_ptr(), sum256);
                                                
                                                slice[(x * 2 + (y - start) * width * 2) as usize] = it_v[3] as f32;
                                                slice[(x * 2 + 2 + (y - start) * width * 2) as usize] = it_v[2] as f32;
                                                slice[(x * 2 + (1 + y - start) * width * 2) as usize] = it_v[1] as f32;
                                                slice[(x * 2 + 2 + (1 + y - start) * width * 2) as usize] = it_v[0] as f32;
                                                slice[1 + (x * 2 + (y - start) * width * 2) as usize] = su_v[3] as f32;
                                                slice[1 + (x * 2 + 2 + (y - start) * width * 2) as usize] = su_v[2] as f32;
                                                slice[1 + (x * 2 + (1 + y - start) * width * 2) as usize] = su_v[1] as f32;
                                                slice[1 + (x * 2 + 2 + (1 + y - start) * width * 2) as usize] = su_v[0] as f32;
                                            }
                                        }
                                    }
                                })
                            }
                            re_render = false;
                        }
                        else { // wait a bit
                            re_render = true;
                            std::thread::sleep(std::time::Duration::from_millis(1));
                            return
                        }
                    });
                    cxthread.unlock_mapped_texture(&texture);
                    //println!("{}", (Cx::profile_time_ns()-time1) as f64/1000.);
                    Cx::send_signal(frame_signal, 0);
                }
                else {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                
            }
        });
    }
    
    fn handle_signal(&mut self, _cx: &mut Cx, event: &Event) -> bool {
        if let Event::Signal(se) = event {
            if self.frame_signal.is_signal(se) { // we haz new texture
                return true
            }
        }
        false
    }
    
    fn send_new_loc(&mut self, index: usize, new_loc: MandelbrotLoc) {
        if let Some(sender) = &self.sender {
            let _ = sender.send((index, new_loc));
        }
    }
}

struct App {
    view: View<ScrollBar>,
    window: DesktopWindow,
    mandel_blit: Blit,
    frame: f32,
    palette: f32,
    palette_scale: f32,
    outer_scale: f32,
    capture: Capture,
    gamepad: Gamepad,
    current_loc: MandelbrotLoc,
    loc_history: Vec<MandelbrotLoc>,
    mandel: Mandelbrot,
    color_offset: f32,
}

main_app!(App, "Example");

impl Style for App {
    fn style(cx: &mut Cx) -> Self {
        set_dark_style(cx);
        Self {
            current_loc: MandelbrotLoc {
                zoom: 1.0,
                center_x: -1.5,
                center_y: 0.0
            },
            color_offset: 0.0,
            palette: 1.0,
            palette_scale: 1.0,
            outer_scale: 1.0,
            loc_history: Vec::new(),
            mandel: Mandelbrot::style(cx),
            capture: Capture::default(),
            gamepad: Gamepad::default(),
            window: DesktopWindow::style(cx),
            view: View {
                is_overlay: true,
                scroll_h: Some(ScrollBar::style(cx)),
                scroll_v: Some(ScrollBar::style(cx)),
                ..Style::style(cx)
            },
            mandel_blit: Blit {
                shader: cx.add_shader(App::def_blit_shader(), "App.blit"),
                ..Blit::style(cx)
            },
            frame: 0.
        }
    }
}

impl App {
    fn def_blit_shader() -> ShaderGen {
        Blit::def_blit_shader().compose(shader_ast!({
            let texcam: texture2d<Texture>;
            let time: float<Uniform>;
            
            let scale_delta: float<Uniform>;
            let offset_delta: vec2<Uniform>;
            let color_offset: float<Uniform>;
            let palette: float<Uniform>;
            let palette_scale: float<Uniform>;
            let outer_scale: float<Uniform>;
            
            fn kaleido(uv: vec2, angle: float, base: float, spin: float) -> vec2 {
                let a = atan(uv.y, uv.x);
                if a<0. {a = PI + a}
                let d = length(uv);
                a = abs(fmod (a + spin, angle * 2.0) - angle);
                return vec2(abs(sin(a + base)) * d, abs(cos(a + base)) * d);
            }
            
            fn pixel() -> vec4 {
                let comp_texpos1 = (geom.xy - vec2(0.5, 0.5)) * scale_delta + vec2(0.5, 0.5) + offset_delta;
                let comp_texpos2 = (vec2(1.0 - geom.x, geom.y) - vec2(0.5, 0.5)) * scale_delta + vec2(0.5, 0.5) + offset_delta;
                let fr1 = sample2d(texturez, comp_texpos1).rg; //kaleido(geom.xy-vec2(0.5,0.5), 3.14/8., time, 2.*time)).rg;
                let fr2 = sample2d(texturez, comp_texpos2).rg;
                let cam = sample2d(texcam, geom.xy); //kaleido(geom.xy-vec2(0.5,0.5), 3.14/8., time, 2.*time));
                if fr1.x > 1.0 {
                    return vec4(cam.xyz, 1.0);
                }
                let dx = sin(2. * atan(dfdx(fr1.y), dfdy(fr1.y)) + time * 10.) * 0.5 + 0.5;
                let index = (color_offset + fr1.x * 8.0 - outer_scale * 0.05 * log(fr1.y)) * palette_scale;
                let fract = mix(df_iq_pal0(index), mix(
                    df_iq_pal1(index),
                    mix(df_iq_pal2(index), mix(df_iq_pal3(index), mix(
                        df_iq_pal4(index),
                        mix(df_iq_pal5(index), mix(
                            df_iq_pal6(index),
                            df_iq_pal7(index),
                            clamp(palette - 6.0, 0., 1.)
                        ), clamp(palette - 5.0, 0., 1.)),
                        clamp(palette - 4.0, 0., 1.)
                    ), clamp(palette - 3.0, 0., 1.)), clamp(palette - 2.0, 0., 1.)),
                    clamp(palette - 1.0, 0., 1.)
                ), clamp(palette, 0., 1.));
                return vec4(fract.xyz, alpha);
            }
        }))
    }
    
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        
        if let Event::Construct = event {
            self.capture.init(cx, 0, CaptureFormat::NV12, 3840, 2160, 30.0);
            self.mandel.init(cx);
            self.gamepad.init(0, 0.1);
        }
        
        self.window.handle_desktop_window(cx, event);
        
        self.view.handle_scroll_bars(cx, event);
        
        if let CaptureEvent::NewFrame = self.capture.handle_signal(cx, event) {
            self.view.redraw_view_area(cx);
        }
        
        if self.mandel.handle_signal(cx, event) {
            self.view.redraw_view_area(cx);
        }
    }
    
    fn do_gamepad_interaction(&mut self, _cx: &mut Cx) {
        self.gamepad.poll();
        // update the mandelbrot location
        
        self.current_loc.center_x += 0.05 * self.gamepad.right_thumb.x as f64 * self.current_loc.zoom;
        self.current_loc.center_y -= 0.05 * self.gamepad.right_thumb.y as f64 * self.current_loc.zoom;
        self.current_loc.zoom = self.current_loc.zoom * (1.0 - 0.03 * self.gamepad.left_thumb.y as f64);
        let mut pred_loc = self.current_loc.clone();
        for _ in 0..10 {
            pred_loc.center_x += 0.05 * self.gamepad.right_thumb.x as f64 * pred_loc.zoom;
            pred_loc.center_y -= 0.05 * self.gamepad.right_thumb.y as f64 * pred_loc.zoom;
            if self.gamepad.left_thumb.y<0.0 {
                pred_loc.zoom = pred_loc.zoom * (1.0 - 0.03 * self.gamepad.left_thumb.y as f64);
                pred_loc.zoom = pred_loc.zoom * (1.0 - 0.03 * self.gamepad.left_thumb.y as f64);
                pred_loc.zoom = pred_loc.zoom * (1.0 - 0.03 * self.gamepad.left_thumb.y as f64);
                pred_loc.zoom = pred_loc.zoom * (1.0 - 0.03 * self.gamepad.left_thumb.y as f64);
            };
        }
        
        self.mandel.send_new_loc(self.loc_history.len(), pred_loc.clone());
        self.loc_history.push(pred_loc.clone());
        if self.gamepad.buttons & GamepadButtonLeftShoulder > 0 {
            if self.palette > 0.0 {self.palette -= 0.02;}
            else {self.palette = 0.0;}
        }
        if self.gamepad.buttons & GamepadButtonRightShoulder > 0 {
            if self.palette < 7.0 {self.palette += 0.02;}
            else {self.palette = 7.0;}
        }
        if self.gamepad.buttons_down_edge & GamepadButtonDpadUp > 0 {
            if self.palette_scale > 0.5 {self.palette_scale -= 0.50;}
            else {self.palette_scale = 0.5;}
        }
        if self.gamepad.buttons_down_edge & GamepadButtonDpadDown > 0 {
            if self.palette_scale < 10.0 {self.palette_scale += 0.5;}
            else {self.palette_scale = 10.0;}
        }
       if self.gamepad.buttons_down_edge & GamepadButtonDpadLeft > 0 {
            if self.outer_scale > 0.02 {self.outer_scale -= 0.5;}
            else {self.outer_scale = 0.02;}
        }
        if self.gamepad.buttons_down_edge & GamepadButtonDpadRight > 0 {
            if self.outer_scale < 5.0 {self.outer_scale += 0.5;}
            else {self.outer_scale = 5.0;}
        }
         self.color_offset += 0.02 * self.gamepad.right_trigger;
        self.color_offset -= 0.02 * self.gamepad.left_trigger;
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        
        let _ = self.window.begin_desktop_window(cx);
        
        let _ = self.view.begin_view(cx, Layout {
            abs_origin: Some(Vec2::zero()),
            padding: Padding {l: 0., t: 0., r: 0., b: 0.},
            ..Default::default()
        });
        
        self.do_gamepad_interaction(cx);
        
        self.view.redraw_view_area(cx);
        let inst = self.mandel_blit.draw_blit_walk(cx, &self.mandel.texture, Bounds::Fill, Bounds::Fill, Margin::zero());
        inst.push_uniform_texture_2d(cx, &self.capture.texture);
        inst.push_uniform_float(cx, self.frame);
        if let Some(index) = cx.get_mapped_texture_user_data(&self.mandel.texture) {
            inst.push_uniform_float(cx, (self.current_loc.zoom / self.loc_history[index].zoom) as f32);
            let screen_dx = ((self.loc_history[index].center_x - self.current_loc.center_x) / self.loc_history[index].zoom) / 4.0;
            let screen_dy = ((self.loc_history[index].center_y - self.current_loc.center_y) / self.loc_history[index].zoom) / 3.0;
            inst.push_uniform_vec2f(cx, -screen_dx as f32, -screen_dy as f32);
            inst.push_uniform_float(cx, self.color_offset);
            inst.push_uniform_float(cx, self.palette as f32);
            inst.push_uniform_float(cx, self.palette_scale as f32);
            inst.push_uniform_float(cx, self.outer_scale as f32);
        }
        else {
            println!("ERROR")
        }
        
        self.frame += 0.001;
        cx.reset_turtle_walk();
        
        self.view.redraw_view_area(cx);
        self.view.end_view(cx);
        
        self.window.end_desktop_window(cx);
    }
}