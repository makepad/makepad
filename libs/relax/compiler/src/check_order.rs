//! `makepad check`'s diagnostics in rustc's emission order (rustc_interface passes.rs): name
//! resolution, the early checks and lints, misc_checking_1 (attributes, the entry point, library
//! stability), well-formedness, each body owner's type check, each body owner's unsafety, pattern,
//! borrow and liveness checks (MIR_borrow_checking), then misc_checking_3 (private interfaces, dead
//! code, the late lints, privacy, lint expectations), the errors after the analysis last. Within a
//! phase: body owners in definition order, everything else by its place in the crate (a module
//! file's items stand where its `mod` item does). The check prints the one sorted list itself, on
//! one thread: the same check prints the same text, whatever order its threads finished in.
use crate::borrowck::event::{GandalfEvent, Origin};
use crate::program::{DefId, DefKind, Program, NO_DEF};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Phase { Resolve, Early, Attrs, Wf, Typeck, Mir, Late, Post }

impl Phase {
    /// (check_reuse.rs: a stored entry's phase)
    pub(crate) fn code(self) -> u8 { self as u8 }
    pub(crate) fn of_code(c: u8) -> Option<Self> {
        [Phase::Resolve, Phase::Early, Phase::Attrs, Phase::Wf, Phase::Typeck, Phase::Mir, Phase::Late, Phase::Post].get(c as usize).copied()
    }
}

/// The steps of a body owner's MIR checks: check_unsafety, mir_borrowck (mir_built's thir
/// check_match, mir_promoted's tail_expr_drop_order lint, then borrowck's unused_mut and its
/// errors), check_liveness.
pub(crate) const MIR_UNSAFETY: u8 = 0;
pub(crate) const MIR_PATTERNS: u8 = 1;
pub(crate) const MIR_PROMOTED: u8 = 2;
pub(crate) const MIR_UNUSED_MUT: u8 = 3;
pub(crate) const MIR_BORROWCK: u8 = 4;
pub(crate) const MIR_LIVENESS: u8 = 5;
/// The steps of misc_checking_3: check_private_in_public, check_mod_deathness, the late lint
/// pass, clashing_extern_declarations, check_mod_privacy, check_expectations.
pub(crate) const LATE_PRIVATE_IN_PUBLIC: u8 = 0;
pub(crate) const LATE_DEAD: u8 = 1;
pub(crate) const LATE_LINTS: u8 = 2;
pub(crate) const LATE_CLASHING: u8 = 3;
pub(crate) const LATE_PRIVACY: u8 = 4;
pub(crate) const LATE_EXPECTATIONS: u8 = 5;

/// Where a diagnostic is reported in rustc: its phase and step, and whose group it is in (the
/// body owner of the type and MIR checks, the module of dead code and the late lints).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stage { pub phase: Phase, pub step: u8, pub group: DefId }

impl Stage {
    pub(crate) const fn new(phase: Phase, step: u8) -> Self { Self { phase, step, group: NO_DEF } }
    pub(crate) const fn of(phase: Phase, step: u8, group: DefId) -> Self { Self { phase, step, group } }
    /// rustc reports these before the analysis can stop (its required analyses): an error of
    /// one of them means no late lint is reported.
    pub(crate) fn required(&self) -> bool { self.phase < Phase::Late }
}

/// The resolver's errors by the step that reports them (rustc_resolve resolve_crate:
/// build_reduced_graph during expansion, finalize_imports, finalize_macro_resolutions (step 2,
/// `stage_of_uncoded`), late_resolve_crate, report_errors), and the codes misc_checking_3
/// reports; wherever the check finds them.
pub(crate) fn stage_of_code(code: &str) -> Option<Stage> {
    let (phase, step) = match code {
        "E0428" => (Phase::Resolve, 0),
        "E0252" | "E0254" | "E0255" | "E0259" | "E0260" | "E0430" | "E0431" | "E0432" => (Phase::Resolve, 1),
        "E0106" | "E0261" | "E0401" | "E0403" | "E0408" | "E0409" | "E0415" | "E0416" | "E0426" | "E0433" | "E0434" | "E0435" | "E0437" | "E0438" | "E0637" => (Phase::Resolve, 3),
        // (use_injections: the smart-resolve errors are reported once resolution is done)
        "E0404" | "E0405" | "E0411" | "E0412" | "E0422" | "E0423" | "E0424" | "E0425" | "E0531" | "E0532" | "E0573" | "E0574" | "E0575" | "E0576" => (Phase::Resolve, 4),
        "E0659" => (Phase::Resolve, 5),
        "E0603" => (Phase::Resolve, 6),
        "E0446" => (Phase::Late, LATE_PRIVATE_IN_PUBLIC),
        "E0451" => (Phase::Late, LATE_PRIVACY),
        _ => return None,
    };
    Some(Stage::new(phase, step))
}

/// The earliest place rustc reports these codes: the type lowering's errors come with the
/// signatures (check_type_wf) or with the body that names the type, never before.
fn floor_of_code(code: &str) -> Option<Stage> {
    match code {
        "E0107" | "E0782" => Some(Stage::new(Phase::Wf, 0)),
        _ => None,
    }
}

/// Where rustc reports an error it words without a code (`path:l:c: error: ..`): a macro path
/// that names no macro, in finalize_macro_resolutions.
fn stage_of_uncoded(e: &str) -> Option<Stage> {
    let first = e.lines().next().unwrap_or("");
    let at = first.find(": error: ")?;
    first[at + 9..].starts_with("cannot find macro ").then_some(Stage::new(Phase::Resolve, 2))
}

/// Where rustc reports error `e` that the check found at `stage`.
pub(crate) fn stage_for(stage: Stage, e: &str) -> Stage {
    let code = code_of(e);
    let s = stage_of_code(code).or_else(|| stage_of_uncoded(e)).map_or(stage, |c| Stage { group: stage.group, ..c });
    match floor_of_code(code) {
        Some(f) if (s.phase, s.step) < (f.phase, f.step) => Stage { group: s.group, ..f },
        _ => s,
    }
}

/// Where rustc reports lint `name` (in `def`'s scope).
pub(crate) fn stage_of_lint(name: &str, def: DefId) -> Stage {
    if relax_types::lint::is_late(name) {
        let step = match name {
            "private_interfaces" | "private_bounds" | "unnameable_types" => LATE_PRIVATE_IN_PUBLIC,
            "dead_code" | "dead_code_pub_in_binary" => LATE_DEAD,
            "clashing_extern_declarations" => LATE_CLASHING,
            "unfulfilled_lint_expectations" => LATE_EXPECTATIONS,
            _ => LATE_LINTS,
        };
        return Stage::of(Phase::Late, step, def);
    }
    let (phase, step) = match name {
        "unused_variables" | "unused_assignments" => (Phase::Mir, MIR_LIVENESS),
        "unused_mut" => (Phase::Mir, MIR_UNUSED_MUT),
        "unused_unsafe" | "unsafe_op_in_unsafe_fn" | "deprecated_safe_2024" => (Phase::Mir, MIR_UNSAFETY),
        "unreachable_patterns" | "irrefutable_let_patterns" | "bindings_with_variant_name" | "overlapping_range_endpoints"
        | "non_contiguous_range_endpoints" | "unconditional_recursion" | "const_item_mutation" => (Phase::Mir, MIR_PATTERNS),
        "tail_expr_drop_order" => (Phase::Mir, MIR_PROMOTED),
        // (the lints of the optimized MIR: the metadata encoding after the analysis asks for it)
        "arithmetic_overflow" | "unconditional_panic" => (Phase::Post, 0),
        "unreachable_code" | "deprecated" | "deprecated_in_future" | "tyvar_behind_raw_pointer" | "late_bound_lifetime_arguments"
        | "ambiguous_associated_items" | "rust_2021_prelude_collisions" | "rust_2024_prelude_collisions" | "trivial_casts"
        | "trivial_numeric_casts" | "long_running_const_eval" => (Phase::Typeck, 0),
        "coherence_leak_check" | "uncovered_param_in_projection" | "refining_impl_trait_internal" | "refining_impl_trait_reachable"
        | "redundant_lifetimes" => (Phase::Wf, 0),
        "unused_attributes" | "invalid_doc_attributes" | "unknown_diagnostic_attributes" | "malformed_diagnostic_format_literals"
        | "malformed_diagnostic_attributes" | "misplaced_diagnostic_attributes" => (Phase::Attrs, 0),
        // the resolver's lints and the early lint pass (with the lints buffered for it)
        _ => (Phase::Early, 2),
    };
    Stage::of(phase, step, def)
}

/// The code of a check line `path:l:c: E0425: msg` (`""`: `path:l:c: error: msg`).
pub(crate) fn code_of(line: &str) -> &str {
    let first = line.lines().next().unwrap_or("");
    let Some(i) = first.find(": ") else { return "" };
    let rest = &first[i + 2..];
    let b = rest.as_bytes();
    if b.len() >= 6 && b[0] == b'E' && b[1..5].iter().all(u8::is_ascii_digit) && b[5] == b':' { &rest[..5] } else { "" }
}

/// A place in a crate: the crate, then (line, col) of each `mod` item from the crate root down
/// to the file, then (line, col) in the file; the file breaks ties (`include!`d files).
type Place = (u32, Vec<(u32, u32)>, u32);

struct Places<'p> {
    prog: &'p Program,
    by_path: HashMap<&'p str, u32>,
    /// file -> where its `mod` item stands (file, position)
    decl: HashMap<u32, (u32, u32)>,
    chains: HashMap<u32, Vec<(u32, u32)>>,
}

impl<'p> Places<'p> {
    fn new(prog: &'p Program) -> Self {
        let mut by_path = HashMap::new();
        for (i, p) in prog.file_paths.iter().enumerate() { by_path.entry(p.as_str()).or_insert(i as u32); }
        let mut decl = HashMap::new();
        for m in prog.mods.iter() {
            if m.parent_mod.is_none() || m.lexical_parent.is_some() || m.def == NO_DEF { continue; }
            let d = prog.def(m.def);
            if d.file == m.file { continue; }
            let lo = prog.files[d.file as usize].ast.item(d.item).lo;
            decl.entry(m.file).or_insert((d.file, lo));
        }
        Self { prog, by_path, decl, chains: HashMap::new() }
    }

    fn chain(&mut self, file: u32) -> Vec<(u32, u32)> {
        if let Some(c) = self.chains.get(&file) { return c.clone(); }
        let mut c = Vec::new();
        let mut at = file;
        // (from the file up to the crate root; a cycle cannot happen, a bound keeps it so)
        for _ in 0..64 {
            let Some(&(f, lo)) = self.decl.get(&at) else { break };
            let (l, col) = self.prog.files[f as usize].line_col(lo);
            c.push((l as u32, col as u32));
            at = f;
        }
        c.reverse();
        self.chains.insert(file, c.clone());
        c
    }

    fn at(&mut self, file: u32, line: u32, col: u32) -> Place {
        let krate = self.prog.file_crate.get(file as usize).copied().unwrap_or(u32::MAX);
        let mut c = self.chain(file);
        c.push((line, col));
        (krate, c, file)
    }

    fn at_path(&mut self, path: &str, line: u32, col: u32) -> Place {
        match self.by_path.get(path).copied() {
            Some(f) => self.at(f, line, col),
            None => (u32::MAX, vec![(line, col)], u32::MAX),
        }
    }

    fn of_def(&mut self, d: DefId) -> Place {
        match self.def_at(d) {
            Some((file, l, c)) => self.at(file, l, c),
            None => (0, Vec::new(), 0),
        }
    }

    /// (file, line, col) of definition `d` (None: no definition)
    fn def_at(&self, d: DefId) -> Option<(u32, u32, u32)> {
        if d == NO_DEF || d.0 as usize >= self.prog.defs.len() { return None; }
        let def = self.prog.def(d);
        let lo = if def.kind == DefKind::Mod && self.prog.mods.get(def.sub as usize).is_some_and(|m| m.parent_mod.is_none()) { 0 }
            else { self.prog.files[def.file as usize].ast.item(def.item).lo };
        let (l, c) = self.prog.files[def.file as usize].line_col(lo);
        Some((def.file, l as u32, c as u32))
    }

    /// The definition whose place groups an entry of `s`: its body owner's (the type and MIR
    /// checks), its module's (dead code, the late lints); `At`: the entry's own place.
    fn group_of(&self, s: Stage) -> GroupOf {
        match (s.phase, s.step) {
            (Phase::Typeck | Phase::Mir, _) if s.group != NO_DEF => GroupOf::Def(self.typeck_root(s.group)),
            (Phase::Late, LATE_DEAD | LATE_LINTS) if s.group != NO_DEF => GroupOf::Def(self.module_of(s.group)),
            // (an error of a body owner the check did not name: its own place stands for the owner's)
            (Phase::Typeck | Phase::Mir, _) => GroupOf::At,
            _ => GroupOf::Zero,
        }
    }

    /// The body owner that type-checks `d` (a closure and a constant inside a body are checked
    /// with the body: rustc's typeck root).
    fn typeck_root(&self, d: DefId) -> DefId {
        let mut at = d;
        for _ in 0..256 {
            if at == NO_DEF { return d; }
            let def = self.prog.def(at);
            if !matches!(def.kind, DefKind::Closure | DefKind::AnonConst) || def.parent == NO_DEF { return at; }
            at = def.parent;
        }
        d
    }

    /// The module whose items `d` is among (rustc's module walks: check_mod_deathness, the late
    /// lint pass).
    fn module_of(&self, d: DefId) -> DefId {
        let mut at = d;
        for _ in 0..256 {
            if at == NO_DEF { return NO_DEF; }
            let def = self.prog.def(at);
            if def.kind == DefKind::Mod { return at; }
            at = def.parent;
        }
        NO_DEF
    }
}

pub(crate) enum Out {
    /// an error line of the check (`path:l:c: E0425: msg`, a lint error)
    Error(String),
    Warning(String),
    Borrow(Box<GandalfEvent>),
}

enum GroupOf { Zero, At, Def(DefId) }

/// Where an entry's group stands, as the sort placed it (check_reuse.rs stores it with the
/// entry): nowhere, at the entry's own place, or at a definition's (path, line, col).
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum GroupAt { Zero, At, Path(String, u32, u32) }

/// `seq`: the order its analysis reported it in, where that is rustc's (the liveness lints of a
/// body owner: lint_liveness.rs); 0 elsewhere, the place decides. `group_at`: a replayed entry's
/// group (check_reuse.rs); None: the stage's.
pub(crate) struct Entry { pub stage: Stage, pub seq: u32, pub out: Out, pub group_at: Option<GroupAt> }

impl Entry {
    pub(crate) fn new(stage: Stage, seq: u32, out: Out) -> Self { Self { stage, seq, out, group_at: None } }
}

/// The group of each of `stages` as `sort` places it, by path (check_reuse.rs stores it).
pub(crate) fn groups_at(prog: &Program, stages: &[Stage]) -> Vec<GroupAt> {
    let places = Places::new(prog);
    stages.iter().map(|&s| match places.group_of(s) {
        GroupOf::Zero => GroupAt::Zero,
        GroupOf::At => GroupAt::At,
        GroupOf::Def(d) => match places.def_at(d) {
            Some((file, l, c)) => GroupAt::Path(prog.file_paths.get(file as usize).cloned().unwrap_or_default(), l, c),
            None => GroupAt::Zero,
        },
    }).collect()
}

/// Every source path a check line names (`path:line:col`, its `-->` lines included), as written.
pub(crate) fn paths_named(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim_start();
        let l = l.strip_prefix("--> ").or_else(|| l.strip_prefix("::: ")).unwrap_or(l);
        if let Some((p, _, _)) = location(l) { out.push(p); }
    }
    out
}

/// The path of a check line's place (`location`).
pub(crate) fn path_of(text: &str) -> Option<&str> {
    location(text).map(|(p, _, _)| p)
}

/// (path, line, col) of a check line or a rendered borrow error.
fn location(text: &str) -> Option<(&str, u32, u32)> {
    let first = text.lines().find_map(|line| line.trim().strip_prefix("--> ")).unwrap_or_else(|| text.lines().next().unwrap_or(""));
    for (i, _) in first.match_indices(':') {
        let mut it = first[i + 1..].splitn(3, ':');
        if let (Some(l), Some(c)) = (it.next(), it.next()) {
            if let (Ok(l), Ok(c)) = (l.trim().parse::<u32>(), c.trim().parse::<u32>()) {
                return Some((&first[..i], l, c));
            }
        }
    }
    None
}

/// `owner`: the body owner whose MIR checks these are -- rustc runs every step of one owner's
/// before the next owner's (MIR_borrow_checking); a phase's other steps are passes over the crate.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Key { krate: u32, phase: Phase, owner: Place, step: u8, group: Place, seq: u32, at: Place, text: String }

/// The entries in rustc's order, one of each error and lint text (a body checked twice reports
/// once); the borrow errors as the borrow checker reported them.
pub(crate) fn sort(prog: &Program, entries: Vec<Entry>) -> Vec<Entry> {
    let mut places = Places::new(prog);
    let mut keyed: Vec<(Key, Entry)> = entries.into_iter().map(|e| {
        let text = match &e.out { Out::Error(s) | Out::Warning(s) => s.clone(), Out::Borrow(ev) => ev.error.render() };
        let at = match &e.out {
            Out::Borrow(ev) => places.at_path(&ev.error.file, ev.error.line, ev.error.col),
            _ => match location(&text) {
                Some((p, l, c)) => places.at_path(p, l, c),
                // (no place: first, as the check always printed them)
                None => (0, Vec::new(), 0),
            },
        };
        let s = e.stage;
        let group = match &e.group_at {
            Some(GroupAt::Zero) => (0, Vec::new(), 0),
            Some(GroupAt::At) => at.clone(),
            Some(GroupAt::Path(p, l, c)) => places.at_path(p, *l, *c),
            None => match places.group_of(s) {
                GroupOf::Def(d) => places.of_def(d),
                GroupOf::At => at.clone(),
                GroupOf::Zero => (0, Vec::new(), 0),
            },
        };
        let krate = if s.phase == Phase::Typeck || s.phase == Phase::Mir { group.0 } else { at.0 };
        let owner = if s.phase == Phase::Mir { group.clone() } else { (0, Vec::new(), 0) };
        (Key { krate, phase: s.phase, owner, step: s.step, group, seq: e.seq, at, text }, e)
    }).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut seen = std::collections::HashSet::new();
    keyed.into_iter().filter(|(k, e)| matches!(e.out, Out::Borrow(_)) || seen.insert(k.text.clone())).map(|(_, e)| e).collect()
}

/// Prints the sorted entries: errors and borrow errors when `emit` (JSON lines too under
/// `json`), warnings when `warnings`; everything on stderr, as rustc prints.
pub(crate) fn print(entries: &[Entry], emit: bool, warnings: bool, json: bool) {
    if !emit { return; }
    for e in entries {
        match &e.out {
            Out::Error(s) => {
                if json { crate::diag::emit(&crate::diag::from_legacy(s, "typeck")); } else { stderr_line!("{}", s); }
            }
            Out::Warning(s) => if warnings && !json { stderr_line!("{}", s); },
            Out::Borrow(ev) => {
                if ev.origin == Origin::Gate { continue; }
                if ev.origin != Origin::Run {
                    if ev.origin == Origin::Background {
                        stderr_line!("relax: [t={:.0} ms] borrow check failed: {} at {}:{}: {}\n{}", crate::clock::ms_since_start(), ev.error.fn_path, ev.error.file, ev.error.line, ev.error.message, ev.error.render());
                    } else { stderr_line!("{}", ev.error.render()); }
                }
                if json { crate::diag::emit(&ev.error.to_diagnostic()); }
            }
        }
    }
}
