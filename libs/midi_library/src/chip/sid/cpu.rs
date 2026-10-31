//! A 6502 (the C64's 6510, the NES's 2A03 without its decimal mode): every
//! documented instruction and the stable undocumented ones music players
//! use (LAX, SAX, DCP, ISC, SLO, RLA, SRE, RRA, ANC, ALR, ARR, SBX and the
//! NOPs), with cycle counts (page crossings included) for the timers and
//! the raster a tune may read.

/// What the CPU reads and writes.
pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
}

const C: u8 = 0x01;
const Z: u8 = 0x02;
const I: u8 = 0x04;
const D: u8 = 0x08;
const B: u8 = 0x10;
const U: u8 = 0x20;
const V: u8 = 0x40;
const N: u8 = 0x80;

#[derive(Clone, Debug, Default)]
pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    pub p: u8,
    /// Cycles run.
    pub cycles: u64,
    /// Decimal mode works (the 6510; the 2A03 has none).
    pub decimal: bool,
    /// An instruction that locks a real 6502 (KIL) was met.
    pub jammed: bool,
}

#[derive(Clone, Copy)]
enum Mode {
    Imp,
    Acc,
    Imm,
    Zp,
    Zpx,
    Zpy,
    Abs,
    Abx,
    Aby,
    Ind,
    Izx,
    Izy,
    Rel,
}

impl Cpu {
    pub fn new(decimal: bool) -> Cpu {
        Cpu { sp: 0xFD, p: U | I, decimal, ..Default::default() }
    }

    fn set_zn(&mut self, v: u8) {
        self.p = (self.p & !(Z | N)) | if v == 0 { Z } else { 0 } | (v & N);
    }

    fn flag(&mut self, f: u8, on: bool) {
        if on {
            self.p |= f;
        } else {
            self.p &= !f;
        }
    }

    pub fn push(&mut self, bus: &mut dyn Bus, v: u8) {
        bus.write(0x100 | self.sp as u16, v);
        self.sp = self.sp.wrapping_sub(1);
    }

    pub fn pull(&mut self, bus: &mut dyn Bus) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        bus.read(0x100 | self.sp as u16)
    }

    fn read16(bus: &mut dyn Bus, addr: u16) -> u16 {
        bus.read(addr) as u16 | (bus.read(addr.wrapping_add(1)) as u16) << 8
    }

    /// A 16-bit read that wraps within the page (the 6502's indirect bug).
    fn read16_page(bus: &mut dyn Bus, addr: u16) -> u16 {
        let hi = (addr & 0xFF00) | (addr.wrapping_add(1) & 0x00FF);
        bus.read(addr) as u16 | (bus.read(hi) as u16) << 8
    }

    /// The operand's address (and whether indexing crossed a page).
    fn addr(&mut self, bus: &mut dyn Bus, mode: Mode) -> (u16, bool) {
        let pc = self.pc;
        match mode {
            Mode::Imp | Mode::Acc => (0, false),
            Mode::Imm => {
                self.pc = pc.wrapping_add(1);
                (pc, false)
            }
            Mode::Zp => {
                self.pc = pc.wrapping_add(1);
                (bus.read(pc) as u16, false)
            }
            Mode::Zpx => {
                self.pc = pc.wrapping_add(1);
                (bus.read(pc).wrapping_add(self.x) as u16, false)
            }
            Mode::Zpy => {
                self.pc = pc.wrapping_add(1);
                (bus.read(pc).wrapping_add(self.y) as u16, false)
            }
            Mode::Abs => {
                self.pc = pc.wrapping_add(2);
                (Self::read16(bus, pc), false)
            }
            Mode::Abx | Mode::Aby => {
                self.pc = pc.wrapping_add(2);
                let base = Self::read16(bus, pc);
                let a = base.wrapping_add(if matches!(mode, Mode::Abx) { self.x } else { self.y } as u16);
                (a, a & 0xFF00 != base & 0xFF00)
            }
            Mode::Ind => {
                self.pc = pc.wrapping_add(2);
                let ptr = Self::read16(bus, pc);
                (Self::read16_page(bus, ptr), false)
            }
            Mode::Izx => {
                self.pc = pc.wrapping_add(1);
                let zp = bus.read(pc).wrapping_add(self.x);
                (bus.read(zp as u16) as u16 | (bus.read(zp.wrapping_add(1) as u16) as u16) << 8, false)
            }
            Mode::Izy => {
                self.pc = pc.wrapping_add(1);
                let zp = bus.read(pc);
                let base = bus.read(zp as u16) as u16 | (bus.read(zp.wrapping_add(1) as u16) as u16) << 8;
                let a = base.wrapping_add(self.y as u16);
                (a, a & 0xFF00 != base & 0xFF00)
            }
            Mode::Rel => {
                self.pc = pc.wrapping_add(1);
                let off = bus.read(pc) as i8;
                (self.pc.wrapping_add(off as u16), false)
            }
        }
    }

    fn adc(&mut self, v: u8) {
        let carry = (self.p & C) as u16;
        if self.decimal && self.p & D != 0 {
            let a = self.a as u16;
            let mut lo = (a & 0x0F) + (v as u16 & 0x0F) + carry;
            if lo > 9 {
                lo += 6;
            }
            let mut hi = (a >> 4) + (v as u16 >> 4) + if lo > 0x0F { 1 } else { 0 };
            let bin = a + v as u16 + carry;
            self.flag(Z, bin & 0xFF == 0);
            self.flag(N, hi & 8 != 0);
            self.flag(V, ((a ^ (hi << 4)) & 0x80 != 0) && ((a ^ v as u16) & 0x80 == 0));
            if hi > 9 {
                hi += 6;
            }
            self.flag(C, hi > 0x0F);
            self.a = (((hi << 4) | (lo & 0x0F)) & 0xFF) as u8;
            return;
        }
        let sum = self.a as u16 + v as u16 + carry;
        let r = sum as u8;
        self.flag(C, sum > 0xFF);
        self.flag(V, (!(self.a ^ v) & (self.a ^ r) & 0x80) != 0);
        self.a = r;
        self.set_zn(r);
    }

    fn sbc(&mut self, v: u8) {
        if self.decimal && self.p & D != 0 {
            let borrow = 1 - (self.p & C) as i16;
            let a = self.a as i16;
            let bin = a - v as i16 - borrow;
            let mut lo = (a & 0x0F) - (v as i16 & 0x0F) - borrow;
            let mut hi = (a >> 4) - (v as i16 >> 4);
            if lo < 0 {
                lo -= 6;
                hi -= 1;
            }
            if hi < 0 {
                hi -= 6;
            }
            self.flag(C, bin >= 0);
            self.flag(V, ((a ^ v as i16) & (a ^ bin) & 0x80) != 0);
            self.set_zn(bin as u8);
            self.a = (((hi << 4) | (lo & 0x0F)) & 0xFF) as u8;
            return;
        }
        self.adc(!v);
    }

    fn cmp(&mut self, r: u8, v: u8) {
        let d = r.wrapping_sub(v);
        self.flag(C, r >= v);
        self.set_zn(d);
    }

    fn asl(&mut self, v: u8) -> u8 {
        self.flag(C, v & 0x80 != 0);
        let r = v << 1;
        self.set_zn(r);
        r
    }

    fn lsr(&mut self, v: u8) -> u8 {
        self.flag(C, v & 1 != 0);
        let r = v >> 1;
        self.set_zn(r);
        r
    }

    fn rol(&mut self, v: u8) -> u8 {
        let c = self.p & C;
        self.flag(C, v & 0x80 != 0);
        let r = (v << 1) | c;
        self.set_zn(r);
        r
    }

    fn ror(&mut self, v: u8) -> u8 {
        let c = self.p & C;
        self.flag(C, v & 1 != 0);
        let r = (v >> 1) | (c << 7);
        self.set_zn(r);
        r
    }

    fn branch(&mut self, target: u16, take: bool) -> u32 {
        if take {
            let extra = if target & 0xFF00 != self.pc & 0xFF00 { 2 } else { 1 };
            self.pc = target;
            extra
        } else {
            0
        }
    }

    /// An interrupt: the return address and flags pushed, then the vector.
    pub fn interrupt(&mut self, bus: &mut dyn Bus, vector: u16) {
        self.push(bus, (self.pc >> 8) as u8);
        self.push(bus, self.pc as u8);
        self.push(bus, (self.p | U) & !B);
        self.p |= I;
        self.pc = Self::read16(bus, vector);
        self.cycles += 7;
    }

    /// Run one instruction; the cycles it took.
    pub fn step(&mut self, bus: &mut dyn Bus) -> u32 {
        use Mode::*;
        let op = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        // (mode, base cycles, page-cross penalty)
        let mode = match op & 0x1F {
            0x00 => match op {
                0x20 => Abs,
                0x00 | 0x40 | 0x60 => Imp,
                _ => Imm,
            },
            0x01 | 0x03 => Izx,
            0x02 => match op {
                0x82 | 0xA2 | 0xC2 | 0xE2 => Imm,
                _ => Imp,
            },
            0x04..=0x07 => Zp,
            0x08 | 0x0A => match op {
                0x0A | 0x2A | 0x4A | 0x6A => Acc,
                _ => Imp,
            },
            0x09 | 0x0B => Imm,
            0x0C..=0x0F => {
                if op == 0x6C {
                    Ind
                } else {
                    Abs
                }
            }
            0x10 => Rel,
            0x11 | 0x13 => Izy,
            0x12 => Imp,
            0x14 | 0x15 => Zpx,
            0x16 | 0x17 => {
                if matches!(op, 0x96 | 0xB6 | 0x97 | 0xB7) {
                    Zpy
                } else {
                    Zpx
                }
            }
            0x18 | 0x1A => Imp,
            0x19 | 0x1B => Aby,
            0x1C | 0x1D => Abx,
            0x1E | 0x1F => {
                if matches!(op, 0x9E | 0xBE | 0x9F | 0xBF) {
                    Aby
                } else {
                    Abx
                }
            }
            _ => Imp,
        };
        let base = CYCLES[op as usize] as u32;
        let (addr, crossed) = self.addr(bus, mode);
        let mut extra = 0u32;
        // Reads that cost a cycle more across a page.
        let read_penalty = |crossed: bool| if crossed { 1 } else { 0 };
        macro_rules! load {
            () => {{
                extra += read_penalty(crossed);
                bus.read(addr)
            }};
        }
        macro_rules! rmw {
            ($f:expr) => {{
                if matches!(mode, Acc) {
                    let v = self.a;
                    self.a = $f(self, v);
                } else {
                    let v = bus.read(addr);
                    let r = $f(self, v);
                    bus.write(addr, r);
                }
            }};
        }
        match op {
            // Loads, stores, transfers.
            0xA9 | 0xA5 | 0xB5 | 0xAD | 0xBD | 0xB9 | 0xA1 | 0xB1 => {
                self.a = load!();
                self.set_zn(self.a);
            }
            0xA2 | 0xA6 | 0xB6 | 0xAE | 0xBE => {
                self.x = load!();
                self.set_zn(self.x);
            }
            0xA0 | 0xA4 | 0xB4 | 0xAC | 0xBC => {
                self.y = load!();
                self.set_zn(self.y);
            }
            0x85 | 0x95 | 0x8D | 0x9D | 0x99 | 0x81 | 0x91 => bus.write(addr, self.a),
            0x86 | 0x96 | 0x8E => bus.write(addr, self.x),
            0x84 | 0x94 | 0x8C => bus.write(addr, self.y),
            0xAA => {
                self.x = self.a;
                self.set_zn(self.x);
            }
            0xA8 => {
                self.y = self.a;
                self.set_zn(self.y);
            }
            0x8A => {
                self.a = self.x;
                self.set_zn(self.a);
            }
            0x98 => {
                self.a = self.y;
                self.set_zn(self.a);
            }
            0xBA => {
                self.x = self.sp;
                self.set_zn(self.x);
            }
            0x9A => self.sp = self.x,
            // Stack.
            0x48 => self.push(bus, self.a),
            0x08 => self.push(bus, self.p | B | U),
            0x68 => {
                self.a = self.pull(bus);
                self.set_zn(self.a);
            }
            0x28 => self.p = (self.pull(bus) & !B) | U,
            // Arithmetic and logic.
            0x69 | 0x65 | 0x75 | 0x6D | 0x7D | 0x79 | 0x61 | 0x71 => {
                let v = load!();
                self.adc(v);
            }
            0xE9 | 0xEB | 0xE5 | 0xF5 | 0xED | 0xFD | 0xF9 | 0xE1 | 0xF1 => {
                let v = load!();
                self.sbc(v);
            }
            0x29 | 0x25 | 0x35 | 0x2D | 0x3D | 0x39 | 0x21 | 0x31 => {
                self.a &= load!();
                self.set_zn(self.a);
            }
            0x09 | 0x05 | 0x15 | 0x0D | 0x1D | 0x19 | 0x01 | 0x11 => {
                self.a |= load!();
                self.set_zn(self.a);
            }
            0x49 | 0x45 | 0x55 | 0x4D | 0x5D | 0x59 | 0x41 | 0x51 => {
                self.a ^= load!();
                self.set_zn(self.a);
            }
            0xC9 | 0xC5 | 0xD5 | 0xCD | 0xDD | 0xD9 | 0xC1 | 0xD1 => {
                let v = load!();
                self.cmp(self.a, v);
            }
            0xE0 | 0xE4 | 0xEC => {
                let v = load!();
                self.cmp(self.x, v);
            }
            0xC0 | 0xC4 | 0xCC => {
                let v = load!();
                self.cmp(self.y, v);
            }
            0x24 | 0x2C => {
                let v = bus.read(addr);
                self.flag(Z, self.a & v == 0);
                self.p = (self.p & !(N | V)) | (v & (N | V));
            }
            // Increments.
            0xE6 | 0xF6 | 0xEE | 0xFE => {
                let v = bus.read(addr).wrapping_add(1);
                bus.write(addr, v);
                self.set_zn(v);
            }
            0xC6 | 0xD6 | 0xCE | 0xDE => {
                let v = bus.read(addr).wrapping_sub(1);
                bus.write(addr, v);
                self.set_zn(v);
            }
            0xE8 => {
                self.x = self.x.wrapping_add(1);
                self.set_zn(self.x);
            }
            0xC8 => {
                self.y = self.y.wrapping_add(1);
                self.set_zn(self.y);
            }
            0xCA => {
                self.x = self.x.wrapping_sub(1);
                self.set_zn(self.x);
            }
            0x88 => {
                self.y = self.y.wrapping_sub(1);
                self.set_zn(self.y);
            }
            // Shifts.
            0x0A | 0x06 | 0x16 | 0x0E | 0x1E => rmw!(|c: &mut Cpu, v| c.asl(v)),
            0x4A | 0x46 | 0x56 | 0x4E | 0x5E => rmw!(|c: &mut Cpu, v| c.lsr(v)),
            0x2A | 0x26 | 0x36 | 0x2E | 0x3E => rmw!(|c: &mut Cpu, v| c.rol(v)),
            0x6A | 0x66 | 0x76 | 0x6E | 0x7E => rmw!(|c: &mut Cpu, v| c.ror(v)),
            // Jumps and calls.
            0x4C | 0x6C => self.pc = addr,
            0x20 => {
                let ret = self.pc.wrapping_sub(1);
                self.push(bus, (ret >> 8) as u8);
                self.push(bus, ret as u8);
                self.pc = addr;
            }
            0x60 => {
                let lo = self.pull(bus) as u16;
                let hi = self.pull(bus) as u16;
                self.pc = ((hi << 8) | lo).wrapping_add(1);
            }
            0x40 => {
                self.p = (self.pull(bus) & !B) | U;
                let lo = self.pull(bus) as u16;
                let hi = self.pull(bus) as u16;
                self.pc = (hi << 8) | lo;
            }
            0x00 => {
                self.pc = self.pc.wrapping_add(1);
                self.push(bus, (self.pc >> 8) as u8);
                self.push(bus, self.pc as u8);
                self.push(bus, self.p | B | U);
                self.p |= I;
                self.pc = Self::read16(bus, 0xFFFE);
            }
            // Branches.
            0x10 => extra += self.branch(addr, self.p & N == 0),
            0x30 => extra += self.branch(addr, self.p & N != 0),
            0x50 => extra += self.branch(addr, self.p & V == 0),
            0x70 => extra += self.branch(addr, self.p & V != 0),
            0x90 => extra += self.branch(addr, self.p & C == 0),
            0xB0 => extra += self.branch(addr, self.p & C != 0),
            0xD0 => extra += self.branch(addr, self.p & Z == 0),
            0xF0 => extra += self.branch(addr, self.p & Z != 0),
            // Flags.
            0x18 => self.p &= !C,
            0x38 => self.p |= C,
            0x58 => self.p &= !I,
            0x78 => self.p |= I,
            0xB8 => self.p &= !V,
            0xD8 => self.p &= !D,
            0xF8 => self.p |= D,
            0xEA => {}
            // Undocumented.
            0xA7 | 0xB7 | 0xAF | 0xBF | 0xA3 | 0xB3 | 0xAB => {
                let v = load!();
                self.a = v;
                self.x = v;
                self.set_zn(v);
            }
            0x87 | 0x97 | 0x8F | 0x83 => bus.write(addr, self.a & self.x),
            0xC7 | 0xD7 | 0xCF | 0xDF | 0xDB | 0xC3 | 0xD3 => {
                let v = bus.read(addr).wrapping_sub(1);
                bus.write(addr, v);
                self.cmp(self.a, v);
            }
            0xE7 | 0xF7 | 0xEF | 0xFF | 0xFB | 0xE3 | 0xF3 => {
                let v = bus.read(addr).wrapping_add(1);
                bus.write(addr, v);
                self.sbc(v);
            }
            0x07 | 0x17 | 0x0F | 0x1F | 0x1B | 0x03 | 0x13 => {
                let v = bus.read(addr);
                let r = self.asl(v);
                bus.write(addr, r);
                self.a |= r;
                self.set_zn(self.a);
            }
            0x27 | 0x37 | 0x2F | 0x3F | 0x3B | 0x23 | 0x33 => {
                let v = bus.read(addr);
                let r = self.rol(v);
                bus.write(addr, r);
                self.a &= r;
                self.set_zn(self.a);
            }
            0x47 | 0x57 | 0x4F | 0x5F | 0x5B | 0x43 | 0x53 => {
                let v = bus.read(addr);
                let r = self.lsr(v);
                bus.write(addr, r);
                self.a ^= r;
                self.set_zn(self.a);
            }
            0x67 | 0x77 | 0x6F | 0x7F | 0x7B | 0x63 | 0x73 => {
                let v = bus.read(addr);
                let r = self.ror(v);
                bus.write(addr, r);
                self.adc(r);
            }
            0x0B | 0x2B => {
                self.a &= bus.read(addr);
                self.set_zn(self.a);
                self.flag(C, self.a & 0x80 != 0);
            }
            0x4B => {
                self.a &= bus.read(addr);
                self.a = self.lsr(self.a);
            }
            0x6B => {
                self.a &= bus.read(addr);
                self.a = (self.a >> 1) | ((self.p & C) << 7);
                self.set_zn(self.a);
                self.flag(C, self.a & 0x40 != 0);
                self.flag(V, ((self.a >> 6) ^ (self.a >> 5)) & 1 != 0);
            }
            0xCB => {
                let v = bus.read(addr);
                let ax = self.a & self.x;
                self.flag(C, ax >= v);
                self.x = ax.wrapping_sub(v);
                self.set_zn(self.x);
            }
            0x8B => {
                self.a = self.x & bus.read(addr);
                self.set_zn(self.a);
            }
            0x9F | 0x93 => bus.write(addr, self.a & self.x & ((addr >> 8) as u8).wrapping_add(1)),
            0x9E => bus.write(addr, self.x & ((addr >> 8) as u8).wrapping_add(1)),
            0x9C => bus.write(addr, self.y & ((addr >> 8) as u8).wrapping_add(1)),
            0x9B => {
                self.sp = self.a & self.x;
                bus.write(addr, self.sp & ((addr >> 8) as u8).wrapping_add(1));
            }
            0xBB => {
                let v = bus.read(addr) & self.sp;
                self.a = v;
                self.x = v;
                self.sp = v;
                self.set_zn(v);
            }
            // The NOPs (reading their operand).
            0x1A | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA | 0x80 | 0x82 | 0x89 | 0xC2 | 0xE2 | 0x04 | 0x44 | 0x64 | 0x14 | 0x34 | 0x54 | 0x74 | 0xD4 | 0xF4 | 0x0C | 0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => {
                if matches!(mode, Abx) {
                    extra += read_penalty(crossed);
                }
            }
            // KIL: a real CPU stops.
            _ => {
                self.jammed = true;
                self.pc = self.pc.wrapping_sub(1);
            }
        }
        let n = base + extra;
        self.cycles += n as u64;
        n
    }
}

/// Base cycles per opcode.
const CYCLES: [u8; 256] = [
    7, 6, 2, 8, 3, 3, 5, 5, 3, 2, 2, 2, 4, 4, 6, 6, //
    2, 5, 2, 8, 4, 4, 6, 6, 2, 4, 2, 7, 4, 4, 7, 7, //
    6, 6, 2, 8, 3, 3, 5, 5, 4, 2, 2, 2, 4, 4, 6, 6, //
    2, 5, 2, 8, 4, 4, 6, 6, 2, 4, 2, 7, 4, 4, 7, 7, //
    6, 6, 2, 8, 3, 3, 5, 5, 3, 2, 2, 2, 3, 4, 6, 6, //
    2, 5, 2, 8, 4, 4, 6, 6, 2, 4, 2, 7, 4, 4, 7, 7, //
    6, 6, 2, 8, 3, 3, 5, 5, 4, 2, 2, 2, 5, 4, 6, 6, //
    2, 5, 2, 8, 4, 4, 6, 6, 2, 4, 2, 7, 4, 4, 7, 7, //
    2, 6, 2, 6, 3, 3, 3, 3, 2, 2, 2, 2, 4, 4, 4, 4, //
    2, 6, 2, 6, 4, 4, 4, 4, 2, 5, 2, 5, 5, 5, 5, 5, //
    2, 6, 2, 6, 3, 3, 3, 3, 2, 2, 2, 2, 4, 4, 4, 4, //
    2, 5, 2, 5, 4, 4, 4, 4, 2, 4, 2, 4, 4, 4, 4, 4, //
    2, 6, 2, 8, 3, 3, 5, 5, 2, 2, 2, 2, 4, 4, 6, 6, //
    2, 5, 2, 8, 4, 4, 6, 6, 2, 4, 2, 7, 4, 4, 7, 7, //
    2, 6, 2, 8, 3, 3, 5, 5, 2, 2, 2, 2, 4, 4, 6, 6, //
    2, 5, 2, 8, 4, 4, 6, 6, 2, 4, 2, 7, 4, 4, 7, 7, //
];

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl Bus for Ram {
        fn read(&mut self, addr: u16) -> u8 {
            self.0[addr as usize]
        }
        fn write(&mut self, addr: u16, value: u8) {
            self.0[addr as usize] = value;
        }
    }

    fn run(code: &[u8]) -> (Cpu, Ram) {
        let mut ram = Ram(vec![0; 65536]);
        ram.0[0x1000..0x1000 + code.len()].copy_from_slice(code);
        let mut cpu = Cpu::new(true);
        cpu.pc = 0x1000;
        // Run to the BRK that ends the code.
        while cpu.pc < 0x1000 + code.len() as u16 && !cpu.jammed {
            if ram.0[cpu.pc as usize] == 0 {
                break;
            }
            cpu.step(&mut ram);
        }
        (cpu, ram)
    }

    #[test]
    fn arithmetic_loops_and_calls() {
        // Sum 1..=10 in A with a loop.
        let (cpu, _) = run(&[0xA9, 0x00, 0xA2, 0x0A, 0x18, 0x86, 0x10, 0x65, 0x10, 0xCA, 0xD0, 0xF8, 0x00]);
        assert_eq!(cpu.a, 55);
        // JSR to a subroutine that sets X, RTS back.
        let (cpu, _) = run(&[0x20, 0x06, 0x10, 0xA0, 0x07, 0x00, 0xA2, 0x42, 0x60]);
        assert_eq!((cpu.x, cpu.y), (0x42, 7));
        // SBC with borrow, CMP, decimal ADC.
        let (cpu, _) = run(&[0x38, 0xA9, 0x50, 0xE9, 0x20, 0x00]);
        assert_eq!(cpu.a, 0x30);
        let (cpu, _) = run(&[0xF8, 0x18, 0xA9, 0x19, 0x69, 0x28, 0x00]);
        assert_eq!(cpu.a, 0x47);
        // Indirect indexed store and LAX.
        let (_, ram) = run(&[0xA9, 0x00, 0x85, 0x20, 0xA9, 0x30, 0x85, 0x21, 0xA0, 0x05, 0xA9, 0x99, 0x91, 0x20, 0x00]);
        assert_eq!(ram.0[0x3005], 0x99);
        let (cpu, _) = run(&[0xA9, 0x77, 0x85, 0x40, 0xA7, 0x40, 0x00]);
        assert_eq!((cpu.a, cpu.x), (0x77, 0x77));
    }
}
