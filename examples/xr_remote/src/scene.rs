use crate::protocol::{
    default_session_config, EyeViewPacket, SessionConfigPacket, StreamConfigPacket, TrackingPacket,
    XrRemoteCodec, XrRemoteEye, XR_REMOTE_PROTOCOL_VERSION,
};
use makepad_widgets::makepad_math::*;

#[derive(Clone, Copy, Debug)]
pub struct SceneBox {
    pub center: Vec3f,
    pub size: Vec3f,
    pub color: [u8; 4],
}

#[derive(Clone, Copy, Debug)]
struct ProjectedVertex {
    x: f32,
    y: f32,
    depth: f32,
}

const CLEAR_BGRA: [u8; 4] = [8, 10, 16, 255];
const LIGHT_DIR: Vec3f = Vec3f {
    x: 0.35,
    y: 0.8,
    z: -0.45,
};
const SCENE_BOXES: [SceneBox; 8] = [
    SceneBox {
        center: Vec3f {
            x: 0.42,
            y: -0.22,
            z: -0.72,
        },
        size: Vec3f {
            x: 1.65,
            y: 0.08,
            z: 1.18,
        },
        color: [0x24, 0x34, 0x44, 0xff],
    },
    SceneBox {
        center: Vec3f {
            x: 1.20,
            y: 0.10,
            z: -0.72,
        },
        size: Vec3f {
            x: 0.18,
            y: 0.72,
            z: 1.16,
        },
        color: [0x1c, 0x27, 0x33, 0xff],
    },
    SceneBox {
        center: Vec3f {
            x: 0.42,
            y: 0.10,
            z: -1.22,
        },
        size: Vec3f {
            x: 1.62,
            y: 0.72,
            z: 0.18,
        },
        color: [0x1a, 0x24, 0x30, 0xff],
    },
    SceneBox {
        center: Vec3f {
            x: 0.05,
            y: -0.05,
            z: -0.76,
        },
        size: Vec3f {
            x: 0.28,
            y: 0.18,
            z: 0.28,
        },
        color: [0xff, 0x6a, 0x4d, 0xff],
    },
    SceneBox {
        center: Vec3f {
            x: 0.42,
            y: 0.02,
            z: -0.76,
        },
        size: Vec3f {
            x: 0.24,
            y: 0.32,
            z: 0.24,
        },
        color: [0x58, 0xd6, 0x8d, 0xff],
    },
    SceneBox {
        center: Vec3f {
            x: 0.78,
            y: -0.01,
            z: -0.76,
        },
        size: Vec3f {
            x: 0.24,
            y: 0.26,
            z: 0.24,
        },
        color: [0x68, 0xa8, 0xff, 0xff],
    },
    SceneBox {
        center: Vec3f {
            x: 0.42,
            y: 0.34,
            z: -0.76,
        },
        size: Vec3f {
            x: 0.24,
            y: 0.24,
            z: 0.24,
        },
        color: [0xff, 0xff, 0x7a, 0xff],
    },
    SceneBox {
        center: Vec3f {
            x: 0.42,
            y: 0.34,
            z: -1.02,
        },
        size: Vec3f {
            x: 0.16,
            y: 0.82,
            z: 0.16,
        },
        color: [0xff, 0x8a, 0x54, 0xff],
    },
];

const FACE_NORMALS: [Vec3f; 6] = [
    Vec3f {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    },
    Vec3f {
        x: -1.0,
        y: 0.0,
        z: 0.0,
    },
    Vec3f {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    },
    Vec3f {
        x: 0.0,
        y: -1.0,
        z: 0.0,
    },
    Vec3f {
        x: 0.0,
        y: 0.0,
        z: 1.0,
    },
    Vec3f {
        x: 0.0,
        y: 0.0,
        z: -1.0,
    },
];

pub fn default_stream_config(codec: XrRemoteCodec, eye: XrRemoteEye) -> StreamConfigPacket {
    let session = default_session_config();
    StreamConfigPacket {
        eye,
        codec,
        width: session.per_eye_width,
        height: session.per_eye_height,
        fps: session.fps,
        config_id: 0,
    }
}

pub fn render_eye_scene(
    output: &mut Vec<u8>,
    depth: &mut Vec<f32>,
    tracking: &TrackingPacket,
    eye: XrRemoteEye,
    session: &SessionConfigPacket,
) {
    let width = session.per_eye_width as usize;
    let height = session.per_eye_height as usize;
    let pixel_count = width.saturating_mul(height);
    output.resize(pixel_count.saturating_mul(4), 0);
    for px in output.chunks_exact_mut(4) {
        px.copy_from_slice(&CLEAR_BGRA);
    }
    depth.resize(pixel_count, f32::INFINITY);
    depth.fill(f32::INFINITY);
    render_eye(
        output,
        depth,
        width,
        0,
        0,
        width,
        height,
        eye_view(tracking, eye),
    );
}

pub fn eye_view(tracking: &TrackingPacket, eye: XrRemoteEye) -> &EyeViewPacket {
    match eye {
        XrRemoteEye::Left => &tracking.left_eye,
        XrRemoteEye::Right => &tracking.right_eye,
    }
}

pub fn make_tracking_packet(
    tracking_id: u64,
    predicted_display_time_ns: u64,
    head_pose: Pose,
    ipd_meters: f32,
    fov_y_degrees: f32,
    per_eye_width: u32,
    per_eye_height: u32,
    anchor: Option<makepad_widgets::event::xr::XrAnchor>,
) -> TrackingPacket {
    let aspect = per_eye_width as f32 / per_eye_height.max(1) as f32;
    let right = head_pose.orientation.rotate_vec3(&vec3f(1.0, 0.0, 0.0));
    let half_ipd = right.scale(ipd_meters * 0.5);
    TrackingPacket {
        version: XR_REMOTE_PROTOCOL_VERSION,
        tracking_id,
        predicted_display_time_ns,
        head_pose,
        left_eye: EyeViewPacket {
            pose: Pose::new(head_pose.orientation, head_pose.position - half_ipd),
            fov_y_degrees,
            aspect,
        },
        right_eye: EyeViewPacket {
            pose: Pose::new(head_pose.orientation, head_pose.position + half_ipd),
            fov_y_degrees,
            aspect,
        },
        anchor,
    }
}

fn render_eye(
    output: &mut [u8],
    depth: &mut [f32],
    total_width: usize,
    viewport_x: usize,
    viewport_y: usize,
    viewport_width: usize,
    viewport_height: usize,
    eye: &EyeViewPacket,
) {
    let view = eye.pose.to_mat4().invert();
    for scene_box in SCENE_BOXES {
        render_box(
            output,
            depth,
            total_width,
            viewport_x,
            viewport_y,
            viewport_width,
            viewport_height,
            &view,
            eye.fov_y_degrees,
            eye.aspect,
            scene_box,
        );
    }
}

fn render_box(
    output: &mut [u8],
    depth: &mut [f32],
    total_width: usize,
    viewport_x: usize,
    viewport_y: usize,
    viewport_width: usize,
    viewport_height: usize,
    view: &Mat4f,
    fov_y_degrees: f32,
    aspect: f32,
    scene_box: SceneBox,
) {
    let half = scene_box.size.scale(0.5);
    let corners = [
        vec3f(scene_box.center.x - half.x, scene_box.center.y - half.y, scene_box.center.z - half.z),
        vec3f(scene_box.center.x + half.x, scene_box.center.y - half.y, scene_box.center.z - half.z),
        vec3f(scene_box.center.x + half.x, scene_box.center.y + half.y, scene_box.center.z - half.z),
        vec3f(scene_box.center.x - half.x, scene_box.center.y + half.y, scene_box.center.z - half.z),
        vec3f(scene_box.center.x - half.x, scene_box.center.y - half.y, scene_box.center.z + half.z),
        vec3f(scene_box.center.x + half.x, scene_box.center.y - half.y, scene_box.center.z + half.z),
        vec3f(scene_box.center.x + half.x, scene_box.center.y + half.y, scene_box.center.z + half.z),
        vec3f(scene_box.center.x - half.x, scene_box.center.y + half.y, scene_box.center.z + half.z),
    ];
    let faces = [
        (FACE_NORMALS[0], [1usize, 5, 6, 2]),
        (FACE_NORMALS[1], [4usize, 0, 3, 7]),
        (FACE_NORMALS[2], [3usize, 2, 6, 7]),
        (FACE_NORMALS[3], [4usize, 5, 1, 0]),
        (FACE_NORMALS[4], [5usize, 4, 7, 6]),
        (FACE_NORMALS[5], [0usize, 1, 2, 3]),
    ];
    let eye_pos = view.invert().transform_vec4(vec4f(0.0, 0.0, 0.0, 1.0)).to_vec3f();
    for (normal, indices) in faces {
        let face_center = (corners[indices[0]]
            + corners[indices[1]]
            + corners[indices[2]]
            + corners[indices[3]])
            .scale(0.25);
        let to_eye = (eye_pos - face_center).normalize();
        if normal.dot(to_eye) <= 0.0 {
            continue;
        }

        let p0 = match project(corners[indices[0]], view, fov_y_degrees, aspect) {
            Some(value) => value,
            None => continue,
        };
        let p1 = match project(corners[indices[1]], view, fov_y_degrees, aspect) {
            Some(value) => value,
            None => continue,
        };
        let p2 = match project(corners[indices[2]], view, fov_y_degrees, aspect) {
            Some(value) => value,
            None => continue,
        };
        let p3 = match project(corners[indices[3]], view, fov_y_degrees, aspect) {
            Some(value) => value,
            None => continue,
        };

        let light = normal
            .normalize()
            .dot(LIGHT_DIR.normalize())
            .clamp(0.15, 1.0);
        let color = [
            (scene_box.color[0] as f32 * light).clamp(0.0, 255.0) as u8,
            (scene_box.color[1] as f32 * light).clamp(0.0, 255.0) as u8,
            (scene_box.color[2] as f32 * light).clamp(0.0, 255.0) as u8,
            scene_box.color[3],
        ];
        raster_triangle(
            output,
            depth,
            total_width,
            viewport_x,
            viewport_y,
            viewport_width,
            viewport_height,
            p0,
            p1,
            p2,
            color,
        );
        raster_triangle(
            output,
            depth,
            total_width,
            viewport_x,
            viewport_y,
            viewport_width,
            viewport_height,
            p0,
            p2,
            p3,
            color,
        );
    }
}

fn project(vertex: Vec3f, view: &Mat4f, fov_y_degrees: f32, aspect: f32) -> Option<ProjectedVertex> {
    let view_space = view.transform_vec4(vec4f(vertex.x, vertex.y, vertex.z, 1.0));
    if view_space.z >= -0.05 {
        return None;
    }
    let tan_half = (fov_y_degrees.to_radians() * 0.5).tan().max(0.001);
    let ndc_x = (view_space.x / -view_space.z) / (tan_half * aspect.max(0.001));
    let ndc_y = (view_space.y / -view_space.z) / tan_half;
    Some(ProjectedVertex {
        x: (ndc_x * 0.5 + 0.5).clamp(-2.0, 3.0),
        y: (0.5 - ndc_y * 0.5).clamp(-2.0, 3.0),
        depth: -view_space.z,
    })
}

#[allow(clippy::too_many_arguments)]
fn raster_triangle(
    output: &mut [u8],
    depth: &mut [f32],
    total_width: usize,
    viewport_x: usize,
    viewport_y: usize,
    viewport_width: usize,
    viewport_height: usize,
    a: ProjectedVertex,
    b: ProjectedVertex,
    c: ProjectedVertex,
    color: [u8; 4],
) {
    let to_screen = |vertex: ProjectedVertex| -> (f32, f32, f32) {
        (
            vertex.x * viewport_width as f32 + viewport_x as f32,
            vertex.y * viewport_height as f32 + viewport_y as f32,
            vertex.depth,
        )
    };
    let (ax, ay, az) = to_screen(a);
    let (bx, by, bz) = to_screen(b);
    let (cx, cy, cz) = to_screen(c);

    let min_x = ax.min(bx).min(cx).floor().max(viewport_x as f32) as i32;
    let max_x = ax
        .max(bx)
        .max(cx)
        .ceil()
        .min((viewport_x + viewport_width - 1) as f32) as i32;
    let min_y = ay.min(by).min(cy).floor().max(viewport_y as f32) as i32;
    let max_y = ay
        .max(by)
        .max(cy)
        .ceil()
        .min((viewport_y + viewport_height - 1) as f32) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }

    let area = edge(ax, ay, bx, by, cx, cy);
    if area.abs() < 0.0001 {
        return;
    }

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let sample_x = px as f32 + 0.5;
            let sample_y = py as f32 + 0.5;
            let w0 = edge(bx, by, cx, cy, sample_x, sample_y) / area;
            let w1 = edge(cx, cy, ax, ay, sample_x, sample_y) / area;
            let w2 = edge(ax, ay, bx, by, sample_x, sample_y) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let depth_value = az * w0 + bz * w1 + cz * w2;
            let index = py as usize * total_width + px as usize;
            if index >= depth.len() || depth_value >= depth[index] {
                continue;
            }
            depth[index] = depth_value;
            let base = index * 4;
            output[base..base + 4].copy_from_slice(&color);
        }
    }
}

fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}
