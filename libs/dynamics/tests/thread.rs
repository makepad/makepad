use makepad_dynamics::builder::compile;
use makepad_dynamics::forward::step;
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::Model;
use makepad_dynamics::model_spec::*;
use makepad_dynamics::sdf::attach;
use makepad_dynamics::sdf_torus::Torus;
use makepad_dynamics::thread::{bind_thread_pool, TaskPool};
use makepad_dynamics::types::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A pool of `threads` threads, each taking every `threads`-th task, last first: the tasks run at once
/// and out of order.
struct Striped {
    threads: usize,
    fan_outs: AtomicUsize,
}

impl TaskPool for Striped {
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        self.fan_outs.fetch_add(1, Ordering::Relaxed);
        std::thread::scope(|s| {
            for k in 0..self.threads {
                s.spawn(move || (k..n).step_by(self.threads).rev().for_each(f));
            }
        });
    }
    fn threads(&self) -> usize {
        self.threads
    }
}

/// A pile of every primitive kind dropped on a plane, and a field torus resting on a fixed one.
fn pile() -> Model {
    let mut s = ModelSpec::default();
    s.option.timestep = 0.005;
    s.bodies[0].geoms.push(GeomSpec { r#type: GEOM_PLANE, size: [0.0, 0.0, 1.0], ..GeomSpec::default() });
    let kinds = [
        GeomSpec { r#type: GEOM_SPHERE, size: [0.15, 0.0, 0.0], ..GeomSpec::default() },
        GeomSpec { r#type: GEOM_CAPSULE, size: [0.1, 0.15, 0.0], ..GeomSpec::default() },
        GeomSpec { r#type: GEOM_ELLIPSOID, size: [0.2, 0.15, 0.1], ..GeomSpec::default() },
        GeomSpec { r#type: GEOM_CYLINDER, size: [0.12, 0.15, 0.0], ..GeomSpec::default() },
        GeomSpec { r#type: GEOM_BOX, size: [0.15, 0.12, 0.1], ..GeomSpec::default() },
    ];
    for i in 0..20 {
        let (x, y, z) = ((i % 5) as f64 * 0.32 - 0.64, (i / 5 % 2) as f64 * 0.32, 0.2 + (i / 10) as f64 * 0.3);
        let q = [1.0, 0.1 * i as f64, 0.05 * i as f64, 0.2];
        let n = q.iter().map(|q| q * q).sum::<f64>().sqrt();
        s.bodies.push(BodySpec {
            pos: [x, y, z], quat: q.map(|q| q / n), mass: 1.0, ipos: [0.0; 3], inertia: [0.01; 3],
            joints: vec![JointSpec { r#type: JNT_FREE, ..JointSpec::default() }],
            geoms: vec![kinds[i % 5].clone()],
            ..BodySpec::default()
        });
    }
    let torus = [0.4, 0.12];
    // the tori collide with each other only
    let field = GeomSpec { r#type: GEOM_BOX, size: [0.52, 0.52, 0.12], contype: 2, conaffinity: 2, ..GeomSpec::default() };
    s.bodies[0].geoms.push(GeomSpec { pos: [0.0, 2.0, 0.5], ..field.clone() });
    s.bodies.push(BodySpec {
        pos: [0.1, 2.0, 0.73], mass: 1.0, ipos: [0.0; 3], inertia: [0.05; 3],
        joints: vec![JointSpec { r#type: JNT_FREE, ..JointSpec::default() }],
        geoms: vec![field],
        ..BodySpec::default()
    });
    let mut m = compile(&s).expect("compile the pile");
    for g in [1, m.ngeom - 1] {
        attach(&mut m, g, Arc::new(Torus { radius: torus }));
    }
    m
}

#[test]
fn a_bound_pool_spreads_the_narrowphase_with_the_same_results() {
    let m = pile();
    let mut serial = make_data(&m);
    let mut pooled = make_data(&m);
    let pool = Arc::new(Striped { threads: 4, fan_outs: AtomicUsize::new(0) });
    bind_thread_pool(&mut pooled, Some(pool.clone()));
    let (mut contacts, mut seeds) = (0, 0);
    for s in 0..120 {
        step(&m, &mut serial);
        step(&m, &mut pooled);
        assert_eq!(serial.ncon, pooled.ncon, "contacts at step {s}");
        assert_eq!(&serial.contact[..serial.ncon as usize], &pooled.contact[..pooled.ncon as usize], "contacts at step {s}");
        assert_eq!(serial.sdf_seeds, pooled.sdf_seeds, "the field pairs' seeds at step {s}");
        assert!(serial.qpos == pooled.qpos && serial.qvel == pooled.qvel, "state at step {s}");
        contacts = contacts.max(serial.ncon);
        seeds = seeds.max(serial.sdf_seeds.len());
    }
    assert!(contacts > 20 && seeds > 0, "the pile touches ({contacts} contacts) and the tori keep seeds ({seeds})");
    assert!(pool.fan_outs.load(Ordering::Relaxed) >= 120, "a narrowphase on the pool every step");
    assert!(pooled.collision_tasks.len() > 4, "the pairs in chunks of a fifth of a thread's share: {}", pooled.collision_tasks.len());
}
