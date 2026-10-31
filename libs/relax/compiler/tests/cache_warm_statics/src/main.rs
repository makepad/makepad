// Statics and constants whose values a warm run takes from the disk cache instead of running
// their initializers, and code keyed by higher-ranked fn-pointer types. The shapes are the
// Wayland protocol tables and the shader descriptors of a makepad app: a static of structs whose
// raw pointers address a promoted array (the initializer's code writes it at run time: it holds
// pointers, so it is not constant bytes), pointers into the middle of other statics, C strings,
// fn pointers, and maps / vectors of `fn(&T)` types. A warm run must print the cold run's line.
use std::collections::HashMap;
use std::ffi::CStr;

#[repr(C)]
struct Message {
    name: *const u8,
    signature: &'static str,
    types: *const *const Interface,
}

#[repr(C)]
struct Interface {
    name: &'static CStr,
    version: u32,
    method_count: u32,
    methods: *const Message,
}
unsafe impl Sync for Interface {}

static PAIR: [u64; 4] = [11, 22, 33, 44];

// as wayland-client's generated protocol tables: promoted slices of pointer-bearing structs
struct InterfacePointers(&'static [*const Interface]);
unsafe impl Sync for InterfacePointers {}
struct Messages(&'static [Message]);
unsafe impl Sync for Messages {}

static ATTACH_TYPES: InterfacePointers = InterfacePointers(&[&SURFACE as *const Interface, std::ptr::null()]);
static SURFACE_REQUESTS: Messages = Messages(&[
    Message { name: c"attach".as_ptr() as *const u8, signature: "?oii", types: ATTACH_TYPES.0.as_ptr() },
    Message { name: c"damage".as_ptr() as *const u8, signature: "iiii", types: std::ptr::null() },
    Message { name: c"commit".as_ptr() as *const u8, signature: "", types: std::ptr::null() },
]);
static SURFACE: Interface = Interface { name: c"surface", version: 6, method_count: 3, methods: SURFACE_REQUESTS.0.as_ptr() };

// a pointer into the middle of another static
static THIRD: &u64 = &PAIR[2];

struct Field {
    name: &'static str,
    kind: Kind,
    read: Option<fn(&dyn std::any::Any) -> Option<u32>>,
}
enum Kind {
    Builtin(&'static str),
    Array(&'static Kind, usize),
}
fn read_u32(v: &dyn std::any::Any) -> Option<u32> {
    v.downcast_ref::<u32>().copied()
}
const FIELDS: &[Field] = {
    static FIELDS: [Field; 2] = [
        Field { name: "pos", kind: Kind::Array(&Kind::Builtin("vec2"), 4), read: None },
        Field { name: "count", kind: Kind::Builtin("u32"), read: Some(read_u32) },
    ];
    &FIELDS
};

const fn build_manifest<const N: usize>(head: &[u8], tail: u8) -> [u8; N] {
    let mut out = [tail; N];
    let mut i = 0;
    while i < head.len() && i < N {
        out[i] = head[i];
        i += 1;
    }
    out
}
const MANIFEST: &[u8; 12] = &build_manifest(b"format=v1", b'!');

fn kind_len(k: &Kind) -> usize {
    match k {
        Kind::Builtin(n) => n.len(),
        Kind::Array(e, n) => kind_len(e) * n,
    }
}

fn twice(v: &dyn std::any::Any) -> Box<dyn std::any::Any> {
    Box::new(v.downcast_ref::<u64>().copied().unwrap_or(0) * 2)
}
fn bump(v: &mut u64) {
    *v += 5;
}

fn main() {
    let mut sum: u64 = 0;
    let s = &SURFACE;
    sum += s.version as u64 + s.method_count as u64 + s.name.to_bytes().len() as u64;
    for i in 0..s.method_count as usize {
        let m = unsafe { &*s.methods.add(i) };
        let name = unsafe { CStr::from_ptr(m.name as *const std::ffi::c_char) };
        sum = sum * 31 + name.to_bytes().len() as u64 + m.signature.len() as u64;
        if !m.types.is_null() {
            let t = unsafe { &**m.types };
            sum += t.version as u64 + (t as *const Interface == s as *const Interface) as u64;
        }
    }
    sum += *THIRD + (THIRD as *const u64 == &PAIR[2] as *const u64) as u64;
    for f in FIELDS {
        sum = sum * 7 + f.name.len() as u64 + kind_len(&f.kind) as u64;
        if let Some(r) = f.read {
            sum += r(&9u32).unwrap_or(0) as u64;
        }
    }
    sum += MANIFEST.iter().map(|b| *b as u64).sum::<u64>();
    let mut ops: HashMap<&str, fn(&dyn std::any::Any) -> Box<dyn std::any::Any>> = HashMap::new();
    ops.insert("twice", twice);
    let r = ops["twice"](&21u64);
    sum += *r.downcast_ref::<u64>().unwrap();
    let mut hooks: Vec<fn(&mut u64)> = Vec::new();
    hooks.push(bump);
    for h in &hooks {
        h(&mut sum);
    }
    println!("cache_warm_statics result {}", sum);
}
