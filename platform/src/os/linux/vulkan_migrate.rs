//! Transactional GPU resource migration. Both devices stay alive until the
//! bounded byte carrier finishes. No pixels or geometry pass through the CPU.
//! The controller must stop ordinary GPU submissions and retire shared leases
//! before constructing this job; CPU dirty/update flags remain authoritative.
use super::gpu_bridge::GpuBridge;
use super::*;
use std::collections::VecDeque;

const CARRIER_WIDTH: u32 = 1024;
const CARRIER_HEIGHT: u32 = 256;
const CARRIER_BYTES: u64 = CARRIER_WIDTH as u64 * CARRIER_HEIGHT as u64 * 4;

enum CopyJob {
    Buffer {
        source: vk::Buffer,
        destination: vk::Buffer,
        size: u64,
        offset: u64,
    },
    Image {
        key: VulkanTextureKey,
        source: vk::Image,
        destination: vk::Image,
        width: u32,
        height: u32,
        layers: u32,
        bytes_per_pixel: u32,
        aspect: vk::ImageAspectFlags,
        layout: vk::ImageLayout,
        row: u32,
        layer: u32,
    },
}

#[derive(Clone, Copy)]
enum Chunk {
    Buffer {
        source: vk::Buffer,
        destination: vk::Buffer,
        offset: u64,
        size: u64,
        last: bool,
    },
    Image {
        source: vk::Image,
        destination: vk::Image,
        region: vk::BufferImageCopy,
        layout: vk::ImageLayout,
        first_row: bool,
        last_row: bool,
        bytes: u64,
    },
}

impl CopyJob {
    fn chunk(&self) -> Chunk {
        match *self {
            Self::Buffer {
                source,
                destination,
                size,
                offset,
            } => {
                let count = (size - offset).min(CARRIER_BYTES);
                Chunk::Buffer {
                    source,
                    destination,
                    offset,
                    size: count,
                    last: offset + count == size,
                }
            }
            Self::Image {
                source,
                destination,
                width,
                height,
                bytes_per_pixel,
                aspect,
                layout,
                row,
                layer,
                ..
            } => {
                let rows = (CARRIER_BYTES / (u64::from(width) * u64::from(bytes_per_pixel))) as u32;
                let rows = rows.min(height - row);
                Chunk::Image {
                    source,
                    destination,
                    layout,
                    first_row: row == 0,
                    last_row: row + rows == height,
                    bytes: u64::from(width) * u64::from(rows) * u64::from(bytes_per_pixel),
                    region: vk::BufferImageCopy::default()
                        .image_subresource(
                            vk::ImageSubresourceLayers::default()
                                .aspect_mask(aspect)
                                .mip_level(0)
                                .base_array_layer(layer)
                                .layer_count(1),
                        )
                        .image_offset(vk::Offset3D {
                            x: 0,
                            y: row as i32,
                            z: 0,
                        })
                        .image_extent(vk::Extent3D {
                            width,
                            height: rows,
                            depth: 1,
                        }),
                }
            }
        }
    }

    fn advance(&mut self, chunk: Chunk) -> bool {
        match (self, chunk) {
            (Self::Buffer { offset, size, .. }, Chunk::Buffer { size: count, .. }) => {
                *offset += count;
                *offset == *size
            }
            (
                Self::Image {
                    row,
                    layer,
                    height,
                    layers,
                    ..
                },
                Chunk::Image { region, .. },
            ) => {
                *row += region.image_extent.height;
                if *row == *height {
                    *row = 0;
                    *layer += 1;
                }
                *layer == *layers
            }
            _ => unreachable!(),
        }
    }
}

/// Must be explicitly retired with the source renderer still alive. Its
/// candidate contains copied cache entries under their original IDs; resetting
/// allocation metadata after commit would discard preserved history/geometry.
pub(super) struct ResourceMigration {
    pub target: Box<CxVulkan>,
    bridge: Option<GpuBridge>,
    source_scratch: Option<VulkanBuffer>,
    destination_scratch: Option<VulkanBuffer>,
    jobs: VecDeque<CopyJob>,
    submitted: Option<Chunk>,
    failed: Option<String>,
    pub copied_bytes: u64,
}

impl ResourceMigration {
    /// The exact source device and its resident cache must remain alive and
    /// unmodified through all polls and `finish`. Ordinary GPU work is frozen.
    pub unsafe fn prepare(
        source: &CxVulkan,
        cx: &Cx,
        target: Box<CxVulkan>,
    ) -> Result<Self, String> {
        if !source.shared_readers_drained() {
            return Err("GPU migration still has shared framebuffer readers".into());
        }
        let mut migration = Self {
            target,
            bridge: None,
            source_scratch: None,
            destination_scratch: None,
            jobs: VecDeque::new(),
            submitted: None,
            failed: None,
            copied_bytes: 0,
        };
        // Nothing has been submitted before prepare_resources succeeds, so
        // allocation failures can discard the candidate immediately.
        if let Err(error) = migration.prepare_resources(source, cx) {
            migration.retire_scratch(source);
            return Err(error);
        }
        if migration.jobs.is_empty() {
            return Ok(migration);
        }
        match GpuBridge::new(source, &migration.target, carrier_extent()) {
            Ok(bridge) => migration.bridge = Some(bridge),
            Err(error) => {
                migration.retire_scratch(source);
                return Err(error);
            }
        }
        Ok(migration)
    }

    fn prepare_resources(&mut self, source: &CxVulkan, cx: &Cx) -> Result<(), String> {
        for (&key, resource) in &source.textures {
            if source.is_shared_image(key.0) {
                continue;
            }
            let Some(slot) = cx.textures.0.pool.get(key.0 .0) else {
                continue;
            };
            if TextureId::from_pool_slot(key.0 .0, slot.generation) != key.0
                || cx.textures.0.is_free(key.0 .0)
            {
                continue;
            }
            if !resource.owns_image
                || resource.sampler.is_some()
                || resource.ycbcr_conversion.is_some()
            {
                return Err(format!(
                    "texture {key} requires its external owner to recreate the import"
                ));
            }
            // The requested TextureFormat may have changed since the last
            // upload. Migrate the resident allocation; normal dirty/update
            // processing will apply the pending CPU change after commit.
            let allocation = cx.textures[key.0]
                .alloc
                .as_ref()
                .ok_or_else(|| format!("texture {key} has no resident allocation metadata"))?;
            let depth = matches!(
                allocation.category,
                TextureCategory::DepthBuffer | TextureCategory::DepthBufferSampled
            );
            let bytes_per_pixel = match resource.format {
                vk::Format::R8_UNORM => 1,
                vk::Format::R8G8_UNORM => 2,
                vk::Format::B8G8R8A8_UNORM
                | vk::Format::R8G8B8A8_UNORM
                | vk::Format::R32_SFLOAT
                | vk::Format::D32_SFLOAT => 4,
                vk::Format::R16G16B16A16_SFLOAT => 8,
                vk::Format::R32G32B32A32_SFLOAT => 16,
                other => {
                    return Err(format!(
                        "texture {key} has an unsupported migration format: {other:?}"
                    ))
                }
            };
            if resource.width == 0
                || resource.height == 0
                || u64::from(resource.width) * u64::from(bytes_per_pixel) > CARRIER_BYTES
            {
                return Err(format!(
                    "texture {key} exceeds the bounded migration row size"
                ));
            }
            let copied = if depth {
                if resource.layers != 1 || resource.format != vk::Format::D32_SFLOAT {
                    return Err(format!(
                        "texture {key} has unsupported migration depth subresources"
                    ));
                }
                self.target.create_depth_target_layers_usage(
                    resource.width,
                    resource.height,
                    resource.format,
                    resource.layers,
                    matches!(allocation.category, TextureCategory::DepthBufferSampled),
                )?
            } else if matches!(
                allocation.category,
                TextureCategory::Vec | TextureCategory::VecMip | TextureCategory::VecCube
            ) {
                self.target.create_texture_resource(
                    resource.width,
                    resource.height,
                    resource.layers,
                    resource.is_cube,
                    resource.format,
                )?
            } else if matches!(
                allocation.category,
                TextureCategory::Render | TextureCategory::RenderCube
            ) {
                self.target.create_color_target_resource(
                    resource.width,
                    resource.height,
                    resource.format,
                    resource.is_cube,
                )?
            } else {
                return Err(format!(
                    "texture {key} cannot be migrated as an owned image"
                ));
            };
            let destination = copied.image;
            self.target.textures.insert(key, copied);
            // Undefined images have no contents to preserve. Keep them
            // undefined so their ordinary initialization still happens.
            if resource.layout != vk::ImageLayout::UNDEFINED {
                self.jobs.push_back(CopyJob::Image {
                    key,
                    source: resource.image,
                    destination,
                    width: resource.width,
                    height: resource.height,
                    layers: resource.layers,
                    bytes_per_pixel,
                    aspect: if depth {
                        vk::ImageAspectFlags::DEPTH
                    } else {
                        vk::ImageAspectFlags::COLOR
                    },
                    layout: resource.layout,
                    row: 0,
                    layer: 0,
                });
            }
        }
        for (&id, resource) in &source.geometries {
            if !CxVulkan::geometry_id_is_live(cx, id) || cx.geometries.0.is_free(id.slot_index()) {
                continue;
            }
            let vertex_buffer = device_buffer(
                &self.target,
                resource.vertex_buffer.size,
                vk::BufferUsageFlags::VERTEX_BUFFER | migration_buffer_usage(),
            )?;
            let index_buffer = match device_buffer(
                &self.target,
                resource.index_buffer.size,
                vk::BufferUsageFlags::INDEX_BUFFER | migration_buffer_usage(),
            ) {
                Ok(buffer) => buffer,
                Err(error) => {
                    self.target.destroy_buffer(vertex_buffer);
                    return Err(error);
                }
            };
            for (old, new) in [
                (resource.vertex_buffer, vertex_buffer),
                (resource.index_buffer, index_buffer),
            ] {
                if old.size % 4 != 0 {
                    self.target.destroy_buffer(vertex_buffer);
                    self.target.destroy_buffer(index_buffer);
                    return Err("geometry migration requires four-byte aligned buffers".into());
                }
                self.jobs.push_back(CopyJob::Buffer {
                    source: old.buffer,
                    destination: new.buffer,
                    size: old.size,
                    offset: 0,
                });
            }
            self.target.geometries.insert(
                id,
                VulkanGeometryResource {
                    vertex_buffer,
                    index_buffer,
                },
            );
        }
        if !self.jobs.is_empty() {
            self.source_scratch = Some(device_buffer(
                source,
                CARRIER_BYTES,
                migration_buffer_usage(),
            )?);
            self.destination_scratch = Some(device_buffer(
                &self.target,
                CARRIER_BYTES,
                migration_buffer_usage(),
            )?);
        }
        Ok(())
    }

    /// One bounded send/receive step. The caller services control messages and
    /// the independent presenter between polls. Completion includes the final
    /// destination fence; a submitted copy alone is never a commit signal.
    pub unsafe fn poll(&mut self, source: &CxVulkan) -> Result<bool, String> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let result = self.poll_inner(source);
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }

    fn poll_inner(&mut self, source: &CxVulkan) -> Result<bool, String> {
        let Some(bridge) = self.bridge.as_mut() else {
            return Ok(self.jobs.is_empty());
        };
        if !unsafe { bridge.poll_initialization(source, &self.target) }? {
            return Ok(false);
        }
        if let Some(chunk) = self.submitted {
            let scratch = self.destination_scratch.unwrap();
            if unsafe {
                bridge.receive_commands(source, &self.target, |command, carrier| {
                    receive_chunk(&self.target, command, carrier, scratch.buffer, chunk)
                })
            }? {
                let job = self
                    .jobs
                    .front_mut()
                    .ok_or("migration lost its pending resource")?;
                if job.advance(chunk) {
                    if let CopyJob::Image { key, layout, .. } = job {
                        self.target
                            .textures
                            .get_mut(key)
                            .ok_or("migration lost its destination image")?
                            .layout = *layout;
                    }
                    self.jobs.pop_front();
                }
                self.copied_bytes += match chunk {
                    Chunk::Buffer { size, .. } => size,
                    Chunk::Image { bytes, .. } => bytes,
                };
                self.submitted = None;
            }
            return Ok(false);
        }
        if !unsafe { bridge.writable(source, &self.target) }? {
            return Ok(false);
        }
        let Some(job) = self.jobs.front() else {
            return Ok(true);
        };
        let chunk = job.chunk();
        let scratch = self.source_scratch.unwrap();
        unsafe {
            bridge.send_commands(source, |command, carrier| {
                send_chunk(source, command, carrier, scratch.buffer, chunk)
            })
        }?;
        self.submitted = Some(chunk);
        Ok(false)
    }

    // Also used on cancellation/error. The controller normally polls to idle
    // first; wait-idle is the last cleanup barrier, not the transfer mechanism.
    pub unsafe fn drained(&self, source: &CxVulkan) -> Result<bool, String> {
        self.bridge
            .as_ref()
            .map(|bridge| unsafe { bridge.idle(source, &self.target) })
            .unwrap_or(Ok(true))
    }

    pub unsafe fn finish(mut self, source: &CxVulkan) -> Box<CxVulkan> {
        if let Some(bridge) = self.bridge.take() {
            if !unsafe { bridge.idle(source, &self.target) }.unwrap_or(false) {
                source.device_wait_idle();
                self.target.device_wait_idle();
            }
            unsafe { bridge.destroy(source, &self.target) };
        }
        self.retire_scratch(source);
        self.target
    }

    fn retire_scratch(&mut self, source: &CxVulkan) {
        if let Some(buffer) = self.source_scratch.take() {
            source.destroy_buffer(buffer);
        }
        if let Some(buffer) = self.destination_scratch.take() {
            self.target.destroy_buffer(buffer);
        }
    }
}

fn carrier_extent() -> vk::Extent2D {
    vk::Extent2D {
        width: CARRIER_WIDTH,
        height: CARRIER_HEIGHT,
    }
}

fn carrier_region() -> vk::BufferImageCopy {
    vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width: CARRIER_WIDTH,
            height: CARRIER_HEIGHT,
            depth: 1,
        })
}

fn buffer_barrier(
    gpu: &CxVulkan,
    command: vk::CommandBuffer,
    buffer: vk::Buffer,
    source: vk::AccessFlags,
    destination: vk::AccessFlags,
) {
    unsafe {
        gpu.device.cmd_pipeline_barrier(
            command,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[vk::BufferMemoryBarrier::default()
                .src_access_mask(source)
                .dst_access_mask(destination)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(buffer)
                .offset(0)
                .size(vk::WHOLE_SIZE)],
            &[],
        )
    };
}

fn image_barrier(
    gpu: &CxVulkan,
    command: vk::CommandBuffer,
    image: vk::Image,
    region: vk::BufferImageCopy,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
) {
    unsafe {
        gpu.device.cmd_pipeline_barrier(
            command,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[vk::ImageMemoryBarrier::default()
                .src_access_mask(if old == vk::ImageLayout::UNDEFINED {
                    vk::AccessFlags::empty()
                } else {
                    vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE
                })
                .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                .old_layout(old)
                .new_layout(new)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(region.image_subresource.aspect_mask)
                        .level_count(1)
                        .base_array_layer(region.image_subresource.base_array_layer)
                        .layer_count(1),
                )],
        )
    };
}

fn send_chunk(
    gpu: &CxVulkan,
    command: vk::CommandBuffer,
    carrier: vk::Image,
    scratch: vk::Buffer,
    chunk: Chunk,
) -> Result<(), String> {
    buffer_barrier(
        gpu,
        command,
        scratch,
        vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
        vk::AccessFlags::TRANSFER_WRITE,
    );
    unsafe {
        gpu.device
            .cmd_fill_buffer(command, scratch, 0, CARRIER_BYTES, 0)
    };
    buffer_barrier(
        gpu,
        command,
        scratch,
        vk::AccessFlags::TRANSFER_WRITE,
        vk::AccessFlags::TRANSFER_WRITE,
    );
    match chunk {
        Chunk::Buffer {
            source,
            offset,
            size,
            ..
        } => {
            buffer_barrier(
                gpu,
                command,
                source,
                vk::AccessFlags::MEMORY_WRITE,
                vk::AccessFlags::TRANSFER_READ,
            );
            unsafe {
                gpu.device.cmd_copy_buffer(
                    command,
                    source,
                    scratch,
                    &[vk::BufferCopy::default()
                        .src_offset(offset)
                        .dst_offset(0)
                        .size(size)],
                )
            };
        }
        Chunk::Image {
            source,
            region,
            layout,
            ..
        } => {
            image_barrier(
                gpu,
                command,
                source,
                region,
                layout,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            );
            unsafe {
                gpu.device.cmd_copy_image_to_buffer(
                    command,
                    source,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    scratch,
                    &[region],
                )
            };
            image_barrier(
                gpu,
                command,
                source,
                region,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                layout,
            );
        }
    }
    buffer_barrier(
        gpu,
        command,
        scratch,
        vk::AccessFlags::TRANSFER_WRITE,
        vk::AccessFlags::TRANSFER_READ,
    );
    unsafe {
        gpu.device.cmd_copy_buffer_to_image(
            command,
            scratch,
            carrier,
            vk::ImageLayout::GENERAL,
            &[carrier_region()],
        )
    };
    Ok(())
}

fn receive_chunk(
    gpu: &CxVulkan,
    command: vk::CommandBuffer,
    carrier: vk::Image,
    scratch: vk::Buffer,
    chunk: Chunk,
) -> Result<(), String> {
    buffer_barrier(
        gpu,
        command,
        scratch,
        vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
        vk::AccessFlags::TRANSFER_WRITE,
    );
    unsafe {
        gpu.device.cmd_copy_image_to_buffer(
            command,
            carrier,
            vk::ImageLayout::GENERAL,
            scratch,
            &[carrier_region()],
        )
    };
    buffer_barrier(
        gpu,
        command,
        scratch,
        vk::AccessFlags::TRANSFER_WRITE,
        vk::AccessFlags::TRANSFER_READ,
    );
    match chunk {
        Chunk::Buffer {
            destination,
            offset,
            size,
            last,
            ..
        } => {
            unsafe {
                gpu.device.cmd_copy_buffer(
                    command,
                    scratch,
                    destination,
                    &[vk::BufferCopy::default()
                        .src_offset(0)
                        .dst_offset(offset)
                        .size(size)],
                )
            };
            if last {
                buffer_barrier(
                    gpu,
                    command,
                    destination,
                    vk::AccessFlags::TRANSFER_WRITE,
                    vk::AccessFlags::MEMORY_READ,
                );
            }
        }
        Chunk::Image {
            destination,
            region,
            layout,
            first_row,
            last_row,
            ..
        } => {
            if first_row {
                image_barrier(
                    gpu,
                    command,
                    destination,
                    region,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                );
            }
            unsafe {
                gpu.device.cmd_copy_buffer_to_image(
                    command,
                    scratch,
                    destination,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                )
            };
            if last_row {
                image_barrier(
                    gpu,
                    command,
                    destination,
                    region,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    layout,
                );
            }
        }
    }
    Ok(())
}

fn device_buffer(
    gpu: &CxVulkan,
    size: u64,
    usage: vk::BufferUsageFlags,
) -> Result<VulkanBuffer, String> {
    let buffer = unsafe {
        gpu.device.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(size.max(4))
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )
    }
    .map_err(|error| format!("create migration buffer: {error:?}"))?;
    let requirements = unsafe { gpu.device.get_buffer_memory_requirements(buffer) };
    let memory_type = match gpu.find_memory_type(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    ) {
        Ok(index) => index,
        Err(error) => {
            unsafe { gpu.device.destroy_buffer(buffer, None) };
            return Err(error);
        }
    };
    let memory = match unsafe {
        gpu.device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type),
            None,
        )
    } {
        Ok(memory) => memory,
        Err(error) => {
            unsafe { gpu.device.destroy_buffer(buffer, None) };
            return Err(format!("allocate migration buffer: {error:?}"));
        }
    };
    if let Err(error) = unsafe { gpu.device.bind_buffer_memory(buffer, memory, 0) } {
        unsafe {
            gpu.device.destroy_buffer(buffer, None);
            gpu.device.free_memory(memory, None);
        }
        return Err(format!("bind migration buffer: {error:?}"));
    }
    Ok(VulkanBuffer {
        buffer,
        memory,
        size: size.max(4),
    })
}
