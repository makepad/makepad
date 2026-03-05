use crate::render::Vec3;
use std::sync::OnceLock;

#[derive(Clone, Copy)]
struct LightingConfig {
    tone_gamma: f64,
    tone_gain: f64,
    tone_bias: f64,
    diffuse_strength: f64,
    specular_strength: f64,
    specular_power: f64,
    specular2_strength: f64,
    specular2_power: f64,
    rim_strength: f64,
    rim_power: f64,
    fog_strength: f64,
    fog_gamma: f64,
    iter_tint_strength: f64,
    light0_scale: f64,
    light1_scale: f64,
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

fn config() -> &'static LightingConfig {
    static CFG: OnceLock<LightingConfig> = OnceLock::new();
    CFG.get_or_init(|| LightingConfig {
        tone_gamma: env_f64("TONE_GAMMA", 0.6),
        tone_gain: env_f64("TONE_GAIN", 0.8),
        tone_bias: env_f64("TONE_BIAS", -0.05),
        diffuse_strength: env_f64("LIGHT_DIFFUSE", 1.0),
        specular_strength: env_f64("LIGHT_SPEC", 0.6),
        specular_power: env_f64("LIGHT_SPEC_POW", 48.0),
        specular2_strength: env_f64("LIGHT_SPEC2", 0.15),
        specular2_power: env_f64("LIGHT_SPEC2_POW", 16.0),
        rim_strength: env_f64("LIGHT_RIM", 0.05),
        rim_power: env_f64("LIGHT_RIM_POW", 2.8),
        fog_strength: env_f64("FOG_STRENGTH", 0.04),
        fog_gamma: env_f64("FOG_GAMMA", 1.2),
        iter_tint_strength: env_f64("ITER_TINT", 0.45),
        light0_scale: env_f64("LIGHT0_SCALE", 0.35),
        light1_scale: env_f64("LIGHT1_SCALE", 0.85),
    })
}

#[derive(Clone, Copy)]
struct ParsedLight {
    idx: usize,
    dir: Vec3,
    color: Vec3,
    spec_power: f64,
    diff_mode: i32,
    i_light_pos: u8,
    is_positional: bool,
}

struct CosTables {
    // DiffCosTabNsmall[0..7][0..127] from MB3D.
    diff_small: [[f64; 128]; 8],
}

fn mb3d_light_local_dir(angle_xy: f64, angle_z: f64) -> Vec3 {
    // HeaderTrafos:
    // dTmp := -LXpos; dTmp2 := LYpos;
    // BuildViewVectorFOV(dTmp2, dTmp); SVectorChangeSign.
    Vec3::new(-angle_xy.sin(), -angle_z.sin(), -(angle_xy.cos() * angle_z.cos())).normalize()
}

fn light_is_active_non_lightmap(l: &crate::m3p::M3PLight) -> bool {
    // MB3D: iLightOption := Loption and 3; 0=on, 1=off, 2=lightmap, 3->off.
    let mut opt = (l.l_option & 3) as i32;
    if opt == 3 {
        opt = 1;
    }
    opt == 0
}

fn parse_light(idx: usize, l: &crate::m3p::M3PLight, camera: &crate::render::Camera) -> Option<ParsedLight> {
    if !light_is_active_non_lightmap(l) {
        return None;
    }

    let lamp = l.l_amp.max(0.0);
    if lamp <= 0.0 {
        return None;
    }

    let i_light_pos = ((l.l_option >> 2) & 7) | ((l.l_function & 0x80) >> 4);
    let is_positional = (i_light_pos & 1) != 0;
    let diff_mode = ((l.l_function >> 4) & 3) as i32;
    let spec_power = (2u32 << (l.l_function & 0x07)) as f64;

    let local_dir = mb3d_light_local_dir(l.angle_xy, l.angle_z);
    let r = camera.right.normalize();
    let u = camera.up.normalize();
    let f = camera.forward.normalize();
    let dir = r
        .scale(local_dir.x)
        .add(u.scale(local_dir.y))
        .add(f.scale(local_dir.z))
        .normalize();

    let mut color = Vec3::new(
        l.color[0] as f64 / 255.0,
        l.color[1] as f64 / 255.0,
        l.color[2] as f64 / 255.0,
    );
    let lamp_mul = if is_positional { lamp * 1.3 } else { lamp };
    color = color.scale(lamp_mul);

    if color.x <= 0.0 && color.y <= 0.0 && color.z <= 0.0 {
        return None;
    }

    Some(ParsedLight {
        idx,
        dir,
        color,
        spec_power,
        diff_mode,
        i_light_pos,
        is_positional,
    })
}

fn cos_tables() -> &'static CosTables {
    static TABLES: OnceLock<CosTables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut diff_small = [[0.0f64; 128]; 8];

        for i in 0..128 {
            let mut d = 1.0 - (i as f64 - 2.0) / 60.0;
            diff_small[0][i] = if d > 0.15 {
                (d - 0.08) * 1.086_956_5
            } else if d <= 0.0 {
                0.0
            } else {
                d.powf(((0.505 - d) * 3.8).max(1.0))
            };
            diff_small[1][i] = d.max(0.0) * d.max(0.0);
            diff_small[2][i] = d * 0.5 + 0.5;
            diff_small[3][i] = diff_small[2][i] * diff_small[2][i];
        }

        for k in 0..4 {
            let mut tmp = [0.0f64; 128];
            for j in 0..128 {
                tmp[j] = diff_small[k][j].max(0.0).sqrt();
            }
            for j in 0..128 {
                let mut e = 0.0;
                for i in 0..=60 {
                    let l = (j as i32 + i - 30).abs() as usize;
                    if l < 128 {
                        e += tmp[l];
                    }
                }
                let t = e * 0.011 + (e * 0.007) * (e * 0.007);
                diff_small[k + 4][j] = t * t;
            }
        }

        CosTables { diff_small }
    })
}

fn make_spline_coeff(xs: f64) -> [f64; 4] {
    // Math3D.MakeSplineCoeff
    let w3 = (1.0 / 6.0) * xs * xs * xs;
    let w0 = (1.0 / 6.0) + 0.5 * xs * (xs - 1.0) - w3;
    let w2 = xs + w0 - 2.0 * w3;
    let w1 = 1.0 - w0 - w2 - w3;
    [w0, w1, w2, w3]
}

fn interp_tab4(tab: &[f64; 128], ip: usize, w: [f64; 4]) -> f64 {
    tab[ip] * w[0] + tab[ip + 1] * w[1] + tab[ip + 2] * w[2] + tab[ip + 3] * w[3]
}

fn get_cos_tab_val(tnr: i32, dotp: f64, rough: f64) -> f64 {
    let tnr = tnr.clamp(0, 3) as usize;
    let rough = rough.clamp(0.0, 1.0);
    let mut t = 62.0 - 60.0 * dotp;
    let mut ip = t.trunc() as i32 - 1;
    if ip < 0 {
        ip = 0;
        t = 0.0;
    } else if ip > 124 {
        ip = 124;
        t = 1.0;
    } else {
        t = t.fract();
    }
    let ipu = ip as usize;
    let w = make_spline_coeff(t);
    let tables = cos_tables();
    let a = interp_tab4(&tables.diff_small[tnr], ipu, w);
    let b = interp_tab4(&tables.diff_small[tnr + 4], ipu, w);
    a + rough * (b - a)
}

fn get_cos_tab_val_sqr(tnr: i32, dotp: f64, rough: f64) -> f64 {
    let tnr = tnr.clamp(0, 3) as usize;
    let rough = rough.clamp(0.0, 1.0);
    let mut t = 62.0 - 60.0 * dotp;
    let mut ip = t.trunc() as i32 - 1;
    if ip < 0 {
        ip = 0;
        t = 0.0;
    } else if ip > 124 {
        ip = 124;
        t = 1.0;
    } else {
        t = t.fract();
    }
    let ipu = ip as usize;
    let w = make_spline_coeff(t);
    let tables = cos_tables();
    let a = interp_tab4(&tables.diff_small[tnr], ipu, w);
    let b = interp_tab4(&tables.diff_small[tnr + 4], ipu, w);
    let a2 = a * a;
    let b2 = b * b;
    a2 + rough * (b2 - a2)
}

fn apply_diff_mode_mb3d(mode: i32, ndotl: f64, rough: f64, calc_pix_col_sqr: bool) -> f64 {
    if calc_pix_col_sqr {
        get_cos_tab_val_sqr(mode, ndotl, rough)
    } else {
        get_cos_tab_val(mode, ndotl, rough)
    }
}

pub fn dominant_shadow_light_dir(
    lighting: &crate::m3p::M3PLighting,
    camera: &crate::render::Camera,
) -> Option<Vec3> {
    let mut best_dir = None;
    let mut best_luma = 0.0;
    for (idx, l) in lighting.lights.iter().enumerate() {
        if let Some(pl) = parse_light(idx, l, camera) {
            let vis_mode = pl.i_light_pos & 14;
            if pl.is_positional || vis_mode != 0 {
                continue;
            }
            let luma = pl.color.x * 0.299 + pl.color.y * 0.587 + pl.color.z * 0.114;
            if luma > best_luma {
                best_luma = luma;
                best_dir = Some(pl.dir);
            }
        }
    }
    best_dir
}

pub fn soft_hs_light_dir(
    lighting: &crate::m3p::M3PLighting,
    camera: &crate::render::Camera,
    params: &crate::render::RenderParams,
) -> Option<(usize, Vec3, u8)> {
    // MB3D softHS: CalcHSsoft selects the last enabled HS light from calcHardShadow bits.
    if (params.b_calc1_hs_soft & 1) == 0 {
        return None;
    }

    let mut selected = None;
    let hs_bits = params.b_calculate_hard_shadow as u16;
    for itmp in 0..6usize {
        let mask = 4u16 << itmp;
        if (hs_bits & mask) != 0 {
            selected = Some(itmp);
        }
    }
    let idx = selected?;
    let l = lighting.lights.get(idx)?;
    let pl = parse_light(idx, l, camera)?;
    if pl.is_positional {
        // Positional softHS requires CalcHSsoft positional branch; not yet implemented.
        return None;
    }
    Some((idx, pl.dir, pl.i_light_pos))
}

/// Shade a hit pixel using a stable two-light material model.
pub fn shade(
    normal: Vec3,
    roughness: f64,
    view_dir: Vec3,
    iters: i32,
    shadow_steps: i32,
    max_iters: i32,
    min_iters: i32,
    _p: Vec3,
    _camera: &crate::render::Camera,
    ao_factor: f64,
    direct_light_factor: f64,
    depth: f64,
    y_pos: f64,
    max_depth: f64,
    lighting: &crate::m3p::M3PLighting,
    ssao: &crate::m3p::M3PSSAO,
    formulas: &[crate::formulas::FormulaSlot],
    params: &crate::render::RenderParams,
    debug: bool,
) -> [u8; 3] {
    let cfg = config();
    let strict_mb3d = true;

    let m_zz = depth / params.step_width;

    // AO is estimated in render pass; keep the expensive per-shade AO path optional.
    let mut final_ao = ao_factor.clamp(0.0, 1.0);
    // Base-first workflow: keep AO ray pass opt-in while core lighting is tuned.
    if ssao.calc_amb_shadow && env_f64("SHADE_AO_RAYS", 0.0) != 0.0 {
        let mut final_ao_val = 0.0;
        
        let num_rays = if ssao.quality == 0 { 3 } else { 
            let mut count = 1;
            let abr = std::f64::consts::PI * 0.5 / (ssao.quality as f64 + 0.9);
            for iy in 1..=ssao.quality {
                let itmp = ((iy as f64 * abr).sin() * std::f64::consts::PI * 2.0 / abr).round() as i32;
                count += itmp.max(1);
            }
            count as usize
        };
        
        if debug {
            println!("  num_rays: {}, quality: {}", num_rays, ssao.quality);
        }
        
        let mut min_ra = vec![0.0; num_rays];
        let mut rot_m = vec![Vec3::new(0.0, 0.0, 0.0); num_rays];
        
        let abr = std::f64::consts::PI * 0.5 / (ssao.quality as f64 + 0.9);
        let mut ray_idx = 0;
        
        if ssao.quality > 0 {
            rot_m[0] = normal;
            ray_idx += 1;
            for iy in 1..=ssao.quality {
                let itmp = ((iy as f64 * abr).sin() * std::f64::consts::PI * 2.0 / abr).round() as i32;
                let itmp_f = itmp.max(1) as f64;
                for ix in 0..itmp.max(1) {
                    let angle_y = abr * (iy as f64 + 0.25 - 0.25);
                    let angle_z = (ix as f64) * std::f64::consts::PI * 2.0 / itmp_f;
                    
                    let sy = angle_y.sin();
                    let cy = angle_y.cos();
                    let sz = angle_z.sin();
                    let cz = angle_z.cos();
                    
                    let local_dir = Vec3::new(sy * cz, sy * sz, cy);
                    
                    let w = normal;
                    let u = if w.x.abs() > 0.1 { Vec3::new(0.0, 1.0, 0.0).cross(w).normalize() } else { Vec3::new(1.0, 0.0, 0.0).cross(w).normalize() };
                    let v = w.cross(u);
                    
                    rot_m[ray_idx] = u.scale(local_dir.x).add(v.scale(local_dir.y)).add(w.scale(local_dir.z)).normalize();
                    ray_idx += 1;
                }
            }
        } else {
            rot_m[0] = normal;
            rot_m[1] = normal.add(_camera.right).normalize();
            rot_m[2] = normal.add(_camera.up).normalize();
        }
        
        let d_step_mul = 1.0 + abr.sin();
        let de_mul = ((num_rays as f64) * 0.5).sqrt();
        
        // MB3D AO math operates in step units.
        let step_ao = 1.0 + m_zz.abs() * params.de_stop_factor;

        let s_max_d = ssao.deao_max_l as f64 * 0.5
            * ((params.camera.width * params.camera.width + params.camera.height * params.camera.height) as f64).sqrt();

        let mut ms_de_stop_steps = params.de_stop_header * step_ao;
        if ms_de_stop_steps > 10000.0 { ms_de_stop_steps = 10000.0; }
        if ms_de_stop_steps < params.de_stop_header { ms_de_stop_steps = params.de_stop_header; }

        let step_ao_actual = ms_de_stop_steps / params.de_stop_header;
        let max_dist_steps = s_max_d * step_ao_actual.sqrt();

        let mut ms_de_stop = if params.b_vary_de_stop {
            ms_de_stop_steps / (d_step_mul * d_step_mul)
        } else {
            params.de_stop_header / (d_step_mul * d_step_mul)
        };
        
        for i in 0..num_rays {
            let s_vec = rot_m[i];
            
            let mut dt1 = step_ao_actual * d_step_mul;
            let mut s_tmp = 1.0;
            
            let mut b_first_step = false; // bMCTFirstStepRandom is false for this file
            
            loop {
                let mut b_end = false;
                
                if b_first_step {
                    b_first_step = false;
                    dt1 = dt1 * 1.25; // Average of (rand * 1.5 + 0.5)
                } else if dt1 > max_dist_steps {
                    dt1 = max_dist_steps;
                    b_end = true;
                }
                
                let probe_pos = _p.add(s_vec.scale(dt1 * params.step_width));
                
                let (_, de_world) = crate::formulas::hybrid_de((probe_pos.x, probe_pos.y, probe_pos.z), formulas, &params.iter_params);
                let dt2 = de_world / params.step_width;
                
                let md_d10 = 0.1 / (max_dist_steps * de_mul);
                let val = ((dt2 - ms_de_stop) / dt1 + md_d10).min(s_tmp);
                if val < s_tmp {
                    s_tmp = val;
                }
                
                if debug && i == 0 {
                    println!("    AO ray 0 step(steps): dt1={:.4e}, dt2={:.4e}, ms_de_stop={:.4e}, val={:.4}, s_tmp={:.4}", dt1, dt2, ms_de_stop, val, s_tmp);
                }
                
                if s_tmp < 0.02 {
                    break;
                }
                
                let step_add = if dt2 > dt1 * d_step_mul { dt2 } else { dt1 * d_step_mul };
                dt1 += step_add;
                
                if b_end {
                    break;
                }
            }
            
            min_ra[i] = s_tmp.max(0.0) * de_mul;
        }
        
        // Correction step
        let mut s_add = vec![0.0; num_rays];
        let d_min_a_dif = (abr * 1.2).cos();
        let correction_weight = if ssao.quality == 1 { 0.2 } else { 0.1666 };
        
        for iy in 0..num_rays {
            let mut max_add = 1.0 - min_ra[iy];
            if max_add > 0.0 {
                for ix in 0..num_rays {
                    if ix != iy {
                        let d_tmp = rot_m[iy].dot(rot_m[ix]);
                        if d_tmp > d_min_a_dif {
                            let overlap = min_ra[ix] - d_tmp.acos() * abr + 1.0;
                            if overlap > 0.0 {
                                s_add[iy] += max_add.min(overlap) * correction_weight;
                            }
                        }
                    }
                }
            }
        }
        
        for iy in 0..num_rays {
            final_ao_val += (s_add[iy] + min_ra[iy]).min(1.0);
        }

        // Matches CalcAmbShadowDEfor1pos:
        // AmbShadowNorm = 1 - dAmount/RayCount.
        let amb_shadow_norm = (1.0 - final_ao_val / num_rays as f64).clamp(0.0, 1.0);
        let s_amplitude = ssao.amb_shad;

        // Matches PaintThread.calcAmbshadow:
        // if amp>1:
        //   dAmbS = 1 - AmbShadowNorm;
        //   dAmbS += (amp-1) * (dAmbS^2 - dAmbS)
        // else:
        //   dAmbS = 1 - amp * AmbShadowNorm
        let mut d_amb_s = if s_amplitude > 1.0 {
            let mut d = 1.0 - amb_shadow_norm;
            d = d + (s_amplitude - 1.0) * (d * d - d);
            d
        } else {
            1.0 - s_amplitude * amb_shadow_norm
        };

        d_amb_s = d_amb_s.clamp(0.0, 1.0);
        final_ao = d_amb_s;
    }
    // MB3D: dFog := sDiffuseShadowing * (dAmbSh - 1) + 1
    //      = (1 - sDiffuseShadowing) + sDiffuseShadowing * dAmbSh.
    let diffuse_shadowing = ssao.diffuse_shadowing.clamp(0.0, 1.0);
    let diff_ao = (1.0 - diffuse_shadowing) + diffuse_shadowing * final_ao;
    // MB3D: dRough = roughByte * sRoughnessFactor, with sRoughnessFactor = RoughnessFactor * (1/255)^2.
    let rough_scale = lighting.roughness_factor as f64 / (255.0 * 255.0);
    let rough_byte = (roughness.clamp(0.0, 1.0) * 255.0).round();
    let d_rough = rough_byte * rough_scale;
    let parsed_lights: Vec<ParsedLight> = lighting
        .lights
        .iter()
        .enumerate()
        .filter_map(|(idx, l)| parse_light(idx, l, _camera))
        .collect();

    // Ambient colors from M3P
    let amb_bottom = Vec3::new(
        lighting.ambient_bottom[0] as f64 / 255.0,
        lighting.ambient_bottom[1] as f64 / 255.0,
        lighting.ambient_bottom[2] as f64 / 255.0,
    );
    let amb_top = Vec3::new(
        lighting.ambient_top[0] as f64 / 255.0,
        lighting.ambient_top[1] as f64 / 255.0,
        lighting.ambient_top[2] as f64 / 255.0,
    );

    // Calculate SIgradient
    let d_tmp = iters as f64;
    let max_it = max_iters as f64;
    let min_it = min_iters as f64;
    
    let mut si_gradient_f = 32767.0 - (d_tmp - min_it) * 32767.0 / (max_it - min_it + 1.0);
    if si_gradient_f > 32766.5 { si_gradient_f = 32767.0; }
    if si_gradient_f < 0.0 { si_gradient_f = 0.0; }
    let si_gradient = si_gradient_f.round() as i32;

    // Calculate ir
    let mut s_c_start = ((lighting.tbpos_9 + 30) as f64 * 0.01111111111111111).powi(2) * 32767.0 - 10900.0;
    let mut s_c_mul = ((lighting.tbpos_10 + 30) as f64 * 0.01111111111111111).powi(2) * 32767.0 - 10900.0 - s_c_start;
    
    if (lighting.tboptions & 0x10000) != 0 {
        let d_tmp = s_c_start + s_c_mul * (lighting.fine_col_adj_2 as i32 - 30) as f64 * 0.0166666666666666;
        s_c_start = s_c_start + s_c_mul * (lighting.fine_col_adj_1 as i32 - 30) as f64 * 0.0166666666666666;
        s_c_mul = d_tmp - s_c_start;
    }
    if s_c_mul.abs() > 1e-4 {
        s_c_mul = 2.0 / s_c_mul;
    } else if s_c_mul < 0.0 {
        s_c_mul = -2000.0;
    } else {
        s_c_mul = 2000.0;
    }
    
    // iDif[0] is sColZmul * PLV.zPos
    // sColZmul = 11 * -0.005 / (1.689668e-12 * 1920) = -16954034875.5
    // PLV.zPos = depth + z1
    let z1 = params.camera.z_start - params.camera.mid.z;
    let plv_z_pos = depth + z1;
    let mut s_col_z_mul = 0.0;
    if (lighting.tboptions & 0x20000) != 0 {
        s_col_z_mul = (lighting.tbpos_11 as f64 * -0.005) / (params.step_width * 1920.0);
    }
    let i_dif_0 = s_col_z_mul * plv_z_pos;
    
    let ir_f = ((si_gradient as f64 - s_c_start) * s_c_mul + i_dif_0) * 16384.0;
    let ir = ir_f.round() as i32;
    
    // bColCycling is true
    let ir_cycled = ir & 32767;
    
    if debug {
        println!("  s_c_start: {:.4}, s_c_mul: {:.4}, si_gradient: {}, i_dif_0: {:.4}, ir_f: {:.4}, ir_cycled: {}", 
                 s_c_start, s_c_mul, si_gradient, i_dif_0, ir_f, ir_cycled);
    }

    let c = surface_color(ir_cycled, lighting);
    let cs = surface_spec_color(ir_cycled, lighting);

    let diffuse_color = Vec3::new(c.0, c.1, c.2);
    let spec_color = Vec3::new(cs.0, cs.1, cs.2);

    let s_diff_base = (lighting.tbpos_5 as f64 * 0.02).max(0.0);
    let s_spec_base = (((lighting.tbpos_7 & 0x0FFF) as f64) * 0.02).max(0.004);
    let s_diff = if strict_mb3d {
        s_diff_base
    } else {
        s_diff_base * cfg.diffuse_strength
    };
    let s_spec = if strict_mb3d {
        s_spec_base
    } else {
        s_spec_base * cfg.specular_strength
    };

    // view_dir is passed as object->camera.
    let v_to_cam = view_dir.normalize();
    let v_from_cam = v_to_cam.scale(-1.0);
    let n = normal.normalize();
    let cam_up = _camera.up.normalize();

    let b_amb_rel_obj = (lighting.tboptions & 0x20000000) != 0;

    // Ambient light
    let ny = if b_amb_rel_obj { n.y } else { n.dot(cam_up) };
    let w_top = (ny * 0.5 + 0.5).clamp(0.0, 1.0);
    let w_bot = 1.0 - w_top;
    let amb_light = amb_top.scale(w_top).add(amb_bottom.scale(w_bot)).scale(final_ao);

    // Diffuse light accumulation
    let mut total_diffuse = Vec3::new(0.0, 0.0, 0.0);
    let mut total_specular = Vec3::new(0.0, 0.0, 0.0);
    let mut total_specular2 = Vec3::new(0.0, 0.0, 0.0);

    // For each light:
    for pl in &parsed_lights {
        let li = pl.idx;
        let hsl = &lighting.lights[li];
        let i_hs_enabled = 1 - (((hsl.l_option >> 6) & 1) as i32);
        let i_hs_calced = i_hs_enabled & (((params.b_hs_calculated as i32) >> (li + 2)) & 1);
        let mut i_hs_mask = 0x400i32 << li;
        if ((params.b_calc1_hs_soft & 1) != 0) && (i_hs_calced != 0) {
            i_hs_mask = -1;
        }
        let soft_hs = i_hs_mask == -1;
        let no_hs = soft_hs || ((shadow_steps & i_hs_mask) == 0) || (i_hs_calced == 0);
        let b_sub_amb_sh = (i_hs_calced ^ i_hs_enabled) != 0;
        let mut hs_mul = if b_sub_amb_sh { final_ao } else { diff_ao };
        if soft_hs {
            let soft = ((shadow_steps >> 10) as f64 * (1.0 / 63.0)).clamp(0.0, 1.0);
            hs_mul *= soft;
        }
        let light_gate = if no_hs { hs_mul } else { 0.0 };

        let diff_dot = apply_diff_mode_mb3d(pl.diff_mode, n.dot(pl.dir), d_rough, lighting.calc_pix_col_sqr);
        let mut diff_shadowed = diff_dot * light_gate;
        if !strict_mb3d {
            diff_shadowed *= direct_light_factor;
        }
        total_diffuse = total_diffuse.add(pl.color.scale(diff_shadowed));
        
        if debug {
            println!(
                "    Light[{}]: diff_dot={}, hs_mul={}, no_hs={}, soft_hs={}, diff_shadowed={}",
                li, diff_dot, light_gate, no_hs, soft_hs, diff_shadowed
            );
        }

        // MB3D DotOf2VecNormalize: reflect camera->object vector on normal, then dot with light.
        let reflect_view = v_from_cam.sub(n.scale(2.0 * n.dot(v_from_cam)));
        let spec_dot = pl.dir.dot(reflect_view);
        if spec_dot > 0.0 {
            let att = 1.0;
            let mut spec_mul = (att + (d_rough * 2.0).min(1.0) * (1.0 / pl.spec_power - att)) * s_spec;
            if spec_mul < 0.0 {
                spec_mul = 0.0;
            }
            if spec_mul > 0.0 {
                let spec_pow = spec_dot.powf(pl.spec_power);
                let mut spec_shadowed = spec_pow * spec_mul * light_gate;
                if !strict_mb3d {
                    spec_shadowed *= direct_light_factor;
                }
                total_specular = total_specular.add(pl.color.scale(spec_shadowed));
            }

            if !strict_mb3d {
                let spec_pow2 = spec_dot.powf(cfg.specular2_power);
                total_specular2 = total_specular2.add(
                    pl.color.scale(spec_pow2 * cfg.specular2_strength * diff_ao * direct_light_factor)
                );
            }
        }
    }

    // Final color
    let mut final_color = if strict_mb3d {
        Vec3::new(
            amb_light.x * diffuse_color.x + diffuse_color.x * s_diff * total_diffuse.x
                + spec_color.x * total_specular.x,
            amb_light.y * diffuse_color.y + diffuse_color.y * s_diff * total_diffuse.y
                + spec_color.y * total_specular.y,
            amb_light.z * diffuse_color.z + diffuse_color.z * s_diff * total_diffuse.z
                + spec_color.z * total_specular.z,
        )
    } else {
        Vec3::new(
            amb_light.x * diffuse_color.x + diffuse_color.x * s_diff * total_diffuse.x
                + spec_color.x * (total_specular.x + total_specular2.x),
            amb_light.y * diffuse_color.y + diffuse_color.y * s_diff * total_diffuse.y
                + spec_color.y * (total_specular.y + total_specular2.y),
            amb_light.z * diffuse_color.z + diffuse_color.z * s_diff * total_diffuse.z
                + spec_color.z * (total_specular.z + total_specular2.z),
        )
    };

    if debug {
        println!("  normal: {:?}", n);
        println!("  amb_bottom: {:?}, amb_top: {:?}", amb_bottom, amb_top);
        println!("  diffuse_color: {:?}", diffuse_color);
        println!("  total_diffuse: {:?}", total_diffuse);
        println!("  total_specular: {:?}", total_specular);
        println!("  amb_light: {:?}", amb_light);
        println!("  final_color: {:?}", final_color);
        println!("  final_ao: {:.4}", final_ao);
    }

    if !strict_mb3d {
        // Non-source experimental boosts (disabled in strict mode).
        let rim = (1.0 - n.dot(v_to_cam).max(0.0)).powf(cfg.rim_power) * cfg.rim_strength;
        final_color = final_color.add(Vec3::new(rim, rim, rim));

        let fog_t = if max_depth > 1e-20 {
            (depth / max_depth).clamp(0.0, 1.0).powf(cfg.fog_gamma) * cfg.fog_strength
        } else {
            0.0
        };
        let fog_color = amb_top;
        final_color = final_color.scale(1.0 - fog_t).add(fog_color.scale(fog_t));
    }

    // Calculate Zpos
    let z_pos_f = 32767.0 - (params.z_cmul / 256.0) * ((m_zz * params.z_corr + 1.0).sqrt() - 1.0);
    let z_pos = z_pos_f.round().clamp(0.0, 32767.0) as i32;

    let mut d_tmp = if z_pos < 32768 {
        ((z_pos as f64 - 28000.0) * lighting.s_depth + 1.0).max(0.0)
    } else {
        (1.0f64 - (1.0f64 - 28000.0f64 * lighting.s_depth).max(0.0f64)).max(0.0f64)
    };
    
    // Calculate sShad, sShadZmul, sShadGr
    let tbpos_3 = lighting.tbpos_3;
    let tbpos_6 = lighting.tbpos_6;
    let mut s_tmp_shad = 128.0;
    
    let b_vol_light = (params.b_vol_light_nr & 7) != 0;
    
    let mut d_tmp_shad = 2.2 / params.s_z_step_div_raw;
    let mut s_shad_gr = (tbpos_6 as f64 - 53.0) * params.s_z_step_div_raw * 0.00065;
    // ImScale is 1.0 in almost all calls to MakeLightValsFromHeaderLight
    let mut s_dyn_fog_mul = params.s_z_step_div_raw * 0.015;
    
    if b_vol_light {
        s_dyn_fog_mul = 0.0005;
        d_tmp_shad = 50.0;
        s_shad_gr = (tbpos_6 as f64 - 53.0) * 0.00002;
    } else {
        if params.b_dfog_it > 0 {
            d_tmp_shad *= 0.25;
            s_shad_gr *= 4.0;
            s_dyn_fog_mul *= 4.0;
        } else {
            s_tmp_shad = 137.0;
        }
    }
    
    let sqrt_tbpos3_and_ffff = ((tbpos_3 & 0xFFFF) as f64).sqrt();
    let s_shad = (s_tmp_shad - sqrt_tbpos3_and_ffff * 11.313708) * d_tmp_shad * 0.28;
    
    let sqrt_tbpos3_shr_16 = ((tbpos_3 >> 16) as f64).sqrt();
    let s_shad_z_mul = d_tmp_shad * 0.7 / (params.camera.z_end - params.camera.z_start) * (128.0 - sqrt_tbpos3_shr_16 * 11.313708);
    
    let b_dfog_options = if !lighting.lights.is_empty() { lighting.lights[0].free_byte & 3 } else { 0 };
    let b_dfog_options = if b_dfog_options == 3 { 1 } else { b_dfog_options };

    let mut ir_for_fog = shadow_steps as f64;
    // MB3D uses PsiLight.Shadow (ray-march DE count), not fractal escape iterations.
    if b_vol_light {
        let mut eax = shadow_steps & 0x3FF;
        let cl = eax >> 7;
        eax &= 0x7F;
        eax <<= cl;
        ir_for_fog = eax as f64;
    } else {
        ir_for_fog = (shadow_steps & 0x3FF) as f64;
    }
    
    let mut d_fog = (ir_for_fog - s_shad - s_shad_z_mul * plv_z_pos) * s_shad_gr;
    if (b_dfog_options & 2) != 0 {
        d_fog = d_fog.max(0.0);
    }
    
    let mut d_tmp3 = (1.0f64).min(ir_for_fog * s_dyn_fog_mul) * d_fog;
    
    if (b_dfog_options & 1) != 0 {
        d_fog = d_fog.clamp(0.0, 1.0);
        d_tmp3 = d_tmp3.clamp(0.0, 1.0);
    }
    
    // AddSVectors(@LiLSDAI[4], Add2SVecsWeight(PLValigned.sDynFogCol, PLValigned.sDynFogCol2, dFog - dTmp3, dTmp3));
    let s_dyn_fog_col = Vec3::new(
        lighting.dyn_fog_col[0] as f64 / 255.0,
        lighting.dyn_fog_col[1] as f64 / 255.0,
        lighting.dyn_fog_col[2] as f64 / 255.0,
    );
    let s_dyn_fog_col2 = Vec3::new(
        lighting.dyn_fog_col2[0] as f64 / 255.0,
        lighting.dyn_fog_col2[1] as f64 / 255.0,
        lighting.dyn_fog_col2[2] as f64 / 255.0,
    );
    
    let fog_add = Vec3::new(
        s_dyn_fog_col.x * (d_fog - d_tmp3) + s_dyn_fog_col2.x * d_tmp3,
        s_dyn_fog_col.y * (d_fog - d_tmp3) + s_dyn_fog_col2.y * d_tmp3,
        s_dyn_fog_col.z * (d_fog - d_tmp3) + s_dyn_fog_col2.z * d_tmp3,
    );

    if debug {
        println!("  depth: {:.4e}, m_zz: {:.4e}, z_pos_f: {:.2}, z_pos: {}, d_tmp: {:.4}", depth, m_zz, z_pos_f, z_pos, d_tmp);
        println!("  s_shad: {:.4}, s_shad_z_mul: {:.4}, s_shad_gr: {:.4}, d_fog: {:.4}, d_tmp3: {:.4}", s_shad, s_shad_z_mul, s_shad_gr, d_fog, d_tmp3);
    }

    if d_tmp < 1.0 {
        d_tmp = 1.0 - f64::powi(1.0 - d_tmp, 2);
    }

    let i_dfunc = ((lighting.tboptions >> 30) & 0x3) as i32;
    let s = if b_amb_rel_obj {
        (v_from_cam.y.asin() / std::f64::consts::PI + 0.5).clamp(0.0, 1.0)
    } else {
        let yy = y_pos.clamp(0.0, 1.0);
        match i_dfunc {
            1 => yy * yy,
            0 => yy,
            _ => yy.sqrt(),
        }
    };
    let dep_c = Vec3::new(
        lighting.depth_col[0] as f64 / 255.0,
        lighting.depth_col[1] as f64 / 255.0,
        lighting.depth_col[2] as f64 / 255.0,
    );
    let dep_c2 = Vec3::new(
        lighting.depth_col2[0] as f64 / 255.0,
        lighting.depth_col2[1] as f64 / 255.0,
        lighting.depth_col2[2] as f64 / 255.0,
    );
    let dep_c_interp = Vec3::new(
        dep_c2.x * s + dep_c.x * (1.0 - s),
        dep_c2.y * s + dep_c.y * (1.0 - s),
        dep_c2.z * s + dep_c.z * (1.0 - s),
    );

    if z_pos < 32768 {
        final_color = Vec3::new(
            final_color.x * d_tmp,
            final_color.y * d_tmp,
            final_color.z * d_tmp,
        );
    }
    
    // LiLSDAI[4] := Add2SVecsWeight2(LiLSDAI[4], DepC, Max0S(1 - dTmp));
    let t_dep = (1.0f64 - d_tmp).max(0.0f64);
    final_color = Vec3::new(
        final_color.x + dep_c_interp.x * t_dep,
        final_color.y + dep_c_interp.y * t_dep,
        final_color.z + dep_c_interp.z * t_dep,
    );

    if (b_dfog_options & 1) != 0 {
        final_color = Vec3::new(
            final_color.x * (1.0 - d_fog),
            final_color.y * (1.0 - d_fog),
            final_color.z * (1.0 - d_fog),
        );
    }

    final_color = Vec3::new(
        final_color.x + fog_add.x,
        final_color.y + fog_add.y,
        final_color.z + fog_add.z,
    );
    
    // No depth fog for now
    
    let final_color_toned = if strict_mb3d {
        Vec3::new(
            final_color.x.clamp(0.0, 1.0),
            final_color.y.clamp(0.0, 1.0),
            final_color.z.clamp(0.0, 1.0),
        )
    } else {
        // Non-source experimental tone mapping (disabled in strict mode).
        let tone = |c: f64| ((c.clamp(0.0, 1.0).powf(cfg.tone_gamma) * cfg.tone_gain) + cfg.tone_bias).clamp(0.0, 1.0);
        Vec3::new(
            tone(final_color.x),
            tone(final_color.y),
            tone(final_color.z),
        )
    };

    [
        (final_color_toned.x * 255.0) as u8,
        (final_color_toned.y * 255.0) as u8,
        (final_color_toned.z * 255.0) as u8,
    ]
}

/// Map normalized iteration count to a color (0..1 each channel)
pub fn surface_color(si_gradient: i32, lighting: &crate::m3p::M3PLighting) -> (f64, f64, f64) {
    let mut t = si_gradient as f64 / 32768.0;
    
    // Wrap around
    t = t - t.floor();
    if t < 0.0 { t += 1.0; }
    
    let mut stops: Vec<(f64, (f64, f64, f64))> = lighting.l_cols.iter().map(|s| {
        let pos = s.pos as f64 / 32768.0;
        let c = (
            s.color_dif[0] as f64 / 255.0,
            s.color_dif[1] as f64 / 255.0,
            s.color_dif[2] as f64 / 255.0,
        );
        (pos, c)
    }).collect();

    if stops.is_empty() {
        return (0.5, 0.5, 0.5);
    }
    
    stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    
    // Wrap around: add the first color at pos 1.0 + first.pos
    if let Some(first) = stops.first().cloned() {
        stops.push((1.0 + first.0, first.1));
    }

    let mut c = stops.last().unwrap().1;
    for i in 0..stops.len() - 1 {
        let (p1, c1) = stops[i];
        let (p2, c2) = stops[i+1];
        
        // Handle wrap around case where t might be between last element and 1.0 + first
        let mut t_check = t;
        if i == stops.len() - 2 && t < p1 {
            t_check += 1.0;
        }

        if t_check >= p1 && t_check <= p2 {
            let f = if p2 > p1 { (t_check - p1) / (p2 - p1) } else { 0.0 };
            c = (
                c1.0 * (1.0 - f) + c2.0 * f,
                c1.1 * (1.0 - f) + c2.1 * f,
                c1.2 * (1.0 - f) + c2.2 * f,
            );
            break;
        }
    }
    c
}

pub fn surface_spec_color(si_gradient: i32, lighting: &crate::m3p::M3PLighting) -> (f64, f64, f64) {
    let mut t = si_gradient as f64 / 32768.0;

    t = t - t.floor();
    if t < 0.0 { t += 1.0; }

    let mut stops: Vec<(f64, (f64, f64, f64))> = lighting.l_cols.iter().map(|s| {
        let pos = s.pos as f64 / 32768.0;
        let c = (
            s.color_spe[0] as f64 / 255.0,
            s.color_spe[1] as f64 / 255.0,
            s.color_spe[2] as f64 / 255.0,
        );
        (pos, c)
    }).collect();

    if stops.is_empty() {
        return (1.0, 1.0, 1.0);
    }

    stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    if let Some(first) = stops.first().cloned() {
        stops.push((1.0 + first.0, first.1));
    }

    let mut c = stops.last().unwrap().1;
    for i in 0..stops.len() - 1 {
        let (p1, c1) = stops[i];
        let (p2, c2) = stops[i + 1];
        let mut t_check = t;
        if i == stops.len() - 2 && t < p1 {
            t_check += 1.0;
        }
        if t_check >= p1 && t_check <= p2 {
            let f = if p2 > p1 { (t_check - p1) / (p2 - p1) } else { 0.0 };
            c = (
                c1.0 * (1.0 - f) + c2.0 * f,
                c1.1 * (1.0 - f) + c2.1 * f,
                c1.2 * (1.0 - f) + c2.2 * f,
            );
            break;
        }
    }
    c
}

pub fn iteration_color(t: f64, lighting: &crate::m3p::M3PLighting) -> (f64, f64, f64) {
    let t = t.clamp(0.0, 1.0);
    
    // Convert pos to 0..1
    let mut stops: Vec<(f64, (f64, f64, f64))> = lighting.i_cols.iter().map(|s| {
        let pos = s.pos as f64 / 32768.0;
        let c = (
            s.color[0] as f64 / 255.0,
            s.color[1] as f64 / 255.0,
            s.color[2] as f64 / 255.0,
        );
        (pos, c)
    }).collect();

    if stops.is_empty() {
        return (0.5, 0.5, 0.5);
    }
    
    // Wrap around: add the first color at pos 1.0
    if let Some(first) = stops.first().cloned() {
        stops.push((1.0, first.1));
    }

    if stops.len() == 1 {
        return stops[0].1;
    }

    if t <= stops[0].0 {
        return stops[0].1;
    }
    if t >= stops.last().unwrap().0 {
        return stops.last().unwrap().1;
    }

    for i in 0..stops.len() - 1 {
        let (p0, c0) = stops[i];
        let (p1, c1) = stops[i + 1];
        if t >= p0 && t <= p1 {
            let s = if p1 > p0 { (t - p0) / (p1 - p0) } else { 0.0 };
            return lerp3(c0, c1, s);
        }
    }
    
    stops.last().unwrap().1
}

fn lerp3(a: (f64, f64, f64), b: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}
