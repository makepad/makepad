//! Conformance battery for the TOML 1.0 subset parser: real-manifest corpus,
//! positive lexemes, negative cases with spans, and a quoted-string span check.
use makepad_toml_parser::{parse_toml, Toml, TomlDocument, TomlErr};
use std::fs;
use std::path::{Path, PathBuf};

fn get_str<'a>(doc: &'a TomlDocument, path: &[&str]) -> Option<&'a str> {
    doc.get_path(path).and_then(Toml::as_str)
}

fn get_num(doc: &TomlDocument, path: &[&str]) -> Option<f64> {
    doc.get_path(path).and_then(Toml::as_num)
}

fn skip_dir_name(name: &str) -> bool {
    name == "old" || name == "local" || name == ".git" || name.starts_with("target")
}

fn walk_cargo_tomls(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if skip_dir_name(&name) {
                continue;
            }
            walk_cargo_tomls(&path, out);
        } else if file_type.is_file() && name == "Cargo.toml" {
            out.push(path);
        }
    }
}

fn span_inside(src: &str, err: &TomlErr) -> bool {
    err.span.start <= src.len() && err.span.end() <= src.len()
}

/// The error span overlaps `token` (the `nth` occurrence, 0-based).
fn span_points_at(src: &str, err: &TomlErr, token: &str, nth: usize) -> bool {
    if !span_inside(src, err) {
        return false;
    }
    let Some((i, _)) = src.match_indices(token).nth(nth) else {
        return false;
    };
    let j = i + token.len();
    err.span.start < j && err.span.end() > i
}

fn parse_ok(src: &str) -> TomlDocument {
    parse_toml(src).unwrap_or_else(|err| panic!("should parse {src:?}: {err:?}"))
}

#[test]
fn corpus_parses_every_cargo_toml() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root");
    let mut files = Vec::new();
    walk_cargo_tomls(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "expected at least one Cargo.toml under {}",
        root.display()
    );

    let mut parsed = 0usize;
    let mut failed = 0usize;
    let mut listings = Vec::new();
    let mut bad_spans = Vec::new();

    for path in &files {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        match parse_toml(&text) {
            Ok(_) => parsed += 1,
            Err(err) => {
                failed += 1;
                let rel = path.strip_prefix(&root).unwrap_or(path);
                listings.push(format!("{}: {}", rel.display(), err));
                if !span_inside(&text, &err) {
                    bad_spans.push(format!(
                        "{}: span start={} len={} file_len={}",
                        rel.display(),
                        err.span.start,
                        err.span.len,
                        text.len()
                    ));
                }
            }
        }
    }

    println!("corpus: parsed={parsed} failed={failed}");
    for line in &listings {
        println!("  FAIL {line}");
    }
    assert!(
        bad_spans.is_empty(),
        "parse errors whose span is outside the file:\n{}",
        bad_spans.join("\n")
    );
}

#[test]
fn positive_cases() {
    // Dotted keys.
    let doc = parse_ok("a.b.c = 1\n");
    assert_eq!(get_num(&doc, &["a", "b", "c"]), Some(1.0));

    // `[a.b]` after `[a]`.
    let doc = parse_ok("[a]\nx = 1\n[a.b]\ny = 2\n");
    assert_eq!(get_num(&doc, &["a", "x"]), Some(1.0));
    assert_eq!(get_num(&doc, &["a", "b", "y"]), Some(2.0));

    // Four `[[bin]]` tables; names are unique.
    let doc = parse_ok(
        "[[bin]]\nname = \"studio\"\n\
         [[bin]]\nname = \"studio-git-guard\"\n\
         [[bin]]\nname = \"studio-rustc-guard\"\n\
         [[bin]]\nname = \"studio-flow\"\n",
    );
    let bins = doc
        .get_path(&["bin"])
        .and_then(Toml::as_array_of_tables)
        .expect("[[bin]]");
    assert_eq!(bins.len(), 4);
    let names: Vec<&str> = bins.iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "studio",
            "studio-git-guard",
            "studio-rustc-guard",
            "studio-flow"
        ]
    );
    let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "[[bin]].name values must be unique");
    assert_eq!(get_str(&doc, &["bin", "name"]), Some("studio-flow"));

    // Inline tables in an array.
    let doc = parse_ok("x = [{a=1},{a=2}]\n");
    let arr = doc.get_path(&["x"]).and_then(Toml::as_array).unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0].as_table().unwrap()["a"].as_num(), Some(1.0));
    assert_eq!(arr[1].as_table().unwrap()["a"].as_num(), Some(2.0));

    // Multi-line basic string with a line-ending backslash.
    let doc = parse_ok("s = \"\"\"\nhello \\\n    world\n\"\"\"\n");
    assert_eq!(get_str(&doc, &["s"]), Some("hello world\n"));

    // Literal multi-line string (escapes are not processed).
    let doc = parse_ok("s = '''\nraw \\n not an escape\n'''\n");
    assert_eq!(get_str(&doc, &["s"]), Some("raw \\n not an escape\n"));

    // Unicode escapes: é and 😀.
    let doc = parse_ok("a = \"\\u00e9\"\nb = \"\\U0001F600\"\n");
    assert_eq!(get_str(&doc, &["a"]), Some("é"));
    assert_eq!(get_str(&doc, &["b"]), Some("\u{1F600}"));

    // Integers: hex, octal, binary, underscore, signed decimal.
    let doc = parse_ok("h = 0xFF\no = 0o77\nb = 0b101\nu = 1_000\np = +7\nm = -7\n");
    assert_eq!(get_num(&doc, &["h"]), Some(255.0));
    assert_eq!(get_num(&doc, &["o"]), Some(63.0));
    assert_eq!(get_num(&doc, &["b"]), Some(5.0));
    assert_eq!(get_num(&doc, &["u"]), Some(1000.0));
    assert_eq!(get_num(&doc, &["p"]), Some(7.0));
    assert_eq!(get_num(&doc, &["m"]), Some(-7.0));

    // Floats, inf, -inf, nan.
    let doc = parse_ok("a = 1e10\nb = 6.626e-34\nc = inf\nd = -inf\ne = nan\n");
    assert_eq!(get_num(&doc, &["a"]), Some(1e10));
    let tiny = get_num(&doc, &["b"]).unwrap();
    assert!(
        (tiny - 6.626e-34).abs() / 6.626e-34 < 1e-12,
        "6.626e-34 got {tiny}"
    );
    assert_eq!(get_num(&doc, &["c"]), Some(f64::INFINITY));
    assert_eq!(get_num(&doc, &["d"]), Some(f64::NEG_INFINITY));
    assert!(get_num(&doc, &["e"]).unwrap().is_nan());

    // Booleans.
    let doc = parse_ok("t = true\nf = false\n");
    assert_eq!(doc.get_path(&["t"]).and_then(Toml::as_bool), Some(true));
    assert_eq!(doc.get_path(&["f"]).and_then(Toml::as_bool), Some(false));

    // Offset / local datetimes and local date / time, kept as source text.
    let doc = parse_ok(
        "odt = 1979-05-27T07:32:00Z\n\
         odt2 = 1979-05-27 07:32:00-07:00\n\
         ldt = 1979-05-27T07:32:00\n\
         ld = 1979-05-27\n\
         lt = 07:32:00\n",
    );
    let date = |k| match doc.get_path(&[k]) {
        Some(Toml::Date(v, _)) => v.clone(),
        other => panic!("{k}: {other:?}"),
    };
    assert_eq!(date("odt"), "1979-05-27T07:32:00Z");
    assert_eq!(date("odt2"), "1979-05-27 07:32:00-07:00");
    assert_eq!(date("ldt"), "1979-05-27T07:32:00");
    assert_eq!(date("ld"), "1979-05-27");
    assert_eq!(date("lt"), "07:32:00");

    // Comments after values.
    let doc = parse_ok("a = 1 # after value\nb = true # yes\n");
    assert_eq!(get_num(&doc, &["a"]), Some(1.0));
    assert_eq!(doc.get_path(&["b"]).and_then(Toml::as_bool), Some(true));

    // CRLF files.
    let doc = parse_ok("[a]\r\nb = 1 # trailing\r\nc = \"x\"\r\n");
    assert_eq!(get_num(&doc, &["a", "b"]), Some(1.0));
    assert_eq!(get_str(&doc, &["a", "c"]), Some("x"));

    // Quoted URL key under `[patch]`.
    let doc = parse_ok("[patch]\n\"https://a.b/c\" = 1\n");
    assert_eq!(get_num(&doc, &["patch", "https://a.b/c"]), Some(1.0));

    // Empty tables with nothing inside.
    let doc = parse_ok("[x]\n[y]\n");
    assert!(doc
        .get_path(&["x"])
        .and_then(Toml::as_table)
        .unwrap()
        .is_empty());
    assert!(doc
        .get_path(&["y"])
        .and_then(Toml::as_table)
        .unwrap()
        .is_empty());
}

#[test]
fn negative_cases_error_and_span() {
    struct Case {
        src: &'static str,
        token: &'static str,
        nth: usize,
        why: &'static str,
    }
    let cases = [
        Case {
            src: "a = 1\na = 2\n",
            token: "a = 2",
            nth: 0,
            why: "duplicate key",
        },
        Case {
            src: "[a]\n[a]\n",
            token: "[a]",
            nth: 1,
            why: "table redefined [a] twice",
        },
        Case {
            src: "a = 1\n[a]\n",
            token: "[a]",
            nth: 0,
            why: "[a] after a = 1",
        },
        Case {
            src: "a.b = 1\n[a.b]\n",
            token: "[a.b]",
            nth: 0,
            why: "[a.b] after a.b = 1",
        },
        Case {
            src: "[a.b]\nx = 1\n[a]\nb.y = 2\n",
            token: "b.y",
            nth: 0,
            why: "dotted extension of a header-defined table",
        },
        Case {
            src: "t = {a=1,}\n",
            token: "}",
            nth: 0,
            why: "inline table trailing comma",
        },
        Case {
            src: "t = {a=1,\nb=2}\n",
            token: "\nb=2",
            nth: 0,
            why: "newline inside inline table",
        },
        Case {
            src: "n = 01\n",
            token: "01",
            nth: 0,
            why: "leading zero",
        },
        Case {
            src: "n = +0xFF\n",
            token: "+0xFF",
            nth: 0,
            why: "signed hex",
        },
        Case {
            src: "d = 1234-\n",
            token: "1234-",
            nth: 0,
            why: "truncated date",
        },
        Case {
            src: "s = \"hello",
            token: "hello",
            nth: 0,
            why: "unterminated string",
        },
        Case {
            src: "s = \"a\u{1}b\"\n",
            token: "\u{1}",
            nth: 0,
            why: "control char in basic string",
        },
        Case {
            src: "s = \"\"\"a\\ b\"\"\"\n",
            token: "\\ b",
            nth: 0,
            why: r#" """a\ b""" without newline after the backslash "#,
        },
        Case {
            src: "[[a]]\n[a]\n",
            token: "[a]",
            nth: 1,
            why: "[[a]] then [a]",
        },
        Case {
            src: "foo bar = 1\n",
            token: "bar",
            nth: 0,
            why: "bare key with space",
        },
    ];

    let mut failures = Vec::new();
    for case in cases {
        match parse_toml(case.src) {
            Ok(doc) => failures.push(format!(
                "{}: expected error, parsed {doc:?} from {:?}",
                case.why, case.src
            )),
            Err(err) => {
                if !span_points_at(case.src, &err, case.token, case.nth) {
                    let shown = case
                        .src
                        .get(err.span.start..err.span.end().min(case.src.len()))
                        .unwrap_or("");
                    failures.push(format!(
                        "{}: span start={} len={} ({shown:?}) does not point at {:?} (nth {}); msg={}; src={:?}",
                        case.why,
                        err.span.start,
                        err.span.len,
                        case.token,
                        case.nth,
                        err.msg,
                        case.src
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "negative cases:\n{}",
        failures.join("\n")
    );
}

#[test]
fn span_of_name_studio_on_line_3() {
    let src = "id = 1\nversion = \"0\"\nname = \"studio\"\n";
    assert_eq!(src.lines().nth(2), Some("name = \"studio\""));
    let doc = parse_ok(src);
    let span = doc
        .get_path(&["name"])
        .and_then(Toml::span)
        .expect("Str span")
        .clone();
    let start = src.find("\"studio\"").expect("quoted studio");
    let end = start + "\"studio\"".len();
    assert_eq!(span.start, start, "span.start should be the opening quote");
    assert_eq!(span.end(), end, "span.end should be past the closing quote");
    assert_eq!(&src[span.start..span.end()], "\"studio\"");
}
