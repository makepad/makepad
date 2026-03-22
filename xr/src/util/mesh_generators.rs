use makepad_widgets::*;
#[derive(Clone, Debug, Default)]
pub struct PbrMeshGenerator {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
}

impl PbrMeshGenerator {
    pub fn into_geometry(self, cx: &mut Cx) -> Geometry {
        let geometry = Geometry::new(cx);
        geometry.update(cx, self.indices, self.vertices);
        geometry
    }

    fn push_vertex(
        &mut self,
        position: Vec3f,
        normal: Vec3f,
        uv: Vec2f,
        color: Vec4f,
        tangent: Vec4f,
    ) -> u32 {
        let index = (self.vertices.len() / 16) as u32;
        self.vertices.extend_from_slice(&[
            position.x,
            position.y,
            position.z,
            normal.x,
            normal.y,
            normal.z,
            uv.x,
            uv.y,
            color.x,
            color.y,
            color.z,
            color.w,
            tangent.x,
            tangent.y,
            tangent.z,
            tangent.w,
        ]);
        index
    }

    fn push_oriented_triangle(
        &mut self,
        i0: u32,
        p0: Vec3f,
        i1: u32,
        p1: Vec3f,
        i2: u32,
        p2: Vec3f,
        outward: Vec3f,
    ) {
        let cross = Vec3f::cross(p1 - p0, p2 - p0);
        if cross.dot(outward) >= 0.0 {
            self.indices.extend_from_slice(&[i0, i1, i2]);
        } else {
            self.indices.extend_from_slice(&[i0, i2, i1]);
        }
    }

    fn push_oriented_quad(
        &mut self,
        i0: u32,
        p0: Vec3f,
        i1: u32,
        p1: Vec3f,
        i2: u32,
        p2: Vec3f,
        i3: u32,
        p3: Vec3f,
        outward: Vec3f,
    ) {
        self.push_oriented_triangle(i0, p0, i2, p2, i1, p1, outward);
        self.push_oriented_triangle(i1, p1, i2, p2, i3, p3, outward);
    }
}

pub fn tree_branch_segment_mesh(_sides: usize, top_scale: f32) -> PbrMeshGenerator {
    let top_scale = top_scale.clamp(0.4, 1.0);
    let mut mesh = PbrMeshGenerator::default();
    let color = vec4f(1.0, 1.0, 1.0, 1.0);
    let faces = [
        (
            vec3f(1.0, 0.0, 0.0),
            vec4f(0.0, 0.0, 1.0, 1.0),
            [
                vec3f(1.0, 0.0, -1.0),
                vec3f(1.0, 0.0, 1.0),
                vec3f(top_scale, 1.0, -top_scale),
                vec3f(top_scale, 1.0, top_scale),
            ],
        ),
        (
            vec3f(-1.0, 0.0, 0.0),
            vec4f(0.0, 0.0, -1.0, 1.0),
            [
                vec3f(-1.0, 0.0, 1.0),
                vec3f(-1.0, 0.0, -1.0),
                vec3f(-top_scale, 1.0, top_scale),
                vec3f(-top_scale, 1.0, -top_scale),
            ],
        ),
        (
            vec3f(0.0, 0.0, 1.0),
            vec4f(-1.0, 0.0, 0.0, 1.0),
            [
                vec3f(1.0, 0.0, 1.0),
                vec3f(-1.0, 0.0, 1.0),
                vec3f(top_scale, 1.0, top_scale),
                vec3f(-top_scale, 1.0, top_scale),
            ],
        ),
        (
            vec3f(0.0, 0.0, -1.0),
            vec4f(1.0, 0.0, 0.0, 1.0),
            [
                vec3f(-1.0, 0.0, -1.0),
                vec3f(1.0, 0.0, -1.0),
                vec3f(-top_scale, 1.0, -top_scale),
                vec3f(top_scale, 1.0, -top_scale),
            ],
        ),
    ];

    for (normal, tangent, quad) in faces {
        let i0 = mesh.push_vertex(quad[0], normal, vec2f(0.0, 0.0), color, tangent);
        let i1 = mesh.push_vertex(quad[1], normal, vec2f(1.0, 0.0), color, tangent);
        let i2 = mesh.push_vertex(quad[2], normal, vec2f(0.0, 1.0), color, tangent);
        let i3 = mesh.push_vertex(quad[3], normal, vec2f(1.0, 1.0), color, tangent);
        mesh.push_oriented_quad(i0, quad[0], i1, quad[1], i2, quad[2], i3, quad[3], normal);
    }

    mesh
}

pub fn stylized_leaf_mesh() -> PbrMeshGenerator {
    let mut mesh = PbrMeshGenerator::default();
    let color = vec4f(1.0, 1.0, 1.0, 1.0);
    let tangent = vec4f(1.0, 0.0, 0.0, 1.0);
    let normal = vec3f(0.0, 0.0, 1.0);

    let base = vec3f(0.0, 0.0, 0.0);
    let left = vec3f(-0.34, 0.46, 0.0);
    let tip = vec3f(0.0, 1.0, 0.0);
    let right = vec3f(0.34, 0.46, 0.0);

    let base_i = mesh.push_vertex(base, normal, vec2f(0.5, 0.0), color, tangent);
    let left_i = mesh.push_vertex(left, normal, vec2f(0.0, 0.46), color, tangent);
    let tip_i = mesh.push_vertex(tip, normal, vec2f(0.5, 1.0), color, tangent);
    let right_i = mesh.push_vertex(right, normal, vec2f(1.0, 0.46), color, tangent);

    mesh.push_oriented_triangle(base_i, base, left_i, left, tip_i, tip, normal);
    mesh.push_oriented_triangle(base_i, base, tip_i, tip, right_i, right, normal);
    mesh
}
