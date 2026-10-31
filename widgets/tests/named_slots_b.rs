//! Scoped lookups: a path enters a scope node by naming it, so two instances of one shell
//! keep their own same-named children apart.
use makepad_widgets::makepad_script::script;
use makepad_widgets::*;

fn build(cx: &mut Cx, scoped: bool) -> WidgetRef {
    cx.with_vm(|vm| makepad_widgets::script_mod(vm));
    cx.with_vm(|vm| vm.capture_errors());
    let doc = if scoped {
        cx.with_vm(|vm| {
            vm.eval(script! {
                use mod.prelude.widgets.*
                let Shell = View{
                    search_scope: true
                    title := Label{ text: "T" }
                    slot := View{ chat := Label{ text: "c" } }
                }
                View{
                    p1 := Shell{ slot +: { chat.text: "one" } }
                    p2 := Shell{ slot +: { chat.text: "two" } }
                }
            })
        })
    } else {
        cx.with_vm(|vm| {
            vm.eval(script! {
                use mod.prelude.widgets.*
                let Shell = View{
                    title := Label{ text: "T" }
                    slot := View{ chat := Label{ text: "c" } }
                }
                View{
                    p1 := Shell{ slot +: { chat.text: "one" } }
                    p2 := Shell{ slot +: { chat.text: "two" } }
                }
            })
        })
    };
    let w = cx.with_vm(|vm| WidgetRef::script_from_value(vm, doc));
    let errs = cx.with_vm(|vm| vm.take_errors());
    assert!(errs.is_empty(), "script errors: {:?}", errs);
    w
}

fn texts(cx: &Cx, set: WidgetSet) -> Vec<String> {
    let _ = cx;
    set.iter().map(|w| w.text()).collect()
}

#[test]
fn scoped_instances_are_addressed_by_name() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let root = build(&mut cx, true);

    // the scope walls hide the shells' children from an unnamed search
    assert!(root.widget(&cx, ids!(chat)).is_empty());
    assert!(root.widget(&cx, ids!(slot.chat)).is_empty());
    assert_eq!(texts(&cx, root.widgets(&cx, &[ids!(chat)])).len(), 0);

    // naming the scope enters it, twice over (second call is warm)
    for _ in 0..2 {
        let c1 = root.widget(&cx, ids!(p1.chat));
        let c2 = root.widget(&cx, ids!(p2.chat));
        assert!(!c1.is_empty() && !c2.is_empty());
        assert_ne!(c1.widget_uid(), c2.widget_uid());
        assert_eq!(c1.text(), "one");
        assert_eq!(c2.text(), "two");
    }
    // a second lookup is a cache hit: no node is walked
    let walked = cx.widget_tree().stats().walk_nodes;
    assert_eq!(root.widget(&cx, ids!(p1.chat)).text(), "one");
    assert_eq!(cx.widget_tree().stats().walk_nodes, walked);
    let nested = root.widget(&cx, ids!(p2.slot.chat));
    assert_eq!(nested.text(), "two");
    assert_eq!(root.widget(&cx, ids!(p1.slot.chat)).text(), "one");
    assert!(root.widget(&cx, ids!(p3.chat)).is_empty());

    // a scope reached through the shell's own handle searches inside it
    let p2 = root.widget(&cx, ids!(p2));
    assert_eq!(p2.widget(&cx, ids!(chat)).text(), "two");

    // flood from inside p2 finds p2's chat first
    let title2 = root.widget(&cx, ids!(p2.title));
    assert!(!title2.is_empty());
    assert_eq!(title2.widget_flood(&cx, ids!(chat)).text(), "two");
    let title1 = root.widget(&cx, ids!(p1.title));
    assert_eq!(title1.widget_flood(&cx, ids!(chat)).text(), "one");
    // flood from the root reaches a scope by name as well
    assert_eq!(root.widget_flood(&cx, ids!(p1.chat)).text(), "one");

    // the set variants agree
    assert_eq!(texts(&cx, root.widgets(&cx, &[ids!(p1.chat)])), vec!["one"]);
    assert_eq!(texts(&cx, root.widgets(&cx, &[ids!(p2.chat)])), vec!["two"]);
    assert_eq!(texts(&cx, title2.widgets_flood(&cx, &[ids!(chat)])), vec!["two"]);
    assert_eq!(texts(&cx, root.widgets(&cx, &[ids!(p1.chat), ids!(p2.chat)])), vec!["one", "two"]);
}

#[test]
fn unscoped_shells_keep_first_match() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let root = build(&mut cx, false);
    assert_eq!(root.widget(&cx, ids!(chat)).text(), "one");
    assert_eq!(texts(&cx, root.widgets(&cx, &[ids!(chat)])), vec!["one", "two"]);
    assert_eq!(root.widget(&cx, ids!(p2.chat)).text(), "two");
}
