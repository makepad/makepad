use crate::geometry::ContactManifold;
use crate::math::Real;
use crate::utils::OrthonormalBasis;

pub(crate) fn reduce_manifold_naive(
    manifold: &ContactManifold,
    selected: &mut [usize; 4],
    num_selected: &mut usize,
    prediction_distance: Real,
) {
    if manifold.points.len() <= 4 {
        return;
    }

    // 1. Find the deepest contact.
    *selected = [usize::MAX; 4];

    let mut deepest_dist = Real::MAX;
    for (i, pt) in manifold.points.iter().enumerate() {
        if pt.dist < deepest_dist {
            deepest_dist = pt.dist;
            selected[0] = i;
        }
    }

    if selected[0] == usize::MAX {
        *num_selected = 0;
        return;
    }

    // 2. Find the point that is the furthest from the deepest point.
    let selected_a = manifold.points[selected[0]].local_p1;
    let mut furthest_dist = -Real::MAX;
    for (i, pt) in manifold.points.iter().enumerate() {
        let dist = (pt.local_p1 - selected_a).length_squared();
        if i != selected[0] && pt.dist <= prediction_distance && dist > furthest_dist {
            furthest_dist = dist;
            selected[1] = i;
        }
    }

    if selected[1] == usize::MAX {
        *num_selected = 1;
        return;
    }

    // 3. Now find the two points furthest from the segment we built so far.
    let selected_b = manifold.points[selected[1]].local_p1;

    if selected_a == selected_b {
        *num_selected = 1;
        return;
    }

    let selected_ab = selected_b - selected_a;
    let tangent = selected_ab.cross(manifold.local_n1);

    // Find the points that minimize and maximize the dot product with the tangent.
    let mut min_dot = Real::MAX;
    let mut max_dot = -Real::MAX;
    for (i, pt) in manifold.points.iter().enumerate() {
        if i == selected[0] || i == selected[1] || pt.dist > prediction_distance {
            continue;
        }

        let dot = (pt.local_p1 - selected_a).dot(tangent);
        if dot < min_dot {
            min_dot = dot;
            selected[2] = i;
        }

        if dot > max_dot {
            max_dot = dot;
            selected[3] = i;
        }
    }

    if selected[2] == usize::MAX {
        *num_selected = 2;
    } else if selected[2] == selected[3] {
        *num_selected = 3;
    } else {
        *num_selected = 4;
    }
}

// Run contact reduction using Bepu's InternalReduce algorithm.
// The general idea is quite similar to our naive approach except that they add some
// additional heuristics. This is implemented mainly for comparison purpose to see
// if there is a strong advantage to having the extra checks.
#[allow(dead_code)]
pub(crate) fn reduce_manifold_bepu_like(
    manifold: &ContactManifold,
    selected: &mut [usize; 4],
    num_selected: &mut usize,
) {
    if manifold.points.len() <= 4 {
        return;
    }

    // Step 1: Find the deepest contact, biased by extremity for frame stability.
    // The extremity heuristic helps maintain consistent contact selection across frames
    // when multiple contacts have similar depths.
    let mut best_score = -Real::MAX;
    const EXTREMITY_SCALE: Real = 1e-2;
    // Use an arbitrary direction (roughly 38 degrees from X axis) to break ties
    const EXTREMITY_DIR_X: Real = 0.7946898;
    const EXTREMITY_DIR_Y: Real = 0.6070158;

    let tangents = manifold.local_n1.orthonormal_basis();

    for (i, pt) in manifold.points.iter().enumerate() {
        // Extremity measures how far the contact is from the origin in the tangent plane
        let tx1 = pt.local_p1.dot(tangents[0]);
        let ty1 = pt.local_p1.dot(tangents[1]);

        let extremity = (tx1 * EXTREMITY_DIR_X + ty1 * EXTREMITY_DIR_Y).abs();

        // Score = depth + small extremity bias (only for non-speculative contacts)
        // Negative dist = deeper penetration = higher score
        let score = if pt.dist >= 0.0 {
            -pt.dist // Speculative contact, no extremity bias
        } else {
            -pt.dist + extremity * EXTREMITY_SCALE
        };

        if score > best_score {
            best_score = score;
            selected[0] = i;
        }
    }

    // Step 2: Find the point most distant from the first contact.
    // This establishes a baseline "edge" for the manifold.
    let contact0_pos = manifold.points[selected[0]].local_p1;
    let mut max_distance_squared = 0.0;

    for (i, pt) in manifold.points.iter().enumerate() {
        let offset = pt.local_p1 - contact0_pos;
        let offset_x = offset.dot(tangents[0]);
        let offset_y = offset.dot(tangents[1]);
        let distance_squared = offset_x * offset_x + offset_y * offset_y;

        if distance_squared > max_distance_squared {
            max_distance_squared = distance_squared;
            selected[1] = i;
        }
    }

    // Early out if the contacts are too close together
    let epsilon = 1e-6;
    if max_distance_squared <= epsilon {
        // Only one meaningful contact
        *num_selected = 1;
    } else {
        // Step 3: Find two more contacts that maximize positive and negative signed area.
        // Using the first two contacts as an edge, we look for contacts that form triangles
        // with the largest magnitude negative and positive areas. This maximizes the
        // spatial extent of the contact manifold.

        *num_selected = 2;
        selected[2] = usize::MAX;
        selected[3] = usize::MAX;

        let contact1_pos = manifold.points[selected[1]].local_p1;
        let edge_offset = contact1_pos - contact0_pos;
        let edge_offset_x = edge_offset.dot(tangents[0]);
        let edge_offset_y = edge_offset.dot(tangents[1]);

        let mut min_signed_area = 0.0;
        let mut max_signed_area = 0.0;

        for (i, pt) in manifold.points.iter().enumerate() {
            let candidate_offset = pt.local_p1 - contact0_pos;
            let candidate_offset_x = candidate_offset.dot(tangents[0]);
            let candidate_offset_y = candidate_offset.dot(tangents[1]);

            // Signed area of the triangle formed by (contact0, contact1, candidate)
            // This is a 2D cross product: (candidate - contact0) × (contact1 - contact0)
            let mut signed_area =
                candidate_offset_x * edge_offset_y - candidate_offset_y * edge_offset_x;

            // Penalize speculative contacts (they're less important)
            if pt.dist >= 0.0 {
                signed_area *= 0.25;
            }

            if signed_area < min_signed_area {
                min_signed_area = signed_area;
                selected[2] = i;
            }
            if signed_area > max_signed_area {
                max_signed_area = signed_area;
                selected[3] = i;
            }
        }

        // Check if the signed areas are significant enough
        // Epsilon based on the edge length squared
        let area_epsilon = max_distance_squared * max_distance_squared * 1e-6;

        // If the areas are too small, don't add those contacts
        if min_signed_area * min_signed_area <= area_epsilon {
            selected[2] = usize::MAX;
        }
        if max_signed_area * max_signed_area <= area_epsilon {
            selected[3] = usize::MAX;
        }

        let keep2 = selected[2] != usize::MAX;
        let keep3 = selected[3] != usize::MAX;
        *num_selected += keep2 as usize + keep3 as usize;

        if !keep2 {
            selected[2] = selected[3];
        }
    }
}
