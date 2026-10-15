//! Direct-display pipe data. This module has no backend state or OS calls.
//!
//! The direct backend drives one wide desktop: every connector of the
//! rendering GPU side by side, left to right along their top edges, at their
//! native pixels divided by the global DPI factor (see `linux_wide_desktop`).
//! Each connector shows its own rectangle of it. A connector that does not
//! fit within the GPU's limits shows the whole desktop letterboxed.
//! This module is the UI-facing contract: the window manager reads the
//! snapshot to list displays and their state; it never touches Vulkan.
//!
//! Outside the direct Vulkan backend (Wayland/X11 builds, hosted children)
//! the snapshot is empty with `direct == false`.

/// One DRM connector as the renderer currently sees it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LinuxDisplayOutput {
    /// The sysfs connector name, e.g. `card1-DP-2` or `card0-eDP-1`.
    pub name: String,
    /// Selected mode width in native pixels (0 when no mode was selected).
    pub width: u32,
    /// Selected mode height in native pixels (0 when no mode was selected).
    pub height: u32,
    /// Selected mode refresh in Hz (0.0 when no mode was selected).
    pub refresh_hz: f64,
    /// The main screen: the dock goes here. It may be on any GPU. A host
    /// with its own per-screen placement may still send new windows and
    /// menus to whichever screen is active (the one the pointer or the
    /// focused window is on) instead.
    pub primary: bool,
    /// The DRM card driving this connector, e.g. `card1`.
    pub card: String,
    /// Top-left corner of this connector's rectangle in the wide desktop, in
    /// native pixels; `None` when it is not part of the desktop.
    pub desktop_position: Option<(u32, u32)>,
    /// The connector has a live swapchain and its last presentation succeeded.
    pub active: bool,
    /// Why this screen does not show what was asked: the requested mode failed (a
    /// fallback is shown) or the output itself failed. `None` when it shows `requested`.
    pub error: Option<String>,
    /// The mode asked for (`WxH@Hz` etc.), if any; `width`/`height`/`refresh_hz` are
    /// what is shown.
    pub requested: Option<String>,
    /// Human-readable state: `active`, `unsupported: …`, `failed: …`, `lost: …`.
    /// Failure text is the actual Vulkan/DRM error, never a guess from sysfs.
    pub status: String,
    /// The monitor's preferred (EDID, DRM_MODE_TYPE_PREFERRED) mode as (width, height, Hz);
    /// `None` when the connector names none.
    pub native: Option<(u32, u32, f64)>,
    /// Every mode the connector offers (width, height, refresh Hz),
    /// deduplicated and sorted by area then refresh, both descending. A failed
    /// row lists its connector's DRM (EDID) modes, so another can be picked.
    pub modes: Vec<(u32, u32, f64)>,
    /// This connector's entry in the runtime mode-override table
    /// (`MAKEPAD_DRM_MODES` / `linux_set_display_mode`), if any, regardless
    /// of whether the connector has started yet.
    pub mode_override: Option<String>,
    /// The basename of `canonicalize(/sys/class/drm/<card>/device)`: the
    /// card's PCI address, for joining this screen to a host's own GPU
    /// list by a key that survives card renumbering. `None` when the
    /// sysfs link cannot be resolved.
    pub pci: Option<String>,
}

/// Everything the renderer knows about connected displays.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LinuxDisplaySnapshot {
    /// `true` only while the direct Vulkan backend owns the displays.
    pub direct: bool,
    /// Outputs in default order: built-in panels first, then by name.
    pub outputs: Vec<LinuxDisplayOutput>,
    /// What the renderer runs now (see `DisplayArrangement`).
    pub arrangement: DisplayArrangement,
    /// The pacing setting (see `DisplayPacing`).
    pub pacing: DisplayPacing,
    /// The output whose presents pace the desktop now, `pacing` resolved against the
    /// connected outputs and the arrangement; `None`: every screen at its own rate
    /// (`Auto` in extended) or no screen shows the desktop.
    pub paced_by: Option<String>,
}

/// Which screen's refresh sets how often the desktop is rendered. Other screens show the
/// newest frame at their own refresh; no screen ever causes an extra render. Switching is
/// immediate (no modeset); the backend resolves it again on every hotplug, mode change and
/// arrangement change.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum DisplayPacing {
    /// Extended: each screen at its own refresh (`paced_by` is `None`) -- the desktop is
    /// rendered as often as its fastest screen shows frames, and every screen presents the
    /// newest frame at its own refresh, so a slow monitor never holds back the panel and
    /// gets no frames it cannot show. Mirror (presenting): the slowest EXTERNAL output paces
    /// the one shared frame (the audience screen shows every frame, no judder there); with
    /// no external output, the built-in panel.
    #[default]
    Auto,
    /// The slowest screen paces (least GPU work).
    Slowest,
    /// The fastest screen paces (smoothest).
    Fastest,
    /// This screen paces (by connector name); a disconnected one falls back to `Auto`.
    Screen(String),
}

impl DisplayPacing {
    /// Wire form: `["auto"]`, `["slowest"]`, `["fastest"]` or `["screen", name]`.
    pub fn args(&self) -> Vec<&str> {
        match self { Self::Auto => vec!["auto"], Self::Slowest => vec!["slowest"], Self::Fastest => vec!["fastest"], Self::Screen(name) => vec!["screen", name] }
    }
    pub fn from_args(args: &[String]) -> Option<Self> {
        Some(match args {
            [kind] if kind == "auto" => Self::Auto,
            [kind] if kind == "slowest" => Self::Slowest,
            [kind] if kind == "fastest" => Self::Fastest,
            [kind, name] if kind == "screen" && !name.is_empty() => Self::Screen(name.clone()),
            _ => return None,
        })
    }
}

/// How the outputs share the desktop.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum DisplayArrangement {
    /// One wide desktop: each output shows its own rectangle of it.
    #[default]
    Extended,
    /// One desktop rendered once at `optimise_for`'s mode; that output shows it 1:1 and every
    /// other output shows the same picture scaled to fit, aspect kept (letterboxed), each at
    /// its own mode and refresh. Only `optimise_for` has a `desktop_position`: the desktop is
    /// one screen.
    Mirror { optimise_for: String },
}

/// Estimated timing of the next frame submitted to a direct display.
#[derive(Clone, Copy, Debug)]
pub struct LinuxDisplayFrameTiming {
    /// Estimated start of scanout, in `Cx::monotonic_now()` seconds.
    /// Present completion is the phase reference; Vulkan does not promise
    /// an exact physical scanout timestamp. Panel illumination can be later.
    pub present_time: f64,
    pub refresh_interval: f64,
}

/// Environment variable carrying the direct renderer's Vulkan device UUID
/// (32 lowercase hex digits). The direct backend sets it in its own process
/// before `Event::Startup`, so child processes spawned with an inherited
/// environment pick the same GPU for their offscreen renderer instead of the
/// discrete-first default. A host that scrubs or rebuilds the child
/// environment must forward this variable explicitly.
pub const LINUX_VULKAN_DEVICE_ENV: &str = "MAKEPAD_VULKAN_DEVICE_UUID";
pub const POINTER_SPEED_MIN: u32 = 25;
pub const POINTER_SPEED_MAX: u32 = 300;
pub const POINTER_SPEED_DEFAULT: u32 = 100;

/// Owned arguments for a display request, also retained by Cx until its reply.
#[derive(Clone, Debug)]
pub enum DisplayCommand {
    Source(String),
    Order(Vec<String>),
    Mode { name: String, mode: Option<String> },
    Arrangement(DisplayArrangement),
    Pacing(DisplayPacing),
}

impl DisplayCommand {
    /// Pure argument validation, shared by the client and the receiving host.
    pub fn validate(&self) -> Result<(), String> {
        if let Self::Mode { mode: Some(mode), .. } = self {
            let mode = mode.trim();
            if !mode.is_empty() && !mode_string_is_valid(mode) {
                return Err(format!("{mode:?} is not a display mode: use WxH, WxH@Hz or WxH-Hz with digits only"));
            }
        }
        Ok(())
    }

    pub fn tag(&self) -> u16 {
        use crate::cmds::cmd_tag;
        match self {
            Self::Source(_) => cmd_tag::DirectDisplaySource,
            Self::Order(_) => cmd_tag::DirectDisplayOrder,
            Self::Mode { .. } => cmd_tag::DirectDisplayMode,
            Self::Arrangement(_) => cmd_tag::DirectDisplayArrangement,
            Self::Pacing(_) => cmd_tag::DirectDisplayPacing,
        }
    }
}

fn mode_string_is_valid(mode: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|byte| byte.is_ascii_digit());
    let resolution = match mode.split_once('@').or_else(|| mode.split_once('-')) {
        Some((resolution, rate)) => {
            if !digits(rate) { return false; }
            resolution
        }
        None => mode,
    };
    match resolution.split_once('x') {
        Some((width, height)) => digits(width) && digits(height),
        None => false,
    }
}

/// Inventory payload plus placed screens in desktop points, in output order.
/// Native mode/position fields above keep the existing display API's pixel units.
pub struct DisplayInventory {
    pub snapshot: LinuxDisplaySnapshot,
    pub screens: Vec<crate::ScreenInfo>,
}

impl LinuxDisplaySnapshot {
    pub fn pack(&self, dpi: f64) -> Vec<u8> {
        let mut w = crate::media::PackW::new();
        w.u8(self.direct as u8).u32(self.outputs.len() as u32);
        for o in &self.outputs {
            w.str(&o.name).u32(o.width).u32(o.height).f64(o.refresh_hz)
                .u8(o.primary as u8).str(&o.card).u8(o.desktop_position.is_some() as u8);
            if let Some((x, y)) = o.desktop_position {
                w.u32(x).u32(y).f64(x as f64 / dpi).f64(y as f64 / dpi)
                    .f64(o.width as f64 / dpi).f64(o.height as f64 / dpi);
            }
            w.u8(o.active as u8).str(&o.status).u32(o.modes.len() as u32);
            for &(width, height, refresh) in &o.modes { w.u32(width).u32(height).f64(refresh); }
            w.u8(o.mode_override.is_some() as u8);
            if let Some(mode) = &o.mode_override { w.str(mode); }
            w.u8(o.pci.is_some() as u8);
            if let Some(pci) = &o.pci { w.str(pci); }
            w.u8(o.error.is_some() as u8);
            if let Some(error) = &o.error { w.str(error); }
            w.u8(o.requested.is_some() as u8);
            if let Some(requested) = &o.requested { w.str(requested); }
            w.u8(o.native.is_some() as u8);
            if let Some((width, height, refresh)) = o.native { w.u32(width).u32(height).f64(refresh); }
        }
        match &self.arrangement {
            DisplayArrangement::Extended => { w.u8(0); }
            DisplayArrangement::Mirror { optimise_for } => { w.u8(1).str(optimise_for); }
        }
        let args = self.pacing.args();
        w.u32(args.len() as u32);
        for arg in args { w.str(arg); }
        w.u8(self.paced_by.is_some() as u8);
        if let Some(name) = &self.paced_by { w.str(name); }
        w.finish()
    }
}

impl DisplayInventory {
    pub fn read(bytes: &[u8]) -> Option<Self> {
        let mut r = crate::media::PackR::new(bytes);
        let direct = r.u8() != 0;
        let mut outputs = Vec::new();
        let mut screens = Vec::new();
        for _ in 0..r.u32() {
            if !r.ok { return None; }
            let mut o = LinuxDisplayOutput {
                name: r.str(), width: r.u32(), height: r.u32(), refresh_hz: r.f64(),
                primary: r.u8() != 0, card: r.str(), ..Default::default()
            };
            if r.u8() != 0 {
                o.desktop_position = Some((r.u32(), r.u32()));
                let bounds = [r.f64(), r.f64(), r.f64(), r.f64()];
                screens.push(crate::ScreenInfo { bounds, work_area: bounds, is_primary: o.primary });
            }
            o.active = r.u8() != 0;
            o.status = r.str();
            for _ in 0..r.u32() {
                if !r.ok { return None; }
                o.modes.push((r.u32(), r.u32(), r.f64()));
            }
            if r.u8() != 0 { o.mode_override = Some(r.str()); }
            if r.u8() != 0 { o.pci = Some(r.str()); }
            if r.u8() != 0 { o.error = Some(r.str()); }
            if r.u8() != 0 { o.requested = Some(r.str()); }
            if r.u8() != 0 { o.native = Some((r.u32(), r.u32(), r.f64())); }
            outputs.push(o);
        }
        let arrangement = if r.u8() != 0 { DisplayArrangement::Mirror { optimise_for: r.str() } } else { DisplayArrangement::Extended };
        let count = r.u32().min(2);
        let args: Vec<String> = (0..count).map(|_| r.str()).collect();
        let pacing = DisplayPacing::from_args(&args)?;
        let paced_by = if r.u8() != 0 { Some(r.str()) } else { None };
        (r.ok && r.at_end()).then_some(Self { snapshot: LinuxDisplaySnapshot { direct, outputs, arrangement, pacing, paced_by }, screens })
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_inventory_round_trips_with_errors_requests_native_modes_and_mirror() {
        let snapshot = LinuxDisplaySnapshot {
            direct: true,
            outputs: vec![
                LinuxDisplayOutput { name: "card0-eDP-1".into(), width: 2560, height: 1600, refresh_hz: 240.0, primary: true,
                    card: "card0".into(), desktop_position: Some((0, 0)), active: true, status: "active".into(),
                    modes: vec![(2560, 1600, 240.0)], pci: Some("0000:00:02.0".into()), ..Default::default() },
                LinuxDisplayOutput { name: "card1-HDMI-A-2".into(), width: 3840, height: 2160, refresh_hz: 239.995,
                    card: "card1".into(), status: "requested mode failed".into(), error: Some("create swapchain: x".into()),
                    requested: Some("3840x2160@30".into()), native: Some((3840, 2160, 239.995)), modes: vec![(3840, 2160, 239.995), (3840, 2160, 29.97)], ..Default::default() },
            ],
            arrangement: DisplayArrangement::Mirror { optimise_for: "card0-eDP-1".into() },
            pacing: DisplayPacing::Screen("card1-HDMI-A-2".into()),
            paced_by: Some("card1-HDMI-A-2".into()),
        };
        let inventory = DisplayInventory::read(&snapshot.pack(2.0)).expect("reads back");
        assert_eq!(inventory.snapshot, snapshot);
        assert_eq!(inventory.screens.len(), 1, "only placed screens have desktop rects");
        assert_eq!(inventory.screens[0].bounds, [0.0, 0.0, 1280.0, 800.0]);
    }
    #[test]
    fn every_pacing_setting_round_trips_on_the_wire() {
        for pacing in [DisplayPacing::Auto, DisplayPacing::Slowest, DisplayPacing::Fastest, DisplayPacing::Screen("card0-eDP-1".into())] {
            let args: Vec<String> = pacing.args().into_iter().map(str::to_owned).collect();
            assert_eq!(DisplayPacing::from_args(&args), Some(pacing));
        }
        assert_eq!(DisplayPacing::from_args(&["screen".into()]), None, "a screen setting names its screen");
        assert_eq!(DisplayPacing::from_args(&["sometimes".into()]), None);
    }
}
