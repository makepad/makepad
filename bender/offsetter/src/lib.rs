use bender_geometry::{LineSegment, Point, Polygon, Polyline, Vector};

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub join_kind: JoinKind,
    pub cap_kind: CapKind,
    pub miter_limit: f32,
    pub arc_tolerance: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            join_kind: JoinKind::default(),
            cap_kind: CapKind::default(),
            miter_limit: 10.0,
            arc_tolerance: 1E-2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum JoinKind {
    Bevel,
    Miter,
    Round,
}

impl Default for JoinKind {
    fn default() -> Self {
        JoinKind::Miter
    }
}

#[derive(Clone, Copy, Debug)]
pub enum CapKind {
    Butt,
    Square,
    Round,
}

impl Default for CapKind {
    fn default() -> Self {
        CapKind::Butt
    }
}

pub fn offset_polyline(polyline: &Polyline, distance: f32, options: Options) -> Polygon {
    let mut offsets = Vec::new();
    generate_cap_and_joins(polyline.edges(), distance, options, &mut offsets);
    generate_cap_and_joins(
        polyline.edges().rev().map(|edge| edge.reverse()),
        distance,
        options,
        &mut offsets,
    );
    Polygon { vertices: offsets }
}

pub fn offset_polygon(polygon: &Polygon, distance: f32, options: Options) -> Polygon {
    let mut edges = polygon.edges();
    let (vertex, normal) = loop {
        if let Some(edge) = edges.next() {
            if let Some(normal) = edge.normal().normalize() {
                break (edge.start(), normal);
            }
        }
        return Polygon {
            vertices: Vec::new(),
        };
    };
    let mut offsets = Vec::new();
    let front_normal = normal;
    let back_normal = generate_joins(normal, edges, distance, options, &mut offsets);
    generate_join(
        vertex,
        back_normal,
        front_normal,
        distance,
        options,
        &mut offsets,
    );
    Polygon { vertices: offsets }
}

fn generate_cap_and_joins(
    mut edges: impl Iterator<Item = LineSegment>,
    distance: f32,
    options: Options,
    offsets: &mut Vec<Point>,
) {
    let (vertex, normal) = loop {
        if let Some(edge) = edges.next() {
            if let Some(normal) = edge.normal().normalize() {
                break (edge.start(), normal);
            }
        }
        return;
    };
    generate_cap(vertex, normal, distance, options, offsets);
    generate_joins(normal, edges, distance, options, offsets);
}

fn generate_joins(
    mut normal: Vector,
    edges: impl Iterator<Item = LineSegment>,
    distance: f32,
    options: Options,
    offsets: &mut Vec<Point>,
) -> Vector {
    for edge in edges {
        let (next_vertex, next_normal) = if let Some(normal) = edge.normal().normalize() {
            (edge.start(), normal)
        } else {
            break;
        };
        generate_join(next_vertex, normal, next_normal, distance, options, offsets);
        normal = next_normal;
    }
    normal
}

fn generate_cap(
    vertex: Point,
    normal: Vector,
    distance: f32,
    Options {
        cap_kind,
        arc_tolerance,
        ..
    }: Options,
    offsets: &mut Vec<Point>,
) {
    match cap_kind {
        CapKind::Butt => {
            offsets.push(vertex + normal * distance);
            offsets.push(vertex - normal * distance);
        }
        CapKind::Round => generate_round(vertex, -normal, normal, distance, arc_tolerance, offsets),
        CapKind::Square => {
            let tangent = normal.perpendicular();
            let offset_0 = vertex + normal * distance;
            let offset_1 = vertex - normal * distance;
            offsets.push(offset_0);
            offsets.push(offset_0 + tangent * distance);
            offsets.push(offset_1 + tangent * distance);
            offsets.push(offset_1);
        }
    }
}

fn generate_join(
    vertex: Point,
    normal_0: Vector,
    normal_1: Vector,
    distance: f32,
    Options {
        join_kind,
        miter_limit,
        arc_tolerance,
        ..
    }: Options,
    offsets: &mut Vec<Point>,
) {
    let sin_theta = normal_0.cross(normal_1);
    if sin_theta < 0.0 {
        offsets.push(vertex - normal_0 * distance);
        offsets.push(vertex - normal_1 * distance);
        return;
    }
    match join_kind {
        JoinKind::Bevel => {
            offsets.push(vertex - normal_0 * distance);
            offsets.push(vertex - normal_1 * distance);
        }
        JoinKind::Miter => {
            generate_miter(vertex, normal_0, normal_1, distance, miter_limit, offsets)
        }
        JoinKind::Round => {
            generate_round(vertex, normal_0, normal_1, distance, arc_tolerance, offsets)
        }
    }
}

fn generate_miter(
    vertex: Point,
    normal_0: Vector,
    normal_1: Vector,
    distance: f32,
    miter_limit: f32,
    offsets: &mut Vec<Point>,
) {
    offsets.push(vertex - normal_0 * distance);
    let diagonal = normal_0 + normal_1;
    let sin_half_theta = (diagonal / 2.0).length();
    let miter_length = distance / sin_half_theta;
    let miter_ratio = miter_length / (2.0 * distance);
    if miter_ratio < miter_limit {
        offsets.push(vertex - diagonal.normalize().unwrap() * miter_length);
    }
    offsets.push(vertex - normal_1 * distance);
}

fn generate_round(
    vertex: Point,
    normal_0: Vector,
    normal_1: Vector,
    distance: f32,
    arc_tolerance: f32,
    offsets: &mut Vec<Point>,
) {
    let cos_theta = normal_0.dot(normal_1).max(-1.0).min(1.0);
    let theta = cos_theta.acos();
    let delta_theta = (1.0 - arc_tolerance / distance).acos();
    let count = (theta / delta_theta).ceil() as usize;
    if count == 0 {
        offsets.push(vertex + normal_1 * distance);
    } else {
        for index in 0..=count {
            let phi = theta * (index as f32 / count as f32);
            let normal = normal_0 * phi.cos() + normal_0.perpendicular() * phi.sin();
            offsets.push(vertex - normal * distance);
        }
    }
}
