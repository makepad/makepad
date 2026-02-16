use crate::broad_phase;
use crate::contact::ContactManifold;
use crate::hash;
use crate::narrow_phase;
use crate::rigid_body::{BodyType, RigidBody};
use crate::solver::{self, SolverContact};
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

pub struct PhysicsWorld {
    pub bodies: Vec<RigidBody>,
    pub gravity: Vec3f,
    pub dt: f32,
    pub ground_y: f32,
    pub frame: u64,

    // Reusable buffers — never deallocated, only cleared each frame
    aabbs: Vec<Aabb>,
    pairs: Vec<(usize, usize)>,
    manifolds: Vec<ContactManifold>,
    solver_contacts: Vec<SolverContact>,
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
            manifolds: Vec::new(),
            solver_contacts: Vec::new(),
        }
    }

    /// Apply operations then advance one physics frame.
    /// Uses rapier's TGS (Temporal Gauss-Seidel) substepping:
    ///   Collision detection runs ONCE per frame (before substep loop).
    ///   dt is split into NUM_SOLVER_ITERATIONS substeps.
    ///   Each substep: apply gravity → update constraints (from current poses)
    ///   → warmstart → PGS solve → integrate positions → stabilization.
    pub fn step(&mut self, ops: &[PhysicsOp]) {
        self.apply_ops(ops);

        let substep_dt = self.dt / NUM_SOLVER_ITERATIONS as f32;

        // Pre-compute per-substep gravity increment (rapier: force * inv_mass * substep_dt).
        // Since gravity is already acceleration, increment = gravity * substep_dt.
        let gravity_increment = self.gravity * substep_dt;

        // Collision detection runs ONCE before the substep loop (matching rapier).
        // Contact points are stored in body-local coordinates so they can be
        // re-transformed using updated poses each substep.
        broad_phase::broad_phase(&self.bodies, &mut self.aabbs, &mut self.pairs);
        narrow_phase::narrow_phase(
            &self.bodies,
            &self.pairs,
            self.ground_y,
            &mut self.manifolds,
        );

        // Build initial solver constraints from contact manifolds
        solver::prepare_contacts(
            &self.bodies,
            &self.manifolds,
            substep_dt,
            &mut self.solver_contacts,
        );

        for substep in 0..NUM_SOLVER_ITERATIONS {
            // 1. Apply gravity increment for this substep
            for body in self.bodies.iter_mut() {
                if body.body_type == BodyType::Dynamic {
                    body.linear_velocity += gravity_increment;
                }
            }

            // 2. Update constraints from current poses (NOT re-running collision detection).
            // On the first substep, constraints are already fresh from prepare_contacts.
            // On subsequent substeps, re-transform contact points and recompute biases
            // from updated body positions (matching rapier's constraint update).
            if substep > 0 {
                solver::update_contacts(&self.bodies, &mut self.solver_contacts, substep_dt);
            }

            // 3. Warmstart: apply cached impulses (every substep, matching rapier)
            if WARMSTART_COEFFICIENT != 0.0 {
                solver::warmstart(
                    &mut self.bodies,
                    &mut self.solver_contacts,
                    WARMSTART_COEFFICIENT,
                );
            }

            // 4. PGS constraint solve (with bias for penetration correction)
            solver::solve_contacts(
                &mut self.bodies,
                &mut self.solver_contacts,
                NUM_INTERNAL_PGS_ITERATIONS,
            );

            // 5. Integrate positions with substep dt
            for body in self.bodies.iter_mut() {
                if body.body_type == BodyType::Dynamic {
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
                NUM_INTERNAL_STABILIZATION_ITERATIONS,
            );
        }

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
    }

    /// Fast-forward: restore snapshot, then replay ops for each frame.
    pub fn resync(&mut self, snap: &Snapshot, op_log: &[(u64, &[PhysicsOp])]) {
        self.restore(snap);
        for &(expected_frame, ops) in op_log {
            debug_assert_eq!(expected_frame, self.frame);
            self.step(ops);
        }
    }

    fn apply_ops(&mut self, ops: &[PhysicsOp]) {
        for op in ops {
            match op {
                PhysicsOp::SpawnDynamic {
                    position,
                    half_extents,
                    velocity,
                    density,
                } => {
                    let mut body = RigidBody::new_dynamic(*position, *half_extents, *density);
                    body.linear_velocity = *velocity;
                    self.bodies.push(body);
                }
                PhysicsOp::SpawnFixed {
                    position,
                    half_extents,
                } => {
                    self.bodies
                        .push(RigidBody::new_fixed(*position, *half_extents));
                }
                PhysicsOp::ApplyForce { body, force } => {
                    if *body < self.bodies.len() && self.bodies[*body].is_dynamic() {
                        self.bodies[*body].linear_velocity += *force * self.dt;
                    }
                }
                PhysicsOp::ApplyImpulse { body, impulse } => {
                    if *body < self.bodies.len() && self.bodies[*body].is_dynamic() {
                        let inv_mass = self.bodies[*body].inv_mass;
                        self.bodies[*body].linear_velocity += *impulse * inv_mass;
                    }
                }
                PhysicsOp::RemoveBody { body } => {
                    if *body < self.bodies.len() {
                        self.bodies.swap_remove(*body);
                    }
                }
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
