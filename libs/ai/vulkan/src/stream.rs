//! Command streams: dispatches recorded into a command buffer, submitted
//! and waited for. A recorded stream can be submitted again unchanged (the
//! CUDA-graph equivalent: a decode step's whole graph replays per token).

use crate::device::{vk_err, Device, Result};
use crate::kernel::Args;
use crate::memory::Buffer;
use makepad_vulkan::vk;
use std::sync::{Arc, Mutex};

pub struct Stream {
    device: makepad_vulkan::Device,
    pool: vk::CommandPool,
    queue: vk::Queue,
    queue_lock: Arc<Mutex<()>>,
    layout: vk::PipelineLayout,
    timestamp_period_ns: f32,
    cb: vk::CommandBuffer,
    fence: vk::Fence,
    pending: bool,
    recording: bool,
    queries: Option<(vk::QueryPool, u32)>,
    query_count: u32,
    /// (label, query index) of the timed spans, when timing.
    pub spans: Vec<(String, u32)>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.wait();
        unsafe {
            let d = &self.device;
            d.destroy_fence(self.fence, None);
            d.free_command_buffers(self.pool, &[self.cb]);
            d.destroy_command_pool(self.pool, None);
            if let Some((pool, _)) = self.queries {
                d.destroy_query_pool(pool, None);
            }
        }
    }
}

impl Device {
    /// A stream; with `timestamps` > 0 it can time up to that many spans.
    pub fn stream(&self, timestamps: u32) -> Result<Stream> {
        unsafe {
            let d = &self.device;
            // Recording any buffer externally synchronizes its pool, so even
            // allocation/recording on different worker threads needs one pool
            // per stream, not just a lock around command-buffer allocation.
            let pool = d.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(self.queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            ).map_err(vk_err("vkCreateCommandPool"))?;
            let cb = match d.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            ) {
                Ok(buffers) => buffers[0],
                Err(e) => {
                    d.destroy_command_pool(pool, None);
                    return Err(vk_err("vkAllocateCommandBuffers")(e));
                }
            };
            let fence = match d.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None) {
                Ok(fence) => fence,
                Err(e) => {
                    d.free_command_buffers(pool, &[cb]);
                    d.destroy_command_pool(pool, None);
                    return Err(vk_err("vkCreateFence")(e));
                }
            };
            let queries = if timestamps > 0 {
                match d.create_query_pool(
                    &vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(timestamps * 2),
                    None,
                ) {
                    Ok(queries) => Some((queries, timestamps * 2)),
                    Err(e) => {
                        d.destroy_fence(fence, None);
                        d.free_command_buffers(pool, &[cb]);
                        d.destroy_command_pool(pool, None);
                        return Err(vk_err("vkCreateQueryPool")(e));
                    }
                }
            } else {
                None
            };
            Ok(Stream {
                device: self.device.clone(),
                pool,
                queue: self.queue,
                queue_lock: Arc::clone(&self.queue_lock),
                layout: self.pipeline_layout,
                timestamp_period_ns: self.features.timestamp_period_ns,
                cb,
                fence,
                pending: false,
                recording: false,
                queries,
                query_count: 0,
                spans: Vec::new(),
            })
        }
    }
}

impl Stream {
    /// Starts recording (dropping what was recorded before).
    pub fn begin(&mut self) -> Result<()> {
        self.wait()?;
        self.pending = false;
        unsafe {
            let d = &self.device;
            d.reset_command_buffer(self.cb, vk::CommandBufferResetFlags::empty()).map_err(vk_err("vkResetCommandBuffer"))?;
            d.begin_command_buffer(self.cb, &vk::CommandBufferBeginInfo::default())
                .map_err(vk_err("vkBeginCommandBuffer"))?;
            if let Some((pool, count)) = self.queries {
                d.cmd_reset_query_pool(self.cb, pool, 0, count);
            }
        }
        self.query_count = 0;
        self.spans.clear();
        self.recording = true;
        Ok(())
    }

    /// One kernel dispatch of `groups` workgroups.
    pub fn dispatch(&mut self, args: &Args, groups: [u32; 3]) {
        debug_assert!(self.recording);
        let d = &self.device;
        unsafe {
            d.cmd_bind_pipeline(self.cb, vk::PipelineBindPoint::COMPUTE, args.kernel().pipeline);
            d.cmd_push_constants(self.cb, self.layout, vk::ShaderStageFlags::COMPUTE, 0, &args.bytes);
            d.cmd_dispatch(self.cb, groups[0].max(1), groups[1].max(1), groups[2].max(1));
        }
    }

    /// Everything dispatched before is visible to everything after.
    pub fn barrier(&mut self) {
        let mb = [vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE)];
        let stages = vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER;
        unsafe {
            self.device.cmd_pipeline_barrier(self.cb, stages, stages, vk::DependencyFlags::empty(), &mb, &[], &[]);
        }
    }

    pub fn copy(&mut self, src: &Buffer, src_offset: u64, dst: &Buffer, dst_offset: u64, size: u64) {
        let region = [vk::BufferCopy { src_offset, dst_offset, size }];
        unsafe { self.device.cmd_copy_buffer(self.cb, src.buffer, dst.buffer, &region) };
    }

    pub fn fill(&mut self, dst: &Buffer, offset: u64, size: u64, value: u32) {
        unsafe { self.device.cmd_fill_buffer(self.cb, dst.buffer, offset, size, value) };
    }

    /// Opens a timed span (when the stream has timestamps left).
    pub fn mark(&mut self, label: &str) {
        if let Some((pool, count)) = self.queries {
            if self.query_count < count {
                unsafe {
                    self.device.cmd_write_timestamp(self.cb, vk::PipelineStageFlags::BOTTOM_OF_PIPE, pool, self.query_count)
                };
                self.spans.push((label.to_string(), self.query_count));
                self.query_count += 1;
            }
        }
    }

    pub fn end(&mut self) -> Result<()> {
        if self.recording {
            unsafe { self.device.end_command_buffer(self.cb).map_err(vk_err("vkEndCommandBuffer"))? };
            self.recording = false;
        }
        Ok(())
    }

    /// Submits what was recorded (again, if it ran before).
    pub fn submit(&mut self) -> Result<()> {
        self.end()?;
        self.wait()?;
        // A failed submit leaves the reset fence unsignaled: Drop must only
        // wait when a submission actually owns it.
        self.pending = false;
        unsafe {
            let d = &self.device;
            d.reset_fences(&[self.fence]).map_err(vk_err("vkResetFences"))?;
            let cbs = [self.cb];
            let submit = [vk::SubmitInfo::default().command_buffers(&cbs)];
            // Serialize host queue access, not recording or GPU fence waits.
            let _queue = self.queue_lock.lock().unwrap_or_else(|e| e.into_inner());
            d.queue_submit(self.queue, &submit, self.fence).map_err(vk_err("vkQueueSubmit"))?;
            self.pending = true;
        }
        Ok(())
    }

    pub fn wait(&self) -> Result<()> {
        if !self.pending { return Ok(()); }
        unsafe {
            self.device
                .wait_for_fences(&[self.fence], true, u64::MAX)
                .map_err(vk_err("vkWaitForFences"))
        }
    }

    pub fn submit_wait(&mut self) -> Result<()> {
        self.submit()?;
        self.wait()
    }

    /// Milliseconds between consecutive marks, after the stream ran.
    pub fn timings(&self) -> Result<Vec<(String, f64)>> {
        let Some((pool, _)) = self.queries else { return Ok(Vec::new()) };
        if self.query_count < 2 {
            return Ok(Vec::new());
        }
        let mut ticks = vec![0u64; self.query_count as usize];
        unsafe {
            self.device
                .get_query_pool_results(pool, 0, &mut ticks, vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT)
                .map_err(vk_err("vkGetQueryPoolResults"))?;
        }
        let period = self.timestamp_period_ns as f64;
        let mut out = Vec::new();
        for i in 0..self.spans.len().saturating_sub(1) {
            let dt = ticks[i + 1].wrapping_sub(ticks[i]) as f64 * period / 1e6;
            out.push((self.spans[i].0.clone(), dt));
        }
        Ok(out)
    }
}

impl Device {
    /// Writes `data` into `dst` at `offset` (a mapped copy on unified
    /// memory, else through a staging buffer).
    pub fn upload(&self, dst: &Buffer, offset: u64, data: &[u8]) -> Result<()> {
        if let Some(p) = dst.mapped() {
            assert!(offset + data.len() as u64 <= dst.size);
            unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), p.add(offset as usize), data.len()) };
            return Ok(());
        }
        let staging = self.buffer(data.len() as u64, crate::memory::BufferKind::Staging)?;
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), staging.mapped().unwrap(), data.len()) };
        let mut s = self.stream(0)?;
        s.begin()?;
        s.copy(&staging, 0, dst, offset, data.len() as u64);
        s.submit_wait()
    }

    /// Reads `len` bytes of `src` at `offset`.
    pub fn download(&self, src: &Buffer, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut out = vec![0u8; len];
        if let Some(p) = src.mapped() {
            unsafe { std::ptr::copy_nonoverlapping(p.add(offset as usize), out.as_mut_ptr(), len) };
            return Ok(out);
        }
        let staging = self.buffer(len as u64, crate::memory::BufferKind::Staging)?;
        let mut s = self.stream(0)?;
        s.begin()?;
        s.copy(src, offset, &staging, 0, len as u64);
        s.submit_wait()?;
        unsafe { std::ptr::copy_nonoverlapping(staging.mapped().unwrap(), out.as_mut_ptr(), len) };
        Ok(out)
    }
}
