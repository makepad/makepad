//! The geometry kernels the benchmarks and the cross-width test share: a 1M-vertex heightfield
//! (fbm noise, one pass with finite-difference normals, or heights then normals) and a
//! 100k-instance transform (position, quaternion, scale -> mat4 + colour).
#![allow(dead_code)]

pub const W: usize = 1024;

pub const ONE_PASS: &str = r#"
    let W = 1024
    let base = input(f32)
    let pos = output(vec3)
    let nrm = output(vec3)
    let amp = param(6.0)
    fn height(x, z) { base[int(z) * W + int(x)] + fbm2(vec2(x, z) * 0.01, 4, 2.0, 0.5) * amp }
    fn vertex(i) {
        let x = float(i % W)
        let z = float(i / W)
        let h = height(x, z)
        pos[i] = vec3(x, h, z)
        let dx = height(x + 1.0, z) - h
        let dz = height(x, z + 1.0) - h
        nrm[i] = normalize(vec3(-dx, 1.0, -dz))
    }
"#;

/// Pass 1: heights (and positions).
pub const HEIGHTS: &str = r#"
    let W = 1024
    let base = input(f32)
    let pos = output(vec3)
    let hgt = output(f32)
    let amp = param(6.0)
    fn vertex(i) {
        let x = float(i % W)
        let z = float(i / W)
        let h = base[i] + fbm2(vec2(x, z) * 0.01, 4, 2.0, 0.5) * amp
        pos[i] = vec3(x, h, z)
        hgt[i] = h
    }
"#;

/// Pass 2: normals from the neighbours' heights (forward differences;
/// the last row and column reuse their own height).
pub const NORMALS: &str = r#"
    let W = 1024
    let hgt = input(f32)
    let nrm = output(vec3)
    fn vertex(i) {
        let x = i % W
        let z = i / W
        let h = hgt[i]
        let dx = hgt[if x < W - 1 { i + 1 } else { i }] - h
        let dz = hgt[if z < count / W - 1 { i + W } else { i }] - h
        nrm[i] = normalize(vec3(-dx, 1.0, -dz))
    }
"#;

pub const INSTANCES: &str = r#"
    let src = input(f32)
    let xf = output(mat4, 24, 0, inst)
    let col = output(vec4, 24, 16, inst)
    let t = param(0.0)
    fn instance(i) {
        let p = vec3(src[i * 8], src[i * 8 + 1], src[i * 8 + 2])
        let q = normalize(vec4(src[i * 8 + 3], src[i * 8 + 4], src[i * 8 + 5], src[i * 8 + 6]))
        let s = src[i * 8 + 7]
        let spin = quat_axis_angle(vec3(0.0, 1.0, 0.0), t + hash01(i, 1) * TAU)
        xf[i] = mat4_trs(p + vec3(0.0, sin(t + float(i) * 0.01) * 0.2, 0.0), quat_mul(spin, q), vec3(s, s, s))
        col[i] = vec4(hash01(i, 2), hash01(i, 3), hash01(i, 4), 1.0)
    }
"#;

/// (name, source) of each.
pub const GEOMETRY: &[(&str, &str)] = &[("one_pass", ONE_PASS), ("heights", HEIGHTS), ("normals", NORMALS), ("instances", INSTANCES)];
