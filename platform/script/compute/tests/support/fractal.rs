//! The fractal tile kernels of the AI hub's kernel domain (branch rc-hubcompute,
//! libs/ai/hub/src/kernels/{fractal,multibrot}.splash at f54c1657e0, verbatim) and their f32
//! forms (the same escape-time loops in f32, as the shallow-zoom tiles run): masked loops whose
//! lanes leave at different iterations, divergent breaks, per-lane iteration counts.
#![allow(dead_code)]

pub const FRACTAL_F64: &str = r#"
// Escape-time fractals, one tile per call: res x res texels, row-major,
// texel centres at view.x + (col + 0.5) * step (y likewise).
//
// Per texel a u32: the low 24 bits are the smooth iteration count in 1/64
// steps (capped at 2^24 - 1), the top byte the class: 0 escaped, 255 inside.
// The bailout is |z|^2 < 256; z is frozen at escape; n counts the steps
// taken. Mandelbrot's main cardioid and period-2 bulb are inside up front.
//
// de = 1 (distance estimation): |dz| is carried by its magnitude recurrence
// |dz'| = 2 |z| |dz| + 1 (Julia: dz0 = 1, no +1) before each step; an
// escaped texel's top byte is then the distance code
// clamp(round(log2(max(dist, 1e-9)) * 20 + 100), 1, 254), dist = |z| ln|z| /
// |dz| in texels.
// trap = 1 (cross orbit trap): the low 24 bits are min(max((1 -
// log2(max(trap, 1e-30))) * 8, 0) * 64, 2^24 - 1), trap the orbit's closest
// approach to the axes over the steps after which it was still bounded,
// for escaped and inside texels; the cardioid and bulb are iterated then.
//
// kind: 0 Mandelbrot, 1 Julia, 2 Burning Ship, 3 Tricorn (Multibrot is
// multibrot.splash: its inner power loop needs a lower iteration cap).
// view (f64): x0, y0, step, julia re, julia im.
let view = input(f64)
let out = output(u32)
let res = param(256, 1, 4096)
let max_iter = param(500, 1, 16384)
let kind = param(0, 0, 3)
let de = param(0, 0, 1)
let trap = param(0, 0, 1)

fn low24(v) {
    let w = v * 64.0
    if w < 0.0 {
        w = f64(0.0)
    }
    if w > 16777215.0 {
        w = f64(16777215.0)
    }
    int(float(floor(w)))
}

fn element(i) {
    let r = int(res)
    let col = i % r
    let row = i / r
    let step = view[2]
    let cr = view[0] + (f64(col) + 0.5) * step
    let ci = view[1] + (f64(row) + 0.5) * step
    let k = int(kind)
    let mi = int(max_iter)
    let d_on = int(de)
    let t_on = int(trap)
    let zr = f64(0.0)
    let zi = f64(0.0)
    let ar = cr
    let ai = ci
    let dz = f64(0.0)
    let dz_add = f64(1.0)
    if k == 1 {
        zr = cr
        zi = ci
        ar = view[3]
        ai = view[4]
        dz = f64(1.0)
        dz_add = f64(0.0)
    }
    let inside = 0
    if k == 0 && t_on == 0 {
        let x = cr - 0.25
        let q = x * x + ci * ci
        if q * (q + x) < 0.25 * ci * ci {
            inside = 1
        }
        let x1 = cr + 1.0
        if x1 * x1 + ci * ci < 0.0625 {
            inside = 1
        }
    }
    let n = 0
    let mag = zr * zr + zi * zi
    let tr = f64(1000000000.0)
    if inside == 0 {
        inside = 1
        for it in 0..16384 {
            if it >= mi {
                break
            }
            if d_on == 1 {
                dz = 2.0 * sqrt(mag) * dz + dz_add
            }
            let fr = zr * zr - zi * zi
            let fi = (zr + zr) * zi
            if k == 2 {
                let a = abs(zr)
                let b = abs(zi)
                fr = a * a - b * b
                fi = (a + a) * b
            }
            if k == 3 {
                fi = -2.0 * zr * zi
            }
            zr = fr + ar
            zi = fi + ai
            mag = zr * zr + zi * zi
            n = n + 1
            if mag < 256.0 {
                if t_on == 1 {
                    let ax = abs(zr)
                    let ay = abs(zi)
                    let d = if ax < ay { ax } else { ay }
                    if d < tr {
                        tr = d
                    }
                }
            } else {
                inside = 0
                break
            }
        }
    }
    let tv = (1.0 - log2(max(tr, 1e-30))) * 8.0
    if inside == 1 {
        out[i] = if t_on == 1 { -16777216 + low24(tv) } else { -16777216 }
    } else {
        let nu = f64(n)
        if mag > 1.0 {
            nu = f64(n) + 1.0 - ln(0.5 * ln(mag)) / ln(f64(2.0))
        }
        let value = if t_on == 1 { low24(tv) } else { low24(nu) }
        if d_on == 1 && dz > 0.0 && mag > 1.0 {
            let za = sqrt(mag)
            let dist = za * ln(za) / dz / step
            let code = int(float(round(log2(max(dist, 1e-9)) * 20.0 + 100.0)))
            if code < 1 {
                code = 1
            }
            if code > 254 {
                code = 254
            }
            let top = if code >= 128 { (code - 256) * 16777216 } else { code * 16777216 }
            out[i] = top + value
        } else {
            out[i] = value
        }
    }
}
"#;

pub const MULTIBROT_F64: &str = r#"
// Multibrot z <- z^power + c (power 2..8), one tile per call, in the texel
// format of fractal.splash: res x res texels, row-major, texel centres at
// view.x + (col + 0.5) * step; low 24 bits the smooth count in 1/64 steps,
// top byte 0 escaped or 255 inside; bailout |z|^2 < 256. de and trap as in
// fractal.splash, with |dz'| = power |z|^(power-1) |dz| + 1.
// Its power loop makes an iteration dearer, so its cap is 6144 iterations.
// view (f64): x0, y0, step.
let view = input(f64)
let out = output(u32)
let res = param(256, 1, 4096)
let max_iter = param(500, 1, 6144)
let power = param(3, 2, 8)
let de = param(0, 0, 1)
let trap = param(0, 0, 1)

fn low24(v) {
    let w = v * 64.0
    if w < 0.0 {
        w = f64(0.0)
    }
    if w > 16777215.0 {
        w = f64(16777215.0)
    }
    int(float(floor(w)))
}

fn element(i) {
    let r = int(res)
    let col = i % r
    let row = i / r
    let step = view[2]
    let cr = view[0] + (f64(col) + 0.5) * step
    let ci = view[1] + (f64(row) + 0.5) * step
    let p = int(power)
    let pf = f64(p)
    let mi = int(max_iter)
    let d_on = int(de)
    let t_on = int(trap)
    let zr = f64(0.0)
    let zi = f64(0.0)
    let dz = f64(0.0)
    let n = 0
    let mag = zr * zr + zi * zi
    let tr = f64(1000000000.0)
    let inside = 1
    for it in 0..6144 {
        if it >= mi {
            break
        }
        if d_on == 1 {
            let za = sqrt(mag)
            let zp = za
            for j in 0..6 {
                if j >= p - 2 {
                    break
                }
                zp = zp * za
            }
            dz = pf * zp * dz + 1.0
        }
        let wr = zr
        let wi = zi
        for j in 0..7 {
            if j >= p - 1 {
                break
            }
            let t = wr * zr - wi * zi
            wi = wr * zi + wi * zr
            wr = t
        }
        zr = wr + cr
        zi = wi + ci
        mag = zr * zr + zi * zi
        n = n + 1
        if mag < 256.0 {
            if t_on == 1 {
                let ax = abs(zr)
                let ay = abs(zi)
                let d = if ax < ay { ax } else { ay }
                if d < tr {
                    tr = d
                }
            }
        } else {
            inside = 0
            break
        }
    }
    let tv = (1.0 - log2(max(tr, 1e-30))) * 8.0
    if inside == 1 {
        out[i] = if t_on == 1 { -16777216 + low24(tv) } else { -16777216 }
    } else {
        let nu = f64(n)
        if mag > 1.0 {
            nu = f64(n) + 1.0 - ln(0.5 * ln(mag)) / ln(pf)
        }
        let value = if t_on == 1 { low24(tv) } else { low24(nu) }
        if d_on == 1 && dz > 0.0 && mag > 1.0 {
            let za = sqrt(mag)
            let dist = za * ln(za) / dz / step
            let code = int(float(round(log2(max(dist, 1e-9)) * 20.0 + 100.0)))
            if code < 1 {
                code = 1
            }
            if code > 254 {
                code = 254
            }
            let top = if code >= 128 { (code - 256) * 16777216 } else { code * 16777216 }
            out[i] = top + value
        } else {
            out[i] = value
        }
    }
}
"#;

/// fractal.splash in f32 (view as f32 words): the shallow-zoom form.
pub const FRACTAL_F32: &str = r#"
let view = input(f32)
let out = output(u32)
let res = param(256, 1, 4096)
let max_iter = param(500, 1, 16384)
let kind = param(0, 0, 3)

fn element(i) {
    let r = int(res)
    let col = i % r
    let row = i / r
    let step = view[2]
    let cr = view[0] + (float(col) + 0.5) * step
    let ci = view[1] + (float(row) + 0.5) * step
    let k = int(kind)
    let mi = int(max_iter)
    let zr = 0.0
    let zi = 0.0
    let ar = cr
    let ai = ci
    if k == 1 {
        zr = cr
        zi = ci
        ar = view[3]
        ai = view[4]
    }
    let inside = 0
    if k == 0 {
        let x = cr - 0.25
        let q = x * x + ci * ci
        if q * (q + x) < 0.25 * ci * ci {
            inside = 1
        }
        let x1 = cr + 1.0
        if x1 * x1 + ci * ci < 0.0625 {
            inside = 1
        }
    }
    let n = 0
    let mag = zr * zr + zi * zi
    if inside == 0 {
        inside = 1
        for it in 0..16384 {
            if it >= mi {
                break
            }
            let fr = zr * zr - zi * zi
            let fi = (zr + zr) * zi
            if k == 2 {
                let a = abs(zr)
                let b = abs(zi)
                fr = a * a - b * b
                fi = (a + a) * b
            }
            if k == 3 {
                fi = -2.0 * zr * zi
            }
            zr = fr + ar
            zi = fi + ai
            mag = zr * zr + zi * zi
            n = n + 1
            if mag < 256.0 {
            } else {
                inside = 0
                break
            }
        }
    }
    if inside == 1 {
        out[i] = -16777216
    } else {
        let nu = float(n)
        if mag > 1.0 {
            nu = float(n) + 1.0 - ln(0.5 * ln(mag)) / ln(2.0)
        }
        let v = nu * 64.0
        if v < 0.0 {
            v = 0.0
        }
        if v > 16777215.0 {
            v = 16777215.0
        }
        out[i] = int(floor(v))
    }
}
"#;

/// multibrot.splash in f32: an inner power loop under the escape loop.
pub const MULTIBROT_F32: &str = r#"
let view = input(f32)
let out = output(u32)
let res = param(256, 1, 4096)
let max_iter = param(500, 1, 8192)
let power = param(3, 2, 8)

fn element(i) {
    let r = int(res)
    let col = i % r
    let row = i / r
    let step = view[2]
    let cr = view[0] + (float(col) + 0.5) * step
    let ci = view[1] + (float(row) + 0.5) * step
    let p = int(power)
    let mi = int(max_iter)
    let zr = 0.0
    let zi = 0.0
    let n = 0
    let mag = zr * zr + zi * zi
    let inside = 1
    for it in 0..8192 {
        if it >= mi {
            break
        }
        let wr = zr
        let wi = zi
        for j in 0..7 {
            if j >= p - 1 {
                break
            }
            let t = wr * zr - wi * zi
            wi = wr * zi + wi * zr
            wr = t
        }
        zr = wr + cr
        zi = wi + ci
        mag = zr * zr + zi * zi
        n = n + 1
        if mag < 256.0 {
        } else {
            inside = 0
            break
        }
    }
    if inside == 1 {
        out[i] = -16777216
    } else {
        let nu = float(n)
        if mag > 1.0 {
            nu = float(n) + 1.0 - ln(0.5 * ln(mag)) / ln(float(p))
        }
        let v = nu * 64.0
        if v < 0.0 {
            v = 0.0
        }
        if v > 16777215.0 {
            v = 16777215.0
        }
        out[i] = int(floor(v))
    }
}
"#;
