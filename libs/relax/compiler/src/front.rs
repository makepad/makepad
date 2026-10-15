//! A file's front-end arrays as an image (plans/memory-use.md, "Image design"): written once,
//! when the file is final (Program::map_image at its publication; a lazily parsed part before
//! ParsedFile::part publishes it), into the run's image store (cache/front_run.rs), and from
//! then on read through the mapping -- clean file pages instead of heap. Warm runs map the same
//! images (crate stores refer to them) and read them through the same functions (restore,
//! apply_part). MAKEPAD_RUN_FRONT=0 disables front images; MAKEPAD_RUN_FRONT_VERIFY=1 compares every
//! section with the parse it replaces.

use crate::arena::Arena;
use crate::image::{Reader, Writer};
use crate::packed::PackedToks;
use crate::parser::ParsedFile;
use crate::source::SourceRegistry;
use crate::origin_table::{Table, Regions, Maps, Origins};
use std::sync::atomic::{AtomicU64, Ordering};

const S_SRC: u32 = 1;
const S_TOKS: u32 = 2;
const S_EXP_MAP: u32 = 3;
const S_TOK_ORIGIN: u32 = 4;
const S_RULES_EXP: u32 = 5;
const S_BODY_ORIGIN: u32 = 6;
const S_TOKEN_ORIGINS: u32 = 7;
const S_SOURCES: u32 = 8;
/// tree arrays: 100 + index in AST_ARRAYS order
const S_AST: u32 = 100;

/// The tree's arrays (every Ast arena), in section order.
macro_rules! ast_arrays {
    ($m:ident) => {
        $m!(exprs: Expr, pats: Pat, tys: Ty, blocks: Block, root_attrs: Attr, root_items: ItemId, mac_args: MacArgs, path_segs: PathSeg, stmts: Stmt, expr_lists: ExprId, pat_lists: PatId, ty_lists: TyId, field_exprs: FieldExpr, arms: Arm, closure_params: ClosureParam, field_pats: FieldPat, qselfs: QSelf, generic_args: GenericArgs, fn_ptrs: FnPtr, generic_arg_lists: GenericArg, bounds: Bound, generic_params: GenericParam, ident_lists: Ident, attrs: Attr, asm_ops: AsmOp, items: Item, fn_sigs: FnSig, params: Param, field_defs: FieldDef, variants: Variant, use_trees: UseTree, item_lists: ItemId, where_preds: WherePred, method_calls: MethodCallRow, for_exprs: ForRow, closure_exprs: ClosureRow, struct_exprs: StructRow, mac_calls: MacCall, while_exprs: WhileRow, let_stmts: LetStmt, attrs_stmts: AttrsStmt, use_items: UseItem, struct_items: StructItem, union_items: UnionItem, enum_items: EnumItem, alias_items: TypeAliasItem, trait_items: TraitItem, trait_alias_items: TraitAliasItem, impl_items: ImplItem, foreign_mods: ForeignModItem, vis_paths: Path)
    };
}

pub static MAPPED_FILES: AtomicU64 = AtomicU64::new(0);
pub static MAPPED_BYTES: AtomicU64 = AtomicU64::new(0);
pub static RECORDED_FILES: AtomicU64 = AtomicU64::new(0);
pub static RECORDED_BYTES: AtomicU64 = AtomicU64::new(0);
/// images found but not used (another parse result: a bug or a stale store)
pub static MISMATCHED: AtomicU64 = AtomicU64::new(0);

fn mismatch(path: &str, section: &str) -> bool {
    MISMATCHED.fetch_add(1, Ordering::Relaxed);
    if crate::preload::trace() {
        eprintln!("makepad-run front mismatch: {} {}", path, section);
    }
    false
}

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_FRONT").map_or(true, |v| v != "0"))
}

pub fn verify() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_FRONT_VERIFY").map_or(false, |v| v == "1"))
}

/// The image of a loaded file (its arrays as they are after the load; the lazily parsed part
/// is not in it).
pub fn encode(pf: &ParsedFile) -> Vec<u8> {
    image(pf).finish()
}

pub fn image(pf: &ParsedFile) -> Writer<'_> {
    let mut w = Writer::default();
    w.add(S_SRC, &pf.src);
    w.add(S_TOKS, pf.toks.words());
    w.add(S_EXP_MAP, pf.exp_map.bytes());
    w.add(S_TOK_ORIGIN, pf.tok_origin.bytes());
    w.add(S_RULES_EXP, pf.rules_exp.bytes());
    w.add(S_BODY_ORIGIN, pf.body_origin.bytes());
    w.add(S_TOKEN_ORIGINS, pf.token_origins.bytes());
    w.add_chunks(S_SOURCES, source_chunks(pf, &pf.token_origins));
    let a = &pf.ast;
    let mut k = 0u32;
    macro_rules! put {
        ($($f:ident: $t:ident),*) => {
            $( w.add(S_AST + k, &a.$f); k += 1; )*
        };
    }
    ast_arrays!(put);
    let _ = k;
    w
}

/// Heap capacity retired by the main image (parts are accounted separately).
pub fn heap_bytes(pf: &ParsedFile) -> usize {
    let mut n = pf.src.heap_bytes() + pf.toks.heap_bytes() + pf.exp_map.heap_bytes()
        + pf.tok_origin.heap_bytes() + pf.token_origins.heap_bytes() + pf.rules_exp.heap_bytes() + pf.body_origin.heap_bytes();
    macro_rules! count { ($($f:ident: $t:ident),*) => { $( n += pf.ast.$f.heap_bytes(); )* }; }
    ast_arrays!(count);
    n
}

/// Makes `pf`'s arrays slices of its image when the image holds the same parse; false
/// otherwise (`pf` unchanged).
pub fn apply(path: &str, pf: &mut ParsedFile, img: &'static [u8]) -> bool {
    use crate::ast::*;
    let Some(r) = Reader::new(img) else { return false };
    macro_rules! read {
        ($ty:ty, $index:expr, $id:expr, $section:literal) => {
            match r.slice::<$ty>($index, $id) {
                Some(x) => x,
                None => return mismatch(path, $section),
            }
        };
    }
    let src = read!(u8, 0, S_SRC, "src");
    let Some(toks) = PackedToks::mapped(read!(u32, 1, S_TOKS, "toks")) else { return mismatch(path, "toks") };
    let Some(em) = Table::<Regions>::mapped(read!(u8, 2, S_EXP_MAP, "exp_map")) else { return mismatch(path, "exp_map") };
    let Some(to) = Table::<Maps>::mapped(read!(u8, 3, S_TOK_ORIGIN, "tok_origin")) else { return mismatch(path, "tok_origin") };
    let Some(re) = Table::<Regions>::mapped(read!(u8, 4, S_RULES_EXP, "rules_exp")) else { return mismatch(path, "rules_exp") };
    let Some(bo) = Table::<Maps>::mapped(read!(u8, 5, S_BODY_ORIGIN, "body_origin")) else { return mismatch(path, "body_origin") };
    let origins = read!(u8, 6, S_TOKEN_ORIGINS, "token_origins");
    let sources = read!(u8, 7, S_SOURCES, "sources");
    let Some(origins) = restore_origins(origins, sources, &mut pf.sources, false) else { return mismatch(path, "sources"); };
    // The key identifies the original source. Expansion text must also match; verify mode
    // additionally compares array contents instead of only their lengths.
    if src != &pf.src[..] { return mismatch(path, "src"); }
    let v = verify();
    macro_rules! check {
        ($mapped:expr, $parsed:expr, $same:expr, $section:literal) => {
            if $mapped.len() != $parsed.len() { return mismatch(path, concat!($section, ".len")); }
            if v && !$same { return mismatch(path, $section); }
        };
    }
    check!(toks, pf.toks, toks.words() == pf.toks.words(), "toks");
    check!(em, pf.exp_map, em == pf.exp_map, "exp_map");
    check!(to, pf.tok_origin, to == pf.tok_origin, "tok_origin");
    check!(re, pf.rules_exp, re == pf.rules_exp, "rules_exp");
    check!(bo, pf.body_origin, bo == pf.body_origin, "body_origin");
    if origins != pf.token_origins { return mismatch(path, "token_origins"); }
    // the tree's arrays: same lengths (and, verifying, the same nodes)
    let a = &pf.ast;
    let mut k = 0usize;
    macro_rules! get {
        ($($f:ident: $t:ident),*) => {
            ($( {
                let Some(x) = r.slice::<$t>(8 + k, S_AST + k as u32) else { return mismatch(path, concat!("ast.", stringify!($f))); };
                if x.len() != a.$f.len() { return mismatch(path, concat!("ast.", stringify!($f), ".len")); }
                if v && format!("{:?}", x) != format!("{:?}", &a.$f[..]) { return mismatch(path, concat!("ast.", stringify!($f))); }
                k += 1;
                x
            }, )*)
        };
    }
    let got = ast_arrays!(get);
    let _ = k;
    let mut before = pf.src.heap_bytes() + pf.toks.heap_bytes() + pf.exp_map.heap_bytes() + pf.tok_origin.heap_bytes() + pf.token_origins.heap_bytes() + pf.rules_exp.heap_bytes() + pf.body_origin.heap_bytes();
    pf.src = Arena::Mapped(src);
    pf.toks = toks;
    pf.exp_map = em;
    pf.tok_origin = to;
    pf.rules_exp = re;
    pf.body_origin = bo;
    pf.token_origins = origins;
    let a = &mut pf.ast;
    macro_rules! set {
        ($($f:ident: $t:ident),*) => {
            let ($($f,)*) = got;
            $( before += a.$f.heap_bytes(); a.$f = Arena::Mapped($f); )*
        };
    }
    ast_arrays!(set);
    MAPPED_FILES.fetch_add(1, Ordering::Relaxed);
    MAPPED_BYTES.fetch_add(before as u64, Ordering::Relaxed);
    true
}

/// part image sections
const P_EXT: u32 = 200;
const P_TOKS: u32 = 201;
const P_EXP_MAP: u32 = 202;
const P_TOK_ORIGIN: u32 = 203;
/// (item, block, expr lo, expr hi, pat lo, pat hi) per skipped body
const P_BODIES: u32 = 204;
const P_BASE: u32 = 205;
const P_TOKEN_ORIGINS: u32 = 206;
const P_SOURCES: u32 = 207;
const P_RULES_EXP: u32 = 208;
const P_BODY_ORIGIN: u32 = 209;
const P_AST: u32 = 300;

pub static PARTS_MAPPED: AtomicU64 = AtomicU64::new(0);
pub static PARTS_RECORDED: AtomicU64 = AtomicU64::new(0);

/// The image of a file's lazily parsed bodies (tests: the published part).
pub fn encode_part(pf: &ParsedFile) -> Option<Vec<u8>> {
    Some(part_image(pf, pf.part.get()?, pf.ast.part.get()?)?.finish())
}

/// Whether a part gets an image: one segment whose bodies all parsed and made no item (a
/// part with a parse error stays the parser's).
fn part_qualifies(fp: &crate::parser::FilePart, a: &crate::ast::Ast) -> bool {
    !fp.bodies.is_empty() && fp.next.get().is_none() && fp.bodies.iter().all(|(_, r)| r.is_ok()) && a.items.is_empty()
}

/// The image of part `fp` (with its AST part `a`) of `pf`, if it qualifies.
pub fn part_image<'a>(pf: &'a ParsedFile, fp: &'a crate::parser::FilePart, a: &'a crate::ast::Ast) -> Option<Writer<'a>> {
    if !part_qualifies(fp, a) { return None; }
    let mut bodies: Vec<u32> = Vec::new();
    for (item, r) in fp.bodies.iter() {
        let (b, rg) = r.ok()?;
        bodies.extend_from_slice(&[item, b.0, rg[0], rg[1], rg[2], rg[3]]);
    }
    let mut w = Writer::default();
    w.add(P_EXT, &fp.ext);
    w.add(P_TOKS, fp.toks.words());
    w.add(P_EXP_MAP, fp.exp_map.bytes());
    w.add(P_TOK_ORIGIN, fp.tok_origin.bytes());
    w.add_owned(P_BODIES, &bodies);
    w.add(P_BASE, &a.base[..]);
    w.add(P_TOKEN_ORIGINS, fp.token_origins.bytes());
    w.add_chunks(P_SOURCES, source_chunks(pf, &fp.token_origins));
    let mut k = 0u32;
    macro_rules! put {
        ($($f:ident: $t:ident),*) => {
            $( w.add(P_AST + k, &a.$f); k += 1; )*
        };
    }
    ast_arrays!(put);
    let _ = k;
    // its macro_rules expansions (hygiene), after the tree
    w.add(P_RULES_EXP, fp.rules_exp.bytes());
    w.add(P_BODY_ORIGIN, fp.body_origin.bytes());
    Some(w)
}

/// A part read from its image, continuing a file whose arrays end at `ends` (AST ids) and
/// `src`/`toks`/`rules` (positions); `sources`: the file's registry (restore_origins).
fn part_from_image(path: &str, ends: [u32; crate::ast::ARENAS], (src, toks, rules): (usize, usize, usize), img: &'static [u8], sources: &mut Option<std::sync::Arc<SourceRegistry>>, certified: bool) -> Option<(crate::parser::FilePart, crate::ast::Ast)> {
    use crate::ast::*;
    let Some(r) = Reader::new(img) else { return None };
    let (Some(ext), Some(ptoks), Some(em), Some(to), Some(bodies), Some(base), Some(origins), Some(srcs)) = (
        r.slice::<u8>(0, P_EXT),
        r.slice::<u32>(1, P_TOKS).and_then(PackedToks::mapped),
        r.slice::<u8>(2, P_EXP_MAP).and_then(Table::<Regions>::mapped),
        r.slice::<u8>(3, P_TOK_ORIGIN).and_then(Table::<Maps>::mapped),
        r.slice::<u32>(4, P_BODIES),
        r.slice::<u32>(5, P_BASE),
        r.slice::<u8>(6, P_TOKEN_ORIGINS),
        r.slice::<u8>(7, P_SOURCES),
    ) else {
        mismatch(path, "part.sections");
        return None;
    };
    let Some(origins) = restore_origins(origins, srcs, sources, certified) else { mismatch(path, "part.sources"); return None; };
    // the part continues the file's ids and positions: its base must be the file's ends
    if base != &ends[..] { mismatch(path, "part.base"); return None; }
    if bodies.len() % 6 != 0 { mismatch(path, "part.bodies"); return None; }
    let mut a = Ast::default();
    a.base.copy_from_slice(base);
    let mut k = 0usize;
    macro_rules! get {
        ($($f:ident: $t:ident),*) => {
            $( match r.slice::<$t>(8 + k, P_AST + k as u32) { Some(x) => a.$f = Arena::Mapped(x), None => { mismatch(path, concat!("part.ast.", stringify!($f))); return None; } } k += 1; )*
        };
    }
    ast_arrays!(get);
    let (Some(rules_exp), Some(body_origin)) = (r.slice::<u8>(8 + k, P_RULES_EXP).and_then(Table::<Regions>::mapped), r.slice::<u8>(9 + k, P_BODY_ORIGIN).and_then(Table::<Maps>::mapped)) else {
        mismatch(path, "part.rules");
        return None;
    };
    let fp = crate::parser::FilePart {
        src_base: src as u32, tok_base: toks as u32, rules_base: rules as u32,
        rules_exp, body_origin, next: Default::default(),
        ext: Arena::Mapped(ext),
        toks: ptoks,
        exp_map: em,
        tok_origin: to,
        token_origins: origins,
        bodies: crate::parser::PartBodies::Mapped(bodies),
        image: None,
    };
    Some((fp, a))
}

/// Whether part `a` (with its AST `aa`) holds the same parse as `b` (`ba`): every array's
/// length, and verifying, every array's contents.
fn same_part(a: &crate::parser::FilePart, aa: &crate::ast::Ast, b: &crate::parser::FilePart, ba: &crate::ast::Ast) -> bool {
    let v = verify();
    let mut s = aa.base == ba.base && a.bodies.iter().eq(b.bodies.iter()) && a.ext.len() == b.ext.len() && a.toks.len() == b.toks.len()
        && a.exp_map.len() == b.exp_map.len() && a.tok_origin.len() == b.tok_origin.len() && a.token_origins.len() == b.token_origins.len()
        && a.rules_exp.len() == b.rules_exp.len() && a.body_origin.len() == b.body_origin.len();
    if v {
        s = s && a.ext[..] == b.ext[..] && a.toks.words() == b.toks.words() && a.exp_map == b.exp_map && a.tok_origin == b.tok_origin && a.token_origins == b.token_origins
            && a.rules_exp == b.rules_exp && a.body_origin == b.body_origin;
    }
    macro_rules! cmp {
        ($($f:ident: $t:ident),*) => {
            $( s = s && aa.$f.len() == ba.$f.len() && (!v || format!("{:?}", &aa.$f[..]) == format!("{:?}", &ba.$f[..])); )*
        };
    }
    ast_arrays!(cmp);
    s
}

/// A part this run parsed, before its publication (ParsedFile::part): its arrays become slices
/// of its image when the image holds the same parse.
pub fn map_part(path: &str, pf: &ParsedFile, fp: &mut crate::parser::FilePart, a: &mut crate::ast::Ast, img: &'static [u8]) -> bool {
    let mut sources = pf.sources.clone();
    let Some((mfp, ma)) = part_from_image(path, *a.base, (fp.src_base as usize, fp.tok_base as usize, fp.rules_base as usize), img, &mut sources, false) else { return false };
    // the part's source registry is its file's: an image must not need another one
    let same_registry = match (&sources, &pf.sources) {
        (Some(x), Some(y)) => std::sync::Arc::ptr_eq(x, y),
        (None, None) => true,
        _ => false,
    };
    if !same_registry || !same_part(&mfp, &ma, fp, a) {
        return mismatch(path, "part.verify");
    }
    *fp = mfp;
    *a = ma;
    PARTS_MAPPED.fetch_add(1, Ordering::Relaxed);
    true
}

/// Warm restore before anything asked for bodies. Verify mode compares a fresh parse.
/// `certified`: the image belongs to a certified collected graph (restore_origins).
pub fn apply_part(path: &str, pf: &mut ParsedFile, img: &'static [u8], cfg: &crate::cfg::CfgSet, certified: bool) -> bool {
    if pf.part.get().is_some() || pf.ast.part.get().is_some() || pf.lazy.is_none() { return false; }
    let base = (pf.src.len(), pf.toks.len(), pf.rules_exp.len());
    let ends = pf.ast.ends();
    let mut sources = pf.sources.take();
    let got = part_from_image(path, ends, base, img, &mut sources, certified);
    pf.sources = sources;
    let Some((fp, a)) = got else { return false };
    if verify() {
        // Verify reparses as a warm run would.
        let (fresh, fresh_ast) = crate::parser::parse_part(pf, cfg);
        if !same_part(&fp, &a, &fresh, &fresh_ast) {
            return mismatch(path, "part.verify");
        }
    }
    let _ = pf.ast.part.set(Box::new(a));
    let _ = pf.part.set(Box::new(fp));
    PARTS_MAPPED.fetch_add(1, Ordering::Relaxed);
    true
}

/// Store source identities and immutable snapshots next to the arena. Numeric IDs in an
/// image are local to this manifest; restore never treats them as IDs in another load.
fn source_chunks(pf: &ParsedFile, origins: &Table<Origins>) -> Vec<crate::image::Chunk<'static>> {
    use crate::image::Chunk;
    use crate::crate_image::Codec;
    let mut ids: Vec<_> = origins.iter().map(|o| o.source.file).filter(|id| *id != 0).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut head = Vec::new();
    ids.len().put(&mut head);
    let mut chunks = vec![Chunk::Owned(head)];
    for id in ids {
        let src = pf.sources.as_ref().expect("origin source registry").get(id).expect("origin source snapshot");
        let mut head = Vec::new();
        id.put(&mut head);
        src.path.put(&mut head);
        src.src.len().put(&mut head);
        chunks.push(Chunk::Owned(head));
        chunks.push(match &src.src {
            crate::source::SourceBytes::Shared(bytes) => Chunk::Shared(bytes.clone()),
            crate::source::SourceBytes::Mapped(bytes) => Chunk::Mapped(bytes),
        });
    }
    chunks
}

fn restore_origins(origins: &'static [u8], bytes: &'static [u8], sources: &mut Option<std::sync::Arc<SourceRegistry>>, certified: bool) -> Option<Table<Origins>> {
    use crate::crate_image::{Codec, Decode};
    let origins = Table::<Origins>::mapped(origins)?;
    let mut r = Decode::new(bytes);
    let count = usize::get(&mut r)?;
    if count > r.bytes.len() { return None; }
    let mut snapshots = Vec::with_capacity(count);
    for _ in 0..count {
        let old = u32::get(&mut r)?;
        let path = String::get(&mut r)?;
        let len = usize::get(&mut r)?;
        snapshots.push((old, path, r.take(len)?));
    }
    if !r.bytes.is_empty() { return None; }
    if certified {
        // The completed collected graph references this exact immutable file image.
        // Its origin array is trusted like the AST arrays, under the store checksum;
        // preserving source IDs makes a full origin scan/remap unnecessary at boot.
        match sources {
            None => {
                if !snapshots.is_empty() { *sources = Some(std::sync::Arc::new(SourceRegistry::from_mapped(snapshots)?)); }
                return Some(origins);
            }
            // a lazily parsed part continues its file's registry: same IDs, nothing to remap
            Some(reg) if snapshots.iter().all(|(old, path, src)| reg.adopt_mapped(*old, path, src)) => return Some(origins),
            Some(_) => {}
        }
    }
    let mut ids = std::collections::HashMap::new();
    for (old, path, src) in snapshots {
        if old == 0 || ids.contains_key(&old) { return None; }
        let len = u32::try_from(src.len()).ok()?;
        let id = sources.get_or_insert_with(Default::default).register_mapped(&path, src).ok()?;
        ids.insert(old, (id, len));
    }
    let mut same = true;
    let mut end = 0;
    for o in origins.iter() {
        if o.lo < end || o.hi < o.lo || o.source.hi < o.source.lo { return None; }
        end = o.hi;
        if o.source.file != 0 {
            let &(id, len) = ids.get(&o.source.file)?;
            if o.source.hi > len { return None; }
            same &= id == o.source.file;
        }
    }
    if same { return Some(origins); }
    Some(origins.remap_files(|f| ids[&f].0))
}

fn same_origins(a: &ParsedFile, ao: &Table<Origins>, b: &ParsedFile, bo: &Table<Origins>) -> bool {
    ao.len() == bo.len() && ao.iter().zip(bo.iter()).all(|(x, y)| {
        if (x.lo, x.hi, x.source.lo, x.source.hi) != (y.lo, y.hi, y.source.lo, y.source.hi) { return false; }
        match (x.source.file, y.source.file) {
            (0, 0) => true,
            (0, _) | (_, 0) => false,
            (xi, yi) => {
                let xs = a.sources.as_ref().and_then(|s| s.get(xi).ok());
                let ys = b.sources.as_ref().and_then(|s| s.get(yi).ok());
                xs.zip(ys).is_some_and(|(x, y)| x.path == y.path && *x.src == *y.src)
            }
        }
    })
}

/// The run summary line.
pub fn summary() -> Option<String> {
    let (m, r) = (MAPPED_FILES.load(Ordering::Relaxed), RECORDED_FILES.load(Ordering::Relaxed));
    let bad = MISMATCHED.load(Ordering::Relaxed);
    if m + r + bad == 0 {
        return None;
    }
    Some(format!(
        "makepad-run front: {} parts (lazily parsed bodies) mapped, {} recorded; {} files mapped from images ({:.1} MB of heap released), {} recorded ({:.1} MB), {} images not matching the parse, {} mapped arrays copied back by a change ({:.1} MB)",
        PARTS_MAPPED.load(Ordering::Relaxed),
        PARTS_RECORDED.load(Ordering::Relaxed),
        m,
        MAPPED_BYTES.load(Ordering::Relaxed) as f64 / 1e6,
        r,
        RECORDED_BYTES.load(Ordering::Relaxed) as f64 / 1e6,
        bad,
        crate::arena::COPIES.load(Ordering::Relaxed),
        crate::arena::COPIED_BYTES.load(Ordering::Relaxed) as f64 / 1e6
    ))
}

/// Construct arrays directly from a validated store; no temporary source/token/AST heap.
/// The caller pins the store before passing its process-lifetime slices here.
pub fn restore(meta: &mut crate::crate_image::FileMeta, img: &'static [u8]) -> Option<ParsedFile> {
    restore_inner(meta, img, false)
}

/// The collected graph's single input certificate already verified this source hash and
/// selected this exact file-image key. Keep section/bounds and asynchronous checksum checks;
/// preserve source identities so authored origins can remain mapped without a boot-time scan.
pub fn restore_certified(meta: &mut crate::crate_image::FileMeta, img: &'static [u8]) -> Option<ParsedFile> {
    restore_inner(meta, img, true)
}

fn restore_inner(meta: &mut crate::crate_image::FileMeta, img: &'static [u8], certified: bool) -> Option<ParsedFile> {
    // A provider owns live lowering state; current image formats cannot reconstruct it.
    use crate::ast::*;
    let r = Reader::new(img)?;
    let src = r.slice::<u8>(0, S_SRC)?;
    if meta.src_len as usize > src.len() || (!certified && crate::crate_image::file_hash(&meta.path, &src[..meta.src_len as usize]) != meta.hash) {
        return None;
    }
    let mut ast = Ast::default();
    let mut k = 0usize;
    macro_rules! get { ($($f:ident: $t:ident),*) => { $(ast.$f=Arena::Mapped(r.slice::<$t>(8+k,S_AST+k as u32)?);k+=1;)* }; }
    ast_arrays!(get);
    let _ = k;
    let mut sources = None;
    let origins = restore_origins(r.slice(6, S_TOKEN_ORIGINS)?, r.slice(7, S_SOURCES)?, &mut sources, certified)?;
    if meta.token_input {
        sources.get_or_insert_with(Default::default).register_mapped(&meta.path, &src[..meta.src_len as usize]).ok()?;
    }
    Some(ParsedFile {
        src: Arena::Mapped(src),
        toks: PackedToks::mapped(r.slice(1, S_TOKS)?)?,
        ast,
        edition: meta.edition,
        src_len: meta.src_len,
        source_hash: std::sync::OnceLock::from((meta.path.clone(), meta.hash)),
        token_input: meta.token_input,
        sources,
        token_origins: origins,
        lazy: meta.lazy.take(),
        exp_map: Table::mapped(r.slice(2, S_EXP_MAP)?)?,
        tok_origin: Table::mapped(r.slice(3, S_TOK_ORIGIN)?)?,
        rules_exp: Table::mapped(r.slice(4, S_RULES_EXP)?)?,
        body_origin: Table::mapped(r.slice(5, S_BODY_ORIGIN)?)?,
        line_starts: Default::default(),
        part: Default::default(),
        impl_heads: Default::default(),
        image: None,
        sink: None,
    })
}

pub fn verify_file(image: &ParsedFile, fresh: &ParsedFile) {
    let a = &image.ast;
    let b = &fresh.ast;
    macro_rules! cmp { ($($f:ident: $t:ident),*) => { $(assert_eq!(format!("{:?}",&a.$f[..]),format!("{:?}",&b.$f[..]),"front AST {}",stringify!($f));)* }; }
    ast_arrays!(cmp);
    assert_eq!(&image.src[..], &fresh.src[..], "front source");
    assert_eq!(image.toks.words(), fresh.toks.words(), "front tokens");
    assert_eq!(image.exp_map, fresh.exp_map);
    assert_eq!(image.tok_origin, fresh.tok_origin);
    assert!(same_origins(image, &image.token_origins, fresh, &fresh.token_origins), "front token origins");
    assert_eq!(image.rules_exp, fresh.rules_exp);
    assert_eq!(image.body_origin, fresh.body_origin);
    assert_eq!((image.edition, image.src_len, image.token_input), (fresh.edition, fresh.src_len, fresh.token_input));
    assert_eq!(crate::crate_image::encode(&image.lazy), crate::crate_image::encode(&fresh.lazy));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_file_images_remap_source_ids_and_lazy_origins() {
        use crate::pm_tokens::{Span, TokenTree};
        let sess = std::sync::Arc::new(crate::Session::default());
        let cfg = crate::cfg::CfgSet::host(&sess, &[]);
        let sources = std::sync::Arc::new(SourceRegistry::default());
        sources.source_id("/fake/earlier.rs");
        let path = "/fake/image-token.rs";
        sources.register(path, std::sync::Arc::from(&b"module construct"[..])).unwrap();
        let source = "// imported\néé source construct for an entire function\n";
        let imported = sources.register("/fake/image-import.rs", std::sync::Arc::from(source.as_bytes())).unwrap();
        let span = Span { file: imported, lo: 16, hi: source.trim_end().len() as u32, ..Span::DUMMY };
        fn set_spans(tokens: &mut [TokenTree], span: Span) {
            for t in tokens {
                t.set_span(span);
                if let TokenTree::Group(g) = t { set_spans(&mut g.stream.trees, span); }
            }
        }
        let mut stream = crate::builtins::lex_stream("pub fn f() -> u32 { concat!(\"one\", \"two\"); 3 }").unwrap();
        set_spans(&mut stream.trees, span);
        let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new("test", path, "test", &cfg));
        let table = std::sync::Arc::new(crate::macros::MacroTable::default());
        let (mut f, used) = crate::parser::parse_token_stream(stream, path, 2021,
            Some(crate::macros::MacroScope::new(table)), ctx, sources, true).unwrap();
        let prep = crate::program::prepare_file(&mut f, &cfg);
        assert!(f.lazy.is_some());
        let main = image(encode(&f));
        let mut changed = main.to_vec();
        let descriptor = 16 + 6 * 24; // S_TOKEN_ORIGINS
        let offset = u64::from_le_bytes(changed[descriptor..descriptor + 8].try_into().unwrap()) as usize;
        changed[offset + 8 + 8 + 2] ^= 1; // one block: header, index, then the first row's head, file id, source lo delta
        assert!(!apply(path, &mut f, image(changed)));
        let metadata = crate::crate_image::file_meta(path, "test", &f, &used.unwrap(), &prep, false);
        let mut meta = crate::crate_image::decode(&metadata).unwrap();
        let mut restored = restore(&mut meta, main).unwrap();
        assert_ne!(restored.token_origins.get(0).unwrap().source.file, imported);
        verify_file(&restored, &f);
        let part = f.part(&cfg).unwrap();
        assert!(!part.token_origins.is_empty());
        let at = part.token_origins.get(0).unwrap().lo;
        let part_image = image(encode_part(&f).unwrap());
        assert!(apply_part(path, &mut restored, part_image, &cfg, false));
        assert!(same_origins(&f, &f.part.get().unwrap().token_origins, &restored, &restored.part.get().unwrap().token_origins));
        let mut program = crate::program::Program::new(&sess);
        for n in 0..5 { program.source_id(&format!("/fake/reserved-{n}.rs")); }
        restored.bind_sources(path, &program.sources).unwrap();
        program.file_paths.push(path.into());
        program.files.push(std::sync::Arc::new(restored));
        let loc = program.source_location(0, at);
        assert_eq!(loc.source.path(), "/fake/image-import.rs");
        assert_eq!((loc.span.lo, loc.span.hi), (span.lo, span.hi));
        assert_eq!(loc.source.bytes(), source.as_bytes());
    }

    fn lazy_file(sess: &std::sync::Arc<crate::Session>) -> (ParsedFile, crate::cfg::CfgSet) {
        let cfg = crate::cfg::CfgSet::host(sess, &[]);
        let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new("test", "test.rs", "test", &cfg));
        let table = std::sync::Arc::new(crate::macros::MacroTable::default());
        let (mut f, _) = crate::parser::parse_owned_ctx(sess,
            b"pub fn f(x: u32) -> u32 { let y = x + 3; y * 2 }".to_vec(), None, 2021,
            Some(crate::macros::MacroScope::new(table)), "test", Some(ctx), true,
        ).unwrap();
        crate::program::prepare_file(&mut f, &cfg);
        assert!(f.lazy.is_some());
        (f, cfg)
    }

    fn image(bytes: Vec<u8>) -> &'static [u8] {
        // The store aligns payloads to 8; emulate that without relying on Vec<u8>'s alignment.
        let mut words = vec![0u64; (bytes.len() + 7) / 8];
        let dst = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, words.len() * 8) };
        dst[..bytes.len()].copy_from_slice(&bytes);
        let words = Box::leak(words.into_boxed_slice());
        unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, bytes.len()) }
    }

    /// A part image built from the part before its publication.
    fn part_bytes(f: &ParsedFile, fp: &crate::parser::FilePart, a: &crate::ast::Ast) -> &'static [u8] {
        image(part_image(f, fp, a).unwrap().finish())
    }

    #[test]
    fn cold_main_and_part_map_before_publication_match_fresh_parse() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let (mut f, cfg) = lazy_file(sess);
        let main = image(encode(&f));
        assert!(heap_bytes(&f) > 0);
        assert!(apply("test.rs", &mut f, main));
        assert_eq!(heap_bytes(&f), 0);
        let (fresh, _) = lazy_file(sess);
        verify_file(&f, &fresh);
        let (mut fp, mut a) = crate::parser::parse_part(&f, &cfg);
        let part = part_bytes(&f, &fp, &a);
        assert!(a.exprs.heap_bytes() > 0);
        assert!(map_part("test.rs", &f, &mut fp, &mut a, part));
        assert_eq!(a.exprs.heap_bytes(), 0);
        assert_eq!(fp.toks.heap_bytes(), 0);
        // The warm path reads the same image to exactly the same ids and body ranges.
        let (mut warm, _) = lazy_file(sess);
        assert!(apply("test.rs", &mut warm, main));
        assert!(apply_part("test.rs", &mut warm, part, &cfg, false));
        assert_eq!(format!("{:?}", fp.bodies), format!("{:?}", warm.part.get().unwrap().bodies));
        assert_eq!(format!("{:?}", a.exprs), format!("{:?}", warm.ast.part.get().unwrap().exprs));
    }

    #[test]
    fn cold_part_rejection_keeps_the_parse() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let (f, cfg) = lazy_file(sess);
        let (mut fp, mut a) = crate::parser::parse_part(&f, &cfg);
        let before = format!("{:?}", a.exprs);
        let mut bytes = part_image(&f, &fp, &a).unwrap().finish();
        bytes[0] ^= 1; // invalid image header
        assert!(!map_part("test.rs", &f, &mut fp, &mut a, image(bytes)));
        assert_eq!(before, format!("{:?}", a.exprs));
        assert!(!fp.toks.is_mapped());
        // A structurally valid image with different body ids must also leave the parse intact.
        let mut bytes = part_image(&f, &fp, &a).unwrap().finish();
        let table = 16 + 4 * 24; // P_BODIES section descriptor
        let at = u64::from_le_bytes(bytes[table..table + 8].try_into().unwrap()) as usize;
        bytes[at + 4] ^= 1;
        assert!(!map_part("test.rs", &f, &mut fp, &mut a, image(bytes)));
        assert_eq!(before, format!("{:?}", a.exprs));
        assert!(!fp.toks.is_mapped());
    }
}
