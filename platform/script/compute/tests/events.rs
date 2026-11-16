//! A neutral event front end: declarations, shared state, scripted ports and user sites.

use std::collections::VecDeque;

use makepad_script_compute::ir::{self, PortSig, Ty};
use makepad_script_compute::lower::event::{lower_events, DeclArg, DeclForm, DeclHandle, EventLowered, EventSpec};
use makepad_script_compute::{lower, parse, parse_prelude, wasm, with_prelude, ShaderError};

const PORTS: &[PortSig] = &[
    PortSig { name: "send", params: &[Ty::I32, Ty::I32], rets: &[] },
    PortSig { name: "sample", params: &[Ty::I32], rets: &[Ty::I32] },
    PortSig { name: "clock", params: &[], rets: &[Ty::I32] },
    PortSig { name: "cap", params: &[], rets: &[] },
];

const FORMS: &[DeclForm] = &[
    DeclForm { name: "channel", handle: DeclHandle::Arg(0), class: 0 },
    DeclForm { name: "periodic", handle: DeclHandle::Ordinal, class: 1 },
    DeclForm { name: "once", handle: DeclHandle::Ordinal, class: 1 },
];

const SPEC: EventSpec<'static> = EventSpec { ports: PORTS, forms: FORMS, entry_fns: &["init"], max_entry_cost: 100_000, cap_port: Some(3), marks: false };
const PRELUDE: &str = "fn pulse(c: i32) { send(c, 1) send(c, 0) }";

// Put init after the declarations: named entries still precede their handlers.
const SOURCE: &str = "\
let a = channel(3)
let b = channel(7)
var n: i32 = 0
var initial: i32 = -1
var sampled: i32 = 0
var stamp: i32 = 0
var capped: i32 = 0

let repeat = periodic(10, fn() {
    n = n + 1
    sampled = sample(b)
    stamp = clock()
    send(a, n)
    send(repeat, stamp)
})

let later = once(20, fn() {
    send(b, sampled)
    send(later, n)
    var i: i32 = 0
    while i >= 0 {
        i = i + 1
    }
    capped = i
})

fn init() {
    initial = n
    send(a, initial)
    pulse(b)
}
";

const WHILE: &str = "while i >= 0 {\n        i = i + 1\n    }";

fn lower_source(source: &str, spec: &EventSpec<'_>) -> Result<EventLowered, ShaderError> {
    let toks = parse::lex(source)?;
    let items = parse::Parser::new(&toks).items()?;
    let own = items.iter().map(|item| item.name().to_string()).collect();
    let all = with_prelude(&items, parse_prelude(PRELUDE, source.len()), &own);
    lower_events(&all, source.len() + 1, spec)
}

#[derive(Debug)]
struct Call {
    port: u16,
    site: u32,
    args: Vec<u32>,
}

struct Recording {
    calls: Vec<Call>,
    answers: VecDeque<(u16, i32)>,
}

impl ir::Ports for Recording {
    fn call(&mut self, port: u16, site: u32, args: &[u32], rets: &mut [u32]) {
        let sig = PORTS.get(port as usize).unwrap_or_else(|| panic!("unexpected port {port} at site {site}"));
        assert_eq!(args.len(), sig.params.len(), "{} received the wrong argument count", sig.name);
        assert_eq!(rets.len(), sig.rets.len(), "{} received the wrong result count", sig.name);
        self.calls.push(Call { port, site, args: args.to_vec() });
        if matches!(port, 1 | 2) {
            let (expected, answer) = self.answers.pop_front().unwrap_or_else(|| panic!("no scripted answer for {} at site {site}", sig.name));
            assert_eq!(port, expected, "sample and clock consumed the script out of order");
            rets[0] = answer as u32;
        }
    }
}

fn run<'a>(lowered: &EventLowered, entry: usize, state: &mut [u32], ports: &'a mut Recording) -> &'a [Call] {
    let program = &lowered.entries[entry].program;
    let mut scratch = vec![0; program.scratch_words()];
    let mut mem = ir::Mem { ctx: &mut [], state, shared: ir::Shared::Read(&lowered.shared_init), bufs: &[] };
    let mut io = ir::Io { ins: [&[], &[]], outs: [&mut [], &mut []] };
    let first = ports.calls.len();
    ir::run_ports(program, &mut scratch, &mut mem, &mut io, 1, ports);
    &ports.calls[first..]
}

fn state_word(lowered: &EventLowered, state: &[u32], name: &str) -> i32 {
    let var = lowered.state_vars.iter().find(|var| var.name == name).unwrap_or_else(|| panic!("missing state variable {name}"));
    assert_eq!(var.words, 1, "{name} must occupy one integer state word");
    state[var.offset as usize] as i32
}

fn span_of(source: &str, text: &str) -> parse::Span {
    let start = source.find(text).unwrap_or_else(|| panic!("missing source text {text:?}"));
    parse::Span { start, end: start + text.len() }
}

#[test]
fn source_marks_are_optional_and_transparent() {
    use ir::{Op, Program, Stmt, Val};

    const SOURCE: &str = "fn init() { let unused = fn() { send(9, 9) } send(3, 4) pulse(7) }";
    let off = lower_source(SOURCE, &SPEC).expect("unmarked source must lower");
    let on = lower_source(SOURCE, &EventSpec { marks: true, ..SPEC }).expect("marked source must lower");
    let plain = &off.entries[0].program;
    let marked = &on.entries[0].program;
    // The pre-mark lowering of this source: constants and three port calls,
    // the two in the prelude sharing their user's call site.
    let baseline = Program {
        vals: vec![Ty::I32; 5],
        body: vec![
            Stmt::Def(Val(0), Op::ConstI(3)),
            Stmt::Def(Val(1), Op::ConstI(4)),
            Stmt::Port { port: 0, args: vec![Val(0), Val(1)], rets: vec![], site: 0 },
            Stmt::Def(Val(2), Op::ConstI(7)),
            Stmt::Def(Val(3), Op::ConstI(1)),
            Stmt::Port { port: 0, args: vec![Val(2), Val(3)], rets: vec![], site: 1 },
            Stmt::Def(Val(4), Op::ConstI(0)),
            Stmt::Port { port: 0, args: vec![Val(2), Val(4)], rets: vec![], site: 1 },
        ],
        ..Default::default()
    };
    fn strip(p: &Program) -> (Program, Vec<u32>) {
        fn block(b: &mut ir::Block, marks: &mut Vec<u32>) {
            b.retain_mut(|s| match s {
                Stmt::Mark(n) => { marks.push(*n); false }
                Stmt::If(_, t, e) => { block(t, marks); block(e, marks); true }
                Stmt::Loop { body, .. } => { block(body, marks); true }
                _ => true,
            });
        }
        let mut p = p.clone();
        let mut marks = Vec::new();
        block(&mut p.body, &mut marks);
        for f in &mut p.funcs {
            block(&mut f.body, &mut marks);
        }
        (p, marks)
    }
    assert!(off.marks.is_empty() && strip(plain).1.is_empty(), "disabled marks create neither table entries nor IR statements");
    assert_eq!(format!("{plain:?}"), format!("{baseline:?}"), "disabled marks preserve the pre-change structural IR");
    assert_eq!(on.marks, [span_of(SOURCE, "send(3, 4)"), span_of(SOURCE, "pulse(7)")], "spans follow user source order and exclude lambda bindings and prelude statements");
    assert_eq!(strip(marked).1, [0, 1], "each emitted source statement starts with its table index");
    check_marks(marked, "lowering");
    assert_eq!(format!("{:?}", strip(marked).0), format!("{plain:?}"), "lowering and its optimizations preserve every non-mark statement");
    assert_eq!(marked.total_cost(), plain.total_cost(), "marks have zero total cost");
    assert_eq!(marked.cost(), plain.cost(), "marks have zero weighted cost");
    assert_eq!(ir::to_words(marked), ir::to_words(plain), "word encoding omits debug info without changing the format");
    assert!(ir::validate_ports(marked, &on.regions(&on.entries[0]), PORTS).is_ok(), "marks are valid without operands or definitions");
    let mut outputs = Vec::new();
    for lowered in [&off, &on] {
        let mut state = lowered.state_init.clone();
        let mut ports = Recording { calls: Vec::new(), answers: VecDeque::new() };
        run(lowered, 0, &mut state, &mut ports);
        outputs.push((state, ports.calls.into_iter().map(|c| (c.port, c.site, c.args)).collect::<Vec<_>>()));
    }
    assert_eq!(outputs[0], outputs[1], "interpreter state and port calls are identical with marks enabled");
    for (name, pass) in [
        ("optimize", makepad_script_compute::opt::optimize as fn(&mut Program)),
        ("fuse_fma", makepad_script_compute::opt::fuse_fma),
    ] {
        let (mut a, mut b) = (plain.clone(), marked.clone());
        pass(&mut a);
        pass(&mut b);
        assert_eq!(format!("{a:?}"), format!("{:?}", strip(&b).0), "{name}: marks do not change non-mark statements");
        check_marks(&b, name);
    }

    // Exercise the passes which move statements, flatten branches, or use
    // adjacency, with marks on every statement and at block ends.
    use ir::{Bin, Cmp, Region, Var};
    let mut movement = Program {
        vals: (0..15).map(|k| if k == 7 { Ty::Bool } else { Ty::I32 }).collect(),
        vars: vec![Ty::I32],
        body: vec![
            Stmt::Def(Val(0), Op::FrameCount),
            Stmt::Def(Val(1), Op::ConstI(1)),
            Stmt::Def(Val(2), Op::ConstI(2)),
            Stmt::Def(Val(3), Op::Load { region: Region::Shared, base: 0, extent: 8, off: Some(Val(0)) }),
            Stmt::Def(Val(4), Op::Bin(Bin::AddI, Val(3), Val(1))),
            Stmt::Port { port: 0, args: vec![Val(0), Val(4)], rets: vec![], site: 0 },
            Stmt::Def(Val(5), Op::Load { region: Region::Shared, base: 1, extent: 7, off: Some(Val(0)) }),
            Stmt::Def(Val(6), Op::Bin(Bin::AddI, Val(3), Val(5))),
            Stmt::Def(Val(7), Op::CmpI(Cmp::Gt, Val(0), Val(1))),
            Stmt::If(Val(7), vec![Stmt::Def(Val(8), Op::Bin(Bin::AddI, Val(0), Val(1))), Stmt::Set(Var(0), Val(8))], vec![Stmt::Def(Val(9), Op::Bin(Bin::AddI, Val(0), Val(2))), Stmt::Set(Var(0), Val(9))]),
            Stmt::Call { f: 0, args: vec![Val(0), Val(1)], rets: vec![] },
            Stmt::Loop {
                cap: 2,
                body: vec![
                    Stmt::Def(Val(10), Op::Bin(Bin::MulI, Val(0), Val(1))),
                    Stmt::Def(Val(11), Op::Get(Var(0))),
                    Stmt::Def(Val(12), Op::Bin(Bin::AddI, Val(11), Val(10))),
                    Stmt::Set(Var(0), Val(12)),
                    Stmt::Store { region: Region::Buf(0), base: 0, extent: 8, off: Some(Val(0)), val: Val(12) },
                ],
            },
            Stmt::Store { region: Region::Buf(1), base: 0, extent: 8, off: Some(Val(0)), val: Val(4) },
            Stmt::Def(Val(13), Op::Bin(Bin::AddI, Val(0), Val(2))),
            Stmt::Store { region: Region::Buf(1), base: 1, extent: 7, off: Some(Val(0)), val: Val(13) },
            Stmt::Loop { cap: 2, body: vec![Stmt::Def(Val(14), Op::ConstI(99))] },
        ],
        funcs: vec![Program {
            vals: vec![Ty::I32; 3],
            params: vec![Val(0), Val(1)],
            body: vec![
                Stmt::Def(Val(2), Op::Bin(Bin::AddI, Val(0), Val(1))),
                Stmt::Store { region: Region::State, base: 0, extent: 1, off: None, val: Val(2) },
                Stmt::Port { port: 0, args: vec![Val(0), Val(2)], rets: vec![], site: 1 },
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    fn mark_all(b: &mut ir::Block, next: &mut u32) {
        let mut out = Vec::new();
        for mut s in std::mem::take(b) {
            out.push(Stmt::Mark(*next));
            *next += 1;
            match &mut s {
                Stmt::If(_, t, e) => { mark_all(t, next); mark_all(e, next); }
                Stmt::Loop { body, .. } => mark_all(body, next),
                _ => {}
            }
            out.push(s);
        }
        out.push(Stmt::Mark(*next));
        *next += 1;
        *b = out;
    }
    fn check_marks(p: &Program, name: &str) {
        let marks = strip(p).1;
        let mut seen = std::collections::HashSet::new();
        for n in &marks {
            assert!(seen.insert(*n), "{name}: once: mark {n} appears more than once");
        }
        assert!(marks.windows(2).all(|pair| pair[0] < pair[1]), "{name}: order: marks must rise in pre-order: {marks:?}");
    }
    fn effect_marks(p: &Program, name: &str) -> std::collections::HashMap<String, Option<u32>> {
        fn block(b: &ir::Block, path: &str, mark: &mut Option<u32>, out: &mut std::collections::HashMap<String, Option<u32>>, name: &str) {
            for s in b {
                let source = *mark;
                // These fixture identities survive value substitution; the
                // block path distinguishes branch and loop assignments.
                let key = match s {
                    Stmt::Mark(n) => { *mark = Some(*n); continue; }
                    Stmt::Def(..) => continue,
                    Stmt::If(c, t, e) => {
                        let key = format!("{path}/if{}", c.0);
                        block(t, &format!("{key}/then"), mark, out, name);
                        block(e, &format!("{key}/else"), mark, out, name);
                        key
                    }
                    Stmt::Loop { body, .. } => {
                        block(body, &format!("{path}/loop"), mark, out, name);
                        continue;
                    }
                    Stmt::Set(v, _) => format!("{path}/set{}", v.0),
                    Stmt::Store { region, base, .. } => format!("{path}/store{region:?}/{base}"),
                    Stmt::Port { site, .. } => format!("{path}/port{site}"),
                    Stmt::Call { f, .. } => format!("{path}/call{f}"),
                    Stmt::CallHost { f, .. } => format!("{path}/host{f}"),
                    Stmt::Out { ch, .. } => format!("{path}/out{ch}"),
                    Stmt::Break(d) => format!("{path}/break{d}"),
                    Stmt::Continue(d) => format!("{path}/continue{d}"),
                };
                assert!(out.insert(key.clone(), source).is_none(), "{name}: effect identity {key} must be unique in the fixture");
            }
        }
        let mut out = std::collections::HashMap::new();
        block(&p.body, "body", &mut None, &mut out, name);
        for (i, f) in p.funcs.iter().enumerate() {
            block(&f.body, &format!("function{i}"), &mut None, &mut out, name);
        }
        out
    }
    let mut next = 0;
    mark_all(&mut movement.body, &mut next);
    for f in &mut movement.funcs {
        mark_all(&mut f.body, &mut next);
    }
    check_marks(&movement, "movement input");
    let before = effect_marks(&movement, "movement input");
    let regions = ir::Regions { state: 1, shared: 8, bufs: vec![true, true], ..Default::default() };
    assert!(ir::validate_ports(&movement, &regions, PORTS).is_ok(), "movement input: validity: every nested block must be valid");
    for (name, pass) in [
        ("licm", makepad_script_compute::opt::licm as fn(&mut Program)),
        ("if_convert", makepad_script_compute::opt::if_convert),
        ("cluster_loads", makepad_script_compute::opt::cluster_loads),
        ("cluster_stores", makepad_script_compute::opt::cluster_stores),
        ("dead_loops", makepad_script_compute::opt::dead_loops),
        ("optimize", makepad_script_compute::opt::optimize),
    ] {
        let (mut a, mut b) = (strip(&movement).0, movement.clone());
        pass(&mut a);
        pass(&mut b);
        assert!(ir::validate_ports(&b, &regions, PORTS).is_ok(), "{name}: marked motion preserves valid IR");
        assert_eq!(format!("{a:?}"), format!("{:?}", strip(&b).0), "{name}: marks do not inhibit motion, flattening, fusion or dead-code removal");
        assert_eq!(a.total_cost(), b.total_cost(), "{name}: marks have zero total cost");
        assert_eq!(a.cost(), b.cost(), "{name}: marks have zero weighted cost");
        assert_eq!(ir::to_words(&a), ir::to_words(&b), "{name}: marks do not change word encoding");
        check_marks(&b, name);
        let mut expected = before.clone();
        if matches!(name, "if_convert" | "optimize") {
            let source = expected.remove("body/if7").expect("movement input: the If must have a source mark");
            expected.remove("body/if7/then/set0");
            expected.remove("body/if7/else/set0");
            expected.insert("body/set0".into(), source);
        }
        for (effect, mark) in effect_marks(&b, name) {
            assert_eq!(Some(&mark), expected.get(&effect), "{name}: effect boundary: surviving {effect} retains its source mark");
        }
    }

    // Both assignments use values computed under branch-local marks.
    // Flattening must leave the selected Set under the enclosing If's mark.
    let mut selected = Program {
        vals: vec![Ty::Bool, Ty::I32, Ty::I32, Ty::I32],
        vars: vec![Ty::I32],
        params: vec![Val(0), Val(1)],
        body: vec![
            Stmt::Mark(0),
            Stmt::If(Val(0), vec![
                Stmt::Mark(1),
                Stmt::Def(Val(2), Op::Bin(Bin::AddI, Val(1), Val(1))),
                Stmt::Mark(2),
                Stmt::Set(Var(0), Val(2)),
            ], vec![
                Stmt::Mark(3),
                Stmt::Def(Val(3), Op::Bin(Bin::MulI, Val(1), Val(1))),
                Stmt::Mark(4),
                Stmt::Set(Var(0), Val(3)),
            ]),
            Stmt::Mark(5),
        ],
        ..Default::default()
    };
    makepad_script_compute::opt::if_convert(&mut selected);
    check_marks(&selected, "if_convert computed branches");
    assert_eq!(strip(&selected).1, [0, 5], "if_convert: flattening drops every branch mark");
    assert_eq!(effect_marks(&selected, "if_convert computed branches"), std::collections::HashMap::from([("body/set0".into(), Some(0))]), "if_convert: effect boundary: the merged Set belongs to the If's mark");
}

fn check_calls(lowered: &EventLowered, calls: &[Call], expected: &[(u16, &[u32], &str)], context: &str) {
    assert_eq!(calls.len(), expected.len(), "{context}: missing or extra port calls: {calls:?}");
    for (k, (call, &(port, args, source))) in calls.iter().zip(expected).enumerate() {
        assert_eq!((call.port, call.args.as_slice()), (port, args), "{context}: port or arguments of call {k}");
        let site = lowered.sites.get(call.site as usize).unwrap_or_else(|| panic!("{context}: call {k} has unknown site {}", call.site));
        assert_eq!(*site, span_of(SOURCE, source), "{context}: call {k} must name the user's source span for {source:?}");
    }
}

#[test]
fn event_entries_share_state_and_report_user_sites() {
    let lowered = lower_source(SOURCE, &SPEC).unwrap_or_else(|e| panic!("the event fixture must lower: {e}"));
    let declarations: Vec<_> = lowered.decls.iter().map(|d| (d.name.as_str(), d.form, d.handle)).collect();
    assert_eq!(declarations, [("a", 0, 3), ("b", 0, 7), ("repeat", 1, 0), ("later", 2, 1)], "argument handles and shared-class ordinals must follow source order");
    assert_eq!(lowered.decls[0].args, [DeclArg::Int(3)], "channel a must retain its constant argument");
    assert_eq!(lowered.decls[1].args, [DeclArg::Int(7)], "channel b must retain its constant argument");
    assert_eq!(lowered.decls[2].args, [DeclArg::Int(10), DeclArg::Entry(1)], "periodic must refer to entry 1 after init");
    assert_eq!(lowered.decls[3].args, [DeclArg::Int(20), DeclArg::Entry(2)], "once must refer to entry 2 after periodic");
    let names: Vec<_> = lowered.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["init", "repeat", "later"], "named entries must precede handlers in source order");
    let handlers = [&lowered.decls[2], &lowered.decls[3]].map(|d| match d.args[1] {
        DeclArg::Entry(k) => k as usize,
        arg => panic!("{} must have an entry argument, got {arg:?}", d.name),
    });

    for entry in &lowered.entries {
        ir::validate_ports(&entry.program, &lowered.regions(entry), PORTS).unwrap_or_else(|e| panic!("{} must validate against the mock ports: {e}", entry.name));
        assert!(ir::has_ports(&entry.program), "{} lost its port statements", entry.name);
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        assert!(makepad_script_compute::native::compile(&entry.program).is_none(), "native must decline ports in {}", entry.name);
        let entries = [wasm::Entry { program: &entry.program, simd: false }];
        assert!(wasm::module(&entries, Default::default()).is_none(), "wasm must decline ports in {}", entry.name);
    }

    let mut state = lowered.state_init.clone();
    let mut ports = Recording { calls: Vec::new(), answers: VecDeque::from([(1, 41), (2, 100), (1, -6), (2, 200), (1, 23), (2, 300)]) };
    assert_eq!(state_word(&lowered, &state, "initial"), -1, "init's observation must start with a distinct integer value");
    let calls = run(&lowered, 0, &mut state, &mut ports);
    check_calls(&lowered, calls, &[(0, &[3, 0], "send(a, initial)"), (0, &[7, 1], "pulse(b)"), (0, &[7, 0], "pulse(b)")], "init");
    assert_eq!(state_word(&lowered, &state, "initial"), 0, "init must observe the counter's initial value");
    assert_eq!(state_word(&lowered, &state, "n"), 0, "init must leave the counter at zero");

    for (k, (sample, stamp)) in [(41, 100), (-6, 200), (23, 300)].into_iter().enumerate() {
        let count = k as u32 + 1;
        let calls = run(&lowered, handlers[0], &mut state, &mut ports);
        check_calls(&lowered, calls, &[(1, &[7], "sample(b)"), (2, &[], "clock()"), (0, &[3, count], "send(a, n)"), (0, &[0, stamp as u32], "send(repeat, stamp)")], "periodic");
        assert_eq!(state_word(&lowered, &state, "n"), count as i32, "periodic run {count} must increment shared state once");
        assert_eq!(state_word(&lowered, &state, "sampled"), sample, "periodic run {count} must store the scripted sample result");
        assert_eq!(state_word(&lowered, &state, "stamp"), stamp, "periodic run {count} must store the scripted clock result");
    }
    assert!(ports.answers.is_empty(), "every scripted sample and clock answer must be consumed");

    for k in 1..=2 {
        let calls = run(&lowered, handlers[1], &mut state, &mut ports);
        check_calls(&lowered, calls, &[(0, &[7, 23], "send(b, sampled)"), (0, &[1, 3], "send(later, n)"), (3, &[], WHILE)], "once");
        assert_eq!(state_word(&lowered, &state, "capped"), lower::LOOP_CAP as i32, "once run {k} must stop its local counter at the lowering's loop cap");
        assert_eq!(state_word(&lowered, &state, "n"), 3, "once run {k} must preserve the three increments");
    }

    let calls = run(&lowered, 0, &mut state, &mut ports);
    check_calls(&lowered, calls, &[(0, &[3, 3], "send(a, initial)"), (0, &[7, 1], "pulse(b)"), (0, &[7, 0], "pulse(b)")], "init after handlers");
    assert_eq!(state_word(&lowered, &state, "initial"), 3, "init must see the state left by the other entries");

    // Corrupt one contract at a time in an otherwise valid entry.
    let entry = &lowered.entries[0];
    for (unknown, expected) in [(true, "no port 4"), (false, "port `send`: wrong arity")] {
        let mut program = entry.program.clone();
        let call = program.body.iter_mut().find(|s| matches!(s, ir::Stmt::Port { .. })).expect("init must contain a direct port call to edit");
        if let ir::Stmt::Port { port, args, .. } = call {
            if unknown {
                *port = PORTS.len() as u16;
            } else {
                args.pop().expect("send must have an argument to remove");
            }
        }
        let error = ir::validate_ports(&program, &lowered.regions(entry), PORTS).expect_err("an edited port contract must be refused");
        assert_eq!(error, expected, "validation must identify the edited port contract");
    }
}

#[test]
fn event_lowering_refuses_invalid_declarations_and_cost() {
    let cases = [
        ("let a = channel(sample(3))\nfn init() { send(3, 0) }", SPEC, "sample(3)", "`channel(..)` takes constants and functions"),
        ("let a = unknown_form(3)\nfn init() { send(3, 0) }", SPEC, "unknown_form(3)", "unknown function `unknown_form`"),
        ("fn init() { send(3, 0) }", EventSpec { max_entry_cost: 0, marks: false, ..SPEC }, "fn init() { send(3, 0) }", "operations; an event's limit is 0"),
    ];
    for (source, spec, at, message) in cases {
        let error = lower_source(source, &spec).err().unwrap_or_else(|| panic!("must refuse {source:?}"));
        assert!(error.message.contains(message), "{source:?}: expected {message:?}, got {error}");
        assert_eq!(parse::Span { start: error.start, end: error.end }, span_of(source, at), "{source:?}: refusal must name the offending user source");
    }
}

#[test]
fn typed_top_level_lets_keep_their_type() {
    // A join of typed i32 constants selects integers; a join of untyped
    // literals keeps the f32 rule and converts where it is used.
    let typed = "let high: i32 = 1\nlet low: i32 = 0\nvar on = false\nfn init() {\n    on = !on\n    send(3, if on { high } else { low })\n}\n";
    let untyped = typed.replace(": i32 = ", " = ");
    let vals = |source: &str| lower_source(source, &SPEC).unwrap_or_else(|e| panic!("{e}\n{source}")).entries[0].program.vals.clone();
    assert!(!vals(typed).iter().any(|t| matches!(t, Ty::F32 | Ty::F64)), "typed i32 constants must join as i32");
    assert!(vals(&untyped).contains(&Ty::F32), "untyped literals must keep the f32 join");
    // Still a constant: a declaration argument and an index into a table.
    let lowered = lower_source("let k: i32 = 7\nlet t: i32 = [5, 6]\nlet a = channel(k)\nfn init() { send(a, t[k - 6]) }\n", &SPEC)
        .unwrap_or_else(|e| panic!("a typed constant must lower: {e}"));
    assert_eq!(lowered.decls[0].args, [DeclArg::Int(7)], "a typed constant is a declaration argument");
    let mut state = lowered.state_init.clone();
    let mut ports = Recording { calls: Vec::new(), answers: VecDeque::new() };
    let calls: Vec<_> = run(&lowered, 0, &mut state, &mut ports).iter().map(|c| (c.port, c.args.clone())).collect();
    assert_eq!(calls, [(0, vec![7, 6])], "a typed constant indexes a table");
    let error = lower_source("let k: i32 = 1.5\nfn init() { send(3, k) }\n", &SPEC).err().expect("a fraction is not an i32");
    assert!(error.message.contains("declared i32"), "unexpected refusal: {error}");
}
