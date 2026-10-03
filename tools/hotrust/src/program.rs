//! The whole program: crates, parsed files, interned names, definitions and
//! module scopes. Item collection, `use` resolution (fixpoint) and path lookup.

use crate::ast::*;
use crate::cfg::{CfgSet, Src};
use crate::parser::{parse_owned, ParsedFile};
use std::collections::HashMap;

pub type Sym = u32;

#[derive(Default)]
pub struct Interner {
    map: HashMap<String, Sym>,
    strs: Vec<String>,
}

impl Interner {
    pub fn intern(&mut self, s: &str) -> Sym {
        if let Some(&x) = self.map.get(s) {
            return x;
        }
        let id = self.strs.len() as Sym;
        self.strs.push(s.to_string());
        self.map.insert(s.to_string(), id);
        id
    }
    pub fn get(&self, s: &str) -> Option<Sym> {
        self.map.get(s).copied()
    }
    pub fn str(&self, s: Sym) -> &str {
        &self.strs[s as usize]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DefId(pub u32);

pub const NO_DEF: DefId = DefId(u32::MAX);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Prim {
    Bool,
    Char,
    Str,
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    F32,
    F64,
}

pub const PRIMS: [(&str, Prim); 17] = [
    ("bool", Prim::Bool),
    ("char", Prim::Char),
    ("str", Prim::Str),
    ("i8", Prim::I8),
    ("i16", Prim::I16),
    ("i32", Prim::I32),
    ("i64", Prim::I64),
    ("i128", Prim::I128),
    ("isize", Prim::Isize),
    ("u8", Prim::U8),
    ("u16", Prim::U16),
    ("u32", Prim::U32),
    ("u64", Prim::U64),
    ("u128", Prim::U128),
    ("usize", Prim::Usize),
    ("f32", Prim::F32),
    ("f64", Prim::F64),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DefKind {
    Mod,
    Fn,
    Struct,
    Union,
    Enum,
    Variant,
    Const,
    Static,
    Trait,
    Impl,
    TypeAlias,
    AssocFn,
    AssocConst,
    AssocTy,
    ForeignFn,
    ForeignStatic,
    MacroRules,
    Prim(Prim),
}

pub struct Def {
    pub kind: DefKind,
    pub name: Sym,
    /// enclosing module (items), impl/trait (assoc items), enum (variants)
    pub parent: DefId,
    pub file: u32,
    pub item: ItemId,
    /// Mod: module index; Variant: variant index; Enum: variant list index;
    /// Impl: impl index; Trait: trait index
    pub sub: u32,
    pub is_pub: bool,
    pub krate: u32,
    /// module index whose scope resolves names used by this item
    pub scope: u32,
}

pub struct Module {
    pub def: DefId,
    /// lexical parent for anonymous block modules (lookups fall through)
    pub lexical_parent: Option<u32>,
    /// the named module that `self`/`super` refer to
    pub normal: u32,
    pub parent_mod: Option<u32>,
    pub krate: u32,
    pub file: u32,
    pub types: HashMap<Sym, DefId>,
    pub values: HashMap<Sym, DefId>,
    pub macros: HashMap<Sym, DefId>,
    pub globs: Vec<DefId>,
}

pub struct ImplData {
    pub def: DefId,
    pub file: u32,
    pub item: ItemId,
    pub module: u32,
    pub items: Vec<DefId>,
}

pub struct Import {
    pub module: u32,
    pub global: bool,
    pub segs: Vec<(SegKind, Sym)>,
    /// binding name (None for globs)
    pub bind: Option<Sym>,
    pub glob: bool,
    pub is_pub: bool,
    pub done: bool,
    pub file: u32,
    pub pos: u32,
}

pub struct CrateInfo {
    pub name: Sym,
    pub root_mod: u32,
    pub edition: u16,
    /// (extern name, crate index)
    pub deps: Vec<(Sym, u32)>,
    pub cfg: CfgSet,
}

pub struct Program {
    pub syms: Interner,
    pub files: Vec<ParsedFile>,
    pub file_paths: Vec<String>,
    pub file_crate: Vec<u32>,
    pub crates: Vec<CrateInfo>,
    pub defs: Vec<Def>,
    pub mods: Vec<Module>,
    pub variants: Vec<Vec<DefId>>,
    pub impls: Vec<ImplData>,
    pub traits: Vec<Vec<DefId>>,
    pub imports: Vec<Import>,
    /// (file, block) -> anonymous module holding the block's items
    pub block_mods: HashMap<(u32, u32), u32>,
    /// (file, item) -> def
    pub item_defs: HashMap<(u32, u32), DefId>,
    pub prim_defs: Vec<DefId>,
    /// index of the crate whose `prelude` module supplies the std prelude
    pub prelude_crate: Option<u32>,
    pub errors: Vec<String>,
}

impl Program {
    pub fn new() -> Program {
        let mut p = Program {
            syms: Interner::default(),
            files: Vec::new(),
            file_paths: Vec::new(),
            file_crate: Vec::new(),
            crates: Vec::new(),
            defs: Vec::new(),
            mods: Vec::new(),
            variants: Vec::new(),
            impls: Vec::new(),
            traits: Vec::new(),
            imports: Vec::new(),
            block_mods: HashMap::new(),
            item_defs: HashMap::new(),
            prim_defs: Vec::new(),
            prelude_crate: None,
            errors: Vec::new(),
        };
        for kw in ["Self", "self", "super", "crate", "prelude", "core", "std"] {
            p.syms.intern(kw);
        }
        for (name, prim) in PRIMS {
            let s = p.syms.intern(name);
            let d = p.add_def(Def { kind: DefKind::Prim(prim), name: s, parent: NO_DEF, file: 0, item: ItemId(0), sub: 0, is_pub: true, krate: 0, scope: 0 });
            p.prim_defs.push(d);
        }
        p
    }

    pub fn add_def(&mut self, d: Def) -> DefId {
        self.defs.push(d);
        DefId(self.defs.len() as u32 - 1)
    }

    #[inline]
    pub fn def(&self, d: DefId) -> &Def {
        &self.defs[d.0 as usize]
    }

    pub fn name(&self, d: DefId) -> &str {
        self.syms.str(self.defs[d.0 as usize].name)
    }

    pub fn prim_def(&self, p: Prim) -> DefId {
        for i in 0..PRIMS.len() {
            if PRIMS[i].1 == p {
                return self.prim_defs[i];
            }
        }
        NO_DEF
    }

    pub fn src(&self, file: u32) -> Src<'_> {
        let f = &self.files[file as usize];
        Src { src: &f.src, toks: &f.toks }
    }

    pub fn ident(&mut self, file: u32, id: Ident) -> Sym {
        let f = &self.files[file as usize];
        let s = std::str::from_utf8(&f.src[id.lo as usize..id.hi as usize]).unwrap_or("?");
        let s = match s.strip_prefix("r#") {
            Some(x) => x,
            None => s,
        };
        // borrow juggling: copy to a local string first
        let owned = s.to_string();
        self.syms.intern(&owned)
    }

    /// Readable path of a def, for messages.
    pub fn def_path(&self, d: DefId) -> String {
        let mut parts = Vec::new();
        let mut cur = d;
        let mut guard = 0;
        while cur != NO_DEF && guard < 64 {
            let def = self.def(cur);
            parts.push(self.syms.str(def.name).to_string());
            cur = def.parent;
            guard += 1;
        }
        parts.reverse();
        parts.join("::")
    }

    // ------------------------------------------------------------ loading

    /// Loads a crate from its root file: parses every active module file.
    pub fn load_crate(&mut self, name: &str, root: &str, edition: u16, cfg: CfgSet, deps: Vec<(Sym, u32)>) -> Result<u32, String> {
        let krate = self.crates.len() as u32;
        let cname = self.syms.intern(name);
        let root_def = self.add_def(Def { kind: DefKind::Mod, name: cname, parent: NO_DEF, file: 0, item: ItemId(0), sub: 0, is_pub: true, krate, scope: 0 });
        let root_mod = self.new_module(root_def, None, None, krate, 0);
        self.defs[root_def.0 as usize].sub = root_mod;
        self.defs[root_def.0 as usize].scope = root_mod;
        self.crates.push(CrateInfo { name: cname, root_mod, edition, deps, cfg });
        let file = self.load_file(root, edition, krate)?;
        self.defs[root_def.0 as usize].file = file;
        self.mods[root_mod as usize].file = file;
        let dir = dir_of(root).to_string();
        let items = self.files[file as usize].ast.root_items.clone();
        self.collect_items(file, &items, root_mod, &dir, true)?;
        Ok(krate)
    }

    fn load_file(&mut self, path: &str, edition: u16, krate: u32) -> Result<u32, String> {
        let src = std::fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
        let pf = match parse_owned(src, edition) {
            Ok(p) => p,
            Err((pos, msg)) => {
                let src = std::fs::read(path).unwrap_or_default();
                let (l, c) = crate::lexer::line_col(&src, pos);
                return Err(format!("{}:{}:{}: {}", path, l, c, msg));
            }
        };
        self.intern_idents(&pf);
        self.files.push(pf);
        self.file_paths.push(path.to_string());
        self.file_crate.push(krate);
        Ok(self.files.len() as u32 - 1)
    }

    fn intern_idents(&mut self, pf: &ParsedFile) {
        for t in &pf.toks {
                if t.kind == crate::lexer::T::Lifetime {
                        let s = std::str::from_utf8(&pf.src[t.lo as usize + 1..t.hi as usize]).unwrap_or("?");
                        self.syms.intern(s);
                }
                if t.kind == crate::lexer::T::Ident || t.kind == crate::lexer::T::RawIdent {
                        let s = std::str::from_utf8(&pf.src[t.lo as usize..t.hi as usize]).unwrap_or("?");
                        let s = match s.strip_prefix("r#") {
                            Some(x) => x,
                            None => s,
                        };
                        self.syms.intern(s);
                }
            }
    }

    /// Live edit: replaces a file's syntax tree when its item structure is unchanged.
    /// Returns the defs (fns) whose source text changed.
    pub fn replace_file(&mut self, file: u32, path: &str, src: Vec<u8>) -> Result<Vec<DefId>, String> {
        let edition = self.files[file as usize].edition;
        let pf = match parse_owned(src, edition) {
            Ok(p) => p,
            Err((pos, msg)) => return Err(format!("parse error at byte {}: {}", pos, msg)),
        };
        let old = &self.files[file as usize];
        if old.ast.items.len() != pf.ast.items.len() {
            return Err("item structure changed (add/remove of items needs a reload)".to_string());
        }
        let mut changed = Vec::new();
        for (i, d) in self.defs.iter().enumerate() {
            if d.file != file || !matches!(d.kind, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::Static | DefKind::AssocConst) {
                continue;
            }
            let a = old.ast.item(d.item);
            let b = pf.ast.item(d.item);
            if old.src[a.lo as usize..a.hi as usize] != pf.src[b.lo as usize..b.hi as usize] {
                changed.push(DefId(i as u32));
            }
        }
        self.intern_idents(&pf);
        self.files[file as usize] = pf;
        // reports and positions name the file the new code came from
        self.file_paths[file as usize] = path.to_string();
        Ok(changed)
    }

    fn new_module(&mut self, def: DefId, parent_mod: Option<u32>, lexical_parent: Option<u32>, krate: u32, file: u32) -> u32 {
        let idx = self.mods.len() as u32;
        let normal = match lexical_parent {
            Some(lp) => self.mods[lp as usize].normal,
            None => idx,
        };
        self.mods.push(Module {
            def,
            lexical_parent,
            normal,
            parent_mod,
            krate,
            file,
            types: HashMap::new(),
            values: HashMap::new(),
            macros: HashMap::new(),
            globs: Vec::new(),
        });
        idx
    }

    fn active(&self, file: u32, attrs: &[Attr]) -> bool {
        let krate = self.file_crate[file as usize];
        crate::cfg::active(self.src(file), attrs, &self.crates[krate as usize].cfg)
    }

    fn define(&mut self, module: u32, ns_type: bool, name: Sym, d: DefId) {
        let m = &mut self.mods[module as usize];
        if ns_type {
            m.types.insert(name, d);
        } else {
            m.values.insert(name, d);
        }
    }

    /// Collects items of one module body. `dir`: directory for child `mod x;` files.
    fn collect_items(&mut self, file: u32, items: &[ItemId], module: u32, dir: &str, mod_rs: bool) -> Result<(), String> {
        let krate = self.mods[module as usize].krate;
        let mod_def = self.mods[module as usize].def;
        for &iid in items {
            let (attrs_active, kind_tag) = {
                let it = self.files[file as usize].ast.item(iid);
                (self.active(file, &it.attrs), 0)
            };
            let _ = kind_tag;
            if !attrs_active {
                continue;
            }
            let it_name = self.files[file as usize].ast.item(iid).name;
            let is_pub = !matches!(self.files[file as usize].ast.item(iid).vis, Vis::Private);
            let name = self.ident(file, it_name);
            // take a cheap descriptor of the kind to avoid holding a borrow
            let kind = item_tag(&self.files[file as usize].ast.item(iid).kind);
            let mk = |kind: DefKind| Def { kind, name, parent: mod_def, file, item: iid, sub: 0, is_pub, krate, scope: module };
            match kind {
                Tag::Fn => {
                    let d = self.add_def(mk(DefKind::Fn));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, false, name, d);
                    self.collect_body_items(file, iid, module)?;
                }
                Tag::Struct => {
                    let d = self.add_def(mk(DefKind::Struct));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                    // tuple and unit structs also live in the value namespace
                    if let ItemKind::Struct(_, data) = &self.files[file as usize].ast.item(iid).kind {
                        if !matches!(data, VariantData::Struct(_)) {
                            self.define(module, false, name, d);
                        }
                    }
                }
                Tag::Union => {
                    let d = self.add_def(mk(DefKind::Union));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                }
                Tag::Enum => {
                    let d = self.add_def(mk(DefKind::Enum));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                    let mut vars = Vec::new();
                    let n = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::Enum(_, v) => v.len(),
                        _ => 0,
                    };
                    for vi in 0..n {
                        let vname_id = match &self.files[file as usize].ast.item(iid).kind {
                            ItemKind::Enum(_, v) => v[vi].name,
                            _ => Ident::default(),
                        };
                        let vname = self.ident(file, vname_id);
                        let vd = self.add_def(Def { kind: DefKind::Variant, name: vname, parent: d, file, item: iid, sub: vi as u32, is_pub: true, krate, scope: module });
                        vars.push(vd);
                    }
                    let li = self.variants.len() as u32;
                    self.variants.push(vars);
                    self.defs[d.0 as usize].sub = li;
                }
                Tag::Const | Tag::Static => {
                    let k = if kind == Tag::Const { DefKind::Const } else { DefKind::Static };
                    let d = self.add_def(mk(k));
                    self.item_defs.insert((file, iid.0), d);
                    // `const _: () = ...;` defines nothing nameable
                    if self.syms.str(name) != "_" {
                        self.define(module, false, name, d);
                    }
                    self.collect_body_items(file, iid, module)?;
                }
                Tag::TypeAlias => {
                    let d = self.add_def(mk(DefKind::TypeAlias));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                }
                Tag::Trait => {
                    let d = self.add_def(mk(DefKind::Trait));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                    let sub_items = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::Trait { items, .. } => items.clone(),
                        _ => Vec::new(),
                    };
                    let mut list = Vec::new();
                    for si in sub_items {
                        if let Some(ad) = self.collect_assoc(file, si, d, krate, module)? {
                            list.push(ad);
                        }
                        self.collect_body_items(file, si, module)?;
                    }
                    let ti = self.traits.len() as u32;
                    self.traits.push(list);
                    self.defs[d.0 as usize].sub = ti;
                }
                Tag::Impl => {
                    let d = self.add_def(mk(DefKind::Impl));
                    self.item_defs.insert((file, iid.0), d);
                    let sub_items = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::Impl { items, .. } => items.clone(),
                        _ => Vec::new(),
                    };
                    let mut list = Vec::new();
                    for si in sub_items {
                        if let Some(ad) = self.collect_assoc(file, si, d, krate, module)? {
                            list.push(ad);
                        }
                        self.collect_body_items(file, si, module)?;
                    }
                    let ii = self.impls.len() as u32;
                    self.impls.push(ImplData { def: d, file, item: iid, module, items: list });
                    self.defs[d.0 as usize].sub = ii;
                }
                Tag::Mod => {
                    let d = self.add_def(mk(DefKind::Mod));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                    let m = self.new_module(d, Some(module), None, krate, file);
                    self.defs[d.0 as usize].sub = m;
                    let sname = self.syms.str(name).to_string();
                    let mut path_attr = Vec::new();
                    crate::crates::path_attrs_src(self.src(file), &self.files[file as usize].ast.item(iid).attrs, &mut path_attr);
                    let inline = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::Mod(Some(items)) => Some(items.clone()),
                        _ => None,
                    };
                    match inline {
                        Some(inner) => {
                            let d2 = if !path_attr.is_empty() { format!("{}/{}", dir, path_attr[0]) } else { format!("{}/{}", dir, sname) };
                            self.collect_items(file, &inner, m, &d2, true)?;
                        }
                        None => {
                            let base = if mod_rs { dir.to_string() } else { dir.to_string() };
                            let (cands, child_mod_rs): (Vec<String>, bool) = if !path_attr.is_empty() {
                                let fdir = dir_of(&self.file_paths[file as usize]).to_string();
                                let mut v = Vec::new();
                                for pa in &path_attr {
                                    v.push(format!("{}/{}", fdir, pa));
                                }
                                (v, true)
                            } else {
                                (vec![format!("{}/{}.rs", base, sname), format!("{}/{}/mod.rs", base, sname)], false)
                            };
                            let mut loaded = false;
                            for c in &cands {
                                if std::path::Path::new(c).exists() {
                                    let edition = self.crates[krate as usize].edition;
                                    let f = self.load_file(c, edition, krate)?;
                                    self.mods[m as usize].file = f;
                                    let is_mod_rs = child_mod_rs || c.ends_with("/mod.rs");
                                    let cdir = if is_mod_rs { dir_of(c).to_string() } else { format!("{}/{}", dir_of(c), stem_of(c)) };
                                    let items = self.files[f as usize].ast.root_items.clone();
                                    self.collect_items(f, &items, m, &cdir, is_mod_rs)?;
                                    loaded = true;
                                    break;
                                }
                            }
                            if !loaded {
                                return Err(format!("module file not found: {}", cands[0]));
                            }
                        }
                    }
                }
                Tag::ForeignMod => {
                    let sub_items = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::ForeignMod(_, items) => items.clone(),
                        _ => Vec::new(),
                    };
                    for si in sub_items {
                        if !self.active(file, &self.files[file as usize].ast.item(si).attrs) {
                            continue;
                        }
                        let n = self.files[file as usize].ast.item(si).name;
                        let sn = self.ident(file, n);
                        let ip = !matches!(self.files[file as usize].ast.item(si).vis, Vis::Private);
                        let k = match &self.files[file as usize].ast.item(si).kind {
                            ItemKind::Fn(..) => DefKind::ForeignFn,
                            ItemKind::Static(..) => DefKind::ForeignStatic,
                            _ => continue,
                        };
                        let d = self.add_def(Def { kind: k, name: sn, parent: mod_def, file, item: si, sub: 0, is_pub: ip, krate, scope: module });
                        self.item_defs.insert((file, si.0), d);
                        self.define(module, k != DefKind::ForeignFn && k != DefKind::ForeignStatic, sn, d);
                    }
                }
                Tag::Use => {
                    let (global, tree) = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::Use { global, tree } => (*global, tree.clone()),
                        _ => continue,
                    };
                    let pos = self.files[file as usize].ast.item(iid).lo;
                    let mut prefix = Vec::new();
                    self.flatten_use(file, module, global, &tree, &mut prefix, is_pub, pos);
                }
                Tag::ExternCrate => {
                    // `extern crate foo as bar;` binds `bar` (or `foo`) to the crate root
                    let rename = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::ExternCrate(r) => *r,
                        _ => None,
                    };
                    let bind = match rename {
                        Some(r) => self.ident(file, r),
                        None => name,
                    };
                    let pos = self.files[file as usize].ast.item(iid).lo;
                    self.imports.push(Import { module, global: true, segs: vec![(SegKind::Ident, name)], bind: Some(bind), glob: false, is_pub, done: false, file, pos });
                }
                Tag::MacroRules => {
                    let d = self.add_def(mk(DefKind::MacroRules));
                    self.mods[module as usize].macros.insert(name, d);
                }
                Tag::Other => {}
            }
        }
        Ok(())
    }

    fn collect_assoc(&mut self, file: u32, si: ItemId, owner: DefId, krate: u32, module: u32) -> Result<Option<DefId>, String> {
        if !self.active(file, &self.files[file as usize].ast.item(si).attrs) {
            return Ok(None);
        }
        let n = self.files[file as usize].ast.item(si).name;
        let name = self.ident(file, n);
        let is_pub = !matches!(self.files[file as usize].ast.item(si).vis, Vis::Private);
        let kind = match &self.files[file as usize].ast.item(si).kind {
            ItemKind::Fn(..) => DefKind::AssocFn,
            ItemKind::Const(..) => DefKind::AssocConst,
            ItemKind::TypeAlias { .. } => DefKind::AssocTy,
            _ => return Ok(None),
        };
        let d = self.add_def(Def { kind, name, parent: owner, file, item: si, sub: 0, is_pub, krate, scope: module });
        self.item_defs.insert((file, si.0), d);
        Ok(Some(d))
    }

    /// Items declared inside a fn/const body get anonymous block modules.
    fn collect_body_items(&mut self, file: u32, iid: ItemId, module: u32) -> Result<(), String> {
        let body = match &self.files[file as usize].ast.item(iid).kind {
            ItemKind::Fn(_, Some(b), _) => Some(*b),
            _ => None,
        };
        if let Some(b) = body {
            self.collect_block_items(file, b, module)?;
        }
        Ok(())
    }

    fn collect_block_items(&mut self, file: u32, b: BlockId, module: u32) -> Result<(), String> {
        // items directly in this block
        let mut items = Vec::new();
        let mut sub_blocks = Vec::new();
        {
            let ast = &self.files[file as usize].ast;
            let blk = ast.block(b);
            for s in &blk.stmts {
                match s {
                    Stmt::Item(i) => items.push(*i),
                    Stmt::Expr(e, _) => blocks_in_expr(ast, *e, &mut sub_blocks),
                    Stmt::Let { init, else_, .. } => {
                        if let Some(e) = init {
                            blocks_in_expr(ast, *e, &mut sub_blocks);
                        }
                        if let Some(eb) = else_ {
                            sub_blocks.push(*eb);
                        }
                    }
                    Stmt::Empty => {}
                }
            }
        }
        let mut inner = module;
        if !items.is_empty() {
            let krate = self.mods[module as usize].krate;
            let mdef = self.mods[module as usize].def;
            inner = self.new_module(mdef, Some(module), Some(module), krate, file);
            self.block_mods.insert((file, b.0), inner);
            self.collect_items(file, &items, inner, ".", true)?;
        }
        for sb in sub_blocks {
            self.collect_block_items(file, sb, inner)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn flatten_use(&mut self, file: u32, module: u32, global: bool, t: &UseTree, prefix: &mut Vec<(SegKind, Sym)>, is_pub: bool, pos: u32) {
        match t {
            UseTree::Path(seg, rest) => {
                let s = self.ident(file, seg.name);
                prefix.push((seg.kind, s));
                self.flatten_use(file, module, global, rest, prefix, is_pub, pos);
                prefix.pop();
            }
            UseTree::Name(seg, rename) => {
                let s = self.ident(file, seg.name);
                let mut segs = prefix.clone();
                // `use foo::{self}` imports `foo` itself
                let bind;
                if seg.kind == SegKind::SelfValue && !segs.is_empty() {
                    bind = match rename {
                        Some(r) => self.ident(file, *r),
                        None => segs[segs.len() - 1].1,
                    };
                } else {
                    segs.push((seg.kind, s));
                    bind = match rename {
                        Some(r) => self.ident(file, *r),
                        None => s,
                    };
                }
                self.imports.push(Import { module, global, segs, bind: Some(bind), glob: false, is_pub, done: false, file, pos });
            }
            UseTree::Glob => {
                self.imports.push(Import { module, global, segs: prefix.clone(), bind: None, glob: true, is_pub, done: false, file, pos });
            }
            UseTree::Group(v) => {
                for x in v {
                    self.flatten_use(file, module, global, x, prefix, is_pub, pos);
                }
            }
        }
    }

    // ------------------------------------------------------------ imports

    /// Resolves all imports to a fixpoint. Unresolved imports are reported.
    pub fn resolve_imports(&mut self) {
        loop {
            let mut progress = false;
            for i in 0..self.imports.len() {
                if self.imports[i].done {
                    continue;
                }
                if self.try_import(i) {
                    self.imports[i].done = true;
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
        for i in 0..self.imports.len() {
            if !self.imports[i].done {
                let im = &self.imports[i];
                let mut p = String::new();
                for (k, s) in &im.segs {
                    if !p.is_empty() {
                        p.push_str("::");
                    }
                    p.push_str(match k {
                        SegKind::Crate => "crate",
                        SegKind::SelfValue => "self",
                        SegKind::Super => "super",
                        _ => self.syms.str(*s),
                    });
                }
                if im.glob {
                    p.push_str("::*");
                }
                let (l, _) = crate::lexer::line_col(&self.files[im.file as usize].src, im.pos);
                self.errors.push(format!("{}:{}: unresolved import `{}`", self.file_paths[im.file as usize], l, p));
            }
        }
    }

    fn try_import(&mut self, i: usize) -> bool {
        let module = self.imports[i].module;
        let global = self.imports[i].global;
        let glob = self.imports[i].glob;
        let segs = self.imports[i].segs.clone();
        let edition = self.crates[self.mods[module as usize].krate as usize].edition;
        if glob {
            let target = match self.resolve_mod_path(module, global, &segs, edition < 2018, true) {
                Some(t) => t,
                None => return false,
            };
            self.mods[module as usize].globs.push(target);
            return true;
        }
        let bind = self.imports[i].bind.unwrap_or(0);
        let (last_kind, last) = segs[segs.len() - 1];
        let mut tdef = None;
        let mut vdef = None;
        if segs.len() == 1 && (global || (edition >= 2018 && last_kind == SegKind::Ident)) {
            // `use foo;` / `extern crate foo` / `use ::foo` -> extern crate, else item in scope
            if let Some(c) = self.extern_crate(module, last) {
                tdef = Some(self.mods[self.crates[c as usize].root_mod as usize].def);
            } else if !global {
                tdef = self.lookup_in_scope(module, last, true);
                vdef = self.lookup_in_scope(module, last, false);
            }
        } else if segs.len() == 1 && last_kind != SegKind::Ident {
            let m = match self.resolve_mod_path(module, global, &segs, edition < 2018, true) {
                Some(m) => m,
                None => return false,
            };
            tdef = Some(m);
        } else {
            let parent = match self.resolve_mod_path(module, global, &segs[..segs.len() - 1], edition < 2018, true) {
                Some(p) => p,
                None => return false,
            };
            tdef = self.lookup_in_container(parent, last, true);
            vdef = self.lookup_in_container(parent, last, false);
            if tdef.is_none() && vdef.is_none() {
                // macros only (e.g. `use crate::log;` for macro_rules) count as resolved
                if self.lookup_macro_in_container(parent, last) {
                    return true;
                }
                return false;
            }
        }
        if tdef.is_none() && vdef.is_none() {
            return false;
        }
        if self.syms.str(bind) == "_" {
            return true;
        }
        if let Some(d) = tdef {
            self.mods[module as usize].types.entry(bind).or_insert(d);
        }
        if let Some(d) = vdef {
            self.mods[module as usize].values.entry(bind).or_insert(d);
        }
        true
    }

    pub fn extern_crate(&self, module: u32, name: Sym) -> Option<u32> {
        let krate = self.mods[module as usize].krate;
        for (n, c) in &self.crates[krate as usize].deps {
            if *n == name {
                return Some(*c);
            }
        }
        None
    }

    /// Resolves a path to a container (module, enum, trait, type). `from_root`: 2015-style
    /// use paths are crate-relative.
    pub fn resolve_mod_path(&self, module: u32, global: bool, segs: &[(SegKind, Sym)], from_root: bool, in_use: bool) -> Option<DefId> {
        let krate = self.mods[module as usize].krate;
        let mut cur: Option<DefId> = None;
        for (i, (kind, name)) in segs.iter().enumerate() {
            let next = if i == 0 {
                match kind {
                    SegKind::Crate => Some(self.mods[self.crates[krate as usize].root_mod as usize].def),
                    SegKind::SelfValue => Some(self.mods[self.mods[module as usize].normal as usize].def),
                    SegKind::Super => {
                        let n = self.mods[module as usize].normal;
                        let p = self.mods[n as usize].parent_mod?;
                        Some(self.mods[self.mods[p as usize].normal as usize].def)
                    }
                    SegKind::SelfType => None,
                    SegKind::Ident => {
                        if global {
                            match self.extern_crate(module, *name) {
                                Some(c) => Some(self.mods[self.crates[c as usize].root_mod as usize].def),
                                None => {
                                    let r = self.crates[krate as usize].root_mod;
                                    self.lookup_in_module(r, *name, true, &mut Vec::new())
                                }
                            }
                        } else if from_root && in_use {
                            let r = self.crates[krate as usize].root_mod;
                            match self.lookup_in_module(r, *name, true, &mut Vec::new()) {
                                Some(d) => Some(d),
                                None => match self.extern_crate(module, *name) {
                                    Some(c) => Some(self.mods[self.crates[c as usize].root_mod as usize].def),
                                    None => None,
                                },
                            }
                        } else {
                            match self.lookup_in_scope(module, *name, true) {
                                Some(d) => Some(d),
                                None => match self.extern_crate(module, *name) {
                                    Some(c) => Some(self.mods[self.crates[c as usize].root_mod as usize].def),
                                    None => None,
                                },
                            }
                        }
                    }
                }
            } else {
                let c = cur?;
                match kind {
                    SegKind::Super => {
                        let d = self.def(c);
                        if d.kind != DefKind::Mod {
                            return None;
                        }
                        let p = self.mods[d.sub as usize].parent_mod?;
                        Some(self.mods[p as usize].def)
                    }
                    SegKind::SelfValue => Some(c),
                    _ => self.lookup_in_container(c, *name, true),
                }
            };
            cur = next;
            cur?;
        }
        cur
    }

    /// Name in a container: module (items, imports, globs), enum (variants), trait (items).
    pub fn lookup_in_container(&self, c: DefId, name: Sym, ns_type: bool) -> Option<DefId> {
        let d = self.def(c);
        match d.kind {
            DefKind::Mod => self.lookup_in_module(d.sub, name, ns_type, &mut Vec::new()),
            DefKind::Enum => {
                for &v in &self.variants[d.sub as usize] {
                    if self.def(v).name == name {
                        return Some(v);
                    }
                }
                None
            }
            DefKind::Trait => {
                for &v in &self.traits[d.sub as usize] {
                    if self.def(v).name == name {
                        return Some(v);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn lookup_macro_in_container(&self, c: DefId, name: Sym) -> bool {
        let d = self.def(c);
        if d.kind == DefKind::Mod {
            return self.mods[d.sub as usize].macros.contains_key(&name);
        }
        false
    }

    /// Items + imports of a module, then its globs.
    pub fn lookup_in_module(&self, m: u32, name: Sym, ns_type: bool, visiting: &mut Vec<u32>) -> Option<DefId> {
        let md = &self.mods[m as usize];
        let direct = if ns_type { md.types.get(&name) } else { md.values.get(&name) };
        if let Some(&d) = direct {
            return Some(d);
        }
        for x in visiting.iter() {
            if *x == m {
                return None;
            }
        }
        visiting.push(m);
        for &g in &md.globs {
            let gd = self.def(g);
            let r = match gd.kind {
                DefKind::Mod => self.lookup_in_module(gd.sub, name, ns_type, visiting),
                DefKind::Enum => {
                    if ns_type {
                        None
                    } else {
                        self.lookup_in_container(g, name, false)
                    }
                }
                _ => None,
            };
            if r.is_some() {
                return r;
            }
        }
        // modules stay marked for the whole query: a glob graph with diamonds is
        // searched once per module, not once per path
        None
    }

    /// Lexical lookup from a (possibly anonymous) module: block modules fall through to
    /// their parents; then the extern prelude, std prelude and primitive types.
    pub fn lookup_in_scope(&self, module: u32, name: Sym, ns_type: bool) -> Option<DefId> {
        let mut m = module;
        loop {
            if let Some(d) = self.lookup_in_module(m, name, ns_type, &mut Vec::new()) {
                return Some(d);
            }
            match self.mods[m as usize].lexical_parent {
                Some(p) => m = p,
                None => break,
            }
        }
        if ns_type {
            if let Some(c) = self.extern_crate(module, name) {
                return Some(self.mods[self.crates[c as usize].root_mod as usize].def);
            }
        }
        if let Some(pc) = self.prelude_crate {
            let root = self.crates[pc as usize].root_mod;
            if let Some(pn) = self.syms.get("prelude") {
                if let Some(pm) = self.mods[root as usize].types.get(&pn) {
                    let pmd = self.def(*pm);
                    if pmd.kind == DefKind::Mod {
                        if let Some(d) = self.lookup_in_module(pmd.sub, name, ns_type, &mut Vec::new()) {
                            return Some(d);
                        }
                    }
                }
            }
        }
        if ns_type {
            for i in 0..PRIMS.len() {
                if self.syms.get(PRIMS[i].0) == Some(name) {
                    return Some(self.prim_defs[i]);
                }
            }
        }
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    Fn,
    Struct,
    Union,
    Enum,
    Const,
    Static,
    TypeAlias,
    Trait,
    Impl,
    Mod,
    ForeignMod,
    Use,
    ExternCrate,
    MacroRules,
    Other,
}

fn item_tag(k: &ItemKind) -> Tag {
    match k {
        ItemKind::Fn(..) => Tag::Fn,
        ItemKind::Struct(..) => Tag::Struct,
        ItemKind::Union(..) => Tag::Union,
        ItemKind::Enum(..) => Tag::Enum,
        ItemKind::Const(..) => Tag::Const,
        ItemKind::Static(..) => Tag::Static,
        ItemKind::TypeAlias { .. } => Tag::TypeAlias,
        ItemKind::Trait { .. } => Tag::Trait,
        ItemKind::Impl { .. } => Tag::Impl,
        ItemKind::Mod(_) => Tag::Mod,
        ItemKind::ForeignMod(..) => Tag::ForeignMod,
        ItemKind::Use { .. } => Tag::Use,
        ItemKind::ExternCrate(_) => Tag::ExternCrate,
        ItemKind::MacroRules(_) => Tag::MacroRules,
        _ => Tag::Other,
    }
}

/// Blocks directly reachable from an expression (not through nested blocks' statements).
pub fn blocks_in_expr(ast: &Ast, e: ExprId, out: &mut Vec<BlockId>) {
    match &ast.expr(e).kind {
        ExprKind::Block(b, _) | ExprKind::Unsafe(b) | ExprKind::Async(_, b) | ExprKind::ConstBlock(b) | ExprKind::Loop(b, _) => {
            out.push(*b)
        }
        ExprKind::If(c, t, el) => {
            blocks_in_expr(ast, *c, out);
            out.push(*t);
            if let Some(x) = el {
                blocks_in_expr(ast, *x, out);
            }
        }
        ExprKind::While(c, b, _) => {
            blocks_in_expr(ast, *c, out);
            out.push(*b);
        }
        ExprKind::For(_, it, b, _) => {
            blocks_in_expr(ast, *it, out);
            out.push(*b);
        }
        ExprKind::Match(x, arms) => {
            blocks_in_expr(ast, *x, out);
            for a in arms {
                blocks_in_expr(ast, a.body, out);
            }
        }
        ExprKind::Closure { body, .. } => blocks_in_expr(ast, *body, out),
        ExprKind::Call(f, args) => {
            blocks_in_expr(ast, *f, out);
            for a in args {
                blocks_in_expr(ast, *a, out);
            }
        }
        ExprKind::MethodCall { recv, args, .. } => {
            blocks_in_expr(ast, *recv, out);
            for a in args {
                blocks_in_expr(ast, *a, out);
            }
        }
        ExprKind::Paren(x) | ExprKind::Unary(_, x) | ExprKind::AddrOf(_, _, x) | ExprKind::Try(x) | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::Cast(x, _) => {
            blocks_in_expr(ast, *x, out)
        }
        ExprKind::Binary(_, a, b) | ExprKind::Assign(a, b) | ExprKind::AssignOp(_, a, b) | ExprKind::Index(a, b) => {
            blocks_in_expr(ast, *a, out);
            blocks_in_expr(ast, *b, out);
        }
        ExprKind::Return(Some(x)) | ExprKind::Break(_, Some(x)) => blocks_in_expr(ast, *x, out),
        ExprKind::Tuple(v) | ExprKind::Array(v) => {
            for x in v {
                blocks_in_expr(ast, *x, out);
            }
        }
        ExprKind::Struct(_, fields, base) => {
            for f in fields {
                blocks_in_expr(ast, f.expr, out);
            }
            if let Some(b) = base {
                blocks_in_expr(ast, *b, out);
            }
        }
        _ => {}
    }
}

pub fn dir_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => ".",
    }
}

pub fn stem_of(path: &str) -> &str {
    let f = match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    };
    match f.rfind('.') {
        Some(i) => &f[..i],
        None => f,
    }
}
