use crate::bounding_volume::Aabb;
use crate::math::{Real, Vector};
use crate::partitioning::{Bvh, BvhBuildStrategy};

fn make_test_aabb(i: usize) -> Aabb {
    Aabb::from_half_extents(Vector::splat(i as Real).into(), Vector::splat(1.0))
}

#[test]
fn test_leaves_iteration() {
    let leaves = [
        make_test_aabb(0), // mins at (0,0,0) - should pass
        make_test_aabb(5), // mins at (5,5,5) - should be filtered out
    ];
    let bvh = Bvh::from_leaves(BvhBuildStrategy::Binned, &leaves);

    // Only allow nodes with mins.x <= 3.0 (should only pass leaf 0)
    let check = |node: &crate::partitioning::BvhNode| -> bool { node.mins.x <= 3.0 };

    let mut found_invalid_leaf = false;
    for leaf_index in bvh.leaves(check) {
        if leaf_index == 1 {
            // This is the leaf that should be filtered out
            found_invalid_leaf = true;
            break;
        }
    }

    if found_invalid_leaf {
        panic!("Leaves iterator returned an invalid leaf");
    }
}

#[test]
fn bvh_build_and_removal() {
    // Check various combination of building pattern and removal pattern.
    // The tree validity is asserted at every step.
    #[derive(Copy, Clone, Debug)]
    enum BuildPattern {
        Ploc,
        Binned,
        Insert,
    }

    #[derive(Copy, Clone, Debug)]
    enum RemovalPattern {
        InOrder,
        RevOrder,
        EvenOdd,
    }

    for build_pattern in [
        BuildPattern::Ploc,
        BuildPattern::Binned,
        BuildPattern::Insert,
    ] {
        for removal_pattern in [
            RemovalPattern::InOrder,
            RemovalPattern::RevOrder,
            RemovalPattern::EvenOdd,
        ] {
            for len in 1..=100 {
                std::println!(
                    "Testing build: {:?}, removal: {:?}, len: {}",
                    build_pattern,
                    removal_pattern,
                    len
                );
                let leaves: std::vec::Vec<_> = (0..len).map(make_test_aabb).collect();

                let mut bvh = match build_pattern {
                    BuildPattern::Binned => Bvh::from_leaves(BvhBuildStrategy::Binned, &leaves),
                    BuildPattern::Ploc => Bvh::from_leaves(BvhBuildStrategy::Ploc, &leaves),
                    BuildPattern::Insert => {
                        let mut bvh = Bvh::new();
                        for i in 0..len {
                            bvh.insert(make_test_aabb(i), i as u32);
                            bvh.assert_well_formed();
                        }
                        bvh
                    }
                };

                for _ in 0..3 {
                    bvh.assert_well_formed();

                    match removal_pattern {
                        RemovalPattern::InOrder => {
                            // Remove in insertion order.
                            for i in 0..len {
                                bvh.remove(i as u32);
                                bvh.assert_well_formed();
                            }
                        }
                        RemovalPattern::RevOrder => {
                            // Remove in reverse insertion order.
                            for i in (0..len).rev() {
                                bvh.remove(i as u32);
                                bvh.assert_well_formed();
                            }
                        }
                        RemovalPattern::EvenOdd => {
                            // Remove even indices first, then odd.
                            for i in (0..len).filter(|i| i % 2 == 0) {
                                bvh.remove(i as u32);
                                bvh.assert_well_formed();
                            }
                            for i in (0..len).filter(|i| i % 2 != 0) {
                                bvh.remove(i as u32);
                                bvh.assert_well_formed();
                            }
                        }
                    }

                    // Re-insert everything.
                    for (i, leaf) in leaves.iter().enumerate() {
                        bvh.insert(*leaf, i as u32);
                    }
                }
            }
        }
    }
}
