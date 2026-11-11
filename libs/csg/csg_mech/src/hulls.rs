//! A part's convex hull as the sim meets it: coarse but close (a few of its corners, whose own hull keeps
//! within a distance of it).
//!
//! Model frame, millimetres.

use crate::geom::*;
use makepad_dynamics::hull::convex_hull;
use std::collections::HashMap;

/// A hull of `points` with corners only from them, as few as greedily found, that keeps within `tol` (mm) of
/// their whole hull: no corner of the whole hull is farther than `tol` from it, so no direction finds it
/// short of the whole by more (the farthest a point of the whole lies from it is at a corner). It starts
/// from the corners farthest along 26 directions and grows by rounds: of its faces with a corner of the
/// whole farther than `tol` outside, each whose farthest is farther than those of the faces around it takes
/// that corner, until none is left. Its size follows the hull's curvature, not its meshing: a mesher-fine
/// ball of 6 mm keeps about 1200 corners at 0.02 mm, however finely meshed. A flat or degenerate cloud is
/// returned as it is.
pub fn coarse_hull(points: &[P3], tol: f64) -> Vec<P3> {
    let Some((ids, _)) = hull_faces(points) else { return points.to_vec() };
    let corners: Vec<P3> = ids.iter().map(|&i| points[i]).collect();
    let mut chosen = vec![false; corners.len()];
    let mut picked: Vec<usize> = Vec::new();
    for d in (0..27).filter(|&k| k != 13).map(|k| [(k % 3) as f64 - 1.0, ((k / 3) % 3) as f64 - 1.0, (k / 9) as f64 - 1.0]) {
        let k = (0..corners.len()).max_by(|&a, &b| dot(corners[a], d).total_cmp(&dot(corners[b], d))).unwrap();
        if !chosen[k] {
            chosen[k] = true;
            picked.push(k);
        }
    }
    // each corner's nearest picked corner, where its search for the coarse hull's nearest face starts
    let mut home: Vec<usize> = corners
        .iter()
        .map(|&p| *picked.iter().min_by(|&&a, &&b| len(sub(corners[a], p)).total_cmp(&len(sub(corners[b], p)))).unwrap())
        .collect();
    // the corners not yet within `tol` of it: as it grows, those within stay within
    let mut open: Vec<usize> = (0..corners.len()).filter(|&i| !chosen[i]).collect();
    loop {
        let sub_points: Vec<P3> = picked.iter().map(|&k| corners[k]).collect();
        let Some((kept, faces)) = hull_faces(&sub_points) else { return corners };
        // the coarse hull's corners and faces as corners of the whole; the faces around each of its corners
        let kept: Vec<usize> = kept.iter().map(|&i| picked[i]).collect();
        let faces: Vec<[usize; 3]> = faces.iter().map(|f| f.map(|i| picked[i])).collect();
        let mut around: HashMap<usize, Vec<usize>> = HashMap::new();
        for (f, face) in faces.iter().enumerate() {
            for &c in face {
                around.entry(c).or_default().push(f);
            }
        }
        let mut worst: Vec<Option<(f64, usize)>> = vec![None; faces.len()];
        open.retain(|&i| {
            if chosen[i] {
                return false;
            }
            let start = around.get(&home[i]).map_or(0, |fs| fs[0]);
            let (f, d) = nearest_face(corners[i], &corners, &faces, &around, start);
            home[i] = *faces[f].iter().min_by(|&&a, &&b| len(sub(corners[a], corners[i])).total_cmp(&len(sub(corners[b], corners[i])))).unwrap();
            if d > tol && worst[f].is_none_or(|(w, _)| d > w) {
                worst[f] = Some((d, i));
            }
            d > tol
        });
        // a face's farthest corner joins if no face sharing a corner with it has one farther: the faces next to
        // a corner that joins are cut down by it, and their own farthest may well come within `tol`
        let farther = |g: usize, f: usize, d: f64| worst[g].is_some_and(|(e, _)| e > d || (e == d && g < f));
        let far: Vec<usize> = (0..faces.len())
            .filter_map(|f| worst[f].filter(|&(d, _)| !faces[f].iter().any(|c| around[c].iter().any(|&g| farther(g, f, d)))).map(|(_, i)| i))
            .collect();
        if far.is_empty() {
            return kept.iter().map(|&k| corners[k]).collect();
        }
        for i in far {
            chosen[i] = true;
            picked.push(i);
        }
    }
}

/// The hull of `points`: its corners and its faces (counter-clockwise seen from outside), as indices into
/// `points`. None for a flat or degenerate cloud.
fn hull_faces(points: &[P3]) -> Option<(Vec<usize>, Vec<[usize; 3]>)> {
    let flat: Vec<f64> = points.iter().flatten().copied().collect();
    let hull = convex_hull(&flat, points.len()).ok()?;
    Some((hull.vertices.iter().map(|&id| id as usize).collect(), hull.faces.iter().map(|f| f.map(|id| id as usize)).collect()))
}

/// The face of a convex hull nearest to `p` outside it, and how far: from `start`, to whichever face sharing
/// a corner with the current one is nearer, until none is. On a convex hull the faces within any distance of
/// an outside point join up, so the walk ends at the nearest.
fn nearest_face(p: P3, corners: &[P3], faces: &[[usize; 3]], around: &HashMap<usize, Vec<usize>>, start: usize) -> (usize, f64) {
    let gap = |f: usize| len(sub(p, closest_on_triangle(p, corners[faces[f][0]], corners[faces[f][1]], corners[faces[f][2]])));
    let (mut f, mut d) = (start, gap(start));
    loop {
        let next = faces[f].iter().flat_map(|c| around[c].iter().copied()).map(|g| (g, gap(g))).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        if next.1 >= d {
            return (f, d);
        }
        (f, d) = next;
    }
}

/// The point of triangle `abc` nearest to `p` (by its Voronoi regions: corners, edges, inside).
fn closest_on_triangle(p: P3, a: P3, b: P3, c: P3) -> P3 {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return add(b, scale(sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)))
}
