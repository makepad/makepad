//! End-to-end probe/regression for the `/** ... */` doc-annotation channel
//! (ONE form; position determines meaning): Rust lexes doc comments into
//! `#[doc]` tokens; the script! macro folds `/** */` back verbatim and
//! demotes `///` to plain `//` (`script.rs::tp_to_str`); the splash
//! tokenizer captures the blocks (`ScriptTokenizer::docs`); objects record
//! their construction ip (`ScriptObjectData::made_at`); the cascade query
//! (`ScriptVm::construction_chain` + `resolve_body_value_names` +
//! `parse_doc_hint`) resolves docs per prototype level, names per inline
//! literal, and range/step hints.
//!
//! Historical note: before the macro fix this file did not compile —
//! `error[E0423]: expected value, found built-in attribute 'doc'` at every
//! doc comment, because the interpolation branch swallowed any `#`+group
//! without checking the delimiter.

use makepad_script::*;

fn new_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn main() {
    let vm = &mut new_vm();
    let sm = script! {
        use mod.std.assert
        /** the base's doc line */
        let Base = {
            /** the base color */
            color: 2.0
        }
        /** the thing's doc line
         * and its second line */
        let thing = Base{
            /** the color field's doc line */
            color: 1.0
            /** corner radius 0..24 step 0.5 */
            radius: 3.0
            speed: /**pulse speed 0..2 step 0.05*/ 0.35
            gap: /**gap size*/ 4.0
            /// legacy line docs are NOT annotations
            plain: 7.0
        }
        assert(/**meaning*/ 42 == 42)
        assert(thing.color == 1.0)
        thing
    };
    println!("--- embedded code string ---");
    println!("{}", sm.code);
    assert!(
        sm.code.contains("/** the thing's doc line"),
        "/** */ blocks must fold back into the embedded source"
    );
    assert!(
        sm.code.contains("speed: /**pulse speed 0..2 step 0.05*/ 0.35"),
        "value-position /**name*/ forms must fold back inline"
    );
    assert!(
        sm.code.contains("// legacy line docs are NOT annotations"),
        "/// must demote to a plain // comment"
    );
    assert_eq!(sm.values.len(), 0, "docs must not become interpolations");

    let v = vm.eval(sm);
    let obj = v.as_object().expect("script must return the thing object");

    println!("--- construction chain of `thing` ---");
    let chain = vm.construction_chain(v);
    for (i, lvl) in chain.iter().enumerate() {
        println!(
            "level {i}: made_at body {} index {} loc {:?}",
            lvl.made_at.body, lvl.made_at.index, lvl.loc
        );
        println!("  doc: {:?}", lvl.doc);
        for (f, d) in &lvl.field_docs {
            println!("  field {f}: {d:?}");
        }
        println!("  own_keys: {:?}", lvl.own_keys);
    }
    assert_eq!(chain.len(), 2, "thing -> Base, then non-object proto");
    let thing_doc = chain[0].doc.as_deref().expect("thing has an object doc");
    assert!(thing_doc.contains("the thing's doc line"));
    assert!(
        thing_doc.contains("and its second line"),
        "multiline blocks keep their lines (gutter * stripped)"
    );
    assert!(
        !thing_doc.contains('*'),
        "the * gutter must be stripped: {thing_doc:?}"
    );
    assert!(
        chain[0]
            .field_docs
            .iter()
            .any(|(f, d)| *f == id!(color) && d.contains("color field's doc line")),
        "field doc attaches to the enclosing literal's key"
    );
    let radius_doc = chain[0]
        .field_docs
        .iter()
        .find(|(f, _)| *f == id!(radius))
        .map(|(_, d)| d.clone())
        .expect("radius carries a field doc");
    let hint = parse_doc_hint(&radius_doc);
    assert_eq!(hint.name, "corner radius");
    assert_eq!(hint.min, Some(0.0));
    assert_eq!(hint.max, Some(24.0));
    assert_eq!(hint.step, Some(0.5));
    assert!(
        !chain[0].field_docs.iter().any(|(f, _)| *f == id!(plain)),
        "/// must not produce a field doc"
    );
    assert_eq!(
        chain[1].doc.as_deref(),
        Some("the base's doc line"),
        "prototype level carries its own doc"
    );
    assert!(chain[0].loc.is_some(), "made_at resolves to a source loc");

    println!("--- value-position names ---");
    let body = vm.bx.heap.made_at(obj).body;
    let names = vm.bx.code.resolve_body_value_names(body);
    for vn in &names {
        println!("value-name @tok{}: {}", vn.token, vn.name);
    }
    let speed = names
        .iter()
        .find(|vn| vn.name.starts_with("pulse speed"))
        .expect("speed literal is named");
    let hint = parse_doc_hint(&speed.name);
    assert_eq!(hint.name, "pulse speed");
    assert_eq!(hint.min, Some(0.0));
    assert_eq!(hint.max, Some(2.0));
    assert_eq!(hint.step, Some(0.05));
    for e in ["gap size", "meaning"] {
        assert!(
            names.iter().any(|vn| vn.name == e),
            "missing value name: {e}"
        );
    }
    println!("--- all doc-channel assertions passed ---");
}
