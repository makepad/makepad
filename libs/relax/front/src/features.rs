//! The crate's `#![feature(..)]` declarations (rustc_expand/src/config.rs `features`) and the
//! feature tables of rustc_feature (feature_tables.rs, generated from rustc 1.97.1).
//! Gating is a check-time diagnostic: nothing here runs for a crate the check does not report.

pub use crate::feature_tables::{ACCEPTED, GATED_ATTRS, GATED_CFGS, REMOVED, UNSTABLE};

/// One name in a crate's `#![feature(..)]` attributes.
#[derive(Debug, Clone)]
pub struct Declared {
    pub name: String,
    /// byte position of the name
    pub pos: u32,
    /// rustc_feature REMOVED: E0557, and the feature is not enabled
    pub removed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Features {
    pub declared: Vec<Declared>,
    /// byte position of the first `feature` attribute
    pub first_attr: Option<u32>,
}

impl Features {
    /// The crate enabled `name` (a removed feature is not).
    pub fn has(&self, name: &str) -> bool {
        self.declared.iter().any(|d| !d.removed && d.name == name)
    }

    /// `features()` of rustc_expand/src/config.rs over the root file's attributes (cfg_attr
    /// expanded; rustc stops at the first `#![cfg(..)]` that does not hold).
    pub fn read(prog: &crate::program::Program, root_file: u32, cfg: &crate::cfg::CfgSet) -> Features {
        let file = &prog.files[root_file as usize];
        let src = prog.src(root_file);
        let mut out = Features::default();
        for attr in crate::cfg::expand_cfg_attr_list(src, &file.ast.root_attrs, cfg) {
            let (lo, hi) = (attr.toks.lo, attr.toks.hi);
            if hi > lo && file.tok_text(lo) == "cfg" && !crate::cfg::active(src, std::slice::from_ref(&attr), cfg) { break; }
            if !attr.inner || hi < lo + 3 || file.tok_text(lo) != "feature" || file.tok_text(lo + 1) != "(" || file.tok_text(hi - 1) != ")" { continue; }
            out.first_attr.get_or_insert(file.tok(lo).lo);
            let mut i = lo + 2;
            while i < hi - 1 {
                let name = file.tok_text(i);
                if name != "," {
                    out.declared.push(Declared { name: name.to_string(), pos: file.tok(i).lo, removed: REMOVED.contains(&name) });
                }
                i += 1;
            }
        }
        out
    }
}
