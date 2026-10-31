//! Print one `<name> <size> <align>` line per layout, with stable labels for diffing.
//! Uses only std, so `src/main.rs` is also a standalone rustc entry point.
//! Compare rustc and Relax on the same target: std layouts can be platform-specific.

pub mod code_graph;
pub mod specimens;

use code_graph::*;
use specimens::*;
use std::alloc::Layout;
use std::borrow::Cow;
use std::cell::{Cell, OnceCell, RefCell, UnsafeCell};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::mem::{align_of, size_of};
use std::num::{
    NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI128, NonZeroIsize,
    NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize,
};
use std::ops::{Range, RangeFrom, RangeFull, RangeInclusive, RangeTo};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::{self, Rc};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{self, Arc, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

fn report<T>(name: &str) {
    println!("{} {} {}", name, size_of::<T>(), align_of::<T>());
}

fn report_value<T>(name: &str, _: &T) {
    report::<T>(name);
}

macro_rules! layouts {
    ($($name:literal => $ty:ty),* $(,)?) => {
        $(report::<$ty>($name);)*
    };
}

// Explicit labels avoid differences in compiler-generated type names or spacing.
macro_rules! with_options {
    ($($name:literal => $ty:ty),* $(,)?) => {
        $(
            report::<$ty>($name);
            report::<Option<$ty>>(concat!("Option<", $name, ">"));
        )*
    };
}

/// Deterministic BTreeMap/BTreeSet exercise (inserts, removes, ranges, iteration both ways,
/// entry, split_off, append, retain, clone): one checksum line, equal under rustc and Relax.
fn btree_check() {
    use std::collections::{BTreeMap, BTreeSet};
    let mut x: u64 = 0x9E3779B97F4A7C15;
    let mut rnd = move || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
    let mut m: BTreeMap<u32, u64> = BTreeMap::new();
    let mut s: BTreeSet<String> = BTreeSet::new();
    let mut h: u64 = 1469598103934665603;
    let mix = |v: u64, h: &mut u64| { *h ^= v; *h = h.wrapping_mul(1099511628211); };
    for i in 0..200_000u64 {
        let r = rnd();
        let k = (r % 5000) as u32;
        match r % 7 {
            0 | 1 | 2 => { if let Some(old) = m.insert(k, i) { mix(old, &mut h); } }
            3 => { if let Some(v) = m.remove(&k) { mix(v ^ 7, &mut h); } }
            4 => { *m.entry(k).or_insert(0) += 3; }
            5 => { for (a, b) in m.range(k..k.saturating_add(40)) { mix(*a as u64 ^ *b, &mut h); } }
            _ => { s.insert(format!("{:05}", k)); if r % 3 == 0 { s.remove(&format!("{:05}", (k + 1) % 5000)); } }
        }
        if i % 25_000 == 0 {
            for (a, b) in m.iter().rev().take(50) { mix(*a as u64 + *b, &mut h); }
            let mut tail = m.split_off(&2500);
            tail.retain(|k, _| k % 3 != 0);
            let c = tail.clone();
            m.append(&mut tail);
            mix(c.len() as u64, &mut h);
            m.retain(|_, v| *v % 11 != 0);
        }
    }
    for (a, b) in &m { mix(*a as u64 * 31 + *b, &mut h); }
    for k in &s { for b in k.bytes() { mix(b as u64, &mut h); } }
    mix(m.first_key_value().map_or(0, |(a, _)| *a as u64), &mut h);
    mix(m.last_key_value().map_or(0, |(a, _)| *a as u64), &mut h);
    println!("btree_check {} {} {:016x}", m.len(), s.len(), h);
}

/// io::Error behaviour (kinds, OS codes, messages, custom payloads): equal under rustc and Relax.
fn io_check() {
    use std::io::{Error, ErrorKind};
    let e = std::fs::File::open("/nonexistent/memlane/probe").unwrap_err();
    println!("io_os {:?} {:?} {}", e.kind(), e.raw_os_error(), e);
    let e = Error::from(ErrorKind::TimedOut);
    println!("io_simple {:?} {:?} {} {:?}", e.kind(), e.raw_os_error(), e, e);
    let e = Error::new(ErrorKind::Other, "custom message");
    println!("io_custom_kind {:?}", e.kind());
    println!("io_custom_ref {:?}", e.get_ref().map(|r| r.to_string()));
    println!("io_custom {}", e);
    println!("io_custom_inner {}", e.into_inner().map_or(0, |b| b.to_string().len()));
    let e = Error::other(format!("x{}", 5));
    println!("io_other {:?} {}", e.kind(), e);
    let e = Error::from_raw_os_error(13);
    println!("io_raw {:?} {:?} {}", e.kind(), e.raw_os_error(), e);
    let r: Result<(), Error> = Err(Error::new(ErrorKind::InvalidData, "bad"));
    println!("io_result {:?}", r.map_err(|e| (e.kind(), e.to_string())));
    let w = std::io::Write::write_all(&mut [0u8; 2].as_mut_slice(), b"abc").unwrap_err();
    println!("io_write {:?} {}", w.kind(), w);
}

fn main() {
    btree_check();
    io_check();
    // Primitives and integer niches.
    with_options! {
        "u8" => u8,
        "u16" => u16,
        "u32" => u32,
        "u64" => u64,
        "u128" => u128,
        "usize" => usize,
        "i8" => i8,
        "i16" => i16,
        "i32" => i32,
        "i64" => i64,
        "i128" => i128,
        "isize" => isize,
        "bool" => bool,
        "char" => char,
        "f32" => f32,
        "f64" => f64,
        "()" => (),
        "Infallible" => Infallible,
        "NonZeroU8" => NonZeroU8,
        "NonZeroU16" => NonZeroU16,
        "NonZeroU32" => NonZeroU32,
        "NonZeroU64" => NonZeroU64,
        "NonZeroU128" => NonZeroU128,
        "NonZeroUsize" => NonZeroUsize,
        "NonZeroI8" => NonZeroI8,
        "NonZeroI16" => NonZeroI16,
        "NonZeroI32" => NonZeroI32,
        "NonZeroI64" => NonZeroI64,
        "NonZeroI128" => NonZeroI128,
        "NonZeroIsize" => NonZeroIsize,
    }

    // Thin and fat pointers, including owning and weak pointers.
    with_options! {
        "&u8" => &'static u8,
        "&u64" => &'static u64,
        "&()" => &'static (),
        "&[u8;4]" => &'static [u8; 4],
        "&[u8]" => &'static [u8],
        "&[u64]" => &'static [u64],
        "&[()]" => &'static [()],
        "&str" => &'static str,
        "&dyn(ProbeTrait)" => &'static dyn ProbeTrait,
        "&mut(u8)" => &'static mut u8,
        "&mut([u8])" => &'static mut [u8],
        "&mut(str)" => &'static mut str,
        "&mut(dyn(ProbeTrait))" => &'static mut dyn ProbeTrait,
        "*const(u8)" => *const u8,
        "*mut(u8)" => *mut u8,
        "*const([u8])" => *const [u8],
        "*mut([u8])" => *mut [u8],
        "*const(str)" => *const str,
        "*const(dyn(ProbeTrait))" => *const dyn ProbeTrait,
        "NonNull<u8>" => NonNull<u8>,
        "NonNull<()>" => NonNull<()>,
        "NonNull<[u8]>" => NonNull<[u8]>,
        "NonNull<str>" => NonNull<str>,
        "NonNull<dyn(ProbeTrait)>" => NonNull<dyn ProbeTrait>,
        "Box<u8>" => Box<u8>,
        "Box<u64>" => Box<u64>,
        "Box<()>" => Box<()>,
        "Box<[u8;4]>" => Box<[u8; 4]>,
        "Box<[u8]>" => Box<[u8]>,
        "Box<[()]>" => Box<[()]>,
        "Box<str>" => Box<str>,
        "Box<dyn(ProbeTrait)>" => Box<dyn ProbeTrait>,
        "Rc<u32>" => Rc<u32>,
        "Rc<str>" => Rc<str>,
        "Rc<[u8]>" => Rc<[u8]>,
        "Rc<dyn(ProbeTrait)>" => Rc<dyn ProbeTrait>,
        "rc::Weak<u32>" => rc::Weak<u32>,
        "rc::Weak<str>" => rc::Weak<str>,
        "rc::Weak<[u8]>" => rc::Weak<[u8]>,
        "rc::Weak<dyn(ProbeTrait)>" => rc::Weak<dyn ProbeTrait>,
        "Arc<u32>" => Arc<u32>,
        "Arc<str>" => Arc<str>,
        "Arc<[u8]>" => Arc<[u8]>,
        "Arc<dyn(ProbeTrait)>" => Arc<dyn ProbeTrait>,
        "sync::Weak<u32>" => sync::Weak<u32>,
        "sync::Weak<str>" => sync::Weak<str>,
        "sync::Weak<[u8]>" => sync::Weak<[u8]>,
        "sync::Weak<dyn(ProbeTrait)>" => sync::Weak<dyn ProbeTrait>,
    }

    // Interior mutability and synchronization.
    with_options! {
        "Cell<bool>" => Cell<bool>,
        "Cell<u32>" => Cell<u32>,
        "Cell<NonZeroU32>" => Cell<NonZeroU32>,
        "Cell<Option<NonZeroU32>>" => Cell<Option<NonZeroU32>>,
        "RefCell<u32>" => RefCell<u32>,
        "RefCell<Vec<u8>>" => RefCell<Vec<u8>>,
        "UnsafeCell<bool>" => UnsafeCell<bool>,
        "UnsafeCell<u32>" => UnsafeCell<u32>,
        "UnsafeCell<NonZeroU32>" => UnsafeCell<NonZeroU32>,
        "OnceCell<u32>" => OnceCell<u32>,
        "OnceCell<NonZeroU32>" => OnceCell<NonZeroU32>,
        "OnceCell<String>" => OnceCell<String>,
        "OnceLock<u32>" => OnceLock<u32>,
        "Mutex<u32>" => Mutex<u32>,
        "RwLock<u32>" => RwLock<u32>,
        "AtomicBool" => AtomicBool,
        "AtomicU64" => AtomicU64,
        "Sender<u32>" => Sender<u32>,
        "Receiver<u32>" => Receiver<u32>,
        "SyncSender<u32>" => SyncSender<u32>,
        "JoinHandle<()>" => JoinHandle<()>,
    }

    // Sequences, strings and collections; Vec also covers zero-sized elements.
    with_options! {
        "Vec<u8>" => Vec<u8>,
        "Vec<u32>" => Vec<u32>,
        "Vec<u64>" => Vec<u64>,
        "Vec<bool>" => Vec<bool>,
        "Vec<NonZeroU32>" => Vec<NonZeroU32>,
        "Vec<String>" => Vec<String>,
        "Vec<()>" => Vec<()>,
        "Vec<[u8;0]>" => Vec<[u8; 0]>,
        "Vec<Zst>" => Vec<Zst>,
        "Vec<AlignedZst>" => Vec<AlignedZst>,
        "String" => String,
        "Cow<'static,str>" => Cow<'static, str>,
        "Cow<'static,[u8]>" => Cow<'static, [u8]>,
        "PathBuf" => PathBuf,
        "OsString" => OsString,
        "HashMap<u32,u64>" => HashMap<u32, u64>,
        "HashMap<String,Vec<u8>>" => HashMap<String, Vec<u8>>,
        "HashSet<u32>" => HashSet<u32>,
        "BTreeMap<u32,u64>" => BTreeMap<u32, u64>,
        "BTreeMap<String,Vec<String>>" => BTreeMap<String, Vec<String>>,
        "BTreeSet<u32>" => BTreeSet<u32>,
        "VecDeque<u8>" => VecDeque<u8>,
        "BinaryHeap<u32>" => BinaryHeap<u32>,
    }

    // Time, ranges, allocation, errors, formatting and function pointers.
    with_options! {
        "Duration" => Duration,
        "Instant" => Instant,
        "SystemTime" => SystemTime,
        "Range<u8>" => Range<u8>,
        "Range<u32>" => Range<u32>,
        "Range<usize>" => Range<usize>,
        "RangeInclusive<u8>" => RangeInclusive<u8>,
        "RangeInclusive<u32>" => RangeInclusive<u32>,
        "RangeFrom<u32>" => RangeFrom<u32>,
        "RangeTo<u32>" => RangeTo<u32>,
        "RangeFull" => RangeFull,
        "Ordering" => Ordering,
        "Layout" => Layout,
        "io::Error" => io::Error,
        "fmt::Arguments<'static>" => fmt::Arguments<'static>,
        "fn()" => fn(),
        "fn(u8,u64)->u64" => fn(u8, u64) -> u64,
        "fn(&u8)->&u8" => fn(&u8) -> &u8,
        "unsafe_fn(*const(u8))->u8" => unsafe fn(*const u8) -> u8,
        "extern_C_fn(u32)->u32" => extern "C" fn(u32) -> u32,
    }

    // Tuple reordering, padding and arrays.
    with_options! {
        "(u8,)" => (u8,),
        "(u8,u32)" => (u8, u32),
        "(u32,u8)" => (u32, u8),
        "(u8,u32,u8,u64,u16)" => (u8, u32, u8, u64, u16),
        "(u32,u32,Option<u32>)" => (u32, u32, Option<u32>),
        "(u32,u32,u32,u8)" => (u32, u32, u32, u8),
        "([u8;3],u64)" => ([u8; 3], u64),
        "(bool,u32)" => (bool, u32),
        "(char,u64)" => (char, u64),
        "(NonZeroU32,u8)" => (NonZeroU32, u8),
        "(Vec<u8>,u32)" => (Vec<u8>, u32),
        "(u8,Vec<u8>)" => (u8, Vec<u8>),
        "[u8;0]" => [u8; 0],
        "[u8;1]" => [u8; 1],
        "[u8;3]" => [u8; 3],
        "[u8;16]" => [u8; 16],
        "[u8;20]" => [u8; 20],
        "[u8;32]" => [u8; 32],
        "[u16;3]" => [u16; 3],
        "[u64;0]" => [u64; 0],
        "[u64;3]" => [u64; 3],
        "[u128;2]" => [u128; 2],
        "[();16]" => [(); 16],
        "[bool;3]" => [bool; 3],
        "[NonZeroU32;3]" => [NonZeroU32; 3],
        "[Option<u32>;3]" => [Option<u32>; 3],
    }

    // Result variants with and without payload niches.
    with_options! {
        "Result<(),()>" => Result<(), ()>,
        "Result<u8,u8>" => Result<u8, u8>,
        "Result<u32,u64>" => Result<u32, u64>,
        "Result<u64,u32>" => Result<u64, u32>,
        "Result<bool,()>" => Result<bool, ()>,
        "Result<char,()>" => Result<char, ()>,
        "Result<NonZeroU32,()>" => Result<NonZeroU32, ()>,
        "Result<(),NonZeroU32>" => Result<(), NonZeroU32>,
        "Result<NonZeroU8,NonZeroU8>" => Result<NonZeroU8, NonZeroU8>,
        "Result<&u8,()>" => Result<&'static u8, ()>,
        "Result<(),&u8>" => Result<(), &'static u8>,
        "Result<Vec<u8>,()>" => Result<Vec<u8>, ()>,
        "Result<Vec<u8>,u32>" => Result<Vec<u8>, u32>,
        "Result<String,()>" => Result<String, ()>,
        "Result<String,Vec<u8>>" => Result<String, Vec<u8>>,
        "Result<Box<u32>,()>" => Result<Box<u32>, ()>,
        "Result<u32,Infallible>" => Result<u32, Infallible>,
        "Result<Infallible,u32>" => Result<Infallible, u32>,
        "Result<(),io::Error>" => Result<(), io::Error>,
        "Result<Vec<u8>,io::Error>" => Result<Vec<u8>, io::Error>,
        "Result<Option<NonZeroU32>,()>" => Result<Option<NonZeroU32>, ()>,
        "Result<Duration,()>" => Result<Duration, ()>,
    }

    // Synthetic variants, explicit reprs and generic fields.
    with_options! {
        "Zst" => Zst,
        "AlignedZst" => AlignedZst,
        "Unit2" => Unit2,
        "Unit3" => Unit3,
        "Unit256" => Unit256,
        "Unit257" => Unit257,
        "OneData<bool>" => OneData<bool>,
        "OneData<char>" => OneData<char>,
        "OneData<&u32>" => OneData<&'static u32>,
        "OneData<NonZeroU32>" => OneData<NonZeroU32>,
        "OneData<Vec<u8>>" => OneData<Vec<u8>>,
        "OneData<String>" => OneData<String>,
        "OneData<Box<u32>>" => OneData<Box<u32>>,
        "OneData<Box<str>>" => OneData<Box<str>>,
        "SeveralEmpty<bool>" => SeveralEmpty<bool>,
        "SeveralEmpty<char>" => SeveralEmpty<char>,
        "SeveralEmpty<&u32>" => SeveralEmpty<&'static u32>,
        "SeveralEmpty<NonZeroU32>" => SeveralEmpty<NonZeroU32>,
        "SeveralEmpty<Vec<u8>>" => SeveralEmpty<Vec<u8>>,
        "SeveralEmpty<String>" => SeveralEmpty<String>,
        "SeveralEmpty<Box<u32>>" => SeveralEmpty<Box<u32>>,
        "Step" => Step,
        "Basis" => Basis,
        "DataU8" => DataU8,
        "DataC" => DataC,
        "DataCU8" => DataCU8,
        "Mixed" => Mixed,
        "MixedC" => MixedC,
        "MixedPacked" => MixedPacked,
        "MixedAligned" => MixedAligned,
        "MixedCAligned" => MixedCAligned,
        "Generic<u8>" => Generic<u8>,
        "Generic<u64>" => Generic<u64>,
        "Generic<NonZeroU32>" => Generic<NonZeroU32>,
        "Generic<Vec<u8>>" => Generic<Vec<u8>>,
        "GenericC<u8>" => GenericC<u8>,
        "GenericC<u64>" => GenericC<u64>,
        "GenericC<NonZeroU32>" => GenericC<NonZeroU32>,
        "GenericC<Vec<u8>>" => GenericC<Vec<u8>>,
        "Pair<u8,u64>" => Pair<u8, u64>,
        "Pair<bool,Vec<u8>>" => Pair<bool, Vec<u8>>,
        "Transparent<bool>" => Transparent<bool>,
        "Transparent<NonZeroU32>" => Transparent<NonZeroU32>,
    }

    // Scope's records and every non-std type they contain.
    with_options! {
        "GraphSite" => GraphSite,
        "GraphEdge" => GraphEdge,
        "GraphUnresolved" => GraphUnresolved,
        "GraphEntity" => GraphEntity,
        "GraphScope" => GraphScope,
        "GraphMetrics" => GraphMetrics,
        "GraphStep" => GraphStep,
        "GraphBasis" => GraphBasis,
        "GraphSiteDetail" => GraphSiteDetail,
        "GraphByteSpan" => GraphByteSpan,
        "GraphEdgeKind" => GraphEdgeKind,
        "GraphEntityKind" => GraphEntityKind,
        "GraphVisibility" => GraphVisibility,
        "GraphLookup" => GraphLookup,
        "GraphNamespace" => GraphNamespace,
        "GraphReceiver" => GraphReceiver,
        "GraphTypeRole" => GraphTypeRole,
        "GraphOwnership" => GraphOwnership,
        "GraphWidgetNamespace" => GraphWidgetNamespace,
        "GraphDepKind" => GraphDepKind,
        "GraphBodyStatus" => GraphBodyStatus,
        "GraphScopeKind" => GraphScopeKind,
        "GraphStableKey" => GraphStableKey,
        "GraphDeclKey" => GraphDeclKey,
        "GraphStableInput" => GraphStableInput,
        "GraphDeclInput" => GraphDeclInput,
        "GraphDocumentStamp" => GraphDocumentStamp,
        "GraphSymbolRef" => GraphSymbolRef,
        "GraphCodeRef" => GraphCodeRef,
        "GraphAnalysisContext" => GraphAnalysisContext,
        "GraphPathPosition" => GraphPathPosition,
        "Vec<GraphStep>" => Vec<GraphStep>,
        "Vec<GraphStableKey>" => Vec<GraphStableKey>,
        "Vec<GraphSite>" => Vec<GraphSite>,
        "Vec<GraphEdge>" => Vec<GraphEdge>,
        "Arc<GraphDocumentStamp>" => Arc<GraphDocumentStamp>,
        "Arc<GraphAnalysisContext>" => Arc<GraphAnalysisContext>,
        "(u32,u32,GraphNamespace,u32)" => (u32, u32, GraphNamespace, u32),
        "(GraphScopeId,GraphNamespace,GraphSym)" => (GraphScopeId, GraphNamespace, GraphSym),
    }

    // A second option needs another unused representation.
    layouts! {
        "Option<Option<bool>>" => Option<Option<bool>>,
        "Option<Option<char>>" => Option<Option<char>>,
        "Option<Option<u8>>" => Option<Option<u8>>,
        "Option<Option<u32>>" => Option<Option<u32>>,
        "Option<Option<NonZeroU8>>" => Option<Option<NonZeroU8>>,
        "Option<Option<NonZeroU32>>" => Option<Option<NonZeroU32>>,
        "Option<Option<&u8>>" => Option<Option<&'static u8>>,
        "Option<Option<&str>>" => Option<Option<&'static str>>,
        "Option<Option<NonNull<u8>>>" => Option<Option<NonNull<u8>>>,
        "Option<Option<Box<u32>>>" => Option<Option<Box<u32>>>,
        "Option<Option<Box<[u8]>>>" => Option<Option<Box<[u8]>>>,
        "Option<Option<Rc<u32>>>" => Option<Option<Rc<u32>>>,
        "Option<Option<Arc<u32>>>" => Option<Option<Arc<u32>>>,
        "Option<Option<Vec<u8>>>" => Option<Option<Vec<u8>>>,
        "Option<Option<Vec<()>>>" => Option<Option<Vec<()>>>,
        "Option<Option<String>>" => Option<Option<String>>,
        "Option<Option<Duration>>" => Option<Option<Duration>>,
        "Option<Option<Instant>>" => Option<Option<Instant>>,
        "Option<Option<Ordering>>" => Option<Option<Ordering>>,
        "Option<Option<Layout>>" => Option<Option<Layout>>,
        "Option<Option<fn()>>" => Option<Option<fn()>>,
        "Option<Option<Basis>>" => Option<Option<Basis>>,
        "Option<Option<GraphBasis>>" => Option<Option<GraphBasis>>,
    }

    // Measure closure environments without coercing them to fn pointers.
    let tuple = (1u8, 2u64);
    let by_value = move || tuple;
    report_value("closure(move_tuple_u8_u64)", &by_value);
    report_value("Option<closure(move_tuple_u8_u64)>", &Some(by_value));
    report_value(
        "Option<Option<closure(move_tuple_u8_u64)>>",
        &Some(Some(by_value)),
    );

    let a = 1u8;
    let b = 2u64;
    let split = move || (a, b);
    report_value("closure(move_u8_u64)", &split);
    report_value("Option<closure(move_u8_u64)>", &Some(split));
    report_value("Option<Option<closure(move_u8_u64)>>", &Some(Some(split)));

    let by_reference = || tuple;
    report_value("closure(borrow_tuple_u8_u64)", &by_reference);
    report_value("Option<closure(borrow_tuple_u8_u64)>", &Some(by_reference));
    report_value(
        "Option<Option<closure(borrow_tuple_u8_u64)>>",
        &Some(Some(by_reference)),
    );
}
