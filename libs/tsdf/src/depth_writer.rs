//! The depth writer: one long-lived thread ("xr-tsdf-depth-writer") that integrates headset
//! depth frames into the TSDF volume and publishes its snapshots to the store. Ported from the
//! retired Android backend's `openxr_depth.rs` (b22a714840): the frame side only builds a
//! [`DepthMeshJob`] and hands it over; the worker keeps the newest job (and, among the jobs it
//! had no time for, the most novel one), integrates at most one per turn, refreshes the
//! projected height field the alignment reads, publishes a snapshot when anything changed, and
//! runs the store's cooperative step (the alignment matcher).
//!
//! Who waits on what: the frame side never waits (an unbounded channel; the worker drains it
//! to the newest job and counts the drops). The worker sleeps on the channel for at most
//! `DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS` so its periodic work keeps running without frames.
//! Readers (physics queries, peers) take the published `Arc` snapshot from the store.

use crate::*;
use makepad_math::{vec3f, vec4f, CameraFov, Mat4f, Pose, Quat};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

const DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS: u64 = 8;
const DEPTH_COOPERATIVE_STEP_INTERVAL_MILLIS: u64 = 8;
const DEPTH_COOPERATIVE_IDLE_POLL_INTERVAL_MILLIS: u64 = 33;
const DEPTH_TSDF_INPUT_THROTTLING_DISABLED: bool = true;

/// One eye of a depth frame: the depth camera's pose in the local space (y up, metres) and its
/// field of view (radians: left, right, up, down; left and down negative).
#[derive(Clone, Copy, Debug, Default)]
pub struct DepthFrameView {
    pub orientation: [f32; 4],
    pub position: [f32; 3],
    pub fov: [f32; 4],
}

/// A headset depth image (XR_META_environment_depth): `width` x `height` D16 texels of
/// `views[eye]`'s projection with `near`/`far` (`far` 0 = infinite).
pub struct DepthFrame {
    pub width: u32,
    pub height: u32,
    pub eye: usize,
    pub near: f32,
    pub far: f32,
    pub views: [DepthFrameView; 2],
    pub depth: Vec<u16>,
}

struct PendingDepthCandidate {
    job: DepthMeshJob,
    novelty: DepthFrameNovelty,
}

/// The frame side of the depth writer (one per process: the store is the process's).
pub struct DepthWriter {
    mailbox: Sender<DepthMeshJob>,
    store: XrTsdfStore,
    next_generation: u64,
    last_reset_generation: u64,
    last_depth_readback_at: Option<Instant>,
}

impl DepthWriter {
    /// Starts the worker (on the process's TSDF store).
    pub fn new() -> Result<Self, String> {
        let store = xr_tsdf_store();
        let (sender, receiver) = mpsc::channel();
        let worker_store = store.clone();
        std::thread::Builder::new()
            .name("xr-tsdf-depth-writer".into())
            .spawn(move || {
                background_priority();
                depth_preprocess_tsdf_writer_worker(receiver, worker_store)
            })
            .map_err(|e| format!("depth worker unavailable: {e}"))?;
        Ok(Self { mailbox: sender, store, next_generation: 1, last_reset_generation: 0, last_depth_readback_at: None })
    }

    /// Hands one depth frame to the worker (never waits).
    pub fn submit(&mut self, frame: DepthFrame) -> Result<(), String> {
        self.store.record_seen();
        let reset_generation = self.store.reset_generation();
        if self.last_reset_generation != reset_generation {
            self.last_reset_generation = reset_generation;
            self.last_depth_readback_at = None;
        }
        let eye = frame.eye.min(1);
        if frame.width == 0 || frame.height == 0 || frame.depth.len() < (frame.width * frame.height) as usize {
            let err = "depth frame has invalid dimensions".to_string();
            self.store.set_error(err.clone());
            return Err(err);
        }
        let view = frame.views[eye];
        let [x, y, z, w] = view.orientation;
        let local_from_depth_eye = Pose::new(Quat { x, y, z, w }, vec3f(view.position[0], view.position[1], view.position[2]));
        let depth_view_from_world = local_from_depth_eye.invert().to_mat4();
        let [angle_left, angle_right, angle_up, angle_down] = view.fov;
        let depth_proj = Mat4f::from_camera_fov(&CameraFov { angle_left, angle_right, angle_up, angle_down }, frame.near, if frame.far.is_finite() { frame.far } else { 0.0 });
        let inv_depth_proj = depth_proj.invert();
        let world_from_depth_view = depth_view_from_world.invert();
        let camera_world = world_from_depth_view.transform_vec4(vec4f(0.0, 0.0, 0.0, 1.0));
        if !camera_world.w.is_finite() || camera_world.w.abs() < 1.0e-6 {
            let err = "depth camera transform is invalid".to_string();
            self.store.set_error(err.clone());
            return Err(err);
        }
        let camera_world = vec3f(camera_world.x / camera_world.w, camera_world.y / camera_world.w, camera_world.z / camera_world.w);

        let now = Instant::now();
        if !DEPTH_TSDF_INPUT_THROTTLING_DISABLED
            && !submit_should_readback_depth_frame(self.store.latest_tsdf_snapshot().as_deref(), camera_world, inv_depth_proj, world_from_depth_view, now, self.last_depth_readback_at)
        {
            self.store.record_drop();
            return Ok(());
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        let job = DepthMeshJob {
            reset_generation,
            generation,
            eye_index: eye,
            width: frame.width,
            height: frame.height,
            voxel_size_meters: self.store.voxel_size_meters(),
            camera_world,
            depth_proj,
            inv_depth_proj,
            depth_view_from_world,
            world_from_depth_view,
            depth: frame.depth,
        };
        if self.mailbox.send(job).is_err() {
            let error = "depth worker is unavailable".to_string();
            self.store.set_error(error.clone());
            return Err(error);
        }
        self.last_depth_readback_at = Some(now);
        Ok(())
    }
}

fn pending_depth_candidate_should_replace(pending: &PendingDepthCandidate, candidate: &PendingDepthCandidate) -> bool {
    const EPSILON: f32 = 1.0e-4;
    if candidate.novelty.score > pending.novelty.score + EPSILON {
        return true;
    }
    if (candidate.novelty.score - pending.novelty.score).abs() <= EPSILON {
        if candidate.novelty.valid_samples > pending.novelty.valid_samples {
            return true;
        }
        if candidate.novelty.valid_samples == pending.novelty.valid_samples {
            return candidate.job.generation > pending.job.generation;
        }
    }
    false
}

fn enqueue_pending_depth_candidate(pending_depth_candidate: &mut Option<PendingDepthCandidate>, candidate: PendingDepthCandidate, store: &XrTsdfStore) {
    match pending_depth_candidate {
        Some(pending) if pending_depth_candidate_should_replace(pending, &candidate) => {
            store.record_drop();
            *pending = candidate;
        }
        Some(_) => store.record_drop(),
        None => *pending_depth_candidate = Some(candidate),
    }
}

/// The worker yields to the frame threads: it runs as long as it is given (a whole core while
/// the room changes), and at the app's own priority it held the headset's frame loop late.
pub(crate) fn background_priority() {
    #[cfg(any(target_os = "android", target_os = "linux"))]
    {
        extern "C" {
            fn setpriority(which: i32, who: u32, prio: i32) -> i32;
            fn gettid() -> i32;
        }
        // (PRIO_PROCESS with a thread id: that thread alone)
        unsafe { setpriority(0, gettid() as u32, 10) };
    }
}

fn depth_preprocess_tsdf_writer_worker(mailbox: Receiver<DepthMeshJob>, store: XrTsdfStore) {
    let mut preprocess_state = DepthPreprocessWorkerState::default();
    let mut volume = DepthMeshVolume::new(store.voxel_size_meters());
    let mut next_height_map_slice_at = Instant::now();
    let mut next_height_map_publish_at = next_height_map_slice_at + matched_height_map_publish_interval(&store);
    let mut next_cooperative_step_at = next_height_map_slice_at;
    let mut applied_reset_generation = store.reset_generation();
    let mut pending_depth_candidate = None::<PendingDepthCandidate>;
    let mut last_depth_integration_at = None::<Instant>;
    let mut bricks = BrickProducer::default();
    loop {
        let configured_voxel_size = store.voxel_size_meters();
        let requested_reset_generation = store.reset_generation();
        if (volume.voxel_size_meters() - configured_voxel_size).abs() > f32::EPSILON || applied_reset_generation != requested_reset_generation {
            applied_reset_generation = requested_reset_generation;
            volume = DepthMeshVolume::new(configured_voxel_size);
            next_height_map_slice_at = Instant::now();
            next_height_map_publish_at = next_height_map_slice_at + matched_height_map_publish_interval(&store);
            next_cooperative_step_at = next_height_map_slice_at;
            pending_depth_candidate = None;
            last_depth_integration_at = None;
            bricks.reset(&store);
        }
        let mut applied_update = false;
        match mailbox.recv_timeout(Duration::from_millis(DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS)) {
            Ok(mut job) => {
                loop {
                    match mailbox.try_recv() {
                        Ok(next_job) => {
                            store.record_drop();
                            job = next_job;
                        }
                        Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                    }
                }
                if job.reset_generation == store.reset_generation() && (job.voxel_size_meters - store.voxel_size_meters()).abs() <= f32::EPSILON {
                    let candidate = PendingDepthCandidate { novelty: score_depth_job_novelty(&volume, &job), job };
                    enqueue_pending_depth_candidate(&mut pending_depth_candidate, candidate, &store);
                }
            }
            // the frame side is gone (its process is ending): so is the worker
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }

        let can_integrate_depth = pending_depth_candidate.is_some()
            && (DEPTH_TSDF_INPUT_THROTTLING_DISABLED
                || last_depth_integration_at.is_none_or(|last| last.elapsed() >= Duration::from_millis(DEPTH_TSD_TARGET_INTEGRATION_INTERVAL_MILLIS)));
        if can_integrate_depth {
            let candidate = pending_depth_candidate.take().expect("candidate should exist when integration is allowed");
            match preprocess_depth_mesh(candidate.job, &mut preprocess_state) {
                Ok(job) => {
                    if job.reset_generation() == store.reset_generation() && (job.voxel_size_meters() - store.voxel_size_meters()).abs() <= f32::EPSILON {
                        apply_preprocessed_depth_mesh(job, &preprocess_state, &mut volume);
                        volume.discard_obsolete_surface_state();
                        applied_update = true;
                        last_depth_integration_at = Some(Instant::now());
                    }
                }
                Err(err) => store.set_error(err),
            }
        }

        let surface_analysis_enabled = store.surface_analysis_enabled();
        let now = Instant::now();
        if surface_analysis_enabled {
            sync_projected_height_field_layout(&mut volume);
            sync_projected_height_field_player_cutout(&mut volume);
        } else {
            next_height_map_slice_at = now;
            next_height_map_publish_at = now + matched_height_map_publish_interval(&store);
            volume.set_projected_height_publish_pending(false);
        }
        let mut slice_credits = 0usize;
        if surface_analysis_enabled {
            let slice_interval = Duration::from_millis(DEPTH_PROJECTED_HEIGHT_REFRESH_INTERVAL_MILLIS);
            while now >= next_height_map_slice_at && slice_credits < DEPTH_ALIGN_PROJECTED_HEIGHT_MAX_SLICE_CREDITS {
                slice_credits += 1;
                next_height_map_slice_at += slice_interval;
            }
            let refresh_budget = projected_height_refresh_budget(volume.pending_projected_height_sample_count(), now, next_height_map_publish_at, slice_credits);
            if refresh_budget != 0 && refresh_projected_height_field(&mut volume, refresh_budget) {
                volume.set_projected_height_publish_pending(true);
            }
        }
        let height_map_changed = if surface_analysis_enabled {
            if now >= next_height_map_publish_at && (volume.projected_height_publish_pending() || !volume.has_published_height_map()) {
                while now >= next_height_map_publish_at {
                    next_height_map_publish_at += matched_height_map_publish_interval(&store);
                }
                volume.set_projected_height_publish_pending(false);
                update_published_height_map(&mut volume)
            } else {
                false
            }
        } else {
            volume.clear_published_height_map()
        };
        if applied_update || height_map_changed {
            let previous_snapshot = store.latest_tsdf_snapshot();
            let snapshot = volume.published_tsdf_snapshot(previous_snapshot.as_deref());
            bricks.note_snapshot(&snapshot);
            store.publish_tsdf_snapshot(snapshot);
            volume.clear_published_tsdf_dirty_state();
        }
        // the changed bricks for peers (debounced: a brick touched every frame goes out at most
        // every BRICK_DEBOUNCE)
        bricks.produce(&store, Instant::now());

        let now = Instant::now();
        if now >= next_cooperative_step_at {
            let schedule_fast = store.run_cooperative_step().is_some_and(|result| result.did_work || result.has_more_work);
            next_cooperative_step_at = now
                + Duration::from_millis(if schedule_fast { DEPTH_COOPERATIVE_STEP_INTERVAL_MILLIS } else { DEPTH_COOPERATIVE_IDLE_POLL_INTERVAL_MILLIS });
        }
    }
}

fn matched_height_map_publish_interval(store: &XrTsdfStore) -> Duration {
    let base = Duration::from_millis(DEPTH_PUBLISHED_HEIGHT_MAP_INTERVAL_MILLIS);
    let stats = store.cooperative_step_stats();
    let cycle_micros = stats.average_cycle_micros.max(stats.last_cycle_micros);
    if cycle_micros == 0 { base } else { base.max(Duration::from_micros(cycle_micros)) }
}
