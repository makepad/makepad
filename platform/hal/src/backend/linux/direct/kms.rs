//! VK_KHR_display connector/mode/plane policy from the native Vulkan direct backend.
use super::drm_sys;
use makepad_vulkan::vk;
use std::{collections::{HashMap, HashSet}, path::{Path, PathBuf}};
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DrmConnector {
    pub card: PathBuf,
    pub name: String,
    pub id: u32,
    /// Hash of the sysfs EDID (the mode list without one): a replaced monitor on the same
    /// connector changes it even though the name does not.
    pub fingerprint: u64,
}


#[derive(Clone, Default)]
pub(super) struct DrmFilter {
    pub card: Option<String>,
    pub connector: Option<String>,
}

impl DrmFilter {
    pub fn from_env() -> Self {
        Self {
            card: std::env::var("MAKEPAD_DRM_DEVICE")
                .ok()
                .filter(|v| !v.is_empty()),
            connector: std::env::var("MAKEPAD_DRM_CONNECTOR")
                .ok()
                .filter(|v| !v.is_empty()),
        }
    }
}


pub(super) fn connector_rank(name: &str) -> (u8, String) {
    let kind = name.split_once('-').map(|(_, rest)| rest).unwrap_or(name);
    let builtin = ["eDP", "LVDS", "DSI", "DPI"]
        .iter()
        .any(|prefix| kind.starts_with(prefix));
    (if builtin { 0 } else { 1 }, name.to_owned())
}

pub(super) fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// EDID + mode list hash for one sysfs connector directory. A monitor swap
/// on the same connector changes it; an identical replug does not (that case
/// is covered by connector-level kernel events and the acquire watchdog).
pub(super) fn connector_fingerprint(dir: &Path) -> u64 {
    // The EDID names the monitor. The mode list is derived by the driver and can
    // change for the same monitor (e.g. once a driver has validated modes after the
    // first modeset), so it only stands in when there is no EDID.
    let hash = 0xcbf2_9ce4_8422_2325u64;
    match std::fs::read(dir.join("edid")) {
        Ok(edid) if !edid.is_empty() => fnv1a(hash, &edid),
        _ => std::fs::read(dir.join("modes")).map_or(hash, |modes| fnv1a(hash, &modes)),
    }
}

/// Connected connectors from sysfs, in primary-preference order. sysfs only
/// tells us the cable state; whether a connector can be driven is decided by
/// vkGetDrmDisplayEXT/vkAcquireDrmDisplayEXT and reported per output.
pub(super) fn scan_drm_connectors(filter: &DrmFilter) -> Result<Vec<DrmConnector>, String> {
    let entries =
        std::fs::read_dir("/sys/class/drm").map_err(|e| format!("read DRM connectors: {e}"))?;
    let mut connectors = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read DRM connector entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((card, connector)) = name.split_once('-') else {
            continue;
        };
        if !card.starts_with("card") || !card[4..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let card_path = PathBuf::from("/dev/dri").join(card);
        if filter
            .card
            .as_ref()
            .is_some_and(|filter| filter != card && PathBuf::from(filter) != card_path)
        {
            continue;
        }
        if std::fs::read_to_string(entry.path().join("status"))
            .ok()
            .as_deref()
            .map(str::trim)
            != Some("connected")
        {
            continue;
        }
        let id = match std::fs::read_to_string(entry.path().join("connector_id"))
            .ok()
            .and_then(|id| id.trim().parse::<u32>().ok())
        {
            Some(id) => id,
            None => continue,
        };
        if filter.connector.as_ref().is_some_and(|filter| {
            filter != &name && filter != connector && filter.parse::<u32>().ok() != Some(id)
        }) {
            continue;
        }
        let fingerprint = connector_fingerprint(&entry.path());
        connectors.push(DrmConnector {
            card: card_path,
            name,
            id,
            fingerprint,
        });
    }
    connectors.sort_by_key(|connector| connector_rank(&connector.name));
    Ok(connectors)
}


/// Every mode DRM lists for a connector (the EDID modes plus driver additions), as
/// (width, height, Hz), largest and fastest first, and the monitor's preferred one.
/// Works on any card, acquired or not.
pub(super) fn drm_connector_modes(fd: i32, connector_id: u32) -> (Vec<(u32, u32, f64)>, Option<(u32, u32, f64)>) {
    let connector = unsafe { drm_sys::drmModeGetConnector(fd, connector_id) };
    if connector.is_null() {
        return (Vec::new(), None);
    }
    let hz = |mode: &drm_sys::Mode| {
        let total = u64::from(mode.htotal) * u64::from(mode.vtotal);
        let hz = if total == 0 { f64::from(mode.vrefresh) } else { mode.clock as f64 * 1000.0 / total as f64 };
        (hz * 1000.0).round() / 1000.0
    };
    let mut preferred = None;
    let mut modes: Vec<(u32, u32, f64)> = unsafe {
        let connector = &*connector;
        if connector.count_modes <= 0 || connector.modes.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(connector.modes, connector.count_modes as usize)
                .iter()
                .filter(|mode| mode.flags & (1 << 4) == 0)
                .map(|mode| {
                    let entry = (u32::from(mode.hdisplay), u32::from(mode.vdisplay), hz(mode));
                    if mode.type_ & (1 << 3) != 0 && preferred.is_none() { preferred = Some(entry); }
                    entry
                })
                .collect()
        }
    };
    unsafe { drm_sys::drmModeFreeConnector(connector) };
    modes.sort_by(|a, b| {
        let area = |m: &(u32, u32, f64)| u64::from(m.0) * u64::from(m.1);
        area(b).cmp(&area(a)).then_with(|| b.2.total_cmp(&a.2))
    });
    modes.dedup();
    (modes, preferred)
}

pub(super) fn drm_native_resolution(fd: i32, connector_id: u32) -> Option<vk::Extent2D> {
    let connector = unsafe { drm_sys::drmModeGetConnector(fd, connector_id) };
    if connector.is_null() {
        return None;
    }
    let result = unsafe {
        let connector = &*connector;
        if connector.count_modes <= 0 || connector.modes.is_null() {
            None
        } else {
            std::slice::from_raw_parts(connector.modes, connector.count_modes as usize)
                .iter()
                .filter(|mode| mode.type_ & (1 << 3) != 0 && mode.flags & (1 << 4) == 0)
                .max_by_key(|mode| u32::from(mode.hdisplay) * u32::from(mode.vdisplay))
                .map(|mode| vk::Extent2D {
                    width: u32::from(mode.hdisplay),
                    height: u32::from(mode.vdisplay),
                })
        }
    };
    unsafe { drm_sys::drmModeFreeConnector(connector) };
    result
}


#[derive(Clone)]
pub(super) struct DisplayMode {
    pub mode: vk::DisplayModeKHR,
    pub extent: vk::Extent2D,
    pub refresh_mhz: u32,
    /// Every mode the connector offers (width, height, refresh Hz),
    /// deduplicated and sorted by area then refresh, both descending. Only
    /// populated by `select_display_mode`'s own enumeration; a `DisplayMode`
    /// rebuilt elsewhere from an already-chosen mode (a plane move, or
    /// `direct_claim_plane`'s re-check of a live output) leaves it empty,
    /// since those never change which modes the connector offers.
    pub modes: Vec<(u32, u32, f64)>,
}


#[derive(Clone, Copy)]
pub(super) struct PlaneCandidate {
    pub plane_index: u32,
    pub stack_index: u32,
    pub alpha: vk::DisplayPlaneAlphaFlagsKHR,
}


pub(super) fn select_display_mode(
    loader: &makepad_vulkan::Instance,
    device: vk::PhysicalDevice,
    display: vk::DisplayKHR,
    requested: Option<&str>,
    native_resolution: Option<vk::Extent2D>,
) -> Result<DisplayMode, String> {
    let properties = unsafe { loader.get_physical_device_display_properties(device) }
        .map_err(|e| format!("enumerate Vulkan displays: {e:?}"))?;
    let property = properties
        .iter()
        .find(|p| p.display == display)
        .ok_or("DRM display properties unavailable")?;
    if !property
        .supported_transforms
        .contains(vk::SurfaceTransformFlagsKHR::IDENTITY)
    {
        return Err("display does not support the identity transform".into());
    }
    let modes = unsafe { loader.get_display_mode_properties(device, display) }
        .map_err(|e| format!("enumerate display modes: {e:?}"))?;
    let selected = modes
        .iter()
        .filter(|mode| {
            let Some(requested) = requested else {
                return true;
            };
            let size = mode.parameters.visible_region;
            let resolution = format!("{}x{}", size.width, size.height);
            let hz = (mode.parameters.refresh_rate + 500) / 1000;
            requested == resolution
                || requested == format!("{resolution}-{hz}")
                || requested == format!("{resolution}@{hz}")
        })
        .max_by_key(|mode| {
            let size = mode.parameters.visible_region;
            (
                size == native_resolution.unwrap_or(property.physical_resolution),
                u64::from(size.width) * u64::from(size.height),
                mode.parameters.refresh_rate,
            )
        })
        .ok_or_else(|| {
            format!(
                "Requested display mode {:?} unavailable; modes: {}",
                requested,
                modes
                    .iter()
                    .map(|mode| format!(
                        "{}x{}-{}",
                        mode.parameters.visible_region.width,
                        mode.parameters.visible_region.height,
                        (mode.parameters.refresh_rate + 500) / 1000
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let extent = selected.parameters.visible_region;
    let refresh_mhz = selected.parameters.refresh_rate;
    eprintln!(
        "Vulkan direct mode: {}x{} @ {:.3} Hz ({})",
        extent.width,
        extent.height,
        refresh_mhz as f64 / 1000.0,
        if requested.is_some() {
            "explicit override"
        } else {
            "fastest native-resolution mode"
        }
    );
    let mut all_modes: Vec<(u32, u32, f64)> = modes
        .iter()
        .map(|mode| {
            let size = mode.parameters.visible_region;
            (size.width, size.height, mode.parameters.refresh_rate as f64 / 1000.0)
        })
        .collect();
    all_modes.sort_by(|a, b| {
        let area = |m: &(u32, u32, f64)| u64::from(m.0) * u64::from(m.1);
        area(b).cmp(&area(a)).then_with(|| b.2.total_cmp(&a.2))
    });
    all_modes.dedup();
    Ok(DisplayMode {
        mode: selected.display_mode,
        extent,
        refresh_mhz,
        modes: all_modes,
    })
}

/// Every plane that can scan out `mode` on `display` at full size with a
/// usable alpha mode. `excluded` planes are skipped outright. A plane the
/// driver reports as currently driving another display is skipped unless it
/// is in `owned` (one of our own outputs, which a re-match may move); the
/// driver's `current_display` can also lag behind our own surface creation.
pub(super) fn plane_candidates(
    loader: &makepad_vulkan::Instance,
    device: vk::PhysicalDevice,
    display: vk::DisplayKHR,
    mode: &DisplayMode,
    excluded: &HashSet<u32>,
    owned: &HashSet<u32>,
) -> Result<Vec<PlaneCandidate>, String> {
    let planes = unsafe { loader.get_physical_device_display_plane_properties(device) }
        .map_err(|e| format!("enumerate display planes: {e:?}"))?;
    let extent = mode.extent;
    let mut candidates = Vec::new();
    for (index, plane) in planes.iter().enumerate() {
        let index = index as u32;
        if excluded.contains(&index) {
            continue;
        }
        if plane.current_display != vk::DisplayKHR::null()
            && plane.current_display != display
            && !owned.contains(&index)
        {
            continue;
        }
        let supported = unsafe { loader.get_display_plane_supported_displays(device, index) }
            .map_err(|e| format!("query plane displays: {e:?}"))?;
        if !supported.contains(&display) {
            continue;
        }
        let caps = unsafe { loader.get_display_plane_capabilities(device, mode.mode, index) }
            .map_err(|e| format!("query display plane capabilities: {e:?}"))?;
        if extent.width < caps.min_src_extent.width
            || extent.height < caps.min_src_extent.height
            || extent.width > caps.max_src_extent.width
            || extent.height > caps.max_src_extent.height
            || extent.width < caps.min_dst_extent.width
            || extent.height < caps.min_dst_extent.height
            || extent.width > caps.max_dst_extent.width
            || extent.height > caps.max_dst_extent.height
            || caps.min_src_position.x > 0
            || caps.min_src_position.y > 0
            || caps.max_src_position.x < 0
            || caps.max_src_position.y < 0
            || caps.min_dst_position.x > 0
            || caps.min_dst_position.y > 0
            || caps.max_dst_position.x < 0
            || caps.max_dst_position.y < 0
        {
            continue;
        }
        let Some(alpha) = [
            vk::DisplayPlaneAlphaFlagsKHR::OPAQUE,
            vk::DisplayPlaneAlphaFlagsKHR::GLOBAL,
            vk::DisplayPlaneAlphaFlagsKHR::PER_PIXEL,
            vk::DisplayPlaneAlphaFlagsKHR::PER_PIXEL_PREMULTIPLIED,
        ]
        .into_iter()
        .find(|alpha| caps.supported_alpha.contains(*alpha)) else {
            continue;
        };
        candidates.push(PlaneCandidate {
            plane_index: index,
            stack_index: plane.current_stack_index,
            alpha,
        });
    }
    Ok(candidates)
}

pub(super) fn create_plane_surface(
    loader: &makepad_vulkan::Instance,
    mode: &DisplayMode,
    candidate: &PlaneCandidate,
) -> Result<vk::SurfaceKHR, String> {
    let info = vk::DisplaySurfaceCreateInfoKHR::default()
        .display_mode(mode.mode)
        .plane_index(candidate.plane_index)
        .plane_stack_index(candidate.stack_index)
        .transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
        .global_alpha(1.0)
        .alpha_mode(candidate.alpha)
        .image_extent(mode.extent);
    unsafe { loader.create_display_plane_surface(&info, None) }
        .map_err(|e| format!("create Vulkan direct-display surface: {e:?}"))
}

/// Bipartite matching of displays (in rank order) to planes: augmenting paths
/// move an earlier display to another compatible plane rather than rejecting
/// a later one, and never unmatch a display already matched. Bounded by
/// displays × planes.
pub(super) fn assign_planes(candidates: &[Vec<u32>]) -> Vec<Option<u32>> {
    fn augment(
        display: usize,
        candidates: &[Vec<u32>],
        owner: &mut HashMap<u32, usize>,
        visited: &mut HashSet<u32>,
    ) -> bool {
        for &plane in &candidates[display] {
            if !visited.insert(plane) {
                continue;
            }
            let free = match owner.get(&plane) {
                None => true,
                Some(&other) => augment(other, candidates, owner, visited),
            };
            if free {
                owner.insert(plane, display);
                return true;
            }
        }
        false
    }
    let mut owner: HashMap<u32, usize> = HashMap::new();
    for display in 0..candidates.len() {
        let mut visited = HashSet::new();
        augment(display, candidates, &mut owner, &mut visited);
    }
    let mut assignment = vec![None; candidates.len()];
    for (plane, display) in owner {
        assignment[display] = Some(plane);
    }
    assignment
}


pub(super) fn choose_output_format(formats: &[vk::SurfaceFormatKHR]) -> Result<vk::SurfaceFormatKHR, String> {
    const PREFERRED: [vk::Format; 4] = [
        vk::Format::B8G8R8A8_UNORM,
        vk::Format::R8G8B8A8_UNORM,
        vk::Format::A8B8G8R8_UNORM_PACK32,
        vk::Format::A2B10G10R10_UNORM_PACK32,
    ];
    for preferred in PREFERRED {
        if let Some(format) = formats
            .iter()
            .find(|f| f.format == preferred && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        {
            return Ok(*format);
        }
    }
    if formats.len() == 1
        && formats[0].format == vk::Format::UNDEFINED
        && formats[0].color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
    {
        // The surface accepts any format in the sRGB-nonlinear colour space.
        return Ok(vk::SurfaceFormatKHR {
            format: vk::Format::B8G8R8A8_UNORM,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
        });
    }
    Err(format!(
        "no colour-correct (UNORM, sRGB nonlinear) surface format; offered: {}",
        formats
            .iter()
            .map(|f| format!("{:?}/{:?}", f.format, f.color_space))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

