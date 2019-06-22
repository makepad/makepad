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
pub mod cap_winmf;
pub use crate::cap_winmf::*;
use std::sync::Mutex;
use core::arch::x86_64::*;

#[derive(Default)]
struct Mandelbrot {
    texture: Texture,
    num_threads: usize,
    width: usize,
    zoom: f64,
    height: usize,
    image_read:bool,
    image_signal: Signal,
    image_front: Mutex<Vec<f32>>
}

impl Mandelbrot {
    fn init(&mut self, cx: &mut Cx) {
        // lets start a mandelbrot thread that produces frames
        self.image_signal = cx.new_signal();
        self.width = 3000;
        self.height = 1500;
        self.image_read = true;
        self.zoom = 1.0;
        self.num_threads = 30;
        self.texture.set_desc(cx, Some(TextureDesc {
            format: TextureFormat::ImageRGf32,
            width: Some(self.width),
            height: Some(self.height),
            multisample: None
        }));
        self.image_thread();
    }
    
    fn image_thread(&mut self) {
        #[inline]
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
                    return (count, _mm256_sqrt_pd(merge_sum));
                }
                merge_sum = _mm256_or_pd(_mm256_and_pd(sum, mask),_mm256_andnot_pd(mask, merge_sum));
                count = _mm256_add_pd(count, _mm256_and_pd(add, mask));
                x = _mm256_add_pd(_mm256_sub_pd(xx, yy), c_x);
                y = _mm256_add_pd(_mm256_add_pd(xy, xy), c_y);
            }
            return (count, add);
        }
        
        fn calc_mandel(x: f64, y: f64) -> u32 {
            let mut rr = x;
            let mut ri = y;
            let mut n = 0;
            while n < 80 {
                let mut tr = rr * rr - ri * ri + x;
                let mut ti = 2.0 * rr * ri + y;
                rr = tr;
                ri = ti;
                n = n + 1;
                if rr * ri > 5.0 {
                    return n;
                }
            }
            return n;
        }
        
        let mut zoom = self.zoom;
        self.zoom /= 1.1;
        // lets spawn fractal.height over 32 threads
        let num_threads = self.num_threads;
        let width = self.width;
        let height = self.height;
        let fwidth = self.width as f64;
        let fheight = self.height as f64;
        let center_x = 0;
        let center_y = 0;
        let chunk_height = height / num_threads;
        let mut thread_pool = scoped_threadpool::Pool::new(self.num_threads as u32);
        let mut image_back: Vec<f32> = Vec::new();
        
        let mut image_signal = self.image_signal.clone();
        let mandel: u64 = self as *mut _ as u64; // this is safe. sure :)
        
        std::thread::spawn(move || {
            let mut zoom = 1.0;
            let mut center_x = -1.5;
            let mut center_y = 0.0;
            loop {
                zoom = zoom / 1.003;
                let time1 = Cx::profile_time_ns();
                thread_pool.scoped( | scope | {
                    image_back.resize(width * height * 2, 0.0);
                    let mut iter = image_back.chunks_mut((chunk_height * width * 2) as usize);
                    for i in 0..num_threads {
                        let thread_num = i;
                        let slice = iter.next().unwrap();
                        //println!("{}", chunk_height);
                        scope.execute(move || {
                            let mut it_v = [0f64, 0f64, 0f64, 0f64];
                            let mut su_v = [0f64, 0f64, 0f64, 0f64];
                            let start = thread_num * chunk_height as usize;
                            for y in (start..(start + chunk_height)).step_by(2) {
                                for x in (0..width).step_by(2) {
                                    //continue;
                                    // ok lets now do the simd mandel
                                    unsafe {
                                        let c_re = _mm256_set_pd(
                                            (x as f64 - fwidth * 0.5) * 4.0 / fwidth * zoom + center_x,
                                            ((x + 1) as f64 - fwidth * 0.5) * 4.0 / fwidth * zoom + center_x,
                                            (x as f64 - fwidth * 0.5) * 4.0 / fwidth * zoom + center_x,
                                            ((x + 1) as f64 - fwidth * 0.5) * 4.0 / fwidth * zoom + center_x
                                        );
                                        let c_im = _mm256_set_pd(
                                            (y as f64 - fheight * 0.5) * 3.0 / fheight * zoom + center_y,
                                            (y as f64 - fheight * 0.5) * 3.0 / fheight * zoom + center_y,
                                            ((y + 1) as f64 - fheight * 0.5) * 3.0 / fheight * zoom + center_y,
                                            ((y + 1) as f64 - fheight * 0.5) * 3.0 / fheight * zoom + center_y
                                        );
                                        let (it256, sum256) = calc_mandel_avx2(c_re, c_im, 130);
                                        _mm256_store_pd(it_v.as_ptr(), it256);
                                        _mm256_store_pd(su_v.as_ptr(), sum256);
                                        
                                        slice[(x* 2 + (y - start) * width * 2) as usize] = it_v[3] as f32;
                                        slice[(x* 2 + 2 + (y - start) * width* 2) as usize] = it_v[2] as f32;
                                        slice[(x* 2 + (1 + y - start) * width* 2) as usize] = it_v[1] as f32;
                                        slice[(x* 2 + 2 + (1 + y - start) * width* 2) as usize] = it_v[0] as f32;
                                        slice[1+(x* 2 + (y - start) * width * 2) as usize] = su_v[3] as f32;
                                        slice[1+(x* 2 + 2 + (y - start) * width* 2) as usize] = su_v[2] as f32;
                                        slice[1+(x* 2 + (1 + y - start) * width* 2) as usize] = su_v[1] as f32;
                                        slice[1+(x* 2 + 2 + (1 + y - start) * width* 2) as usize] = su_v[0] as f32;
                                    }
                                }
                            }
                        })
                    }
                    
                });
                let time2 = Cx::profile_time_ns();
                //println!("{}", (time2-time1)as f64 /1000.0);
                // alright we have a texture ready. now what.
                let mandel_ref = unsafe{&mut (*(mandel as *mut Mandelbrot))};
                while !mandel_ref.image_read{
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                let mut front =  mandel_ref.image_front.lock().unwrap();
                mandel_ref.image_read = false;
                std::mem::swap(&mut (*front), &mut image_back);
                Cx::send_signal(image_signal, 0);
            }
        });
    }
    
    fn handle_signal(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if let Event::Signal(se) = event {
            if self.image_signal.is_signal(se) { // we haz new texture
                let mut front = self.image_front.lock().unwrap();
                let cxtex = &mut cx.textures[self.texture.texture_id.unwrap()];
                std::mem::swap(&mut (*front), &mut cxtex.image_f32);
                cxtex.update_image = true;
                self.image_read = true;
                return true
            }
        }
        false
    }
}

struct App {
    view: View<ScrollBar>,
    window: DesktopWindow,
    blit: Blit,
    mandel_blit: Blit,
    frame: f32,
    cap: CapWinMF,
    mandel: Mandelbrot,
}

main_app!(App, "Example");

impl Style for App {
    fn style(cx: &mut Cx) -> Self {
        set_dark_style(cx);
        Self {
            mandel: Mandelbrot::default(),
            cap: CapWinMF::default(),
            window: DesktopWindow::style(cx),
            view: View {
                is_overlay:true,
                scroll_h: Some(ScrollBar::style(cx)),
                scroll_v: Some(ScrollBar::style(cx)),
                ..Style::style(cx)
            },
            blit: Blit::style(cx),
            mandel_blit: Blit{
                shader: cx.add_shader(App::def_blit_shader(), "App.blit"),
                ..Blit::style(cx)
            },
            frame:0.
        }
    }
}

impl App {
    fn def_blit_shader() -> ShaderGen {
        Blit::def_blit_shader().compose(shader_ast!({
            let texcam:texture2d<Texture>; 
            let time:float<Uniform>; 
            fn pixel() -> vec4 {
                let fr1 = sample2d(texturez, geom.xy).rg;
                let fr2 = sample2d(texturez, vec2(1.0-geom.x,geom.y)).rg;
                let cam = sample2d(texcam, vec2(1.0-geom.x,geom.y)); 
                //return vec4(df_iq_pal4(fr.x*2.0).xyz, alpha);
                //if fr1.x == 0.{
                //    return vec4(0.,0.,0.,1.);
               // }
                //let fract = df_iq_pal4(fr1.x*0.03 +fr2.x*0.03 - 0.1*log(fr1.y*fr2.y));
                return vec4(cam.xyz, alpha);
            }
        }))
    }
    
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        
        if let Event::Construct = event {
            self.cap.init(cx, 0, 3840, 2160, 30.0);
            self.mandel.init(cx);
        }
        
        self.window.handle_desktop_window(cx, event);
        
        self.view.handle_scroll_bars(cx, event);
        
        if let CapEvent::NewImage = self.cap.handle_signal(cx, event) {
            self.view.redraw_view_area(cx);
        }
        
        if self.mandel.handle_signal(cx, event) {
            self.view.redraw_view_area(cx);
        }
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        
        let _ = self.window.begin_desktop_window(cx);
        
        let _ = self.view.begin_view(cx, Layout {
            abs_origin: Some(Vec2::zero()),
            padding: Padding {l: 0., t: 0., r: 0., b: 0.},
            ..Default::default()
        }); 
        
        self.view.redraw_view_area(cx);
        let inst = self.mandel_blit.draw_blit_walk(cx, &self.mandel.texture, Bounds::Fill, Bounds::Fill, Margin::zero());
        inst.push_uniform_texture_2d(cx, &self.cap.texture);
        inst.push_uniform_float(cx, self.frame);
        self.frame += 1.;
        cx.reset_turtle_walk();

        self.view.redraw_view_area(cx);
        self.view.end_view(cx);
        
        self.window.end_desktop_window(cx);
    }
}