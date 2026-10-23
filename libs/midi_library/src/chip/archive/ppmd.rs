//! PPMd variant H with 7z's range coder (7z method 03 04 01): the model
//! (contexts and their symbol statistics in one memory block, managed by
//! the variant's unit allocator, SEE escape estimation, binary contexts,
//! rescaling and model restarts) exactly as the variant defines it, since
//! a decoder must make the same model as the encoder did.
//!
//! Provenance: a Rust translation of PPMd var.H (PPMd by Dmitry Shkarin) as
//! laid out in the LZMA SDK's Ppmd7 by Igor Pavlov. Both are public domain
//! (the LZMA SDK is dedicated to the public domain by Igor Pavlov; PPMd var.H
//! was released into the public domain by Dmitry Shkarin).

const MAX_O: usize = 64;
const UNIT_SIZE: u32 = 12;
const INT_BITS: u32 = 7;
const PERIOD_BITS: u32 = 7;
const BIN_SCALE: u32 = 1 << (INT_BITS + PERIOD_BITS);
const MAX_FREQ: u32 = 124;
const NUM_INDEXES: usize = 4 + 4 + 4 + 26;
const TOP: u32 = 1 << 24;
const BOT: u32 = 1 << 15;

const INIT_BIN_ESC: [u16; 8] = [0x3CDD, 0x1F3F, 0x59BF, 0x48F3, 0x64A1, 0x5ABC, 0x6632, 0x6051];
const EXP_ESCAPE: [u8; 16] = [25, 14, 9, 7, 5, 5, 4, 4, 4, 3, 3, 3, 2, 2, 2, 2];

#[derive(Clone, Copy, Default)]
struct See {
    summ: u16,
    shift: u8,
    count: u8,
}

impl See {
    fn update(&mut self) {
        if (self.shift as u32) < PERIOD_BITS {
            self.count = self.count.wrapping_sub(1);
            if self.count == 0 {
                self.summ = self.summ.wrapping_shl(1);
                self.count = (3u32 << self.shift) as u8;
                self.shift += 1;
            }
        }
    }
}

/// A state outside the heap (CreateSuccessors' `upState`).
#[derive(Clone, Copy)]
struct State {
    symbol: u8,
    freq: u8,
    successor: u32,
}

struct Model {
    heap: Vec<u8>,
    size: u32,
    align: u32,
    min_context: u32,
    max_context: u32,
    found_state: u32,
    order_fall: u32,
    init_esc: u32,
    prev_success: u32,
    max_order: u32,
    hi_bits_flag: u32,
    run_length: i32,
    init_rl: i32,
    glue_count: u32,
    text: u32,
    lo_unit: u32,
    hi_unit: u32,
    units_start: u32,
    indx2units: [u8; NUM_INDEXES],
    units2indx: [u8; 128],
    free_list: [u32; NUM_INDEXES],
    ns2indx: [u8; 256],
    ns2bsindx: [u8; 256],
    hb2flag: [u8; 256],
    dummy_see: See,
    see: [[See; 16]; 25],
    bin_summ: [[u16; 64]; 128],
}

impl Model {
    fn new(size: u32) -> Model {
        let align = 4 - (size & 3);
        let mut m = Model {
            heap: vec![0u8; (align + size + UNIT_SIZE) as usize],
            size,
            align,
            min_context: 0,
            max_context: 0,
            found_state: 0,
            order_fall: 0,
            init_esc: 0,
            prev_success: 0,
            max_order: 0,
            hi_bits_flag: 0,
            run_length: 0,
            init_rl: 0,
            glue_count: 0,
            text: 0,
            lo_unit: 0,
            hi_unit: 0,
            units_start: 0,
            indx2units: [0; NUM_INDEXES],
            units2indx: [0; 128],
            free_list: [0; NUM_INDEXES],
            ns2indx: [0; 256],
            ns2bsindx: [0; 256],
            hb2flag: [0; 256],
            dummy_see: See::default(),
            see: [[See::default(); 16]; 25],
            bin_summ: [[0; 64]; 128],
        };
        let mut k = 0usize;
        for i in 0..NUM_INDEXES {
            let step = if i >= 12 { 4 } else { (i >> 2) + 1 };
            for _ in 0..step {
                m.units2indx[k] = i as u8;
                k += 1;
            }
            m.indx2units[i] = k as u8;
        }
        m.ns2bsindx[0] = 0;
        m.ns2bsindx[1] = 2;
        for v in m.ns2bsindx[2..11].iter_mut() {
            *v = 4;
        }
        for v in m.ns2bsindx[11..].iter_mut() {
            *v = 6;
        }
        for i in 0..3 {
            m.ns2indx[i] = i as u8;
        }
        let (mut mm, mut kk) = (3u32, 1u32);
        for i in 3..256 {
            m.ns2indx[i] = mm as u8;
            kk -= 1;
            if kk == 0 {
                mm += 1;
                kk = mm - 2;
            }
        }
        for v in m.hb2flag[0x40..].iter_mut() {
            *v = 8;
        }
        m
    }

    // Heap access.
    #[inline]
    fn u8_at(&self, at: u32) -> u8 {
        self.heap[at as usize]
    }
    #[inline]
    fn set_u8(&mut self, at: u32, v: u8) {
        self.heap[at as usize] = v;
    }
    #[inline]
    fn u16_at(&self, at: u32) -> u16 {
        let a = at as usize;
        u16::from_le_bytes([self.heap[a], self.heap[a + 1]])
    }
    #[inline]
    fn set_u16(&mut self, at: u32, v: u16) {
        let a = at as usize;
        self.heap[a..a + 2].copy_from_slice(&v.to_le_bytes());
    }
    #[inline]
    fn u32_at(&self, at: u32) -> u32 {
        let a = at as usize;
        u32::from_le_bytes([self.heap[a], self.heap[a + 1], self.heap[a + 2], self.heap[a + 3]])
    }
    #[inline]
    fn set_u32(&mut self, at: u32, v: u32) {
        let a = at as usize;
        self.heap[a..a + 4].copy_from_slice(&v.to_le_bytes());
    }

    // A context: NumStats u16, SummFreq u16, Stats u32, Suffix u32; its
    // one state (when NumStats is 1) sits at +2.
    fn num_stats(&self, c: u32) -> u32 {
        self.u16_at(c) as u32
    }
    fn set_num_stats(&mut self, c: u32, v: u32) {
        self.set_u16(c, v as u16);
    }
    fn summ_freq(&self, c: u32) -> u32 {
        self.u16_at(c + 2) as u32
    }
    fn set_summ_freq(&mut self, c: u32, v: u32) {
        self.set_u16(c + 2, v as u16);
    }
    fn stats(&self, c: u32) -> u32 {
        self.u32_at(c + 4)
    }
    fn set_stats(&mut self, c: u32, v: u32) {
        self.set_u32(c + 4, v);
    }
    fn suffix(&self, c: u32) -> u32 {
        self.u32_at(c + 8)
    }
    fn set_suffix(&mut self, c: u32, v: u32) {
        self.set_u32(c + 8, v);
    }
    fn one_state(c: u32) -> u32 {
        c + 2
    }

    // A state: Symbol u8, Freq u8, Successor (u16 low, u16 high).
    fn sym(&self, s: u32) -> u8 {
        self.u8_at(s)
    }
    fn freq(&self, s: u32) -> u32 {
        self.u8_at(s + 1) as u32
    }
    fn set_freq(&mut self, s: u32, v: u32) {
        self.set_u8(s + 1, v as u8);
    }
    fn successor(&self, s: u32) -> u32 {
        self.u16_at(s + 2) as u32 | (self.u16_at(s + 4) as u32) << 16
    }
    fn set_successor(&mut self, s: u32, v: u32) {
        self.set_u16(s + 2, v as u16);
        self.set_u16(s + 4, (v >> 16) as u16);
    }
    fn state(&self, s: u32) -> State {
        State { symbol: self.sym(s), freq: self.freq(s) as u8, successor: self.successor(s) }
    }
    fn put_state(&mut self, s: u32, st: State) {
        self.set_u8(s, st.symbol);
        self.set_u8(s + 1, st.freq);
        self.set_successor(s, st.successor);
    }
    fn swap_states(&mut self, a: u32, b: u32) {
        let (x, y) = (self.state(a), self.state(b));
        self.put_state(a, y);
        self.put_state(b, x);
    }

    fn i2u(&self, i: usize) -> u32 {
        self.indx2units[i] as u32
    }
    fn u2i(&self, nu: u32) -> usize {
        self.units2indx[nu as usize - 1] as usize
    }

    fn insert_node(&mut self, node: u32, indx: usize) {
        let head = self.free_list[indx];
        self.set_u32(node, head);
        self.free_list[indx] = node;
    }

    fn remove_node(&mut self, indx: usize) -> u32 {
        let node = self.free_list[indx];
        self.free_list[indx] = self.u32_at(node);
        node
    }

    fn split_block(&mut self, ptr: u32, old: usize, new: usize) {
        let nu = self.i2u(old) - self.i2u(new);
        let ptr = ptr + self.i2u(new) * UNIT_SIZE;
        let mut i = self.u2i(nu);
        if self.i2u(i) != nu {
            i -= 1;
            let k = self.i2u(i);
            self.insert_node(ptr + k * UNIT_SIZE, (nu - k - 1) as usize);
        }
        self.insert_node(ptr, i);
    }

    // A free node: Stamp u16, NU u16, Next u32, Prev u32.
    fn glue_free_blocks(&mut self) {
        let head = self.align + self.size;
        let mut n = head;
        self.glue_count = 255;
        for i in 0..NUM_INDEXES {
            let nu = self.i2u(i) as u16;
            let mut next = self.free_list[i];
            self.free_list[i] = 0;
            while next != 0 {
                let node = next;
                self.set_u32(node + 4, n);
                self.set_u32(n + 8, next);
                n = next;
                next = self.u32_at(node);
                self.set_u16(node, 0);
                self.set_u16(node + 2, nu);
            }
        }
        self.set_u16(head, 1);
        self.set_u32(head + 4, n);
        self.set_u32(n + 8, head);
        if self.lo_unit != self.hi_unit {
            self.set_u16(self.lo_unit, 1);
        }
        // Glue neighbours.
        while n != head {
            let node = n;
            let mut nu = self.u16_at(node + 2) as u32;
            loop {
                let node2 = node + nu * UNIT_SIZE;
                nu += self.u16_at(node2 + 2) as u32;
                if self.u16_at(node2) != 0 || nu >= 0x10000 {
                    break;
                }
                let (prev2, next2) = (self.u32_at(node2 + 8), self.u32_at(node2 + 4));
                self.set_u32(prev2 + 4, next2);
                self.set_u32(next2 + 8, prev2);
                self.set_u16(node + 2, nu as u16);
            }
            n = self.u32_at(node + 4);
        }
        // Back into the free lists.
        n = self.u32_at(head + 4);
        while n != head {
            let mut node = n;
            let next = self.u32_at(node + 4);
            let mut nu = self.u16_at(node + 2) as u32;
            while nu > 128 {
                self.insert_node(node, NUM_INDEXES - 1);
                nu -= 128;
                node += 128 * UNIT_SIZE;
            }
            let mut i = self.u2i(nu);
            if self.i2u(i) != nu {
                i -= 1;
                let k = self.i2u(i);
                self.insert_node(node + k * UNIT_SIZE, (nu - k - 1) as usize);
            }
            self.insert_node(node, i);
            n = next;
        }
    }

    fn alloc_units_rare(&mut self, indx: usize) -> u32 {
        if self.glue_count == 0 {
            self.glue_free_blocks();
            if self.free_list[indx] != 0 {
                return self.remove_node(indx);
            }
        }
        let mut i = indx;
        loop {
            i += 1;
            if i == NUM_INDEXES {
                let bytes = self.i2u(indx) * UNIT_SIZE;
                self.glue_count -= 1;
                return if self.units_start - self.text > bytes {
                    self.units_start -= bytes;
                    self.units_start
                } else {
                    0
                };
            }
            if self.free_list[i] != 0 {
                break;
            }
        }
        let r = self.remove_node(i);
        self.split_block(r, i, indx);
        r
    }

    fn alloc_units(&mut self, indx: usize) -> u32 {
        if self.free_list[indx] != 0 {
            return self.remove_node(indx);
        }
        let bytes = self.i2u(indx) * UNIT_SIZE;
        if bytes <= self.hi_unit - self.lo_unit {
            let r = self.lo_unit;
            self.lo_unit += bytes;
            return r;
        }
        self.alloc_units_rare(indx)
    }

    fn copy_units(&mut self, dest: u32, src: u32, nu: u32) {
        let n = (nu * UNIT_SIZE) as usize;
        self.heap.copy_within(src as usize..src as usize + n, dest as usize);
    }

    fn shrink_units(&mut self, old: u32, old_nu: u32, new_nu: u32) -> u32 {
        let i0 = self.u2i(old_nu);
        let i1 = self.u2i(new_nu);
        if i0 == i1 {
            return old;
        }
        if self.free_list[i1] != 0 {
            let ptr = self.remove_node(i1);
            self.copy_units(ptr, old, new_nu);
            self.insert_node(old, i0);
            return ptr;
        }
        self.split_block(old, i0, i1);
        old
    }

    fn restart(&mut self) {
        self.free_list = [0; NUM_INDEXES];
        self.text = self.align;
        self.hi_unit = self.text + self.size;
        self.lo_unit = self.hi_unit - self.size / 8 / UNIT_SIZE * 7 * UNIT_SIZE;
        self.units_start = self.lo_unit;
        self.glue_count = 0;
        self.order_fall = self.max_order;
        self.init_rl = -(self.max_order.min(12) as i32) - 1;
        self.run_length = self.init_rl;
        self.prev_success = 0;
        self.hi_unit -= UNIT_SIZE;
        self.min_context = self.hi_unit;
        self.max_context = self.hi_unit;
        let c = self.min_context;
        self.set_suffix(c, 0);
        self.set_num_stats(c, 256);
        self.set_summ_freq(c, 257);
        self.found_state = self.lo_unit;
        self.set_stats(c, self.lo_unit);
        for i in 0..256u32 {
            let s = self.lo_unit + i * 6;
            self.put_state(s, State { symbol: i as u8, freq: 1, successor: 0 });
        }
        self.lo_unit += (256 / 2) * UNIT_SIZE;
        for i in 0..128 {
            for k in 0..8 {
                let val = (BIN_SCALE - INIT_BIN_ESC[k] as u32 / (i as u32 + 2)) as u16;
                for m in (0..64).step_by(8) {
                    self.bin_summ[i][k + m] = val;
                }
            }
        }
        for i in 0..25 {
            for k in 0..16 {
                self.see[i][k] = See { summ: ((5 * i as u32 + 10) << (PERIOD_BITS - 4)) as u16, shift: (PERIOD_BITS - 4) as u8, count: 4 };
            }
        }
    }

    fn init(&mut self, max_order: u32) {
        self.max_order = max_order;
        self.restart();
        self.dummy_see = See { summ: 0, shift: PERIOD_BITS as u8, count: 64 };
    }

    fn create_successors(&mut self, skip: bool) -> u32 {
        let mut c = self.min_context;
        let up_branch = self.successor(self.found_state);
        let mut ps: [u32; MAX_O] = [0; MAX_O];
        let mut num = 0;
        if !skip {
            ps[num] = self.found_state;
            num += 1;
        }
        let fsym = self.sym(self.found_state);
        while self.suffix(c) != 0 {
            c = self.suffix(c);
            let s = if self.num_stats(c) != 1 {
                let mut s = self.stats(c);
                while self.sym(s) != fsym {
                    s += 6;
                }
                s
            } else {
                Self::one_state(c)
            };
            let successor = self.successor(s);
            if successor != up_branch {
                c = successor;
                if num == 0 {
                    return c;
                }
                break;
            }
            ps[num] = s;
            num += 1;
        }
        let up_symbol = self.u8_at(up_branch);
        let up_freq = if self.num_stats(c) == 1 {
            self.freq(Self::one_state(c))
        } else {
            let mut s = self.stats(c);
            while self.sym(s) != up_symbol {
                s += 6;
            }
            let cf = self.freq(s) - 1;
            let s0 = self.summ_freq(c).wrapping_sub(self.num_stats(c)).wrapping_sub(cf);
            1 + if 2 * cf <= s0 { (5 * cf > s0) as u32 } else { (2 * cf + 3 * s0 - 1) / (2 * s0) }
        };
        let up = State { symbol: up_symbol, freq: up_freq as u8, successor: up_branch + 1 };
        loop {
            let c1 = if self.hi_unit != self.lo_unit {
                self.hi_unit -= UNIT_SIZE;
                self.hi_unit
            } else if self.free_list[0] != 0 {
                self.remove_node(0)
            } else {
                let c1 = self.alloc_units_rare(0);
                if c1 == 0 {
                    return 0;
                }
                c1
            };
            self.set_num_stats(c1, 1);
            self.put_state(Self::one_state(c1), up);
            self.set_suffix(c1, c);
            num -= 1;
            self.set_successor(ps[num], c1);
            c = c1;
            if num == 0 {
                break;
            }
        }
        c
    }

    fn update_model(&mut self) {
        let fs = self.found_state;
        let fsym = self.sym(fs);
        let ffreq = self.freq(fs);
        let mut f_successor = self.successor(fs);
        if ffreq < MAX_FREQ / 4 && self.suffix(self.min_context) != 0 {
            let c = self.suffix(self.min_context);
            if self.num_stats(c) == 1 {
                let s = Self::one_state(c);
                if self.freq(s) < 32 {
                    self.set_freq(s, self.freq(s) + 1);
                }
            } else {
                let mut s = self.stats(c);
                if self.sym(s) != fsym {
                    loop {
                        s += 6;
                        if self.sym(s) == fsym {
                            break;
                        }
                    }
                    if self.freq(s) >= self.freq(s - 6) {
                        self.swap_states(s, s - 6);
                        s -= 6;
                    }
                }
                if self.freq(s) < MAX_FREQ - 9 {
                    self.set_freq(s, self.freq(s) + 2);
                    self.set_summ_freq(c, self.summ_freq(c) + 2);
                }
            }
        }
        if self.order_fall == 0 {
            let c = self.create_successors(true);
            self.min_context = c;
            self.max_context = c;
            if c == 0 {
                self.restart();
                return;
            }
            self.set_successor(self.found_state, c);
            return;
        }
        self.set_u8(self.text, fsym);
        self.text += 1;
        let mut successor = self.text;
        if self.text >= self.units_start {
            self.restart();
            return;
        }
        if f_successor != 0 {
            if f_successor <= successor {
                let cs = self.create_successors(false);
                if cs == 0 {
                    self.restart();
                    return;
                }
                f_successor = cs;
            }
            self.order_fall -= 1;
            if self.order_fall == 0 {
                successor = f_successor;
                if self.max_context != self.min_context {
                    self.text -= 1;
                }
            }
        } else {
            self.set_successor(self.found_state, successor);
            f_successor = self.min_context;
        }
        let ns = self.num_stats(self.min_context);
        let s0 = self.summ_freq(self.min_context).wrapping_sub(ns).wrapping_sub(self.freq(self.found_state).wrapping_sub(1));
        let mut c = self.max_context;
        while c != self.min_context {
            let ns1 = self.num_stats(c);
            if ns1 != 1 {
                if ns1 & 1 == 0 {
                    let old_nu = ns1 >> 1;
                    let i = self.u2i(old_nu);
                    if i != self.u2i(old_nu + 1) {
                        let ptr = self.alloc_units(i + 1);
                        if ptr == 0 {
                            self.restart();
                            return;
                        }
                        let old = self.stats(c);
                        self.copy_units(ptr, old, old_nu);
                        self.insert_node(old, i);
                        self.set_stats(c, ptr);
                    }
                }
                let sf = self.summ_freq(c);
                let add = (2 * ns1 < ns) as u32 + 2 * ((4 * ns1 <= ns) as u32 & (sf <= 8 * ns1) as u32);
                self.set_summ_freq(c, sf + add);
            } else {
                let s = self.alloc_units(0);
                if s == 0 {
                    self.restart();
                    return;
                }
                let st = self.state(Self::one_state(c));
                self.put_state(s, st);
                self.set_stats(c, s);
                let f = self.freq(s);
                self.set_freq(s, if f < MAX_FREQ / 4 - 1 { f << 1 } else { MAX_FREQ - 4 });
                let sf = self.freq(s) + self.init_esc + (ns > 3) as u32;
                self.set_summ_freq(c, sf);
            }
            let mut cf = 2 * self.freq(self.found_state) * (self.summ_freq(c) + 6);
            let sf = s0 + self.summ_freq(c);
            if cf < 6 * sf {
                cf = 1 + (cf > sf) as u32 + (cf >= 4 * sf) as u32;
                self.set_summ_freq(c, self.summ_freq(c) + 3);
            } else {
                cf = 4 + (cf >= 9 * sf) as u32 + (cf >= 12 * sf) as u32 + (cf >= 15 * sf) as u32;
                self.set_summ_freq(c, self.summ_freq(c) + cf);
            }
            let s = self.stats(c) + ns1 * 6;
            self.put_state(s, State { symbol: fsym, freq: cf as u8, successor });
            self.set_num_stats(c, ns1 + 1);
            c = self.suffix(c);
        }
        self.max_context = f_successor;
        self.min_context = f_successor;
    }

    fn rescale(&mut self) {
        let mc = self.min_context;
        let stats = self.stats(mc);
        let mut s = self.found_state;
        {
            let tmp = self.state(s);
            while s != stats {
                let prev = self.state(s - 6);
                self.put_state(s, prev);
                s -= 6;
            }
            self.put_state(s, tmp);
        }
        let mut esc_freq = self.summ_freq(mc) - self.freq(s);
        self.set_freq(s, self.freq(s) + 4);
        let adder = (self.order_fall != 0) as u32;
        self.set_freq(s, (self.freq(s) + adder) >> 1);
        let mut sum_freq = self.freq(s);
        let mut i = self.num_stats(mc) - 1;
        loop {
            s += 6;
            esc_freq -= self.freq(s);
            self.set_freq(s, (self.freq(s) + adder) >> 1);
            sum_freq += self.freq(s);
            if self.freq(s) > self.freq(s - 6) {
                let tmp = self.state(s);
                let mut s1 = s;
                loop {
                    let prev = self.state(s1 - 6);
                    self.put_state(s1, prev);
                    s1 -= 6;
                    if s1 == stats || tmp.freq as u32 <= self.freq(s1 - 6) {
                        break;
                    }
                }
                self.put_state(s1, tmp);
            }
            i -= 1;
            if i == 0 {
                break;
            }
        }
        if self.freq(s) == 0 {
            let num_stats = self.num_stats(mc);
            let mut i = 0;
            loop {
                i += 1;
                s -= 6;
                if self.freq(s) != 0 {
                    break;
                }
            }
            esc_freq += i;
            self.set_num_stats(mc, num_stats - i);
            if self.num_stats(mc) == 1 {
                let mut tmp = self.state(stats);
                loop {
                    tmp.freq -= tmp.freq >> 1;
                    esc_freq >>= 1;
                    if esc_freq <= 1 {
                        break;
                    }
                }
                let idx = self.u2i((num_stats + 1) >> 1);
                self.insert_node(stats, idx);
                self.found_state = Self::one_state(mc);
                self.put_state(self.found_state, tmp);
                return;
            }
            let n0 = (num_stats + 1) >> 1;
            let n1 = (self.num_stats(mc) + 1) >> 1;
            if n0 != n1 {
                let ns = self.shrink_units(stats, n0, n1);
                self.set_stats(mc, ns);
            }
        }
        self.set_summ_freq(mc, sum_freq + esc_freq - (esc_freq >> 1));
        self.found_state = self.stats(mc);
    }

    /// The SEE context for an escape from the current context, and the
    /// escape frequency it estimates.
    fn make_esc_freq(&mut self, num_masked: u32) -> (Option<(usize, usize)>, u32) {
        let mc = self.min_context;
        let ns = self.num_stats(mc);
        let non_masked = ns - num_masked;
        if ns != 256 {
            let suffix_ns = self.num_stats(self.suffix(mc));
            let i = self.ns2indx[(non_masked - 1) as usize] as usize;
            let k = (non_masked < suffix_ns.wrapping_sub(ns)) as usize + 2 * (self.summ_freq(mc) < 11 * ns) as usize + 4 * (num_masked > non_masked) as usize + self.hi_bits_flag as usize;
            let see = &mut self.see[i][k];
            let r = (see.summ >> see.shift) as u32;
            see.summ = see.summ.wrapping_sub(r as u16);
            (Some((i, k)), r + (r == 0) as u32)
        } else {
            (None, 1)
        }
    }

    fn next_context(&mut self) {
        let c = self.successor(self.found_state);
        if self.order_fall == 0 && c > self.text {
            self.min_context = c;
            self.max_context = c;
        } else {
            self.update_model();
        }
    }

    fn update1(&mut self) {
        let mut s = self.found_state;
        self.set_freq(s, self.freq(s) + 4);
        self.set_summ_freq(self.min_context, self.summ_freq(self.min_context) + 4);
        if self.freq(s) > self.freq(s - 6) {
            self.swap_states(s, s - 6);
            s -= 6;
            self.found_state = s;
            if self.freq(s) > MAX_FREQ {
                self.rescale();
            }
        }
        self.next_context();
    }

    fn update1_0(&mut self) {
        self.prev_success = (2 * self.freq(self.found_state) > self.summ_freq(self.min_context)) as u32;
        self.run_length += self.prev_success as i32;
        self.set_summ_freq(self.min_context, self.summ_freq(self.min_context) + 4);
        let f = self.freq(self.found_state) + 4;
        self.set_freq(self.found_state, f);
        if f > MAX_FREQ {
            self.rescale();
        }
        self.next_context();
    }

    fn update_bin(&mut self) {
        let f = self.freq(self.found_state);
        self.set_freq(self.found_state, f + (f < 128) as u32);
        self.prev_success = 1;
        self.run_length += 1;
        self.next_context();
    }

    fn update2(&mut self) {
        let s = self.found_state;
        self.set_freq(s, self.freq(s) + 4);
        self.set_summ_freq(self.min_context, self.summ_freq(self.min_context) + 4);
        if self.freq(s) > MAX_FREQ {
            self.rescale();
        }
        self.run_length = self.init_rl;
        self.update_model();
    }
}

/// 7z's range decoder for PPMd.
struct Rc<'a> {
    data: &'a [u8],
    at: usize,
    range: u32,
    code: u32,
}

impl Rc<'_> {
    fn byte(&mut self) -> u32 {
        let b = self.data.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        b as u32
    }
    fn threshold(&mut self, total: u32) -> u32 {
        self.range /= total;
        self.code / self.range
    }
    fn normalize(&mut self) {
        if self.range < TOP {
            self.code = (self.code << 8) | self.byte();
            self.range <<= 8;
            if self.range < TOP {
                self.code = (self.code << 8) | self.byte();
                self.range <<= 8;
            }
        }
    }
    fn decode(&mut self, start: u32, size: u32) {
        self.code = self.code.wrapping_sub(start.wrapping_mul(self.range));
        self.range = self.range.wrapping_mul(size);
        self.normalize();
    }
    fn bit(&mut self, size0: u32, total: u32) -> u32 {
        let bound = (self.range / total) * size0;
        let sym = if self.code < bound {
            self.range = bound;
            0
        } else {
            self.code -= bound;
            self.range -= bound;
            1
        };
        self.normalize();
        sym
    }
}

fn decode_symbol(p: &mut Model, rc: &mut Rc) -> i32 {
    let mut mask = [true; 256];
    let mc = p.min_context;
    if p.num_stats(mc) != 1 {
        let mut s = p.stats(mc);
        let count = rc.threshold(p.summ_freq(mc));
        let mut hi = p.freq(s);
        if count < hi {
            rc.decode(0, p.freq(s));
            p.found_state = s;
            let sym = p.sym(s);
            p.update1_0();
            return sym as i32;
        }
        p.prev_success = 0;
        let mut i = p.num_stats(mc) - 1;
        loop {
            s += 6;
            hi += p.freq(s);
            if hi > count {
                rc.decode(hi - p.freq(s), p.freq(s));
                p.found_state = s;
                let sym = p.sym(s);
                p.update1();
                return sym as i32;
            }
            i -= 1;
            if i == 0 {
                break;
            }
        }
        if count >= p.summ_freq(mc) {
            return -2;
        }
        p.hi_bits_flag = p.hb2flag[p.sym(p.found_state) as usize] as u32;
        rc.decode(hi, p.summ_freq(mc) - hi);
        mask[p.sym(s) as usize] = false;
        let mut i = p.num_stats(mc) - 1;
        loop {
            s -= 6;
            mask[p.sym(s) as usize] = false;
            i -= 1;
            if i == 0 {
                break;
            }
        }
    } else {
        let one = Model::one_state(mc);
        let suffix_ns = p.num_stats(p.suffix(mc));
        p.hi_bits_flag = p.hb2flag[p.sym(p.found_state) as usize] as u32;
        let row = (p.freq(one) - 1) as usize;
        let col = p.prev_success as usize + p.ns2bsindx[(suffix_ns - 1) as usize] as usize + p.hi_bits_flag as usize + 2 * p.hb2flag[p.sym(one) as usize] as usize + ((p.run_length >> 26) & 0x20) as usize;
        let prob = p.bin_summ[row][col] as u32;
        if rc.bit(prob, BIN_SCALE) == 0 {
            p.bin_summ[row][col] = (prob + (1 << INT_BITS) - ((prob + (1 << (PERIOD_BITS - 2))) >> PERIOD_BITS)) as u16;
            p.found_state = one;
            let sym = p.sym(one);
            p.update_bin();
            return sym as i32;
        }
        let prob = prob - ((prob + (1 << (PERIOD_BITS - 2))) >> PERIOD_BITS);
        p.bin_summ[row][col] = prob as u16;
        p.init_esc = EXP_ESCAPE[(prob >> 10) as usize] as u32;
        mask[p.sym(one) as usize] = false;
        p.prev_success = 0;
    }
    loop {
        let num_masked = p.num_stats(p.min_context);
        loop {
            p.order_fall += 1;
            let suffix = p.suffix(p.min_context);
            if suffix == 0 {
                return -1;
            }
            p.min_context = suffix;
            if p.num_stats(p.min_context) != num_masked {
                break;
            }
        }
        let mc = p.min_context;
        let mut hi = 0;
        let mut s = p.stats(mc);
        let num = p.num_stats(mc) - num_masked;
        let mut ps: Vec<u32> = Vec::with_capacity(num as usize);
        while (ps.len() as u32) != num {
            if mask[p.sym(s) as usize] {
                hi += p.freq(s);
                ps.push(s);
            }
            s += 6;
        }
        let (see, esc) = p.make_esc_freq(num_masked);
        let freq_sum = esc + hi;
        let count = rc.threshold(freq_sum);
        if count < hi {
            let mut acc = 0;
            let mut k = 0;
            loop {
                acc += p.freq(ps[k]);
                if acc > count {
                    break;
                }
                k += 1;
            }
            let s = ps[k];
            rc.decode(acc - p.freq(s), p.freq(s));
            match see {
                Some((i, j)) => p.see[i][j].update(),
                None => p.dummy_see.update(),
            }
            p.found_state = s;
            let sym = p.sym(s);
            p.update2();
            return sym as i32;
        }
        if count >= freq_sum {
            return -2;
        }
        rc.decode(hi, freq_sum - hi);
        match see {
            Some((i, j)) => p.see[i][j].summ = p.see[i][j].summ.wrapping_add(freq_sum as u16),
            None => p.dummy_see.summ = p.dummy_see.summ.wrapping_add(freq_sum as u16),
        }
        for &s in &ps {
            mask[p.sym(s) as usize] = false;
        }
    }
}

/// A 7z PPMd stream: `props` (the order, then the memory size as u32 LE),
/// `size` bytes out.
pub fn decode(props: &[u8], data: &[u8], size: usize) -> Result<Vec<u8>, String> {
    if props.len() < 5 {
        return Err("ppmd: bad properties".into());
    }
    let order = props[0] as u32;
    let mem = u32::from_le_bytes([props[1], props[2], props[3], props[4]]);
    if !(2..=64).contains(&order) || mem < (1 << 11) || mem > 0xFFFF_FFFF - 12 * 3 {
        return Err("ppmd: bad properties".into());
    }
    let mut p = Model::new(mem);
    p.init(order);
    let mut rc = Rc { data, at: 0, range: 0xFFFF_FFFF, code: 0 };
    if rc.byte() != 0 {
        return Err("ppmd: bad stream".into());
    }
    for _ in 0..4 {
        rc.code = (rc.code << 8) | rc.byte();
    }
    let _ = BOT;
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let sym = decode_symbol(&mut p, &mut rc);
        if sym < 0 {
            return Err(format!("ppmd: data error after {} bytes", out.len()));
        }
        out.push(sym as u8);
    }
    Ok(out)
}
