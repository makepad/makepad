use crate::render::Vec3;

pub const MB3D_LIGHTING_ATTRIBUTE_NAME: &str = "makepad.mb3d_lighting";
pub const MB3D_CAMERA_ATTRIBUTE_NAME: &str = "makepad.mb3d_camera";
pub const MB3D_MIP_LEVEL_ATTRIBUTE_NAME: &str = "makepad.mb3d_mip_level";
pub const MB3D_MIP_TOTAL_LEVELS_ATTRIBUTE_NAME: &str = "makepad.mb3d_mip_total_levels";
pub const MB3D_MIP_FILTER_ATTRIBUTE_NAME: &str = "makepad.mb3d_mip_filter";

#[derive(Debug, Clone, Copy)]
pub struct ViewerLight {
    pub dir: Vec3,
    pub color: Vec3,
    pub spec_power: f64,
}

#[derive(Debug, Clone)]
pub struct ViewerLightingMetadata {
    pub ambient_bottom: Vec3,
    pub ambient_top: Vec3,
    pub depth_col: Vec3,
    pub depth_col2: Vec3,
    pub dyn_fog_col: Vec3,
    pub dyn_fog_col2: Vec3,
    pub s_diff: f64,
    pub s_spec: f64,
    pub rough_scale: f64,
    pub lights: Vec<ViewerLight>,
}

#[derive(Debug, Clone, Copy)]
pub struct ViewerCameraMetadata {
    pub mid: Vec3,
    pub right_step: Vec3,
    pub up_step: Vec3,
    pub forward_dir: Vec3,
    pub fov_y: f64,
    pub z_start_delta: f64,
}

impl ViewerCameraMetadata {
    pub fn from_camera(camera: &crate::render::Camera) -> Self {
        Self {
            mid: camera.mid,
            right_step: camera.right,
            up_step: camera.up,
            forward_dir: camera.forward.normalize(),
            fov_y: camera.fov_y,
            z_start_delta: camera.z_start - camera.mid.z,
        }
    }

    pub fn encode_string(&self) -> String {
        let mut out = String::new();
        out.push_str("version=1\n");
        push_vec3_line(&mut out, "mid", self.mid);
        push_vec3_line(&mut out, "right_step", self.right_step);
        push_vec3_line(&mut out, "up_step", self.up_step);
        push_vec3_line(&mut out, "forward_dir", self.forward_dir);
        push_scalar_line(&mut out, "fov_y", self.fov_y);
        push_scalar_line(&mut out, "z_start_delta", self.z_start_delta);
        out
    }

    pub fn decode_string(input: &str) -> Result<Self, String> {
        let mut out = Self {
            mid: Vec3::new(0.0, 0.0, 0.0),
            right_step: Vec3::new(0.0, 0.0, 0.0),
            up_step: Vec3::new(0.0, 0.0, 0.0),
            forward_dir: Vec3::new(0.0, 0.0, 1.0),
            fov_y: 0.0,
            z_start_delta: 0.0,
        };
        let mut saw_version = false;

        for line in input.lines() {
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("invalid metadata line {line:?}"));
            };
            match key {
                "version" => {
                    if value != "1" {
                        return Err(format!("unsupported metadata version {value:?}"));
                    }
                    saw_version = true;
                }
                "mid" => out.mid = parse_vec3(value)?,
                "right_step" => out.right_step = parse_vec3(value)?,
                "up_step" => out.up_step = parse_vec3(value)?,
                "forward_dir" => out.forward_dir = parse_vec3(value)?,
                "fov_y" => out.fov_y = parse_scalar(value)?,
                "z_start_delta" => out.z_start_delta = parse_scalar(value)?,
                _ => {}
            }
        }

        if !saw_version {
            return Err("missing metadata version".to_string());
        }
        Ok(out)
    }
}

impl Default for ViewerLightingMetadata {
    fn default() -> Self {
        Self {
            ambient_bottom: Vec3::new(0.0, 0.0, 0.0),
            ambient_top: Vec3::new(0.0, 0.0, 0.0),
            depth_col: Vec3::new(0.0, 0.0, 0.0),
            depth_col2: Vec3::new(0.0, 0.0, 0.0),
            dyn_fog_col: Vec3::new(0.0, 0.0, 0.0),
            dyn_fog_col2: Vec3::new(0.0, 0.0, 0.0),
            s_diff: 0.0,
            s_spec: 0.0,
            rough_scale: 0.0,
            lights: Vec::new(),
        }
    }
}

impl ViewerLightingMetadata {
    pub fn encode_string(&self) -> String {
        let mut out = String::new();
        out.push_str("version=1\n");
        push_vec3_line(&mut out, "ambient_bottom", self.ambient_bottom);
        push_vec3_line(&mut out, "ambient_top", self.ambient_top);
        push_vec3_line(&mut out, "depth_col", self.depth_col);
        push_vec3_line(&mut out, "depth_col2", self.depth_col2);
        push_vec3_line(&mut out, "dyn_fog_col", self.dyn_fog_col);
        push_vec3_line(&mut out, "dyn_fog_col2", self.dyn_fog_col2);
        push_scalar_line(&mut out, "s_diff", self.s_diff);
        push_scalar_line(&mut out, "s_spec", self.s_spec);
        push_scalar_line(&mut out, "rough_scale", self.rough_scale);
        out.push_str("lights=");
        for (index, light) in self.lights.iter().enumerate() {
            if index > 0 {
                out.push('|');
            }
            out.push_str(&format!(
                "{:.9},{:.9},{:.9},{:.9},{:.9},{:.9},{:.9}",
                light.dir.x,
                light.dir.y,
                light.dir.z,
                light.color.x,
                light.color.y,
                light.color.z,
                light.spec_power
            ));
        }
        out.push('\n');
        out
    }

    pub fn decode_string(input: &str) -> Result<Self, String> {
        let mut out = Self::default();
        let mut saw_version = false;

        for line in input.lines() {
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("invalid metadata line {line:?}"));
            };
            match key {
                "version" => {
                    if value != "1" {
                        return Err(format!("unsupported metadata version {value:?}"));
                    }
                    saw_version = true;
                }
                "ambient_bottom" => out.ambient_bottom = parse_vec3(value)?,
                "ambient_top" => out.ambient_top = parse_vec3(value)?,
                "depth_col" => out.depth_col = parse_vec3(value)?,
                "depth_col2" => out.depth_col2 = parse_vec3(value)?,
                "dyn_fog_col" => out.dyn_fog_col = parse_vec3(value)?,
                "dyn_fog_col2" => out.dyn_fog_col2 = parse_vec3(value)?,
                "s_diff" => out.s_diff = parse_scalar(value)?,
                "s_spec" => out.s_spec = parse_scalar(value)?,
                "rough_scale" => out.rough_scale = parse_scalar(value)?,
                "lights" => {
                    out.lights = if value.is_empty() {
                        Vec::new()
                    } else {
                        value
                            .split('|')
                            .map(parse_light)
                            .collect::<Result<Vec<_>, _>>()?
                    };
                }
                _ => {}
            }
        }

        if !saw_version {
            return Err("missing metadata version".to_string());
        }
        Ok(out)
    }
}

fn push_vec3_line(out: &mut String, key: &str, value: Vec3) {
    out.push_str(&format!(
        "{key}={:.9},{:.9},{:.9}\n",
        value.x, value.y, value.z
    ));
}

fn push_scalar_line(out: &mut String, key: &str, value: f64) {
    out.push_str(&format!("{key}={value:.9}\n"));
}

fn parse_scalar(value: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .map_err(|err| format!("invalid float {value:?}: {err}"))
}

fn parse_vec3(value: &str) -> Result<Vec3, String> {
    let values = parse_csv_f64::<3>(value)?;
    Ok(Vec3::new(values[0], values[1], values[2]))
}

fn parse_light(value: &str) -> Result<ViewerLight, String> {
    let values = parse_csv_f64::<7>(value)?;
    Ok(ViewerLight {
        dir: Vec3::new(values[0], values[1], values[2]),
        color: Vec3::new(values[3], values[4], values[5]),
        spec_power: values[6],
    })
}

fn parse_csv_f64<const N: usize>(value: &str) -> Result<[f64; N], String> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != N {
        return Err(format!(
            "expected {N} comma-separated floats, got {} in {value:?}",
            parts.len()
        ));
    }
    let mut out = [0.0; N];
    for (index, part) in parts.into_iter().enumerate() {
        out[index] = parse_scalar(part)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(left: f64, right: f64) {
        assert!((left - right).abs() < 1.0e-6, "{left} != {right}");
    }

    #[test]
    fn lighting_metadata_roundtrips() {
        let meta = ViewerLightingMetadata {
            ambient_bottom: Vec3::new(0.1, 0.2, 0.3),
            ambient_top: Vec3::new(0.4, 0.5, 0.6),
            depth_col: Vec3::new(0.7, 0.8, 0.9),
            depth_col2: Vec3::new(0.15, 0.25, 0.35),
            dyn_fog_col: Vec3::new(0.45, 0.55, 0.65),
            dyn_fog_col2: Vec3::new(0.75, 0.85, 0.95),
            s_diff: 1.2,
            s_spec: 3.4,
            rough_scale: 0.0123,
            lights: vec![
                ViewerLight {
                    dir: Vec3::new(0.0, 0.6, 0.8),
                    color: Vec3::new(1.0, 0.8, 0.6),
                    spec_power: 64.0,
                },
                ViewerLight {
                    dir: Vec3::new(-0.5, 0.2, 0.84),
                    color: Vec3::new(0.2, 0.3, 0.4),
                    spec_power: 16.0,
                },
            ],
        };

        let encoded = meta.encode_string();
        let decoded = ViewerLightingMetadata::decode_string(&encoded).unwrap();
        approx_eq(decoded.ambient_bottom.x, 0.1);
        approx_eq(decoded.ambient_top.z, 0.6);
        approx_eq(decoded.depth_col.y, 0.8);
        approx_eq(decoded.dyn_fog_col2.x, 0.75);
        approx_eq(decoded.s_diff, 1.2);
        approx_eq(decoded.s_spec, 3.4);
        approx_eq(decoded.rough_scale, 0.0123);
        assert_eq!(decoded.lights.len(), 2);
        approx_eq(decoded.lights[0].dir.z, 0.8);
        approx_eq(decoded.lights[1].color.y, 0.3);
        approx_eq(decoded.lights[1].spec_power, 16.0);
    }

    #[test]
    fn camera_metadata_roundtrips() {
        let meta = ViewerCameraMetadata {
            mid: Vec3::new(1.0, 2.0, 3.0),
            right_step: Vec3::new(0.5, 0.0, 0.0),
            up_step: Vec3::new(0.0, 0.5, 0.0),
            forward_dir: Vec3::new(0.0, 0.0, 1.0),
            fov_y: 34.5,
            z_start_delta: -8.25,
        };

        let encoded = meta.encode_string();
        let decoded = ViewerCameraMetadata::decode_string(&encoded).unwrap();
        approx_eq(decoded.mid.x, 1.0);
        approx_eq(decoded.mid.y, 2.0);
        approx_eq(decoded.mid.z, 3.0);
        approx_eq(decoded.right_step.x, 0.5);
        approx_eq(decoded.up_step.y, 0.5);
        approx_eq(decoded.forward_dir.z, 1.0);
        approx_eq(decoded.fov_y, 34.5);
        approx_eq(decoded.z_start_delta, -8.25);
    }
}
