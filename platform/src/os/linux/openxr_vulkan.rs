use super::*;
use crate::{
    makepad_math::{vec3, vec3f, vec4f, Pose, Quat, Vec3f},
    os::linux::vulkan::{CxVulkan, CxVulkanOpenXrSessionData},
    thread::SignalToUI,
    xr_depth_voxels::{
        xr_depth_voxels_store, XrDepthEvidenceChunk, XrDepthPhysicsBox, XrDepthPhysicsChunk,
        XrDepthPhysicsChunkKey, XrDepthPlaneAccumulator, XrDepthPlaneKey, XrDepthSurfaceChunk,
        XrDepthVoxels, XrDepthVoxelsStore,
    },
};
use parry3d::{
    math::{IVector, Pose as ParryPose, Vector},
    shape::SharedShape,
};
use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{channel, Receiver, Sender},
    Arc, RwLock,
};

pub(super) struct CxOpenXrVulkanSession {
    _color_images: Vec<XrSwapchainImageVulkanKHR>,
    _depth_images: Vec<XrSwapchainImageVulkanKHR>,
    render_targets: CxVulkanOpenXrSessionData,
    depth_voxel_pipeline: CxOpenXrDepthVoxelPipeline,
}

const DEPTH_VOXEL_EYE_INDEX: usize = 0;
const DEPTH_VOXEL_SAMPLE_STEP: u32 = 4;
const DEPTH_VOXEL_SIZE_METERS: f32 = 0.05;
const DEPTH_VOXEL_MIN_DISTANCE_METERS: f32 = 0.08;
const DEPTH_VOXEL_MAX_DISTANCE_METERS: f32 = 6.0;
const DEPTH_VOXEL_MIN_DEPTH_VALUE: f32 = 1.0 / 65535.0;
const DEPTH_VOXEL_MAX_DEPTH_VALUE: f32 = 0.9995;
const DEPTH_VOXEL_EVIDENCE_MAX: i16 = 8;
const DEPTH_VOXEL_HIT_WEIGHT: i16 = 2;
const DEPTH_VOXEL_MISS_WEIGHT: i16 = 2;
const DEPTH_VOXEL_RAY_STEP_SCALE: f32 = 1.0;
const DEPTH_PHYSICS_REBUILD_INTERVAL_MS: u64 = 500;
const DEPTH_PHYSICS_PENDING_CHANGE_THRESHOLD: usize = 768;
const DEPTH_PHYSICS_CHUNK_SIZE: i32 = 96;
const DEPTH_SURFACE_SAMPLE_MAX_COUNT: u16 = 24;
const DEPTH_SURFACE_SAMPLE_MIN_COUNT: u16 = 3;
const DEPTH_PLANE_HORIZONTAL_NORMAL_DOT: f32 = 0.86;
const DEPTH_PLANE_VERTICAL_NORMAL_DOT_MAX: f32 = 0.35;
const DEPTH_PLANE_VERTICAL_BINS: usize = 8;
const DEPTH_PLANE_DISTANCE_BIN_METERS: f32 = 0.10;
const DEPTH_PLANE_DISTANCE_TOLERANCE_METERS: f32 = 0.14;
const DEPTH_PLANE_NORMAL_TOLERANCE_DOT: f32 = 0.84;
const DEPTH_PLANE_MIN_SUPPORT: usize = 24;
const DEPTH_PLANE_MAX_PER_CHUNK: usize = 6;
const DEPTH_PLANE_MIN_SPAN_METERS: f32 = 0.45;
const DEPTH_PLANE_THICKNESS_METERS: f32 = 0.09;
const DEPTH_PLANE_RECT_MIN_COVERAGE_NUM: usize = 1;
const DEPTH_PLANE_RECT_MIN_COVERAGE_DEN: usize = 2;
const DEPTH_PLANE_CELL_SIZE_METERS: f32 = 0.15;
const DEPTH_PLANE_MIN_CELL_SUPPORT: u16 = 2;
const DEPTH_PLANE_MAX_CELL_SUPPORT: u16 = 32;
const DEPTH_PLANE_MIN_TOTAL_SUPPORT: u32 = 96;
const DEPTH_PLANE_MAX_PHYSICS_PLANES: usize = 12;
const DEPTH_PLANE_MAX_BOXES_PER_PLANE: usize = 3;
const DEPTH_PLANE_STALE_GENERATIONS: u64 = 360;
const DEPTH_PLANE_STALE_SUPPORT_KEEP: u32 = 512;
const DEPTH_PHYSICS_FALLBACK_MIN_OCCUPIED: usize = 96;
const DEPTH_PHYSICS_FALLBACK_COARSE_STRIDE: i32 = 2;
const DEPTH_PHYSICS_FALLBACK_MAX_BOXES: usize = 12;
const DEPTH_PHYSICS_SLAB_MIN_AREA: usize = 36;
const DEPTH_PHYSICS_SLAB_MIN_EDGE: i32 = 3;
const DEPTH_PHYSICS_SLAB_MAX_THICKNESS: i32 = 4;
const DEPTH_PHYSICS_SLAB_MIN_COVERAGE_NUM: usize = 2;
const DEPTH_PHYSICS_SLAB_MIN_COVERAGE_DEN: usize = 3;

struct CxOpenXrDepthVoxelJob {
    generation: u64,
    eye_index: usize,
    width: u32,
    height: u32,
    sample_step: u32,
    camera_world: Vec3f,
    inv_depth_proj: Mat4f,
    world_from_depth_view: Mat4f,
    depth: Vec<u16>,
}

struct CxOpenXrDepthVoxelPipeline {
    sender: Sender<CxOpenXrDepthVoxelJob>,
    busy: Arc<AtomicBool>,
    store: XrDepthVoxelsStore,
    _volume: Arc<RwLock<XrDepthVoxels>>,
    next_generation: u64,
}

impl CxOpenXrDepthVoxelPipeline {
    fn new() -> Self {
        let store = xr_depth_voxels_store();
        let volume = store
            .ensure_volume(|| XrDepthVoxels::new(DEPTH_VOXEL_SAMPLE_STEP, DEPTH_VOXEL_SIZE_METERS));
        let busy = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = channel();
        std::thread::spawn({
            let busy = busy.clone();
            let store = store.clone();
            let volume = volume.clone();
            move || depth_voxel_worker(receiver, busy, store, volume)
        });
        Self {
            sender,
            busy,
            store,
            _volume: volume,
            next_generation: 1,
        }
    }

    fn submit(
        &mut self,
        vulkan: &mut CxVulkan,
        render_targets: &CxVulkanOpenXrSessionData,
        frame: &CxOpenXrFrame,
        depth_image_index: usize,
    ) -> Result<(), String> {
        self.store.record_seen();
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.store.record_drop();
            return Ok(());
        }

        let generation = self.next_generation;
        self.next_generation += 1;

        let job_result = (|| {
            let width = render_targets.depth_width;
            let height = render_targets.depth_height;
            if width == 0 || height == 0 {
                return Err("OpenXR depth voxel readback dimensions are zero".to_string());
            }
            let world_from_depth_view = frame.eyes[DEPTH_VOXEL_EYE_INDEX].depth_view_mat.invert();
            let camera_world = world_from_depth_view.transform_vec4(vec4f(0.0, 0.0, 0.0, 1.0));
            if !camera_world.w.is_finite() || camera_world.w.abs() < 1.0e-6 {
                return Err("OpenXR depth voxel camera transform is invalid".to_string());
            }
            let depth = vulkan.read_openxr_depth_image(
                render_targets,
                depth_image_index,
                DEPTH_VOXEL_EYE_INDEX,
            )?;
            Ok(CxOpenXrDepthVoxelJob {
                generation,
                eye_index: DEPTH_VOXEL_EYE_INDEX,
                width,
                height,
                sample_step: DEPTH_VOXEL_SAMPLE_STEP,
                camera_world: vec3(
                    camera_world.x / camera_world.w,
                    camera_world.y / camera_world.w,
                    camera_world.z / camera_world.w,
                ),
                inv_depth_proj: frame.eyes[DEPTH_VOXEL_EYE_INDEX].depth_proj_mat.invert(),
                world_from_depth_view,
                depth,
            })
        })();

        let job = match job_result {
            Ok(job) => job,
            Err(err) => {
                self.busy.store(false, Ordering::Release);
                self.store.set_error(err.clone());
                return Err(err);
            }
        };

        if let Err(err) = self.sender.send(job) {
            let err = format!("OpenXR depth voxel worker is unavailable: {err}");
            self.busy.store(false, Ordering::Release);
            self.store.set_error(err.clone());
            return Err(err);
        }

        Ok(())
    }
}

impl Cx {
    pub(crate) fn openxr_draw_pass_to_vulkan(
        &mut self,
        draw_pass_id: DrawPassId,
        frame: &CxOpenXrFrame,
    ) -> Result<(), String> {
        let draw_list_id = self.passes[draw_pass_id]
            .main_draw_list_id
            .ok_or_else(|| "OpenXR Vulkan render failed: missing main draw list".to_string())?;
        let dpi_factor = self.passes[draw_pass_id]
            .dpi_factor
            .ok_or_else(|| "OpenXR Vulkan render failed: missing dpi factor".to_string())?;
        let color_image_index = frame.swap_chain_index as usize;
        let depth_image_index = frame
            .depth_image
            .map(|image| image.swapchain_index as usize);
        let zero_mat = Mat4f { v: [0.0; 16] };
        let render_targets =
            {
                let session =
                    self.os.openxr.session.as_ref().ok_or_else(|| {
                        "OpenXR Vulkan render failed: missing session".to_string()
                    })?;
                let vulkan_session = session.vulkan.as_ref().ok_or_else(|| {
                    "OpenXR Vulkan render failed: missing Vulkan session".to_string()
                })?;
                &vulkan_session.render_targets as *const CxVulkanOpenXrSessionData
            };

        for eye in 0..2 {
            let pass = &mut self.passes[draw_pass_id];
            pass.set_dpi_factor(dpi_factor);
            pass.paint_dirty = true;
            pass.os.shader_variant = SHADER_VARIANT_XR;
            pass.pass_uniforms.camera_projection = frame.eyes[eye].proj_mat;
            pass.pass_uniforms.camera_view = frame.eyes[eye].view_mat;
            pass.pass_uniforms.camera_projection_r = frame.eyes[eye].proj_mat;
            pass.pass_uniforms.camera_view_r = frame.eyes[eye].view_mat;
            if depth_image_index.is_some() {
                pass.pass_uniforms.depth_projection = frame.eyes[eye].depth_proj_mat;
                pass.pass_uniforms.depth_view = frame.eyes[eye].depth_view_mat;
            } else {
                pass.pass_uniforms.depth_projection = zero_mat;
                pass.pass_uniforms.depth_view = zero_mat;
            }
            pass.pass_uniforms.depth_projection_r = pass.pass_uniforms.depth_projection;
            pass.pass_uniforms.depth_view_r = pass.pass_uniforms.depth_view;

            let mut vulkan =
                self.os.vulkan.take().ok_or_else(|| {
                    "OpenXR Vulkan render failed: backend unavailable".to_string()
                })?;
            let result = unsafe {
                vulkan.draw_openxr_view(
                    self,
                    draw_pass_id,
                    draw_list_id,
                    &*render_targets,
                    color_image_index,
                    eye,
                    depth_image_index,
                )
            };
            self.os.vulkan = Some(vulkan);
            result?;
        }
        Ok(())
    }
}

impl CxOpenXrVulkanSession {
    pub(crate) fn submit_depth_voxel_job(
        &mut self,
        vulkan: &mut CxVulkan,
        frame: &CxOpenXrFrame,
        depth_image_index: usize,
    ) -> Result<(), String> {
        self.depth_voxel_pipeline
            .submit(vulkan, &self.render_targets, frame, depth_image_index)
    }
}

impl CxOpenXrSession {
    pub(super) fn create_session_vulkan(
        xr: &LibOpenXr,
        system_id: XrSystemId,
        instance: XrInstance,
        vulkan: &mut CxVulkan,
        options: CxOpenXrOptions,
    ) -> Result<CxOpenXrSession, String> {
        let mut graphics_requirements = XrGraphicsRequirementsVulkanKHR::default();
        unsafe {
            (xr.xrGetVulkanGraphicsRequirements2KHR)(
                instance,
                system_id,
                &mut graphics_requirements,
            )
        }
        .to_result("xrGetVulkanGraphicsRequirements2KHR")?;

        let vk_instance = (vulkan.instance_handle().as_raw() as usize) as *const std::ffi::c_void;
        let vk_physical_device =
            (vulkan.physical_device_handle().as_raw() as usize) as *const std::ffi::c_void;
        let vk_device = (vulkan.device_handle().as_raw() as usize) as *const std::ffi::c_void;

        let mut runtime_physical_device = ptr::null();
        let get_info = XrVulkanGraphicsDeviceGetInfoKHR {
            system_id,
            vulkan_instance: vk_instance,
            ..Default::default()
        };
        unsafe {
            (xr.xrGetVulkanGraphicsDevice2KHR)(instance, &get_info, &mut runtime_physical_device)
        }
        .to_result("xrGetVulkanGraphicsDevice2KHR")?;
        if runtime_physical_device != vk_physical_device {
            return Err(format!(
                "OpenXR Vulkan device mismatch: runtime returned {:?}, renderer uses {:?}",
                runtime_physical_device, vk_physical_device
            ));
        }

        let gfx_binding = XrGraphicsBindingVulkanKHR {
            ty: XrStructureType::GRAPHICS_BINDING_VULKAN_KHR,
            next: ptr::null(),
            instance: vk_instance,
            physical_device: vk_physical_device,
            device: vk_device,
            queue_family_index: vulkan.queue_family_index(),
            queue_index: 0,
        };
        let session_create = XrSessionCreateInfo {
            ty: XrStructureType::SESSION_CREATE_INFO,
            next: &gfx_binding as *const _ as *const _,
            create_flags: XrSessionCreateFlags(0),
            system_id,
        };
        let mut session = XrSession(0);
        unsafe { (xr.xrCreateSession)(instance, &session_create, &mut session) }
            .to_result("xrCreateSession")?;

        let (head_space, local_space, width, height) =
            Self::describe_primary_stereo_session(xr, instance, system_id, session, options)?;

        let swapchain_formats = xr_array_fetch(0i64, |cap, len, buf| {
            unsafe { (xr.xrEnumerateSwapchainFormats)(session, cap, len, buf) }
                .to_result("xrEnumerateSwapchainFormats")
        })?;
        let color_format = vulkan.swapchain_format();
        let color_format_raw = i64::from(color_format.as_raw());
        if !swapchain_formats.contains(&color_format_raw) {
            return Err(format!(
                "OpenXR Vulkan swapchain format {:?} not supported by runtime: {:?}",
                color_format, swapchain_formats
            ));
        }

        let swap_chain_create_info = XrSwapchainCreateInfo {
            usage_flags: XrSwapchainUsageFlags::SAMPLED | XrSwapchainUsageFlags::COLOR_ATTACHMENT,
            format: color_format_raw,
            width,
            height,
            sample_count: 1,
            face_count: 1,
            array_size: 2,
            mip_count: 1,
            ..Default::default()
        };
        let mut color_swap_chain = XrSwapchain(0);
        unsafe { (xr.xrCreateSwapchain)(session, &swap_chain_create_info, &mut color_swap_chain) }
            .to_result("xrCreateSwapchain")?;

        let color_images =
            xr_array_fetch(XrSwapchainImageVulkanKHR::default(), |cap, len, buf| {
                unsafe {
                    (xr.xrEnumerateSwapchainImages)(
                        color_swap_chain,
                        cap,
                        len,
                        buf as *mut std::ffi::c_void,
                    )
                }
                .to_result("xrEnumerateSwapchainImages")
            })?;

        let (passthrough, passthrough_layer, depth_provider, depth_swap_chain) =
            Self::create_passthrough_and_depth(xr, session, options)?;

        let mut depth_swapchain_state = XrEnvironmentDepthSwapchainStateMETA {
            ty: XrStructureType::ENVIRONMENT_DEPTH_SWAPCHAIN_STATE_META,
            next: 0 as *mut _,
            width: 0,
            height: 0,
        };
        unsafe {
            (xr.xrGetEnvironmentDepthSwapchainStateMETA)(
                depth_swap_chain,
                &mut depth_swapchain_state,
            )
        }
        .to_result("xrGetEnvironmentDepthSwapchainStateMETA")?;

        let depth_images =
            xr_array_fetch(XrSwapchainImageVulkanKHR::default(), |cap, len, buf| {
                unsafe {
                    (xr.xrEnumerateEnvironmentDepthSwapchainImagesMETA)(
                        depth_swap_chain,
                        cap,
                        len,
                        buf as *mut std::ffi::c_void,
                    )
                }
                .to_result("xrEnumerateEnvironmentDepthSwapchainImagesMETA")
            })?;

        let color_vk_images: Vec<vk::Image> = color_images
            .iter()
            .map(|image| vk::Image::from_raw(image.image))
            .collect();
        let depth_vk_images: Vec<vk::Image> = depth_images
            .iter()
            .map(|image| vk::Image::from_raw(image.image))
            .collect();
        let render_targets = vulkan.create_openxr_session_data(
            &color_vk_images,
            &depth_vk_images,
            color_format,
            width,
            height,
            depth_swapchain_state.width,
            depth_swapchain_state.height,
        )?;

        unsafe { (xr.xrStartEnvironmentDepthProviderMETA)(depth_provider) }
            .to_result("xrStartEnvironmentDepthProviderMETA")?;
        let inputs = CxOpenXrInputs::new_inputs(xr, session, instance)?;

        Ok(CxOpenXrSession {
            order_counter: 0,
            color_images: Vec::new(),
            depth_images: Vec::new(),
            gl_depth_textures: Vec::new(),
            gl_frame_buffers: Vec::new(),
            vulkan: Some(CxOpenXrVulkanSession {
                _color_images: color_images,
                _depth_images: depth_images,
                render_targets,
                depth_voxel_pipeline: CxOpenXrDepthVoxelPipeline::new(),
            }),
            color_swap_chain,
            depth_swap_chain,
            depth_provider,
            passthrough,
            passthrough_layer,
            width,
            height,
            handle: session,
            head_space,
            local_space,
            active: false,
            anchor: CxOpenXrAnchor::default(),
            depth_swap_chain_index: 0,
            frame_state: XrFrameState::default(),
            inputs,
        })
    }

    pub(super) fn destroy_session_vulkan(&mut self, vulkan: Option<&mut CxVulkan>) {
        if let Some(vulkan_session) = self.vulkan.take() {
            if let Some(vulkan) = vulkan {
                vulkan.destroy_openxr_session_data(vulkan_session.render_targets);
            } else {
                crate::error!(
                    "OpenXR destroy lost Vulkan backend before XR resources were released"
                );
            }
        }
        xr_depth_voxels_store().clear();
    }
}

fn depth_voxel_worker(
    receiver: Receiver<CxOpenXrDepthVoxelJob>,
    busy: Arc<AtomicBool>,
    store: XrDepthVoxelsStore,
    volume: Arc<RwLock<XrDepthVoxels>>,
) {
    while let Ok(job) = receiver.recv() {
        if let Err(err) = integrate_depth_voxels(job, &volume) {
            store.set_error(err);
        } else if let Ok(volume) = volume.read() {
            store.record_integrated(&volume);
        } else {
            store.set_error("OpenXR depth voxel volume lock was poisoned".to_string());
        }
        busy.store(false, Ordering::Release);
        SignalToUI::set_ui_signal();
    }
}

fn integrate_depth_voxels(
    job: CxOpenXrDepthVoxelJob,
    volume: &Arc<RwLock<XrDepthVoxels>>,
) -> Result<(), String> {
    let grid_width = ((job.width.saturating_sub(1)) / job.sample_step + 1) as usize;
    let grid_height = ((job.height.saturating_sub(1)) / job.sample_step + 1) as usize;
    let mut deltas = HashMap::<IVector, i16>::new();

    let mut volume = volume
        .write()
        .map_err(|_| "OpenXR depth voxel volume lock was poisoned".to_string())?;
    volume.generation = job.generation;
    volume.eye_index = job.eye_index;
    volume.image_width = job.width;
    volume.image_height = job.height;
    volume.sample_step = job.sample_step;

    for grid_y in 0..grid_height {
        let src_y = ((grid_y as u32) * job.sample_step).min(job.height.saturating_sub(1));
        for grid_x in 0..grid_width {
            let src_x = ((grid_x as u32) * job.sample_step).min(job.width.saturating_sub(1));
            let Some(world) = depth_pixel_to_world(&job, src_x, src_y) else {
                continue;
            };
            accumulate_ray_deltas(&volume, job.camera_world, world, &mut deltas);
            if let Some(normal) = depth_pixel_to_world_normal(&job, src_x, src_y, world) {
                accumulate_plane_sample(&mut volume, job.generation, world, normal);
            }
        }
    }

    let topology_changes = apply_voxel_deltas(&mut volume, deltas);
    if topology_changes != 0 {
        volume.latest_topology_generation = job.generation;
        volume.pending_physics_changes += topology_changes;
    }

    if volume.pending_physics_changes != 0 && should_rebuild_physics(&volume) {
        prune_plane_accumulators(&mut volume);
        let physics_chunks = build_physics_snapshot(&volume);
        volume.physics_box_count = physics_chunks.iter().map(|chunk| chunk.boxes.len()).sum();
        volume.physics_chunk_count = physics_chunks.len();
        volume.physics_generation = job.generation;
        volume.physics_chunks = physics_chunks;
        volume.pending_physics_changes = 0;
        volume.last_physics_rebuild_at = std::time::Instant::now();
    }
    volume.update_bounds();
    Ok(())
}

fn depth_pixel_to_world(job: &CxOpenXrDepthVoxelJob, x: u32, y: u32) -> Option<Vec3f> {
    let width = job.width as usize;
    let raw_depth = *job.depth.get(y as usize * width + x as usize)?;
    let depth = raw_depth as f32 / u16::MAX as f32;
    if !(DEPTH_VOXEL_MIN_DEPTH_VALUE..DEPTH_VOXEL_MAX_DEPTH_VALUE).contains(&depth) {
        return None;
    }

    let uv_x = (x as f32 + 0.5) / job.width as f32;
    let uv_y = (y as f32 + 0.5) / job.height as f32;
    let clip = vec4f(uv_x * 2.0 - 1.0, uv_y * 2.0 - 1.0, depth * 2.0 - 1.0, 1.0);
    let view = job.inv_depth_proj.transform_vec4(clip);
    if !view.w.is_finite() || view.w.abs() < 1.0e-6 {
        return None;
    }

    let view = vec4f(view.x / view.w, view.y / view.w, view.z / view.w, 1.0);
    let distance = view.to_vec3f().length();
    if !distance.is_finite()
        || !(DEPTH_VOXEL_MIN_DISTANCE_METERS..=DEPTH_VOXEL_MAX_DISTANCE_METERS).contains(&distance)
    {
        return None;
    }

    let world = job.world_from_depth_view.transform_vec4(view);
    if !world.w.is_finite() || world.w.abs() < 1.0e-6 {
        return None;
    }
    let inv_w = 1.0 / world.w;
    let world = vec3(world.x * inv_w, world.y * inv_w, world.z * inv_w);
    if world.x.is_finite() && world.y.is_finite() && world.z.is_finite() {
        Some(world)
    } else {
        None
    }
}

fn depth_pixel_to_world_normal(
    job: &CxOpenXrDepthVoxelJob,
    x: u32,
    y: u32,
    world: Vec3f,
) -> Option<Vec3f> {
    let step = job.sample_step.max(1);
    let next_x = (x + step).min(job.width.saturating_sub(1));
    let next_y = (y + step).min(job.height.saturating_sub(1));
    let prev_x = x.saturating_sub(step);
    let prev_y = y.saturating_sub(step);

    let sample_x = if next_x != x {
        depth_pixel_to_world(job, next_x, y)
    } else if prev_x != x {
        depth_pixel_to_world(job, prev_x, y)
    } else {
        None
    }?;
    let sample_y = if next_y != y {
        depth_pixel_to_world(job, x, next_y)
    } else if prev_y != y {
        depth_pixel_to_world(job, x, prev_y)
    } else {
        None
    }?;

    let tangent_x = sample_x - world;
    let tangent_y = sample_y - world;
    if tangent_x.length() <= 1.0e-4 || tangent_y.length() <= 1.0e-4 {
        return None;
    }

    let mut normal = Vec3f::cross(tangent_x, tangent_y).normalize();
    if normal.length() <= 1.0e-4 {
        return None;
    }

    let view_dir = (job.camera_world - world).normalize();
    if normal.dot(view_dir) < 0.0 {
        normal = normal.scale(-1.0);
    }
    Some(normal)
}

fn accumulate_ray_deltas(
    volume: &XrDepthVoxels,
    camera_world: Vec3f,
    hit_world: Vec3f,
    deltas: &mut HashMap<IVector, i16>,
) {
    let hit_key = voxel_key_at_world(volume, hit_world);
    *deltas.entry(hit_key).or_default() += DEPTH_VOXEL_HIT_WEIGHT;

    let ray = hit_world - camera_world;
    let distance = ray.length();
    if distance <= volume.voxel_size_meters {
        return;
    }

    let dir = ray * (1.0 / distance);
    let mut last_key = hit_key;
    let mut t = volume.voxel_size_meters * 0.5;
    let t_end = (distance - volume.voxel_size_meters * 0.5).max(0.0);
    let t_step = (volume.voxel_size_meters * DEPTH_VOXEL_RAY_STEP_SCALE)
        .max(volume.voxel_size_meters * 0.25);

    while t < t_end {
        let sample = camera_world + dir * t;
        let sample_key = voxel_key_at_world(volume, sample);
        if sample_key != hit_key && sample_key != last_key {
            *deltas.entry(sample_key).or_default() -= DEPTH_VOXEL_MISS_WEIGHT;
            last_key = sample_key;
        }
        t += t_step;
    }
}

fn accumulate_plane_sample(
    volume: &mut XrDepthVoxels,
    generation: u64,
    hit_world: Vec3f,
    normal: Vec3f,
) {
    let sample = SurfaceSample {
        point: hit_world,
        normal,
    };
    let Some(bucket_key) = plane_bucket_for_sample(sample) else {
        return;
    };
    let plane_key = XrDepthPlaneKey {
        family: bucket_key.family,
        orientation_bin: bucket_key.orientation_bin,
        distance_bin: bucket_key.distance_bin,
    };
    let (_, _plane_normal, axis_u, axis_v) = plane_basis(plane_key);
    let cell_u = (axis_u.dot(hit_world) / DEPTH_PLANE_CELL_SIZE_METERS).round() as i16;
    let cell_v = (axis_v.dot(hit_world) / DEPTH_PLANE_CELL_SIZE_METERS).round() as i16;

    let plane = volume.planes.entry(plane_key).or_default();
    plane.total_support = plane.total_support.saturating_add(1);
    plane.last_seen_generation = generation;

    let cell = plane.cells.entry((cell_u, cell_v)).or_default();
    cell.support = cell.support.saturating_add(1).min(DEPTH_PLANE_MAX_CELL_SUPPORT);
    cell.last_seen_generation = generation;

    let voxel_key = voxel_key_at_world(volume, hit_world);
    let (chunk_key, id_in_chunk) = parry3d::shape::Voxels::chunk_key_and_id_in_chunk(voxel_key);
    let chunk_id = {
        let (chunk_header, _) = volume
            .surfaces
            .header_or_insert_with(chunk_key, XrDepthSurfaceChunk::default);
        chunk_header.id
    };
    let surface_chunk = &mut volume.surfaces.chunks[chunk_id];
    let sample_count = &mut surface_chunk.sample_count[id_in_chunk];
    if *sample_count >= DEPTH_SURFACE_SAMPLE_MAX_COUNT {
        surface_chunk.point_sum[id_in_chunk] = surface_chunk.point_sum[id_in_chunk].scale(0.5);
        surface_chunk.normal_sum[id_in_chunk] = surface_chunk.normal_sum[id_in_chunk].scale(0.5);
        *sample_count = (*sample_count / 2).max(1);
    }
    surface_chunk.point_sum[id_in_chunk] = surface_chunk.point_sum[id_in_chunk] + hit_world;
    surface_chunk.normal_sum[id_in_chunk] = surface_chunk.normal_sum[id_in_chunk] + normal;
    *sample_count += 1;
}

fn prune_plane_accumulators(volume: &mut XrDepthVoxels) {
    let generation = volume.generation;
    volume.planes.retain(|_, plane| {
        plane.cells.retain(|_, cell| {
            generation.saturating_sub(cell.last_seen_generation) <= DEPTH_PLANE_STALE_GENERATIONS
                || cell.support >= DEPTH_PLANE_MIN_CELL_SUPPORT
        });
        !plane.cells.is_empty()
            && (generation.saturating_sub(plane.last_seen_generation) <= DEPTH_PLANE_STALE_GENERATIONS
                || plane.total_support >= DEPTH_PLANE_STALE_SUPPORT_KEEP)
    });
}

fn apply_voxel_deltas(volume: &mut XrDepthVoxels, deltas: HashMap<IVector, i16>) -> usize {
    let mut topology_changes = 0;
    for (voxel_key, delta) in deltas {
        let (chunk_key, id_in_chunk) = parry3d::shape::Voxels::chunk_key_and_id_in_chunk(voxel_key);
        let evidence = {
            let chunk_id = {
                let (chunk_header, _) = volume
                    .evidence
                    .header_or_insert_with(chunk_key, XrDepthEvidenceChunk::default);
                chunk_header.id
            };
            let evidence = &mut volume.evidence.chunks[chunk_id].evidence[id_in_chunk];
            *evidence =
                (*evidence + delta).clamp(-DEPTH_VOXEL_EVIDENCE_MAX, DEPTH_VOXEL_EVIDENCE_MAX);
            *evidence
        };

        let is_filled = volume
            .voxels
            .voxel_state(voxel_key)
            .map(|state| !state.is_empty())
            .unwrap_or(false);

        if !is_filled && evidence >= volume.activation_threshold {
            let previous = volume.voxels.set_voxel(voxel_key, true);
            if previous.is_empty() {
                volume.active_voxel_count += 1;
                topology_changes += 1;
            }
        } else if is_filled && evidence <= volume.removal_threshold {
            let previous = volume.voxels.set_voxel(voxel_key, false);
            if !previous.is_empty() {
                volume.active_voxel_count = volume.active_voxel_count.saturating_sub(1);
                clear_surface_sample(volume, voxel_key);
                topology_changes += 1;
            }
        }
    }
    topology_changes
}

fn clear_surface_sample(volume: &mut XrDepthVoxels, voxel_key: IVector) {
    let (chunk_key, id_in_chunk) = parry3d::shape::Voxels::chunk_key_and_id_in_chunk(voxel_key);
    let Some(chunk_header) = volume.surfaces.chunk_headers.get(&chunk_key) else {
        return;
    };
    let surface_chunk = &mut volume.surfaces.chunks[chunk_header.id];
    surface_chunk.point_sum[id_in_chunk] = Vec3f::default();
    surface_chunk.normal_sum[id_in_chunk] = Vec3f::default();
    surface_chunk.sample_count[id_in_chunk] = 0;
}

fn voxel_key_at_world(volume: &XrDepthVoxels, world: Vec3f) -> IVector {
    volume
        .voxels
        .voxel_at_point(Vector::new(world.x, world.y, world.z))
}

fn voxel_neighbor_offsets() -> [IVector; 6] {
    [
        IVector::new(1, 0, 0),
        IVector::new(-1, 0, 0),
        IVector::new(0, 1, 0),
        IVector::new(0, -1, 0),
        IVector::new(0, 0, 1),
        IVector::new(0, 0, -1),
    ]
}

#[derive(Clone, Copy)]
struct GridBox {
    origin: IVector,
    dims: [i32; 3],
}

#[derive(Clone, Copy)]
struct SliceRect {
    min_u: i32,
    min_v: i32,
    size_u: i32,
    size_v: i32,
}

impl SliceRect {
    fn area(self) -> usize {
        (self.size_u * self.size_v).max(0) as usize
    }
}

#[derive(Clone, Copy)]
struct SurfaceSample {
    point: Vec3f,
    normal: Vec3f,
}

#[derive(Clone, Copy)]
struct ProjectedPlaneRect {
    min_u: f32,
    max_u: f32,
    min_v: f32,
    max_v: f32,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct PlaneBucketKey {
    family: u8,
    orientation_bin: i16,
    distance_bin: i16,
}

#[derive(Clone, Copy)]
enum PlaneFamily {
    Horizontal,
    Vertical,
}

fn should_rebuild_physics(volume: &XrDepthVoxels) -> bool {
    volume.physics_generation == 0
        || volume.pending_physics_changes >= DEPTH_PHYSICS_PENDING_CHANGE_THRESHOLD
        || volume.last_physics_rebuild_at.elapsed().as_millis()
            >= DEPTH_PHYSICS_REBUILD_INTERVAL_MS as u128
}

fn build_physics_snapshot(volume: &XrDepthVoxels) -> Vec<XrDepthPhysicsChunk> {
    let mut world_boxes = build_plane_world_boxes(volume);
    if world_boxes.is_empty() {
        return Vec::new();
    }

    world_boxes.sort_by(|a, b| {
        let a_volume = a.half_extents.x * a.half_extents.y * a.half_extents.z;
        let b_volume = b.half_extents.x * b.half_extents.y * b.half_extents.z;
        b_volume
            .partial_cmp(&a_volume)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut shapes = Vec::with_capacity(world_boxes.len());
    for world_box in &world_boxes {
        shapes.push((
            ParryPose::from_parts(
                Vector::new(
                    world_box.pose.position.x,
                    world_box.pose.position.y,
                    world_box.pose.position.z,
                ),
                parry3d::math::Rotation::from_axis_angle(
                    Vector::Y,
                    quat_yaw(world_box.pose.orientation),
                ),
            ),
            SharedShape::cuboid(
                world_box.half_extents.x,
                world_box.half_extents.y,
                world_box.half_extents.z,
            ),
        ));
    }

    vec![XrDepthPhysicsChunk {
        key: XrDepthPhysicsChunkKey { x: 0, y: 0, z: 0 },
        boxes: world_boxes,
        shape: SharedShape::compound(shapes),
    }]
}

fn physics_chunk_key_for_voxel(voxel: IVector) -> XrDepthPhysicsChunkKey {
    XrDepthPhysicsChunkKey {
        x: div_floor_i32(voxel.x, DEPTH_PHYSICS_CHUNK_SIZE),
        y: div_floor_i32(voxel.y, DEPTH_PHYSICS_CHUNK_SIZE),
        z: div_floor_i32(voxel.z, DEPTH_PHYSICS_CHUNK_SIZE),
    }
}

fn physics_chunk_origin(key: XrDepthPhysicsChunkKey) -> IVector {
    IVector::new(
        key.x * DEPTH_PHYSICS_CHUNK_SIZE,
        key.y * DEPTH_PHYSICS_CHUNK_SIZE,
        key.z * DEPTH_PHYSICS_CHUNK_SIZE,
    )
}

fn build_plane_world_boxes(volume: &XrDepthVoxels) -> Vec<XrDepthPhysicsBox> {
    let mut planes: Vec<_> = volume.planes.iter().collect();
    planes.sort_by(|(_, a), (_, b)| b.total_support.cmp(&a.total_support));

    let mut boxes = Vec::new();
    for (plane_key, plane) in planes.into_iter().take(DEPTH_PLANE_MAX_PHYSICS_PLANES) {
        if plane.total_support < DEPTH_PLANE_MIN_TOTAL_SUPPORT {
            continue;
        }
        boxes.extend(extract_plane_boxes_from_accumulator(*plane_key, plane));
        if boxes.len() >= DEPTH_PLANE_MAX_PHYSICS_PLANES * DEPTH_PLANE_MAX_BOXES_PER_PLANE {
            break;
        }
    }
    boxes
}

fn extract_plane_boxes_from_accumulator(
    plane_key: XrDepthPlaneKey,
    plane: &XrDepthPlaneAccumulator,
) -> Vec<XrDepthPhysicsBox> {
    let Some((min_u, max_u, min_v, max_v)) = plane_cell_bounds(plane) else {
        return Vec::new();
    };
    let width = (max_u - min_u + 1) as usize;
    let height = (max_v - min_v + 1) as usize;
    let mut mask = vec![false; width * height];
    for (&(u, v), cell) in &plane.cells {
        if cell.support < DEPTH_PLANE_MIN_CELL_SUPPORT {
            continue;
        }
        let x = (u - min_u) as usize;
        let y = (v - min_v) as usize;
        mask[y * width + x] = true;
    }

    let mut working = smooth_slice_mask(&mask, width, height);
    let mut boxes = Vec::new();
    let min_cells = ((DEPTH_PLANE_MIN_SPAN_METERS / DEPTH_PLANE_CELL_SIZE_METERS).ceil() as i32).max(1);
    for _ in 0..DEPTH_PLANE_MAX_BOXES_PER_PLANE {
        let Some(rect) = largest_true_rect(&working, width, height) else {
            break;
        };
        if rect.size_u < min_cells || rect.size_v < min_cells {
            break;
        }
        let filled = bitmap_rect_filled_count(&mask, width, rect);
        if filled * DEPTH_PLANE_RECT_MIN_COVERAGE_DEN
            < rect.area() * DEPTH_PLANE_RECT_MIN_COVERAGE_NUM
        {
            clear_bitmap_rect(&mut working, width, rect);
            continue;
        }
        if let Some(physics_box) = plane_box_from_rect(plane_key, min_u, min_v, rect) {
            boxes.push(physics_box);
        }
        clear_bitmap_rect(&mut working, width, rect);
    }
    boxes
}

fn plane_cell_bounds(plane: &XrDepthPlaneAccumulator) -> Option<(i16, i16, i16, i16)> {
    let mut iter = plane
        .cells
        .iter()
        .filter(|(_, cell)| cell.support >= DEPTH_PLANE_MIN_CELL_SUPPORT);
    let (&(first_u, first_v), _) = iter.next()?;
    let mut min_u = first_u;
    let mut max_u = first_u;
    let mut min_v = first_v;
    let mut max_v = first_v;
    for (&(u, v), _) in iter {
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }
    Some((min_u, max_u, min_v, max_v))
}

fn plane_box_from_rect(
    plane_key: XrDepthPlaneKey,
    base_u: i16,
    base_v: i16,
    rect: SliceRect,
) -> Option<XrDepthPhysicsBox> {
    let (family, normal, axis_u, axis_v) = plane_basis(plane_key);
    let min_u = (base_u + rect.min_u as i16) as f32 * DEPTH_PLANE_CELL_SIZE_METERS
        - DEPTH_PLANE_CELL_SIZE_METERS * 0.5;
    let max_u = (base_u + (rect.min_u + rect.size_u) as i16) as f32 * DEPTH_PLANE_CELL_SIZE_METERS
        + DEPTH_PLANE_CELL_SIZE_METERS * 0.5;
    let min_v = (base_v + rect.min_v as i16) as f32 * DEPTH_PLANE_CELL_SIZE_METERS
        - DEPTH_PLANE_CELL_SIZE_METERS * 0.5;
    let max_v = (base_v + (rect.min_v + rect.size_v) as i16) as f32 * DEPTH_PLANE_CELL_SIZE_METERS
        + DEPTH_PLANE_CELL_SIZE_METERS * 0.5;

    let span_u = max_u - min_u;
    let span_v = max_v - min_v;
    if span_u < DEPTH_PLANE_MIN_SPAN_METERS || span_v < DEPTH_PLANE_MIN_SPAN_METERS {
        return None;
    }

    let plane_distance = plane_key.distance_bin as f32 * DEPTH_PLANE_DISTANCE_BIN_METERS;
    let center = axis_u.scale((min_u + max_u) * 0.5)
        + axis_v.scale((min_v + max_v) * 0.5)
        + normal.scale(plane_distance);

    let pose = match family {
        PlaneFamily::Horizontal => Pose::new(Quat::default(), center),
        PlaneFamily::Vertical => Pose::new(Quat::look_rotation(normal, vec3f(0.0, 1.0, 0.0)), center),
    };
    let half_extents = match family {
        PlaneFamily::Horizontal => vec3f(span_u * 0.5, DEPTH_PLANE_THICKNESS_METERS * 0.5, span_v * 0.5),
        PlaneFamily::Vertical => vec3f(span_u * 0.5, span_v * 0.5, DEPTH_PLANE_THICKNESS_METERS * 0.5),
    };

    Some(XrDepthPhysicsBox { pose, half_extents })
}

fn plane_basis(plane_key: XrDepthPlaneKey) -> (PlaneFamily, Vec3f, Vec3f, Vec3f) {
    let (family, normal) = plane_bucket_normal(PlaneBucketKey {
        family: plane_key.family,
        orientation_bin: plane_key.orientation_bin,
        distance_bin: plane_key.distance_bin,
    });
    match family {
        PlaneFamily::Horizontal => (
            family,
            normal,
            vec3f(1.0, 0.0, 0.0),
            vec3f(0.0, 0.0, 1.0),
        ),
        PlaneFamily::Vertical => {
            let up = vec3f(0.0, 1.0, 0.0);
            let axis_u = Vec3f::cross(up, normal).normalize();
            (family, normal, axis_u, up)
        }
    }
}

fn clear_bitmap_rect(mask: &mut [bool], width: usize, rect: SliceRect) {
    for v in rect.min_v..rect.min_v + rect.size_v {
        for u in rect.min_u..rect.min_u + rect.size_u {
            mask[v as usize * width + u as usize] = false;
        }
    }
}

fn extract_plane_boxes(surface_samples: &[SurfaceSample], voxel_size_meters: f32) -> Vec<XrDepthPhysicsBox> {
    let mut remaining = surface_samples.to_vec();
    let mut boxes = Vec::new();

    for _ in 0..DEPTH_PLANE_MAX_PER_CHUNK {
        let Some(bucket_key) = dominant_plane_bucket(&remaining) else {
            break;
        };
        let (family, bucket_normal) = plane_bucket_normal(bucket_key);
        let (plane_normal, plane_distance) =
            refine_plane_from_bucket(&remaining, bucket_key, family, bucket_normal);

        let mut inliers = Vec::new();
        remaining.retain(|sample| {
            if is_plane_inlier(*sample, plane_normal, plane_distance) {
                inliers.push(*sample);
                false
            } else {
                true
            }
        });

        if inliers.len() < DEPTH_PLANE_MIN_SUPPORT {
            break;
        }
        if let Some(physics_box) =
            build_plane_box(&inliers, plane_normal, plane_distance, family, voxel_size_meters)
        {
            boxes.push(physics_box);
        }
    }

    boxes
}

fn dominant_plane_bucket(surface_samples: &[SurfaceSample]) -> Option<PlaneBucketKey> {
    let mut counts = HashMap::<PlaneBucketKey, usize>::new();
    for sample in surface_samples {
        let Some(bucket_key) = plane_bucket_for_sample(*sample) else {
            continue;
        };
        *counts.entry(bucket_key).or_default() += 1;
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .and_then(|(bucket_key, count)| {
            if count >= DEPTH_PLANE_MIN_SUPPORT {
                Some(bucket_key)
            } else {
                None
            }
        })
}

fn plane_bucket_for_sample(sample: SurfaceSample) -> Option<PlaneBucketKey> {
    let normal = sample.normal.normalize();
    if normal.length() <= 1.0e-4 {
        return None;
    }

    let (family, orientation_bin, bucket_normal) = if normal.y.abs() >= DEPTH_PLANE_HORIZONTAL_NORMAL_DOT {
        let sign = if normal.y >= 0.0 { 1 } else { -1 };
        (0u8, sign, vec3f(0.0, sign as f32, 0.0))
    } else if normal.y.abs() <= DEPTH_PLANE_VERTICAL_NORMAL_DOT_MAX {
        let horizontal = vec3f(normal.x, 0.0, normal.z).normalize();
        if horizontal.length() <= 1.0e-4 {
            return None;
        }
        let yaw = horizontal.z.atan2(horizontal.x);
        let wrapped = (yaw + std::f32::consts::PI) / (std::f32::consts::TAU);
        let orientation_bin =
            ((wrapped * DEPTH_PLANE_VERTICAL_BINS as f32).floor() as i16)
                .rem_euclid(DEPTH_PLANE_VERTICAL_BINS as i16);
        (1u8, orientation_bin, plane_bucket_normal(PlaneBucketKey {
            family: 1,
            orientation_bin,
            distance_bin: 0,
        }).1)
    } else {
        return None;
    };

    let distance =
        bucket_normal.dot(sample.point) / DEPTH_PLANE_DISTANCE_BIN_METERS;
    Some(PlaneBucketKey {
        family,
        orientation_bin,
        distance_bin: distance.round() as i16,
    })
}

fn plane_bucket_normal(bucket_key: PlaneBucketKey) -> (PlaneFamily, Vec3f) {
    if bucket_key.family == 0 {
        let sign = if bucket_key.orientation_bin >= 0 { 1.0 } else { -1.0 };
        (PlaneFamily::Horizontal, vec3f(0.0, sign, 0.0))
    } else {
        let yaw = ((bucket_key.orientation_bin as f32 + 0.5)
            / DEPTH_PLANE_VERTICAL_BINS as f32)
            * std::f32::consts::TAU
            - std::f32::consts::PI;
        (
            PlaneFamily::Vertical,
            vec3f(yaw.cos(), 0.0, yaw.sin()).normalize(),
        )
    }
}

fn refine_plane_from_bucket(
    surface_samples: &[SurfaceSample],
    bucket_key: PlaneBucketKey,
    family: PlaneFamily,
    bucket_normal: Vec3f,
) -> (Vec3f, f32) {
    let mut normal_sum = Vec3f::default();
    let mut distance_sum = 0.0;
    let mut count = 0usize;

    for sample in surface_samples {
        if plane_bucket_for_sample(*sample) != Some(bucket_key) {
            continue;
        }
        normal_sum = normal_sum + sample.normal;
        distance_sum += bucket_normal.dot(sample.point);
        count += 1;
    }

    if count == 0 {
        return (bucket_normal, 0.0);
    }

    let refined_normal = match family {
        PlaneFamily::Horizontal => vec3f(0.0, if normal_sum.y >= 0.0 { 1.0 } else { -1.0 }, 0.0),
        PlaneFamily::Vertical => {
            let horizontal = vec3f(normal_sum.x, 0.0, normal_sum.z).normalize();
            if horizontal.length() > 1.0e-4 {
                horizontal
            } else {
                bucket_normal
            }
        }
    };
    let plane_distance = distance_sum / count as f32;
    (refined_normal, plane_distance)
}

fn is_plane_inlier(sample: SurfaceSample, plane_normal: Vec3f, plane_distance: f32) -> bool {
    sample.normal.normalize().dot(plane_normal) >= DEPTH_PLANE_NORMAL_TOLERANCE_DOT
        && (plane_normal.dot(sample.point) - plane_distance).abs()
            <= DEPTH_PLANE_DISTANCE_TOLERANCE_METERS
}

fn build_plane_box(
    inliers: &[SurfaceSample],
    plane_normal: Vec3f,
    _plane_distance: f32,
    family: PlaneFamily,
    voxel_size_meters: f32,
) -> Option<XrDepthPhysicsBox> {
    match family {
        PlaneFamily::Horizontal => build_horizontal_plane_box(inliers, voxel_size_meters),
        PlaneFamily::Vertical => build_vertical_plane_box(inliers, plane_normal, voxel_size_meters),
    }
}

fn build_horizontal_plane_box(
    inliers: &[SurfaceSample],
    voxel_size_meters: f32,
) -> Option<XrDepthPhysicsBox> {
    let up = vec3f(0.0, 1.0, 0.0);
    let axis_x = principal_horizontal_axis(inliers);
    let axis_z = Vec3f::cross(axis_x, up).normalize();
    if axis_x.length() <= 1.0e-4 || axis_z.length() <= 1.0e-4 {
        return None;
    }

    let rect = largest_projected_plane_rect(inliers, axis_x, axis_z, voxel_size_meters)?;
    let mut center_y = 0.0;
    for sample in inliers {
        center_y += sample.point.y;
    }
    center_y /= inliers.len() as f32;

    let span_x = rect.max_u - rect.min_u;
    let span_z = rect.max_v - rect.min_v;
    if span_x < DEPTH_PLANE_MIN_SPAN_METERS || span_z < DEPTH_PLANE_MIN_SPAN_METERS {
        return None;
    }

    let yaw = axis_x.z.atan2(axis_x.x);
    let pose = Pose::new(
        Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), yaw),
        axis_x.scale((rect.min_u + rect.max_u) * 0.5)
            + axis_z.scale((rect.min_v + rect.max_v) * 0.5)
            + up.scale(center_y),
    );
    Some(XrDepthPhysicsBox {
        pose,
        half_extents: vec3f(
            span_x * 0.5,
            DEPTH_PLANE_THICKNESS_METERS * 0.5,
            span_z * 0.5,
        ),
    })
}

fn build_vertical_plane_box(
    inliers: &[SurfaceSample],
    plane_normal: Vec3f,
    voxel_size_meters: f32,
) -> Option<XrDepthPhysicsBox> {
    let forward = vec3f(plane_normal.x, 0.0, plane_normal.z).normalize();
    if forward.length() <= 1.0e-4 {
        return None;
    }
    let up = vec3f(0.0, 1.0, 0.0);
    let axis_x = Vec3f::cross(up, forward).normalize();
    if axis_x.length() <= 1.0e-4 {
        return None;
    }

    let rect = largest_projected_plane_rect(inliers, axis_x, up, voxel_size_meters)?;
    let mut center_z = 0.0;
    for sample in inliers {
        center_z += forward.dot(sample.point);
    }
    center_z /= inliers.len() as f32;

    let span_x = rect.max_u - rect.min_u;
    let span_y = rect.max_v - rect.min_v;
    if span_x < DEPTH_PLANE_MIN_SPAN_METERS || span_y < DEPTH_PLANE_MIN_SPAN_METERS {
        return None;
    }

    let pose = Pose::new(
        Quat::look_rotation(forward, up),
        axis_x.scale((rect.min_u + rect.max_u) * 0.5)
            + up.scale((rect.min_v + rect.max_v) * 0.5)
            + forward.scale(center_z),
    );
    Some(XrDepthPhysicsBox {
        pose,
        half_extents: vec3f(
            span_x * 0.5,
            span_y * 0.5,
            DEPTH_PLANE_THICKNESS_METERS * 0.5,
        ),
    })
}

fn largest_projected_plane_rect(
    inliers: &[SurfaceSample],
    axis_u: Vec3f,
    axis_v: Vec3f,
    cell_size: f32,
) -> Option<ProjectedPlaneRect> {
    if inliers.is_empty() {
        return None;
    }

    let mut min_u = f32::INFINITY;
    let mut max_u = f32::NEG_INFINITY;
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;
    for sample in inliers {
        let u = axis_u.dot(sample.point);
        let v = axis_v.dot(sample.point);
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }
    if !min_u.is_finite() || !min_v.is_finite() {
        return None;
    }

    let span_u = max_u - min_u + cell_size;
    let span_v = max_v - min_v + cell_size;
    if span_u < DEPTH_PLANE_MIN_SPAN_METERS || span_v < DEPTH_PLANE_MIN_SPAN_METERS {
        return None;
    }

    let width = ((span_u / cell_size).ceil() as usize).max(1);
    let height = ((span_v / cell_size).ceil() as usize).max(1);
    let min_cells = ((DEPTH_PLANE_MIN_SPAN_METERS / cell_size).ceil() as i32).max(1);
    let mut mask = vec![false; width * height];
    for sample in inliers {
        let u = (((axis_u.dot(sample.point) - min_u) / cell_size).floor() as i32)
            .clamp(0, width as i32 - 1);
        let v = (((axis_v.dot(sample.point) - min_v) / cell_size).floor() as i32)
            .clamp(0, height as i32 - 1);
        mask[v as usize * width + u as usize] = true;
    }

    let smoothed = smooth_slice_mask(&mask, width, height);
    let rect = largest_true_rect(&smoothed, width, height)?;
    if rect.size_u < min_cells || rect.size_v < min_cells {
        return None;
    }

    let filled = bitmap_rect_filled_count(&mask, width, rect);
    if filled * DEPTH_PLANE_RECT_MIN_COVERAGE_DEN
        < rect.area() * DEPTH_PLANE_RECT_MIN_COVERAGE_NUM
    {
        return None;
    }

    Some(ProjectedPlaneRect {
        min_u: min_u + rect.min_u as f32 * cell_size,
        max_u: min_u + (rect.min_u + rect.size_u) as f32 * cell_size,
        min_v: min_v + rect.min_v as f32 * cell_size,
        max_v: min_v + (rect.min_v + rect.size_v) as f32 * cell_size,
    })
}

fn bitmap_rect_filled_count(mask: &[bool], width: usize, rect: SliceRect) -> usize {
    let mut filled = 0;
    for v in rect.min_v..rect.min_v + rect.size_v {
        for u in rect.min_u..rect.min_u + rect.size_u {
            if mask[v as usize * width + u as usize] {
                filled += 1;
            }
        }
    }
    filled
}

fn principal_horizontal_axis(inliers: &[SurfaceSample]) -> Vec3f {
    if inliers.is_empty() {
        return vec3f(1.0, 0.0, 0.0);
    }
    let mut mean_x = 0.0;
    let mut mean_z = 0.0;
    for sample in inliers {
        mean_x += sample.point.x;
        mean_z += sample.point.z;
    }
    mean_x /= inliers.len() as f32;
    mean_z /= inliers.len() as f32;

    let mut xx = 0.0;
    let mut xz = 0.0;
    let mut zz = 0.0;
    for sample in inliers {
        let dx = sample.point.x - mean_x;
        let dz = sample.point.z - mean_z;
        xx += dx * dx;
        xz += dx * dz;
        zz += dz * dz;
    }

    let yaw = 0.5 * (2.0 * xz).atan2(xx - zz);
    let axis = vec3f(yaw.cos(), 0.0, yaw.sin()).normalize();
    if axis.length() > 1.0e-4 {
        axis
    } else {
        vec3f(1.0, 0.0, 0.0)
    }
}

fn quat_yaw(q: Quat) -> f32 {
    2.0 * q.y.atan2(q.w)
}

fn div_floor_i32(value: i32, divisor: i32) -> i32 {
    let mut result = value / divisor;
    let remainder = value % divisor;
    if remainder != 0 && ((remainder < 0) != (divisor < 0)) {
        result -= 1;
    }
    result
}

fn decompose_chunk_physics_boxes(occupied: &HashSet<IVector>) -> Vec<GridBox> {
    let mut remaining = occupied.clone();
    let mut boxes = Vec::new();

    for axis in [1usize, 0, 2] {
        extract_slab_boxes(axis, &mut remaining, &mut boxes);
    }

    boxes.extend(decompose_greedy_boxes(&remaining));
    boxes.sort_by_key(|grid_box| {
        (
            grid_box.origin.y,
            grid_box.origin.z,
            grid_box.origin.x,
            grid_box.dims[1],
            grid_box.dims[2],
            grid_box.dims[0],
        )
    });
    boxes
}

fn decompose_chunk_fallback_boxes(
    chunk_origin: IVector,
    occupied: &HashSet<IVector>,
    voxel_size: Vector,
) -> Vec<XrDepthPhysicsBox> {
    let stride = DEPTH_PHYSICS_FALLBACK_COARSE_STRIDE.max(1);
    let mut coarse_occupied = HashSet::new();
    for voxel in occupied {
        coarse_occupied.insert(IVector::new(
            voxel.x / stride,
            voxel.y / stride,
            voxel.z / stride,
        ));
    }

    let mut coarse_boxes = decompose_chunk_physics_boxes(&coarse_occupied);
    coarse_boxes.sort_by_key(|grid_box| {
        let volume = grid_box.dims[0] * grid_box.dims[1] * grid_box.dims[2];
        (-volume, grid_box.origin.y, grid_box.origin.z, grid_box.origin.x)
    });
    coarse_boxes.truncate(DEPTH_PHYSICS_FALLBACK_MAX_BOXES);

    let coarse_voxel_size = Vector::new(
        voxel_size.x * stride as f32,
        voxel_size.y * stride as f32,
        voxel_size.z * stride as f32,
    );

    coarse_boxes
        .into_iter()
        .map(|grid_box| {
            let coarse_seed = IVector::new(
                grid_box.origin.x * stride,
                grid_box.origin.y * stride,
                grid_box.origin.z * stride,
            );
            physics_box_from_grid_box(chunk_origin + coarse_seed, grid_box.dims, coarse_voxel_size)
        })
        .collect()
}

fn extract_slab_boxes(axis: usize, remaining: &mut HashSet<IVector>, boxes: &mut Vec<GridBox>) {
    loop {
        let mut best_candidate = None;
        let mut best_score = 0usize;

        for slice in 0..DEPTH_PHYSICS_CHUNK_SIZE {
            let Some(candidate) = best_slab_on_slice(axis, slice, remaining) else {
                continue;
            };
            let footprint = slab_footprint(candidate, axis);
            let volume = (candidate.dims[0] * candidate.dims[1] * candidate.dims[2]) as usize;
            let score = footprint * 8 + volume;
            if score > best_score {
                best_score = score;
                best_candidate = Some(candidate);
            }
        }

        let Some(candidate) = best_candidate else {
            break;
        };
        remove_grid_box(candidate, remaining);
        boxes.push(candidate);
    }
}

fn best_slab_on_slice(axis: usize, slice: i32, remaining: &HashSet<IVector>) -> Option<GridBox> {
    let width = DEPTH_PHYSICS_CHUNK_SIZE as usize;
    let height = DEPTH_PHYSICS_CHUNK_SIZE as usize;
    let mask = build_slice_mask(axis, slice, remaining);
    if mask.iter().filter(|&&filled| filled).count() < DEPTH_PHYSICS_SLAB_MIN_AREA {
        return None;
    }

    let smoothed = smooth_slice_mask(&mask, width, height);
    let rect = largest_true_rect(&smoothed, width, height)?;
    if rect.area() < DEPTH_PHYSICS_SLAB_MIN_AREA
        || rect.size_u < DEPTH_PHYSICS_SLAB_MIN_EDGE
        || rect.size_v < DEPTH_PHYSICS_SLAB_MIN_EDGE
        || !slice_rect_meets_coverage(axis, slice, rect, remaining)
    {
        return None;
    }

    let mut slice_min = slice;
    let mut slice_max = slice + 1;
    while slice_min > 0 && slice_max - slice_min < DEPTH_PHYSICS_SLAB_MAX_THICKNESS {
        let next_slice = slice_min - 1;
        if !slice_rect_meets_coverage(axis, next_slice, rect, remaining) {
            break;
        }
        slice_min = next_slice;
    }
    while slice_max < DEPTH_PHYSICS_CHUNK_SIZE
        && slice_max - slice_min < DEPTH_PHYSICS_SLAB_MAX_THICKNESS
    {
        if !slice_rect_meets_coverage(axis, slice_max, rect, remaining) {
            break;
        }
        slice_max += 1;
    }

    Some(grid_box_from_slab(
        axis,
        slice_min,
        slice_max - slice_min,
        rect,
    ))
}

fn slab_footprint(grid_box: GridBox, axis: usize) -> usize {
    let (u_axis, v_axis) = plane_axes(axis);
    (grid_box.dims[u_axis] * grid_box.dims[v_axis]) as usize
}

fn build_slice_mask(axis: usize, slice: i32, remaining: &HashSet<IVector>) -> Vec<bool> {
    let size = DEPTH_PHYSICS_CHUNK_SIZE as usize;
    let mut mask = vec![false; size * size];
    for u in 0..DEPTH_PHYSICS_CHUNK_SIZE {
        for v in 0..DEPTH_PHYSICS_CHUNK_SIZE {
            if remaining.contains(&slice_point(axis, slice, u, v)) {
                mask[u as usize * size + v as usize] = true;
            }
        }
    }
    mask
}

fn smooth_slice_mask(mask: &[bool], width: usize, height: usize) -> Vec<bool> {
    let mut smoothed = mask.to_vec();
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if mask[index] {
                continue;
            }
            let mut neighbors = 0;
            let mut left = false;
            let mut right = false;
            let mut up = false;
            let mut down = false;
            for oy in -1..=1 {
                for ox in -1..=1 {
                    if ox == 0 && oy == 0 {
                        continue;
                    }
                    let nx = x as i32 + ox;
                    let ny = y as i32 + oy;
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let filled = mask[ny as usize * width + nx as usize];
                    if filled {
                        neighbors += 1;
                        left |= ox == -1 && oy == 0;
                        right |= ox == 1 && oy == 0;
                        up |= ox == 0 && oy == -1;
                        down |= ox == 0 && oy == 1;
                    }
                }
            }
            if neighbors >= 5 || (left && right) || (up && down) {
                smoothed[index] = true;
            }
        }
    }
    smoothed
}

fn largest_true_rect(mask: &[bool], width: usize, height: usize) -> Option<SliceRect> {
    let mut heights = vec![0usize; width];
    let mut best_rect = None;
    let mut best_area = 0usize;

    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            heights[x] = if mask[index] { heights[x] + 1 } else { 0 };
        }

        let mut stack: Vec<(usize, usize)> = Vec::new();
        for x in 0..=width {
            let current_height = if x < width { heights[x] } else { 0 };
            let mut start = x;
            while let Some(&(prev_start, prev_height)) = stack.last() {
                if prev_height <= current_height {
                    break;
                }
                stack.pop();
                let rect_area = prev_height * (x - prev_start);
                if rect_area > best_area {
                    best_area = rect_area;
                    best_rect = Some(SliceRect {
                        min_u: prev_start as i32,
                        min_v: (y + 1 - prev_height) as i32,
                        size_u: (x - prev_start) as i32,
                        size_v: prev_height as i32,
                    });
                }
                start = prev_start;
            }
            if current_height > 0 {
                if stack
                    .last()
                    .map(|&(_, prev_height)| prev_height == current_height)
                    .unwrap_or(false)
                {
                    continue;
                }
                stack.push((start, current_height));
            }
        }
    }

    best_rect
}

fn slice_rect_meets_coverage(
    axis: usize,
    slice: i32,
    rect: SliceRect,
    remaining: &HashSet<IVector>,
) -> bool {
    let filled = slice_rect_filled_count(axis, slice, rect, remaining);
    filled * DEPTH_PHYSICS_SLAB_MIN_COVERAGE_DEN
        >= rect.area() * DEPTH_PHYSICS_SLAB_MIN_COVERAGE_NUM
}

fn slice_rect_filled_count(
    axis: usize,
    slice: i32,
    rect: SliceRect,
    remaining: &HashSet<IVector>,
) -> usize {
    let mut filled = 0;
    for u in rect.min_u..rect.min_u + rect.size_u {
        for v in rect.min_v..rect.min_v + rect.size_v {
            if remaining.contains(&slice_point(axis, slice, u, v)) {
                filled += 1;
            }
        }
    }
    filled
}

fn plane_axes(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (0, 2),
        2 => (0, 1),
        _ => (0, 1),
    }
}

fn slice_point(axis: usize, slice: i32, u: i32, v: i32) -> IVector {
    let (u_axis, v_axis) = plane_axes(axis);
    let mut coords = [0; 3];
    coords[axis] = slice;
    coords[u_axis] = u;
    coords[v_axis] = v;
    IVector::new(coords[0], coords[1], coords[2])
}

fn grid_box_from_slab(axis: usize, slice_min: i32, thickness: i32, rect: SliceRect) -> GridBox {
    let (u_axis, v_axis) = plane_axes(axis);
    let mut origin = [0; 3];
    let mut dims = [1; 3];
    origin[axis] = slice_min;
    origin[u_axis] = rect.min_u;
    origin[v_axis] = rect.min_v;
    dims[axis] = thickness;
    dims[u_axis] = rect.size_u;
    dims[v_axis] = rect.size_v;
    GridBox {
        origin: IVector::new(origin[0], origin[1], origin[2]),
        dims,
    }
}

fn remove_grid_box(grid_box: GridBox, remaining: &mut HashSet<IVector>) {
    remove_seed_box(grid_box.origin, grid_box.dims, remaining);
}

fn decompose_greedy_boxes(occupied: &HashSet<IVector>) -> Vec<GridBox> {
    let mut seeds: Vec<_> = occupied.iter().copied().collect();
    seeds.sort_by_key(|key| (key.y, key.z, key.x));

    let mut remaining = occupied.clone();
    let mut boxes = Vec::new();
    for seed in seeds {
        if !remaining.contains(&seed) {
            continue;
        }
        let dims = best_box_dims(seed, &remaining);
        remove_seed_box(seed, dims, &mut remaining);
        boxes.push(GridBox { origin: seed, dims });
    }
    boxes
}

fn best_box_dims(seed: IVector, remaining: &HashSet<IVector>) -> [i32; 3] {
    const AXIS_ORDERS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    let mut best = [1, 1, 1];
    let mut best_volume = 1;
    let mut best_perimeter = 3;

    for order in AXIS_ORDERS {
        let dims = expand_box(seed, remaining, order);
        let volume = dims[0] * dims[1] * dims[2];
        let perimeter = dims[0] + dims[1] + dims[2];
        if volume > best_volume || (volume == best_volume && perimeter > best_perimeter) {
            best = dims;
            best_volume = volume;
            best_perimeter = perimeter;
        }
    }

    best
}

fn expand_box(seed: IVector, remaining: &HashSet<IVector>, axis_order: [usize; 3]) -> [i32; 3] {
    let mut dims = [1, 1, 1];

    loop {
        let mut changed = false;
        for axis in axis_order {
            while can_expand_box(seed, dims, axis, remaining) {
                dims[axis] += 1;
                changed = true;
            }
        }
        if !changed {
            return dims;
        }
    }
}

fn can_expand_box(
    seed: IVector,
    dims: [i32; 3],
    axis: usize,
    remaining: &HashSet<IVector>,
) -> bool {
    match axis {
        0 => {
            let x = dims[0];
            for y in 0..dims[1] {
                for z in 0..dims[2] {
                    if !remaining.contains(&(seed + IVector::new(x, y, z))) {
                        return false;
                    }
                }
            }
        }
        1 => {
            let y = dims[1];
            for x in 0..dims[0] {
                for z in 0..dims[2] {
                    if !remaining.contains(&(seed + IVector::new(x, y, z))) {
                        return false;
                    }
                }
            }
        }
        2 => {
            let z = dims[2];
            for x in 0..dims[0] {
                for y in 0..dims[1] {
                    if !remaining.contains(&(seed + IVector::new(x, y, z))) {
                        return false;
                    }
                }
            }
        }
        _ => return false,
    }

    true
}

fn remove_seed_box(seed: IVector, dims: [i32; 3], remaining: &mut HashSet<IVector>) {
    for x in 0..dims[0] {
        for y in 0..dims[1] {
            for z in 0..dims[2] {
                remaining.remove(&(seed + IVector::new(x, y, z)));
            }
        }
    }
}

fn physics_box_from_grid_box(
    seed: IVector,
    dims: [i32; 3],
    voxel_size: Vector,
) -> XrDepthPhysicsBox {
    let mins = Vector::new(
        seed.x as f32 * voxel_size.x,
        seed.y as f32 * voxel_size.y,
        seed.z as f32 * voxel_size.z,
    );
    let maxs = Vector::new(
        (seed.x + dims[0]) as f32 * voxel_size.x,
        (seed.y + dims[1]) as f32 * voxel_size.y,
        (seed.z + dims[2]) as f32 * voxel_size.z,
    );

    XrDepthPhysicsBox {
        pose: Pose::new(
            Quat::default(),
            vec3(
                (mins.x + maxs.x) * 0.5,
                (mins.y + maxs.y) * 0.5,
                (mins.z + maxs.z) * 0.5,
            ),
        ),
        half_extents: vec3(
            (maxs.x - mins.x) * 0.5,
            (maxs.y - mins.y) * 0.5,
            (maxs.z - mins.z) * 0.5,
        ),
    }
}
