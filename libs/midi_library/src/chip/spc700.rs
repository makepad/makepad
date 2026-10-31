//! The SPC700: the SNES sound CPU (Sony's 8-bit core with a direct page,
//! 16-bit word ops, bit ops and its own branch set), every opcode, with
//! approximate cycle counts (the sound drivers pace themselves on the
//! timers, which follow the cycles).

pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
}

const N: u8 = 0x80;
const V: u8 = 0x40;
const P: u8 = 0x20;
const B: u8 = 0x10;
const H: u8 = 0x08;
const I: u8 = 0x04;
const Z: u8 = 0x02;
const C: u8 = 0x01;

#[derive(Clone, Debug, Default)]
pub struct Spc700 {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    pub psw: u8,
    pub cycles: u64,
    /// SLEEP or STOP was met.
    pub halted: bool,
}

impl Spc700 {
    fn set_nz(&mut self, v: u8) {
        self.psw = (self.psw & !(N | Z)) | (v & N) | if v == 0 { Z } else { 0 };
    }

    fn flag(&mut self, f: u8, on: bool) {
        if on {
            self.psw |= f;
        } else {
            self.psw &= !f;
        }
    }

    fn fetch(&mut self, bus: &mut dyn Bus) -> u8 {
        let v = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn fetch16(&mut self, bus: &mut dyn Bus) -> u16 {
        let lo = self.fetch(bus) as u16;
        lo | (self.fetch(bus) as u16) << 8
    }

    fn page(&self) -> u16 {
        if self.psw & P != 0 { 0x100 } else { 0 }
    }

    fn dp(&self, d: u8) -> u16 {
        self.page() | d as u16
    }

    fn read_dp16(&self, bus: &mut dyn Bus, d: u8) -> u16 {
        bus.read(self.dp(d)) as u16 | (bus.read(self.dp(d.wrapping_add(1))) as u16) << 8
    }

    fn push(&mut self, bus: &mut dyn Bus, v: u8) {
        bus.write(0x100 | self.sp as u16, v);
        self.sp = self.sp.wrapping_sub(1);
    }

    fn pop(&mut self, bus: &mut dyn Bus) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        bus.read(0x100 | self.sp as u16)
    }

    fn call(&mut self, bus: &mut dyn Bus, to: u16) {
        self.push(bus, (self.pc >> 8) as u8);
        self.push(bus, self.pc as u8);
        self.pc = to;
    }

    fn adc(&mut self, a: u8, b: u8) -> u8 {
        let c = (self.psw & C) as u16;
        let r = a as u16 + b as u16 + c;
        self.flag(C, r > 0xFF);
        self.flag(H, (a & 15) + (b & 15) + c as u8 > 15);
        self.flag(V, (!(a ^ b) & (a ^ r as u8) & 0x80) != 0);
        self.set_nz(r as u8);
        r as u8
    }

    fn sbc(&mut self, a: u8, b: u8) -> u8 {
        self.adc(a, !b)
    }

    fn cmp(&mut self, a: u8, b: u8) {
        let r = a as i16 - b as i16;
        self.flag(C, r >= 0);
        self.set_nz(r as u8);
    }

    /// An operand address for the ALU group (OR AND EOR CMP ADC SBC MOV):
    /// the low nibble picks the mode.
    fn alu_addr(&mut self, bus: &mut dyn Bus, low: u8) -> u16 {
        match low {
            0x4 => {
                let d = self.fetch(bus);
                self.dp(d)
            }
            0x5 => self.fetch16(bus),
            0x6 => self.dp(self.x),
            0x7 => {
                let d = self.fetch(bus).wrapping_add(self.x);
                self.read_dp16(bus, d)
            }
            0x14 => {
                let d = self.fetch(bus).wrapping_add(self.x);
                self.dp(d)
            }
            0x15 => self.fetch16(bus).wrapping_add(self.x as u16),
            0x16 => self.fetch16(bus).wrapping_add(self.y as u16),
            0x17 => {
                let d = self.fetch(bus);
                self.read_dp16(bus, d).wrapping_add(self.y as u16)
            }
            _ => 0,
        }
    }

    fn alu(&mut self, op: u8, a: u8, b: u8) -> u8 {
        match op >> 5 {
            0 => {
                let r = a | b;
                self.set_nz(r);
                r
            }
            1 => {
                let r = a & b;
                self.set_nz(r);
                r
            }
            2 => {
                let r = a ^ b;
                self.set_nz(r);
                r
            }
            3 => {
                self.cmp(a, b);
                a
            }
            4 => self.adc(a, b),
            _ => self.sbc(a, b),
        }
    }

    fn shift(&mut self, kind: u8, v: u8) -> u8 {
        let c = self.psw & C;
        let (r, out) = match kind {
            0 => (v << 1, v & 0x80 != 0),
            1 => ((v << 1) | c, v & 0x80 != 0),
            2 => (v >> 1, v & 1 != 0),
            _ => ((v >> 1) | (c << 7), v & 1 != 0),
        };
        self.flag(C, out);
        self.set_nz(r);
        r
    }

    fn branch(&mut self, bus: &mut dyn Bus, take: bool) -> u32 {
        let off = self.fetch(bus) as i8;
        if take {
            self.pc = self.pc.wrapping_add(off as u16);
            2
        } else {
            0
        }
    }

    /// The bit operand `m.b` (13-bit address, 3-bit bit number).
    fn membit(&mut self, bus: &mut dyn Bus) -> (u16, u8) {
        let v = self.fetch16(bus);
        (v & 0x1FFF, (v >> 13) as u8)
    }

    /// Run one instruction; its cycles.
    pub fn step(&mut self, bus: &mut dyn Bus) -> u32 {
        if self.halted {
            self.cycles += 2;
            return 2;
        }
        let op = self.fetch(bus);
        let hi = op >> 4;
        let lo = op & 15;
        let mut cyc: u32 = 2;
        let mut extra = 0;
        match (hi, lo) {
            // ALU group: OR AND EOR CMP ADC SBC on A, and their dp/dp,
            // (X),(Y) and dp,#imm forms (columns 4-9, rows even/odd up to Bx).
            (0x0..=0xB, 0x4..=0x9) if hi & 1 == 0 || lo <= 0x9 => {
                let group = (hi >> 1) << 5;
                let odd = hi & 1 == 1;
                match (odd, lo) {
                    (false, 0x8) => {
                        let v = self.fetch(bus);
                        self.a = self.alu(group, self.a, v);
                    }
                    (false, 0x9) => {
                        // dd, ds: source first.
                        let s = self.fetch(bus);
                        let d = self.fetch(bus);
                        let sv = bus.read(self.dp(s));
                        let dv = bus.read(self.dp(d));
                        let r = self.alu(group, dv, sv);
                        if group >> 5 != 3 {
                            bus.write(self.dp(d), r);
                        }
                        cyc = 6;
                    }
                    (true, 0x8) => {
                        let v = self.fetch(bus);
                        let d = self.fetch(bus);
                        let dv = bus.read(self.dp(d));
                        let r = self.alu(group, dv, v);
                        if group >> 5 != 3 {
                            bus.write(self.dp(d), r);
                        }
                        cyc = 5;
                    }
                    (true, 0x9) => {
                        let xv = bus.read(self.dp(self.x));
                        let yv = bus.read(self.dp(self.y));
                        let r = self.alu(group, xv, yv);
                        if group >> 5 != 3 {
                            bus.write(self.dp(self.x), r);
                        }
                        cyc = 5;
                    }
                    _ => {
                        let mode = if odd { lo | 0x10 } else { lo };
                        let at = self.alu_addr(bus, mode);
                        let v = bus.read(at);
                        self.a = self.alu(group, self.a, v);
                        cyc = 3 + (lo as u32 & 3);
                    }
                }
            }
            // MOV A <- (Ex/Fx columns 4-8 like the ALU modes).
            (0xE, 0x4..=0x7) | (0xF, 0x4..=0x7) => {
                let mode = if hi == 0xF { lo | 0x10 } else { lo };
                let at = self.alu_addr(bus, mode);
                self.a = bus.read(at);
                self.set_nz(self.a);
                cyc = 3 + (lo as u32 & 3);
            }
            (0xE, 0x8) => {
                self.a = self.fetch(bus);
                self.set_nz(self.a);
            }
            // MOV -> memory from A (Cx/Dx columns 4-7).
            (0xC, 0x4..=0x7) | (0xD, 0x4..=0x7) => {
                let mode = if hi == 0xD { lo | 0x10 } else { lo };
                let at = self.alu_addr(bus, mode);
                bus.write(at, self.a);
                cyc = 4 + (lo as u32 & 3);
            }
            (_, 0x1) => {
                // TCALL n.
                let n = hi as u16;
                let at = 0xFFDE - 2 * n;
                let to = bus.read(at) as u16 | (bus.read(at + 1) as u16) << 8;
                self.call(bus, to);
                cyc = 8;
            }
            (_, 0x2) => {
                // SET1 / CLR1 d.b.
                let d = self.fetch(bus);
                let bit = hi >> 1;
                let at = self.dp(d);
                let v = bus.read(at);
                bus.write(at, if hi & 1 == 0 { v | (1 << bit) } else { v & !(1 << bit) });
                cyc = 4;
            }
            (_, 0x3) => {
                // BBS / BBC d.b, r.
                let d = self.fetch(bus);
                let v = bus.read(self.dp(d));
                let set = v & (1 << (hi >> 1)) != 0;
                extra = self.branch(bus, if hi & 1 == 0 { set } else { !set });
                cyc = 5;
            }
            (_, 0xA) if hi & 1 == 0 || hi == 0xF => match hi {
                0x0 | 0x2 | 0x4 | 0x6 | 0x8 | 0xA | 0xC | 0xE => {
                    let (at, b) = self.membit(bus);
                    let bit = bus.read(at) >> b & 1 != 0;
                    let c = self.psw & C != 0;
                    match hi {
                        0x0 => self.flag(C, c | bit),
                        0x2 => self.flag(C, c | !bit),
                        0x4 => self.flag(C, c & bit),
                        0x6 => self.flag(C, c & !bit),
                        0x8 => self.flag(C, c ^ bit),
                        0xA => self.flag(C, bit),
                        0xC => {
                            let v = bus.read(at);
                            bus.write(at, if c { v | (1 << b) } else { v & !(1 << b) });
                        }
                        _ => {
                            let v = bus.read(at);
                            bus.write(at, v ^ (1 << b));
                        }
                    }
                    cyc = 5;
                }
                _ => {
                    // MOV dd, ds (FA).
                    let s = self.fetch(bus);
                    let d = self.fetch(bus);
                    let v = bus.read(self.dp(s));
                    bus.write(self.dp(d), v);
                    cyc = 5;
                }
            },
            (_, 0xA) => {
                // Word ops on odd rows: DECW INCW CMPW ADDW SUBW MOVW YA,d MOVW d,YA.
                let d = self.fetch(bus);
                let w = self.read_dp16(bus, d);
                let ya = (self.y as u16) << 8 | self.a as u16;
                let set_ya = |s: &mut Spc700, v: u16| {
                    s.a = v as u8;
                    s.y = (v >> 8) as u8;
                    s.flag(N, v & 0x8000 != 0);
                    s.flag(Z, v == 0);
                };
                match hi {
                    0x1 | 0x3 => {
                        let r = if hi == 1 { w.wrapping_sub(1) } else { w.wrapping_add(1) };
                        bus.write(self.dp(d), r as u8);
                        bus.write(self.dp(d.wrapping_add(1)), (r >> 8) as u8);
                        self.flag(N, r & 0x8000 != 0);
                        self.flag(Z, r == 0);
                        cyc = 6;
                    }
                    0x5 => {
                        let r = ya.wrapping_sub(w);
                        self.flag(C, ya >= w);
                        self.flag(N, r & 0x8000 != 0);
                        self.flag(Z, r == 0);
                        cyc = 4;
                    }
                    0x7 => {
                        let r = ya as u32 + w as u32;
                        self.flag(C, r > 0xFFFF);
                        self.flag(H, (ya & 0xFFF) + (w & 0xFFF) > 0xFFF);
                        self.flag(V, (!(ya ^ w) & (ya ^ r as u16) & 0x8000) != 0);
                        set_ya(self, r as u16);
                        cyc = 5;
                    }
                    0x9 => {
                        let r = ya.wrapping_sub(w);
                        self.flag(C, ya >= w);
                        self.flag(H, (ya & 0xFFF) >= (w & 0xFFF));
                        self.flag(V, ((ya ^ w) & (ya ^ r) & 0x8000) != 0);
                        set_ya(self, r);
                        cyc = 5;
                    }
                    0xB => {
                        set_ya(self, w);
                        cyc = 5;
                    }
                    _ => {
                        // 0xDA MOVW d, YA.
                        bus.write(self.dp(d), self.a);
                        bus.write(self.dp(d.wrapping_add(1)), self.y);
                        cyc = 5;
                    }
                }
            }
            (_, 0xB) | (_, 0xC) if hi <= 0xB => {
                // Shifts and INC/DEC on d, !a, d+X, A.
                let kind = hi >> 1;
                let target: Option<u16> = match (hi & 1, lo) {
                    (0, 0xB) => {
                        let d = self.fetch(bus);
                        Some(self.dp(d))
                    }
                    (0, 0xC) => Some(self.fetch16(bus)),
                    (1, 0xB) => {
                        let d = self.fetch(bus).wrapping_add(self.x);
                        Some(self.dp(d))
                    }
                    _ => None,
                };
                let v = target.map_or(self.a, |at| bus.read(at));
                let r = match kind {
                    0..=3 => self.shift(kind, v),
                    4 => {
                        let r = v.wrapping_sub(1);
                        self.set_nz(r);
                        r
                    }
                    _ => {
                        let r = v.wrapping_add(1);
                        self.set_nz(r);
                        r
                    }
                };
                match target {
                    Some(at) => bus.write(at, r),
                    None => self.a = r,
                }
                cyc = if target.is_some() { 5 } else { 2 };
            }
            (0x0, 0x0) => {}
            (0x2, 0x0) => self.psw &= !P,
            (0x4, 0x0) => self.psw |= P,
            (0x6, 0x0) => self.psw &= !C,
            (0x8, 0x0) => self.psw |= C,
            (0xA, 0x0) => self.psw |= I,
            (0xC, 0x0) => self.psw &= !I,
            (0xE, 0x0) => self.psw &= !(V | H),
            (_, 0x0) => {
                // Branches on odd rows: BPL BMI BVC BVS BCC BCS BNE BEQ.
                let take = match hi {
                    0x1 => self.psw & N == 0,
                    0x3 => self.psw & N != 0,
                    0x5 => self.psw & V == 0,
                    0x7 => self.psw & V != 0,
                    0x9 => self.psw & C == 0,
                    0xB => self.psw & C != 0,
                    0xD => self.psw & Z == 0,
                    _ => self.psw & Z != 0,
                };
                extra = self.branch(bus, take);
            }
            _ => {
                cyc = self.misc(bus, op, &mut extra);
            }
        }
        let n = cyc + extra;
        self.cycles += n as u64;
        n
    }

    fn misc(&mut self, bus: &mut dyn Bus, op: u8, extra: &mut u32) -> u32 {
        match op {
            0x0C | 0xCB | 0xD8 | 0xC9 | 0xCC | 0xD9 | 0xDB => {
                // MOV memory <- X or Y.
                let (at, v) = match op {
                    0xCB => {
                        let d = self.fetch(bus);
                        (self.dp(d), self.y)
                    }
                    0xD8 => {
                        let d = self.fetch(bus);
                        (self.dp(d), self.x)
                    }
                    0xC9 => (self.fetch16(bus), self.x),
                    0xCC => (self.fetch16(bus), self.y),
                    0xD9 => {
                        let d = self.fetch(bus).wrapping_add(self.y);
                        (self.dp(d), self.x)
                    }
                    0xDB => {
                        let d = self.fetch(bus).wrapping_add(self.x);
                        (self.dp(d), self.y)
                    }
                    _ => {
                        // 0x0C ASL !a is handled with the shifts; never here.
                        (0, 0)
                    }
                };
                bus.write(at, v);
                4
            }
            0xE9 | 0xEB | 0xEC | 0xF8 | 0xF9 | 0xFB | 0xCD | 0x8D => {
                // MOV X or Y <- memory or immediate.
                let v = match op {
                    0xE9 => {
                        let a = self.fetch16(bus);
                        bus.read(a)
                    }
                    0xEB | 0xF8 => {
                        let d = self.fetch(bus);
                        bus.read(self.dp(d))
                    }
                    0xEC => {
                        let a = self.fetch16(bus);
                        bus.read(a)
                    }
                    0xF9 => {
                        let d = self.fetch(bus).wrapping_add(self.y);
                        bus.read(self.dp(d))
                    }
                    0xFB => {
                        let d = self.fetch(bus).wrapping_add(self.x);
                        bus.read(self.dp(d))
                    }
                    _ => self.fetch(bus),
                };
                if matches!(op, 0xE9 | 0xF8 | 0xF9 | 0xCD) {
                    self.x = v;
                } else {
                    self.y = v;
                }
                self.set_nz(v);
                3
            }
            0x8F => {
                let v = self.fetch(bus);
                let d = self.fetch(bus);
                bus.write(self.dp(d), v);
                5
            }
            0x1E | 0x3E | 0x5E | 0x7E | 0xC8 | 0xAD => {
                let (r, v) = match op {
                    0x1E => {
                        let a = self.fetch16(bus);
                        (self.x, bus.read(a))
                    }
                    0x3E => {
                        let d = self.fetch(bus);
                        (self.x, bus.read(self.dp(d)))
                    }
                    0x5E => {
                        let a = self.fetch16(bus);
                        (self.y, bus.read(a))
                    }
                    0x7E => {
                        let d = self.fetch(bus);
                        (self.y, bus.read(self.dp(d)))
                    }
                    0xC8 => (self.x, self.fetch(bus)),
                    _ => (self.y, self.fetch(bus)),
                };
                self.cmp(r, v);
                3
            }
            0x7D => {
                self.a = self.x;
                self.set_nz(self.a);
                2
            }
            0xDD => {
                self.a = self.y;
                self.set_nz(self.a);
                2
            }
            0x5D => {
                self.x = self.a;
                self.set_nz(self.x);
                2
            }
            0xFD => {
                self.y = self.a;
                self.set_nz(self.y);
                2
            }
            0x9D => {
                self.x = self.sp;
                self.set_nz(self.x);
                2
            }
            0xBD => {
                self.sp = self.x;
                2
            }
            0xAF => {
                bus.write(self.dp(self.x), self.a);
                self.x = self.x.wrapping_add(1);
                4
            }
            0xBF => {
                self.a = bus.read(self.dp(self.x));
                self.x = self.x.wrapping_add(1);
                self.set_nz(self.a);
                4
            }
            0x1D | 0x3D | 0xDC | 0xFC => {
                let r = match op {
                    0x1D => {
                        self.x = self.x.wrapping_sub(1);
                        self.x
                    }
                    0x3D => {
                        self.x = self.x.wrapping_add(1);
                        self.x
                    }
                    0xDC => {
                        self.y = self.y.wrapping_sub(1);
                        self.y
                    }
                    _ => {
                        self.y = self.y.wrapping_add(1);
                        self.y
                    }
                };
                self.set_nz(r);
                2
            }
            0xCF => {
                let r = self.y as u16 * self.a as u16;
                self.a = r as u8;
                self.y = (r >> 8) as u8;
                self.set_nz(self.y);
                9
            }
            0x9E => {
                let ya = (self.y as u32) << 8 | self.a as u32;
                let x = self.x as u32;
                self.flag(V, self.y as u32 >= x);
                self.flag(H, (self.y & 15) >= (self.x & 15));
                if (self.y as u32) < (x << 1) {
                    if x != 0 {
                        self.a = (ya / x) as u8;
                        self.y = (ya % x) as u8;
                    }
                } else {
                    let d = 256 - x;
                    let r = ya.wrapping_sub(x << 9);
                    self.a = (255 - r / d) as u8;
                    self.y = (x + r % d) as u8;
                }
                self.set_nz(self.a);
                12
            }
            0x9F => {
                self.a = self.a.rotate_left(4);
                self.set_nz(self.a);
                5
            }
            0xDF => {
                if self.psw & C != 0 || self.a > 0x99 {
                    self.a = self.a.wrapping_add(0x60);
                    self.psw |= C;
                }
                if self.psw & H != 0 || self.a & 15 > 9 {
                    self.a = self.a.wrapping_add(6);
                }
                self.set_nz(self.a);
                3
            }
            0xBE => {
                if self.psw & C == 0 || self.a > 0x99 {
                    self.a = self.a.wrapping_sub(0x60);
                    self.psw &= !C;
                }
                if self.psw & H == 0 || self.a & 15 > 9 {
                    self.a = self.a.wrapping_sub(6);
                }
                self.set_nz(self.a);
                3
            }
            0x0D => {
                self.push(bus, self.psw);
                4
            }
            0x2D => {
                self.push(bus, self.a);
                4
            }
            0x4D => {
                self.push(bus, self.x);
                4
            }
            0x6D => {
                self.push(bus, self.y);
                4
            }
            0x8E => {
                self.psw = self.pop(bus);
                4
            }
            0xAE => {
                self.a = self.pop(bus);
                4
            }
            0xCE => {
                self.x = self.pop(bus);
                4
            }
            0xEE => {
                self.y = self.pop(bus);
                4
            }
            0x0E | 0x4E => {
                let at = self.fetch16(bus);
                let v = bus.read(at);
                let r = self.a.wrapping_sub(v);
                self.set_nz(r);
                bus.write(at, if op == 0x0E { v | self.a } else { v & !self.a });
                6
            }
            0x0F => {
                let to = bus_read16(bus, 0xFFDE);
                self.call(bus, to);
                self.push(bus, self.psw);
                self.psw = (self.psw | B) & !I;
                8
            }
            0x1F => {
                let a = self.fetch16(bus).wrapping_add(self.x as u16);
                self.pc = bus_read16(bus, a);
                6
            }
            0x2F => {
                *extra = self.branch(bus, true);
                2
            }
            0x3F => {
                let a = self.fetch16(bus);
                self.call(bus, a);
                8
            }
            0x4F => {
                let u = self.fetch(bus);
                self.call(bus, 0xFF00 | u as u16);
                6
            }
            0x5F => {
                self.pc = self.fetch16(bus);
                3
            }
            0x6F => {
                let lo = self.pop(bus) as u16;
                let hi = self.pop(bus) as u16;
                self.pc = hi << 8 | lo;
                5
            }
            0x7F => {
                self.psw = self.pop(bus);
                let lo = self.pop(bus) as u16;
                let hi = self.pop(bus) as u16;
                self.pc = hi << 8 | lo;
                6
            }
            0x2E | 0xDE => {
                let d = self.fetch(bus);
                let d = if op == 0xDE { d.wrapping_add(self.x) } else { d };
                let v = bus.read(self.dp(d));
                *extra = self.branch(bus, v != self.a);
                5
            }
            0x6E => {
                let d = self.fetch(bus);
                let v = bus.read(self.dp(d)).wrapping_sub(1);
                bus.write(self.dp(d), v);
                *extra = self.branch(bus, v != 0);
                5
            }
            0xFE => {
                self.y = self.y.wrapping_sub(1);
                *extra = self.branch(bus, self.y != 0);
                4
            }
            0xED => {
                self.psw ^= C;
                3
            }
            0xEF | 0xFF => {
                self.halted = true;
                2
            }
            _ => 2,
        }
    }
}

fn bus_read16(bus: &mut dyn Bus, at: u16) -> u16 {
    bus.read(at) as u16 | (bus.read(at.wrapping_add(1)) as u16) << 8
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl Bus for Ram {
        fn read(&mut self, a: u16) -> u8 {
            self.0[a as usize]
        }
        fn write(&mut self, a: u16, v: u8) {
            self.0[a as usize] = v;
        }
    }

    fn run(code: &[u8], steps: usize) -> (Spc700, Ram) {
        let mut ram = Ram(vec![0; 65536]);
        ram.0[0x400..0x400 + code.len()].copy_from_slice(code);
        let mut cpu = Spc700 { pc: 0x400, sp: 0xEF, ..Default::default() };
        for _ in 0..steps {
            cpu.step(&mut ram);
        }
        (cpu, ram)
    }

    #[test]
    fn moves_alu_words_branches_and_calls() {
        // MOV A,#5; MOV X,#3; MOV $10,A; ADC A,#$10 (C clear): 0x15.
        let (c, r) = run(&[0xE8, 0x05, 0xCD, 0x03, 0xC4, 0x10, 0x60, 0x88, 0x10], 5);
        assert_eq!((c.a, c.x, r.0[0x10]), (0x15, 3, 5));
        // Count Y down from 4 with DBNZ Y: four passes adding X to A.
        let (c, _) = run(&[0xE8, 0x00, 0xCD, 0x02, 0x8D, 0x04, 0x60, 0x7D, 0x60, 0xFE, 0xFC], 4 + 4 * 3);
        assert_eq!((c.y, c.pc), (0, 0x40B));
        // MOVW YA,$20 / INCW $20 / CALL and RET.
        let mut code = vec![0xBA, 0x20, 0x3A, 0x20, 0x3F, 0x10, 0x04, 0x00];
        code.resize(0x10, 0);
        code.extend([0xE8, 0x77, 0x6F]);
        let mut ram = Ram(vec![0; 65536]);
        ram.0[0x20] = 0x34;
        ram.0[0x21] = 0x12;
        ram.0[0x400..0x400 + code.len()].copy_from_slice(&code);
        let mut cpu = Spc700 { pc: 0x400, sp: 0xEF, ..Default::default() };
        for _ in 0..5 {
            cpu.step(&mut ram);
        }
        assert_eq!((cpu.y, ram.0[0x20], cpu.a, cpu.pc), (0x12, 0x35, 0x77, 0x407));
        // MUL and DIV.
        let (c, _) = run(&[0xE8, 0x10, 0x8D, 0x10, 0xCF, 0xCD, 0x30, 0x9E], 5);
        assert_eq!((c.a, c.y), ((0x100u16 / 0x30) as u8, (0x100u16 % 0x30) as u8));
    }
}
