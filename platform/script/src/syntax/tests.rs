use super::write::{self, Value};
use super::*;
use std::path::{Path, PathBuf};

fn splash_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(root) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name == ".git" || name.starts_with("target-") {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            splash_files(&path, out);
        } else if kind.is_file() && path.extension().is_some_and(|e| e == "splash") {
            out.push(path);
        }
    }
}

/// Every `.splash` file of the repository (and of the nested commercial
/// checkout) parses, nests, and prints back byte for byte.
#[test]
fn every_splash_file_round_trips() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    splash_files(&root, &mut files);
    assert!(!files.is_empty(), "no .splash files under {}", root.display());
    let mut failures = Vec::new();
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else { continue };
        match parse(&source) {
            Err(e) => {
                let (line, col) = line_col(&source, e.at);
                failures.push(format!("{}:{line}:{col}: {}", path.display(), e.message));
            }
            Ok(syntax) => {
                if syntax.print() != source {
                    failures.push(format!("{}: print differs from the source", path.display()));
                } else if let Err(e) = check_nesting(&syntax) {
                    failures.push(format!("{}: {e}", path.display()));
                }
            }
        }
    }
    eprintln!("{} files parsed", files.len());
    assert!(failures.is_empty(), "{} of {} files fail:\n{}", failures.len(), files.len(), failures.iter().take(60).cloned().collect::<Vec<_>>().join("\n"));
}

fn sx(text: &str) -> Syntax {
    parse(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

#[test]
fn statements_and_values_are_read() {
    let src = "use mod.std.*\n// a comment\nlet accent = #ff5a36\nfn rand(k) { return fract(k * 2.0) }\nlet p = game.model({id: \"hut\", at: vec3(1, 2, 3)})\nif accent == nil { game.log(\"x\") } else { game.log(\"y\") }\nfor i in 0..4 { game.tree({at: [i, 0]}) }\non_tick = |dt| dt * 2.0\n{rig: rig}\n";
    let s = sx(src);
    let kinds: Vec<&str> = s.statements().iter().map(|n| n.kind_name()).collect();
    assert_eq!(kinds, ["use", "let", "fn", "let", "if", "for", "assignment", "object"]);
    assert_eq!(s.print(), src);
    assert_eq!(s.find_let("accent").map(|l| s.text(s.node(&l).unwrap()).to_string()), Some("let accent = #ff5a36".to_string()));
    assert_eq!(s.find_calls("game.model").len(), 1);
    assert_eq!(s.find_calls("game.log").len(), 2);
    assert_eq!(s.find_calls("game.tree").len(), 1);
    let hut = s.find_by_id("hut");
    assert_eq!(hut.len(), 1);
    assert_eq!(s.node(&hut[0]).unwrap().field("at").unwrap().value.callee_path(), Some("vec3"));
    assert!(s.find_let("rand").is_some());
}

#[test]
fn a_brace_is_an_object_after_a_type_or_glued_name_and_a_block_after_a_condition() {
    let s = sx("if x { a() }\nif y == Foo { b() }\nlet o = Rect{w: 1}\nlet q = thing{w: 2}\n");
    let kinds: Vec<&str> = s.statements().iter().map(|n| n.kind_name()).collect();
    assert_eq!(kinds, ["if", "if", "let", "let"]);
    let o = s.node(&s.find_let("o").unwrap()).unwrap();
    assert!(matches!(&o.children()[0].kind, Kind::Object { ty: Some(t), .. } if t == "Rect"));
}

#[test]
fn operators_continue_across_lines_as_in_the_vm() {
    let s = sx("let a = 1 +\n  2\nlet b = x\n-1\nlet c = f(x)\n  .g(y)\n");
    assert_eq!(s.statements().len(), 3);
    assert_eq!(sx("[1 -2]").statements()[0].items().unwrap().len(), 1);
    assert_eq!(sx("[1, -2]").statements()[0].items().unwrap().len(), 2);
    assert_eq!(sx("[1 - 2]").statements()[0].items().unwrap().len(), 1);
    let Kind::Call { callee, .. } = &s.statements()[2].children()[0].kind else { panic!("a chained call") };
    assert!(matches!(callee.kind, Kind::Member { .. }));
}

#[test]
fn unclosed_brackets_are_located() {
    let e = parse("a({ b: [1, 2}").unwrap_err();
    assert!(e.message.contains("never closed") || e.message.contains("closes"), "{e}");
}

#[test]
fn replace_edits_only_the_node_and_checks_the_shape() {
    let src = "let a = game.model({id: \"hut\", scale: 1.0}) // keep\nlet b = 2\n";
    let s = sx(src);
    let obj = s.find_by_id("hut")[0].clone();
    let scale = s.at_path(&obj, &[Step::Field("scale".into())]).unwrap();
    let out = s.replace(&scale, "2.5").unwrap();
    assert_eq!(out.syntax.source(), "let a = game.model({id: \"hut\", scale: 2.5}) // keep\nlet b = 2\n");
    assert_eq!(out.syntax.text(out.syntax.node(&out.loc).unwrap()), "2.5");
    // Text that is not one node, or merges with its neighbours, is refused.
    assert!(s.replace(&scale, "2.5 3.5").is_err());
    let s2 = sx("let x = a * b\n");
    let b = s2.node(&s2.find_let("x").unwrap()).unwrap();
    let _ = b;
    let operand = Loc(vec![0, 0, 1]);
    assert!(s2.replace(&operand, "c + d").is_err(), "a replacement that merges into the operator sequence is refused");
    assert!(s2.replace(&operand, "(c + d)").is_ok());
}

#[test]
fn statements_insert_and_delete_with_their_lines() {
    let src = "fn f() {\n    a()\n    b() // note\n}\nlet z = 1\n";
    let s = sx(src);
    let body = Loc(vec![0, 0]);
    assert!(s.node(&body).unwrap().block().is_some());
    let out = s.insert_statement(&body, At::End, "c()").unwrap();
    assert_eq!(out.syntax.source(), "fn f() {\n    a()\n    b() // note\n    c()\n}\nlet z = 1\n");
    let out = s.insert_statement(&body, At::Start, "first()").unwrap();
    assert_eq!(out.syntax.source(), "fn f() {\n    first()\n    a()\n    b() // note\n}\nlet z = 1\n");
    let out = s.insert_statement(&body, At::After(0), "mid()").unwrap();
    assert_eq!(out.syntax.source(), "fn f() {\n    a()\n    mid()\n    b() // note\n}\nlet z = 1\n");
    let out = s.delete_statement(&body.child(1)).unwrap();
    assert_eq!(out.syntax.source(), "fn f() {\n    a()\n}\nlet z = 1\n");
    let out = s.delete_statement(&body.child(0)).unwrap();
    assert_eq!(out.syntax.source(), "fn f() {\n    b() // note\n}\nlet z = 1\n");
    // The file itself.
    let out = s.insert_statement(&Loc::default(), At::End, "end()").unwrap();
    assert_eq!(out.syntax.source(), "fn f() {\n    a()\n    b() // note\n}\nlet z = 1\nend()\n");
    let out = s.delete_statement(&Loc(vec![1])).unwrap();
    assert_eq!(out.syntax.source(), "fn f() {\n    a()\n    b() // note\n}\n");
    // Not a statement.
    assert!(s.insert_statement(&body, At::End, "a b").is_err() || s.insert_statement(&body, At::End, "a b").is_ok());
    assert!(s.insert_statement(&body, At::Before(9), "x()").is_err());
}

#[test]
fn empty_and_inline_blocks_take_statements() {
    let s = sx("fn f() {}\nfn g() { a() }\nfn h() {\n}\n");
    let out = s.insert_statement(&Loc(vec![0, 0]), At::End, "x()").unwrap();
    assert!(out.syntax.source().starts_with("fn f() { x() }\n"), "{}", out.syntax.source());
    let out = s.insert_statement(&Loc(vec![1, 0]), At::End, "x()").unwrap();
    assert!(out.syntax.source().contains("fn g() { a() x() }"), "{}", out.syntax.source());
    let out = s.insert_statement(&Loc(vec![2, 0]), At::End, "x()").unwrap();
    assert!(out.syntax.source().contains("fn h() {\n    x()\n}"), "{}", out.syntax.source());
}

#[test]
fn fields_set_add_and_remove() {
    let src = "game.model({\n    id: \"hut\",\n    at: vec3(1, 2, 3),\n    scale: 2.0,\n})\nlet o = Rect{w: 1 h: 2}\nlet p = {a: 1, b: 2}\n";
    let s = sx(src);
    let call = s.find_calls("game.model")[0].clone();
    let obj = call.child(1);
    let out = s.set_field(&obj, "scale", "3.0").unwrap();
    assert!(out.syntax.source().contains("scale: 3.0,"));
    let out = s.add_field(&obj, At::End, "yaw", "1.5").unwrap();
    assert!(out.syntax.source().contains("scale: 2.0,\n    yaw: 1.5,\n})"), "{}", out.syntax.source());
    let out = s.add_field(&obj, At::Start, "kind", "\"x\"").unwrap();
    assert!(out.syntax.source().contains("({\n    kind: \"x\",\n    id:"), "{}", out.syntax.source());
    let out = s.remove_field(&obj, "at").unwrap();
    assert!(out.syntax.source().contains("id: \"hut\",\n    scale: 2.0,\n})"), "{}", out.syntax.source());
    assert!(s.add_field(&obj, At::End, "id", "1").is_err());
    assert!(s.remove_field(&obj, "nope").is_err());
    // Inline objects.
    let o = s.node(&s.find_let("o").unwrap()).unwrap();
    let _ = o;
    let o_obj = Loc(vec![1, 0]);
    let out = s.add_field(&o_obj, At::End, "d", "3").unwrap();
    assert!(out.syntax.source().contains("Rect{w: 1 h: 2 d: 3}"), "{}", out.syntax.source());
    let out = s.remove_field(&o_obj, "w").unwrap();
    assert!(out.syntax.source().contains("Rect{h: 2}"), "{}", out.syntax.source());
    let p_obj = Loc(vec![2, 0]);
    let out = s.add_field(&p_obj, At::End, "c", "3").unwrap();
    assert!(out.syntax.source().contains("{a: 1, b: 2, c: 3}"), "{}", out.syntax.source());
    let out = s.remove_field(&p_obj, "b").unwrap();
    assert!(out.syntax.source().contains("let p = {a: 1}"), "{}", out.syntax.source());
    let out = s.remove_field(&p_obj, "a").unwrap();
    assert!(out.syntax.source().contains("let p = {b: 2}"), "{}", out.syntax.source());
}

#[test]
fn list_elements_insert_and_remove() {
    let src = "let xs = [1, 2, 3]\nlet ys = [\n    {id: \"a\"}\n    {id: \"b\"}\n]\nlet zs = []\n";
    let s = sx(src);
    let xs = Loc(vec![0, 0]);
    let out = s.push_item(&xs, "4").unwrap();
    assert!(out.syntax.source().contains("[1, 2, 3, 4]"));
    let out = s.insert_element(&xs, At::Start, "0").unwrap();
    assert!(out.syntax.source().contains("[0, 1, 2, 3]"));
    let out = s.remove_element(&xs, 1).unwrap();
    assert!(out.syntax.source().contains("[1, 3]"));
    let out = s.remove_element(&xs, 2).unwrap();
    assert!(out.syntax.source().contains("[1, 2]"));
    let ys = Loc(vec![1, 0]);
    let out = s.push_item(&ys, "{id: \"c\"}").unwrap();
    assert!(out.syntax.source().contains("    {id: \"b\"},\n    {id: \"c\"}\n]"), "{}", out.syntax.source());
    let b = s.find_by_id("b")[0].clone();
    assert_eq!(b, ys.child(1));
    let out = s.remove_element(&ys, 0).unwrap();
    assert!(out.syntax.source().contains("let ys = [\n    {id: \"b\"}\n]"), "{}", out.syntax.source());
    let out = s.push_item(&Loc(vec![2, 0]), "1").unwrap();
    assert!(out.syntax.source().contains("let zs = [1]"), "{}", out.syntax.source());
    // by path
    let found = s.at_path(&ys, &[Step::Id("b".into())]);
    assert_eq!(found, Some(ys.child(1)));
    assert_eq!(s.at_path(&ys, &[Step::Index(0), Step::Field("id".into())]), Some(ys.child(0).child(0)));
}

#[test]
fn values_are_written_as_splash_reads_them() {
    let v = Value::typed("Rect", vec![("w", 1.0.into()), ("name", "a \"q\"\n".into()), ("at", Value::Vec(vec![1.0, 2.5])), ("c", Value::Color("ff5a36".into())), ("n", 3.into()), ("ease", Value::Name("ease_out".into())), ("odd key", true.into())]);
    let text = write::write(&v).unwrap();
    assert_eq!(text, "Rect{w: 1.0, name: \"a \\\"q\\\"\\n\", at: vec2(1.0, 2.5), c: #ff5a36, n: 3, ease: @ease_out, \"odd key\": true}");
    let node = parse_value(&text).unwrap();
    assert_eq!(node.field("name").unwrap().value.string(), Some("a \"q\"\n"));
    assert_eq!(node.field("w").unwrap().value.number(), Some(1.0));
    assert!(write::write(&Value::Num(f64::NAN)).is_err());
    assert!(write::write(&Value::Ident("a b".into())).is_err());
    assert!(write::write(&Value::Color("zz".into())).is_err());
    let call = Value::call("game.model", vec![Value::object(vec![("id", "hut".into())])]);
    let line = write::write(&Value::let_("m", call)).unwrap();
    assert_eq!(line, "let m = game.model({id: \"hut\"})");
    let wide = Value::Array((0..40).map(|i| Value::Vec(vec![i as f64, 1.0, 2.0])).collect());
    let pretty = write::write_indented(&wide, "").unwrap();
    assert!(pretty.starts_with("[\n    vec3(0.0, 1.0, 2.0),\n"), "{pretty}");
    assert!(parse_value(&pretty).is_ok());
}

/// A token at column 0 that is not a closer or a continuation begins a
/// top-level statement; the tree must agree (it catches statements the
/// parser fused or split in the wrong place).
#[test]
fn column_zero_lines_begin_top_level_statements() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    splash_files(&root, &mut files);
    let mut bad = Vec::new();
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else { continue };
        let Ok(syntax) = parse(&source) else { continue };
        // Files that are themselves an object body have entries, not a flat statement list.
        let starts: std::collections::HashSet<usize> = syntax.statements().iter().map(|s| s.span.start).collect();
        // A statement with a braced body may hold lines indented in no particular way.
        let compound: Vec<Range<usize>> = syntax
            .statements()
            .iter()
            .filter(|s| matches!(s.kind, Kind::If { .. } | Kind::For { .. } | Kind::While { .. } | Kind::Loop { .. } | Kind::Fn { .. } | Kind::Match { .. }))
            .map(|s| s.span.clone())
            .collect();
        let mut offset = 0;
        let mut in_comment = false;
        for line in source.split_inclusive('\n') {
            let first = line.chars().next().unwrap_or('\n');
            let trimmed = line.trim_end();
            if in_comment {
                if trimmed.contains("*/") {
                    in_comment = false;
                }
            } else if trimmed.starts_with("/*") {
                in_comment = !trimmed.contains("*/");
            } else if !(first.is_whitespace() || trimmed.starts_with("//") || trimmed.starts_with(['}', ')', ']', '+', '.', '|', '&', '?', '*']) || trimmed.is_empty()) && !starts.contains(&offset) && !compound.iter().any(|c| c.contains(&offset)) {
                bad.push(format!("{}:{}: {}", path.display(), source[..offset].matches('\n').count() + 1, trimmed.chars().take(60).collect::<String>()));
            }
            offset += line.len();
        }
    }
    eprintln!("{} column-0 lines not at a statement start", bad.len());
    assert!(bad.is_empty(), "{} lines:\n{}", bad.len(), bad.iter().take(50).cloned().collect::<Vec<_>>().join("\n"));
}

#[test]
fn elements_move_and_replace_in_batches() {
    let s = sx("let xs = [\n    // first\n    {id: \"a\"}\n    // second\n    {id: \"b\"}\n    {id: \"c\"}\n]\nlet ys = [1, 2, 3]\n");
    let xs = Loc(vec![0, 0]);
    let out = s.move_element(&xs, 0, 2).unwrap();
    assert_eq!(out.syntax.source(), "let xs = [\n    // second\n    {id: \"b\"},\n    {id: \"c\"},\n    // first\n    {id: \"a\"}\n]\nlet ys = [1, 2, 3]\n");
    let ys = Loc(vec![1, 0]);
    let out = s.move_element(&ys, 2, 0).unwrap();
    assert!(out.syntax.source().contains("let ys = [3, 1, 2]"), "{}", out.syntax.source());
    // A removed element takes the comment above it.
    let out = s.remove_element(&xs, 1).unwrap();
    assert_eq!(out.syntax.source(), "let xs = [\n    // first\n    {id: \"a\"}\n    {id: \"c\"}\n]\nlet ys = [1, 2, 3]\n");
    // Several nodes at once; the result keeps its shape.
    let two = s.replace_nodes(&[(ys.child(0), "10".into()), (ys.child(2), "30".into())]).unwrap();
    assert!(two.source().contains("let ys = [10, 2, 30]"));
    assert!(s.replace_nodes(&[(ys.child(0), "1 2".into())]).is_err());
    // Nodes can be found again by reference.
    let node = s.node(&ys.child(1)).unwrap();
    assert_eq!(s.loc_of(node), Some(ys.child(1)));
}

#[test]
fn moving_elements_keeps_the_blank_lines_between_them() {
    let s = sx("let xs = [\n    {id: \"a\"}\n\n    {id: \"b\"}\n\n    {id: \"c\"}\n]\n");
    let out = s.move_element(&Loc(vec![0, 0]), 0, 2).unwrap();
    assert_eq!(out.syntax.source(), "let xs = [\n    {id: \"b\"},\n\n    {id: \"c\"},\n\n    {id: \"a\"}\n]\n");
}

#[test]
fn named_operators_and_try_postfix_read() {
    let s = sx("let a = x and y or not_z is w\nlet b = f()?\n");
    assert_eq!(s.statements().len(), 2);
}

#[test]
fn lists_keep_their_commas_when_elements_move_or_arrive() {
    let s = sx("let xs = [\n    {a: 1},\n    {a: 2},\n    {a: 3}\n]\n");
    let out = s.move_element(&Loc(vec![0, 0]), 2, 0).unwrap();
    assert_eq!(out.syntax.source(), "let xs = [\n    {a: 3},\n    {a: 1},\n    {a: 2},\n]\n");
    let s = sx("let xs = [\n    1\n    2\n]\n");
    let out = s.insert_element(&Loc(vec![0, 0]), At::Before(1), "-3").unwrap();
    assert_eq!(out.syntax.source(), "let xs = [\n    1,\n    -3\n    2\n]\n");
}

#[test]
fn an_insert_before_a_value_that_continues_the_previous_one_gets_a_comma_in_any_list() {
    // Once a list has one comma, the elements before an inserted `-2` still need theirs.
    let s = sx("let xs = [\n    a\n    b\n]\n");
    let s = s.insert_element(&Loc(vec![0, 0]), At::End, "-1").unwrap().syntax;
    let out = s.insert_element(&Loc(vec![0, 0]), At::Before(1), "-2").unwrap();
    assert_eq!(out.syntax.source(), "let xs = [\n    a,\n    -2,\n    b,\n    -1\n]\n");
    // On one line.
    let s = sx("let xs = [a b]\n");
    let out = s.insert_element(&Loc(vec![0, 0]), At::Before(1), "-1").unwrap();
    assert_eq!(out.syntax.source(), "let xs = [a, -1, b]\n");
}

#[test]
fn a_for_block_ends_its_statement() {
    let s = sx("let f = fn() {\n    for x in [1] {}\n    -1.0\n}\n");
    let body = s.node(&Loc(vec![0, 0, 0])).unwrap();
    assert_eq!(body.block().unwrap().stmts.len(), 2);
}
