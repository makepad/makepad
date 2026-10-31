//! The page's half of the bridge, natively: what web.js / wasm_bridge.js do with the messages, for
//! a host that plays the page itself (the trace host `makepad-web-trace`). The derives give every
//! message type both directions: `FromWasmRead` reads a message the program wrote (the generated
//! `FromWasmMsg` JS readers), `ToWasmWrite` writes one for it to read (the generated `ToWasmMsg`
//! JS writers), with the same u32 stream (u64 values aligned to an even u32).

use crate::to_wasm::ToWasmMsgRef;

/// A message the program wrote, read by the page.
pub trait FromWasmRead: Sized {
    fn read_from_wasm(inp: &mut ToWasmMsgRef) -> Self;
}

/// A message the page writes for the program.
pub trait ToWasmWrite {
    fn write_to_wasm(&self, out: &mut ToWasmWriter);
}

/// The u32 stream of one or more ToWasm blocks (wasm_bridge.js `ToWasmMsg`), and where byte
/// buffers go: `data_u8` copies one into the program's memory (`wasm_new_data_u8`) and answers its
/// address, as `push_data_u8` does.
pub struct ToWasmWriter<'a> {
    pub u32s: Vec<u32>,
    pub data_u8: &'a mut dyn FnMut(&[u8]) -> u32,
}

impl<'a> ToWasmWriter<'a> {
    pub fn new(data_u8: &'a mut dyn FnMut(&[u8]) -> u32) -> Self {
        // the header word (u64 length and capacity) is the buffer's; blocks start at u32 2
        Self { u32s: vec![0, 0], data_u8 }
    }
    pub fn push_u32(&mut self, v: u32) {
        self.u32s.push(v);
    }
    pub fn push_f64(&mut self, v: f64) {
        if self.u32s.len() & 1 != 0 {
            self.u32s.push(0);
        }
        let b = v.to_bits();
        self.u32s.push(b as u32);
        self.u32s.push((b >> 32) as u32);
    }
    /// `push_str`: the string's UTF-16 code units, as JS strings hold them.
    pub fn push_str(&mut self, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        self.u32s.push(units.len() as u32);
        self.u32s.extend(units.iter().map(|u| *u as u32));
    }
    /// One block: its id, the block-end word, the body, aligned to a whole u64; the end word is
    /// the absolute u64 index one past the block (ToWasmMsgRef::block_skip).
    pub fn block(&mut self, id: u64, body: impl FnOnce(&mut Self)) {
        if self.u32s.len() & 1 != 0 {
            self.u32s.push(0);
        }
        self.u32s.push(id as u32);
        self.u32s.push((id >> 32) as u32);
        let end_at = self.u32s.len();
        self.u32s.push(0);
        body(self);
        if self.u32s.len() & 1 != 0 {
            self.u32s.push(0);
        }
        self.u32s[end_at] = (self.u32s.len() >> 1) as u32;
    }
    /// The finished buffer as u64 words; word 0 is left for the program's header.
    pub fn into_words(mut self) -> Vec<u64> {
        if self.u32s.len() & 1 != 0 {
            self.u32s.push(0);
        }
        self.u32s.chunks(2).map(|c| c[0] as u64 | (c[1] as u64) << 32).collect()
    }
}

macro_rules! prim {
    ($t:ty, $read:expr, $write:expr) => {
        impl FromWasmRead for $t {
            fn read_from_wasm(inp: &mut ToWasmMsgRef) -> Self {
                $read(inp)
            }
        }
        impl ToWasmWrite for $t {
            fn write_to_wasm(&self, out: &mut ToWasmWriter) {
                $write(*self, out)
            }
        }
    };
}
prim!(u32, |i: &mut ToWasmMsgRef| i.read_u32(), |v: u32, o: &mut ToWasmWriter| o.push_u32(v));
prim!(usize, |i: &mut ToWasmMsgRef| i.read_u32() as usize, |v: usize, o: &mut ToWasmWriter| o.push_u32(v as u32));
prim!(bool, |i: &mut ToWasmMsgRef| i.read_u32() != 0, |v: bool, o: &mut ToWasmWriter| o.push_u32(v as u32));
prim!(f32, |i: &mut ToWasmMsgRef| i.read_f32(), |v: f32, o: &mut ToWasmWriter| o.push_u32(v.to_bits()));
prim!(f64, |i: &mut ToWasmMsgRef| i.read_f64(), |v: f64, o: &mut ToWasmWriter| o.push_f64(v));

impl FromWasmRead for String {
    /// `read_str`: one code point per u32
    fn read_from_wasm(inp: &mut ToWasmMsgRef) -> Self {
        inp.read_string()
    }
}
impl ToWasmWrite for String {
    fn write_to_wasm(&self, out: &mut ToWasmWriter) {
        out.push_str(self)
    }
}

impl<T: FromWasmRead> FromWasmRead for Vec<T> {
    fn read_from_wasm(inp: &mut ToWasmMsgRef) -> Self {
        let n = inp.read_u32();
        (0..n).map(|_| T::read_from_wasm(inp)).collect()
    }
}
impl<T: ToWasmWrite> ToWasmWrite for Vec<T> {
    fn write_to_wasm(&self, out: &mut ToWasmWriter) {
        out.push_u32(self.len() as u32);
        for x in self {
            x.write_to_wasm(out);
        }
    }
}

impl<T: FromWasmRead, const N: usize> FromWasmRead for [T; N] {
    fn read_from_wasm(inp: &mut ToWasmMsgRef) -> Self {
        std::array::from_fn(|_| T::read_from_wasm(inp))
    }
}
impl<T: ToWasmWrite, const N: usize> ToWasmWrite for [T; N] {
    fn write_to_wasm(&self, out: &mut ToWasmWriter) {
        for x in self {
            x.write_to_wasm(out);
        }
    }
}

impl<T: FromWasmRead> FromWasmRead for Option<T> {
    fn read_from_wasm(inp: &mut ToWasmMsgRef) -> Self {
        if inp.read_u32() != 0 { Some(T::read_from_wasm(inp)) } else { None }
    }
}
impl<T: ToWasmWrite> ToWasmWrite for Option<T> {
    fn write_to_wasm(&self, out: &mut ToWasmWriter) {
        match self {
            Some(x) => {
                out.push_u32(1);
                x.write_to_wasm(out);
            }
            None => out.push_u32(0),
        }
    }
}

impl<T: FromWasmRead> FromWasmRead for Box<T> {
    fn read_from_wasm(inp: &mut ToWasmMsgRef) -> Self {
        Box::new(T::read_from_wasm(inp))
    }
}
impl<T: ToWasmWrite> ToWasmWrite for Box<T> {
    fn write_to_wasm(&self, out: &mut ToWasmWriter) {
        (**self).write_to_wasm(out)
    }
}

/// The program's memory as the page sees it: the address of wasm address 0 (the trace host sets
/// it). A `WasmDataU8` the program hands the page is read from there.
pub static MEMORY_BASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

std::thread_local! {
    /// The `{ptr, len, capacity}` of every buffer read since the last `take_data_u8_frees`: the
    /// page gives each back with `wasm_free_data_u8` once it has the bytes, as web.js does.
    static READ_DATA_U8: std::cell::RefCell<Vec<(u32, u32, u32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub fn take_data_u8_frees() -> Vec<(u32, u32, u32)> {
    READ_DATA_U8.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

/// The bytes at wasm address `ptr` (`len` long) of the program's memory.
///
/// # Safety
/// `MEMORY_BASE` is the program's memory and `ptr..ptr + len` lies in it.
pub unsafe fn memory_bytes(ptr: u32, len: u32) -> &'static [u8] {
    if len == 0 {
        return &[];
    }
    let base = MEMORY_BASE.load(std::sync::atomic::Ordering::Relaxed);
    std::slice::from_raw_parts((base | ptr as usize) as *const u8, len as usize)
}

impl FromWasmRead for crate::WasmDataU8 {
    /// `{ptr, len, capacity}`: the page copies the bytes and owes the program the free
    fn read_from_wasm(inp: &mut ToWasmMsgRef) -> Self {
        let (ptr, len, cap) = (inp.read_u32(), inp.read_u32(), inp.read_u32());
        READ_DATA_U8.with(|r| r.borrow_mut().push((ptr, len, cap)));
        crate::WasmDataU8::from_vec_u8(unsafe { memory_bytes(ptr, len) }.to_vec())
    }
}
impl ToWasmWrite for crate::WasmDataU8 {
    /// `push_data_u8`: a copy in the program's memory, by address and length
    fn write_to_wasm(&self, out: &mut ToWasmWriter) {
        let bytes = self.as_slice();
        let ptr = (out.data_u8)(bytes);
        out.push_u32(ptr);
        out.push_u32(bytes.len() as u32);
    }
}
