//! `#[cfg(...)]` evaluation for one target and one crate's feature set.

use crate::ast::{Attr, TokRange};
use crate::lexer::T;
use crate::lexer::Tok;

#[derive(Clone)]
pub struct CfgSet {
    pub session: std::sync::Arc<crate::Session>,
    /// `name` or `name=value` entries that are true
    pub set: Vec<String>,
}

impl CfgSet {
    /// The target the program is read for: the machine Relax runs on (JIT target), or
    /// MAKEPAD_RUN_CHECK_TARGET's.
    pub fn host(sess: &std::sync::Arc<crate::Session>, features: &[String]) -> CfgSet {
        // MAKEPAD_RUN_CHECK_TARGET=x86_64-pc-windows-gnu: type-check (`makepad check` / `makepad run
        // --check`, with cargo's `--target` of the same triple) another platform's code from here
        // and MAKEPAD_RUN_CHECK_TARGET=aarch64-apple-darwin / x86_64-unknown-linux-gnu (`makepad host-gen`
        // reads the provided crates as each target sees them)
        let mut cs = match sess.options.check_target {
            crate::options::CheckTarget::WindowsX64 => CfgSet::windows_x64(sess, features),
            crate::options::CheckTarget::MacosArm64 => CfgSet::macos_arm64(sess, features),
            crate::options::CheckTarget::LinuxX64 => CfgSet::linux_x64(sess, features),
            crate::options::CheckTarget::AndroidArm64 => CfgSet::android_arm64(sess, features),
            crate::options::CheckTarget::Web => CfgSet::web(sess, features),
            // machine() adds the extra cfgs itself
            crate::options::CheckTarget::Host => return CfgSet::machine(sess, features),
        };
        // cargo with `--target` applies RUSTFLAGS to the target's artifacts only, not to the
        // proc-macro world (machine()), which Relax builds for here
        cs.set.extend(sess.options.cfgs.iter().cloned());
        cs
    }

    /// The machine Relax runs on, whatever MAKEPAD_RUN_CHECK_TARGET checks: code Relax itself runs
    /// at compile time (the proc-macro world, pmhost.rs) is built for here. A world read as
    /// another target called that target's OS (Windows' FlsAlloc on Linux): every JIT derive
    /// failed, and `makepad host-gen` wrote a short Windows table.
    pub fn machine(sess: &std::sync::Arc<crate::Session>, features: &[String]) -> CfgSet {
        let mut cs = CfgSet::machine_base(sess, features);
        // cargo without `--target` (what a Host check target stands for) applies RUSTFLAGS
        // `--cfg` to everything it builds, proc macros and build scripts included
        if sess.options.check_target == crate::options::CheckTarget::Host {
            cs.set.extend(sess.options.cfgs.iter().cloned());
        }
        cs
    }

    fn machine_base(sess: &std::sync::Arc<crate::Session>, features: &[String]) -> CfgSet {
        if cfg!(target_os = "android") {
            CfgSet::android_arm64(sess, features)
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            CfgSet::macos_arm64(sess, features)
        } else if cfg!(windows) {
            CfgSet::windows_x64(sess, features)
        } else {
            CfgSet::linux_x64(sess, features)
        }
    }

    /// aarch64 macOS, release, panic=abort, not test.
    pub fn macos_arm64(sess: &std::sync::Arc<crate::Session>, features: &[String]) -> CfgSet {
        let mut set = Vec::new();
        for s in [
            "unix",
            "target_os=macos",
            "target_family=unix",
            "target_vendor=apple",
            "target_arch=aarch64",
            "target_pointer_width=64",
            "target_endian=little",
            "target_env=",
            "target_has_atomic=8",
            "target_has_atomic=16",
            "target_has_atomic=32",
            "target_has_atomic=64",
            "target_has_atomic=ptr",
            "panic=abort",
            // rustc --print cfg --target aarch64-apple-darwin (1.97.1): the target's default features
            "target_feature=aes",
            "target_feature=crc",
            "target_feature=dit",
            "target_feature=dotprod",
            "target_feature=dpb",
            "target_feature=dpb2",
            "target_feature=fcma",
            "target_feature=fhm",
            "target_feature=flagm",
            "target_feature=fp16",
            "target_feature=frintts",
            "target_feature=jsconv",
            "target_feature=lor",
            "target_feature=lse",
            "target_feature=neon",
            "target_feature=paca",
            "target_feature=pacg",
            "target_feature=pan",
            "target_feature=pmuv3",
            "target_feature=ras",
            "target_feature=rcpc",
            "target_feature=rcpc2",
            "target_feature=rdm",
            "target_feature=sb",
            "target_feature=sha2",
            "target_feature=sha3",
            "target_feature=ssbs",
            "target_feature=vh",
        ] {
            set.push(s.to_string());
        }
        for f in features {
            set.push(format!("feature={}", f));
        }
        CfgSet { session: sess.clone(), set }
    }

    /// aarch64 Android, release, panic=abort, not test.
    pub fn android_arm64(sess: &std::sync::Arc<crate::Session>, features: &[String]) -> CfgSet {
        // rustc --print cfg --target aarch64-linux-android: neon is the only default
        // target_feature (do not assume LSE/AES/SHA2).
        let mut set: Vec<String> = [
            "unix", "target_os=android", "target_family=unix", "target_env=",
            "target_vendor=unknown", "target_arch=aarch64",
            "target_pointer_width=64", "target_endian=little", "panic=abort",
            "target_feature=neon", "target_has_atomic=8", "target_has_atomic=16",
            "target_has_atomic=32", "target_has_atomic=64", "target_has_atomic=ptr",
        ].iter().map(|s| s.to_string()).collect();
        for f in features {
            set.push(format!("feature={}", f));
        }
        CfgSet { session: sess.clone(), set }
    }

    /// x86_64 Linux, release (no debug_assertions), panic=abort, not test.
    pub fn linux_x64(sess: &std::sync::Arc<crate::Session>, features: &[String]) -> CfgSet {
        let mut set = Vec::new();
        for s in [
            "unix",
            "target_os=linux",
            "target_family=unix",
            "target_arch=x86_64",
            "target_pointer_width=64",
            "target_endian=little",
            "target_env=gnu",
            "target_vendor=unknown",
            "target_has_atomic=8",
            "target_has_atomic=16",
            "target_has_atomic=32",
            "target_has_atomic=64",
            "target_has_atomic=ptr",
            "panic=abort",
            "target_feature=sse",
            "target_feature=sse2",
            "target_feature=fxsr",
        ] {
            set.push(s.to_string());
        }
        for f in features {
            set.push(format!("feature={}", f));
        }
        CfgSet { session: sess.clone(), set }
    }
    /// x86_64-pc-windows-gnu (`rustc --print cfg --target x86_64-pc-windows-gnu`), release,
    /// panic=abort, not test.
    pub fn windows_x64(sess: &std::sync::Arc<crate::Session>, features: &[String]) -> CfgSet {
        let mut set = Vec::new();
        for s in [
            "windows",
            "target_os=windows",
            "target_family=windows",
            "target_arch=x86_64",
            "target_pointer_width=64",
            "target_endian=little",
            "target_env=gnu",
            "target_abi=",
            "target_vendor=pc",
            "target_has_atomic=8",
            "target_has_atomic=16",
            "target_has_atomic=32",
            "target_has_atomic=64",
            "target_has_atomic=ptr",
            "panic=abort",
            "target_feature=sse",
            "target_feature=sse2",
            "target_feature=fxsr",
        ] {
            set.push(s.to_string());
        }
        for f in features {
            set.push(format!("feature={}", f));
        }
        CfgSet { session: sess.clone(), set }
    }
    /// The web target as the trace host runs it: `rustc --print cfg --target
    /// wasm32-unknown-unknown` (1.98), plus `atomics` of makepad's threaded web build (cargo
    /// makepad wasm's default: `+atomics,+bulk-memory,+mutable-globals`). Two differences, both because this machine's JIT runs the code:
    /// `target_pointer_width` is 64 (usize is the machine's word), and std/core/alloc are read
    /// with the machine's cfg (`std_cfg`): only the program's own crates take this set.
    pub fn web(sess: &std::sync::Arc<crate::Session>, features: &[String]) -> CfgSet {
        let mut set: Vec<String> = [
            "target_arch=wasm32", "target_os=unknown", "target_family=wasm", "target_env=", "target_abi=",
            "target_vendor=unknown", "target_pointer_width=64", "target_endian=little", "panic=abort",
            "target_feature=bulk-memory", "target_feature=multivalue", "target_feature=mutable-globals",
            "target_feature=nontrapping-fptoint", "target_feature=reference-types", "target_feature=sign-ext",
            "target_has_atomic=8", "target_has_atomic=16", "target_has_atomic=32", "target_has_atomic=64",
            "target_has_atomic=ptr", "target_feature=atomics",
        ].iter().map(|s| s.to_string()).collect();
        for f in features {
            set.push(format!("feature={}", f));
        }
        CfgSet { session: sess.clone(), set }
    }

    /// The target triple of this cfg set (build scripts' TARGET / HOST).
    pub fn triple(&self) -> &'static str {
        if self.has("target_arch=wasm32") {
            crate::options::CheckTarget::WEB_TRIPLE
        } else if self.has("target_os=windows") {
            "x86_64-pc-windows-gnu"
        } else if self.has("target_os=android") {
            "aarch64-linux-android"
        } else if self.has("target_os=macos") {
            "aarch64-apple-darwin"
        } else {
            "x86_64-unknown-linux-gnu"
        }
    }

    /// The CARGO_CFG_* variables cargo gives a build script for the target's basic cfgs (OS, arch,
    /// env, vendor, family, endianness, pointer width, and the family flag `unix` / `windows`).
    pub fn build_script_env(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        for key in ["target_endian", "target_pointer_width", "target_family", "target_os", "target_arch", "target_env", "target_vendor"] {
            let prefix = format!("{}=", key);
            if let Some(e) = self.set.iter().find(|e| e.starts_with(&prefix)) {
                v.push((format!("CARGO_CFG_{}", key.to_uppercase()), e[prefix.len()..].to_string()));
            }
        }
        for flag in ["unix", "windows"] {
            if self.has(flag) {
                v.push((format!("CARGO_CFG_{}", flag.to_uppercase()), String::new()));
            }
        }
        // cargo gives a build script every `--cfg` of RUSTFLAGS the same way: `CARGO_CFG_CUSTOM`
        // (empty) for `--cfg custom`, `CARGO_CFG_KEY=value` for `--cfg key="value"` (several
        // values of one key are joined by commas)
        for e in &self.session.options.cfgs {
            let (k, val) = e.split_once('=').unwrap_or((e.as_str(), ""));
            let name = format!("CARGO_CFG_{}", k.to_uppercase());
            match v.iter_mut().find(|(n, _)| *n == name) {
                Some((_, old)) if !val.is_empty() => {
                    if !old.is_empty() {
                        old.push(',');
                    }
                    old.push_str(val);
                }
                Some(_) => {}
                None => v.push((name, val.to_string())),
            }
        }
        v
    }

    /// Is `name` / `name=value` set? (dependency platform predicates, resolve.rs)
    pub fn holds(&self, s: &str) -> bool {
        self.has(s)
    }

    fn has(&self, s: &str) -> bool {
        for x in &self.set {
            if x == s {
                return true;
            }
        }
        false
    }
}

/// Source and tokens of one file.
pub trait SourceTokens {
    fn token(&self, i: u32) -> Tok;
    fn bytes(&self, lo: u32, hi: u32) -> &[u8];
}

/// A file's source and its packed tokens as cfg evaluation reads them, where the caller also
/// changes the file's tree (so cannot lend the whole file).
pub struct PackedSrc<'a> {
    pub src: &'a [u8],
    pub toks: &'a crate::packed::PackedToks,
}

impl SourceTokens for PackedSrc<'_> {
    fn token(&self, i: u32) -> Tok { self.toks.at(i as usize) }
    fn bytes(&self, lo: u32, hi: u32) -> &[u8] {
        let n = self.src.len();
        &self.src[(lo as usize).min(n)..(hi as usize).min(n)]
    }
}

#[derive(Clone, Copy)]
pub struct Src<'a> {
    pub src: &'a [u8],
    /// tokens of a working array (not a published file's: those go through `parts`)
    pub plain: &'a [Tok],
    /// expansion text past the end of `src` (while parsing; empty afterwards: then `src`
    /// already holds it)
    pub ext: &'a [u8],
    /// Immutable body segments published after the file was parsed.
    pub parts: Option<&'a dyn SourceTokens>,
}

impl<'a> Src<'a> {
    #[inline]
    pub fn tok(&self, i: usize) -> Tok {
        if let Some(parts) = self.parts { return parts.token(i as u32); }
        self.plain[i]
    }
}

pub fn tok_str<'a>(p: Src<'a>, i: usize) -> &'a [u8] {
    let t = p.tok(i);
    if let Some(parts) = p.parts { return parts.bytes(t.lo, t.hi); }
    let (lo, hi) = (t.lo as usize, t.hi as usize);
    if lo >= p.src.len() && !p.ext.is_empty() {
        // a token of a macro expansion (the parser's text past the file's end)
        let b = p.src.len();
        return &p.ext[(lo - b).min(p.ext.len())..(hi - b).min(p.ext.len())];
    }
    &p.src[lo.min(p.src.len())..hi.min(p.src.len())]
}

/// Evaluates a predicate starting at token `*i` (inside an attribute range ending at `end`).
fn pred(p: Src, i: &mut usize, end: usize, cfg: &CfgSet) -> bool {
    if *i >= end {
        return false;
    }
    // a macro's `$e:expr` argument arrives parenthesized (macros.rs transcribe): `(any(..))`
    if p.tok(*i).kind == T::OpenParen {
        *i += 1;
        let r = pred(p, i, end, cfg);
        if *i < end && p.tok(*i).kind == T::CloseParen {
            *i += 1;
        }
        return r;
    }
    let name = tok_str(p, *i);
    *i += 1;
    if *i < end && p.tok(*i).kind == T::OpenParen && (name == b"all" || name == b"any" || name == b"not") {
        *i += 1;
        let mut vals = Vec::new();
        while *i < end && p.tok(*i).kind != T::CloseParen {
            vals.push(pred(p, i, end, cfg));
            if *i < end && p.tok(*i).kind == T::Comma {
                *i += 1;
            }
        }
        *i += 1; // `)`
        return match name {
            b"all" => {
                let mut r = true;
                for v in &vals {
                    r = r && *v;
                }
                r
            }
            b"any" => {
                let mut r = false;
                for v in &vals {
                    r = r || *v;
                }
                r
            }
            _ => vals.len() == 1 && !vals[0],
        };
    }
    // `cfg(true)` / `cfg(false)` (boolean literals, stable since 1.88)
    if name == b"true" || name == b"false" {
        return name == b"true";
    }
    let n = String::from_utf8_lossy(name).into_owned();
    if *i < end && p.tok(*i).kind == T::Eq {
        *i += 1;
        let v = tok_str(p, *i);
        *i += 1;
        let v = String::from_utf8_lossy(v).into_owned();
        let v = v.trim_matches('"');
        return cfg.has(&format!("{}={}", n, v));
    }
    cfg.has(&n)
}

/// Is an item/statement with these attributes active? (all `cfg` attrs must hold;
/// `cfg_attr(pred, cfg(x))` is followed too)
pub fn active(p: Src, attrs: &[Attr], cfg: &CfgSet) -> bool {
    // Configure nested cfg_attr before inspecting cfg, as rustc's
    // StripUnconfigured::configure does. A cfg inside a false nested cfg_attr
    // must not disable the item.
    let expanded;
    let attrs = if attrs.iter().any(|a| a.toks.hi > a.toks.lo && tok_str(p, a.toks.lo as usize) == b"cfg_attr") {
        expanded = expand_cfg_attr_list(p, attrs, cfg);
        &expanded[..]
    } else {
        attrs
    };
    for a in attrs {
        let lo = a.toks.lo as usize;
        let hi = a.toks.hi as usize;
        if hi <= lo {
            continue;
        }
        let name = tok_str(p, lo);
        if name == b"cfg" && lo + 1 < hi && p.tok(lo + 1).kind == T::OpenParen {
            let mut i = lo + 2;
            if !pred(p, &mut i, hi - 1, cfg) {
                return false;
            }
        }
    }
    true
}

/// Configured outer attributes preceding an item keyword (past its visibility).
/// Prescans must use the same cfg_attr expansion as the parser before publishing
/// macros, exports or imports. Returned ranges still address the original tokens.
pub fn attrs_before(p: Src, mut i: usize, cfg: &CfgSet) -> Vec<Attr> {
    if i > 0 && p.tok(i - 1).kind == T::CloseParen {
        let mut open = i - 1;
        while open > 0 && p.tok(open).kind != T::OpenParen {
            open -= 1;
        }
        if open > 0 && p.tok(open - 1).kind == T::KwPub {
            i = open - 1;
        }
    } else if i > 0 && p.tok(i - 1).kind == T::KwPub {
        i -= 1;
    }
    let mut attrs = Vec::new();
    while i >= 3 && p.tok(i - 1).kind == T::CloseBracket {
        let close = i - 1;
        let mut open = close;
        let mut depth = 0;
        loop {
            match p.tok(open).kind {
                T::CloseBracket => depth += 1,
                T::OpenBracket => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            if open == 0 {
                break;
            }
            open -= 1;
        }
        if open == 0 || p.tok(open - 1).kind != T::Pound {
            break;
        }
        attrs.push(Attr { inner: false, toks: TokRange { lo: (open + 1) as u32, hi: close as u32 } });
        i = open - 1;
    }
    attrs.reverse();
    expand_cfg_attr_list(p, &attrs, cfg)
}

/// `cfg!(pred)`: the predicate in tokens [lo, hi).
pub fn eval_tokens(p: Src, lo: usize, hi: usize, cfg: &CfgSet) -> bool {
    let mut i = lo;
    pred(p, &mut i, hi, cfg)
}

/// Is `#[test]` among the attributes (tests are inactive in a normal build)?
pub fn is_test(p: Src, attrs: &[Attr]) -> bool {
    for a in attrs {
        let lo = a.toks.lo as usize;
        if a.toks.hi as usize == lo + 1 && tok_str(p, lo) == b"test" {
            return true;
        }
    }
    false
}

/// For `cfg_attr(pred, ...)`: does `pred` hold?
pub fn cfg_attr_holds(p: Src, a: &Attr, cfg: &CfgSet) -> bool {
    let lo = a.toks.lo as usize;
    let hi = a.toks.hi as usize;
    if hi <= lo + 2 {
        return false;
    }
    let mut i = lo + 2;
    pred(p, &mut i, hi - 1, cfg)
}

/// rustc_expand config.rs `expand_cfg_attr`: `#[cfg_attr(pred, a, b(..))]` becomes `#[a] #[b(..)]`
/// when pred holds and nothing otherwise (recursively: the expanded attributes may be cfg_attr
/// again). The expanded attributes are sub-ranges of the original tokens.
pub fn expand_cfg_attr_list(p: Src, attrs: &[Attr], cfg: &CfgSet) -> Vec<Attr> {
    let mut out = Vec::with_capacity(attrs.len());
    for a in attrs {
        let lo = a.toks.lo as usize;
        let hi = a.toks.hi as usize;
        if hi <= lo + 2 || tok_str(p, lo) != b"cfg_attr" || p.tok(lo + 1).kind != T::OpenParen {
            out.push(a.clone());
            continue;
        }
        // tokens: cfg_attr ( pred , a1 , a2 ... )
        let end = hi - 1; // the closing paren
        let mut i = lo + 2;
        if !pred(p, &mut i, end, cfg) {
            continue;
        }
        let parts = cfg_attr_parts(p, a, i);
        out.extend(expand_cfg_attr_list(p, &parts, cfg));
    }
    out
}

/// The attributes of `cfg_attr(pred, a1, a2(..))` whose predicate ends before token `from`
/// (the first token after it): `[a1, a2(..)]`, sub-ranges of the original tokens.
pub fn cfg_attr_parts(p: Src, a: &Attr, from: usize) -> Vec<Attr> {
    let end = a.toks.hi as usize - 1; // the closing paren
    let mut i = from;
    let mut parts = Vec::new();
    while i < end {
        if p.tok(i).kind == T::Comma {
            i += 1;
            continue;
        }
        let start = i;
        let mut depth = 0i32;
        while i < end {
            match p.tok(i).kind {
                T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
                T::Comma if depth == 0 => break,
                _ => {}
            }
            i += 1;
        }
        if i > start {
            parts.push(Attr { inner: a.inner, toks: TokRange { lo: start as u32, hi: i as u32 } });
        }
    }
    parts
}

/// The token after the predicate of `cfg_attr(pred, ...)` (the predicate starts after the
/// opening paren at `a.toks.lo + 2`).
pub fn cfg_attr_pred_end(p: Src, a: &Attr) -> usize {
    let end = a.toks.hi as usize - 1;
    let mut i = a.toks.lo as usize + 2;
    let mut depth = 0i32;
    while i < end {
        match p.tok(i).kind {
            T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
            T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
            T::Comma if depth == 0 => break,
            _ => {}
        }
        i += 1;
    }
    i
}
