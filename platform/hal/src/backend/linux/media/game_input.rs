//! Linux gamepads through the kernel's evdev API. Each app owns its reader;
//! focus policy belongs to the app, including when hosted by a WM.
//! Device I/O stays on one sleeping worker. The HAL consumes a latest-state
//! snapshot, never a queue of old stick reports.
use super::{libc_sys, lock, Signal};
use crate::media::{game::{GameDevice, GameState, GamepadState}, game_output};
use crate::backend::linux::io::{self as poll_io, PollFd, READ};
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{self, Read},
    os::{fd::AsRawFd, raw::c_long, unix::{fs::OpenOptionsExt, net::UnixStream}},
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Snapshot {
    pub devices: Vec<GameDevice>,
    pub states: Vec<GameState>,
}

pub(super) struct GameInput {
    latest: Arc<Mutex<Option<Arc<Snapshot>>>>,
    stop: Arc<AtomicBool>,
    wake: UnixStream,
}
impl GameInput {
    pub fn new(signal: Signal) -> io::Result<Self> {
        let (read, wake) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        let latest = Arc::new(Mutex::new(None));
        let output = latest.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new().name("hal-linux-gamepad".into()).spawn(move || {
            let mut pads: Vec<Pad> = Vec::new();
            let mut denied = HashSet::new();
            let mut serial = 0;
            let mut scan = Instant::now();
            while !stopped.load(Ordering::Acquire) {
                let mut changed = false;
                if Instant::now() >= scan {
                    scan = Instant::now() + Duration::from_secs(1);
                    if let Ok(entries) = std::fs::read_dir("/dev/input") {
                        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| {
                            p.file_name().and_then(|s| s.to_str()).and_then(|s| s.strip_prefix("event"))
                                .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
                        }).collect();
                        paths.sort();
                        denied.retain(|p| paths.contains(p));
                        for path in paths {
                            if pads.iter().any(|p| p.path == path) { continue; }
                            match Pad::open(path.clone(), serial + 1) {
                                Ok(Some(pad)) => {
                                    serial += 1;
                                    eprintln!("[hal-gamepad] connected {} ({:04x}:{:04x}) at {}", pad.info.name, pad.info.vendor, pad.info.product, path.display());
                                    pads.push(pad);
                                    changed = true;
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    if denied.insert(path.clone()) {
                                        eprintln!("[hal-gamepad] open {}: {e}", path.display());
                                    }
                                }
                            }
                        }
                    }
                }
                pads.retain_mut(|pad| match pad.drain() {
                    Ok(dirty) => { changed |= dirty; true }
                    Err(e) => {
                        eprintln!("[hal-gamepad] disconnected {}: {e}", pad.info.name);
                        changed = true;
                        false
                    }
                });
                if changed {
                    let snapshot = Arc::new(Snapshot {
                        devices: pads.iter().map(|p| p.info.clone()).collect(),
                        states: pads.iter().map(|p| GameState::Gamepad(p.decoder.published.clone())).collect(),
                    });
                    // Only the pointer swap is shared, not allocation, scanning or I/O.
                    let old = lock(&output).replace(snapshot);
                    drop(old);
                    signal.set();
                }
                let mut fds: Vec<_> = pads.iter().map(|p| PollFd {fd: p.file.as_raw_fd(), events: READ, revents: 0}).collect();
                if let Err(e) = poll_io::wait(read.as_raw_fd(), &mut fds, Some(scan)) {
                    eprintln!("[hal-gamepad] poll: {e}");
                    break;
                }
            }
        })?;
        Ok(Self {latest, stop, wake})
    }
    pub fn take(&self) -> Option<Arc<Snapshot>> { lock(&self.latest).take() }
}
impl Drop for GameInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // HUP wakes poll without a blocking send or a UI-thread join.
        let _ = self.wake.shutdown(std::net::Shutdown::Both);
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct InputEvent { seconds: c_long, micros: c_long, kind: u16, code: u16, value: i32 }
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct AbsInfo { value: i32, minimum: i32, maximum: i32, fuzz: i32, flat: i32, resolution: i32 }

fn read_ioctl(file: &File, nr: u32, bytes: &mut [u8]) -> bool {
    let request = 0x8000_0000u32 | ((bytes.len() as u32) << 16) | (b'E' as u32) << 8 | nr;
    unsafe { libc_sys::ioctl(file.as_raw_fd(), request as std::os::raw::c_ulong, bytes.as_mut_ptr()) >= 0 }
}
fn bit(bytes: &[u8], code: usize) -> bool { bytes.get(code / 8).is_some_and(|b| b & (1 << (code % 8)) != 0) }
fn abs(file: &File, code: u16) -> Option<AbsInfo> {
    let mut bytes = [0; std::mem::size_of::<AbsInfo>()];
    read_ioctl(file, 0x40 + code as u32, &mut bytes).then(|| unsafe {std::ptr::read_unaligned(bytes.as_ptr().cast())})
}

struct Pad {
    path: PathBuf,
    file: File,
    info: GameDevice,
    decoder: Decoder,
}
impl Pad {
    fn open(path: PathBuf, id: u64) -> io::Result<Option<Self>> {
        let file = OpenOptions::new().read(true).custom_flags(libc_sys::O_NONBLOCK).open(&path)?;
        let mut keys = [0; 96]; // KEY_CNT / 8
        if !read_ioctl(&file, 0x21, &mut keys) || !bit(&keys, 0x130) { return Ok(None); }
        let mut name = [0; 256];
        read_ioctl(&file, 0x06, &mut name);
        let name = String::from_utf8_lossy(&name[..name.iter().position(|b| *b == 0).unwrap_or(name.len())]).into_owned();
        let mut ids = [0; 8];
        read_ioctl(&file, 0x02, &mut ids);
        let mut supported = [0; 8];
        read_ioctl(&file, 0x23, &mut supported);
        let axes = std::array::from_fn(|code| if bit(&supported, code) { abs(&file, code as u16) } else { None });
        let mut decoder = Decoder::new(axes);
        // A button already held when the pad (re)connects is not a press.
        decoder.resync(&mut &file)?;
        decoder.published = decoder.state.clone();
        Ok(Some(Self {
            path, file, decoder,
            info: GameDevice { id, name, announce: true, haptics: Default::default(),
                vendor: u16::from_ne_bytes([ids[2], ids[3]]) as u32,
                product: u16::from_ne_bytes([ids[4], ids[5]]) as u32, output: game_output::NONE },
        }))
    }
    fn drain(&mut self) -> io::Result<bool> {
        let mut changed = false;
        let mut events = [InputEvent::default(); 64];
        loop {
            let bytes = unsafe {std::slice::from_raw_parts_mut(events.as_mut_ptr().cast::<u8>(), std::mem::size_of_val(&events))};
            let count = match self.file.read(bytes) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => n / std::mem::size_of::<InputEvent>(),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(changed),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            let fed = self.decoder.feed(&events[..count], &mut &self.file)?;
            if fed.overran {
                eprintln!("[hal-gamepad] {} overran; resynchronizing at the next report", self.info.name);
            }
            changed |= fed.changed;
        }
    }
}

/// The kernel's current state of a device, which a resync reads.
trait Kernel {
    /// EVIOCGKEY: the keys down now, one bit per code (KEY_CNT / 8 bytes).
    fn keys(&mut self) -> io::Result<[u8; 96]>;
    /// EVIOCGABS: an axis now.
    fn abs(&mut self, code: u16) -> io::Result<AbsInfo>;
}
impl Kernel for &File {
    fn keys(&mut self) -> io::Result<[u8; 96]> {
        let mut keys = [0; 96];
        if !read_ioctl(*self, 0x18, &mut keys) { return Err(io::Error::last_os_error()); }
        Ok(keys)
    }
    fn abs(&mut self, code: u16) -> io::Result<AbsInfo> { abs(*self, code).ok_or_else(io::Error::last_os_error) }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Fed { changed: bool, overran: bool }

fn set_bit(bytes: &mut [u8; 96], code: u16, on: bool) {
    let (byte, mask) = (code as usize / 8, 1u8 << (code % 8));
    if let Some(b) = bytes.get_mut(byte) { if on { *b |= mask } else { *b &= !mask } }
}

/// One pad's evdev stream folded into its state, a report (SYN_REPORT) at a
/// time. Only presses the stream delivered are presses: a button found down
/// by a state query (at open, or resynchronizing after SYN_DROPPED) whose
/// press was never seen reads as up until it is released. A resync takes
/// axes and releases as they are, so nothing sticks, but it never invents a
/// press edge: an overrun cannot open a menu or fire a weapon.
struct Decoder {
    axes: [Option<AbsInfo>; 64], // ABS_CNT, the evdev ABI (not a device limit).
    /// Keys down as the kernel last told us.
    down: [u8; 96],
    /// Keys down whose press the stream never delivered.
    unseen: [u8; 96],
    state: GamepadState,
    published: GamepadState,
    dropped: bool,
}
impl Decoder {
    fn new(axes: [Option<AbsInfo>; 64]) -> Self {
        Self { axes, down: [0; 96], unseen: [0; 96], state: Default::default(), published: Default::default(), dropped: false }
    }
    fn button(state: &mut GamepadState, code: u16) -> Option<&mut f32> {
        Some(match code {
            0x130 => &mut state.a, 0x131 => &mut state.b,
            0x133 => &mut state.x, 0x134 => &mut state.y,
            0x136 => &mut state.left_shoulder, 0x137 => &mut state.right_shoulder,
            0x138 => &mut state.left_trigger, 0x139 => &mut state.right_trigger,
            0x13a => &mut state.select, 0x13b => &mut state.start,
            0x13c => &mut state.home, 0x13d => &mut state.left_thumb,
            0x13e => &mut state.right_thumb,
            0x220 => &mut state.dpad_up, 0x221 => &mut state.dpad_down,
            0x222 => &mut state.dpad_left, 0x223 => &mut state.dpad_right,
            _ => return None,
        })
    }
    fn apply_key(&mut self, code: u16) {
        let held = bit(&self.down, code as usize) && !bit(&self.unseen, code as usize);
        if let Some(slot) = Self::button(&mut self.state, code) { *slot = held as u8 as f32; }
    }
    fn key(&mut self, code: u16, value: i32) {
        let down = value != 0;
        set_bit(&mut self.down, code, down);
        if !down { set_bit(&mut self.unseen, code, false); }
        self.apply_key(code);
    }
    fn axis(&mut self, code: u16, value: i32) {
        let Some(a) = self.axes.get(code as usize).copied().flatten() else { return; };
        if a.maximum <= a.minimum { return; }
        let unit = ((value as f64 - a.minimum as f64) / (a.maximum as f64 - a.minimum as f64)).clamp(0.0, 1.0) as f32;
        // Makepad's sticks use positive Y for up, like Apple and XInput.
        // evdev uses positive Y for down. Apps own dead zones and response curves.
        match code {
            0 => self.state.left_stick[0] = unit * 2.0 - 1.0,
            1 => self.state.left_stick[1] = 1.0 - unit * 2.0,
            3 => self.state.right_stick[0] = unit * 2.0 - 1.0,
            4 => self.state.right_stick[1] = 1.0 - unit * 2.0,
            2 => self.state.left_trigger = unit,
            5 => self.state.right_trigger = unit,
            16 => { self.state.dpad_left = (value < 0) as u8 as f32; self.state.dpad_right = (value > 0) as u8 as f32; }
            17 => { self.state.dpad_up = (value < 0) as u8 as f32; self.state.dpad_down = (value > 0) as u8 as f32; }
            _ => {}
        }
    }
    /// Take the kernel's state as it is now (open, or the report after SYN_DROPPED).
    fn resync(&mut self, kernel: &mut impl Kernel) -> io::Result<()> {
        let keys = kernel.keys()?;
        for code in 0..768u16 {
            let now = bit(&keys, code as usize);
            let seen = bit(&self.down, code as usize) && !bit(&self.unseen, code as usize);
            set_bit(&mut self.unseen, code, now && !seen);
            set_bit(&mut self.down, code, now);
        }
        self.state = Default::default();
        for code in 0..768 { self.apply_key(code); }
        for code in 0..self.axes.len() {
            if self.axes[code].is_some() {
                let a = kernel.abs(code as u16)?;
                self.axes[code] = Some(a);
                self.axis(code as u16, a.value);
            }
        }
        Ok(())
    }
    /// Fold a batch of events. Events between SYN_DROPPED and the next
    /// SYN_REPORT are incomplete and dropped; that report resynchronizes.
    fn feed(&mut self, events: &[InputEvent], kernel: &mut impl Kernel) -> io::Result<Fed> {
        let mut fed = Fed::default();
        for event in events {
            if event.kind == 0 && event.code == 3 {
                self.dropped = true;
                fed.overran = true;
            } else if event.kind == 0 && event.code == 0 {
                if self.dropped { self.resync(kernel)?; self.dropped = false; }
                if self.state != self.published {
                    // Counted per report, so a tap folded into one read (or one snapshot the
                    // HAL takes later) still arrives as a press.
                    self.state.count_presses_since(&self.published);
                    self.published = self.state.clone();
                    fed.changed = true;
                }
            } else if !self.dropped {
                match event.kind { 1 => self.key(event.code, event.value), 3 => self.axis(event.code, event.value), _ => {} }
            }
        }
        Ok(fed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BTN_A: u16 = 0x130;
    const BTN_START: u16 = 0x13b;
    const ABS_X: u16 = 0;

    /// What the kernel answers a state query with.
    struct Fake { keys: [u8; 96], x: i32 }
    impl Default for Fake {
        fn default() -> Self { Self { keys: [0; 96], x: 0 } }
    }
    impl Kernel for Fake {
        fn keys(&mut self) -> io::Result<[u8; 96]> { Ok(self.keys) }
        fn abs(&mut self, _code: u16) -> io::Result<AbsInfo> { Ok(AbsInfo { value: self.x, minimum: -32768, maximum: 32767, ..Default::default() }) }
    }
    impl Fake {
        fn hold(mut self, code: u16) -> Self { set_bit(&mut self.keys, code, true); self }
    }
    fn ev(kind: u16, code: u16, value: i32) -> InputEvent { InputEvent { kind, code, value, ..Default::default() } }
    fn key(code: u16, value: i32) -> InputEvent { ev(1, code, value) }
    fn syn() -> InputEvent { ev(0, 0, 0) }
    fn dropped() -> InputEvent { ev(0, 3, 0) }
    fn decoder() -> Decoder {
        let mut axes = [None; 64];
        axes[ABS_X as usize] = Some(AbsInfo { minimum: -32768, maximum: 32767, ..Default::default() });
        Decoder::new(axes)
    }

    #[test]
    fn a_press_inside_an_overrun_is_no_press_until_pressed_again() {
        let mut d = decoder();
        let mut k = Fake::default();
        d.feed(&[key(BTN_A, 1), syn()], &mut k).unwrap();
        assert_eq!(d.published.a, 1.0);
        // The ring overflowed while Start went down; the kernel kept the newest event.
        let mut k = Fake { x: 32767, ..Default::default() }.hold(BTN_A).hold(BTN_START);
        let fed = d.feed(&[dropped(), key(BTN_START, 1), syn()], &mut k).unwrap();
        assert!(fed.overran && fed.changed, "the stick moved");
        assert_eq!(d.published.start, 0.0, "a resync never invents a press edge");
        assert_eq!(d.published.a, 1.0, "a button held across the overrun stays held, no flicker");
        assert!(d.published.left_stick[0] > 0.99, "axes take the resync's values");
        // Start keeps reading up while held, then a real press counts.
        d.feed(&[ev(3, ABS_X, 0), syn()], &mut k).unwrap();
        assert_eq!(d.published.start, 0.0);
        d.feed(&[key(BTN_START, 0), syn(), key(BTN_START, 1), syn()], &mut k).unwrap();
        assert_eq!(d.published.start, 1.0);
    }

    #[test]
    fn a_release_inside_an_overrun_releases_at_the_resync() {
        let mut d = decoder();
        let mut k = Fake::default();
        d.feed(&[key(BTN_A, 1), syn()], &mut k).unwrap();
        // Released during the overrun: nothing may stick.
        let fed = d.feed(&[dropped(), key(BTN_A, 0), syn()], &mut Fake::default()).unwrap();
        assert!(fed.changed);
        assert_eq!(d.published.a, 0.0);
        assert!(!d.dropped);
    }

    #[test]
    fn events_after_the_resync_report_apply_normally() {
        let mut d = decoder();
        let mut k = Fake::default();
        // A whole batch as one read: overrun, its report, then two live reports.
        d.feed(&[dropped(), key(BTN_START, 1), syn(), key(BTN_START, 1), syn(), key(BTN_START, 0), syn()], &mut k).unwrap();
        assert_eq!(d.published.start, 0.0);
        d.feed(&[key(BTN_START, 1), syn()], &mut k).unwrap();
        assert_eq!(d.published.start, 1.0, "the next real press is a press");
    }

    #[test]
    fn a_tap_inside_one_read_is_counted() {
        use crate::media::game::gamepad_button as button;
        let mut d = decoder();
        let mut k = Fake::default();
        // A 5 ms tap: press and release reports in the same read, before anyone samples.
        let fed = d.feed(&[key(BTN_A, 1), syn(), key(BTN_A, 0), syn()], &mut k).unwrap();
        assert_eq!(d.published.a, 0.0, "the latest state is up again");
        assert!(fed.changed, "the press is news");
        assert_eq!(d.published.presses[button::A], 1);
        d.feed(&[key(BTN_A, 1), syn(), key(BTN_A, 0), syn(), key(BTN_A, 1), syn()], &mut k).unwrap();
        assert_eq!((d.published.a, d.published.presses[button::A]), (1.0, 3));
        // A resync's unseen press is no press, and keeps the count.
        d.feed(&[key(BTN_A, 0), syn(), dropped(), syn()], &mut Fake::default().hold(BTN_START)).unwrap();
        assert_eq!((d.published.presses[button::A], d.published.presses[button::START]), (3, 0));
    }

    #[test]
    fn a_button_held_at_connect_is_no_press() {
        let mut d = decoder();
        d.resync(&mut Fake::default().hold(BTN_START)).unwrap();
        d.published = d.state.clone();
        assert_eq!(d.published.start, 0.0, "a reconnect never produces a press");
        let mut k = Fake::default();
        d.feed(&[key(BTN_START, 2), syn()], &mut k).unwrap();
        assert_eq!(d.published.start, 0.0, "autorepeat of the unseen press is no press");
        d.feed(&[key(BTN_START, 0), syn(), key(BTN_START, 1), syn()], &mut k).unwrap();
        assert_eq!(d.published.start, 1.0);
    }
}
