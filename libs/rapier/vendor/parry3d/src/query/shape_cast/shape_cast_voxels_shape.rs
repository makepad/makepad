use crate::math::{IVector, Pose, Real, Vector};
use crate::query::{QueryDispatcher, ShapeCastHit, ShapeCastOptions};
use crate::shape::{Cuboid, Shape, Voxels};

/// Time Of Impact of a voxels shape with any other shape, under a translational movement.
pub fn cast_shapes_voxels_shape<D>(
    dispatcher: &D,
    pos12: &Pose,
    vel12: Vector,
    g1: &Voxels,
    g2: &dyn Shape,
    options: ShapeCastOptions,
) -> Option<ShapeCastHit>
where
    D: ?Sized + QueryDispatcher,
{
    use num_traits::Bounded;

    let mut hit = None;
    let mut smallest_t = options.max_time_of_impact;
    let start_aabb2_1 = g2.compute_aabb(pos12);

    let mut check_voxels_in_range = |search_domain: [IVector; 2]| {
        for vox in g1.voxels_in_range(search_domain[0], search_domain[1]) {
            if !vox.state.is_empty() {
                // PERF: could we check the canonical shape instead, and deduplicate accordingly?
                let center = g1.voxel_center(vox.grid_coords);
                let cuboid = Cuboid::new(g1.voxel_size() / 2.0);
                let vox_pos12 = Pose::from_translation(center).inverse() * pos12;
                if let Some(new_hit) = dispatcher
                    .cast_shapes(&vox_pos12, vel12, &cuboid, g2, options)
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
            if vel12[i] > 0.0 {
                let t = (search_domain_aabb.maxs[i] - start_aabb2_1.maxs[i]) / vel12[i];
                if t < 0.0 {
                    (Real::max_value(), true)
                } else {
                    (t, true)
                }
            } else if vel12[i] < 0.0 {
                let t = (search_domain_aabb.mins[i] - start_aabb2_1.mins[i]) / vel12[i];
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
        if toi[0].0 > options.max_time_of_impact && toi[1].0 > options.max_time_of_impact {
            break;
        }

        #[cfg(feature = "dim3")]
        if toi[0].0 > options.max_time_of_impact
            && toi[1].0 > options.max_time_of_impact
            && toi[2].0 > options.max_time_of_impact
        {
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
pub fn cast_shapes_shape_voxels<D>(
    dispatcher: &D,
    pos12: &Pose,
    vel12: Vector,
    g1: &dyn Shape,
    g2: &Voxels,
    options: ShapeCastOptions,
) -> Option<ShapeCastHit>
where
    D: ?Sized + QueryDispatcher,
{
    cast_shapes_voxels_shape(
        dispatcher,
        &pos12.inverse(),
        -(pos12.rotation.inverse() * vel12),
        g2,
        g1,
        options,
    )
    .map(|time_of_impact| time_of_impact.swapped())
}
