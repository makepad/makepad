//! The Game Boy's CPU (the Sharp LR35902: a Z80-like core with its own
//! loads and the CB bit ops), decoded by the opcode's x/y/z fields; cycle
//! counts are close, which is all a sound driver called per frame needs.

pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
}

const ZF: u8 = 0x80;
const NF: u8 = 0x40;
const HF: u8 = 0x20;
const CF: u8 = 0x10;

#[derive(Clone, Debug, Default)]
pub struct Lr35902 {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
    pub ime: bool,
    pub halted: bool,
    pub cycles: u64,
}

impl Lr35902 {
    fn hl(&self) -> u16 {
        (self.h as u16) << 8 | self.l as u16
    }
    fn set_hl(&mut self, v: u16) {
        self.h = (v >> 8) as u8;
        self.l = v as u8;
    }
    fn rp(&self, p: u8) -> u16 {
        match p {
            0 => (self.b as u16) << 8 | self.c as u16,
            1 => (self.d as u16) << 8 | self.e as u16,
            2 => self.hl(),
            _ => self.sp,
        }
    }
    fn set_rp(&mut self, p: u8, v: u16) {
        match p {
            0 => {
                self.b = (v >> 8) as u8;
                self.c = v as u8;
            }
            1 => {
                self.d = (v >> 8) as u8;
                self.e = v as u8;
            }
            2 => self.set_hl(v),
            _ => self.sp = v,
        }
    }
    fn rp2(&self, p: u8) -> u16 {
        if p == 3 { (self.a as u16) << 8 | self.f as u16 } else { self.rp(p) }
    }
    fn set_rp2(&mut self, p: u8, v: u16) {
        if p == 3 {
            self.a = (v >> 8) as u8;
            self.f = v as u8 & 0xF0;
        } else {
            self.set_rp(p, v);
        }
    }
    fn r(&mut self, bus: &mut dyn Bus, k: u8) -> u8 {
        match k {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => self.h,
            5 => self.l,
            6 => bus.read(self.hl()),
            _ => self.a,
        }
    }
    fn set_r(&mut self, bus: &mut dyn Bus, k: u8, v: u8) {
        match k {
            0 => self.b = v,
            1 => self.c = v,
            2 => self.d = v,
            3 => self.e = v,
            4 => self.h = v,
            5 => self.l = v,
            6 => bus.write(self.hl(), v),
            _ => self.a = v,
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
    pub fn push(&mut self, bus: &mut dyn Bus, v: u16) {
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, (v >> 8) as u8);
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, v as u8);
    }
    fn pop(&mut self, bus: &mut dyn Bus) -> u16 {
        let lo = bus.read(self.sp) as u16;
        self.sp = self.sp.wrapping_add(1);
        let hi = bus.read(self.sp) as u16;
        self.sp = self.sp.wrapping_add(1);
        hi << 8 | lo
    }
    fn cond(&self, y: u8) -> bool {
        match y & 3 {
            0 => self.f & ZF == 0,
            1 => self.f & ZF != 0,
            2 => self.f & CF == 0,
            _ => self.f & CF != 0,
        }
    }
    fn flags(&mut self, z: bool, n: bool, h: bool, c: bool) {
        self.f = if z { ZF } else { 0 } | if n { NF } else { 0 } | if h { HF } else { 0 } | if c { CF } else { 0 };
    }
    fn alu(&mut self, op: u8, v: u8) {
        let a = self.a;
        let carry = (self.f & CF != 0) as u8;
        match op {
            0 | 1 => {
                let c = if op == 1 { carry } else { 0 };
                let r = a as u16 + v as u16 + c as u16;
                self.flags(r as u8 == 0, false, (a & 15) + (v & 15) + c > 15, r > 0xFF);
                self.a = r as u8;
            }
            2 | 3 | 7 => {
                let c = if op == 3 { carry } else { 0 };
                let r = a as i16 - v as i16 - c as i16;
                self.flags(r as u8 == 0, true, ((a & 15) as i16 - (v & 15) as i16 - c as i16) < 0, r < 0);
                if op != 7 {
                    self.a = r as u8;
                }
            }
            4 => {
                self.a &= v;
                self.flags(self.a == 0, false, true, false);
            }
            5 => {
                self.a ^= v;
                self.flags(self.a == 0, false, false, false);
            }
            _ => {
                self.a |= v;
                self.flags(self.a == 0, false, false, false);
            }
        }
    }
    fn add_sp(&mut self, bus: &mut dyn Bus) -> u16 {
        let d = self.fetch(bus) as i8 as i16 as u16;
        let sp = self.sp;
        self.flags(false, false, (sp & 15) + (d & 15) > 15, (sp & 0xFF) + (d & 0xFF) > 0xFF);
        sp.wrapping_add(d)
    }

    /// Run one instruction; its cycles (in 4 MHz clocks).
    pub fn step(&mut self, bus: &mut dyn Bus) -> u32 {
        if self.halted {
            self.cycles += 4;
            return 4;
        }
        let op = self.fetch(bus);
        let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
        let (p, q) = (y >> 1, y & 1);
        let mut cyc = 4;
        match x {
            0 => match z {
                0 => match y {
                    0 => {}
                    1 => {
                        let a = self.fetch16(bus);
                        bus.write(a, self.sp as u8);
                        bus.write(a.wrapping_add(1), (self.sp >> 8) as u8);
                        cyc = 20;
                    }
                    2 => {
                        self.fetch(bus);
                    }
                    _ => {
                        let d = self.fetch(bus) as i8;
                        if y == 3 || self.cond(y - 4) {
                            self.pc = self.pc.wrapping_add(d as u16);
                            cyc = 12;
                        } else {
                            cyc = 8;
                        }
                    }
                },
                1 => {
                    if q == 0 {
                        let v = self.fetch16(bus);
                        self.set_rp(p, v);
                        cyc = 12;
                    } else {
                        let hl = self.hl();
                        let v = self.rp(p);
                        let r = hl as u32 + v as u32;
                        let z = self.f & ZF;
                        self.f = z | if (hl & 0xFFF) + (v & 0xFFF) > 0xFFF { HF } else { 0 } | if r > 0xFFFF { CF } else { 0 };
                        self.set_hl(r as u16);
                        cyc = 8;
                    }
                }
                2 => {
                    let addr = match p {
                        0 => self.rp(0),
                        1 => self.rp(1),
                        _ => {
                            let hl = self.hl();
                            self.set_hl(if p == 2 { hl.wrapping_add(1) } else { hl.wrapping_sub(1) });
                            hl
                        }
                    };
                    if q == 0 {
                        bus.write(addr, self.a);
                    } else {
                        self.a = bus.read(addr);
                    }
                    cyc = 8;
                }
                3 => {
                    let v = self.rp(p);
                    self.set_rp(p, if q == 0 { v.wrapping_add(1) } else { v.wrapping_sub(1) });
                    cyc = 8;
                }
                4 | 5 => {
                    let v = self.r(bus, y);
                    let r = if z == 4 { v.wrapping_add(1) } else { v.wrapping_sub(1) };
                    let c = self.f & CF;
                    self.f = c | if r == 0 { ZF } else { 0 } | if z == 5 { NF } else { 0 } | if (z == 4 && v & 15 == 15) || (z == 5 && v & 15 == 0) { HF } else { 0 };
                    self.set_r(bus, y, r);
                    cyc = if y == 6 { 12 } else { 4 };
                }
                6 => {
                    let v = self.fetch(bus);
                    self.set_r(bus, y, v);
                    cyc = if y == 6 { 12 } else { 8 };
                }
                _ => {
                    let a = self.a;
                    let c = self.f & CF != 0;
                    match y {
                        0 => {
                            self.a = a.rotate_left(1);
                            self.flags(false, false, false, a & 0x80 != 0);
                        }
                        1 => {
                            self.a = a.rotate_right(1);
                            self.flags(false, false, false, a & 1 != 0);
                        }
                        2 => {
                            self.a = (a << 1) | c as u8;
                            self.flags(false, false, false, a & 0x80 != 0);
                        }
                        3 => {
                            self.a = (a >> 1) | ((c as u8) << 7);
                            self.flags(false, false, false, a & 1 != 0);
                        }
                        4 => {
                            let mut v = a;
                            let mut carry = c;
                            if self.f & NF == 0 {
                                if carry || v > 0x99 {
                                    v = v.wrapping_add(0x60);
                                    carry = true;
                                }
                                if self.f & HF != 0 || v & 15 > 9 {
                                    v = v.wrapping_add(6);
                                }
                            } else {
                                if carry {
                                    v = v.wrapping_sub(0x60);
                                }
                                if self.f & HF != 0 {
                                    v = v.wrapping_sub(6);
                                }
                            }
                            self.a = v;
                            let n = self.f & NF;
                            self.f = n | if v == 0 { ZF } else { 0 } | if carry { CF } else { 0 };
                        }
                        5 => {
                            self.a = !a;
                            self.f |= NF | HF;
                        }
                        6 => self.f = (self.f & ZF) | CF,
                        _ => self.f = (self.f & ZF) | if c { 0 } else { CF },
                    }
                }
            },
            1 => {
                if z == 6 && y == 6 {
                    self.halted = true;
                } else {
                    let v = self.r(bus, z);
                    self.set_r(bus, y, v);
                    cyc = if z == 6 || y == 6 { 8 } else { 4 };
                }
            }
            2 => {
                let v = self.r(bus, z);
                self.alu(y, v);
                cyc = if z == 6 { 8 } else { 4 };
            }
            _ => match z {
                0 => match y {
                    0..=3 => {
                        if self.cond(y) {
                            self.pc = self.pop(bus);
                            cyc = 20;
                        } else {
                            cyc = 8;
                        }
                    }
                    4 => {
                        let n = self.fetch(bus);
                        bus.write(0xFF00 | n as u16, self.a);
                        cyc = 12;
                    }
                    5 => {
                        self.sp = self.add_sp(bus);
                        cyc = 16;
                    }
                    6 => {
                        let n = self.fetch(bus);
                        self.a = bus.read(0xFF00 | n as u16);
                        cyc = 12;
                    }
                    _ => {
                        let v = self.add_sp(bus);
                        self.set_hl(v);
                        cyc = 12;
                    }
                },
                1 => {
                    if q == 0 {
                        let v = self.pop(bus);
                        self.set_rp2(p, v);
                        cyc = 12;
                    } else {
                        match p {
                            0 | 1 => {
                                self.pc = self.pop(bus);
                                if p == 1 {
                                    self.ime = true;
                                }
                                cyc = 16;
                            }
                            2 => self.pc = self.hl(),
                            _ => {
                                self.sp = self.hl();
                                cyc = 8;
                            }
                        }
                    }
                }
                2 => match y {
                    0..=3 => {
                        let a = self.fetch16(bus);
                        if self.cond(y) {
                            self.pc = a;
                            cyc = 16;
                        } else {
                            cyc = 12;
                        }
                    }
                    4 => {
                        bus.write(0xFF00 | self.c as u16, self.a);
                        cyc = 8;
                    }
                    5 => {
                        let a = self.fetch16(bus);
                        bus.write(a, self.a);
                        cyc = 16;
                    }
                    6 => {
                        self.a = bus.read(0xFF00 | self.c as u16);
                        cyc = 8;
                    }
                    _ => {
                        let a = self.fetch16(bus);
                        self.a = bus.read(a);
                        cyc = 16;
                    }
                },
                3 => match y {
                    0 => {
                        self.pc = self.fetch16(bus);
                        cyc = 16;
                    }
                    1 => cyc = self.cb(bus),
                    6 => self.ime = false,
                    7 => self.ime = true,
                    _ => {}
                },
                4 => {
                    let a = self.fetch16(bus);
                    if y < 4 && self.cond(y) {
                        let pc = self.pc;
                        self.push(bus, pc);
                        self.pc = a;
                        cyc = 24;
                    } else {
                        cyc = 12;
                    }
                }
                5 => {
                    if q == 0 {
                        let v = self.rp2(p);
                        self.push(bus, v);
                        cyc = 16;
                    } else if p == 0 {
                        let a = self.fetch16(bus);
                        let pc = self.pc;
                        self.push(bus, pc);
                        self.pc = a;
                        cyc = 24;
                    }
                }
                6 => {
                    let v = self.fetch(bus);
                    self.alu(y, v);
                    cyc = 8;
                }
                _ => {
                    let pc = self.pc;
                    self.push(bus, pc);
                    self.pc = y as u16 * 8;
                    cyc = 16;
                }
            },
        }
        self.cycles += cyc as u64;
        cyc
    }

    fn cb(&mut self, bus: &mut dyn Bus) -> u32 {
        let op = self.fetch(bus);
        let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
        let v = self.r(bus, z);
        let c = self.f & CF != 0;
        match x {
            0 => {
                let (r, out) = match y {
                    0 => (v.rotate_left(1), v & 0x80 != 0),
                    1 => (v.rotate_right(1), v & 1 != 0),
                    2 => ((v << 1) | c as u8, v & 0x80 != 0),
                    3 => ((v >> 1) | ((c as u8) << 7), v & 1 != 0),
                    4 => (v << 1, v & 0x80 != 0),
                    5 => ((v >> 1) | (v & 0x80), v & 1 != 0),
                    6 => (v.rotate_left(4), false),
                    _ => (v >> 1, v & 1 != 0),
                };
                self.flags(r == 0, false, false, out);
                self.set_r(bus, z, r);
            }
            1 => {
                let cf = self.f & CF;
                self.f = cf | HF | if v & (1 << y) == 0 { ZF } else { 0 };
            }
            2 => self.set_r(bus, z, v & !(1 << y)),
            _ => self.set_r(bus, z, v | (1 << y)),
        }
        if z == 6 { 16 } else { 8 }
    }
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

    #[test]
    fn loads_arithmetic_loops_and_calls() {
        // LD A,0; LD B,10; loop: ADD A,B; DEC B; JR NZ,loop; LD (C000),A
        let code = [0x3E, 0x00, 0x06, 0x0A, 0x80, 0x05, 0x20, 0xFC, 0xEA, 0x00, 0xC0];
        let mut ram = Ram(vec![0; 65536]);
        ram.0[0x100..0x100 + code.len()].copy_from_slice(&code);
        let mut cpu = Lr35902 { pc: 0x100, sp: 0xFFFE, ..Default::default() };
        while cpu.pc < 0x100 + code.len() as u16 {
            cpu.step(&mut ram);
        }
        assert_eq!(ram.0[0xC000], 55);
        // CALL / RET, SWAP, BIT.
        let code = [0xCD, 0x10, 0x02, 0x76];
        let mut ram = Ram(vec![0; 65536]);
        ram.0[0x200..0x204].copy_from_slice(&code);
        ram.0[0x210..0x215].copy_from_slice(&[0x3E, 0x12, 0xCB, 0x37, 0xC9]);
        let mut cpu = Lr35902 { pc: 0x200, sp: 0xFFFE, ..Default::default() };
        for _ in 0..5 {
            cpu.step(&mut ram);
        }
        assert_eq!((cpu.a, cpu.halted), (0x21, true));
    }
}
