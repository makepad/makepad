use makepad_dynamics::builder::compile;
use makepad_dynamics::data::Data;
use makepad_dynamics::forward::step;
use makepad_dynamics::io::make_data;
use makepad_dynamics::model::Model;
use makepad_dynamics::model_spec::*;
use makepad_dynamics::sdf::attach;
use makepad_dynamics::sdf_torus::Torus;
use makepad_dynamics::thread::{bind_thread_pool, TaskPool};
use makepad_dynamics::types::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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

/// A pool of `threads` workers, the caller among them, that lends a fan-out only the workers no other
/// fan-out holds: a fan-out from inside one that holds them all gets none, and its caller runs every task.
struct Saturable {
    threads: usize,
    held: AtomicUsize,
    alone: AtomicUsize,
}

impl TaskPool for Saturable {
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        let want = (self.threads - 1).min(n.saturating_sub(1));
        let mut held = self.held.load(Ordering::SeqCst);
        let got = loop {
            let got = want.min(self.threads - 1 - held);
            match self.held.compare_exchange(held, held + got, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break got,
                Err(now) => held = now,
            }
        };
        if got == 0 {
            self.alone.fetch_add(1, Ordering::Relaxed);
        }
        let next = AtomicUsize::new(0);
        let work = || loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= n {
                break;
            }
            f(i);
        };
        std::thread::scope(|s| {
            for _ in 0..got {
                s.spawn(work);
            }
            work();
        });
        self.held.fetch_sub(got, Ordering::SeqCst);
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
    let mut at_40 = None;
    for s in 0..120 {
        step(&m, &mut serial);
        step(&m, &mut pooled);
        if s == 39 {
            at_40 = Some(serial.clone());
        }
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

    // worlds stepped as the tasks of a fan-out that holds every worker: their own fan-outs find the pool full,
    // run on their callers, and come out the same
    let want = at_40.unwrap();
    let full = Arc::new(Saturable { threads: 4, held: AtomicUsize::new(0), alone: AtomicUsize::new(0) });
    let worlds: Vec<Mutex<Data>> = (0..4).map(|_| {
        let mut d = make_data(&m);
        bind_thread_pool(&mut d, Some(full.clone()));
        Mutex::new(d)
    }).collect();
    full.fan_out(worlds.len(), &|k| {
        let mut d = worlds[k].lock().unwrap();
        for _ in 0..40 {
            step(&m, &mut d);
        }
    });
    for (k, d) in worlds.iter().enumerate() {
        let d = d.lock().unwrap();
        assert!(d.qpos == want.qpos && d.qvel == want.qvel && d.sdf_seeds == want.sdf_seeds, "world {k} in a full pool");
    }
    assert!(full.alone.load(Ordering::Relaxed) >= 4 * 40, "the inner fan-outs ran on their callers alone");
}
