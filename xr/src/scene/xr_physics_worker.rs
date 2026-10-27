use super::{
    xr_depth::{
        clear_depth_query_state_for_scene, sync_depth_query_surfaces_with_store,
        RetainedDepthQueryHit,
    },
    xr_hands::sync_hands_on_scene,
    xr_physics::{PhysicsScene, XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE},
    XrPhysicsBody, XrPlayerHands,
};
use crate::{
    prelude::*,
    scene::{XrBodyKind, XrRuntimeBodyState},
    util::clock::Instant,
};
use makepad_widgets::makepad_platform::XrTsdfStore;
use makepad_widgets::makepad_platform::event::xr::{XrController, XrHand};
use std::{
    collections::HashMap,
    mem,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

/// The physics' fixed step (seconds of the requests' clock): every step is this long whatever
/// the cadence of the requests, so a body falls or slides by the same amount per unit of time
/// and the draw can interpolate between two states for any picture's time.
pub const XR_WORKER_FIXED_DT: f64 = 1.0 / 120.0;
const XR_WORKER_SIMULATION_DT_MIN: f32 = 1.0 / 480.0;
const XR_WORKER_SIMULATION_DT_MAX: f32 = 1.0 / 45.0;
/// The states kept for the draw's interpolation (at 120 Hz: the last ~66 ms).
const XR_WORKER_HISTORY: usize = 8;
/// A request this far from the simulated time (backwards, or ahead after a stall or a change
/// of clock) restarts the clock there instead of simulating the whole gap step by step.
const XR_WORKER_CLOCK_JUMP: f64 = 0.25;
/// At most this much simulated time is caught up in one batch (30 steps of ~0.1 ms); a longer
/// gap is skipped (logged).
const XR_WORKER_MAX_CATCH_UP: f64 = 0.25;
/// Hands are extrapolated past their newest sample by at most this many sample intervals.
const XR_WORKER_HAND_MAX_EXTRAPOLATION: f32 = 1.5;
const XR_PHYSICS_WORKER_MAX_PENDING_BODY_DESPAWNS: usize = 8;
const XR_PHYSICS_WORKER_MAX_PENDING_BODY_IMPULSES: usize = 8;
const XR_PHYSICS_WORKER_MAX_PENDING_BODY_WRENCHES: usize = 8;
const XR_PHYSICS_WORKER_MAX_PENDING_BODY_DRIVES: usize = 8;
const XR_PHYSICS_WORKER_MAX_PENDING_CAR_CONTROLS: usize = 8;

#[derive(Clone)]
struct PhysicsWorkerRebuild {
    revision: u64,
    gravity: f32,
    cubes: Vec<XrPhysicsBody>,
    floor_y: Option<f32>,
}

#[derive(Clone)]
struct PhysicsWorkerStep {
    revision: u64,
    /// the hands' time (seconds; on a headset the picture's display time)
    time: f64,
    hands: Vec<XrPlayerHands>,
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
    /// every hand sample since the worker last looked, oldest first (none is dropped: each is
    /// a hand pose the steps between it and its neighbours interpolate)
    pending_steps: Vec<PhysicsWorkerStep>,
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

/// One physics state as the draw interpolates it: every body's pose at `time`.
#[derive(Clone, Debug, Default)]
pub struct XrPhysicsState {
    pub time: f64,
    pub bodies: Vec<(WidgetUid, Pose)>,
}

/// What the worker counts (since the last result the UI took is not known to it: totals).
#[derive(Clone, Copy, Debug, Default)]
pub struct XrPhysicsCounters {
    /// fixed steps simulated
    pub steps: u64,
    /// step requests (hand samples) taken
    pub samples: u64,
    /// samples whose hands differed from the sample before (any player's palm or grip moved)
    pub new_hands: u64,
    /// simulated time skipped (seconds) because a batch fell too far behind
    pub skipped_seconds: f64,
}

pub(super) struct XrPhysicsWorkerResult {
    pub(super) revision: u64,
    /// the newest states, oldest first (the newest is `runtime_bodies`' time)
    pub(super) history: Vec<XrPhysicsState>,
    pub(super) counters: XrPhysicsCounters,
    pub(super) runtime_bodies: HashMap<WidgetUid, XrRuntimeBodyState>,
    pub(super) runtime_contacts: Vec<(WidgetUid, WidgetUid)>,
    pub(super) depth_query_retained_hits: Option<HashMap<u64, RetainedDepthQueryHit>>,
    pub(super) physics_compute_ms: f64,
    pub(super) physics_tsdf_query_ms: f64,
    pub(super) physics_engine_step_ms: f64,
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
        cubes: Vec<XrPhysicsBody>,
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
        time: f64,
        hands: Vec<XrPlayerHands>,
        floor_y: Option<f32>,
        time_scale: f32,
        include_retained_hits: bool,
    ) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.pending_steps.push(PhysicsWorkerStep {
                revision,
                time,
                hands,
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
            mailbox.pending_steps.clear();
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
    let mut scene: Option<PhysicsScene> = None;
    let mut retained_hits = HashMap::new();
    let mut runtime_bodies_scratch = HashMap::new();
    let mut runtime_contacts_scratch = Vec::new();
    let mut retained_hits_snapshot_scratch = HashMap::new();
    let mut clock = WorkerClock::default();
    let mut total_body_spawn_apply_count = 0usize;
    let mut total_body_spawn_miss_count = 0usize;

    loop {
        let (
            pending_reset_revision,
            pending_rebuild,
            pending_steps,
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
                std::mem::take(&mut guard.pending_steps),
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
            clock.restart();
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
            clock.restart();
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

        let pending_steps: Vec<PhysicsWorkerStep> = pending_steps.into_iter().filter(|s| s.revision == revision).collect();
        if !pending_body_drives.is_empty() && pending_steps.is_empty() {
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

        if !pending_steps.is_empty() {
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

        if let Some(newest) = pending_steps.last() {
            let (floor_y, time_scale, include_retained_hits) = (newest.floor_y, newest.time_scale, newest.include_retained_hits);
            let started = Instant::now();
            for step in pending_steps {
                clock.take_sample(step.time, step.hands);
            }
            let mut physics_tsdf_query_ms = 0.0;
            let mut physics_engine_step_ms = 0.0;
            // fixed steps up to a little past the newest sample (one sample interval and a
            // quarter): the next picture's time is then already simulated, so it is drawn
            // between two states, never extrapolated
            let steps = clock.steps_to_run();
            let ran = !steps.is_empty();
            for step_time in steps {
                let Some(scene) = scene.as_mut() else { break };
                let hands = clock.hands_at(step_time);
                let simulation_dt = ((XR_WORKER_FIXED_DT as f32) * time_scale.clamp(0.1, 1.0))
                    .clamp(XR_WORKER_SIMULATION_DT_MIN, XR_WORKER_SIMULATION_DT_MAX);
                scene.set_simulation_dt(simulation_dt);
                scene.sync_vehicle_query_sources_pre_step();
                sync_hands_on_scene(Some(&mut *scene), &hands);
                let tsdf_query_started = Instant::now();
                sync_depth_query_surfaces_with_store(&mut retained_hits, Some(&mut *scene), &depth_mesh, floor_y);
                physics_tsdf_query_ms += tsdf_query_started.elapsed().as_secs_f64() * 1000.0;
                let engine_step_started = Instant::now();
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
                physics_engine_step_ms += engine_step_started.elapsed().as_secs_f64() * 1000.0;
                clock.stepped(step_time, scene);
            }
            if let Some(scene) = scene.as_ref().filter(|_| ran) {
                let stats = scene.depth_query_stats();
                snapshot_runtime_bodies(scene, &mut runtime_bodies_scratch);
                scene.snapshot_active_contacts(&mut runtime_contacts_scratch);
                if include_retained_hits {
                    snapshot_retained_hits(&retained_hits, &mut retained_hits_snapshot_scratch);
                } else {
                    retained_hits_snapshot_scratch.clear();
                }
                let recycled = publish_worker_result(
                    &latest_result,
                    XrPhysicsWorkerResult {
                        revision,
                        history: clock.history.iter().cloned().collect(),
                        counters: clock.counters,
                        runtime_bodies: mem::take(&mut runtime_bodies_scratch),
                        runtime_contacts: mem::take(&mut runtime_contacts_scratch),
                        depth_query_retained_hits: include_retained_hits
                            .then(|| mem::take(&mut retained_hits_snapshot_scratch)),
                        physics_compute_ms: started.elapsed().as_secs_f64() * 1000.0,
                        physics_tsdf_query_ms,
                        physics_engine_step_ms,
                        physics_depth_query_surface_count: stats.surface_count,
                        physics_scene_body_count: scene.cubes.len(),
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
                    history: clock.history.iter().cloned().collect(),
                    counters: clock.counters,
                    runtime_bodies,
                    runtime_contacts,
                    depth_query_retained_hits: None,
                    physics_compute_ms: 0.0,
                    physics_tsdf_query_ms: 0.0,
                    physics_engine_step_ms: 0.0,
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

/// The worker's clock: the simulated time, the hand samples the steps interpolate, the
/// states the draw interpolates.
#[derive(Default)]
struct WorkerClock {
    /// the time of the newest simulated state
    sim_time: Option<f64>,
    /// the hand samples' mean spacing (seconds)
    period: f64,
    /// the newest hand samples by time, oldest first (two bracket a step; a third covers a
    /// sample that came a little late)
    samples: std::collections::VecDeque<(f64, Vec<XrPlayerHands>)>,
    history: std::collections::VecDeque<XrPhysicsState>,
    counters: XrPhysicsCounters,
}

impl WorkerClock {
    /// A new scene (or none): the clock starts again at the next sample.
    fn restart(&mut self) {
        self.sim_time = None;
        self.samples.clear();
        self.history.clear();
    }

    fn take_sample(&mut self, time: f64, hands: Vec<XrPlayerHands>) {
        self.counters.samples += 1;
        if let Some((last_time, last)) = self.samples.back() {
            let dt = time - *last_time;
            if dt <= 0.0 || dt > XR_WORKER_CLOCK_JUMP {
                // a clock that went backwards or jumped: start from this sample
                self.samples.clear();
                if self.sim_time.is_some_and(|t| (t - time).abs() > XR_WORKER_CLOCK_JUMP) {
                    self.sim_time = None;
                    self.history.clear();
                }
            } else {
                self.period = if self.period > 0.0 { self.period + (dt - self.period) * 0.1 } else { dt };
                if hands_moved(last, &hands) {
                    self.counters.new_hands += 1;
                }
            }
        }
        self.samples.push_back((time, hands));
        while self.samples.len() > 3 {
            self.samples.pop_front();
        }
    }

    /// The times of the fixed steps to simulate now: up to one sample interval and a quarter
    /// past the newest sample.
    fn steps_to_run(&mut self) -> Vec<f64> {
        let Some(&(newest, _)) = self.samples.back() else { return Vec::new() };
        let period = if self.period > 0.0 { self.period.clamp(1.0 / 144.0, 1.0 / 30.0) } else { 1.0 / 90.0 };
        let target = newest + period * 1.25;
        let mut t = *self.sim_time.get_or_insert(newest - XR_WORKER_FIXED_DT);
        if target - t > XR_WORKER_MAX_CATCH_UP {
            let skipped = target - XR_WORKER_MAX_CATCH_UP - t;
            self.counters.skipped_seconds += skipped;
            eprintln!("[xr physics] the steps fell {:.0} ms behind the hands: {:.0} ms skipped", (target - t) * 1000.0, skipped * 1000.0);
            t += skipped;
            self.sim_time = Some(t);
        }
        let mut out = Vec::new();
        while t + XR_WORKER_FIXED_DT <= target + 1.0e-9 {
            t += XR_WORKER_FIXED_DT;
            out.push(t);
        }
        out
    }

    /// Every player's hands at `time`: between the two samples around it, or carried on past
    /// the newest by its motion (at most `XR_WORKER_HAND_MAX_EXTRAPOLATION` intervals).
    fn hands_at(&self, time: f64) -> Vec<XrPlayerHands> {
        let n = self.samples.len();
        let Some((_, newest)) = self.samples.back() else { return Vec::new() };
        if n < 2 {
            return newest.clone();
        }
        // the pair to interpolate: the first sample at or after `time`, and the one before it
        let at = self.samples.iter().position(|(t, _)| *t >= time).unwrap_or(n - 1).max(1);
        let (ta, a) = &self.samples[at - 1];
        let (tb, b) = &self.samples[at];
        let span = (tb - ta).max(1.0e-6);
        let f = (((time - ta) / span) as f32).clamp(0.0, 1.0 + XR_WORKER_HAND_MAX_EXTRAPOLATION);
        b.iter()
            .map(|hb| match a.iter().find(|ha| ha.player == hb.player) {
                Some(ha) => XrPlayerHands {
                    player: hb.player,
                    left_hand: lerp_hand(&ha.left_hand, &hb.left_hand, f),
                    right_hand: lerp_hand(&ha.right_hand, &hb.right_hand, f),
                    left_controller: lerp_controller(&ha.left_controller, &hb.left_controller, f),
                    right_controller: lerp_controller(&ha.right_controller, &hb.right_controller, f),
                },
                None => hb.clone(),
            })
            .collect()
    }

    /// A step done: its state into the history.
    fn stepped(&mut self, time: f64, scene: &PhysicsScene) {
        self.sim_time = Some(time);
        self.counters.steps += 1;
        let mut state = if self.history.len() >= XR_WORKER_HISTORY { self.history.pop_front().unwrap_or_default() } else { XrPhysicsState::default() };
        state.time = time;
        state.bodies.clear();
        for cube in &scene.cubes {
            if scene.body_is_valid(cube.body) && scene.body_is_enabled(cube.body) {
                state.bodies.push((cube.widget_uid, scene.body_pose(cube.body)));
            }
        }
        self.history.push_back(state);
    }
}

/// Whether any hand or controller of `b` is somewhere else than in `a` (a new hand sample).
fn hands_moved(a: &[XrPlayerHands], b: &[XrPlayerHands]) -> bool {
    a.len() != b.len()
        || a.iter().zip(b).any(|(a, b)| {
            a.player != b.player
                || a.left_hand.joints[0] != b.left_hand.joints[0]
                || a.right_hand.joints[0] != b.right_hand.joints[0]
                || a.left_controller.grip_pose != b.left_controller.grip_pose
                || a.right_controller.grip_pose != b.right_controller.grip_pose
        })
}

/// A hand between two samples (`f` 0..1, above 1 carried on): joints, tips and aim move; the
/// discrete state (tracked, pinches, the grasp) is the nearer sample's.
fn lerp_hand(a: &XrHand, b: &XrHand, f: f32) -> XrHand {
    if !a.in_view() || !b.in_view() || (f - 1.0).abs() < 1.0e-6 {
        return b.clone();
    }
    let mut out = if f < 0.5 { a.clone() } else { b.clone() };
    for (o, (ja, jb)) in out.joints.iter_mut().zip(a.joints.iter().zip(b.joints.iter())) {
        *o = Pose::from_lerp(*ja, *jb, f);
    }
    for (o, (ta, tb)) in out.tips.iter_mut().zip(a.tips.iter().zip(b.tips.iter())) {
        *o = ta + (tb - ta) * f;
    }
    out.aim_pose = Pose::from_lerp(a.aim_pose, b.aim_pose, f);
    out
}

fn lerp_controller(a: &XrController, b: &XrController, f: f32) -> XrController {
    if !a.active() || !b.active() {
        return b.clone();
    }
    let mut out = if f < 0.5 { a.clone() } else { b.clone() };
    out.grip_pose = Pose::from_lerp(a.grip_pose, b.grip_pose, f);
    out.aim_pose = Pose::from_lerp(a.aim_pose, b.aim_pose, f);
    out.grip = a.grip + (b.grip - a.grip) * f.min(1.0);
    out.trigger = a.trigger + (b.trigger - a.trigger) * f.min(1.0);
    out
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

fn build_scene(gravity: f32, cubes: Vec<XrPhysicsBody>) -> PhysicsScene {
    let mut scene = PhysicsScene::new(gravity);
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
                    scene.set_body_gravity_scale(body_handle, cube.gravity_scale.max(0.0));
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
    scene: &PhysicsScene,
    runtime_bodies: &mut HashMap<WidgetUid, XrRuntimeBodyState>,
) {
    runtime_bodies.clear();
    runtime_bodies.reserve(scene.cubes.len().saturating_sub(runtime_bodies.capacity()));
    for cube in &scene.cubes {
        if !scene.body_is_valid(cube.body) || !scene.body_is_enabled(cube.body) {
            continue;
        }
        runtime_bodies.insert(
            cube.widget_uid,
            XrRuntimeBodyState {
                pose: scene.body_pose(cube.body),
                scale: cube.scale,
                linvel: scene
                    .shadow_body_motion_for_body(cube.body)
                    .map(|(linvel, _)| linvel)
                    .unwrap_or_else(|| scene.body_linvel(cube.body)),
                angvel: scene
                    .shadow_body_motion_for_body(cube.body)
                    .map(|(_, angvel)| angvel)
                    .unwrap_or_else(|| scene.body_angvel(cube.body)),
                sleeping: scene.body_is_sleeping(cube.body),
                dynamic_body: scene.body_is_dynamic(cube.body),
                shadowed: scene.is_shadow_body(cube.body),
                held_by: scene.held_by_for_body(cube.body),
                held_by_player: scene.held_by_player_for_body(cube.body),
                grab_offset: scene.grab_offset_for_body(cube.body),
                second_hold: scene.second_hold_for_body(cube.body),
                linked_support_local_poses: scene.cube_linked_support_local_poses(*cube),
                linked_support_spin_angles: scene.cube_linked_support_spin_angles(*cube),
                linked_support_steer_angles: scene.cube_linked_support_steer_angles(*cube),
            },
        );
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
