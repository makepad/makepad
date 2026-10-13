// ported from compiler/rustc_target/src/target_features.rs @8d1a764
// ported from compiler/rustc_codegen_ssa/src/target_features.rs @8d1a764
// ported from compiler/rustc_attr_parsing/src/attributes/codegen_attrs.rs @8d1a764
//! `#[target_feature]`: the feature tables of the targets Relax builds for (x86_64, aarch64),
//! the attribute's parse and the codegen_fn_attrs subset it yields (from_target_feature_attr).
//! The other architectures' tables are not ported: Relax has no such target (Session::target).
//! `#[force_target_feature]` is feature-gated (effective_target_features) and cut (DEBLOAT).
use crate::ast::Attr;
use crate::lexer::T;
use crate::program::Program;

/// Stability information for target features.
#[derive(Debug, Copy, Clone)]
pub enum Stability {
    Stable,
    /// cfg-stable; `#[target_feature]` needs the named nightly feature.
    CfgStableToggleUnstable(&'static str),
    /// `#[target_feature]` needs the named nightly feature.
    Unstable(&'static str),
    /// Not a user-facing feature; it can never be toggled.
    InternalOnly { reason: &'static str, hard_error: bool },
}
use Stability::*;

impl Stability {
    /// target_features.rs:121-139 (`in_cfg` false: the `#[target_feature]` question).
    pub fn requires_nightly(&self) -> Option<&'static str> {
        match *self {
            Unstable(f) | CfgStableToggleUnstable(f) => Some(f),
            Stable => None,
            InternalOnly { .. } => panic!("internal-only features should not reach this far"),
        }
    }
    pub fn is_cfg_stable_toggle_unstable(&self) -> bool { matches!(self, CfgStableToggleUnstable(_)) }
    pub fn toggle_allowed(&self) -> Result<(), &'static str> {
        match self {
            Unstable(_) | CfgStableToggleUnstable(_) | Stable => Ok(()),
            InternalOnly { reason, .. } => Err(reason),
        }
    }
}

type ImpliedFeatures = &'static [&'static str];

static AARCH64_FEATURES: &[(&str, Stability, ImpliedFeatures)] = &[
    ("aes", Stable, &["neon"]),
    ("bf16", Stable, &[]),
    ("bti", Stable, &[]),
    ("crc", Stable, &[]),
    ("cssc", Unstable("aarch64_unstable_target_feature"), &[]),
    ("dit", Stable, &[]),
    ("dotprod", Stable, &["neon"]),
    ("dpb", Stable, &[]),
    ("dpb2", Stable, &["dpb"]),
    ("ecv", Unstable("aarch64_unstable_target_feature"), &[]),
    ("f32mm", Stable, &["sve"]),
    ("f64mm", Stable, &["sve"]),
    ("faminmax", Unstable("aarch64_unstable_target_feature"), &[]),
    ("fcma", Stable, &["neon"]),
    ("fhm", Stable, &["fp16"]),
    ("flagm", Stable, &[]),
    ("flagm2", Unstable("aarch64_unstable_target_feature"), &[]),
    ("fp-armv8", InternalOnly { reason: "Rust ties `fp-armv8` to `neon`", hard_error: false }, &[]),
    ("fp8", Unstable("aarch64_unstable_target_feature"), &["faminmax", "lut", "bf16"]),
    ("fp8dot2", Unstable("aarch64_unstable_target_feature"), &["fp8dot4"]),
    ("fp8dot4", Unstable("aarch64_unstable_target_feature"), &["fp8fma"]),
    ("fp8fma", Unstable("aarch64_unstable_target_feature"), &["fp8"]),
    ("fp16", Stable, &["neon"]),
    ("frintts", Stable, &[]),
    ("hbc", Unstable("aarch64_unstable_target_feature"), &[]),
    ("i8mm", Stable, &[]),
    ("jsconv", Stable, &["neon"]),
    ("lor", Stable, &[]),
    ("lse", Stable, &[]),
    ("lse2", Unstable("aarch64_unstable_target_feature"), &[]),
    ("lse128", Unstable("aarch64_unstable_target_feature"), &["lse"]),
    ("lut", Unstable("aarch64_unstable_target_feature"), &[]),
    ("mops", Unstable("aarch64_unstable_target_feature"), &[]),
    ("mte", Stable, &[]),
    ("neon", Stable, &[]),
    ("outline-atomics", Unstable("aarch64_unstable_target_feature"), &[]),
    ("paca", Stable, &[]),
    ("pacg", Stable, &[]),
    ("pan", Stable, &[]),
    ("pauth-lr", Unstable("aarch64_unstable_target_feature"), &[]),
    ("pmuv3", Stable, &[]),
    ("rand", Stable, &[]),
    ("ras", Stable, &[]),
    ("rcpc", Stable, &[]),
    ("rcpc2", Stable, &["rcpc"]),
    ("rcpc3", Unstable("aarch64_unstable_target_feature"), &["rcpc2"]),
    ("rdm", Stable, &["neon"]),
    ("reserve-x18", InternalOnly { reason: "use `-Zfixed-x18` compiler flag instead", hard_error: false }, &[]),
    ("sb", Stable, &[]),
    ("sha2", Stable, &["neon"]),
    ("sha3", Stable, &["sha2"]),
    ("sm4", Stable, &["neon"]),
    ("sme", Unstable("aarch64_unstable_target_feature"), &["bf16"]),
    ("sme-b16b16", Unstable("aarch64_unstable_target_feature"), &["bf16", "sme2", "sve-b16b16"]),
    ("sme-f8f16", Unstable("aarch64_unstable_target_feature"), &["sme-f8f32"]),
    ("sme-f8f32", Unstable("aarch64_unstable_target_feature"), &["sme2", "fp8"]),
    ("sme-f16f16", Unstable("aarch64_unstable_target_feature"), &["sme2"]),
    ("sme-f64f64", Unstable("aarch64_unstable_target_feature"), &["sme"]),
    ("sme-fa64", Unstable("aarch64_unstable_target_feature"), &["sme", "sve2"]),
    ("sme-i16i64", Unstable("aarch64_unstable_target_feature"), &["sme"]),
    ("sme-lutv2", Unstable("aarch64_unstable_target_feature"), &[]),
    ("sme2", Unstable("aarch64_unstable_target_feature"), &["sme"]),
    ("sme2p1", Unstable("aarch64_unstable_target_feature"), &["sme2"]),
    ("spe", Stable, &[]),
    ("ssbs", Stable, &[]),
    ("ssve-fp8dot2", Unstable("aarch64_unstable_target_feature"), &["ssve-fp8dot4"]),
    ("ssve-fp8dot4", Unstable("aarch64_unstable_target_feature"), &["ssve-fp8fma"]),
    ("ssve-fp8fma", Unstable("aarch64_unstable_target_feature"), &["sme2", "fp8"]),
    ("sve", Stable, &["neon", "fp16"]),
    ("sve-b16b16", Unstable("aarch64_unstable_target_feature"), &["bf16"]),
    ("sve2", Stable, &["sve"]),
    ("sve2-aes", Stable, &["sve2", "aes"]),
    ("sve2-bitperm", Stable, &["sve2"]),
    ("sve2-sha3", Stable, &["sve2", "sha3"]),
    ("sve2-sm4", Stable, &["sve2", "sm4"]),
    ("sve2p1", Unstable("aarch64_unstable_target_feature"), &["sve2"]),
    ("tme", Stable, &[]),
    ("v8.1a", Unstable("aarch64_ver_target_feature"), &["crc", "lse", "rdm", "pan", "lor", "vh"]),
    ("v8.2a", Unstable("aarch64_ver_target_feature"), &["v8.1a", "ras", "dpb"]),
    ("v8.3a", Unstable("aarch64_ver_target_feature"), &["v8.2a", "rcpc", "paca", "pacg", "jsconv"]),
    ("v8.4a", Unstable("aarch64_ver_target_feature"), &["v8.3a", "dotprod", "dit", "flagm"]),
    ("v8.5a", Unstable("aarch64_ver_target_feature"), &["v8.4a", "ssbs", "sb", "dpb2", "bti"]),
    ("v8.6a", Unstable("aarch64_ver_target_feature"), &["v8.5a", "bf16", "i8mm"]),
    ("v8.7a", Unstable("aarch64_ver_target_feature"), &["v8.6a", "wfxt"]),
    ("v8.8a", Unstable("aarch64_ver_target_feature"), &["v8.7a", "hbc", "mops"]),
    ("v8.9a", Unstable("aarch64_ver_target_feature"), &["v8.8a", "cssc"]),
    ("v9.1a", Unstable("aarch64_ver_target_feature"), &["v9a", "v8.6a"]),
    ("v9.2a", Unstable("aarch64_ver_target_feature"), &["v9.1a", "v8.7a"]),
    ("v9.3a", Unstable("aarch64_ver_target_feature"), &["v9.2a", "v8.8a"]),
    ("v9.4a", Unstable("aarch64_ver_target_feature"), &["v9.3a", "v8.9a"]),
    ("v9.5a", Unstable("aarch64_ver_target_feature"), &["v9.4a"]),
    ("v9a", Unstable("aarch64_ver_target_feature"), &["v8.5a"]),
    ("vh", Stable, &[]),
    ("wfxt", Unstable("aarch64_unstable_target_feature"), &[]),
];

const AARCH64_TIED_FEATURES: &[&[&str]] = &[
    &["paca", "pacg"], // Together these represent `pauth` in LLVM
];

static X86_FEATURES: &[(&str, Stability, ImpliedFeatures)] = &[
    ("adx", Stable, &[]),
    ("aes", Stable, &["sse2"]),
    ("amx-avx512", Unstable("x86_amx_intrinsics"), &["amx-tile"]),
    ("amx-bf16", Unstable("x86_amx_intrinsics"), &["amx-tile"]),
    ("amx-complex", Unstable("x86_amx_intrinsics"), &["amx-tile"]),
    ("amx-fp8", Unstable("x86_amx_intrinsics"), &["amx-tile"]),
    ("amx-fp16", Unstable("x86_amx_intrinsics"), &["amx-tile"]),
    ("amx-int8", Unstable("x86_amx_intrinsics"), &["amx-tile"]),
    ("amx-movrs", Unstable("x86_amx_intrinsics"), &["amx-tile"]),
    ("amx-tile", Unstable("x86_amx_intrinsics"), &[]),
    ("apxf", Unstable("apx_target_feature"), &[]),
    ("avx", Stable, &["sse4.2"]),
    ("avx2", Stable, &["avx"]),
    ("avx10.1", Unstable("avx10_target_feature"), &[
        "avx512bf16", "avx512bitalg", "avx512bw", "avx512cd", "avx512dq", "avx512f", "avx512fp16",
        "avx512ifma", "avx512vbmi", "avx512vbmi2", "avx512vl", "avx512vnni", "avx512vpopcntdq",
    ]),
    ("avx10.2", Unstable("avx10_target_feature"), &["avx10.1", "avxvnni", "avxvnniint8", "avxvnniint16"]),
    ("avx512bf16", Stable, &["avx512bw"]),
    ("avx512bitalg", Stable, &["avx512bw"]),
    ("avx512bw", Stable, &["avx512f"]),
    ("avx512cd", Stable, &["avx512f"]),
    ("avx512dq", Stable, &["avx512f"]),
    ("avx512f", Stable, &["avx2", "fma", "f16c"]),
    ("avx512fp16", Stable, &["avx512bw"]),
    ("avx512ifma", Stable, &["avx512f"]),
    ("avx512vbmi", Stable, &["avx512bw"]),
    ("avx512vbmi2", Stable, &["avx512bw"]),
    ("avx512vl", Stable, &["avx512f"]),
    ("avx512vnni", Stable, &["avx512f"]),
    ("avx512vp2intersect", Stable, &["avx512f"]),
    ("avx512vpopcntdq", Stable, &["avx512f"]),
    ("avxifma", Stable, &["avx2"]),
    ("avxneconvert", Stable, &["avx2"]),
    ("avxvnni", Stable, &["avx2"]),
    ("avxvnniint8", Stable, &["avx2"]),
    ("avxvnniint16", Stable, &["avx2"]),
    ("bmi1", Stable, &[]),
    ("bmi2", Stable, &[]),
    ("clflushopt", Unstable("clflushopt_target_feature"), &[]),
    ("cmpxchg16b", Stable, &[]),
    ("ermsb", Unstable("ermsb_target_feature"), &[]),
    ("f16c", Stable, &["avx"]),
    ("fma", Stable, &["avx"]),
    ("fma4", Unstable("fma4_target_feature"), &["avx", "sse4a"]),
    ("fxsr", Stable, &[]),
    ("gfni", Stable, &["sse2"]),
    ("kl", Stable, &["sse2"]),
    ("lahfsahf", Unstable("lahfsahf_target_feature"), &[]),
    ("lzcnt", Stable, &[]),
    ("movbe", Stable, &[]),
    ("movdir64b", Unstable("movdir64b_target_feature"), &[]),
    ("movdiri", Unstable("movdiri_target_feature"), &[]),
    ("movrs", Unstable("movrs_target_feature"), &[]),
    ("pclmulqdq", Stable, &["sse2"]),
    ("popcnt", Stable, &[]),
    ("prfchw", Unstable("prfchw_target_feature"), &[]),
    ("rdrand", Stable, &[]),
    ("rdseed", Stable, &[]),
    ("retpoline-external-thunk", InternalOnly { reason: "use `-Zretpoline-external-thunk` compiler flag instead", hard_error: false }, &[]),
    ("retpoline-indirect-branches", InternalOnly { reason: "use `-Zretpoline` compiler flag instead", hard_error: false }, &[]),
    ("retpoline-indirect-calls", InternalOnly { reason: "use `-Zretpoline` compiler flag instead", hard_error: false }, &[]),
    ("rtm", Unstable("rtm_target_feature"), &[]),
    ("sha", Stable, &["sse2"]),
    ("sha512", Stable, &["avx2"]),
    ("sm3", Stable, &["avx"]),
    ("sm4", Stable, &["avx2"]),
    ("soft-float", InternalOnly { reason: "use a soft-float target instead", hard_error: false }, &[]),
    ("sse", Stable, &[]),
    ("sse2", Stable, &["sse"]),
    ("sse3", Stable, &["sse2"]),
    ("sse4.1", Stable, &["ssse3"]),
    ("sse4.2", Stable, &["sse4.1"]),
    ("sse4a", Stable, &["sse3"]),
    ("ssse3", Stable, &["sse3"]),
    ("tbm", Stable, &[]),
    ("vaes", Stable, &["avx2", "aes"]),
    ("vpclmulqdq", Stable, &["avx", "pclmulqdq"]),
    ("widekl", Stable, &["kl"]),
    ("x87", Unstable("x87_target_feature"), &[]),
    ("xop", Unstable("xop_target_feature"), &["fma4", "avx", "sse4a"]),
    ("xsave", Stable, &[]),
    ("xsavec", Stable, &["xsave"]),
    ("xsaveopt", Stable, &["xsave"]),
    ("xsaves", Stable, &["xsave"]),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch { X86_64, AArch64 }

/// The architecture of the session's target triple (Session::target).
pub fn target_arch(prog: &Program) -> Arch {
    if prog.session.options.target.starts_with("aarch64") { Arch::AArch64 } else { Arch::X86_64 }
}

/// Target::rust_target_features (target_features.rs:1183-1204).
pub fn rust_target_features(arch: Arch) -> &'static [(&'static str, Stability, ImpliedFeatures)] {
    match arch { Arch::X86_64 => X86_FEATURES, Arch::AArch64 => AARCH64_FEATURES }
}

fn lookup(arch: Arch, name: &str) -> Option<&'static (&'static str, Stability, ImpliedFeatures)> {
    rust_target_features(arch).iter().find(|f| f.0 == name)
}

/// Target::tied_target_features (target_features.rs:1250).
pub fn tied_target_features(arch: Arch) -> &'static [&'static [&'static str]] {
    match arch { Arch::X86_64 => &[], Arch::AArch64 => AARCH64_TIED_FEATURES }
}

/// Target::implied_target_features (target_features.rs:1259-1277): the transitive closure,
/// the base feature included. The `implied_target_features` provider sorts it
/// (codegen_ssa target_features.rs:581-593).
pub fn implied_target_features(arch: Arch, base_feature: &'static str) -> Vec<&'static str> {
    let mut features: Vec<&'static str> = Vec::new();
    let mut new_features = vec![base_feature];
    while let Some(new_feature) = new_features.pop() {
        if !features.contains(&new_feature) {
            features.push(new_feature);
            let (_, _, implied) = lookup(arch, new_feature).unwrap_or_else(|| panic!("encountered non-Rust target feature {new_feature}"));
            new_features.extend(implied.iter().copied());
        }
    }
    features.sort_unstable();
    features
}

/// Target::abi_required_features (target_features.rs:1289-1383) for the hard-float ABIs of
/// Relax's targets (x86_64: rustc_abi None; aarch64: rustc_abi None): (required, incompatible).
pub fn abi_required_features(arch: Arch) -> (&'static [&'static str], &'static [&'static str]) {
    match arch {
        Arch::X86_64 => (&["x87", "sse2"], &["soft-float"]),
        Arch::AArch64 => (&["neon"], &[]),
    }
}

/// check_tied_features (codegen_ssa target_features.rs:420-434): every listed feature is
/// enabled here, so a tie is broken when one of its members is missing.
pub fn check_tied_features(arch: Arch, enabled: &[&str]) -> Option<&'static [&'static str]> {
    if !enabled.is_empty() {
        for tied in tied_target_features(arch) {
            let first = enabled.contains(&tied[0]);
            if tied[1..].iter().any(|f| enabled.contains(f) != first) { return Some(tied); }
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetFeatureKind { Implied, Enabled }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TargetFeature { pub name: &'static str, pub kind: TargetFeatureKind }

/// The `#[target_feature]` part of CodegenFnAttrs (middle/codegen_fn_attrs.rs:100).
#[derive(Clone, Default, Debug)]
pub struct TargetFeatureAttrs {
    pub target_features: Vec<TargetFeature>,
    pub safe_target_features: bool,
    /// source position of the (first) attribute, for diagnostics
    pub attr_pos: Option<u32>,
}

/// parse_tf_attribute (codegen_attrs.rs:507-540) for every `#[target_feature(..)]` of an item:
/// (feature, source position) in order. Errors are rustc's attribute-parse errors.
pub fn parse_target_feature_attrs(prog: &Program, file: u32, attrs: &[Attr], errors: &mut Vec<(u32, String)>) -> (Vec<(String, u32)>, Option<u32>) {
    let src = prog.src(file);
    let mut features = Vec::new();
    let mut first = None;
    for attr in attrs {
        let lo = attr.toks.lo as usize;
        let hi = attr.toks.hi as usize;
        if hi <= lo || crate::cfg::tok_str(src, lo) != b"target_feature" { continue; }
        first.get_or_insert(src.tok(lo).lo);
        // expect_list
        if hi < lo + 3 || src.tok(lo + 1).kind != T::OpenParen || src.tok(hi - 1).kind != T::CloseParen {
            errors.push((src.tok(lo).lo, "malformed `target_feature` attribute input: expected `#[target_feature(enable = \"feat1, feat2\")]`".into()));
            continue;
        }
        if hi == lo + 3 {
            // warn_empty_attribute: a warning, no features
            continue;
        }
        let mut i = lo + 2;
        while i < hi - 1 {
            // expect_name_value(item, Some(enable)) + expect_string_literal
            if src.tok(i).kind != T::Ident || i + 2 >= hi || src.tok(i + 1).kind != T::Eq {
                errors.push((src.tok(i).lo, "malformed `target_feature` attribute input: expected `enable = \"..\"`".into()));
                break;
            }
            if crate::cfg::tok_str(src, i) != b"enable" {
                errors.push((src.tok(i).lo, "malformed `target_feature` attribute input: expected `enable`".into()));
                break;
            }
            if !matches!(src.tok(i + 2).kind, T::Str | T::RawStr) {
                errors.push((src.tok(i + 2).lo, "malformed `target_feature` attribute input: expected a string literal".into()));
                break;
            }
            let value = crate::lit::unescape_str(std::str::from_utf8(crate::cfg::tok_str(src, i + 2)).unwrap_or(""));
            for feature in value.split(',') { features.push((feature.to_string(), src.tok(i).lo)); }
            i += 3;
            if i < hi - 1 {
                if src.tok(i).kind != T::Comma {
                    errors.push((src.tok(i).lo, "malformed `target_feature` attribute input".into()));
                    break;
                }
                i += 1;
            }
        }
    }
    (features, first)
}

/// from_target_feature_attr (codegen_ssa target_features.rs:22-174): validate each feature and
/// record it with its implied features. Relax enables no nightly features, so a feature that
/// needs one is the feature-gate error (E0658).
pub fn from_target_feature_attr(arch: Arch, features: &[(String, u32)], out: &mut Vec<TargetFeature>, errors: &mut Vec<(u32, String)>) {
    let (_, incompatible) = abi_required_features(arch);
    for (feature, pos) in features {
        let Some(&(name, stability, _)) = lookup(arch, feature) else {
            let hint = match feature.strip_prefix('+') {
                Some(stripped) if lookup(arch, stripped).is_some() => format!("; consider removing the leading `+` in the feature name: `{stripped}`"),
                _ => String::new(),
            };
            errors.push((*pos, format!("the feature named `{feature}` is not valid for this target{hint}")));
            continue;
        };
        if let Err(reason) = stability.toggle_allowed() {
            errors.push((*pos, format!("target feature `{feature}` cannot be enabled with `#[target_feature]`: {reason}")));
        } else if stability.requires_nightly().is_some() {
            let explain = if stability.is_cfg_stable_toggle_unstable() {
                format!("the target feature `{feature}` is allowed in cfg but unstable otherwise")
            } else {
                format!("the target feature `{feature}` is currently unstable")
            };
            errors.push((*pos, format!("E0658: {explain}")));
        } else {
            for implied in implied_target_features(arch, name) {
                if incompatible.contains(&implied) {
                    // "neon" on aarch64 and "sse" on x86 are future-compat lints there; neither
                    // is incompatible on Relax's hard-float targets.
                    errors.push((*pos, format!("target feature `{implied}` cannot be enabled with `#[target_feature]`: this feature is incompatible with the target ABI")));
                }
                let kind = if implied != name { TargetFeatureKind::Implied } else { TargetFeatureKind::Enabled };
                out.push(TargetFeature { name: implied, kind });
                if !lookup(arch, implied).is_some_and(|f| f.1.toggle_allowed().is_ok()) {
                    panic!("internal-only feature {implied} should not be toggled by `#[target_feature]`");
                }
            }
        }
    }
}

/// TyCtxt::is_target_feature_call_safe (middle/ty/context.rs:1252-1265); no Relax target is
/// wasm-like.
pub fn is_target_feature_call_safe(callee_features: &[TargetFeature], body_features: &[TargetFeature]) -> bool {
    callee_features.iter().all(|feature| body_features.iter().any(|f| f.name == feature.name))
}

crate::collected_enum!(TargetFeatureKind; 0 => Implied, 1 => Enabled);
impl crate::crate_image::Codec for TargetFeature {
    fn put(&self, w: &mut Vec<u8>) {
        self.name.to_string().put(w); self.kind.put(w);
    }
    fn get(r: &mut crate::crate_image::Decode<'_>) -> Option<Self> {
        use crate::crate_image::Codec;
        let name = String::get(r)?;
        let name = X86_FEATURES.iter().chain(AARCH64_FEATURES).find(|f| f.0 == name)?.0;
        Some(Self { name, kind: Codec::get(r)? })
    }
}
