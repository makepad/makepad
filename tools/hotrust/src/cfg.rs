//! `#[cfg(...)]` evaluation for one target and one crate's feature set.

use crate::ast::Attr;
use crate::lexer::T;
use crate::lexer::Tok;

pub struct CfgSet {
    /// `name` or `name=value` entries that are true
    pub set: Vec<String>,
}

impl CfgSet {
    /// The machine HotRust runs on (JIT target).
    pub fn host(features: &[String]) -> CfgSet {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            CfgSet::macos_arm64(features)
        } else {
            CfgSet::linux_x64(features)
        }
    }

    /// aarch64 macOS, release, panic=abort, not test.
    pub fn macos_arm64(features: &[String]) -> CfgSet {
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
            "target_feature=neon",
        ] {
            set.push(s.to_string());
        }
        for f in features {
            set.push(format!("feature={}", f));
        }
        CfgSet { set }
    }

    /// x86_64 Linux, release (no debug_assertions), panic=abort, not test.
    pub fn linux_x64(features: &[String]) -> CfgSet {
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
        CfgSet { set }
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
#[derive(Clone, Copy)]
pub struct Src<'a> {
    pub src: &'a [u8],
    pub toks: &'a [Tok],
}

fn tok_str<'a>(p: Src<'a>, i: usize) -> &'a [u8] {
    let t = p.toks[i];
    &p.src[t.lo as usize..t.hi as usize]
}

/// Evaluates a predicate starting at token `*i` (inside an attribute range ending at `end`).
fn pred(p: Src, i: &mut usize, end: usize, cfg: &CfgSet) -> bool {
    if *i >= end {
        return false;
    }
    let name = tok_str(p, *i);
    *i += 1;
    if *i < end && p.toks[*i].kind == T::OpenParen && (name == b"all" || name == b"any" || name == b"not") {
        *i += 1;
        let mut vals = Vec::new();
        while *i < end && p.toks[*i].kind != T::CloseParen {
            vals.push(pred(p, i, end, cfg));
            if *i < end && p.toks[*i].kind == T::Comma {
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
    let n = String::from_utf8_lossy(name).into_owned();
    if *i < end && p.toks[*i].kind == T::Eq {
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
    for a in attrs {
        let lo = a.toks.lo as usize;
        let hi = a.toks.hi as usize;
        if hi <= lo {
            continue;
        }
        let name = tok_str(p, lo);
        if name == b"cfg" && lo + 1 < hi && p.toks[lo + 1].kind == T::OpenParen {
            let mut i = lo + 2;
            if !pred(p, &mut i, hi - 1, cfg) {
                return false;
            }
        } else if name == b"cfg_attr" && lo + 1 < hi {
            let mut i = lo + 2;
            if pred(p, &mut i, hi - 1, cfg) {
                // following attrs separated by commas: look for `cfg(`
                while i < hi - 1 {
                    if p.toks[i].kind == T::Ident && tok_str(p, i) == b"cfg" && p.toks[i + 1].kind == T::OpenParen {
                        let mut j = i + 2;
                        if !pred(p, &mut j, hi - 1, cfg) {
                            return false;
                        }
                        i = j;
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }
    true
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
