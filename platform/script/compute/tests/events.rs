//! A neutral event front end: declarations, shared state, scripted ports and user sites.

use std::collections::VecDeque;

use makepad_script_compute::ir::{self, PortSig, Ty};
use makepad_script_compute::lower::event::{lower_events, DeclArg, DeclForm, DeclHandle, EventLowered, EventSpec};
use makepad_script_compute::{lower, parse, parse_prelude, wasm, with_prelude, ShaderError};

const PORTS: &[PortSig] = &[
    PortSig { bufs: &[], name: "send", params: &[Ty::I32, Ty::I32], rets: &[] },
    PortSig { bufs: &[], name: "sample", params: &[Ty::I32], rets: &[Ty::I32] },
    PortSig { bufs: &[], name: "clock", params: &[], rets: &[Ty::I32] },
    PortSig { bufs: &[], name: "cap", params: &[], rets: &[] },
];

const FORMS: &[DeclForm] = &[
    DeclForm { buffer: None, name: "channel", handle: DeclHandle::Arg(0), class: 0 },
    DeclForm { buffer: None, name: "periodic", handle: DeclHandle::Ordinal, class: 1 },
    DeclForm { buffer: None, name: "once", handle: DeclHandle::Ordinal, class: 1 },
];

const SPEC: EventSpec<'static> = EventSpec { ports: PORTS, forms: FORMS, entry_fns: &["init"], floats: true, max_entry_cost: 100_000, cap_port: Some(3), marks: false, call_ops: None };
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

/// A front end with marks on learns where its program branches on a source condition: each
/// `if`, `while` and short-circuit `&&` condition has its own mark, placed right before the `If`
/// that tests it (or its negation, a `while`'s exit), in the same rising order as statements.
/// A condition that takes no branch (a select) has a mark in no program.
#[test]
fn condition_marks_name_each_branching_condition() {
    use ir::Stmt;
    const SOURCE: &str = "\
var x: i32 = 0
fn init() {
    if sample(1) > 2 { send(1, 1) }
    var i: i32 = 0
    while i < 3 { i = i + 1 }
    if sample(2) > 0 && sample(3) > 0 { send(2, 2) }
    x = if sample(4) > 1 { x + 5 } else { x + 6 }
}
";
    let off = lower_source(SOURCE, &SPEC).expect("unmarked source must lower");
    let on = lower_source(SOURCE, &EventSpec { marks: true, ..SPEC }).expect("marked source must lower");
    assert!(off.conds.is_empty() && off.marks.is_empty(), "disabled marks record no conditions");
    let conds: Vec<(&str, bool)> = on.conds.iter().map(|&(id, tests)| (&SOURCE[on.marks[id as usize].start..on.marks[id as usize].end], tests)).collect();
    assert_eq!(conds, [
        ("sample(1) > 2", true),
        ("i < 3", false),
        ("sample(2) > 0", true),
        ("sample(2) > 0 && sample(3) > 0", true),
        ("sample(4) > 1", true),
    ], "each condition's span in source order, and whether its `If` tests it or its negation");
    // Every mark in program order, each with what follows it in its block.
    fn walk(b: &[Stmt], out: &mut Vec<(u32, bool)>) {
        for (k, s) in b.iter().enumerate() {
            match s {
                Stmt::Mark(n) => out.push((*n, matches!(b.get(k + 1), Some(Stmt::If(..))))),
                Stmt::If(_, t, e) => { walk(t, out); walk(e, out); }
                Stmt::Loop { body, .. } => walk(body, out),
                _ => {}
            }
        }
    }
    let mut marks = Vec::new();
    walk(&on.entries[0].program.body, &mut marks);
    assert!(marks.windows(2).all(|pair| pair[0].0 < pair[1].0), "condition marks rise in program order with the statements': {marks:?}");
    let placed: Vec<u32> = marks.iter().filter(|m| on.conds.iter().any(|c| c.0 == m.0)).map(|m| m.0).collect();
    assert_eq!(placed, on.conds[..4].iter().map(|c| c.0).collect::<Vec<_>>(), "every branching condition is in the program; the select's is not");
    assert!(marks.iter().filter(|m| placed.contains(&m.0)).all(|m| m.1), "each condition's mark comes right before its `If`");
    assert_eq!(ir::to_words(&on.entries[0].program), ir::to_words(&off.entries[0].program), "the program's words are the same without its marks: a back end emits the same code");
    let mut outputs = Vec::new();
    for lowered in [&off, &on] {
        let mut state = lowered.state_init.clone();
        let mut ports = Recording { calls: Vec::new(), answers: [(1, 3), (1, 1), (1, 1), (1, 2)].into_iter().collect() };
        run(lowered, 0, &mut state, &mut ports);
        outputs.push((state, ports.calls.into_iter().map(|c| (c.port, c.site, c.args)).collect::<Vec<_>>()));
    }
    assert_eq!(outputs[0], outputs[1], "condition marks change neither state nor port calls");
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

#[test]
fn literal_defaults_follow_the_programs_float_type() {
    const SOURCE: &str = "\
let module_number = 17
var from_constant = module_number
var quotient = 7 / 2
var negative_quotient = -7 / 2
var minimum = -2147483648
var maximum = 2147483647
var smallest = min(7, 2)
var largest = max(7, 2)
var clamped = clamp(7, 0, 2)

fn helper() { return 0 }
fn tail() { 9 }

fn init() {
    let bare = 23
    send(3, int(bare))
    send(3, int(helper()))
    send(3, int(helper()))
    send(3, int(tail()))
    send(3, int(tail()))
}
";
    let spec = EventSpec { floats: false, call_ops: Some(0), ..SPEC };
    for floats in [false, true] {
        let lowered = lower_source(SOURCE, &EventSpec { floats, ..spec })
            .unwrap_or_else(|e| panic!("floats={floats}: literal defaults must lower: {e}"));
        let ty = if floats { Ty::F32 } else { Ty::I32 };
        let sig = if floats { "f32" } else { "i32" };
        let program = &lowered.entries[0].program;
        assert!(program.vars.len() > lowered.state_vars.len(), "floats={floats}: the bare let has a local variable");
        assert!(program.vars.iter().all(|t| *t == ty), "floats={floats}: the bare let and inferred result variables default to {ty:?}: {:?}", program.vars);
        for (name, integer, float) in [
            ("from_constant", 17, 17.0f32),
            ("quotient", 3, 3.5),
            ("negative_quotient", -3, -3.5),
            ("minimum", i32::MIN, i32::MIN as f32),
            ("maximum", i32::MAX, i32::MAX as f32),
            ("smallest", 2, 2.0),
            ("largest", 7, 7.0),
            ("clamped", 2, 2.0),
        ] {
            let var = lowered.state_vars.iter().find(|v| v.name == name).expect("initialized state exists");
            assert_eq!(var.sig, sig, "floats={floats}: {name} takes the program's default literal type");
            let expected = if floats { float.to_bits() } else { integer as u32 };
            assert_eq!(lowered.state_init[var.offset as usize], expected, "floats={floats}: {name} keeps its constant value, division rule and i32 boundaries");
        }
        for name in ["helper", "tail"] {
            let index = lowered.funcs.iter().position(|f| f.name == name).unwrap_or_else(|| panic!("{name} must be kept as a function"));
            let function = &program.funcs[index];
            let results: Vec<_> = function.results.iter().map(|v| function.vals[v.0 as usize]).collect();
            assert_eq!(results, [ty], "floats={floats}: {name}'s bare literal result defaults to {ty:?}");
        }
    }
    for (expression, message) in [
        ("1.5", "this program has no float type: write an integer literal"),
        ("2.0", "this program has no float type: write an integer literal"),
        ("1e3", "this program has no float type: write an integer literal"),
        ("PI", "PI needs a float type; this program has none"),
        ("TAU", "TAU needs a float type; this program has none"),
        ("E", "E needs a float type; this program has none"),
        ("1 / 0", "division by zero in a constant"),
        ("1 % 0", "division by zero in a constant"),
        ("2147483647 + 1", "constant 2147483648 does not fit an i32"),
        ("-2147483648 / -1", "constant 2147483648 does not fit an i32"),
        ("-0x80000000", "constant 2147483648 does not fit an i32"),
    ] {
        let source = format!("fn init() {{ let value = {expression} }}");
        let error = lower_source(&source, &spec).err().unwrap_or_else(|| panic!("must refuse {expression}"));
        assert_eq!(error.message, message, "{expression}: refusal names the no-float rule");
        assert_eq!(parse::Span { start: error.start, end: error.end }, span_of(&source, expression), "{expression}: refusal names its own source span");
    }
    for source in [
        "fn init() { let value = 2147483648 }",
        "var value = 2147483648\nfn init() {}",
        "fn init() { send(3, int(2147483648)) }",
        "let value = periodic(2147483648, fn() {})",
    ] {
        let error = lower_source(source, &spec).err().unwrap_or_else(|| panic!("must refuse {source}"));
        assert_eq!(error.message, "constant 2147483648 does not fit an i32", "{source}: literal conversion must not saturate or fall back to float");
    }
    let source = "let table = [{value: 7}]\nlet tick = periodic(1, fn() { send(0, table[1.5].value) })";
    let error = lower_source(source, &spec).err().expect("a constant object lookup must reject a float-written index");
    assert_eq!(error.message, "this program has no float type: write an integer literal", "object lookup folding cannot bypass literal validation");
    assert_eq!(parse::Span { start: error.start, end: error.end }, span_of(source, "1.5"), "the folded object lookup names its index literal");
}

#[test]
fn big_helpers_become_shared_functions_with_the_inlined_behaviour() {
    use ir::{Program, Region, Stmt};

    // `scale` (a local array, a loop, a port, state read and written) is called from three
    // places and from `twice`, itself called twice; the entries have arrays of their own.
    const SOURCE: &str = "\
let a = channel(3)
let b = channel(7)
var total: i32 = 1
var last: i32 = 0
var hits: i32 = 0

fn scale(x: i32) -> i32 {
    let t = [0; 4]
    var i: i32 = 0
    while i < 4 {
        t[i] = float(x * i + total)
        i = i + 1
    }
    send(a, int(t[3]))
    hits = hits + 1
    return int(t[1] + t[2])
}

fn twice(x: i32) -> i32 {
    last = scale(x) + scale(x + 1)
    return last
}

let tick = periodic(10, fn() {
    let keep = [5, 6]
    total = total + twice(int(keep[0]) + sample(b))
    send(b, int(keep[1]) + total)
})

let later = once(20, fn() {
    total = total - scale(hits)
    send(b, last + hits)
})

fn init() {
    send(a, twice(2))
}
";
    let inlined = lower_source(SOURCE, &SPEC).expect("inlined source must lower");
    // Marked (as firmware front ends lower), so marks inside functions are checked to be transparent too.
    let called = lower_source(SOURCE, &EventSpec { call_ops: Some(0), marks: true, ..SPEC }).expect("source with functions must lower");
    assert!(inlined.funcs.is_empty() && inlined.entries.iter().all(|e| e.program.funcs.is_empty()), "None inlines every call");
    let names: Vec<&str> = called.funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"scale") && names.contains(&"twice"), "both helpers are functions: {names:?}");
    for f in &called.funcs {
        assert_eq!(&SOURCE[f.span.start..f.span.start + 3 + f.name.len()], format!("fn {}", f.name), "a function's span is its declaration");
        // Its own statements' marks are in its range (the innermost range that holds them).
        let own = f.marks.clone().filter(|&m| !called.funcs.iter().any(|g| g.name != f.name && g.marks.contains(&m) && f.marks.start <= g.marks.start && g.marks.end <= f.marks.end));
        let spans: Vec<_> = own.map(|m| called.marks[m as usize]).collect();
        assert!(!spans.is_empty() && spans.iter().all(|s| f.span.start <= s.start && s.end <= f.span.end), "`{}` owns the marks of its own statements: {spans:?}", f.name);
    }
    let funcs = &called.entries[0].program.funcs;
    assert_eq!(funcs.len(), called.funcs.len(), "every entry holds every function");
    fn calls(b: &ir::Block) -> usize {
        b.iter().map(|s| match s {
            Stmt::Call { .. } => 1,
            Stmt::If(_, t, e) => calls(t) + calls(e),
            Stmt::Loop { body, .. } => calls(body),
            _ => 0,
        }).sum()
    }
    for e in &called.entries {
        assert_eq!(format!("{:?}", e.program.funcs), format!("{funcs:?}"), "`{}` holds the same functions", e.name);
        assert!(calls(&e.program.body) > 0, "`{}` calls a function instead of inlining it", e.name);
        assert!(e.cost > 0 && e.cost <= SPEC.max_entry_cost, "`{}` has a cost through its calls", e.name);
    }
    // The functions' locals are above every entry's own frame words.
    fn frame_words(b: &ir::Block, out: &mut Vec<u32>) {
        for s in b {
            match s {
                Stmt::Def(_, ir::Op::Load { region: Region::Frame, base, .. }) | Stmt::Store { region: Region::Frame, base, .. } => out.push(*base),
                Stmt::If(_, t, e) => {
                    frame_words(t, out);
                    frame_words(e, out);
                }
                Stmt::Loop { body, .. } => frame_words(body, out),
                _ => {}
            }
        }
    }
    let entry_top = called.entries.iter().map(|e| {
        let mut w = Vec::new();
        frame_words(&e.program.body, &mut w);
        w.into_iter().max().map_or(0, |x| x + 1)
    }).max().unwrap();
    let mut words = Vec::new();
    funcs.iter().for_each(|g: &Program| frame_words(&g.body, &mut words));
    assert!(!words.is_empty() && words.iter().all(|w| *w >= entry_top), "function frame words {words:?} sit above the entries' ({entry_top})");
    assert!(called.entries.iter().all(|e| e.program.frame_words > *words.iter().max().unwrap()), "each entry's frame covers the functions'");

    // Same ports, sites and state, entry by entry, whichever way the helpers are lowered.
    let mut runs = Vec::new();
    for lowered in [&inlined, &called] {
        let mut state = lowered.state_init.clone();
        let mut ports = Recording { calls: Vec::new(), answers: VecDeque::from([(1, 4), (1, -3)]) };
        let order = [lowered.entries.iter().position(|e| e.name == "init").unwrap(), 1, 2, 0, 1];
        let mut log = Vec::new();
        for k in order {
            let entry = if k == 0 { "init" } else { ["", "tick", "later"][k] };
            let at = lowered.entries.iter().position(|e| e.name == entry).unwrap();
            for c in run(lowered, at, &mut state, &mut ports) {
                log.push((entry, c.port, lowered.sites[c.site as usize], c.args.clone()));
            }
            log.push((entry, u16::MAX, span_of(SOURCE, "fn init"), ["total", "last", "hits"].iter().map(|n| state_word(lowered, &state, n) as u32).collect()));
        }
        runs.push(log);
    }
    assert_eq!(runs[0], runs[1], "functions behave as the inlined helpers");
    let inner = span_of(SOURCE, "send(a, int(t[3]))");
    assert!(runs[1].iter().any(|(_, port, site, _)| *port == 0 && *site == inner), "a port inside a function names its own site");
}

#[test]
fn library_helpers_stay_inlined_so_ports_name_the_users_call() {
    use ir::{Block, Stmt};

    // `pulse` is the prelude's two-port wrapper, called from three places; `helper` is the user's,
    // called from two. Shared, `pulse` would give its ports one site, in the prelude.
    const SOURCE: &str = "\
var n: i32 = 1

fn helper(x: i32) {
    pulse(x)
    n = n * 3 + x
}

let tick = periodic(10, fn() {
    helper(n)
    pulse(n)
})

fn init() {
    helper(n + 1)
    pulse(n + 2)
}
";
    let lowered = lower_source(SOURCE, &EventSpec { call_ops: Some(0), ..SPEC }).expect("source with functions must lower");
    let names: Vec<&str> = lowered.funcs.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["helper"], "the user's helper is shared, the library's wrapper is not");
    fn sites(b: &Block, out: &mut Vec<u32>) {
        for s in b {
            match s {
                Stmt::Port { site, .. } => out.push(*site),
                Stmt::If(_, t, e) => {
                    sites(t, out);
                    sites(e, out);
                }
                Stmt::Loop { body, .. } => sites(body, out),
                _ => {}
            }
        }
    }
    let mut used = Vec::new();
    for e in &lowered.entries {
        sites(&e.program.body, &mut used);
    }
    for f in &lowered.entries[0].program.funcs {
        sites(&f.body, &mut used);
    }
    let mut spans: Vec<_> = used.iter().map(|&k| lowered.sites[k as usize]).collect();
    spans.sort_by_key(|s| s.start);
    spans.dedup();
    assert_eq!(spans, [span_of(SOURCE, "pulse(x)"), span_of(SOURCE, "pulse(n)"), span_of(SOURCE, "pulse(n + 2)")],
        "a wrapper inside a shared helper names the helper's call, and each bare call names itself");
}

#[test]
fn literal_arguments_share_a_function_only_where_they_compute_as_the_literal() {
    use ir::Stmt;

    // `pulse` takes its channel handle (a literal) to ports and a comparison: both calls share
    // one function. `half` folds its literal in f64 when inlined (3 / 2 * 2 is 3); as an i32
    // value it would compute 2, so its calls keep copies.
    const SOURCE: &str = "\
let a = channel(3)
let b = channel(7)
var hits: i32 = 0

fn pulse(p) {
    send(p, 1)
    if p == 3 { hits = hits + 1 }
    send(p, 0)
}

fn half(x) {
    send(a, x / 2 * 2)
    hits = hits + 1
}

let tick = periodic(10, fn() {
    pulse(a)
    pulse(b)
    half(3)
    half(7)
    send(b, hits)
})
";
    let inlined = lower_source(SOURCE, &SPEC).expect("inlined source must lower");
    let called = lower_source(SOURCE, &EventSpec { call_ops: Some(0), ..SPEC }).expect("source with functions must lower");
    let names: Vec<&str> = called.funcs.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["pulse"], "calls with different literals share one function where the literal reaches only ports and comparisons");
    fn calls(b: &ir::Block) -> usize {
        b.iter().map(|s| match s {
            Stmt::Call { .. } => 1,
            Stmt::If(_, t, e) => calls(t) + calls(e),
            Stmt::Loop { body, .. } => calls(body),
            _ => 0,
        }).sum()
    }
    let tick = called.entries.iter().find(|e| e.name == "tick").unwrap();
    assert_eq!(calls(&tick.program.body), 2, "both `pulse` calls call the shared function");
    assert_eq!(called.copies, [("half".to_string(), 2)], "the calls that kept a copy are counted per helper");
    assert!(inlined.copies.is_empty(), "without functions nothing is counted");
    let mut runs = Vec::new();
    for lowered in [&inlined, &called] {
        let mut state = lowered.state_init.clone();
        let mut ports = Recording { calls: Vec::new(), answers: VecDeque::new() };
        let at = lowered.entries.iter().position(|e| e.name == "tick").unwrap();
        let log: Vec<_> = run(lowered, at, &mut state, &mut ports).into_iter().map(|c| (c.port, lowered.sites[c.site as usize], c.args.clone())).collect();
        runs.push(log);
    }
    assert_eq!(runs[0], runs[1], "shared and inlined builds make the same port calls");
    let halves: Vec<_> = runs[1].iter().filter(|(_, site, _)| *site == span_of(SOURCE, "send(a, x / 2 * 2)")).map(|(_, _, args)| args[1]).collect();
    assert_eq!(halves, [3, 7], "a literal argument computes as the literal (half(3) is 3), shared or inlined");
}

#[test]
fn hoisted_products_stay_out_of_their_loop_and_inner_products_still_fuse() {
    // `y * k` is invariant in the inner loop: licm hoists it, and fuse_mla must not
    // fuse it back into the inner add. `x * m + y` lives in the inner loop and fuses there.
    // The multipliers are state so no pass folds or strength-reduces the products, and the
    // trip counts are large enough that no pass unrolls the loops.
    let source = "\
var total: i32 = 0
var k: i32 = 46
var m: i32 = 3
let tick = periodic(10, fn() {
    for y in 0..56 {
        for x in 0..46 {
            total = total + y * k + x * m + y
        }
    }
})
";
    let lowered = lower_source(source, &SPEC).expect("nested product fixture lowers");
    let entry = lowered.entries.iter().position(|e| e.name == "tick").expect("tick is an entry");
    fn products(b: &ir::Block, depth: u32, out: &mut Vec<(u32, &'static str)>) {
        for s in b {
            match s {
                ir::Stmt::Def(_, ir::Op::Bin(ir::Bin::MulI, ..)) => out.push((depth, "mul")),
                ir::Stmt::Def(_, ir::Op::Fma(ir::Fma::MulAddI, ..)) => out.push((depth, "mla")),
                ir::Stmt::If(_, t, e) => {
                    products(t, depth, out);
                    products(e, depth, out);
                }
                ir::Stmt::Loop { body, .. } => products(body, depth + 1, out),
                _ => {}
            }
        }
    }
    let mut found = Vec::new();
    products(&lowered.entries[entry].program.body, 0, &mut found);
    assert!(found.contains(&(1, "mul")) || found.contains(&(1, "mla")), "y * 46 is computed in the outer loop: {found:?}");
    assert_eq!(found.iter().filter(|&&(depth, _)| depth == 2).count(), 1, "the inner loop holds only x * 3's product: {found:?}");
    assert!(found.contains(&(2, "mla")), "x * 3 + y still fuses inside the inner loop: {found:?}");
    let mut state = lowered.state_init.clone();
    let mut ports = Recording { calls: Vec::new(), answers: VecDeque::new() };
    run(&lowered, entry, &mut state, &mut ports);
    let expected: i32 = (0..56).flat_map(|y| (0..46).map(move |x| y * 46 + x * 3 + y)).sum();
    assert_eq!(state_word(&lowered, &state, "total"), expected, "the hoisted form computes the same total");
}

/// An event's cost is the bound its handler is checked and timed against: a bound past u64 stays
/// at u64::MAX through every enclosing loop and branch, never wrapping to a small cost.
#[test]
fn total_cost_saturates_through_loops_and_branches() {
    use makepad_script_compute::ir::{Program, Stmt, Val};
    let wide = |body| vec![Stmt::Loop { cap: u32::MAX, body }];
    let program = Program {
        vals: vec![Ty::Bool],
        body: vec![Stmt::If(Val(0), wide(wide(wide(Vec::new()))), Vec::new())],
        ..Default::default()
    };
    assert_eq!(program.total_cost(), u64::MAX, "three nested u32::MAX loops under a branch saturate");
}
