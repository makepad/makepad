pub enum GpuPerformance {
    Tier1, // quest 1
    Tier2, // quest 2
    Tier3, // intel, androids
    Tier4, // nvidia/ati/apple
    Tier5, // need a way to detect a 3090 :)
}

pub struct GpuInfo {
    pub min_uniform_vectors: u32,
    /// Native backends support these formats; WebGL reports its extension.
    pub float_color_targets: bool,
    /// RGBA16Float colour targets that ordinary (BGRA8-declared) draw shaders
    /// can render into WITH blending — an HDR scene pass. Set by backends
    /// that build the matching pipeline variant (Metal; Vulkan where the
    /// device blends RGBA16Float, see `CxVulkan::float16_blend_targets`).
    pub float16_blend_targets: bool,
    /// Block-compressed texture formats the backend samples and uploads as
    /// [`crate::texture::TextureFormat::VecMipCompressed`]. Off unless the
    /// backend sets them after probing the device.
    pub texture_bc7: bool,
    pub texture_astc4x4: bool,
    pub performance: GpuPerformance,
    pub vendor: String,
    pub renderer: String,
}

impl Default for GpuInfo {
    fn default() -> Self {
        Self {
            // default to a nice gpu
            min_uniform_vectors: 1024,
            float_color_targets: true,
            float16_blend_targets: false,
            texture_bc7: false,
            texture_astc4x4: false,
            performance: GpuPerformance::Tier4,
            vendor: "unknown".to_string(),
            renderer: "unknown".to_string(),
        }
    }
}

impl GpuInfo {
    pub fn init_from_info(&mut self, min_uniform_vectors: u32, vendor: String, renderer: String) {
        self.vendor = vendor;
        self.vendor.make_ascii_lowercase();
        self.renderer = renderer;
        self.renderer.make_ascii_lowercase();

        self.min_uniform_vectors = min_uniform_vectors;

        // default tier 3
        self.performance = GpuPerformance::Tier3;

        // extremely useless performance separation. need to make this better
        if self.vendor.contains("qualcomm") && self.renderer.contains("540") {
            // its a quest 1
            self.performance = GpuPerformance::Tier1;
        }
        if self.vendor.contains("qualcomm") && self.renderer.contains("610") {
            // its a quest 2
            self.performance = GpuPerformance::Tier2;
        }
        if self.vendor.contains("intel") {
            self.performance = GpuPerformance::Tier3;
        }
        if self.vendor.contains("ati") {
            self.performance = GpuPerformance::Tier4;
        }
        if self.vendor.contains("nvidia") {
            self.performance = GpuPerformance::Tier4;
        }
    }

    pub fn is_low_on_uniform_vectors(&self) -> bool {
        self.min_uniform_vectors < 512
    }
}

/// The largest single GPU buffer the device allocates, in bytes (Metal's
/// `maxBufferLength`); 0 while unknown. A mesh past it cannot be drawn in
/// one buffer.
static MAX_GPU_BUFFER_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn max_gpu_buffer_bytes() -> u64 {
    MAX_GPU_BUFFER_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set by the backend once it knows its device.
pub fn set_max_gpu_buffer_bytes(bytes: u64) {
    MAX_GPU_BUFFER_BYTES.store(bytes, std::sync::atomic::Ordering::Relaxed);
}

/// The memory the device works in, in bytes (Metal's
/// `recommendedMaxWorkingSetSize`, else half the machine's memory); 0
/// while unknown. Everything a mesh holds at once on its way to the GPU
/// must fit in it.
static GPU_WORKING_SET_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn gpu_working_set_bytes() -> u64 {
    GPU_WORKING_SET_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// `MAKEPAD_GPU_WORKING_SET=<bytes>` runs as on a device with no more
/// memory than that (a smaller device's limits, tried on this one).
pub fn set_gpu_working_set_bytes(bytes: u64) {
    let emulated = std::env::var("MAKEPAD_GPU_WORKING_SET").ok().and_then(|v| v.parse::<u64>().ok());
    let bytes = match emulated {
        Some(e) if bytes == 0 || e < bytes => e,
        _ => bytes,
    };
    GPU_WORKING_SET_BYTES.store(bytes, std::sync::atomic::Ordering::Relaxed);
}
