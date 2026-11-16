//! Source spans and bounded, scope-local name suggestions shared by all checking phases.
use crate::{diag::{Diagnostic, Suggestion}, program::Program};
use crate::source::{SourceLocation, SourceRef, SourceSpan};

/// The marker of an error rustc reports without a code (see `record_uncoded_error`).
pub const UNCODED_PREFIX: &str = "error: ";

/// An identifier as rustc prints it (rustc_span `Ident`'s Display, `is_raw_guess`, and
/// rustc_resolve's `names_to_string`): `r#` stays only on a name reserved in `edition`.
fn ident_display(text: &str, edition: u16) -> &str {
    match text.strip_prefix("r#") {
        Some(bare) if !crate::parser::Parser::reserved_name(bare.as_bytes(), edition) => bare,
        _ => text,
    }
}

impl Program {
    /// The unresolved item's name, scope and span from rustc_resolve/src/late/diagnostics.rs:383-560
    /// `make_base_error`: diagnose the last segment, retaining the resolved prefix's description
    /// (`Res::descr`, rustc_hir/src/def.rs:220-239,800-814), so qualified names identify their scope.
    pub fn unresolved_name_message(&self, file: u32, module: u32, path: &crate::ast::Path, mut expected: &str) -> (u32, String) {
        use crate::{ast::SegKind, program::DefKind};
        let f = &self.files[file as usize];
        let segs = f.ast.segs(path.segs);
        let last = segs.last().unwrap();
        let krate = self.mods[module as usize].krate;
        let edition = self.crates[krate as usize].edition;
        let name = ident_display(f.text(last.name), edition);
        let scope = if segs.len() == 1 && !path.global() {
            "this scope".to_string()
        } else if segs.len() == 1 && path.global() {
            if edition >= 2018 {
                expected = "crate";
                "the list of imported crates".to_string()
            } else {
                "the crate root".to_string()
            }
        } else if segs.len() == 2 && segs[0].kind() == SegKind::Crate {
            "the crate root".to_string()
        } else {
            let names = crate::program::path_segs(self, file, path);
            let _quiet = crate::program::QuietLookups::begin();
            let parent = self.resolve_mod_path(module, path.global(), &names[..names.len() - 1], false, false);
            let descr = match parent.map(|d| self.def(d)) {
                Some(d) if d.kind == DefKind::Mod && d.krate != krate && d.sub == self.crates[d.krate as usize].root_mod => "crate ",
                Some(d) if d.kind == DefKind::Mod => "module ",
                Some(d) if d.kind == DefKind::Enum => "enum ",
                Some(d) if d.kind == DefKind::Trait => "trait ",
                _ => "",
            };
            let mut prefix = if path.global() { "::".to_string() } else { String::new() };
            for (i, seg) in segs[..segs.len() - 1].iter().enumerate() {
                if i > 0 { prefix.push_str("::"); }
                prefix.push_str(ident_display(f.text(seg.name), edition));
            }
            format!("{descr}`{prefix}`")
        };
        (last.name.lo, format!("cannot find {expected} `{name}` in {scope}"))
    }

    /// The message (not the label) from rustc_resolve/src/diagnostics/impls.rs:3027-3400
    /// `report_path_resolution_error`; keyword failures follow ident.rs:1894-2018
    /// `resolve_path_with_ribs`. Relax has no labels, so these are the complete path errors.
    pub fn path_resolution_message(&self, module: u32, global: bool, names: &[&str], failed: usize, parent: Option<crate::program::DefId>) -> String {
        let name = names[failed];
        if global && failed == 0 && matches!(name, "crate" | "self" | "super" | "Self") {
            return format!("global paths cannot start with `{name}`");
        }
        if name == "super" && !global && names[..failed].iter().all(|n| matches!(*n, "self" | "super")) {
            return self.too_many_super_message(module);
        }
        if failed > 0 && matches!(name, "crate" | "super" | "Self" | "self") && !(name == "self" && failed + 1 == names.len()) {
            return if name == "self" {
                "`self` in paths can only be used in start position or last position".to_string()
            } else {
                format!("`{name}` in paths can only be used in start position")
            };
        }
        let krate = self.mods[module as usize].krate;
        let edition = self.crates[krate as usize].edition;
        let scope = if failed > 0 {
            match names[failed - 1] {
                "crate" => "the crate root".to_string(),
                prev => format!("`{}`", ident_display(prev, edition)),
            }
        } else if global {
            if edition >= 2018 { "the list of imported crates" } else { "the crate root" }.to_string()
        } else {
            "this scope".to_string()
        };
        let root = self.mods[self.crates[krate as usize].root_mod as usize].def;
        let at_root = parent == Some(root) || (global && failed == 0 && edition < 2018);
        if at_root && name == "_" {
            return "invalid crate or module name `_`".to_string();
        }
        // At the crate root rustc 2015 also prints the plain message when lookup_import_candidates
        // finds a module of this name elsewhere (impls.rs:3100-3127); relax has no such search yet.
        let expected = if at_root && edition < 2018 && name != "core" {
            "module or crate "
        } else if at_root || failed > 0 || global || name == "Self" {
            ""
        } else if name.strip_prefix("r#").unwrap_or(name).chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            "type "
        } else {
            "module or crate "
        };
        format!("cannot find {expected}`{}` in {scope}", ident_display(name, edition))
    }

    /// Include the enclosing named module in an excessive-super error, as
    /// `resolve_path_with_ribs` does in rustc_resolve/src/ident.rs:1919-1932.
    pub fn too_many_super_message(&self, module: u32) -> String {
        let mut parts = Vec::new();
        let mut current = self.mods[module as usize].normal;
        while let Some(parent) = self.mods[current as usize].parent_mod {
            parts.push(self.syms.str(self.def(self.mods[current as usize].def).name));
            current = self.mods[parent as usize].normal;
        }
        let mut path = "crate".to_string();
        for part in parts.into_iter().rev() { path.push_str("::"); path.push_str(part); }
        format!("too many leading `super` keywords within `{path}`")
    }

    pub fn source_id(&self, path: &str) -> u32 { self.sources.source_id(path) }

    /// AST/parser position to authored file, full range, snapshot and line index.
    pub fn source_location(&self, file: u32, pos: u32) -> SourceLocation<'_> {
        let f = &self.files[file as usize];
        let span = f.source_span(pos);
        let source = if span.file == 0 {
            SourceRef::Current { path: &self.file_paths[file as usize], src: &f.src[..f.src_len as usize], lines: &f.line_starts }
        } else {
            SourceRef::Registered(f.sources.as_ref().unwrap_or(&self.sources).get(span.file).expect("parsed token source registered in its snapshot"))
        };
        SourceLocation { source, span }
    }

    /// Convert an error returned by parse_token_stream before its file has been published.
    pub fn parse_diagnostic(&self, path: &str, span: SourceSpan, msg: String) -> Result<Diagnostic, String> {
        let id = if span.file == 0 { self.source_id(path) } else { span.file };
        let source = SourceRef::Registered(self.sources.get(id)?);
        Ok(SourceLocation { source, span }.diagnostic("parse", msg))
    }

    pub fn diagnostic(&self, file: u32, pos: u32, phase: &'static str, msg: String) -> Diagnostic {
        let f = &self.files[file as usize];
        let location = self.source_location(file, pos);
        let mut d = location.diagnostic(phase, msg);
        if !matches!(d.code.as_str(), "E0425" | "E0432" | "E0433" | "RR0001" | "RM0001") { return d; }
        let module = self.defs.iter().filter(|d| d.file == file && d.item.0 != u32::MAX).filter_map(|d| {
            let it = f.ast.items.get(d.item.0 as usize)?;
            (it.lo <= pos && pos <= it.hi).then_some((it.hi.saturating_sub(it.lo), d.scope))
        }).min().map(|x|x.1).or_else(||self.mods.iter().position(|m| m.file == file).map(|m|m as u32));
        if let Some(module) = module { self.suggest(&mut d, &location, module); }
        d
    }

    pub fn suggest(&self, d: &mut Diagnostic, location: &SourceLocation<'_>, module: u32) {
        if !matches!(d.code.as_str(), "E0425" | "E0432" | "E0433" | "RR0001" | "RM0001") { return; }
        let Some(name) = d.message.split('`').nth(1) else { return };
        // the candidates' lookups are no uses of the imports they go through
        let _quiet = crate::program::QuietLookups::begin();
        // A path error's span identifies the failed segment; unresolved items and imports
        // identify the last segment. Never replace the entire qualified path.
        let span = location.span;
        let token = location.source.bytes().get(span.lo as usize..span.hi as usize).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("");
        let bare = token.strip_prefix("r#").unwrap_or(token);
        // make_base_error now prints the item and its scope separately; both spellings
        // identify a qualified lookup (rustc_resolve/src/late/diagnostics.rs:489-553).
        let scope = d.message.split('`').nth(2).unwrap_or("");
        let qualified = name.contains("::") || (scope.starts_with(" in ") && !scope.starts_with(" in this scope"));
        let name = if qualified && name.split("::").any(|s| s == bare) { bare } else { name.rsplit("::").next().unwrap_or(name) };
        if qualified && self.syms.get(name).is_some_and(|s| self.lookup_in_scope(module,s,true).is_some()) { return; }
        let mut modules = vec![module];
        if let Some(pc) = self.prelude_crate {
            let root = self.crates[pc as usize].root_mod;
            if let Some(p) = self.syms.get("prelude").and_then(|s| self.mods[root as usize].types.get(&s)) { modules.push(self.def(*p).sub); }
        }
        // the namespace of the unresolved name: E0425 is rustc's code for a value and, since
        // rustc 1.100, for a type too (`cannot find type`; late.rs error_code), E0433 a path's module
        let type_ns = d.code == "E0433" || (d.code == "E0425" && ["cannot find type ", "cannot find crate ", "cannot find external crate "].iter().any(|p| d.message.starts_with(p)));
        let value_ns = d.code == "E0425" && !type_ns;
        let mut seen = crate::fxhash::HashSet::default();
        let mut names = Vec::new();
        while let Some(m) = modules.pop() {
            if !seen.insert(m) { continue; }
            let Some(m) = self.mods.get(m as usize) else { continue };
            let keys: Vec<_> = if d.code == "RM0001" { m.macros.keys().collect() }
                else if value_ns { m.values.keys().collect() }
                else if type_ns { m.types.keys().collect() }
                else { m.types.keys().chain(m.values.keys()).collect() };
            names.extend(keys.into_iter().map(|s|self.syms.str(*s)));
            if let Some(p) = m.lexical_parent { modules.push(p); }
            for (g,_) in &m.globs { let g = self.def(*g); if g.kind == crate::program::DefKind::Mod { modules.push(g.sub); } }
        }
        if d.code != "RM0001" {
            for (n,_) in &self.crates[self.mods[module as usize].krate as usize].deps { names.push(self.syms.str(*n)); }
        }
        if d.code != "RM0001" {
            names.retain(|n| self.syms.get(n).is_some_and(|s| {
                (!value_ns && self.lookup_in_scope(module,s,true).is_some()) ||
                (!type_ns && self.lookup_in_scope(module,s,false).is_some())
            }));
            if !value_ns { names.extend(crate::program::PRIMS.iter().map(|(n,_)|*n)); }
        }
        let Some(new) = crate::diag::closest(name, names.into_iter()) else { return };
        // Only offer an edit when the mapped source token really is the unresolved name.
        let replacement = (bare == name).then(|| (d.file.clone(),d.line,d.col,d.end_line,d.end_col,new.clone()));
        let message = format!("did you mean `{}`?",new);
        if let Some(old) = d.suggestion.take() { if old.message != message { d.notes.push(old.message); } }
        d.suggestion = Some(Suggestion { message, replacement });
    }

    /// An error rustc emits without an error code: shown exactly as rustc words it
    /// (`path:l:c: error: message`); only a Relax-internal failure carries `internal:`.
    pub fn record_uncoded_error(&self, file: u32, pos: u32, phase: &'static str, msg: String) -> String {
        self.record_error(file, pos, phase, format!("{}{}", UNCODED_PREFIX, msg))
    }

    pub fn record_error(&self, file: u32, pos: u32, phase: &'static str, msg: String) -> String {
        let location = self.source_location(file, pos);
        let (l,c) = location.source.line_col(location.span.lo);
        let human = format!("{}:{}:{}: {}",location.source.path(),l,c,msg);
        if crate::diag::enabled() { crate::diag::record(human, self.diagnostic(file,pos,phase,msg)) } else { human }
    }
}
