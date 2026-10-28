//! Hardware H.264 encode on the GPU's video encode queue (VK_KHR_video_encode_queue +
//! VK_KHR_video_encode_h264), on the process's Vulkan device and outside any session: the Linux
//! backend of `makepad_video::VideoStreamEncoder` (through the host API in
//! `crate::stream_encode`).
//!
//! A picture is NV12 packed into a BGRA8 texture the app rendered (shared as a DMA-BUF and
//! registered with the copy registry) or CPU NV12 bytes. The texture's rows are copied, on the
//! GPU, into the planes of the encode source image (texture -> buffer -> plane: the bytes of a
//! BGRA8 row *are* the plane's bytes) once the producer's sync_file has signalled; the encode
//! then runs on the video queue, and only the compressed bytes reach the CPU.
//!
//! The stream is for a live view: H.264 High, no B frames, every P frame references the one
//! before it (two DPB slots), an IDR only on request or at the start, low-delay VBR rate
//! control. Parameter sets come from [`crate::h264_bits`] (our writer) and precede every IDR;
//! the driver's own encoding of them is compared once at open and used instead when it reports
//! that it overrode a field (the slices then follow its values).

use super::device::{error, lock, Device};
use crate::h264_bits::{level_for, StreamParams};
use makepad_vulkan::vk;
use std::os::fd::{IntoRawFd, OwnedFd};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// One picture at a time: the encoder is driven synchronously (`StreamEncoder::encode_*`
/// returns the picture's bitstream), so one set of per-picture objects is enough.
const SLOTS: usize = 1;
/// The bitstream buffer of one picture (an IDR of a 4K desktop is well under this at LAN rates).
const BITSTREAM_BYTES: u64 = 16 << 20;
/// Picture images (source and references) round their size up to this (see `video_image`).
const IMAGE_ALIGN: [u32; 2] = [256, 64];
const LOG2_MAX_FRAME_NUM: u32 = 16;
/// H.265's picture order count LSBs (its POC counts pictures from the IDR).
const LOG2_MAX_POC_LSB: u32 = 16;

/// The lowest H.265 level (`StdVideoH265LevelIdc` value) of a `w` x `h` stream at `fps`: its
/// picture size and luma sample rate (Table A.8: levels 5.1 .. 6.2).
fn h265_level(w: u32, h: u32, fps: u32) -> i32 {
    let size = w as u64 * h as u64;
    let rate = size * fps.max(1) as u64;
    match (size, rate) {
        (s, r) if s <= 8_912_896 && r <= 534_773_760 => 8,
        (s, r) if s <= 8_912_896 && r <= 1_069_547_520 => 9,
        (_, r) if r <= 1_069_547_520 => 10,
        (_, r) if r <= 2_139_095_040 => 11,
        _ => 12,
    }
}

/// The stream's codec.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Codec {
    H264,
    /// H.265: Main (4:2:0) or Format range extensions (4:4:4), CABAC always (H.264 4:4:4 is
    /// CAVLC-only on NVIDIA), transform skip for text.
    H265,
}

/// How the stream trades encode time, latency and bits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tuning {
    /// The encoder's quality level (`VkVideoEncodeQualityLevelInfoKHR`, 0 = its fastest); larger
    /// than it has: its best.
    pub quality: u32,
    /// Constant bit rate (a VBV-capped stream whose every picture fits the link) instead of VBR.
    pub constant: bool,
    /// The rate control's virtual buffer, in pictures (the largest burst a picture may make,
    /// as a multiple of an average picture); 0: 2.
    pub buffer_pictures: u32,
    /// The encoder's best pictures per bit: ultra-low-latency tuning (on NVIDIA about 60% more
    /// encode time than low-latency, and a third fewer bits for the same PSNR on desktop text);
    /// else low-latency tuning, its fastest. Both encode each picture as it comes (no lookahead,
    /// no reordering): the latency is the encode time alone.
    pub sharp: bool,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { quality: 0, constant: false, buffer_pictures: 2, sharp: true }
    }
}

/// The video profile and its chained infos, at a fixed address for the stream's life: every
/// object made for the profile (session, images, buffers, queries) points at the same chain.
struct Profile {
    h264: vk::VideoEncodeH264ProfileInfoKHR,
    h265: vk::VideoEncodeH265ProfileInfoKHR,
    usage: vk::VideoEncodeUsageInfoKHR,
    info: vk::VideoProfileInfoKHR,
    list: vk::VideoProfileListInfoKHR,
}

impl Profile {
    /// H.264 High (4:2:0) or High 4:4:4 Predictive when `full_chroma`; H.265 Main or Format
    /// range extensions.
    fn new(codec: Codec, full_chroma: bool, sharp: bool) -> Box<Self> {
        let mut p = Box::new(Profile {
            h264: vk::VideoEncodeH264ProfileInfoKHR::default().std_profile_idc(if full_chroma { vk::StdVideoH264ProfileIdc::HIGH_444_PREDICTIVE } else { vk::StdVideoH264ProfileIdc::HIGH }),
            h265: vk::VideoEncodeH265ProfileInfoKHR::default().std_profile_idc(if full_chroma { vk::StdVideoH265ProfileIdc::FORMAT_RANGE_EXTENSIONS } else { vk::StdVideoH265ProfileIdc::MAIN }),
            usage: vk::VideoEncodeUsageInfoKHR::default()
                .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::STREAMING)
                .video_content_hints(vk::VideoEncodeContentFlagsKHR::DESKTOP)
                .tuning_mode(if sharp { vk::VideoEncodeTuningModeKHR::ULTRA_LOW_LATENCY } else { vk::VideoEncodeTuningModeKHR::LOW_LATENCY }),
            info: vk::VideoProfileInfoKHR::default()
                .video_codec_operation(match codec { Codec::H264 => vk::VideoCodecOperationFlagsKHR::ENCODE_H264, Codec::H265 => vk::VideoCodecOperationFlagsKHR::ENCODE_H265 })
                .chroma_subsampling(if full_chroma { vk::VideoChromaSubsamplingFlagsKHR::TYPE_444 } else { vk::VideoChromaSubsamplingFlagsKHR::TYPE_420 })
                .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8),
            list: vk::VideoProfileListInfoKHR::default(),
        });
        let p_ref = &mut *p;
        match codec {
            Codec::H264 => {
                p_ref.h264.p_next = (&mut p_ref.usage as *mut vk::VideoEncodeUsageInfoKHR).cast();
                p_ref.info.p_next = (&mut p_ref.h264 as *mut vk::VideoEncodeH264ProfileInfoKHR).cast();
            }
            Codec::H265 => {
                p_ref.h265.p_next = (&mut p_ref.usage as *mut vk::VideoEncodeUsageInfoKHR).cast();
                p_ref.info.p_next = (&mut p_ref.h265 as *mut vk::VideoEncodeH265ProfileInfoKHR).cast();
            }
        }
        p_ref.list.profile_count = 1;
        p_ref.list.p_profiles = &p_ref.info;
        p
    }
}

struct VideoImage {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

struct Slot {
    src: VideoImage,
    /// The packed planes on their way from the textures to `src` (device memory).
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    bitstream: vk::Buffer,
    bitstream_memory: vk::DeviceMemory,
    bitstream_coherent: bool,
    mapped: usize,
    cb: vk::CommandBuffer,
    fence: vk::Fence,
}

/// A QP delta per region of the picture (VK_KHR_video_encode_quantization_map): the caller's
/// deltas (`StreamEncoder::set_qp_map`) staged, uploaded before the next picture's encode.
struct QpMap {
    image: VideoImage,
    /// Its texels (columns, rows) and the pixels each one covers.
    extent: vk::Extent2D,
    texel: vk::Extent2D,
    /// The QP deltas the encoder takes (min, max).
    range: [i32; 2],
    /// Bytes per texel of its format (R8_SINT 1, R16_SINT 2, R32_SINT 4).
    bytes: u32,
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    mapped: usize,
    coherent: bool,
    /// Deltas staged and not uploaded yet; uploaded once (the image is in its layout).
    pending: std::cell::Cell<bool>,
    uploaded: std::cell::Cell<bool>,
}

/// The Vulkan objects of a stream; the worker uses them, `encode_frame` records into `slots`.
struct Encoder {
    dev: Arc<Device>,
    _profile: Box<Profile>,
    session: vk::VideoSessionKHR,
    session_memory: Vec<vk::DeviceMemory>,
    params: vk::VideoSessionParametersKHR,
    dpb: Vec<VideoImage>,
    slots: Vec<Slot>,
    pool: vk::CommandPool,
    queries: vk::QueryPool,
    /// GPU timestamps around the encode (when its queue family has them).
    stamps: Option<vk::QueryPool>,
    /// Which queue (engine) of the encode family it encodes on.
    queue_index: usize,
    /// The last picture's encode on the GPU (ns), when measured.
    last_encode_ns: std::cell::Cell<Option<u64>>,
    /// The picture (w, h) and the coded size (aligned).
    size: [u32; 2],
    coded: [u32; 2],
    /// SPS + PPS, Annex-B.
    parameter_sets: Vec<u8>,
    quality_level: u32,
    /// The quality levels the encoder has (0 ..= this - 1).
    quality_levels: u32,
    fps: u32,
    max_bitrate: u64,
    /// 4:4:4 (High 4:4:4 Predictive / Format range extensions) instead of 4:2:0.
    full_chroma: bool,
    codec: Codec,
    tuning: Tuning,
    /// QP deltas per region, when the encoder takes them.
    qp_map: Option<QpMap>,
}

unsafe impl Send for Encoder {}
unsafe impl Sync for Encoder {}

/// What a picture is: its place in the stream and the reference it predicts from.
#[derive(Clone, Copy)]
struct Picture {
    slot: usize,
    idr: bool,
    idr_pic_id: u16,
    frame_num: u32,
    poc: i32,
    /// The DPB slot this picture reconstructs into, and the one it references (P only).
    setup: usize,
    reference: Option<(usize, u32, i32)>,
    /// The first picture: the session is reset and its rate control set.
    reset: bool,
    /// The rate control to set before this picture (kbit/s), if it changed.
    bitrate: Option<u32>,
    /// The rate control in effect when this picture's coding scope begins.
    current_kbps: Option<u32>,
    /// The QP ceiling for this picture's rate control (0: none; `StreamEncoder::set_max_qp`).
    max_qp: u32,
    /// A refinement: the same picture again at `REFINE_QP` (rate control off), near lossless.
    refine: bool,
    /// Rate control is off when this picture's coding scope begins (after a refinement).
    rc_off_now: bool,
}

/// The QP of a refinement picture: near lossless (text and edges as they are).
const REFINE_QP: i32 = 10;

fn default_kbps(w: u32, h: u32, fps: u32, full_chroma: bool) -> u32 {
    // quality first (a LAN): about 0.3 bits per pixel per frame for 4:2:0, 0.45 for 4:4:4 (twice
    // the samples); the encoder's own maximum caps it
    let per_mille: u64 = if full_chroma { 450 } else { 300 };
    ((w as u64 * h as u64 * fps.max(1) as u64 * per_mille / 1000) / 1000).clamp(2_000, 1_000_000) as u32
}

/// H.264 level value -> StdVideoH264LevelIdc.
fn std_level(level_idc: u8) -> vk::StdVideoH264LevelIdc {
    let raw = match level_idc {
        31 => 8,
        32 => 9,
        40 => 10,
        41 => 11,
        42 => 12,
        50 => 13,
        51 => 14,
        52 => 15,
        60 => 16,
        61 => 17,
        _ => 18,
    };
    vk::StdVideoH264LevelIdc::from_raw(raw)
}

fn std_level_value(level: vk::StdVideoH264LevelIdc) -> u8 {
    match level.as_raw() {
        8 => 31,
        9 => 32,
        10 => 40,
        11 => 41,
        12 => 42,
        13 => 50,
        14 => 51,
        15 => 52,
        16 => 60,
        17 => 61,
        18 => 62,
        _ => 30,
    }
}

/// A picture format the encoder takes, with the image flags and tiling it wants for it.
#[derive(Clone, Copy)]
struct PictureFormat {
    format: vk::Format,
    flags: vk::ImageCreateFlags,
    tiling: vk::ImageTiling,
}

unsafe fn video_image(dev: &Device, profile: &Profile, pf: PictureFormat, extent: [u32; 2], usage: vk::ImageUsageFlags, families: &[u32]) -> Result<VideoImage, String> {
    let format = pf.format;
    // the image is larger than any coded extent it holds (the pictures stay `coded`, cropped by
    // the SPS): NVIDIA's 4:4:4 encoder (RTX PRO 6000, driver 610) reads past the end of a
    // picture image sized to the coded extent (NVENC MMU fault, Xid 31, then the device is lost),
    // at sizes that depend on width and height (3080x608, 1696x1440, 3040x632...); the same
    // layout with twice the memory, or the image rounded up to 256 x 64, encodes every one
    let extent = [extent[0].div_ceil(IMAGE_ALIGN[0]) * IMAGE_ALIGN[0], extent[1].div_ceil(IMAGE_ALIGN[1]) * IMAGE_ALIGN[1]];
    let mut list = profile.list;
    list.p_next = std::ptr::null_mut();
    let mut info = vk::ImageCreateInfo::default()
        .flags(pf.flags)
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D { width: extent[0], height: extent[1], depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(pf.tiling)
        .usage(usage)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .next_video_profile_list_info_khr(&mut list);
    info = if families.len() > 1 { info.sharing_mode(vk::SharingMode::CONCURRENT).queue_family_indices(families) } else { info.sharing_mode(vk::SharingMode::EXCLUSIVE) };
    let image = dev.raw.create_image(&info, None).map_err(error)?;
    let req = dev.raw.get_image_memory_requirements(image);
    let ty = dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
    let memory = match dev.raw.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None) {
        Ok(m) => m,
        Err(e) => {
            dev.raw.destroy_image(image, None);
            return Err(error(e));
        }
    };
    dev.raw.bind_image_memory(image, memory, 0).map_err(error)?;
    let view = dev
        .raw
        .create_image_view(
            &vk::ImageViewCreateInfo::default().image(image).view_type(vk::ImageViewType::TYPE_2D).format(format).subresource_range(
                vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1),
            ),
            None,
        )
        .map_err(error)?;
    Ok(VideoImage { image, memory, view })
}

/// The format of the profile's QP delta maps, the pixels one texel covers (the smallest the
/// encoder offers) and its bytes per texel; None when it offers none this code writes.
unsafe fn qp_map_format(dev: &Device, list: &mut vk::VideoProfileListInfoKHR) -> Option<(PictureFormat, vk::Extent2D, u32)> {
    let call = dev.instance.table.get_physical_device_video_format_properties_khr?;
    let info = vk::PhysicalDeviceVideoFormatInfoKHR::default().image_usage(vk::ImageUsageFlags::VIDEO_ENCODE_QUANTIZATION_DELTA_MAP_KHR).next_video_profile_list_info_khr(list);
    let mut count = 0u32;
    if call(dev.physical, &info, &mut count, std::ptr::null_mut()) != vk::Result::SUCCESS || count == 0 {
        return None;
    }
    // each answer with its texel size chained (the arrays stay put while the driver writes)
    let mut texels = vec![vk::VideoFormatQuantizationMapPropertiesKHR::default(); count as usize];
    let mut props = vec![vk::VideoFormatPropertiesKHR::default(); count as usize];
    for (p, t) in props.iter_mut().zip(texels.iter_mut()) {
        p.p_next = (t as *mut vk::VideoFormatQuantizationMapPropertiesKHR).cast();
    }
    if call(dev.physical, &info, &mut count, props.as_mut_ptr()) != vk::Result::SUCCESS {
        return None;
    }
    props
        .iter()
        .zip(texels.iter())
        .take(count as usize)
        .filter_map(|(p, t)| {
            // VK_FORMAT_R8_SINT, R16_SINT, R32_SINT
            let bytes = match p.format.as_raw() {
                14 => 1,
                75 => 2,
                99 => 4,
                _ => return None,
            };
            (t.quantization_map_texel_size.width > 0).then_some((PictureFormat { format: p.format, flags: p.image_create_flags, tiling: p.image_tiling }, t.quantization_map_texel_size, bytes))
        })
        .min_by_key(|(_, t, _)| t.width * t.height)
}

/// A QP delta map image of `extent` texels and its staging buffer (all deltas 0 until set).
#[allow(clippy::too_many_arguments)]
unsafe fn qp_map(dev: &Device, profile: &Profile, pf: PictureFormat, extent: vk::Extent2D, texel: vk::Extent2D, range: [i32; 2], bytes: u32, family: u32) -> Result<QpMap, String> {
    let image = video_image(dev, profile, pf, [extent.width, extent.height], vk::ImageUsageFlags::VIDEO_ENCODE_QUANTIZATION_DELTA_MAP_KHR | vk::ImageUsageFlags::TRANSFER_DST, &[family])?;
    let size = extent.width as u64 * extent.height as u64 * bytes as u64;
    let (staging, staging_memory, coherent) = buffer(dev, size, vk::BufferUsageFlags::TRANSFER_SRC, None, true)?;
    let mapped = dev.raw.map_memory(staging_memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()).map_err(error)? as usize;
    std::ptr::write_bytes(mapped as *mut u8, 0, size as usize);
    Ok(QpMap { image, extent, texel, range, bytes, staging, staging_memory, mapped, coherent, pending: std::cell::Cell::new(false), uploaded: std::cell::Cell::new(false) })
}

unsafe fn buffer(dev: &Device, size: u64, usage: vk::BufferUsageFlags, profile: Option<&Profile>, host: bool) -> Result<(vk::Buffer, vk::DeviceMemory, bool), String> {
    let mut list = profile.map(|p| {
        let mut l = p.list;
        l.p_next = std::ptr::null_mut();
        l
    });
    let mut info = vk::BufferCreateInfo::default().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE);
    if let Some(list) = list.as_mut() {
        info = info.next_video_profile_list_info_khr(list);
    }
    let buffer = dev.raw.create_buffer(&info, None).map_err(error)?;
    let req = dev.raw.get_buffer_memory_requirements(buffer);
    let (ty, coherent) = if host {
        match dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_CACHED) {
            Ok(t) => (t, dev.memory.memory_types[t as usize].property_flags.contains(vk::MemoryPropertyFlags::HOST_COHERENT)),
            Err(_) => (dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?, true),
        }
    } else {
        (dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?, true)
    };
    let memory = match dev.raw.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None) {
        Ok(m) => m,
        Err(e) => {
            dev.raw.destroy_buffer(buffer, None);
            return Err(error(e));
        }
    };
    dev.raw.bind_buffer_memory(buffer, memory, 0).map_err(error)?;
    Ok((buffer, memory, coherent))
}

impl Encoder {
    /// Everything a stream of `size` needs (4:4:4 when `full_chroma`), or why this GPU cannot
    /// encode it.
    fn new(dev: Arc<Device>, size: [u32; 2], fps: u32, full_chroma: bool, codec: Codec, tuning: Tuning) -> Result<Self, String> {
        let video = dev.video_encode.as_ref().ok_or("this GPU has no hardware video encode queue")?;
        if codec == Codec::H265 && !video.h265 {
            return Err("this GPU's encoder has no H.265 (VK_KHR_video_encode_h265)".into());
        }
        if size[0] == 0 || size[1] == 0 || size[0] % 4 != 0 || size[1] % 2 != 0 {
            return Err(format!("stream size {}x{}: the width must be a multiple of 4 and the height even", size[0], size[1]));
        }
        let profile = Profile::new(codec, full_chroma, tuning.sharp);
        let picture_format = if full_chroma { vk::Format::G8_B8R8_2PLANE_444_UNORM } else { vk::Format::G8_B8R8_2PLANE_420_UNORM };
        unsafe {
            // capabilities of the profile
            let mut h264_caps = vk::VideoEncodeH264CapabilitiesKHR::default();
            let mut h265_caps = vk::VideoEncodeH265CapabilitiesKHR::default();
            let mut encode_caps = vk::VideoEncodeCapabilitiesKHR::default();
            let mut qp_caps = vk::VideoEncodeQuantizationMapCapabilitiesKHR::default();
            let mut h264_qp_caps = vk::VideoEncodeH264QuantizationMapCapabilitiesKHR::default();
            let mut h265_qp_caps = vk::VideoEncodeH265QuantizationMapCapabilitiesKHR::default();
            let mut caps = vk::VideoCapabilitiesKHR::default().next_video_encode_capabilities_khr(&mut encode_caps);
            caps = match codec {
                Codec::H264 => caps.next_video_encode_h264_capabilities_khr(&mut h264_caps),
                Codec::H265 => caps.next_video_encode_h265_capabilities_khr(&mut h265_caps),
            };
            if video.qp_map {
                caps = caps.next_video_encode_quantization_map_capabilities_khr(&mut qp_caps);
                caps = match codec {
                    Codec::H264 => caps.next_video_encode_h264_quantization_map_capabilities_khr(&mut h264_qp_caps),
                    Codec::H265 => caps.next_video_encode_h265_quantization_map_capabilities_khr(&mut h265_qp_caps),
                };
            }
            dev.instance.get_physical_device_video_capabilities(dev.physical, &profile.info, &mut caps).map_err(|e| format!("{codec:?} encode capabilities: {e:?}"))?;
            let qp_range = match codec {
                Codec::H264 => [h264_qp_caps.min_qp_delta, h264_qp_caps.max_qp_delta],
                Codec::H265 => [h265_qp_caps.min_qp_delta, h265_qp_caps.max_qp_delta],
            };
            let gran = caps.picture_access_granularity;
            // macroblocks (H.264) or 8x8 coding blocks (H.265), then the encoder's picture access
            // granularity
            let unit = match codec { Codec::H264 => 16, Codec::H265 => 8 };
            let align = |v: u32, g: u32| (v.div_ceil(unit) * unit).div_ceil(g.max(1)) * g.max(1);
            let coded = [align(size[0], gran.width).max(caps.min_coded_extent.width), align(size[1], gran.height).max(caps.min_coded_extent.height)];
            if coded[0] > caps.max_coded_extent.width || coded[1] > caps.max_coded_extent.height {
                return Err(format!("stream size {}x{} exceeds the encoder's {}x{}", size[0], size[1], caps.max_coded_extent.width, caps.max_coded_extent.height));
            }
            let p_refs = match codec { Codec::H264 => h264_caps.max_p_picture_l0_reference_count, Codec::H265 => h265_caps.max_p_picture_l0_reference_count };
            if caps.max_dpb_slots < 2 || caps.max_active_reference_pictures < 1 || p_refs < 1 {
                return Err(format!("the {codec:?} encoder takes no P frame references"));
            }
            let feedback = vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BUFFER_OFFSET | vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BYTES_WRITTEN;
            if !encode_caps.supported_encode_feedback_flags.contains(feedback) {
                return Err("the encoder reports no bitstream size".into());
            }
            let rc_mode = if tuning.constant { vk::VideoEncodeRateControlModeFlagsKHR::CBR } else { vk::VideoEncodeRateControlModeFlagsKHR::VBR };
            if !encode_caps.rate_control_modes.contains(rc_mode) {
                return Err(format!("the encoder has no {rc_mode:?} rate control"));
            }
            let quality_levels = encode_caps.max_quality_levels.max(1);
            // picture formats
            let mut list = profile.list;
            list.p_next = std::ptr::null_mut();
            let format_of = |usage: vk::ImageUsageFlags, list: &mut vk::VideoProfileListInfoKHR| -> Result<PictureFormat, String> {
                let info = vk::PhysicalDeviceVideoFormatInfoKHR::default().image_usage(usage).next_video_profile_list_info_khr(list);
                let formats = dev.instance.get_physical_device_video_format_properties(dev.physical, &info).map_err(|e| format!("video formats: {e:?}"))?;
                formats
                    .iter()
                    .find(|f| f.format == picture_format && f.image_type == vk::ImageType::TYPE_2D)
                    .map(|f| PictureFormat { format: f.format, flags: f.image_create_flags, tiling: f.image_tiling })
                    .ok_or_else(|| format!("the encoder takes no {picture_format:?} pictures for {usage:?}"))
            };
            let src_format = format_of(vk::ImageUsageFlags::VIDEO_ENCODE_SRC_KHR | vk::ImageUsageFlags::TRANSFER_DST, &mut list)?;
            let mut list = profile.list;
            list.p_next = std::ptr::null_mut();
            let dpb_format = format_of(vk::ImageUsageFlags::VIDEO_ENCODE_DPB_KHR, &mut list)?;
            // a QP delta map's format and the pixels a texel covers (the smallest texel offered)
            let qp_format = if video.qp_map && qp_caps.max_quantization_map_extent.width > 0 && qp_range[1] > qp_range[0] {
                let mut list = profile.list;
                list.p_next = std::ptr::null_mut();
                qp_map_format(&dev, &mut list)
            } else {
                None
            };
            // session
            let header = caps.std_header_version;
            let session_flags = vk::VideoSessionCreateFlagsKHR::ALLOW_ENCODE_PARAMETER_OPTIMIZATIONS
                | if qp_format.is_some() { vk::VideoSessionCreateFlagsKHR::ALLOW_ENCODE_QUANTIZATION_DELTA_MAP } else { vk::VideoSessionCreateFlagsKHR::empty() };
            let session = dev
                .raw
                .create_video_session(
                    &vk::VideoSessionCreateInfoKHR::default()
                        // the driver may change parameters for speed (it reports what it did; its
                        // own parameter sets are sent then): NVIDIA's H.265 encode is 5x faster
                        // with it (4K 4:4:4: 16.6 -> 3.1 ms at low-latency tuning, RTX 5090)
                        .flags(session_flags)
                        .queue_family_index(video.family)
                        .video_profile(&profile.info)
                        .picture_format(src_format.format)
                        .max_coded_extent(vk::Extent2D { width: coded[0], height: coded[1] })
                        .reference_picture_format(dpb_format.format)
                        .max_dpb_slots(2)
                        .max_active_reference_pictures(1)
                        .std_header_version(&header),
                    None,
                )
                .map_err(|e| format!("vkCreateVideoSessionKHR: {e:?}"))?;
            let mut enc = Encoder {
                dev: dev.clone(),
                _profile: profile,
                session,
                session_memory: Vec::new(),
                params: vk::VideoSessionParametersKHR::null(),
                dpb: Vec::new(),
                slots: Vec::new(),
                pool: vk::CommandPool::null(),
                queries: vk::QueryPool::null(),
                stamps: None,
                queue_index: 0,
                last_encode_ns: std::cell::Cell::new(None),
                size,
                coded,
                parameter_sets: Vec::new(),
                quality_level: tuning.quality.min(quality_levels - 1),
                quality_levels,
                fps: fps.max(1),
                max_bitrate: encode_caps.max_bitrate,
                full_chroma,
                codec,
                tuning,
                qp_map: None,
            };
            if let Some((pf, texel, bytes)) = qp_format {
                let extent = vk::Extent2D {
                    width: coded[0].div_ceil(texel.width).min(qp_caps.max_quantization_map_extent.width),
                    height: coded[1].div_ceil(texel.height).min(qp_caps.max_quantization_map_extent.height),
                };
                match qp_map(&dev, &enc._profile, pf, extent, texel, qp_range, bytes, video.family) {
                    Ok(map) => enc.qp_map = Some(map),
                    Err(e) => eprintln!("[hal-video] QP delta map: {e}; encoding without"),
                }
            }
            for req in dev.raw.get_video_session_memory_requirements(session).map_err(error)? {
                let ty = dev
                    .memory_type(req.memory_requirements.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
                    .or_else(|_| dev.memory_type(req.memory_requirements.memory_type_bits, vk::MemoryPropertyFlags::empty()))?;
                let memory = dev
                    .raw
                    .allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.memory_requirements.size).memory_type_index(ty), None)
                    .map_err(error)?;
                enc.session_memory.push(memory);
                dev.raw
                    .bind_video_session_memory(
                        session,
                        &[vk::BindVideoSessionMemoryInfoKHR::default()
                            .memory_bind_index(req.memory_bind_index)
                            .memory(memory)
                            .memory_size(req.memory_requirements.size)],
                    )
                    .map_err(error)?;
            }
            match codec {
                Codec::H264 => enc.h264_parameters(&h264_caps)?,
                Codec::H265 => enc.h265_parameters(&h265_caps)?,
            }
            // reference pictures and the per-picture objects
            for _ in 0..2 {
                let image = video_image(&dev, &enc._profile, dpb_format, coded, vk::ImageUsageFlags::VIDEO_ENCODE_DPB_KHR, &[video.family])?;
                enc.dpb.push(image);
            }
            enc.pool = dev
                .raw
                .create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(video.family).flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None)
                .map_err(error)?;
            let cbs = dev
                .raw
                .allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(enc.pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(SLOTS as u32))
                .map_err(error)?;
            let mut query_feedback = vk::QueryPoolVideoEncodeFeedbackCreateInfoKHR::default().encode_feedback_flags(feedback);
            let mut query_profile = enc._profile.info;
            enc.queries = dev
                .raw
                .create_query_pool(
                    &vk::QueryPoolCreateInfo::default()
                        .query_type(vk::QueryType::VIDEO_ENCODE_FEEDBACK_KHR)
                        .query_count(SLOTS as u32)
                        .next_query_pool_video_encode_feedback_create_info_khr(&mut query_feedback)
                        .next_video_profile_info_khr(&mut query_profile),
                    None,
                )
                .map_err(error)?;
            if video.timestamp_bits > 0 {
                enc.stamps = Some(dev.raw.create_query_pool(&vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(2), None).map_err(error)?);
            }
            let families = [dev.family, video.family];
            // luma, then the interleaved chroma (4:2:0: a quarter of the samples; 4:4:4: all)
            let luma = size[0] as u64 * size[1] as u64;
            let plane_bytes = if full_chroma { luma * 3 } else { luma * 3 / 2 };
            let align = caps.min_bitstream_buffer_size_alignment.max(1);
            let bitstream_size = BITSTREAM_BYTES.div_ceil(align) * align;
            for cb in cbs {
                let src = video_image(&dev, &enc._profile, src_format, coded, vk::ImageUsageFlags::VIDEO_ENCODE_SRC_KHR | vk::ImageUsageFlags::TRANSFER_DST, &families)?;
                let (staging, staging_memory, _) = buffer(&dev, plane_bytes, vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST, None, false)?;
                let (bitstream, bitstream_memory, bitstream_coherent) = buffer(&dev, bitstream_size, vk::BufferUsageFlags::VIDEO_ENCODE_DST_KHR, Some(&enc._profile), true)?;
                let mapped = dev.raw.map_memory(bitstream_memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()).map_err(error)? as usize;
                let fence = dev.raw.create_fence(&vk::FenceCreateInfo::default(), None).map_err(error)?;
                enc.slots.push(Slot { src, staging, staging_memory, bitstream, bitstream_memory, bitstream_coherent, mapped, cb, fence });
            }
            Ok(enc)
        }
    }

    /// The H.264 SPS and PPS (ours, checked against the driver's encoding of them).
    unsafe fn h264_parameters(&mut self, h264_caps: &vk::VideoEncodeH264CapabilitiesKHR) -> Result<(), String> {
        let (size, coded, full_chroma) = (self.size, self.coded, self.full_chroma);
        let level = level_for(size[0], size[1], self.fps);
        let level = level.min(std_level_value(h264_caps.max_level_idc).max(31));
        let params = StreamParams {
            profile_idc: if full_chroma { 244 } else { 100 },
            chroma_format_idc: if full_chroma { 3 } else { 1 },
            level_idc: level,
            width: size[0],
            height: size[1],
            log2_max_frame_num_minus4: (LOG2_MAX_FRAME_NUM - 4) as u8,
            max_num_ref_frames: 1,
            // CAVLC for 4:4:4: NVIDIA's encoder fails the command buffer
            // (ERROR_INITIALIZATION_FAILED) for High 4:4:4 Predictive with CABAC
            cabac: !full_chroma,
            transform_8x8: true,
            pic_init_qp_minus26: 0,
            chroma_qp_index_offset: 0,
            full_range: false,
        };
        // the coded size may exceed the macroblock-rounded size (granularity): cropped
        let crop_unit = if full_chroma { 1 } else { 2 };
        let (crop_right, crop_bottom) = ((coded[0] - size[0]) / crop_unit, (coded[1] - size[1]) / crop_unit);
        let vui = vk::StdVideoH264SequenceParameterSetVui {
            flags: vk::StdVideoH264SpsVuiFlags::default()
                .video_signal_type_present_flag(1)
                .video_full_range_flag(params.full_range as u32)
                .color_description_present_flag(1)
                .bitstream_restriction_flag(1),
            aspect_ratio_idc: vk::StdVideoH264AspectRatioIdc::SQUARE,
            video_format: 5,
            colour_primaries: 1,
            transfer_characteristics: 1,
            matrix_coefficients: 1,
            max_num_reorder_frames: 0,
            max_dec_frame_buffering: params.max_num_ref_frames,
            ..Default::default()
        };
        let sps = vk::StdVideoH264SequenceParameterSet {
            flags: vk::StdVideoH264SpsFlags::default()
                .direct_8x8_inference_flag(1)
                .frame_mbs_only_flag(1)
                .frame_cropping_flag((crop_right != 0 || crop_bottom != 0) as u32)
                .vui_parameters_present_flag(1),
            profile_idc: if full_chroma { vk::StdVideoH264ProfileIdc::HIGH_444_PREDICTIVE } else { vk::StdVideoH264ProfileIdc::HIGH },
            level_idc: std_level(level),
            chroma_format_idc: if full_chroma { vk::StdVideoH264ChromaFormatIdc::TYPE_444 } else { vk::StdVideoH264ChromaFormatIdc::TYPE_420 },
            log2_max_frame_num_minus4: params.log2_max_frame_num_minus4,
            pic_order_cnt_type: vk::StdVideoH264PocType::TYPE_2,
            max_num_ref_frames: params.max_num_ref_frames,
            pic_width_in_mbs_minus1: coded[0] / 16 - 1,
            pic_height_in_map_units_minus1: coded[1] / 16 - 1,
            frame_crop_right_offset: crop_right,
            frame_crop_bottom_offset: crop_bottom,
            p_sequence_parameter_set_vui: &vui as *const _,
            ..Default::default()
        };
        let pps = vk::StdVideoH264PictureParameterSet {
            flags: vk::StdVideoH264PpsFlags::default()
                .transform_8x8_mode_flag(params.transform_8x8 as u32)
                .deblocking_filter_control_present_flag(1)
                .entropy_coding_mode_flag(params.cabac as u32),
            weighted_bipred_idc: vk::StdVideoH264WeightedBipredIdc::DEFAULT,
            ..Default::default()
        };
        let sps_list = [sps];
        let pps_list = [pps];
        let add = vk::VideoEncodeH264SessionParametersAddInfoKHR::default().std_sp_ss(&sps_list).std_pp_ss(&pps_list);
        let mut h264_params = vk::VideoEncodeH264SessionParametersCreateInfoKHR::default().max_std_sps_count(1).max_std_pps_count(1).parameters_add_info(&add);
        let mut quality = vk::VideoEncodeQualityLevelInfoKHR::default().quality_level(self.quality_level);
        let mut qp_params = vk::VideoEncodeQuantizationMapSessionParametersCreateInfoKHR::default().quantization_map_texel_size(self.qp_map.as_ref().map_or(vk::Extent2D::default(), |m| m.texel));
        let mut info = vk::VideoSessionParametersCreateInfoKHR::default()
            .video_session(self.session)
            .next_video_encode_h264_session_parameters_create_info_khr(&mut h264_params)
            .next_video_encode_quality_level_info_khr(&mut quality);
        if self.qp_map.is_some() {
            // pictures may carry a QP delta map of this texel size
            info = info.flags(vk::VideoSessionParametersCreateFlagsKHR::QUANTIZATION_MAP_COMPATIBLE).next_video_encode_quantization_map_session_parameters_create_info_khr(&mut qp_params);
        }
        self.params = self.dev.raw.create_video_session_parameters(&info, None).map_err(|e| format!("vkCreateVideoSessionParametersKHR: {e:?}"))?;
        // our parameter sets, checked against the driver's encoding of the same parameters
        // our writer describes macroblock-rounded coding; a coarser granularity is the driver's
        let ours = if coded == [size[0].div_ceil(16) * 16, size[1].div_ceil(16) * 16] { params.parameter_sets() } else { Vec::new() };
        let mut h264_get = vk::VideoEncodeH264SessionParametersGetInfoKHR::default().write_std_sps(true).write_std_pps(true);
        let mut h264_feedback = vk::VideoEncodeH264SessionParametersFeedbackInfoKHR::default();
        let mut feedback_info = vk::VideoEncodeSessionParametersFeedbackInfoKHR::default().next_video_encode_h264_session_parameters_feedback_info_khr(&mut h264_feedback);
        let driver = self.dev.raw.get_encoded_video_session_parameters(
            &vk::VideoEncodeSessionParametersGetInfoKHR::default().video_session_parameters(self.params).next_video_encode_h264_session_parameters_get_info_khr(&mut h264_get),
            &mut feedback_info,
        );
        self.parameter_sets = match driver {
            Ok(driver) if feedback_info.has_overrides != 0 || ours.is_empty() => {
                eprintln!(
                    "[hal-video] the encoder overrode parameters (sps {}, pps {}): its parameter sets are used ({} bytes; ours {} bytes)",
                    h264_feedback.has_std_sps_overrides, h264_feedback.has_std_pps_overrides, driver.len(), ours.len()
                );
                driver
            }
            Ok(driver) => {
                if driver != ours {
                    eprintln!("[hal-video] parameter sets: ours {} bytes, the driver's {} bytes (no overrides; ours are sent)", ours.len(), driver.len());
                }
                ours
            }
            Err(e) if !ours.is_empty() => {
                eprintln!("[hal-video] vkGetEncodedVideoSessionParametersKHR: {e:?}; ours are sent");
                ours
            }
            Err(e) => return Err(format!("vkGetEncodedVideoSessionParametersKHR: {e:?}")),
        };
        Ok(())
    }

    /// The H.265 VPS, SPS and PPS, as the driver encodes them: one 8-bit layer, no reordering
    /// (each P picture references the one before, its reference picture set in the slice
    /// header), 32x32 coding tree blocks, SAO, transform skip (sharp text) and the BT.709
    /// limited range VUI; the conformance window crops the coded size to the picture.
    unsafe fn h265_parameters(&mut self, h265_caps: &vk::VideoEncodeH265CapabilitiesKHR) -> Result<(), String> {
        let (size, coded, full_chroma) = (self.size, self.coded, self.full_chroma);
        let level = h265_level(size[0], size[1], self.fps).min(h265_caps.max_level_idc.as_raw());
        let profile_idc = if full_chroma { vk::StdVideoH265ProfileIdc::FORMAT_RANGE_EXTENSIONS } else { vk::StdVideoH265ProfileIdc::MAIN };
        let ptl = vk::StdVideoH265ProfileTierLevel {
            flags: vk::StdVideoH265ProfileTierLevelFlags::default().general_progressive_source_flag(1).general_frame_only_constraint_flag(1),
            general_profile_idc: profile_idc,
            general_level_idc: vk::StdVideoH265LevelIdc::from_raw(level),
        };
        // two pictures in the decoder: this one and its reference; output at once
        let mut dpbm = vk::StdVideoH265DecPicBufMgr { ..Default::default() };
        dpbm.max_dec_pic_buffering_minus1[0] = 1;
        let vps = vk::StdVideoH265VideoParameterSet {
            flags: vk::StdVideoH265VpsFlags::default().vps_temporal_id_nesting_flag(1).vps_sub_layer_ordering_info_present_flag(1),
            p_dec_pic_buf_mgr: &dpbm,
            p_profile_tier_level: &ptl,
            ..Default::default()
        };
        let vui = vk::StdVideoH265SequenceParameterSetVui {
            flags: vk::StdVideoH265SpsVuiFlags::default().video_signal_type_present_flag(1).colour_description_present_flag(1),
            aspect_ratio_idc: vk::StdVideoH265AspectRatioIdc::SQUARE,
            video_format: 5,
            colour_primaries: 1,
            transfer_characteristics: 1,
            matrix_coeffs: 1,
            ..Default::default()
        };
        let sub = if full_chroma { 1 } else { 2 };
        let (crop_right, crop_bottom) = ((coded[0] - size[0]) / sub, (coded[1] - size[1]) / sub);
        // CTB 32 (all NVIDIA has), coding blocks 8..32, transforms 4..32
        let sps = vk::StdVideoH265SequenceParameterSet {
            flags: vk::StdVideoH265SpsFlags::default()
                .sps_temporal_id_nesting_flag(1)
                .sps_sub_layer_ordering_info_present_flag(1)
                .conformance_window_flag((crop_right != 0 || crop_bottom != 0) as u32)
                .sample_adaptive_offset_enabled_flag(1)
                .amp_enabled_flag(1)
                .vui_parameters_present_flag(1),
            chroma_format_idc: if full_chroma { vk::StdVideoH265ChromaFormatIdc::TYPE_444 } else { vk::StdVideoH265ChromaFormatIdc::TYPE_420 },
            pic_width_in_luma_samples: coded[0],
            pic_height_in_luma_samples: coded[1],
            log2_max_pic_order_cnt_lsb_minus4: (LOG2_MAX_POC_LSB - 4) as u8,
            log2_min_luma_coding_block_size_minus3: 0,
            log2_diff_max_min_luma_coding_block_size: 2,
            log2_min_luma_transform_block_size_minus2: 0,
            log2_diff_max_min_luma_transform_block_size: 3,
            max_transform_hierarchy_depth_inter: 3,
            max_transform_hierarchy_depth_intra: 3,
            conf_win_right_offset: crop_right,
            conf_win_bottom_offset: crop_bottom,
            p_profile_tier_level: &ptl,
            p_dec_pic_buf_mgr: &dpbm,
            p_sequence_parameter_set_vui: &vui,
            ..Default::default()
        };
        // VK_VIDEO_ENCODE_H265_STD_TRANSFORM_SKIP_ENABLED_FLAG_SET_BIT_KHR (bit 10)
        let skip = h265_caps.std_syntax_flags.as_raw() & (1 << 10) != 0;
        let pps = vk::StdVideoH265PictureParameterSet {
            flags: vk::StdVideoH265PpsFlags::default()
                .cu_qp_delta_enabled_flag(1)
                .transform_skip_enabled_flag(skip as u32)
                .pps_loop_filter_across_slices_enabled_flag(1)
                .deblocking_filter_control_present_flag(1),
            diff_cu_qp_delta_depth: 0,
            ..Default::default()
        };
        let (vps_list, sps_list, pps_list) = ([vps], [sps], [pps]);
        let add = vk::VideoEncodeH265SessionParametersAddInfoKHR::default().std_vp_ss(&vps_list).std_sp_ss(&sps_list).std_pp_ss(&pps_list);
        let mut h265_params = vk::VideoEncodeH265SessionParametersCreateInfoKHR::default().max_std_vps_count(1).max_std_sps_count(1).max_std_pps_count(1).parameters_add_info(&add);
        let mut quality = vk::VideoEncodeQualityLevelInfoKHR::default().quality_level(self.quality_level);
        let mut qp_params = vk::VideoEncodeQuantizationMapSessionParametersCreateInfoKHR::default().quantization_map_texel_size(self.qp_map.as_ref().map_or(vk::Extent2D::default(), |m| m.texel));
        let mut info = vk::VideoSessionParametersCreateInfoKHR::default()
            .video_session(self.session)
            .next_video_encode_h265_session_parameters_create_info_khr(&mut h265_params)
            .next_video_encode_quality_level_info_khr(&mut quality);
        if self.qp_map.is_some() {
            // pictures may carry a QP delta map of this texel size
            info = info.flags(vk::VideoSessionParametersCreateFlagsKHR::QUANTIZATION_MAP_COMPATIBLE).next_video_encode_quantization_map_session_parameters_create_info_khr(&mut qp_params);
        }
        self.params = self.dev.raw.create_video_session_parameters(&info, None).map_err(|e| format!("vkCreateVideoSessionParametersKHR (H.265): {e:?}"))?;
        let mut get = vk::VideoEncodeH265SessionParametersGetInfoKHR::default().write_std_vps(true).write_std_sps(true).write_std_pps(true);
        let mut h265_feedback = vk::VideoEncodeH265SessionParametersFeedbackInfoKHR::default();
        let mut feedback_info = vk::VideoEncodeSessionParametersFeedbackInfoKHR::default().next_video_encode_h265_session_parameters_feedback_info_khr(&mut h265_feedback);
        self.parameter_sets = self
            .dev
            .raw
            .get_encoded_video_session_parameters(
                &vk::VideoEncodeSessionParametersGetInfoKHR::default().video_session_parameters(self.params).next_video_encode_h265_session_parameters_get_info_khr(&mut get),
                &mut feedback_info,
            )
            .map_err(|e| format!("vkGetEncodedVideoSessionParametersKHR (H.265): {e:?}"))?;
        if feedback_info.has_overrides != 0 {
            eprintln!(
                "[hal-video] H.265: the encoder overrode parameters (vps {}, sps {}, pps {}); its parameter sets are sent ({} bytes)",
                h265_feedback.has_std_vps_overrides, h265_feedback.has_std_sps_overrides, h265_feedback.has_std_pps_overrides, self.parameter_sets.len()
            );
        }
        Ok(())
    }

    fn rate_control_layer(&self, kbps: u32) -> vk::VideoEncodeRateControlLayerInfoKHR {
        let average = (kbps as u64 * 1000).min(self.max_bitrate.max(1));
        // CBR: every picture's share of the rate, a burst only as large as the buffer; VBR:
        // a moving picture may take twice the average while a still one takes nothing
        let peak = if self.tuning.constant { average } else { (average * 2).min(self.max_bitrate.max(1)) };
        vk::VideoEncodeRateControlLayerInfoKHR::default()
            .average_bitrate(average)
            .max_bitrate(peak)
            .frame_rate_numerator(self.fps)
            .frame_rate_denominator(1)
    }

    fn rate_control_mode(&self) -> vk::VideoEncodeRateControlModeFlagsKHR {
        if self.tuning.constant { vk::VideoEncodeRateControlModeFlagsKHR::CBR } else { vk::VideoEncodeRateControlModeFlagsKHR::VBR }
    }

    /// The rate control's virtual buffer (ms): `buffer_pictures` pictures' time, and its
    /// initial fullness half of it.
    fn buffer_ms(&self) -> (u32, u32) {
        let ms = (self.tuning.buffer_pictures.max(1) * 1000 / self.fps).max(1);
        (ms, (ms / 2).max(1))
    }

    /// Records the encode of `pic` into its slot's command buffer.
    unsafe fn record(&self, pic: &Picture) -> Result<(), String> {
        let d = &self.dev.raw;
        let slot = &self.slots[pic.slot];
        d.reset_command_buffer(slot.cb, vk::CommandBufferResetFlags::empty()).map_err(|e| format!("vkResetCommandBuffer: {e:?}"))?;
        d.begin_command_buffer(slot.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(|e| format!("vkBeginCommandBuffer: {e:?}"))?;
        d.cmd_reset_query_pool(slot.cb, self.queries, pic.slot as u32, 1);
        if let Some(stamps) = self.stamps {
            d.cmd_reset_query_pool(slot.cb, stamps, 0, 2);
            d.cmd_write_timestamp(slot.cb, vk::PipelineStageFlags::TOP_OF_PIPE, stamps, 0);
        }
        if pic.reset {
            // the DPB images start in the DPB layout
            let barriers: Vec<_> = self
                .dpb
                .iter()
                .map(|img| {
                    vk::ImageMemoryBarrier::default()
                        .image(img.image)
                        .old_layout(vk::ImageLayout::UNDEFINED)
                        .new_layout(vk::ImageLayout::VIDEO_ENCODE_DPB_KHR)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                        .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1))
                })
                .collect();
            d.cmd_pipeline_barrier(slot.cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::ALL_COMMANDS, vk::DependencyFlags::empty(), &[], &[], &barriers);
        }
        let extent = vk::Extent2D { width: self.coded[0], height: self.coded[1] };
        let resource = |view: vk::ImageView| vk::VideoPictureResourceInfoKHR::default().coded_extent(extent).image_view_binding(view);
        let setup_resource = resource(self.dpb[pic.setup].view);
        let ref_resource = pic.reference.map(|(s, _, _)| resource(self.dpb[s].view));
        let src_resource = resource(slot.src.view);

        // the coding scope: the reconstructed picture (no slot yet) and the reference
        let mut begin_slots = vec![vk::VideoReferenceSlotInfoKHR::default().slot_index(-1).picture_resource(&setup_resource)];
        if let (Some((s, _, _)), Some(r)) = (pic.reference, ref_resource.as_ref()) {
            begin_slots.push(vk::VideoReferenceSlotInfoKHR::default().slot_index(s as i32).picture_resource(r));
        }
        // the per-layer codec info goes with every layer (NVIDIA's driver fails the command
        // buffer without it, although the spec leaves it optional)
        let mut layer_h264_now = vk::VideoEncodeH264RateControlLayerInfoKHR::default();
        let mut layer_h265_now = vk::VideoEncodeH265RateControlLayerInfoKHR::default();
        qp_ceiling(&mut layer_h264_now, &mut layer_h265_now, pic.max_qp);
        let layer_now = pic.current_kbps.map(|k| {
            let layer = self.rate_control_layer(k);
            [match self.codec { Codec::H264 => layer.next_video_encode_h264_rate_control_layer_info_khr(&mut layer_h264_now), Codec::H265 => layer.next_video_encode_h265_rate_control_layer_info_khr(&mut layer_h265_now) }]
        });
        // no GOP hints: the reference pattern hints come only with a regular GOP, and this stream's
        // GOP is open-ended (an IDR on request)
        let mut h264_rc_now = vk::VideoEncodeH264RateControlInfoKHR::default().gop_frame_count(u32::MAX).idr_period(u32::MAX).temporal_layer_count(1);
        let mut h265_rc_now = vk::VideoEncodeH265RateControlInfoKHR::default().gop_frame_count(u32::MAX).idr_period(u32::MAX).sub_layer_count(1);
        let mut rc_now = vk::VideoEncodeRateControlInfoKHR::default();
        let mut begin = vk::VideoBeginCodingInfoKHR::default().video_session(self.session).video_session_parameters(self.params).reference_slots(&begin_slots);
        let (buffer_ms, initial_ms) = self.buffer_ms();
        if pic.rc_off_now {
            rc_now = rc_now.rate_control_mode(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED);
            begin = begin.next_video_encode_rate_control_info_khr(&mut rc_now);
        } else if let Some(layer) = layer_now.as_ref() {
            rc_now = rc_now.rate_control_mode(self.rate_control_mode()).layers(layer).virtual_buffer_size_in_ms(buffer_ms).initial_virtual_buffer_size_in_ms(initial_ms);
            begin = begin.next_video_encode_rate_control_info_khr(&mut rc_now);
            begin = match self.codec {
                Codec::H264 => begin.next_video_encode_h264_rate_control_info_khr(&mut h264_rc_now),
                Codec::H265 => begin.next_video_encode_h265_rate_control_info_khr(&mut h265_rc_now),
            };
        }
        // a QP delta map set since the last picture: into its image (outside the coding scope)
        if let Some(map) = self.qp_map.as_ref().filter(|m| m.pending.get()) {
            let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
            let old = if map.uploaded.get() { vk::ImageLayout::VIDEO_ENCODE_QUANTIZATION_MAP_KHR } else { vk::ImageLayout::UNDEFINED };
            let to_copy = vk::ImageMemoryBarrier::default()
                .image(map.image.image)
                .old_layout(old)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .subresource_range(range);
            d.cmd_pipeline_barrier(slot.cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[to_copy]);
            let layer = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
            let region = vk::BufferImageCopy::default().image_subresource(layer).image_extent(vk::Extent3D { width: map.extent.width, height: map.extent.height, depth: 1 });
            d.cmd_copy_buffer_to_image(slot.cb, map.staging, map.image.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
            let to_map = vk::ImageMemoryBarrier::default()
                .image(map.image.image)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::VIDEO_ENCODE_QUANTIZATION_MAP_KHR)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::MEMORY_READ)
                .subresource_range(range);
            d.cmd_pipeline_barrier(slot.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::ALL_COMMANDS, vk::DependencyFlags::empty(), &[], &[], &[to_map]);
            map.pending.set(false);
            map.uploaded.set(true);
        }
        d.cmd_begin_video_coding(slot.cb, &begin);
        if pic.refine {
            // rate control off for this picture: its slices carry REFINE_QP
            let mut rc = vk::VideoEncodeRateControlInfoKHR::default().rate_control_mode(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED);
            let control = vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL).next_video_encode_rate_control_info_khr(&mut rc);
            d.cmd_control_video_coding(slot.cb, &control);
        } else if pic.reset || pic.bitrate.is_some() {
            let kbps = pic.bitrate.or(pic.current_kbps).unwrap_or(0);
            let mut layer_h264 = vk::VideoEncodeH264RateControlLayerInfoKHR::default();
            let mut layer_h265 = vk::VideoEncodeH265RateControlLayerInfoKHR::default();
            qp_ceiling(&mut layer_h264, &mut layer_h265, pic.max_qp);
            let layer = self.rate_control_layer(kbps);
            let layer = [match self.codec { Codec::H264 => layer.next_video_encode_h264_rate_control_layer_info_khr(&mut layer_h264), Codec::H265 => layer.next_video_encode_h265_rate_control_layer_info_khr(&mut layer_h265) }];
            let mut h264_rc = vk::VideoEncodeH264RateControlInfoKHR::default().gop_frame_count(u32::MAX).idr_period(u32::MAX).temporal_layer_count(1);
            let mut h265_rc = vk::VideoEncodeH265RateControlInfoKHR::default().gop_frame_count(u32::MAX).idr_period(u32::MAX).sub_layer_count(1);
            let mut rc = vk::VideoEncodeRateControlInfoKHR::default().rate_control_mode(self.rate_control_mode()).layers(&layer).virtual_buffer_size_in_ms(buffer_ms).initial_virtual_buffer_size_in_ms(initial_ms);
            let mut quality = vk::VideoEncodeQualityLevelInfoKHR::default().quality_level(self.quality_level);
            let mut flags = vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL;
            let mut control = vk::VideoCodingControlInfoKHR::default();
            if pic.reset {
                flags |= vk::VideoCodingControlFlagsKHR::RESET | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL;
                control = control.next_video_encode_quality_level_info_khr(&mut quality);
            }
            control = control.flags(flags).next_video_encode_rate_control_info_khr(&mut rc);
            control = match self.codec {
                Codec::H264 => control.next_video_encode_h264_rate_control_info_khr(&mut h264_rc),
                Codec::H265 => control.next_video_encode_h265_rate_control_info_khr(&mut h265_rc),
            };
            d.cmd_control_video_coding(slot.cb, &control);
        }

        // the picture: reference lists always (empty for an IDR: the driver reads them anyway)
        let qp = if pic.refine { REFINE_QP } else { 0 };
        // H.264
        let mut l0 = [0xffu8; vk::STD_VIDEO_H264_MAX_NUM_LIST_REF as usize];
        if let Some((s, _, _)) = pic.reference {
            l0[0] = s as u8;
        }
        let ref_lists = vk::StdVideoEncodeH264ReferenceListsInfo {
            ref_pic_list0: l0,
            ref_pic_list1: [0xff; vk::STD_VIDEO_H264_MAX_NUM_LIST_REF as usize],
            ..Default::default()
        };
        let std_picture = vk::StdVideoEncodeH264PictureInfo {
            flags: vk::StdVideoEncodeH264PictureInfoFlags::default().idr_pic_flag(pic.idr as u32).is_reference(1),
            idr_pic_id: pic.idr_pic_id,
            primary_pic_type: if pic.idr { vk::StdVideoH264PictureType::IDR } else { vk::StdVideoH264PictureType::P },
            frame_num: pic.frame_num,
            pic_order_cnt: pic.poc,
            p_ref_lists: &ref_lists as *const _,
            ..Default::default()
        };
        let slice_header = vk::StdVideoEncodeH264SliceHeader {
            slice_type: if pic.idr { vk::StdVideoH264SliceType::I } else { vk::StdVideoH264SliceType::P },
            cabac_init_idc: vk::StdVideoH264CabacInitIdc::TYPE_0,
            ..Default::default()
        };
        let slices = [vk::VideoEncodeH264NaluSliceInfoKHR::default().std_slice_header(&slice_header).constant_qp(qp)];
        let mut h264_picture = vk::VideoEncodeH264PictureInfoKHR::default().nalu_slice_entries(&slices).std_picture_info(&std_picture);
        let setup_info = vk::StdVideoEncodeH264ReferenceInfo {
            primary_pic_type: std_picture.primary_pic_type,
            frame_num: pic.frame_num,
            pic_order_cnt: pic.poc,
            ..Default::default()
        };
        let mut setup_dpb = vk::VideoEncodeH264DpbSlotInfoKHR::default().std_reference_info(&setup_info);
        let ref_info = pic.reference.map(|(_, frame_num, poc)| vk::StdVideoEncodeH264ReferenceInfo {
            primary_pic_type: if frame_num == 0 { vk::StdVideoH264PictureType::IDR } else { vk::StdVideoH264PictureType::P },
            frame_num,
            pic_order_cnt: poc,
            ..Default::default()
        });
        let mut ref_dpb = ref_info.as_ref().map(|r| vk::VideoEncodeH264DpbSlotInfoKHR::default().std_reference_info(r));
        // H.265: POC counts pictures from the IDR; a P picture's reference picture set is the one
        // picture before it (in its slice header, no sets in the SPS)
        let mut l0_265 = [0xffu8; vk::STD_VIDEO_H265_MAX_NUM_LIST_REF as usize];
        if let Some((s, _, _)) = pic.reference {
            l0_265[0] = s as u8;
        }
        let ref_lists_265 = vk::StdVideoEncodeH265ReferenceListsInfo {
            ref_pic_list0: l0_265,
            ref_pic_list1: [0xff; vk::STD_VIDEO_H265_MAX_NUM_LIST_REF as usize],
            ..Default::default()
        };
        let mut rps = vk::StdVideoH265ShortTermRefPicSet { ..Default::default() };
        if pic.reference.is_some() {
            rps.num_negative_pics = 1;
            rps.used_by_curr_pic_s0_flag = 1;
            rps.delta_poc_s0_minus1[0] = 0;
        }
        let std_picture_265 = vk::StdVideoEncodeH265PictureInfo {
            flags: vk::StdVideoEncodeH265PictureInfoFlags::default().is_reference(1).irap_pic_flag(pic.idr as u32).pic_output_flag(1),
            pic_type: if pic.idr { vk::StdVideoH265PictureType::IDR } else { vk::StdVideoH265PictureType::P },
            pic_order_cnt_val: pic.poc,
            p_ref_lists: &ref_lists_265,
            p_short_term_ref_pic_set: &rps,
            ..Default::default()
        };
        let segment_header = vk::StdVideoEncodeH265SliceSegmentHeader {
            flags: vk::StdVideoEncodeH265SliceSegmentHeaderFlags::default()
                .first_slice_segment_in_pic_flag(1)
                .slice_sao_luma_flag(1)
                .slice_sao_chroma_flag(1)
                .slice_loop_filter_across_slices_enabled_flag(1),
            slice_type: if pic.idr { vk::StdVideoH265SliceType::I } else { vk::StdVideoH265SliceType::P },
            max_num_merge_cand: 5,
            ..Default::default()
        };
        let segments = [vk::VideoEncodeH265NaluSliceSegmentInfoKHR::default().std_slice_segment_header(&segment_header).constant_qp(qp)];
        let mut h265_picture = vk::VideoEncodeH265PictureInfoKHR::default().nalu_slice_segment_entries(&segments).std_picture_info(&std_picture_265);
        let setup_info_265 = vk::StdVideoEncodeH265ReferenceInfo { pic_type: std_picture_265.pic_type, pic_order_cnt_val: pic.poc, ..Default::default() };
        let mut setup_dpb_265 = vk::VideoEncodeH265DpbSlotInfoKHR::default().std_reference_info(&setup_info_265);
        let ref_info_265 = pic.reference.map(|(_, _, poc)| vk::StdVideoEncodeH265ReferenceInfo {
            pic_type: if poc == 0 { vk::StdVideoH265PictureType::IDR } else { vk::StdVideoH265PictureType::P },
            pic_order_cnt_val: poc,
            ..Default::default()
        });
        let mut ref_dpb_265 = ref_info_265.as_ref().map(|r| vk::VideoEncodeH265DpbSlotInfoKHR::default().std_reference_info(r));

        let setup_slot = vk::VideoReferenceSlotInfoKHR::default().slot_index(pic.setup as i32).picture_resource(&setup_resource);
        let setup_slot = match self.codec {
            Codec::H264 => setup_slot.next_video_encode_h264_dpb_slot_info_khr(&mut setup_dpb),
            Codec::H265 => setup_slot.next_video_encode_h265_dpb_slot_info_khr(&mut setup_dpb_265),
        };
        let mut ref_slots = Vec::new();
        if let (Some((s, _, _)), Some(r)) = (pic.reference, ref_resource.as_ref()) {
            let slot_info = vk::VideoReferenceSlotInfoKHR::default().slot_index(s as i32).picture_resource(r);
            match (self.codec, ref_dpb.as_mut(), ref_dpb_265.as_mut()) {
                (Codec::H264, Some(dpb), _) => ref_slots.push(slot_info.next_video_encode_h264_dpb_slot_info_khr(dpb)),
                (Codec::H265, _, Some(dpb)) => ref_slots.push(slot_info.next_video_encode_h265_dpb_slot_info_khr(dpb)),
                _ => {}
            }
        }
        let encode = vk::VideoEncodeInfoKHR::default()
            .dst_buffer(slot.bitstream)
            .dst_buffer_offset(0)
            .dst_buffer_range(BITSTREAM_BYTES)
            .src_picture_resource(src_resource)
            .setup_reference_slot(&setup_slot)
            .reference_slots(&ref_slots);
        let encode = match self.codec {
            Codec::H264 => encode.next_video_encode_h264_picture_info_khr(&mut h264_picture),
            Codec::H265 => encode.next_video_encode_h265_picture_info_khr(&mut h265_picture),
        };
        // the picture's QP deltas per region, once a map was set
        let mut qp_info = vk::VideoEncodeQuantizationMapInfoKHR::default();
        let encode = match self.qp_map.as_ref().filter(|m| m.uploaded.get()) {
            Some(map) => {
                qp_info = qp_info.quantization_map(map.image.view).quantization_map_extent(map.extent);
                encode.flags(vk::VideoEncodeFlagsKHR::WITH_QUANTIZATION_DELTA_MAP).next_video_encode_quantization_map_info_khr(&mut qp_info)
            }
            None => encode,
        };
        d.cmd_begin_query(slot.cb, self.queries, pic.slot as u32, vk::QueryControlFlags::empty());
        d.cmd_encode_video(slot.cb, &encode);
        d.cmd_end_query(slot.cb, self.queries, pic.slot as u32);
        d.cmd_end_video_coding(slot.cb, &vk::VideoEndCodingInfoKHR::default());
        if let Some(stamps) = self.stamps {
            d.cmd_write_timestamp(slot.cb, vk::PipelineStageFlags::BOTTOM_OF_PIPE, stamps, 1);
        }
        d.end_command_buffer(slot.cb).map_err(|e| format!("vkEndCommandBuffer (encode): {e:?}"))
    }

    /// Submits `pic` on the video queue once `copied` (the copy of its picture into the
    /// encode source) is signalled, waits for it, and returns its bitstream.
    unsafe fn encode(&self, pic: &Picture, copied: vk::Semaphore) -> Result<Vec<u8>, String> {
        let d = &self.dev.raw;
        self.record(pic)?;
        let slot = &self.slots[pic.slot];
        let video = self.dev.video_encode.as_ref().unwrap();
        let cbs = [slot.cb];
        let stages = [vk::PipelineStageFlags::ALL_COMMANDS];
        let sems = [copied];
        let info = vk::SubmitInfo::default().command_buffers(&cbs).wait_semaphores(&sems).wait_dst_stage_mask(&stages);
        let (queue, queue_lock) = match self.queue_index.checked_sub(1).and_then(|i| video.more.get(i)) {
            Some((queue, l)) => (*queue, l),
            None => (video.queue, &video.lock),
        };
        {
            let _q = lock(queue_lock);
            d.queue_submit(queue, &[info], slot.fence).map_err(|e| format!("vkQueueSubmit (video): {e:?}"))?;
        }
        loop {
            match d.wait_for_fences(&[slot.fence], true, 1_000_000_000) {
                Ok(()) => break,
                Err(vk::Result::TIMEOUT) => {
                    if self.dev.lost.load(Ordering::Acquire) {
                        return Err("device lost".into());
                    }
                }
                Err(e) => return Err(format!("vkWaitForFences (video): {e:?}")),
            }
        }
        d.reset_fences(&[slot.fence]).map_err(error)?;
        if let Some(stamps) = self.stamps {
            let mut t = [0u64; 2];
            let results = d.table.get_query_pool_results.ok_or("vkGetQueryPoolResults is unavailable")?;
            if results(d.handle(), stamps, 0, 2, std::mem::size_of_val(&t), t.as_mut_ptr().cast(), 8, vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT) == vk::Result::SUCCESS {
                let bits = video.timestamp_bits;
                let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
                self.last_encode_ns.set(Some(((t[1].wrapping_sub(t[0]) & mask) as f64 * self.dev.properties.limits.timestamp_period as f64) as u64));
            }
        }
        // one query, three 32-bit values: bitstream offset, bytes written, status
        let mut result = [0u32; 3];
        let results = d.table.get_query_pool_results.ok_or("vkGetQueryPoolResults is unavailable")?;
        let flags = vk::QueryResultFlags::WAIT | vk::QueryResultFlags::WITH_STATUS_KHR;
        results(d.handle(), self.queries, pic.slot as u32, 1, std::mem::size_of_val(&result), result.as_mut_ptr().cast(), std::mem::size_of_val(&result) as u64, flags).result().map_err(|e| format!("vkGetQueryPoolResults: {e:?}"))?;
        let (offset, written, status) = (result[0] as usize, result[1] as usize, result[2] as i32);
        if status <= 0 {
            return Err(format!("encode status {status}"));
        }
        if offset + written > BITSTREAM_BYTES as usize {
            return Err(format!("bitstream overflow (feedback {result:?})"));
        }
        if !slot.bitstream_coherent {
            d.invalidate_mapped_memory_ranges(&[vk::MappedMemoryRange::default().memory(slot.bitstream_memory).size(vk::WHOLE_SIZE)]).map_err(error)?;
        }
        let bytes = std::slice::from_raw_parts((slot.mapped as *const u8).add(offset), written);
        let mut out = Vec::with_capacity(written + if pic.idr { self.parameter_sets.len() } else { 0 });
        if pic.idr {
            out.extend_from_slice(&self.parameter_sets);
        }
        out.extend_from_slice(bytes);
        Ok(out)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            let d = &self.dev.raw;
            for s in &self.slots {
                // a slot's last picture finished before its job returned; the fences are idle
                d.unmap_memory(s.bitstream_memory);
                d.destroy_fence(s.fence, None);
                d.destroy_buffer(s.bitstream, None);
                d.free_memory(s.bitstream_memory, None);
                d.destroy_buffer(s.staging, None);
                d.free_memory(s.staging_memory, None);
                d.destroy_image_view(s.src.view, None);
                d.destroy_image(s.src.image, None);
                d.free_memory(s.src.memory, None);
            }
            for i in &self.dpb {
                d.destroy_image_view(i.view, None);
                d.destroy_image(i.image, None);
                d.free_memory(i.memory, None);
            }
            if let Some(m) = &self.qp_map {
                d.unmap_memory(m.staging_memory);
                d.destroy_buffer(m.staging, None);
                d.free_memory(m.staging_memory, None);
                d.destroy_image_view(m.image.view, None);
                d.destroy_image(m.image.image, None);
                d.free_memory(m.image.memory, None);
            }
            if self.queries != vk::QueryPool::null() {
                d.destroy_query_pool(self.queries, None);
                if let Some(stamps) = self.stamps {
                    d.destroy_query_pool(stamps, None);
                }
            }
            if self.pool != vk::CommandPool::null() {
                d.destroy_command_pool(self.pool, None);
            }
            if self.params != vk::VideoSessionParametersKHR::null() {
                d.destroy_video_session_parameters(self.params, None);
            }
            d.destroy_video_session(self.session, None);
            for m in &self.session_memory {
                d.free_memory(*m, None);
            }
        }
    }
}

/// A live H.264 stream on the process's Vulkan device, outside any session: pictures come
/// from a registered DMA-BUF image holding NV12 packed into BGRA8 texels (the layout of
/// `makepad_video::GpuPictureMaker::nv12_picture`: `w/4` texels a row, the `h` luma rows,
/// then the `h/2` interleaved chroma rows) or from CPU NV12 bytes. Each call returns that
/// picture's Annex-B access unit (parameter sets before every IDR).
pub(crate) struct StreamEncoder {
    enc: Encoder,
    copy_pool: vk::CommandPool,
    copy_cb: vk::CommandBuffer,
    /// The copy into the encode source is done (the encode waits for it).
    copied: vk::Semaphore,
    /// The picture's producer is done: a sync_file imported for each picture that brings one.
    ready: vk::Semaphore,
    /// CPU pictures' upload buffer (made at the first one).
    upload: Option<(vk::Buffer, vk::DeviceMemory, usize, bool)>,
    count: u32,
    frame_num: u32,
    /// H.265's picture order count (pictures since the IDR).
    poc_count: i32,
    idr_pic_id: u16,
    /// The DPB slot holding the last picture, its frame_num and POC.
    last: Option<(usize, u32, i32)>,
    kbps: u32,
    /// The rate control the session has (None before the first picture reset it).
    applied_kbps: Option<u32>,
    applied_max_qp: u32,
    max_qp: u32,
    /// What the encoder is, for logs and status lines.
    pub info: String,
    /// GPU timestamps around the copy (graphics queue): where a picture's time goes.
    stamps: vk::QueryPool,
    /// The session's rate control is off (the last picture was a refinement).
    rc_off: bool,
    timing: Timing,
}

unsafe impl Send for StreamEncoder {}

/// Where texture pictures' encode time went, summed over a window, written to the log every
/// `TIMING_WINDOW_NS` while pictures come: the whole call (frame fence imported to bitstream
/// read), the copy on the GPU, how often the frame was not yet finished at submit.
#[derive(Default)]
struct Timing {
    since_ns: u64,
    pictures: u32,
    total_ns: u64,
    copy_ns: u64,
    encode_ns: u64,
    frame_pending: u32,
}

const TIMING_WINDOW_NS: u64 = 10_000_000_000;

impl Timing {
    fn add(&mut self, info: &str, total_ns: u64, copy_ns: u64, encode_ns: u64, frame_pending: bool) {
        let now = crate::clock_ns();
        if self.pictures == 0 {
            self.since_ns = now;
        }
        self.pictures += 1;
        self.total_ns += total_ns;
        self.copy_ns += copy_ns;
        self.encode_ns += encode_ns;
        self.frame_pending += frame_pending as u32;
        if now - self.since_ns >= TIMING_WINDOW_NS {
            let n = self.pictures as f64;
            eprintln!(
                "[hal-video] {}: {} pictures, mean {:.2} ms from the call to the bitstream: the copy {:.2} ms and the encode {:.2} ms on the GPU, the rest waiting (the frame was still rendering at submit for {} of them)",
                info, self.pictures, self.total_ns as f64 / n / 1e6, self.copy_ns as f64 / n / 1e6, self.encode_ns as f64 / n / 1e6, self.frame_pending
            );
            *self = Timing::default();
        }
    }
}

fn default_kbps_for(w: u32, h: u32, fps: u32, full_chroma: bool) -> u32 {
    default_kbps(w, h, fps, full_chroma)
}

impl StreamEncoder {
    /// Makes the process's Vulkan device offscreen when no backend made one (a tool that encodes
    /// pictures without a window: the encoder bench). False when it cannot.
    pub fn offscreen_device() -> bool {
        Device::existing().is_some() || Device::shared(&[], &[], &mut super::surface::Offscreen).is_ok()
    }

    /// The encode engines (queues) of the process's GPU; 0 without hardware encode.
    pub fn engines() -> u32 {
        Device::existing().and_then(|d| d.video_encode.as_ref().map(|v| 1 + v.more.len() as u32)).unwrap_or(0)
    }

    /// The largest coded picture of the codec's profile (4:4:4 when `full_chroma`) on this GPU.
    pub fn max_size(codec: Codec, full_chroma: bool) -> Option<[u32; 2]> {
        let dev = Device::existing()?;
        if !dev.video_encode.as_ref()?.h265 && codec == Codec::H265 {
            return None;
        }
        let profile = Profile::new(codec, full_chroma, true);
        let mut h264_caps = vk::VideoEncodeH264CapabilitiesKHR::default();
        let mut h265_caps = vk::VideoEncodeH265CapabilitiesKHR::default();
        let mut encode_caps = vk::VideoEncodeCapabilitiesKHR::default();
        let caps = vk::VideoCapabilitiesKHR::default().next_video_encode_capabilities_khr(&mut encode_caps);
        let mut caps = match codec {
            Codec::H264 => caps.next_video_encode_h264_capabilities_khr(&mut h264_caps),
            Codec::H265 => caps.next_video_encode_h265_capabilities_khr(&mut h265_caps),
        };
        unsafe { dev.instance.get_physical_device_video_capabilities(dev.physical, &profile.info, &mut caps) }.ok()?;
        Some([caps.max_coded_extent.width, caps.max_coded_extent.height])
    }

    /// An encoder on engine `engine` (its queue; an engine the GPU does not have is engine 0),
    /// 4:4:4 when `full_chroma`.
    pub fn open(width: u32, height: u32, fps: u32, kbps: u32, engine: usize, full_chroma: bool, codec: Codec, tuning: Tuning) -> Result<StreamEncoder, String> {
        let dev = Device::existing().ok_or("the HAL has no Vulkan device yet (no window rendered)")?;
        let fps = fps.clamp(1, 240);
        let mut enc = Encoder::new(dev.clone(), [width, height], fps, full_chroma, codec, tuning)?;
        enc.queue_index = engine;
        let kbps = if kbps == 0 { default_kbps_for(width, height, fps, full_chroma) } else { kbps };
        unsafe {
            let d = &dev.raw;
            let copy_pool = d
                .create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(dev.family).flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None)
                .map_err(error)?;
            let copy_cb = d.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(copy_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).map_err(error)?[0];
            let copied = d.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).map_err(error)?;
            let ready = d.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).map_err(error)?;
            let stamps = d.create_query_pool(&vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(2), None).map_err(error)?;
            let name = std::ffi::CStr::from_ptr(dev.properties.device_name.as_ptr()).to_string_lossy().into_owned();
            let profile = match (codec, full_chroma) {
                (Codec::H264, false) => "H.264 High",
                (Codec::H264, true) => "H.264 High 4:4:4",
                (Codec::H265, false) => "H.265 Main",
                (Codec::H265, true) => "H.265 RExt 4:4:4",
            };
            let info = format!(
                "{profile} {} {width}x{height} (coded {}x{}) {fps} fps {kbps} kbit/s {} buffer {} pictures, quality {}/{} on {name}",
                if tuning.sharp { "sharp" } else { "fast" }, enc.coded[0], enc.coded[1], if tuning.constant { "CBR" } else { "VBR" }, tuning.buffer_pictures, enc.quality_level, enc.quality_levels - 1
            );
            Ok(StreamEncoder {
                enc,
                copy_pool,
                copy_cb,
                copied,
                ready,
                upload: None,
                count: 0,
                frame_num: 0,
                poc_count: 0,
                idr_pic_id: 0,
                last: None,
                kbps,
                applied_kbps: None,
                applied_max_qp: 0,
                max_qp: 0,
                info,
                stamps,
                rc_off: false,
                timing: Timing::default(),
            })
        }
    }

    /// The last picture's encode on the video queue (ns, GPU timestamps; 0 when not measured).
    pub fn last_gpu_ns(&self) -> u64 {
        self.enc.last_encode_ns.get().unwrap_or(0)
    }

    pub fn set_kbps(&mut self, kbps: u32) {
        if kbps > 0 {
            self.kbps = kbps;
        }
    }

    /// The QP ceiling from the next picture on (0: the encoder's own range).
    pub fn set_max_qp(&mut self, qp: u32) {
        self.max_qp = qp;
    }

    /// The QP delta map's grid, when the encoder takes one: its columns and rows, the pixels each
    /// texel covers, and the deltas it takes (min, max).
    pub fn qp_grid(&self) -> Option<([u32; 2], [u32; 2], [i32; 2])> {
        self.enc.qp_map.as_ref().map(|m| ([m.extent.width, m.extent.height], [m.texel.width, m.texel.height], m.range))
    }

    /// QP deltas for the pictures from the next one on, one per texel of [`Self::qp_grid`] (row
    /// by row), clamped to its range (negative: more bits there). False when the encoder takes
    /// none or the grid differs.
    pub fn set_qp_map(&mut self, deltas: &[i8]) -> bool {
        let Some(m) = self.enc.qp_map.as_ref() else { return false };
        let n = (m.extent.width * m.extent.height) as usize;
        if deltas.len() != n {
            return false;
        }
        unsafe {
            for (i, d) in deltas.iter().enumerate() {
                let v = (*d as i32).clamp(m.range[0], m.range[1]);
                match m.bytes {
                    1 => *(m.mapped as *mut i8).add(i) = v as i8,
                    2 => *(m.mapped as *mut i16).add(i) = v as i16,
                    _ => *(m.mapped as *mut i32).add(i) = v,
                }
            }
        }
        if !m.coherent {
            let flushed = unsafe { self.enc.dev.raw.flush_mapped_memory_ranges(&[vk::MappedMemoryRange::default().memory(m.staging_memory).size(vk::WHOLE_SIZE)]) };
            if flushed.is_err() {
                return false;
            }
        }
        m.pending.set(true);
        true
    }

    /// The picture's place in the stream (an IDR when `key` or the first).
    fn next_picture(&mut self, key: bool, refine: bool) -> Picture {
        let idr = key || self.last.is_none();
        if idr {
            self.frame_num = 0;
            self.poc_count = 0;
            self.idr_pic_id = self.idr_pic_id.wrapping_add(if self.count == 0 { 0 } else { 1 });
        }
        let setup = match self.last {
            Some((s, _, _)) if !idr => 1 - s,
            _ => 0,
        };
        // H.264: POC type 2 (twice frame_num); H.265: pictures since the IDR (never wraps: 2^31
        // pictures)
        let poc = match self.enc.codec { Codec::H264 => (self.frame_num * 2) as i32, Codec::H265 => self.poc_count };
        self.poc_count += 1;
        let max_qp = self.max_qp;
        let pic = Picture {
            slot: 0,
            idr,
            idr_pic_id: self.idr_pic_id,
            frame_num: self.frame_num,
            poc,
            setup,
            reference: if idr { None } else { self.last },
            reset: self.applied_kbps.is_none(),
            // after a refinement the stream's rate control is set again (and with a new QP ceiling)
            bitrate: (self.applied_kbps != Some(self.kbps) || self.rc_off || max_qp != self.applied_max_qp).then_some(self.kbps),
            max_qp,
            current_kbps: self.applied_kbps,
            // a refinement needs a stream (not the first picture, not a key frame)
            refine: refine && !idr && self.applied_kbps.is_some(),
            rc_off_now: self.rc_off,
        };
        self.applied_kbps = Some(self.kbps);
        self.applied_max_qp = max_qp;
        self.rc_off = pic.refine;
        self.last = Some((setup, self.frame_num, poc));
        // frame_num wraps at MaxFrameNum; POC type 2 follows it (2 x FrameNumOffset + frame_num)
        self.frame_num = (self.frame_num + 1) % (1 << LOG2_MAX_FRAME_NUM);
        self.count = self.count.wrapping_add(1);
        pic
    }

    /// After a failed picture the reference chain and the session state are unknown: the next
    /// picture resets the session and is an IDR.
    fn failed(&mut self) {
        self.last = None;
        self.applied_kbps = None;
        self.rc_off = false;
    }

    unsafe fn begin_copy(&self) -> Result<(), String> {
        let d = &self.enc.dev.raw;
        d.reset_command_buffer(self.copy_cb, vk::CommandBufferResetFlags::empty()).map_err(error)?;
        d.begin_command_buffer(self.copy_cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(error)?;
        // the copy's start (after the waits on its submit) and end, on the GPU's clock
        d.cmd_reset_query_pool(self.copy_cb, self.stamps, 0, 2);
        d.cmd_write_timestamp(self.copy_cb, vk::PipelineStageFlags::TOP_OF_PIPE, self.stamps, 0);
        Ok(())
    }

    /// The staged planes (in `buffer`) into the encode source image, which then goes to the
    /// encode layout.
    unsafe fn record_planes(&self, buffer: vk::Buffer) {
        let d = &self.enc.dev.raw;
        let [w, h] = self.enc.size;
        let slot = &self.enc.slots[0];
        let staged = vk::BufferMemoryBarrier::default()
            .buffer(buffer)
            .size(vk::WHOLE_SIZE)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE | vk::AccessFlags::HOST_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let to_dst = vk::ImageMemoryBarrier::default()
            .image(slot.src.image)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .subresource_range(range);
        d.cmd_pipeline_barrier(self.copy_cb, vk::PipelineStageFlags::TRANSFER | vk::PipelineStageFlags::HOST, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[staged], &[to_dst]);
        let plane = |aspect| vk::ImageSubresourceLayers::default().aspect_mask(aspect).layer_count(1);
        d.cmd_copy_buffer_to_image(
            self.copy_cb,
            buffer,
            slot.src.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[
                vk::BufferImageCopy::default().image_subresource(plane(vk::ImageAspectFlags::PLANE_0)).image_extent(vk::Extent3D { width: w, height: h, depth: 1 }),
                vk::BufferImageCopy::default()
                    .buffer_offset(w as u64 * h as u64)
                    .image_subresource(plane(vk::ImageAspectFlags::PLANE_1))
                    .image_extent(if self.enc.full_chroma { vk::Extent3D { width: w, height: h, depth: 1 } } else { vk::Extent3D { width: w / 2, height: h / 2, depth: 1 } }),
            ],
        );
        let to_src = vk::ImageMemoryBarrier::default()
            .image(slot.src.image)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::VIDEO_ENCODE_SRC_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::MEMORY_READ)
            .subresource_range(range);
        d.cmd_pipeline_barrier(self.copy_cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::ALL_COMMANDS, vk::DependencyFlags::empty(), &[], &[], &[to_src]);
    }

    /// Submits the recorded copy on the graphics queue (after `ready` when `wait_ready`),
    /// signalling `copied`, then encodes.
    unsafe fn finish(&mut self, pic: Picture, wait_ready: bool) -> Result<Vec<u8>, String> {
        let dev = self.enc.dev.clone();
        let d = &dev.raw;
        d.cmd_write_timestamp(self.copy_cb, vk::PipelineStageFlags::BOTTOM_OF_PIPE, self.stamps, 1);
        d.end_command_buffer(self.copy_cb).map_err(|e| format!("vkEndCommandBuffer (copy): {e:?}"))?;
        let cbs = [self.copy_cb];
        let signals = [self.copied];
        let waits = [self.ready];
        let stages = [vk::PipelineStageFlags::ALL_COMMANDS];
        let mut info = vk::SubmitInfo::default().command_buffers(&cbs).signal_semaphores(&signals);
        if wait_ready {
            info = info.wait_semaphores(&waits).wait_dst_stage_mask(&stages);
        }
        {
            let _queue = lock(&dev.queue_lock);
            d.queue_submit(dev.queue, &[info], vk::Fence::null()).map_err(|e| format!("vkQueueSubmit (copy): {e:?}"))?;
        }
        let out = self.enc.encode(&pic, self.copied);
        if out.is_err() {
            // the copy was submitted (it signals `copied`, which nothing waits for now): it is done
            // before anything it uses is reused or destroyed
            {
                let _queue = lock(&dev.queue_lock);
                let _ = d.queue_wait_idle(dev.queue);
            }
            self.failed();
        }
        out
    }

    /// A failed encode that saw the device lost marks it so (the HAL replaces it; its own
    /// submits may not see the loss while nothing renders).
    pub fn note_failure(&self, e: &str) {
        if e.contains("ERROR_DEVICE_LOST") {
            self.enc.dev.lost.store(true, Ordering::Release);
        }
    }

    /// Whether this encoder runs on `dev` (after a device loss the process has a new one, and
    /// an encoder of the old one can only fail: it is opened again).
    pub fn on_device(&self, dev: &Arc<Device>) -> bool {
        Arc::ptr_eq(&self.enc.dev, dev) && !self.enc.dev.lost.load(Ordering::Acquire)
    }

    /// A picture from the registered DMA-BUF `image` (`image_size`: its size) holding a
    /// `source_height` tall packed NV12 picture: its rows from `y0` on (this encoder's height of
    /// them: a tile of it, or all of it), once its producer's `sync` (a sync_file; `None` when its
    /// work is known to be complete) has signalled.
    /// `refine`: the same picture as the last one again at a near-lossless QP (a still desktop).
    pub fn encode_rows(&mut self, image: vk::Image, image_size: [u32; 2], source_height: u32, x0: u32, y0: u32, sync: Option<OwnedFd>, key: bool, refine: bool) -> Result<Vec<u8>, String> {
        let [w, h] = self.enc.size;
        let full = self.enc.full_chroma;
        // the packed picture: luma rows, then chroma rows (4:2:0: half as many, each w bytes; 4:4:4:
        // one per luma row, each 2w bytes = two texture rows)
        let chroma_rows = if full { source_height * 2 } else { source_height / 2 };
        if image_size[0] * 4 < x0 + w || image_size[1] < source_height + chroma_rows || y0 + h > source_height || y0 % 2 != 0 || x0 % 4 != 0 {
            return Err(format!("{w}x{h} at {x0},{y0} of a {source_height} tall NV12 picture in a {}x{} texture: out of it", image_size[0], image_size[1]));
        }
        let pic = self.next_picture(key, refine);
        let started = crate::clock_ns();
        // the frame that rendered the picture still running on the GPU when this is called
        let frame_pending = sync.as_ref().is_some_and(|fd| {
            use crate::backend::linux::io::{poll, PollFd, READ};
            use std::os::fd::AsRawFd;
            let mut p = PollFd { fd: fd.as_raw_fd(), events: READ, revents: 0 };
            unsafe { poll(&mut p, 1, 0) == 0 }
        });
        unsafe {
            let dev = self.enc.dev.clone();
            let d = &dev.raw;
            let wait_ready = match sync {
                Some(fd) => {
                    let raw = fd.into_raw_fd();
                    let imported = d.import_semaphore_fd(
                        &vk::ImportSemaphoreFdInfoKHR::default()
                            .semaphore(self.ready)
                            .flags(vk::SemaphoreImportFlags::TEMPORARY)
                            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
                            .fd(raw),
                    );
                    if let Err(e) = imported {
                        // the import did not take the descriptor
                        extern "C" {
                            fn close(fd: i32) -> i32;
                        }
                        close(raw);
                        self.failed();
                        return Err(format!("sync_file import: {e:?}"));
                    }
                    true
                }
                None => false,
            };
            if let Err(e) = self.begin_copy() {
                self.failed();
                return Err(e);
            }
            let slot = &self.enc.slots[0];
            // the texture from the foreign family (its producer), as the HAL's external images go
            let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
            let acquire = vk::ImageMemoryBarrier::default()
                .image(image)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
                .dst_queue_family_index(dev.family)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .subresource_range(range);
            d.cmd_pipeline_barrier(self.copy_cb, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[acquire]);
            // the rows of a BGRA8 texel row are the plane's bytes: luma rows, then chroma rows
            let layer = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
            d.cmd_copy_image_to_buffer(
                self.copy_cb,
                image,
                vk::ImageLayout::GENERAL,
                slot.staging,
                &[
                    vk::BufferImageCopy::default().image_subresource(layer).image_offset(vk::Offset3D { x: (x0 / 4) as i32, y: y0 as i32, z: 0 }).image_extent(vk::Extent3D { width: w / 4, height: h, depth: 1 }),
                    vk::BufferImageCopy::default()
                        .buffer_offset(w as u64 * h as u64)
                        .image_subresource(layer)
                        .image_offset(vk::Offset3D { x: (x0 / 4) as i32, y: (source_height + if full { y0 * 2 } else { y0 / 2 }) as i32, z: 0 })
                        .image_extent(vk::Extent3D { width: w / 4, height: if full { h * 2 } else { h / 2 }, depth: 1 }),
                ],
            );
            let release = vk::ImageMemoryBarrier::default()
                .image(image)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(dev.family)
                .dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .subresource_range(range);
            d.cmd_pipeline_barrier(self.copy_cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[], &[release]);
            self.record_planes(slot.staging);
            let out = self.finish(pic, wait_ready)?;
            let mut stamps = [0u64; 2];
            let results = d.table.get_query_pool_results.ok_or("vkGetQueryPoolResults is unavailable")?;
            let read = results(d.handle(), self.stamps, 0, 2, std::mem::size_of_val(&stamps), stamps.as_mut_ptr().cast(), 8, vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT);
            if read == vk::Result::SUCCESS {
                let mask = if dev.timestamp_bits >= 64 { u64::MAX } else { (1u64 << dev.timestamp_bits) - 1 };
                let copy_ns = (stamps[1].wrapping_sub(stamps[0]) & mask) as f64 * dev.properties.limits.timestamp_period as f64;
                let encode_ns = self.enc.last_encode_ns.get().unwrap_or(0);
                self.timing.add(&self.info, crate::clock_ns() - started, copy_ns as u64, encode_ns, frame_pending);
            }
            Ok(out)
        }
    }

    /// A picture from tightly packed NV12 bytes (NV24 for a 4:4:4 stream).
    pub fn encode_nv12(&mut self, nv12: &[u8], key: bool) -> Result<Vec<u8>, String> {
        let [w, h] = self.enc.size;
        // NV12, or NV24 (full-size interleaved chroma) for a 4:4:4 stream
        let bytes = if self.enc.full_chroma { w as usize * h as usize * 3 } else { w as usize * h as usize * 3 / 2 };
        if nv12.len() != bytes {
            return Err(format!("{} picture of {} bytes, a {w}x{h} one has {bytes}", if self.enc.full_chroma { "NV24" } else { "NV12" }, nv12.len()));
        }
        unsafe {
            if self.upload.is_none() {
                let dev = self.enc.dev.clone();
                let (buf, mem, coherent) = buffer(&dev, bytes as u64, vk::BufferUsageFlags::TRANSFER_SRC, None, true)?;
                let mapped = dev.raw.map_memory(mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()).map_err(error)? as usize;
                let _ = coherent;
                self.upload = Some((buf, mem, mapped, coherent));
            }
            let (buf, mem, mapped, coherent) = self.upload.unwrap();
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), mapped as *mut u8, bytes);
            if !coherent {
                self.enc.dev.raw.flush_mapped_memory_ranges(&[vk::MappedMemoryRange::default().memory(mem).size(vk::WHOLE_SIZE)]).map_err(error)?;
            }
            let pic = self.next_picture(key, false);
            if let Err(e) = self.begin_copy() {
                self.failed();
                return Err(e);
            }
            self.record_planes(buf);
            self.finish(pic, false)
        }
    }
}

impl Drop for StreamEncoder {
    fn drop(&mut self) {
        unsafe {
            let d = &self.enc.dev.raw;
            // the last picture's work finished before its call returned (the encode fence covers
            // the copy it waited for)
            if let Some((buf, mem, _, _)) = self.upload.take() {
                d.unmap_memory(mem);
                d.destroy_buffer(buf, None);
                d.free_memory(mem, None);
            }
            d.destroy_semaphore(self.copied, None);
            d.destroy_semaphore(self.ready, None);
            d.destroy_query_pool(self.stamps, None);
            d.destroy_command_pool(self.copy_pool, None);
        }
    }
}

/// A QP ceiling on a rate control layer (0: none), every picture type.
fn qp_ceiling(h264: &mut vk::VideoEncodeH264RateControlLayerInfoKHR, h265: &mut vk::VideoEncodeH265RateControlLayerInfoKHR, max_qp: u32) {
    if max_qp == 0 {
        return;
    }
    let q = max_qp as i32;
    h264.use_max_qp = 1;
    h264.max_qp = vk::VideoEncodeH264QpKHR { qp_i: q, qp_p: q, qp_b: q };
    h265.use_max_qp = 1;
    h265.max_qp = vk::VideoEncodeH265QpKHR { qp_i: q, qp_p: q, qp_b: q };
}
