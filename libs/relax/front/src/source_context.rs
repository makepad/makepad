//! Immutable Rust source snapshots, outline reuse and parse diagnostics.
use crate::{builtins::FileCtx, source::{SourceRegistry, SourceSpan}};
use std::{collections::BTreeMap, path::{Path, PathBuf}, rc::Rc, sync::{Arc, OnceLock}};
use relax_session::sync::Mutex;

pub struct Context {
    sources: Arc<SourceRegistry>,
    reads: Arc<crate::vfs::ReadSnapshot>,
    session: crate::Session,
    rust_outlines: Mutex<BTreeMap<(String, u16), Arc<OnceLock<Option<Arc<crate::rs_outline::Outline>>>>>>,
    outline_reuse: Mutex<BTreeMap<String, Arc<crate::parse_work::Reuse>>>,
    demand_identity: OnceLock<u128>,
    fingerprint: OnceLock<u128>,
}
impl Context {
    pub fn new(_root: &str, sources: Arc<SourceRegistry>, session: &crate::Session) -> Arc<Self> {
        Arc::new(Self { sources, reads: crate::vfs::ReadSnapshot::new(session.runtime.profile), session: crate::Session { options: session.options.clone(), runtime: session.runtime.clone() },
            rust_outlines: Default::default(), outline_reuse: Default::default(),
            demand_identity: OnceLock::new(), fingerprint: OnceLock::new() })
    }
    pub fn for_units(crates: &[crate::crates::CrateUnit], sources: Arc<SourceRegistry>, session: &crate::Session) -> Vec<Arc<Self>> {
        crates.iter().map(|c| Self::new(&c.src_path, sources.clone(), session)).collect()
    }
    pub fn for_crates(crates: &[crate::preload::PreCrate], sources: Arc<SourceRegistry>, session: &crate::Session) -> Vec<Arc<Self>> {
        crates.iter().map(|c| Self::new(&c.root, sources.clone(), session)).collect()
    }
    pub fn bind_source_reads(&self) -> crate::vfs::ReadScope { self.reads.bind() }
    pub fn finish_load(&mut self) { self.fingerprint(); self.release_prescan(); }
    pub fn release_prescan(&self) {
        self.rust_outlines.lock().unwrap().clear();
        self.outline_reuse.lock().unwrap().clear();
    }
    /// `path`'s authored bytes are now a mapped image's: the load's read snapshot drops its copy.
    pub fn release_read(&self, path: &str) { self.reads.release(std::path::Path::new(path)); }
    pub fn report_host_profile(&self) { self.reads.report(); }
    pub fn read_source(&self, path: &str) -> Result<Arc<crate::source::SourceFile>, String> {
        let _reads = self.bind_source_reads();
        self.sources.get(self.sources.source_id(path))
    }
    pub fn exists(&self, path: &str) -> bool {
        self.sources.overridden_paths().iter().any(|p| p == path) || crate::vfs::exists(path)
    }
    pub fn edited(&self, path: &str, src: Arc<[u8]>) -> Arc<Self> {
        Self::new(path, Arc::new(self.sources.overlay(path, src)), &self.session)
    }
    pub(crate) fn set_outline_reuse(&self, path: &str, reuse: Arc<crate::parse_work::Reuse>) {
        self.outline_reuse.lock().unwrap().insert(path.into(), reuse);
    }
    pub fn rust_outline(&self, path: &str, src: &[u8], edition: u16, cfg: &crate::cfg::CfgSet) -> Option<Arc<crate::rs_outline::Outline>> {
        let key = (crate::vfs::normalize_path(path), edition);
        let cell = self.rust_outlines.lock().unwrap().entry(key).or_default().clone();
        cell.get_or_init(|| {
            let _profile = crate::load_profile::span(self.session.runtime.profile, "rs-outline", path);
            let outline = crate::rs_outline::Outline::build(src, edition, cfg)?;
            crate::why::event("outline", path, 0, "", None);
            Some(Arc::new(outline))
        }).clone()
    }
    /// rust_outline from `tokens` (the file's lex), and the file's cfg-active tokens.
    pub fn rust_outline_lexed(&self, path: &str, src: &[u8], edition: u16, cfg: &crate::cfg::CfgSet, tokens: &[crate::lexer::Tok]) -> Option<(Arc<crate::rs_outline::Outline>, Vec<crate::lexer::Tok>)> {
        let key = (crate::vfs::normalize_path(path), edition);
        let cell = self.rust_outlines.lock().unwrap().entry(key).or_default().clone();
        let mut built = None;
        let outline = cell.get_or_init(|| {
            let _profile = crate::load_profile::span(self.session.runtime.profile, "rs-outline", path);
            let (outline, active) = crate::rs_outline::Outline::of_tokens(src, tokens, cfg)?;
            built = Some(active);
            crate::why::event("outline", path, 0, "", None);
            Some(Arc::new(outline))
        }).clone()?;
        let active = built.unwrap_or_else(|| crate::preload::configured_tokens(src, tokens, cfg));
        Some((outline, active))
    }
    pub fn sources(&self) -> &Arc<SourceRegistry> {
        &self.sources
    }
    pub fn fingerprint(&self) -> u128 { *self.fingerprint.get_or_init(|| self.demand_identity.get().copied().unwrap_or(0)) }
    pub fn restore_fingerprint(&self, fingerprint: u128) { let _ = self.fingerprint.set(fingerprint); }
    pub fn set_demand_identity(&self, identity: u128) { let _ = self.demand_identity.set(identity); }
    pub fn source_site(&self, path: &str, span: SourceSpan) -> (String, usize) {
        let id = if span.file == 0 { self.sources.source_id(path) } else { span.file };
        self.sources.get(id).map(|s| (s.path.clone(), s.line_col(span.lo).0)).unwrap_or_else(|_| (path.into(), 0))
    }
    pub fn diagnostic(
        &self,
        path: &str,
        span: SourceSpan,
        phase: &'static str,
        message: String,
    ) -> crate::diag::Diagnostic {
        let id = if span.file == 0 {
            self.sources.source_id(path)
        } else {
            span.file
        };
        match self.sources.get(id) {
            Ok(source) => crate::source::SourceLocation {
                source: crate::source::SourceRef::Registered(source),
                span,
            }
            .diagnostic(phase, message),
            Err(_) => crate::diag::Diagnostic::new(phase, path, 1, 1, message),
        }
    }
    pub fn parse(
        &self,
        src: Vec<u8>,
        pre: Option<Vec<crate::lexer::Tok>>,
        edition: u16,
        macros: Option<crate::macros::MacroScope>,
        ctx: Rc<FileCtx>,
        lazy: bool,
    ) -> Result<(crate::parser::ParsedFile, Option<crate::macros::MacroUse>), String> {
        self.parse_input(src, pre, edition, macros, ctx, lazy)
    }
    pub(crate) fn parse_input(
        &self,
        src: Vec<u8>,
        pre: Option<Vec<crate::lexer::Tok>>,
        edition: u16,
        macros: Option<crate::macros::MacroScope>,
        ctx: Rc<FileCtx>,
        lazy: bool,
    ) -> Result<(crate::parser::ParsedFile, Option<crate::macros::MacroUse>), String> {
        let _reads = self.bind_source_reads();
        let path = &ctx.path;
        let reuse = self.outline_reuse.lock().unwrap().get(path).cloned().map(crate::parse_work::Session::new);
        crate::why::event("parse", path, 0, "", None);
        let result = {
            let _profile = crate::load_profile::span(self.session.runtime.profile, "parse-rs", path);
            crate::parser::parse_owned_input(
                &ctx.cfg.session,
                src,
                pre,
                edition,
                macros,
                &ctx.krate,
                Some(ctx.clone()),
                lazy,
                None,
                Some(self.sources.clone()),
            )
        };
        if self.session.runtime.profile || crate::why::enabled() {
            if let Ok((file, _)) = &result {
                let mut deferred = 0;
                let mut eager = 0;
                let mut initializers = 0;
                for item in &file.ast.items {
                    if let crate::ast::ItemKind::Fn(sig, body, _) = &item.kind {
                        deferred += usize::from(file.ast.fn_sig(*sig).lazy_body.is_some());
                        eager += usize::from(body.is_some());
                    }
                    if matches!(&item.kind, crate::ast::ItemKind::Const(_, e) | crate::ast::ItemKind::Static(_, _, e) if e.is_some()) {
                        initializers += 1;
                    }
                }
                let language = "rs";
                if self.session.runtime.profile { eprintln!("{language}-parse-bodies: deferred={deferred} eager={eager} initializers={initializers} items={} file={path:?}", file.ast.items.len()); }
                if eager != 0 || initializers != 0 { crate::why::event("parse-bodies", path, 0, "", None); }
            }
        }
        if let Some(reuse) = reuse { let _ = reuse.finish(); }
        result.map_err(|(span, m)| self.record(path, span, m))
    }
    fn record(&self, path: &str, span: SourceSpan, message: String) -> String {
        let d = self.diagnostic(path, span, "parse", message);
        crate::diag::record(format!("{}:{}:{}: {}", d.file, d.line, d.col, d.message), d)
    }
}

static OUTPUT_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_output_dir(dir: &str) {
    let _ = OUTPUT_DIR.set(crate::zone_alloc::kept_with(|| PathBuf::from(crate::vfs::normalize_path(dir))));
}

fn output_dir_entry(dir: &Path, name: &str) -> bool {
    OUTPUT_DIR.get().is_some_and(|o| o.parent() == Some(dir) && o.file_name().and_then(|n| n.to_str()) == Some(name))
}

fn directory_hash(path: &str, entries: &[(String, bool)]) -> u128 {
    let mut hash = crate::sip::Sip128::new();
    hash.str(path);
    for (name, dir) in entries {
        hash.str(name);
        hash.u32(*dir as u32);
    }
    hash.finish128()
}
pub fn input_hash(path: &str) -> Option<u128> {
    if crate::vfs::is_dir(path) {
        crate::vfs::read_dir_entries(path)
            .ok()
            .map(|entries| {
                let dir = PathBuf::from(crate::vfs::normalize_path(path));
                directory_hash(path, &entries.into_iter().filter(|entry| !output_dir_entry(&dir, &entry.name)).map(|entry| (entry.name, entry.is_dir)).collect::<Vec<_>>())
            })
    } else {
        crate::vfs::read(path)
            .ok()
            .map(|src| crate::crate_image::file_hash(path, &src))
    }
}

/// The recorded stat of an input whose recorded hash `hash` is still the file's hash right now:
/// read from the disk (not a load's read snapshot) between two equal stats, and not changed
/// within `STAT_RACY_NS` of `now_ns`. None: hash it at every validation.
pub fn input_stamp(path: &str, hash: u128, now_ns: u64) -> Option<crate::vfs::FileStat> {
    let before = crate::vfs::file_stat(path)?;
    if before.mtime_ns.max(before.ctime_ns).saturating_add(crate::vfs::STAT_RACY_NS) >= now_ns {
        return None;
    }
    let fresh = if crate::vfs::is_dir(path) {
        input_hash(path)?
    } else {
        crate::crate_image::file_hash(path, &std::fs::read(path).ok()?)
    };
    (fresh == hash && crate::vfs::file_stat(path)? == before).then_some(before)
}
pub fn cmd_lex(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("usage: makepad lex <file>");
        return 2;
    };
    let src = match crate::vfs::read(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{path}: {e}");
            return 1;
        }
    };
    let mut toks = Vec::new();
    if let Err(e) = crate::lexer::lex(&src, 2021, &mut toks) {
        let (l, c) = crate::lexer::line_col(&src, e.pos);
        eprintln!("{path}:{l}:{c}: {}", e.msg);
        return 1;
    }
    for t in toks
        .into_iter()
        .take_while(|t| t.kind != crate::lexer::T::Eof)
    {
        println!(
            "{}",
            String::from_utf8_lossy(&src[t.lo as usize..t.hi as usize]).replace('\n', "\\n")
        );
    }
    0
}

