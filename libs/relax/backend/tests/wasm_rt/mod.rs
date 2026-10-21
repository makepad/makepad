//! A small WebAssembly interpreter for the wasm backend's tests: it decodes and runs the
//! subset the backend emits (MVP numerics, sign-extension, saturating truncation, bulk memory,
//! multi-value, exceptions with try_table, a SIMD128 subset), checks the section layout and
//! traps on what it does not know. Test code only: no third-party runtime.

#![allow(dead_code)]

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Trap {
    Trap(String),
    Exception,
}

fn t<T>(m: &str) -> Result<T, Trap> {
    Err(Trap::Trap(m.to_string()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct FType {
    pub params: Vec<u8>,
    pub results: Vec<u8>,
}

pub type HostFn = Box<dyn FnMut(&mut Vec<u8>, &[u64]) -> Result<Vec<u64>, Trap>>;

pub struct Func {
    ty: u32,
    locals: Vec<u8>,
    code: std::rc::Rc<Vec<u8>>,
    /// block/loop/if/try_table start (after the opcode) -> (else pc or 0, end pc)
    ctl: std::rc::Rc<HashMap<usize, (usize, usize)>>,
}

pub struct Instance {
    pub types: Vec<FType>,
    pub imports: Vec<(String, String, u32)>,
    pub funcs: Vec<Func>,
    pub table: Vec<Option<u32>>,
    pub mem: Vec<u8>,
    pub globals: Vec<u64>,
    pub exports: HashMap<String, (u8, u32)>,
    pub host: Vec<Option<HostFn>>,
    pub steps: u64,
    pub max_steps: u64,
    depth: u32,
}

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn u8(&mut self) -> Result<u8, Trap> {
        let v = *self.b.get(self.p).ok_or(Trap::Trap("eof".into()))?;
        self.p += 1;
        Ok(v)
    }
    fn u(&mut self) -> Result<u64, Trap> {
        let mut r = 0u64;
        let mut s = 0;
        loop {
            let b = self.u8()?;
            r |= ((b & 0x7f) as u64) << s;
            s += 7;
            if b & 0x80 == 0 {
                return Ok(r);
            }
        }
    }
    fn s(&mut self) -> Result<i64, Trap> {
        let mut r = 0i64;
        let mut s = 0;
        loop {
            let b = self.u8()?;
            r |= ((b & 0x7f) as i64) << s;
            s += 7;
            if b & 0x80 == 0 {
                if s < 64 && b & 0x40 != 0 {
                    r |= -1i64 << s;
                }
                return Ok(r);
            }
        }
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Trap> {
        let r = self.b.get(self.p..self.p + n).ok_or(Trap::Trap("eof".into()))?;
        self.p += n;
        Ok(r)
    }
    fn name(&mut self) -> Result<String, Trap> {
        let n = self.u()? as usize;
        Ok(String::from_utf8_lossy(self.bytes(n)?).to_string())
    }
}

/// Skips one instruction's immediates (after its opcode `op`).
fn skip_imm(r: &mut Rd, op: u8) -> Result<(), Trap> {
    match op {
        0x02 | 0x03 | 0x04 | 0x06 => {
            r.s()?;
        }
        0x1F => {
            r.s()?;
            let n = r.u()?;
            for _ in 0..n {
                let k = r.u8()?;
                if k < 2 {
                    r.u()?;
                }
                r.u()?;
            }
        }
        0x08 | 0x0C | 0x0D | 0x10 | 0x20 | 0x21 | 0x22 | 0x23 | 0x24 => {
            r.u()?;
        }
        0x0E => {
            let n = r.u()?;
            for _ in 0..=n {
                r.u()?;
            }
        }
        0x11 => {
            r.u()?;
            r.u()?;
        }
        0x28..=0x3E => {
            r.u()?;
            r.u()?;
        }
        0x3F | 0x40 => {
            r.u8()?;
        }
        0x41 | 0x42 => {
            r.s()?;
        }
        0x43 => {
            r.bytes(4)?;
        }
        0x44 => {
            r.bytes(8)?;
        }
        0xFC => {
            let k = r.u()?;
            match k {
                8 => {
                    r.u()?;
                    r.u8()?;
                }
                9 => {
                    r.u()?;
                }
                10 => {
                    r.u8()?;
                    r.u8()?;
                }
                11 => {
                    r.u8()?;
                }
                _ => {}
            }
        }
        0xFD => {
            let k = r.u()?;
            match k {
                0 | 11 => {
                    r.u()?;
                    r.u()?;
                }
                12 | 13 => {
                    r.bytes(16)?;
                }
                21..=34 => {
                    r.u8()?;
                }
                _ => {}
            }
        }
        _ => {}
    }
    Ok(())
}

fn ctl_map(code: &[u8], start: usize) -> Result<HashMap<usize, (usize, usize)>, Trap> {
    let mut m = HashMap::new();
    let mut st: Vec<(usize, usize)> = Vec::new();
    let mut r = Rd { b: code, p: start };
    while r.p < code.len() {
        let op = r.u8()?;
        let here = r.p;
        match op {
            0x02 | 0x03 | 0x04 | 0x1F | 0x06 => {
                st.push((here, 0));
            }
            0x05 | 0x19 => {
                st.last_mut().ok_or(Trap::Trap("else".into()))?.1 = here;
            }
            0x0B => {
                if let Some((s, e)) = st.pop() {
                    m.insert(s, (e, here));
                }
            }
            _ => {}
        }
        skip_imm(&mut r, op)?;
    }
    Ok(m)
}

impl Instance {
    pub fn new(bytes: &[u8]) -> Result<Instance, Trap> {
        let mut r = Rd { b: bytes, p: 0 };
        if r.bytes(8)? != [0, 0x61, 0x73, 0x6D, 1, 0, 0, 0] {
            return t("magic");
        }
        let mut inst = Instance { types: vec![], imports: vec![], funcs: vec![], table: vec![], mem: vec![], globals: vec![], exports: HashMap::new(), host: vec![], steps: 0, max_steps: 50_000_000, depth: 0 };
        let mut fty: Vec<u32> = vec![];
        let mut last_id = 0u8;
        let mut datas: Vec<(Option<u64>, Vec<u8>)> = vec![];
        let order = |id: u8| -> u8 {
            match id {
                1 => 1,
                2 => 2,
                3 => 3,
                4 => 4,
                5 => 5,
                13 => 6,
                6 => 7,
                7 => 8,
                8 => 9,
                9 => 10,
                12 => 11,
                10 => 12,
                11 => 13,
                _ => 0,
            }
        };
        while r.p < bytes.len() {
            let id = r.u8()?;
            let n = r.u()? as usize;
            let body = r.bytes(n)?;
            if id != 0 {
                if order(id) <= last_id {
                    return t(&format!("section {} out of order", id));
                }
                last_id = order(id);
            }
            let mut s = Rd { b: body, p: 0 };
            match id {
                0 => {}
                1 => {
                    for _ in 0..s.u()? {
                        if s.u8()? != 0x60 {
                            return t("functype");
                        }
                        let np = s.u()? as usize;
                        let params = s.bytes(np)?.to_vec();
                        let nr = s.u()? as usize;
                        let results = s.bytes(nr)?.to_vec();
                        inst.types.push(FType { params, results });
                    }
                }
                2 => {
                    for _ in 0..s.u()? {
                        let m = s.name()?;
                        let f = s.name()?;
                        let k = s.u8()?;
                        let ti = s.u()? as u32;
                        if k != 0 {
                            return t("import kind");
                        }
                        inst.imports.push((m, f, ti));
                        inst.host.push(None);
                    }
                }
                3 => {
                    for _ in 0..s.u()? {
                        fty.push(s.u()? as u32);
                    }
                }
                4 => {
                    s.u()?;
                    s.u8()?;
                    s.u8()?;
                    let n = s.u()? as usize;
                    inst.table = vec![None; n];
                }
                5 => {
                    s.u()?;
                    let _fl = s.u8()?;
                    let pages = s.u()? as usize;
                    inst.mem = vec![0; pages * 65536];
                }
                13 => {}
                6 => {
                    for _ in 0..s.u()? {
                        s.u8()?;
                        s.u8()?;
                        let op = s.u8()?;
                        let v = s.s()?;
                        s.u8()?;
                        inst.globals.push(if op == 0x41 { v as i32 as u32 as u64 } else { v as u64 });
                    }
                }
                7 => {
                    for _ in 0..s.u()? {
                        let n = s.name()?;
                        let k = s.u8()?;
                        let i = s.u()? as u32;
                        inst.exports.insert(n, (k, i));
                    }
                }
                9 => {
                    for _ in 0..s.u()? {
                        if s.u()? != 0 {
                            return t("elem flags");
                        }
                        s.u8()?;
                        let off = s.s()? as usize;
                        s.u8()?;
                        for k in 0..s.u()? as usize {
                            let f = s.u()? as u32;
                            inst.table[off + k] = Some(f);
                        }
                    }
                }
                12 => {}
                10 => {
                    let n = s.u()? as usize;
                    if n != fty.len() {
                        return t("code count");
                    }
                    for k in 0..n {
                        let sz = s.u()? as usize;
                        let fb = s.bytes(sz)?;
                        let mut q = Rd { b: fb, p: 0 };
                        let mut locals = vec![];
                        for _ in 0..q.u()? {
                            let c = q.u()?;
                            let ty = q.u8()?;
                            for _ in 0..c {
                                locals.push(ty);
                            }
                        }
                        let code = fb.to_vec();
                        let ctl = ctl_map(&code, q.p)?;
                        let start = q.p;
                        let mut ctl = ctl;
                        ctl.insert(usize::MAX, (start, 0));
                        inst.funcs.push(Func { ty: fty[k], locals, code: std::rc::Rc::new(code), ctl: std::rc::Rc::new(ctl) });
                    }
                }
                11 => {
                    for _ in 0..s.u()? {
                        let fl = s.u()?;
                        let addr = if fl == 0 {
                            let op = s.u8()?;
                            let v = s.s()?;
                            s.u8()?;
                            Some(if op == 0x41 { v as i32 as u32 as u64 } else { v as u64 })
                        } else {
                            None
                        };
                        let n = s.u()? as usize;
                        datas.push((addr, s.bytes(n)?.to_vec()));
                    }
                }
                _ => return t(&format!("section {}", id)),
            }
        }
        for (a, d) in datas {
            if let Some(a) = a {
                let a = a as usize;
                if a + d.len() > inst.mem.len() {
                    return t("data out of memory");
                }
                inst.mem[a..a + d.len()].copy_from_slice(&d);
            }
        }
        Ok(inst)
    }

    pub fn bind(&mut self, m: &str, f: &str, h: HostFn) {
        for (k, (mm, ff, _)) in self.imports.iter().enumerate() {
            if mm == m && ff == f {
                self.host[k] = Some(h);
                return;
            }
        }
    }

    pub fn call_export(&mut self, name: &str, args: &[u64]) -> Result<Vec<u64>, Trap> {
        let (k, i) = *self.exports.get(name).ok_or(Trap::Trap(format!("no export {}", name)))?;
        if k != 0 {
            return t("not a function");
        }
        let mut st: Vec<u128> = args.iter().map(|a| *a as u128).collect();
        self.invoke(i, &mut st)?;
        Ok(st.into_iter().map(|v| v as u64).collect())
    }

    fn ftype(&self, f: u32) -> FType {
        let ni = self.imports.len() as u32;
        if f < ni {
            self.types[self.imports[f as usize].2 as usize].clone()
        } else {
            self.types[self.funcs[(f - ni) as usize].ty as usize].clone()
        }
    }

    fn invoke(&mut self, f: u32, st: &mut Vec<u128>) -> Result<(), Trap> {
        let ty = self.ftype(f);
        let np = ty.params.len();
        if st.len() < np {
            return t("stack underflow at call");
        }
        let args: Vec<u128> = st.split_off(st.len() - np);
        let ni = self.imports.len() as u32;
        if f < ni {
            let mut h = self.host[f as usize].take().ok_or(Trap::Trap(format!("unbound import {}.{}", self.imports[f as usize].0, self.imports[f as usize].1)))?;
            let a64: Vec<u64> = args.iter().map(|a| *a as u64).collect();
            let r = h(&mut self.mem, &a64);
            self.host[f as usize] = Some(h);
            let r = r?;
            if r.len() != ty.results.len() {
                return t("host result count");
            }
            st.extend(r.into_iter().map(|v| v as u128));
            return Ok(());
        }
        self.depth += 1;
        if self.depth > 3000 {
            self.depth -= 1;
            return t("call stack exhausted");
        }
        let r = self.run(f - ni, args);
        self.depth -= 1;
        let res = r?;
        st.extend(res);
        Ok(())
    }

    fn ea(&self, base: u128, off: u64, n: usize) -> Result<usize, Trap> {
        let a = (base as u64 as u32 as u64).wrapping_add(off) as usize;
        if a + n > self.mem.len() {
            return t("out of bounds memory access");
        }
        Ok(a)
    }

    fn run(&mut self, fi: u32, args: Vec<u128>) -> Result<Vec<u128>, Trap> {
        let ty = self.types[self.funcs[fi as usize].ty as usize].clone();
        let mut locals: Vec<u128> = args;
        locals.extend(std::iter::repeat(0).take(self.funcs[fi as usize].locals.len()));
        let code = self.funcs[fi as usize].code.clone();
        let ctl = self.funcs[fi as usize].ctl.clone();
        self.exec(&code, &ctl, &mut locals, ty.results.len())
    }

    fn exec(&mut self, code: &[u8], ctl: &HashMap<usize, (usize, usize)>, l: &mut Vec<u128>, nres: usize) -> Result<Vec<u128>, Trap> {
        // label: (kind 0 block/if/try 1 loop, branch target pc, stack height, try catch label or MAX)
        struct Lab {
            lp: bool,
            target: usize,
            h: usize,
            catch: Option<u32>,
            /// legacy try: the catch_all body's pc
            legacy: usize,
        }
        let mut labs: Vec<Lab> = Vec::new();
        let mut st: Vec<u128> = Vec::new();
        let mut r = Rd { b: code, p: ctl[&usize::MAX].0 };
        macro_rules! pop {
            () => {
                st.pop().ok_or(Trap::Trap("stack underflow".into()))?
            };
        }
        macro_rules! bin64 {
            ($f:expr) => {{
                let b = pop!() as u64;
                let a = pop!() as u64;
                let f: fn(u64, u64) -> Result<u64, Trap> = $f;
                st.push(f(a, b)? as u128);
            }};
        }
        macro_rules! bin32 {
            ($f:expr) => {{
                let b = pop!() as u32;
                let a = pop!() as u32;
                let f: fn(u32, u32) -> Result<u32, Trap> = $f;
                st.push(f(a, b)? as u128);
            }};
        }
        macro_rules! f64b {
            ($f:expr) => {{
                let b = f64::from_bits(pop!() as u64);
                let a = f64::from_bits(pop!() as u64);
                let f: fn(f64, f64) -> f64 = $f;
                st.push(f(a, b).to_bits() as u128);
            }};
        }
        macro_rules! f32b {
            ($f:expr) => {{
                let b = f32::from_bits(pop!() as u32);
                let a = f32::from_bits(pop!() as u32);
                let f: fn(f32, f32) -> f32 = $f;
                st.push(f(a, b).to_bits() as u128);
            }};
        }
        macro_rules! f64c {
            ($f:expr) => {{
                let b = f64::from_bits(pop!() as u64);
                let a = f64::from_bits(pop!() as u64);
                let f: fn(f64, f64) -> bool = $f;
                st.push(f(a, b) as u128);
            }};
        }
        macro_rules! f32c {
            ($f:expr) => {{
                let b = f32::from_bits(pop!() as u32);
                let a = f32::from_bits(pop!() as u32);
                let f: fn(f32, f32) -> bool = $f;
                st.push(f(a, b) as u128);
            }};
        }
        // branch to label depth d
        macro_rules! branch {
            ($d:expr) => {{
                let d = $d as usize;
                if d >= labs.len() {
                    // the function body's label: return
                    let res = st.split_off(st.len() - nres);
                    return Ok(res);
                }
                let k = labs.len() - 1 - d;
                let lab = &labs[k];
                st.truncate(lab.h);
                r.p = lab.target;
                if lab.lp {
                    labs.truncate(k + 1);
                } else {
                    labs.truncate(k);
                }
            }};
        }
        loop {
            self.steps += 1;
            if self.steps > self.max_steps {
                return t("step limit");
            }
            if r.p >= code.len() {
                return t("ran off the code");
            }
            let op = r.u8()?;
            let here = r.p;
            match op {
                0x00 => return t("unreachable"),
                0x01 => {}
                0x02 | 0x03 | 0x04 => {
                    let bt = r.s()?;
                    if bt != -64 {
                        return t("block type");
                    }
                    let (els, end) = ctl[&here];
                    if op == 0x04 {
                        let c = pop!() as u32;
                        labs.push(Lab { lp: false, target: end, h: st.len(), catch: None, legacy: 0 });
                        if c == 0 {
                            if els != 0 {
                                r.p = els;
                            } else {
                                r.p = end;
                                labs.pop();
                            }
                        }
                    } else if op == 0x03 {
                        labs.push(Lab { lp: true, target: r.p, h: st.len(), catch: None, legacy: 0 });
                    } else {
                        labs.push(Lab { lp: false, target: end, h: st.len(), catch: None, legacy: 0 });
                    }
                }
                0x1F => {
                    let bt = r.s()?;
                    if bt != -64 {
                        return t("try block type");
                    }
                    let n = r.u()?;
                    let mut catch = None;
                    for _ in 0..n {
                        let k = r.u8()?;
                        if k != 2 {
                            return t("only catch_all");
                        }
                        catch = Some(r.u()? as u32);
                    }
                    let (_, end) = ctl[&here];
                    labs.push(Lab { lp: false, target: end, h: st.len(), catch, legacy: 0 });
                }
                0x06 => {
                    if r.s()? != -64 {
                        return t("try block type");
                    }
                    let (cat, end) = ctl[&here];
                    if cat == 0 {
                        return t("try without catch_all");
                    }
                    labs.push(Lab { lp: false, target: end, h: st.len(), catch: None, legacy: cat });
                }
                0x05 | 0x19 => {
                    // end of the then arm / try body: skip to after end
                    let lab = labs.pop().ok_or(Trap::Trap("else".into()))?;
                    r.p = lab.target;
                }
                0x0B => {
                    if labs.pop().is_none() {
                        let res = st.split_off(st.len().checked_sub(nres).ok_or(Trap::Trap("results".into()))?);
                        return Ok(res);
                    }
                }
                0x08 => {
                    r.u()?;
                    // find the innermost try in this frame
                    let e = Trap::Exception;
                    match labs.iter().rposition(|x| x.catch.is_some() || x.legacy != 0) {
                        Some(k) if labs[k].legacy != 0 => {
                            let h = labs[k].h;
                            let cat = labs[k].legacy;
                            labs.truncate(k + 1);
                            labs[k].legacy = 0;
                            st.truncate(h);
                            r.p = cat;
                        }
                        Some(k) => {
                            let c = labs[k].catch.unwrap();
                            let h = labs[k].h;
                            labs.truncate(k);
                            st.truncate(h);
                            branch!(c);
                        }
                        None => return Err(e),
                    }
                }
                0x0C => {
                    let d = r.u()?;
                    branch!(d);
                }
                0x0D => {
                    let d = r.u()?;
                    if pop!() as u32 != 0 {
                        branch!(d);
                    }
                }
                0x0E => {
                    let n = r.u()? as usize;
                    let mut v = Vec::with_capacity(n + 1);
                    for _ in 0..=n {
                        v.push(r.u()?);
                    }
                    let i = pop!() as u32 as usize;
                    let d = if i < n { v[i] } else { v[n] };
                    branch!(d);
                }
                0x0F => {
                    let res = st.split_off(st.len() - nres);
                    return Ok(res);
                }
                0x10 | 0x11 => {
                    let callee = if op == 0x10 {
                        r.u()? as u32
                    } else {
                        let ti = r.u()? as u32;
                        r.u()?;
                        let i = pop!() as u32 as usize;
                        let f = *self.table.get(i).ok_or(Trap::Trap("table index".into()))?;
                        let f = f.ok_or(Trap::Trap("null table entry".into()))?;
                        if self.ftype(f) != self.types[ti as usize] {
                            return t("indirect call signature mismatch");
                        }
                        f
                    };
                    match self.invoke(callee, &mut st) {
                        Ok(()) => {}
                        Err(Trap::Exception) => match labs.iter().rposition(|x| x.catch.is_some() || x.legacy != 0) {
                            Some(k) if labs[k].legacy != 0 => {
                                let h = labs[k].h;
                                let cat = labs[k].legacy;
                                labs.truncate(k + 1);
                                labs[k].legacy = 0;
                                st.truncate(h);
                                r.p = cat;
                            }
                            Some(k) => {
                                let c = labs[k].catch.unwrap();
                                let h = labs[k].h;
                                labs.truncate(k);
                                st.truncate(h);
                                branch!(c);
                            }
                            None => return Err(Trap::Exception),
                        },
                        Err(e) => return Err(e),
                    }
                }
                0x1A => {
                    pop!();
                }
                0x1B => {
                    let c = pop!() as u32;
                    let b = pop!();
                    let a = pop!();
                    st.push(if c != 0 { a } else { b });
                }
                0x20 => {
                    let i = r.u()? as usize;
                    st.push(l[i]);
                }
                0x21 => {
                    let i = r.u()? as usize;
                    l[i] = pop!();
                }
                0x22 => {
                    let i = r.u()? as usize;
                    l[i] = *st.last().ok_or(Trap::Trap("tee".into()))?;
                }
                0x23 => {
                    let i = r.u()? as usize;
                    st.push(self.globals[i] as u128);
                }
                0x24 => {
                    let i = r.u()? as usize;
                    self.globals[i] = pop!() as u64;
                }
                0x28..=0x35 => {
                    let _al = r.u()?;
                    let off = r.u()?;
                    let base = pop!();
                    let n = match op {
                        0x28 | 0x2A | 0x34 | 0x35 => 4,
                        0x29 | 0x2B => 8,
                        0x2C | 0x2D | 0x30 | 0x31 => 1,
                        _ => 2,
                    };
                    let a = self.ea(base, off, n)?;
                    let mut w = [0u8; 8];
                    w[..n].copy_from_slice(&self.mem[a..a + n]);
                    let u = u64::from_le_bytes(w);
                    let v: u64 = match op {
                        0x2C => u as u8 as i8 as i32 as u32 as u64,
                        0x2E => u as u16 as i16 as i32 as u32 as u64,
                        0x30 => u as u8 as i8 as i64 as u64,
                        0x32 => u as u16 as i16 as i64 as u64,
                        0x34 => u as u32 as i32 as i64 as u64,
                        _ => u,
                    };
                    st.push(v as u128);
                }
                0x36..=0x3E => {
                    let _al = r.u()?;
                    let off = r.u()?;
                    let v = pop!() as u64;
                    let base = pop!();
                    let n = match op {
                        0x36 | 0x38 | 0x3E => 4,
                        0x37 | 0x39 => 8,
                        0x3A | 0x3C => 1,
                        _ => 2,
                    };
                    let a = self.ea(base, off, n)?;
                    self.mem[a..a + n].copy_from_slice(&v.to_le_bytes()[..n]);
                }
                0x41 => st.push(r.s()? as i32 as u32 as u128),
                0x42 => st.push(r.s()? as u64 as u128),
                0x43 => {
                    let b = r.bytes(4)?;
                    st.push(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u128);
                }
                0x44 => {
                    let b = r.bytes(8)?;
                    let mut w = [0u8; 8];
                    w.copy_from_slice(b);
                    st.push(u64::from_le_bytes(w) as u128);
                }
                0x45 => {
                    let a = pop!() as u32;
                    st.push((a == 0) as u128);
                }
                0x46..=0x4F => bin32!(match op {
                    0x46 => |a, b| Ok((a == b) as u32),
                    0x47 => |a, b| Ok((a != b) as u32),
                    0x48 => |a, b| Ok(((a as i32) < (b as i32)) as u32),
                    0x49 => |a, b| Ok((a < b) as u32),
                    0x4A => |a, b| Ok((a as i32 > b as i32) as u32),
                    0x4B => |a, b| Ok((a > b) as u32),
                    0x4C => |a, b| Ok((a as i32 <= b as i32) as u32),
                    0x4D => |a, b| Ok((a <= b) as u32),
                    0x4E => |a, b| Ok((a as i32 >= b as i32) as u32),
                    _ => |a, b| Ok((a >= b) as u32),
                }),
                0x50 => {
                    let a = pop!() as u64;
                    st.push((a == 0) as u128);
                }
                0x51..=0x5A => bin64!(match op {
                    0x51 => |a, b| Ok((a == b) as u64),
                    0x52 => |a, b| Ok((a != b) as u64),
                    0x53 => |a, b| Ok(((a as i64) < (b as i64)) as u64),
                    0x54 => |a, b| Ok((a < b) as u64),
                    0x55 => |a, b| Ok((a as i64 > b as i64) as u64),
                    0x56 => |a, b| Ok((a > b) as u64),
                    0x57 => |a, b| Ok((a as i64 <= b as i64) as u64),
                    0x58 => |a, b| Ok((a <= b) as u64),
                    0x59 => |a, b| Ok((a as i64 >= b as i64) as u64),
                    _ => |a, b| Ok((a >= b) as u64),
                }),
                0x5B..=0x60 => f32c!(match op {
                    0x5B => |a, b| a == b,
                    0x5C => |a, b| a != b,
                    0x5D => |a, b| a < b,
                    0x5E => |a, b| a > b,
                    0x5F => |a, b| a <= b,
                    _ => |a, b| a >= b,
                }),
                0x61..=0x66 => f64c!(match op {
                    0x61 => |a, b| a == b,
                    0x62 => |a, b| a != b,
                    0x63 => |a, b| a < b,
                    0x64 => |a, b| a > b,
                    0x65 => |a, b| a <= b,
                    _ => |a, b| a >= b,
                }),
                0x67..=0x69 => {
                    let a = pop!() as u32;
                    st.push(match op {
                        0x67 => a.leading_zeros(),
                        0x68 => a.trailing_zeros(),
                        _ => a.count_ones(),
                    } as u128);
                }
                0x6A..=0x78 => bin32!(match op {
                    0x6A => |a, b| Ok(a.wrapping_add(b)),
                    0x6B => |a, b| Ok(a.wrapping_sub(b)),
                    0x6C => |a, b| Ok(a.wrapping_mul(b)),
                    0x6D => |a, b| if b == 0 || (a == 0x8000_0000 && b == u32::MAX) { t("int div") } else { Ok((a as i32 / b as i32) as u32) },
                    0x6E => |a, b| if b == 0 { t("int div") } else { Ok(a / b) },
                    0x6F => |a, b| if b == 0 { t("int div") } else { Ok((a as i32).wrapping_rem(b as i32) as u32) },
                    0x70 => |a, b| if b == 0 { t("int div") } else { Ok(a % b) },
                    0x71 => |a, b| Ok(a & b),
                    0x72 => |a, b| Ok(a | b),
                    0x73 => |a, b| Ok(a ^ b),
                    0x74 => |a, b| Ok(a << (b & 31)),
                    0x75 => |a, b| Ok(((a as i32) >> (b & 31)) as u32),
                    0x76 => |a, b| Ok(a >> (b & 31)),
                    0x77 => |a, b| Ok(a.rotate_left(b & 31)),
                    _ => |a, b| Ok(a.rotate_right(b & 31)),
                }),
                0x79..=0x7B => {
                    let a = pop!() as u64;
                    st.push(match op {
                        0x79 => a.leading_zeros(),
                        0x7A => a.trailing_zeros(),
                        _ => a.count_ones(),
                    } as u128);
                }
                0x7C..=0x8A => bin64!(match op {
                    0x7C => |a, b| Ok(a.wrapping_add(b)),
                    0x7D => |a, b| Ok(a.wrapping_sub(b)),
                    0x7E => |a, b| Ok(a.wrapping_mul(b)),
                    0x7F => |a, b| if b == 0 || (a == 1 << 63 && b == u64::MAX) { t("int div") } else { Ok((a as i64 / b as i64) as u64) },
                    0x80 => |a, b| if b == 0 { t("int div") } else { Ok(a / b) },
                    0x81 => |a, b| if b == 0 { t("int div") } else { Ok((a as i64).wrapping_rem(b as i64) as u64) },
                    0x82 => |a, b| if b == 0 { t("int div") } else { Ok(a % b) },
                    0x83 => |a, b| Ok(a & b),
                    0x84 => |a, b| Ok(a | b),
                    0x85 => |a, b| Ok(a ^ b),
                    0x86 => |a, b| Ok(a << (b & 63)),
                    0x87 => |a, b| Ok(((a as i64) >> (b & 63)) as u64),
                    0x88 => |a, b| Ok(a >> (b & 63)),
                    0x89 => |a, b| Ok(a.rotate_left((b & 63) as u32)),
                    _ => |a, b| Ok(a.rotate_right((b & 63) as u32)),
                }),
                0x8B..=0x91 => {
                    let a = f32::from_bits(pop!() as u32);
                    let v = match op {
                        0x8B => a.abs(),
                        0x8C => -a,
                        0x8D => a.ceil(),
                        0x8E => a.floor(),
                        0x8F => a.trunc(),
                        0x90 => a.round_ties_even(),
                        _ => a.sqrt(),
                    };
                    st.push(v.to_bits() as u128);
                }
                0x92..=0x98 => f32b!(match op {
                    0x92 => |a, b| a + b,
                    0x93 => |a, b| a - b,
                    0x94 => |a, b| a * b,
                    0x95 => |a, b| a / b,
                    0x96 => |a, b| if a.is_nan() || b.is_nan() { f32::NAN } else if a == b { if a.is_sign_negative() { a } else { b } } else { a.min(b) },
                    0x97 => |a, b| if a.is_nan() || b.is_nan() { f32::NAN } else if a == b { if a.is_sign_positive() { a } else { b } } else { a.max(b) },
                    _ => |a, b| a.copysign(b),
                }),
                0x99..=0x9F => {
                    let a = f64::from_bits(pop!() as u64);
                    let v = match op {
                        0x99 => a.abs(),
                        0x9A => -a,
                        0x9B => a.ceil(),
                        0x9C => a.floor(),
                        0x9D => a.trunc(),
                        0x9E => a.round_ties_even(),
                        _ => a.sqrt(),
                    };
                    st.push(v.to_bits() as u128);
                }
                0xA0..=0xA6 => f64b!(match op {
                    0xA0 => |a, b| a + b,
                    0xA1 => |a, b| a - b,
                    0xA2 => |a, b| a * b,
                    0xA3 => |a, b| a / b,
                    0xA4 => |a, b| if a.is_nan() || b.is_nan() { f64::NAN } else if a == b { if a.is_sign_negative() { a } else { b } } else { a.min(b) },
                    0xA5 => |a, b| if a.is_nan() || b.is_nan() { f64::NAN } else if a == b { if a.is_sign_positive() { a } else { b } } else { a.max(b) },
                    _ => |a, b| a.copysign(b),
                }),
                0xA7 => {
                    let a = pop!() as u64;
                    st.push(a as u32 as u128);
                }
                0xAC => {
                    let a = pop!() as u32;
                    st.push(a as i32 as i64 as u64 as u128);
                }
                0xAD => {
                    let a = pop!() as u32;
                    st.push(a as u128);
                }
                0xB2..=0xBA => {
                    let a = pop!();
                    let v: u64 = match op {
                        0xB2 => (a as u32 as i32 as f32).to_bits() as u64,
                        0xB3 => (a as u32 as f32).to_bits() as u64,
                        0xB4 => (a as u64 as i64 as f32).to_bits() as u64,
                        0xB5 => (a as u64 as f32).to_bits() as u64,
                        0xB6 => (f64::from_bits(a as u64) as f32).to_bits() as u64,
                        0xB7 => (a as u32 as i32 as f64).to_bits(),
                        0xB8 => (a as u32 as f64).to_bits(),
                        0xB9 => (a as u64 as i64 as f64).to_bits(),
                        _ => (a as u64 as f64).to_bits(),
                    };
                    st.push(v as u128);
                }
                0xBB => {
                    let a = f32::from_bits(pop!() as u32);
                    st.push((a as f64).to_bits() as u128);
                }
                0xBC | 0xBE => {
                    let a = pop!() as u32;
                    st.push(a as u128);
                }
                0xBD | 0xBF => {
                    let a = pop!() as u64;
                    st.push(a as u128);
                }
                0xC0..=0xC4 => {
                    let a = pop!();
                    let v: u64 = match op {
                        0xC0 => a as u8 as i8 as i32 as u32 as u64,
                        0xC1 => a as u16 as i16 as i32 as u32 as u64,
                        0xC2 => a as u8 as i8 as i64 as u64,
                        0xC3 => a as u16 as i16 as i64 as u64,
                        _ => a as u32 as i32 as i64 as u64,
                    };
                    st.push(v as u128);
                }
                0xFC => {
                    let k = r.u()?;
                    match k {
                        0..=7 => {
                            let a = pop!();
                            let x = if k & 2 == 0 { f32::from_bits(a as u32) as f64 } else { f64::from_bits(a as u64) };
                            let v: u64 = match k {
                                0 | 2 => x as i32 as u32 as u64,
                                1 | 3 => x as u32 as u64,
                                4 | 6 => x as i64 as u64,
                                _ => x as u64,
                            };
                            st.push(v as u128);
                        }
                        10 => {
                            r.u8()?;
                            r.u8()?;
                            let n = pop!() as u32 as usize;
                            let s = pop!() as u32 as usize;
                            let d = pop!() as u32 as usize;
                            if s + n > self.mem.len() || d + n > self.mem.len() {
                                return t("out of bounds memory access");
                            }
                            self.mem.copy_within(s..s + n, d);
                        }
                        11 => {
                            r.u8()?;
                            let n = pop!() as u32 as usize;
                            let v = pop!() as u8;
                            let d = pop!() as u32 as usize;
                            if d + n > self.mem.len() {
                                return t("out of bounds memory access");
                            }
                            self.mem[d..d + n].fill(v);
                        }
                        _ => return t(&format!("0xFC {}", k)),
                    }
                }
                0xFD => {
                    let k = r.u()?;
                    let lanes32 = |v: u128| -> [u32; 4] { [v as u32, (v >> 32) as u32, (v >> 64) as u32, (v >> 96) as u32] };
                    let from32 = |a: [u32; 4]| -> u128 { a[0] as u128 | (a[1] as u128) << 32 | (a[2] as u128) << 64 | (a[3] as u128) << 96 };
                    match k {
                        0 => {
                            r.u()?;
                            let off = r.u()?;
                            let base = pop!();
                            let a = self.ea(base, off, 16)?;
                            let mut w = [0u8; 16];
                            w.copy_from_slice(&self.mem[a..a + 16]);
                            st.push(u128::from_le_bytes(w));
                        }
                        11 => {
                            r.u()?;
                            let off = r.u()?;
                            let v = pop!();
                            let base = pop!();
                            let a = self.ea(base, off, 16)?;
                            self.mem[a..a + 16].copy_from_slice(&v.to_le_bytes());
                        }
                        12 => {
                            let b = r.bytes(16)?;
                            let mut w = [0u8; 16];
                            w.copy_from_slice(b);
                            st.push(u128::from_le_bytes(w));
                        }
                        17 | 19 => {
                            let a = pop!() as u32;
                            st.push(from32([a; 4]));
                        }
                        27 | 31 => {
                            let ln = r.u8()? as usize;
                            let v = pop!();
                            st.push(lanes32(v)[ln] as u128);
                        }
                        28 | 32 => {
                            let ln = r.u8()? as usize;
                            let s = pop!() as u32;
                            let v = pop!();
                            let mut a = lanes32(v);
                            a[ln] = s;
                            st.push(from32(a));
                        }
                        78..=81 => {
                            let b = pop!();
                            let a = pop!();
                            st.push(match k {
                                78 => a & b,
                                79 => a & !b,
                                80 => a | b,
                                _ => a ^ b,
                            });
                        }
                        82 => {
                            let c = pop!();
                            let b = pop!();
                            let a = pop!();
                            st.push((a & c) | (b & !c));
                        }
                        83 => {
                            let a = pop!();
                            st.push((a != 0) as u128);
                        }
                        174 | 177 | 181 | 228..=231 => {
                            let b = lanes32(pop!());
                            let a = lanes32(pop!());
                            let mut o = [0u32; 4];
                            for i in 0..4 {
                                let (x, y) = (a[i], b[i]);
                                let (fx, fy) = (f32::from_bits(x), f32::from_bits(y));
                                o[i] = match k {
                                    174 => x.wrapping_add(y),
                                    177 => x.wrapping_sub(y),
                                    181 => x.wrapping_mul(y),
                                    228 => (fx + fy).to_bits(),
                                    229 => (fx - fy).to_bits(),
                                    230 => (fx * fy).to_bits(),
                                    _ => (fx / fy).to_bits(),
                                };
                            }
                            st.push(from32(o));
                        }
                        _ => return t(&format!("simd op {}", k)),
                    }
                }
                _ => return t(&format!("opcode {:#x}", op)),
            }
        }
    }
}
