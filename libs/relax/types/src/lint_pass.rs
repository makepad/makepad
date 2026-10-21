//! The lint passes of `makepad check`: rustc_lint's builtin lints over the checked crates'
//! definitions and typed bodies. Each pass is a port of the rustc pass named at it; they report
//! through `lint::emit_in` (levels from the attributes in scope) and never run in `makepad run`.

use crate::ast::*;
use crate::ast_visit::{self, Visitor};
use crate::lint::{self, Lint};
use crate::program::{DefId, DefKind, Program, NO_DEF};
use crate::tcx::Tcx;
use crate::typeck::{Body, Res};

// ---------------------------------------------------------------- item-level passes

/// Runs the passes over the definitions of the crates marked in `own`.
pub fn check_items(prog: &Program, tcx: &Tcx, own: &[bool]) {
    if !lint::enabled() { return; }
    crate::lint_early::check_items(prog, tcx, own);
    crate::lint_late::check_items(prog, tcx, own);
    for i in 0..prog.defs.len() {
        let def = prog.defs[i];
        if !own.get(def.krate as usize).copied().unwrap_or(false) { continue; }
        let d = DefId(i as u32);
        if lint::generated(prog, d) { continue; }
        nonstandard_style_item(prog, d);
    }
}

// ---------------------------------------------------------------- nonstandard_style

/// `char::to_titlecase`: the uppercase mapping except for the digraphs (Unicode SpecialCasing
/// keeps the Greek iota-subscript letters in their Lt form).
fn to_titlecase(c: char) -> Vec<char> {
    match c as u32 {
        0x1C4..=0x1C6 => vec!['\u{1C5}'],
        0x1C7..=0x1C9 => vec!['\u{1C8}'],
        0x1CA..=0x1CC => vec!['\u{1CB}'],
        0x1F1..=0x1F3 => vec!['\u{1F2}'],
        0x1F80..=0x1F87 | 0x1F90..=0x1F97 | 0x1FA0..=0x1FA7 => vec![char::from_u32(c as u32 + 8).unwrap_or(c)],
        0x1F88..=0x1F8F | 0x1F98..=0x1F9F | 0x1FA8..=0x1FAF | 0x1FBC | 0x1FCC | 0x1FFC => vec![c],
        0x10D0..=0x10FA | 0x10FD..=0x10FF => vec![c],
        0x1FB3 => vec!['\u{1FBC}'],
        0x1FC3 => vec!['\u{1FCC}'],
        0x1FF3 => vec!['\u{1FFC}'],
        _ => c.to_uppercase().collect(),
    }
}

/// rustc_lint nonstandard_style.rs: `char_has_case`.
fn char_has_case(c: char) -> bool {
    !c.to_lowercase().eq(to_titlecase(c))
}

fn changes_when_titlecased(c: char) -> bool {
    to_titlecase(c) != [c]
}

fn not_camel_case(s: &str) -> bool {
    let mut last = '\0';
    s.chars().any(|snd| {
        let fst = std::mem::replace(&mut last, snd);
        match (fst, snd) {
            ('_', '_') => return true,
            ('_', _) if char_has_case(snd) => return true,
            (_, '_') if char_has_case(fst) => return true,
            _ => snd.is_uppercase() && changes_when_titlecased(snd),
        }
    })
}

fn is_upper_camel_case(name: &str) -> bool {
    let name = name.trim_matches('_');
    let Some(first) = name.chars().next() else { return true };
    !(changes_when_titlecased(first) || not_camel_case(name))
}

fn is_snake_case(ident: &str) -> bool {
    if ident.is_empty() { return true; }
    let ident = ident.trim_start_matches('\'');
    let ident = ident.trim_matches('_');
    if ident.contains("__") { return false; }
    ident.chars().all(|c| c.to_lowercase().eq([c]))
}

/// Where a naming lint reports: the scope definition, extra attributes of the named node and
/// the file.
struct Site<'a> {
    prog: &'a Program,
    def: DefId,
    file: u32,
    inner: &'a [Attr],
}

impl Site<'_> {
    fn emit(&self, lint: &Lint, id: Ident, msg: String) {
        if lint::external(self.prog, self.file, id.lo) { return; }
        lint::emit_in(self.prog, self.def, self.inner, lint, self.file, id.lo, &msg);
    }
    fn text(&self, id: Ident) -> &str { self.prog.files[self.file as usize].text(id) }

    fn camel(&self, sort: &str, id: Ident) {
        let name = self.text(id);
        if !is_upper_camel_case(name) {
            let msg = format!("{} `{}` should have an upper camel case name", sort, name);
            self.emit(&lint::NON_CAMEL_CASE_TYPES, id, msg);
        }
    }

    fn snake(&self, sort: &str, id: Ident) {
        let name = self.text(id);
        if !is_snake_case(name) {
            let msg = format!("{} `{}` should have a snake case name", sort, name);
            self.emit(&lint::NON_SNAKE_CASE, id, msg);
        }
    }

    fn upper(&self, sort: &str, id: Ident) {
        let name = self.text(id);
        if !name.chars().all(|c| c.to_uppercase().eq([c])) {
            let msg = format!("{} `{}` should have an upper case name", sort, name);
            self.emit(&lint::NON_UPPER_CASE_GLOBALS, id, msg);
        }
    }
}

fn nonstandard_style_item(prog: &Program, d: DefId) {
    let def = prog.def(d);
    let kind = def.kind;
    if !matches!(kind, DefKind::Mod | DefKind::Fn | DefKind::AssocFn | DefKind::Struct | DefKind::Union | DefKind::Enum
        | DefKind::TypeAlias | DefKind::Trait | DefKind::Static | DefKind::Const | DefKind::AssocConst | DefKind::AssocTy | DefKind::Impl) { return; }
    // the crate root module has no item of its own
    if kind == DefKind::Mod && def.parent == NO_DEF { return; }
    let file = def.file;
    let ast = &prog.files[file as usize].ast;
    let item = *ast.item(def.item);
    let attrs = ast.list(item.attrs);
    let site = Site { prog, def: d, file, inner: &[] };
    let parent_kind = if def.parent != NO_DEF { prog.def(def.parent).kind } else { DefKind::Mod };
    // `is_inherent`: an assoc item of an inherent impl
    let impl_trait = |p: DefId| -> Option<bool> {
        let pd = prog.def(p);
        let pa = &prog.files[pd.file as usize].ast;
        pa.as_impl(&pa.item(pd.item).kind).map(|i| i.trait_.is_some())
    };
    let no_mangle = || lint::has_attr(prog, file, attrs, "no_mangle");
    if kind == DefKind::Mod {
        // the crate root's and a block's module have no item of their own
        if def.parent != NO_DEF && prog.block_owner.get(&def.sub).is_none() && matches!(item.kind, ItemKind::Mod(_)) { site.snake("module", item.name); }
        return;
    }
    let has_repr_c = attrs.iter().any(|a| lint::attr_path(prog, file, a) == b"repr" && lint::attr_args(prog, file, a).iter().any(|w| w == "C"));
    match kind {
        DefKind::Fn | DefKind::AssocFn => {
            let ItemKind::Fn(sig, _, _) = item.kind else { return };
            let sig = *ast.fn_sig(sig);
            let rust_abi = match sig.abi { None => true, Some(None) => false, Some(Some(a)) => site.text(a).trim_matches('"') == "Rust" };
            if kind == DefKind::Fn {
                if rust_abi || !no_mangle() { site.snake("function", item.name); }
            } else if parent_kind == DefKind::Trait {
                site.snake("trait method", item.name);
            } else if parent_kind == DefKind::Impl && impl_trait(def.parent) == Some(false) {
                if rust_abi || !no_mangle() { site.snake("method", item.name); }
            }
            // a trait method without a body names its parameters here (they are no patterns of a body)
            if parent_kind == DefKind::Trait && !matches!(item.kind, ItemKind::Fn(_, Some(_), _)) && sig.lazy_body.is_none() {
                for p in ast.list(sig.params) { snake_pat_idents(&site, ast, p.pat); }
            }
        }
        DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias => {
            if !has_repr_c {
                site.camel("type", item.name);
            }
        }
        DefKind::Trait => {
            if !has_repr_c { site.camel("trait", item.name); }
        }
        DefKind::AssocTy => {
            // (the associated type a `fn f() -> impl Trait` of a trait stands for has the fn's item)
            if !matches!(item.kind, ItemKind::TypeAlias(_)) { return; }
            if parent_kind == DefKind::Trait || (parent_kind == DefKind::Impl && impl_trait(def.parent) == Some(false)) {
                site.camel("associated type", item.name);
            }
        }
        DefKind::Static => {
            if !no_mangle() { site.upper("static variable", item.name); }
        }
        DefKind::Const => site.upper("constant", item.name),
        DefKind::AssocConst => {
            if parent_kind == DefKind::Trait || (parent_kind == DefKind::Impl && impl_trait(def.parent) == Some(false)) {
                site.upper("associated constant", item.name);
            }
        }
        _ => {}
    }
    // generic parameters
    if let Some(g) = ast.kind_generics(&item.kind) {
        for p in ast.list(g.params) {
            let inner = ast.list(p.attrs);
            let s = Site { prog, def: d, file, inner };
            match p.kind {
                GenericParamKind::Type(..) => s.camel("type parameter", p.name),
                GenericParamKind::Lifetime(_) => s.snake("lifetime", p.name),
                GenericParamKind::Const(..) => s.upper("const parameter", p.name),
            }
        }
    }
    // fields and variants
    let fields = |list: List<FieldDef>, inner_extra: &[Attr]| {
        for f in ast.list(list) {
            if let Some(n) = f.name {
                let mut inner = inner_extra.to_vec();
                inner.extend_from_slice(ast.list(f.attrs));
                let s = Site { prog, def: d, file, inner: &inner };
                s.snake("structure field", n);
            }
        }
    };
    match item.kind {
        ItemKind::Struct(r) => if let VariantData::Struct(l) | VariantData::Tuple(l) = ast.row(r).data { fields(l, &[]) },
        ItemKind::Union(r) => fields(ast.row(r).fields, &[]),
        ItemKind::Enum(r) => {
            for v in ast.list(ast.row(r).variants) {
                let vattrs = ast.list(v.attrs);
                if !has_repr_c {
                    Site { prog, def: d, file, inner: vattrs }.camel("variant", v.name);
                }
                if let VariantData::Struct(l) | VariantData::Tuple(l) = v.data { fields(l, vattrs) }
            }
        }
        _ => {}
    }
}

/// `check_pat`'s binding names in a pattern without a body (a trait method's parameters).
fn snake_pat_idents(site: &Site, ast: &Ast, p: PatId) {
    struct W<'s, 'a> { site: &'s Site<'a>, ast: &'a Ast }
    impl<'s, 'a> Visitor<'a> for W<'s, 'a> {
        fn ast(&self) -> &'a Ast { self.ast }
        fn pat(&mut self, p: PatId) {
            if let Pat::Ident { name, .. } = *self.ast.pat(p) { self.site.snake("variable", name); }
            ast_visit::walk_pat(self, p);
        }
    }
    // SAFETY of lifetimes: the site and the tree outlive the walk
    let ast: &Ast = unsafe { &*(ast as *const Ast) };
    ast_visit::Visitor::pat(&mut W { site, ast }, p);
}

// ---------------------------------------------------------------- body passes

/// Runs the body passes over one typed body owner.
pub fn check_body(prog: &Program, tcx: &Tcx, body: &Body) {
    if !lint::enabled() { return; }
    crate::lint_dead::collect_body(prog, tcx, body);
    // the output of derive and attribute macros is not judged
    if lint::generated(prog, body.def) { return; }
    crate::lint_late::check_body(prog, tcx, body);
    let d = prog.def(body.def);
    let pf = &prog.files[body.file as usize];
    let ast = &pf.ast;
    let mut w = Late { prog, body, ast, stack: Vec::new() };
    match ast.item(d.item).kind {
        ItemKind::Fn(s, _, _) => {
            let Ok(Some(block)) = prog.fn_body(body.file, d.item) else { return };
            for p in ast.list(ast.fn_sig(s).params) {
                w.enter_attrs(p.attrs);
                w.pat(p.pat);
                w.leave_attrs(p.attrs);
            }
            w.block(block);
            crate::lint_unreachable::check(prog, tcx, body, Some(block), None);
            let params: Vec<(PatId, List<Attr>)> = ast.list(ast.fn_sig(s).params).iter().map(|p| (p.pat, p.attrs)).collect();
            crate::lint_liveness::check(prog, tcx, body, &params, Some(block), None);
        }
        ItemKind::Const(..) | ItemKind::Static(..) => {
            let Ok(Some(e)) = prog.const_body(body.file, d.item) else { return };
            w.expr(e);
            crate::lint_unreachable::check(prog, tcx, body, None, Some(e));
            crate::lint_liveness::check(prog, tcx, body, &[], None, Some(e));
        }
        _ => {}
    }
}

struct Late<'a> {
    prog: &'a Program,
    body: &'a Body,
    ast: &'a Ast,
    /// the attributes of the enclosing statements, arms, ..., outermost first
    stack: Vec<Attr>,
}

impl<'a> Late<'a> {
    fn site(&self) -> Site<'_> {
        Site { prog: self.prog, def: self.body.def, file: self.body.file, inner: &self.stack }
    }
}

impl<'a> Visitor<'a> for Late<'a> {
    fn ast(&self) -> &'a Ast { self.ast }
    fn enter_attrs(&mut self, attrs: List<Attr>) { crate::ast_visit::push_attrs(&mut self.stack, self.ast, attrs); }
    fn leave_attrs(&mut self, attrs: List<Attr>) { crate::ast_visit::pop_attrs(&mut self.stack, self.ast, attrs); }

    fn pat(&mut self, p: PatId) {
        match *self.ast.pat(p) {
            // nonstandard_style check_pat: a binding (a field pattern's shorthand names the field
            // already) and a constant that looks like a binding
            Pat::Ident { name, .. } => {
                // an identifier pattern resolved to a constant or unit struct is a path pattern
                if self.body.pat_local.get(&p.0).is_some() {
                    self.site().snake("variable", name);
                } else if let Some(Res::Def(c)) = self.body.pat_res.get(&p.0) {
                    if self.prog.def(*c).kind == DefKind::Const { self.site().upper("constant in pattern", name); }
                }
            }
            Pat::Struct(_, fs, _) => {
                for f in self.ast.list(fs) {
                    self.enter_attrs(f.attrs);
                    if !(f.shorthand && matches!(self.ast.pat(f.pat), Pat::Ident { .. })) { self.pat(f.pat); }
                    self.leave_attrs(f.attrs);
                }
                return;
            }
            Pat::Path(path) => {
                if path.segs.len() == 1 && path.no_qself() {
                    if let Some(Res::Def(c)) = self.body.pat_res.get(&p.0) {
                        if self.prog.def(*c).kind == DefKind::Const {
                            let seg = self.ast.segs(path.segs)[0];
                            self.site().upper("constant in pattern", seg.name);
                        }
                    }
                }
            }
            _ => {}
        }
        ast_visit::walk_pat(self, p);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_names() {
        assert!(is_snake_case("foo_bar"));
        assert!(is_snake_case("_foo"));
        assert!(is_snake_case("'a"));
        assert!(!is_snake_case("fooBar"));
        assert!(!is_snake_case("foo__bar"));
    }

    #[test]
    fn camel_case_names() {
        assert!(is_upper_camel_case("FooBar"));
        assert!(is_upper_camel_case("_Foo"));
        assert!(is_upper_camel_case("X86_64"));
        assert!(!is_upper_camel_case("foo_bar"));
        assert!(!is_upper_camel_case("Foo__Bar"));
    }
}
