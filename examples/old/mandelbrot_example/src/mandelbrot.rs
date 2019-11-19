use core::arch::x86_64::*;
use std::sync::mpsc;
use render::*;
use serde::*;

#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct MandelbrotLoc {
    pub zoom: f64,
    pub center_x: f64,
    pub center_y: f64,
    pub rotate: f64,
}

#[derive(Default)]
pub struct Mandelbrot {
    pub start_loc: MandelbrotLoc,
    pub texture: Texture,
    pub num_threads: usize,
    pub num_iters: usize,
    pub width: usize,
    pub height: usize,
    pub frame_signal: Signal,
    pub sender: Option<mpsc::Sender<(usize, MandelbrotLoc, bool)>>,
}

impl Style for Mandelbrot {
    fn style(_cx: &mut Cx) -> Self {
        Self {
            start_loc: MandelbrotLoc {
                zoom: 1.0,
                rotate: 0.0,
                center_x: -1.5,
                center_y: 0.0
            },
            texture: Texture::default(),
            num_threads: 30,
            num_iters: 2048,
            width: 3840,
            height: 2160,
            frame_signal: Signal::empty(),
            sender: None,
        }
    }
}
impl Mandelbrot {
    pub fn init(&mut self, cx: &mut Cx) {
        // lets start a mandelbrot thread that produces frames
        self.frame_signal = cx.new_signal();
        self.texture.set_desc(cx, Some(TextureDesc {
            format: TextureFormat::MappedRGf32,
            width: Some(self.width),
            height: Some(self.height),
            multisample: None
        }));
        
        unsafe fn calc_mandel_avx2(c_x: __m256d, c_y: __m256d, max_iter: usize, cen_x: f64, cen_y: f64, sin_t: f64, cos_t: f64) -> (__m256d, __m256d) {
            // rotate points with simd
            let mcen_x = _mm256_set1_pd(cen_x);
            let mcen_y = _mm256_set1_pd(cen_y);
            let msin_t = _mm256_set1_pd(sin_t);
            let mcos_t = _mm256_set1_pd(cos_t);
            let mx = _mm256_sub_pd(c_x, mcen_x);
            let my = _mm256_sub_pd(c_y, mcen_y);
            let start_x = _mm256_add_pd(_mm256_sub_pd(_mm256_mul_pd(mx, mcos_t), _mm256_mul_pd(my, msin_t)), mcen_x);
            let start_y = _mm256_add_pd(_mm256_add_pd(_mm256_mul_pd(my, mcos_t), _mm256_mul_pd(mx, msin_t)), mcen_y);
            let mut x = start_x;
            let mut y = start_y;
            
            let mut count = _mm256_set1_pd(0.0);
            let mut merge_sum = _mm256_set1_pd(0.0);
            let add = _mm256_set1_pd(1.0);
            let max_dist = _mm256_set1_pd(4.0);
            
            for _ in 0..max_iter {
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
                x = _mm256_add_pd(_mm256_sub_pd(xx, yy), start_x);
                y = _mm256_add_pd(_mm256_add_pd(xy, xy), start_y);
            }
            return (_mm256_set1_pd(2.0), merge_sum);
        }
        
        // lets spawn fractal.height over 32 threads
        let num_threads = self.num_threads;
        let num_iters = self.num_iters;
        let width = self.width;
        let height = self.height;
        let center_x = 0.5 * self.width as f64;
        let center_y = 0.5 * self.height as f64;
        let awidth = 5.33 / self.width as f64;
        let aheight = 3.0 / self.height as f64;
        let chunk_height = height / num_threads;
        
        // stuff that goes into the threads
        let mut thread_pool = scoped_threadpool::Pool::new(self.num_threads as u32);
        let frame_signal = self.frame_signal.clone();
        let mut cxthread = cx.new_cxthread();
        let texture = self.texture.clone();
        let mut loc = self.start_loc.clone();
        let mut re_render = true;
        let mut sin_theta = 0.0;
        let mut cos_theta = 1.0;
        let (tx, rx) = mpsc::channel();
        self.sender = Some(tx);
        std::thread::spawn(move || {
            let mut user_data = 0;
            let mut has_hires = false;
            loop {
                let mut high_delta_zoom = false;
                while let Ok((recv_user_data, new_loc, high_delta)) = rx.try_recv() {
                    if high_delta {
                        high_delta_zoom = true;
                    }
                    user_data = recv_user_data;
                    if loc != new_loc {
                        re_render = true;
                        sin_theta = new_loc.rotate.sin();
                        cos_theta = new_loc.rotate.cos();
                        loc = new_loc;
                    }
                }
                if re_render && high_delta_zoom {
                    has_hires = false;
                    // fast 2x2 pixel version
                    thread_pool.scoped( | scope | {
                        if let Some(mapped_texture) = cxthread.lock_mapped_texture_f32(&texture, user_data) {
                            let mut iter = mapped_texture.chunks_mut((chunk_height * width * 2) as usize);
                            let dx = awidth * loc.zoom;
                            let dy = aheight * loc.zoom;
                            for i in 0..num_threads {
                                let thread_num = i;
                                let slice = iter.next().unwrap();
                                let loc = loc.clone();
                                scope.execute(move || {
                                    let it_v = [0f64, 0f64, 0f64, 0f64];
                                    let su_v = [0f64, 0f64, 0f64, 0f64];
                                    let start = thread_num * chunk_height as usize;
                                    for y in (start..(start + chunk_height)).step_by(2) {
                                        for x in (0..width).step_by(2) {
                                            unsafe {
                                                let vx = (x as f64 - center_x) * awidth * loc.zoom + loc.center_x;
                                                let vy = (y as f64 - center_y) * aheight * loc.zoom + loc.center_y;
                                                let c_re = _mm256_set_pd(vx, vx + dx, vx, vx + dx);
                                                let c_im = _mm256_set_pd(vy, vy, vy + dy, vy + dy);
                                                let (it256, sum256) = calc_mandel_avx2(c_re, c_im, num_iters, loc.center_x, loc.center_y, sin_theta, cos_theta);
                                                _mm256_store_pd(it_v.as_ptr(), it256);
                                                _mm256_store_pd(su_v.as_ptr(), sum256);
                                                let off = (x * 2 + (y - start) * width * 2) as usize;
                                                let off_dy = off + width * 2 as usize;
                                                slice[off] = it_v[3] as f32;
                                                slice[off + 1] = su_v[3] as f32;
                                                slice[off + 2] = it_v[2] as f32;
                                                slice[off + 3] = su_v[2] as f32;
                                                slice[off_dy] = it_v[1] as f32;
                                                slice[off_dy + 1] = su_v[1] as f32;
                                                slice[off_dy + 2] = it_v[0] as f32;
                                                slice[off_dy + 3] = su_v[0] as f32;
                                            }
                                        }
                                    }
                                })
                            }
                            re_render = false;
                        }
                        else { // wait a bit
                            re_render = true;
                        }
                    });
                    cxthread.unlock_mapped_texture(&texture);
                    Cx::send_signal(frame_signal, 0);
                }
                else if !has_hires || !high_delta_zoom && re_render {
                    // fancy antialised version rendering 8k effectively
                    
                    thread_pool.scoped( | scope | {
                        if let Some(mapped_texture) = cxthread.lock_mapped_texture_f32(&texture, user_data) {
                            
                            let dx = 0.5 * awidth * loc.zoom;
                            let dy = 0.5 * aheight * loc.zoom;
                            
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
                                    for y in (start..(start + chunk_height)).step_by(1) {
                                        for x in (0..width).step_by(1) {
                                            unsafe {
                                                let vx = (x as f64 - center_x) * awidth * loc.zoom + loc.center_x;
                                                let vy = (y as f64 - center_y) * aheight * loc.zoom + loc.center_y;
                                                let c_re = _mm256_set_pd(vx, vx + dx, vx, vx + dx);
                                                let c_im = _mm256_set_pd(vy, vy, vy + dy, vy + dy);
                                                let (it256, sum256) = calc_mandel_avx2(c_re, c_im, num_iters, loc.center_x, loc.center_y, sin_theta, cos_theta);
                                                _mm256_store_pd(it_v.as_ptr(), it256);
                                                _mm256_store_pd(su_v.as_ptr(), sum256);
                                                let off = (x * 2 + (y - start) * width * 2) as usize;
                                                slice[off] = ((it_v[3] + it_v[2] + it_v[1] + it_v[0]) / 4.0) as f32;
                                                slice[off + 1] = ((su_v[3] + su_v[2] + su_v[1] + su_v[0]) / 4.0) as f32;
                                            }
                                        }
                                    }
                                })
                            }
                            re_render = false;
                            has_hires = true;
                        }
                        else {
                            re_render = true;
                            has_hires = false;
                        }
                    });
                    cxthread.unlock_mapped_texture(&texture);
                    Cx::send_signal(frame_signal, 0);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                
            }
        });
    }
    
    pub fn handle_signal(&mut self, _cx: &mut Cx, event: &Event) -> bool {
        if let Event::Signal(se) = event {
            if self.frame_signal.is_signal(se) { // we haz new texture
                return true
            }
        }
        false
    }
    
    pub fn send_new_loc(&mut self, index: usize, new_loc: MandelbrotLoc, high_delta_zoom: bool) {
        if let Some(sender) = &self.sender {
            let _ = sender.send((index, new_loc, high_delta_zoom));
        }
    }
}
