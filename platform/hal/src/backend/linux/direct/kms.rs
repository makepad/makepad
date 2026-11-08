//! VK_KHR_display connector/mode/plane policy from the native Vulkan direct backend.
use super::{drm_sys, heads::{Head, KmsState}};
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
    /// The monitor's name from its EDID; empty when it names none.
    pub monitor: String,
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

/// EDID + mode list hash for one sysfs connector directory, and the monitor's EDID name. A
/// monitor swap on the same connector changes the hash; an identical replug does not (that
/// case is covered by connector-level kernel events and the acquire watchdog).
pub(super) fn connector_identity(dir: &Path) -> (u64, String) {
    // The EDID names the monitor. The mode list is derived by the driver and can
    // change for the same monitor (e.g. once a driver has validated modes after the
    // first modeset), so it only stands in when there is no EDID.
    let hash = 0xcbf2_9ce4_8422_2325u64;
    match std::fs::read(dir.join("edid")) {
        Ok(edid) if !edid.is_empty() => (fnv1a(hash, &edid), edid_monitor_name(&edid)),
        _ => (std::fs::read(dir.join("modes")).map_or(hash, |modes| fnv1a(hash, &modes)), String::new()),
    }
}

/// The monitor name descriptor (tag 0xFC) of an EDID base block: up to 13 characters,
/// ended by a line feed and padded with spaces. Empty when there is none.
pub(super) fn edid_monitor_name(edid: &[u8]) -> String {
    for offset in [54usize, 72, 90, 108] {
        let Some(d) = edid.get(offset..offset + 18) else { break };
        if d[0] == 0 && d[1] == 0 && d[2] == 0 && d[3] == 0xfc {
            let text: String = d[5..].iter().take_while(|&&b| b != 0x0a).map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' }).collect();
            return text.trim().to_owned();
        }
    }
    String::new()
}

/// Connected connectors in primary-preference order. On startup and hotplug,
/// `probe` refreshes each connector through the owner's DRM master descriptor
/// before reading sysfs: its status and EDID can still describe the old cable
/// state. Ordinary periodic scans pass a no-op and only read cached metadata.
pub(super) fn scan_drm_connectors(
    filter: &DrmFilter,
    probe: impl FnMut(&Path, u32),
) -> Result<Vec<DrmConnector>, String> {
    scan_sysfs_connectors(Path::new("/sys/class/drm"), filter, probe)
}

/// `scan_drm_connectors` over the DRM class directory `root`.
pub(super) fn scan_sysfs_connectors(
    root: &Path,
    filter: &DrmFilter,
    mut probe: impl FnMut(&Path, u32),
) -> Result<Vec<DrmConnector>, String> {
    let entries =
        std::fs::read_dir(root).map_err(|e| format!("read DRM connectors: {e}"))?;
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
        probe(&card_path, id);
        if std::fs::read_to_string(entry.path().join("status"))
            .ok()
            .as_deref()
            .map(str::trim)
            != Some("connected")
        {
            continue;
        }
        let (fingerprint, monitor) = connector_identity(&entry.path());
        connectors.push(DrmConnector {
            card: card_path,
            name,
            id,
            fingerprint,
            monitor,
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

/// The card's heads as DRM shows them: what each CRTC lights, and each connector's possible
/// CRTCs (the union of its encoders'). Reads the last detected state only.
pub(super) fn drm_kms_state(fd: i32) -> Result<KmsState, String> {
    let resources = unsafe { drm_sys::drmModeGetResources(fd) };
    if resources.is_null() {
        return Err(format!("DRM resources: {}", std::io::Error::last_os_error()));
    }
    let list = |ptr: *mut u32, count: i32| if ptr.is_null() || count <= 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(ptr, count as usize) } };
    let mut state = KmsState::default();
    unsafe {
        let r = &*resources;
        state.heads = list(r.crtcs, r.count_crtcs).iter().map(|&crtc| Head { crtc, lights: 0, connected: false }).collect();
        for &id in list(r.connectors, r.count_connectors) {
            let connector = drm_sys::drmModeGetConnectorCurrent(fd, id);
            if connector.is_null() { continue; }
            let c = &*connector;
            let mut possible = 0;
            for &encoder_id in list(c.encoders, c.count_encoders) {
                let encoder = drm_sys::drmModeGetEncoder(fd, encoder_id);
                if encoder.is_null() { continue; }
                possible |= (*encoder).possible_crtcs;
                if encoder_id == c.encoder_id {
                    if let Some(head) = state.heads.iter_mut().find(|h| h.crtc == (*encoder).crtc_id) {
                        head.lights = id;
                        head.connected = c.connection == drm_sys::CONNECTED;
                    }
                }
                drm_sys::drmModeFreeEncoder(encoder);
            }
            state.possible.insert(id, possible);
            drm_sys::drmModeFreeConnector(connector);
        }
        drm_sys::drmModeFreeResources(resources);
    }
    Ok(state)
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
    /// deduplicated and sorted by area then refresh, both descending.
    pub modes: Vec<(u32, u32, f64)>,
}


#[derive(Clone, Copy)]
pub(super) struct PlaneCandidate {
    pub plane_index: u32,
    pub stack_index: u32,
    pub alpha: vk::DisplayPlaneAlphaFlagsKHR,
}


/// A display mode as (width, height, refresh in mHz).
pub(super) type ModeKey = (u32, u32, u32);

pub(super) fn mode_key(mode: &DisplayMode) -> ModeKey { (mode.extent.width, mode.extent.height, mode.refresh_mhz) }

pub(super) fn key_label(mode: ModeKey) -> String { format!("{}x{}@{}", mode.0, mode.1, (mode.2 + 500) / 1000) }

/// The mode `requested` names (`WxH`, `WxH-Hz` or `WxH@Hz`), or with none the largest mode at
/// the native resolution at its fastest refresh.
pub(super) fn choose_mode(modes: &[ModeKey], requested: Option<&str>, native: vk::Extent2D) -> Option<ModeKey> {
    modes.iter().copied().filter(|&(w, h, mhz)| {
        let Some(requested) = requested else { return true };
        let resolution = format!("{w}x{h}");
        let hz = (mhz + 500) / 1000;
        requested == resolution || requested == format!("{resolution}-{hz}") || requested == format!("{resolution}@{hz}")
    }).max_by_key(|&(w, h, mhz)| ((w, h) == (native.width, native.height), u64::from(w) * u64::from(h), mhz))
}

/// Whether `mode` is at or above `failed` on the way down: a larger area, or the same
/// resolution at the same or a faster refresh.
fn at_or_above(mode: ModeKey, failed: ModeKey) -> bool {
    let area = |m: ModeKey| u64::from(m.0) * u64::from(m.1);
    area(mode) > area(failed) || ((mode.0, mode.1) == (failed.0, failed.1) && mode.2 >= failed.2)
}

/// The modes to try, in order: `first`, then each lower refresh at its resolution, then each
/// smaller resolution from its fastest refresh, then those of `extra` (the last mode that worked,
/// the monitor's own) that `modes` lists. With `failed` (a mode that failed to present) every mode at or above it is
/// left out, so a failure steps down and never comes back up.
pub(super) fn mode_ladder(modes: &[ModeKey], first: ModeKey, extra: &[ModeKey], failed: Option<ModeKey>) -> Vec<ModeKey> {
    let area = |m: &ModeKey| u64::from(m.0) * u64::from(m.1);
    let mut same: Vec<_> = modes.iter().copied().filter(|m| (m.0, m.1) == (first.0, first.1) && m.2 < first.2).collect();
    same.sort_by_key(|m| std::cmp::Reverse(m.2));
    let mut smaller: Vec<_> = modes.iter().copied().filter(|m| area(m) < area(&first)).collect();
    smaller.sort_by_key(|m| (std::cmp::Reverse(area(m)), std::cmp::Reverse(m.2)));
    let mut ladder = Vec::new();
    for mode in [first].into_iter().chain(same).chain(smaller).chain(extra.iter().copied().filter(|m| modes.contains(m))) {
        if !ladder.contains(&mode) && !failed.is_some_and(|f| at_or_above(mode, f)) { ladder.push(mode); }
    }
    ladder
}

/// The display's modes with their handles, after checking it can be driven unrotated, and its
/// physical resolution.
fn display_modes(
    loader: &makepad_vulkan::Instance,
    device: vk::PhysicalDevice,
    display: vk::DisplayKHR,
) -> Result<(Vec<(ModeKey, vk::DisplayModeKHR)>, vk::Extent2D), String> {
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
    let modes = modes.iter().map(|m| {
        let size = m.parameters.visible_region;
        ((size.width, size.height, m.parameters.refresh_rate), m.display_mode)
    }).collect();
    Ok((modes, property.physical_resolution))
}

/// `modes` as (width, height, Hz), deduplicated, by area then refresh, both descending.
fn mode_list(modes: &[(ModeKey, vk::DisplayModeKHR)]) -> Vec<(u32, u32, f64)> {
    let mut all: Vec<(u32, u32, f64)> = modes.iter().map(|((w, h, mhz), _)| (*w, *h, *mhz as f64 / 1000.0)).collect();
    all.sort_by(|a, b| {
        let area = |m: &(u32, u32, f64)| u64::from(m.0) * u64::from(m.1);
        area(b).cmp(&area(a)).then_with(|| b.2.total_cmp(&a.2))
    });
    all.dedup();
    all
}

fn unavailable(requested: Option<&str>, modes: &[(ModeKey, vk::DisplayModeKHR)]) -> String {
    format!("Requested display mode {:?} unavailable; modes: {}", requested,
        modes.iter().map(|(m, _)| format!("{}x{}-{}", m.0, m.1, (m.2 + 500) / 1000)).collect::<Vec<_>>().join(", "))
}

/// The modes to try for a connector, in `mode_ladder` order from the mode `requested` names
/// (else, or when it lacks that mode, the fastest native-resolution mode), with `working`
/// (the last mode that worked on it) and the native one as the last resorts, skipping
/// everything at or above `failed`. Also returns why the first mode is not the one asked
/// for, if it is not.
pub(super) fn display_mode_ladder(
    loader: &makepad_vulkan::Instance,
    device: vk::PhysicalDevice,
    display: vk::DisplayKHR,
    requested: Option<&str>,
    native_resolution: Option<vk::Extent2D>,
    working: Option<ModeKey>,
    failed: Option<(ModeKey, &str)>,
) -> Result<(Vec<DisplayMode>, Option<String>), String> {
    let (modes, physical) = display_modes(loader, device, display)?;
    let keys: Vec<ModeKey> = modes.iter().map(|(m, _)| *m).collect();
    let native = native_resolution.unwrap_or(physical);
    let auto = choose_mode(&keys, None, native).ok_or("the display lists no modes")?;
    let (first, mut why) = match requested.map(|r| choose_mode(&keys, Some(r), native)) {
        Some(None) => (auto, Some(unavailable(requested, &modes))),
        Some(Some(mode)) => (mode, None),
        None => (auto, None),
    };
    let ladder = mode_ladder(&keys, first, &working.into_iter().chain([auto]).collect::<Vec<_>>(), failed.map(|f| f.0));
    if let Some((mode, reason)) = failed.filter(|f| at_or_above(first, f.0)) {
        why = Some(format!("{} failed to present ({reason})", key_label(mode)));
    }
    if ladder.is_empty() {
        return Err(why.unwrap_or_else(|| "no display mode".into()));
    }
    let list = mode_list(&modes);
    let ladder = ladder.into_iter().filter_map(|key| modes.iter().find(|(m, _)| *m == key).map(|(_, mode)| DisplayMode {
        mode: *mode,
        extent: vk::Extent2D { width: key.0, height: key.1 },
        refresh_mhz: key.2,
        modes: list.clone(),
    })).collect();
    Ok((ladder, why))
}

/// Per connector, the mode whose presentation failed: its ladder starts below it, until the
/// monitor is replugged or replaced or a mode is picked for it. A mode that never showed a
/// frame steps down at its first failure; one that showed frames first is rebuilt at the
/// same mode once, and steps down if it fails again. A step happens on a failure only, never
/// on elapsed time.
#[derive(Default)]
pub(super) struct Demotions { connectors: HashMap<String, Demotion> }
#[derive(Default)]
struct Demotion { fingerprint: u64, struck: Option<ModeKey>, failed: Option<(ModeKey, String)> }
impl Demotions {
    /// `mode` failed to present on `connector` (`presented`: after showing frames). Returns
    /// whether the connector steps down below it.
    pub fn failed(&mut self, connector: &DrmConnector, mode: ModeKey, presented: bool, reason: &str) -> bool {
        let entry = self.connectors.entry(connector.name.clone()).or_default();
        if entry.fingerprint != connector.fingerprint { *entry = Demotion { fingerprint: connector.fingerprint, ..Default::default() }; }
        if presented && entry.struck != Some(mode) {
            entry.struck = Some(mode);
            return false;
        }
        entry.struck = None;
        entry.failed = Some((mode, reason.to_owned()));
        true
    }
    /// The mode the connector's ladder starts below, and why.
    pub fn below(&self, connector: &DrmConnector) -> Option<(ModeKey, &str)> {
        self.connectors.get(&connector.name).filter(|d| d.fingerprint == connector.fingerprint)
            .and_then(|d| d.failed.as_ref()).map(|(m, r)| (*m, r.as_str()))
    }
    /// A mode was picked for the connector: everything may be tried again.
    pub fn forget(&mut self, name: &str) { self.connectors.remove(name); }
    /// Only connectors still connected, with the same monitor, keep theirs.
    pub fn keep(&mut self, connectors: &[DrmConnector]) {
        self.connectors.retain(|name, d| connectors.iter().any(|c| c.name == *name && c.fingerprint == d.fingerprint));
    }
}

/// Per connector, the last mode that showed a frame on its monitor: a last resort of its ladder.
/// Another monitor on the connector starts without one.
#[derive(Default)]
pub(super) struct Working { connectors: HashMap<String, (u64, ModeKey)> }
impl Working {
    /// `mode` showed a frame on `connector`.
    pub fn showed(&mut self, connector: &DrmConnector, mode: ModeKey) {
        self.connectors.insert(connector.name.clone(), (connector.fingerprint, mode));
    }
    pub fn get(&self, connector: &DrmConnector) -> Option<ModeKey> {
        self.connectors.get(&connector.name).filter(|w| w.0 == connector.fingerprint).map(|w| w.1)
    }
    /// Only connectors still connected, with the same monitor, keep theirs.
    pub fn keep(&mut self, connectors: &[DrmConnector]) {
        self.connectors.retain(|name, w| connectors.iter().any(|c| c.name == *name && c.fingerprint == w.0));
    }
}

/// One display plane as the driver reports it for a display at a mode.
#[derive(Clone, Copy, Debug)]
pub(super) struct PlaneRow {
    pub current: vk::DisplayKHR,
    pub stack: u32,
    /// The alpha mode it scans `mode` out with on `display` at full size; `None` when it
    /// cannot (the display is not among its supported ones, or the size is out of range).
    pub alpha: Option<vk::DisplayPlaneAlphaFlagsKHR>,
}

/// Every plane of `device`, and whether it can scan out `mode` on `display` at full size with
/// a usable alpha mode.
pub(super) fn plane_rows(
    loader: &makepad_vulkan::Instance,
    device: vk::PhysicalDevice,
    display: vk::DisplayKHR,
    mode: &DisplayMode,
) -> Result<Vec<PlaneRow>, String> {
    let planes = unsafe { loader.get_physical_device_display_plane_properties(device) }
        .map_err(|e| format!("enumerate display planes: {e:?}"))?;
    let extent = mode.extent;
    let mut rows = Vec::new();
    for (index, plane) in planes.iter().enumerate() {
        let index = index as u32;
        let mut row = PlaneRow { current: plane.current_display, stack: plane.current_stack_index, alpha: None };
        let supported = unsafe { loader.get_display_plane_supported_displays(device, index) }
            .map_err(|e| format!("query plane displays: {e:?}"))?;
        if supported.contains(&display) {
            let caps = unsafe { loader.get_display_plane_capabilities(device, mode.mode, index) }
                .map_err(|e| format!("query display plane capabilities: {e:?}"))?;
            let fits = extent.width >= caps.min_src_extent.width
                && extent.height >= caps.min_src_extent.height
                && extent.width <= caps.max_src_extent.width
                && extent.height <= caps.max_src_extent.height
                && extent.width >= caps.min_dst_extent.width
                && extent.height >= caps.min_dst_extent.height
                && extent.width <= caps.max_dst_extent.width
                && extent.height <= caps.max_dst_extent.height
                && caps.min_src_position.x <= 0
                && caps.min_src_position.y <= 0
                && caps.max_src_position.x >= 0
                && caps.max_src_position.y >= 0
                && caps.min_dst_position.x <= 0
                && caps.min_dst_position.y <= 0
                && caps.max_dst_position.x >= 0
                && caps.max_dst_position.y >= 0;
            row.alpha = fits.then(|| [
                vk::DisplayPlaneAlphaFlagsKHR::OPAQUE,
                vk::DisplayPlaneAlphaFlagsKHR::GLOBAL,
                vk::DisplayPlaneAlphaFlagsKHR::PER_PIXEL,
                vk::DisplayPlaneAlphaFlagsKHR::PER_PIXEL_PREMULTIPLIED,
            ]
            .into_iter()
            .find(|alpha| caps.supported_alpha.contains(*alpha))).flatten();
        }
        rows.push(row);
    }
    Ok(rows)
}

/// The displays the driver lists as connected.
pub(super) fn connected_displays(loader: &makepad_vulkan::Instance, device: vk::PhysicalDevice) -> Result<HashSet<vk::DisplayKHR>, String> {
    let properties = unsafe { loader.get_physical_device_display_properties(device) }
        .map_err(|e| format!("enumerate Vulkan displays: {e:?}"))?;
    Ok(properties.iter().map(|p| p.display).collect())
}

/// The planes `display` may take, best first. Occupancy comes from the session's own book
/// (`book`: plane -> the display we hold on it) and, for planes not in it, from the driver's
/// `current` display:
/// 1. the plane the book gives `display` (a rebuild keeps its plane);
/// 2. a plane the driver already shows `display` on;
/// 3. an empty plane, or one showing a display that is gone;
/// 4. a plane the console shows a connected display on that we do not hold.
///
/// A plane the book gives another display is taken, and so is one the driver shows a display
/// we hold on (`held`; its mode spans a second head).
pub(super) fn plane_order(
    rows: &[PlaneRow],
    display: vk::DisplayKHR,
    book: &HashMap<u32, vk::DisplayKHR>,
    held: &HashSet<vk::DisplayKHR>,
    connected: &HashSet<vk::DisplayKHR>,
) -> Vec<PlaneCandidate> {
    let mut ranked: Vec<(u8, PlaneCandidate)> = rows.iter().enumerate().filter_map(|(index, row)| {
        let alpha = row.alpha?;
        let index = index as u32;
        let rank = match book.get(&index) {
            Some(holder) if *holder == display => 0,
            Some(_) => return None,
            None if row.current == display => 1,
            None if row.current == vk::DisplayKHR::null() || !connected.contains(&row.current) => 2,
            None if held.contains(&row.current) => return None,
            None => 3,
        };
        Some((rank, PlaneCandidate { plane_index: index, stack_index: row.stack, alpha }))
    }).collect();
    ranked.sort_by_key(|(rank, c)| (*rank, c.plane_index));
    ranked.into_iter().map(|(_, c)| c).collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_monitor_name_comes_from_the_edid_name_descriptor() {
        let mut edid = vec![0u8; 128];
        // a range-limits descriptor first, then the name
        edid[72..77].copy_from_slice(&[0, 0, 0, 0xfd, 0]);
        edid[90..95].copy_from_slice(&[0, 0, 0, 0xfc, 0]);
        edid[95..108].copy_from_slice(b"Monitor 27\n  ");
        assert_eq!(edid_monitor_name(&edid), "Monitor 27");
        assert_eq!(edid_monitor_name(&edid[..64]), "", "a short EDID names nothing");
        assert_eq!(edid_monitor_name(&[0u8; 128]), "");
    }

    #[test]
    fn a_mode_that_fails_to_present_steps_down_and_never_back_up() {
        let k4 = |hz: u32| (3840, 2160, hz * 1000);
        let qhd = |hz: u32| (2560, 1440, hz * 1000);
        let fhd = |hz: u32| (1920, 1080, hz * 1000);
        let modes = [fhd(60), k4(60), qhd(144), k4(240), fhd(240), k4(120), qhd(60), k4(165), k4(144), qhd(240)];
        let native = vk::Extent2D { width: 3840, height: 2160 };
        let auto = choose_mode(&modes, None, native).unwrap();
        assert_eq!(auto, k4(240), "auto: the fastest native mode");
        assert_eq!(choose_mode(&modes, Some("2560x1440@144"), native), Some(qhd(144)));
        assert_eq!(choose_mode(&modes, Some("1280x720"), native), None);
        assert_eq!(mode_ladder(&modes, auto, &[], None),
            vec![k4(240), k4(165), k4(144), k4(120), k4(60), qhd(240), qhd(144), qhd(60), fhd(240), fhd(60)],
            "the next refresh at the same resolution, then down");
        let monitor = DrmConnector { card: PathBuf::from("/dev/dri/card1"), name: "card1-DP-1".into(), id: 1, fingerprint: 7, monitor: String::new() };
        let mut demotions = Demotions::default();
        let first = |demotions: &Demotions, request: ModeKey, extra: &[ModeKey]| mode_ladder(&modes, request, extra, demotions.below(&monitor).map(|b| b.0)).first().copied();
        // 240 Hz never shows a frame: down at once
        assert!(demotions.failed(&monitor, k4(240), false, "ERROR_UNKNOWN"));
        assert_eq!(demotions.below(&monitor), Some((k4(240), "ERROR_UNKNOWN")));
        assert_eq!(first(&demotions, auto, &[k4(240)]), Some(k4(165)), "and the mode that worked before is skipped too");
        // 165 Hz shows frames, then fails: once more at 165, then down
        assert!(!demotions.failed(&monitor, k4(165), true, "flip stalled"));
        assert_eq!(first(&demotions, auto, &[]), Some(k4(165)));
        assert!(demotions.failed(&monitor, k4(165), true, "flip stalled"));
        assert_eq!(first(&demotions, auto, &[]), Some(k4(144)));
        // a smaller requested mode is below the failure: it stands
        assert_eq!(first(&demotions, qhd(240), &[]), Some(qhd(240)));
        // the same monitor stays demoted across rescans; a replaced one starts again
        demotions.keep(std::slice::from_ref(&monitor));
        assert!(demotions.below(&monitor).is_some());
        let replaced = DrmConnector { fingerprint: 8, ..monitor.clone() };
        assert_eq!(demotions.below(&replaced), None);
        demotions.keep(std::slice::from_ref(&replaced));
        assert_eq!(demotions.below(&monitor), None, "replaced: forgotten");
        // a mode picked for it lets every mode be tried again
        assert!(demotions.failed(&monitor, k4(240), false, "ERROR_UNKNOWN"));
        demotions.forget(&monitor.name);
        assert_eq!(first(&demotions, auto, &[]), Some(k4(240)));
        // the smallest mode failing leaves nothing to show
        assert!(demotions.failed(&monitor, fhd(60), false, "ERROR_UNKNOWN"));
        assert!(mode_ladder(&modes, auto, &[], demotions.below(&monitor).map(|b| b.0)).is_empty());
    }

    /// The mode that worked on a connector belongs to its monitor: another monitor there does
    /// not list it, and a ladder rung the display does not list panicked the display worker.
    #[test]
    fn a_mode_that_worked_on_another_monitor_is_not_tried() {
        let modes = [(1920, 1080, 60_000), (1280, 720, 60_000)];
        let old = (3840, 2160, 144_000);
        assert_eq!(mode_ladder(&modes, modes[0], &[old, modes[1]], None), modes.to_vec(), "only the modes the display lists");
        let monitor = DrmConnector { card: PathBuf::from("/dev/dri/card1"), name: "card1-DP-1".into(), id: 1, fingerprint: 7, monitor: String::new() };
        let replaced = DrmConnector { fingerprint: 8, ..monitor.clone() };
        let mut working = Working::default();
        working.showed(&monitor, old);
        assert_eq!(working.get(&monitor), Some(old));
        assert_eq!(working.get(&replaced), None, "another monitor on the connector");
        working.keep(std::slice::from_ref(&monitor));
        assert_eq!(working.get(&monitor), Some(old), "the same monitor keeps it across rescans");
        working.keep(std::slice::from_ref(&replaced));
        assert_eq!(working.get(&monitor), None, "replaced: forgotten");
    }
}
