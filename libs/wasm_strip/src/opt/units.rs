//! The module-wise strip: whole units of code (a crate's module: a widget
//! with its draw types, impls and registration; a decoder; an effect family)
//! that a coverage run never used are cut from the linked module.
//!
//! 1. [`instrument`] gives every function an "entered" byte in a memory of
//!    its own, exported as [`COVERAGE_EXPORT`], so app memory is untouched:
//!    each entry stores the current phase there. The phase is 1
//!    (registration) until a phase-mark function is first entered, then 2
//!    (use).
//! 2. The app runs (a film's full replay); the host reads the memory and
//!    [`Coverage::from_memory`] names the functions entered.
//! 3. [`plan`] groups functions into units by their demangled owner path
//!    (crate and module) and keeps every unit one of whose functions was
//!    entered in phase 2, or that a keep entry names. Registration alone
//!    (`script_mod`, run for every module at startup) is not use.
//! 4. [`apply`] stubs the call sites: a direct call of a stripped
//!    registration function drops its arguments (and gives zeros for its
//!    discarded result), so startup skips it; any other call of a stripped function becomes `unreachable`;
//!    table entries and `ref.func`s of stripped functions point at one
//!    shared trapping stub per type. Indices stay valid; dead-code
//!    elimination then removes the stripped bodies and everything only they
//!    referenced, and [`zero_dead_data`](super::data::zero_dead_data) clears the data only they pointed
//!    at (the embedded Splash sources, theme and icon text of a stripped
//!    widget) and splits the segments at the cleared runs.

use super::data::{code_anchors, zero_dead_data, DataLive};
use super::demangle::demangle;
use super::ir::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// The export the instrumented module's coverage memory goes by.
pub const COVERAGE_EXPORT: &str = "__cov";
/// The phase byte of a function entered only while the app registered.
pub const PHASE_REGISTRATION: u8 = 1;
/// The phase byte of a function entered after.
pub const PHASE_USE: u8 = 2;

/// The demangled name of a symbol, else the symbol.
fn readable(symbol: &str) -> String {
    demangle(symbol).map(|d| d.name).unwrap_or_else(|| symbol.to_string())
}

/// The unit a function belongs to: the first two segments of its owner path
/// (`makepad_widgets_core::button`), the crate alone for a crate-root item.
pub fn unit_of(symbol: &str) -> Option<String> {
    let d = demangle(symbol)?;
    match d.owner.len() {
        0 => None,
        1 | 2 => Some(d.owner[0].clone()),
        _ => Some(format!("{}::{}", d.owner[0], d.owner[1])),
    }
}

/// A registration function: a module's `script_mod`, which every module
/// runs at startup whether the app uses it or not.
pub fn is_registration(symbol: &str) -> bool {
    readable(symbol).ends_with("::script_mod")
}

/// Adds the coverage memory, the phase global and the entry stores.
/// `phase_marks`: substrings of demangled names; the first entry of a
/// function matching one ends the registration phase.
pub fn instrument(module: &mut Module, phase_marks: &[String]) -> Result<(), String> {
    let imported = module.num_imported_funcs();
    let count = imported as usize + module.funcs.len();
    let pages = (count.max(1) as u32 + 0xffff) / 0x10000;
    let memory = module.memory_types().len() as u32;
    module.memories.push(MemType { limits: Limits { min: pages, max: Some(pages) }, shared: false });
    let phase = module.global_types().len() as u32;
    module.globals.push(Global {
        ty: GlobalType { ty: ValType::I32, mutable: true },
        init: vec![Instr::I32Const(PHASE_REGISTRATION as i32), Instr::End],
    });
    module.exports.push(Export { name: COVERAGE_EXPORT.into(), kind: ExternKind::Memory, index: memory });
    let names: HashMap<u32, &str> = module.names.as_ref().map(|n| n.funcs.iter().map(|(i, s)| (*i, s.as_str())).collect()).unwrap_or_default();
    let mut marked = 0;
    for (i, func) in module.funcs.iter_mut().enumerate() {
        let index = imported + i as u32;
        let mut prefix = Vec::new();
        if !phase_marks.is_empty() {
            if let Some(name) = names.get(&index) {
                let name = readable(name);
                if phase_marks.iter().any(|m| name.contains(m.as_str())) {
                    prefix.push(Instr::I32Const(PHASE_USE as i32));
                    prefix.push(Instr::GlobalSet(phase));
                    marked += 1;
                }
            }
        }
        prefix.push(Instr::I32Const(0));
        prefix.push(Instr::GlobalGet(phase));
        prefix.push(Instr::Store(0x3a, MemArg { align: 0, offset: index, memory }));
        func.body.splice(0..0, prefix);
    }
    if !phase_marks.is_empty() && marked == 0 {
        return Err(format!("no function matches the phase marks {phase_marks:?}"));
    }
    Ok(())
}

/// Which functions a run entered, by symbol, with the phase of the last entry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Coverage {
    pub entered: BTreeMap<String, u8>,
}

impl Coverage {
    /// From the instrumented module and the bytes of its coverage memory.
    pub fn from_memory(module: &Module, memory: &[u8]) -> Coverage {
        let mut entered = BTreeMap::new();
        if let Some(names) = &module.names {
            for (index, symbol) in &names.funcs {
                if let Some(&phase) = memory.get(*index as usize) {
                    if phase != 0 {
                        entered.insert(symbol.clone(), phase);
                    }
                }
            }
        }
        Coverage { entered }
    }

    /// `<phase>\t<symbol>` per entered function.
    pub fn to_text(&self) -> String {
        self.entered.iter().map(|(s, p)| format!("{p}\t{s}\n")).collect()
    }

    pub fn parse(text: &str) -> Coverage {
        let entered = text
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .filter_map(|(p, s)| Some((s.to_string(), p.parse().ok()?)))
            .collect();
        Coverage { entered }
    }

    /// Merged with another run: the higher phase wins.
    pub fn merge(&mut self, other: &Coverage) {
        for (s, p) in &other.entered {
            let e = self.entered.entry(s.clone()).or_insert(0);
            *e = (*e).max(*p);
        }
    }
}

/// One unit: its functions, what of it ran, and whether it is stripped.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnitReport {
    pub name: String,
    pub functions: usize,
    pub bytes: usize,
    /// Functions entered while registering / after.
    pub entered_registration: usize,
    pub entered_use: usize,
    pub kept_by: Option<String>,
    pub stripped: bool,
}

/// What [`apply`] cuts.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Stripped functions (absolute indices).
    pub stripped: HashSet<u32>,
    /// Stripped registration functions: their calls drop the arguments.
    pub registration: HashSet<u32>,
    pub units: Vec<UnitReport>,
}

impl Plan {
    pub fn to_text(&self) -> String {
        let mut units = self.units.clone();
        units.sort_by(|a, b| b.stripped.cmp(&a.stripped).then(b.bytes.cmp(&a.bytes)));
        let (cut, cut_bytes) = units.iter().filter(|u| u.stripped).fold((0, 0), |(n, b), u| (n + 1, b + u.bytes));
        let mut out = format!("units: {} of {} stripped, {} bytes of code\n", cut, units.len(), cut_bytes);
        for u in &units {
            out.push_str(&format!(
                "  {} {:>9} {:>5} fns  entered {:>4} at registration, {:>4} after  {}{}\n",
                if u.stripped { "CUT " } else { "keep" },
                u.bytes,
                u.functions,
                u.entered_registration,
                u.entered_use,
                u.name,
                u.kept_by.as_ref().map(|k| format!("  (kept: {k})")).unwrap_or_default()
            ));
        }
        out
    }
}

/// A keep entry names a unit (`crate::module`), a crate (`crate`) or a
/// prefix (`crate::mod*`).
fn keeps(entry: &str, unit: &str) -> bool {
    match entry.strip_suffix('*') {
        Some(prefix) => unit.starts_with(prefix),
        None => unit == entry || unit.starts_with(&format!("{entry}::")),
    }
}

/// The units to strip: every unit none of whose functions was entered after
/// registration and that no keep entry names, unless it ran while
/// registering without a registration of its own (another module's
/// registration used it), or kept code that ran
/// called one of its functions that ran (other than its registration, whose
/// call is dropped): that call happens again, so the unit stays. Exported
/// and start functions, and functions without a name, are never stripped.
pub fn plan(module: &Module, coverage: &Coverage, keep: &[String]) -> Plan {
    let imported = module.num_imported_funcs();
    let Some(names) = &module.names else { return Plan::default() };
    let mut pinned: HashSet<u32> = module.exports.iter().filter(|e| e.kind == ExternKind::Func).map(|e| e.index).collect();
    pinned.extend(module.start);
    let mut by_unit: BTreeMap<String, Vec<(u32, &str)>> = BTreeMap::new();
    let mut unit_of_func: HashMap<u32, String> = HashMap::new();
    let mut entered: HashSet<u32> = HashSet::new();
    let mut registration: HashSet<u32> = HashSet::new();
    for (index, symbol) in &names.funcs {
        if coverage.entered.get(symbol).is_some_and(|p| *p > 0) {
            entered.insert(*index);
        }
        if *index < imported || pinned.contains(index) {
            continue;
        }
        if let Some(unit) = unit_of(symbol) {
            if is_registration(symbol) {
                registration.insert(*index);
            }
            unit_of_func.insert(*index, unit.clone());
            by_unit.entry(unit).or_default().push((*index, symbol));
        }
    }
    let phase = |s: &str| coverage.entered.get(s).copied().unwrap_or(0);
    let mut kept_by: HashMap<String, String> = HashMap::new();
    for (name, funcs) in &by_unit {
        if funcs.iter().any(|(_, s)| phase(s) >= PHASE_USE) {
            kept_by.insert(name.clone(), "ran".into());
        } else if funcs.iter().any(|(_, s)| phase(s) > 0) && !funcs.iter().any(|(i, _)| registration.contains(i)) {
            // It ran while registering, and not as its own registration
            // (it has none): another module's registration needs it.
            kept_by.insert(name.clone(), "ran while another module registered".into());
        } else if let Some(k) = keep.iter().find(|k| keeps(k, name)) {
            kept_by.insert(name.clone(), format!("keep {k}"));
        }
    }
    // Direct calls between functions that both ran.
    let mut calls: Vec<(u32, u32)> = Vec::new();
    for (i, func) in module.funcs.iter().enumerate() {
        let caller = imported + i as u32;
        if !entered.contains(&caller) {
            continue;
        }
        for instr in &func.body {
            if let Instr::Call(f) | Instr::ReturnCall(f) = instr {
                if entered.contains(f) && !registration.contains(f) {
                    calls.push((caller, *f));
                }
            }
        }
    }
    let name_of = |i: u32| names.funcs.iter().find(|(j, _)| *j == i).map(|(_, s)| readable(s)).unwrap_or_default();
    loop {
        let mut added = false;
        for (caller, callee) in &calls {
            let caller_kept = unit_of_func.get(caller).map_or(true, |u| kept_by.contains_key(u));
            let Some(unit) = unit_of_func.get(callee) else { continue };
            if caller_kept && !kept_by.contains_key(unit) {
                kept_by.insert(unit.clone(), format!("{} calls {}", name_of(*caller), name_of(*callee)));
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    let mut out = Plan::default();
    for (name, funcs) in by_unit {
        let entered_use = funcs.iter().filter(|(_, s)| phase(s) >= PHASE_USE).count();
        let entered_registration = funcs.iter().filter(|(_, s)| phase(s) == PHASE_REGISTRATION).count();
        let bytes = funcs.iter().map(|(i, _)| body_size(&module.funcs[(*i - imported) as usize])).sum();
        let by = kept_by.get(&name).cloned();
        let stripped = by.is_none();
        if stripped {
            for (index, _) in &funcs {
                out.stripped.insert(*index);
                if registration.contains(index) {
                    out.registration.insert(*index);
                }
            }
        }
        out.units.push(UnitReport {
            name,
            functions: funcs.len(),
            bytes,
            entered_registration,
            entered_use,
            kept_by: by.filter(|b| b != "ran"),
            stripped,
        });
    }
    out
}

/// The script modules a native collect run saw registered and used (the
/// manifest's `modules-registered.txt` and `modules.txt`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModuleUse {
    pub registered: BTreeSet<String>,
    pub used: BTreeSet<String>,
}

/// The registration strip from a collect run, no browser needed: the
/// `script_mod` of every module that registered and was not used (and no
/// keep entry names) is stripped. Its calls are dropped, so startup skips
/// it; nothing else is stubbed, and dead-code elimination then removes
/// what only that registration reached (the module's widget and draw
/// types, their vtables, its Splash text).
pub fn plan_modules(module: &Module, modules: &ModuleUse, keep: &[String]) -> Plan {
    let mut out = Plan::default();
    let Some(names) = &module.names else { return out };
    let imported = module.num_imported_funcs();
    let pinned: HashSet<u32> = module.exports.iter().filter(|e| e.kind == ExternKind::Func).map(|e| e.index).collect();
    for (index, symbol) in &names.funcs {
        if *index < imported || pinned.contains(index) {
            continue;
        }
        let name = readable(symbol);
        let Some(path) = name.strip_suffix("::script_mod") else { continue };
        if !modules.registered.contains(path) {
            continue;
        }
        let bytes = body_size(&module.funcs[(*index - imported) as usize]);
        let kept_by = if modules.used.contains(path) {
            None
        } else {
            keep.iter().find(|k| keeps(k, path)).map(|k| format!("keep {k}"))
        };
        let stripped = !modules.used.contains(path) && kept_by.is_none();
        if stripped {
            out.stripped.insert(*index);
            out.registration.insert(*index);
        }
        out.units.push(UnitReport {
            name: path.to_string(),
            functions: 1,
            bytes,
            entered_registration: 0,
            entered_use: usize::from(modules.used.contains(path)),
            kept_by,
            stripped,
        });
    }
    out
}

fn body_size(func: &Func) -> usize {
    let mut out = Vec::new();
    super::encode::func_body(&mut out, func);
    out.len()
}

/// Stubs every reference to a stripped function (see the module docs).
/// The stripped bodies stay until dead-code elimination drops them.
pub fn apply(module: &mut Module, plan: &Plan) {
    if plan.stripped.is_empty() {
        return;
    }
    let imported = module.num_imported_funcs();
    let func_types = module.func_type_indices();
    let mut stubs: HashMap<u32, u32> = HashMap::new();
    let mut stub_of = |module: &mut Module, func: u32| -> u32 {
        let ty = func_types[func as usize];
        *stubs.entry(ty).or_insert_with(|| {
            module.funcs.push(Func { ty, locals: Vec::new(), body: vec![Instr::Unreachable, Instr::End] });
            let index = imported + module.funcs.len() as u32 - 1;
            if let Some(names) = &mut module.names {
                names.funcs.push((index, format!("__stripped_stub_{ty}")));
            }
            index
        })
    };
    let gone = |f: u32| plan.stripped.contains(&f);
    // Calls in the bodies that stay.
    for i in 0..module.funcs.len() {
        if gone(imported + i as u32) {
            continue;
        }
        if !module.funcs[i].body.iter().any(|instr| matches!(instr, Instr::Call(f) | Instr::ReturnCall(f) | Instr::RefFunc(f) if gone(*f))) {
            continue;
        }
        let body = std::mem::take(&mut module.funcs[i].body);
        let mut out = Vec::with_capacity(body.len());
        for instr in body {
            match instr {
                Instr::Call(f) if gone(f) => {
                    let ty = &module.types[func_types[f as usize] as usize];
                    if plan.registration.contains(&f) && ty.results.iter().all(|r| !r.is_ref() && *r != ValType::V128) {
                        // Skipped: its arguments dropped, a zero for each
                        // result (a registration's value is discarded).
                        out.extend(std::iter::repeat(Instr::Drop).take(ty.params.len()));
                        out.extend(ty.results.iter().map(|r| match r {
                            ValType::I64 => Instr::I64Const(0),
                            ValType::F32 => Instr::F32Const(0),
                            ValType::F64 => Instr::F64Const(0),
                            _ => Instr::I32Const(0),
                        }));
                    } else {
                        out.push(Instr::Unreachable);
                    }
                }
                Instr::ReturnCall(f) if gone(f) => out.push(Instr::Unreachable),
                Instr::RefFunc(f) if gone(f) => {
                    let stub = stub_of(module, f);
                    out.push(Instr::RefFunc(stub));
                }
                other => out.push(other),
            }
        }
        module.funcs[i].body = out;
    }
    // Table entries.
    for e in 0..module.elems.len() {
        let items = std::mem::replace(&mut module.elems[e].items, ElemItems::Funcs(Vec::new()));
        module.elems[e].items = match items {
            ElemItems::Funcs(funcs) => ElemItems::Funcs(funcs.into_iter().map(|f| if gone(f) { stub_of(module, f) } else { f }).collect()),
            ElemItems::Exprs(ty, exprs) => ElemItems::Exprs(
                ty,
                exprs
                    .into_iter()
                    .map(|expr| expr.into_iter().map(|instr| match instr { Instr::RefFunc(f) if gone(f) => Instr::RefFunc(stub_of(module, f)), other => other }).collect())
                    .collect(),
            ),
        };
    }
    for g in 0..module.globals.len() {
        let init = std::mem::take(&mut module.globals[g].init);
        module.globals[g].init = init.into_iter().map(|instr| match instr { Instr::RefFunc(f) if gone(f) => Instr::RefFunc(stub_of(module, f)), other => other }).collect();
    }
}

/// Table entries nothing can name: a function pointer is a table index,
/// written in live code (`i32.const`) or held in the data (a vtable, a
/// stored callback) that is left once what only dead code pointed at is
/// cleared. An entry whose index neither holds points at a
/// trapping stub of its type, and dead-code elimination drops the function
/// when nothing else uses it; repeated until nothing changes (a dropped
/// function's constants and the data only it reached go with it). Left
/// alone when the table is imported or exported (the host can name any
/// entry). `boundaries`: data anchors of the code before the strip, which
/// keep the regions of what it pointed at apart. Returns the entries
/// stubbed.
pub fn prune_table(module: &mut Module, boundaries: &BTreeSet<u32>) -> usize {
    if module.imports.iter().any(|i| matches!(i.desc, ImportDesc::Table(_))) || module.exports.iter().any(|e| e.kind == ExternKind::Table) {
        return 0;
    }
    let mut total = 0;
    loop {
        // The trapping stubs there are (any function that only traps).
        let imported = module.num_imported_funcs();
        let mut stubs: HashMap<u32, u32> = HashMap::new();
        for (i, func) in module.funcs.iter().enumerate() {
            if func.locals.is_empty() && func.body == [Instr::Unreachable, Instr::End] {
                stubs.entry(func.ty).or_insert(imported + i as u32);
            }
        }
        let mut named: HashSet<u32> = HashSet::new();
        // A constant names an entry unless it is plainly a number: an
        // operand of arithmetic or a comparison, or an address loaded from.
        let mut consts = |instrs: &[Instr]| {
            for (i, instr) in instrs.iter().enumerate() {
                if let Instr::I32Const(v) = instr {
                    if !matches!(instrs.get(i + 1), Some(Instr::Num(_) | Instr::Load(..) | Instr::SimdMem(..) | Instr::BrTable(..) | Instr::If(_) | Instr::BrIf(_))) {
                        named.insert(*v as u32);
                    }
                }
            }
        };
        for func in &module.funcs {
            consts(&func.body);
        }
        for global in &module.globals {
            consts(&global.init);
        }
        // Every aligned word of the data live code reaches (a region per
        // anchor, through the pointers the live regions hold).
        // Region bounds from the code before the strip as well: a dead
        // vtable keeps its own region instead of joining its neighbour's.
        let data = DataLive::new(module, boundaries);
        for start in &data.live {
            let (start, end) = data.region(*start);
            for (s, e, i) in &data.spans {
                let (lo, hi) = (start.max(*s), end.min(*e));
                let bytes = &module.datas[*i].bytes;
                let mut a = (lo + 3) & !3;
                // A word that starts in the region counts, wherever the
                // next anchor falls.
                while a < hi && a + 4 <= *e {
                    let at = (a - s) as usize;
                    named.insert(u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]));
                    a += 4;
                }
            }
        }
        // Data no anchor reaches before its first anchor stays named too.
        for (s, e, i) in &data.spans {
            let first = data.anchors.range(*s..*e).next().copied().unwrap_or(*e);
            let bytes = &module.datas[*i].bytes;
            let mut a = (s + 3) & !3;
            while a + 4 <= first {
                let at = (a - s) as usize;
                named.insert(u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]));
                a += 4;
            }
        }
        let func_types = module.func_type_indices();
        let stub_set: HashSet<u32> = stubs.values().copied().collect();
        let mut changed = 0;
        for e in 0..module.elems.len() {
            let ElemMode::Active { table: 0, offset } = &module.elems[e].mode else { continue };
            let [Instr::I32Const(base), Instr::End] = offset.as_slice() else { continue };
            let base = *base as u32;
            let ElemItems::Funcs(funcs) = &module.elems[e].items else { continue };
            let mut funcs = funcs.clone();
            for (k, f) in funcs.iter_mut().enumerate() {
                if stub_set.contains(f) || named.contains(&(base + k as u32)) {
                    continue;
                }
                let ty = func_types[*f as usize];
                let stub = *stubs.entry(ty).or_insert_with(|| {
                    module.funcs.push(Func { ty, locals: Vec::new(), body: vec![Instr::Unreachable, Instr::End] });
                    let index = imported + module.funcs.len() as u32 - 1;
                    if let Some(names) = &mut module.names {
                        names.funcs.push((index, format!("__unnamed_entry_stub_{ty}")));
                    }
                    index
                });
                *f = stub;
                changed += 1;
            }
            module.elems[e].items = ElemItems::Funcs(funcs);
        }
        if changed == 0 {
            break;
        }
        total += changed;
        let before = code_anchors(module, &HashSet::new());
        super::dce::run(module);
        zero_dead_data(module, &before);
    }
    total
}

/// The whole strip: stub, drop the dead code, clear its data. Returns the
/// bytes of data cleared.
pub fn strip(module: &mut Module, plan: &Plan) -> usize {
    let before = code_anchors(module, &HashSet::new());
    apply(module, plan);
    super::dce::run(module);
    let cleared = zero_dead_data(module, &before);
    prune_table(module, &before);
    cleared
}
