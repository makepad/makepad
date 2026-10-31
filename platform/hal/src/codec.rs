//! Field packing for the command and event tables.
//!
//! A command carries a header (`id`, `flags`, `data`) and eight 32-bit words (`Cmd::a`, four
//! u64 holding word pairs); an event carries a header (`window`, `data`), six words (`b`) and four
//! f64 (`a`). The tables in `cmds.rs` / `events.rs` name each field with a *kind* from [`k`];
//! the kind says which lane the field uses and how many words, and the table macros generate the
//! constructor, the decoder and the writer method from that one line.

use crate::types::{Attr, Id, Range, Slice, StrId};

/// Words in `Cmd::a`.
pub const CMD_WORDS: usize = 8;
/// Words in `RawEvent::b`.
pub const EVENT_WORDS: usize = 6;
/// f64 in `RawEvent::a`.
pub const EVENT_FLOATS: usize = 4;

/// Collects the packed form of one command / event.
#[derive(Default)]
pub struct Packer {
    pub u: [u32; 8],
    ui: usize,
    pub f: [f64; 4],
    fi: usize,
    pub id: Id,
    pub flags: u16,
    pub data: Range,
}

impl Packer {
    pub fn new() -> Packer {
        Packer::default()
    }
    pub fn put_u32(&mut self, v: u32) {
        self.u[self.ui] = v;
        self.ui += 1;
    }
    pub fn put_u64(&mut self, v: u64) {
        self.put_u32(v as u32);
        self.put_u32((v >> 32) as u32);
    }
    pub fn put_f64(&mut self, v: f64) {
        self.f[self.fi] = v;
        self.fi += 1;
    }
    /// The words as the u64 pairs of `Cmd::a`.
    pub fn words_a4(&self) -> [u64; 4] {
        let mut a = [0u64; 4];
        for (i, a) in a.iter_mut().enumerate() {
            *a = self.u[2 * i] as u64 | (self.u[2 * i + 1] as u64) << 32;
        }
        a
    }
    /// The words as the u64 triple of `RawEvent::b` (six words).
    pub fn words_b3(&self) -> [u64; 3] {
        let mut b = [0u64; 3];
        for (i, b) in b.iter_mut().enumerate() {
            *b = self.u[2 * i] as u64 | (self.u[2 * i + 1] as u64) << 32;
        }
        b
    }
}

/// Reads fields back in the order they were packed.
pub struct Reader<'a> {
    u: [u32; 8],
    ui: usize,
    f: [f64; 4],
    fi: usize,
    pub id: Id,
    pub flags: u16,
    pub data: Range,
    pub arena: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn from_cmd(c: &crate::types::Cmd, arena: &'a [u8]) -> Reader<'a> {
        let mut u = [0u32; 8];
        for i in 0..4 {
            u[2 * i] = c.a[i] as u32;
            u[2 * i + 1] = (c.a[i] >> 32) as u32;
        }
        Reader { u, ui: 0, f: [0.0; 4], fi: 0, id: c.id, flags: c.flags, data: c.data, arena }
    }
    pub fn from_event(e: &crate::types::RawEvent, arena: &'a [u8]) -> Reader<'a> {
        let mut u = [0u32; 8];
        for i in 0..3 {
            u[2 * i] = e.b[i] as u32;
            u[2 * i + 1] = (e.b[i] >> 32) as u32;
        }
        Reader { u, ui: 0, f: e.a, fi: 0, id: e.window, flags: 0, data: e.data, arena }
    }
    pub fn get_u32(&mut self) -> u32 {
        let v = self.u[self.ui];
        self.ui += 1;
        v
    }
    pub fn get_u64(&mut self) -> u64 {
        let lo = self.get_u32() as u64;
        lo | (self.get_u32() as u64) << 32
    }
    pub fn get_f64(&mut self) -> f64 {
        let v = self.f[self.fi];
        self.fi += 1;
        v
    }
    /// The bytes a range names; an out-of-range range reads as empty.
    pub fn bytes(&self, r: Range) -> &'a [u8] {
        let a = r.off as usize;
        a.checked_add(r.len as usize).and_then(|e| self.arena.get(a..e)).unwrap_or(&[])
    }
}

/// Something that can take bytes into its arena and name them by a [`Range`] (the command writer,
/// the event queue).
pub trait ArenaSink {
    fn put_bytes(&mut self, bytes: &[u8]) -> Range;
}

/// One field kind of a table.
pub trait Field {
    /// The packed value (what the constructor takes).
    type T;
    /// What a writer method takes (bytes for a range, otherwise `T`).
    type In<'a>;
    /// What the decoder yields.
    type Out<'a>;
    /// 32-bit words used.
    const UW: usize;
    /// f64 used.
    const FW: usize;
    fn pack(v: Self::T, p: &mut Packer);
    fn lower<'a, S: ArenaSink>(s: &mut S, v: Self::In<'a>) -> Self::T;
    fn read<'a>(r: &mut Reader<'a>) -> Self::Out<'a>;
    /// What the decoder yields for a value the writer was given (round-trip tests).
    fn expect<'a>(v: Self::In<'a>) -> Self::Out<'a>;
    /// A deterministic value for round-trip tests.
    fn sample(n: u32) -> Self::In<'static>;
}

/// The field kinds a table line can name.
#[allow(non_camel_case_types)]
pub mod k {

    /// Header id (`Cmd::id` / `RawEvent::window`).
    pub struct obj;
    /// Header payload range (`Cmd::data` / `RawEvent::data`); the writer takes the bytes.
    pub struct rng;
    /// A borrowed run of the caller's memory (4 words).
    pub struct slice;
    /// A borrowed run whose length fits 32 bits (3 words).
    pub struct slice32;
    /// Header flags (`Cmd::flags`).
    pub struct flags;
    /// An id in the words (2).
    pub struct id;
    pub struct sid;
    pub struct u32;
    pub struct i32;
    pub struct u64;
    pub struct bool;
    pub struct f32;
    /// event float lane
    pub struct f64;
    /// f64 in the command word lane (2 words).
    pub struct f64w;
    /// four f32 (x, y, w, h)
    pub struct rect;
    pub struct v2;
    /// four u32
    pub struct urect;
    /// two u16 in one word
    pub struct u16x2;
    pub struct attr;
}

macro_rules! simple_field {
    ($kind:ident, $t:ty, $uw:expr, $fw:expr, |$p:ident, $v:ident| $pack:expr, |$r:ident| $read:expr, |$n:ident| $sample:expr) => {
        impl Field for k::$kind {
            type T = $t;
            type In<'a> = $t;
            type Out<'a> = $t;
            const UW: usize = $uw;
            const FW: usize = $fw;
            #[inline]
            fn pack($v: $t, $p: &mut Packer) {
                $pack
            }
            #[inline]
            fn lower<'a, S: ArenaSink>(_s: &mut S, v: $t) -> $t {
                v
            }
            #[inline]
            fn read<'a>($r: &mut Reader<'a>) -> $t {
                $read
            }
            fn expect<'a>(v: Self::In<'a>) -> Self::Out<'a> {
                v
            }
            fn sample($n: u32) -> $t {
                $sample
            }
        }
    };
}

simple_field!(obj, Id, 0, 0, |p, v| p.id = v, |r| r.id, |n| Id::new(n, n + 1));
simple_field!(flags, u16, 0, 0, |p, v| p.flags = v, |r| r.flags, |n| n as u16);
simple_field!(id, Id, 2, 0, |p, v| { p.put_u32(v.index); p.put_u32(v.gen) }, |r| { let i = r.get_u32(); Id::new(i, r.get_u32()) }, |n| Id::new(n * 3, n * 3 + 1));
simple_field!(sid, StrId, 1, 0, |p, v| p.put_u32(v), |r| r.get_u32(), |n| n);
simple_field!(u32, u32, 1, 0, |p, v| p.put_u32(v), |r| r.get_u32(), |n| n * 5);
simple_field!(i32, i32, 1, 0, |p, v| p.put_u32(v as u32), |r| r.get_u32() as i32, |n| -(n as i32) * 3);
simple_field!(u64, u64, 2, 0, |p, v| p.put_u64(v), |r| r.get_u64(), |n| (n as u64) << 33 | 0x1_0000_0001);
simple_field!(bool, bool, 1, 0, |p, v| p.put_u32(v as u32), |r| r.get_u32() != 0, |n| n % 2 == 1);
simple_field!(f32, f32, 1, 0, |p, v| p.put_u32(v.to_bits()), |r| f32::from_bits(r.get_u32()), |n| n as f32 + 0.5);
simple_field!(f64, f64, 0, 1, |p, v| p.put_f64(v), |r| r.get_f64(), |n| n as f64 + 0.25);
simple_field!(f64w, f64, 2, 0, |p, v| p.put_u64(v.to_bits()), |r| f64::from_bits(r.get_u64()), |n| n as f64 + 0.25);
simple_field!(
    rect,
    [f32; 4],
    4,
    0,
    |p, v| for x in v {
        p.put_u32(x.to_bits())
    },
    |r| [f32::from_bits(r.get_u32()), f32::from_bits(r.get_u32()), f32::from_bits(r.get_u32()), f32::from_bits(r.get_u32())],
    |n| [n as f32, n as f32 + 0.5, n as f32 + 1.0, n as f32 + 1.5]
);
simple_field!(v2, [f32; 2], 2, 0, |p, v| for x in v { p.put_u32(x.to_bits()) }, |r| [f32::from_bits(r.get_u32()), f32::from_bits(r.get_u32())], |n| [n as f32, n as f32 + 0.5]);
simple_field!(urect, [u32; 4], 4, 0, |p, v| for x in v { p.put_u32(x) }, |r| [r.get_u32(), r.get_u32(), r.get_u32(), r.get_u32()], |n| [n, n + 1, n + 2, n + 3]);
simple_field!(u16x2, [u16; 2], 1, 0, |p, v| p.put_u32(v[0] as u32 | (v[1] as u32) << 16), |r| { let w = r.get_u32(); [w as u16, (w >> 16) as u16] }, |n| [n as u16, n as u16 + 1]);
simple_field!(
    attr,
    Attr,
    4,
    0,
    |p, v| {
        p.put_u32(v.owner);
        p.put_u32(v.site);
        p.put_u32(v.label);
        p.put_u32(v.kind)
    },
    |r| Attr { owner: r.get_u32(), site: r.get_u32(), label: r.get_u32(), kind: r.get_u32() },
    |n| Attr { owner: n, site: n + 1, label: n + 2, kind: n + 3 }
);

impl Field for k::rng {
    type T = Range;
    type In<'a> = &'a [u8];
    type Out<'a> = &'a [u8];
    const UW: usize = 0;
    const FW: usize = 0;
    #[inline]
    fn pack(v: Range, p: &mut Packer) {
        p.data = v;
    }
    #[inline]
    fn lower<'a, S: ArenaSink>(s: &mut S, v: &'a [u8]) -> Range {
        s.put_bytes(v)
    }
    #[inline]
    fn read<'a>(r: &mut Reader<'a>) -> &'a [u8] {
        r.bytes(r.data)
    }
    fn expect<'a>(v: Self::In<'a>) -> Self::Out<'a> {
        v
    }
    fn sample(_n: u32) -> &'static [u8] {
        b"sample payload"
    }
}

/// A borrowed run of memory in the words (ptr, len): the writer points at the caller's bytes
/// instead of copying them (texture and buffer uploads). It decodes to the raw [`Slice`]: the
/// consumer reads it with `Slice::as_bytes` inside the `hal_submit` call, never later.
impl Field for k::slice {
    type T = Slice;
    type In<'a> = &'a [u8];
    type Out<'a> = Slice;
    const UW: usize = 4;
    const FW: usize = 0;
    #[inline]
    fn pack(v: Slice, p: &mut Packer) {
        p.put_u64(v.ptr as usize as u64);
        p.put_u64(v.len as u64);
    }
    #[inline]
    fn lower<'a, S: ArenaSink>(_s: &mut S, v: &'a [u8]) -> Slice {
        Slice::from_bytes(v)
    }
    #[inline]
    fn read<'a>(r: &mut Reader<'a>) -> Slice {
        let ptr = r.get_u64() as usize as *const u8;
        Slice { ptr, len: r.get_u64() as usize }
    }
    fn expect<'a>(v: Self::In<'a>) -> Self::Out<'a> {
        Slice::from_bytes(v)
    }
    fn sample(_n: u32) -> &'static [u8] {
        b"sample slice"
    }
}

/// As [`k::slice`], with the length in one word: for commands that need the other words (texture
/// uploads: rect, layer and the run). A run of 4 GiB or more is cut to its first 4 GiB - 1.
impl Field for k::slice32 {
    type T = Slice;
    type In<'a> = &'a [u8];
    type Out<'a> = Slice;
    const UW: usize = 3;
    const FW: usize = 0;
    #[inline]
    fn pack(v: Slice, p: &mut Packer) {
        p.put_u64(v.ptr as usize as u64);
        p.put_u32(v.len.min(u32::MAX as usize) as u32);
    }
    #[inline]
    fn lower<'a, S: ArenaSink>(_s: &mut S, v: &'a [u8]) -> Slice {
        Slice::from_bytes(v)
    }
    #[inline]
    fn read<'a>(r: &mut Reader<'a>) -> Slice {
        let ptr = r.get_u64() as usize as *const u8;
        Slice { ptr, len: r.get_u32() as usize }
    }
    fn expect<'a>(v: Self::In<'a>) -> Self::Out<'a> {
        Slice::from_bytes(v)
    }
    fn sample(_n: u32) -> &'static [u8] {
        b"sample slice"
    }
}
