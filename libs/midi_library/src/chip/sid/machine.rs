//! Just enough C64 to run a SID tune: 64K of RAM, the banking at $01, a
//! KERNAL that only enters and leaves interrupts, the VIC's raster and the
//! CIAs' timer A (the clocks players run on), and the SID chips, whose
//! writes are kept with the time they happen. A PSID's play routine is
//! called each frame (or each CIA period); a tune without one (RSID) runs
//! freely and is interrupted by the raster and timers it sets up.

use super::cpu::{Bus, Cpu};
use super::{Header, Tune};

/// A write to a SID register.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Write {
    /// Seconds from the start.
    pub t: f64,
    /// Which chip (0..3).
    pub chip: u8,
    /// 0..25.
    pub reg: u8,
    pub value: u8,
}

#[derive(Clone, Debug)]
pub struct Trace {
    pub writes: Vec<Write>,
    /// Calls of the play routine (or interrupts) a second.
    pub rate: f64,
    pub seconds: f64,
    pub chips: usize,
    pub clock: f64,
}

/// The KERNAL's interrupt entry and exit and the vectors: all the ROM a
/// tune needs (the rest reads as zero).
fn kernal() -> Vec<u8> {
    let mut rom = vec![0u8; 0x2000];
    let mut put = |addr: u16, code: &[u8]| {
        let at = (addr - 0xE000) as usize;
        rom[at..at + code.len()].copy_from_slice(code);
    };
    // $FF48: save the registers, through the vector at $0314.
    put(0xFF48, &[0x48, 0x8A, 0x48, 0x98, 0x48, 0x6C, 0x14, 0x03]);
    // $EA31 and $EA81: restore them and return.
    put(0xEA31, &[0x68, 0xA8, 0x68, 0xAA, 0x68, 0x40]);
    put(0xEA81, &[0x68, 0xA8, 0x68, 0xAA, 0x68, 0x40]);
    // NMI: through the vector at $0318 (by default to an RTI).
    put(0xFE43, &[0x6C, 0x18, 0x03]);
    put(0xFE47, &[0x40]);
    // Vectors: NMI, reset, IRQ.
    put(0xFFFA, &[0x43, 0xFE, 0x00, 0x00, 0x48, 0xFF]);
    rom
}

struct C64 {
    ram: Vec<u8>,
    rom: Vec<u8>,
    /// The CPU's cycles now (kept in step by the driver).
    now: u64,
    clock: f64,
    line_cycles: u64,
    lines: u64,
    sids: Vec<u16>,
    writes: Vec<Write>,
    rng: u32,
    // VIC: raster compare and interrupt latch/enable.
    raster_cmp: u16,
    vic_irq: u8,
    vic_mask: u8,
    // CIA 1 and 2 timer A: latch, the cycle it was (re)loaded at, running,
    // interrupt flags and mask.
    cia: [Cia; 2],
}

#[derive(Clone, Copy, Default)]
struct Cia {
    latch: u16,
    loaded_at: u64,
    running: bool,
    icr: u8,
    mask: u8,
    /// The latch was written by the tune.
    set: bool,
}

impl Cia {
    fn period(&self) -> u64 {
        self.latch.max(1) as u64 + 1
    }

    /// Timer A's count now.
    fn count(&self, now: u64) -> u16 {
        if !self.running {
            return self.latch;
        }
        let gone = (now - self.loaded_at.min(now)) % self.period();
        (self.period() - 1 - gone) as u16
    }
}

impl C64 {
    fn bank(&self) -> u8 {
        self.ram[1] & 7
    }

    fn io_visible(&self) -> bool {
        let b = self.bank();
        b & 3 != 0 && b & 4 != 0
    }

    fn kernal_visible(&self) -> bool {
        self.bank() & 2 != 0
    }

    fn sid_of(&self, addr: u16) -> Option<(u8, u8)> {
        for (k, &base) in self.sids.iter().enumerate().rev() {
            if k == 0 {
                if (0xD400..0xD800).contains(&addr) && (self.sids.len() == 1 || !self.sids[1..].iter().any(|&b| (b..b + 0x20).contains(&addr))) {
                    return Some((0, (addr & 0x1F) as u8));
                }
            } else if (base..base + 0x20).contains(&addr) {
                return Some((k as u8, (addr - base) as u8));
            }
        }
        None
    }

    fn raster(&self) -> u16 {
        ((self.now / self.line_cycles) % self.lines) as u16
    }
}

impl Bus for C64 {
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0xD000..=0xDFFF if self.io_visible() => {
                if let Some((_, reg)) = self.sid_of(addr) {
                    return match reg {
                        // The third oscillator and envelope: a player
                        // reading them for noise gets noise.
                        0x1B | 0x1C => {
                            self.rng ^= self.rng << 13;
                            self.rng ^= self.rng >> 17;
                            self.rng ^= self.rng << 5;
                            self.rng as u8
                        }
                        _ => 0,
                    };
                }
                match addr {
                    0xD011 => ((self.raster() >> 1) & 0x80) as u8 | 0x1B,
                    0xD012 => self.raster() as u8,
                    0xD019 => self.vic_irq | if self.vic_irq & self.vic_mask != 0 { 0x80 } else { 0 },
                    0xD01A => self.vic_mask | 0xF0,
                    0xDC00..=0xDDFF => {
                        let k = ((addr >> 8) & 1) as usize;
                        let now = self.now;
                        let c = &mut self.cia[k];
                        match addr & 0x0F {
                            4 => c.count(now) as u8,
                            5 => (c.count(now) >> 8) as u8,
                            0x0D => {
                                let v = c.icr | if c.icr & c.mask != 0 { 0x80 } else { 0 };
                                c.icr = 0;
                                v
                            }
                            0x0E => c.running as u8,
                            0 | 1 => 0xFF,
                            _ => 0,
                        }
                    }
                    _ => 0,
                }
            }
            0xE000..=0xFFFF if self.kernal_visible() => self.rom[(addr - 0xE000) as usize],
            _ => self.ram[addr as usize],
        }
    }

    fn write(&mut self, addr: u16, value: u8) {
        if (0xD000..=0xDFFF).contains(&addr) && self.io_visible() {
            if let Some((chip, reg)) = self.sid_of(addr) {
                if reg < 25 {
                    self.writes.push(Write { t: self.now as f64 / self.clock, chip, reg, value });
                }
                return;
            }
            match addr {
                0xD011 => self.raster_cmp = (self.raster_cmp & 0xFF) | ((value as u16 & 0x80) << 1),
                0xD012 => self.raster_cmp = (self.raster_cmp & 0x100) | value as u16,
                0xD019 => self.vic_irq &= !value,
                0xD01A => self.vic_mask = value & 0x0F,
                0xDC00..=0xDDFF => {
                    let k = ((addr >> 8) & 1) as usize;
                    let now = self.now;
                    let c = &mut self.cia[k];
                    match addr & 0x0F {
                        4 => {
                            c.latch = (c.latch & 0xFF00) | value as u16;
                            c.set = true;
                        }
                        5 => {
                            c.latch = (c.latch & 0x00FF) | (value as u16) << 8;
                            c.set = true;
                            if !c.running {
                                c.loaded_at = now;
                            }
                        }
                        0x0D => {
                            if value & 0x80 != 0 {
                                c.mask |= value & 0x1F;
                            } else {
                                c.mask &= !(value & 0x1F);
                            }
                        }
                        0x0E => {
                            if value & 0x10 != 0 || (value & 1 != 0 && !c.running) {
                                c.loaded_at = now;
                            }
                            c.running = value & 1 != 0;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            return;
        }
        self.ram[addr as usize] = value;
    }
}

/// The most instructions one call of a routine may run (a tune that
/// never returns is stopped).
const CALL_LIMIT: u64 = 2_000_000;

/// Call `addr` as a subroutine (A = `a`) until it returns.
fn call(cpu: &mut Cpu, m: &mut C64, addr: u16, a: u8) -> bool {
    let sp0 = cpu.sp;
    // Return to $0000 - 1: when RTS brings SP back, the call is over.
    cpu.push(m, 0xFF);
    cpu.push(m, 0xFF);
    cpu.pc = addr;
    cpu.a = a;
    let start = cpu.cycles;
    loop {
        m.now = cpu.cycles;
        let op = m.read(cpu.pc);
        cpu.step(m);
        if (op == 0x60 || op == 0x40) && cpu.sp == sp0 {
            return true;
        }
        if cpu.jammed || cpu.cycles - start > CALL_LIMIT * 4 {
            cpu.sp = sp0;
            cpu.jammed = false;
            return false;
        }
    }
}

/// Run `tune`'s `song` (1-based) for `seconds`.
pub fn run(tune: &Tune, song: u16, seconds: f64) -> Trace {
    let h: &Header = &tune.header;
    let (clock, line_cycles, lines) = if h.ntsc { (1_022_727.0, 65u64, 263u64) } else { (985_248.0, 63u64, 312u64) };
    let frame = line_cycles * lines;
    let mut ram = vec![0u8; 0x10000];
    let at = h.load as usize;
    let n = tune.data.len().min(0x10000 - at);
    ram[at..at + n].copy_from_slice(&tune.data[..n]);
    // The IRQ vector the KERNAL calls through, and the banking the tune
    // needs (ROMs out where it is loaded).
    ram[0x0314] = 0x31;
    ram[0x0315] = 0xEA;
    ram[0x0316] = 0x66;
    ram[0x0317] = 0xFE;
    ram[0x0318] = 0x47;
    ram[0x0319] = 0xFE;
    let end = at + n;
    ram[0] = 0x2F;
    ram[1] = if end > 0xE000 { 0x35 } else if end > 0xA000 { 0x36 } else { 0x37 };
    if h.rsid {
        ram[1] = 0x37;
    }
    let mut sids = vec![0xD400];
    if h.sid2 != 0 {
        sids.push(h.sid2);
    }
    if h.sid3 != 0 {
        sids.push(h.sid3);
    }
    let chips = sids.len();
    let mut m = C64 {
        ram,
        rom: kernal(),
        now: 0,
        clock,
        line_cycles,
        lines,
        sids,
        writes: Vec::new(),
        rng: 0x1234_5678,
        raster_cmp: 0x137,
        vic_irq: 0,
        vic_mask: 0,
        cia: [Cia { latch: 0x4025, ..Default::default() }, Cia::default()],
    };
    let mut cpu = Cpu::new(true);
    let song = song.clamp(1, h.songs);
    let cia_speed = h.speed >> ((song - 1).min(31)) & 1 != 0;
    let total = (seconds * clock) as u64;
    let rate;
    if h.play != 0 && !h.rsid {
        // PSID: init once, then play at the frame or CIA rate.
        call(&mut cpu, &mut m, h.init, (song - 1) as u8);
        let period = if cia_speed { m.cia[0].period() } else { frame };
        rate = clock / period as f64;
        let mut next = cpu.cycles.max(1);
        while next < total {
            cpu.cycles = next;
            m.now = next;
            // A tune may turn the ROMs off and on between calls.
            call(&mut cpu, &mut m, h.play, 0);
            // A CIA tune may change its speed.
            let period = if cia_speed && m.cia[0].set { m.cia[0].period() } else if cia_speed { 0x4025 + 1 } else { frame };
            next += period.max(1000);
        }
    } else {
        // RSID (or a PSID without a play routine): init sets up its own
        // interrupts and is interrupted from then on.
        m.cia[0].running = true;
        m.cia[0].mask = 1;
        let init = h.init;
        let sp0 = cpu.sp;
        cpu.push(&mut m, 0xFF);
        cpu.push(&mut m, 0xFF);
        cpu.pc = init;
        cpu.a = (song - 1) as u8;
        let mut idle = false;
        let mut irqs = 0u64;
        // The stack level an interrupt taken while idle returns to.
        let mut from_idle: Option<u8> = None;
        let mut nmi_was = false;
        let mut last_line = u64::MAX;
        while cpu.cycles < total {
            m.now = cpu.cycles;
            // The raster reaching the compare line latches the VIC's flag.
            let line_no = cpu.cycles / line_cycles;
            if line_no != last_line {
                last_line = line_no;
                if (line_no % lines) as u16 == m.raster_cmp {
                    m.vic_irq |= 1;
                }
            }
            // Timer A underflows.
            for c in m.cia.iter_mut() {
                if c.running && cpu.cycles >= c.loaded_at + c.period() {
                    let p = c.period();
                    c.loaded_at += (cpu.cycles - c.loaded_at) / p * p;
                    c.icr |= 1;
                }
            }
            // CIA 2 interrupts the CPU through the NMI (digis, timers): on
            // its edge, whatever the I flag says.
            let nmi_now = (m.cia[1].icr & m.cia[1].mask) != 0;
            if nmi_now && !nmi_was {
                if idle {
                    from_idle = Some(cpu.sp);
                }
                cpu.interrupt(&mut m, 0xFFFA);
                idle = false;
            }
            nmi_was = nmi_now;
            let irq = (m.vic_irq & m.vic_mask) != 0 || (m.cia[0].icr & m.cia[0].mask) != 0;
            if irq && cpu.p & 0x04 == 0 {
                if idle {
                    from_idle = Some(cpu.sp);
                }
                cpu.interrupt(&mut m, 0xFFFE);
                irqs += 1;
                idle = false;
            }
            if idle {
                // Waiting for the next interrupt: move time on a line.
                cpu.cycles += line_cycles;
                continue;
            }
            let op = m.read(cpu.pc);
            cpu.step(&mut m);
            if op == 0x40 && from_idle == Some(cpu.sp) {
                from_idle = None;
                idle = true;
            }
            if op == 0x60 && cpu.sp == sp0 {
                // Init returned: the tune lives in its interrupts.
                idle = true;
                cpu.p &= !0x04;
            }
            if cpu.jammed {
                idle = true;
                cpu.jammed = false;
                cpu.p &= !0x04;
            }
        }
        rate = irqs as f64 / seconds.max(1e-3);
    }
    Trace { writes: m.writes, rate, seconds, chips, clock }
}
