// Port of box3d/src/distance.c
// GJK distance, shape cast, and time of impact.
// Dirk Gregorius contributed portions of the original C code.
//
// Float operation order is preserved from the C source — GJK termination
// depends on it. Do not refactor.

use crate::b3_assert;
use crate::b3_validate;
use crate::constants::linear_slop;
use crate::core::log;
use crate::core::NULL_INDEX;
use crate::math_functions::*;
use crate::math_internal::{blend3, scalar_triple_product};
use crate::types::{
    CastOutput, DistanceInput, DistanceOutput, ShapeCastPairInput, ShapeProxy, Simplex,
    SimplexCache, Sweep, TOIInput, TOIOutput, TOIState,
};

pub const MAX_SIMPLEX_VERTICES: usize = 4;
pub const MAX_GJK_ITERATIONS: i32 = 32;

pub fn get_proxy_support(proxy: &ShapeProxy, axis: Vec3) -> i32 {
    let count = proxy.count();
    let points = proxy.points;

    b3_assert!(count > 0);

    // We move the first vertex into the origin for improved precision.
    // This is necessary since we don't have shape transforms and
    // vertices can potentially be far away from the origin (large).
    let origin = points[0];
    let mut max_index = 0;
    let mut max_projection = 0.0;

    for index in 1..count {
        // We subtract the first vertex since we are shifting into the origin.
        let projection = dot(axis, sub(points[index as usize], origin));
        if projection > max_projection {
            max_index = index;
            max_projection = projection;
        }
    }

    max_index
}

pub fn get_point_support(points: &[Vec3], count: i32, axis: Vec3) -> i32 {
    b3_assert!(count > 0);

    // We move the first vertex into the origin for improved precision.
    // This is necessary since we don't have shape transforms and
    // vertices can potentially be far away from the origin (large).
    let origin = points[0];
    let mut max_index = 0;
    let mut max_projection = 0.0;

    for index in 1..count {
        // We subtract the first vertex since we are shifting into the origin.
        let projection = dot(axis, sub(points[index as usize], origin));
        if projection > max_projection {
            max_index = index;
            max_projection = projection;
        }
    }

    max_index
}

fn barycentric_coords_edge(out: &mut [f32; 3], a: Vec3, b: Vec3) {
    let ab = sub(b, a);

    // Last element is divisor
    let divisor = dot(ab, ab);

    out[0] = dot(b, ab);
    out[1] = -dot(a, ab);
    out[2] = divisor;
}

fn barycentric_coords_tri(out: &mut [f32; 4], a: Vec3, b: Vec3, c: Vec3) {
    let ab = sub(b, a);
    let ac = sub(c, a);

    let b_x_c = cross(b, c);
    let c_x_a = cross(c, a);
    let a_x_b = cross(a, b);

    let ab_x_ac = cross(ab, ac);

    // Last element is divisor
    let divisor = dot(ab_x_ac, ab_x_ac);

    out[0] = dot(b_x_c, ab_x_ac);
    out[1] = dot(c_x_a, ab_x_ac);
    out[2] = dot(a_x_b, ab_x_ac);
    out[3] = divisor;
}

fn barycentric_coords_tet(out: &mut [f32; 5], a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ad = sub(d, a);

    // Last element is divisor (forced to be positive)
    let divisor = scalar_triple_product(ab, ac, ad);

    let sign = if divisor < 0.0 { -1.0 } else { 1.0 };
    out[0] = sign * scalar_triple_product(b, c, d);
    out[1] = sign * scalar_triple_product(a, d, c);
    out[2] = sign * scalar_triple_product(a, b, d);
    out[3] = sign * scalar_triple_product(a, c, b);
    out[4] = sign * divisor;
}

fn get_metric(simplex: &Simplex) -> f32 {
    let count = simplex.count;
    b3_assert!(1 <= count && count <= 4);

    let vertices = &simplex.vertices;

    match count {
        1 => 0.0,

        2 => {
            let a = vertices[0].w;
            let b = vertices[1].w;
            distance(a, b)
        }

        3 => {
            let a = vertices[0].w;
            let b = vertices[1].w;
            let c = vertices[2].w;
            length(cross(sub(b, a), sub(c, a))) / 2.0
        }

        4 => {
            let a = vertices[0].w;
            let b = vertices[1].w;
            let c = vertices[2].w;
            let d = vertices[3].w;
            scalar_triple_product(sub(b, a), sub(c, a), sub(d, a)) / 6.0
        }

        _ => {
            b3_assert!(false, "Should never get here!");
            0.0
        }
    }
}

fn write_cache(cache: &mut SimplexCache, simplex: &Simplex) {
    let count = simplex.count;
    cache.metric = get_metric(simplex);
    cache.count = count as u16;
    for index in 0..count as usize {
        cache.index_a[index] = simplex.vertices[index].index_a as u8;
        cache.index_b[index] = simplex.vertices[index].index_b as u8;
    }
}

fn solve_simplex2(simplex: &mut Simplex) -> bool {
    b3_assert!(simplex.count == 2);
    let vs = &mut simplex.vertices;

    let a = vs[0].w;
    let b = vs[1].w;
    let ab = sub(b, a);

    // Last element is divisor
    let divisor = dot(ab, ab);

    let u = dot(b, ab);
    let v = -dot(a, ab);

    // V( A )
    if v <= 0.0 {
        // Reduce simplex
        simplex.count = 1;
        vs[0].a = 1.0;

        return true;
    }

    // V( B )
    if u <= 0.0 {
        // Reduce simplex
        simplex.count = 1;
        vs[0] = vs[1];
        vs[0].a = 1.0;

        return true;
    }

    // Edge region
    if divisor <= 0.0 {
        return false;
    }

    // VR( AB )
    let denominator = 1.0 / divisor;
    vs[0].a = denominator * u;
    vs[1].a = denominator * v;

    true
}

fn solve_simplex3(simplex: &mut Simplex) -> bool {
    b3_assert!(simplex.count == 3);

    // Get simplex (be aware of aliasing here!)
    let v1 = simplex.vertices[0];
    let v2 = simplex.vertices[1];
    let v3 = simplex.vertices[2];

    let vs = &mut simplex.vertices;

    // Vertex regions
    let mut w_ab = [0.0f32; 3];
    let mut w_bc = [0.0f32; 3];
    let mut w_ca = [0.0f32; 3];
    barycentric_coords_edge(&mut w_ab, v1.w, v2.w);
    barycentric_coords_edge(&mut w_bc, v2.w, v3.w);
    barycentric_coords_edge(&mut w_ca, v3.w, v1.w);

    // VR( A )
    if w_ab[1] <= 0.0 && w_ca[0] <= 0.0 {
        // Reduce simplex
        simplex.count = 1;
        vs[0] = v1;
        vs[0].a = 1.0;

        return true;
    }

    // VR( B )
    if w_bc[1] <= 0.0 && w_ab[0] <= 0.0 {
        // Reduce simplex
        simplex.count = 1;
        vs[0] = v2;
        vs[0].a = 1.0;

        return true;
    }

    // VR( C )
    if w_ca[1] <= 0.0 && w_bc[0] <= 0.0 {
        // Reduce simplex
        simplex.count = 1;
        vs[0] = v3;
        vs[0].a = 1.0;

        return true;
    }

    // Edge regions
    let mut w_abc = [0.0f32; 4];
    barycentric_coords_tri(&mut w_abc, v1.w, v2.w, v3.w);

    // VR( AB )
    if w_abc[2] <= 0.0 && w_ab[0] > 0.0 && w_ab[1] > 0.0 {
        // Reduce simplex
        simplex.count = 2;
        vs[0] = v1;
        vs[1] = v2;

        // Normalize
        let divisor = w_ab[2];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_ab[0] / divisor;
        vs[1].a = w_ab[1] / divisor;

        return true;
    }

    // VR( BC )
    if w_abc[0] <= 0.0 && w_bc[0] > 0.0 && w_bc[1] > 0.0 {
        // Reduce simplex
        simplex.count = 2;
        vs[0] = v2;
        vs[1] = v3;

        // Normalize
        let divisor = w_bc[2];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_bc[0] / divisor;
        vs[1].a = w_bc[1] / divisor;

        return true;
    }

    // VR( CA )
    if w_abc[1] <= 0.0 && w_ca[0] > 0.0 && w_ca[1] > 0.0 {
        // Reduce simplex
        simplex.count = 2;
        vs[0] = v3;
        vs[1] = v1;

        // Normalize
        let divisor = w_ca[2];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_ca[0] / divisor;
        vs[1].a = w_ca[1] / divisor;

        return true;
    }

    // Face region
    let divisor = w_abc[3];
    if divisor <= 0.0 {
        return false;
    }

    // VR( ABC )
    vs[0].a = w_abc[0] / divisor;
    vs[1].a = w_abc[1] / divisor;
    vs[2].a = w_abc[2] / divisor;

    true
}

fn solve_simplex4(simplex: &mut Simplex) -> bool {
    // Get simplex (be aware of aliasing here!)
    b3_assert!(simplex.count == 4);
    let vertex_a = simplex.vertices[0];
    let vertex_b = simplex.vertices[1];
    let vertex_c = simplex.vertices[2];
    let vertex_d = simplex.vertices[3];

    let vs = &mut simplex.vertices;

    // Vertex region
    let mut w_ab = [0.0f32; 3];
    let mut w_ac = [0.0f32; 3];
    let mut w_ad = [0.0f32; 3];
    let mut w_bc = [0.0f32; 3];
    let mut w_cd = [0.0f32; 3];
    let mut w_db = [0.0f32; 3];
    barycentric_coords_edge(&mut w_ab, vertex_a.w, vertex_b.w);
    barycentric_coords_edge(&mut w_ac, vertex_a.w, vertex_c.w);
    barycentric_coords_edge(&mut w_ad, vertex_a.w, vertex_d.w);
    barycentric_coords_edge(&mut w_bc, vertex_b.w, vertex_c.w);
    barycentric_coords_edge(&mut w_cd, vertex_c.w, vertex_d.w);
    barycentric_coords_edge(&mut w_db, vertex_d.w, vertex_b.w);

    // VR( A )
    if w_ab[1] <= 0.0 && w_ac[1] <= 0.0 && w_ad[1] <= 0.0 {
        // Reduce simplex
        simplex.count = 1;
        vs[0] = vertex_a;

        vs[0].a = 1.0;

        return true;
    }

    // VR( B )
    if w_ab[0] <= 0.0 && w_db[0] <= 0.0 && w_bc[1] <= 0.0 {
        // Reduce simplex
        simplex.count = 1;
        vs[0] = vertex_b;

        vs[0].a = 1.0;

        return true;
    }

    // VR( C )
    if w_ac[0] <= 0.0 && w_bc[0] <= 0.0 && w_cd[1] <= 0.0 {
        // Reduce simplex
        simplex.count = 1;
        vs[0] = vertex_c;

        vs[0].a = 1.0;

        return true;
    }

    // VR( D )
    if w_ad[0] <= 0.0 && w_cd[0] <= 0.0 && w_db[1] <= 0.0 {
        // Reduce simplex
        simplex.count = 1;
        vs[0] = vertex_d;

        vs[0].a = 1.0;

        return true;
    }

    // Edge region
    let mut w_acb = [0.0f32; 4];
    let mut w_abd = [0.0f32; 4];
    let mut w_adc = [0.0f32; 4];
    let mut w_bcd = [0.0f32; 4];
    barycentric_coords_tri(&mut w_acb, vertex_a.w, vertex_c.w, vertex_b.w);
    barycentric_coords_tri(&mut w_abd, vertex_a.w, vertex_b.w, vertex_d.w);
    barycentric_coords_tri(&mut w_adc, vertex_a.w, vertex_d.w, vertex_c.w);
    barycentric_coords_tri(&mut w_bcd, vertex_b.w, vertex_c.w, vertex_d.w);

    // VR( AB )
    if w_abd[2] <= 0.0 && w_acb[1] <= 0.0 && w_ab[0] > 0.0 && w_ab[1] > 0.0 {
        // Reduce simplex
        simplex.count = 2;
        vs[0] = vertex_a;
        vs[1] = vertex_b;

        // Normalize
        let divisor = w_ab[2];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_ab[0] / divisor;
        vs[1].a = w_ab[1] / divisor;

        return true;
    }

    // VR( AC )
    if w_acb[2] <= 0.0 && w_adc[1] <= 0.0 && w_ac[0] > 0.0 && w_ac[1] > 0.0 {
        // Reduce simplex
        simplex.count = 2;
        vs[0] = vertex_a;
        vs[1] = vertex_c;

        // Normalize
        let divisor = w_ac[2];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_ac[0] / divisor;
        vs[1].a = w_ac[1] / divisor;

        return true;
    }

    // VR( AD )
    if w_adc[2] <= 0.0 && w_abd[1] <= 0.0 && w_ad[0] > 0.0 && w_ad[1] > 0.0 {
        // Reduce simplex
        simplex.count = 2;
        vs[0] = vertex_a;
        vs[1] = vertex_d;

        // Normalize
        let divisor = w_ad[2];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_ad[0] / divisor;
        vs[1].a = w_ad[1] / divisor;

        return true;
    }

    // VR( BC )
    if w_acb[0] <= 0.0 && w_bcd[2] <= 0.0 && w_bc[0] > 0.0 && w_bc[1] > 0.0 {
        // Reduce simplex
        simplex.count = 2;
        vs[0] = vertex_b;
        vs[1] = vertex_c;

        // Normalize
        let divisor = w_bc[2];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_bc[0] / divisor;
        vs[1].a = w_bc[1] / divisor;

        return true;
    }

    // VR( CD )
    if w_adc[0] <= 0.0 && w_bcd[0] <= 0.0 && w_cd[0] > 0.0 && w_cd[1] > 0.0 {
        // Reduce simplex
        simplex.count = 2;
        vs[0] = vertex_c;
        vs[1] = vertex_d;

        // Normalize
        let divisor = w_cd[2];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_cd[0] / divisor;
        vs[1].a = w_cd[1] / divisor;

        return true;
    }

    // VR( DB )
    if w_abd[0] <= 0.0 && w_bcd[1] <= 0.0 && w_db[0] > 0.0 && w_db[1] > 0.0 {
        // Reduce simplex
        simplex.count = 2;
        vs[0] = vertex_d;
        vs[1] = vertex_b;

        // Normalize
        let divisor = w_db[2];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_db[0] / divisor;
        vs[1].a = w_db[1] / divisor;

        return true;
    }

    // Face regions
    let mut w_abcd = [0.0f32; 5];
    barycentric_coords_tet(&mut w_abcd, vertex_a.w, vertex_b.w, vertex_c.w, vertex_d.w);

    // VR( ACB )
    if w_abcd[3] < 0.0 && w_acb[0] > 0.0 && w_acb[1] > 0.0 && w_acb[2] > 0.0 {
        // Reduce simplex
        simplex.count = 3;
        vs[0] = vertex_a;
        vs[1] = vertex_c;
        vs[2] = vertex_b;

        // Normalize
        let divisor = w_acb[3];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_acb[0] / divisor;
        vs[1].a = w_acb[1] / divisor;
        vs[2].a = w_acb[2] / divisor;

        return true;
    }

    // VR( ABD )
    if w_abcd[2] < 0.0 && w_abd[0] > 0.0 && w_abd[1] > 0.0 && w_abd[2] > 0.0 {
        // Reduce simplex
        simplex.count = 3;
        vs[0] = vertex_a;
        vs[1] = vertex_b;
        vs[2] = vertex_d;

        // Normalize
        let divisor = w_abd[3];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_abd[0] / divisor;
        vs[1].a = w_abd[1] / divisor;
        vs[2].a = w_abd[2] / divisor;

        return true;
    }

    // VR( ADC )
    if w_abcd[1] < 0.0 && w_adc[0] > 0.0 && w_adc[1] > 0.0 && w_adc[2] > 0.0 {
        // Reduce simplex
        simplex.count = 3;
        vs[0] = vertex_a;
        vs[1] = vertex_d;
        vs[2] = vertex_c;

        // Normalize
        let divisor = w_adc[3];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_adc[0] / divisor;
        vs[1].a = w_adc[1] / divisor;
        vs[2].a = w_adc[2] / divisor;

        return true;
    }

    // VR( BCD )
    if w_abcd[0] < 0.0 && w_bcd[0] > 0.0 && w_bcd[1] > 0.0 && w_bcd[2] > 0.0 {
        // Reduce simplex
        simplex.count = 3;
        vs[0] = vertex_b;
        vs[1] = vertex_c;
        vs[2] = vertex_d;

        // Normalize
        let divisor = w_bcd[3];
        if divisor <= 0.0 {
            return false;
        }

        vs[0].a = w_bcd[0] / divisor;
        vs[1].a = w_bcd[1] / divisor;
        vs[2].a = w_bcd[2] / divisor;

        return true;
    }

    // *** Inside tetrahedron ***
    let divisor = w_abcd[4];
    if divisor <= 0.0 {
        return false;
    }

    // VR( ABCD )
    vs[0].a = w_abcd[0] / divisor;
    vs[1].a = w_abcd[1] / divisor;
    vs[2].a = w_abcd[2] / divisor;
    vs[3].a = w_abcd[3] / divisor;

    true
}

fn compute_witness_points(simplex: &Simplex, vertex_a: &mut Vec3, vertex_b: &mut Vec3) {
    let vs = &simplex.vertices;
    let count = simplex.count;
    b3_assert!(1 <= count && count <= 4);

    match count {
        1 => {
            *vertex_a = vs[0].w_a;
            *vertex_b = vs[0].w_b;
        }

        2 => {
            *vertex_a = blend2(vs[0].a, vs[0].w_a, vs[1].a, vs[1].w_a);
            *vertex_b = blend2(vs[0].a, vs[0].w_b, vs[1].a, vs[1].w_b);
        }

        3 => {
            *vertex_a = blend3(vs[0].a, vs[0].w_a, vs[1].a, vs[1].w_a, vs[2].a, vs[2].w_a);
            *vertex_b = blend3(vs[0].a, vs[0].w_b, vs[1].a, vs[1].w_b, vs[2].a, vs[2].w_b);
        }

        4 => {
            // Force identical points and *zero* distance
            let sum = add(
                blend2(vs[0].a, vs[0].w_a, vs[1].a, vs[1].w_a),
                blend2(vs[2].a, vs[2].w_a, vs[3].a, vs[3].w_a),
            );
            *vertex_a = sum;
            *vertex_b = sum;
        }

        _ => {
            b3_assert!(false, "Should never get here!");
        }
    }
}

/// Compute the closest points between two shapes represented as point clouds.
/// SimplexCache cache is input/output. On the first call set SimplexCache.count to zero.
/// The query runs in frame A, so the witness points and normal are returned in frame A.
/// The underlying GJK algorithm may be debugged by passing Some(vec) for `simplexes`;
/// unlike the C version (buffer + capacity) the Vec is unbounded.
pub fn shape_distance(
    input: &DistanceInput,
    cache: &mut SimplexCache,
    mut simplexes: Option<&mut Vec<Simplex>>,
) -> DistanceOutput {
    // The query runs in frame A using the relative pose of B in A.
    let xf = input.transform;

    // Use matrices for faster math
    let m = make_matrix_from_quat(xf.q);
    let mt = transpose(m);

    let proxy_a = &input.proxy_a;
    let proxy_b = &input.proxy_b;

    // Compute initial simplex from cache
    b3_assert!((cache.count as usize) <= MAX_SIMPLEX_VERTICES);

    let mut simplex = Simplex::default();

    simplex.count = cache.count as i32;
    for i in 0..cache.count as usize {
        let index1 = cache.index_a[i] as i32;
        let index2 = cache.index_b[i] as i32;

        b3_assert!(0 <= index1 && index1 < proxy_a.count());
        b3_assert!(0 <= index2 && index2 < proxy_b.count());

        let vertex1 = proxy_a.points[index1 as usize];
        let vertex2 = add(mul_mv(m, proxy_b.points[index2 as usize]), xf.p);

        simplex.vertices[i].index_a = index1;
        simplex.vertices[i].index_b = index2;
        simplex.vertices[i].w_a = vertex1;
        simplex.vertices[i].w_b = vertex2;
        simplex.vertices[i].w = sub(vertex2, vertex1);
        simplex.vertices[i].a = 0.0;
    }

    // Compute the new simplex metric, if it is substantially
    // different than the old metric flush the simplex.
    if simplex.count > 0 {
        let metric1 = cache.metric;
        let metric2 = get_metric(&simplex);

        // todo the tetrahedron metric can be negative
        if 2.0 * metric1 < metric2 || metric2 < 0.5 * metric1 || metric2 < f32::EPSILON {
            // Flush the simplex
            simplex.count = 0;
        }
    }

    // If the cache is invalid or empty
    if simplex.count == 0 {
        let vertex1 = proxy_a.points[0];
        let vertex2 = add(mul_mv(m, proxy_b.points[0]), xf.p);

        simplex.count = 1;
        simplex.vertices[0].index_a = 0;
        simplex.vertices[0].index_b = 0;
        simplex.vertices[0].w_a = vertex1;
        simplex.vertices[0].w_b = vertex2;
        simplex.vertices[0].w = sub(vertex2, vertex1);
        simplex.vertices[0].a = 0.0;
    }

    let mut backup = Simplex::default();

    let mut simplex_index = 0;
    if let Some(list) = simplexes.as_deref_mut() {
        list.push(simplex);
        simplex_index += 1;
    }

    let mut distance_output = DistanceOutput::default();

    // Keep track of squared distance
    let mut distance_sq = f32::MAX;

    let mut normal = Vec3::ZERO;

    // Run GJK
    let mut iteration = 0;
    while iteration < MAX_GJK_ITERATIONS {
        // Solve simplex
        let solved = match simplex.count {
            1 => {
                simplex.vertices[0].a = 1.0;
                true
            }
            2 => solve_simplex2(&mut simplex),
            3 => solve_simplex3(&mut simplex),
            4 => solve_simplex4(&mut simplex),
            _ => {
                b3_assert!(false, "Should never get here!");
                false
            }
        };

        if !solved {
            // No progress - reconstruct last simplex
            b3_assert!(backup.count != 0);
            simplex = backup;
            break;
        }

        if let Some(list) = simplexes.as_deref_mut() {
            list.push(simplex);
            simplex_index += 1;
            distance_output.iterations = iteration;
            distance_output.simplex_count = simplex_index;
        }

        if simplex.count == MAX_SIMPLEX_VERTICES as i32 {
            // Overlap
            let mut local_point_a = Vec3::ZERO;
            let mut local_point_b = Vec3::ZERO;
            compute_witness_points(&simplex, &mut local_point_a, &mut local_point_b);
            distance_output.point_a = local_point_a;
            distance_output.point_b = local_point_b;
            return distance_output;
        }

        // Assure distance progression
        let old_distance_sq = distance_sq;

        // Compute closest point
        let closest_point = {
            let vs = &simplex.vertices;
            match simplex.count {
                1 => vs[0].w,
                2 => blend2(vs[0].a, vs[0].w, vs[1].a, vs[1].w),
                3 => blend3(vs[0].a, vs[0].w, vs[1].a, vs[1].w, vs[2].a, vs[2].w),
                4 => add(
                    blend2(vs[0].a, vs[0].w, vs[1].a, vs[1].w),
                    blend2(vs[2].a, vs[2].w, vs[3].a, vs[3].w),
                ),
                _ => {
                    b3_assert!(false, "Should never get here!");
                    Vec3::ZERO
                }
            }
        };

        distance_sq = dot(closest_point, closest_point);

        if distance_sq >= old_distance_sq {
            // No progress - reconstruct last simplex
            b3_assert!(backup.count != 0);
            simplex = backup;
            break;
        }

        // Build new tentative support point
        let search_direction = {
            let vs = &simplex.vertices;
            match simplex.count {
                1 => {
                    // v = -A
                    neg(vs[0].w)
                }

                2 => {
                    // v = (AB x AO) x AB
                    let a = vs[0].w;
                    let b = vs[1].w;

                    let ab = sub(b, a);

                    cross(cross(ab, neg(a)), ab)
                }

                3 => {
                    // v = AB x AC or v = AC x AB
                    let a = vs[0].w;
                    let b = vs[1].w;
                    let c = vs[2].w;

                    let ab = sub(b, a);
                    let ac = sub(c, a);

                    let n = cross(ab, ac);

                    if dot(n, a) < 0.0 {
                        n
                    } else {
                        neg(n)
                    }
                }

                _ => {
                    b3_assert!(false, "Should never get here!");
                    Vec3::ZERO
                }
            }
        };

        if length_squared(search_direction) < 1000.0 * f32::MIN_POSITIVE {
            // The origin is probably contained by a line segment or triangle.
            // Thus the shapes are overlapped.
            let mut local_point_a = Vec3::ZERO;
            let mut local_point_b = Vec3::ZERO;
            compute_witness_points(&simplex, &mut local_point_a, &mut local_point_b);
            distance_output.point_a = local_point_a;
            distance_output.point_b = local_point_b;
            b3_validate!(distance(local_point_a, local_point_b) < f32::EPSILON);
            return distance_output;
        }

        normal = neg(search_direction);

        // Get new support points
        let search_direction1 = search_direction;
        let index_a = get_proxy_support(&input.proxy_a, neg(search_direction1));
        let support_a = input.proxy_a.points[index_a as usize];
        let search_direction2 = mul_mv(mt, search_direction);
        let index_b = get_proxy_support(&input.proxy_b, search_direction2);
        let support_b = add(mul_mv(m, input.proxy_b.points[index_b as usize]), xf.p);

        // Save current simplex and add new vertex - this can fail if we detect cycling
        backup = simplex;

        // Check for duplicate support points. This is the main termination criteria.
        let mut duplicate = false;
        for i in 0..simplex.count as usize {
            if simplex.vertices[i].index_a == index_a && simplex.vertices[i].index_b == index_b {
                duplicate = true;
                break;
            }
        }

        if duplicate {
            break;
        }

        let count = simplex.count as usize;
        simplex.vertices[count].index_a = index_a;
        simplex.vertices[count].index_b = index_b;
        simplex.vertices[count].w_a = support_a;
        simplex.vertices[count].w_b = support_b;
        simplex.vertices[count].w = sub(support_b, support_a);
        simplex.count += 1;

        iteration += 1;
    }

    normal = normalize(normal);
    if !is_normalized(normal) {
        // Treat as overlap
        return distance_output;
    }

    // Build witness points and safe cache
    let mut local_point_a = Vec3::ZERO;
    let mut local_point_b = Vec3::ZERO;
    compute_witness_points(&simplex, &mut local_point_a, &mut local_point_b);
    write_cache(cache, &simplex);

    // Results stay in frame A
    distance_output.point_a = local_point_a;
    distance_output.point_b = local_point_b;
    distance_output.distance = distance(local_point_a, local_point_b);
    distance_output.normal = normal;
    distance_output.iterations = iteration;
    distance_output.simplex_count = simplex_index;

    // Apply radii if requested
    if input.use_radii {
        let r_a = input.proxy_a.radius;
        let r_b = input.proxy_b.radius;
        distance_output.distance = max_float(0.0, distance_output.distance - r_a - r_b);

        // Keep closest points on perimeter even if overlapped, this way the points move smoothly.
        distance_output.point_a = add(distance_output.point_a, mul_sv(r_a, normal));
        distance_output.point_b = sub(distance_output.point_b, mul_sv(r_b, normal));
    }

    distance_output
}

// Separation function:
// f(t) = (c2 + t * dp2 - c1 - t * dp1 ) * n

// Root finding : f(t) - target = 0
// (c2 + t * dp2 - c1 - t * dp1 ) * n - target = 0
// (c2 - c1) * n + t * (dp2 - dp1) * n - target = 0
// t = [target - (c2 - c1) * n] / [(dp2 - dp1) * n]
// t = (target - d) / [(dp2 - dp1) * n]

/// Perform a linear shape cast of shape B moving and shape A fixed. Determines
/// the hit point, normal, and translation fraction. The query runs in frame A,
/// so the hit point and normal are returned in frame A. Initially touching
/// shapes are a miss (unless can_encroach applies).
pub fn shape_cast(input: &ShapeCastPairInput) -> CastOutput {
    // Compute tolerance
    let linear_slop = linear_slop();
    let total_radius = input.proxy_a.radius + input.proxy_b.radius;
    let mut target = max_float(linear_slop, total_radius - linear_slop);
    let tolerance = 0.25 * linear_slop;

    b3_assert!(target > tolerance);

    // Prepare input for distance query
    let mut cache = SimplexCache::default();

    let mut alpha = 0.0;

    let mut distance_input = DistanceInput {
        proxy_a: input.proxy_a,
        proxy_b: input.proxy_b,
        // The whole cast runs in frame A. Advance the relative pose of B in float each iteration,
        // which keeps the math near the local origin and avoids re-relativizing world poses.
        transform: input.transform,
        use_radii: false,
    };

    let delta2 = input.translation_b;
    let mut output = CastOutput::default();
    output.triangle_index = NULL_INDEX;

    let max_iterations = 20;

    for iteration in 0..max_iterations {
        output.iterations += 1;

        let distance_output = shape_distance(&distance_input, &mut cache, None);

        if distance_output.distance < target + tolerance {
            if iteration == 0 {
                if input.can_encroach && distance_output.distance > 2.0 * linear_slop {
                    target = distance_output.distance - linear_slop;
                } else {
                    // Initial overlap
                    output.hit = true;

                    // Compute a common point
                    let c1 = mul_add(distance_output.point_a, input.proxy_a.radius, distance_output.normal);
                    let c2 = mul_add(distance_output.point_b, -input.proxy_b.radius, distance_output.normal);
                    output.point = lerp(c1, c2, 0.5);
                    return output;
                }
            } else {
                // Logging for bad input data
                if distance_output.distance > 0.0 && !is_normalized(distance_output.normal) {
                    for i in 0..input.proxy_a.count() {
                        let p = input.proxy_a.points[i as usize];
                        log(&format!("pointA[{}] = {{{:.9}, {:.9}, {:.9}}}", i, p.x, p.y, p.z));
                    }
                    log(&format!("radiusA = {:.9}", input.proxy_a.radius));

                    for i in 0..input.proxy_b.count() {
                        let p = input.proxy_b.points[i as usize];
                        log(&format!("pointB[{}] = {{{:.9}, {:.9}, {:.9}}}", i, p.x, p.y, p.z));
                    }
                    log(&format!("radiusB = {:.9}", input.proxy_b.radius));

                    {
                        let xf = input.transform;
                        log(&format!(
                            "transform = {{{{{:.9}, {:.9}, {:.9}}}, {{{{{:.9}, {:.9}, {:.9}}}, {:.9}}}",
                            xf.p.x, xf.p.y, xf.p.z, xf.q.v.x, xf.q.v.y, xf.q.v.z, xf.q.s
                        ));
                    }

                    {
                        let t = input.translation_b;
                        log(&format!("t = {{{:.9}, {:.9}, {:.9}}}", t.x, t.y, t.z));
                    }

                    log(&format!(
                        "maxFraction = {:.9}, canEncroach = {}",
                        input.max_fraction, input.can_encroach as i32
                    ));

                    // Numerical problem. Likely extreme input.
                    return output;
                }

                // Hitting this assert implies that the algorithm brought the shapes too close.
                // b3_assert!(distance_output.distance > 0.0 && is_normalized(distance_output.normal));

                output.fraction = alpha;
                output.point = mul_add(distance_output.point_a, input.proxy_a.radius, distance_output.normal);
                output.normal = distance_output.normal;
                output.hit = true;
                return output;
            }
        }

        b3_assert!(distance_output.distance > 0.0);
        b3_assert!(is_normalized(distance_output.normal));

        // Check if shapes are approaching each other
        let denominator = dot(delta2, distance_output.normal);
        if denominator >= 0.0 {
            // Miss
            return output;
        }

        // Advance sweep
        alpha += (target - distance_output.distance) / denominator;
        if alpha >= input.max_fraction {
            // Success!
            return output;
        }

        distance_input.transform.p = mul_add(input.transform.p, alpha, delta2);
    }

    // Failure!
    output
}

/// Evaluate the transform sweep at a specific time.
pub fn get_sweep_transform(sweep: &Sweep, time: f32) -> Transform {
    let q = nlerp(sweep.q1, sweep.q2, time);
    Transform {
        p: sub(lerp(sweep.c1, sweep.c2, time), rotate_vector(q, sweep.local_center)),
        q,
    }
}

#[inline]
fn get_final_sweep_transform(sweep: &Sweep) -> Transform {
    let q = sweep.q2;
    Transform {
        p: sub(sweep.c2, rotate_vector(q, sweep.local_center)),
        q,
    }
}

fn unique_count(vertex_count: i32, vertices: &[i32; 3]) -> i32 {
    b3_assert!(1 <= vertex_count && vertex_count <= 3);

    match vertex_count {
        1 => 1,

        2 => {
            if vertices[0] != vertices[1] {
                2
            } else {
                1
            }
        }

        3 => {
            if vertices[0] != vertices[1] && vertices[0] != vertices[2] && vertices[1] != vertices[2] {
                // All different
                return 3;
            }

            if vertices[0] == vertices[1] && vertices[0] == vertices[2] && vertices[1] == vertices[2] {
                // All equal
                return 1;
            }

            2
        }

        _ => {
            b3_assert!(false, "Should never get here!");
            0
        }
    }
}

// This checks if the cross product of two edges switches direction.
#[inline]
fn check_fast_edges(xf_a: Transform, local_edge_a: Vec3, xf_b: Transform, local_edge_b: Vec3, axis0: Vec3) -> bool {
    // By taking the local witness axes we make sure that we
    // get the correct orientations (e.g. if one axis was flipped)!
    let edge_a = rotate_vector(xf_a.q, local_edge_a);
    let edge_b = rotate_vector(xf_b.q, local_edge_b);
    let axis = cross(edge_a, edge_b);
    dot(axis, axis0) < 0.0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum SeparationType {
    #[default]
    Unknown = 0,
    Vertices,
    Edges,
    FaceA,
    FaceB,
}

struct SeparationFunction<'a> {
    proxy_a: ShapeProxy<'a>,
    proxy_b: ShapeProxy<'a>,
    sweep_a: Sweep,
    sweep_b: Sweep,

    // These are associated with different bodies depending on the separation function type.
    // It could be two local vectors/points on the same body (for example, both on bodyA).
    witness1: Vec3,
    witness2: Vec3,

    type_: SeparationType,
}

fn make_separation_function<'a>(
    cache: SimplexCache,
    proxy_a: ShapeProxy<'a>,
    sweep_a: &Sweep,
    proxy_b: ShapeProxy<'a>,
    sweep_b: &Sweep,
    world_normal: Vec3,
    t1: f32,
) -> SeparationFunction<'a> {
    b3_assert!(1 <= cache.count && cache.count <= 3);
    b3_validate!(is_normalized(world_normal));

    let mut fcn = SeparationFunction {
        proxy_a,
        proxy_b,
        sweep_a: *sweep_a,
        sweep_b: *sweep_b,
        witness1: Vec3::ZERO,
        witness2: Vec3::ZERO,
        type_: SeparationType::Unknown,
    };

    let mut index_a = [cache.index_a[0] as i32, cache.index_a[1] as i32, cache.index_a[2] as i32];
    let mut index_b = [cache.index_b[0] as i32, cache.index_b[1] as i32, cache.index_b[2] as i32];

    let unique_count_a = unique_count(cache.count as i32, &index_a);
    let unique_count_b = unique_count(cache.count as i32, &index_b);

    let xf_a1 = get_sweep_transform(sweep_a, t1);
    let xf_b1 = get_sweep_transform(sweep_b, t1);

    let q_a = xf_a1.q;
    let q_b = xf_b1.q;

    // Minimize round-off
    let delta_p = sub(xf_b1.p, xf_a1.p);

    match cache.count {
        1 => {
            // Witness is the world space direction
            fcn.type_ = SeparationType::Vertices;
            fcn.witness1 = world_normal;
        }

        2 => {
            if unique_count_a == 2 && unique_count_b == 2 {
                // Edge/Edge
                let v_a1 = proxy_a.points[index_a[0] as usize];
                let mut local_edge_a = sub(proxy_a.points[index_a[1] as usize], v_a1);
                local_edge_a = normalize(local_edge_a);
                let edge_a = rotate_vector(q_a, local_edge_a);

                let v_b1 = proxy_b.points[index_b[0] as usize];
                let mut local_edge_b = sub(proxy_b.points[index_b[1] as usize], v_b1);
                local_edge_b = normalize(local_edge_b);
                let edge_b = rotate_vector(q_b, local_edge_b);

                let mut axis = cross(edge_a, edge_b);
                let length_squared = length_squared(axis);

                // Skip near parallel edges: |e1 x e1| = sin(alpha) * |e1| * |e2|
                const K_TOLERANCE_SQUARED: f32 = 0.05 * 0.05;
                if length_squared < K_TOLERANCE_SQUARED {
                    // The axis is not safe to normalize so we use a world axis instead!
                    fcn.type_ = SeparationType::Vertices;
                    fcn.witness1 = world_normal;
                } else {
                    let delta = add(sub(rotate_vector(q_b, v_b1), rotate_vector(q_a, v_a1)), delta_p);
                    if dot(delta, axis) < 0.0 {
                        // Make axis point from A to B
                        axis = neg(axis);
                        local_edge_b = neg(local_edge_b);
                    }

                    // Check for possible sign flip in edge/edge cross product
                    let xf_a2 = get_final_sweep_transform(sweep_a);
                    let xf_b2 = get_final_sweep_transform(sweep_b);
                    let fast_edges = check_fast_edges(xf_a2, local_edge_a, xf_b2, local_edge_b, axis);
                    if fast_edges {
                        // Not safe to use local edges, fall back to initial world space axis instead
                        fcn.type_ = SeparationType::Vertices;
                        fcn.witness1 = normalize(axis);
                    } else {
                        // Edge cross product is safe. This converges faster than a fixed axis.
                        fcn.type_ = SeparationType::Edges;
                        fcn.witness1 = local_edge_a;
                        fcn.witness2 = local_edge_b;
                    }
                }
            } else {
                b3_validate!(is_normalized(world_normal));

                // Vertex versus edge, use world axis witness
                fcn.type_ = SeparationType::Vertices;
                fcn.witness1 = world_normal;
            }
        }

        3 => {
            if unique_count_a == 3 {
                let v_a1 = proxy_a.points[index_a[0] as usize];
                let v_a2 = proxy_a.points[index_a[1] as usize];
                let v_a3 = proxy_a.points[index_a[2] as usize];
                let mut local_axis_a = cross(sub(v_a2, v_a1), sub(v_a3, v_a1));
                local_axis_a = normalize(local_axis_a);
                let axis_a = rotate_vector(q_a, local_axis_a);

                let local_point_a = mul_sv(1.0 / 3.0, add(add(v_a1, v_a2), v_a3));
                let local_point_b = proxy_b.points[index_b[0] as usize];
                let delta = add(
                    sub(rotate_vector(q_b, local_point_b), rotate_vector(q_a, local_point_a)),
                    delta_p,
                );

                if dot(delta, axis_a) < 0.0 {
                    // Make axis point from A to B
                    local_axis_a = neg(local_axis_a);
                }

                // Witness is the local plane of faceA
                fcn.type_ = SeparationType::FaceA;
                fcn.witness1 = local_axis_a;
                fcn.witness2 = local_point_a;
            } else if unique_count_b == 3 {
                let v_b1 = proxy_b.points[index_b[0] as usize];
                let v_b2 = proxy_b.points[index_b[1] as usize];
                let v_b3 = proxy_b.points[index_b[2] as usize];
                let mut local_axis_b = cross(sub(v_b2, v_b1), sub(v_b3, v_b1));
                local_axis_b = normalize(local_axis_b);
                let axis_b = rotate_vector(q_b, local_axis_b);

                let local_point_a = proxy_a.points[index_a[0] as usize];
                let local_point_b = mul_sv(1.0 / 3.0, add(add(v_b1, v_b2), v_b3));
                let delta = sub(
                    sub(rotate_vector(q_a, local_point_a), rotate_vector(q_b, local_point_b)),
                    delta_p,
                );

                if dot(delta, axis_b) < 0.0 {
                    // Make axis point from B to A
                    local_axis_b = neg(local_axis_b);
                }

                // Witness is the local plane of faceB
                fcn.type_ = SeparationType::FaceB;
                fcn.witness1 = local_axis_b;
                fcn.witness2 = local_point_b;
            } else {
                b3_assert!(unique_count_a == 2 && unique_count_b == 2);

                if index_a[0] == index_a[1] {
                    // Make first two indices are unique
                    index_a[1] = index_a[2];
                    b3_assert!(index_a[0] != index_a[1]);
                }

                let v_a1 = proxy_a.points[index_a[0] as usize];
                let v_a2 = proxy_a.points[index_a[1] as usize];
                let local_edge_a = normalize(sub(v_a2, v_a1));
                let edge_a = rotate_vector(q_a, local_edge_a);

                if index_b[0] == index_b[1] {
                    // Make first two indices are unique
                    index_b[1] = index_b[2];
                    b3_assert!(index_b[0] != index_b[1]);
                }

                let v_b1 = proxy_b.points[index_b[0] as usize];
                let v_b2 = proxy_b.points[index_b[1] as usize];
                let mut local_edge_b = normalize(sub(v_b2, v_b1));
                let edge_b = rotate_vector(q_b, local_edge_b);

                let mut axis = cross(edge_a, edge_b);
                let length_squared = length_squared(axis);

                // Skip near parallel edges: |e1 x e1| = sin(alpha) * |e1| * |e2|
                const K_TOLERANCE_SQUARED: f32 = 0.005 * 0.005;
                if length_squared < K_TOLERANCE_SQUARED {
                    // The axis is not safe to normalize so we use a world axis instead!
                    fcn.type_ = SeparationType::Vertices;
                    fcn.witness1 = world_normal;
                } else {
                    let delta = add(sub(rotate_vector(q_b, v_b1), rotate_vector(q_a, v_a1)), delta_p);
                    if dot(delta, axis) < 0.0 {
                        // Make axis point from A to B
                        axis = neg(axis);
                        local_edge_b = neg(local_edge_b);
                    }

                    // Check for possible sign flip in edge/edge cross product
                    let xf_a2 = get_final_sweep_transform(sweep_a);
                    let xf_b2 = get_final_sweep_transform(sweep_b);
                    let fast_edges = check_fast_edges(xf_a2, local_edge_a, xf_b2, local_edge_b, axis);
                    if fast_edges {
                        // Not safe to use local edges, fall back to initial world space axis instead
                        fcn.type_ = SeparationType::Vertices;
                        fcn.witness1 = normalize(axis);
                    } else {
                        // Edge cross product is safe. This converges faster than a fixed axis.
                        fcn.type_ = SeparationType::Edges;
                        fcn.witness1 = local_edge_a;
                        fcn.witness2 = local_edge_b;
                    }
                }
            }
        }

        _ => {
            b3_assert!(false, "Should never get here!");
        }
    }

    fcn
}

fn find_min_separation(fcn: &SeparationFunction, index_a: &mut i32, index_b: &mut i32, t: f32) -> f32 {
    let xf_a = get_sweep_transform(&fcn.sweep_a, t);
    let xf_b = get_sweep_transform(&fcn.sweep_b, t);

    match fcn.type_ {
        SeparationType::Vertices => {
            let axis = fcn.witness1;

            let local_axis_a = inv_rotate_vector(xf_a.q, axis);
            let local_axis_b = inv_rotate_vector(xf_b.q, neg(axis));

            *index_a = get_point_support(fcn.proxy_a.points, fcn.proxy_a.count(), local_axis_a);
            *index_b = get_point_support(fcn.proxy_b.points, fcn.proxy_b.count(), local_axis_b);

            let delta_p = sub(xf_b.p, xf_a.p);
            let local_point_a = fcn.proxy_a.points[*index_a as usize];
            let local_point_b = fcn.proxy_b.points[*index_b as usize];
            let delta = add(
                sub(rotate_vector(xf_b.q, local_point_b), rotate_vector(xf_a.q, local_point_a)),
                delta_p,
            );
            dot(delta, axis)
        }

        SeparationType::Edges => {
            let edge_a = rotate_vector(xf_a.q, fcn.witness1);
            let edge_b = rotate_vector(xf_b.q, fcn.witness2);
            let mut axis = cross(edge_a, edge_b);
            b3_assert!(axis.x != 0.0 || axis.y != 0.0 || axis.z != 0.0);
            axis = normalize(axis);

            let axis_a = inv_rotate_vector(xf_a.q, axis);
            *index_a = get_point_support(fcn.proxy_a.points, fcn.proxy_a.count(), axis_a);

            let axis_b = inv_rotate_vector(xf_b.q, axis);
            *index_b = get_point_support(fcn.proxy_b.points, fcn.proxy_b.count(), neg(axis_b));

            let delta_p = sub(xf_b.p, xf_a.p);
            let local_point_a = fcn.proxy_a.points[*index_a as usize];
            let local_point_b = fcn.proxy_b.points[*index_b as usize];
            let delta = add(
                sub(rotate_vector(xf_b.q, local_point_b), rotate_vector(xf_a.q, local_point_a)),
                delta_p,
            );

            dot(delta, axis)
        }

        SeparationType::FaceA => {
            let normal = rotate_vector(xf_a.q, fcn.witness1);
            *index_a = -1;
            let point_a = transform_point(xf_a, fcn.witness2);

            let axis_b = inv_rotate_vector(xf_b.q, normal);
            *index_b = get_point_support(fcn.proxy_b.points, fcn.proxy_b.count(), neg(axis_b));
            let point_b = transform_point(xf_b, fcn.proxy_b.points[*index_b as usize]);

            dot(sub(point_b, point_a), normal)
        }

        SeparationType::FaceB => {
            let normal = rotate_vector(xf_b.q, fcn.witness1);

            let axis_a = inv_rotate_vector(xf_a.q, normal);
            *index_a = get_point_support(fcn.proxy_a.points, fcn.proxy_a.count(), neg(axis_a));
            let point_a = transform_point(xf_a, fcn.proxy_a.points[*index_a as usize]);

            *index_b = -1;
            let point_b = transform_point(xf_b, fcn.witness2);

            dot(sub(point_a, point_b), normal)
        }

        _ => {
            b3_assert!(false, "Should never get here!");
            0.0
        }
    }
}

fn evaluate_separation(fcn: &SeparationFunction, index1: i32, index2: i32, beta: f32) -> f32 {
    let transform1 = get_sweep_transform(&fcn.sweep_a, beta);
    let transform2 = get_sweep_transform(&fcn.sweep_b, beta);

    match fcn.type_ {
        SeparationType::Vertices => {
            let axis = fcn.witness1;

            let point1 = transform_point(transform1, fcn.proxy_a.points[index1 as usize]);
            let point2 = transform_point(transform2, fcn.proxy_b.points[index2 as usize]);

            dot(sub(point2, point1), axis)
        }

        SeparationType::Edges => {
            let edge1 = rotate_vector(transform1.q, fcn.witness1);
            let edge2 = rotate_vector(transform2.q, fcn.witness2);
            let mut axis = cross(edge1, edge2);
            axis = normalize(axis);

            let point1 = transform_point(transform1, fcn.proxy_a.points[index1 as usize]);
            let point2 = transform_point(transform2, fcn.proxy_b.points[index2 as usize]);

            dot(sub(point2, point1), axis)
        }

        SeparationType::FaceA => {
            let axis = rotate_vector(transform1.q, fcn.witness1);

            let point1 = transform_point(transform1, fcn.witness2);
            let point2 = transform_point(transform2, fcn.proxy_b.points[index2 as usize]);

            dot(sub(point2, point1), axis)
        }

        SeparationType::FaceB => {
            let axis = rotate_vector(transform2.q, fcn.witness1);

            let point1 = transform_point(transform1, fcn.proxy_a.points[index1 as usize]);
            let point2 = transform_point(transform2, fcn.witness2);

            dot(sub(point1, point2), axis)
        }

        _ => {
            b3_assert!(false, "Should never get here!");
            0.0
        }
    }
}

fn force_fixed_axis(fcn: &mut SeparationFunction, beta: f32) {
    b3_assert!(fcn.type_ == SeparationType::Edges);

    let transform1 = get_sweep_transform(&fcn.sweep_a, beta);
    let transform2 = get_sweep_transform(&fcn.sweep_b, beta);

    let edge1 = rotate_vector(transform1.q, fcn.witness1);
    let edge2 = rotate_vector(transform2.q, fcn.witness2);
    let mut axis = cross(edge1, edge2);
    axis = normalize(axis);

    fcn.type_ = SeparationType::Vertices;
    fcn.witness1 = axis;
    fcn.witness2 = Vec3::ZERO;
}

/// Compute the upper bound on time before two shapes penetrate. Time is represented as
/// a fraction between [0,tMax]. This uses a swept separating axis and may miss some intermediate,
/// non-tunneling collisions. If you change the time interval, you should call this function
/// again.
///
/// Time of Impact using root finding.
pub fn time_of_impact(input: &TOIInput) -> TOIOutput {
    let mut output = TOIOutput::default();

    // Set these to invalid values so they can be validated on exit
    output.state = TOIState::Unknown;
    output.fraction = -1.0;

    let mut sweep_a = input.sweep_a;
    let mut sweep_b = input.sweep_b;

    // Shift to origin
    let origin = sweep_a.c1;
    sweep_a.c1 = Vec3::ZERO;
    sweep_a.c2 = sub(sweep_a.c2, origin);
    sweep_b.c1 = sub(sweep_b.c1, origin);
    sweep_b.c2 = sub(sweep_b.c2, origin);

    let proxy_a = input.proxy_a;
    let proxy_b = input.proxy_b;

    let max_push_back_iterations = proxy_a.count() + proxy_b.count();
    let t_max = input.max_fraction;

    // Setup target distance and tolerance
    let linear_slop = linear_slop();
    let total_radius = proxy_a.radius + proxy_b.radius;
    let target = max_float(linear_slop, total_radius - linear_slop);
    let tolerance = 0.25 * linear_slop;
    b3_assert!(target > tolerance);

    let mut t1 = 0.0f32;
    let max_iterations = 25;
    let mut distance_iterations = 0;

    // Prepare input for distance query.
    let mut cache = SimplexCache::default();
    let mut distance_input = DistanceInput {
        proxy_a,
        proxy_b,
        transform: Transform::IDENTITY,
        use_radii: false,
    };

    // The outer loop progressively attempts to compute new separating axes.
    // This loop terminates when an axis is repeated (no progress is made).
    loop {
        // Get the distance between shapes. We can also use the results to get a separating axis.
        let xf_a = get_sweep_transform(&sweep_a, t1);
        let xf_b = get_sweep_transform(&sweep_b, t1);
        distance_input.transform = inv_mul_transforms(xf_a, xf_b);
        let distance_output = shape_distance(&distance_input, &mut cache, None);
        output.distance = distance_output.distance;

        // The distance query runs in frame A, project the witness data back to the shifted world
        let world_normal = rotate_vector(xf_a.q, distance_output.normal);
        let world_point_a = transform_point(xf_a, distance_output.point_a);
        let world_point_b = transform_point(xf_a, distance_output.point_b);

        output.distance_iterations += 1;
        distance_iterations += 1;

        // If the shapes are overlapped, we give up on continuous collision.
        if distance_output.distance <= 0.0 {
            output.state = TOIState::Overlapped;
            output.fraction = 0.0;
            break;
        }

        if distance_output.distance <= target + tolerance {
            // Success!
            output.state = TOIState::Hit;

            // Averaged hit point
            let p_a = mul_add(world_point_a, proxy_a.radius, world_normal);
            let p_b = mul_add(world_point_b, -proxy_b.radius, world_normal);
            output.point = lerp(p_a, p_b, 0.5);
            output.point = add(output.point, origin);
            output.normal = world_normal;
            output.fraction = t1;
            break;
        }

        if distance_iterations == max_iterations {
            // Progress too slow. This can happen when a capsule rotates around a
            // triangle vertex.
            output.state = TOIState::Failed;
            output.fraction = t1;

            // Averaged hit point
            let p_a = mul_add(world_point_a, input.proxy_a.radius, world_normal);
            let p_b = mul_add(world_point_b, -input.proxy_b.radius, world_normal);
            output.point = lerp(p_a, p_b, 0.5);
            output.point = add(output.point, origin);
            output.normal = world_normal;
            break;
        }

        // Initialize the separating axis.
        let mut function = make_separation_function(cache, proxy_a, &sweep_a, proxy_b, &sweep_b, world_normal, t1);

        // Compute the TOI on the separating axis. We do this by successively resolving the deepest point.
        let mut done = false;
        let mut t2 = t_max;
        let mut push_back_iterations = 0;
        loop {
            let mut index_a = 0;
            let mut index_b = 0;
            let mut s2 = find_min_separation(&function, &mut index_a, &mut index_b, t2);

            // Is the final configuration separated?
            if s2 - target > tolerance {
                // Success!
                output.state = TOIState::Separated;
                output.fraction = input.max_fraction;
                done = true;
                break;
            }

            // Has the separation reached tolerance?
            if s2 >= target - tolerance {
                // Advance the sweeps
                t1 = t2;
                break;
            }

            // Compute the initial separation of the witness points
            let mut s1 = evaluate_separation(&function, index_a, index_b, t1);

            // Check for overlap. This might happen if the root finder runs out of iterations.
            if s1 < target - tolerance {
                // Failed!
                b3_validate!(false);
                output.state = TOIState::Failed;
                output.fraction = t1;
                done = true;
                break;
            }

            // Has the separation reached tolerance?
            if s1 <= target + tolerance {
                // Success! t1 should hold the TOI (could be 0.0)
                output.state = TOIState::Hit;
                output.fraction = t1;
                done = true;
                break;
            }

            // Compute 1D root of: f(x) - target = 0
            let mut root_iteration_count = 0;
            let max_root_iterations = 50;
            let mut a1 = t1;
            let mut a2 = t2;
            loop {
                // Use a mix of false position and bisection.
                let t = if root_iteration_count & 1 != 0 {
                    // False position to improve convergence.
                    a1 + (target - s1) * (a2 - a1) / (s2 - s1)
                } else {
                    // Bisection to guarantee progress.
                    0.5 * (a1 + a2)
                };

                output.root_iterations += 1;
                root_iteration_count += 1;

                let s = evaluate_separation(&function, index_a, index_b, t);

                // Has the separation reached tolerance?
                if abs_float(s - target) <= tolerance {
                    // t2 holds a tentative value for t1
                    t2 = t;
                    break;
                }

                // Ensure we continue to bracket the root.
                if s > target {
                    a1 = t;
                    s1 = s;
                } else {
                    a2 = t;
                    s2 = s;
                }

                if root_iteration_count == max_root_iterations {
                    b3_validate!(false);
                    break;
                }
            }

            // Restart the inner loop if we have a failing edge case.
            if root_iteration_count == max_root_iterations - 1 && function.type_ == SeparationType::Edges {
                b3_validate!(false);

                t2 = input.max_fraction;
                force_fixed_axis(&mut function, t1);
                b3_assert!(function.type_ != SeparationType::Edges);
            }

            output.push_back_iterations += 1;
            push_back_iterations += 1;

            if push_back_iterations == max_push_back_iterations {
                break;
            }
        }

        if done {
            // Averaged hit point
            let p_a = mul_add(world_point_a, input.proxy_a.radius, world_normal);
            let p_b = mul_add(world_point_b, -input.proxy_b.radius, world_normal);
            output.point = lerp(p_a, p_b, 0.5);
            output.point = add(output.point, origin);
            output.normal = world_normal;
            break;
        }
    }

    // It is expected that the state and fraction are set before reaching this
    b3_assert!(output.state != TOIState::Unknown);
    b3_assert!(output.fraction >= 0.0);

    output
}
