// Rigid-body physics workload: libs/box3d stepping a fixed scene (box3d_scene.rs: a hull-box pyramid,
// a revolute plank bridge with a motor and limits, a spherical-joint rope, a distance-joint pendulum and a
// rain of spheres, capsules and rocks). Each iteration builds the world fresh and runs 90 steps of 1/60 s
// with 4 substeps; the checksum folds every body's transform and velocities (bit patterns) and the
// contact count after the last step.
use crate::box3d_scene;

pub fn setup() -> Box<dyn FnMut() -> u64> {
    Box::new(move || {
        let mut scene = box3d_scene::build(3);
        let mut h = 0u64;
        for s in 0..90 {
            box3d_scene::step(&mut scene);
            if s % 30 == 29 {
                h = crate::fold(h, box3d_scene::state_hash(&scene));
            }
        }
        h
    })
}
