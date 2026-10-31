//! Named-child slots on widgets: a template's named children are slots an instance reaches with
//! `slot +: {..}` (merge, add children), `slot := V` (replace in place) and the dotted forms
//! (`body.slot +: {..}`, `split.a.left := V`); a declared property is not a child.
use makepad_widgets::makepad_script::script;
use makepad_widgets::*;

/// `id(text)[children]` for each child, comma separated, ids as written in the document.
fn tree(w: &WidgetRef) -> String {
    let mut kids = Vec::new();
    w.children(&mut |id, c| kids.push((id, c)));
    kids.iter()
        .map(|(id, c)| {
            let mut s = format!("{id}");
            let text = c.text();
            if !text.is_empty() {
                s.push_str(&format!("({text})"));
            }
            let inner = tree(c);
            if !inner.is_empty() {
                s.push_str(&format!("[{inner}]"));
            }
            s
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Builds a document into a widget; the tree and the errors of the evaluation and the build.
fn build(cx: &mut Cx, doc: ScriptValue) -> (String, Vec<String>) {
    let mut errs = cx.with_vm(|vm| vm.take_errors());
    let w = cx.with_vm(|vm| WidgetRef::script_from_value(vm, doc));
    errs.extend(cx.with_vm(|vm| vm.take_errors()));
    (tree(&w), errs)
}

fn ok(cx: &mut Cx, doc: ScriptValue) -> String {
    let (t, errs) = build(cx, doc);
    assert!(errs.is_empty(), "{errs:?}");
    t
}

fn new_cx() -> Cx {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| makepad_widgets::script_mod(vm));
    cx.with_vm(|vm| vm.capture_errors());
    cx
}

#[test]
fn slots_on_a_template_with_named_children() {
    let mut cx = new_cx();
    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        let Shell = View{
            title := Label{ text: "T" }
            slot := View{ a := Label{ text: "A" } }
        }
        Shell{ slot +: { b := Label{ text: "B" } } }
    }));
    assert_eq!(ok(&mut cx, d), "title(T),slot[a(A),b(B)]", "merge +: adds a child to the slot");

    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        let Shell = View{
            title := Label{ text: "T" }
            slot := View{ a := Label{ text: "A" } }
        }
        Shell{ slot := View{ c := Label{ text: "C" } } }
    }));
    assert_eq!(ok(&mut cx, d), "title(T),slot[c(C)]", ":= replaces the slot in place");

    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        let Shell = View{
            title := Label{ text: "T" }
            slot := View{ a := Label{ text: "A" } }
        }
        Shell{ slot.a.text: "A2" title.text: "T2" }
    }));
    assert_eq!(ok(&mut cx, d), "title(T2),slot[a(A2)]", "path properties");

    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        let Shell = View{
            title := Label{ text: "T" }
            slot := View{ a := Label{ text: "A" } }
        }
        let Page = Shell{ slot +: { b := Label{ text: "B" } } }
        View{ p1 := Page{} p2 := Shell{ slot +: { b := Label{ text: "B2" } } } }
    }));
    assert_eq!(
        ok(&mut cx, d),
        "p1[title(T),slot[a(A),b(B)]],p2[title(T),slot[a(A),b(B2)]]",
        "two instances of one shell are independent"
    );
}

#[test]
fn dotted_slots_through_a_nested_body() {
    let mut cx = new_cx();
    let shell = |cx: &mut Cx, doc: ScriptValue| ok(cx, doc);
    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        let Shell = View{
            title := Label{ text: "T" }
            body := View{ slot := View{ a := Label{ text: "A" } } }
        }
        Shell{ body.slot +: { b := Label{ text: "B" } } }
    }));
    assert_eq!(shell(&mut cx, d), "title(T),body[slot[a(A),b(B)]]", "dotted +:");

    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        let Shell = View{
            title := Label{ text: "T" }
            body := View{ slot := View{ a := Label{ text: "A" } } }
        }
        Shell{ body +: { slot := View{ c := Label{ text: "C" } } } }
    }));
    assert_eq!(shell(&mut cx, d), "title(T),body[slot[c(C)]]", "nested block replace");

    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        let Shell = View{
            title := Label{ text: "T" }
            body := View{ slot := View{ a := Label{ text: "A" } } }
        }
        Shell{ body.slot := View{ c := Label{ text: "C" } } }
    }));
    assert_eq!(shell(&mut cx, d), "title(T),body[slot[c(C)]]", "dotted :=");

    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        let Shell = View{
            title := Label{ text: "T" }
            body := View{ slot := View{ a := Label{ text: "A" } } }
        }
        Shell{ body.extra := Label{ text: "E" } }
    }));
    assert_eq!(shell(&mut cx, d), "title(T),body[slot[a(A)],extra(E)]", "dotted := appends a new child");
}

#[test]
fn slots_through_a_typed_field_of_a_registered_shell() {
    let mut cx = new_cx();
    cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        mod.widgets.RegShell = View{
            title := Label{ text: "T" }
            split := Splitter{
                a: View{ left := View{ a1 := Label{ text: "L" } } }
                b: View{ right := View{ b1 := Label{ text: "R" } } }
            }
        }
    }));
    let base = reg(&mut cx, 0);
    let merged = reg(&mut cx, 1);
    let dotted = reg(&mut cx, 2);
    assert_eq!(merged, dotted, "split.a.left +: is the nested-block form");
    assert!(merged.contains("x(X)"), "{merged}");
    assert_ne!(base, merged, "{base}");
    let replaced = reg(&mut cx, 3);
    let replaced_dotted = reg(&mut cx, 4);
    assert_eq!(replaced, replaced_dotted, "split.a.left := is the nested-block form");
    assert!(replaced.contains("y(Y)") && !replaced.contains("a1"), "{replaced}");
    let prop = reg(&mut cx, 5);
    assert!(prop.contains("a1(L2)"), "{prop}");
    // two instances of the registered shell do not see each other's override
    let two = reg(&mut cx, 6);
    assert!(two.contains("one[") && two.contains("two["), "{two}");
    assert_eq!(two.matches("x(X)").count(), 1, "{two}");
}

fn reg(cx: &mut Cx, which: usize) -> String {
    let d = reg_doc(cx, which);
    ok(cx, d)
}

fn reg_doc(cx: &mut Cx, which: usize) -> ScriptValue {
    cx.with_vm(|vm| match which {
        0 => vm.eval(script! {
            use mod.prelude.widgets.*
            mod.widgets.RegShell{}
        }),
        1 => vm.eval(script! {
            use mod.prelude.widgets.*
            mod.widgets.RegShell{ split +: { a +: { left +: { x := Label{ text: "X" } } } } }
        }),
        2 => vm.eval(script! {
            use mod.prelude.widgets.*
            mod.widgets.RegShell{ split.a.left +: { x := Label{ text: "X" } } }
        }),
        3 => vm.eval(script! {
            use mod.prelude.widgets.*
            mod.widgets.RegShell{ split +: { a +: { left := View{ y := Label{ text: "Y" } } } } }
        }),
        4 => vm.eval(script! {
            use mod.prelude.widgets.*
            mod.widgets.RegShell{ split.a.left := View{ y := Label{ text: "Y" } } }
        }),
        5 => vm.eval(script! {
            use mod.prelude.widgets.*
            mod.widgets.RegShell{ split.a.left.a1.text: "L2" }
        }),
        _ => vm.eval(script! {
            use mod.prelude.widgets.*
            View{
                one := mod.widgets.RegShell{ split.a.left +: { x := Label{ text: "X" } } }
                two := mod.widgets.RegShell{}
            }
        }),
    })
}

#[test]
fn a_name_that_is_not_a_direct_child_is_an_error() {
    let mut cx = new_cx();
    cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        mod.widgets.ErrShell = View{
            title := Label{ text: "T" }
            split := Splitter{
                a: View{ left := View{ a1 := Label{ text: "L" } } }
                b: View{ right := View{ b1 := Label{ text: "R" } } }
            }
        }
    }));
    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        mod.widgets.ErrShell{ left +: { x := Label{ text: "X" } } }
    }));
    let (_, errs) = build(&mut cx, d);
    assert!(
        errs.iter().any(|e| e.contains("no property or named child `left` on View (named children: title, split); `left` is at split.a.left")),
        "{errs:?}"
    );
}

#[test]
fn a_declared_property_is_not_a_child() {
    let mut cx = new_cx();
    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        Splitter{ a := View{} }
    }));
    let (_, errs) = build(&mut cx, d);
    assert!(
        errs.iter().any(|e| e.contains("`a` is a property of Splitter, not a child: write `a: ...` or `a +: {...}`")),
        "{errs:?}"
    );
    // the property forms work
    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        Splitter{ a: View{ k := Label{ text: "K" } } }
    }));
    let (_, errs) = build(&mut cx, d);
    assert!(errs.is_empty(), "{errs:?}");
    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        Splitter{ a +: { k := Label{ text: "K" } } }
    }));
    let (_, errs) = build(&mut cx, d);
    assert!(errs.is_empty(), "{errs:?}");    // a plain value under a property's name stays a named entry hosts read as metadata
    // (Stage faces: `bind := "node.port"` on a Slider, which has its own `bind`)
    let d = cx.with_vm(|vm| vm.eval(script! {
        use mod.prelude.widgets.*
        Slider{ bind := "deck.gain" }
    }));
    let (_, errs) = build(&mut cx, d);
    assert!(errs.is_empty(), "{errs:?}");
}
