//! Semantic editor queries over the same Program/Tcx as checking. Coordinates are 1-based
//! UTF-8 byte columns, mapped through ParsedFile's J1 diagnostic source mapping.
use crate::ast::{self, ItemKind, ExprKind, VariantData};
use crate::program::{DefId, DefKind, Program};
use crate::types::{TyId, TyKind};
use crate::serve::Session;
use crate::serve_rpc::{quote as q, string};
use crate::json::Json;

fn kind(k: DefKind) -> &'static str {
    match k { DefKind::Struct|DefKind::Union => "struct", DefKind::Enum => "enum", DefKind::Trait => "trait", DefKind::Fn|DefKind::AssocFn|DefKind::ForeignFn => "fn", DefKind::Mod => "mod", DefKind::TypeAlias|DefKind::AssocTy => "type alias", DefKind::Variant => "variant", _ => "const" }
}
fn ty(s: &Session, t: TyId) -> String { crate::typeck::ty_to_string(false, &s.unit.prog,&s.unit.tcx,t) }
fn loc(p: &Program, file: u32, pos: u32) -> String {
    let location = p.source_location(file, pos);
    let (line,col) = location.source.line_col(location.span.lo);
    let (el,ec) = location.source.line_col(location.span.hi);
    format!("{{\"file\":{},\"line\":{line},\"col\":{col},\"end_line\":{el},\"end_col\":{ec}}}",q(location.source.path()))
}
fn definition(p: &Program, d: DefId) -> String {
    let def = p.def(d);
    if def.item.0 == u32::MAX { return "null".into(); }
    loc(p,def.file,p.files[def.file as usize].ast.item(def.item).name.lo)
}
fn docs_at(p: &Program, file: u32, pos: u32, attrs: ast::List<ast::Attr>) -> String {
    let pf = &p.files[file as usize];
    let mut out = Vec::new();
    // Doc comments are trivia in Relax's lexer; retain their authored text here.
    let location = p.source_location(file, pos);
    let source = location.source.bytes();
    let at = location.span.lo as usize;
    let prefix = String::from_utf8_lossy(&source[..at.min(source.len())]);
    let mut preceding = Vec::new();
    for line in prefix.lines().rev() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("#[") || matches!(line,"pub"|"pub struct"|"struct"|"pub fn"|"fn") { continue; }
        if let Some(doc) = line.strip_prefix("///") { preceding.push(doc.strip_prefix(' ').unwrap_or(doc).to_string()); }
        else { break; }
    }
    preceding.reverse(); out.extend(preceding);
    for a in pf.ast.list(attrs) {
        let tokens: Vec<_> = (a.toks.lo..a.toks.hi).map(|i|pf.tok_text(i)).collect();
        if tokens.first() == Some(&"doc") && tokens.get(1) == Some(&"=") {
            let text = tokens[2..].join("");
            if let Ok(Json::Str(value)) = crate::json::parse(text.as_bytes()) { out.push(value); }
            else if let Some(at) = text.find('"') { if let Some(end) = text.rfind('"') { if end > at { out.push(text[at+1..end].into()); } } }
        }
    }
    out.join("\n")
}
fn docs(p: &Program,d: DefId) -> String {
    let def = p.def(d); if def.item.0 == u32::MAX { return String::new(); }
    let pf = &p.files[def.file as usize]; let it = pf.ast.item(def.item);
    docs_at(p,def.file,it.lo,it.attrs)
}
fn signature_args(s: &Session,d: DefId,args: &[TyId]) -> String {
    let p = &s.unit.prog;
    if let Some(sig) = s.unit.tcx.fn_sig(&s.unit.prog, d) {
        let params = sig.params.iter().map(|t|ty(s,s.unit.tcx.tys.subst(*t,args))).collect::<Vec<_>>().join(", ");
        format!("{}fn {}({}) -> {}",if sig.header.safety == crate::types::Safety::Unsafe { "unsafe " } else { "" },p.name(d),params,ty(s,s.unit.tcx.tys.subst(sig.ret,args)))
    } else if let Some(t) = s.unit.tcx.const_ty(&s.unit.prog, d) { ty(s,t) }
    else if p.def(d).kind == DefKind::TypeAlias { ty(s,s.unit.tcx.alias_ty(&crate::tcx::EvalCx::new(p, &s.unit.tcx, crate::tcx::TypingEnv::fully_monomorphized(&p), &crate::tcx::EvalState::new()), p,d)) }
    else { p.def_path(d) }
}
fn resolve(p: &Program, scope: u32, path: &str) -> Option<DefId> {
    let mut parts = path.trim_start_matches("::").split("::").peekable();
    let first = parts.next()?;
    let mut d = match first {
        "crate" => p.mods[p.crates[p.mods[scope as usize].krate as usize].root_mod as usize].def,
        "self" => p.mods[scope as usize].def,
        "super" => p.mods[p.mods[scope as usize].parent_mod? as usize].def,
        _ => { let sym = p.syms.get(first)?; p.lookup_in_scope(scope,sym,true).or_else(||p.lookup_in_scope(scope,sym,false))? }
    };
    for part in parts { let name = p.syms.get(part)?; d = p.lookup_in_container(d,name,true).or_else(||p.lookup_in_container(d,name,false))?; }
    Some(d)
}
fn field_defs(p: &Program,d: DefId,variant: usize) -> &[ast::FieldDef] {
    let def = p.def(d); let pf = &p.files[def.file as usize];
    let data = match pf.ast.item(def.item).kind {
        ItemKind::Struct(r) => pf.ast.row(r).data,
        ItemKind::Union(r) => return pf.ast.list(pf.ast.row(r).fields),
        ItemKind::Enum(r) => match pf.ast.list(pf.ast.row(r).variants).get(variant) { Some(v) => v.data, None => return &[] },
        _ => return &[],
    };
    match data { VariantData::Unit => &[], VariantData::Tuple(fs)|VariantData::Struct(fs) => pf.ast.list(fs) }
}
fn fields(s: &Session,d: DefId,variant: usize,fs: &[crate::tcx::FieldT]) -> String {
    let p = &s.unit.prog; let ast = field_defs(p,d,variant);
    fs.iter().enumerate().map(|(i,f)| {
        let docs = ast.get(i).map(|f|docs_at(p,p.def(d).file,f.name.map_or(f.ty_span.0,|n|n.lo),f.attrs)).unwrap_or_default();
        let name = if ast.get(i).is_some_and(|f|f.name.is_none()) { i.to_string() } else { p.syms.str(f.name).into() };
        format!("{{\"name\":{},\"type\":{},\"docs\":{}}}",q(&name),q(&ty(s,f.ty)),q(&docs))
    }).collect::<Vec<_>>().join(",")
}
pub fn schema(s: &mut Session,path: &str) -> Result<String,String> {
    s.reach_query_files()?;
    if !s.query_ready() { return Err("fix parse errors before querying this package".into()); }
    let p = &s.unit.prog; let tcx = &s.unit.tcx;
    let root = p.crates[s.root as usize].root_mod;
    let d = resolve(p,root,path).ok_or_else(||format!("cannot resolve `{path}` in the crate root"))?;
    let def = p.def(d);
    if def.item.0 == u32::MAX {
        return Ok(format!("{{\"kind\":{},\"path\":{},\"generics\":[],\"fields\":[],\"variants\":[],\"methods\":[],\"traits\":[],\"docs\":\"\",\"definition\":null}}",q(kind(def.kind)),q(&p.def_path(d))));
    }
    let pf = &p.files[def.file as usize]; let it = pf.ast.item(def.item);
    let generics = match it.kind {
        ItemKind::Fn(sig,_,_) => pf.ast.fn_sig(sig).generics,
        k @ (ItemKind::Struct(_)|ItemKind::Enum(_)|ItemKind::Union(_)|ItemKind::Trait(_)|ItemKind::TypeAlias(_)) => *pf.ast.kind_generics(&k).unwrap(),
        _ => ast::Generics::default(),
    };
    let gs = pf.ast.list(generics.params).iter().map(|g|q(pf.text(g.name))).collect::<Vec<_>>().join(",");
    let mut fs = String::new(); let mut vs = Vec::new();
    if let Some(adt) = tcx.adts.get(&d) {
        if adt.is_enum { for (i,v) in adt.variants.iter().enumerate() { vs.push(format!("{{\"name\":{},\"fields\":[{}]}}",q(p.syms.str(v.name)),fields(s,d,i,&v.fields))); } }
        else if let Some(v) = adt.variants.first() { fs = fields(s,d,0,&v.fields); }
    }
    let mut methods = Vec::new(); let mut traits = Vec::new();
    let mut method_args = std::collections::HashMap::<DefId, Vec<TyId>>::new();
    let target = tcx.adts.get(&d).map(|a|tcx.tys.intern(TyKind::Adt(d,tcx.tys.mk_list(&(0..a.n_generics).map(|i|tcx.tys.intern(TyKind::Param(i))).collect::<Vec<_>>()), crate::types::RegionArgsId::EMPTY))
);
    for imp in &p.impls {
        let idef = p.def(imp.def);
        if { let a=&p.files[idef.file as usize].ast; a.as_impl(&a.item(idef.item).kind).is_some_and(|i| i.negative) } { continue; }
        let Some(&pattern) = tcx.impl_self.get(&imp.def) else { continue; };
        // rustc_hir_typeck/src/method/probe.rs:2034-2085: instantiate the
        // impl header, relate Self, and evaluate its predicates in native inference.
        let env = tcx.param_env_id(p, d);
        let infcx = crate::infer::InferCtxt::new(tcx, crate::tcx::TypingMode::PostAnalysis, crate::tcx::TypingEnv::fully_monomorphized(p).limit);
        let full = infcx.fresh_args_for_item(p, imp.def);
        let instantiated = tcx.instantiate_ty(p, imp.def, &full, pattern);
        let mut ocx = crate::next_solver::fulfill::ObligationCtxt::new(&infcx, p);
        let matched = target.is_some_and(|target| ocx.eq(&env, instantiated, target).is_ok());
        for clause in tcx.clauses_of(p, imp.def) {
            ocx.register_obligation(crate::next_solver::Goal { param_env: env.clone(), predicate: tcx.instantiate_predicate(p, imp.def, &full, &clause) });
        }
        let matched = matched && ocx.evaluate_obligations_error_on_ambiguity().is_ok();
        let mut binds = Vec::new();
        for arg in &full {
            match *arg {
                crate::consts::GenericArg::Type(t) => { let t = infcx.deeply_resolve_ignoring_regions(t); binds.push((!tcx.tys.has_infer(t)).then_some(t)); }
                crate::consts::GenericArg::Const(c) => { let t = infcx.deeply_resolve_ignoring_regions(tcx.tys.intern(TyKind::Const(c))); binds.push((!tcx.tys.has_infer(t)).then_some(t)); }
                crate::consts::GenericArg::Lifetime(_) => {}
            }
        }
        let blanket = matches!(tcx.tys.kind(pattern), TyKind::Param(_)) && matched && binds.iter().all(Option::is_some) && tcx.impl_trait.contains_key(&imp.def);
        let direct = matches!(tcx.tys.kind(pattern),TyKind::Adt(target,_, _) if *target == d) && matched;
        if direct || blanket {
            let args: Vec<_> = binds.into_iter().enumerate().map(|(i,t)|t.unwrap_or_else(||tcx.tys.intern(TyKind::Param(i as u32)))).collect();
            if let Some(tr) = tcx.impl_trait.get(&imp.def) { traits.push(p.def_path(*tr)); }
            for &m in &imp.items {
                if matches!(p.def(m).kind,DefKind::Fn|DefKind::AssocFn) { methods.push(m); method_args.insert(m,args.clone()); }
            }
            if let Some(tr) = tcx.impl_trait.get(&imp.def) {
                let mut trait_args = vec![target.unwrap_or(pattern)];
                trait_args.extend(tcx.impl_trait_args.get(&imp.def).into_iter().flatten().map(|t|tcx.tys.subst(*t,&args)));
                for &m in &p.traits[p.def(*tr).sub as usize] {
                    if matches!(p.def(m).kind,DefKind::Fn|DefKind::AssocFn) && !imp.items.iter().any(|i|p.name(*i)==p.name(m)) {
                        methods.push(m); method_args.insert(m,trait_args.clone());
                    }
                }
            }
        }
    }
    if def.kind == DefKind::Trait { methods.extend(p.traits[def.sub as usize].iter().copied().filter(|d|matches!(p.def(*d).kind,DefKind::Fn|DefKind::AssocFn))); }
    methods.sort_by_key(|d|p.def_path(*d)); methods.dedup(); traits.sort(); traits.dedup();
    let methods = methods.iter().map(|m|format!("{{\"name\":{},\"signature\":{},\"docs\":{}}}",q(p.name(*m)),q(&signature_args(s,*m,method_args.get(m).map_or(&[],Vec::as_slice))),q(&docs(p,*m)))).collect::<Vec<_>>().join(",");
    Ok(format!("{{\"kind\":{},\"path\":{},\"generics\":[{gs}],\"fields\":[{fs}],\"variants\":[{}],\"methods\":[{methods}],\"traits\":[{}],\"docs\":{},\"definition\":{}}}",q(kind(def.kind)),q(&p.def_path(d)),vs.join(","),traits.iter().map(|t|q(t)).collect::<Vec<_>>().join(","),q(&docs(p,d)),definition(p,d)))
}
fn hover(s: &Session,d: DefId) -> String { hover_args(s,d,&[]) }
fn hover_args(s: &Session,d: DefId,args: &[TyId]) -> String {
    let p = &s.unit.prog;
    format!("{{\"kind\":{},\"name\":{},\"type\":{},\"docs\":{},\"definition\":{}}}",q(kind(p.def(d).kind)),q(p.name(d)),q(&signature_args(s,d,args)),q(&docs(p,d)),definition(p,d))
}
fn answer(value: String, definition: String, only: bool) -> String { if only { if definition == "null" { "[]".into() } else { format!("[{definition}]") } } else { value } }
fn hit(p: &Program, file: u32, lo: u32, path: &str, at: u32) -> bool {
    let loc = p.source_location(file, lo);
    loc.source.path() == path && loc.span.lo <= at && at < loc.span.hi
}

fn source_extent(p: &Program, file: u32, lo: u32, hi: u32, path: &str, at: u32) -> Option<u32> {
    let pf = &p.files[file as usize];
    if !pf.token_input && p.file_paths[file as usize] != path { return None; }
    if pf.token_input {
        // An imported-origin token can be inside a function whose endpoints name another
        // file. Query the authored spans of its tokens, rather than comparing those offsets.
        return pf.token_origins.iter().chain(pf.parts().flat_map(|p| p.token_origins.iter()))
            .filter(|o| lo <= o.lo && o.lo < hi && o.source.lo <= at && at < o.source.hi)
            .filter(|o| hit(p, file, o.lo, path, at)).map(|o| o.source.hi - o.source.lo).min();
    }
    let start = p.source_location(file, lo);
    let end = p.source_location(file, hi.saturating_sub(1));
    (start.source.path() == path && end.source.path() == path && start.span.lo <= at && at <= end.span.lo)
        .then(|| end.span.lo.saturating_sub(start.span.lo))
}

pub fn position(s: &mut Session,path: &str,params: &Json,only: bool) -> Result<String,String> {
    if s.file(path).is_none() { s.reach_query_files()?; }
    if !s.query_ready() { return Err("fix parse errors before querying this package".into()); }
    let line = params.get("line").ok_or("line required")?.num(); let col = params.get("col").ok_or("col required")?.num();
    if line < 1.0 || col < 1.0 || line.fract()!=0.0 || col.fract()!=0.0 { return Err("line/col must be positive 1-based integers".into()); }
    let file = s.file(path);
    let p = &s.unit.prog;
    let query_path = file.map_or(path, |f| p.file_paths[f as usize].as_str());
    let registered = if file.is_none() { p.sources.find(query_path).map(|id| p.sources.get(id)).transpose()? } else { None };
    let pf = file.map(|f| &p.files[f as usize]);
    let src = if let Some(pf) = pf { &pf.src[..pf.src_len as usize] }
        else if let Some(source) = &registered { &source.src[..] } else { return Err("file is not loaded".into()); };
    let edition = pf.map_or(2021, |pf| pf.edition);
    let mut offset = 0usize;
    for _ in 1..line as usize { offset += src.get(offset..).and_then(|s|s.iter().position(|b|*b==b'\n')).ok_or("line out of range")?+1; }
    let end = src[offset..].iter().position(|b|*b==b'\n').map_or(src.len(),|n|offset+n);
    let at = offset.checked_add(col as usize-1).ok_or("column out of range")?;
    if at >= end { return Err("column out of range".into()); }
    let mut tokens = Vec::new();
    let lexed = crate::lexer::lex(src,edition,&mut tokens);
    if pf.is_some_and(|pf| !pf.token_input) { lexed.map_err(|e|format!("cannot query unparsed source: {}",e.msg))?; }
    let token = tokens.iter().find(|t|t.lo <= at as u32 && (at as u32) < t.hi);
    if pf.is_some_and(|pf| !pf.token_input) && token.is_none() { return Err("no token at position".into()); }
    let name = token.and_then(|t| std::str::from_utf8(&src[t.lo as usize..t.hi as usize]).ok()).unwrap_or("");
    let at = at as u32;
    let mut owners: Vec<_> = p.defs.iter().enumerate().filter(|(_,d)|matches!(d.kind,DefKind::Fn|DefKind::AssocFn)).filter_map(|(i,d)| {
        let pf = &p.files[d.file as usize];
        let it = pf.ast.item(d.item);
        source_extent(p, d.file, it.lo, it.hi, query_path, at).map(|n| (n, DefId(i as u32)))
    }).collect(); owners.sort_by_key(|v|v.0);
    if let Some((_,owner)) = owners.first() {
        let args = crate::borrowck::identity_args(&s.unit.tcx, &s.unit.prog,*owner).unwrap_or_default();
        // J2 intentionally trims body tables. Recheck exactly the enclosing function, with
        // its generic identity arguments; speculative query diagnostics stay private.
        let state = crate::tcx::EvalState::new();
        let cx = crate::tcx::EvalCx::new(p, &s.unit.tcx, crate::tcx::TypingEnv::fully_monomorphized(&p), &state);
        let body = s.unit.tcx.with_errors(&cx, ||crate::typeck::instantiate_body(&cx,p,&s.unit.tcx,*owner,&args)).0;
        let pf = &p.files[body.file as usize];
        let local = |index: u32| -> String {
            let l = &body.locals[index as usize];
            let location = if l.pat == u32::MAX {
                let it = pf.ast.item(p.def(*owner).item);
                pf.toks.iter().find(|t|t.lo >= it.lo && t.hi <= it.hi && pf.bytes(t.lo,t.hi) == b"self").map_or("null".into(),|t|loc(p,body.file,t.lo))
            } else { match pf.ast.pat(ast::PatId(l.pat)) { ast::Pat::Ident { name,.. } => loc(p,body.file,name.lo), _ => "null".into() } };
            let value = format!("{{\"kind\":\"local\",\"name\":{},\"type\":{},\"docs\":\"\",\"definition\":{location}}}",q(p.syms.str(l.name)),q(&ty(s,l.ty)));
            answer(value,location,only)
        };
        for (index,l) in body.locals.iter().enumerate() {
            if l.pat != u32::MAX && (pf.token_input || p.syms.str(l.name) == name) && matches!(pf.ast.pat(ast::PatId(l.pat)), ast::Pat::Ident { name,.. } if hit(p,body.file,name.lo,query_path,at)) { return Ok(local(index as u32)); }
        }
        for i in 0..body.expr_ty.len() {
            let id = body.expr_at(i); let e = pf.ast.expr(id);
            match e.kind {
                ExprKind::MethodCall(_, row) if hit(p,body.file,pf.ast.row(row).name.lo,query_path,at) => {
                    if let Some(m) = body.methods.get(&id.0) { return Ok(answer(hover_args(s,m.def,s.unit.tcx.tys.list(m.args)),definition(p,m.def),only)); }
                }
                ExprKind::Field(recv,n) if hit(p,body.file,n.lo,query_path,at) => {
                    if let Some((derefs,index)) = body.field_idx.get(&id.0) {
                        let mut t = body.ty(recv);
                        for step in 0..*derefs as usize {
                            t = match s.unit.tcx.tys.kind(t) {
                                TyKind::Ref(_,inner, _)|TyKind::Ptr(_,inner) => *inner,
                                _ => {
                                    let Some((method,args)) = &body.ov_deref(id.0,step) else { break; };
                                    let Some(sig) = s.unit.tcx.fn_sig(&s.unit.prog, *method) else { break; };
                                    let ret = s.unit.tcx.tys.subst(sig.ret,s.unit.tcx.tys.list(*args));
                                    match s.unit.tcx.tys.kind(ret) { TyKind::Ref(_,inner, _) => s.unit.tcx.try_normalize_erasing_regions(p,cx.typing_env,*inner).unwrap_or(*inner), _ => break }
                                }
                            };
                        }
                        for _ in 0..32 {
                            match s.unit.tcx.tys.kind(t) {
                                TyKind::Ref(_,inner, _)|TyKind::Ptr(_,inner) => t = *inner,
                                TyKind::Adt(d,_, _) => {
                                    let def = p.def(*d);
                                    if let Some(f) = field_defs(p,*d,0).get(*index as usize) {
                                        let pos = f.name.map_or(f.ty_span.0,|n|n.lo); let location = loc(p,def.file,pos);
                                        let value = format!("{{\"kind\":\"field\",\"name\":{},\"type\":{},\"docs\":{},\"definition\":{location}}}",q(name),q(&ty(s,body.ty(id))),q(&docs_at(p,def.file,pos,f.attrs)));
                                        return Ok(answer(value,location,only));
                                    } break;
                                }
                                _ => break,
                            }
                        }
                    }
                }
                ExprKind::Path(path) if pf.ast.segs(path.segs).last().is_some_and(|seg|hit(p,body.file,seg.name.lo,query_path,at)) => {
                    if let Some(r) = body.res.get(&id.0) {
                        match r { crate::typeck::Res::Local(l) => return Ok(local(*l)), crate::typeck::Res::Def(d) => return Ok(answer(hover(s,*d),definition(p,*d),only)), _ => {} }
                    }
                }
                _ => {},
            }
        }
    }
    let scope = owners.first().map(|(_,d)|p.def(*d).scope).unwrap_or_else(|| {
        p.defs.iter().filter(|d|d.item.0!=u32::MAX).filter_map(|d| {
            let it=p.files[d.file as usize].ast.item(d.item);
            source_extent(p,d.file,it.lo,it.hi,query_path,at).map(|n| (n,if d.kind == DefKind::Mod { d.sub } else { d.scope }))
        }).min_by_key(|v|v.0).map_or(p.crates[s.root as usize].root_mod,|v|v.1)
    });
    // Use a qualified prefix through this token, so `mod::Type` never becomes a global
    // short-name search. Declaration names resolve directly to their collected definition.
    for (i,d) in p.defs.iter().enumerate() {
        if d.item.0 == u32::MAX { continue; }
        let pf = &p.files[d.file as usize];
        if !pf.token_input && p.file_paths[d.file as usize] != query_path { continue; }
        if (pf.token_input || p.name(DefId(i as u32))==name) && hit(p,d.file,pf.ast.item(d.item).name.lo,query_path,at) {
            let d=DefId(i as u32); return Ok(answer(hover(s,d),definition(p,d),only));
        }
    }
    let Some(token) = token else { return Ok(if only { "[]" } else { "null" }.into()); };
    let mut lo = token.lo as usize;
    while lo > 0 && (src[lo-1].is_ascii_alphanumeric() || matches!(src[lo-1],b'_'|b':')) { lo-=1; }
    let path = std::str::from_utf8(&src[lo..token.hi as usize]).unwrap_or(name);
    if let Some(d) = resolve(p,scope,path) { return Ok(answer(hover(s,d),definition(p,d),only)); }
    let _ = string(params,"path")?;
    Ok(if only { "[]" } else { "null" }.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_and_hits_follow_imported_origins() {
        use crate::pm_tokens::{Span, TokenTree};
        let sess = std::sync::Arc::new(crate::Session::default());
        let mut p = Program::new(&sess);
        let path = "/fake/query-module.rs";
        let source = "struct Outer { value: u32 }";
        let other = "/fake/query-import.rs";
        let authored = "// imported\néé authored_field\n";
        let lo = authored.find("authored_field").unwrap() as u32;
        let hi = authored.trim_end().len() as u32;
        p.sources.register(path, std::sync::Arc::from(source.as_bytes())).unwrap();
        let id = p.sources.register(other, std::sync::Arc::from(authored.as_bytes())).unwrap();
        let mut stream = crate::builtins::lex_stream(source).unwrap();
        let TokenTree::Group(fields) = &mut stream.trees[2] else { panic!("fields") };
        fields.stream.trees[0].set_span(Span { file: id, lo, hi, ..Span::DUMMY });
        let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new("test", path, "test", &crate::cfg::CfgSet::host(&sess, &[])));
        let (pf, _) = crate::parser::parse_token_stream(stream, path, 2021, None, ctx, p.sources.clone(), false).unwrap();
        let at = pf.toks.iter().find(|t| pf.bytes(t.lo, t.hi) == b"value").unwrap().lo;
        let item = *pf.ast.item(pf.ast.root_items[0]);
        p.files.push(std::sync::Arc::new(pf));
        p.file_paths.push(path.into());
        let location = crate::json::parse(loc(&p, 0, at).as_bytes()).unwrap();
        assert_eq!(string(&location, "file").unwrap(), other);
        assert_eq!(location.get("line").unwrap().num(), 2.0);
        assert_eq!(location.get("col").unwrap().num(), 6.0);
        assert_eq!(location.get("end_col").unwrap().num(), 20.0);
        assert!(hit(&p, 0, at, other, hi - 1));
        assert!(!hit(&p, 0, at, path, lo));
        assert!(source_extent(&p, 0, item.lo, item.hi, other, lo).is_some());
    }
}
