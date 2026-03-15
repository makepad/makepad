use crate::bounding_volume::BoundingVolume;
use crate::math::{IVector, Real, Vector};
use crate::query::{NonlinearRigidMotion, QueryDispatcher, ShapeCastHit};
use crate::shape::{Cuboid, Shape, Voxels};

/// Time Of Impact of a voxels shape with any other shape, under a rigid motion (translation + rotation).
pub fn cast_shapes_nonlinear_voxels_shape<D>(
    dispatcher: &D,
    motion1: &NonlinearRigidMotion,
    g1: &Voxels,
    motion2: &NonlinearRigidMotion,
    g2: &dyn Shape,
    start_time: Real,
    end_time: Real,
    stop_at_penetration: bool,
) -> Option<ShapeCastHit>
where
    D: ?Sized + QueryDispatcher,
{
    use num_traits::Bounded;

    // HACK: really supporting nonlinear shape-casting on a rotating voxels shape would
    //       be extremely inefficient without any sort of hierarchical traversal on the voxel.
    //       So for now we assume that the voxels shape only has a translational motion.
    //       We can fix that once we introduce a sparse representation (and its accompanying
    //       acceleration structure) for the voxels shape.
    let mut motion2 = *motion2;
    let mut motion1 = *motion1;
    motion2.linvel -= motion1.linvel;
    motion1.freeze(start_time);
    let pos1 = motion1.position_at_time(start_time);

    // Search for the smallest time of impact.
    //
    // 1. Check all the cells in the AABB at time `start_time`.
    // 2. Check which wall of unchecked voxels is hit first.
    // 3. Check one additional layer of voxels behind that wall.
    // 4. Continue, with a new AABB taken at the position at the time we hit the
    //    wall.
    // 5. Continue until `curr_t` is bigger than `smallest_t`.
    //
    // PERF: This will be fairly efficient if the shape being cast has a size in the same order
    // of magnitude as the voxels, and if it doesn’t have a significant off-center angular
    // motion. Otherwise, this method will be fairly slow, and would require an acceleration
    // structure to improve.

    let mut hit = None;
    let mut smallest_t = end_time;

    // NOTE: we approximate the AABB of the shape using its orientation at the start and
    //       end time. It might miss some cases if the object is rotating fast.
    let start_pos2_1 = pos1.inv_mul(&motion2.position_at_time(start_time));
    let mut end_pos2_1 = pos1.inv_mul(&motion2.position_at_time(end_time));
    end_pos2_1.translation = start_pos2_1.translation;
    let start_aabb2_1 = g2
        .compute_aabb(&start_pos2_1)
        .merged(&g2.compute_aabb(&end_pos2_1));

    let mut check_voxels_in_range = |search_domain: [IVector; 2]| {
        for vox in g1.voxels_in_range(search_domain[0], search_domain[1]) {
            if !vox.state.is_empty() {
                // PERF: could we check the canonical shape instead, and deduplicate accordingly?
                let center = g1.voxel_center(vox.grid_coords);
                let cuboid = Cuboid::new(g1.voxel_size() / 2.0);
                let vox_motion1 = motion1.prepend_translation(center);
                if let Some(new_hit) = dispatcher
                    .cast_shapes_nonlinear(
                        &vox_motion1,
                        &cuboid,
                        &motion2,
                        g2,
                        start_time,
                        end_time,
                        stop_at_penetration,
                    )
                    .ok()
                    .flatten()
                {
                    if new_hit.time_of_impact < smallest_t {
                        smallest_t = new_hit.time_of_impact;
                        hit = Some(new_hit);
                    }
                }
            }
        }
    };

    let mut search_domain = g1.voxel_range_intersecting_local_aabb(&start_aabb2_1);
    check_voxels_in_range(search_domain);

    // Run the propagation.
    let [domain_mins, domain_maxs] = g1.domain();

    loop {
        let search_domain_aabb = g1.voxel_range_aabb(search_domain[0], search_domain[1]);

        // Figure out if we should move the aabb up/down/right/left.
        #[cfg(feature = "dim2")]
        let ii = [0, 1];
        #[cfg(feature = "dim3")]
        let ii = [0, 1, 2];

        let toi = ii.map(|i| {
            if motion2.linvel[i] > 0.0 {
                let t = (search_domain_aabb.maxs[i] - start_aabb2_1.maxs[i]) / motion2.linvel[i];
                if t < 0.0 {
                    (Real::max_value(), true)
                } else {
                    (t, true)
                }
            } else if motion2.linvel[i] < 0.0 {
                let t = (search_domain_aabb.mins[i] - start_aabb2_1.mins[i]) / motion2.linvel[i];
                if t < 0.0 {
                    (Real::max_value(), false)
                } else {
                    (t, false)
                }
            } else {
                (Real::max_value(), false)
            }
        });

        #[cfg(feature = "dim2")]
        if toi[0].0 > end_time && toi[1].0 > end_time {
            break;
        }

        #[cfg(feature = "dim3")]
        if toi[0].0 > end_time && toi[1].0 > end_time && toi[2].0 > end_time {
            break;
        }

        let imin = Vector::from(toi.map(|t| t.0)).min_position();

        if toi[imin].1 {
            search_domain[0][imin] += 1;
            search_domain[1][imin] += 1;

            if search_domain[1][imin] <= domain_maxs[imin] {
                // Check the voxels on the added row.
                let mut prev_row = search_domain[0];
                prev_row[imin] = search_domain[1][imin] - 1;

                let range_to_check = [prev_row, search_domain[1]];
                check_voxels_in_range(range_to_check);
            } else if search_domain[0][imin] >= domain_maxs[imin] {
                // Leaving the shape’s bounds.
                break;
            }
        } else {
            search_domain[0][imin] -= 1;
            search_domain[1][imin] -= 1;

            if search_domain[0][imin] >= domain_mins[imin] {
                // Check the voxels on the added row.
                let mut next_row = search_domain[1];
                next_row[imin] = search_domain[0][imin] + 1;

                let range_to_check = [search_domain[0], next_row];
                check_voxels_in_range(range_to_check);
            } else if search_domain[1][imin] <= domain_mins[imin] {
                // Leaving the shape’s bounds.
                break;
            }
        }
    }

    hit
}

/// Time Of Impact of any shape with a composite shape, under a rigid motion (translation + rotation).
pub fn cast_shapes_nonlinear_shape_voxels<D>(
    dispatcher: &D,
    motion1: &NonlinearRigidMotion,
    g1: &dyn Shape,
    motion2: &NonlinearRigidMotion,
    g2: &Voxels,
    start_time: Real,
    end_time: Real,
    stop_at_penetration: bool,
) -> Option<ShapeCastHit>
where
    D: ?Sized + QueryDispatcher,
{
    cast_shapes_nonlinear_voxels_shape(
        dispatcher,
        motion2,
        g2,
        motion1,
        g1,
        start_time,
        end_time,
        stop_at_penetration,
    )
    .map(|hit| hit.swapped())
}
