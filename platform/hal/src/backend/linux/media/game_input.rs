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
                        states: pads.iter().map(|p| GameState::Gamepad(p.published.clone())).collect(),
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
    axes: [Option<AbsInfo>; 64], // ABS_CNT, the evdev ABI (not a device limit).
    state: GamepadState,
    published: GamepadState,
    dropped: bool,
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
        let mut pad = Self {
            path, file,
            info: GameDevice { id, name, announce: true, haptics: Default::default(),
                vendor: u16::from_ne_bytes([ids[2], ids[3]]) as u32,
                product: u16::from_ne_bytes([ids[4], ids[5]]) as u32, output: game_output::NONE },
            axes, state: Default::default(), published: Default::default(), dropped: false,
        };
        pad.resync()?;
        pad.published = pad.state.clone();
        Ok(Some(pad))
    }
    fn key(&mut self, code: u16, value: i32) {
        let value = if value == 0 {0.0} else {1.0};
        let slot = match code {
            0x130 => &mut self.state.a, 0x131 => &mut self.state.b,
            0x133 => &mut self.state.x, 0x134 => &mut self.state.y,
            0x136 => &mut self.state.left_shoulder, 0x137 => &mut self.state.right_shoulder,
            0x138 => &mut self.state.left_trigger, 0x139 => &mut self.state.right_trigger,
            0x13a => &mut self.state.select, 0x13b => &mut self.state.start,
            0x13c => &mut self.state.home, 0x13d => &mut self.state.left_thumb,
            0x13e => &mut self.state.right_thumb,
            0x220 => &mut self.state.dpad_up, 0x221 => &mut self.state.dpad_down,
            0x222 => &mut self.state.dpad_left, 0x223 => &mut self.state.dpad_right,
            _ => return,
        };
        *slot = value;
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
    fn resync(&mut self) -> io::Result<()> {
        let mut keys = [0; 96];
        if !read_ioctl(&self.file, 0x18, &mut keys) { return Err(io::Error::last_os_error()); }
        self.state = Default::default();
        for code in 0..768 { self.key(code, bit(&keys, code as usize) as i32); }
        for code in 0..self.axes.len() {
            if self.axes[code].is_some() {
                let a = abs(&self.file, code as u16).ok_or_else(io::Error::last_os_error)?;
                self.axes[code] = Some(a);
                self.axis(code as u16, a.value);
            }
        }
        Ok(())
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
            for event in &events[..count] {
                if event.kind == 0 && event.code == 3 {
                    self.dropped = true;
                    eprintln!("[hal-gamepad] {} overran; resynchronizing at the next report", self.info.name);
                } else if event.kind == 0 && event.code == 0 {
                    if self.dropped { self.resync()?; self.dropped = false; }
                    if self.state != self.published { self.published = self.state.clone(); changed = true; }
                } else if !self.dropped {
                    match event.kind { 1 => self.key(event.code, event.value), 3 => self.axis(event.code, event.value), _ => {} }
                }
            }
        }
    }
}
