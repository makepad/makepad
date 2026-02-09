// Robust geometric predicates using adaptive precision floating-point arithmetic.
// Based on Jonathan Shewchuk's "Adaptive Precision Floating-Point Arithmetic
// and Fast Robust Geometric Predicates" (1997).
//
// orient3d(a, b, c, d) computes the sign of the determinant:
//
//   | ax-dx  ay-dy  az-dz |
//   | bx-dx  by-dy  bz-dz |
//   | cx-dx  cy-dy  cz-dz |
//
// This equals the signed volume of tetrahedron abcd * 6.
// If a, b, c form a CCW triangle viewed from the +z direction (normal = +z),
// then a point above that plane (positive z) gives a POSITIVE determinant.

use crate::vec3::Vec3d;

// ============================================================================
// Expansion arithmetic primitives
// ============================================================================

/// Exact sum: returns (s, e) where s + e = a + b exactly.
#[inline(always)]
fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let bv = x - a;
    let av = x - bv;
    let br = b - bv;
    let ar = a - av;
    (x, ar + br)
}

/// Exact difference: returns (s, e) where s + e = a - b exactly.
#[inline(always)]
fn two_diff(a: f64, b: f64) -> (f64, f64) {
    let x = a - b;
    let bv = a - x;
    let av = x + bv;
    let br = bv - b;
    let ar = a - av;
    (x, ar + br)
}

/// Split a f64 into high and low parts for exact multiplication.
#[inline(always)]
fn split(a: f64) -> (f64, f64) {
    let c = 134217729.0 * a; // (2^27 + 1) * a
    let abig = c - a;
    let ahi = c - abig;
    let alo = a - ahi;
    (ahi, alo)
}

/// Exact product: returns (p, e) where p + e = a * b exactly.
#[inline(always)]
fn two_product(a: f64, b: f64) -> (f64, f64) {
    let x = a * b;
    let (ahi, alo) = split(a);
    let (bhi, blo) = split(b);
    let err = ((ahi * bhi - x) + ahi * blo + alo * bhi) + alo * blo;
    (x, err)
}

// ============================================================================
// Expansion arithmetic: variable-length exact sums
// ============================================================================

/// Add a scalar to an expansion. The expansion must be sorted by magnitude.
/// Returns a new expansion (also sorted by magnitude).
fn grow_expansion(e: &[f64], b: f64) -> Vec<f64> {
    let mut result = Vec::with_capacity(e.len() + 1);
    let mut q = b;
    for &ei in e {
        let (nq, ne) = two_sum(q, ei);
        if ne != 0.0 {
            result.push(ne);
        }
        q = nq;
    }
    if q != 0.0 || result.is_empty() {
        result.push(q);
    }
    result
}

/// Multiply an expansion by a scalar.
fn scale_expansion(e: &[f64], b: f64) -> Vec<f64> {
    if e.is_empty() || b == 0.0 {
        return vec![0.0];
    }
    let (_bhi, _blo) = split(b);
    let mut result = Vec::with_capacity(e.len() * 2);
    let (mut q, hh) = two_product(e[0], b);
    if hh != 0.0 {
        result.push(hh);
    }
    for &ei in &e[1..] {
        let (ti1, ti0) = two_product(ei, b);
        let (nq, nqe) = two_sum(q, ti0);
        if nqe != 0.0 {
            result.push(nqe);
        }
        let (nq2, nqe2) = two_sum(ti1, nq);
        if nqe2 != 0.0 {
            result.push(nqe2);
        }
        q = nq2;
    }
    if q != 0.0 || result.is_empty() {
        result.push(q);
    }
    result
}

/// Sum two expansions.
fn expansion_sum(e: &[f64], f: &[f64]) -> Vec<f64> {
    let mut result = e.to_vec();
    for &fi in f {
        result = grow_expansion(&result, fi);
    }
    result
}

/// Negate an expansion.
fn expansion_negate(e: &[f64]) -> Vec<f64> {
    e.iter().map(|&x| -x).collect()
}

/// Get the most significant component (the approximate value).
fn expansion_approx(e: &[f64]) -> f64 {
    if e.is_empty() {
        0.0
    } else {
        e[e.len() - 1]
    }
}

// ============================================================================
// orient2d
// ============================================================================

/// Robust orient2d.
/// Positive if a->b->c is counterclockwise, negative if clockwise, zero if collinear.
pub fn orient2d(ax: f64, ay: f64, bx: f64, by: f64, cx: f64, cy: f64) -> f64 {
    let acx = ax - cx;
    let bcx = bx - cx;
    let acy = ay - cy;
    let bcy = by - cy;

    let det = acx * bcy - acy * bcx;
    let det_sum = (acx * bcy).abs() + (acy * bcx).abs();
    let errbound = 3.3306690738754716e-16 * det_sum;

    if det.abs() > errbound {
        return det;
    }

    // Exact fallback
    let (acx1, acx0) = two_diff(ax, cx);
    let (acy1, acy0) = two_diff(ay, cy);
    let (bcx1, bcx0) = two_diff(bx, cx);
    let (bcy1, bcy0) = two_diff(by, cy);

    // acx * bcy using exact two-component multiplication
    let left = product_2x2(acx1, acx0, bcy1, bcy0);
    // acy * bcx
    let right = product_2x2(acy1, acy0, bcx1, bcx0);

    let diff = expansion_sum(&left, &expansion_negate(&right));
    expansion_approx(&diff)
}

/// Multiply two 2-component expansions: (a1+a0) * (b1+b0)
fn product_2x2(a1: f64, a0: f64, b1: f64, b0: f64) -> Vec<f64> {
    let (p1, p0) = two_product(a0, b0);
    let mut e = if p0 != 0.0 { vec![p0, p1] } else { vec![p1] };

    let (p1, p0) = two_product(a1, b0);
    e = expansion_sum(&e, &if p0 != 0.0 { vec![p0, p1] } else { vec![p1] });

    let (p1, p0) = two_product(a0, b1);
    e = expansion_sum(&e, &if p0 != 0.0 { vec![p0, p1] } else { vec![p1] });

    let (p1, p0) = two_product(a1, b1);
    e = expansion_sum(&e, &if p0 != 0.0 { vec![p0, p1] } else { vec![p1] });

    e
}

// ============================================================================
// orient3d
// ============================================================================

/// Robust orient3d.
///
/// Internally computes the sign of the 3x3 determinant:
///   | ax-dx  ay-dy  az-dz |
///   | bx-dx  by-dy  bz-dz |
///   | cx-dx  cy-dy  cz-dz |
///
/// but returns the NEGATED value so that our CSG convention holds:
///   positive => d is on the front side (same side as the CCW normal of triangle abc)
///   negative => d is on the back side
///   zero     => d is coplanar
///
/// For a CCW triangle in the XY plane (normal = +Z), a point with z > 0
/// gives a positive result.
pub fn orient3d(a: Vec3d, b: Vec3d, c: Vec3d, d: Vec3d) -> f64 {
    let adx = a.x - d.x;
    let ady = a.y - d.y;
    let adz = a.z - d.z;
    let bdx = b.x - d.x;
    let bdy = b.y - d.y;
    let bdz = b.z - d.z;
    let cdx = c.x - d.x;
    let cdy = c.y - d.y;
    let cdz = c.z - d.z;

    let bdxcdy = bdx * cdy;
    let cdxbdy = cdx * bdy;
    let cdxady = cdx * ady;
    let adxcdy = adx * cdy;
    let adxbdy = adx * bdy;
    let bdxady = bdx * ady;

    let det = adz * (bdxcdy - cdxbdy) + bdz * (cdxady - adxcdy) + cdz * (adxbdy - bdxady);

    let permanent = (bdxcdy.abs() + cdxbdy.abs()) * adz.abs()
        + (cdxady.abs() + adxcdy.abs()) * bdz.abs()
        + (adxbdy.abs() + bdxady.abs()) * cdz.abs();
    let errbound = 7.7715611723761027e-16 * permanent;

    if det > errbound || det < -errbound {
        return -det;
    }

    if permanent == 0.0 {
        return 0.0;
    }

    // Exact fallback using expansion arithmetic
    -orient3d_exact(a, b, c, d)
}

/// Fully exact orient3d using expansion arithmetic.
fn orient3d_exact(a: Vec3d, b: Vec3d, c: Vec3d, d: Vec3d) -> f64 {
    // Compute exact differences
    let (adx1, adx0) = two_diff(a.x, d.x);
    let (ady1, ady0) = two_diff(a.y, d.y);
    let (adz1, adz0) = two_diff(a.z, d.z);
    let (bdx1, bdx0) = two_diff(b.x, d.x);
    let (bdy1, bdy0) = two_diff(b.y, d.y);
    let (bdz1, bdz0) = two_diff(b.z, d.z);
    let (cdx1, cdx0) = two_diff(c.x, d.x);
    let (cdy1, cdy0) = two_diff(c.y, d.y);
    let (cdz1, cdz0) = two_diff(c.z, d.z);

    // bc = bdx * cdy - cdx * bdy
    let bc_pos = product_2x2(bdx1, bdx0, cdy1, cdy0);
    let bc_neg = product_2x2(cdx1, cdx0, bdy1, bdy0);
    let bc = expansion_sum(&bc_pos, &expansion_negate(&bc_neg));

    // ca = cdx * ady - adx * cdy
    let ca_pos = product_2x2(cdx1, cdx0, ady1, ady0);
    let ca_neg = product_2x2(adx1, adx0, cdy1, cdy0);
    let ca = expansion_sum(&ca_pos, &expansion_negate(&ca_neg));

    // ab = adx * bdy - bdx * ady
    let ab_pos = product_2x2(adx1, adx0, bdy1, bdy0);
    let ab_neg = product_2x2(bdx1, bdx0, ady1, ady0);
    let ab = expansion_sum(&ab_pos, &expansion_negate(&ab_neg));

    // det = adz * bc + bdz * ca + cdz * ab
    // Each z-component is a 2-component expansion, multiply by the cross terms
    let mut det = Vec::new();

    // adz * bc
    let t = scale_expansion(&bc, adz0);
    det = expansion_sum(&det, &t);
    let t = scale_expansion(&bc, adz1);
    det = expansion_sum(&det, &t);

    // bdz * ca
    let t = scale_expansion(&ca, bdz0);
    det = expansion_sum(&det, &t);
    let t = scale_expansion(&ca, bdz1);
    det = expansion_sum(&det, &t);

    // cdz * ab
    let t = scale_expansion(&ab, cdz0);
    det = expansion_sum(&det, &t);
    let t = scale_expansion(&ab, cdz1);
    det = expansion_sum(&det, &t);

    expansion_approx(&det)
}

// ============================================================================
// in_circle (2D)
// ============================================================================

/// Robust in_circle test.
/// Returns positive if d is inside the circumcircle of (a, b, c) (assuming CCW),
/// negative if outside, zero if on the circle.
pub fn in_circle(ax: f64, ay: f64, bx: f64, by: f64, cx: f64, cy: f64, dx: f64, dy: f64) -> f64 {
    let adx = ax - dx;
    let ady = ay - dy;
    let bdx = bx - dx;
    let bdy = by - dy;
    let cdx = cx - dx;
    let cdy = cy - dy;

    let abdet = adx * bdy - bdx * ady;
    let bcdet = bdx * cdy - cdx * bdy;
    let cadet = cdx * ady - adx * cdy;
    let alift = adx * adx + ady * ady;
    let blift = bdx * bdx + bdy * bdy;
    let clift = cdx * cdx + cdy * cdy;

    let det = alift * bcdet + blift * cadet + clift * abdet;

    let permanent = (bdx * cdy).abs()
        + (cdx * bdy).abs() * alift
        + ((cdx * ady).abs() + (adx * cdy).abs()) * blift
        + ((adx * bdy).abs() + (bdx * ady).abs()) * clift;
    // Error bound from Shewchuk's incircle
    let errbound = 1.1102230246251565e-15 * permanent;

    if det > errbound || det < -errbound {
        return det;
    }

    // Exact fallback
    in_circle_exact(ax, ay, bx, by, cx, cy, dx, dy)
}

/// Fully exact in_circle using expansion arithmetic.
fn in_circle_exact(ax: f64, ay: f64, bx: f64, by: f64, cx: f64, cy: f64, dx: f64, dy: f64) -> f64 {
    let (adx1, adx0) = two_diff(ax, dx);
    let (ady1, ady0) = two_diff(ay, dy);
    let (bdx1, bdx0) = two_diff(bx, dx);
    let (bdy1, bdy0) = two_diff(by, dy);
    let (cdx1, cdx0) = two_diff(cx, dx);
    let (cdy1, cdy0) = two_diff(cy, dy);

    // bc = bdx * cdy - cdx * bdy
    let bc = expansion_sum(
        &product_2x2(bdx1, bdx0, cdy1, cdy0),
        &expansion_negate(&product_2x2(cdx1, cdx0, bdy1, bdy0)),
    );
    // ca = cdx * ady - adx * cdy
    let ca = expansion_sum(
        &product_2x2(cdx1, cdx0, ady1, ady0),
        &expansion_negate(&product_2x2(adx1, adx0, cdy1, cdy0)),
    );
    // ab = adx * bdy - bdx * ady
    let ab = expansion_sum(
        &product_2x2(adx1, adx0, bdy1, bdy0),
        &expansion_negate(&product_2x2(bdx1, bdx0, ady1, ady0)),
    );

    // alift = adx^2 + ady^2
    let adx_sq = product_2x2(adx1, adx0, adx1, adx0);
    let ady_sq = product_2x2(ady1, ady0, ady1, ady0);
    let alift = expansion_sum(&adx_sq, &ady_sq);

    // blift = bdx^2 + bdy^2
    let bdx_sq = product_2x2(bdx1, bdx0, bdx1, bdx0);
    let bdy_sq = product_2x2(bdy1, bdy0, bdy1, bdy0);
    let blift = expansion_sum(&bdx_sq, &bdy_sq);

    // clift = cdx^2 + cdy^2
    let cdx_sq = product_2x2(cdx1, cdx0, cdx1, cdx0);
    let cdy_sq = product_2x2(cdy1, cdy0, cdy1, cdy0);
    let clift = expansion_sum(&cdx_sq, &cdy_sq);

    // det = alift * bc + blift * ca + clift * ab
    let mut det = Vec::new();
    // alift * bc: multiply each term of alift by each term of bc
    det = expansion_product_sum(&det, &alift, &bc);
    det = expansion_product_sum(&det, &blift, &ca);
    det = expansion_product_sum(&det, &clift, &ab);

    expansion_approx(&det)
}

/// Multiply two expansions and add to accumulator.
fn expansion_product_sum(acc: &[f64], a: &[f64], b: &[f64]) -> Vec<f64> {
    let mut result = acc.to_vec();
    for &ai in a {
        let term = scale_expansion(b, ai);
        result = expansion_sum(&result, &term);
    }
    result
}

// ============================================================================
// Robust point classification for CSG
// ============================================================================

/// Classify point d relative to the oriented plane through a, b, c.
/// Uses Shewchuk's exact orient3d.
///
/// Returns:
///   Side::Front if orient3d(a, b, c, d) > 0
///   Side::Back if orient3d(a, b, c, d) < 0
///   Side::On if orient3d(a, b, c, d) == 0
pub fn robust_classify_point(a: Vec3d, b: Vec3d, c: Vec3d, d: Vec3d) -> crate::plane::Side {
    let o = orient3d(a, b, c, d);
    if o > 0.0 {
        crate::plane::Side::Front
    } else if o < 0.0 {
        crate::plane::Side::Back
    } else {
        crate::plane::Side::On
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec3::dvec3;

    #[test]
    fn test_two_sum_basic() {
        let (s, e) = two_sum(1.0, 2.0);
        assert_eq!(s, 3.0);
        assert_eq!(e, 0.0);
    }

    #[test]
    fn test_two_sum_captures_roundoff() {
        let a = 1.0f64;
        let b = 1e-16f64;
        let (s, e) = two_sum(a, b);
        // The naive sum loses b, but two_sum captures it in e
        // Verify s + e represents the exact sum
        assert_eq!(s, 1.0); // 1.0 + 1e-16 rounds to 1.0
        assert!(e != 0.0); // but the error captures the lost 1e-16
    }

    #[test]
    fn test_two_product() {
        let (p, e) = two_product(3.0, 7.0);
        assert_eq!(p, 21.0);
        assert_eq!(e, 0.0);
    }

    #[test]
    fn test_orient3d_clear_above() {
        // Triangle in XY plane, CCW: (0,0,0), (1,0,0), (0,1,0)
        // Normal points in +Z direction
        // Point at (0,0,1) is above -> should be positive
        let a = dvec3(0.0, 0.0, 0.0);
        let b = dvec3(1.0, 0.0, 0.0);
        let c = dvec3(0.0, 1.0, 0.0);
        let d = dvec3(0.0, 0.0, 1.0);
        let r = orient3d(a, b, c, d);
        assert!(
            r > 0.0,
            "point above CCW plane should give positive, got {}",
            r
        );
    }

    #[test]
    fn test_orient3d_clear_below() {
        let a = dvec3(0.0, 0.0, 0.0);
        let b = dvec3(1.0, 0.0, 0.0);
        let c = dvec3(0.0, 1.0, 0.0);
        let d = dvec3(0.0, 0.0, -1.0);
        let r = orient3d(a, b, c, d);
        assert!(
            r < 0.0,
            "point below CCW plane should give negative, got {}",
            r
        );
    }

    #[test]
    fn test_orient3d_coplanar() {
        let a = dvec3(0.0, 0.0, 0.0);
        let b = dvec3(1.0, 0.0, 0.0);
        let c = dvec3(0.0, 1.0, 0.0);
        let d = dvec3(1.0, 1.0, 0.0);
        assert_eq!(orient3d(a, b, c, d), 0.0);
    }

    #[test]
    fn test_orient3d_coplanar_origin() {
        let a = dvec3(0.0, 0.0, 0.0);
        let b = dvec3(1.0, 0.0, 0.0);
        let c = dvec3(0.0, 1.0, 0.0);
        let d = dvec3(0.5, 0.5, 0.0);
        assert_eq!(orient3d(a, b, c, d), 0.0);
    }

    #[test]
    fn test_orient3d_near_coplanar() {
        let a = dvec3(0.0, 0.0, 0.0);
        let b = dvec3(1.0, 0.0, 0.0);
        let c = dvec3(0.0, 1.0, 0.0);

        // Slightly above -> should be positive
        let d = dvec3(0.3, 0.3, 1e-15);
        let r = orient3d(a, b, c, d);
        assert!(
            r > 0.0,
            "expected positive for near-coplanar above, got {}",
            r
        );

        // Slightly below -> should be negative
        let d = dvec3(0.3, 0.3, -1e-15);
        let r = orient3d(a, b, c, d);
        assert!(
            r < 0.0,
            "expected negative for near-coplanar below, got {}",
            r
        );
    }

    #[test]
    fn test_orient3d_large_coordinates() {
        let offset = 1e10;
        let a = dvec3(offset, offset, offset);
        let b = dvec3(offset + 1.0, offset, offset);
        let c = dvec3(offset, offset + 1.0, offset);
        let d = dvec3(offset + 0.5, offset + 0.5, offset + 1.0);
        let r = orient3d(a, b, c, d);
        assert!(
            r > 0.0,
            "expected positive for large coords above, got {}",
            r
        );

        let d = dvec3(offset + 0.5, offset + 0.5, offset - 1.0);
        let r = orient3d(a, b, c, d);
        assert!(
            r < 0.0,
            "expected negative for large coords below, got {}",
            r
        );
    }

    #[test]
    fn test_orient2d_ccw() {
        assert!(orient2d(0.0, 0.0, 1.0, 0.0, 0.0, 1.0) > 0.0);
    }

    #[test]
    fn test_orient2d_cw() {
        assert!(orient2d(0.0, 0.0, 0.0, 1.0, 1.0, 0.0) < 0.0);
    }

    #[test]
    fn test_orient2d_collinear() {
        assert_eq!(orient2d(0.0, 0.0, 1.0, 1.0, 2.0, 2.0), 0.0);
    }

    #[test]
    fn test_robust_classify() {
        let a = dvec3(0.0, 0.0, 0.0);
        let b = dvec3(1.0, 0.0, 0.0);
        let c = dvec3(0.0, 1.0, 0.0);

        use crate::plane::Side;
        assert_eq!(
            robust_classify_point(a, b, c, dvec3(0.0, 0.0, 1.0)),
            Side::Front
        );
        assert_eq!(
            robust_classify_point(a, b, c, dvec3(0.0, 0.0, -1.0)),
            Side::Back
        );
        assert_eq!(
            robust_classify_point(a, b, c, dvec3(0.5, 0.5, 0.0)),
            Side::On
        );
    }

    #[test]
    fn test_orient3d_symmetry() {
        // Swapping two vertices should negate the determinant
        let a = dvec3(1.0, 2.0, 3.0);
        let b = dvec3(4.0, 5.0, 1.0);
        let c = dvec3(2.0, 8.0, 4.0);
        let d = dvec3(3.0, 3.0, 3.0);
        let r1 = orient3d(a, b, c, d);
        let r2 = orient3d(b, a, c, d);
        assert!(
            (r1 + r2).abs() < 1e-10,
            "swapping two points should negate: {} vs {}",
            r1,
            r2
        );
    }

    #[test]
    fn test_grow_expansion() {
        let e = vec![1.0];
        let r = grow_expansion(&e, 2.0);
        assert_eq!(*r.last().unwrap(), 3.0);
    }

    #[test]
    fn test_scale_expansion() {
        let e = vec![3.0];
        let r = scale_expansion(&e, 7.0);
        assert_eq!(*r.last().unwrap(), 21.0);
    }
}
