//! Boot metadata for an all-or-nothing crate restore. Arrays live in separate mapped front
//! images; this compact, checked encoding owns only strings and loader/macro bookkeeping.
//! Hash maps are written in key order; vectors retain their semantic load/definition order.
use crate::fxhash::{HashMap, HashSet};
use crate::{
    macros::*,
    parser::{LazyCtx, ParsedFile},
    preload::CrateAhead,
    program::FilePrep,
};
use std::sync::Arc;

// Demand certificates belong to the graph; crate records keep only their own selection.
pub const VERSION: u32 = 21;

pub(crate) mod strings;

pub trait Codec: Sized {
    fn put(&self, w: &mut Vec<u8>);
    fn get(r: &mut Decode<'_>) -> Option<Self>;
}
pub struct Decode<'a> {
    pub bytes: &'a [u8],
    depth: usize,
}
impl<'a> Decode<'a> {
    pub fn new(bytes: &'a [u8]) -> Self { Self { bytes, depth: 0 } }
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let x = self.bytes.get(..n)?;
        self.bytes = &self.bytes[n..];
        Some(x)
    }
}
pub fn encode<T: Codec>(x: &T) -> Vec<u8> {
    let _mem = crate::memstat::tag(crate::memstat::Tag::FrontRecord);
    let mut b = Vec::new();
    x.put(&mut b);
    b
}
pub fn decode<T: Codec>(b: &[u8]) -> Option<T> {
    let mut r = Decode { bytes: b, depth: 0 };
    let x = T::get(&mut r)?;
    r.bytes.is_empty().then_some(x)
}
macro_rules! num {
    ($($t:ty),*) => {$ (impl Codec for $t {
        fn put(&self, w: &mut Vec<u8>) { w.extend_from_slice(&self.to_le_bytes()); }
        fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self::from_le_bytes(r.take(std::mem::size_of::<Self>())?.try_into().ok()?)) }
    })*};
}
num!(u8, u16, u32, u64, u128, i128);
impl Codec for bool {
    fn put(&self, w: &mut Vec<u8>) {
        (*self as u8).put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        match u8::get(r)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}
impl Codec for usize {
    fn put(&self, w: &mut Vec<u8>) {
        u32::try_from(*self).expect("front metadata length").put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(u32::get(r)? as usize)
    }
}
impl Codec for String {
    fn put(&self, w: &mut Vec<u8>) {
        self.len().put(w);
        w.extend_from_slice(self.as_bytes());
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        let n = usize::get(r)?;
        String::from_utf8(r.take(n)?.to_vec()).ok()
    }
}
impl<T: Codec> Codec for Vec<T> {
    fn put(&self, w: &mut Vec<u8>) {
        self.len().put(w);
        for x in self {
            x.put(w);
        }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        let n = usize::get(r)?;
        if n > r.bytes.len() || r.depth >= 256 {
            return None;
        }
        r.depth += 1;
        let v = (0..n).map(|_| T::get(r)).collect();
        r.depth -= 1;
        v
    }
}
impl<T: Codec> Codec for Option<T> {
    fn put(&self, w: &mut Vec<u8>) {
        self.is_some().put(w);
        if let Some(x) = self {
            x.put(w);
        }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(if bool::get(r)? { Some(T::get(r)?) } else { None })
    }
}
impl<T: Codec> Codec for Arc<T> {
    fn put(&self, w: &mut Vec<u8>) {
        (**self).put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Arc::new(T::get(r)?))
    }
}
impl<T: Codec> Codec for Box<T> {
    fn put(&self, w: &mut Vec<u8>) {
        (**self).put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Box::new(T::get(r)?))
    }
}
impl<A: Codec, B: Codec> Codec for (A, B) {
    fn put(&self, w: &mut Vec<u8>) {
        self.0.put(w);
        self.1.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some((A::get(r)?, B::get(r)?))
    }
}
impl<K: Codec + Ord + std::hash::Hash, V: Codec> Codec for HashMap<K, V> {
    fn put(&self, w: &mut Vec<u8>) {
        let mut v: Vec<_> = self.iter().collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v.len().put(w);
        for (k, v) in v {
            k.put(w);
            v.put(w);
        }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Vec::<(K, V)>::get(r)?.into_iter().collect())
    }
}
impl<T: Codec + Ord + std::hash::Hash> Codec for HashSet<T> {
    fn put(&self, w: &mut Vec<u8>) {
        let mut v: Vec<_> = self.iter().collect();
        v.sort();
        v.len().put(w);
        for x in v {
            x.put(w);
        }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Vec::<T>::get(r)?.into_iter().collect())
    }
}
impl<T: Codec + Ord> Codec for std::collections::BTreeSet<T> {
    fn put(&self, w: &mut Vec<u8>) {
        self.len().put(w);
        for item in self { item.put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Vec::<T>::get(r)?.into_iter().collect()) }
}
macro_rules! fields {
    ($t:ty, $($f:ident),+ $(,)?) => { impl Codec for $t {
        fn put(&self, w: &mut Vec<u8>) { $(self.$f.put(w);)+ }
        fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Self { $($f: Codec::get(r)?,)+ }) }
    }};
}
pub(crate) use fields;
macro_rules! tuple_codec {
    ($($t:ident:$i:tt),+) => { impl<$($t: Codec),+> Codec for ($($t,)+) {
        fn put(&self, w: &mut Vec<u8>) { $(self.$i.put(w);)+ }
        fn get(r: &mut Decode<'_>) -> Option<Self> { Some(($($t::get(r)?,)+)) }
    }};
}
tuple_codec!(A:0, B:1, C:2, D:3, E:4);
tuple_codec!(A:0, B:1, C:2, D:3, E:4, F:5);
impl<K: Codec + Ord, V: Codec> Codec for std::collections::BTreeMap<K, V> {
    fn put(&self, w: &mut Vec<u8>) {
        self.len().put(w);
        for (k, v) in self { k.put(w); v.put(w); }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Vec::<(K, V)>::get(r)?.into_iter().collect()) }
}
impl Codec for crate::lexer::Tok {
    fn put(&self, w: &mut Vec<u8>) {
        let Self { kind, lo, hi } = *self;
        kind.put(w); lo.put(w); hi.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Self { kind: Codec::get(r)?, lo: Codec::get(r)?, hi: Codec::get(r)? })
    }
}
fields!(MTok, kind, text, pos);
// RuleInput is only a parse cache; keep the image format and semantic hashes unchanged.
impl Codec for MacroDef {
    fn put(&self, w: &mut Vec<u8>) {
        self.name.put(w); self.krate.put(w); self.exported.put(w); self.arms.put(w);
        self.builtin.put(w); self.def_file.put(w); self.def_pos.put(w); self.def_mod.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Self { name: Codec::get(r)?, krate: Codec::get(r)?, exported: Codec::get(r)?, arms: Codec::get(r)?,
            builtin: Codec::get(r)?, def_file: Codec::get(r)?, def_pos: Codec::get(r)?, def_mod: Codec::get(r)?, input: None })
    }
}
fields!(UseItem, module, path, bind, is_pub);
fields!(ModItem, pos, end, macro_use);
fields!(LazyCtx, krate, path, module_path, mods, s2_fmt);
/// Option<Box<LazyCtx>>'s encoding
impl Codec for crate::parser::FileLazy {
    fn put(&self, w: &mut Vec<u8>) {
        match self.image() {
            Some(bytes) => w.extend_from_slice(bytes),
            None => { self.is_some().put(w); if let Some(c) = self.as_ref() { c.put(w); } }
        }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> { Some(Option::<Box<LazyCtx>>::get(r)?.into()) }
}
fields!(FilePrep, idents, blocks, lines);
fields!(crate::preload::FileIndex, ci, path, module, names, glob_names, impls, blankets, force);
impl Codec for crate::preload::DemandImage {
    fn put(&self, w: &mut Vec<u8>) {
        self.graph.put(w); self.fingerprint.put(w); self.inputs.put(w); self.probes.put(w);
        self.late_inputs.put(w); self.files.put(w); self.reached.put(w); self.opaque_crates.put(w);
        self.eager_prefix.put(w); self.parts.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Self { graph: Codec::get(r)?, fingerprint: Codec::get(r)?, inputs: Codec::get(r)?, probes: Codec::get(r)?,
            late_inputs: Codec::get(r)?, files: Codec::get(r)?, reached: Codec::get(r)?, opaque_crates: Codec::get(r)?,
            eager_prefix: Codec::get(r)?, parts: Codec::get(r)?, recording: None })
    }
}
impl Codec for crate::lexer::T {
    fn put(&self, w: &mut Vec<u8>) {
        (*self as u8).put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        let n = u8::get(r)?;
        // Authored demand scans also contain the append-only declarative/GPU
        // tokens. The original Rust-only codec silently rejected every such
        // crate's persisted outline before it could be reused.
        if n > Self::Interp as u8 {
            return Some(match n {
                n if n == Self::Colour as u8 => Self::Colour,
                n if n == Self::Duration as u8 => Self::Duration,
                n if n == Self::AtId as u8 => Self::AtId,
                n if n == Self::ColonEq as u8 => Self::ColonEq,
                n if n == Self::PlusColon as u8 => Self::PlusColon,
                n if n == Self::Comment as u8 => Self::Comment,
                n if n == Self::DocComment as u8 => Self::DocComment,
                n if n == Self::Apostrophe as u8 => Self::Apostrophe,
                n if n == Self::OtherPunct as u8 => Self::OtherPunct,
                _ => return None,
            });
        }
        Some(unsafe { std::mem::transmute::<u8, Self>(n) })
    }
}
impl Codec for Frag {
    fn put(&self, w: &mut Vec<u8>) {
        (*self as u8).put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(match u8::get(r)? {
            0 => Self::Expr,
            1 => Self::Ident,
            2 => Self::Ty,
            3 => Self::Tt,
            4 => Self::Pat,
            5 => Self::PatParam,
            6 => Self::Path,
            7 => Self::Literal,
            8 => Self::Block,
            9 => Self::Item,
            10 => Self::Stmt,
            11 => Self::Vis,
            12 => Self::Lifetime,
            13 => Self::Meta,
            _ => return None,
        })
    }
}
impl Codec for MNode {
    fn put(&self, w: &mut Vec<u8>) {
        match self {
            Self::Tok(t) => {
                0u8.put(w);
                t.put(w)
            }
            Self::Var(n, f) => {
                1u8.put(w);
                n.put(w);
                f.put(w)
            }
            Self::Rep(v, s, o) => {
                2u8.put(w);
                v.put(w);
                s.put(w);
                o.put(w)
            }
            Self::Group(a, b, v) => {
                3u8.put(w);
                a.put(w);
                b.put(w);
                v.put(w)
            }
        }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(match u8::get(r)? {
            0 => Self::Tok(Codec::get(r)?),
            1 => Self::Var(Codec::get(r)?, Codec::get(r)?),
            2 => Self::Rep(Codec::get(r)?, Codec::get(r)?, Codec::get(r)?),
            3 => Self::Group(Codec::get(r)?, Codec::get(r)?, Codec::get(r)?),
            _ => return None,
        })
    }
}
impl Codec for TNode {
    fn put(&self, w: &mut Vec<u8>) {
        match self {
            Self::Tok(t) => {
                0u8.put(w);
                t.put(w)
            }
            Self::Var(n) => {
                1u8.put(w);
                n.put(w)
            }
            Self::Rep(v, s, o) => {
                2u8.put(w);
                v.put(w);
                s.put(w);
                o.put(w)
            }
            Self::Group(a, b, v) => {
                3u8.put(w);
                a.put(w);
                b.put(w);
                v.put(w)
            }
            Self::Crate => 4u8.put(w),
        }
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(match u8::get(r)? {
            0 => Self::Tok(Codec::get(r)?),
            1 => Self::Var(Codec::get(r)?),
            2 => Self::Rep(Codec::get(r)?, Codec::get(r)?, Codec::get(r)?),
            3 => Self::Group(Codec::get(r)?, Codec::get(r)?, Codec::get(r)?),
            4 => Self::Crate,
            _ => return None,
        })
    }
}
impl Codec for CrateMods {
    fn put(&self, w: &mut Vec<u8>) {
        self.root.put(w);
        self.src.put(w);
        self.mods.put(w);
        self.items.put(w);
        self.uses.put(w);
        self.externs.put(w);
        self.renames.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Self { root: Codec::get(r)?, src: Codec::get(r)?, mods: Codec::get(r)?, items: Codec::get(r)?, uses: Codec::get(r)?, externs: Codec::get(r)?, renames: Codec::get(r)?, ..Default::default() })
    }
}
impl Codec for MacroUse {
    fn put(&self, w: &mut Vec<u8>) {
        self.local.put(w);
        self.uses.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Self::from_image(Codec::get(r)?, Codec::get(r)?))
    }
}
impl Codec for crate::ast::ItemId {
    fn put(&self, w: &mut Vec<u8>) {
        self.0.put(w)
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Self(u32::get(r)?))
    }
}
impl Codec for crate::ast::BlockId {
    fn put(&self, w: &mut Vec<u8>) {
        self.0.put(w)
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Self(u32::get(r)?))
    }
}
impl Codec for CrateAhead {
    fn put(&self, w: &mut Vec<u8>) {
        self.complete.put(w);
        self.name.put(w);
        self.defs.put(w);
        self.aliases.put(w);
        self.macro_use.put(w);
        self.mods.put(w);
        self.files.put(w);
        self.inputs.put(w);
        self.probes.put(w);
        self.deferred.put(w);
        self.demand_plan.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Self {
            complete: Codec::get(r)?,
            name: Codec::get(r)?,
            defs: Codec::get(r)?,
            aliases: Codec::get(r)?,
            macro_use: Codec::get(r)?,
            mods: Codec::get(r)?,
            files: Codec::get(r)?,
            inputs: Codec::get(r)?,
            probes: Codec::get(r)?,
            deferred: Codec::get(r)?,
            demand_plan: Codec::get(r)?,
            idx: 0,
            prelexed: Vec::new(),
            image: true,
            valid: true,
        })
    }
}
/// Metadata captured at the exact load_file boundary, before collection can ask for bodies.
/// Kept encoded in Program, so recording does not keep a second macro table alive.
pub struct FileMeta {
    pub path: String,
    pub module: String,
    pub hash: u128,
    pub shared: bool,
    pub edition: u16,
    pub src_len: u32,
    pub token_input: bool,
    pub lazy: crate::parser::FileLazy,
    pub used: MacroUse,
    pub prep: FilePrep,
}
fields!(FileMeta, path, module, hash, shared, edition, src_len, token_input, lazy, used, prep);
pub fn file_meta(path: &str, module: &str, pf: &ParsedFile, used: &MacroUse, prep: &FilePrep, shared: bool) -> Vec<u8> {
    let _mem = crate::memstat::tag(crate::memstat::Tag::FrontRecord);
    let mut w = Vec::new();
    path.to_string().put(&mut w);
    module.to_string().put(&mut w);
    pf.source_hash(path).put(&mut w);
    shared.put(&mut w);
    pf.edition.put(&mut w);
    pf.src_len.put(&mut w);
    pf.token_input.put(&mut w);
    pf.lazy.put(&mut w);
    used.put(&mut w);
    prep.put(&mut w);
    w
}
pub fn file_hash(path: &str, src: &[u8]) -> u128 {
    let mut h = crate::sip::Sip128::new();
    h.str(path);
    h.write(src);
    h.finish128()
}
/// A crate's dependency fingerprints as its fingerprint hashes them: by crate name, each
/// (name, fingerprint) once -- the load adds `extern crate x as y` aliases (and std's crates)
/// to a crate's dependencies, the preload knows only the unit graph's.
pub fn canon_deps(mut named: Vec<(String, u128)>) -> Vec<u128> {
    named.sort();
    named.dedup();
    named.into_iter().map(|x| x.1).collect()
}

/// Same byte stream as stable::crate_fingerprints. Dependency aliases are checked separately
/// in the identity because the historical fingerprint includes only their fingerprints.
pub fn fingerprint(name: &str, root: &str, edition: u16, proc_macro: bool, cfg: &[String], files: impl Iterator<Item = u128>, deps: &[u128], selection_hash: u128) -> u128 {
    let mut h = fingerprint_head(name, root, edition, proc_macro, cfg);
    for f in files {
        h.u128(f);
    }
    h.u128(selection_hash);
    h.u32(deps.len() as u32);
    for d in deps {
        h.u128(*d);
    }
    h.finish128()
}

pub fn fingerprint_head(name: &str, root: &str, edition: u16, proc_macro: bool, cfg: &[String]) -> crate::sip::Sip128 {
    let mut h = crate::sip::Sip128::new();
    h.str("crate");
    h.str(name);
    h.str(root);
    h.u32(edition as u32);
    h.u8(proc_macro as u8);
    let mut cfg = cfg.to_vec();
    cfg.sort();
    h.u32(cfg.len() as u32);
    for c in cfg {
        h.str(&c)
    }
    h
}
pub struct Record {
    pub name: String,
    pub root: String,
    pub edition: u16,
    pub cfg: Vec<String>,
    pub proc_macro: bool,
    pub deps: Vec<(String, String)>,
    pub dep_fps: Vec<u128>,
    pub lazy_auto: bool,
    pub parse_options: u128,
    pub ahead: CrateAhead,
    pub files: Vec<FileMeta>,
    pub selection_hash: u128,
}
fields!(Record, name, root, edition, cfg, proc_macro, deps, dep_fps, lazy_auto, parse_options, ahead, files, selection_hash);
impl Record {
    pub fn file_hashes(&self) -> Vec<u128> {
        self.files.iter().map(|f| f.hash).collect()
    }
}
pub struct CrateImage {
    pub selection_hash: u128,
    pub fingerprint: u128,
    pub ahead: CrateAhead,
    pub files: Vec<(String, crate::preload::Preparsed)>,
    /// the record validated (sources, probes, identity unchanged) but its file images are not
    /// all in its store: the crate is parsed, its
    /// fingerprint lets dependents restore
    pub fp_only: bool,
}
/// The compiler owns store discovery and mappings; the front end owns scheduling and replay.
pub struct DemandParts {
    pub eager_prefix: usize,
    pub valid: Vec<bool>,
    pub load: Arc<dyn Fn() -> Vec<Option<Arc<crate::preload::DemandPart>>> + Send + Sync>,
}

/// A checked range in a section of an existing per-crate front image. Planning
/// streams that differ from parser streams use their own image in that store.
#[derive(Clone)]
pub struct DemandSlice {
    pub image: u128,
    pub start: usize,
    pub len: usize,
    pub hash: u128,
}
fields!(DemandSlice, image, start, len, hash);

pub fn demand_token_hash(toks: &[crate::lexer::Tok]) -> u128 {
    let mut h = crate::sip::Sip128::new();
    h.str("demand-tokens-v2");
    // 9 bytes a token, written in blocks (one `write` per token field made the hash a
    // per-byte tail loop)
    let mut block = [0u8; 9 * 256];
    for chunk in toks.chunks(256) {
        for (i, t) in chunk.iter().enumerate() {
            let (kind, lo, hi) = (t.kind as u8, t.lo, t.hi);
            let at = i * 9;
            block[at] = kind;
            block[at + 1..at + 5].copy_from_slice(&lo.to_le_bytes());
            block[at + 5..at + 9].copy_from_slice(&hi.to_le_bytes());
        }
        h.write(&block[..chunk.len() * 9]);
    }
    h.finish128()
}

pub trait DemandWriter {
    fn store_name(&self) -> &str;
    fn surface(&mut self, path: &str, hash: u128, src: &[u8], toks: &[crate::lexer::Tok]) -> Option<(DemandSlice, DemandSlice)>;
    fn definition(&mut self, def: &MacroDef) -> Option<(u128, usize)>;
}
pub trait CrateImages {
    /// Validation is shared only during this immutable preload, never across edits.
    fn begin_load(&self, _crates: &[crate::preload::PreCrate]) {}
    fn end_load(&self) {}
    fn demand_image(&self, _crates: &[crate::preload::PreCrate], _graph: u128) -> Option<Arc<crate::preload::DemandImage>> { None }
    fn take_demand_plan(&self, _crates: &[crate::preload::PreCrate]) -> Option<Arc<crate::preload::DemandPlan>> { None }
    fn demand_parts(&self, _crates: &[crate::preload::PreCrate], _graph: u128) -> Option<DemandParts> { None }
    fn crate_image(&self, c: &crate::preload::PreCrate, dep_fps: &[u128]) -> Option<CrateImage>;
    fn crate_image_selected(&self, c: &crate::preload::PreCrate, dep_fps: &[u128], plan: u128, deferred: &[String]) -> Option<CrateImage> {
        self.crate_image(c, dep_fps).filter(|image| image.ahead.demand_plan == plan && image.ahead.deferred == deferred)
    }
}

/// A separate graph record can be replaced even if its last crate is unchanged.
#[derive(Clone)]
pub struct GraphRecipe {
    pub identity: u128,
    pub crates: Vec<(String, String)>,
}
impl GraphRecipe {
    pub fn new(crates: &[crate::preload::PreCrate]) -> Self {
        let mut h = crate::sip::Sip128::new();
        h.str("eager-front-graph");
        h.u32(VERSION);
        h.u8(crate::parser::lazy_auto() as u8);
        h.u32(crates.len() as u32);
        for c in crates {
            h.str(&c.name); h.str(&c.root); h.u32(c.edition as u32);
            h.u128(parse_options(&c.cfg.session.options)); h.u8(c.proc_macro as u8);
            let mut cfg = c.cfg.set.clone(); cfg.sort();
            h.u32(cfg.len() as u32);
            for value in cfg { h.str(&value); }
            h.u32(c.deps.len() as u32);
            for (alias, name) in &c.deps { h.str(alias); h.str(name); }
            h.u32(c.image_deps.len() as u32);
            for d in &c.image_deps { h.u32(*d as u32); }
            h.u32(c.image_dep_names.len() as u32);
            for (alias, name) in &c.image_dep_names { h.str(alias); h.str(name); }
        }
        Self { identity: h.finish128(), crates: crates.iter().map(|c| (c.name.clone(), c.root.clone())).collect() }
    }
}

/// One certificate for every authored/schema input and module probe in the graph.
/// ASTs stay in per-file images in the named crate stores; no name index is needed.
pub struct GraphImage {
    pub identity: u128,
    pub crates: Vec<((String, String), u128)>,
    pub inputs: Vec<(String, u128)>,
    /// per input (same order): the stat the recorded hash was taken under; None when the input
    /// has none (virtual) or changed too close to `recorded_ns` to trust a stat ("racy")
    pub stamps: Vec<Option<crate::vfs::FileStat>>,
    pub probes: Vec<(String, bool)>,
    /// Whole-plan selection proof, independent of any one crate's identity.
    pub demand_image: Option<Arc<crate::preload::DemandImage>>,
}
fields!(crate::vfs::FileStat, size, mtime_ns, ctime_ns, ino);
fields!(GraphImage, identity, crates, inputs, stamps, probes, demand_image);

/// Share the whole frozen options identity with the code cache. This is conservative, but
/// preserves sharing between apps with the same target and compiler options.
/// rustc: compiler/rustc_session/src/options.rs:280-303 (tracked inputs).
pub fn parse_options(options: &crate::options::Options) -> u128 {
    let mut h = crate::sip::Sip128::new();
    h.u128(options.fingerprint());
    h.finish128()
}
