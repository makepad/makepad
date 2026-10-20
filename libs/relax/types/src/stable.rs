//! Stable identities: values that are the same in every run of the same sources, whatever the
//! interning or compile order (plans/relax-cache.md, step C0).
//!
//! - built on `sip::Sip128` (SipHash-1-3, 128-bit): content addressing for TypeId, crate
//!   fingerprints and cache keys.
//! - `type_hash`: 128-bit hash of a monomorphic type's canonical structure (def identities +
//!   args), independent of TyId numbering. `core::any::TypeId` is this value.
//! - `crate_fingerprints`: per crate, a hash of everything the front end read for it (each
//!   file's source plus its macro-expansion text, which holds include!/include_str!/env! results
//!   and derive / proc-macro output), its name, edition, cfg set, and its dependencies'
//!   fingerprints. Never timestamps.

use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use crate::program::{DefId, Program, NO_DEF};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};
use crate::sip::{hash_bytes, Sip128};

// ------------------------------------------------------------ definitions and types

/// A crate's identity independent of load order: its name and root file.
fn crate_ident(prog: &Program, krate: u32, h: &mut Sip128) {
    let c = &prog.crates[krate as usize];
    h.str(prog.syms.str(c.name));
    let root_file = prog.mods.get(c.root_mod as usize).map(|m| m.file);
    match root_file.and_then(|f| prog.file_paths.get(f as usize)) {
        Some(p) => h.str(p),
        None => h.str(""),
    }
}

/// A definition's identity: crate, def path, kind and, unless the path alone names it, its
/// parent's identity (an impl's or trait's item, a variant), an impl's header, or else the
/// defining file and the item's byte offset there (two same-named items -- local structs in
/// different blocks of one fn -- differ).
///
/// An item its module binds by name is named by its path: its identity must not move when a
/// live edit shifts the text before it (or the edit arrives as another file). `TypeId` is this
/// identity, folded into code as a constant: code built before an edit (a vtable's `type_id`)
/// and after it (a O3 rebuild inlining `TypeId::of::<T>()`) must agree, else
/// `downcast::<T>` fails on a value of type T.
pub fn def_ident(prog: &Program, d: DefId, h: &mut Sip128) {
    if d == NO_DEF {
        h.str("<no def>");
        return;
    }
    let def = prog.def(d);
    crate_ident(prog, def.krate, h);
    h.str(&prog.def_path(d));
    h.str(&format!("{:?}", def.kind));
    if path_names(prog, d) {
        return;
    }
    // an impl, or an item inside an impl / trait / enum, named through its parent
    if let Some(named) = parent_names(prog, d) {
        match named {
            ParentNamed::Impl(head) => { h.str("impl"); h.write(&head); }
            ParentNamed::Item(p) => { h.str("in"); def_ident(prog, p, h); }
        }
        return;
    }
    let file = def.file as usize;
    match prog.file_paths.get(file) {
        Some(p) => h.str(p),
        None => h.str(""),
    }
    let lo = prog.files.get(file).and_then(|f| f.ast.items.get(def.item.0 as usize)).map_or(u32::MAX, |it| it.lo);
    h.u32(lo);
}

/// Whether `d`'s path names it alone: a module item (not one in a fn body's block scope) that
/// its module binds under its name.
fn path_names(prog: &Program, d: DefId) -> bool {
    use crate::program::DefKind;
    let def = prog.def(d);
    let Some(m) = prog.mods.get(def.scope as usize) else { return false };
    if m.lexical_parent.is_some() || m.def != def.parent {
        return false;
    }
    let ns = match def.kind {
        DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::Trait | DefKind::TypeAlias => &m.types,
        DefKind::Fn | DefKind::Const | DefKind::Static => &m.values,
        _ => return false,
    };
    // (`const _` items share the path `_`: each keeps its offset)
    ns.get(&def.name) == Some(&d) && prog.syms.str(def.name) != "_"
}

enum ParentNamed {
    /// a module-level impl: its header text (`impl<T> Trait for Type<T>`), whitespace dropped,
    /// and its ordinal among the file's impls with that header (identical twins differ)
    Impl(Vec<u8>),
    /// an associated item or a variant: its parent's identity names it with its own path
    Item(DefId),
}

/// Whether `d` is named without its position: a module-level impl by its header, an item of
/// an impl, trait or enum so named by that parent (def_path does not tell impls apart).
fn parent_names(prog: &Program, d: DefId) -> Option<ParentNamed> {
    use crate::program::DefKind;
    let def = prog.def(d);
    match def.kind {
        DefKind::Impl => {
            let m = prog.mods.get(def.scope as usize)?;
            if m.lexical_parent.is_some() || m.def != def.parent { return None; }
            let (head, twin) = prog.files.get(def.file as usize)?.impl_head(def.item)?;
            let mut v = head.to_vec();
            v.extend_from_slice(&twin.to_le_bytes());
            Some(ParentNamed::Impl(v))
        }
        DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy | DefKind::Variant => {
            let p = def.parent;
            if p == NO_DEF { return None; }
            let named = path_names(prog, p) || matches!(parent_names(prog, p), Some(ParentNamed::Impl(_)));
            (named && matches!(prog.def(p).kind, DefKind::Impl | DefKind::Trait | DefKind::Enum)).then_some(ParentNamed::Item(p))
        }
        _ => None,
    }
}

/// Whether `d`'s identity (def_ident) carries its file offset: an item neither its path nor
/// its parent names (block-scoped items, `const _`). The code cache keys definitions the same
/// way, so a key and the TypeId folded into its code never disagree.
pub fn ident_has_offset(prog: &Program, d: DefId) -> bool {
    d != NO_DEF && !path_names(prog, d) && parent_names(prog, d).is_none()
}

fn type_into(prog: &Program, tcx: &Tcx, t: TyId, h: &mut Sip128, depth: u32) {
    if depth > 200 {
        // cyclic or absurdly deep: still deterministic
        h.u8(0xff);
        return;
    }
    let list = |v: &[TyId], h: &mut Sip128| {
        h.u32(v.len() as u32);
        for x in v {
            type_into(prog, tcx, *x, h, depth + 1);
        }
    };
    // Free regions were erased at the entry point. Anonymous bound regions are
    // semantic type identity in rustc and must survive hashing. No extra bytes
    // are written for A1's empty binders/erased regions (legacy hashes unchanged).
    use crate::types::{RegionArgsId, RegionId, RegionKind};
    let (rs, r, bound) = match tcx.tys.kind(t) {
        TyKind::Ref(_, _, r) => (RegionArgsId::EMPTY, *r, 0),
        TyKind::Adt(_, _, rs) | TyKind::FnDef(_, _, rs) | TyKind::Alias(_, AliasTy { regions: rs, .. }) => (*rs, RegionId::ERASED, 0),
        TyKind::Dyn(_, _, _, rs, r, bound, _) => (*rs, *r, *bound as u32),
        TyKind::FnPtr(_, _, _, n) => (RegionArgsId::EMPTY, RegionId::ERASED, *n),
        _ => (RegionArgsId::EMPTY, RegionId::ERASED, 0),
    };
    if rs != RegionArgsId::EMPTY || r != RegionId::ERASED || bound != 0 {
        let region = |r, h: &mut Sip128| match tcx.tys.region_kind(r) {
            RegionKind::ReErased => h.u8(0),
            RegionKind::ReBound(db, var) => { h.u8(1); h.u32(db); h.u32(var.0); }
            _ => unreachable!("free region in codegen type hash"),
        };
        h.u8(0xfe);
        h.u32(bound);
        region(r, h);
        let args = tcx.tys.region_args(rs);
        h.u32(args.len() as u32);
        for r in args.iter() { region(*r, h); }
    }
    match tcx.tys.kind(t) {
        TyKind::Bool => h.u8(1),
        TyKind::Char => h.u8(2),
        TyKind::Str => h.u8(3),
        TyKind::Int(p) => {
            h.u8(4);
            h.str(&format!("{:?}", p));
        }
        TyKind::Float(p) => {
            h.u8(5);
            h.str(&format!("{:?}", p));
        }
        TyKind::Never => h.u8(6),
        TyKind::Tuple(v) => {
            h.u8(7);
            list(tcx.tys.list(*v), h);
        }
        TyKind::Array(e, n) => {
            h.u8(8);
            type_into(prog, tcx, *e, h, depth + 1);
            const_into(prog, tcx, *n, h, depth + 1);
        }
        TyKind::Slice(e) => {
            h.u8(9);
            type_into(prog, tcx, *e, h, depth + 1);
        }
        TyKind::Ref(m, e, _) => {
            h.u8(10);
            h.u8(*m as u8);
            type_into(prog, tcx, *e, h, depth + 1);
        }
        TyKind::Ptr(m, e) => {
            h.u8(11);
            h.u8(*m as u8);
            type_into(prog, tcx, *e, h, depth + 1);
        }
        TyKind::Adt(d, args, _) => {
            h.u8(12);
            def_ident(prog, *d, h);
            list(tcx.tys.list(*args), h);
        }
        TyKind::FnPtr(ps, r, c, _) => {
            h.u8(13);
            list(tcx.tys.list(*ps), h);
            type_into(prog, tcx, *r, h, depth + 1);
            h.u8(c.safety as u8);
            h.u8(c.abi as u8);
            h.u8(c.c_variadic as u8);
        }
        TyKind::FnDef(d, args, _) => {
            h.u8(14);
            def_ident(prog, *d, h);
            list(tcx.tys.list(*args), h);
        }
        TyKind::Param(i) => {
            h.u8(15);
            h.u32(*i);
        }
        TyKind::IntVar(i) => { h.u8(25); h.u32(*i); }
        TyKind::FloatVar(i) => { h.u8(26); h.u32(*i); }
        TyKind::Infer(i) => {
            h.u8(16);
            h.u32(*i);
        }
        TyKind::Closure(file, expr, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: state }) => {
            h.u8(17);
            match prog.file_paths.get(*file as usize) {
                Some(p) => h.str(p),
                None => h.str(""),
            }
            h.u32(*expr);
            type_into(prog, tcx, *closure_kind_ty, h, depth + 1);
            type_into(prog, tcx, *sig, h, depth + 1);
            type_into(prog, tcx, *state, h, depth + 1);
            type_into(prog, tcx, *owner, h, depth + 1);
        }
        TyKind::Alias(rigid, alias) => {
            h.u8(18); h.u8(*rigid as u8);
            match alias.kind {
                AliasTyKind::Projection { def_id } => { h.u8(0); def_ident(prog, def_id, h); }
                AliasTyKind::Inherent { def_id } => { h.u8(1); def_ident(prog, def_id, h); }
                AliasTyKind::Opaque { def_id } => { h.u8(2); def_ident(prog, def_id, h); }
                AliasTyKind::Free { def_id } => { h.u8(3); def_ident(prog, def_id, h); }
            }
            list(tcx.tys.list(alias.args), h);
        }
        TyKind::Dyn(d, args, binds, _, _, _, autos) => {
            h.u8(19);
            def_ident(prog, *d, h);
            list(tcx.tys.list(*args), h);
            h.u32(tcx.tys.list(*binds).len() as u32);
            for (bd, bt, n) in tcx.tys.list(*binds) {
                if *n != 0 { h.u8(0xfe); h.u32(*n); }
                def_ident(prog, *bd, h);
                type_into(prog, tcx, *bt, h, depth + 1);
            }
            let mut auto_ids = Vec::new();
            for ad in tcx.tys.list(*autos) { let mut id = Sip128::new(); def_ident(prog, *ad, &mut id); auto_ids.push(id.finish128()); }
            auto_ids.sort();
            h.u32(auto_ids.len() as u32);
            for id in auto_ids { h.u128(id); }
        }
        TyKind::Const(v) => {
            h.u8(20);
            const_into(prog, tcx, *v, h, depth + 1);
        }
        TyKind::Foreign(d) => { h.u8(27); def_ident(prog, *d, h); }
        TyKind::Error => h.u8(22),
        TyKind::Bound(d, b) => {
            h.u8(23);
            h.u32(*d);
            h.u32(b.var.0);
            match b.kind {
                crate::types::BoundTyKind::Anon => h.u8(0),
                crate::types::BoundTyKind::Param(d) => { h.u8(1); def_ident(prog, d, h); }
            }
        }
        TyKind::Placeholder(p) => {
            h.u8(24);
            h.u32(p.universe.0);
            h.u32(p.bound.0);
        }
    }
}

/// 128-bit stable hash of a type (rustc's TypeId is a stable hash too). Equal types (hash-consed:
/// equal TyIds) give equal values; the value does not depend on TyId numbering.
pub fn type_hash(prog: &Program, tcx: &Tcx, t: TyId) -> u128 {
    let mut h = Sip128::new();
    h.str("type");
    type_into(prog, tcx, tcx.tys.erase_regions(t), &mut h, 0);
    h.finish128()
}

// ------------------------------------------------------------ crate fingerprints

/// Per crate (index = crate index): the crate fingerprint. Files are hashed in parallel; the
/// crate hash then folds in name, edition, cfg, its files' hashes in file order, and its
/// dependencies' fingerprints, regardless of load order.
pub fn crate_fingerprints(prog: &Program) -> Vec<u128> {
    let n = prog.files.len();
    let mut fh = vec![0u128; n];
    let threads = std::thread::available_parallelism().map_or(4, |x| x.get()).min(8).max(1);
    let chunk = (n + threads - 1) / threads.max(1);
    // Program is not Sync: the workers see only (path, source) pairs
    // only the file's own source: the expansion text after src_len grows lazily (bodies expand on
    // demand), so it depends on timing and on the app. Macro inputs from outside the source
    // (include*!, env!) are the late-input ledger below.
    let inputs: Vec<(&str, &[u8])> = (0..n)
        .map(|i| {
            let f = &prog.files[i];
            let len = (f.src_len as usize).min(f.src.len());
            (prog.file_paths.get(i).map_or("", |p| p.as_str()), &f.src[..len])
        })
        .collect();
    if chunk > 0 {
        crate::pool::scope(|s| {
            for (out, inp) in fh.chunks_mut(chunk).zip(inputs.chunks(chunk)) {
                s.spawn(move || {
                    for (o, (path, src)) in out.iter_mut().zip(inp) {
                        *o = crate::crate_image::file_hash(path, src);
                    }
                });
            }
        });
    }
    let nc = prog.crates.len();
    let mut per: Vec<Sip128> = (0..nc)
        .map(|c| {
            let ci = &prog.crates[c];
            let root = prog.mods.get(ci.root_mod as usize).and_then(|m| prog.file_paths.get(m.file as usize)).map_or("", String::as_str);
            crate::crate_image::fingerprint_head(prog.syms.str(ci.name), root, ci.edition, ci.proc_macro, &ci.cfg.set)
        })
        .collect();
    for (i, f) in fh.iter().enumerate() {
        if prog.front_macro_fragments.contains(&i) { continue; }
        if let Some(&c) = prog.file_crate.get(i) {
            if let Some(h) = per.get_mut(c as usize) {
                h.u128(*f);
            }
        }
    }
    for (h, c) in per.iter_mut().zip(&prog.crates) { h.u128(c.context.fingerprint()); }
    // Macro resolution depends on the crate and its dependency closure only (rustc_resolve
    // scopes since relax 8d070a4f), both already in the fingerprint.
    let mut out = vec![0u128; nc];
    let mut done = vec![false; nc];
    fn fp(c: usize, prog: &Program, per: &mut Vec<Sip128>, out: &mut Vec<u128>, done: &mut Vec<bool>, depth: u32) -> u128 {
        if done[c] || depth > 512 {
            return out[c];
        }
        let deps: Vec<u32> = prog.crates[c].deps.iter().map(|d| d.1).filter(|&d| d as usize != c).collect();
        // the dependency crates by name, each once (crate_image::canon_deps: a crate image's
        // identity is known before the load, without the extern crate aliases the sources add)
        let mut named: Vec<(String, u128)> = Vec::with_capacity(deps.len());
        for d in deps {
            let v = fp(d as usize, prog, per, out, done, depth + 1);
            named.push((prog.syms.str(prog.crates[d as usize].name).to_string(), v));
        }
        let named = crate::crate_image::canon_deps(named);
        let mut h = per[c].clone();
        h.u32(named.len() as u32);
        for v in named {
            h.u128(v);
        }
        out[c] = h.finish128();
        done[c] = true;
        out[c]
    }
    for c in 0..nc {
        fp(c, prog, &mut per, &mut out, &mut done, 0);
    }
    out
}

/// Fingerprint one completed crate once all its non-self dependencies have fingerprints. A
/// dependency may have a higher index or still be loading; return None until it is ready. Hash
/// only this crate's files, on a worker, so load-time swapping never rehashes earlier crates.
pub fn loaded_crate_fingerprint(prog: &Program, ci: usize, previous: &[Option<u128>]) -> Option<u128> {
    let c = prog.crates.get(ci)?;
    // Match crate_fingerprints: `extern crate self as name` is not a dependency.
    // Genuine dependency cycles still wait for finish() to hash the completed graph.
    // The dependencies as the preload saw them (the crate image's identity: the restore names
    // its store from these), each bound by its (extern alias, crate name) pair -- not every
    // extern crate the sources added while loading (brotli-decompressor's own
    // `extern crate alloc_no_stdlib as alloc` made its load fingerprint differ from the
    // restore's every run). Without preload records (no front images): the crate's deps.
    let pre = prog.front_dep_names.get(ci).filter(|d| !d.is_empty());
    let deps = match pre {
        Some(names) => names.iter().map(|(alias, name)| {
            let k = c.deps.iter().find(|&&(a, k)| prog.syms.str(a) == alias.as_str()
                && prog.crates.get(k as usize).is_some_and(|d| prog.syms.str(d.name) == name.as_str())).map(|&(_, k)| k)?;
            Some((name.clone(), previous.get(k as usize).copied().flatten()?))
        }).collect::<Option<Vec<_>>>()?,
        None => c.deps.iter().filter(|&&(_, k)| k as usize != ci).map(|&(_, k)| {
            let fp = previous.get(k as usize).copied().flatten()?;
            let dep = prog.crates.get(k as usize)?;
            Some((prog.syms.str(dep.name).to_string(), fp))
        }).collect::<Option<Vec<_>>>()?,
    };
    let deps = crate::crate_image::canon_deps(deps);
    let inputs: Vec<_> = prog.file_crate.iter().enumerate().filter(|(i, k)|
        **k as usize == ci && !prog.front_macro_fragments.contains(i)).map(|(i, _)| {
        let f = &prog.files[i];
        (prog.file_paths[i].as_str(), &f.src[..(f.src_len as usize).min(f.src.len())])
    }).collect();
    let name = prog.syms.str(c.name);
    Some(crate::pool::scope(|s| s.spawn(move || {
        crate::crate_image::fingerprint(name, c.root_path(), c.edition, c.proc_macro, &c.cfg.set,
            inputs.into_iter().map(|(p, b)| crate::crate_image::file_hash(p, b)), &deps, c.context.fingerprint())
    }).join().expect("front fingerprint worker")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SipHash-2-4-128 reference vectors (key 00..0f, messages 00, 00 01, ...), first entries.
    #[test]
    fn sip128_reference() {
        let k0 = u64::from_le_bytes([0, 1, 2, 3, 4, 5, 6, 7]);
        let k1 = u64::from_le_bytes([8, 9, 10, 11, 12, 13, 14, 15]);
        let want: [[u8; 16]; 2] = [
            [0xa3, 0x81, 0x7f, 0x04, 0xba, 0x25, 0xa8, 0xe6, 0x6d, 0xf6, 0x72, 0x14, 0xc7, 0x55, 0x02, 0x93],
            [0xda, 0x87, 0xc1, 0xd8, 0x6b, 0x99, 0xaf, 0x44, 0x34, 0x76, 0x59, 0x11, 0x9b, 0x22, 0xfc, 0x45],
        ];
        for (n, w) in want.iter().enumerate() {
            let msg: Vec<u8> = (0..n as u8).collect();
            let mut h = Sip128::new_with_rounds(k0, k1, 2, 4);
            h.write(&msg);
            assert_eq!(h.finish128().to_le_bytes(), *w, "vector {}", n);
        }
    }

    /// Streaming in pieces = one write.
    #[test]
    fn sip128_streaming() {
        let data: Vec<u8> = (0..200u32).map(|x| (x * 7 + 3) as u8).collect();
        let one = hash_bytes(&data);
        for cut in [0, 1, 3, 7, 8, 9, 15, 64, 199, 200] {
            let mut h = Sip128::new();
            h.write(&data[..cut]);
            h.write(&data[cut..]);
            assert_eq!(h.finish128(), one, "cut {}", cut);
        }
    }
}

// ------------------------------------------------------------ build id

/// The running binary's identity, from the linked bytes (nothing is computed from sources): macOS =
/// the page hashes in the linker's code signature, Linux = the linker's GNU build id (a content
/// hash), otherwise a hash of the executable file. Changes with any change to the
/// compiler, codegen or rustc flags; never a timestamp.
pub fn build_id() -> u128 {
    static ID: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            if let Some(cd) = macho_code_directory() {
                let mut h = Sip128::new();
                h.str("build-codedir");
                h.write(cd);
                return h.finish128();
            }
        }
        // the linker's build id (content-derived): reading and hashing the whole executable
        // (~36 MB) took ~15 ms of every launch
        #[cfg(target_os = "linux")]
        {
            if let Some(id) = elf_build_id() {
                let mut h = Sip128::new();
                h.str("build-id");
                h.write(id);
                return h.finish128();
            }
        }
        #[cfg(target_os = "android")]
        let exe = android_runtime_image();
        #[cfg(not(target_os = "android"))]
        let exe = std::env::current_exe().ok().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        let mut h = Sip128::new();
        h.str("build-exe");
        h.write(&exe);
        h.finish128()
    })
}

/// In the Activity, current_exe is app_process64: hash Relax's loaded image instead.
#[cfg(target_os = "android")]
fn android_runtime_image() -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    // NDK r28 dlfcn.h: Dl_info consists of name/base/name/address pointers.
    #[repr(C)]
    struct DlInfo {
        fname: *const std::ffi::c_char,
        base: *mut std::ffi::c_void,
        sname: *const std::ffi::c_char,
        saddr: *mut std::ffi::c_void,
    }
    extern "C" {
        fn dladdr(addr: *const std::ffi::c_void, info: *mut DlInfo) -> i32;
    }
    unsafe {
        let mut info: DlInfo = std::mem::zeroed();
        if dladdr(build_id as *const () as *const std::ffi::c_void, &mut info) != 0 && !info.fname.is_null() {
            let path = std::ffi::OsStr::from_bytes(std::ffi::CStr::from_ptr(info.fname).to_bytes());
            if let Ok(bytes) = std::fs::read(path) {
                return bytes;
            }
        }
    }
    panic!("cannot read makepad run's Android runtime image for its build identity");
}

/// The linker's code signature CodeDirectory of the running executable (arm64 macOS: ld always
/// writes one). It holds a SHA-256 of every page of the final file (code, data, symbol tables,
/// everything), so it changes with any byte; read from the mapped image, no file I/O. (LC_UUID is
/// not enough: it does not cover the symbol table or other link-edit data.)
#[cfg(target_os = "macos")]
fn macho_code_directory() -> Option<&'static [u8]> {
    extern "C" {
        static _mh_execute_header: u8;
    }
    unsafe {
        let base = &_mh_execute_header as *const u8;
        let rd = |off: usize| -> u32 { std::ptr::read_unaligned(base.add(off) as *const u32) };
        let rd64 = |off: usize| -> u64 { std::ptr::read_unaligned(base.add(off) as *const u64) };
        if rd(0) != 0xfeed_facf {
            return None;
        }
        let ncmds = rd(16) as usize;
        let mut off = 32;
        let (mut text_vm, mut link_vm, mut link_fileoff) = (None, 0u64, 0u64);
        let mut sig = None;
        for _ in 0..ncmds {
            let cmd = rd(off);
            let size = rd(off + 4) as usize;
            if size == 0 {
                return None;
            }
            if cmd == 0x19 {
                // LC_SEGMENT_64: segname[16] vmaddr vmsize fileoff filesize
                let name = std::slice::from_raw_parts(base.add(off + 8), 16);
                if name.starts_with(b"__TEXT\0") {
                    text_vm = Some(rd64(off + 24));
                } else if name.starts_with(b"__LINKEDIT\0") {
                    link_vm = rd64(off + 24);
                    link_fileoff = rd64(off + 40);
                }
            } else if cmd == 0x1d {
                // LC_CODE_SIGNATURE: dataoff datasize
                sig = Some((rd(off + 8) as u64, rd(off + 12) as usize));
            }
            off += size;
        }
        let (dataoff, datasize) = sig?;
        let slide = (base as u64).wrapping_sub(text_vm?);
        let blob = std::slice::from_raw_parts(
            slide.wrapping_add(link_vm).wrapping_add(dataoff.wrapping_sub(link_fileoff)) as *const u8,
            datasize,
        );
        // SuperBlob (big endian): magic 0xfade0cc0, length, count, then (type, offset) per blob
        let be = |o: usize| -> Option<usize> { Some(u32::from_be_bytes(blob.get(o..o + 4)?.try_into().ok()?) as usize) };
        if be(0)? != 0xfade_0cc0 {
            return None;
        }
        for i in 0..be(8)? {
            let (ty, boff) = (be(12 + i * 8)?, be(16 + i * 8)?);
            if ty == 0 && be(boff)? == 0xfade_0c02 {
                return blob.get(boff..boff + be(boff + 4)?);
            }
        }
        None
    }
}

/// The executable's NT_GNU_BUILD_ID note, read from its loaded program headers.
#[cfg(target_os = "linux")]
fn elf_build_id() -> Option<&'static [u8]> {
    extern "C" {
        fn getauxval(t: std::ffi::c_ulong) -> std::ffi::c_ulong;
    }
    const AT_PHDR: std::ffi::c_ulong = 3;
    const AT_PHNUM: std::ffi::c_ulong = 5;
    const PT_NOTE: u32 = 4;
    const PT_PHDR: u32 = 6;
    const NT_GNU_BUILD_ID: u32 = 3;
    #[repr(C)]
    struct Phdr { p_type: u32, p_flags: u32, p_offset: u64, p_vaddr: u64, p_paddr: u64, p_filesz: u64, p_memsz: u64, p_align: u64 }
    unsafe {
        let at = getauxval(AT_PHDR) as usize;
        let n = getauxval(AT_PHNUM) as usize;
        if at == 0 || n == 0 || cfg!(not(target_pointer_width = "64")) {
            return None;
        }
        let ph = std::slice::from_raw_parts(at as *const Phdr, n);
        // the load bias: where the program headers are minus where the file says they are
        let bias = at.wrapping_sub(ph.iter().find(|p| p.p_type == PT_PHDR)?.p_vaddr as usize);
        for p in ph.iter().filter(|p| p.p_type == PT_NOTE) {
            let notes = std::slice::from_raw_parts(bias.wrapping_add(p.p_vaddr as usize) as *const u8, p.p_filesz as usize);
            let mut i = 0;
            while i + 12 <= notes.len() {
                let rd = |o: usize| u32::from_ne_bytes([notes[o], notes[o + 1], notes[o + 2], notes[o + 3]]) as usize;
                let (namesz, descsz, ty) = (rd(i), rd(i + 4), rd(i + 8) as u32);
                let name = i + 12;
                let desc = name + ((namesz + 3) & !3);
                let end = desc + ((descsz + 3) & !3);
                if desc + descsz > notes.len() {
                    break;
                }
                if ty == NT_GNU_BUILD_ID && &notes[name..name + namesz] == b"GNU\0" && descsz >= 8 {
                    return Some(&notes[desc..desc + descsz]);
                }
                i = end;
            }
        }
        None
    }
}

/// MAKEPAD_RUN_CACHE_FP=1: prints the build id, every crate's fingerprint and the cost (C0 check:
/// identical across runs of the same sources; must stay a few ms).
pub fn report_fingerprints(prog: &Program) {
    if !prog.session.options.verbose { return; }
    let t = std::time::Instant::now();
    let bid = build_id();
    let tb = t.elapsed();
    let t = std::time::Instant::now();
    let fps = crate_fingerprints(prog);
    let tf = t.elapsed();
    let bytes: usize = prog.files.iter().map(|f| f.src.len()).sum();
    eprintln!(
        "makepad-run cache: build id {:032x} ({:.2} ms); {} crates, {} files, {:.1} MB hashed in {:.2} ms",
        bid,
        tb.as_secs_f64() * 1e3,
        fps.len(),
        prog.files.len(),
        bytes as f64 / 1e6,
        tf.as_secs_f64() * 1e3
    );
    let mode = std::env::var("MAKEPAD_RUN_CACHE_FP").unwrap_or_default();
    if let Ok(want) = std::env::var("MAKEPAD_RUN_CACHE_FP_CRATE") {
        // diagnostics: per-file hashes of one crate (why does a fingerprint differ?)
        for (i, f) in prog.files.iter().enumerate() {
            let c = prog.file_crate.get(i).copied().unwrap_or(u32::MAX) as usize;
            if prog.crates.get(c).map_or(false, |ci| prog.syms.str(ci.name) == want) {
                eprintln!("makepad-run cache: file {} {:032x} src_len {} len {} {}", i, hash_bytes(&f.src), f.src_len, f.src.len(), prog.file_paths.get(i).map_or("", |p| p.as_str()));
            }
        }
    }
    if mode == "all" || mode == "features" {
        for (c, fp) in fps.iter().enumerate() {
            let ci = &prog.crates[c];
            if mode == "features" {
                let mut f: Vec<&str> = ci.cfg.set.iter().map(|x| x.as_str()).collect();
                f.sort();
                let deps: Vec<String> = ci.deps.iter().map(|(n, d)| format!("{}={}", prog.syms.str(*n), prog.syms.str(prog.crates[*d as usize].name))).collect();
                eprintln!("makepad-run cache: crate {:032x} {} [{}] deps [{}]", fp, prog.syms.str(ci.name), f.join(" "), deps.join(" "));
            } else {
                eprintln!("makepad-run cache: crate {:032x} {}", fp, prog.syms.str(ci.name));
            }
        }
    }
}


// rustc_type_ir/src/const_kind.rs:21: hashes contain structure, never arena ids.
fn const_into(prog: &Program, tcx: &Tcx, ct: crate::consts::Const, h: &mut Sip128, depth: u32) {
    use crate::consts::{ConstKind, GenericArg, AliasConstKind, ValTreeKind};
    if depth > 200 { h.u8(0xff); return; }
    match tcx.tys.const_kind(ct) {
        ConstKind::Param(p) => { h.u8(0); h.u32(p.index); }
        ConstKind::Bound(d, b) => { h.u8(1); h.u32(d); h.u32(b.var.0); }
        ConstKind::Placeholder(p) => { h.u8(2); h.u32(p.universe.0); h.u32(p.bound.var.0); }
        ConstKind::Alias(rigid, alias) => {
            h.u8(3); h.u8(rigid as u8);
            h.u8(match alias.kind { AliasConstKind::Projection { .. } => 0, AliasConstKind::InherentSelf { .. } => 1,
                AliasConstKind::InherentImpl { .. } => 2, AliasConstKind::Free { .. } => 3, AliasConstKind::Anon { .. } => 4 });
            def_ident(prog, alias.kind.def_id(), h);
            h.u32(alias.args.len() as u32);
            for arg in &*alias.args {
                match *arg {
                    GenericArg::Type(ty) => { h.u8(0); type_into(prog, tcx, ty, h, depth + 1); }
                    GenericArg::Const(ct) => { h.u8(1); const_into(prog, tcx, ct, h, depth + 1); }
                    GenericArg::Lifetime(region) => {
                        h.u8(2);
                        match tcx.tys.region_kind(region) {
                            crate::types::RegionKind::ReErased => h.u8(0),
                            crate::types::RegionKind::ReBound(d, b) => { h.u8(1); h.u32(d); h.u32(b.0); }
                            _ => panic!("free region in codegen constant hash"),
                        }
                    }
                }
            }
        }
        ConstKind::Value(value) => {
            h.u8(4); type_into(prog, tcx, value.ty, h, depth + 1);
            match value.valtree.kind() {
                ValTreeKind::Leaf(scalar) => { h.u8(0); h.u8(scalar.size()); let bits = scalar.to_bits(scalar.size()); h.u64(bits as u64); h.u64((bits >> 64) as u64); }
                ValTreeKind::Branch(fields) => {
                    h.u8(1); h.u32(fields.len() as u32);
                    for &ct in &**fields { const_into(prog, tcx, ct, h, depth + 1); }
                }
            }
        }
        ConstKind::Error => h.u8(5),
        ConstKind::Infer(_) => panic!("unresolved const in stable hash"),
    }
}
