use super::{
    xr_depth::{
        clear_depth_query_state_for_scene, sync_depth_query_surfaces_with_store,
        RetainedDepthQueryHit,
    },
    xr_hands::sync_hands_on_scene,
    xr_physics::{makepad_pose, RapierScene},
    CollectedXrCube,
};
use crate::{
    prelude::*,
    scene::{XrBodyKind, XrRuntimeBodyState},
};
use makepad_widgets::makepad_platform::{event::XrHand, XrTsdfStore};
use std::{
    collections::HashMap,
    mem,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Instant,
};

const XR_WORKER_SIMULATION_DT_DEFAULT: f32 = 1.0 / 120.0;
const XR_WORKER_SIMULATION_DT_MIN: f32 = 1.0 / 480.0;
const XR_WORKER_SIMULATION_DT_MAX: f32 = 1.0 / 45.0;
const XR_WORKER_SIMULATION_DT_SMOOTHING: f32 = 0.35;
const XR_WORKER_SIMULATION_DT_LADDER: [f32; 5] =
    [1.0 / 120.0, 1.0 / 90.0, 1.0 / 72.0, 1.0 / 60.0, 1.0 / 45.0];
const XR_PHYSICS_WORKER_MAX_PENDING_BODY_SPAWNS: usize = 8;

#[derive(Clone)]
struct PhysicsWorkerRebuild {
    revision: u64,
    gravity: f32,
    cubes: Vec<CollectedXrCube>,
}

#[derive(Clone)]
struct PhysicsWorkerStep {
    revision: u64,
    left_hand: XrHand,
    right_hand: XrHand,
    time_scale: f32,
    include_retained_hits: bool,
}

#[derive(Clone, Copy)]
struct PhysicsWorkerBodySpawn {
    revision: u64,
    spawn: XrBodySpawn,
}

#[derive(Default)]
struct PhysicsWorkerMailbox {
    version: u64,
    shutdown: bool,
    pending_reset_revision: Option<u64>,
    pending_rebuild: Option<PhysicsWorkerRebuild>,
    pending_step: Option<PhysicsWorkerStep>,
    pending_body_spawns:
        SmallVec<[PhysicsWorkerBodySpawn; XR_PHYSICS_WORKER_MAX_PENDING_BODY_SPAWNS]>,
}

pub(super) struct XrPhysicsWorkerResult {
    pub(super) revision: u64,
    pub(super) runtime_bodies: HashMap<WidgetUid, XrRuntimeBodyState>,
    pub(super) depth_query_retained_hits: Option<HashMap<u64, RetainedDepthQueryHit>>,
    pub(super) physics_compute_ms: f64,
    pub(super) physics_tsdf_query_ms: f64,
    pub(super) physics_rapier_step_ms: f64,
    pub(super) physics_depth_query_surface_count: usize,
}

pub(super) struct XrPhysicsWorker {
    mailbox: Arc<(Mutex<PhysicsWorkerMailbox>, Condvar)>,
    latest_result: Arc<Mutex<Option<XrPhysicsWorkerResult>>>,
    join_handle: Option<JoinHandle<()>>,
}

impl XrPhysicsWorker {
    pub(super) fn new(depth_mesh: XrTsdfStore) -> Self {
        let mailbox = Arc::new((Mutex::new(PhysicsWorkerMailbox::default()), Condvar::new()));
        let latest_result = Arc::new(Mutex::new(None));
        let mailbox_thread = mailbox.clone();
        let result_thread = latest_result.clone();
        let join_handle = thread::Builder::new()
            .name("makepad-xr-physics".to_string())
            .spawn(move || physics_worker_loop(depth_mesh, mailbox_thread, result_thread))
            .ok();

        Self {
            mailbox,
            latest_result,
            join_handle,
        }
    }

    pub(super) fn request_rebuild(
        &mut self,
        revision: u64,
        gravity: f32,
        cubes: Vec<CollectedXrCube>,
    ) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.pending_rebuild = Some(PhysicsWorkerRebuild {
                revision,
                gravity,
                cubes,
            });
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
    }

    pub(super) fn request_step(
        &mut self,
        revision: u64,
        left_hand: XrHand,
        right_hand: XrHand,
        time_scale: f32,
        include_retained_hits: bool,
    ) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.pending_step = Some(PhysicsWorkerStep {
                revision,
                left_hand,
                right_hand,
                time_scale,
                include_retained_hits,
            });
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
    }

    pub(super) fn request_body_spawn(&mut self, revision: u64, spawn: XrBodySpawn) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            if mailbox.pending_body_spawns.len() < XR_PHYSICS_WORKER_MAX_PENDING_BODY_SPAWNS {
                mailbox
                    .pending_body_spawns
                    .push(PhysicsWorkerBodySpawn { revision, spawn });
                mailbox.version = mailbox.version.saturating_add(1);
                wake.notify_one();
            }
        }
    }

    pub(super) fn request_reset(&mut self, revision: u64) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.pending_reset_revision = Some(revision);
            mailbox.pending_rebuild = None;
            mailbox.pending_step = None;
            mailbox.pending_body_spawns.clear();
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
    }

    pub(super) fn take_latest_result(&mut self) -> Option<XrPhysicsWorkerResult> {
        self.latest_result
            .lock()
            .ok()
            .and_then(|mut result| result.take())
    }
}

impl Drop for XrPhysicsWorker {
    fn drop(&mut self) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.shutdown = true;
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

fn physics_worker_loop(
    depth_mesh: XrTsdfStore,
    mailbox: Arc<(Mutex<PhysicsWorkerMailbox>, Condvar)>,
    latest_result: Arc<Mutex<Option<XrPhysicsWorkerResult>>>,
) {
    let mut seen_version = 0u64;
    let mut revision = 0u64;
    let mut scene: Option<RapierScene> = None;
    let mut retained_hits = HashMap::new();
    let mut runtime_bodies_scratch = HashMap::new();
    let mut retained_hits_snapshot_scratch = HashMap::new();
    let mut last_step_started_at: Option<Instant> = None;
    let mut adaptive_step_dt = XR_WORKER_SIMULATION_DT_DEFAULT;

    loop {
        let (pending_reset_revision, pending_rebuild, pending_step, pending_body_spawns) = {
            let (lock, wake) = &*mailbox;
            let mut guard = match lock.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            while !guard.shutdown && guard.version == seen_version {
                guard = match wake.wait(guard) {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
            }
            if guard.shutdown {
                clear_depth_query_state_for_scene(scene.as_ref(), &mut retained_hits);
                return;
            }
            seen_version = guard.version;
            (
                guard.pending_reset_revision.take(),
                guard.pending_rebuild.take(),
                guard.pending_step.take(),
                std::mem::take(&mut guard.pending_body_spawns),
            )
        };

        let mut should_publish = false;

        if let Some(reset_revision) = pending_reset_revision {
            revision = reset_revision;
            clear_depth_query_state_for_scene(scene.as_ref(), &mut retained_hits);
            scene = None;
            last_step_started_at = None;
            adaptive_step_dt = XR_WORKER_SIMULATION_DT_DEFAULT;
            should_publish = true;
        }

        if let Some(rebuild) = pending_rebuild {
            revision = rebuild.revision;
            clear_depth_query_state_for_scene(scene.as_ref(), &mut retained_hits);
            scene = Some(build_scene(rebuild.gravity, rebuild.cubes));
            last_step_started_at = None;
            adaptive_step_dt = scene
                .as_ref()
                .map(|scene| scene.simulation_dt())
                .unwrap_or(XR_WORKER_SIMULATION_DT_DEFAULT);
            should_publish = true;
        }

        if !pending_body_spawns.is_empty() {
            if let Some(scene) = scene.as_mut() {
                let mut applied_spawn = false;
                for body_spawn in pending_body_spawns {
                    if body_spawn.revision != revision {
                        continue;
                    }
                    if let Some(query_key) = scene.respawn_body(
                        body_spawn.spawn.widget_uid,
                        body_spawn.spawn.pose,
                        body_spawn.spawn.linvel,
                        body_spawn.spawn.angvel,
                    ) {
                        retained_hits.remove(&query_key);
                    }
                    applied_spawn = true;
                }
                should_publish |= applied_spawn;
            }
        }

        if let Some(step) = pending_step {
            if step.revision == revision {
                let started = Instant::now();
                adaptive_step_dt =
                    choose_worker_simulation_dt(last_step_started_at, started, adaptive_step_dt);
                last_step_started_at = Some(started);
                sync_hands_on_scene(scene.as_mut(), &step.left_hand, &step.right_hand);
                let tsdf_query_started = Instant::now();
                sync_depth_query_surfaces_with_store(
                    &mut retained_hits,
                    scene.as_mut(),
                    &depth_mesh,
                );
                let physics_tsdf_query_ms = tsdf_query_started.elapsed().as_secs_f64() * 1000.0;
                let (runtime_bodies, physics_rapier_step_ms, physics_depth_query_surface_count) =
                    if let Some(scene) = scene.as_mut() {
                        let simulation_dt = (adaptive_step_dt * step.time_scale.clamp(0.1, 1.0))
                            .clamp(XR_WORKER_SIMULATION_DT_MIN, XR_WORKER_SIMULATION_DT_MAX);
                        scene.set_simulation_dt(simulation_dt);
                        let rapier_step_started = Instant::now();
                        scene.step();
                        let physics_rapier_step_ms =
                            rapier_step_started.elapsed().as_secs_f64() * 1000.0;
                        let stats = scene.depth_query_stats();
                        snapshot_runtime_bodies(scene, &mut runtime_bodies_scratch);
                        (
                            mem::take(&mut runtime_bodies_scratch),
                            physics_rapier_step_ms,
                            stats.surface_count,
                        )
                    } else {
                        (HashMap::new(), 0.0, 0)
                    };
                if step.include_retained_hits {
                    snapshot_retained_hits(&retained_hits, &mut retained_hits_snapshot_scratch);
                } else {
                    retained_hits_snapshot_scratch.clear();
                }
                let recycled = publish_worker_result(
                    &latest_result,
                    XrPhysicsWorkerResult {
                        revision,
                        runtime_bodies,
                        depth_query_retained_hits: step
                            .include_retained_hits
                            .then(|| mem::take(&mut retained_hits_snapshot_scratch)),
                        physics_compute_ms: started.elapsed().as_secs_f64() * 1000.0,
                        physics_tsdf_query_ms,
                        physics_rapier_step_ms,
                        physics_depth_query_surface_count,
                    },
                );
                recycle_worker_buffers(
                    recycled,
                    &mut runtime_bodies_scratch,
                    &mut retained_hits_snapshot_scratch,
                );
                continue;
            }
        }

        if should_publish {
            let (runtime_bodies, surface_count) = if let Some(scene) = scene.as_ref() {
                let stats = scene.depth_query_stats();
                snapshot_runtime_bodies(scene, &mut runtime_bodies_scratch);
                (mem::take(&mut runtime_bodies_scratch), stats.surface_count)
            } else {
                (HashMap::new(), 0)
            };
            let recycled = publish_worker_result(
                &latest_result,
                XrPhysicsWorkerResult {
                    revision,
                    runtime_bodies,
                    depth_query_retained_hits: None,
                    physics_compute_ms: 0.0,
                    physics_tsdf_query_ms: 0.0,
                    physics_rapier_step_ms: 0.0,
                    physics_depth_query_surface_count: surface_count,
                },
            );
            recycle_worker_buffers(
                recycled,
                &mut runtime_bodies_scratch,
                &mut retained_hits_snapshot_scratch,
            );
        }
    }
}

fn choose_worker_simulation_dt(
    last_step_started_at: Option<Instant>,
    started: Instant,
    previous_dt: f32,
) -> f32 {
    let measured_dt = last_step_started_at
        .map(|last| (started - last).as_secs_f32())
        .unwrap_or(XR_WORKER_SIMULATION_DT_DEFAULT)
        .clamp(XR_WORKER_SIMULATION_DT_DEFAULT, XR_WORKER_SIMULATION_DT_MAX);
    let smoothed_dt = previous_dt + (measured_dt - previous_dt) * XR_WORKER_SIMULATION_DT_SMOOTHING;
    XR_WORKER_SIMULATION_DT_LADDER
        .into_iter()
        .find(|candidate| *candidate >= smoothed_dt)
        .unwrap_or(XR_WORKER_SIMULATION_DT_MAX)
}

fn publish_worker_result(
    latest_result: &Mutex<Option<XrPhysicsWorkerResult>>,
    result: XrPhysicsWorkerResult,
) -> Option<XrPhysicsWorkerResult> {
    if let Ok(mut latest) = latest_result.lock() {
        return latest.replace(result);
    }
    None
}

fn build_scene(gravity: f32, cubes: Vec<CollectedXrCube>) -> RapierScene {
    let mut scene = RapierScene::new(gravity);
    for cube in cubes {
        let projectile_pool = cube.projectile_pool;
        match cube.body_kind {
            XrBodyKind::Disabled => {}
            XrBodyKind::Dynamic => {
                if cube.is_sphere {
                    scene.spawn_dynamic_sphere(
                        cube.uid,
                        cube.pose,
                        cube.half_extents
                            .x
                            .min(cube.half_extents.y)
                            .min(cube.half_extents.z),
                        cube.scale,
                        cube.density,
                        cube.friction,
                        cube.restitution,
                    );
                } else {
                    scene.spawn_dynamic_box(
                        cube.uid,
                        cube.pose,
                        cube.half_extents,
                        cube.scale,
                        cube.density,
                        cube.friction,
                        cube.restitution,
                    );
                }
                if projectile_pool && !scene.cubes.is_empty() {
                    scene.register_projectile_cube(scene.cubes.len() - 1);
                }
            }
            XrBodyKind::Fixed => {
                if cube.is_sphere {
                    scene.spawn_fixed_sphere(
                        cube.uid,
                        cube.pose,
                        cube.half_extents
                            .x
                            .min(cube.half_extents.y)
                            .min(cube.half_extents.z),
                        cube.scale,
                        cube.friction,
                        cube.restitution,
                    );
                } else {
                    scene.spawn_fixed_box(
                        cube.uid,
                        cube.pose,
                        cube.half_extents,
                        cube.scale,
                        cube.friction,
                        cube.restitution,
                    );
                }
            }
        }
    }
    scene
}

fn snapshot_runtime_bodies(
    scene: &RapierScene,
    runtime_bodies: &mut HashMap<WidgetUid, XrRuntimeBodyState>,
) {
    runtime_bodies.clear();
    runtime_bodies.reserve(scene.cubes.len().saturating_sub(runtime_bodies.capacity()));
    for cube in &scene.cubes {
        if let Some(body) = scene.bodies.get(cube.body) {
            if !body.is_enabled() {
                continue;
            }
            runtime_bodies.insert(
                cube.widget_uid,
                XrRuntimeBodyState {
                    pose: makepad_pose(body.position()),
                    scale: cube.scale,
                },
            );
        }
    }
}

fn snapshot_retained_hits(
    retained_hits: &HashMap<u64, RetainedDepthQueryHit>,
    snapshot: &mut HashMap<u64, RetainedDepthQueryHit>,
) {
    snapshot.clear();
    snapshot.reserve(retained_hits.len().saturating_sub(snapshot.capacity()));
    for (&key, value) in retained_hits {
        snapshot.insert(key, value.clone());
    }
}

fn recycle_worker_buffers(
    recycled: Option<XrPhysicsWorkerResult>,
    runtime_bodies_scratch: &mut HashMap<WidgetUid, XrRuntimeBodyState>,
    retained_hits_snapshot_scratch: &mut HashMap<u64, RetainedDepthQueryHit>,
) {
    let Some(mut recycled) = recycled else {
        return;
    };
    *runtime_bodies_scratch = recycled.runtime_bodies;
    if let Some(retained_hits) = recycled.depth_query_retained_hits.take() {
        *retained_hits_snapshot_scratch = retained_hits;
    }
}
