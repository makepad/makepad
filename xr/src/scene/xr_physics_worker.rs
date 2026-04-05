use super::{
    xr_depth::{
        clear_depth_query_state_for_scene, sync_depth_query_surfaces_with_store,
        RetainedDepthQueryHit,
    },
    xr_hands::sync_hands_on_scene,
    xr_physics::{makepad_pose, RapierScene, XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE},
    CollectedXrCube,
};
use crate::{
    prelude::*,
    scene::{XrBodyKind, XrRuntimeBodyState},
};
use makepad_widgets::makepad_platform::{
    event::{XrController, XrHand},
    XrTsdfStore,
};
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
const XR_PHYSICS_WORKER_MAX_PENDING_BODY_DESPAWNS: usize = 8;
const XR_PHYSICS_WORKER_MAX_PENDING_BODY_IMPULSES: usize = 8;
const XR_PHYSICS_WORKER_MAX_PENDING_BODY_WRENCHES: usize = 8;
const XR_PHYSICS_WORKER_MAX_PENDING_BODY_DRIVES: usize = 8;
const XR_PHYSICS_WORKER_MAX_PENDING_CAR_CONTROLS: usize = 8;

#[derive(Clone)]
struct PhysicsWorkerRebuild {
    revision: u64,
    gravity: f32,
    cubes: Vec<CollectedXrCube>,
    floor_y: Option<f32>,
}

#[derive(Clone)]
struct PhysicsWorkerStep {
    revision: u64,
    left_hand: XrHand,
    right_hand: XrHand,
    left_controller: XrController,
    right_controller: XrController,
    floor_y: Option<f32>,
    time_scale: f32,
    include_retained_hits: bool,
}

#[derive(Clone, Copy)]
struct PhysicsWorkerBodySpawn {
    revision: u64,
    spawn: XrBodySpawn,
}

#[derive(Clone, Copy)]
struct PhysicsWorkerBodyDespawn {
    revision: u64,
    widget_uid: WidgetUid,
}

#[derive(Clone, Copy)]
struct PhysicsWorkerBodyImpulse {
    revision: u64,
    impulse: XrBodyImpulse,
}

#[derive(Clone, Copy)]
struct PhysicsWorkerBodyWrench {
    revision: u64,
    wrench: XrBodyWrench,
}

#[derive(Clone, Copy)]
struct PhysicsWorkerBodyDrive {
    revision: u64,
    drive: XrBodyDrive,
}

#[derive(Clone, Copy)]
struct PhysicsWorkerCarControl {
    revision: u64,
    control: XrCarControl,
}

#[derive(Default)]
struct PhysicsWorkerMailbox {
    version: u64,
    shutdown: bool,
    pending_reset_revision: Option<u64>,
    pending_rebuild: Option<PhysicsWorkerRebuild>,
    pending_step: Option<PhysicsWorkerStep>,
    pending_body_spawns: Vec<PhysicsWorkerBodySpawn>,
    pending_body_despawns:
        SmallVec<[PhysicsWorkerBodyDespawn; XR_PHYSICS_WORKER_MAX_PENDING_BODY_DESPAWNS]>,
    pending_body_impulses:
        SmallVec<[PhysicsWorkerBodyImpulse; XR_PHYSICS_WORKER_MAX_PENDING_BODY_IMPULSES]>,
    pending_body_wrenches:
        SmallVec<[PhysicsWorkerBodyWrench; XR_PHYSICS_WORKER_MAX_PENDING_BODY_WRENCHES]>,
    pending_body_drives:
        SmallVec<[PhysicsWorkerBodyDrive; XR_PHYSICS_WORKER_MAX_PENDING_BODY_DRIVES]>,
    pending_car_controls:
        SmallVec<[PhysicsWorkerCarControl; XR_PHYSICS_WORKER_MAX_PENDING_CAR_CONTROLS]>,
}

pub(super) struct XrPhysicsWorkerResult {
    pub(super) revision: u64,
    pub(super) runtime_bodies: HashMap<WidgetUid, XrRuntimeBodyState>,
    pub(super) runtime_contacts: Vec<(WidgetUid, WidgetUid)>,
    pub(super) depth_query_retained_hits: Option<HashMap<u64, RetainedDepthQueryHit>>,
    pub(super) physics_compute_ms: f64,
    pub(super) physics_tsdf_query_ms: f64,
    pub(super) physics_rapier_step_ms: f64,
    pub(super) physics_depth_query_surface_count: usize,
    pub(super) physics_scene_body_count: usize,
    pub(super) physics_body_spawn_apply_count: usize,
    pub(super) physics_body_spawn_miss_count: usize,
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
        floor_y: Option<f32>,
    ) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.pending_rebuild = Some(PhysicsWorkerRebuild {
                revision,
                gravity,
                cubes,
                floor_y,
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
        left_controller: XrController,
        right_controller: XrController,
        floor_y: Option<f32>,
        time_scale: f32,
        include_retained_hits: bool,
    ) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.pending_step = Some(PhysicsWorkerStep {
                revision,
                left_hand,
                right_hand,
                left_controller,
                right_controller,
                floor_y,
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
            if let Some(pending) = mailbox
                .pending_body_spawns
                .iter_mut()
                .find(|pending| pending.spawn.widget_uid == spawn.widget_uid)
            {
                pending.revision = revision;
                pending.spawn = spawn;
            } else {
                mailbox
                    .pending_body_spawns
                    .push(PhysicsWorkerBodySpawn { revision, spawn });
            }
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
    }

    pub(super) fn request_body_despawn(&mut self, revision: u64, widget_uid: WidgetUid) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            if mailbox.pending_body_despawns.len() < XR_PHYSICS_WORKER_MAX_PENDING_BODY_DESPAWNS {
                mailbox
                    .pending_body_despawns
                    .push(PhysicsWorkerBodyDespawn {
                        revision,
                        widget_uid,
                    });
                mailbox.version = mailbox.version.saturating_add(1);
                wake.notify_one();
            }
        }
    }

    pub(super) fn request_body_impulse(&mut self, revision: u64, impulse: XrBodyImpulse) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            if mailbox.pending_body_impulses.len() < XR_PHYSICS_WORKER_MAX_PENDING_BODY_IMPULSES {
                mailbox
                    .pending_body_impulses
                    .push(PhysicsWorkerBodyImpulse { revision, impulse });
                mailbox.version = mailbox.version.saturating_add(1);
                wake.notify_one();
            }
        }
    }

    pub(super) fn request_body_wrench(&mut self, revision: u64, wrench: XrBodyWrench) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            if mailbox.pending_body_wrenches.len() < XR_PHYSICS_WORKER_MAX_PENDING_BODY_WRENCHES {
                mailbox
                    .pending_body_wrenches
                    .push(PhysicsWorkerBodyWrench { revision, wrench });
                mailbox.version = mailbox.version.saturating_add(1);
                wake.notify_one();
            }
        }
    }

    pub(super) fn request_body_drive(&mut self, revision: u64, drive: XrBodyDrive) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            if mailbox.pending_body_drives.len() < XR_PHYSICS_WORKER_MAX_PENDING_BODY_DRIVES {
                mailbox
                    .pending_body_drives
                    .push(PhysicsWorkerBodyDrive { revision, drive });
                mailbox.version = mailbox.version.saturating_add(1);
                wake.notify_one();
            }
        }
    }

    pub(super) fn request_car_control(&mut self, revision: u64, control: XrCarControl) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            if mailbox.pending_car_controls.len() < XR_PHYSICS_WORKER_MAX_PENDING_CAR_CONTROLS {
                mailbox
                    .pending_car_controls
                    .push(PhysicsWorkerCarControl { revision, control });
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
            mailbox.pending_body_despawns.clear();
            mailbox.pending_body_impulses.clear();
            mailbox.pending_body_wrenches.clear();
            mailbox.pending_body_drives.clear();
            mailbox.pending_car_controls.clear();
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
    let mut runtime_contacts_scratch = Vec::new();
    let mut retained_hits_snapshot_scratch = HashMap::new();
    let mut last_step_started_at: Option<Instant> = None;
    let mut adaptive_step_dt = XR_WORKER_SIMULATION_DT_DEFAULT;
    let mut total_body_spawn_apply_count = 0usize;
    let mut total_body_spawn_miss_count = 0usize;

    loop {
        let (
            pending_reset_revision,
            pending_rebuild,
            pending_step,
            pending_body_spawns,
            pending_body_despawns,
            pending_body_impulses,
            pending_body_wrenches,
            pending_body_drives,
            pending_car_controls,
        ) = {
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
                std::mem::take(&mut guard.pending_body_despawns),
                std::mem::take(&mut guard.pending_body_impulses),
                std::mem::take(&mut guard.pending_body_wrenches),
                std::mem::take(&mut guard.pending_body_drives),
                std::mem::take(&mut guard.pending_car_controls),
            )
        };

        let mut should_publish = false;
        let mut physics_body_spawn_apply_count = 0usize;
        let mut physics_body_spawn_miss_count = 0usize;

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
            if let Some(scene) = scene.as_mut() {
                let floor_y = rebuild.floor_y.or_else(|| {
                    depth_mesh
                        .latest_tsdf_snapshot()
                        .as_deref()
                        .and_then(|snapshot| snapshot.lowest_y_meters())
                });
                scene.sync_floor_halfspace(floor_y);
            }
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
                    let query_keys = scene.respawn_body(
                        body_spawn.spawn.widget_uid,
                        body_spawn.spawn.shadow,
                        body_spawn.spawn.mode,
                        body_spawn.spawn.pose,
                        body_spawn.spawn.linvel,
                        body_spawn.spawn.angvel,
                    );
                    if query_keys.iter().any(Option::is_some) {
                        clear_depth_query_keys(&mut retained_hits, query_keys);
                        physics_body_spawn_apply_count =
                            physics_body_spawn_apply_count.saturating_add(1);
                        total_body_spawn_apply_count =
                            total_body_spawn_apply_count.saturating_add(1);
                    } else {
                        physics_body_spawn_miss_count =
                            physics_body_spawn_miss_count.saturating_add(1);
                        total_body_spawn_miss_count = total_body_spawn_miss_count.saturating_add(1);
                    }
                    applied_spawn = true;
                }
                should_publish |= applied_spawn;
            }
        }

        if !pending_body_despawns.is_empty() {
            if let Some(scene) = scene.as_mut() {
                let mut applied_despawn = false;
                for body_despawn in pending_body_despawns {
                    if body_despawn.revision != revision {
                        continue;
                    }
                    clear_depth_query_keys(
                        &mut retained_hits,
                        scene.despawn_body(body_despawn.widget_uid),
                    );
                    applied_despawn = true;
                }
                should_publish |= applied_despawn;
            }
        }

        if !pending_body_impulses.is_empty() {
            if let Some(scene) = scene.as_mut() {
                let mut applied_impulse = false;
                for body_impulse in pending_body_impulses {
                    if body_impulse.revision != revision {
                        continue;
                    }
                    applied_impulse |= scene.apply_impulse(
                        body_impulse.impulse.widget_uid,
                        body_impulse.impulse.point,
                        body_impulse.impulse.impulse,
                    );
                }
                should_publish |= applied_impulse;
            }
        }

        if !pending_body_wrenches.is_empty() {
            if let Some(scene) = scene.as_mut() {
                let mut applied_wrench = false;
                for body_wrench in pending_body_wrenches {
                    if body_wrench.revision != revision {
                        continue;
                    }
                    applied_wrench |= scene.apply_wrench(
                        body_wrench.wrench.widget_uid,
                        body_wrench.wrench.force,
                        body_wrench.wrench.torque,
                    );
                }
                should_publish |= applied_wrench;
            }
        }

        if !pending_body_drives.is_empty() && pending_step.is_none() {
            if let Some(scene) = scene.as_mut() {
                let mut applied_drive = false;
                let simulation_dt = scene.simulation_dt();
                for body_drive in pending_body_drives.iter().copied() {
                    if body_drive.revision != revision {
                        continue;
                    }
                    applied_drive |= scene.apply_drive(
                        body_drive.drive.widget_uid,
                        body_drive.drive.target_linvel,
                        body_drive.drive.target_angvel,
                        body_drive.drive.max_linear_accel,
                        body_drive.drive.max_angular_accel,
                        body_drive.drive.preserve_vertical_linvel,
                        simulation_dt,
                    );
                }
                should_publish |= applied_drive;
            }
        }

        if pending_step.is_some() {
            if let Some(scene) = scene.as_mut() {
                scene.clear_car_controls();
            }
        }
        if !pending_car_controls.is_empty() {
            if let Some(scene) = scene.as_mut() {
                for car_control in pending_car_controls.iter().copied() {
                    if car_control.revision != revision {
                        continue;
                    }
                    scene.apply_car_control(car_control.control);
                }
            }
        }

        if let Some(step) = pending_step {
            if step.revision == revision {
                let started = Instant::now();
                adaptive_step_dt =
                    choose_worker_simulation_dt(last_step_started_at, started, adaptive_step_dt);
                last_step_started_at = Some(started);
                if let Some(scene) = scene.as_mut() {
                    scene.sync_vehicle_query_sources_pre_step();
                }
                sync_hands_on_scene(
                    scene.as_mut(),
                    &step.left_hand,
                    &step.right_hand,
                    &step.left_controller,
                    &step.right_controller,
                );
                let tsdf_query_started = Instant::now();
                sync_depth_query_surfaces_with_store(
                    &mut retained_hits,
                    scene.as_mut(),
                    &depth_mesh,
                    step.floor_y,
                );
                let physics_tsdf_query_ms = tsdf_query_started.elapsed().as_secs_f64() * 1000.0;
                let (
                    runtime_bodies,
                    runtime_contacts,
                    physics_rapier_step_ms,
                    physics_depth_query_surface_count,
                ) = if let Some(scene) = scene.as_mut() {
                    let simulation_dt = (adaptive_step_dt * step.time_scale.clamp(0.1, 1.0))
                        .clamp(XR_WORKER_SIMULATION_DT_MIN, XR_WORKER_SIMULATION_DT_MAX);
                    scene.set_simulation_dt(simulation_dt);
                    let rapier_step_started = Instant::now();
                    scene.step();
                    for body_drive in pending_body_drives.iter().copied() {
                        if body_drive.revision != revision {
                            continue;
                        }
                        scene.apply_drive(
                            body_drive.drive.widget_uid,
                            body_drive.drive.target_linvel,
                            body_drive.drive.target_angvel,
                            body_drive.drive.max_linear_accel,
                            body_drive.drive.max_angular_accel,
                            body_drive.drive.preserve_vertical_linvel,
                            simulation_dt,
                        );
                    }
                    let physics_rapier_step_ms =
                        rapier_step_started.elapsed().as_secs_f64() * 1000.0;
                    let stats = scene.depth_query_stats();
                    snapshot_runtime_bodies(scene, &mut runtime_bodies_scratch);
                    scene.snapshot_active_contacts(&mut runtime_contacts_scratch);
                    (
                        mem::take(&mut runtime_bodies_scratch),
                        mem::take(&mut runtime_contacts_scratch),
                        physics_rapier_step_ms,
                        stats.surface_count,
                    )
                } else {
                    (HashMap::new(), Vec::new(), 0.0, 0)
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
                        runtime_contacts,
                        depth_query_retained_hits: step
                            .include_retained_hits
                            .then(|| mem::take(&mut retained_hits_snapshot_scratch)),
                        physics_compute_ms: started.elapsed().as_secs_f64() * 1000.0,
                        physics_tsdf_query_ms,
                        physics_rapier_step_ms,
                        physics_depth_query_surface_count,
                        physics_scene_body_count: scene
                            .as_ref()
                            .map(|scene| scene.cubes.len())
                            .unwrap_or(0),
                        physics_body_spawn_apply_count: total_body_spawn_apply_count,
                        physics_body_spawn_miss_count: total_body_spawn_miss_count,
                    },
                );
                recycle_worker_buffers(
                    recycled,
                    &mut runtime_bodies_scratch,
                    &mut runtime_contacts_scratch,
                    &mut retained_hits_snapshot_scratch,
                );
                continue;
            }
        }

        if should_publish {
            let (runtime_bodies, runtime_contacts, surface_count) =
                if let Some(scene) = scene.as_ref() {
                    let stats = scene.depth_query_stats();
                    snapshot_runtime_bodies(scene, &mut runtime_bodies_scratch);
                    scene.snapshot_active_contacts(&mut runtime_contacts_scratch);
                    (
                        mem::take(&mut runtime_bodies_scratch),
                        mem::take(&mut runtime_contacts_scratch),
                        stats.surface_count,
                    )
                } else {
                    (HashMap::new(), Vec::new(), 0)
                };
            let recycled = publish_worker_result(
                &latest_result,
                XrPhysicsWorkerResult {
                    revision,
                    runtime_bodies,
                    runtime_contacts,
                    depth_query_retained_hits: None,
                    physics_compute_ms: 0.0,
                    physics_tsdf_query_ms: 0.0,
                    physics_rapier_step_ms: 0.0,
                    physics_depth_query_surface_count: surface_count,
                    physics_scene_body_count: scene
                        .as_ref()
                        .map(|scene| scene.cubes.len())
                        .unwrap_or(0),
                    physics_body_spawn_apply_count: total_body_spawn_apply_count,
                    physics_body_spawn_miss_count: total_body_spawn_miss_count,
                },
            );
            recycle_worker_buffers(
                recycled,
                &mut runtime_bodies_scratch,
                &mut runtime_contacts_scratch,
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

fn clear_depth_query_keys(
    retained_hits: &mut HashMap<u64, RetainedDepthQueryHit>,
    keys: [Option<u64>; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE],
) {
    for key in keys.into_iter().flatten() {
        retained_hits.remove(&key);
    }
}

fn build_scene(gravity: f32, cubes: Vec<CollectedXrCube>) -> RapierScene {
    let mut scene = RapierScene::new(gravity);
    for cube in cubes {
        let spawn_pool = cube.spawn_pool;
        match cube.body_kind {
            XrBodyKind::Disabled => {}
            XrBodyKind::Dynamic => {
                if cube.physics_shape == XrPhysicsShape::Sphere {
                    scene.spawn_dynamic_sphere_with_support(
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
                        cube.depth_query_support,
                    );
                } else {
                    scene.spawn_dynamic_box_with_support(
                        cube.uid,
                        cube.pose,
                        cube.half_extents,
                        cube.scale,
                        cube.density,
                        cube.friction,
                        cube.restitution,
                        cube.depth_query_support,
                    );
                }
                if let Some(body_handle) = scene.cubes.last().map(|cube| cube.body) {
                    if let Some(body) = scene.bodies.get_mut(body_handle) {
                        body.set_gravity_scale(cube.gravity_scale.max(0.0), true);
                    }
                }
                if spawn_pool && !scene.cubes.is_empty() {
                    scene.register_spawn_pool_cube(scene.cubes.len() - 1);
                }
            }
            XrBodyKind::Fixed => {
                if cube.physics_shape == XrPhysicsShape::Sphere {
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
                    linvel: scene
                        .shadow_body_motion_for_body(cube.body)
                        .map(|(linvel, _)| linvel)
                        .unwrap_or_else(|| {
                            let linvel = body.linvel();
                            vec3f(linvel.x, linvel.y, linvel.z)
                        }),
                    angvel: scene
                        .shadow_body_motion_for_body(cube.body)
                        .map(|(_, angvel)| angvel)
                        .unwrap_or_else(|| {
                            let angvel = body.angvel();
                            vec3f(angvel.x, angvel.y, angvel.z)
                        }),
                    sleeping: body.is_sleeping(),
                    dynamic_body: body.body_type() == rapier3d::prelude::RigidBodyType::Dynamic,
                    shadowed: scene.is_shadow_body(cube.body),
                    held_by: scene.held_by_for_body(cube.body),
                    linked_support_local_poses: scene.cube_linked_support_local_poses(*cube),
                    linked_support_spin_angles: scene.cube_linked_support_spin_angles(*cube),
                    linked_support_steer_angles: scene.cube_linked_support_steer_angles(*cube),
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
    runtime_contacts_scratch: &mut Vec<(WidgetUid, WidgetUid)>,
    retained_hits_snapshot_scratch: &mut HashMap<u64, RetainedDepthQueryHit>,
) {
    let Some(mut recycled) = recycled else {
        return;
    };
    *runtime_bodies_scratch = recycled.runtime_bodies;
    *runtime_contacts_scratch = recycled.runtime_contacts;
    if let Some(retained_hits) = recycled.depth_query_retained_hits.take() {
        *retained_hits_snapshot_scratch = retained_hits;
    }
}
