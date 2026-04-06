use crate::{
    makepad_math::{vec3f, vec4f},
    os::linux::{
        openxr::CxOpenXrFrame,
        vulkan::{CxVulkan, CxVulkanOpenXrSessionData},
    },
    thread::SignalToUI,
    xr_tsdf::{
        apply_preprocessed_depth_mesh, preprocess_depth_mesh, projected_height_refresh_budget,
        refresh_projected_height_field, score_depth_job_novelty,
        submit_should_readback_depth_frame, sync_projected_height_field_layout,
        sync_projected_height_field_player_cutout, update_published_height_map, xr_tsdf_store,
        DepthFrameNovelty, DepthMeshJob, DepthMeshVolume, DepthPreprocessWorkerState, XrTsdfStore,
        DEPTH_ALIGN_PROJECTED_HEIGHT_MAX_SLICE_CREDITS,
        DEPTH_PROJECTED_HEIGHT_REFRESH_INTERVAL_MILLIS, DEPTH_PUBLISHED_HEIGHT_MAP_INTERVAL_MILLIS,
        DEPTH_TSD_TARGET_INTEGRATION_INTERVAL_MILLIS, DEPTH_VOXEL_EYE_INDEX,
    },
};
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

const DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS: u64 = 8;
const DEPTH_COOPERATIVE_STEP_INTERVAL_MILLIS: u64 = 8;
const DEPTH_COOPERATIVE_IDLE_POLL_INTERVAL_MILLIS: u64 = 33;
const DEPTH_TSDF_INPUT_THROTTLING_DISABLED: bool = true;

#[derive(Default)]
struct LatestDepthJobMailbox {
    latest: Option<DepthMeshJob>,
}

type SharedLatestDepthJobMailbox = Arc<(Mutex<LatestDepthJobMailbox>, Condvar)>;

struct PendingDepthCandidate {
    job: DepthMeshJob,
    novelty: DepthFrameNovelty,
}

fn replace_latest_depth_job(mailbox: &SharedLatestDepthJobMailbox, job: DepthMeshJob) -> bool {
    let (lock, condvar) = &**mailbox;
    let mut state = lock.lock().unwrap_or_else(|err| err.into_inner());
    let replaced = state.latest.replace(job).is_some();
    condvar.notify_one();
    replaced
}

fn take_latest_depth_job(
    mailbox: &SharedLatestDepthJobMailbox,
    timeout: Duration,
) -> Option<DepthMeshJob> {
    let (lock, condvar) = &**mailbox;
    let mut state = lock.lock().unwrap_or_else(|err| err.into_inner());
    if state.latest.is_none() && !timeout.is_zero() {
        let (guard, _) = condvar
            .wait_timeout(state, timeout)
            .unwrap_or_else(|err| err.into_inner());
        state = guard;
    }
    state.latest.take()
}

fn pending_depth_candidate_should_replace(
    pending: &PendingDepthCandidate,
    candidate: &PendingDepthCandidate,
) -> bool {
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

fn enqueue_pending_depth_candidate(
    pending_depth_candidate: &mut Option<PendingDepthCandidate>,
    candidate: PendingDepthCandidate,
    store: &XrTsdfStore,
) {
    match pending_depth_candidate {
        Some(pending) if pending_depth_candidate_should_replace(pending, &candidate) => {
            store.record_drop();
            *pending = candidate;
        }
        Some(_) => {
            store.record_drop();
        }
        None => {
            *pending_depth_candidate = Some(candidate);
        }
    }
}

pub(super) struct CxOpenXrDepthMeshPipeline {
    mailbox: SharedLatestDepthJobMailbox,
    store: XrTsdfStore,
    next_generation: u64,
    last_reset_generation: u64,
    last_depth_readback_at: Option<Instant>,
}

impl CxOpenXrDepthMeshPipeline {
    pub fn new() -> Self {
        let store = xr_tsdf_store();
        let mailbox = Arc::new((Mutex::new(LatestDepthJobMailbox::default()), Condvar::new()));
        std::thread::spawn({
            let mailbox = mailbox.clone();
            let store = store.clone();
            move || depth_preprocess_tsdf_writer_worker(mailbox, store)
        });
        Self {
            mailbox,
            store,
            next_generation: 1,
            last_reset_generation: 0,
            last_depth_readback_at: None,
        }
    }

    pub fn submit(
        &mut self,
        vulkan: &mut CxVulkan,
        render_targets: &CxVulkanOpenXrSessionData,
        frame: &CxOpenXrFrame,
        depth_image_index: usize,
    ) -> Result<(), String> {
        self.store.record_seen();
        let reset_generation = self.store.reset_generation();
        if self.last_reset_generation != reset_generation {
            self.last_reset_generation = reset_generation;
            self.last_depth_readback_at = None;
        }
        let pose_result = (|| {
            let width = render_targets.depth_width;
            let height = render_targets.depth_height;
            if width == 0 || height == 0 {
                return Err("OpenXR depth readback dimensions are zero".to_string());
            }
            let depth_proj = frame.eyes[DEPTH_VOXEL_EYE_INDEX].depth_proj_mat;
            let inv_depth_proj = depth_proj.invert();
            let world_from_depth_view = frame.eyes[DEPTH_VOXEL_EYE_INDEX].depth_view_mat.invert();
            let camera_world = world_from_depth_view.transform_vec4(vec4f(0.0, 0.0, 0.0, 1.0));
            if !camera_world.w.is_finite() || camera_world.w.abs() < 1.0e-6 {
                return Err("OpenXR depth camera transform is invalid".to_string());
            }
            let camera_world = vec3f(
                camera_world.x / camera_world.w,
                camera_world.y / camera_world.w,
                camera_world.z / camera_world.w,
            );
            Ok((
                width,
                height,
                depth_proj,
                inv_depth_proj,
                world_from_depth_view,
                camera_world,
            ))
        })();

        let (width, height, depth_proj, inv_depth_proj, world_from_depth_view, camera_world) =
            match pose_result {
                Ok(parts) => parts,
                Err(err) => {
                    self.store.set_error(err.clone());
                    return Err(err);
                }
            };

        let now = Instant::now();
        if !DEPTH_TSDF_INPUT_THROTTLING_DISABLED
            && !submit_should_readback_depth_frame(
                self.store.latest_tsdf_snapshot().as_deref(),
                camera_world,
                inv_depth_proj,
                world_from_depth_view,
                now,
                self.last_depth_readback_at,
            )
        {
            self.store.record_drop();
            return Ok(());
        }

        let generation = self.next_generation;
        self.next_generation += 1;
        let voxel_size_meters = self.store.voxel_size_meters();

        let job_result: Result<DepthMeshJob, String> = (|| {
            let depth = vulkan.read_openxr_depth_image(
                render_targets,
                depth_image_index,
                DEPTH_VOXEL_EYE_INDEX,
            )?;
            Ok(DepthMeshJob {
                reset_generation,
                generation,
                eye_index: DEPTH_VOXEL_EYE_INDEX,
                width,
                height,
                voxel_size_meters,
                camera_world,
                depth_proj,
                inv_depth_proj,
                depth_view_from_world: frame.eyes[DEPTH_VOXEL_EYE_INDEX].depth_view_mat,
                world_from_depth_view,
                depth,
            })
        })();

        let job = match job_result {
            Ok(job) => job,
            Err(err) => {
                self.store.set_error(err.clone());
                return Err(err);
            }
        };

        if replace_latest_depth_job(&self.mailbox, job) {
            self.store.record_drop();
        }
        self.last_depth_readback_at = Some(now);

        Ok(())
    }
}

fn depth_preprocess_tsdf_writer_worker(mailbox: SharedLatestDepthJobMailbox, store: XrTsdfStore) {
    let mut preprocess_state = DepthPreprocessWorkerState::default();
    let mut volume = DepthMeshVolume::new(store.voxel_size_meters());
    let mut next_height_map_slice_at = Instant::now();
    let mut next_height_map_publish_at =
        next_height_map_slice_at + matched_height_map_publish_interval(&store);
    let mut next_cooperative_step_at = next_height_map_slice_at;
    let mut applied_reset_generation = store.reset_generation();
    let mut pending_depth_candidate = None::<PendingDepthCandidate>;
    let mut last_depth_integration_at = None::<Instant>;
    loop {
        let configured_voxel_size = store.voxel_size_meters();
        if (volume.voxel_size_meters() - configured_voxel_size).abs() > f32::EPSILON {
            volume = DepthMeshVolume::new(configured_voxel_size);
            next_height_map_slice_at = Instant::now();
            next_height_map_publish_at =
                next_height_map_slice_at + matched_height_map_publish_interval(&store);
            next_cooperative_step_at = next_height_map_slice_at;
            pending_depth_candidate = None;
            last_depth_integration_at = None;
        }
        let requested_reset_generation = store.reset_generation();
        if applied_reset_generation != requested_reset_generation {
            applied_reset_generation = requested_reset_generation;
            volume = DepthMeshVolume::new(configured_voxel_size);
            next_height_map_slice_at = Instant::now();
            next_height_map_publish_at =
                next_height_map_slice_at + matched_height_map_publish_interval(&store);
            next_cooperative_step_at = next_height_map_slice_at;
            pending_depth_candidate = None;
            last_depth_integration_at = None;
        }
        let mut applied_update = false;
        if let Some(mut job) = take_latest_depth_job(
            &mailbox,
            Duration::from_millis(DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS),
        ) {
            loop {
                if job.reset_generation != store.reset_generation() {
                    break;
                }
                if (job.voxel_size_meters - store.voxel_size_meters()).abs() > f32::EPSILON {
                    break;
                }
                let candidate = PendingDepthCandidate {
                    novelty: score_depth_job_novelty(&volume, &job),
                    job,
                };
                enqueue_pending_depth_candidate(&mut pending_depth_candidate, candidate, &store);
                if let Some(next_job) = take_latest_depth_job(&mailbox, Duration::ZERO) {
                    job = next_job;
                    continue;
                }
                break;
            }
        }

        let can_integrate_depth = pending_depth_candidate.is_some()
            && (DEPTH_TSDF_INPUT_THROTTLING_DISABLED
                || last_depth_integration_at.is_none_or(|last| {
                    last.elapsed()
                        >= Duration::from_millis(DEPTH_TSD_TARGET_INTEGRATION_INTERVAL_MILLIS)
                }));
        if can_integrate_depth {
            let candidate = pending_depth_candidate
                .take()
                .expect("candidate should exist when integration is allowed");
            let result = preprocess_depth_mesh(candidate.job, &mut preprocess_state);
            match result {
                Ok(job) => {
                    if job.reset_generation() == store.reset_generation()
                        && (job.voxel_size_meters() - store.voxel_size_meters()).abs()
                            <= f32::EPSILON
                    {
                        apply_preprocessed_depth_mesh(job, &preprocess_state, &mut volume);
                        volume.discard_obsolete_surface_state();
                        applied_update = true;
                        last_depth_integration_at = Some(Instant::now());
                    }
                }
                Err(err) => {
                    store.set_error(err);
                }
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
            let slice_interval =
                Duration::from_millis(DEPTH_PROJECTED_HEIGHT_REFRESH_INTERVAL_MILLIS);
            while now >= next_height_map_slice_at
                && slice_credits < DEPTH_ALIGN_PROJECTED_HEIGHT_MAX_SLICE_CREDITS
            {
                slice_credits += 1;
                next_height_map_slice_at += slice_interval;
            }
            let refresh_budget = projected_height_refresh_budget(
                volume.pending_projected_height_sample_count(),
                now,
                next_height_map_publish_at,
                slice_credits,
            );
            if refresh_budget != 0 && refresh_projected_height_field(&mut volume, refresh_budget) {
                volume.set_projected_height_publish_pending(true);
            }
        }
        let height_map_changed = if surface_analysis_enabled {
            if now >= next_height_map_publish_at
                && (volume.projected_height_publish_pending() || !volume.has_published_height_map())
            {
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
        let snapshot_changed = applied_update || height_map_changed;
        if snapshot_changed {
            let previous_snapshot = store.latest_tsdf_snapshot();
            let snapshot = volume.published_tsdf_snapshot(previous_snapshot.as_deref());
            store.publish_tsdf_snapshot(snapshot);
            volume.clear_published_tsdf_dirty_state();
            SignalToUI::set_ui_signal();
        }

        let now = Instant::now();
        if now >= next_cooperative_step_at {
            let schedule_fast = store
                .run_cooperative_step()
                .is_some_and(|result| result.did_work || result.has_more_work);
            next_cooperative_step_at = now
                + Duration::from_millis(if schedule_fast {
                    DEPTH_COOPERATIVE_STEP_INTERVAL_MILLIS
                } else {
                    DEPTH_COOPERATIVE_IDLE_POLL_INTERVAL_MILLIS
                });
        }
    }
}

fn matched_height_map_publish_interval(store: &XrTsdfStore) -> Duration {
    let base = Duration::from_millis(DEPTH_PUBLISHED_HEIGHT_MAP_INTERVAL_MILLIS);
    let stats = store.cooperative_step_stats();
    let cycle_micros = stats.average_cycle_micros.max(stats.last_cycle_micros);
    if cycle_micros == 0 {
        base
    } else {
        base.max(Duration::from_micros(cycle_micros))
    }
}
