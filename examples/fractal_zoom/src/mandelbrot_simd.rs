
use {
    std::simd::*,
    crate::mandelbrot::*
};

// simd constructor helpers to make the code readable
// the syntax is simdtype+v for 'vector' and +s for 'scalar' 
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

// SIMD is a way to do computations in parallel using vector types
// for example:
// let a:u32x2 = [1,2]; let b:u32x2 = [3,4];
// a+b is then [1+3,2+4]
// For the CPU this code takes (roughly) the same time to execute as 1+3
// So you get 2 times the performance here. And you can also use
// SIMD widths of 2,4,8 depending on your CPU's abilities
// to get a 2x 4x or 8x speedup (rougly) of what you are doing.

// Most ARM chips (and the WASM standard) have a SIMD width of 128 bits
// This means you get at most 2xf64 or 4xf32 float computations in parallel.
// Intel/AMD have 256 and intel used to have 512 bits even, but nobody used it.

// Using SIMD can be great for computations where you can arrange a loop
// to run 2 4 or 8 steps at a time 
// computing a mandelbrot fractal is such an ideal case

// each component of the vector [1,2] is called a 'lane'
// in order to still be able to do logic on those lanes independently
// SIMD has something called 'masks'. These are the same vectors,
// except they store bools. So [true,false] for instance.
// And then you can use mask.select(iftruevec, iffalsevec) to use the true/false
// to pick from truevec or falsevec. With this and binary logic such as
// AND and OR and XOR to combine the mask vectors,
// you can construct very efficient code.

// here we use f32x4 to compute 4 mandelbrot pixels at the same time with f32 precision.
fn mandelbrot_pixel_f32x4(max_iter: u32, c_x: f32x4, c_y: f32x4) -> (u32x4, f32x4) {
    let mut x = c_x;
    let mut y = c_y;  
    // in SIMD mandelbrot the loop has to continue
    // until all the 4 lanes have exitted
    // this means you need to hold onto the magsq/iter 
    // values per lane at the moment it needs to exit
    // until everyone has exitted
    let mut magsq_out = f32x4s(0.0);
    let mut iter_out = u32x4s(max_iter);
    let mut exitted = m32x4s(false);
    
    for n in 0..max_iter {
        let xy = x * y;
        let xx = x * x;
        let yy = y * y;
        let magsq = xx + yy;
        
        // this compares the magsq to > 4.0 and stores the result in a mask
        // masks are vectors of bools you can use to select values
        // in simd types by lane
        //let if_exit = magsq.simd_gt(f32x4s(4.0));
        let if_exit = magsq.simd_gt(f32x4s(4.0));

        // this boolean logic is only 1 when the value 'changed to 1'
        // and 0 otherwise. so it stores if we have a new exit on our lanes
        let new_exit = (if_exit ^ exitted) & if_exit;
        // merge it into our exitted set 
        exitted = exitted | new_exit;
        
        // when a lane has a 'new exit' it stores the current value
        // otherwise it uses the old value (magsq and iter)
        // the syntax is mask.select(truesimd, falsesimd)
        magsq_out = new_exit.select(magsq, magsq_out);
        iter_out = new_exit.select(u32x4s(n), iter_out);

        // if all our lanes have exitted, return the results
        if exitted.all() {
            return (iter_out, magsq_out)
        }
        
        x = (xx - yy) + c_x;
        y = (xy + xy) + c_y;
    }
    // one of our lanes has hit max_iter. 
    return (iter_out, magsq_out)
}

// this is the main image loop for the f32 SIMD
pub fn mandelbrot_f32x4(tile: &mut Tile, max_iter: usize) {
    
    // store the tile size / fractals position and size into simd vectors
    // this way we can do math with it on 4 pixel positions in parallel
    // to transform them to 'fractal space' we feed the mandelbrot pixel calculation
    let tile_size = (f32x4s(TILE_SIZE_X as f32), f32x4s(TILE_SIZE_Y as f32));
    let fractal_pos = (f32x4s(tile.fractal.pos.x as f32), f32x4s(tile.fractal.pos.y as f32));
    let fractal_size = (f32x4s(tile.fractal.size.x as f32), f32x4s(tile.fractal.size.y as f32));
    
    for y in 0..TILE_SIZE_Y {
        for x in (0..TILE_SIZE_X).step_by(4) {
            let xf = x as f32;
            // store the position of the pixel in a simd vector
            let pixel_pos = (f32x4v(xf, xf + 1.0, xf + 2.0, xf + 3.0), f32x4s(y as f32));
            
            // in parallel compute the x and y values of the pixel in fractal space
            let fp_x = fractal_pos.0 + fractal_size.0 * pixel_pos.0 / tile_size.0;
            let fp_y = fractal_pos.1 + fractal_size.1 * pixel_pos.1 / tile_size.1;
            
            // compute 4 pixels in parallel 
            let (iter, magsq) = mandelbrot_pixel_f32x4(max_iter as u32, fp_x, fp_y);
            
            // scale and clamp the magnitude squared so that it can become a 
            // fixed point 16 bit value we can pack into 2x8bit components of the texture 
            let magsq = (magsq + f32x4s(127.0)) * f32x4s(256.0);
            
            let magsq = magsq.simd_clamp(f32x4s(0.0), f32x4s(65535.0));
            
            //let magsq = magsq.simd_clamp(f32x4s(0.0), f32x4s(65535.0));
            
            // cast our float magnitude squared into an integer simd vector
            let magsq: u32x4 = magsq.cast();
            
            // here we unpack the simd vectors and write into the texture buffer
            for i in 0..4 {
                // we use a u32 (W Z Y X) to pack in our fractal data, we unpack this in the shader
                tile.buffer[y * TILE_SIZE_X + x + i] = iter[i] as u32 | ((magsq[i]) << 16);
            }
        }
    }
}


// The same as the above, except using 2 lanes of f64s
fn mandelbrot_pixel_f64x2(max_iter: u64, c_x: f64x2, c_y: f64x2) -> (u64x2, f64x2) {
    let mut x = c_x;
    let mut y = c_y;
    let mut magsq_out = f64x2s(0.0);
    let mut iter_out = u64x2s(max_iter);
    let mut exitted = m64x2s(false);
    for n in 0..max_iter {
        let xy = x * y;
        let xx = x * x;
        let yy = y * y;
        let magsq = xx + yy;
        
        //let if_exit = magsq.simd_gt(f64x2s(4.0));
        let if_exit = magsq.simd_gt(f64x2s(4.0));
        
        let new_exit = (if_exit ^ exitted) & if_exit;
        exitted = exitted | new_exit;
        magsq_out = new_exit.select(magsq, magsq_out);
        iter_out = new_exit.select(u64x2s(n), iter_out);
        if exitted.all() {
            return (iter_out, magsq_out)
        }
        
        x = (xx - yy) + c_x;
        y = (xy + xy) + c_y;
    }
    return (iter_out, magsq_out)
}

pub fn mandelbrot_f64x2(tile: &mut Tile, max_iter: usize) {
    let tile_size = (f64x2s(TILE_SIZE_X as f64), f64x2s(TILE_SIZE_Y as f64));
    let fractal_pos = (f64x2s(tile.fractal.pos.x), f64x2s(tile.fractal.pos.y));
    let fractal_size = (f64x2s(tile.fractal.size.x), f64x2s(tile.fractal.size.y));
    for y in 0..TILE_SIZE_Y {
        for x in (0..TILE_SIZE_X).step_by(2) {
            let xf = x as f64;
            let pixel_pos = (f64x2v(xf, xf + 1.0), f64x2s(y as f64));
            let fp_x = fractal_pos.0 + fractal_size.0 * pixel_pos.0 / tile_size.0;
            let fp_y = fractal_pos.1 + fractal_size.1 * pixel_pos.1 / tile_size.1;
            let (iter, magsq) = mandelbrot_pixel_f64x2(max_iter as u64, fp_x, fp_y);
            let magsq = (magsq + f64x2s(127.0)) * f64x2s(256.0);
            
            //let magsq = magsq.simd_clamp(f64x2s(0.0), f64x2s(65535.0));
            let magsq = magsq.simd_clamp(f64x2s(0.0), f64x2s(65535.0));
           
            let magsq: u64x2 = magsq.cast();
            for i in 0..2 {
                tile.buffer[y * TILE_SIZE_X + x + i] = iter[i] as u32 | ((magsq[i]) << 16) as u32;
            }
        }
    }
}

// 2 lane f64 antialiased by supersampling 4 pixels
#[allow(dead_code)]
pub fn mandelbrot_f64x2_4xaa(tile: &mut Tile, max_iter: usize) {
    let tile_size = (f64x2s(TILE_SIZE_X as f64), f64x2s(TILE_SIZE_Y as f64));
    let fractal_pos = (f64x2s(tile.fractal.pos.x), f64x2s(tile.fractal.pos.y));
    let fractal_size = (f64x2s(tile.fractal.size.x), f64x2s(tile.fractal.size.y));
    for y in 0..TILE_SIZE_Y {
        for x in 0..TILE_SIZE_X {
            let xf = x as f64;
            let yf = y as f64;
            let pixel_pos = (f64x2v(xf, xf + 0.5), f64x2s(yf));
            let fp_x = fractal_pos.0 + fractal_size.0 * pixel_pos.0 / tile_size.0;
            let fp_y = fractal_pos.1 + fractal_size.1 * pixel_pos.1 / tile_size.1;
            let (iter1, magsq1) = mandelbrot_pixel_f64x2(max_iter as u64, fp_x, fp_y);
            let pixel_pos = (f64x2v(xf, xf + 0.5), f64x2s(yf+0.5));
            let fp_x = fractal_pos.0 + fractal_size.0 * pixel_pos.0 / tile_size.0;
            let fp_y = fractal_pos.1 + fractal_size.1 * pixel_pos.1 / tile_size.1;
            let (iter2, magsq2) = mandelbrot_pixel_f64x2(max_iter as u64, fp_x, fp_y);
            let iter = (iter1 + iter2).reduce_sum() / 4;
            let magsq = (magsq1 + magsq2).reduce_sum() / 4.0;
            let magsq = ((magsq + 127.0)* 256.0).max(0.0).min(65535.0) as u32;
            tile.buffer[y * TILE_SIZE_X + x] = iter as u32 | (magsq << 16);
        }
    }
}

// 2 lane f64 antialiased by supersampling 4 pixels
#[allow(dead_code)]
pub fn mandelbrot_f64x2_16xaa(tile: &mut Tile, max_iter: usize) {
    let ts = (f64x2s(TILE_SIZE_X as f64), f64x2s(TILE_SIZE_Y as f64));
    let fp = (f64x2s(tile.fractal.pos.x), f64x2s(tile.fractal.pos.y));
    let fs = (f64x2s(tile.fractal.size.x), f64x2s(tile.fractal.size.y));
    for y in 0..TILE_SIZE_Y {
        for x in 0..TILE_SIZE_X {
            let xf = x as f64;
            let yf = y as f64;
            fn kernel_2x(xf:f64, yf:f64, fp:(f64x2,f64x2), fs:(f64x2,f64x2), ts:(f64x2,f64x2), max_iter: usize)->(u64x2, f64x2){
                let pp = (f64x2v(xf, xf + 0.25), f64x2s(yf));
                let fp_x = fp.0 + fs.0 * pp.0 / ts.0;
                let fp_y = fp.1 + fs.1 * pp.1 / ts.1;
                mandelbrot_pixel_f64x2(max_iter as u64, fp_x, fp_y)
            }
            fn kernel_4x(xf:f64, yf:f64, fp:(f64x2,f64x2), fs:(f64x2,f64x2), ts:(f64x2,f64x2), max_iter: usize)->(u64x2, f64x2){
                let (i1,m1) = kernel_2x(xf, yf, fp, fs, ts, max_iter);
                let (i2,m2) = kernel_2x(xf, yf+0.25, fp, fs, ts, max_iter);
                return (i1+i2, m1+m2)
            }
            let (i1, m1) = kernel_4x(xf, yf, fp, fs, ts, max_iter);
            let (i2, m2) = kernel_4x(xf+0.5, yf, fp, fs, ts, max_iter);
            let (i3, m3) = kernel_4x(xf, yf+0.5, fp, fs, ts, max_iter);
            let (i4, m4) = kernel_4x(xf+0.5, yf+0.5, fp, fs, ts, max_iter);
            let iter = (i1 + i2 + i3 + i4).reduce_sum() / 16;
            let magsq = (m1 + m2 + m3 + m4).reduce_sum() / 16.0;
            let magsq = ((magsq + 127.0)* 256.0).max(0.0).min(65535.0) as u32;
            tile.buffer[y * TILE_SIZE_X + x] = iter as u32 | (magsq << 16);
        }
    }
}