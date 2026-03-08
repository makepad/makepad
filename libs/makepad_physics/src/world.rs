use crate::broad_phase;
use crate::contact::ContactManifold;
use crate::hash;
use crate::narrow_phase;
use crate::rigid_body::{BodyType, RigidBody};
use crate::solver::{self, SolverContact, SolverFriction};
use makepad_math::*;

/// The only way to mutate physics state from outside.
/// The primary resolves ordering, clients apply in the exact same order.
#[derive(Clone, Debug)]
pub enum PhysicsOp {
    SpawnDynamic {
        position: Vec3f,
        half_extents: Vec3f,
        velocity: Vec3f,
        density: f32,
    },
    SpawnFixed {
        position: Vec3f,
        half_extents: Vec3f,
    },
    ApplyForce {
        body: usize,
        force: Vec3f,
    },
    ApplyImpulse {
        body: usize,
        impulse: Vec3f,
    },
    RemoveBody {
        body: usize,
    },
}

/// Rapier-matching TGS solver parameters.
const NUM_SOLVER_ITERATIONS: usize = 4;
const NUM_INTERNAL_PGS_ITERATIONS: usize = 1;
const NUM_INTERNAL_STABILIZATION_ITERATIONS: usize = 1;
const WARMSTART_COEFFICIENT: f32 = 1.0;

// Rapier-like sleep defaults.
const SLEEP_LINEAR_THRESHOLD: f32 = 0.4;
const SLEEP_ANGULAR_THRESHOLD: f32 = 0.5;
const TIME_UNTIL_SLEEP: f32 = 2.0;
const WAKE_LINEAR_THRESHOLD: f32 = 0.5;
const WAKE_ANGULAR_THRESHOLD: f32 = 0.625;

pub struct PhysicsWorld {
    pub bodies: Vec<RigidBody>,
    pub gravity: Vec3f,
    pub dt: f32,
    pub ground_y: f32,
    pub frame: u64,

    // Reusable buffers — never deallocated, only cleared each frame
    aabbs: Vec<Aabb>,
    pairs: Vec<(usize, usize)>,
    prev_manifolds: Vec<ContactManifold>,
    manifolds: Vec<ContactManifold>,
    solver_contacts: Vec<SolverContact>,
    solver_frictions: Vec<SolverFriction>,
}

impl PhysicsWorld {
    pub fn new(gravity: Vec3f, dt: f32) -> Self {
        PhysicsWorld {
            bodies: Vec::new(),
            gravity,
            dt,
            ground_y: 0.0,
            frame: 0,
            aabbs: Vec::new(),
            pairs: Vec::new(),
            prev_manifolds: Vec::new(),
            manifolds: Vec::new(),
            solver_contacts: Vec::new(),
            solver_frictions: Vec::new(),
        }
    }

    /// Apply operations then advance one physics frame.
    /// Uses rapier's TGS (Temporal Gauss-Seidel) substepping:
    ///   Collision detection runs ONCE per frame (before substep loop).
    ///   dt is split into NUM_SOLVER_ITERATIONS substeps.
    ///   Each substep: apply gravity → update constraints (from current poses)
    ///   → warmstart → PGS solve → integrate positions → stabilization.
    pub fn step(&mut self, ops: &[PhysicsOp]) {
        let topology_changed = self.apply_ops(ops);

        // Keep sleeping bodies fully frozen unless contacts/impulses wake them.
        for body in self.bodies.iter_mut() {
            if body.body_type == BodyType::Dynamic && body.sleeping {
                body.linear_velocity = Vec3f::default();
                body.angular_velocity = Vec3f::default();
            }
        }

        let substep_dt = self.dt / NUM_SOLVER_ITERATIONS as f32;

        // Pre-compute per-substep gravity increment (rapier: force * inv_mass * substep_dt).
        // Since gravity is already acceleration, increment = gravity * substep_dt.
        let gravity_increment = self.gravity * substep_dt;

        // Collision detection runs ONCE before the substep loop (matching rapier).
        // Contact points are stored in body-local coordinates so they can be
        // re-transformed using updated poses each substep.
        if topology_changed {
            self.prev_manifolds.clear();
            self.manifolds.clear();
        } else {
            std::mem::swap(&mut self.prev_manifolds, &mut self.manifolds);
            self.manifolds.clear();
        }

        broad_phase::broad_phase(&self.bodies, &mut self.aabbs, &mut self.pairs);
        narrow_phase::narrow_phase(
            &self.bodies,
            &self.pairs,
            self.ground_y,
            &self.prev_manifolds,
            &mut self.manifolds,
        );
        solver::inherit_warmstart_impulses(&self.prev_manifolds, &mut self.manifolds);

        // Build initial solver constraints from contact manifolds
        solver::prepare_contacts(
            &self.bodies,
            &self.manifolds,
            substep_dt,
            &mut self.solver_contacts,
            &mut self.solver_frictions,
        );

        for _substep in 0..NUM_SOLVER_ITERATIONS {
            // 1. Apply gravity increment for this substep
            for body in self.bodies.iter_mut() {
                if body.body_type == BodyType::Dynamic && !body.sleeping {
                    body.linear_velocity += gravity_increment;
                }
            }

            // 2. Update constraints from current poses (NOT re-running collision detection).
            // This runs on every substep in Rapier because it also advances the
            // warmstart accumulators used for writeback.
            solver::update_contacts(
                &self.bodies,
                &mut self.solver_contacts,
                substep_dt,
                WARMSTART_COEFFICIENT,
            );
            solver::update_frictions(
                &self.bodies,
                &mut self.solver_frictions,
                substep_dt,
                WARMSTART_COEFFICIENT,
            );

            // 3. Warmstart: apply cached impulses (every substep, matching rapier)
            if WARMSTART_COEFFICIENT != 0.0 {
                solver::warmstart(
                    &mut self.bodies,
                    &mut self.solver_contacts,
                    &mut self.solver_frictions,
                    WARMSTART_COEFFICIENT,
                );
            }

            // 4. PGS constraint solve (with bias for penetration correction)
            solver::solve_contacts(
                &mut self.bodies,
                &mut self.solver_contacts,
                &mut self.solver_frictions,
                NUM_INTERNAL_PGS_ITERATIONS,
            );

            // 5. Integrate positions with substep dt
            for body in self.bodies.iter_mut() {
                if body.body_type == BodyType::Dynamic && !body.sleeping {
                    body.pose.position += body.linear_velocity * substep_dt;
                    body.pose.orientation = body
                        .pose
                        .orientation
                        .integrate(body.angular_velocity, substep_dt);
                }
            }

            // 6. Stabilization solve (without bias) — corrects residual penetration
            solver::solve_contacts_wo_bias(
                &mut self.bodies,
                &mut self.solver_contacts,
                &mut self.solver_frictions,
                NUM_INTERNAL_STABILIZATION_ITERATIONS,
            );
        }

        solver::writeback_impulses(
            &self.solver_contacts,
            &self.solver_frictions,
            &mut self.manifolds,
        );

        self.update_sleep_states();
        self.frame += 1;
    }

    /// Hash the current state for determinism verification.
    pub fn hash_state(&self) -> u64 {
        hash::hash_bodies(&self.bodies)
    }

    /// Serialize full state for resync / late-join / rollback.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            frame: self.frame,
            bodies: self.bodies.clone(),
        }
    }

    /// Restore from a snapshot.
    pub fn restore(&mut self, snap: &Snapshot) {
        self.frame = snap.frame;
        self.bodies.clear();
        self.bodies.extend_from_slice(&snap.bodies);
        self.aabbs.clear();
        self.pairs.clear();
        self.prev_manifolds.clear();
        self.manifolds.clear();
        self.solver_contacts.clear();
        self.solver_frictions.clear();
    }

    /// Fast-forward: restore snapshot, then replay ops for each frame.
    pub fn resync(&mut self, snap: &Snapshot, op_log: &[(u64, &[PhysicsOp])]) {
        self.restore(snap);
        for &(expected_frame, ops) in op_log {
            debug_assert_eq!(expected_frame, self.frame);
            self.step(ops);
        }
    }

    fn apply_ops(&mut self, ops: &[PhysicsOp]) -> bool {
        let mut topology_changed = false;
        for op in ops {
            match op {
                PhysicsOp::SpawnDynamic {
                    position,
                    half_extents,
                    velocity,
                    density,
                } => {
                    topology_changed = true;
                    let mut body = RigidBody::new_dynamic(*position, *half_extents, *density);
                    body.linear_velocity = *velocity;
                    body.wake_up();
                    self.bodies.push(body);
                }
                PhysicsOp::SpawnFixed {
                    position,
                    half_extents,
                } => {
                    topology_changed = true;
                    self.bodies
                        .push(RigidBody::new_fixed(*position, *half_extents));
                }
                PhysicsOp::ApplyForce { body, force } => {
                    if *body < self.bodies.len() && self.bodies[*body].is_dynamic() {
                        self.bodies[*body].linear_velocity += *force * self.dt;
                        self.bodies[*body].wake_up();
                    }
                }
                PhysicsOp::ApplyImpulse { body, impulse } => {
                    if *body < self.bodies.len() && self.bodies[*body].is_dynamic() {
                        let inv_mass = self.bodies[*body].inv_mass;
                        self.bodies[*body].linear_velocity += *impulse * inv_mass;
                        self.bodies[*body].wake_up();
                    }
                }
                PhysicsOp::RemoveBody { body } => {
                    if *body < self.bodies.len() {
                        topology_changed = true;
                        self.bodies.swap_remove(*body);
                    }
                }
            }
        }
        topology_changed
    }

    fn update_sleep_states(&mut self) {
        for body in self.bodies.iter_mut() {
            if body.body_type != BodyType::Dynamic {
                continue;
            }

            let lin_speed = body.linear_velocity.length();
            let ang_speed = body.angular_velocity.length();

            if body.sleeping {
                if lin_speed > WAKE_LINEAR_THRESHOLD || ang_speed > WAKE_ANGULAR_THRESHOLD {
                    body.wake_up();
                } else {
                    body.linear_velocity = Vec3f::default();
                    body.angular_velocity = Vec3f::default();
                }
                continue;
            }

            if lin_speed <= SLEEP_LINEAR_THRESHOLD && ang_speed <= SLEEP_ANGULAR_THRESHOLD {
                body.sleep_time += self.dt;
                if body.sleep_time >= TIME_UNTIL_SLEEP {
                    body.sleeping = true;
                    body.linear_velocity = Vec3f::default();
                    body.angular_velocity = Vec3f::default();
                }
            } else {
                body.sleep_time = 0.0;
            }
        }
    }
}

/// Serializable snapshot of the physics world state.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub frame: u64,
    pub bodies: Vec<RigidBody>,
}
