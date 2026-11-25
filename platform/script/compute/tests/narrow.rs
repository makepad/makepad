//! Element representations, event buffer bindings and their memory effects.

use makepad_script_compute::ir::{self, Elem, Op, PortBuf, PortSig, Program, RawBuf, Region, Stmt, Ty, Val};
use makepad_script_compute::lower::event::{lower_events, BufForm, DeclForm, DeclHandle, EventLowered, EventSpec};
use makepad_script_compute::{js, lower, parse, wasm, ShaderError};

const PORTS: &[PortSig] = &[
    PortSig { name: "fill", params: &[Ty::I32; 4], rets: &[], bufs: &[PortBuf { param: 0, elem: None, writes: true }] },
    PortSig { name: "total", params: &[Ty::I32; 3], rets: &[Ty::I32], bufs: &[PortBuf { param: 0, elem: None, writes: false }] },
    PortSig { name: "fill16", params: &[Ty::I32; 4], rets: &[], bufs: &[PortBuf { param: 0, elem: Some(Elem::U16), writes: true }] },
    PortSig { name: "cap", params: &[], rets: &[], bufs: &[] },
];

const FORMS: &[DeclForm] = &[
    DeclForm { name: "channel", handle: DeclHandle::Arg(0), class: 0, buffer: None },
    DeclForm { name: "periodic", handle: DeclHandle::Ordinal, class: 1, buffer: None },
    DeclForm { name: "bytes", handle: DeclHandle::Ordinal, class: 2, buffer: Some(BufForm { elem: Elem::U8, len: &[0], writable: true }) },
    DeclForm { name: "halves", handle: DeclHandle::Ordinal, class: 2, buffer: Some(BufForm { elem: Elem::I16, len: &[0, 1], writable: true }) },
    DeclForm { name: "table", handle: DeclHandle::Ordinal, class: 2, buffer: Some(BufForm { elem: Elem::U16, len: &[0], writable: false }) },
    DeclForm { name: "words", handle: DeclHandle::Ordinal, class: 2, buffer: Some(BufForm { elem: Elem::Word, len: &[0], writable: true }) },
];

const SPEC: EventSpec<'static> = EventSpec {
    ports: PORTS,
    forms: FORMS,
    floats: true,
    entry_fns: &["init", "bounded", "reversed", "unbounded", "floating", "float_start", "float_bound", "literal_start", "literal_bound", "same_cap", "port_loop"],
    max_entry_cost: 100_000,
    cap_port: Some(3),
    marks: false,
    call_ops: Some(0),
};

fn lower_source(source: &str) -> Result<EventLowered, ShaderError> {
    let toks = parse::lex(source)?;
    let items = parse::Parser::new(&toks).items()?;
    lower_events(&items, source.len() + 1, &SPEC)
}

struct Recording {
    bufs: Vec<RawBuf>,
    calls: Vec<(u16, Vec<u32>)>,
}

impl ir::Ports for Recording {
    fn call(&mut self, port: u16, _site: u32, args: &[u32], rets: &mut [u32]) {
        let sig = &PORTS[port as usize];
        assert_eq!(args.len(), sig.params.len(), "{} receives its declared argument count", sig.name);
        assert_eq!(rets.len(), sig.rets.len(), "{} receives its declared result count", sig.name);
        self.calls.push((port, args.to_vec()));
        if port == 3 {
            return;
        }
        let buf = self.bufs[args[0] as usize];
        let mut sum = 0u32;
        for i in 0..args[2] {
            let at = ir::buf_index(args[1], Some(i), buf.len);
            match port {
                0 | 2 => buf.store_word(at, args[3]),
                1 => sum = sum.wrapping_add(buf.load_word(at)),
                _ => panic!("unexpected buffer port {port}"),
            }
        }
        if port == 1 {
            rets[0] = sum;
        }
    }
}

fn state_at(lowered: &EventLowered, name: &str) -> usize {
    let var = lowered.state_vars.iter().find(|v| v.name == name).unwrap_or_else(|| panic!("missing state variable {name}"));
    assert_eq!(var.words, 1, "{name} occupies one state word");
    var.offset as usize
}

fn run(lowered: &EventLowered, name: &str, state: &mut [u32], bufs: &[RawBuf], ports: &mut Recording) {
    let entry = lowered.entries.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("missing entry {name}"));
    let mut scratch = vec![0; entry.program.scratch_words()];
    let mut mem = ir::Mem { ctx: &mut [], state, shared: ir::Shared::Read(&lowered.shared_init), bufs };
    let mut io = ir::Io::default();
    ir::run_ports(&entry.program, &mut scratch, &mut mem, &mut io, 1, ports);
}

fn walk(block: &ir::Block, visit: &mut impl FnMut(&Stmt)) {
    for s in block {
        visit(s);
        match s {
            Stmt::If(_, t, e) => {
                walk(t, visit);
                walk(e, visit);
            }
            Stmt::Loop { body, .. } => walk(body, visit),
            _ => {}
        }
    }
}

#[test]
fn narrow_buffers_in_the_reference() {
    const SOURCE: &str = "\
let c = channel(9)
let bytes = bytes(8)
let halves = halves(2, 3)
let table = table(2)
let words = words(2)
var n: i32 = 2000
var real: f32 = 2000.0
var start: f32 = -100.0
var small: i32 = 0
var signed: i32 = 0
var unsigned: i32 = 0
var word: i32 = 0
var last: i32 = 0
var past: i32 = 0
var length: i32 = 0
var before: i32 = 0
var after: i32 = 0
var sum: i32 = 0
var signed_sum: i32 = 0
var unsigned_sum: i32 = 0
var word_sum: i32 = 0
var cleared: i32 = -1
var doubled: i32 = 0
var hits: i32 = 0
let tick = periodic(10, fn() { fill(words, 0, 1, c) })

fn clear(b) { fill(b, 0, len(b), 0) }
fn twice(x: i32) { return x + x }
fn init() {
    bytes[0] = 300
    small = bytes[0]
    halves[0] = 0x1_8000
    signed = halves[0]
    unsigned = table[0]
    words[0] = 0x1234_5678
    word = words[0]
    bytes[100] = 511
    last = bytes[7]
    past = bytes[100]
    length = len(halves)
    let a = bytes[3]
    fill(bytes, 0, 8, 7)
    let b = bytes[3]
    before = a
    after = b
    sum = total(bytes, 0, 8)
    signed_sum = total(halves, 0, 1)
    unsigned_sum = total(table, 0, 1)
    word_sum = total(words, 0, 1)
    clear(bytes)
    clear(halves)
    cleared = total(bytes, 0, 8) + total(halves, 0, 6)
    doubled = twice(n) + twice(n + 1)
}
fn bounded() { for i in 0..min(n, 64) { hits = hits + 1 } }
fn reversed() { for i in 0..min(64, n) { hits = hits + 1 } }
fn unbounded() { for i in 0..n { hits = hits + 1 } }
fn floating() { for i in -100..min(-64, real) { hits = hits + 1 } }
fn float_start() { for i in start..min(n, -64) { hits = hits + 1 } }
fn float_bound() { for i in 0..min(n, float(64)) { hits = hits + 1 } }
fn literal_start() { for i in 0.0..min(n, 64) { hits = hits + 1 } }
fn literal_bound() { for i in 0..min(n, 64.0) { hits = hits + 1 } }
fn same_cap() { for i in 0..min(n, 1023) { hits = hits + 1 } }
fn port_loop() {
    for i in 0..min(n, 3) {
        let a = halves[1]
        fill(halves, 1, 1, i + 1)
        hits = hits + halves[1] - a
    }
}
";
    let lowered = lower_source(SOURCE).expect("event buffers and buffer helpers lower");
    let metadata: Vec<_> = lowered.buffers.iter().map(|b| (b.decl, b.elem, b.len, b.writable)).collect();
    assert_eq!(metadata, [(1, Elem::U8, 8, true), (2, Elem::I16, 6, true), (3, Elem::U16, 2, false), (4, Elem::Word, 2, true)], "buffer numbering follows declarations and lengths multiply");
    assert_eq!(lowered.decls.iter().map(|d| d.handle).collect::<Vec<_>>(), [9, 0, 1, 2, 3, 0], "buffer bindings preserve declaration handles and class ordinals");
    assert!(lowered.funcs.iter().any(|f| f.name == "twice"), "call_ops zero keeps an eligible numeric helper as a function");
    assert!(lowered.funcs.iter().all(|f| f.name != "clear"), "both buffer helper calls inline despite call_ops zero");
    for e in &lowered.entries {
        assert_eq!(e.program.bufs, [Elem::U8, Elem::I16, Elem::U16, Elem::Word], "{} carries every buffer kind", e.name);
        assert_eq!(lowered.regions(e).bufs, [true, true, false, true], "{} carries buffer writability", e.name);
        assert!(e.program.funcs.iter().all(|f| f.bufs.is_empty()), "{} keeps buffer metadata on the entry only", e.name);
        walk(&e.program.body, &mut |s| assert!(!matches!(s, Stmt::Def(_, Op::BufLen(_))), "{} uses constant declared buffer lengths", e.name));
    }

    let mut bytes = [0u32; 2];
    let mut halves = [0u32; 3];
    let mut table = [u32::from_ne_bytes([0xff, 0xff, 0, 0])];
    let mut words = [0u32; 2];
    let bufs = [
        RawBuf { ptr: bytes.as_mut_ptr(), len: 8, writable: true, elem: Elem::U8 },
        RawBuf { ptr: halves.as_mut_ptr(), len: 6, writable: true, elem: Elem::I16 },
        RawBuf { ptr: table.as_mut_ptr(), len: 2, writable: false, elem: Elem::U16 },
        RawBuf { ptr: words.as_mut_ptr(), len: 2, writable: true, elem: Elem::Word },
    ];
    let mut ports = Recording { bufs: bufs.to_vec(), calls: Vec::new() };
    let mut state = lowered.state_init.clone();
    run(&lowered, "init", &mut state, &bufs, &mut ports);
    for (name, expected, property) in [
        ("small", 44, "U8 stores truncate and loads zero-extend"),
        ("signed", -32768, "I16 stores truncate and loads sign-extend"),
        ("unsigned", 65535, "U16 loads zero-extend read-only memory"),
        ("word", 0x1234_5678, "Word stores and loads preserve all bits"),
        ("last", 255, "past-end stores clamp to the last element"),
        ("past", 255, "past-end loads clamp to the last element"),
        ("length", 6, "len multiplies the declared dimensions"),
        ("before", 0, "the pre-port load keeps its earlier value"),
        ("after", 7, "CSE does not reuse a load across a port"),
        ("sum", 56, "ports read the filled U8 elements"),
        ("signed_sum", -32768, "ports read sign-extended I16 elements"),
        ("unsigned_sum", 65535, "ports read zero-extended U16 elements"),
        ("word_sum", 0x1234_5678, "ports preserve Word values"),
        ("cleared", 0, "both inlined buffer helpers clear their own region"),
        ("doubled", 8002, "outlined numeric helpers still execute"),
    ] {
        assert_eq!(state[state_at(&lowered, name)] as i32, expected, "{property}");
    }
    assert_eq!(ports.calls, [
        (0, vec![0, 0, 8, 7]), (1, vec![0, 0, 8]), (1, vec![1, 0, 1]),
        (1, vec![2, 0, 1]), (1, vec![3, 0, 1]), (0, vec![0, 0, 8, 0]),
        (0, vec![1, 0, 6, 0]), (1, vec![0, 0, 8]), (1, vec![1, 0, 6]),
    ], "ports receive buffer numbers, and each inlined clear uses its own region and constant length");
    assert!(bufs[..2].iter().all(|b| (0..b.len).all(|i| b.load_word(i) == 0)), "both clear calls write every element of their buffer");
    bufs[2].store_word(0, 0);
    assert_eq!(bufs[2].load_word(0), 65535, "RawBuf stores respect read-only narrow memory");
    run(&lowered, "tick", &mut state, &bufs, &mut ports);
    assert_eq!(bufs[3].load_word(0), 9, "declaration handlers share the buffer table and numeric handles");

    for (name, cap, reports) in [
        ("bounded", 65, 0), ("reversed", 65, 0), ("unbounded", lower::LOOP_CAP, 1),
        ("floating", lower::LOOP_CAP, 1), ("float_start", lower::LOOP_CAP, 1),
        ("float_bound", lower::LOOP_CAP, 1), ("same_cap", lower::LOOP_CAP, 0),
        ("literal_start", lower::LOOP_CAP, 1), ("literal_bound", lower::LOOP_CAP, 1),
    ] {
        let e = lowered.entries.iter().find(|e| e.name == name).expect("range entry exists");
        let mut caps = Vec::new();
        let mut found_reports = 0;
        walk(&e.program.body, &mut |s| match s {
            Stmt::Loop { cap, .. } => caps.push(*cap),
            Stmt::Port { port: 3, .. } => found_reports += 1,
            _ => {}
        });
        assert_eq!(caps, [cap], "{name}: only proven integer ranges get a derived cap");
        assert_eq!(found_reports, reports, "{name}: only unproven ranges keep a cap report");
    }
    state[state_at(&lowered, "real")] = f32::NAN.to_bits();
    for (name, count, reports) in [("bounded", 64, 0), ("reversed", 64, 0), ("unbounded", 1024, 1), ("floating", 100, 0), ("port_loop", 3, 0)] {
        state[state_at(&lowered, "hits")] = 0;
        ports.calls.clear();
        run(&lowered, name, &mut state, &bufs, &mut ports);
        assert_eq!(state[state_at(&lowered, "hits")], count, "{name}: range completes or reports its existing runtime cap");
        assert_eq!(ports.calls.iter().filter(|(p, _)| *p == 3).count(), reports, "{name}: cap reports occur only on runtime overflow");
    }

    for (source, causes) in [
        ("let t = table(2) fn init() { t[0] = 1 }", &["t", "read-only"][..]),
        ("fn init() { fill(5, 0, 1, 0) }", &["fill", "buffer name"][..]),
        ("let b = bytes(2) fn init() { fill(b, b, 1, 0) }", &["argument 2", "number"][..]),
        ("let b = bytes(0) fn init() {}", &["bytes", "1..=i32::MAX"][..]),
        ("let b = halves(65536, 65536) fn init() {}", &["halves", "1..=i32::MAX"][..]),
        ("let b = bytes(2147483648) fn init() {}", &["bytes", "integer constants"][..]),
        ("let b = bytes(2) fn init() { fill16(b, 0, 1, 0) }", &["fill16", "element kind"][..]),
        ("let t = table(2) fn init() { fill(t, 0, 1, 0) }", &["fill", "read-only"][..]),
    ] {
        let error = lower_source(source).err().unwrap_or_else(|| panic!("invalid buffer operation must fail: {source}"));
        for cause in causes {
            assert!(error.message.contains(*cause), "buffer diagnostic names {cause:?}: {}", error.message);
        }
    }
}

#[test]
fn narrow_buffers_are_declined_and_round_trip() {
    let p = Program {
        vals: vec![Ty::I32],
        bufs: vec![Elem::U8],
        body: vec![
            Stmt::Call { f: 0, args: vec![], rets: vec![Val(0)] },
            Stmt::Store { region: Region::State, base: 0, extent: 1, off: None, val: Val(0) },
        ],
        funcs: vec![Program {
            vals: vec![Ty::I32],
            results: vec![Val(0)],
            body: vec![Stmt::Def(Val(0), Op::Load { region: Region::Buf(0), base: 0, extent: 8, off: None })],
            ..Default::default()
        }],
        ..Default::default()
    };
    let regions = ir::Regions { ctx: 0, state: 1, shared: 0, frame: 0, shared_writable: false, bufs: vec![true], io: false };
    assert!(ir::validate(&p, &regions).is_ok(), "integer narrow loads validate using the entry's element table");
    let mut invalid = p.clone();
    invalid.funcs[0].vals[0] = Ty::F32;
    assert!(ir::validate(&invalid, &regions).unwrap_err().contains("narrow buffer load"), "a narrow F32 load fails validation");
    invalid = p.clone();
    invalid.bufs.push(Elem::Word);
    assert!(ir::validate(&invalid, &regions).unwrap_err().contains("more host buffers"), "element entries beyond the regions fail validation");
    invalid = p.clone();
    invalid.vals.push(Ty::F32);
    invalid.body.extend([
        Stmt::Def(Val(1), Op::ConstF(1.0)),
        Stmt::Store { region: Region::Buf(0), base: 0, extent: 8, off: None, val: Val(1) },
    ]);
    assert!(ir::validate(&invalid, &regions).unwrap_err().contains("narrow buffer store"), "a narrow F32 store fails validation");

    // A word-only host call must refuse the narrow view before reading or writing it.
    use makepad_script_compute::host;
    let f = host::find("poly.triangulate").expect("the built-in host function is registered");
    invalid = p.clone();
    invalid.funcs.clear();
    invalid.vals = vec![Ty::I32; 3];
    invalid.body = vec![
        Stmt::Def(Val(0), Op::ConstI(0)),
        Stmt::Def(Val(1), Op::ConstI(8)),
        Stmt::CallHost { f, args: vec![], slices: vec![ir::SliceArg { buf: 0, off: Val(0), len: Val(1) }; 2], rets: vec![Val(2)] },
    ];
    assert!(ir::validate(&invalid, &regions).unwrap_err().contains("not word memory"), "host slices cannot reinterpret a narrow buffer as words");
    let mut allocation = [0x1234_5678u32; 2];
    let narrow = RawBuf { ptr: allocation.as_mut_ptr(), len: 8, writable: true, elem: Elem::U8 };
    let slices = [host::SliceRaw { buf: 0, off: 0, len: 8 }; 2];
    assert!(!host::invoke(f, &[], &slices, &mut [0], &[narrow]), "runtime host calls refuse narrow slices");
    assert_eq!(allocation, [0x1234_5678; 2], "a refused host call leaves the narrow allocation untouched");

    assert!(ir::has_narrow(&p), "the entry advertises its narrow buffer");
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    {
        use makepad_script_compute::{native, native_vec};
        assert!(native::compile_with(&p, 1).is_none(), "native scalar code declines narrow buffers");
        assert!(native_vec::compile(&p).is_none(), "native vector code declines narrow buffers");
    }
    assert!(js::function(&p).is_none(), "JavaScript declines narrow buffers");
    assert!(wasm::module(&[wasm::Entry { program: &p, simd: false }], wasm::Target::default()).is_none(), "a wasm module declines an entry with narrow buffers");
    let encoded = ir::to_words(&p);
    let decoded = ir::from_words(&encoded).expect("narrow buffer words decode");
    assert_eq!(decoded.bufs, p.bufs, "word round trip preserves element kinds");
    assert!(decoded.funcs[0].bufs.is_empty(), "word round trip keeps function element tables empty");
    assert_eq!(ir::to_words(&decoded), encoded, "word round trip preserves the whole program");
    assert!(ir::validate(&decoded, &regions).is_ok(), "decoded narrow program validates");
    assert_eq!(ir::flat(&p).bufs, p.bufs, "flattening calls preserves entry element kinds");
    let mut optimized = p.clone();
    makepad_script_compute::opt::optimize(&mut optimized);
    assert_eq!(optimized.bufs, p.bufs, "optimization preserves entry element kinds");

    let mut legacy = p.clone();
    legacy.bufs.clear();
    let legacy_words = ir::to_words(&legacy);
    assert!(!ir::has_narrow(&legacy), "a missing element table means Word buffers");
    assert_eq!(&encoded[..encoded.len() - 2], legacy_words, "the optional entry table is appended after the unchanged legacy program encoding");
    assert_eq!(&encoded[encoded.len() - 2..], &[1, Elem::U8 as u32], "the extension contains its length followed by its kinds");
    assert!(ir::from_words(&legacy_words).expect("legacy words decode").bufs.is_empty(), "legacy words retain their implicit Word representation");
}
