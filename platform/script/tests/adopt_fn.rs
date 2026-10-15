//! A fn moves between VMs by its tokens (`fn_image`, `adopt_fn`): the
//! adopting VM runs it with its own names, and its errors name the place
//! it is written.

use makepad_script::tokenizer::ScriptTokenTag;
use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) }
}

fn eval(vm: &mut ScriptVm, file: &str, code: &str) -> ScriptValue {
    vm.capture_errors();
    let v = vm.eval(ScriptMod::new("", "", file, 0, 0, code, &[]));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{errors:?}");
    v
}

#[test]
fn a_fn_moves_to_another_vm_by_its_tokens() {
    let mut doc = test_vm();
    let fns = eval(&mut doc, "doc.splash", "let k = 100.0\n{\n    plain: fn(x) { return x * 2.0 + K }\n    closure: |a, b| a * b + K\n    words: fn() { let s = \"two words\" return s.len() }\n    named: fn twice(x) { x + x }\n}\n").as_object().unwrap();
    let image = |doc: &ScriptVm, k: &str| doc.fn_image(doc.value(fns, LiveId::from_str(k)).as_object().unwrap()).unwrap();
    let images: Vec<(&str, ScriptFnImage)> = ["plain", "closure", "words", "named"].into_iter().map(|k| (k, image(&doc, k))).collect();
    assert_eq!(images[0].1.text(), "fn(x) { return x * 2.0 + K }");
    assert_eq!((images[0].1.line(), images[0].1.col()), (2, 11));
    assert_eq!(images[1].1.text(), "|a, b| a * b + K");
    drop(doc);
    // Another VM, its own `K`.
    let mut vm = test_vm();
    let scope = eval(&mut vm, "host.splash", "let K = 1.0\n{K: K}").as_object().unwrap();
    let parent = vm.body_end_scope((vm.body_count() - 1) as u16).unwrap();
    let _ = scope;
    let f = |vm: &mut ScriptVm, k: usize| vm.adopt_fn(&images[k].1, parent).unwrap_or_else(|| panic!("{} adopts", images[k].0));
    let plain = f(&mut vm, 0);
    assert_eq!(vm.call(plain.into(), &[3.0.into()]).as_number(), Some(7.0));
    let closure = f(&mut vm, 1);
    assert_eq!(vm.call(closure.into(), &[3.0.into(), 4.0.into()]).as_number(), Some(13.0));
    let words = f(&mut vm, 2);
    assert_eq!(vm.call(words.into(), &[]).as_number(), Some(9.0));
    let named = f(&mut vm, 3);
    assert_eq!(vm.call(named.into(), &[5.0.into()]).as_number(), Some(10.0));
    // Its own place: the adopted body reports the fn's file and line.
    let ip = vm.bx.heap.fn_script_ip(plain).unwrap();
    let loc = vm.ip_to_loc(ip).unwrap();
    assert!((loc.file(), loc.line()) == ("doc.splash", 2), "{loc:?}");
}

fn image_of(code: &str, file: &str, key: &str) -> ScriptFnImage {
    let mut doc = test_vm();
    let obj = eval(&mut doc, file, code).as_object().unwrap();
    doc.fn_image(doc.value(obj, LiveId::from_str(key)).as_object().unwrap()).unwrap()
}

/// An adopting VM with a `K` of its own, and the scope an adopted fn is parented to.
fn host_vm() -> (ScriptVm<'static>, ScriptObject) {
    let mut vm = test_vm();
    eval(&mut vm, "host.splash", "let K = 1.0\n{K: K}");
    let parent = vm.body_end_scope((vm.body_count() - 1) as u16).unwrap();
    (vm, parent)
}

#[test]
fn an_adopted_fns_errors_name_the_place_it_is_written() {
    let image = image_of("let pad = 1\n{\n    boom: fn(x) { return x.nope }\n}\n", "lib/race_hud.splash", "boom");
    assert_eq!(image.file(), "lib/race_hud.splash");
    let (mut vm, parent) = host_vm();
    let boom = vm.adopt_fn(&image, parent).unwrap();
    vm.capture_errors();
    let _ = vm.call(boom.into(), &[ScriptValue::from(3.0)]);
    let errors = vm.take_errors();
    assert!(!errors.is_empty());
    // The fn is written at line 2, column 10 (0-based) of its own file.
    assert!(errors[0].starts_with("lib/race_hud.splash:2:"), "{errors:?}");
    assert!(!errors[0].contains("adopted://"), "{errors:?}");
    let ip = vm.bx.heap.fn_script_ip(boom).unwrap();
    let loc = vm.ip_to_loc(ip).unwrap();
    assert_eq!((loc.file(), loc.line()), ("lib/race_hud.splash", 2), "{loc:?}");
}

#[test]
fn an_unchanged_fn_keeps_its_hash_and_a_changed_one_does_not() {
    let code = "let pad = 1\n{\n    f: fn(x) { return x + 1.0 }\n}\n";
    let a = image_of(code, "lib/a.splash", "f");
    // Evaluated again, in another VM.
    let b = image_of(code, "lib/a.splash", "f");
    assert_eq!(a.hash(), b.hash());
    // Unrelated edits elsewhere in the file do not change it when it stays where it is.
    let c = image_of("let pad = 2\n{\n    f: fn(x) { return x + 1.0 }\n    g: 7\n}\n", "lib/a.splash", "f");
    assert_eq!(a.hash(), c.hash());
    let d = image_of("let pad = 1\n{\n    f: fn(x) { return x + 2.0 }\n}\n", "lib/a.splash", "f");
    assert_ne!(a.hash(), d.hash());
    // Adopting twice into one VM reuses the body and keeps working.
    let (mut vm, parent) = host_vm();
    let f1 = vm.adopt_fn(&a, parent).unwrap();
    let f2 = vm.adopt_fn(&b, parent).unwrap();
    assert_eq!(vm.call(f1.into(), &[1.0.into()]).as_number(), Some(2.0));
    assert_eq!(vm.call(f2.into(), &[1.0.into()]).as_number(), Some(2.0));
}

#[test]
fn a_capture_fails_by_name_with_the_place_and_a_hint() {
    // `gun` exists only in the source VM's scope.
    let image = image_of("let gun = 5.0\n{\n    shoot: fn(x) { return x + gun }\n}\n", "game.splash", "shoot");
    let (mut vm, parent) = host_vm();
    let shoot = vm.adopt_fn(&image, parent).unwrap();
    vm.capture_errors();
    let _ = vm.call(shoot.into(), &[1.0.into()]);
    let errors = vm.take_errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let e = &errors[0];
    assert!(e.starts_with("game.splash:2:"), "{e}");
    assert!(e.contains("gun"), "{e}");
    assert!(e.contains("values from the game come in as inputs"), "{e}");
    // The general list of every name in scope is not in it.
    assert!(!e.contains("Did you mean"), "{e}");
}

#[test]
fn an_image_gives_its_tokens_with_their_text() {
    let image = image_of("let pad = 1\n{\n    f: fn(x) { use mod.speed\n return x + \"é\" + 2.5 }\n}\n", "t.splash", "f");
    let toks: Vec<(ScriptTokenTag, &str)> = image.tokens().map(|(t, s)| (t.token.kind(), s)).collect();
    let texts: Vec<&str> = toks.iter().map(|t| t.1).collect();
    assert_eq!(texts, ["fn", "(", "x", ")", "{", "use", "mod", ".", "speed", "return", "x", "+", "\"é\"", "+", "2.5", "}"]);
    assert_eq!(toks[5].0, ScriptTokenTag::Identifier);
    assert_eq!(toks[12].0, ScriptTokenTag::String);
    // Every token's text is the characters it spans in the image's text.
    let chars: Vec<char> = image.text().chars().collect();
    for (t, s) in image.tokens() {
        assert_eq!(chars[t.span()].iter().collect::<String>(), s);
    }
}

#[test]
fn a_fn_of_a_body_fed_by_appending_has_an_image() {
    // A game's source is appended to a module that carries no code of its
    // own (the streaming eval): its fns still have their text and place.
    let mut doc = test_vm();
    doc.capture_errors();
    let module = ScriptMod::new("", "", "game.splash", 1, 7, "", &[]);
    let obj = doc.eval_with_append_source(module, "let a = 1\n{f: || a + 41}\n;", ScriptObject::ZERO);
    assert!(doc.take_errors().is_empty());
    let f = doc.value(obj.as_object().expect("the last value"), LiveId::from_str("f")).as_object().unwrap();
    let image = doc.fn_image(f).expect("an appended body's fn has an image");
    assert_eq!(image.text(), "|| a + 41");
    assert_eq!(image.file(), "game.splash");
    assert_eq!(image.line(), 2, "the module starts at line 1: the fn is on its second line");
}
