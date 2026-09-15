//! Linux system controls behind the shell's audio, monitor and power panels:
//! the default audio output and input (device lists, selection, volume,
//! mute), backlight brightness where the hardware has one, and the system
//! battery. Backend only — the panels draw what a [`SystemSnapshot`] says.
//!
//! # Shape
//!
//! One long-lived worker (`cx.thread_spawner()`, thread `wm-system`) owns
//! every OS call. The UI talks to it through [`SystemController`]:
//!
//! * Slider values (output volume, input volume, brightness) go into one
//!   atomic slot each. A drag overwrites the slot; the worker takes the
//!   latest value per pass, so a drag can never queue more than one
//!   process per slider.
//! * Discrete commands ([`SystemCommand`]) go through a bounded queue with a
//!   non-blocking send. A full queue hands the command back for a retry on a
//!   later frame.
//! * The worker publishes immutable [`SystemSnapshot`]s over a bounded
//!   to-UI channel (the UI keeps the newest with `try_recv_flush`) and raises
//!   the UI signal. A snapshot holds observed state only; every command's
//!   result is an explicit [`CommandOutcome`], and `applied_seq` tells the UI
//!   which of its commands the worker has finished with.
//! * The worker sleeps in `poll(2)` on an eventfd the controller writes and
//!   on the stdout of a `pactl subscribe` child, so hotplug, default-device
//!   changes and volume changes made elsewhere arrive as events, not polls.
//!   Backlight and battery are re-read from sysfs every few seconds (two
//!   tiny file reads) and on [`SystemCommand::Refresh`].
//!
//! # OS primitives
//!
//! * Audio: `pactl` (libpulse's own client, talking to pipewire-pulse or
//!   PulseAudio) with `-f json` for reads and plain verbs for writes. Every
//!   invocation is `argv`-only (no shell), has a deadline and a read cap,
//!   and gets `PR_SET_PDEATHSIG` so nothing outlives the worker. Volume and
//!   mute act on the server's default resolved at action time (never on a
//!   device remembered from an earlier snapshot). Selecting a device sets
//!   the server-wide default (`default.audio.sink`/`source` metadata on
//!   PipeWire), which is what the session manager's re-link policy follows
//!   for every default-following stream, native or Pulse; the outcome then
//!   reports which streams were observed on the new default (`pw-dump`,
//!   the whole graph) and which stayed, instead of claiming a move.
//! * Brightness: `/sys/class/backlight/*`. Writes try the sysfs node first
//!   (works where a udev rule grants the seat user write access) and fall
//!   back to logind's `Session.SetBrightness` over `busctl` (the caller's own
//!   active session, no polkit, no privilege). A denied write is reported as
//!   such; nothing is dimmed by other means.
//! * Battery: `/sys/class/power_supply/*` with `type=Battery`, excluding
//!   `scope=Device` (mouse/keyboard batteries) and absent packs. Mains/USB
//!   supplies give the AC state.
//! * Display scale: the Display panel's DPI slider is persisted as integer
//!   hundredths (`130` = 1.30×) in `$XDG_CONFIG_HOME/makepad/wm/display-scale`
//!   (only when `XDG_CONFIG_HOME` is absolute) or else
//!   `$HOME/.config/makepad/wm/display-scale`. The worker reads it once at
//!   start-up (`display_settings_loaded` says the read happened; `dpi_scale`
//!   is `Some` only for a valid 100..=300 value, never a made-up default) and
//!   writes it on [`SystemCommand::SaveDpiScale`] through a per-PID temporary
//!   file renamed over the target. The scale itself is applied by the UI
//!   through the platform's `dpi_override`; a failed save is reported as an
//!   outcome and changes nothing on screen.
//! * Display source ("Optimize for"): the connector name whose native
//!   pixels define the shared framebuffer, in `display-source` next to
//!   `display-scale`, same read/write rules ([`SystemCommand::SaveDisplaySource`],
//!   `display_source`). The name is data for the renderer's own selection
//!   API, never a command; it is validated as a bounded connector name.
//! * GPU choice ("Compositor"): which GPU the compositor should start on
//!   next time, in `display-gpu` beside the two above as the PCI identity
//!   `<pci-address> <vendor>:<device>` (`cardN` numbering is not stable
//!   across boots). This is the display GPU, not each application's.
//!   Nothing changes at runtime: the session script resolves the identity
//!   to that boot's card and sets `MAKEPAD_DRM_DEVICE` for the WM process
//!   with `MAKEPAD_WM_GPU_FROM_SAVED=1`; an external `MAKEPAD_DRM_DEVICE`
//!   (marker unset) is recorded as `gpu_env`. The panel shows the current
//!   GPU and the next-start one. The worker also lists the GPUs from
//!   `/sys/class/drm` (PCI ids, driver, connected connectors) so the panel
//!   can offer only usable ones.
//! * Pointer speed: mouse and touchpad multipliers as integer hundredths
//!   (`100` = 1.00×, range 25..=300) in `mouse-speed` and `touchpad-speed`
//!   beside the display files, same read/write rules
//!   ([`SystemCommand::SavePointerSpeed`], `pointer_speeds`). The worker
//!   reads each once at start-up (`input_settings_loaded` says the read
//!   happened; a slot is `Some` only for a valid in-range value). The UI
//!   applies the platform's process-wide speed; this only persists it.
//!
//! Compiled for Linux only (the inner `cfg` below makes the module empty
//! elsewhere), so no other platform's behaviour changes.

#![cfg(all(target_os = "linux", not(target_env = "ohos")))]

use {
    makepad_strict_json::{parse as parse_json, Value},
    makepad_widgets::{
        makepad_platform::{
            linux_input::{POINTER_SPEED_MAX, POINTER_SPEED_MIN},
            thread::{
                to_ui_bounded, SpawnError, TaskHandle, ThreadOptions, ThreadSpawner, ToUIReceiver,
                ToUISender,
            },
        },
        Cx,
    },
    std::{
        collections::VecDeque,
        ffi::{c_int, c_short, c_uint, c_ulong},
        fmt,
        fs::File,
        io::{ErrorKind, Read, Write},
        num::NonZeroUsize,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::process::CommandExt,
        },
        path::{Path, PathBuf},
        process::{Child, ChildStdout, Command, Stdio},
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError},
            Arc,
        },
        thread::sleep,
        time::Duration,
    },
};

// ======================================================================
// Tunables
// ======================================================================

/// `PA_VOLUME_NORM`: the raw volume that is 100%.
const PA_VOLUME_NORM: u64 = 0x10000;
const BACKLIGHT_ROOT: &str = "/sys/class/backlight";
const POWER_ROOT: &str = "/sys/class/power_supply";
/// Deadline for one `pactl` invocation.
const COMMAND_TIMEOUT: f64 = 2.5;
/// Deadline for the logind call (busctl gets its own 2s bus timeout inside).
const LOGIND_TIMEOUT: f64 = 3.5;
/// Read cap for device lists (verbose property maps per device).
const LIST_OUTPUT_CAP: usize = 4 << 20;
/// Read cap for everything else, and for stderr.
const SMALL_OUTPUT_CAP: usize = 64 << 10;
/// Read cap for `pw-dump` (the whole PipeWire graph; ~150 KB on an idle
/// desktop, a few MB with many nodes).
const DUMP_OUTPUT_CAP: usize = 32 << 20;
/// `pw-dump` nests params several levels deeper than the strict parser's
/// default cap.
const DUMP_JSON_DEPTH: u32 = 32;
/// Quiet time after the last audio event before re-sampling.
const AUDIO_DEBOUNCE: f64 = 0.15;
/// Never re-sample audio more often than this.
const AUDIO_MIN_INTERVAL: f64 = 0.25;
/// Audio poll cadence while the event stream is down.
const AUDIO_POLL_INTERVAL: f64 = 5.0;
/// Backlight and battery re-read cadence.
const SYSFS_INTERVAL: f64 = 5.0;
const SUBSCRIBE_BACKOFF_MIN: f64 = 2.0;
const SUBSCRIBE_BACKOFF_MAX: f64 = 30.0;
/// How many command outcomes a snapshot carries.
const OUTCOME_HISTORY: usize = 8;
const COMMAND_QUEUE: usize = 32;
/// Discrete commands applied per worker pass; the rest wait for the next
/// pass so slider values and sysfs reads are never starved by a backlog.
const COMMANDS_PER_PASS: usize = 8;
const SNAPSHOT_QUEUE: usize = 4;
/// Re-reads of a backlight after a write, 40ms apart, for hardware that
/// applies asynchronously.
const BRIGHTNESS_SETTLE_READS: usize = 3;
const BRIGHTNESS_SETTLE: Duration = Duration::from_millis(40);
/// Grace before re-counting streams after a default-device change, so the
/// session manager's re-links are observed rather than raced.
const STREAM_SETTLE: Duration = Duration::from_millis(150);
/// Longest single sleep in the worker's poll.
const MAX_POLL_MS: f64 = 60_000.0;
/// Line buffer cap for `pactl subscribe` output.
const SUBSCRIBE_LINE_CAP: usize = 64 << 10;
/// The persisted display scale's range, in hundredths (1.00× .. 3.00×).
pub const DISPLAY_SCALE_MIN: u32 = 100;
pub const DISPLAY_SCALE_MAX: u32 = 300;
/// The display settings files under the user's config root.
const DISPLAY_SETTINGS_DIR: [&str; 2] = ["makepad", "wm"];
const DISPLAY_SCALE_FILE: &str = "display-scale";
const DISPLAY_SOURCE_FILE: &str = "display-source";
/// Longest connector name accepted as a display source (`DP-2`,
/// `HDMI-A-1`, `eDP-1` are a few bytes; this is a sanity bound).
pub const DISPLAY_SOURCE_MAX_LEN: usize = 128;
const DISPLAY_GPU_FILE: &str = "display-gpu";
/// Longest persisted GPU identity line (`0000:01:00.0 10de:2b85`).
pub const GPU_CHOICE_MAX_LEN: usize = 128;
const MOUSE_SPEED_FILE: &str = "mouse-speed";
const TOUCHPAD_SPEED_FILE: &str = "touchpad-speed";
const DRM_ROOT: &str = "/sys/class/drm";

// ======================================================================
// libc — the three calls std does not wrap
// ======================================================================

const POLLIN: c_short = 0x001;
const POLLERR: c_short = 0x008;
const POLLHUP: c_short = 0x010;
const POLLNVAL: c_short = 0x020;
const EFD_NONBLOCK: c_int = 0o4000;
const EFD_CLOEXEC: c_int = 0o2000000;
const PR_SET_PDEATHSIG: c_int = 1;
const SIGTERM: c_ulong = 15;
const EINTR: i32 = 4;

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

extern "C" {
    fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
    fn eventfd(initval: c_uint, flags: c_int) -> c_int;
    fn prctl(option: c_int, arg2: c_ulong, arg3: c_ulong, arg4: c_ulong, arg5: c_ulong) -> c_int;
}

fn now() -> f64 {
    Cx::monotonic_now()
}

// ======================================================================
// Observed state
// ======================================================================

/// Command sequence number handed out by the controller; `0` is "nothing".
pub type Seq = u32;

fn seq_after(a: Seq, b: Seq) -> bool {
    a.wrapping_sub(b) as i32 > 0
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Ready,
    /// Why the subsystem cannot be read (no `pactl`, no sound server, ...).
    Unavailable(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Running,
    Idle,
    Suspended,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioDirection {
    Output,
    Input,
}

/// One sink (output) or source (input) as the sound server lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    /// The server's device name — the identity for `SelectOutput`/`SelectInput`.
    pub id: String,
    pub index: u32,
    /// The human label (`description`).
    pub label: String,
    /// The short name PipeWire gives it (`node.nick`), when there is one.
    pub nick: Option<String>,
    pub state: DeviceState,
    /// The active port's label ("Headphones", "HDMI / DisplayPort").
    pub port: Option<String>,
    /// False when the active port reports "not available" (jack unplugged).
    pub plugged: bool,
    /// Volume clamped to 0..=100 for a normal slider.
    pub percent: u32,
    /// Volume as reported, which can exceed 100 when something else boosted it.
    pub raw_percent: u32,
    pub muted: bool,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioState {
    pub availability: Availability,
    /// Real outputs, default first-class via `is_default`. The server's
    /// dummy `auto_null` sink is not listed.
    pub outputs: Vec<AudioDevice>,
    /// Microphone-like sources: monitors and loopbacks are filtered out.
    pub inputs: Vec<AudioDevice>,
    /// How many sources the filter hid.
    pub hidden_inputs: usize,
    /// The default sink's id when it is a listed output.
    pub default_output: Option<String>,
    /// The default source's id when it is a listed input.
    pub default_input: Option<String>,
    /// True while `pactl subscribe` delivers events; false means the worker
    /// is polling every few seconds instead.
    pub live_events: bool,
}

impl AudioState {
    fn unavailable(reason: String, live_events: bool) -> Self {
        Self {
            availability: Availability::Unavailable(reason),
            outputs: Vec::new(),
            inputs: Vec::new(),
            hidden_inputs: 0,
            default_output: None,
            default_input: None,
            live_events,
        }
    }

    pub fn default_output(&self) -> Option<&AudioDevice> {
        self.outputs.iter().find(|d| d.is_default)
    }

    pub fn default_input(&self) -> Option<&AudioDevice> {
        self.inputs.iter().find(|d| d.is_default)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrightnessCapability {
    /// No `/sys/class/backlight` device: the panel says "Fixed brightness".
    NoBacklight,
    Backlight,
}

/// How the last brightness write got through, learned by trying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrightnessAccess {
    Untested,
    Sysfs,
    Logind,
    Denied(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlight {
    pub name: String,
    /// The kernel's `type`: `raw`, `firmware` or `platform`.
    pub kind: String,
    pub max: u32,
    pub raw: u32,
    pub percent: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrightnessState {
    pub capability: BrightnessCapability,
    /// Preferred device first (`raw` before `firmware` before `platform`).
    pub devices: Vec<Backlight>,
    pub access: BrightnessAccess,
}

impl BrightnessState {
    pub fn primary(&self) -> Option<&Backlight> {
        self.devices.first()
    }

    pub fn percent(&self) -> Option<u32> {
        self.primary().map(|b| b.percent)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerCapability {
    /// No system battery (a desktop): show nothing.
    NoBattery,
    Battery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryStatus {
    Unknown,
    Charging,
    Discharging,
    NotCharging,
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatteryInfo {
    pub name: String,
    pub model: Option<String>,
    pub percent: Option<u32>,
    pub status: BatteryStatus,
    /// `(energy_now, energy_full)` in µWh when the driver reports energy.
    pub energy_uwh: Option<(u64, u64)>,
    /// `(charge_now, charge_full)` in µAh when the driver reports charge.
    pub charge_uah: Option<(u64, u64)>,
    pub seconds_to_empty: Option<u32>,
    pub seconds_to_full: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerState {
    pub capability: PowerCapability,
    pub batteries: Vec<BatteryInfo>,
    /// Energy-weighted across packs when every pack reports energy (or
    /// charge); otherwise the mean of the `capacity` files.
    pub percent: Option<u32>,
    pub status: BatteryStatus,
    /// `Some(true)` on external power, `Some(false)` on battery, `None` when
    /// no Mains/USB supply is listed (typical desktop).
    pub ac_online: Option<bool>,
    /// Only when a single pack reports it; never estimated.
    pub seconds_to_empty: Option<u32>,
    pub seconds_to_full: Option<u32>,
}

/// One DRM card — a GPU — as sysfs lists it, with the connectors that have
/// something plugged in. The renderer's outputs are named `cardN-…`, so
/// `card` links an output to its GPU; `pci` is what the Display panel
/// persists, because `cardN` numbering can change between boots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuInfo {
    /// `card0`, `card1`: the DRM node name for this boot.
    pub card: String,
    /// The PCI address (`0000:01:00.0`): the identity that survives a reboot.
    pub pci: String,
    /// PCI vendor id, lowercase hex without `0x` (`8086`, `10de`, `1002`).
    pub vendor: String,
    /// PCI device id, lowercase hex without `0x`.
    pub device: String,
    /// The kernel driver bound to it (`xe`, `i915`, `nvidia`, `amdgpu`).
    pub driver: String,
    /// Connectors reporting `connected`, without the `cardN-` prefix.
    pub connected: Vec<String>,
}

impl GpuInfo {
    /// The line the Display panel persists and the session script resolves:
    /// `<pci> <vendor>:<device>`.
    pub fn identity(&self) -> String {
        format!("{} {}:{}", self.pci, self.vendor, self.device)
    }

    /// True when an identity line names exactly this card: the PCI address
    /// AND the vendor:device pair, both. A different GPU at the same
    /// address (a stick moved to another machine) or the same model at
    /// another address is not a match — the choice then falls back to
    /// Auto. The session script resolves with the same rule.
    pub fn matches(&self, identity: &str) -> bool {
        !self.pci.is_empty() && identity == self.identity()
    }

    pub fn vendor_name(&self) -> &'static str {
        match self.vendor.as_str() {
            "8086" => "Intel",
            "10de" => "NVIDIA",
            "1002" => "AMD",
            "1a03" => "ASPEED",
            "15ad" => "VMware",
            "1af4" => "virtio",
            _ => "GPU",
        }
    }

    /// `Intel · xe`, `NVIDIA · nvidia`.
    pub fn label(&self) -> String {
        if self.driver.is_empty() {
            self.vendor_name().to_string()
        } else {
            format!("{} \u{b7} {}", self.vendor_name(), self.driver)
        }
    }

    /// A built-in panel is connected: what the renderer's automatic choice
    /// prefers.
    pub fn has_builtin(&self) -> bool {
        self.connected
            .iter()
            .any(|c| c.starts_with("eDP") || c.starts_with("LVDS") || c.starts_with("DSI"))
    }
}

// ======================================================================
// Commands and outcomes
// ======================================================================

/// Discrete commands. Slider values do not go here — they have their own
/// coalescing setters on the controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemCommand {
    SetOutputMuted(bool),
    ToggleOutputMute,
    SetInputMuted(bool),
    ToggleInputMute,
    /// Make this sink (an `AudioDevice::id` from `outputs`) the default.
    SelectOutput(String),
    /// Make this source (an `AudioDevice::id` from `inputs`) the default.
    SelectInput(String),
    /// Re-read everything now (a panel opening, for instance).
    Refresh,
    /// Persist the display scale (integer hundredths, 100..=300) once the
    /// slider is released. The scale is already applied on screen by then;
    /// this only decides what the next start uses.
    SaveDpiScale(u32),
    /// Persist the "Optimize for" output: the connector name (as the
    /// renderer lists it) whose native pixels define the shared
    /// framebuffer. Only what the next start asks the renderer for.
    SaveDisplaySource(String),
    /// Persist which GPU the compositor should start on next time:
    /// `Some(identity)` from [`GpuInfo::identity`], `None` for Auto (the
    /// renderer's own preference, which removes the file). Nothing changes
    /// until the WM restarts; the session script resolves the identity to
    /// that boot's DRM card. This is the display GPU, not each app's.
    SaveGpuChoice(Option<String>),
    /// Persist a pointer speed (integer hundredths, 25..=300). The speed is
    /// already applied in-process by then; this only decides the next start.
    SavePointerSpeed { touchpad: bool, value: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    OutputVolume,
    InputVolume,
    Brightness,
    OutputMute,
    InputMute,
    SelectOutput,
    SelectInput,
    Refresh,
    DpiScale,
    DisplaySource,
    GpuChoice,
    PointerSpeed,
}

/// Which streams a [`StreamFollow`] counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamScope {
    /// Every PipeWire stream node and its links, native clients (ALSA via
    /// pipewire-alsa, native apps) and Pulse clients alike — from `pw-dump`.
    AllStreams,
    /// Only Pulse clients' streams — from `pactl`, when `pw-dump` is
    /// missing or its output could not be read. Native streams are not
    /// counted then.
    PulseClients,
}

/// Streams observed after a default-device change: those now linked to the
/// new default and those that stayed elsewhere (pinned to a device by their
/// client, or not re-linked by the session manager).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamFollow {
    pub following: usize,
    pub elsewhere: usize,
    pub scope: StreamScope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandResult {
    /// The backend accepted the command and the snapshot shows the result.
    Applied,
    /// The default device changed; `streams` is `None` when the stream list
    /// could not be read afterwards (nothing is claimed about them then).
    Selected { streams: Option<StreamFollow> },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutcome {
    pub seq: Seq,
    pub kind: CommandKind,
    pub result: CommandResult,
}

/// Everything the worker observed, published as one immutable value.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemSnapshot {
    /// Increments per publish.
    pub serial: u64,
    /// The newest command sequence the worker has finished; a command is
    /// pending while its seq is after this.
    pub applied_seq: Seq,
    /// `Cx::monotonic_now()` at publish.
    pub sampled_at: f64,
    pub audio: AudioState,
    pub brightness: BrightnessState,
    pub power: PowerState,
    /// The last few outcomes, oldest first; match on `seq`.
    pub outcomes: Vec<CommandOutcome>,
    /// The persisted display scale in hundredths: what the file held at
    /// start-up, then what the last successful save wrote. `None` when there
    /// is no valid saved value — the launch default stands then.
    pub dpi_scale: Option<u32>,
    /// The persisted "Optimize for" connector name: what the file held at
    /// start-up, then what the last successful save wrote. `None` when
    /// there is no valid saved name — the renderer's own default stands.
    pub display_source: Option<String>,
    /// The start-up read of the display settings files has happened
    /// (whatever they held), so the UI may apply them once.
    pub display_settings_loaded: bool,
    /// The GPUs sysfs lists (`/sys/class/drm/cardN`) with their connected
    /// connectors; the renderer's outputs (`cardN-…`) map onto `card`.
    pub gpus: Vec<GpuInfo>,
    /// The persisted next-start GPU: `Some(identity)`, or `None` for Auto.
    pub gpu_choice: Option<String>,
    /// External `MAKEPAD_DRM_DEVICE` this process was started with. Set
    /// only when the wrapper did not apply a saved choice
    /// (`MAKEPAD_WM_GPU_FROM_SAVED` is not `1`): a true override. `None`
    /// for Auto, a wrapper-applied saved choice, or no device in the
    /// environment.
    pub gpu_env: Option<String>,
    /// Saved pointer speeds in hundredths: index 0 mouse, index 1 touchpad.
    /// `None` when that file was missing or out of range — the process
    /// default stands then.
    pub pointer_speeds: [Option<u32>; 2],
    /// The start-up read of `mouse-speed` / `touchpad-speed` has happened
    /// (whatever they held), so the UI may apply them once.
    pub input_settings_loaded: bool,
}

impl Default for SystemSnapshot {
    fn default() -> Self {
        Self {
            serial: 0,
            applied_seq: 0,
            sampled_at: 0.0,
            dpi_scale: None,
            display_source: None,
            display_settings_loaded: false,
            gpus: Vec::new(),
            gpu_choice: None,
            gpu_env: None,
            pointer_speeds: [None, None],
            input_settings_loaded: false,
            audio: AudioState::unavailable("not sampled yet".into(), false),
            brightness: BrightnessState {
                capability: BrightnessCapability::NoBacklight,
                devices: Vec::new(),
                access: BrightnessAccess::Untested,
            },
            power: PowerState {
                capability: PowerCapability::NoBattery,
                batteries: Vec::new(),
                percent: None,
                status: BatteryStatus::Unknown,
                ac_online: None,
                seconds_to_empty: None,
                seconds_to_full: None,
            },
            outcomes: Vec::new(),
        }
    }
}

impl SystemSnapshot {
    pub fn outcome(&self, seq: Seq) -> Option<&CommandOutcome> {
        self.outcomes.iter().rev().find(|o| o.seq == seq)
    }
}

// ======================================================================
// The controller — the UI thread's handle
// ======================================================================

#[derive(Debug)]
pub enum StartError {
    Wake(std::io::Error),
    Spawn(SpawnError),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wake(error) => write!(f, "eventfd: {error}"),
            Self::Spawn(error) => write!(f, "worker: {error}"),
        }
    }
}

struct Envelope {
    seq: Seq,
    command: SystemCommand,
}

/// A slot is `seq << 32 | value`; `SLOT_EMPTY` is taken (value never reaches
/// `u32::MAX`, the values are 0..=100).
const SLOT_EMPTY: u64 = u64::MAX;

struct Slots {
    output_volume: AtomicU64,
    input_volume: AtomicU64,
    brightness: AtomicU64,
}

impl Slots {
    fn new() -> Self {
        Self {
            output_volume: AtomicU64::new(SLOT_EMPTY),
            input_volume: AtomicU64::new(SLOT_EMPTY),
            brightness: AtomicU64::new(SLOT_EMPTY),
        }
    }
}

fn pack_slot(seq: Seq, value: u32) -> u64 {
    ((seq as u64) << 32) | value as u64
}

fn take_slot(slot: &AtomicU64) -> Option<(Seq, u32)> {
    let packed = slot.swap(SLOT_EMPTY, Ordering::AcqRel);
    if packed == SLOT_EMPTY {
        None
    } else {
        Some(((packed >> 32) as Seq, (packed & 0xffff_ffff) as u32))
    }
}

/// Owned by the UI. Every method returns at once; nothing here locks or
/// waits on the worker.
pub struct SystemController {
    commands: SyncSender<Envelope>,
    slots: Arc<Slots>,
    stop: Arc<AtomicBool>,
    wake: Arc<File>,
    snapshots: ToUIReceiver<Arc<SystemSnapshot>>,
    latest: Arc<SystemSnapshot>,
    next_seq: Seq,
    worker: Option<TaskHandle<()>>,
}

impl SystemController {
    /// Spawn the worker. Call once at start-up; the first snapshot arrives
    /// with the next UI signal.
    pub fn start(spawner: &ThreadSpawner) -> Result<Self, StartError> {
        let wake = Arc::new(new_eventfd().map_err(StartError::Wake)?);
        let (commands, command_rx) = sync_channel(COMMAND_QUEUE);
        let queue = NonZeroUsize::new(SNAPSHOT_QUEUE).unwrap_or(NonZeroUsize::MIN);
        let (publish, snapshots) = to_ui_bounded(queue);
        let slots = Arc::new(Slots::new());
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker::new(stop.clone(), wake.clone(), command_rx, slots.clone(), publish);
        let handle = spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("wm-system".into()),
                    ..Default::default()
                },
                move || worker.run(),
            )
            .map_err(StartError::Spawn)?;
        Ok(Self {
            commands,
            slots,
            stop,
            wake,
            snapshots,
            latest: Arc::new(SystemSnapshot::default()),
            next_seq: 1,
            worker: Some(handle),
        })
    }

    /// The newest snapshot seen by [`Self::poll`].
    pub fn snapshot(&self) -> &Arc<SystemSnapshot> {
        &self.latest
    }

    /// Take the newest published snapshot, if any arrived. Call on
    /// `Event::Signal` and on the status tick.
    pub fn poll(&mut self) -> bool {
        match self.snapshots.try_recv_flush() {
            Ok(snapshot) => {
                self.latest = snapshot;
                true
            }
            Err(_) => false,
        }
    }

    /// Default output volume, 0..=100. Coalesced: only the latest value of
    /// a drag reaches the sound server.
    pub fn set_output_volume(&mut self, percent: u32) -> Seq {
        self.store_slot(|s| &s.output_volume, percent)
    }

    /// Default input volume, 0..=100. Coalesced like the output.
    pub fn set_input_volume(&mut self, percent: u32) -> Seq {
        self.store_slot(|s| &s.input_volume, percent)
    }

    /// Primary backlight, 0..=100 of its `max_brightness`. Coalesced. Note
    /// that 0 switches many panels fully off; a UI floor is the panel's call.
    pub fn set_brightness(&mut self, percent: u32) -> Seq {
        self.store_slot(|s| &s.brightness, percent)
    }

    /// Queue a discrete command. `Err(command)` means the bounded queue is
    /// full (or the worker is gone — see [`Self::worker_alive`]); keep the
    /// command and try again on a later frame.
    pub fn send(&mut self, command: SystemCommand) -> Result<Seq, SystemCommand> {
        let seq = self.next_seq;
        match self.commands.try_send(Envelope { seq, command }) {
            Ok(()) => {
                self.advance_seq();
                self.wake();
                Ok(seq)
            }
            Err(TrySendError::Full(envelope)) | Err(TrySendError::Disconnected(envelope)) => {
                Err(envelope.command)
            }
        }
    }

    /// True until a snapshot whose `applied_seq` covers `seq` has arrived.
    pub fn is_pending(&self, seq: Seq) -> bool {
        seq_after(seq, self.latest.applied_seq)
    }

    pub fn worker_alive(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| !w.is_finished())
    }

    /// Ask the worker to exit and let go of it. Returns at once; the worker
    /// finishes the command it is on (bounded), kills its `pactl subscribe`
    /// child and ends. Also runs on drop.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.wake();
        if let Some(worker) = self.worker.take() {
            worker.cancel();
            worker.detach();
        }
    }

    fn store_slot(&mut self, pick: fn(&Slots) -> &AtomicU64, percent: u32) -> Seq {
        let seq = self.next_seq;
        self.advance_seq();
        pick(&self.slots).store(pack_slot(seq, percent.min(100)), Ordering::Release);
        self.wake();
        seq
    }

    fn advance_seq(&mut self) {
        self.next_seq = self.next_seq.wrapping_add(1);
        if self.next_seq == 0 {
            self.next_seq = 1;
        }
    }

    /// One non-blocking write to the eventfd. It can only fail when the
    /// counter would overflow, which means the worker is already awake.
    fn wake(&self) {
        let _ = (&*self.wake).write_all(&1u64.to_ne_bytes());
    }
}

impl Drop for SystemController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn new_eventfd() -> std::io::Result<File> {
    // SAFETY: plain syscall; the returned descriptor is owned right here.
    let fd = unsafe { eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh, valid descriptor nobody else owns.
    Ok(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}

// ======================================================================
// Bounded process execution
// ======================================================================

#[derive(Debug)]
enum ExecError {
    NotInstalled(String),
    Spawn(String),
    Timeout(String),
    Failed {
        program: String,
        code: Option<i32>,
        stderr: String,
    },
    OutputTooLarge(String),
    Parse(&'static str),
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled(program) => write!(f, "{program} is not installed"),
            Self::Spawn(message) => write!(f, "{message}"),
            Self::Timeout(program) => write!(f, "{program} did not finish in time"),
            Self::Failed {
                program,
                code,
                stderr,
            } => {
                write!(f, "{program} failed")?;
                if let Some(code) = code {
                    write!(f, " (exit {code})")?;
                }
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
            Self::OutputTooLarge(program) => write!(f, "{program} output exceeded the read limit"),
            Self::Parse(message) => write!(f, "pactl json: {message}"),
        }
    }
}

/// `argv`-only spawn: no shell, stdin closed, `LC_ALL=C` so `pactl`'s
/// event lines are not translated, and `PR_SET_PDEATHSIG` so the child dies
/// with the worker thread that spawned it.
fn spawn_process(program: &str, args: &[&str], stdout: Stdio, stderr: Stdio) -> Result<Child, ExecError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    // SAFETY: runs in the forked child before exec; prctl is
    // async-signal-safe and touches no Rust state.
    unsafe {
        command.pre_exec(|| {
            prctl(PR_SET_PDEATHSIG, SIGTERM, 0, 0, 0);
            Ok(())
        });
    }
    command.spawn().map_err(|error| match error.kind() {
        ErrorKind::NotFound => ExecError::NotInstalled(program.to_string()),
        _ => ExecError::Spawn(format!("{program}: {error}")),
    })
}

/// Read one chunk after `poll` said the pipe is ready. `false` = closed.
fn pump_pipe<R: Read>(pipe: &mut Option<R>, into: &mut Vec<u8>) -> bool {
    let Some(reader) = pipe.as_mut() else {
        return false;
    };
    let mut chunk = [0u8; 4096];
    match reader.read(&mut chunk) {
        Ok(0) => false,
        Ok(n) => {
            into.extend_from_slice(&chunk[..n]);
            true
        }
        Err(error) => matches!(error.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock),
    }
}

fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Run to completion within `timeout` seconds and return stdout. Both pipes
/// are drained under `poll`, so a chatty child never deadlocks on a full
/// pipe and a hung one is killed at the deadline.
fn run_bounded(program: &str, args: &[&str], timeout: f64, cap: usize) -> Result<String, ExecError> {
    let mut child = spawn_process(program, args, Stdio::piped(), Stdio::piped())?;
    let deadline = now() + timeout;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut out = Vec::new();
    let mut err = Vec::new();
    while stdout.is_some() || stderr.is_some() {
        let remaining = deadline - now();
        if remaining <= 0.0 {
            reap(&mut child);
            return Err(ExecError::Timeout(program.to_string()));
        }
        let mut fds = [
            PollFd {
                fd: stdout.as_ref().map_or(-1, |s| s.as_raw_fd()),
                events: POLLIN,
                revents: 0,
            },
            PollFd {
                fd: stderr.as_ref().map_or(-1, |s| s.as_raw_fd()),
                events: POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: `fds` is a live array of two pollfd structs.
        let ready = unsafe { poll(fds.as_mut_ptr(), 2, (remaining * 1000.0).ceil() as c_int) };
        if ready < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(EINTR) {
                continue;
            }
            reap(&mut child);
            return Err(ExecError::Spawn(format!("{program}: poll failed")));
        }
        if fds[0].revents != 0 && !pump_pipe(&mut stdout, &mut out) {
            stdout = None;
        }
        if fds[1].revents != 0 && !pump_pipe(&mut stderr, &mut err) {
            stderr = None;
        }
        if out.len() > cap || err.len() > SMALL_OUTPUT_CAP {
            reap(&mut child);
            return Err(ExecError::OutputTooLarge(program.to_string()));
        }
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                reap(&mut child);
                return Err(ExecError::Spawn(format!("{program}: {error}")));
            }
        }
        if now() >= deadline {
            reap(&mut child);
            return Err(ExecError::Timeout(program.to_string()));
        }
        sleep(Duration::from_millis(5));
    };
    let stderr_text = String::from_utf8_lossy(&err).trim().to_string();
    if !status.success() {
        return Err(ExecError::Failed {
            program: program.to_string(),
            code: status.code(),
            stderr: stderr_text,
        });
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn pactl_json(args: &[&str]) -> Result<Value, ExecError> {
    let mut argv: Vec<&str> = vec!["-f", "json"];
    argv.extend_from_slice(args);
    let text = run_bounded("pactl", &argv, COMMAND_TIMEOUT, LIST_OUTPUT_CAP)?;
    parse_json(text.as_bytes()).map_err(ExecError::Parse)
}

fn pactl(args: &[&str]) -> CommandResult {
    match run_bounded("pactl", args, COMMAND_TIMEOUT, SMALL_OUTPUT_CAP) {
        Ok(_) => CommandResult::Applied,
        Err(error) => CommandResult::Failed(error.to_string()),
    }
}

// ======================================================================
// JSON helpers over `pactl -f json`
// ======================================================================

fn obj_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

fn obj_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key)?.as_u64()
}

fn obj_bool(value: &Value, key: &str) -> Option<bool> {
    value.get(key)?.as_bool()
}

fn prop<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get("properties")?.get(key)?.as_str()
}

/// The loudest channel, as a percentage of `PA_VOLUME_NORM`.
fn volume_percent(device: &Value) -> u32 {
    let mut max = 0u64;
    if let Some(Value::Obj(channels)) = device.get("volume") {
        for (_, channel) in channels {
            if let Some(value) = obj_u64(channel, "value") {
                max = max.max(value);
            }
        }
    }
    ((max * 100 + PA_VOLUME_NORM / 2) / PA_VOLUME_NORM).min(u32::MAX as u64) as u32
}

fn device_state(state: Option<&str>) -> DeviceState {
    match state {
        Some("RUNNING") => DeviceState::Running,
        Some("IDLE") => DeviceState::Idle,
        Some("SUSPENDED") => DeviceState::Suspended,
        _ => DeviceState::Unknown,
    }
}

/// The active port's label and whether its jack reports something plugged.
fn active_port(device: &Value) -> (Option<String>, bool) {
    let mut label = None;
    let mut plugged = true;
    if let (Some(active), Some(ports)) = (
        obj_str(device, "active_port"),
        device.get("ports").and_then(Value::as_arr),
    ) {
        for port in ports {
            if obj_str(port, "name") == Some(active) {
                label = obj_str(port, "description").map(str::to_string);
                plugged = obj_str(port, "availability") != Some("not available");
            }
        }
    }
    (label, plugged)
}

/// Sinks: hide the server's dummy. Sources: hide the dummy, every sink
/// monitor (three independent signals, so the exact `pactl` encoding of
/// `monitor_of_sink` does not matter) and loopback nodes.
fn is_hidden(device: &Value, name: &str, direction: AudioDirection) -> bool {
    if name == "auto_null" {
        return true;
    }
    if direction == AudioDirection::Output {
        return false;
    }
    let monitor = match device.get("monitor_of_sink") {
        Some(Value::Str(sink)) => !sink.is_empty() && sink != "n/a",
        Some(Value::Int(index)) => *index >= 0 && *index != 0xffff_ffff,
        _ => false,
    } || prop(device, "device.class") == Some("monitor")
        || name.ends_with(".monitor");
    let loopback = name.starts_with("loopback")
        || prop(device, "factory.name").is_some_and(|f| f.contains("loopback"))
        || prop(device, "node.name").is_some_and(|n| n.starts_with("loopback"));
    monitor || loopback
}

/// The listed devices, the hidden count, and the hidden devices' indices.
fn parse_devices(
    list: &Value,
    default: Option<&str>,
    direction: AudioDirection,
) -> (Vec<AudioDevice>, usize, Vec<u32>) {
    let mut devices = Vec::new();
    let mut hidden = 0;
    let mut hidden_indices = Vec::new();
    for item in list.as_arr().unwrap_or(&[]) {
        let (Some(name), Some(index)) = (obj_str(item, "name"), obj_u64(item, "index")) else {
            continue;
        };
        let index = index.min(u32::MAX as u64) as u32;
        if is_hidden(item, name, direction) {
            hidden += 1;
            hidden_indices.push(index);
            continue;
        }
        let raw_percent = volume_percent(item);
        let (port, plugged) = active_port(item);
        devices.push(AudioDevice {
            id: name.to_string(),
            index,
            label: obj_str(item, "description").unwrap_or(name).to_string(),
            nick: prop(item, "node.nick")
                .or_else(|| prop(item, "device.nick"))
                .map(str::to_string),
            state: device_state(obj_str(item, "state")),
            port,
            plugged,
            percent: raw_percent.min(100),
            raw_percent,
            muted: obj_bool(item, "mute").unwrap_or(false),
            is_default: Some(name) == default,
        });
    }
    (devices, hidden, hidden_indices)
}

/// Three `pactl` reads: the server info (defaults), the sinks, the sources.
fn read_audio(live_events: bool) -> Result<(AudioState, Vec<u32>), ExecError> {
    let info = pactl_json(&["info"])?;
    let default_sink = obj_str(&info, "default_sink_name").map(str::to_string);
    let default_source = obj_str(&info, "default_source_name").map(str::to_string);
    let sinks = pactl_json(&["list", "sinks"])?;
    let sources = pactl_json(&["list", "sources"])?;
    let (outputs, _, _) = parse_devices(&sinks, default_sink.as_deref(), AudioDirection::Output);
    let (inputs, hidden_inputs, hidden_indices) =
        parse_devices(&sources, default_source.as_deref(), AudioDirection::Input);
    let default_output = outputs.iter().find(|d| d.is_default).map(|d| d.id.clone());
    let default_input = inputs.iter().find(|d| d.is_default).map(|d| d.id.clone());
    Ok((
        AudioState {
            availability: Availability::Ready,
            outputs,
            inputs,
            hidden_inputs,
            default_output,
            default_input,
            live_events,
        },
        hidden_indices,
    ))
}

// ======================================================================
// sysfs: backlight and power supplies
// ======================================================================

fn read_trim(path: &Path) -> Option<String> {
    let mut text = String::new();
    File::open(path).ok()?.take(4096).read_to_string(&mut text).ok()?;
    Some(text.trim().to_string())
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trim(path)?.parse().ok()
}

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "raw" => 0,
        "firmware" => 1,
        "platform" => 2,
        _ => 3,
    }
}

fn read_backlights() -> Vec<Backlight> {
    let mut devices = Vec::new();
    let Ok(dir) = std::fs::read_dir(BACKLIGHT_ROOT) else {
        return devices;
    };
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let Some(max) = read_u64(&path.join("max_brightness")) else {
            continue;
        };
        if max == 0 || max > u32::MAX as u64 {
            continue;
        }
        let raw = read_u64(&path.join("actual_brightness"))
            .or_else(|| read_u64(&path.join("brightness")))
            .unwrap_or(0)
            .min(max);
        devices.push(Backlight {
            name,
            kind: read_trim(&path.join("type")).unwrap_or_default(),
            max: max as u32,
            raw: raw as u32,
            percent: ((raw * 100 + max / 2) / max) as u32,
        });
    }
    devices.sort_by(|a, b| {
        kind_rank(&a.kind)
            .cmp(&kind_rank(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    devices
}

fn read_brightness(access: &BrightnessAccess) -> BrightnessState {
    let devices = read_backlights();
    BrightnessState {
        capability: if devices.is_empty() {
            BrightnessCapability::NoBacklight
        } else {
            BrightnessCapability::Backlight
        },
        devices,
        access: access.clone(),
    }
}

fn battery_status(text: Option<&str>) -> BatteryStatus {
    match text {
        Some("Charging") => BatteryStatus::Charging,
        Some("Discharging") => BatteryStatus::Discharging,
        Some("Not charging") => BatteryStatus::NotCharging,
        Some("Full") => BatteryStatus::Full,
        _ => BatteryStatus::Unknown,
    }
}

fn level_pair(path: &Path, now_file: &str, full_file: &str) -> Option<(u64, u64)> {
    let now = read_u64(&path.join(now_file))?;
    let full = read_u64(&path.join(full_file))?;
    (full > 0).then_some((now, full))
}

fn ratio_percent(now: u64, full: u64) -> u32 {
    ((now.saturating_mul(100) + full / 2) / full).min(100) as u32
}

fn read_battery(path: &Path, name: String) -> BatteryInfo {
    let energy_uwh = level_pair(path, "energy_now", "energy_full");
    let charge_uah = level_pair(path, "charge_now", "charge_full");
    let percent = read_u64(&path.join("capacity"))
        .map(|c| c.min(100) as u32)
        .or_else(|| energy_uwh.map(|(n, f)| ratio_percent(n, f)))
        .or_else(|| charge_uah.map(|(n, f)| ratio_percent(n, f)));
    let seconds = |file: &str| read_u64(&path.join(file)).map(|s| s.min(u32::MAX as u64) as u32);
    BatteryInfo {
        name,
        model: read_trim(&path.join("model_name")).filter(|m| !m.is_empty()),
        percent,
        status: battery_status(read_trim(&path.join("status")).as_deref()),
        energy_uwh,
        charge_uah,
        seconds_to_empty: seconds("time_to_empty_now"),
        seconds_to_full: seconds("time_to_full_now"),
    }
}

/// Weighted by energy when every pack reports it, else by charge, else the
/// mean of what the packs say.
fn combined_percent(batteries: &[BatteryInfo]) -> Option<u32> {
    if batteries.is_empty() {
        return None;
    }
    for pick in [
        (|b: &BatteryInfo| b.energy_uwh) as fn(&BatteryInfo) -> Option<(u64, u64)>,
        |b: &BatteryInfo| b.charge_uah,
    ] {
        let sum = batteries.iter().try_fold((0u64, 0u64), |(now, full), b| {
            pick(b).map(|(n, f)| (now.saturating_add(n), full.saturating_add(f)))
        });
        if let Some((now, full)) = sum {
            if full > 0 {
                return Some(ratio_percent(now, full));
            }
        }
    }
    let known: Vec<u64> = batteries.iter().filter_map(|b| b.percent.map(u64::from)).collect();
    if known.is_empty() {
        None
    } else {
        Some((known.iter().sum::<u64>() / known.len() as u64) as u32)
    }
}

fn combined_status(batteries: &[BatteryInfo]) -> BatteryStatus {
    let any = |status: BatteryStatus| batteries.iter().any(|b| b.status == status);
    if any(BatteryStatus::Charging) {
        BatteryStatus::Charging
    } else if any(BatteryStatus::Discharging) {
        BatteryStatus::Discharging
    } else if any(BatteryStatus::NotCharging) {
        BatteryStatus::NotCharging
    } else if !batteries.is_empty() && batteries.iter().all(|b| b.status == BatteryStatus::Full) {
        BatteryStatus::Full
    } else {
        BatteryStatus::Unknown
    }
}

fn read_power() -> PowerState {
    let mut batteries = Vec::new();
    let mut mains_seen = false;
    let mut mains_online = false;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(POWER_ROOT)
        .map(|dir| dir.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // `scope=Device` is a peripheral (mouse, keyboard, headset): neither
        // a system battery nor, when it reports `online`, the machine's AC.
        let scope = read_trim(&path.join("scope")).unwrap_or_default();
        if scope.eq_ignore_ascii_case("Device") {
            continue;
        }
        let kind = read_trim(&path.join("type")).unwrap_or_default();
        if kind == "Battery" {
            // Absent packs report `present=0`.
            if read_u64(&path.join("present")).unwrap_or(1) == 0 {
                continue;
            }
            batteries.push(read_battery(&path, name));
        } else if !kind.is_empty() {
            // Mains, USB*, Wireless, UPS: anything with an `online` flag.
            if let Some(online) = read_u64(&path.join("online")) {
                mains_seen = true;
                mains_online |= online != 0;
            }
        }
    }
    let single = |pick: fn(&BatteryInfo) -> Option<u32>| match batteries.as_slice() {
        [only] => pick(only),
        _ => None,
    };
    PowerState {
        capability: if batteries.is_empty() {
            PowerCapability::NoBattery
        } else {
            PowerCapability::Battery
        },
        percent: combined_percent(&batteries),
        status: combined_status(&batteries),
        ac_online: mains_seen.then_some(mains_online),
        seconds_to_empty: single(|b| b.seconds_to_empty),
        seconds_to_full: single(|b| b.seconds_to_full),
        batteries,
    }
}

// ======================================================================
// sysfs: DRM cards (GPUs)
// ======================================================================

/// The DRM cards and their connected connectors: a handful of tiny sysfs
/// reads per card, so it rides the same cadence as the backlight. `pci` is
/// the device symlink's target directory (`0000:01:00.0`); a card with no
/// PCI device (a virtual one) keeps an empty `pci`.
fn read_gpus() -> Vec<GpuInfo> {
    let Ok(dir) = std::fs::read_dir(DRM_ROOT) else {
        return Vec::new();
    };
    let mut cards: Vec<GpuInfo> = Vec::new();
    let mut connectors: Vec<(String, String)> = Vec::new();
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("card") else {
            continue;
        };
        match rest.split_once('-') {
            None => {
                if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let device = entry.path().join("device");
                let leaf = |path: Option<PathBuf>| {
                    path.and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .unwrap_or_default()
                };
                let hex = |file: &str| {
                    read_trim(&device.join(file))
                        .map(|v| v.trim_start_matches("0x").to_ascii_lowercase())
                        .unwrap_or_default()
                };
                cards.push(GpuInfo {
                    card: name.clone(),
                    pci: leaf(std::fs::canonicalize(&device).ok()),
                    vendor: hex("vendor"),
                    device: hex("device"),
                    driver: leaf(std::fs::read_link(device.join("driver")).ok()),
                    connected: Vec::new(),
                });
            }
            Some((digits, connector)) => {
                if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                if read_trim(&entry.path().join("status")).as_deref() == Some("connected") {
                    connectors.push((format!("card{digits}"), connector.to_string()));
                }
            }
        }
    }
    for (card, connector) in connectors {
        if let Some(gpu) = cards.iter_mut().find(|g| g.card == card) {
            gpu.connected.push(connector);
        }
    }
    for gpu in &mut cards {
        gpu.connected.sort();
    }
    cards.sort_by(|a, b| a.card.cmp(&b.card));
    cards
}

// ======================================================================
// The display settings files
// ======================================================================

/// `$XDG_CONFIG_HOME/makepad/wm/<file>` when `XDG_CONFIG_HOME` is an
/// absolute path (the spec says a relative one is to be ignored), else
/// `$HOME/.config/makepad/wm/<file>`.
fn display_settings_path(file: &str) -> Result<PathBuf, String> {
    let absolute = |key: &str| std::env::var_os(key).map(PathBuf::from).filter(|p| p.is_absolute());
    let root = match absolute("XDG_CONFIG_HOME") {
        Some(dir) => dir,
        None => absolute("HOME")
            .ok_or_else(|| "neither XDG_CONFIG_HOME nor HOME is an absolute path".to_string())?
            .join(".config"),
    };
    let mut path = root;
    for part in DISPLAY_SETTINGS_DIR {
        path.push(part);
    }
    path.push(file);
    Ok(path)
}

/// The file's one line as a scale, only when it is a whole number of
/// hundredths inside the slider's range.
fn parse_display_scale(text: &str) -> Option<u32> {
    let value: u32 = text.trim().parse().ok()?;
    (DISPLAY_SCALE_MIN..=DISPLAY_SCALE_MAX).contains(&value).then_some(value)
}

/// A connector name as the renderer lists it: non-empty, bounded, one
/// line, no control characters, no surrounding whitespace. Data only — it
/// is handed to the renderer's selection API and compared with its
/// inventory, never executed.
pub fn validate_display_source(name: &str) -> Result<&str, String> {
    if name.is_empty() {
        return Err("display source name is empty".into());
    }
    if name.len() > DISPLAY_SOURCE_MAX_LEN {
        return Err(format!("display source name is longer than {DISPLAY_SOURCE_MAX_LEN} bytes"));
    }
    if name.chars().any(char::is_control) {
        return Err("display source name contains control characters".into());
    }
    if name.trim() != name {
        return Err("display source name has surrounding whitespace".into());
    }
    Ok(name)
}

/// The saved scale, or `None` for a missing, unreadable or invalid file.
fn read_display_scale() -> Option<u32> {
    let path = display_settings_path(DISPLAY_SCALE_FILE).ok()?;
    parse_display_scale(&read_trim(&path)?)
}

/// The saved "Optimize for" connector name, or `None` for a missing,
/// unreadable or invalid file.
fn read_display_source() -> Option<String> {
    let path = display_settings_path(DISPLAY_SOURCE_FILE).ok()?;
    let text = read_trim(&path)?;
    validate_display_source(&text).ok().map(str::to_string)
}

fn is_lower_hex(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// `dddd:bb:ss.f` as sysfs names a PCI function (a domain of 4 to 8 hex
/// digits, lowercase).
fn is_pci_address(s: &str) -> bool {
    let Some((domain, rest)) = s.split_once(':') else {
        return false;
    };
    let Some((bus, rest)) = rest.split_once(':') else {
        return false;
    };
    let Some((slot, function)) = rest.split_once('.') else {
        return false;
    };
    is_lower_hex(domain, 4, 8) && is_lower_hex(bus, 2, 2) && is_lower_hex(slot, 2, 2) && is_lower_hex(function, 1, 1)
}

/// `vvvv:dddd`, lowercase hex.
fn is_pci_ids(s: &str) -> bool {
    s.split_once(':').is_some_and(|(vendor, device)| is_lower_hex(vendor, 4, 4) && is_lower_hex(device, 4, 4))
}

/// `<pci-address> <vendor>:<device>` exactly as [`GpuInfo::identity`]
/// writes it: one space, both halves in sysfs's lowercase hex, nothing
/// else on the line. The session script accepts the same shape and no
/// other. Data for a sysfs comparison, never a command.
pub fn validate_gpu_choice(line: &str) -> Result<&str, String> {
    if line.is_empty() {
        return Err("GPU identity is empty".into());
    }
    if line.len() > GPU_CHOICE_MAX_LEN {
        return Err(format!("GPU identity is longer than {GPU_CHOICE_MAX_LEN} bytes"));
    }
    let mut parts = line.split(' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(pci), Some(ids), None) if is_pci_address(pci) && is_pci_ids(ids) => Ok(line),
        _ => Err("GPU identity is not `<pci-address> <vendor>:<device>`".into()),
    }
}

/// The saved next-start GPU identity, or `None` (Auto) for a missing,
/// unreadable or invalid file.
fn read_gpu_choice() -> Option<String> {
    let path = display_settings_path(DISPLAY_GPU_FILE).ok()?;
    let text = read_trim(&path)?;
    validate_gpu_choice(&text).ok().map(str::to_string)
}

fn pointer_speed_file(touchpad: bool) -> &'static str {
    if touchpad { TOUCHPAD_SPEED_FILE } else { MOUSE_SPEED_FILE }
}

/// The file's one line as a pointer speed, only when it is a whole number
/// of hundredths inside 25..=300. Invalid and out-of-range values are
/// ignored so a bad file cannot move the pointer.
fn parse_pointer_speed(text: &str) -> Option<u32> {
    let value: u32 = text.trim().parse().ok()?;
    (POINTER_SPEED_MIN..=POINTER_SPEED_MAX).contains(&value).then_some(value)
}

/// The saved mouse or touchpad speed, or `None` for a missing, unreadable
/// or invalid file.
fn read_pointer_speed(touchpad: bool) -> Option<u32> {
    let path = display_settings_path(pointer_speed_file(touchpad)).ok()?;
    parse_pointer_speed(&read_trim(&path)?)
}

/// Remove one settings file; one that is already gone is fine.
fn remove_display_setting(file: &str) -> Result<(), String> {
    let path = display_settings_path(file)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

/// Write one settings file atomically: a temporary file unique to this
/// process in the target's own directory (so the rename stays on one
/// filesystem), flushed, then renamed over the target. A failure at any
/// step removes the temporary and leaves whatever the target held.
fn write_display_setting(file: &str, line: &str) -> Result<PathBuf, String> {
    let path = display_settings_path(file)?;
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let temp = dir.join(format!(".{file}.{}.tmp", std::process::id()));
    let written = File::create(&temp).and_then(|mut handle| {
        handle.write_all(format!("{line}\n").as_bytes())?;
        handle.sync_all()
    });
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("write {}: {error}", temp.display()));
    }
    if let Err(error) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("replace {}: {error}", path.display()));
    }
    Ok(path)
}

// ======================================================================
// The worker
// ======================================================================

struct Subscription {
    child: Child,
    stdout: ChildStdout,
    line: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PipeEvent {
    Nothing,
    Readable,
    HungUp,
}

struct Worker {
    stop: Arc<AtomicBool>,
    wake: Arc<File>,
    commands: Receiver<Envelope>,
    slots: Arc<Slots>,
    publish: ToUISender<Arc<SystemSnapshot>>,
    pending_publish: Option<Arc<SystemSnapshot>>,
    snapshot: SystemSnapshot,
    outcomes: VecDeque<CommandOutcome>,
    subscription: Option<Subscription>,
    subscribe_retry_at: f64,
    subscribe_backoff: f64,
    audio_dirty_since: Option<f64>,
    last_audio_sample: f64,
    next_audio_poll: f64,
    next_sysfs_sample: f64,
    /// Indices of the sources the input filter hid (monitors, loopbacks), so
    /// a screen recorder on a monitor is not counted as a mic stream.
    hidden_source_indices: Vec<u32>,
    brightness_access: BrightnessAccess,
    /// Discrete commands were left in the queue by the per-pass cap: the
    /// next pass runs without sleeping.
    backlog: bool,
    serial: u64,
}

impl Worker {
    fn new(
        stop: Arc<AtomicBool>,
        wake: Arc<File>,
        commands: Receiver<Envelope>,
        slots: Arc<Slots>,
        publish: ToUISender<Arc<SystemSnapshot>>,
    ) -> Self {
        Self {
            stop,
            wake,
            commands,
            slots,
            publish,
            pending_publish: None,
            snapshot: SystemSnapshot::default(),
            outcomes: VecDeque::new(),
            subscription: None,
            subscribe_retry_at: 0.0,
            subscribe_backoff: SUBSCRIBE_BACKOFF_MIN,
            audio_dirty_since: None,
            last_audio_sample: 0.0,
            next_audio_poll: 0.0,
            next_sysfs_sample: 0.0,
            hidden_source_indices: Vec::new(),
            brightness_access: BrightnessAccess::Untested,
            backlog: false,
            serial: 0,
        }
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn run(mut self) {
        let start = now();
        // The saved display settings first, published on their own before
        // the sound server is asked anything: the desktop applies them as
        // soon as this first snapshot lands, not after `pactl` answers.
        self.snapshot.dpi_scale = read_display_scale();
        self.snapshot.display_source = read_display_source();
        self.snapshot.gpu_choice = read_gpu_choice();
        // Wrapper-applied saved choice sets MAKEPAD_WM_GPU_FROM_SAVED=1 and
        // is not an override; only a true external MAKEPAD_DRM_DEVICE is.
        self.snapshot.gpu_env = if std::env::var("MAKEPAD_WM_GPU_FROM_SAVED").ok().as_deref() == Some("1") {
            None
        } else {
            std::env::var("MAKEPAD_DRM_DEVICE").ok().filter(|v| !v.is_empty())
        };
        self.snapshot.pointer_speeds = [read_pointer_speed(false), read_pointer_speed(true)];
        self.snapshot.display_settings_loaded = true;
        self.snapshot.input_settings_loaded = true;
        self.publish(start);
        self.spawn_subscription(start);
        self.sample_audio(start);
        self.sample_sysfs(start);
        self.publish(start);
        while !self.stopping() {
            let timeout = if self.backlog { 0 } else { self.poll_timeout_ms(now()) };
            let event = self.wait(timeout);
            if self.stopping() {
                break;
            }
            let now = now();
            let mut changed = self.pump_subscription(event, now);
            // Sliders first (the user's latest intent), then a bounded
            // slice of the discrete queue. Nothing is applied once a
            // shutdown was asked for: queued work is dropped, not done
            // behind the UI's back.
            let (slots_changed, slots_touched) = self.run_slots(now);
            changed |= slots_changed;
            let (commands_changed, audio_touched) = self.run_commands(now);
            changed |= commands_changed;
            if self.stopping() {
                break;
            }
            if audio_touched || slots_touched || self.audio_due(now) {
                changed |= self.sample_audio(now);
            }
            if now >= self.next_sysfs_sample {
                changed |= self.sample_sysfs(now);
            }
            if self.subscription.is_none() && now >= self.subscribe_retry_at {
                changed |= self.spawn_subscription(now);
            }
            if changed {
                self.publish(now);
            } else {
                self.retry_publish();
            }
        }
        self.drop_subscription(now());
    }

    // ---- scheduling ---------------------------------------------------

    fn audio_due_at(&self) -> Option<f64> {
        self.audio_dirty_since
            .map(|since| (since + AUDIO_DEBOUNCE).max(self.last_audio_sample + AUDIO_MIN_INTERVAL))
    }

    fn audio_due(&self, now: f64) -> bool {
        if self.audio_due_at().is_some_and(|due| now >= due) {
            return true;
        }
        self.subscription.is_none() && now >= self.next_audio_poll
    }

    fn poll_timeout_ms(&self, now: f64) -> c_int {
        let mut deadline = self.next_sysfs_sample;
        if let Some(due) = self.audio_due_at() {
            deadline = deadline.min(due);
        }
        if self.subscription.is_none() {
            deadline = deadline.min(self.next_audio_poll).min(self.subscribe_retry_at);
        }
        if self.pending_publish.is_some() {
            deadline = deadline.min(now + 0.05);
        }
        ((deadline - now).max(0.0) * 1000.0).ceil().min(MAX_POLL_MS) as c_int
    }

    /// Sleep until the controller wakes us, the event stream has something,
    /// or the next deadline.
    fn wait(&mut self, timeout_ms: c_int) -> PipeEvent {
        let mut fds = [
            PollFd {
                fd: self.wake.as_raw_fd(),
                events: POLLIN,
                revents: 0,
            },
            PollFd {
                fd: self.subscription.as_ref().map_or(-1, |s| s.stdout.as_raw_fd()),
                events: POLLIN,
                revents: 0,
            },
        ];
        let count = if self.subscription.is_some() { 2 } else { 1 };
        // SAFETY: `fds` is a live array of `count` pollfd structs.
        let ready = unsafe { poll(fds.as_mut_ptr(), count, timeout_ms) };
        if ready < 0 {
            if std::io::Error::last_os_error().raw_os_error() != Some(EINTR) {
                // Not expected (EBADF/EINVAL); do not spin on it.
                sleep(Duration::from_millis(50));
            }
            return PipeEvent::Nothing;
        }
        if fds[0].revents & POLLIN != 0 {
            self.drain_wake();
        }
        let revents = fds[1].revents;
        if count == 2 && revents & POLLIN != 0 {
            PipeEvent::Readable
        } else if count == 2 && revents & (POLLHUP | POLLERR | POLLNVAL) != 0 {
            PipeEvent::HungUp
        } else {
            PipeEvent::Nothing
        }
    }

    fn drain_wake(&self) {
        let mut counter = [0u8; 8];
        while matches!((&*self.wake).read(&mut counter), Ok(n) if n > 0) {}
    }

    // ---- `pactl subscribe` ----------------------------------------------

    /// Start the event stream. Returns whether the snapshot's `live_events`
    /// flag changed.
    fn spawn_subscription(&mut self, now: f64) -> bool {
        match spawn_process("pactl", &["subscribe"], Stdio::piped(), Stdio::null()) {
            Ok(mut child) => match child.stdout.take() {
                Some(stdout) => {
                    self.subscription = Some(Subscription {
                        child,
                        stdout,
                        line: Vec::new(),
                    });
                    self.subscribe_backoff = SUBSCRIBE_BACKOFF_MIN;
                    // Events may have passed while we were not listening.
                    self.mark_audio_dirty(now);
                    let was_live = self.snapshot.audio.live_events;
                    self.snapshot.audio.live_events = true;
                    !was_live
                }
                None => {
                    reap(&mut child);
                    self.schedule_resubscribe(now)
                }
            },
            Err(_) => self.schedule_resubscribe(now),
        }
    }

    fn schedule_resubscribe(&mut self, now: f64) -> bool {
        self.subscribe_retry_at = now + self.subscribe_backoff;
        self.subscribe_backoff = (self.subscribe_backoff * 2.0).min(SUBSCRIBE_BACKOFF_MAX);
        self.next_audio_poll = self.next_audio_poll.max(now + AUDIO_POLL_INTERVAL);
        let was_live = self.snapshot.audio.live_events;
        self.snapshot.audio.live_events = false;
        was_live
    }

    fn drop_subscription(&mut self, now: f64) -> bool {
        if let Some(mut subscription) = self.subscription.take() {
            reap(&mut subscription.child);
        }
        self.schedule_resubscribe(now)
    }

    /// Read what the event stream has and note whether devices changed.
    fn pump_subscription(&mut self, event: PipeEvent, now: f64) -> bool {
        let Some(subscription) = self.subscription.as_mut() else {
            return false;
        };
        let alive = match event {
            PipeEvent::Nothing => return false,
            PipeEvent::HungUp => false,
            PipeEvent::Readable => {
                let mut chunk = [0u8; 4096];
                match subscription.stdout.read(&mut chunk) {
                    Ok(0) => false,
                    Ok(n) => {
                        subscription.line.extend_from_slice(&chunk[..n]);
                        let mut dirty = false;
                        while let Some(end) = subscription.line.iter().position(|&b| b == b'\n') {
                            let line: Vec<u8> = subscription.line.drain(..=end).collect();
                            dirty |= is_device_event(&String::from_utf8_lossy(&line));
                        }
                        if subscription.line.len() > SUBSCRIBE_LINE_CAP {
                            subscription.line.clear();
                        }
                        if dirty {
                            self.mark_audio_dirty(now);
                        }
                        true
                    }
                    Err(error) => {
                        matches!(error.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock)
                    }
                }
            }
        };
        if alive {
            false
        } else {
            self.drop_subscription(now)
        }
    }

    fn mark_audio_dirty(&mut self, now: f64) {
        if self.audio_dirty_since.is_none() {
            self.audio_dirty_since = Some(now);
        }
    }

    // ---- commands -------------------------------------------------------

    /// Apply up to `COMMANDS_PER_PASS` queued commands, stopping at once on
    /// shutdown. Returns (snapshot changed, audio touched).
    fn run_commands(&mut self, now: f64) -> (bool, bool) {
        let mut changed = false;
        let mut audio_touched = false;
        let mut applied = 0;
        self.backlog = false;
        while applied < COMMANDS_PER_PASS && !self.stopping() {
            let envelope = match self.commands.try_recv() {
                Ok(envelope) => envelope,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.stop.store(true, Ordering::Release);
                    break;
                }
            };
            let (kind, result, touched) = self.execute(envelope.command, now);
            audio_touched |= touched;
            self.record(envelope.seq, kind, result);
            changed = true;
            applied += 1;
        }
        self.backlog = applied == COMMANDS_PER_PASS && !self.stopping();
        (changed, audio_touched)
    }

    /// Apply the latest slider values, one process each, skipping the rest
    /// on shutdown. Returns (snapshot changed, audio touched).
    fn run_slots(&mut self, now: f64) -> (bool, bool) {
        let mut changed = false;
        let mut audio_touched = false;
        if self.stopping() {
            return (changed, audio_touched);
        }
        if let Some((seq, percent)) = take_slot(&self.slots.output_volume) {
            let result = self.set_level(AudioDirection::Output, percent, now);
            self.record(seq, CommandKind::OutputVolume, result);
            audio_touched = true;
            changed = true;
        }
        if self.stopping() {
            return (changed, audio_touched);
        }
        if let Some((seq, percent)) = take_slot(&self.slots.input_volume) {
            let result = self.set_level(AudioDirection::Input, percent, now);
            self.record(seq, CommandKind::InputVolume, result);
            audio_touched = true;
            changed = true;
        }
        if self.stopping() {
            return (changed, audio_touched);
        }
        if let Some((seq, percent)) = take_slot(&self.slots.brightness) {
            let result = self.set_brightness(percent, now);
            self.record(seq, CommandKind::Brightness, result);
            changed = true;
        }
        (changed, audio_touched)
    }

    fn execute(&mut self, command: SystemCommand, now: f64) -> (CommandKind, CommandResult, bool) {
        match command {
            SystemCommand::SetOutputMuted(muted) => (
                CommandKind::OutputMute,
                self.set_mute(AudioDirection::Output, Some(muted), now),
                true,
            ),
            SystemCommand::ToggleOutputMute => (
                CommandKind::OutputMute,
                self.set_mute(AudioDirection::Output, None, now),
                true,
            ),
            SystemCommand::SetInputMuted(muted) => (
                CommandKind::InputMute,
                self.set_mute(AudioDirection::Input, Some(muted), now),
                true,
            ),
            SystemCommand::ToggleInputMute => (
                CommandKind::InputMute,
                self.set_mute(AudioDirection::Input, None, now),
                true,
            ),
            SystemCommand::SelectOutput(id) => (
                CommandKind::SelectOutput,
                self.select(AudioDirection::Output, &id, now),
                false,
            ),
            SystemCommand::SelectInput(id) => (
                CommandKind::SelectInput,
                self.select(AudioDirection::Input, &id, now),
                false,
            ),
            SystemCommand::Refresh => {
                self.sample_audio(now);
                self.sample_sysfs(now);
                (CommandKind::Refresh, CommandResult::Applied, false)
            }
            SystemCommand::SaveDpiScale(value) => (CommandKind::DpiScale, self.save_display_scale(value), false),
            SystemCommand::SaveDisplaySource(name) => {
                (CommandKind::DisplaySource, self.save_display_source(&name), false)
            }
            SystemCommand::SaveGpuChoice(choice) => {
                (CommandKind::GpuChoice, self.save_gpu_choice(choice.as_deref()), false)
            }
            SystemCommand::SavePointerSpeed { touchpad, value } => {
                (CommandKind::PointerSpeed, self.save_pointer_speed(touchpad, value), false)
            }
        }
    }

    /// Persist the next-start GPU; `None` (Auto) removes the file. The
    /// snapshot records the choice only once the file reflects it.
    fn save_gpu_choice(&mut self, choice: Option<&str>) -> CommandResult {
        match choice {
            Some(identity) => {
                let identity = match validate_gpu_choice(identity) {
                    Ok(identity) => identity,
                    Err(message) => return CommandResult::Failed(message),
                };
                match write_display_setting(DISPLAY_GPU_FILE, identity) {
                    Ok(_) => {
                        self.snapshot.gpu_choice = Some(identity.to_string());
                        CommandResult::Applied
                    }
                    Err(message) => CommandResult::Failed(format!("GPU choice not saved: {message}")),
                }
            }
            None => match remove_display_setting(DISPLAY_GPU_FILE) {
                Ok(()) => {
                    self.snapshot.gpu_choice = None;
                    CommandResult::Applied
                }
                Err(message) => CommandResult::Failed(format!("GPU choice not cleared: {message}")),
            },
        }
    }

    /// Persist the display scale. The snapshot records the value only once
    /// it is on disk; a refused value or a failed write reports why and
    /// leaves the recorded value alone.
    fn save_display_scale(&mut self, value: u32) -> CommandResult {
        if !(DISPLAY_SCALE_MIN..=DISPLAY_SCALE_MAX).contains(&value) {
            return CommandResult::Failed(format!(
                "display scale {value} is outside {DISPLAY_SCALE_MIN}..={DISPLAY_SCALE_MAX}"
            ));
        }
        match write_display_setting(DISPLAY_SCALE_FILE, &value.to_string()) {
            Ok(_) => {
                self.snapshot.dpi_scale = Some(value);
                CommandResult::Applied
            }
            Err(message) => CommandResult::Failed(format!("display scale not saved: {message}")),
        }
    }

    /// Persist a pointer speed. The snapshot records the value only once
    /// it is on disk; a refused value or a failed write reports why and
    /// leaves the recorded value alone.
    fn save_pointer_speed(&mut self, touchpad: bool, value: u32) -> CommandResult {
        let label = if touchpad { "touchpad speed" } else { "mouse speed" };
        if !(POINTER_SPEED_MIN..=POINTER_SPEED_MAX).contains(&value) {
            return CommandResult::Failed(format!(
                "{label} {value} is outside {POINTER_SPEED_MIN}..={POINTER_SPEED_MAX}"
            ));
        }
        match write_display_setting(pointer_speed_file(touchpad), &value.to_string()) {
            Ok(_) => {
                self.snapshot.pointer_speeds[touchpad as usize] = Some(value);
                CommandResult::Applied
            }
            Err(message) => CommandResult::Failed(format!("{label} not saved: {message}")),
        }
    }

    /// Persist the "Optimize for" connector name, same rules as the scale.
    fn save_display_source(&mut self, name: &str) -> CommandResult {
        let name = match validate_display_source(name) {
            Ok(name) => name,
            Err(message) => return CommandResult::Failed(message),
        };
        match write_display_setting(DISPLAY_SOURCE_FILE, name) {
            Ok(_) => {
                self.snapshot.display_source = Some(name.to_string());
                CommandResult::Applied
            }
            Err(message) => CommandResult::Failed(format!("display source not saved: {message}")),
        }
    }

    fn record(&mut self, seq: Seq, kind: CommandKind, result: CommandResult) {
        if self.outcomes.len() >= OUTCOME_HISTORY {
            self.outcomes.pop_front();
        }
        self.outcomes.push_back(CommandOutcome { seq, kind, result });
        if seq_after(seq, self.snapshot.applied_seq) {
            self.snapshot.applied_seq = seq;
        }
    }

    fn default_id(&self, direction: AudioDirection) -> Option<String> {
        match direction {
            AudioDirection::Output => self.snapshot.audio.default_output.clone(),
            AudioDirection::Input => self.snapshot.audio.default_input.clone(),
        }
    }

    fn known_device(&self, direction: AudioDirection, id: &str) -> Option<u32> {
        let devices = match direction {
            AudioDirection::Output => &self.snapshot.audio.outputs,
            AudioDirection::Input => &self.snapshot.audio.inputs,
        };
        devices.iter().find(|d| d.id == id).map(|d| d.index)
    }

    /// The server's current default, asked for now rather than taken from
    /// the snapshot, so a default change a moment ago is honoured. It must
    /// be a listed device: the dummy sink or a monitor source is refused
    /// rather than steered.
    fn resolve_default(&mut self, direction: AudioDirection, now: f64) -> Result<String, CommandResult> {
        let (verb, what) = match direction {
            AudioDirection::Output => ("get-default-sink", "output"),
            AudioDirection::Input => ("get-default-source", "input"),
        };
        let name = run_bounded("pactl", &[verb], COMMAND_TIMEOUT, SMALL_OUTPUT_CAP)
            .map_err(|error| CommandResult::Failed(error.to_string()))?;
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(CommandResult::Failed(format!("no default {what}")));
        }
        if self.known_device(direction, &name).is_none() {
            self.sample_audio(now);
            if self.known_device(direction, &name).is_none() {
                return Err(CommandResult::Failed(format!(
                    "the default {what} is not a usable device: {name}"
                )));
            }
        }
        Ok(name)
    }

    /// Volume of the current default device, 0..=100.
    fn set_level(&mut self, direction: AudioDirection, percent: u32, now: f64) -> CommandResult {
        let target = match self.resolve_default(direction, now) {
            Ok(target) => target,
            Err(failed) => return failed,
        };
        let verb = match direction {
            AudioDirection::Output => "set-sink-volume",
            AudioDirection::Input => "set-source-volume",
        };
        let level = format!("{}%", percent.min(100));
        pactl(&[verb, &target, &level])
    }

    /// Mute of the current default device. `None` toggles server-side, so a
    /// stale snapshot cannot double-flip.
    fn set_mute(&mut self, direction: AudioDirection, muted: Option<bool>, now: f64) -> CommandResult {
        let target = match self.resolve_default(direction, now) {
            Ok(target) => target,
            Err(failed) => return failed,
        };
        let verb = match direction {
            AudioDirection::Output => "set-sink-mute",
            AudioDirection::Input => "set-source-mute",
        };
        let flag = match muted {
            Some(true) => "1",
            Some(false) => "0",
            None => "toggle",
        };
        pactl(&[verb, &target, flag])
    }

    /// Change the default device, then observe: is it the default now, and
    /// which streams followed. The session manager (WirePlumber, or
    /// PulseAudio's core) re-links streams that were not pinned to a
    /// device; pinned ones (a client that opened a sink by name) stay, and
    /// the counts say so.
    fn select(&mut self, direction: AudioDirection, id: &str, now: f64) -> CommandResult {
        let what = match direction {
            AudioDirection::Output => "output",
            AudioDirection::Input => "input",
        };
        if self.known_device(direction, id).is_none() {
            self.sample_audio(now);
            if self.known_device(direction, id).is_none() {
                return CommandResult::Failed(format!("no such {what}: {id}"));
            }
        }
        let verb = match direction {
            AudioDirection::Output => "set-default-sink",
            AudioDirection::Input => "set-default-source",
        };
        if let CommandResult::Failed(message) = pactl(&[verb, id]) {
            return CommandResult::Failed(message);
        }
        self.sample_audio(now);
        let observed = self.default_id(direction);
        if observed.as_deref() != Some(id) {
            return CommandResult::Failed(format!(
                "server kept {} as the default {what}",
                observed.as_deref().unwrap_or("nothing")
            ));
        }
        let Some(index) = self.known_device(direction, id) else {
            return CommandResult::Failed(format!("{what} {id} vanished after selection"));
        };
        let streams = match self.count_streams(direction, id, index) {
            Ok(first) if first.elsewhere > 0 => {
                sleep(STREAM_SETTLE);
                Some(self.count_streams(direction, id, index).unwrap_or(first))
            }
            Ok(first) => Some(first),
            Err(_) => None,
        };
        CommandResult::Selected { streams }
    }

    /// Pulse client observations. WirePlumber also moves native PipeWire
    /// clients following the default, but these counts cover Pulse only.
    fn count_streams(
        &self,
        direction: AudioDirection,
        _default_id: &str,
        default_index: u32,
    ) -> Result<StreamFollow, ExecError> {
        self.count_streams_pulse(direction, default_index)
    }

    fn count_streams_pulse(
        &self,
        direction: AudioDirection,
        default_index: u32,
    ) -> Result<StreamFollow, ExecError> {
        let (what, key) = match direction {
            AudioDirection::Output => ("sink-inputs", "sink"),
            AudioDirection::Input => ("source-outputs", "source"),
        };
        let list = pactl_json(&["list", what])?;
        let mut follow = StreamFollow {
            following: 0,
            elsewhere: 0,
            scope: StreamScope::PulseClients,
        };
        for stream in list.as_arr().unwrap_or(&[]) {
            let Some(index) = obj_u64(stream, key) else {
                continue;
            };
            let index = index.min(u32::MAX as u64) as u32;
            if direction == AudioDirection::Input && self.hidden_source_indices.contains(&index) {
                continue;
            }
            if index == default_index {
                follow.following += 1;
            } else {
                follow.elsewhere += 1;
            }
        }
        Ok(follow)
    }

    /// Write the primary backlight: sysfs first, logind when sysfs denies.
    /// Success is the device reading back what was written.
    fn set_brightness(&mut self, percent: u32, now: f64) -> CommandResult {
        let Some(device) = self.snapshot.brightness.devices.first().cloned() else {
            return CommandResult::Failed("no backlight device".into());
        };
        let raw = ((percent.min(100) as u64 * device.max as u64 + 50) / 100) as u32;
        let node = Path::new(BACKLIGHT_ROOT).join(&device.name).join("brightness");
        let access = match std::fs::write(&node, raw.to_string()) {
            Ok(()) => BrightnessAccess::Sysfs,
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {
                let value = raw.to_string();
                match run_bounded(
                    "busctl",
                    &[
                        "--timeout=2",
                        "call",
                        "org.freedesktop.login1",
                        "/org/freedesktop/login1/session/auto",
                        "org.freedesktop.login1.Session",
                        "SetBrightness",
                        "ssu",
                        "backlight",
                        &device.name,
                        &value,
                    ],
                    LOGIND_TIMEOUT,
                    SMALL_OUTPUT_CAP,
                ) {
                    Ok(_) => BrightnessAccess::Logind,
                    Err(logind) => BrightnessAccess::Denied(format!(
                        "sysfs: {error}; logind: {logind}"
                    )),
                }
            }
            Err(error) => BrightnessAccess::Denied(format!("sysfs write failed: {error}")),
        };
        self.brightness_access = access.clone();
        if let BrightnessAccess::Denied(message) = access {
            self.sample_sysfs(now);
            return CommandResult::Failed(message);
        }
        // Success is the driver holding the requested value (`brightness`).
        // The physical reading (`actual_brightness`) can lag or quantise;
        // give it a short bounded settle so the snapshot that follows shows
        // the level the user will see, but do not fail on it.
        let actual_node = Path::new(BACKLIGHT_ROOT).join(&device.name).join("actual_brightness");
        for _ in 0..BRIGHTNESS_SETTLE_READS {
            if read_u64(&actual_node) == Some(raw as u64) {
                break;
            }
            sleep(BRIGHTNESS_SETTLE);
        }
        let requested = read_u64(&node);
        self.sample_sysfs(now);
        match requested {
            Some(held) if held == raw as u64 => CommandResult::Applied,
            Some(held) => CommandResult::Failed(format!(
                "asked {} for {raw}, driver holds {held}",
                device.name
            )),
            None => CommandResult::Failed(format!("{} vanished after the write", device.name)),
        }
    }

    // ---- sampling -------------------------------------------------------

    fn sample_audio(&mut self, now: f64) -> bool {
        self.audio_dirty_since = None;
        self.last_audio_sample = now;
        self.next_audio_poll = now + AUDIO_POLL_INTERVAL;
        let live = self.subscription.is_some();
        let audio = match read_audio(live) {
            Ok((audio, hidden)) => {
                self.hidden_source_indices = hidden;
                audio
            }
            Err(error) => AudioState::unavailable(error.to_string(), live),
        };
        if audio == self.snapshot.audio {
            false
        } else {
            self.snapshot.audio = audio;
            true
        }
    }

    fn sample_sysfs(&mut self, now: f64) -> bool {
        self.next_sysfs_sample = now + SYSFS_INTERVAL;
        let brightness = read_brightness(&self.brightness_access);
        let power = read_power();
        let mut changed = false;
        if brightness != self.snapshot.brightness {
            self.snapshot.brightness = brightness;
            changed = true;
        }
        if power != self.snapshot.power {
            self.snapshot.power = power;
            changed = true;
        }
        let gpus = read_gpus();
        if gpus != self.snapshot.gpus {
            self.snapshot.gpus = gpus;
            changed = true;
        }
        changed
    }

    // ---- publishing -----------------------------------------------------

    fn publish(&mut self, now: f64) {
        self.serial += 1;
        self.snapshot.serial = self.serial;
        self.snapshot.sampled_at = now;
        self.snapshot.outcomes = self.outcomes.iter().cloned().collect();
        self.pending_publish = None;
        let snapshot = Arc::new(self.snapshot.clone());
        self.offer(snapshot);
    }

    fn offer(&mut self, snapshot: Arc<SystemSnapshot>) {
        match self.publish.try_send(snapshot) {
            Ok(()) => {}
            Err(TrySendError::Full(snapshot)) => self.pending_publish = Some(snapshot),
            Err(TrySendError::Disconnected(_)) => self.stop.store(true, Ordering::Release),
        }
    }

    fn retry_publish(&mut self) {
        if let Some(snapshot) = self.pending_publish.take() {
            self.offer(snapshot);
        }
    }
}

/// `Event 'change' on sink #548` — the facilities that change what the
/// panels show. Stream and client events are ignored; they fire on every
/// play/pause and never change a device list or a device's volume.
fn is_device_event(line: &str) -> bool {
    let Some(rest) = line.split(" on ").nth(1) else {
        return false;
    };
    let facility = rest.trim_start().split([' ', '#']).next().unwrap_or("");
    matches!(facility, "sink" | "source" | "server" | "card")
}
