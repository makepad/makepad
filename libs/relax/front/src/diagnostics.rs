//! Source spans and bounded, scope-local name suggestions shared by all checking phases.
use crate::{diag::{Diagnostic, Suggestion}, program::Program};
use crate::source::{SourceLocation, SourceRef, SourceSpan};

/// The marker of an error rustc reports without a code (see `record_uncoded_error`).
pub const UNCODED_PREFIX: &str = "error: ";

impl Program {
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
        if !matches!(d.code.as_str(), "E0412" | "E0425" | "E0433" | "RR0001" | "RM0001") { return d; }
        let module = self.defs.iter().filter(|d| d.file == file && d.item.0 != u32::MAX).filter_map(|d| {
            let it = f.ast.items.get(d.item.0 as usize)?;
            (it.lo <= pos && pos <= it.hi).then_some((it.hi.saturating_sub(it.lo), d.scope))
        }).min().map(|x|x.1).or_else(||self.mods.iter().position(|m| m.file == file).map(|m|m as u32));
        if let Some(module) = module { self.suggest(&mut d, &location, module); }
        d
    }

    pub fn suggest(&self, d: &mut Diagnostic, location: &SourceLocation<'_>, module: u32) {
        if !matches!(d.code.as_str(), "E0412" | "E0425" | "E0433" | "RR0001" | "RM0001") { return; }
        let Some(name) = d.message.split('`').nth(1) else { return };
        // A path error's span identifies the unresolved leading segment; import collection
        // supplies the last segment's span. Never replace the entire qualified path.
        let span = location.span;
        let token = location.source.bytes().get(span.lo as usize..span.hi as usize).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("");
        let bare = token.strip_prefix("r#").unwrap_or(token);
        let qualified = name.contains("::");
        let name = if qualified && name.split("::").any(|s| s == bare) { bare } else { name.rsplit("::").next().unwrap_or(name) };
        if qualified && self.syms.get(name).is_some_and(|s| self.lookup_in_scope(module,s,true).is_some()) { return; }
        let mut modules = vec![module];
        if let Some(pc) = self.prelude_crate {
            let root = self.crates[pc as usize].root_mod;
            if let Some(p) = self.syms.get("prelude").and_then(|s| self.mods[root as usize].types.get(&s)) { modules.push(self.def(*p).sub); }
        }
        let mut seen = crate::fxhash::HashSet::default();
        let mut names = Vec::new();
        while let Some(m) = modules.pop() {
            if !seen.insert(m) { continue; }
            let Some(m) = self.mods.get(m as usize) else { continue };
            let keys: Vec<_> = if d.code == "RM0001" { m.macros.keys().collect() }
                else if d.code == "E0425" { m.values.keys().collect() }
                else if d.code == "E0412" || d.code == "E0433" { m.types.keys().collect() }
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
                (d.code != "E0425" && self.lookup_in_scope(module,s,true).is_some()) ||
                (!matches!(d.code.as_str(), "E0412" | "E0433") && self.lookup_in_scope(module,s,false).is_some())
            }));
            if d.code != "E0425" { names.extend(crate::program::PRIMS.iter().map(|(n,_)|*n)); }
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
