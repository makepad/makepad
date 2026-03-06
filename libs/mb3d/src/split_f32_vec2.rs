use super::*;
use makepad_math::{vec2, Vec2f};

const DS_SPLITTER: f32 = 4097.0;

fn ds_new(v: f32) -> Vec2f {
    vec2(v, 0.0)
}

fn ds_from_split(v: F2) -> Vec2f {
    vec2(v.x, v.y)
}

fn ds_quick_two_sum(a: f32, b: f32) -> Vec2f {
    let s = a + b;
    let e = b - (s - a);
    vec2(s, e)
}

fn ds_two_sum(a: f32, b: f32) -> Vec2f {
    let s = a + b;
    let bb = s - a;
    let e = (a - (s - bb)) + (b - bb);
    vec2(s, e)
}

fn ds_split(a: f32) -> (f32, f32) {
    let c = DS_SPLITTER * a;
    let hi = c - (c - a);
    let lo = a - hi;
    (hi, lo)
}

fn ds_two_prod(a: f32, b: f32) -> Vec2f {
    let p = a * b;
    let (a_hi, a_lo) = ds_split(a);
    let (b_hi, b_lo) = ds_split(b);
    let e = ((a_hi * b_hi - p) + a_hi * b_lo + a_lo * b_hi) + a_lo * b_lo;
    vec2(p, e)
}

fn ds_renorm(v: Vec2f) -> Vec2f {
    ds_quick_two_sum(v.x, v.y)
}

fn ds_add(a: Vec2f, b: Vec2f) -> Vec2f {
    let s = ds_two_sum(a.x, b.x);
    ds_quick_two_sum(s.x, s.y + a.y + b.y)
}

fn ds_sub(a: Vec2f, b: Vec2f) -> Vec2f {
    ds_add(a, vec2(-b.x, -b.y))
}

fn ds_add_f(a: Vec2f, b: f32) -> Vec2f {
    ds_add(a, ds_new(b))
}

fn ds_mul_f(a: Vec2f, b: f32) -> Vec2f {
    let p = ds_two_prod(a.x, b);
    ds_renorm(ds_quick_two_sum(p.x, p.y + a.y * b))
}

fn ds_mul(a: Vec2f, b: Vec2f) -> Vec2f {
    let p = ds_two_prod(a.x, b.x);
    ds_renorm(ds_quick_two_sum(
        p.x,
        p.y + a.x * b.y + a.y * b.x + a.y * b.y,
    ))
}

fn ds_div(a: Vec2f, b: Vec2f) -> Vec2f {
    let q1 = a.x / b.x;
    let r = ds_sub(a, ds_mul_f(b, q1));
    let q2 = r.x / b.x;
    let r2 = ds_sub(r, ds_mul_f(b, q2));
    let q3 = r2.x / b.x;
    ds_renorm(ds_add_f(ds_quick_two_sum(q1, q2), q3))
}

fn ds_abs(v: Vec2f) -> Vec2f {
    if v.x < 0.0 || (v.x == 0.0 && v.y < 0.0) {
        vec2(-v.x, -v.y)
    } else {
        v
    }
}

fn ds_sqrt(v: Vec2f) -> Vec2f {
    let x = ds_to_f(v).max(0.0).sqrt();
    if x == 0.0 {
        return ds_new(0.0);
    }
    let xds = ds_new(x);
    ds_mul_f(ds_add(xds, ds_div(v, xds)), 0.5)
}

fn ds_to_f(v: Vec2f) -> f32 {
    v.x + v.y
}

fn ds_lt(a: Vec2f, b: Vec2f) -> bool {
    a.x < b.x || (a.x == b.x && a.y < b.y)
}

fn ds_gt(a: Vec2f, b: Vec2f) -> bool {
    a.x > b.x || (a.x == b.x && a.y > b.y)
}

fn ds_box_fold(a: Vec2f, fold: Vec2f) -> Vec2f {
    ds_sub(ds_sub(ds_abs(ds_add(a, fold)), ds_abs(ds_sub(a, fold))), a)
}

#[derive(Clone, Copy, Debug)]
struct Vec2MarchHit {
    depth: Vec2f,
    iters: i32,
    shadow_steps: i32,
    hit: bool,
}

#[derive(Clone, Copy, Debug)]
struct Vec2Num3 {
    x: Vec2f,
    y: Vec2f,
    z: Vec2f,
}

fn ds_max(a: Vec2f, b: Vec2f) -> Vec2f {
    if ds_lt(a, b) {
        b
    } else {
        a
    }
}

fn vec2_num3_add(a: Vec2Num3, b: Vec2Num3) -> Vec2Num3 {
    Vec2Num3 {
        x: ds_add(a.x, b.x),
        y: ds_add(a.y, b.y),
        z: ds_add(a.z, b.z),
    }
}

fn vec2_num3_scale(v: Vec2Num3, s: Vec2f) -> Vec2Num3 {
    Vec2Num3 {
        x: ds_mul(v.x, s),
        y: ds_mul(v.y, s),
        z: ds_mul(v.z, s),
    }
}

fn vec2_num3_scale_f(v: Vec2Num3, s: f32) -> Vec2Num3 {
    Vec2Num3 {
        x: ds_mul_f(v.x, s),
        y: ds_mul_f(v.y, s),
        z: ds_mul_f(v.z, s),
    }
}

fn vec2_num3_dot(a: Vec2Num3, b: Vec2Num3) -> Vec2f {
    ds_add(ds_add(ds_mul(a.x, b.x), ds_mul(a.y, b.y)), ds_mul(a.z, b.z))
}

fn vec2_num3_normalize(v: Vec2Num3) -> Vec2Num3 {
    let len = ds_sqrt(vec2_num3_dot(v, v));
    if ds_gt(len, ds_new(1.0e-30)) {
        Vec2Num3 {
            x: ds_div(v.x, len),
            y: ds_div(v.y, len),
            z: ds_div(v.z, len),
        }
    } else {
        v
    }
}

fn vec2_num3_to_f3(v: Vec2Num3) -> F3 {
    F3 {
        x: ds_to_f(v.x),
        y: ds_to_f(v.y),
        z: ds_to_f(v.z),
    }
}

fn split_vec3_to_vec2_num3(v: F3Split) -> Vec2Num3 {
    Vec2Num3 {
        x: ds_from_split(v.x),
        y: ds_from_split(v.y),
        z: ds_from_split(v.z),
    }
}

fn split_vec3_to_f3(v: F3Split) -> F3 {
    F3 {
        x: v.x.x + v.x.y,
        y: v.y.x + v.y.y,
        z: v.z.x + v.z.y,
    }
}

fn f3_cross(a: F3, b: F3) -> F3 {
    F3 {
        x: a.y * b.z - a.z * b.y,
        y: a.z * b.x - a.x * b.z,
        z: a.x * b.y - a.y * b.x,
    }
}

fn seed_for_pixel(x: usize, y: usize) -> u32 {
    (x as u32)
        .wrapping_mul(0x45d9f3b)
        .wrapping_add((y as u32).wrapping_mul(0x2710_0001))
        .wrapping_add(0x2456_3487)
}

fn hybrid_de_vec2_uploads(scene: &GpuDsUploads, px: Vec2f, py: Vec2f, pz: Vec2f) -> (i32, Vec2f) {
    let cx = if scene.is_julia {
        ds_from_split(scene.julia_x)
    } else {
        px
    };
    let cy = if scene.is_julia {
        ds_from_split(scene.julia_y)
    } else {
        py
    };
    let cz = if scene.is_julia {
        ds_from_split(scene.julia_z)
    } else {
        pz
    };

    let mut x = px;
    let mut y = py;
    let mut z = pz;
    let mut w = ds_new(1.0);
    let mut r2 = ds_new(0.0);
    let mut iters = 0i32;
    let mut slot = 0usize;
    let mut remaining = scene.slot0_iters;

    for _ in 0..128 {
        if remaining <= 0 {
            slot += 1;
            if slot >= 2 {
                slot = scene.repeat_from_slot;
            }
            remaining = if slot == 0 {
                scene.slot0_iters
            } else {
                scene.slot1_iters
            };
        }

        if slot == 0 {
            let fold = ds_from_split(scene.ab_fold);
            x = ds_box_fold(x, fold);
            y = ds_box_fold(y, fold);
            z = ds_box_fold(z, fold);

            let rr = ds_add(ds_add(ds_mul(x, x), ds_mul(y, y)), ds_mul(z, z));
            let m = if ds_lt(rr, ds_from_split(scene.ab_min_r2)) {
                ds_from_split(scene.ab_scale_div_min_r2)
            } else if ds_lt(rr, ds_new(1.0)) {
                ds_div(ds_from_split(scene.ab_scale), rr)
            } else {
                ds_from_split(scene.ab_scale)
            };

            w = ds_mul(w, m);
            x = ds_add(ds_mul(x, m), cx);
            y = ds_add(ds_mul(y, m), cy);
            z = ds_add(ds_mul(z, m), cz);
        } else {
            x = ds_abs(x);
            y = ds_abs(y);
            z = ds_abs(z);

            if ds_lt(x, y) {
                std::mem::swap(&mut x, &mut y);
            }
            if ds_lt(x, z) {
                std::mem::swap(&mut x, &mut z);
            }
            if ds_lt(y, z) {
                std::mem::swap(&mut y, &mut z);
            }

            let nx = ds_add(
                ds_add(ds_mul(x, ds_from_split(scene.rot0.x)), ds_mul(y, ds_from_split(scene.rot0.y))),
                ds_mul(z, ds_from_split(scene.rot0.z)),
            );
            let ny = ds_add(
                ds_add(ds_mul(x, ds_from_split(scene.rot1.x)), ds_mul(y, ds_from_split(scene.rot1.y))),
                ds_mul(z, ds_from_split(scene.rot1.z)),
            );
            let nz = ds_add(
                ds_add(ds_mul(x, ds_from_split(scene.rot2.x)), ds_mul(y, ds_from_split(scene.rot2.y))),
                ds_mul(z, ds_from_split(scene.rot2.z)),
            );

            let sf = ds_sub(ds_from_split(scene.menger_scale), ds_new(1.0));
            x = ds_sub(ds_mul(nx, ds_from_split(scene.menger_scale)), ds_mul(ds_from_split(scene.menger_cx), sf));
            y = ds_sub(ds_mul(ny, ds_from_split(scene.menger_scale)), ds_mul(ds_from_split(scene.menger_cy), sf));
            let c = ds_mul(ds_from_split(scene.menger_cz), sf);
            z = ds_sub(c, ds_abs(ds_sub(ds_mul(nz, ds_from_split(scene.menger_scale)), c)));
            w = ds_mul(w, ds_from_split(scene.menger_scale));
        }

        iters += 1;
        remaining -= 1;
        r2 = ds_add(ds_add(ds_mul(x, x), ds_mul(y, y)), ds_mul(z, z));
        if ds_gt(r2, ds_from_split(scene.rstop)) || iters >= scene.max_iters {
            break;
        }
    }

    let de = ds_div(ds_sqrt(r2), ds_abs(w));
    (iters, de)
}

fn calc_de_vec2_uploads(scene: &GpuDsUploads, px: Vec2f, py: Vec2f, pz: Vec2f) -> (i32, Vec2f) {
    let (iters, de) = hybrid_de_vec2_uploads(scene, px, py, pz);
    (iters, ds_max(de, ds_from_split(scene.de_floor)))
}

fn shaderlike_ray_for_pixel_vec2(
    scene: &GpuDsUploads,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> (Vec2Num3, Vec2Num3) {
    let frag_x = x as f32;
    let frag_y = y as f32;
    let half_w = width as f32 * 0.5;
    let half_h = height as f32 * 0.5;
    let fov_mul = (scene.fov_y * 0.017453292519943295_f32) / height.max(1) as f32;

    let cafx = (half_w - frag_x) * fov_mul;
    let cafy = (frag_y - half_h) * fov_mul;
    let sx = cafx.sin();
    let cx = cafx.cos();
    let sy = cafy.sin();
    let cy = cafy.cos();

    let local_dir = F3 {
        x: -sx,
        y: sy,
        z: cx * cy,
    }
    .normalize();

    let cam_right = split_vec3_to_vec2_num3(scene.cam_right);
    let cam_up = split_vec3_to_vec2_num3(scene.cam_up);
    let cam_forward = split_vec3_to_vec2_num3(scene.cam_forward);

    let dir = vec2_num3_normalize(vec2_num3_add(
        vec2_num3_add(
            vec2_num3_scale_f(cam_right, local_dir.x),
            vec2_num3_scale_f(cam_up, local_dir.y),
        ),
        vec2_num3_scale_f(cam_forward, local_dir.z),
    ));

    let step_width = ds_from_split(scene.step_width);
    let x_offset = ds_mul(ds_new(frag_x - half_w), step_width);
    let y_offset = ds_mul(ds_new(frag_y - half_h), step_width);
    let mid = Vec2Num3 {
        x: ds_from_split(scene.mid_x),
        y: ds_from_split(scene.mid_y),
        z: ds_from_split(scene.mid_z),
    };

    let origin = vec2_num3_add(
        vec2_num3_add(
            vec2_num3_add(mid, vec2_num3_scale(cam_forward, ds_from_split(scene.z_start_delta))),
            vec2_num3_scale(cam_right, x_offset),
        ),
        vec2_num3_scale(cam_up, y_offset),
    );

    (origin, dir)
}

fn scene_destop_at_steps_vec2(scene: &GpuDsUploads, depth_steps: Vec2f) -> Vec2f {
    ds_mul(
        ds_from_split(scene.de_stop),
        ds_add(ds_new(1.0), ds_mul(ds_abs(depth_steps), ds_from_split(scene.de_stop_factor))),
    )
}

fn ray_march_vec2_uploads(
    scene: &GpuDsUploads,
    origin: Vec2Num3,
    dir: Vec2Num3,
    seed0: u32,
) -> Vec2MarchHit {
    let mut t = ds_new(0.0);
    let mut last_de;
    let mut last_step;
    let mut rsfmul = ds_new(1.0);
    let mut step_count = 0.0f32;
    let mut seed = seed0;
    let mut first_step = scene.first_step_random;

    let pos = vec2_num3_add(origin, vec2_num3_scale(dir, t));
    let (iters, de) = calc_de_vec2_uploads(scene, pos.x, pos.y, pos.z);
    let current_destop = scene_destop_at_steps_vec2(scene, ds_div(t, ds_from_split(scene.step_width)));
    if iters >= scene.max_iters || ds_lt(de, current_destop) {
        return Vec2MarchHit {
            depth: t,
            iters,
            shadow_steps: step_count.round().clamp(0.0, 1023.0) as i32,
            hit: true,
        };
    }

    last_step = ds_max(
        ds_mul(de, ds_from_split(scene.s_z_step_div)),
        ds_mul(ds_from_split(scene.step_width), ds_new(0.11)),
    );
    last_de = de;

    for _ in 0..2_000_000 {
        let current_destop = scene_destop_at_steps_vec2(scene, ds_div(t, ds_from_split(scene.step_width)));
        let pos = vec2_num3_add(origin, vec2_num3_scale(dir, t));
        let (iters, mut de) = calc_de_vec2_uploads(scene, pos.x, pos.y, pos.z);

        let max_de = ds_add(last_de, last_step);
        if ds_gt(de, max_de) {
            de = max_de;
        }

        if iters < scene.max_iters && !ds_lt(de, current_destop) {
            let mut step = ds_max(
                ds_mul(
                    ds_mul(ds_sub(de, ds_mul(ds_from_split(scene.ms_de_sub), current_destop)), ds_from_split(scene.s_z_step_div)),
                    rsfmul,
                ),
                ds_mul(ds_from_split(scene.step_width), ds_new(0.11)),
            );
            let max_step_here = ds_mul(
                ds_max(current_destop, ds_mul(ds_from_split(scene.step_width), ds_new(0.4))),
                ds_from_split(scene.mct_mh04_zsd),
            );

            if ds_lt(max_step_here, step) {
                if scene.d_fog_on_it == 0 || iters == scene.d_fog_on_it {
                    step_count += ds_to_f(max_step_here) / ds_to_f(step).max(1.0e-30);
                }
                step = max_step_here;
            } else if scene.d_fog_on_it == 0 || iters == scene.d_fog_on_it {
                step_count += 1.0;
            }

            if first_step {
                seed = seed.wrapping_mul(214013).wrapping_add(2531011);
                first_step = false;
                let jitter = ((seed & 0x7fff_ffff) as f32) * (1.0 / 2147483647.0);
                step = ds_mul(step, ds_new(jitter));
            }

            let de_eps = ds_add(de, ds_new(1.0e-30));
            if ds_gt(last_de, de_eps) {
                let denom = ds_to_f(ds_sub(last_de, de)).max(1.0e-30);
                let ratio = ds_to_f(last_step) / denom;
                rsfmul = if ratio < 1.0 {
                    ds_new(ratio.max(0.5))
                } else {
                    ds_new(1.0)
                };
            } else {
                rsfmul = ds_new(1.0);
            }

            last_de = de;
            last_step = step;
            t = ds_add(t, step);

            if ds_gt(t, ds_from_split(scene.max_ray_length)) {
                return Vec2MarchHit {
                    depth: t,
                    iters: 0,
                    shadow_steps: 0,
                    hit: false,
                };
            }
        } else {
            let mut refine_step = ds_mul(last_step, ds_new(-0.5));
            for _ in 0..scene.bin_search_steps {
                t = ds_add(t, refine_step);
                let rpos = vec2_num3_add(origin, vec2_num3_scale(dir, t));
                let destop_here = scene_destop_at_steps_vec2(scene, ds_div(t, ds_from_split(scene.step_width)));
                let (ri, rd) = calc_de_vec2_uploads(scene, rpos.x, rpos.y, rpos.z);
                if ds_lt(rd, destop_here) || ri >= scene.max_iters {
                    refine_step = ds_mul(ds_abs(refine_step), ds_new(-0.55));
                } else {
                    refine_step = ds_mul(ds_abs(refine_step), ds_new(0.55));
                }
            }

            let hit_pos = vec2_num3_add(origin, vec2_num3_scale(dir, t));
            let (final_iters, _) = calc_de_vec2_uploads(scene, hit_pos.x, hit_pos.y, hit_pos.z);
            return Vec2MarchHit {
                depth: t,
                iters: final_iters,
                shadow_steps: step_count.round().clamp(0.0, 1023.0) as i32,
                hit: true,
            };
        }
    }

    Vec2MarchHit {
        depth: t,
        iters: 0,
        shadow_steps: 0,
        hit: false,
    }
}

#[derive(Clone, Copy, Debug)]
struct Vec2SurfaceSample {
    normal: F3,
    roughness: f32,
}

fn offset_vec2_pos(base: Vec2Num3, dir: F3, scale: f32) -> Vec2Num3 {
    Vec2Num3 {
        x: ds_add_f(base.x, dir.x * scale),
        y: ds_add_f(base.y, dir.y * scale),
        z: ds_add_f(base.z, dir.z * scale),
    }
}

fn calc_de_vec2_at_pos(scene: &GpuDsUploads, pos: Vec2Num3) -> f32 {
    ds_to_f(calc_de_vec2_uploads(scene, pos.x, pos.y, pos.z).1)
}

fn calc_raw_de_vec2_at_pos(scene: &GpuDsUploads, pos: Vec2Num3) -> f32 {
    ds_to_f(hybrid_de_vec2_uploads(scene, pos.x, pos.y, pos.z).1)
}

fn surface_sample_vec2_uploads(
    scene: &GpuDsUploads,
    hit_pos: Vec2Num3,
    depth: Vec2f,
) -> Vec2SurfaceSample {
    let step_width = ds_to_f(ds_from_split(scene.step_width)).max(1.0e-30);
    let de_stop_header = ds_to_f(ds_from_split(scene.de_stop_header));
    let de_stop_factor = ds_to_f(ds_from_split(scene.de_stop_factor));
    let forward = split_vec3_to_f3(scene.cam_forward).normalize();
    let right = split_vec3_to_f3(scene.cam_right).normalize();
    let up = split_vec3_to_f3(scene.cam_up).normalize();

    let m_zz = ds_to_f(depth) / step_width;
    let n_offset = de_stop_header.min(1.0) * (1.0 + m_zz.abs() * de_stop_factor) * 0.15 * step_width;

    let fwd = forward.scale(n_offset);
    let rt = right.scale(n_offset);
    let upv = up.scale(n_offset);

    let dz = calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, forward, n_offset))
        - calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, forward, -n_offset));
    let dx = calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, right, n_offset))
        - calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, right, -n_offset));
    let dy = calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, up, n_offset))
        - calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, up, -n_offset));

    let normal_basis = F3 { x: dx, y: dy, z: dz };
    let normal_coarse = rt
        .normalize()
        .scale(dx)
        .add(upv.normalize().scale(dy))
        .add(fwd.normalize().scale(dz))
        .normalize();

    let smooth_n = scene.sm_normals.min(8);
    if smooth_n <= 0 {
        return Vec2SurfaceSample {
            normal: normal_coarse,
            roughness: 0.0,
        };
    }

    let noffset2 = n_offset * 2.0;
    let step_snorm = noffset2 * 3.0 / (smooth_n as f32 + 0.5);
    if step_snorm <= 1.0e-30 {
        return Vec2SurfaceSample {
            normal: normal_coarse,
            roughness: 0.0,
        };
    }

    let create_xy_vecs_from_normals_mb3d = |n: F3| {
        let d = n.y * n.y + n.x * n.x;
        if d < 1.0e-25 {
            return (
                F3 { x: 1.0, y: 0.0, z: 0.0 },
                F3 { x: 0.0, y: 1.0, z: 0.0 },
            );
        }
        let denom = (d + n.z * n.z + 1.0e-30).sqrt();
        let half_angle = (-n.z / denom).clamp(-1.0, 1.0).acos() * 0.5;
        let (mut sin_a, cos_a) = half_angle.sin_cos();
        sin_a /= d.sqrt();
        let d0 = -n.y * sin_a;
        let d1 = n.x * sin_a;
        let vx = F3 {
            x: 1.0 - 2.0 * d1 * d1,
            y: 2.0 * d0 * d1,
            z: 2.0 * d1 * cos_a,
        };
        let vy = F3 {
            x: vx.y,
            y: 1.0 - 2.0 * d0 * d0,
            z: -2.0 * d0 * cos_a,
        };
        (vx, vy)
    };

    let rotate_vector_reverse_basis = |v: F3| {
        right
            .scale(v.x)
            .add(up.scale(v.y))
            .add(forward.scale(v.z))
    };

    let mut dnn = calc_de_vec2_at_pos(scene, hit_pos);
    if smooth_n < 8 {
        dnn = (
            dnn
                + calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, right, -noffset2))
                + calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, right, noffset2))
                + calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, up, -noffset2))
                + calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, up, noffset2))
        ) * 0.2;
    }

    let (vx_basis, vy_basis) = create_xy_vecs_from_normals_mb3d(normal_basis);
    let vx = rotate_vector_reverse_basis(vx_basis).normalize();
    let vy = rotate_vector_reverse_basis(vy_basis).normalize();
    let mut nn1 = 0.0f32;
    let mut nn2 = 0.0f32;
    let mut ds1 = 0.0f32;
    let mut ds2 = 0.0f32;

    for it in -smooth_n..=smooth_n {
        if it == 0 {
            continue;
        }
        let t = it as f32 * step_snorm;
        let de_x = calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, vx, t));
        let dt = (de_x - dnn) / it as f32;
        nn1 += dt;
        ds1 += dt * dt;
    }
    for it in -smooth_n..=smooth_n {
        if it == 0 {
            continue;
        }
        let t = it as f32 * step_snorm;
        let de_y = calc_de_vec2_at_pos(scene, offset_vec2_pos(hit_pos, vy, t));
        let dt = (de_y - dnn) / it as f32;
        nn2 += dt;
        ds2 += dt * dt;
    }

    let d_m = (smooth_n * 2) as f32;
    let d_t2 = noffset2 * 0.5 / (d_m * step_snorm).max(1.0e-30);
    let mut d_sg = ds1 * d_m - nn1 * nn1;
    d_sg += ds2 * d_m - nn2 * nn2;

    let denom = (normal_basis.dot(normal_basis) + 1.0e-30).max(1.0e-30);
    let mut rough = ((d_sg * 7.0 * d_t2 * d_t2) / denom).max(0.0).sqrt() - 0.05;
    rough = rough.clamp(0.0, 1.0);

    let out_n = rotate_vector_reverse_basis(F3 {
        x: normal_basis.x + nn1 * d_t2,
        y: normal_basis.y + nn2 * d_t2,
        z: normal_basis.z,
    })
    .normalize();

    Vec2SurfaceSample {
        normal: out_n,
        roughness: rough,
    }
}

fn soft_hs_bits_vec2_uploads(
    scene: &GpuDsUploads,
    hit_pos: Vec2Num3,
    depth_world: f32,
    ray_dir: F3,
    normal: F3,
    light_dir: F3,
    i_light_pos: u8,
    y: usize,
    width: usize,
    height: usize,
) -> i32 {
    let step_width = ds_to_f(ds_from_split(scene.step_width)).max(1.0e-30);
    let max_ray_length = ds_to_f(ds_from_split(scene.max_ray_length));
    let fov_y = scene.fov_y;
    let hs_max_length_multiplier = ds_to_f(ds_from_split(scene.hs_max_length_multiplier)).max(1.0e-30);
    let soft_shadow_radius = ds_to_f(ds_from_split(scene.soft_shadow_radius)).max(1.0e-30);
    let s_z_step_div_raw = ds_to_f(ds_from_split(scene.s_z_step_div_raw));
    let de_stop = ds_to_f(ds_from_split(scene.de_stop));
    let de_stop_factor = ds_to_f(ds_from_split(scene.de_stop_factor));
    let ms_de_sub = ds_to_f(ds_from_split(scene.ms_de_sub));
    let mct_mh04_zsd = ds_to_f(ds_from_split(scene.mct_mh04_zsd));

    let view_dir = ray_dir.normalize();

    let mut refined_pos = hit_pos;
    let mut refined_depth = depth_world.max(0.0);
    let mut refine_step = step_width;
    for _ in 0..8 {
        let de_ref = calc_de_vec2_at_pos(scene, refined_pos);
        let de_stop_ref = de_stop * (1.0 + (refined_depth / step_width).abs() * de_stop_factor);
        if de_ref <= de_stop_ref {
            refined_pos = offset_vec2_pos(refined_pos, view_dir, -refine_step);
            refined_depth = (refined_depth - refine_step).max(0.0);
        } else {
            refined_pos = offset_vec2_pos(refined_pos, view_dir, refine_step);
            refined_depth += refine_step;
        }
        refine_step *= 0.5;
    }

    let mut depth_steps = refined_depth / step_width - 0.1;
    if depth_steps < 0.0 {
        depth_steps = 0.0;
    }
    let mut pos = offset_vec2_pos(refined_pos, view_dir, -0.1 * step_width);

    let zz = depth_steps.abs();
    let zend_steps = (max_ray_length / step_width).max(1.0e-30);
    let fov_y_rad = fov_y * std::f32::consts::PI / 180.0;
    let max_l_hs = (width as f32 + y as f32)
        * 0.6
        * (1.0 + 0.5 * zz.min(zend_steps * 0.4) * fov_y_rad.max(0.0) / (height as f32).max(1.0))
        * hs_max_length_multiplier;
    if max_l_hs <= 0.0 {
        return 63;
    }

    if (i_light_pos & 1) != 0 {
        return 63;
    }

    let mut zr_soft = 1.0f32;
    let zr_s_mul = 80.0 / soft_shadow_radius;
    let n = normal.normalize();
    let l = light_dir.normalize();
    let v = view_dir;
    let hs_vec = l.scale(-1.0);
    let zz2mul = -hs_vec.dot(v);

    if n.dot(hs_vec) > 0.0 {
        return 0;
    }

    let mut d_t1 = max_l_hs;
    let mut zz2_steps = depth_steps;
    let mut ms_de_stop_world = de_stop * (1.0 + zz2_steps.abs() * de_stop_factor);
    let mut step_factor_diff = 1.0f32;
    let mut de_world = calc_de_vec2_at_pos(scene, pos);

    loop {
        let r_last_de_world = de_world;
        let max_step_world = (ms_de_stop_world.max(0.4 * step_width)) * mct_mh04_zsd;
        let r_last_step_world = ((de_world - ms_de_sub * ms_de_stop_world)
            * s_z_step_div_raw
            * step_factor_diff)
            .max(0.11 * step_width)
            .min(max_step_world);
        if r_last_step_world <= 0.0 {
            break;
        }

        let r_last_step_width = r_last_step_world / step_width;
        d_t1 -= r_last_step_width;
        pos = offset_vec2_pos(pos, l, r_last_step_world);
        zz2_steps += r_last_step_width * zz2mul;
        ms_de_stop_world = de_stop * (1.0 + zz2_steps.abs() * de_stop_factor);

        de_world = calc_de_vec2_at_pos(scene, pos);
        let traveled = (max_l_hs - d_t1).max(0.0);
        let soft_term =
            ((de_world - ms_de_stop_world) / step_width) * zr_s_mul / (traveled + 0.11)
                + (traveled / max_l_hs.max(1.0e-30)).powi(8);
        zr_soft = zr_soft.min(soft_term);

        if de_world <= ms_de_stop_world {
            break;
        }
        if de_world > r_last_de_world + r_last_step_world {
            de_world = r_last_de_world + r_last_step_world;
        }
        if r_last_de_world > de_world + 1.0e-30 {
            let s_tmp = r_last_step_world / (r_last_de_world - de_world);
            if s_tmp < 1.0 {
                step_factor_diff = s_tmp.max(0.5);
            } else {
                step_factor_diff = 1.0;
            }
        } else {
            step_factor_diff = 1.0;
        }
        if d_t1 < 0.0 {
            break;
        }
    }

    (zr_soft.clamp(0.0, 1.0) * 63.4)
        .round()
        .clamp(0.0, 63.0) as i32
}

fn ao_step_jitter_vec2(pixel_x: i32, pixel_y: i32, ray_idx: usize) -> f32 {
    let mut v = (pixel_x as u32).wrapping_mul(73_856_093)
        ^ (pixel_y as u32).wrapping_mul(19_349_663)
        ^ (ray_idx as u32).wrapping_mul(83_492_791);
    v ^= v >> 13;
    v = v.wrapping_mul(1_274_126_177);
    (v as f32) / (u32::MAX as f32)
}

fn deao_vec2_uploads(
    scene: &GpuDsUploads,
    hit_pos: Vec2Num3,
    normal: F3,
    depth: f32,
    pixel_x: i32,
    pixel_y: i32,
    width: usize,
    height: usize,
    ssao: &m3p::M3PSSAO,
) -> f32 {
    if !(ssao.calc_amb_shadow && ssao.mode == 3) {
        return 1.0;
    }

    let step_width = ds_to_f(ds_from_split(scene.step_width)).max(1.0e-30);
    let de_stop_factor = ds_to_f(ds_from_split(scene.de_stop_factor));
    let de_stop_header = ds_to_f(ds_from_split(scene.de_stop_header)).max(1.0e-30);
    let de_scale = ds_to_f(ds_from_split(scene.de_scale));
    let m_zz = depth / step_width;

    let normal_basis_w = normal.normalize();
    let axis = if normal_basis_w.x.abs() > 0.1 {
        F3 { x: 0.0, y: 1.0, z: 0.0 }
    } else {
        F3 { x: 1.0, y: 0.0, z: 0.0 }
    };
    let normal_basis_u = f3_cross(axis, normal_basis_w).normalize();
    let normal_basis_v = f3_cross(normal_basis_w, normal_basis_u);
    let make_world_dir = |polar: f32, azimuth: f32| {
        let (sy, cy) = polar.sin_cos();
        let (sz, cz) = azimuth.sin_cos();
        let local_dir = F3 {
            x: sy * cz,
            y: sy * sz,
            z: cy,
        };
        normal_basis_u
            .scale(local_dir.x)
            .add(normal_basis_v.scale(local_dir.y))
            .add(normal_basis_w.scale(local_dir.z))
            .normalize()
    };

    let (dither_y, dither_x) = if ssao.ao_dithering > 0 {
        let denom = ssao.ao_dithering as f32;
        (
            (pixel_y.rem_euclid(ssao.ao_dithering + 1) as f32) * 0.5 / denom,
            (pixel_x.rem_euclid(ssao.ao_dithering + 1) as f32) * 0.5 / denom,
        )
    } else {
        (0.25, 0.0)
    };

    let mut rot_m = Vec::new();
    let (_row_abr, d_step_mul, d_min_a_dif, correction_weight) = if ssao.quality == 0 {
        let row_abr = std::f32::consts::PI / 3.0;
        let polar = if ssao.ao_dithering > 0 {
            (dither_y + 0.5) * 50.0_f32.to_radians()
        } else {
            row_abr * 0.5
        };
        for itmp in 0..3 {
            let azimuth = (itmp as f32 + dither_x) * std::f32::consts::PI * 2.0 / 3.0;
            rot_m.push(make_world_dir(polar, azimuth));
        }
        (row_abr, 1.8, -1.0, 0.3)
    } else {
        let row_abr = std::f32::consts::PI * 0.5 / (ssao.quality as f32 + 0.9);
        if dither_y >= 0.1 {
            rot_m.push(normal_basis_w);
        }
        for iy in 1..=ssao.quality {
            let row_count = ((iy as f32 * row_abr).sin() * std::f32::consts::PI * 2.0 / row_abr)
                .round()
                .max(1.0) as i32;
            let polar = row_abr * (iy as f32 + dither_y - 0.25);
            for ix in 0..row_count {
                let azimuth = (ix as f32 + dither_x) * std::f32::consts::PI * 2.0 / row_count as f32;
                rot_m.push(make_world_dir(polar, azimuth));
            }
        }
        let correction_weight = if ssao.quality == 1 { 0.2 } else { 0.1666 };
        (
            row_abr,
            1.0 + row_abr.sin(),
            (row_abr * 1.2).cos(),
            correction_weight,
        )
    };

    if scene.adaptive_ao_subsampling && rot_m.len() >= 12 {
        let mut write = 0usize;
        for read in (0..rot_m.len()).step_by(2) {
            rot_m[write] = rot_m[read];
            write += 1;
        }
        rot_m.truncate(write);
    }

    let ray_count = rot_m.len();
    if ray_count == 0 {
        return 1.0;
    }

    let mut min_ra = vec![0.0f32; ray_count];
    let mut s_add = vec![0.0f32; ray_count];

    let de_mul = ((ray_count as f32) * 0.5).sqrt();
    let overlap_abr = 1.2 / (1.0 / de_mul).asin();
    let step_ao = 1.0 + m_zz.abs() * de_stop_factor;
    let s_max_d = ssao.deao_max_l as f32
        * 0.5
        * ((width * width + height * height) as f32).sqrt();

    let mut ms_de_stop_steps = de_stop_header * step_ao;
    if ms_de_stop_steps > 10000.0 {
        ms_de_stop_steps = 10000.0;
    }
    if ms_de_stop_steps < de_stop_header {
        ms_de_stop_steps = de_stop_header;
    }

    let step_ao_actual = ms_de_stop_steps / de_stop_header;
    let max_dist_steps = s_max_d * step_ao_actual.sqrt();
    let ms_de_stop = if scene.b_vary_de_stop {
        ms_de_stop_steps / (d_step_mul * d_step_mul)
    } else {
        de_stop_header / (d_step_mul * d_step_mul)
    };

    for i in 0..ray_count {
        let s_vec = rot_m[i];
        let mut dt1 = step_ao_actual * d_step_mul;
        let mut s_tmp = 1.0f32;
        let mut b_first_step = scene.first_step_random;

        loop {
            let mut b_end = false;

            if b_first_step {
                b_first_step = false;
                dt1 *= ao_step_jitter_vec2(pixel_x, pixel_y, i) * 1.5 + 0.5;
            } else if dt1 > max_dist_steps {
                dt1 = max_dist_steps;
                b_end = true;
            }

            let probe_pos = offset_vec2_pos(hit_pos, s_vec, dt1 * step_width);
            let de_world = calc_raw_de_vec2_at_pos(scene, probe_pos);
            let dt2 = de_world * de_scale / step_width;

            let md_d10 = 0.1 / (max_dist_steps * de_mul);
            let val = ((dt2 - ms_de_stop) / dt1 + md_d10).min(s_tmp);
            if val < s_tmp {
                s_tmp = val;
            }

            if s_tmp < 0.02 {
                break;
            }

            let step_add = if dt2 > dt1 * d_step_mul {
                dt2
            } else {
                dt1 * d_step_mul
            };
            dt1 += step_add;

            if b_end {
                break;
            }
        }

        min_ra[i] = s_tmp.max(0.0) * de_mul;
    }

    let mut final_ao_val = 0.0f32;
    for iy in 0..ray_count {
        let max_add = 1.0 - min_ra[iy];
        if max_add > 0.0 {
            for ix in 0..ray_count {
                if ix != iy {
                    let d_tmp = rot_m[iy].dot(rot_m[ix]);
                    if d_tmp > d_min_a_dif {
                        let overlap = min_ra[ix] - d_tmp.acos() * overlap_abr + 1.0;
                        if overlap > 0.0 {
                            s_add[iy] += max_add.min(overlap) * correction_weight;
                        }
                    }
                }
            }
        }
    }

    for iy in 0..ray_count {
        final_ao_val += (s_add[iy] + min_ra[iy]).min(1.0);
    }

    let amb_shadow_norm = (1.0 - final_ao_val / ray_count as f32).clamp(0.0, 1.0);
    let s_amplitude = ssao.amb_shad as f32;
    let mut d_amb_s = if s_amplitude > 1.0 {
        let mut d = 1.0 - amb_shadow_norm;
        d = d + (s_amplitude - 1.0) * (d * d - d);
        d
    } else {
        1.0 - s_amplitude * amb_shadow_norm
    };
    d_amb_s = d_amb_s.clamp(0.0, 1.0);
    d_amb_s
}

fn render_vec2_ported_pixels(
    scene: &CathedralScene,
    width: usize,
) -> Result<(Vec<u8>, usize, usize, usize), String> {
    let scale = (width as f64 / scene.base_width).max(0.001);
    let mut params = render::RenderParams::from_m3p(&scene.m3p);
    params.apply_image_scale(scale);
    let width = params.camera.width as usize;
    let height = params.camera.height as usize;

    let uploads = build_ds_uploads(scene, width);
    let lighting_state = build_standalone_lighting_state(&scene.m3p.lighting, &params.camera, &params);
    let soft_hs_light = standalone_soft_hs_light_dir(&scene.m3p.lighting, &params.camera, &params);

    let mut pixels = vec![0u8; width * height * 4];
    let mut hits = 0usize;

    for y in 0..height {
        let y_pos = (y as f64 + 0.5) / height.max(1) as f64;
        for x in 0..width {
            let (origin, dir) = shaderlike_ray_for_pixel_vec2(&uploads, width, height, x, y);
            let hit = ray_march_vec2_uploads(&uploads, origin, dir, seed_for_pixel(x, y));
            let idx = (y * width + x) * 4;

            if !hit.hit {
                pixels[idx] = 10;
                pixels[idx + 1] = 10;
                pixels[idx + 2] = 15;
                pixels[idx + 3] = 255;
                continue;
            }

            hits += 1;
            let hit_pos = vec2_num3_add(origin, vec2_num3_scale(dir, hit.depth));
            let surface = surface_sample_vec2_uploads(&uploads, hit_pos, hit.depth);
            let view_dir = vec2_num3_to_f3(dir);

            let mut shadow_word = hit.shadow_steps & 0x3ff;
            if let Some((_li, light_dir, i_light_pos)) = soft_hs_light {
                shadow_word |= 0xFC00;
                let soft_bits = soft_hs_bits_vec2_uploads(
                    &uploads,
                    hit_pos,
                    ds_to_f(hit.depth),
                    view_dir,
                    surface.normal,
                    F3 {
                        x: light_dir.x as f32,
                        y: light_dir.y as f32,
                        z: light_dir.z as f32,
                    },
                    i_light_pos,
                    y,
                    width,
                    height,
                );
                shadow_word = (shadow_word & 0x03FF) | (soft_bits << 10);
            }

            let final_ao = deao_vec2_uploads(
                &uploads,
                hit_pos,
                surface.normal,
                ds_to_f(hit.depth),
                x as i32,
                y as i32,
                width,
                height,
                &scene.m3p.ssao,
            );

            let color = standalone_shade_with_final_ao_mb3d(
                &lighting_state,
                &scene.m3p.ssao,
                &params,
                render::Vec3::new(
                    surface.normal.x as f64,
                    surface.normal.y as f64,
                    surface.normal.z as f64,
                ),
                surface.roughness as f64,
                render::Vec3::new(
                    -view_dir.x as f64,
                    -view_dir.y as f64,
                    -view_dir.z as f64,
                ),
                hit.iters,
                shadow_word,
                final_ao as f64,
                ds_to_f(hit.depth) as f64,
                y_pos,
                params.max_ray_length,
            );

            pixels[idx] = color[0];
            pixels[idx + 1] = color[1];
            pixels[idx + 2] = color[2];
            pixels[idx + 3] = 255;
        }
    }

    Ok((pixels, width, height, hits))
}
#[test]
#[ignore = "manual render of ported vec2 cpu path at 480x270"]
fn render_ported_vec2_25pct() {
    let path = format!("{}/../../local/mb3d/cathedral.m3p", env!("CARGO_MANIFEST_DIR"));
    let scene = load_cathedral_scene(&path).expect("cathedral scene should load");
    let width = 480usize;
    let (pixels, out_w, out_h, hits) =
        render_vec2_ported_pixels(&scene, width).expect("ported vec2 pixels should render");
    let output_path = "/tmp/mb3d_ported_vec2_480x270.png";
    encode_png(output_path, &pixels, out_w, out_h).expect("ported vec2 png should encode");
    println!(
        "ported vec2 render: {}x{} hits={} output={}",
        out_w, out_h, hits, output_path
    );
}

#[test]
#[ignore = "manual debug of vec2 center primary march progress"]
fn debug_vec2_center_primary_progress() {
    let path = format!("{}/../../local/mb3d/cathedral.m3p", env!("CARGO_MANIFEST_DIR"));
    let scene = load_cathedral_scene(&path).expect("cathedral scene should load");
    let width = 960usize;
    let scale = (width as f64 / scene.base_width).max(0.001);
    let mut params = render::RenderParams::from_m3p(&scene.m3p);
    params.apply_image_scale(scale);
    let out_w = params.camera.width as usize;
    let out_h = params.camera.height as usize;
    let center_x = out_w / 2;
    let center_y = out_h / 2;

    let uploads = build_ds_uploads(&scene, out_w);
    let (origin, dir) = shaderlike_ray_for_pixel_vec2(&uploads, out_w, out_h, center_x, center_y);
    let pos0 = vec2_num3_add(origin, vec2_num3_scale(dir, ds_new(0.0)));
    let (iters0, de0) = calc_de_vec2_uploads(&uploads, pos0.x, pos0.y, pos0.z);
    let stop0 = scene_destop_at_steps_vec2(&uploads, ds_new(0.0));

    println!(
        "center {}x{} pixel=({}, {}) step_width={:.9e} de_stop={:.9e} max_ray_length={:.9e} s_z_step_div={:.9e} ms_de_sub={:.9e} mct_mh04_zsd={:.9e}",
        out_w,
        out_h,
        center_x,
        center_y,
        ds_to_f(ds_from_split(uploads.step_width)),
        ds_to_f(ds_from_split(uploads.de_stop)),
        ds_to_f(ds_from_split(uploads.max_ray_length)),
        ds_to_f(ds_from_split(uploads.s_z_step_div)),
        ds_to_f(ds_from_split(uploads.ms_de_sub)),
        ds_to_f(ds_from_split(uploads.mct_mh04_zsd)),
    );
    println!(
        "origin=({:.9e}, {:.9e}, {:.9e}) dir=({:.9e}, {:.9e}, {:.9e})",
        ds_to_f(origin.x),
        ds_to_f(origin.y),
        ds_to_f(origin.z),
        ds_to_f(dir.x),
        ds_to_f(dir.y),
        ds_to_f(dir.z),
    );
    println!(
        "first_eval: iters={} de={:.9e} stop={:.9e} ratio={:.9e}",
        iters0,
        ds_to_f(de0),
        ds_to_f(stop0),
        ds_to_f(de0) / ds_to_f(stop0).max(1.0e-30),
    );

    let limits = [16384usize, 65536usize, 262144usize, 1048576usize, 2000000usize];
    for limit in limits {
        let mut t = ds_new(0.0);
        let mut last_step;
        let mut last_de;
        let mut rsfmul = ds_new(1.0);
        let pos = vec2_num3_add(origin, vec2_num3_scale(dir, t));
        let (iters, de) = calc_de_vec2_uploads(&uploads, pos.x, pos.y, pos.z);
        let current_destop = scene_destop_at_steps_vec2(&uploads, ds_div(t, ds_from_split(uploads.step_width)));
        if iters >= uploads.max_iters || ds_lt(de, current_destop) {
            println!("limit={} hit immediately", limit);
            continue;
        }
        last_step = ds_max(
            ds_mul(de, ds_from_split(uploads.s_z_step_div)),
            ds_mul(ds_from_split(uploads.step_width), ds_new(0.11)),
        );
        last_de = de;

        let mut hit = false;
        let mut miss = false;
        let mut steps_taken = 0usize;
        for step_idx in 0..limit {
            let current_destop =
                scene_destop_at_steps_vec2(&uploads, ds_div(t, ds_from_split(uploads.step_width)));
            let pos = vec2_num3_add(origin, vec2_num3_scale(dir, t));
            let (iters, mut de) = calc_de_vec2_uploads(&uploads, pos.x, pos.y, pos.z);
            let max_de = ds_add(last_de, last_step);
            if ds_gt(de, max_de) {
                de = max_de;
            }

            if iters < uploads.max_iters && !ds_lt(de, current_destop) {
                let mut step = ds_max(
                    ds_mul(
                        ds_mul(
                            ds_sub(de, ds_mul(ds_from_split(uploads.ms_de_sub), current_destop)),
                            ds_from_split(uploads.s_z_step_div),
                        ),
                        rsfmul,
                    ),
                    ds_mul(ds_from_split(uploads.step_width), ds_new(0.11)),
                );
                let max_step_here = ds_mul(
                    ds_max(current_destop, ds_mul(ds_from_split(uploads.step_width), ds_new(0.4))),
                    ds_from_split(uploads.mct_mh04_zsd),
                );
                if ds_lt(max_step_here, step) {
                    step = max_step_here;
                }

                let de_eps = ds_add(de, ds_new(1.0e-30));
                if ds_gt(last_de, de_eps) {
                    let denom = ds_to_f(ds_sub(last_de, de)).max(1.0e-30);
                    let ratio = ds_to_f(last_step) / denom;
                    rsfmul = if ratio < 1.0 {
                        ds_new(ratio.max(0.5))
                    } else {
                        ds_new(1.0)
                    };
                } else {
                    rsfmul = ds_new(1.0);
                }

                last_de = de;
                last_step = step;
                t = ds_add(t, step);
                steps_taken = step_idx + 1;

                if ds_gt(t, ds_from_split(uploads.max_ray_length)) {
                    miss = true;
                    break;
                }
            } else {
                hit = true;
                steps_taken = step_idx + 1;
                break;
            }
        }

        let state = if hit {
            "hit"
        } else if miss {
            "miss"
        } else {
            "exhausted"
        };
        println!(
            "limit={} state={} steps_taken={} depth={:.9e} depth_steps={:.9e} last_de={:.9e}",
            limit,
            state,
            steps_taken,
            ds_to_f(t),
            ds_to_f(ds_div(t, ds_from_split(uploads.step_width))),
            ds_to_f(last_de),
        );
    }
}

#[test]
#[ignore = "manual compare of direct vec2 primary marcher against working self-contained ds primary"]
fn compare_direct_vec2_primary_to_selfcontained_ds_tiny() {
    let path = format!("{}/../../local/mb3d/cathedral.m3p", env!("CARGO_MANIFEST_DIR"));
    let scene = load_cathedral_scene(&path).expect("cathedral scene should load");
    let width = 96usize;
    let scale = (width as f64 / scene.base_width).max(0.001);
    let mut params = render::RenderParams::from_m3p(&scene.m3p);
    params.apply_image_scale(scale);
    let out_w = params.camera.width as usize;
    let out_h = params.camera.height as usize;

    let uploads = build_ds_uploads(&scene, out_w);
    let orbit_ds = orbit_scene_from_ds_uploads::<Ds>(&uploads);
    let march_ds = build_march_params_from_ds_uploads::<Ds>(&uploads);

    let mut mismatched_hits = 0usize;
    let mut mismatched_iters = 0usize;
    let mut max_depth_diff = 0.0f32;
    let mut sum_depth_diff = 0.0f64;
    let mut depth_diff_count = 0usize;

    for y in 0..out_h {
        for x in 0..out_w {
            let (origin_v2, dir_v2) = shaderlike_ray_for_pixel_vec2(&uploads, out_w, out_h, x, y);
            let vec2_hit = ray_march_vec2_uploads(&uploads, origin_v2, dir_v2, seed_for_pixel(x, y));

            let seed = seed_for_pixel(x, y);
            let (origin_ds, dir_ds) = shaderlike_ray_for_pixel_num::<Ds>(&uploads, out_w, out_h, x, y);
            let ds_hit = ray_march_scene_num(&orbit_ds, &march_ds, origin_ds, dir_ds, seed);

            match (ds_hit, vec2_hit.hit) {
                (MarchResult::Miss, false) => {}
                (MarchResult::Miss, true) | (MarchResult::Hit { .. }, false) => {
                    mismatched_hits += 1;
                }
                (
                    MarchResult::Hit {
                        depth: ds_depth,
                        iters: ds_iters,
                        ..
                    },
                    true,
                ) => {
                    if ds_iters != vec2_hit.iters {
                        mismatched_iters += 1;
                    }
                    let depth_diff = (ds_depth.to_f64() as f32 - ds_to_f(vec2_hit.depth)).abs();
                    max_depth_diff = max_depth_diff.max(depth_diff);
                    sum_depth_diff += depth_diff as f64;
                    depth_diff_count += 1;
                }
            }
        }
    }

    println!(
        "direct vec2 vs self-contained ds primary: {}x{} mismatched_hits={} mismatched_iters={} max_depth_diff={:.9e} avg_depth_diff={:.9e}",
        out_w,
        out_h,
        mismatched_hits,
        mismatched_iters,
        max_depth_diff,
        sum_depth_diff / depth_diff_count.max(1) as f64
    );
}

#[test]
#[ignore = "manual compare of ported vec2 surface pass against self-contained ds surface pass"]
fn compare_direct_vec2_surface_to_selfcontained_ds_tiny() {
    let path = format!("{}/../../local/mb3d/cathedral.m3p", env!("CARGO_MANIFEST_DIR"));
    let scene = load_cathedral_scene(&path).expect("cathedral scene should load");
    let width = 96usize;
    let scale = (width as f64 / scene.base_width).max(0.001);
    let mut params = render::RenderParams::from_m3p(&scene.m3p);
    params.apply_image_scale(scale);
    let out_w = params.camera.width as usize;
    let out_h = params.camera.height as usize;

    let uploads = build_ds_uploads(&scene, out_w);
    let orbit_ds = orbit_scene_from_ds_uploads::<Ds>(&uploads);
    let march_ds = build_march_params_from_ds_uploads::<Ds>(&uploads);

    let mut hit_count = 0usize;
    let mut mismatched_hits = 0usize;
    let mut avg_angle_err = 0.0f64;
    let mut max_angle_err = 0.0f64;
    let mut avg_rough_err = 0.0f64;
    let mut max_rough_err = 0.0f64;

    for y in 0..out_h {
        for x in 0..out_w {
            let seed = seed_for_pixel(x, y);
            let (origin_v2, dir_v2) = shaderlike_ray_for_pixel_vec2(&uploads, out_w, out_h, x, y);
            let vec2_hit = ray_march_vec2_uploads(&uploads, origin_v2, dir_v2, seed);
            let (origin_ds, dir_ds) = shaderlike_ray_for_pixel_num::<Ds>(&uploads, out_w, out_h, x, y);
            let ds_hit = ray_march_scene_num(&orbit_ds, &march_ds, origin_ds, dir_ds, seed);

            match (ds_hit, vec2_hit.hit) {
                (MarchResult::Miss, false) => {}
                (MarchResult::Miss, true) | (MarchResult::Hit { .. }, false) => {
                    mismatched_hits += 1;
                }
                (
                    MarchResult::Hit {
                        depth: ds_depth,
                        ..
                    },
                    true,
                ) => {
                    hit_count += 1;
                    let hit_pos_v2 = vec2_num3_add(origin_v2, vec2_num3_scale(dir_v2, vec2_hit.depth));
                    let hit_pos_ds = origin_ds.add(dir_ds.scale(ds_depth));
                    let ds_surface = standalone_surface_sample_num(
                        &uploads,
                        &orbit_ds,
                        &march_ds,
                        hit_pos_ds,
                        ds_depth.to_f64(),
                    );
                    let v2_surface = surface_sample_vec2_uploads(&uploads, hit_pos_v2, vec2_hit.depth);
                    let dot = ds_surface
                        .normal
                        .normalize()
                        .dot(
                            render::Vec3::new(
                                v2_surface.normal.x as f64,
                                v2_surface.normal.y as f64,
                                v2_surface.normal.z as f64,
                            )
                            .normalize(),
                        )
                        .clamp(-1.0, 1.0);
                    let angle_err = dot.acos().to_degrees();
                    avg_angle_err += angle_err;
                    max_angle_err = max_angle_err.max(angle_err);
                    let rough_err = (ds_surface.roughness - v2_surface.roughness as f64).abs();
                    avg_rough_err += rough_err;
                    max_rough_err = max_rough_err.max(rough_err);
                }
            }
        }
    }

    println!(
        "ported vec2 vs self-contained ds surface: {}x{} hits={} mismatched_hits={} avg_angle_err={:.6} max_angle_err={:.6} avg_rough_err={:.6} max_rough_err={:.6}",
        out_w,
        out_h,
        hit_count,
        mismatched_hits,
        avg_angle_err / hit_count.max(1) as f64,
        max_angle_err,
        avg_rough_err / hit_count.max(1) as f64,
        max_rough_err,
    );
}

#[test]
#[ignore = "manual compare of ported vec2 soft shadow pass against self-contained ds soft shadow pass"]
fn compare_direct_vec2_soft_hs_to_selfcontained_ds_tiny() {
    let path = format!("{}/../../local/mb3d/cathedral.m3p", env!("CARGO_MANIFEST_DIR"));
    let scene = load_cathedral_scene(&path).expect("cathedral scene should load");
    let width = 96usize;
    let scale = (width as f64 / scene.base_width).max(0.001);
    let mut params = render::RenderParams::from_m3p(&scene.m3p);
    params.apply_image_scale(scale);
    let out_w = params.camera.width as usize;
    let out_h = params.camera.height as usize;

    let uploads = build_ds_uploads(&scene, out_w);
    let orbit_ds = orbit_scene_from_ds_uploads::<Ds>(&uploads);
    let march_ds = build_march_params_from_ds_uploads::<Ds>(&uploads);
    let (_li, light_dir_ds, i_light_pos) =
        standalone_soft_hs_light_dir(&scene.m3p.lighting, &params.camera, &params)
            .expect("cathedral should have a soft shadow light");
    let light_dir_v2 = F3 {
        x: light_dir_ds.x as f32,
        y: light_dir_ds.y as f32,
        z: light_dir_ds.z as f32,
    };

    let mut hit_count = 0usize;
    let mut mismatched_hits = 0usize;
    let mut avg_abs_err = 0.0f64;
    let mut max_abs_err = 0i32;

    for y in 0..out_h {
        for x in 0..out_w {
            let seed = seed_for_pixel(x, y);
            let (origin_v2, dir_v2) = shaderlike_ray_for_pixel_vec2(&uploads, out_w, out_h, x, y);
            let vec2_hit = ray_march_vec2_uploads(&uploads, origin_v2, dir_v2, seed);
            let (origin_ds, dir_ds) = shaderlike_ray_for_pixel_num::<Ds>(&uploads, out_w, out_h, x, y);
            let ds_hit = ray_march_scene_num(&orbit_ds, &march_ds, origin_ds, dir_ds, seed);

            match (ds_hit, vec2_hit.hit) {
                (MarchResult::Miss, false) => {}
                (MarchResult::Miss, true) | (MarchResult::Hit { .. }, false) => {
                    mismatched_hits += 1;
                }
                (
                    MarchResult::Hit {
                        depth: ds_depth,
                        ..
                    },
                    true,
                ) => {
                    hit_count += 1;
                    let hit_pos_v2 = vec2_num3_add(origin_v2, vec2_num3_scale(dir_v2, vec2_hit.depth));
                    let hit_pos_ds = origin_ds.add(dir_ds.scale(ds_depth));
                    let ds_surface = standalone_surface_sample_num(
                        &uploads,
                        &orbit_ds,
                        &march_ds,
                        hit_pos_ds,
                        ds_depth.to_f64(),
                    );
                    let ds_bits = standalone_soft_hs_bits_num(
                        &uploads,
                        &orbit_ds,
                        &march_ds,
                        hit_pos_ds,
                        ds_depth.to_f64(),
                        numvec3_to_vec3(dir_ds),
                        ds_surface.normal,
                        light_dir_ds,
                        i_light_pos,
                        y,
                        out_w,
                        out_h,
                    );
                    let vec2_bits = soft_hs_bits_vec2_uploads(
                        &uploads,
                        hit_pos_v2,
                        ds_to_f(vec2_hit.depth),
                        vec2_num3_to_f3(dir_v2),
                        F3 {
                            x: ds_surface.normal.x as f32,
                            y: ds_surface.normal.y as f32,
                            z: ds_surface.normal.z as f32,
                        },
                        light_dir_v2,
                        i_light_pos,
                        y,
                        out_w,
                        out_h,
                    );
                    let abs_err = (ds_bits - vec2_bits).abs();
                    avg_abs_err += abs_err as f64;
                    max_abs_err = max_abs_err.max(abs_err);
                }
            }
        }
    }

    println!(
        "ported vec2 vs self-contained ds soft shadow: {}x{} hits={} mismatched_hits={} avg_abs_err={:.6} max_abs_err={}",
        out_w,
        out_h,
        hit_count,
        mismatched_hits,
        avg_abs_err / hit_count.max(1) as f64,
        max_abs_err,
    );
}

#[test]
#[ignore = "manual compare of ported vec2 deao pass against self-contained ds deao pass"]
fn compare_direct_vec2_deao_to_selfcontained_ds_tiny() {
    let path = format!("{}/../../local/mb3d/cathedral.m3p", env!("CARGO_MANIFEST_DIR"));
    let scene = load_cathedral_scene(&path).expect("cathedral scene should load");
    let width = 96usize;
    let scale = (width as f64 / scene.base_width).max(0.001);
    let mut params = render::RenderParams::from_m3p(&scene.m3p);
    params.apply_image_scale(scale);
    let out_w = params.camera.width as usize;
    let out_h = params.camera.height as usize;

    let uploads = build_ds_uploads(&scene, out_w);
    let orbit_ds = orbit_scene_from_ds_uploads::<Ds>(&uploads);
    let march_ds = build_march_params_from_ds_uploads::<Ds>(&uploads);

    let mut hit_count = 0usize;
    let mut mismatched_hits = 0usize;
    let mut avg_abs_err = 0.0f64;
    let mut max_abs_err = 0.0f64;

    for y in 0..out_h {
        for x in 0..out_w {
            let seed = seed_for_pixel(x, y);
            let (origin_v2, dir_v2) = shaderlike_ray_for_pixel_vec2(&uploads, out_w, out_h, x, y);
            let vec2_hit = ray_march_vec2_uploads(&uploads, origin_v2, dir_v2, seed);
            let (origin_ds, dir_ds) = shaderlike_ray_for_pixel_num::<Ds>(&uploads, out_w, out_h, x, y);
            let ds_hit = ray_march_scene_num(&orbit_ds, &march_ds, origin_ds, dir_ds, seed);

            match (ds_hit, vec2_hit.hit) {
                (MarchResult::Miss, false) => {}
                (MarchResult::Miss, true) | (MarchResult::Hit { .. }, false) => {
                    mismatched_hits += 1;
                }
                (
                    MarchResult::Hit {
                        depth: ds_depth,
                        ..
                    },
                    true,
                ) => {
                    hit_count += 1;
                    let hit_pos_v2 = vec2_num3_add(origin_v2, vec2_num3_scale(dir_v2, vec2_hit.depth));
                    let hit_pos_ds = origin_ds.add(dir_ds.scale(ds_depth));
                    let ds_surface = standalone_surface_sample_num(
                        &uploads,
                        &orbit_ds,
                        &march_ds,
                        hit_pos_ds,
                        ds_depth.to_f64(),
                    );
                    let vec2_ao = deao_vec2_uploads(
                        &uploads,
                        hit_pos_v2,
                        F3 {
                            x: ds_surface.normal.x as f32,
                            y: ds_surface.normal.y as f32,
                            z: ds_surface.normal.z as f32,
                        },
                        ds_to_f(vec2_hit.depth),
                        x as i32,
                        y as i32,
                        out_w,
                        out_h,
                        &scene.m3p.ssao,
                    );
                    let ds_ao = standalone_deao_num(
                        &uploads,
                        &orbit_ds,
                        hit_pos_ds,
                        ds_surface.normal,
                        ds_depth.to_f64(),
                        x as i32,
                        y as i32,
                        out_w,
                        out_h,
                        &scene.m3p.ssao,
                    );
                    let abs_err = (ds_ao - vec2_ao as f64).abs();
                    avg_abs_err += abs_err;
                    max_abs_err = max_abs_err.max(abs_err);
                }
            }
        }
    }

    println!(
        "ported vec2 vs self-contained ds deao: {}x{} hits={} mismatched_hits={} avg_abs_err={:.8} max_abs_err={:.8}",
        out_w,
        out_h,
        hit_count,
        mismatched_hits,
        avg_abs_err / hit_count.max(1) as f64,
        max_abs_err,
    );
}
