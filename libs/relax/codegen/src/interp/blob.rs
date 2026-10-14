//! The relocatable form of an interpreted function: what the interpreter backend hands the
//! runtime as the function's "code" (and what the disk cache stores and links, exactly like
//! machine code). Words that hold per-run values (symbol values, slot table words, callable
//! addresses) are zero in the blob and named by `RelocAt` entries (Pool64); handler addresses are
//! stored relative to the entry thunk pool (one binary: the cache is keyed by the build), branch
//! targets as word offsets, call descriptors and vector ops by value. `install` turns a linked
//! blob into a registered function and returns its entry (a pool thunk).

use super::*;

/// What a relocated word's value is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Prov {
    Sym(Sym),
    /// a slot's table word (`Reloc::Slot`)
    Slot(u32),
    /// a slot's callable address (`Reloc::Thunk`)
    Thunk(u32),
}

/// What lowering records about the code words it emits.
#[derive(Default)]
pub(super) struct Persist {
    /// handler words
    pub handlers: Vec<usize>,
    /// branch target words
    pub targets: Vec<usize>,
    /// words holding a per-run value
    pub prov: Vec<(usize, Prov)>,
    /// words holding a call descriptor pointer
    pub desc: Vec<usize>,
    /// words holding a vector op pointer
    pub vx: Vec<(usize, VX)>,
}

const MAGIC: u64 = 0x4d50_5249_4e54_5031; // "MPRINTP1"

const T_PLAIN: u8 = 0;
const T_HANDLER: u8 = 1;
const T_TARGET: u8 = 2;
const T_RELOC: u8 = 3;
const T_DESC: u8 = 4;
const T_VX: u8 = 5;

struct W {
    w: Vec<u64>,
    relocs: Vec<RelocAt>,
}

impl W {
    fn u(&mut self, v: u64) {
        self.w.push(v);
    }
    fn reloc(&mut self, p: Prov) {
        let at = (self.w.len() * 8) as u32;
        let target = match p {
            Prov::Sym(s) => Reloc::Sym(s),
            Prov::Slot(id) => Reloc::Slot(id),
            Prov::Thunk(id) => Reloc::Thunk(id),
        };
        self.relocs.push(RelocAt { at, kind: RelocKind::Pool64, target });
        self.w.push(0);
    }
}

struct R<'a> {
    w: &'a [u64],
    i: usize,
}

impl R<'_> {
    fn u(&mut self) -> Option<u64> {
        let v = *self.w.get(self.i)?;
        self.i += 1;
        Some(v)
    }
}

fn cls_code(c: Cls) -> u64 {
    match c {
        Cls::I => 0,
        Cls::F32 => 1,
        Cls::F64 => 2,
        Cls::V128 => 3,
    }
}
fn cls_of(v: u64) -> Option<Cls> {
    Some(match v {
        0 => Cls::I,
        1 => Cls::F32,
        2 => Cls::F64,
        3 => Cls::V128,
        _ => return None,
    })
}
fn mem_code(m: Mem) -> u64 {
    match m {
        Mem::Int(b, s) => (b as u64) << 8 | s as u64,
        Mem::F32 => 1 << 16,
        Mem::F64 => 2 << 16,
    }
}
fn mem_of(v: u64) -> Mem {
    match v >> 16 {
        1 => Mem::F32,
        2 => Mem::F64,
        _ => Mem::Int((v >> 8) as u8, v & 1 != 0),
    }
}

fn put_agg(w: &mut W, a: &CAgg) {
    w.u(a.size as u64 | (a.align as u64) << 32);
    w.u(a.fields.len() as u64);
    for (off, m) in &a.fields {
        w.u(*off as u64 | mem_code(*m) << 32);
    }
}
fn get_agg(r: &mut R) -> Option<CAgg> {
    let sa = r.u()?;
    let n = r.u()? as usize;
    let mut fields = Vec::with_capacity(n);
    for _ in 0..n {
        let x = r.u()?;
        fields.push(((x & 0xffff_ffff) as u32, mem_of(x >> 32)));
    }
    Some(CAgg { size: sa as u32, align: (sa >> 32) as u32, fields })
}

fn put_csig(w: &mut W, s: &CSig) {
    w.u(s.args.len() as u64);
    for a in &s.args {
        match a {
            CArg::Scalar(m) => {
                w.u(0);
                w.u(mem_code(*m));
            }
            CArg::Agg(g) => {
                w.u(1);
                put_agg(w, g);
            }
        }
    }
    match &s.ret {
        Some(g) => {
            w.u(1);
            put_agg(w, g);
        }
        None => w.u(0),
    }
    w.u(s.n_fixed as u64);
    w.u(match s.ret_narrow {
        Some(it) => 1 | (it.bits as u64) << 8 | (it.signed as u64) << 16,
        None => 0,
    });
    w.u(s.unwinds as u64);
    w.u(s.conv as u64);
}
fn get_csig(r: &mut R) -> Option<CSig> {
    let n = r.u()? as usize;
    let mut args = Vec::with_capacity(n);
    for _ in 0..n {
        args.push(match r.u()? {
            0 => CArg::Scalar(mem_of(r.u()?)),
            _ => CArg::Agg(get_agg(r)?),
        });
    }
    let ret = if r.u()? != 0 { Some(get_agg(r)?) } else { None };
    let n_fixed = r.u()? as u32;
    let nw = r.u()?;
    let ret_narrow = if nw & 1 != 0 { Some(IntTy { bits: (nw >> 8) as u8, signed: (nw >> 16) & 1 != 0 }) } else { None };
    let unwinds = r.u()? != 0;
    let conv = match r.u()? {
        0 => CConv::Host,
        1 => CConv::SysV64,
        2 => CConv::Win64,
        _ => return None,
    };
    Some(CSig { conv, args, ret, n_fixed, ret_narrow, unwinds })
}

fn vw_code(w: VW) -> u64 {
    match w {
        VW::B => 0,
        VW::H => 1,
        VW::S => 2,
        VW::D => 3,
    }
}
fn vw_of(v: u64) -> VW {
    match v & 3 {
        0 => VW::B,
        1 => VW::H,
        2 => VW::S,
        _ => VW::D,
    }
}

/// A vector op as a word: variant << 16 | w << 8 | k.
fn vx_code(op: VX) -> u64 {
    let (t, w, k): (u64, Option<VW>, u8) = match op {
        VX::Add(w) => (0, Some(w), 0),
        VX::Sub(w) => (1, Some(w), 0),
        VX::MulLo(w) => (2, Some(w), 0),
        VX::MinS(w) => (3, Some(w), 0),
        VX::MaxS(w) => (4, Some(w), 0),
        VX::MinU(w) => (5, Some(w), 0),
        VX::MaxU(w) => (6, Some(w), 0),
        VX::CmpEq(w) => (7, Some(w), 0),
        VX::CmpGtS(w) => (8, Some(w), 0),
        VX::AddSatU(w) => (9, Some(w), 0),
        VX::SubSatU(w) => (10, Some(w), 0),
        VX::AddSatS(w) => (11, Some(w), 0),
        VX::SubSatS(w) => (12, Some(w), 0),
        VX::AvgU(w) => (13, Some(w), 0),
        VX::And => (14, None, 0),
        VX::Or => (15, None, 0),
        VX::Xor => (16, None, 0),
        VX::AndNot => (17, None, 0),
        VX::ZipLo(w) => (18, Some(w), 0),
        VX::ZipHi(w) => (19, Some(w), 0),
        VX::MaddH => (20, None, 0),
        VX::MaddUbs => (21, None, 0),
        VX::SadU8 => (22, None, 0),
        VX::PackUsH => (23, None, 0),
        VX::PackSsH => (24, None, 0),
        VX::PackSsS => (25, None, 0),
        VX::PackUsS => (26, None, 0),
        VX::ByteShuffle => (27, None, 0),
        VX::X86Sha1Rnds4(k) => (28, None, k),
        VX::X86Sha1NextE => (29, None, 0),
        VX::X86Sha1Msg1 => (30, None, 0),
        VX::X86Sha1Msg2 => (31, None, 0),
        VX::X86Sha256Msg1 => (32, None, 0),
        VX::X86Sha256Msg2 => (33, None, 0),
        VX::ArmSha1Su1 => (34, None, 0),
        VX::ArmSha256Su0 => (35, None, 0),
        VX::ArmSha1H => (36, None, 0),
        VX::X86Sha256Rnds2 => (37, None, 0),
        VX::ArmSha1C => (38, None, 0),
        VX::ArmSha1P => (39, None, 0),
        VX::ArmSha1M => (40, None, 0),
        VX::ArmSha1Su0 => (41, None, 0),
        VX::ArmSha256H => (42, None, 0),
        VX::ArmSha256H2 => (43, None, 0),
        VX::ArmSha256Su1 => (44, None, 0),
        VX::Abs(w) => (45, Some(w), 0),
        VX::ShlI(w, k) => (46, Some(w), k),
        VX::ShrUI(w, k) => (47, Some(w), k),
        VX::ShrSI(w, k) => (48, Some(w), k),
        VX::ShufS(k) => (49, None, k),
        VX::BytesShrI(k) => (50, None, k),
        VX::BytesShlI(k) => (51, None, k),
        VX::WidenLoU(w) => (52, Some(w), 0),
        VX::WidenHiU(w) => (53, Some(w), 0),
        VX::WidenLoS(w) => (54, Some(w), 0),
        VX::WidenHiS(w) => (55, Some(w), 0),
        VX::PairAddLongU(w) => (56, Some(w), 0),
        VX::Splat(w) => (57, Some(w), 0),
        VX::Extract(w, k) => (58, Some(w), k),
        VX::Insert(w, k) => (59, Some(w), k),
        VX::MoveMaskB => (60, None, 0),
        VX::AddAcross(w) => (61, Some(w), 0),
        VX::MaxAcrossU(w) => (62, Some(w), 0),
        VX::FromLo64 => (63, None, 0),
    };
    t << 16 | w.map_or(0, vw_code) << 8 | k as u64
}

fn vx_of(v: u64) -> Option<VX> {
    let (t, w, k) = (v >> 16, vw_of(v >> 8), (v & 0xff) as u8);
    Some(match t {
        0 => VX::Add(w),
        1 => VX::Sub(w),
        2 => VX::MulLo(w),
        3 => VX::MinS(w),
        4 => VX::MaxS(w),
        5 => VX::MinU(w),
        6 => VX::MaxU(w),
        7 => VX::CmpEq(w),
        8 => VX::CmpGtS(w),
        9 => VX::AddSatU(w),
        10 => VX::SubSatU(w),
        11 => VX::AddSatS(w),
        12 => VX::SubSatS(w),
        13 => VX::AvgU(w),
        14 => VX::And,
        15 => VX::Or,
        16 => VX::Xor,
        17 => VX::AndNot,
        18 => VX::ZipLo(w),
        19 => VX::ZipHi(w),
        20 => VX::MaddH,
        21 => VX::MaddUbs,
        22 => VX::SadU8,
        23 => VX::PackUsH,
        24 => VX::PackSsH,
        25 => VX::PackSsS,
        26 => VX::PackUsS,
        27 => VX::ByteShuffle,
        28 => VX::X86Sha1Rnds4(k),
        29 => VX::X86Sha1NextE,
        30 => VX::X86Sha1Msg1,
        31 => VX::X86Sha1Msg2,
        32 => VX::X86Sha256Msg1,
        33 => VX::X86Sha256Msg2,
        34 => VX::ArmSha1Su1,
        35 => VX::ArmSha256Su0,
        36 => VX::ArmSha1H,
        37 => VX::X86Sha256Rnds2,
        38 => VX::ArmSha1C,
        39 => VX::ArmSha1P,
        40 => VX::ArmSha1M,
        41 => VX::ArmSha1Su0,
        42 => VX::ArmSha256H,
        43 => VX::ArmSha256H2,
        44 => VX::ArmSha256Su1,
        45 => VX::Abs(w),
        46 => VX::ShlI(w, k),
        47 => VX::ShrUI(w, k),
        48 => VX::ShrSI(w, k),
        49 => VX::ShufS(k),
        50 => VX::BytesShrI(k),
        51 => VX::BytesShlI(k),
        52 => VX::WidenLoU(w),
        53 => VX::WidenHiU(w),
        54 => VX::WidenLoS(w),
        55 => VX::WidenHiS(w),
        56 => VX::PairAddLongU(w),
        57 => VX::Splat(w),
        58 => VX::Extract(w, k),
        59 => VX::Insert(w, k),
        60 => VX::MoveMaskB,
        61 => VX::AddAcross(w),
        62 => VX::MaxAcrossU(w),
        63 => VX::FromLo64,
        _ => return None,
    })
}

/// The function's relocatable form: (bytes, relocations).
pub(super) fn serialize(f: &IFunc, p: &Persist) -> (Vec<u8>, Vec<RelocAt>) {
    let mut w = W { w: Vec::new(), relocs: Vec::new() };
    let n = f.code.len();
    let mut tag = vec![T_PLAIN; n];
    for &i in &p.handlers {
        tag[i] = T_HANDLER;
    }
    for &i in &p.targets {
        tag[i] = T_TARGET;
    }
    let mut prov: Vec<Option<Prov>> = vec![None; n];
    for &(i, pr) in &p.prov {
        tag[i] = T_RELOC;
        prov[i] = Some(pr);
    }
    let mut desc_ix = vec![u32::MAX; n];
    for (k, &i) in p.desc.iter().enumerate() {
        tag[i] = T_DESC;
        desc_ix[i] = k as u32;
    }
    let mut vx_ix: Vec<Option<VX>> = vec![None; n];
    for &(i, op) in &p.vx {
        tag[i] = T_VX;
        vx_ix[i] = Some(op);
    }
    // header
    w.u(MAGIC);
    w.u(n as u64);
    w.u(f.frame_words as u64);
    w.u(f.align as u64);
    w.u(f.params.len() as u64);
    for (c, k) in &f.params {
        w.u(*c as u64 | cls_code(*k) << 32);
    }
    match &f.cplan {
        Some((sig, _, bufs, retbuf)) => {
            w.u(1);
            put_csig(&mut w, sig);
            w.u(bufs.len() as u64);
            for b in bufs {
                w.u(*b as u64);
            }
            w.u(*retbuf as u64);
        }
        None => w.u(0),
    }
    let name = f.name.as_bytes();
    w.u(name.len() as u64);
    for ch in name.chunks(8) {
        let mut b = [0u8; 8];
        b[..ch.len()].copy_from_slice(ch);
        w.u(u64::from_le_bytes(b));
    }
    // tags, packed 8 per word
    for ch in tag.chunks(8) {
        let mut b = [0u8; 8];
        b[..ch.len()].copy_from_slice(ch);
        w.u(u64::from_le_bytes(b));
    }
    // code
    let base = f.code.as_ptr() as u64;
    let pool = pool_base();
    for i in 0..n {
        match tag[i] {
            T_HANDLER => w.u(f.code[i].wrapping_sub(pool)),
            T_TARGET => w.u((f.code[i] - base) / 8),
            T_RELOC => w.reloc(prov[i].unwrap()),
            T_DESC => w.u(desc_ix[i] as u64),
            T_VX => w.u(vx_code(vx_ix[i].unwrap())),
            _ => w.u(f.code[i]),
        }
    }
    // call descriptors
    w.u(p.desc.len() as u64);
    for &i in &p.desc {
        let d = unsafe { &*(f.code[i] as *const CallDesc) };
        match (&d.target, d.src) {
            (Target::Cell(c), _) => {
                w.u(0);
                w.u(*c as u64);
            }
            (Target::Table(_), Some(pr)) => {
                w.u(1);
                w.reloc(pr);
            }
            (Target::Addr(_), Some(pr)) => {
                w.u(2);
                w.reloc(pr);
            }
            (Target::Table(v) | Target::Addr(v), None) => {
                // no provenance: the value as it is (never produced by lowering)
                w.u(3);
                w.u(*v);
            }
        }
        w.u(d.args.len() as u64);
        for (c, k) in &d.args {
            w.u(*c as u64 | cls_code(*k) << 32);
        }
        w.u(d.rets.len() as u64);
        for (c, k) in &d.rets {
            w.u(*c as u64 | cls_code(*k) << 32);
        }
        match &d.csig {
            Some((sig, _)) => {
                w.u(1);
                put_csig(&mut w, sig);
            }
            None => w.u(0),
        }
        w.u(d.frame_words as u64);
    }
    let mut bytes = Vec::with_capacity(w.w.len() * 8);
    for x in &w.w {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    (bytes, w.relocs)
}

/// Is `code` a blob (an interpreted function's relocatable form)?
pub fn is_blob(code: &[u8]) -> bool {
    code.len() >= 16 && code[..8] == MAGIC.to_le_bytes()
}

fn deserialize(bytes: &[u8]) -> Option<IFunc> {
    if bytes.len() % 8 != 0 {
        return None;
    }
    let words: Vec<u64> = bytes.chunks(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
    let mut r = R { w: &words, i: 0 };
    if r.u()? != MAGIC {
        return None;
    }
    let n = r.u()? as usize;
    let frame_words = r.u()? as usize;
    let align = r.u()? as usize;
    let np = r.u()? as usize;
    let mut params = Vec::with_capacity(np);
    for _ in 0..np {
        let x = r.u()?;
        params.push(((x & 0xffff_ffff) as u32, cls_of(x >> 32)?));
    }
    let cplan = if r.u()? != 0 {
        let sig = get_csig(&mut r)?;
        let nb = r.u()? as usize;
        let mut bufs = Vec::with_capacity(nb);
        for _ in 0..nb {
            bufs.push(r.u()? as u32);
        }
        let retbuf = r.u()? as u32;
        let plan = cabi::plan(&sig, sig.conv.target());
        Some((Box::new(sig), plan, bufs, retbuf))
    } else {
        None
    };
    let nl = r.u()? as usize;
    let mut name = Vec::with_capacity(nl);
    for _ in 0..nl.div_ceil(8) {
        name.extend_from_slice(&r.u()?.to_le_bytes());
    }
    name.truncate(nl);
    let name = String::from_utf8(name).ok()?;
    let mut tag = Vec::with_capacity(n);
    for _ in 0..n.div_ceil(8) {
        tag.extend_from_slice(&r.u()?.to_le_bytes());
    }
    tag.truncate(n);
    let mut code = vec![0u64; n].into_boxed_slice();
    let raw: Vec<u64> = (0..n).map(|_| r.u()).collect::<Option<_>>()?;
    let nd = r.u()? as usize;
    let mut descs: Vec<u64> = Vec::with_capacity(nd);
    for _ in 0..nd {
        let (target, src) = match r.u()? {
            0 => (Target::Cell(r.u()? as u32), None),
            1 => (Target::Table(r.u()?), None),
            2 => (Target::Addr(r.u()?), None),
            _ => (Target::Addr(r.u()?), None),
        };
        let na = r.u()? as usize;
        let mut args = Vec::with_capacity(na);
        for _ in 0..na {
            let x = r.u()?;
            args.push(((x & 0xffff_ffff) as u32, cls_of(x >> 32)?));
        }
        let nr = r.u()? as usize;
        let mut rets = Vec::with_capacity(nr);
        for _ in 0..nr {
            let x = r.u()?;
            rets.push(((x & 0xffff_ffff) as u32, cls_of(x >> 32)?));
        }
        let csig = if r.u()? != 0 {
            let sig = get_csig(&mut r)?;
            let plan = cabi::plan(&sig, sig.conv.target());
            Some((Box::new(sig), plan))
        } else {
            None
        };
        let fw = r.u()? as usize;
        let d = Box::new(CallDesc { target, args, rets, csig, frame_words: fw, ic: AtomicPtr::new(std::ptr::null_mut()), src });
        descs.push(Box::into_raw(d) as u64);
    }
    let base = code.as_ptr() as u64;
    let pool = pool_base();
    for i in 0..n {
        code[i] = match tag[i] {
            T_HANDLER => raw[i].wrapping_add(pool),
            T_TARGET => base + 8 * raw[i],
            T_DESC => *descs.get(raw[i] as usize)?,
            T_VX => Box::into_raw(Box::new(vx_of(raw[i])?)) as u64,
            _ => raw[i],
        };
    }
    Some(IFunc { code, frame_words, params, cplan, name, align })
}

/// A linked blob (its relocated words hold this run's values): the function is registered, and
/// its entry (a pool thunk: the slot's table word, callable from native code) is returned.
pub fn install(bytes: &[u8]) -> Result<u64, String> {
    let f = deserialize(bytes).ok_or("makepad run interp: malformed interpreted code")?;
    let k = NEXT.fetch_add(1, Ordering::Relaxed);
    if k >= POOL {
        return Err(format!("makepad run interp: more than {} interpreted functions (the entry thunk pool)", POOL));
    }
    FUNCS[k].store(Box::into_raw(Box::new(f)), Ordering::Release);
    Ok(pool_base() + 8 * k as u64)
}
