//! Opt-in Linux Vulkan pass timing. Default off (`MAKEPAD_VULKAN_PROFILE=1`).
//! CPU fence waits plus whole-pass GPU timestamps on the main command buffer.
//! No query allocation, waits, or logging when disabled.

use crate::cx::Cx;
use crate::draw_pass::DrawPassId;
use ash::vk;
use std::collections::HashMap;
use std::ffi::CStr;
use std::time::Instant;

const QUERY_SLOTS: u32 = 2;
const MAX_PASS_KEYS: usize = 256;
const REPORT_PERIOD: std::time::Duration = std::time::Duration::from_secs(1);

pub(super) struct VulkanProfile {
    enabled: bool,
    validation: bool,
    recycle: bool,
    pid: u32,
    uuid: [u8; 16],
    device_label: String,
    timestamp_valid_bits: u32,
    timestamp_period: f32,
    query_pool: vk::QueryPool,
    query_failed: bool,
    pending: Option<ProfileSample>,
    stats: HashMap<PassKey, PassStats>,
    overflow: [Option<PassStats>; 2],
    composition_submitted: u32,
    capture_submitted: u32,
    composition_busy: u32,
    composition_skipped: u32,
    last_report: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PassKey {
    composition: bool,
    capture: bool,
    pass_id: usize,
    window_id: usize,
    width: u32,
    height: u32,
}

pub(super) struct ProfileSample {
    key: PassKey,
    debug_name: String,
    repaint_id: u64,
    prewait_ms: f64,
    pub(super) encode_ms: f64,
    pub(super) submit_ms: f64,
    pub(super) wrote_timestamps: bool,
}

impl ProfileSample {
    #[cfg(linux_direct)]
    pub(super) fn mark_capture(&mut self, capture: bool) {
        self.key.capture |= capture;
    }
}

struct PassStats {
    debug_name: String,
    composition: bool,
    capture: bool,
    pass_id: usize,
    window_id: usize,
    width: u32,
    height: u32,
    samples: u32,
    gpu_valid: u32,
    prewait_sum: f64,
    prewait_max: f64,
    encode_sum: f64,
    encode_max: f64,
    submit_sum: f64,
    submit_max: f64,
    postwait_sum: f64,
    postwait_max: f64,
    gpu_sum: f64,
    gpu_max: f64,
    repaint_min: u64,
    repaint_max: u64,
}

impl VulkanProfile {
    pub(super) fn from_env(recycle: bool) -> Self {
        let enabled = std::env::var("MAKEPAD_VULKAN_PROFILE").ok().as_deref() == Some("1");
        Self {
            enabled,
            validation: std::env::var_os("MAKEPAD_VULKAN_VALIDATION").is_some(),
            recycle,
            pid: std::process::id(),
            uuid: [0; 16],
            device_label: String::new(),
            timestamp_valid_bits: 0,
            timestamp_period: 0.0,
            query_pool: vk::QueryPool::null(),
            query_failed: false,
            pending: None,
            stats: HashMap::new(),
            overflow: [None, None],
            composition_submitted: 0,
            capture_submitted: 0,
            composition_busy: 0,
            composition_skipped: 0,
            last_report: None,
        }
    }

    #[inline]
    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn collect_pending(&mut self, device: &ash::Device) {
        if !self.enabled {
            return;
        }
        let Some(sample) = self.pending.take() else {
            return;
        };
        let gpu_ms = if sample.wrote_timestamps {
            self.read_gpu_ms(device)
        } else {
            None
        };
        self.aggregate(sample, 0.0, gpu_ms);
        self.maybe_report();
    }

    pub(super) fn begin_sample(
        &self,
        cx: &Cx,
        draw_pass_id: DrawPassId,
        composition: bool,
        width: u32,
        height: u32,
        prewait_ms: f64,
    ) -> Option<ProfileSample> {
        if !self.enabled {
            return None;
        }
        let window = cx.get_pass_window_id(draw_pass_id).map(|w| w.id());
        Some(ProfileSample {
            key: PassKey {
                composition,
                capture: !cx.screenshot_requests.is_empty()
                    || crate::screen_capture::screen_capture_active(),
                pass_id: draw_pass_id.0,
                window_id: window.unwrap_or(0),
                width,
                height,
            },
            debug_name: cx.passes[draw_pass_id]
                .debug_name
                .chars()
                .take(128)
                .collect(),
            repaint_id: cx.repaint_id,
            prewait_ms,
            encode_ms: 0.0,
            submit_ms: 0.0,
            wrote_timestamps: false,
        })
    }

    pub(super) fn begin_timestamps(
        &mut self,
        device: &ash::Device,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        queue_family_index: u32,
        command_buffer: vk::CommandBuffer,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        self.ensure_query_pool(device, instance, physical_device, queue_family_index);
        if self.query_pool == vk::QueryPool::null() {
            return false;
        }
        unsafe {
            device.cmd_reset_query_pool(command_buffer, self.query_pool, 0, QUERY_SLOTS);
            device.cmd_write_timestamp(
                command_buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                self.query_pool,
                0,
            );
        }
        true
    }

    pub(super) fn end_timestamps(
        &mut self,
        device: &ash::Device,
        command_buffer: vk::CommandBuffer,
    ) {
        if !self.enabled || self.query_pool == vk::QueryPool::null() {
            return;
        }
        unsafe {
            device.cmd_write_timestamp(
                command_buffer,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                self.query_pool,
                1,
            );
        }
    }

    pub(super) fn admit_pending(&mut self, sample: ProfileSample) {
        if !self.enabled {
            return;
        }
        if sample.key.composition {
            if sample.key.capture {
                self.capture_submitted = self.capture_submitted.saturating_add(1);
            } else {
                self.composition_submitted = self.composition_submitted.saturating_add(1);
            }
        }
        self.pending = Some(sample);
        if self.last_report.is_none() {
            self.last_report = Some(Instant::now());
        }
    }

    pub(super) fn complete_after_fence(&mut self, device: &ash::Device, postwait_ms: f64) {
        if !self.enabled {
            return;
        }
        let Some(sample) = self.pending.take() else {
            return;
        };
        let gpu_ms = if sample.wrote_timestamps {
            self.read_gpu_ms(device)
        } else {
            None
        };
        self.aggregate(sample, postwait_ms, gpu_ms);
        self.maybe_report();
    }

    #[cfg(linux_direct)]
    pub(super) fn note_composition_busy(&mut self) {
        if !self.enabled {
            return;
        }
        self.composition_busy = self.composition_busy.saturating_add(1);
        if self.last_report.is_none() {
            self.last_report = Some(Instant::now());
        }
        self.maybe_report();
    }

    #[cfg(linux_direct)]
    pub(super) fn note_composition_skipped(&mut self) {
        if !self.enabled {
            return;
        }
        self.composition_skipped = self.composition_skipped.saturating_add(1);
        if self.last_report.is_none() {
            self.last_report = Some(Instant::now());
        }
        self.maybe_report();
    }

    pub(super) fn destroy(&mut self, device: &ash::Device) {
        if self.enabled {
            self.collect_pending(device);
            self.report(true);
        }
        if self.query_pool != vk::QueryPool::null() {
            unsafe { device.destroy_query_pool(self.query_pool, None) };
            self.query_pool = vk::QueryPool::null();
        }
        self.pending = None;
    }

    fn ensure_query_pool(
        &mut self,
        device: &ash::Device,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        queue_family_index: u32,
    ) {
        if self.query_pool != vk::QueryPool::null() || self.query_failed {
            return;
        }
        let props = unsafe { instance.get_physical_device_properties(physical_device) };
        let queues =
            unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
        self.timestamp_valid_bits = queues
            .get(queue_family_index as usize)
            .map(|q| q.timestamp_valid_bits)
            .unwrap_or(0);
        self.timestamp_period = props.limits.timestamp_period;
        self.device_label = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let mut ids = vk::PhysicalDeviceIDProperties::default();
        let mut props2 = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
        unsafe { instance.get_physical_device_properties2(physical_device, &mut props2) };
        self.uuid = ids.device_uuid;

        if self.timestamp_valid_bits == 0 || self.timestamp_period <= 0.0 {
            self.query_failed = true;
            crate::log!(
                "Vulkan profile: GPU timestamps unavailable (valid_bits={} period={}); CPU timing only",
                self.timestamp_valid_bits,
                self.timestamp_period
            );
            return;
        }
        let create_info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(QUERY_SLOTS);
        match unsafe { device.create_query_pool(&create_info, None) } {
            Ok(pool) => self.query_pool = pool,
            Err(err) => {
                self.query_failed = true;
                crate::log!(
                    "Vulkan profile: timestamp query pool create failed ({err:?}); CPU timing only"
                );
            }
        }
    }

    fn read_gpu_ms(&self, device: &ash::Device) -> Option<f64> {
        if self.query_pool == vk::QueryPool::null() {
            return None;
        }
        let mut timestamps = [0u64; 2];
        let result = unsafe {
            device.get_query_pool_results(
                self.query_pool,
                0,
                &mut timestamps,
                vk::QueryResultFlags::TYPE_64,
            )
        };
        match result {
            Ok(()) => gpu_duration_ms(
                timestamps[0],
                timestamps[1],
                self.timestamp_valid_bits,
                self.timestamp_period,
            ),
            Err(vk::Result::NOT_READY) => None,
            Err(_) => None,
        }
    }

    fn aggregate(&mut self, sample: ProfileSample, postwait_ms: f64, gpu_ms: Option<f64>) {
        let stats = if let Some(existing) = self.stats.get_mut(&sample.key) {
            existing
        } else if self.stats.len() < MAX_PASS_KEYS {
            self.stats
                .entry(sample.key)
                .or_insert_with(|| PassStats::new(&sample))
        } else {
            self.overflow[sample.key.capture as usize]
                .get_or_insert_with(|| PassStats::overflow(&sample))
        };
        stats.add(&sample, postwait_ms, gpu_ms);
    }

    fn maybe_report(&mut self) {
        self.report(false);
    }

    fn report(&mut self, force: bool) {
        let Some(started) = self.last_report else {
            return;
        };
        let now = Instant::now();
        let interval = now.duration_since(started);
        if !force && interval < REPORT_PERIOD {
            return;
        }
        if self.stats.is_empty()
            && self.overflow.iter().all(Option::is_none)
            && self.composition_submitted == 0
            && self.capture_submitted == 0
            && self.composition_busy == 0
            && self.composition_skipped == 0
        {
            self.last_report = Some(now);
            return;
        }
        let elapsed = interval.as_secs_f64().max(1e-6);
        let hz = self.composition_submitted as f64 / elapsed;
        let uuid = uuid_hex(&self.uuid);
        crate::log!(
            "Vulkan profile pid={} uuid={} device={:?} validation={} recycle={} interval_s={:.3} composition_submitted_hz={:.2} capture_submitted={} busy={} skipped={}",
            self.pid,
            uuid,
            self.device_label,
            self.validation as u8,
            self.recycle as u8,
            elapsed,
            hz,
            self.capture_submitted,
            self.composition_busy,
            self.composition_skipped
        );
        let mut rows: Vec<&PassStats> = self.stats.values().collect();
        for overflow in self.overflow.iter().filter_map(Option::as_ref) {
            rows.push(overflow);
        }
        for stats in rows {
            crate::log!(
                "Vulkan profile pid={} uuid={} pass={} name={:?} window={} {}x{} composition={} capture={} samples={} cpu_prewait_ms avg={:.3} max={:.3} encoding avg={:.3} max={:.3} submit_frame avg={:.3} max={:.3} postwait avg={:.3} max={:.3} gpu_whole_pass_ms avg={:.3} max={:.3} gpu_valid={}/{} repaint={}..{}",
                self.pid,
                uuid,
                stats.pass_id,
                stats.debug_name,
                stats.window_id,
                stats.width,
                stats.height,
                stats.composition as u8,
                stats.capture as u8,
                stats.samples,
                avg(stats.prewait_sum, stats.samples),
                stats.prewait_max,
                avg(stats.encode_sum, stats.samples),
                stats.encode_max,
                avg(stats.submit_sum, stats.samples),
                stats.submit_max,
                avg(stats.postwait_sum, stats.samples),
                stats.postwait_max,
                avg(stats.gpu_sum, stats.gpu_valid),
                stats.gpu_max,
                stats.gpu_valid,
                stats.samples,
                stats.repaint_min,
                stats.repaint_max
            );
        }
        self.stats.clear();
        self.overflow = [None, None];
        self.composition_submitted = 0;
        self.capture_submitted = 0;
        self.composition_busy = 0;
        self.composition_skipped = 0;
        // Include time spent reporting in the next interval's denominator.
        self.last_report = Some(now);
    }
}

impl PassStats {
    fn new(sample: &ProfileSample) -> Self {
        Self {
            debug_name: sample.debug_name.clone(),
            composition: sample.key.composition,
            capture: sample.key.capture,
            pass_id: sample.key.pass_id,
            window_id: sample.key.window_id,
            width: sample.key.width,
            height: sample.key.height,
            samples: 0,
            gpu_valid: 0,
            prewait_sum: 0.0,
            prewait_max: 0.0,
            encode_sum: 0.0,
            encode_max: 0.0,
            submit_sum: 0.0,
            submit_max: 0.0,
            postwait_sum: 0.0,
            postwait_max: 0.0,
            gpu_sum: 0.0,
            gpu_max: 0.0,
            repaint_min: sample.repaint_id,
            repaint_max: sample.repaint_id,
        }
    }

    fn overflow(sample: &ProfileSample) -> Self {
        let mut stats = Self::new(sample);
        stats.debug_name = "overflow".into();
        stats
    }

    fn add(&mut self, sample: &ProfileSample, postwait_ms: f64, gpu_ms: Option<f64>) {
        self.samples = self.samples.saturating_add(1);
        self.prewait_sum += sample.prewait_ms;
        self.prewait_max = self.prewait_max.max(sample.prewait_ms);
        self.encode_sum += sample.encode_ms;
        self.encode_max = self.encode_max.max(sample.encode_ms);
        self.submit_sum += sample.submit_ms;
        self.submit_max = self.submit_max.max(sample.submit_ms);
        self.postwait_sum += postwait_ms;
        self.postwait_max = self.postwait_max.max(postwait_ms);
        if let Some(gpu_ms) = gpu_ms {
            self.gpu_valid = self.gpu_valid.saturating_add(1);
            self.gpu_sum += gpu_ms;
            self.gpu_max = self.gpu_max.max(gpu_ms);
        }
        self.repaint_min = self.repaint_min.min(sample.repaint_id);
        self.repaint_max = self.repaint_max.max(sample.repaint_id);
        if self.debug_name.is_empty() && !sample.debug_name.is_empty() {
            self.debug_name.clone_from(&sample.debug_name);
        }
    }
}

fn avg(sum: f64, count: u32) -> f64 {
    if count == 0 {
        0.0
    } else {
        sum / count as f64
    }
}

fn gpu_duration_ms(t0: u64, t1: u64, valid_bits: u32, period_ns: f32) -> Option<f64> {
    if period_ns <= 0.0 || valid_bits == 0 {
        return None;
    }
    let bits = valid_bits.min(64);
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let delta = t1.wrapping_sub(t0) & mask;
    Some(delta as f64 * period_ns as f64 / 1_000_000.0)
}

fn uuid_hex(uuid: &[u8; 16]) -> String {
    uuid.iter().map(|byte| format!("{byte:02x}")).collect()
}
