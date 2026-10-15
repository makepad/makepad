// ported from N:rustc_middle/src/mir/interpret/queries.rs @8d1a764
// ported from N:rustc_hir_typeck/src/lib.rs @8d1a764
//! Type properties on the shared query engine. Tcx owns tables; providers borrow the
//! collected program. rustc_middle/src/ty/context.rs:258 and query/plumbing.rs:25.

use crate::types::AliasTyKind;
use crate::types::AliasTy;
use crate::fxhash::HashSet;
use crate::program::{DefId, Program};
use crate::query::{Fingerprint, QueryEngine, QueryError, QueryId, QueryTable};
use crate::tcx::{Tcx, TypingEnv};
use crate::types::{TyId, TyKind};
use std::hash::{Hash, Hasher};
use crate::const_eval::{ConstEvalKey, ErrorHandled, EvalToValTreeResult};
use crate::consts::GenericArg;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TypeKey { env: TypingEnv, ty: TyId }

struct KeyHasher(crate::sip::Sip128);
impl Hasher for KeyHasher {
    fn write(&mut self, bytes: &[u8]) { self.0.write(bytes); }
    fn finish(&self) -> u64 { self.0.clone().finish128() as u64 }
}

// Interned identities are session-local, as are these tables. Q7 must replace this
// encoding with DefPath/type encodings before persisting any of these nodes.
fn type_fp(key: &TypeKey) -> Fingerprint {
    let mut h = KeyHasher(crate::sip::Sip128::new());
    key.hash(&mut h);
    Fingerprint(h.0.finish128())
}

fn property_bool_fp(value: &bool) -> Fingerprint {
    let mut h = KeyHasher(crate::sip::Sip128::new());
    value.hash(&mut h);
    Fingerprint(h.0.finish128())
}
fn property_ty_fp(value: &Result<TyId, crate::next_solver::NoSolution>) -> Fingerprint {
    let mut h = KeyHasher(crate::sip::Sip128::new());
    value.map_or(u32::MAX, |t| t.0).hash(&mut h);
    Fingerprint(h.0.finish128())
}

fn const_key_fp(key: &ConstEvalKey) -> Fingerprint {
    let mut h = KeyHasher(crate::sip::Sip128::new());
    key.hash(&mut h);
    Fingerprint(h.0.finish128())
}

fn const_value_fp(value: &EvalToValTreeResult) -> Fingerprint {
    let mut h = KeyHasher(crate::sip::Sip128::new());
    match value {
        Ok(value) => { 0u8.hash(&mut h); value.hash(&mut h); }
        Err(ErrorHandled::TooGeneric) => 1u8.hash(&mut h),
        // Distinguish reported failure from unresolved generic inputs.
        Err(ErrorHandled::Reported(error)) => { 2u8.hash(&mut h); format!("{error:?}").hash(&mut h); }
    }
    Fingerprint(h.0.finish128())
}

fn coherence_key_fp(key: &(u32, DefId)) -> Fingerprint {
    Fingerprint::from_bytes(&((key.0 as u64) << 32 | key.1.0 as u64).to_le_bytes())
}
fn type_param_key_fp(key: &(DefId, u32, crate::program::Sym)) -> Fingerprint {
    let mut h = KeyHasher(crate::sip::Sip128::new());
    key.hash(&mut h);
    Fingerprint(h.0.finish128())
}
pub struct QueryCx {
    file_count: u32,
    crate_count: u32,
    type_param_clauses: QueryTable<(DefId, u32, crate::program::Sym), Vec<crate::next_solver::Predicate>>,
    specialization_graph: QueryTable<(u32, DefId), bool>,
    inherent_overlap: QueryTable<DefId, bool>,
    pub(crate) engine: QueryEngine,
    pub(crate) declarations: crate::next_solver::sizedness::DeclarationQueries,
    pub(crate) solver: crate::next_solver::search_graph::SolverCache,
    drop: QueryTable<TypeKey, bool>,
    freeze: QueryTable<TypeKey, bool>,
    normalize: QueryTable<TypeKey, Result<TyId, crate::next_solver::NoSolution>>,
    const_eval: QueryTable<ConstEvalKey, EvalToValTreeResult>,
    param_env: QueryTable<DefId, crate::tcx::ParamEnvId>,
    pub(crate) compare_impl_item: QueryTable<DefId, Result<(), crate::next_solver::NoSolution>>,
    typeck: QueryTable<DefId, (u64, std::sync::Arc<crate::typeck::Body>)>,
    opaque_generics: QueryTable<DefId, crate::next_solver::rpitit::OpaqueGenerics>,
    variances: QueryTable<DefId, Vec<crate::types::variance::V>>,
    lifetime_names: QueryTable<DefId, (u64, std::sync::Arc<crate::tcx::lifetime_resolve::LifetimeResolution>)>,
    resolved_bound_vars: QueryTable<DefId, (u64, std::sync::Arc<crate::tcx::resolve_bound_vars::ResolveBoundVars>)>,
    pub(crate) opaque_hidden_type: QueryTable<DefId, Result<TyId, crate::next_solver::NoSolution>>,
    /// rustc_middle/src/query/mod.rs codegen_select_candidate (PseudoCanonicalInput<TraitRef>)
    /// (with the post-boot loader's misses of its computation: they travel with the cached
    /// answer, as Body::deferred_misses do, so every reader fails for the same skipped modules)
    codegen_select: QueryTable<(TypingEnv, crate::next_solver::TraitRef), (Result<crate::next_solver::select::ImplSource, crate::next_solver::codegen::CodegenObligationError>, Misses)>,
    /// rustc_middle/src/query/mod.rs resolve_instance_raw (PseudoCanonicalInput<(DefId, GenericArgsRef)>)
    resolve_instance: QueryTable<(TypingEnv, DefId, crate::types::TyList), (Result<crate::instance::Instance, crate::next_solver::codegen::CodegenObligationError>, Misses)>,
    /// rustc_middle/src/query/mod.rs dyn_compatibility_violations (per trait)
    dyn_compatibility: QueryTable<DefId, Vec<DefId>>,
    /// rustc_middle/src/query/mod.rs own_existential_vtable_entries (per trait)
    own_vtable_entries: QueryTable<DefId, Vec<DefId>>,
    /// rustc_middle/src/query/mod.rs first_method_vtable_slot (per trait ref)
    first_vtable_slot: QueryTable<crate::next_solver::TraitRef, (usize, Misses)>,
}

type Misses = std::sync::Arc<Vec<(u32, String)>>;

/// `f` with this thread's miss notes started clean; returns its value and its misses, and
/// leaves the enclosing notes plus these.
pub(crate) fn with_misses<R>(f: impl FnOnce() -> R) -> (R, Misses) {
    let enclosing = crate::program::missing_save();
    let r = f();
    let misses = crate::program::take_missing();
    crate::program::missing_restore(enclosing);
    crate::program::note_misses(&misses);
    // Most solver goals have no misses; share that immutable empty payload.
    static EMPTY: std::sync::OnceLock<Misses> = std::sync::OnceLock::new();
    let misses = if misses.is_empty() { EMPTY.get_or_init(|| crate::zone_alloc::kept_with(|| std::sync::Arc::new(Vec::new()))).clone() }
        else { std::sync::Arc::new(misses) };
    (r, misses)
}

fn hash_fp<T: Hash>(value: &T) -> Fingerprint {
    let mut h = KeyHasher(crate::sip::Sip128::new());
    value.hash(&mut h);
    Fingerprint(h.0.finish128())
}

// A recomputed whole-owner result changes its fingerprint even when its return
// type is unchanged: node coordinates, call decisions and captures may differ.
fn typeck_fp(value: &(u64, std::sync::Arc<crate::typeck::Body>)) -> Fingerprint {
    Fingerprint::from_bytes(&value.0.to_le_bytes())
}
fn opaque_def_fp(def: &DefId) -> Fingerprint { Fingerprint::from_bytes(&def.0.to_le_bytes()) }
fn opaque_ty_fp(ty: &Result<TyId, crate::next_solver::NoSolution>) -> Fingerprint { Fingerprint::from_bytes(&ty.map_or(u32::MAX, |t| t.0).to_le_bytes()) }

// Lead decision 52 (R1): declaration inputs are per crate. Input (0, k) is the
// declarations crate k can see -- its own and those of its transitive dependencies --
// so a declaration edit changes the inputs of the crates that can see it
// (reload::crates_seeing_edit), as rustc recompiles only those crates and decodes
// every other crate's answers from metadata. Input (0, ALL_CRATES) is every crate's
// declarations: const_eval, representability of an instantiated ADT and any type-keyed
// query whose key is not a plain post-analysis type read it (read_type_declarations).
pub(crate) const DECLARATIONS: QueryId = QueryId { kind: 0, key: ALL_CRATES };
const ALL_CRATES: u32 = u32::MAX;
const FILE_BODIES: u16 = 21;

/// Memory report (MAKEPAD_RUN_MEM_REPORT): per typeck owner (first read ms, last read ms, reads,
/// reads more than 2 s after the first) -- who still reads a Body after its owner is lowered.
static TYPECK_READS: relax_session::sync::Mutex<Option<crate::fxhash::HashMap<u32, (u64, u64, u32, u32)>>> = relax_session::sync::Mutex::new(None);
static TYPECK_READS_T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn note_typeck_read(def: DefId) {
    let ms = TYPECK_READS_T0.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64;
    let mut g = TYPECK_READS.lock().unwrap_or_else(|e| e.into_inner());
    let e = g.get_or_insert_with(Default::default).entry(def.0).or_insert((ms, ms, 0, 0));
    e.1 = ms;
    e.2 += 1;
    if ms > e.0 + 2000 { e.3 += 1; }
}

impl QueryCx {
    /// memory report (MAKEPAD_RUN_MEM_REPORT): the solver cache and the bound-vars tables
    pub fn mem_lines(&self, tys: &crate::types::Types) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut p = |name: &str, (n, b): (usize, usize)| out.push((format!("qt {} ({})", name, n), b as u64));
        p("resolved_bound_vars", self.resolved_bound_vars.mem_stats(&|_, v| { let (a, b) = v.1.heap_bytes_split(); if std::sync::Arc::strong_count(&v.1) > 1 { 8 } else { 16 + std::mem::size_of::<crate::tcx::resolve_bound_vars::ResolveBoundVars>() + a + b } }));
        p("lifetime_names Arc<LifetimeResolution>", self.lifetime_names.mem_stats(&|_, v| if std::sync::Arc::strong_count(&v.1) > 2 { 8 } else { 16 + v.1.heap_bytes() }));
        out.extend(self.solver.mem_lines());
        {
            use std::sync::atomic::Ordering::Relaxed;
            out.push((format!("qt settle retirement: {} values retired, {} derived again since", crate::query::RETIRED_VALUES.load(Relaxed), crate::query::RECOMPUTED_VALUES.load(Relaxed)), 0));
        }
        {
            // keys hold an interned list id: 4 B, no heap. A Vec<TyId> key (the previous layout)
            // was 20 B more inline plus an exact-size heap block (glibc chunk: len * 4 + 8 rounded
            // up to 16, at least 32), held twice: by the slot and by the shard's key index.
            let stats = self.resolve_instance.mem_stats(&|_, v| v.0.as_ref().map_or(0, |i| i.args.capacity() * std::mem::size_of::<crate::consts::GenericArg>()));
            let mut vec_keys = 0usize;
            let inline = std::mem::size_of::<(TypingEnv, DefId, Vec<TyId>)>() - std::mem::size_of::<(TypingEnv, DefId, crate::types::TyList)>();
            self.resolve_instance.for_each(&mut |k, _, _| {
                let n = tys.list(k.2).len();
                vec_keys += 2 * (inline + if n == 0 { 0 } else { (n * 4 + 8).div_ceil(16).max(2) * 16 });
            });
            out.push((format!("qt resolve_instance ({}; {} B; Vec<TyId> keys would add {} B)", stats.0, stats.1, vec_keys), stats.1 as u64));
        }
        {
            let mut acc = std::collections::BTreeMap::new();
            let (mut n, mut shared) = (0u64, 0u64);
            let mut prev = 0u64;
            self.typeck.for_each(&mut |_, v, _| {
                n += 1;
                if std::sync::Arc::strong_count(&v.1) > 1 { shared += 1; }
                v.1.mem_breakdown(&mut acc);
                prev += v.1.prev_layout_bytes(tys) as u64;
            });
            let total: u64 = acc.values().map(|x| x.1).sum();
            out.push((format!("qt typeck bodies ({n}; {shared} also held elsewhere; {total} B; previous layout {prev} B)"), total));
            // reads: [generic owner][read in the last 10 s] -> (bodies, bytes, reads, late reads)
            let now = TYPECK_READS_T0.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64;
            let reads = TYPECK_READS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
            let mut cls = [[(0u64, 0u64, 0u64, 0u64); 2]; 2];
            self.typeck.for_each(&mut |def, v, _| {
                let (_, last, n, late) = reads.get(&def.0).copied().unwrap_or((0, 0, 0, 0));
                let c = &mut cls[!v.1.args.is_empty() as usize][(now.saturating_sub(last) < 10_000) as usize];
                c.0 += 1;
                c.1 += (std::mem::size_of::<crate::typeck::Body>() + v.1.heap_bytes()) as u64;
                c.2 += n as u64;
                c.3 += late as u64;
            });
            for (g, gname) in [(0, "non-generic"), (1, "generic")] {
                for (r, rname) in [(0, "idle 10 s"), (1, "read in the last 10 s")] {
                    let c = cls[g][r];
                    out.push((format!("  typeck reads: {gname} owners, {rname}: {} bodies, {} reads, {} reads > 2 s after the first; {} B", c.0, c.2, c.3, c.1), c.1));
                }
            }
            for (k, (e, b)) in acc {
                out.push((format!("  typeck.{k} ({e}; {b} B)"), b));
            }
        }
        out
    }
    pub(crate) fn read_declarations(&self) -> Result<Fingerprint, QueryError> {
        self.engine.input(DECLARATIONS.kind, DECLARATIONS.key)
    }
    pub(crate) fn engine_mem_lines(&self) -> Vec<(String, u64)> { self.engine.mem_lines() }

    pub fn new() -> Self {
        let mut engine = QueryEngine::new();
        engine.register_input(DECLARATIONS.kind, DECLARATIONS.key, Fingerprint::from_bytes(b"declarations")).unwrap();
        engine.set_input(DECLARATIONS.kind, DECLARATIONS.key, Fingerprint::from_bytes(&0u64.to_le_bytes())).unwrap();
        Self { file_count: 0, crate_count: 0, type_param_clauses: QueryTable::new(20, type_param_key_fp, item_bounds_fp), specialization_graph: QueryTable::new(18, coherence_key_fp, property_bool_fp), inherent_overlap: QueryTable::new(19, opaque_def_fp, property_bool_fp), opaque_generics: QueryTable::new(15, opaque_def_fp, opaque_generics_fp), variances: QueryTable::new(17, opaque_def_fp, variances_fp), lifetime_names: QueryTable::new(13, opaque_def_fp, |v| Fingerprint::from_bytes(&v.0.to_le_bytes())), resolved_bound_vars: QueryTable::new(14, opaque_def_fp, |v| Fingerprint::from_bytes(&v.0.to_le_bytes())), param_env: QueryTable::new(12, opaque_def_fp, param_env_fp), compare_impl_item: QueryTable::new(11, opaque_def_fp, |r| property_bool_fp(&r.is_ok())), typeck: QueryTable::new(10, opaque_def_fp, typeck_fp), opaque_hidden_type: QueryTable::new(9, opaque_def_fp, opaque_ty_fp), engine, declarations: crate::next_solver::sizedness::DeclarationQueries::new(), solver: Default::default(), drop: QueryTable::new(1, type_fp, property_bool_fp), freeze: QueryTable::new(2, type_fp, property_bool_fp), normalize: QueryTable::new(3, type_fp, property_ty_fp),
            const_eval: QueryTable::new(4, const_key_fp, const_value_fp),
            codegen_select: QueryTable::new(22, hash_fp, hash_fp), resolve_instance: QueryTable::new(23, hash_fp, hash_fp),
            dyn_compatibility: QueryTable::new(24, opaque_def_fp, hash_fp), own_vtable_entries: QueryTable::new(25, opaque_def_fp, hash_fp),
            first_vtable_slot: QueryTable::new(26, hash_fp, hash_fp) }
    }

    /// Publish only under the same exclusive Tcx borrow that edits the tables.
    /// `visible`: the crates that can see the change (None: all of them).
    pub(crate) fn collected(&mut self, epoch: u64, visible: Option<&[bool]>) {
        self.collected_with_solver(epoch, visible, false);
    }

    pub(crate) fn collected_with_solver(&mut self, epoch: u64, visible: Option<&[bool]>, keep_solver: bool) {
        let fp = Fingerprint::from_bytes(&epoch.to_le_bytes());
        self.engine.set_input(DECLARATIONS.kind, DECLARATIONS.key, fp).unwrap();
        if let Some(visible) = visible { self.register_crates(visible.len()); }
        for krate in 0..self.crate_count {
            if visible.map_or(true, |v| v.get(krate as usize).copied().unwrap_or(true)) {
                self.engine.set_input(DECLARATIONS.kind, krate, fp).unwrap();
            }
        }
        let solver = &mut self.solver;
        if keep_solver {
            let before = solver.cached_len();
            solver.remove_misses();
            if std::env::var_os("MAKEPAD_RUN_INVALIDATION_TRACE").is_some() { eprintln!("invalidation-trace: solver dropped={} kept={}", before - solver.cached_len(), solver.cached_len()); }
            return;
        }
        let retired = std::mem::take(solver);
        if std::env::var_os("MAKEPAD_RUN_INVALIDATION_TRACE").is_some() { eprintln!("invalidation-trace: solver dropped={} kept=0", retired.cached_len()); }
        // Freeing a session's worth of solver entries took ~0.4 s of every declaration edit
        // (lead decision 52 R1); nothing reads the retired cache, so it is freed off the
        // edit's critical path. (rustc starts each incremental session with an empty cache.)
        if retired.len() > 4096 {
            let _ = std::thread::Builder::new().name("run-solver-free".into()).spawn(move || drop(retired));
        }
    }

    /// Settle-time retirement (caller: exclusive Tcx borrow, FRONT_RW written): the solver cache and the
    /// values of the type-side query tables that a query derives again on demand. A body checked
    /// ahead that something else still holds (a second Arc) is not retired. Returns the number of
    /// table values and solver entries dropped.
    pub(crate) fn retire_settled(&mut self) -> (usize, usize) {
        // a body with errors is kept: recomputing it would report the taint message instead of its
        // errors (the diagnostics context drops repeated reports)
        let mut values = self.typeck.retire(&|_, v| std::sync::Arc::strong_count(&v.1) > 1 || !v.1.errors.is_empty());
        values += self.resolved_bound_vars.retire(&|_, v| std::sync::Arc::strong_count(&v.1) > 1);
        // after resolved_bound_vars: its values hold the lifetime resolutions
        values += self.lifetime_names.retire(&|_, v| std::sync::Arc::strong_count(&v.1) > 1);
        values += self.resolve_instance.retire(&|_, _| false);
        values += self.codegen_select.retire(&|_, _| false);
        let retired = std::mem::take(&mut self.solver);
        let entries = retired.cached_len();
        let _ = std::thread::Builder::new().name("run-solver-free".into()).spawn(move || drop(retired));
        (values, entries)
    }

    pub fn register_crates(&mut self, count: usize) {
        while self.crate_count < count as u32 {
            let krate = self.crate_count;
            self.engine.register_input(DECLARATIONS.kind, krate, Fingerprint::from_bytes(&(krate as u64 | 1 << 40).to_le_bytes())).unwrap();
            self.engine.set_input(DECLARATIONS.kind, krate, Fingerprint::default()).unwrap();
            self.crate_count += 1;
        }
    }

    /// The declarations an owner of crate `krate` can see. rustc's separate compilation:
    /// a crate's queries observe only its own and its dependencies' declarations; the
    /// orphan rules keep a downstream crate's impls out of every upstream goal.
    pub(crate) fn read_crate_declarations(&self, krate: u32) -> Result<Fingerprint, QueryError> {
        if krate >= self.crate_count { return self.read_declarations(); }
        self.engine.input(DECLARATIONS.kind, krate)
    }

    /// The declaration inputs a type-keyed query reads (lead decision 52 R1): under the empty
    /// environment after analysis, the answer for a type depends only on the declarations of
    /// the crates naming its items -- impls of a trait for it live in the trait's or a named
    /// type's crate (orphan rules), its fields are declared in its ADTs' crates -- and each
    /// crate's input covers its dependencies. Any other key (a generic environment, opaque
    /// types being defined, a closure, an unevaluated constant, ...) reads every crate.
    fn read_type_declarations(&self, tcx: &Tcx, prog: &Program, key: &TypeKey) -> Result<(), QueryError> {
        self.read_key_declarations(tcx, prog, key.env, &[key.ty], &[])
    }

    fn read_key_declarations(&self, tcx: &Tcx, prog: &Program, env: TypingEnv, types: &[TyId], defs: &[DefId]) -> Result<(), QueryError> {
        use crate::consts::ConstKind;
        let all = |q: &Self| q.read_declarations().map(|_| ());
        if env.param_env != crate::tcx::ParamEnvId::EMPTY || !matches!(env.mode, crate::tcx::TypingMode::PostAnalysis | crate::tcx::TypingMode::Codegen) {
            return all(self);
        }
        let mut crates: Vec<u32> = Vec::new();
        let add = |d: DefId, crates: &mut Vec<u32>| -> bool {
            if d == crate::program::NO_DEF || (d.0 as usize) >= prog.defs.len() { return false; }
            let k = prog.def(d).krate;
            if !crates.contains(&k) { crates.push(k); }
            true
        };
        for &d in defs { if !add(d, &mut crates) { return all(self); } }
        let mut todo = types.to_vec();
        while let Some(t) = todo.pop() {
            if todo.len() > 256 { return all(self); }
            let known = match tcx.tys.kind(t) {
                TyKind::Bool | TyKind::Char | TyKind::Str | TyKind::Int(_) | TyKind::Float(_) | TyKind::Never | TyKind::Error | TyKind::Param(_) => true,
                TyKind::Tuple(ts) => { todo.extend_from_slice(tcx.tys.list(*ts)); true }
                TyKind::Slice(x) | TyKind::Ptr(_, x) | TyKind::Ref(_, x, _) => { todo.push(*x); true }
                TyKind::Array(x, c) => { todo.push(*x); match tcx.tys.const_kind(*c) { ConstKind::Value(v) => { todo.push(v.ty); true } _ => false } }
                TyKind::Const(c) => match tcx.tys.const_kind(*c) { ConstKind::Value(v) => { todo.push(v.ty); true } _ => false },
                TyKind::Adt(d, args, _) | TyKind::FnDef(d, args, _) => { todo.extend_from_slice(tcx.tys.list(*args)); add(*d, &mut crates) }
                TyKind::Foreign(d) => add(*d, &mut crates),
                TyKind::FnPtr(ps, r, _, _) => { todo.extend_from_slice(tcx.tys.list(*ps)); todo.push(*r); true }
                TyKind::Alias(_, a) => { todo.extend_from_slice(tcx.tys.list(a.args)); add(a.kind.def_id(), &mut crates) }
                TyKind::Dyn(p, args, bindings, _, _, _, autos) => {
                    let mut ok = *p == crate::program::NO_DEF || add(*p, &mut crates);
                    todo.extend(tcx.tys.list(*args).iter().copied());
                    for &(d, x, _) in tcx.tys.list(*bindings).iter() { ok &= add(d, &mut crates); todo.push(x); }
                    for &d in tcx.tys.list(*autos).iter() { ok &= add(d, &mut crates); }
                    ok
                }
                _ => false,
            };
            if !known { return all(self); }
        }
        if crates.is_empty() { return all(self); }
        for k in crates { self.read_crate_declarations(k)?; }
        Ok(())
    }

    fn read_trait_ref_declarations(&self, tcx: &Tcx, prog: &Program, env: TypingEnv, tr: &crate::next_solver::TraitRef) -> Result<(), QueryError> {
        let mut types = Vec::new();
        for arg in &tr.args {
            match arg {
                GenericArg::Type(ty) => types.push(*ty),
                GenericArg::Const(c) => types.push(tcx.tys.intern(TyKind::Const(*c))),
                GenericArg::Lifetime(_) => {},
            }
        }
        self.read_key_declarations(tcx, prog, env, &types, &[tr.def_id])
    }

    pub(crate) fn read_def_declarations(&self, prog: &Program, def: DefId) -> Result<Fingerprint, QueryError> {
        if (def.0 as usize) >= prog.defs.len() { return self.read_declarations(); }
        self.read_crate_declarations(prog.def(def).krate)
    }

    pub fn register_files(&mut self, count: usize) {
        while self.file_count < count as u32 {
            let file = self.file_count;
            self.engine.register_input(FILE_BODIES, file, Fingerprint::from_bytes(&file.to_le_bytes())).unwrap();
            self.engine.set_input(FILE_BODIES, file, Fingerprint::default()).unwrap();
            self.file_count += 1;
        }
    }

    pub(crate) fn file_bodies_changed(&mut self, files: &[u32], epoch: u64) {
        for &file in files {
            self.register_files(file as usize + 1);
            self.engine.set_input(FILE_BODIES, file, Fingerprint::from_bytes(&epoch.to_le_bytes())).unwrap();
        }
    }

    // Lead design rounds 47-48. Nightly resolve_bound_vars.rs:253-285,430-442
    // traverses whole owners. File-relative nodes require the entire edited file
    // to change, including unchanged siblings; declarations keep their own input.
    fn read_owner(&self, prog: &Program, owner: DefId) -> Result<(), QueryError> {
        self.read_def_declarations(prog, owner)?;
        let def = prog.def(owner);
        let pf = &prog.files[def.file as usize];
        let has_body = match def.kind {
            crate::program::DefKind::AnonConst | crate::program::DefKind::Closure => true,
            _ => match pf.ast.item(def.item).kind {
                crate::ast::ItemKind::Fn(sig, body, _) => body.is_some() || pf.ast.fn_sig(sig).lazy_body.is_some(),
                crate::ast::ItemKind::Const(_, body) | crate::ast::ItemKind::Static(_, _, body) => body.is_some(),
                _ => false,
            },
        };
        if has_body { self.engine.input(FILE_BODIES, def.file)?; }
        Ok(())
    }

}

pub(crate) struct Providers<'a> { pub(crate) tcx: &'a Tcx, pub(crate) prog: &'a Program }
impl Providers<'_> {
    pub(crate) fn force(&self, id: QueryId) -> Result<(), QueryError> {
        let q = &self.tcx.queries;
        match id.kind {
            20 => { self.type_param_clauses(q.type_param_clauses.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            22 => { let _ = self.codegen_select_candidate(q.codegen_select.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            23 => { let _ = self.resolve_instance_raw(q.resolve_instance.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            24 => { self.dyn_compatibility_violations(q.dyn_compatibility.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            25 => { self.own_existential_vtable_entries(q.own_vtable_entries.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            26 => { self.first_method_vtable_slot(q.first_vtable_slot.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            18 => { self.specialization_graph(q.specialization_graph.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            19 => { self.crate_inherent_impls_overlap_check(q.inherent_overlap.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            15 => { self.opaque_generics(q.opaque_generics.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            17 => { self.variances_of(q.variances.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            13 => { self.resolve_lifetime_names(q.lifetime_names.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            14 => { self.resolve_bound_vars(q.resolved_bound_vars.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            1 => { self.needs_drop(q.drop.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            2 => { self.is_freeze(q.freeze.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            12 => { self.param_env(q.param_env.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            11 => { let _ = self.compare_impl_item(q.compare_impl_item.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            10 => { self.typeck(q.typeck.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            9 => { let _ = self.type_of_opaque_hir_typeck(q.opaque_hidden_type.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            3 => { let _ = self.try_normalize_generic_arg_after_erasing_regions(q.normalize.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            4 => { let _ = self.const_eval_resolve(q.const_eval.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; }
            5..=8 => self.force_declaration(id)?,
            _ => return Err(QueryError::MissingQuery(id)),
        }
        Ok(())
    }

    pub(crate) fn specialization_graph(&self, key: (u32, DefId)) -> Result<bool, QueryError> {
        self.specialization_graph_with(key, None)
    }
    /// `impls`: the trait's impls in program order when the caller has them (coherence_check).
    fn specialization_graph_with(&self, key: (u32, DefId), impls: Option<&[DefId]>) -> Result<bool, QueryError> {
        let q=&self.tcx.queries;
        q.specialization_graph.get(&q.engine,key,&mut |(krate,def)| {
            q.read_crate_declarations(krate)?;
            q.read_def_declarations(self.prog, def)?;
            Ok(match impls {
                Some(impls) => crate::next_solver::specialization_graph::specialization_graph_of(self.tcx,self.prog,krate,def,impls),
                None => crate::next_solver::specialization_graph::specialization_graph(self.tcx,self.prog,krate,def),
            })
        },&|id| self.force(id))
    }
    pub(crate) fn crate_inherent_impls_overlap_check(&self, krate: DefId) -> Result<bool, QueryError> {
        let q=&self.tcx.queries;
        q.inherent_overlap.get(&q.engine,krate,&mut |krate| {
            q.read_crate_declarations(krate.0)?;
            Ok(crate::next_solver::inherent_impls_overlap::crate_inherent_impls_overlap_check(self.tcx,self.prog,krate.0))
        },&|id| self.force(id))
    }
    pub(crate) fn opaque_generics(&self, def: DefId) -> Result<crate::next_solver::rpitit::OpaqueGenerics, QueryError> {
        let q = &self.tcx.queries;
        q.opaque_generics.get(&q.engine, def, &mut |def| {
            q.read_def_declarations(self.prog, def)?;
            Ok(self.tcx.opaque_generics_uncached(self.prog, def))
        }, &|id| self.force(id))
    }
    pub(crate) fn variances_of(&self, def: DefId) -> Result<Vec<crate::types::variance::V>, QueryError> {
        let q = &self.tcx.queries;
        q.variances.get(&q.engine, def, &mut |def| {
            q.read_def_declarations(self.prog, def)?;
            Ok(self.tcx.variances_of_uncached(self.prog, def))
        }, &|id| self.force(id))
    }

    fn resolve_lifetime_names(&self, def: DefId) -> Result<std::sync::Arc<crate::tcx::lifetime_resolve::LifetimeResolution>, QueryError> {
        let q = &self.tcx.queries;
        let (_, names) = q.lifetime_names.get(&q.engine, def, &mut |def| {
            q.read_owner(self.prog, def)?;
            let parent = self.prog.def(def).parent;
            let parent = if parent != crate::program::NO_DEF && matches!(self.prog.def(parent).kind, crate::program::DefKind::Trait | crate::program::DefKind::Impl) { Some(self.resolve_lifetime_names(parent)?) } else { None };
            let names = crate::tcx::lifetime_resolve::resolve_lifetimes(self.tcx, self.prog, def, parent.as_deref());
            Ok((self.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), names.share()))
        }, &|id| self.force(id))?;
        Ok(names)
    }

    fn resolve_bound_vars(&self, def: DefId) -> Result<std::sync::Arc<crate::tcx::resolve_bound_vars::ResolveBoundVars>, QueryError> {
        let q = &self.tcx.queries;
        let (_, vars) = q.resolved_bound_vars.get(&q.engine, def, &mut |def| {
            q.read_owner(self.prog, def)?;
            let parent = self.prog.def(def).parent;
            let parent = if parent != crate::program::NO_DEF && matches!(self.prog.def(parent).kind, crate::program::DefKind::Trait | crate::program::DefKind::Impl) { Some(self.resolve_bound_vars(parent)?) } else { None };
            let vars = crate::tcx::resolve_bound_vars::resolve_bound_vars(self.tcx, self.prog, def, parent.as_deref());
            Ok((self.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), vars.share()))
        }, &|id| self.force(id))?;
        Ok(vars)
    }

    fn type_param_clauses(&self, key: (DefId, u32, crate::program::Sym)) -> Result<Vec<crate::next_solver::Predicate>, QueryError> {
        let q = &self.tcx.queries;
        q.type_param_clauses.get(&q.engine, key, &mut |(owner, param, assoc)| {
            q.read_def_declarations(self.prog, owner)?;
            Ok(self.tcx.compute_type_param_clauses(self.prog, owner, param, assoc))
        }, &|id| self.force(id))
    }

    /// Selection observes the crates named by its trait and arguments (including dependencies).
    fn codegen_select_candidate(&self, key: (TypingEnv, crate::next_solver::TraitRef)) -> Result<Result<crate::next_solver::select::ImplSource, crate::next_solver::codegen::CodegenObligationError>, QueryError> {
        let q = &self.tcx.queries;
        let mut computed = false;
        let (r, misses) = q.codegen_select.get(&q.engine, key, &mut |(typing_env, trait_ref)| {
            q.read_trait_ref_declarations(self.tcx, self.prog, typing_env, &trait_ref)?;
            computed = true;
            let value = with_misses(|| self.tcx.codegen_select_candidate_uncached(self.prog, typing_env, trait_ref));
            if !value.1.is_empty() { q.read_declarations()?; }
            Ok(value)
        }, &|id| self.force(id))?;
        if !computed && !misses.is_empty() { crate::program::note_misses(&misses); }
        Ok(r)
    }

    fn resolve_instance_raw(&self, key: (TypingEnv, DefId, crate::types::TyList)) -> Result<Result<crate::instance::Instance, crate::next_solver::codegen::CodegenObligationError>, QueryError> {
        let q = &self.tcx.queries;
        let mut computed = false;
        let (r, misses) = q.resolve_instance.get(&q.engine, key, &mut |(typing_env, def, args)| {
            let args = self.tcx.tys.list(args);
            q.read_key_declarations(self.tcx, self.prog, typing_env, args, &[def])?;
            computed = true;
            let value = with_misses(|| self.tcx.resolve_instance_raw_uncached(self.prog, typing_env, def, args));
            if !value.1.is_empty() { q.read_declarations()?; }
            Ok(value)
        }, &|id| self.force(id))?;
        if !computed && !misses.is_empty() { crate::program::note_misses(&misses); }
        Ok(r)
    }

    fn dyn_compatibility_violations(&self, def: DefId) -> Result<Vec<DefId>, QueryError> {
        let q = &self.tcx.queries;
        q.dyn_compatibility.get(&q.engine, def, &mut |def| {
            q.read_def_declarations(self.prog, def)?;
            Ok(self.tcx.dyn_compatibility_violations_uncached(self.prog, def))
        }, &|id| self.force(id))
    }
    fn own_existential_vtable_entries(&self, def: DefId) -> Result<Vec<DefId>, QueryError> {
        let q = &self.tcx.queries;
        q.own_vtable_entries.get(&q.engine, def, &mut |def| {
            q.read_def_declarations(self.prog, def)?;
            Ok(self.tcx.own_existential_vtable_entries_uncached(self.prog, def))
        }, &|id| self.force(id))
    }
    fn first_method_vtable_slot(&self, key: crate::next_solver::TraitRef) -> Result<usize, QueryError> {
        let q = &self.tcx.queries;
        let mut computed = false;
        let (value, misses) = q.first_vtable_slot.get(&q.engine, key, &mut |key| {
            q.read_trait_ref_declarations(self.tcx, self.prog, TypingEnv::fully_monomorphized(self.prog), &key)?;
            computed = true;
            let value = with_misses(|| self.tcx.first_method_vtable_slot_uncached(self.prog, &key));
            if !value.1.is_empty() { q.read_declarations()?; }
            Ok(value)
        }, &|id| self.force(id))?;
        if !computed && !misses.is_empty() { crate::program::note_misses(&misses); }
        Ok(value)
    }

    fn param_env(&self, def: DefId) -> Result<crate::tcx::ParamEnvId, QueryError> {
        let q = &self.tcx.queries;
        q.param_env.get(&q.engine, def, &mut |def| {
            q.read_def_declarations(self.prog, def)?;
            Ok(self.tcx.intern_param_env(crate::next_solver::param_env::param_env(self.tcx, self.prog, def)))
        }, &|id| self.force(id))
    }

    pub(crate) fn compare_impl_item(&self, def: DefId) -> Result<Result<(), crate::next_solver::NoSolution>, QueryError> {
        let q = &self.tcx.queries;
        q.compare_impl_item.get(&q.engine, def, &mut |def| {
            q.read_def_declarations(self.prog, def)?;
            Ok(self.tcx.compare_impl_item_uncached(self.prog, def))
        }, &|id| self.force(id))
    }

    fn typeck(&self, def: DefId) -> Result<std::sync::Arc<crate::typeck::Body>, QueryError> {
        let q = &self.tcx.queries;
        let (_, body) = q.typeck.get(&q.engine, def, &mut |def| {
            q.read_owner(self.prog, def)?;
            let mark = crate::program::missing_mark();
            let mut body = crate::typeck::check_fn(self.prog, self.tcx, def);
            if !body.errors.is_empty() { body.deferred_misses = crate::program::missing_since(mark).into(); }
            Ok((self.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), std::sync::Arc::new(body)))
        }, &|id| self.force(id))?;
        // the post-boot loader's misses travel with the cached failure (Body::deferred_misses)
        if !body.deferred_misses.is_empty() { crate::program::note_misses(&body.deferred_misses); }
        if crate::memstat::enabled() { note_typeck_read(def); }
        Ok(body)
    }

    fn const_eval_resolve(&self, key: ConstEvalKey) -> Result<EvalToValTreeResult, QueryError> {
        let q = &self.tcx.queries;
        q.const_eval.get(&q.engine, key, &mut |key| {
            q.read_declarations()?;
            let value = crate::const_eval::eval_to_valtree(self.tcx, self.prog, &key);
            if let Err(ErrorHandled::Reported(error)) = &value {
                let d = self.prog.def(key.def);
                self.tcx.dcx.report(crate::next_solver::fulfill::ObligationCause {
                    body_id: key.def, span: self.prog.files[d.file as usize].ast.item(d.item).lo,
                }, format!("E0080: constant evaluation failed: {error:?}"));
            }
            Ok(value)
        }, &|id| self.force(id))
    }

    /// rustc_traits/src/normalize_erasing_regions.rs:11-16 provider.
    fn try_normalize_generic_arg_after_erasing_regions(&self, key: TypeKey) -> Result<Result<TyId, crate::next_solver::NoSolution>, QueryError> {
        let q = &self.tcx.queries;
        q.normalize.get(&q.engine, key, &mut |key| {
            q.read_type_declarations(self.tcx, self.prog, &key)?;
            let value = self.tcx.try_normalize_after_erasing_regions(self.prog, key.env, key.ty);
            Ok(value)
        }, &|id| self.force(id))
    }

    /// rustc_ty_utils/src/needs_drop.rs:108,193,236,354: traverse components,
    /// normalize fields, and stop at destructors. A seen set belongs to this walk,
    /// never a provisional false entry in the query cache.
    fn needs_drop(&self, key: TypeKey) -> Result<bool, QueryError> {
        let q = &self.tcx.queries;
        q.drop.get(&q.engine, key, &mut |key| {
            q.read_type_declarations(self.tcx, self.prog, &key)?;
            let value = self.needs_drop_raw(key)?;
            Ok(value)
        }, &|id| self.force(id))
    }
    fn needs_drop_raw(&self, key: TypeKey) -> Result<bool, QueryError> {
            let tcx = self.tcx;
            let mut todo = vec![key.ty];
            let mut seen = HashSet::default();
            while let Some(ty) = todo.pop() {
                // needs_drop.rs:193-200: a component that does not normalize always
                // requires drop
                let Ok(ty) = tcx.try_normalize_erasing_regions(self.prog, key.env, ty) else { return Ok(true) };
                if !seen.insert(ty) { continue; }
                if tcx.is_coroutine(ty) { return Ok(true); }
                match tcx.tys.kind(ty) {
                    TyKind::Adt(d, args, _) => {
                        let adt = &tcx.adts[d];
                        if adt.is_manually_drop() || adt.is_phantom_data() { continue; }
                        if adt.is_box() || tcx.adt_destructor(*d).is_some() { return Ok(true); }
                        if adt.is_union() { continue; }
                        for v in &adt.variants {
                            for f in &v.fields { todo.push(tcx.tys.subst(f.ty, tcx.tys.list(*args))); }
                        }
                    }
                    TyKind::Tuple(fields) => todo.extend_from_slice(tcx.tys.list(*fields)),
                    TyKind::Array(elem, len) => { if len.try_to_target_usize(&self.tcx.tys) != Some(0) { todo.push(*elem); } }
                    TyKind::Slice(elem) | TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: elem, .. }) => todo.push(*elem),
                    TyKind::Param(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, .. }) => {
                        // needs_drop.rs:108-118: a Copy component never needs drop
                        if !tcx.is_copy_modulo_regions(self.prog, key.env, ty) { return Ok(true); }
                    }
                    TyKind::Dyn(..) | TyKind::Error | TyKind::Infer(_) => return Ok(true),
                    _ => {}
                }
            }
            Ok(false)
    }

    /// Structural Freeze until T5: rustc_middle/src/ty/util.rs:1144 and
    /// library/core/src/marker.rs:904-917 (negative UnsafeCell, pointer/PhantomData impls).
    fn is_freeze(&self, key: TypeKey) -> Result<bool, QueryError> {
        let q = &self.tcx.queries;
        q.freeze.get(&q.engine, key, &mut |key| {
            q.read_type_declarations(self.tcx, self.prog, &key)?;
            let value = self.is_freeze_raw(key)?;
            Ok(value)
        }, &|id| self.force(id))
    }
    fn is_freeze_raw(&self, key: TypeKey) -> Result<bool, QueryError> {
            let tcx = self.tcx;
            let mut todo = vec![key.ty];
            let mut seen = HashSet::default();
            while let Some(ty) = todo.pop() {
                let Ok(ty) = tcx.try_normalize_erasing_regions(self.prog, key.env, ty) else { return Ok(false) };
                if !seen.insert(ty) { continue; }
                match tcx.tys.kind(ty) {
                    TyKind::Adt(d, args, _) => {
                        let adt = &tcx.adts[d];
                        if adt.is_unsafe_cell() { return Ok(false); }
                        if adt.is_phantom_data() { continue; }
                        for v in &adt.variants {
                            for f in &v.fields { todo.push(tcx.tys.subst(f.ty, tcx.tys.list(*args))); }
                        }
                    }
                    TyKind::Tuple(fields) => todo.extend_from_slice(tcx.tys.list(*fields)),
                    TyKind::Array(elem, _) | TyKind::Slice(elem) => todo.push(*elem),
                    TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => {
                        if tcx.is_coroutine(ty) { return Ok(false); }
                        todo.push(*up);
                    }
                    TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Never | TyKind::Str
                    | TyKind::Ref(..) | TyKind::Ptr(..) | TyKind::FnPtr(..) | TyKind::FnDef(..) => {}
                    _ => return Ok(false),
                }
            }
            Ok(true)
    }
}

impl Tcx {
    /// rustc_hir_typeck/src/lib.rs:84-101: closures share their typeck root.
    pub fn typeck(&self, prog: &Program, mut def: DefId) -> std::sync::Arc<crate::typeck::Body> {
        while prog.def(def).kind == crate::program::DefKind::Closure { def = prog.def(def).parent; }
        Providers { tcx: self, prog }.typeck(def).expect("typeck query")
    }

    /// Nightly mir/interpret/queries.rs:58,90: retain the environment and full
    /// instance arguments. The provider folds or executes the resolved constant.
    pub fn const_was_evaluated(&self, def: DefId) -> bool {
        let mut found = false;
        self.queries.const_eval.for_each(&mut |key, value, _| {
            found |= key.def == def && value.is_ok();
        });
        found
    }

    pub fn const_eval_resolve(&self, prog: &Program, mut typing_env: TypingEnv, def: DefId, args: &[GenericArg]) -> EvalToValTreeResult {
        // Round-31 L1': executable CTFE runs under the final crate's policy,
        // including requests made while analysing a dependency's declaration.
        typing_env.limit = TypingEnv::fully_monomorphized(prog).limit;
        // queries.rs:66: context-local inference variables cannot enter this cache.
        for arg in args {
            let infer = match arg {
                GenericArg::Type(ty) => self.tys.has_infer(*ty),
                GenericArg::Const(ct) => ct.has_non_region_infer(&self.tys),
                GenericArg::Lifetime(_) => false,
            };
            assert!(!infer, "const_eval_resolve received non-region inference variables");
        }
        Providers { tcx: self, prog }.const_eval_resolve(ConstEvalKey { typing_env, def, args: args.into() })
            .expect("const_eval_resolve query")
    }

    pub fn needs_drop(&self, prog: &Program, env: TypingEnv, ty: TyId) -> bool {
        Providers { tcx: self, prog }.needs_drop(TypeKey { env, ty }).expect("needs_drop query")
    }

    pub fn is_freeze(&self, prog: &Program, env: TypingEnv, ty: TyId) -> bool {
        Providers { tcx: self, prog }.is_freeze(TypeKey { env, ty }).expect("is_freeze query")
    }

    /// rustc_middle/src/ty/mod.rs:1449: field declaration + plain instantiation.
    /// Query providers reading declarations must first read the collected-program
    /// input, until Q3 supplies rustc's per-item type_of query.
    pub fn field_ty(&self, prog: &Program, env: TypingEnv, adt: DefId, variant: u32, field: u32, args: &[TyId]) -> TyId {
        let decl = self.adts[&adt].variants[variant as usize].fields[field as usize].ty;
        self.normalize_erasing_regions(prog, env, self.tys.subst(decl, args))
    }

    /// rustc_middle/src/ty/normalize_erasing_regions.rs:41-66: outside type inference,
    /// normalization is assumed to succeed.
    pub fn normalize_erasing_regions(&self, prog: &Program, env: TypingEnv, ty: TyId) -> TyId {
        match self.try_normalize_erasing_regions(prog, env, ty) {
            Ok(ty) => ty,
            Err(_) => panic!("Failed to normalize {}, maybe try to call try_normalize_erasing_regions instead", crate::typeck::ty_to_string(false, prog, self, ty)),
        }
    }

    /// rustc_middle/src/ty/normalize_erasing_regions.rs:140-149. `value` is
    /// one function binder's payload; nested binders keep their own regions.
    pub fn normalize_erasing_late_bound_regions(&self, prog: &Program, env: TypingEnv, value: TyId) -> TyId {
        let mut replacer = crate::types::fold::BoundVarReplacer::new(
            |_| crate::types::RegionId::ERASED, |_| panic!("bound type in a function region binder"));
        self.normalize_erasing_regions(prog, env, replacer.instantiate(&self.tys, value))
    }

    /// normalize_erasing_regions.rs:95-120: erase before keying; alias-free types bypass
    /// the query. HAS_ALIAS includes type and const aliases at every depth.
    pub fn try_normalize_erasing_regions(&self, prog: &Program, env: TypingEnv, ty: TyId) -> Result<TyId, crate::next_solver::NoSolution> {
        if !self.tys.flags(ty).intersects(crate::types::TypeFlags::HAS_ALIAS) { return Ok(self.tys.erase_regions(ty)); }
        let ty = self.tys.erase_regions(self.tys.set_aliases_to_non_rigid(ty));
        Providers { tcx: self, prog }.try_normalize_generic_arg_after_erasing_regions(TypeKey { env, ty }).expect("normalize_erasing_regions query")
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tcx::TypingMode;
    use crate::next_solver::ParamEnv;
    use crate::types::RegionArgsId;

    fn program(source: &[u8]) -> (Program, Tcx) {
        let dir = std::env::temp_dir().join(format!("relax-type-queries-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, source).unwrap();
        let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
        prog.load_crate("query_tests", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&prog.session, &[]), Vec::new()).unwrap();
        let mut tcx = Tcx::new();
        tcx.collect(&prog);

        tcx.publish_collected();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
        assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
        (prog, tcx)
    }
    fn named(prog: &Program, name: &str) -> DefId {
        for i in 0..prog.defs.len() {
            let d = DefId(i as u32);
            if prog.name(d) == name { return d; }
        }
        panic!("missing {name}")
    }
    fn adt(prog: &Program, tcx: &Tcx, name: &str, args: Vec<TyId>) -> TyId {
        tcx.tys.intern(TyKind::Adt(named(prog, name), tcx.tys.mk_list(&args), RegionArgsId::EMPTY))
    }
    #[test]
    fn round31_normalization_query_separates_limit_values() {
        use crate::tcx::RecursionLimit;
        let dir = std::env::temp_dir().join(format!("relax-r31-query-owner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
        let chain = "trait A { type Out; } trait B {} trait C {} trait D {} trait E {} impl<T: B> A for T { type Out = u8; } impl<T: C> B for T {} impl<T: D> C for T {} impl<T: E> D for T {} impl E for u32 {}";
        for (name, limit) in [("low", 1), ("high", 64), ("same", 64)] {
            let path = dir.join(format!("{name}.rs"));
            std::fs::write(&path, format!("#![no_std]\n#![recursion_limit = \"{limit}\"]\n{chain}\nfn {name}() {{}}")).unwrap();
            let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
            prog.load_crate(name, path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(dir).unwrap();
        let mut tcx = Tcx::new(); tcx.collect(&prog);
        let alias = tcx.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&prog, "Out") }, &[GenericArg::Type(tcx.tys.u32_)]);
        let env = |name| TypingEnv { param_env: Default::default(), mode: TypingMode::PostAnalysis, limit: RecursionLimit::for_item(&prog, named(&prog, name)) };
        let high = env("high"); let low = env("low");
        assert_eq!(high.limit, RecursionLimit(64)); assert_eq!(low.limit, RecursionLimit(1));
        assert_eq!(high, env("same"));
        for _ in 0..2 {
            assert_eq!(tcx.try_normalize_erasing_regions(&prog, high, alias), Ok(tcx.tys.u8_));
            assert!(tcx.try_normalize_erasing_regions(&prog, low, alias).is_err());
        }
        let high_key = TypeKey { env: high, ty: alias };
        let low_key = TypeKey { env: low, ty: alias };
        assert_ne!(tcx.queries.normalize.query_id(&high_key), tcx.queries.normalize.query_id(&low_key));
        tcx.publish_collected();
        assert!(tcx.try_normalize_erasing_regions(&prog, low, alias).is_err());
        assert_eq!(tcx.try_normalize_erasing_regions(&prog, high, alias), Ok(tcx.tys.u8_));
    }

    #[test]
    fn round32_const_query_reports_once_across_hits() {
        let (prog, tcx) = program(b"#![no_std] const BAD: u8 = 255 + 1;");
        let def = named(&prog, "BAD");
        for i in 0..2 {
            assert!(matches!(tcx.const_eval_resolve(&prog, TypingEnv::fully_monomorphized(&prog), def, &[]), Err(ErrorHandled::Reported(_))));
            let (reported, _) = tcx.dcx.drain_reported();
            assert_eq!(reported.len(), if i == 0 { 1 } else { 0 });
            assert_eq!(tcx.dcx.err_count(), 1);
        }
        tcx.dcx.flush_delayed();
    }

    #[test]
    fn typeck_is_definition_owned_and_invalidated() {
        let (prog, mut tcx) = program(b"#![no_std] fn identity<T>(value: T) -> T { let _closure = || (); value }");
        let def = named(&prog, "identity");
        let body = tcx.typeck(&prog, def);
        assert!(body.errors.is_empty(), "{:?}", body.errors);
        assert!(matches!(tcx.tys.kind(body.ret), TyKind::Param(0)));
        let closure = (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|d|
            prog.def(*d).kind == crate::program::DefKind::Closure && prog.def(*d).parent == def).unwrap();
        assert!(std::sync::Arc::ptr_eq(&body, &tcx.typeck(&prog, closure)));
        let state = crate::tcx::EvalState::new();
        let cx = crate::tcx::EvalCx::new(&prog, &tcx, TypingEnv::fully_monomorphized(&prog), &state);
        for ty in [tcx.tys.u8_, tcx.tys.bool_] {
            let instance = crate::typeck::instantiate_body(&cx, &prog, &tcx, def, &[ty]);
            assert!(instance.errors.is_empty(), "{:?}", instance.errors);
            assert_eq!(instance.ret, ty);
            assert!(std::sync::Arc::ptr_eq(&body, &tcx.typeck(&prog, def)));
        }
        let query = tcx.queries.typeck.query_id(&def).unwrap();
        // (an owner query reads its crate's declaration input: lead decision 52 R1)
        assert!(tcx.queries.engine.dependencies(query).contains(&QueryId { kind: 0, key: prog.def(def).krate }));
        tcx.publish_collected();
        let refreshed = tcx.typeck(&prog, def);
        assert!(!std::sync::Arc::ptr_eq(&body, &refreshed));
        assert_eq!(tcx.queries.typeck.query_id(&def), Some(query));
    }

    #[test]
    fn const_eval_identity_and_provider_invalidation() {
        use crate::consts::{Const, ScalarInt, ValTree};
        use crate::types::{RegionId, RegionKind};
        let (prog, mut tcx) = program(b"#![no_std] const A: u8 = 7; const B: u8 = 9;");
        let env = TypingEnv::fully_monomorphized(&prog);
        let n = Const::new_value(&tcx.tys, ValTree::from_scalar_int(&tcx.tys,
            ScalarInt::try_from_uint(1u128 << 127, 16).unwrap()), tcx.tys.intern(TyKind::Int(crate::program::Prim::U128)));
        let region = tcx.tys.intern_region(RegionKind::ReStatic);
        let mut key = ConstEvalKey { typing_env: env, def: named(&prog, "A"),
            args: vec![GenericArg::Lifetime(region), GenericArg::Type(tcx.tys.u8_), GenericArg::Const(n)].into() };
        let mut ids = Vec::new();
        for step in 0..5 {
            match step {
                1 => key.def = named(&prog, "B"),
                2 => key.typing_env.mode = TypingMode::Typeck { defining_opaque_types_and_generators: crate::tcx::DefiningOpaquesId::default() },
                3 => key.args = vec![GenericArg::Lifetime(RegionId::ERASED), GenericArg::Type(tcx.tys.u8_), GenericArg::Const(n)].into(),
                4 => key.args = vec![GenericArg::Lifetime(RegionId::ERASED), GenericArg::Type(tcx.tys.bool_), GenericArg::Const(n)].into(),
                _ => {}
            }
            assert_eq!(tcx.const_eval_resolve(&prog, key.typing_env, key.def, &key.args).unwrap().try_to_leaf().unwrap().to_bits(1), if key.def == named(&prog, "A") { 7 } else { 9 });
            let id = tcx.queries.const_eval.query_id(&key).unwrap();
            assert!(!ids.contains(&id));
            ids.push(id);
            // Lowering the const's declared type reads its owner's resolve_bound_vars (kind 14), itself
            // an input of the collected program; nothing else.
            let deps = tcx.queries.engine.dependencies(id);
            assert!(deps.contains(&DECLARATIONS));
            assert!(deps.iter().all(|d| d.kind == 0 || d.kind == 14), "{deps:?}");
        }
        tcx.publish_collected();
        assert_eq!(tcx.const_eval_resolve(&prog, key.typing_env, key.def, &key.args).unwrap().try_to_leaf().unwrap().to_bits(1), if key.def == named(&prog, "A") { 7 } else { 9 });
        assert_eq!(tcx.queries.const_eval.query_id(&key), ids.last().copied());
    }

    #[test]
    #[should_panic(expected = "const_eval_resolve received non-region inference variables")]
    fn const_eval_rejects_inference_before_caching() {
        use crate::consts::{AliasConst, AliasConstKind, ConstKind, IsRigid};
        let (prog, tcx) = program(b"#![no_std] const A: u8 = 7;");
        let def = named(&prog, "A");
        let ty = tcx.tys.intern(TyKind::Infer(0));
        let nested = tcx.tys.intern_const(ConstKind::Alias(IsRigid::No, AliasConst {
            kind: AliasConstKind::Free { def_id: def }, args: vec![GenericArg::Type(ty)].into(),
        }));
        let _ = tcx.const_eval_resolve(&prog, TypingEnv::fully_monomorphized(&prog), def, &[GenericArg::Const(nested)]);
    }

    #[test]
    fn drop_components_and_freeze() {
        let (prog, tcx) = program(br#"#![no_std]
            #[lang = "drop"] trait Drop { fn drop(&mut self); }
            #[lang = "copy"] trait Copy {}
            #[lang = "owned_box"] struct Heap<T>(*mut T);
            #[lang = "manually_drop"] struct Manual<T>(T);
            #[lang = "phantom_data"] struct Phantom<T>;
            #[lang = "unsafe_cell"] struct Cell<T>(T);
            struct D; impl Drop for D { fn drop(&mut self) {} }
            struct Node { next: Heap<Node> }
            struct Plain { next: *mut Plain }
            union U { d: D }
        "#);
        let env = TypingEnv::fully_monomorphized(&prog);
        let d = adt(&prog, &tcx, "D", vec![]);
        for name in ["D", "Node"] { assert!(tcx.needs_drop(&prog, env, adt(&prog, &tcx, name, vec![]))); }
        for name in ["Plain", "U"] { assert!(!tcx.needs_drop(&prog, env, adt(&prog, &tcx, name, vec![]))); }
        for name in ["Manual", "Phantom"] { assert!(!tcx.needs_drop(&prog, env, adt(&prog, &tcx, name, vec![d]))); }
        assert!(!tcx.needs_drop(&prog, env, tcx.tys.intern(TyKind::Array(d, tcx.tys.const_usize(0)))));
        assert!(tcx.needs_drop(&prog, env, tcx.tys.intern(TyKind::Array(d, tcx.tys.const_usize(2)))));
        assert!(tcx.needs_drop(&prog, env, tcx.tys.intern(TyKind::Slice(d))));
        let cell = adt(&prog, &tcx, "Cell", vec![tcx.tys.bool_]);
        assert!(!tcx.is_freeze(&prog, env, cell));
        assert!(!tcx.is_freeze(&prog, env, tcx.tys.intern(TyKind::Array(cell, tcx.tys.const_usize(0)))));
        assert!(tcx.is_freeze(&prog, env, adt(&prog, &tcx, "Phantom", vec![cell])));
        assert!(tcx.is_freeze(&prog, env, tcx.tys.intern(TyKind::Ptr(false, cell))));
        assert!(tcx.is_freeze(&prog, env, adt(&prog, &tcx, "Node", vec![])));
        let sig = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[]), tcx.tys.unit, crate::types::FnHeader::RUST, 0));
        let owner = tcx.tys.intern(TyKind::FnDef(named(&prog, "drop"), tcx.tys.mk_list(&[]), RegionArgsId::EMPTY));
        let up = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[d, cell])));
        let closure = tcx.tys.intern(TyKind::Closure(0, 0, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: tcx.tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: up }));
        assert!(tcx.needs_drop(&prog, env, closure));
        assert!(!tcx.is_freeze(&prog, env, closure));
    }
    #[test]
    fn polymorphic_copy_bounds() {
        let (prog, tcx) = program(br#"#![no_std]
            #[lang = "copy"] trait Copy {}
            trait Tr { type A: Copy; }
            struct S<T: Tr> { value: T::A }
        "#);
        let param = tcx.tys.intern(TyKind::Param(0));
        let predicates = ParamEnv { clauses: vec![crate::next_solver::collect::trait_predicate(named(&prog, "Copy"), vec![GenericArg::Type(param)])].into() };
        // the struct's own environment: `T: Tr` (its declared item bound makes T::A: Copy)
        let struct_env = ParamEnv { clauses: vec![crate::next_solver::collect::trait_predicate(named(&prog, "Tr"), vec![GenericArg::Type(param)])].into() };
        for mode in [TypingMode::Typeck { defining_opaque_types_and_generators: Default::default() }, TypingMode::PostTypeckUntilBorrowck { defining_opaque_types: Default::default() }, TypingMode::PostAnalysis] {
            let empty = TypingEnv { limit: Default::default(), param_env: Default::default(), mode };
            assert!(tcx.needs_drop(&prog, empty, param));
            let env = TypingEnv { limit: Default::default(), param_env: tcx.intern_param_env(predicates.clone()), mode };
            assert!(!tcx.needs_drop(&prog, env, param));
            let bounded = TypingEnv { limit: Default::default(), param_env: tcx.intern_param_env(struct_env.clone()), mode };
            let projection = tcx.field_ty(&prog, bounded, named(&prog, "S"), 0, 0, &[param]);
            assert!(!tcx.needs_drop(&prog, bounded, projection), "associated Copy bound in {mode:?}");
        }
    }
    #[test]
    fn collected_input_invalidates_fields_and_properties() {
        let (prog, mut tcx) = program(br#"#![no_std]
            #[lang = "drop"] trait Drop { fn drop(&mut self); }
            #[lang = "unsafe_cell"] struct Cell<T>(T);
            struct D; impl Drop for D { fn drop(&mut self) {} }
            struct S { value: bool }
        "#);
        let env = TypingEnv::fully_monomorphized(&prog);
        let d = adt(&prog, &tcx, "D", vec![]);
        let cell = adt(&prog, &tcx, "Cell", vec![d]);
        let sid = named(&prog, "S");
        let ty = adt(&prog, &tcx, "S", vec![]);
        let key = TypeKey { env, ty };
        assert!(!tcx.needs_drop(&prog, env, ty));
        assert!(tcx.is_freeze(&prog, env, ty));
        let drop_id = tcx.queries.drop.query_id(&key).unwrap();
        // (S names only its own crate's items: that crate's declaration input, R1)
        assert_eq!(tcx.queries.engine.dependencies(drop_id), vec![QueryId { kind: 0, key: prog.def(sid).krate }]);
        assert!(tcx.queries.normalize.query_id(&TypeKey { env, ty: tcx.tys.bool_ }).is_none());
        for next in [cell, tcx.tys.bool_] {
            tcx.adts.get_mut(&sid).unwrap().variants[0].fields[0].ty = next;
            tcx.publish_collected();
            assert_eq!(tcx.field_ty(&prog, env, sid, 0, 0, &[]), next);
            assert_eq!(tcx.needs_drop(&prog, env, ty), next == cell);
            assert_eq!(tcx.is_freeze(&prog, env, ty), next != cell);
        }
    }

    #[test]
    fn check_session_invalidation_updates_type_queries() {
        let (prog, mut tcx) = program(br#"#![no_std]
            #[lang = "drop"] trait Drop { fn drop(&mut self); }
            struct D; impl Drop for D { fn drop(&mut self) {} }
            struct S { value: bool }
            struct Removed;
        "#);
        let env = TypingEnv::fully_monomorphized(&prog);
        let sid = named(&prog, "S");
        let ty = adt(&prog, &tcx, "S", vec![]);
        let droppable = adt(&prog, &tcx, "D", vec![]);
        assert!(!tcx.needs_drop(&prog, env, ty));
        tcx.adts.get_mut(&sid).unwrap().variants[0].fields[0].ty = droppable;
        tcx.invalidate_check(&[]);
        assert!(tcx.needs_drop(&prog, env, ty));
        tcx.adts.get_mut(&sid).unwrap().variants[0].fields[0].ty = tcx.tys.bool_;
        tcx.invalidate_check(&[named(&prog, "Removed")]);
        assert!(!tcx.needs_drop(&prog, env, ty));
    }

    #[test]
    fn fields_only_query_aliases_after_substitution_and_erasure() {
        let (prog, tcx) = program(br#"#![no_std]
            trait Tr { type A; }
            struct S<T> { first: T, second: T }
        "#);
        let env = TypingEnv::fully_monomorphized(&prog);
        let sid = named(&prog, "S");
        let region = tcx.tys.intern_region(crate::types::RegionKind::ReStatic);
        let reference = tcx.tys.intern(TyKind::Ref(false, tcx.tys.bool_, region));
        let erased = tcx.tys.erase_regions(reference);
        assert_eq!(tcx.field_ty(&prog, env, sid, 0, 0, &[reference]), erased);
        assert!(tcx.queries.normalize.query_id(&TypeKey { env, ty: erased }).is_none());

        let projection = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: named(&prog, "A") }, args: tcx.tys.mk_list(&[reference]), regions: RegionArgsId::EMPTY }));
        let erased_projection = tcx.tys.erase_regions(projection);
        let predicates = ParamEnv { clauses: vec![
            crate::next_solver::collect::trait_predicate(named(&prog, "Tr"), vec![GenericArg::Type(erased)]),
            crate::next_solver::Predicate::dummy(crate::next_solver::PredicateKind::Clause(crate::next_solver::ClauseKind::Projection(crate::next_solver::ProjectionClause {
                projection_term: crate::next_solver::AliasTerm::Ty(erased_projection), term: crate::next_solver::Term::Ty(tcx.tys.char_) }))),
        ].into() };
        let env = TypingEnv { limit: Default::default(), param_env: tcx.intern_param_env(predicates), mode: TypingMode::PostTypeckUntilBorrowck { defining_opaque_types: Default::default() } };
        for field in 0..2 {
            assert_eq!(tcx.field_ty(&prog, env, sid, 0, field, &[projection]), tcx.tys.char_);
        }
        assert!(tcx.queries.normalize.query_id(&TypeKey { env, ty: projection }).is_none());
        let id = tcx.queries.normalize.query_id(&TypeKey { env, ty: erased_projection }).unwrap();
        assert_eq!(tcx.queries.engine.dependencies(id), vec![DECLARATIONS]);
        // has_assoc excludes closure captures; the normalization fast path must not.
        let sig = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[]), tcx.tys.unit, crate::types::FnHeader::RUST, 0));
        let closure = tcx.tys.intern(TyKind::Closure(0, 0, crate::types::ClosureArgs { parent_args: tcx.tys.unit, closure_kind_ty: tcx.tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: projection }));
        let normalized = tcx.normalize_erasing_regions(&prog, env, closure);
        assert!(matches!(tcx.tys.kind(normalized), TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) if *up == tcx.tys.char_));
    }

    #[test]
    fn unresolved_components_and_coroutines() {
        let (prog, tcx) = program(br#"#![no_std]
            #[lang = "copy"] trait Copy {}
            #[lang = "phantom_data"] struct Phantom<T>;
            trait Tr { type A; }
            trait Shown {}
            impl Shown for () {}
            fn owner() -> impl Shown {}
        "#);
        let env = TypingEnv::fully_monomorphized(&prog);
        let param = tcx.tys.intern(TyKind::Param(0));
        let projection = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: named(&prog, "A") }, args: tcx.tys.mk_list(&[param]), regions: RegionArgsId::EMPTY }));
        for ty in [param, projection] {
            assert!(tcx.needs_drop(&prog, env, ty));
            assert!(!tcx.is_freeze(&prog, env, ty));
        }
        // outside its defining scope an opaque is rigid: only its item bounds speak for it
        let opaque = tcx.fn_sig(&prog, named(&prog, "owner")).unwrap().ret;
        let analysis = TypingEnv { limit: Default::default(), param_env: Default::default(), mode: TypingMode::non_body_analysis() };
        assert!(tcx.needs_drop(&prog, analysis, opaque));
        assert!(!tcx.is_freeze(&prog, analysis, opaque));
        let len = tcx.tys.intern_const(crate::consts::ConstKind::Param(crate::consts::ParamConst { index: 0 }));
        assert!(tcx.needs_drop(&prog, env, tcx.tys.intern(TyKind::Array(param, len))));
        let phantom = adt(&prog, &tcx, "Phantom", vec![param]);
        assert!(!tcx.needs_drop(&prog, env, tcx.tys.intern(TyKind::Array(phantom, len))));
        let sig = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[]), tcx.tys.unit, crate::types::FnHeader::RUST, 0));
        let owner = tcx.tys.intern(TyKind::FnDef(named(&prog, "owner"), tcx.tys.mk_list(&[]), RegionArgsId::EMPTY));
        tcx.coroutines.insert((0, 1), ());
        let coroutine = tcx.tys.intern(TyKind::Closure(0, 1, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: tcx.tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: tcx.tys.unit }));
        assert!(tcx.needs_drop(&prog, env, coroutine));
        assert!(!tcx.is_freeze(&prog, env, coroutine));
    }

}

fn param_env_fp(result: &crate::tcx::ParamEnvId) -> Fingerprint {
    let mut h = KeyHasher(crate::sip::Sip128::new());
    result.hash(&mut h);
    Fingerprint(h.0.finish128())
}
impl Tcx {
    pub(crate) fn type_param_clauses(&self, prog: &Program, owner: DefId, param: u32, assoc: crate::program::Sym) -> Vec<crate::next_solver::Predicate> {
        Providers { tcx: self, prog }.type_param_clauses((owner, param, assoc)).expect("type_param_clauses query")
    }
    pub fn param_env(&self, prog: &Program, def: DefId) -> crate::next_solver::ParamEnv {
        self.param_env_by_id(self.param_env_id(prog, def)).clone()
    }
    /// The declaration query retains its interned identity, as rustc's param_env query does.
    pub fn param_env_id(&self, prog: &Program, def: DefId) -> crate::tcx::ParamEnvId {
        Providers { tcx: self, prog }.param_env(def).expect("param_env query")
    }
    /// The dyn_compatibility_violations query (provider: dyn_compatibility_violations_uncached).
    pub fn dyn_compatibility_violations(&self, prog: &Program, def: DefId) -> Vec<DefId> {
        Providers { tcx: self, prog }.dyn_compatibility_violations(def).expect("dyn_compatibility_violations query")
    }
    /// The own_existential_vtable_entries query (provider: own_existential_vtable_entries_uncached).
    pub fn own_existential_vtable_entries(&self, prog: &Program, def: DefId) -> Vec<DefId> {
        Providers { tcx: self, prog }.own_existential_vtable_entries(def).expect("own_existential_vtable_entries query")
    }
    /// The first_method_vtable_slot query (provider: first_method_vtable_slot_uncached).
    pub fn first_method_vtable_slot(&self, prog: &Program, key: &crate::next_solver::TraitRef) -> usize {
        Providers { tcx: self, prog }.first_method_vtable_slot(key.clone()).expect("first_method_vtable_slot query")
    }
    /// The codegen_select_candidate query (provider: codegen_select_candidate_uncached).
    pub fn codegen_select_candidate(&self, prog: &Program, typing_env: TypingEnv, trait_ref: crate::next_solver::TraitRef) -> Result<crate::next_solver::select::ImplSource, crate::next_solver::codegen::CodegenObligationError> {
        Providers { tcx: self, prog }.codegen_select_candidate((typing_env, trait_ref)).expect("codegen_select_candidate query")
    }
    /// The resolve_instance_raw query (provider: resolve_instance_raw_uncached).
    pub fn resolve_instance_raw(&self, prog: &Program, typing_env: TypingEnv, def: DefId, args: &[TyId]) -> Result<crate::instance::Instance, crate::next_solver::codegen::CodegenObligationError> {
        Providers { tcx: self, prog }.resolve_instance_raw((typing_env, def, self.tys.mk_list(args))).expect("resolve_instance_raw query")
    }
}

impl Tcx {
    pub(crate) fn resolve_lifetime_names(&self, prog: &Program, owner: DefId) -> std::sync::Arc<crate::tcx::lifetime_resolve::LifetimeResolution> {
        Providers { tcx: self, prog }.resolve_lifetime_names(bound_vars_owner(prog, owner)).expect("lifetime name resolution query")
    }
    pub(crate) fn resolve_bound_vars(&self, prog: &Program, owner: DefId) -> std::sync::Arc<crate::tcx::resolve_bound_vars::ResolveBoundVars> {
        Providers { tcx: self, prog }.resolve_bound_vars(bound_vars_owner(prog, owner)).expect("bound-variable resolution query")
    }
}

fn bound_vars_owner(prog: &Program, mut owner: DefId) -> DefId {
    if let Some(data) = prog.impl_trait_in_trait_data.get(&owner) {
        owner = match *data {
            relax_front::assoc::ImplTraitInTraitData::Trait { fn_def_id, .. } | relax_front::assoc::ImplTraitInTraitData::Impl { fn_def_id } => fn_def_id,
        };
    }
    while matches!(prog.def(owner).kind, crate::program::DefKind::AnonConst | crate::program::DefKind::OpaqueTy | crate::program::DefKind::Closure | crate::program::DefKind::Field | crate::program::DefKind::Variant) {
        owner = prog.def(owner).parent;
    }
    owner
}

fn opaque_generics_fp(value: &crate::next_solver::rpitit::OpaqueGenerics) -> Fingerprint {
    let mut hash = KeyHasher(crate::sip::Sip128::new()); value.hash(&mut hash); Fingerprint(hash.0.finish128())
}
fn item_bounds_fp(value: &Vec<crate::next_solver::Predicate>) -> Fingerprint {
    let mut hash = KeyHasher(crate::sip::Sip128::new()); value.hash(&mut hash); Fingerprint(hash.0.finish128())
}
fn variances_fp(value: &Vec<crate::types::variance::V>) -> Fingerprint {
    let mut hash = KeyHasher(crate::sip::Sip128::new()); value.hash(&mut hash); Fingerprint(hash.0.finish128())
}

impl Tcx {
    /// rustc_hir_analysis/src/coherence/mod.rs: coherence is checked after the
    /// complete impl index has been published, independently of body reachability.
    pub fn coherence_check(&self, prog: &Program) {
        self.coherence_check_of(prog, None);
    }

    /// `coherence_check` for the traits/inherent impls of `crates` only (None: every crate).
    pub fn coherence_check_of(&self, prog: &Program, crates: Option<&[u32]>) {
        let providers=Providers { tcx:self,prog };
        // The collected per-trait index supplies impls; this pass preserves each crate's
        // traits in the order of its first impl of them (what a per-crate scan finds).
        let mut traits_of: Vec<Vec<DefId>> = vec![Vec::new(); prog.crates.len()];
        let mut seen: crate::fxhash::HashSet<(u32, DefId)> = Default::default();
        for im in &prog.impls {
            let Some(&td)=self.impl_trait.get(&im.def) else { continue };
            let krate=prog.def(im.def).krate;
            if seen.insert((krate,td)) { traits_of[krate as usize].push(td); }
        }
        for krate in 0..prog.crates.len() as u32 {
            if matches!(prog.syms.str(prog.crates[krate as usize].name),"core"|"alloc"|"std"|"proc_macro") { continue; }
            if crates.is_some_and(|c| !c.contains(&krate)) { continue; }
            for &td in &traits_of[krate as usize] { providers.specialization_graph_with((krate,td), Some(&self.trait_impls_of[&td])).expect("coherence query"); }
            providers.crate_inherent_impls_overlap_check(DefId(krate)).expect("inherent coherence query");
        }
    }
}
