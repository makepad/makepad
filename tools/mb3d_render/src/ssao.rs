use crate::render::Vec3;

pub struct SSAOParams {
    pub quality: i32,
    pub deao_max_l: f64,
    pub ssao_r_count: i32,
    pub ao_dithering: i32,
    pub calc_amb_shadow: bool,
}

pub fn get_ray_dirs(quality: i32) -> Vec<Vec3> {
    let mut dirs = Vec::new();
    if quality == 0 {
        let abr = 60.0 * std::f64::consts::PI / 180.0;
        for x in 0..3 {
            let rot = get_rot_matrix(0.0, 0.5 * abr, (x as f64) * std::f64::consts::PI * 2.0 / 3.0);
            dirs.push(Vec3::new(-rot[2][0], -rot[2][1], -rot[2][2]));
        }
    } else {
        let rot = get_rot_matrix(0.0, 0.0, 0.0);
        dirs.push(Vec3::new(-rot[2][0], -rot[2][1], -rot[2][2]));
        let abr = std::f64::consts::PI * 0.5 / ((quality as f64) + 0.9);
        for y in 1..=quality {
            let dt1 = (y as f64) * abr;
            let itmp = (dt1.sin() * std::f64::consts::PI * 2.0 / abr).round() as i32;
            for x in 0..itmp {
                let rot = get_rot_matrix(0.0, dt1, (x as f64) * std::f64::consts::PI * 2.0 / (itmp as f64));
                dirs.push(Vec3::new(-rot[2][0], -rot[2][1], -rot[2][2]));
            }
        }
    }
    dirs
}

fn get_rot_matrix(angle_x: f64, angle_y: f64, angle_z: f64) -> [[f64; 3]; 3] {
    let cx = angle_x.cos();
    let sx = angle_x.sin();
    let cy = angle_y.cos();
    let sy = angle_y.sin();
    let cz = angle_z.cos();
    let sz = angle_z.sin();
    
    [
        [cy*cz, -cy*sz, sy],
        [cx*sz + sx*sy*cz, cx*cz - sx*sy*sz, -sx*cy],
        [sx*sz - cx*sy*cz, sx*cz + cx*sy*sz, cx*cy]
    ]
}
