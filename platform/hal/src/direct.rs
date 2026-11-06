//! Direct-display pipe data. This module has no backend state or OS calls.
//!
//! The direct backend drives one wide desktop: its screens side by side, left
//! to right along their top edges, at native pixels divided by the global DPI
//! factor (see `wide_desktop`). A screen is shown by one output or mirrored on
//! several (`DisplayArrangement`): it is rendered once, at its optimise-for
//! output's size, and each of its outputs shows that rectangle, scaled to fit
//! when its mode differs. A screen that does not fit within the GPU's limits
//! has no rectangle; its outputs show the whole desktop letterboxed.
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
    /// The monitor's own name from its EDID (`Monitor 27`); empty when it names none.
    pub monitor: String,
    /// Hash of the monitor's EDID (of the connector's mode list without one): the same monitor
    /// on any connector of any boot has the same fingerprint. Saved profiles are keyed by the
    /// connected monitors' fingerprints (`display_profiles`).
    pub fingerprint: u64,
    /// Selected mode width in native pixels (0 when no mode was selected).
    pub width: u32,
    /// Selected mode height in native pixels (0 when no mode was selected).
    pub height: u32,
    /// Selected mode refresh in Hz (0.0 when no mode was selected).
    pub refresh_hz: f64,
    /// The DRM card driving this connector, e.g. `card1`.
    pub card: String,
    /// The connector has a live swapchain and its last presentation succeeded.
    pub active: bool,
    /// Why this screen does not show what was asked: the requested mode failed (a
    /// fallback is shown) or the output itself failed. `None` when it shows `requested`.
    pub error: Option<String>,
    /// The mode asked for (`WxH@Hz` etc.), if any; `width`/`height`/`refresh_hz` are
    /// what is shown.
    pub requested: Option<String>,
    /// Human-readable state: `active`, `off`, `unsupported: …`, `failed: …`, `lost: …`.
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

/// One screen of the desktop: a rectangle of the wide desktop that one or more outputs show.
/// Windows, maximise, snap and the dock live on screens; an output is never a screen of its
/// own when it mirrors another.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LinuxDisplayScreen {
    /// The screen's stable name: its first arranged output's connector name (see
    /// `DisplayArrangement::group`), kept while that output is unplugged and when the
    /// optimise-for output changes, so the windows on it stay where they are.
    pub name: String,
    /// The outputs showing it, in arranged order. Two or more: mirrored.
    pub outputs: Vec<String>,
    /// The output it is optimised for: the screen has that output's size, its refresh paces
    /// the screen, and it shows the screen 1:1; every other output shows it scaled to fit.
    pub optimise_for: String,
    /// The screen's rectangle in the wide desktop, in native pixels.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// The main screen: the dock goes here, wherever it sits in the row.
    pub primary: bool,
}

/// Everything the renderer knows about connected displays.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LinuxDisplaySnapshot {
    /// `true` only while the direct Vulkan backend owns the displays.
    pub direct: bool,
    /// Outputs in default order: built-in panels first, then by name.
    pub outputs: Vec<LinuxDisplayOutput>,
    /// The desktop's screens left to right (see `LinuxDisplayScreen`).
    pub screens: Vec<LinuxDisplayScreen>,
    /// The arrangement the renderer runs (asked for, or restored from a saved profile); its
    /// `screens` over the connected outputs are `screens` above.
    pub arrangement: DisplayArrangement,
    /// The pacing setting (see `DisplayPacing`).
    pub pacing: DisplayPacing,
    /// The output whose presents pace the desktop now, `pacing` resolved against the
    /// connected outputs and the screens; `None`: every screen at its own rate
    /// (`Auto` with several screens) or no screen shows the desktop.
    pub paced_by: Option<String>,
    /// The virtual display, while it is the desktop (see `VirtualDisplay`). Its screen is
    /// the output row named `VIRTUAL_OUTPUT`.
    pub virtual_display: Option<VirtualDisplay>,
}

impl LinuxDisplaySnapshot {
    pub fn output(&self, name: &str) -> Option<&LinuxDisplayOutput> {
        self.outputs.iter().find(|o| o.name == name)
    }
    /// The screen output `name` shows; `None` while it is off, failed or not driven yet.
    pub fn screen_of(&self, name: &str) -> Option<&LinuxDisplayScreen> {
        self.screens.iter().find(|s| s.outputs.iter().any(|o| o == name))
    }
    pub fn main_screen(&self) -> Option<&LinuxDisplayScreen> {
        self.screens.iter().find(|s| s.primary)
    }
}

/// The output row name of the virtual display.
pub const VIRTUAL_OUTPUT: &str = "virtual";

/// A desktop of any size and refresh that no panel defines: the renderer draws it at exactly
/// `width` x `height` device pixels and paces it on its own clock at `refresh_mhz`, with or
/// without a connected panel. A remote viewer (Desk Link) shows it 1:1. The panels either show
/// it scaled to fit (aspect kept, letterboxed; a panel whose mode equals the size shows it 1:1
/// with a plain copy) or are not driven at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualDisplay {
    pub width: u32,
    pub height: u32,
    /// Refresh in millihertz (120 Hz = 120000).
    pub refresh_mhz: u32,
    pub panels: VirtualPanels,
}

/// What the physical panels show while the virtual display is the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VirtualPanels {
    /// The virtual desktop scaled to fit each panel (aspect kept), at the panel's own mode.
    #[default]
    Mirror,
    /// No scanout: the panels are not driven.
    Off,
}

impl VirtualPanels {
    pub fn as_str(self) -> &'static str { match self { Self::Mirror => "mirror", Self::Off => "off" } }
    pub fn parse(s: &str) -> Option<Self> { match s { "mirror" => Some(Self::Mirror), "off" => Some(Self::Off), _ => None } }
}

impl VirtualDisplay {
    pub const MIN_SIZE: u32 = 64;
    pub const MAX_SIZE: u32 = 16384;
    pub fn refresh_hz(&self) -> f64 { self.refresh_mhz as f64 / 1000.0 }
    /// `WxH@Hz` with the refresh in Hz (fractions allowed, `59.94`).
    pub fn mode_string(&self) -> String {
        if self.refresh_mhz % 1000 == 0 { format!("{}x{}@{}", self.width, self.height, self.refresh_mhz / 1000) }
        else { format!("{}x{}@{}", self.width, self.height, self.refresh_hz()) }
    }
    /// Parses `WxH@Hz` (or `WxH`, 60 Hz) and the panels word.
    pub fn parse(mode: &str, panels: &str) -> Result<Self, String> {
        let mode = mode.trim();
        let (size, rate) = mode.split_once('@').unwrap_or((mode, "60"));
        let (w, h) = size.split_once('x').ok_or_else(|| format!("{mode:?} is not WxH@Hz"))?;
        let width: u32 = w.parse().map_err(|_| format!("{mode:?}: width is not a number"))?;
        let height: u32 = h.parse().map_err(|_| format!("{mode:?}: height is not a number"))?;
        let hz: f64 = rate.parse().map_err(|_| format!("{mode:?}: refresh is not a number"))?;
        let panels = VirtualPanels::parse(panels.trim()).ok_or_else(|| format!("{panels:?}: the panels show `mirror` or `off`"))?;
        let display = Self { width, height, refresh_mhz: (hz * 1000.0).round() as u32, panels };
        display.validate()?;
        Ok(display)
    }
    pub fn validate(&self) -> Result<(), String> {
        let range = Self::MIN_SIZE..=Self::MAX_SIZE;
        if !range.contains(&self.width) || !range.contains(&self.height) {
            return Err(format!("a virtual display is {}..{} pixels wide and tall, not {}x{}", Self::MIN_SIZE, Self::MAX_SIZE, self.width, self.height));
        }
        if !(1000..=1_000_000).contains(&self.refresh_mhz) {
            return Err(format!("a virtual display refreshes at 1..1000 Hz, not {}", self.refresh_hz()));
        }
        Ok(())
    }
}

/// Which screen's refresh sets how often the desktop is rendered. Other screens show the
/// newest frame at their own refresh; no screen ever causes an extra render. Switching is
/// immediate (no modeset); the backend resolves it again on every hotplug, mode change and
/// arrangement change.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum DisplayPacing {
    /// Each screen is paced by its optimise-for output (a mirrored screen's other outputs show
    /// the newest frame at their own refresh). One screen: it paces (`paced_by` names it when it
    /// is mirrored). Several screens: each screen at its own refresh (`paced_by` is `None`) --
    /// the desktop is rendered as often as its fastest screen shows frames, and every screen
    /// presents the newest frame at its own refresh, so a slow monitor never holds back the
    /// panel and gets no frames it cannot show.
    #[default]
    Auto,
    /// The slowest output paces (least GPU work).
    Slowest,
    /// The fastest output paces (smoothest).
    Fastest,
    /// This output paces (by connector name); a disconnected one falls back to `Auto`.
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

/// One screen of the desktop as asked for.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ArrangedScreen {
    /// The connector names that show this screen, in the order they joined it; the first names
    /// the screen. Two or more: the screen is rendered once and mirrored on each of them.
    pub outputs: Vec<String>,
    /// The output the screen is optimised for: its mode sets the screen's size and its refresh
    /// paces the screen. `None`, or not connected: the largest connected output of the screen.
    pub optimise_for: Option<String>,
}

/// How the connected outputs make up the desktop's screens: any mix of extended and mirrored.
/// Every connected output shows exactly one screen or is off. A connected output this names
/// nowhere gets a screen of its own after the named ones (in default order), so the empty
/// arrangement extends the desktop over every output and "mirror everything" is one screen
/// listing every output.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DisplayArrangement {
    /// The screens left to right.
    pub screens: Vec<ArrangedScreen>,
    /// Outputs that are not driven (no mode set, no scanout).
    pub off: Vec<String>,
}

/// One screen over the outputs connected now (`DisplayArrangement::group`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenGroup {
    /// The screen's stable name (`LinuxDisplayScreen::name`).
    pub name: String,
    /// Indices into the connected outputs, in arranged order; never empty.
    pub outputs: Vec<usize>,
    /// The arranged optimise-for output when it is one of `outputs`.
    pub optimise_for: Option<usize>,
}

fn arranged_name_is_valid(name: &str) -> bool {
    !name.is_empty() && !name.chars().any(|c| c.is_whitespace() || matches!(c, ',' | '+' | '*' | ':'))
}

impl DisplayArrangement {
    /// Every output on a screen of its own, left to right in this order.
    pub fn extended<S: AsRef<str>>(names: &[S]) -> Self {
        Self { screens: names.iter().map(|n| ArrangedScreen { outputs: vec![n.as_ref().to_owned()], optimise_for: None }).collect(), off: Vec::new() }
    }

    /// Text form, also `MAKEPAD_DISPLAY_ARRANGEMENT` and the wire: screens left to right
    /// separated by `,`, a screen's outputs joined by `+`, `*` before the optimise-for output,
    /// `off:NAME` for an output that is not driven. `card1-DP-2,card1-DP-4,card1-DP-1+*card1-DP-3,off:card0-eDP-1`.
    pub fn text(&self) -> String {
        let mut parts: Vec<String> = self.screens.iter().map(|s| {
            s.outputs.iter().map(|o| if s.optimise_for.as_deref() == Some(o) { format!("*{o}") } else { o.clone() }).collect::<Vec<_>>().join("+")
        }).collect();
        parts.extend(self.off.iter().map(|o| format!("off:{o}")));
        parts.join(",")
    }

    /// Reads `text` (see `text`); blanks around the separators are ignored.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut out = Self::default();
        for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if let Some(name) = part.strip_prefix("off:") {
                out.off.push(name.trim().to_owned());
                continue;
            }
            let mut screen = ArrangedScreen::default();
            for output in part.split('+').map(str::trim) {
                let name = match output.strip_prefix('*') {
                    Some(name) => {
                        if screen.optimise_for.is_some() { return Err(format!("{part:?}: a screen is optimised for one output")); }
                        screen.optimise_for = Some(name.to_owned());
                        name
                    }
                    None => output,
                };
                screen.outputs.push(name.to_owned());
            }
            out.screens.push(screen);
        }
        out.validate()?;
        Ok(out)
    }

    /// Every name is a connector name, named once; every screen has an output, and its
    /// optimise-for output is one of them.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for name in self.screens.iter().flat_map(|s| s.outputs.iter()).chain(self.off.iter()) {
            if !arranged_name_is_valid(name) { return Err(format!("{name:?} is not a connector name")); }
            if !seen.insert(name.as_str()) { return Err(format!("{name} is arranged twice: an output shows one screen or is off")); }
        }
        for screen in &self.screens {
            if screen.outputs.is_empty() { return Err("a screen has no output".into()); }
            if let Some(name) = &screen.optimise_for {
                if !screen.outputs.contains(name) { return Err(format!("a screen is optimised for {name}, which does not show it")); }
            }
        }
        Ok(())
    }

    pub fn is_off(&self, name: &str) -> bool {
        self.off.iter().any(|o| o == name)
    }

    /// The screens over `connected` (the outputs connected now, in default order), left to
    /// right: each arranged screen with its connected outputs (one with none is left out),
    /// then a screen of its own for every connected output named nowhere. Off outputs are
    /// on no screen. A screen is named after its first ARRANGED output, connected or not.
    pub fn group(&self, connected: &[&str]) -> Vec<ScreenGroup> {
        let index = |name: &str| connected.iter().position(|c| *c == name);
        let mut groups: Vec<ScreenGroup> = self.screens.iter().filter_map(|s| {
            let outputs: Vec<usize> = s.outputs.iter().filter_map(|o| index(o)).collect();
            (!outputs.is_empty()).then(|| ScreenGroup {
                name: s.outputs[0].clone(),
                optimise_for: s.optimise_for.as_deref().and_then(index),
                outputs,
            })
        }).collect();
        for (i, name) in connected.iter().enumerate() {
            let named = self.is_off(name) || self.screens.iter().any(|s| s.outputs.iter().any(|o| o == name));
            if !named { groups.push(ScreenGroup { name: (*name).to_owned(), outputs: vec![i], optimise_for: None }); }
        }
        groups
    }
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
/// Desk Link's injected pointer: the uinput device (`BUS_VIRTUAL`, this vendor and product)
/// the WM makes for a viewer's input. Its wheel already carries the content direction the
/// viewer's user saw, in evdev's own sense (+1 = the view moves up, tilted right = right), so
/// the direct backend passes it through and never applies natural scrolling to it.
pub const DESK_LINK_INPUT_VENDOR: u16 = 0x4d50;
pub const DESK_LINK_POINTER_PRODUCT: u16 = 1;

/// Owned arguments for a display request, also retained by Cx until its reply.
#[derive(Clone, Debug)]
pub enum DisplayCommand {
    /// The screen output `name` shows becomes the main screen.
    Source(String),
    Mode { name: String, mode: Option<String> },
    Arrangement(DisplayArrangement),
    Pacing(DisplayPacing),
    /// The virtual display becomes the desktop (or changes size, refresh or what the panels
    /// show: no modeset, the desktop is laid out again); `None`: the panels' own modes again.
    Virtual(Option<VirtualDisplay>),
}

impl DisplayCommand {
    /// Pure argument validation, shared by the client and the receiving host.
    pub fn validate(&self) -> Result<(), String> {
        if let Self::Virtual(Some(display)) = self { display.validate()?; }
        if let Self::Arrangement(arrangement) = self { arrangement.validate()?; }
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
            Self::Mode { .. } => cmd_tag::DirectDisplayMode,
            Self::Arrangement(_) => cmd_tag::DirectDisplayArrangement,
            Self::Pacing(_) => cmd_tag::DirectDisplayPacing,
            Self::Virtual(_) => cmd_tag::DirectDisplayVirtual,
        }
    }
}

/// `WxH`, `WxH@Hz` or `WxH-Hz`, digits only: the forms `MAKEPAD_DRM_MODE(S)` take.
pub fn mode_string_is_valid(mode: &str) -> bool {
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

/// Inventory payload plus the screens in desktop points, left to right.
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
            w.str(&o.name).str(&o.monitor).u64(o.fingerprint).u32(o.width).u32(o.height).f64(o.refresh_hz).str(&o.card);
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
        w.u32(self.screens.len() as u32);
        for s in &self.screens {
            w.str(&s.name).str(&s.optimise_for).u32(s.outputs.len() as u32);
            for o in &s.outputs { w.str(o); }
            w.u32(s.x).u32(s.y).u32(s.width).u32(s.height).u8(s.primary as u8);
            w.f64(s.x as f64 / dpi).f64(s.y as f64 / dpi).f64(s.width as f64 / dpi).f64(s.height as f64 / dpi);
        }
        w.str(&self.arrangement.text());
        let args = self.pacing.args();
        w.u32(args.len() as u32);
        for arg in args { w.str(arg); }
        w.u8(self.paced_by.is_some() as u8);
        if let Some(name) = &self.paced_by { w.str(name); }
        w.u8(self.virtual_display.is_some() as u8);
        if let Some(v) = &self.virtual_display { w.u32(v.width).u32(v.height).u32(v.refresh_mhz).u8(v.panels as u8); }
        w.finish()
    }
}

impl DisplayInventory {
    pub fn read(bytes: &[u8]) -> Option<Self> {
        let mut r = crate::media::PackR::new(bytes);
        let direct = r.u8() != 0;
        let mut outputs = Vec::new();
        for _ in 0..r.u32() {
            if !r.ok { return None; }
            let mut o = LinuxDisplayOutput {
                name: r.str(), monitor: r.str(), fingerprint: r.u64(), width: r.u32(), height: r.u32(), refresh_hz: r.f64(),
                card: r.str(), ..Default::default()
            };
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
        let mut screens = Vec::new();
        let mut infos = Vec::new();
        for _ in 0..r.u32() {
            if !r.ok { return None; }
            let mut s = LinuxDisplayScreen { name: r.str(), optimise_for: r.str(), ..Default::default() };
            for _ in 0..r.u32() {
                if !r.ok { return None; }
                s.outputs.push(r.str());
            }
            s.x = r.u32(); s.y = r.u32(); s.width = r.u32(); s.height = r.u32();
            s.primary = r.u8() != 0;
            let bounds = [r.f64(), r.f64(), r.f64(), r.f64()];
            infos.push(crate::ScreenInfo { bounds, work_area: bounds, is_primary: s.primary });
            screens.push(s);
        }
        let arrangement = DisplayArrangement::parse(&r.str()).ok()?;
        let count = r.u32().min(2);
        let args: Vec<String> = (0..count).map(|_| r.str()).collect();
        let pacing = DisplayPacing::from_args(&args)?;
        let paced_by = if r.u8() != 0 { Some(r.str()) } else { None };
        let virtual_display = if r.u8() != 0 {
            let (width, height, refresh_mhz) = (r.u32(), r.u32(), r.u32());
            let panels = if r.u8() == VirtualPanels::Off as u8 { VirtualPanels::Off } else { VirtualPanels::Mirror };
            Some(VirtualDisplay { width, height, refresh_mhz, panels })
        } else { None };
        (r.ok && r.at_end()).then_some(Self { snapshot: LinuxDisplaySnapshot { direct, outputs, screens, arrangement, pacing, paced_by, virtual_display }, screens: infos })
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_inventory_round_trips_with_screens_mirrors_errors_and_modes() {
        let output = |name: &str, monitor: &str, w: u32, h: u32| LinuxDisplayOutput {
            name: name.into(), monitor: monitor.into(), fingerprint: 0x1234_5678_9abc_def0, width: w, height: h, refresh_hz: 60.0,
            card: "card1".into(), active: true, status: "active".into(), modes: vec![(w, h, 60.0)], pci: Some("0000:01:00.0".into()), ..Default::default()
        };
        let mut failed = output("card1-DP-9", "", 0, 0);
        failed.active = false;
        failed.status = "requested mode failed".into();
        failed.error = Some("create swapchain: x".into());
        failed.requested = Some("3840x2160@30".into());
        failed.native = Some((3840, 2160, 239.995));
        let screen = |name: &str, outputs: &[&str], x: u32, w: u32, h: u32, primary: bool| LinuxDisplayScreen {
            name: name.into(), outputs: outputs.iter().map(|s| s.to_string()).collect(), optimise_for: outputs[0].into(),
            x, y: 0, width: w, height: h, primary,
        };
        let snapshot = LinuxDisplaySnapshot {
            direct: true,
            outputs: vec![output("card1-DP-1", "Monitor A", 2560, 1440), output("card1-DP-2", "Monitor B", 1920, 1080),
                output("card1-DP-3", "Monitor C", 3840, 2160), output("card1-DP-4", "Monitor D", 2560, 1440), failed],
            screens: vec![screen("card1-DP-2", &["card1-DP-2"], 0, 1920, 1080, false), screen("card1-DP-1", &["card1-DP-1"], 1920, 2560, 1440, true),
                screen("card1-DP-3", &["card1-DP-3", "card1-DP-4"], 4480, 3840, 2160, false)],
            arrangement: DisplayArrangement::parse("card1-DP-2,card1-DP-1,*card1-DP-3+card1-DP-4,off:card1-DP-9").unwrap(),
            pacing: DisplayPacing::Screen("card1-DP-1".into()),
            paced_by: Some("card1-DP-1".into()),
            virtual_display: Some(VirtualDisplay { width: 7680, height: 2160, refresh_mhz: 120_000, panels: VirtualPanels::Off }),
        };
        let inventory = DisplayInventory::read(&snapshot.pack(2.0)).expect("reads back");
        assert_eq!(inventory.snapshot, snapshot);
        assert_eq!(inventory.screens.len(), 3, "one screen info per screen, never one per mirror output");
        assert_eq!(inventory.screens[1].bounds, [960.0, 0.0, 1280.0, 720.0]);
        assert!(inventory.screens[1].is_primary, "the main screen in the middle of the row");
    }
    #[test]
    fn an_arrangement_groups_the_connected_outputs() {
        let a = DisplayArrangement::parse(" B , A , C+*D , off:E ").unwrap();
        assert_eq!(a.text(), "B,A,C+*D,off:E");
        assert_eq!(DisplayArrangement::parse(&a.text()).unwrap(), a, "the text form round-trips");
        let connected = ["A", "B", "C", "D", "E", "F"];
        let groups = a.group(&connected);
        let names: Vec<_> = groups.iter().map(|g| (g.name.as_str(), g.outputs.clone(), g.optimise_for)).collect();
        assert_eq!(names, vec![("B", vec![1], None), ("A", vec![0], None), ("C", vec![2, 3], Some(3)), ("F", vec![5], None)],
            "the user's [B] [A] [C+D], E off, a new F extends on the right");
        // C unplugged: the mirror keeps going on D under the same name; D unplugged too: the screen goes
        let groups = a.group(&["A", "B", "D"]);
        assert_eq!((groups[2].name.as_str(), groups[2].outputs.clone(), groups[2].optimise_for), ("C", vec![2], Some(2)));
        assert_eq!(a.group(&["A", "B"]).len(), 2);
        assert_eq!(DisplayArrangement::default().group(&["A", "B"]).len(), 2, "nothing arranged: extend everything");
        assert!(DisplayArrangement::parse("A+B,A").is_err(), "an output shows one screen");
        assert!(DisplayArrangement::parse("*A+*B").is_err(), "one optimise-for output");
        assert!(DisplayArrangement::parse("A,off:A").is_err());
        assert!(DisplayArrangement::parse("A B").is_err());
        assert_eq!(DisplayArrangement::parse("").unwrap(), DisplayArrangement::default());
    }
    #[test]
    fn a_virtual_display_parses_validates_and_names_its_mode() {
        let v = VirtualDisplay::parse("7680x2160@120", "off").unwrap();
        assert_eq!(v, VirtualDisplay { width: 7680, height: 2160, refresh_mhz: 120_000, panels: VirtualPanels::Off });
        assert_eq!(v.mode_string(), "7680x2160@120");
        assert_eq!(VirtualDisplay::parse("1920x1080@59.94", "mirror").unwrap().refresh_mhz, 59_940);
        assert_eq!(VirtualDisplay::parse("1920x1080@59.94", "mirror").unwrap().mode_string(), "1920x1080@59.94");
        assert!(VirtualDisplay::parse("10x10@60", "off").is_err());
        assert!(VirtualDisplay::parse("1920x1080@0", "off").is_err());
        assert!(VirtualDisplay::parse("1920x1080@60", "sometimes").is_err());
        assert!(DisplayCommand::Virtual(Some(VirtualDisplay { width: 99999, ..v })).validate().is_err());
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
