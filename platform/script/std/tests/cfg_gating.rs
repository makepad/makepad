//! Tests for the `#[cfg(feature = "…")]` gating support inside `script_mod!`
//! (via the `script!` proc-macro which produces the same `ScriptMod` value).
//!
//! These tests pin invariants that came out of the spec:
//!   - No-cfg blocks emit `cfg_fragments: Vec::new()` and byte-identical code.
//!   - `feature = "<never-on>"` predicates evaluate to false, the gated text is
//!     dropped from `code`, and a `false` entry is recorded in `cfg_fragments`.
//!   - `not(...)` combinators flip the bool.
//!   - `#(…)` placeholder indices renumber globally against the live values vec
//!     so an unconditional `#(…)` placed after an excluded conditional doesn't
//!     leave a "ghost" `#(N)` reference.
//!   - The all-false-conditional case still goes through the with-conditional
//!     emission path (non-empty `cfg_fragments`).

// The macro-expanded code in with-conditional emission mode references
// `ScriptValue` and the `ScriptApply::script_to_value` trait method
// unqualified, mirroring production callers' `use makepad_script::*`.
#[allow(unused_imports)]
use makepad_script_std::makepad_script::{script, ScriptMod, *};

// Feature names that the script-std crate definitely does *not* define. Using
// `feature = "<never-on>"` lets us exercise the false branch without adding any
// feature to Cargo.toml.
const _: &str = "_makepad_script_mod_cfg_gating_never_on";

#[test]
fn no_cfg_block_has_empty_cfg_fragments() {
    let m: ScriptMod = script! {
        let A = 1
        let B = 2
    };
    assert!(m.cfg_fragments.is_empty(), "cfg_fragments must be empty for no-cfg blocks");
    assert!(m.code.contains("let A = 1"));
    assert!(m.code.contains("let B = 2"));
}

#[test]
fn cfg_gated_use_line_is_excluded_when_feature_off() {
    let m: ScriptMod = script! {
        let A = 1
        #[cfg(feature = "_makepad_script_mod_cfg_gating_never_on")]
        let B = 2
    };
    assert_eq!(m.cfg_fragments, vec![false]);
    assert!(m.code.contains("let A = 1"));
    assert!(!m.code.contains("let B = 2"), "gated text leaked into code: {}", m.code);
}

#[test]
fn not_combinator_flips_the_bool() {
    let m: ScriptMod = script! {
        let A = 1
        #[cfg(not(feature = "_makepad_script_mod_cfg_gating_never_on"))]
        let B = 2
    };
    assert_eq!(m.cfg_fragments, vec![true]);
    assert!(m.code.contains("let A = 1"));
    assert!(m.code.contains("let B = 2"), "expected always-on branch to be present: {}", m.code);
}

#[test]
fn any_and_all_combinators_compile_and_evaluate() {
    let m: ScriptMod = script! {
        #[cfg(any(feature = "_makepad_script_mod_cfg_gating_never_on", feature = "_also_never_on"))]
        let A = 1
        #[cfg(all(feature = "_makepad_script_mod_cfg_gating_never_on", feature = "_also_never_on"))]
        let B = 2
        #[cfg(not(all(feature = "_one_never", feature = "_two_never")))]
        let C = 3
    };
    assert_eq!(m.cfg_fragments, vec![false, false, true]);
    assert!(!m.code.contains("let A = 1"));
    assert!(!m.code.contains("let B = 2"));
    assert!(m.code.contains("let C = 3"));
}

#[test]
fn brace_grouped_form_strips_outer_braces() {
    let m: ScriptMod = script! {
        #[cfg(not(feature = "_makepad_script_mod_cfg_gating_never_on"))] {
            let A = 1
            let B = 2
        }
    };
    assert_eq!(m.cfg_fragments, vec![true]);
    // Outer braces must NOT appear at the top level of the emitted code — the
    // DSL parser doesn't accept a `{ ... }` group at the top level.
    let trimmed = m.code.trim_start();
    assert!(
        !trimmed.starts_with('{'),
        "outer braces leaked into emitted code: {:?}",
        m.code
    );
    assert!(m.code.contains("let A = 1"));
    assert!(m.code.contains("let B = 2"));
}

#[test]
fn brace_grouped_excluded_drops_entire_contents() {
    let m: ScriptMod = script! {
        let Keep = 1
        #[cfg(feature = "_makepad_script_mod_cfg_gating_never_on")] {
            let A = 1
            let B = 2
        }
        let AlsoKeep = 3
    };
    assert_eq!(m.cfg_fragments, vec![false]);
    assert!(m.code.contains("let Keep = 1"));
    assert!(m.code.contains("let AlsoKeep = 3"));
    assert!(!m.code.contains("let A = 1"));
    assert!(!m.code.contains("let B = 2"));
}

#[test]
fn all_false_conditional_sweep_still_uses_with_conditional_path() {
    // Body contains only conditionals where every cfg resolves false. The
    // with-conditional emission path should still be taken — `cfg_fragments`
    // must be non-empty and all `false`, and the no-cfg fast path must NOT
    // accidentally run (which would bake the gated text into a static string).
    let m: ScriptMod = script! {
        let Always = 1
        #[cfg(feature = "_never_a")]
        let A = 2
        #[cfg(feature = "_never_b")]
        let B = 3
    };
    assert_eq!(m.cfg_fragments, vec![false, false]);
    assert!(m.code.contains("let Always = 1"));
    assert!(!m.code.contains("let A = 2"));
    assert!(!m.code.contains("let B = 3"));
}

#[test]
fn placeholder_renumbering_integrity() {
    // An unconditional `#(...)` placeholder appearing AFTER an excluded
    // conditional must reference the next live `__values` index, not a "ghost"
    // index that includes the excluded value.
    use makepad_script_std::makepad_network::NetworkRuntime;
    use makepad_script_std::makepad_script::ScriptVmBase;
    use makepad_script_std::{with_vm, ScriptStd};
    use makepad_script_std::makepad_network::NetworkConfig;
    use std::sync::Arc;

    let runtime = Arc::new(NetworkRuntime::new(NetworkConfig::default()));
    let mut std_state = ScriptStd::with_network_runtime(runtime);
    let mut script_vm = Some(Box::new(ScriptVmBase::new()));
    let m = with_vm(&mut (), &mut std_state, &mut script_vm, |vm| {
        // Three placeholders in source. With feature off, only the first and
        // third (unconditional) contribute values.
        script! {
            before := Foo { val: #(1.0_f64) }
            #[cfg(feature = "_makepad_script_mod_cfg_gating_never_on")]
            middle := Bar { val: #(2.0_f64) }
            after := Baz { val: #(3.0_f64) }
        }
    });

    // 2 placeholders are active (`1.0` and `3.0`); the conditional one (`2.0`)
    // is excluded. The emitted code should reference `#(0)` and `#(1)` only —
    // no `#(2)` ghost.
    assert_eq!(m.values.len(), 2, "expected 2 values, got code: {}", m.code);
    assert_eq!(m.cfg_fragments, vec![false]);
    assert!(m.code.contains("#(0)"), "code missing #(0): {}", m.code);
    assert!(m.code.contains("#(1)"), "code missing #(1): {}", m.code);
    assert!(!m.code.contains("#(2)"), "code has ghost #(2): {}", m.code);
}

#[test]
fn whitespace_gap_preserved_between_pre_text_and_active_conditional() {
    // Regression: if the pre-text ends with an identifier and the conditional
    // body starts with another identifier, the two must be separated by at
    // least one whitespace character — otherwise the DSL parser sees a single
    // smashed token (e.g. `mod.state = statelet pro_label_text` instead of
    // `mod.state = state` followed by `let pro_label_text`).
    let m: ScriptMod = script! {
        mod.state = state
        #[cfg(not(feature = "_makepad_script_mod_cfg_gating_never_on"))]
        let pro_label_text = "ON"
        startup() do mod.foo {}
    };
    assert_eq!(m.cfg_fragments, vec![true]);
    // `state` and `let` must NOT be adjacent — there must be whitespace between
    // the pre-text's last identifier and the conditional body's first token.
    assert!(
        !m.code.contains("statelet"),
        "pre-text and active conditional smashed together: {}",
        m.code
    );
    assert!(m.code.contains("let pro_label_text"));
}

#[test]
fn whitespace_gap_preserved_between_consecutive_conditionals() {
    // Two cfg attributes back-to-back at the top level. When both are active,
    // their bodies must be separated; when one is active, that one must be
    // separated from neighbouring text.
    let m: ScriptMod = script! {
        let a = 1
        #[cfg(not(feature = "_never"))]
        let b = 2
        #[cfg(not(feature = "_never"))]
        let c = 3
        let d = 4
    };
    assert_eq!(m.cfg_fragments, vec![true, true]);
    // All four `let` bindings must appear as separate tokens — none of them
    // may be smashed together with a neighbour.
    for ident in ["let a", "let b", "let c", "let d"] {
        assert!(m.code.contains(ident), "missing `{}` in {:?}", ident, m.code);
    }
}

#[test]
fn cfg_attribute_count_matches_source_order() {
    let m: ScriptMod = script! {
        let A = 1
        #[cfg(feature = "_x")]
        let B = 2
        let C = 3
        #[cfg(not(feature = "_x"))]
        let D = 4
        #[cfg(feature = "_x")]
        let E = 5
    };
    // Three conditionals in source order. First and third are `feature = "_x"`
    // (false); second is `not(feature = "_x")` (true).
    assert_eq!(m.cfg_fragments.len(), 3);
    assert_eq!(m.cfg_fragments[0], false);
    assert_eq!(m.cfg_fragments[1], true);
    assert_eq!(m.cfg_fragments[2], false);
    assert!(m.code.contains("let A = 1"));
    assert!(!m.code.contains("let B = 2"));
    assert!(m.code.contains("let C = 3"));
    assert!(m.code.contains("let D = 4"));
    assert!(!m.code.contains("let E = 5"));
}
