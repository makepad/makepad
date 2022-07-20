
use {
    std::simd::*,
    crate::mandelbrot::*
};

// simd constructor helpers to declog the code
fn f32x4v(a: f32, b: f32, c: f32, d: f32) -> f32x4 {f32x4::from_array([a, b, c, d])}
fn f32x4s(a: f32) -> f32x4 {f32x4::from_array([a; 4])}
fn m32x4s(a: bool) -> Mask::<i32, 4> {Mask::<i32, 4>::from_array([a; 4])}
fn _u32x4v(a: u32, b: u32, c: u32, d: u32) -> u32x4 {u32x4::from_array([a, b, c, d])}
fn u32x4s(a: u32) -> u32x4 {u32x4::from_array([a; 4])}

fn f64x2v(a: f64, b: f64) -> f64x2 {f64x2::from_array([a, b])}
fn f64x2s(a: f64) -> f64x2 {f64x2::from_array([a; 2])}
fn m64x2s(a: bool) -> Mask::<i64, 2> {Mask::<i64, 2>::from_array([a; 2])}
fn u64x2s(a: u64) -> u64x2 {u64x2::from_array([a; 2])}
fn _u64x2v(a: u64, b: u64) -> u64x2 {u64x2::from_array([a, b])}

// 4 lane f32 

fn mandelbrot_pixel_f32_simd(max_iter: u32, c_x: f32x4, c_y: f32x4) -> (u32x4, f32x4) {
    let mut x = c_x;
    let mut y = c_y;
    let mut dist_out = f32x4s(0.0);
    let mut iter_out = u32x4s(max_iter);
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

pub fn mandelbrot_f32_simd(tile: &mut TextureTile, max_iter: usize) {
    let tile_size = (f32x4s(TILE_SIZE_X as f32), f32x4s(TILE_SIZE_Y as f32));
    let fractal_pos = (f32x4s(tile.fractal.pos.x as f32), f32x4s(tile.fractal.pos.y as f32));
    let fractal_size = (f32x4s(tile.fractal.size.x as f32), f32x4s(tile.fractal.size.y as f32));
    
    for y in 0..TILE_SIZE_Y {
        for x in (0..TILE_SIZE_X).step_by(4) {
            let xf = x as f32;
            let tile_pos = (f32x4v(xf, xf + 1.0, xf + 2.0, xf + 3.0), f32x4s(y as f32));
            let fp_x = fractal_pos.0 + fractal_size.0 * tile_pos.0 / tile_size.0;
            let fp_y = fractal_pos.1 + fractal_size.1 * tile_pos.1 / tile_size.1;
            let (iter, dist) = mandelbrot_pixel_f32_simd(max_iter as u32, fp_x, fp_y);
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
    let mut iter_out = u64x2s(max_iter);
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

pub fn mandelbrot_f64_simd(tile: &mut TextureTile, max_iter: usize) {
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
            let (iter, dist) = mandelbrot_pixel_f64_simd(max_iter as u64, fp_x, fp_y);
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
pub fn _mandelbrot_f64_simd_aa(tile: &mut TextureTile, max_iter: usize) {
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
            let (iter1, dist1) = mandelbrot_pixel_f64_simd(max_iter as u64, fp_x, fp_y);
            let tile_pos = (f64x2v(xf, xf + 0.5), f64x2s(yf+0.5));
            let fp_x = fractal_pos.0 + fractal_size.0 * tile_pos.0 / tile_size.0;
            let fp_y = fractal_pos.1 + fractal_size.1 * tile_pos.1 / tile_size.1;
            let (iter2, dist2) = mandelbrot_pixel_f64_simd(max_iter as u64, fp_x, fp_y);
            let iter = (iter1 + iter2).reduce_sum() / 4;
            let dist = (dist1 + dist2).reduce_sum() / 4.0;
            let dist = (dist * 256.0 + 127.0 * 255.0).max(0.0).min(65535.0) as u32;
            tile.buffer[y * TILE_SIZE_X + x] = iter as u32 | (dist << 16);
        }
    }
}

