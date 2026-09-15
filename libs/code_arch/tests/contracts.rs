use makepad_code_arch::*;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

fn example() -> Plan {
    parse(EXAMPLE, &Limits::default()).unwrap()
}
fn parsed(plan: &Plan) -> Result<Plan, ArchError> {
    parse(&format(plan), &Limits::default())
}
fn rejects(text: &str, kind: ErrorKind) -> ArchError {
    let error = parse(text, &Limits::default()).unwrap_err();
    assert_eq!(error.kind, kind, "{error}");
    assert!(!error.to_string().is_empty());
    error
}

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "makepad-code-arch-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self { root }
    }
    fn write(&self, path: impl AsRef<Path>, text: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn example() -> Self {
        let fixture = Self::new();
        for file in example().source.files {
            fixture.write(file.path, "");
        }
        fixture.write("fictional/arch/worker.toml", "# fictional child plan\n");
        fixture
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn arch_cli(fixture: &Fixture, args: &[&str], code: i32) -> std::process::Output {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_arch"))
        .current_dir(&fixture.root)
        .args(args)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(code), "{args:?}: {output:?}");
    output
}

#[test]
fn cli_usage_errors_and_help_exit_codes() {
    let fixture = Fixture::new();
    for args in [
        vec![],
        vec!["unknown"],
        vec!["validate"],
        vec!["manifest", "fictional", "--root"],
        vec!["manifest", "fictional", "--root", "--write"],
        vec!["manifest", "fictional", "--root", ".", "--root", "."],
        vec!["validate", "plan.toml", "extra.toml"],
        vec!["validate", "plan.toml", "--write"],
        vec!["changes", "plan.toml", "--wat"],
        vec!["format", "plan.toml", "--root", "."],
        vec!["format", "plan.toml", "--write", "--write"],
    ] {
        let output = arch_cli(&fixture, &args, 2);
        assert!(String::from_utf8(output.stderr).unwrap().contains("Usage:"));
    }
    let output = arch_cli(&fixture, &["--help"], 0);
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("Exit codes:"));
}

#[test]
fn cli_manifest_is_toml_and_reports_failures() {
    let fixture = Fixture::example();
    let output = arch_cli(&fixture, &["manifest", "fictional/", "--root", "."], 0);
    let manifest = String::from_utf8(output.stdout).unwrap();
    let (header, rest) = EXAMPLE.split_once("files = [").unwrap();
    let (_, rest) = rest.split_once("overview = ").unwrap();
    let plan = parse(
        &format!("{header}{manifest}overview = {rest}"),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(plan.source.files, example().source.files);
    let output = arch_cli(&fixture, &["manifest", "missing/"], 1);
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr).unwrap().contains("error:"));
    for i in 0..513 {
        fixture.write(format!("wide/{i}.rs"), "");
    }
    let output = arch_cli(&fixture, &["manifest", "wide/"], 1);
    assert!(
        output.stdout.is_empty(),
        "must not print a partial manifest"
    );
    assert!(String::from_utf8(output.stderr).unwrap().contains("512"));
}

#[cfg(unix)]
#[test]
fn cli_manifest_escapes_paths_without_changing_them() {
    let fixture = Fixture::new();
    fixture.write("fictional/quote\"line\n.rs", "");
    let output = arch_cli(&fixture, &["manifest", "fictional"], 0);
    let files = String::from_utf8(output.stdout).unwrap();
    let text = format!("[arch]\nversion=0\nscope='fictional/'\ntitle='X'\nprompt='arch/0'\ngenerator='test'\n[source]\n{files}overview='Unknown.'\n[nodes]\n");
    assert_eq!(
        parse(&text, &Limits::default()).unwrap().source.files,
        try_manifest("fictional", &fixture.root).unwrap()
    );
}

#[test]
fn cli_validation_distinguishes_warnings_errors_and_root() {
    let fixture = Fixture::example();
    let caller = Fixture::new();
    caller.write("plan.toml", EXAMPLE);
    let root = fixture.root.to_str().unwrap();
    let output = arch_cli(&caller, &["validate", "plan.toml", "--root", root], 0);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "validation: 0 errors, 0 warnings\n"
    );
    assert!(output.stderr.is_empty());
    let mut plan = example();
    plan.arch.prompt = "arch/future".into();
    caller.write("plan.toml", &format(&plan));
    let output = arch_cli(&caller, &["validate", "--root", root, "plan.toml"], 0);
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("warning: arch.prompt"));
    arch_cli(&caller, &["validate", "plan.toml"], 1);
    fs::remove_file(fixture.root.join("fictional/src/cache.rs")).unwrap();
    let output = arch_cli(&caller, &["validate", "plan.toml", "--root", root], 1);
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("error: nodes.cache.refs"));
}

#[test]
fn cli_changes_exit_success_on_deltas_but_fail_on_io_errors() {
    let fixture = Fixture::example();
    fixture.write("plan.toml", EXAMPLE);
    let output = arch_cli(&fixture, &["changes", "plan.toml", "--root", "."], 0);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "added = []\nremoved = []\nmodified = []\naffected_nodes = []\nunchanged = true\n"
    );
    fixture.write("fictional/src/cache.rs", "// modified");
    fixture.write("fictional/src/added.rs", "// added");
    fs::remove_file(fixture.root.join("fictional/src/worker.rs")).unwrap();
    let output = arch_cli(&fixture, &["changes", "plan.toml"], 0);
    let text = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "added = [\"fictional/src/added.rs\"]",
        "removed = [\"fictional/src/worker.rs\"]",
        "modified = [\"fictional/src/cache.rs\"]",
        "unchanged = false",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    let affected = changes_since(&example(), &fixture.root).affected_nodes;
    assert!(text.contains(&format!("affected_nodes = {affected:?}")));
    let output = arch_cli(&fixture, &["changes", "plan.toml", "--root", "absent"], 1);
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("unchanged = false"));
}

#[test]
fn cli_format_and_input_failures_preserve_files() {
    let fixture = Fixture::new();
    let input = format!("# noncanonical comment\n{EXAMPLE}");
    fixture.write("plan.toml", &input);
    let output = arch_cli(&fixture, &["format", "plan.toml"], 0);
    assert_eq!(String::from_utf8(output.stdout).unwrap(), EXAMPLE);
    assert_eq!(
        fs::read_to_string(fixture.root.join("plan.toml")).unwrap(),
        input
    );
    arch_cli(&fixture, &["format", "--write", "plan.toml"], 0);
    assert_eq!(
        fs::read_to_string(fixture.root.join("plan.toml")).unwrap(),
        EXAMPLE
    );
    arch_cli(&fixture, &["format", "plan.toml", "--write"], 0);
    fixture.write("-plan.toml", EXAMPLE);
    arch_cli(&fixture, &["format", "--", "-plan.toml"], 0);
    for input in [
        "[broken".to_owned(),
        " ".repeat(Limits::default().max_input_bytes + 1),
    ] {
        fixture.write("bad.toml", &input);
        for command in ["validate", "changes", "format"] {
            arch_cli(&fixture, &[command, "bad.toml"], 1);
            arch_cli(&fixture, &[command, "absent.toml"], 1);
        }
        arch_cli(&fixture, &["format", "bad.toml", "--write"], 1);
        assert_eq!(
            fs::read_to_string(fixture.root.join("bad.toml")).unwrap(),
            input
        );
    }
}

#[test]
fn example_round_trip_and_schema_are_valid() {
    let plan = example();
    assert_eq!(plan.nodes.len(), 12);
    assert_eq!(plan.edges.len(), 12);
    assert_eq!(plan.lanes.len(), 2);
    assert_eq!(plan.source.files.len(), 8);
    assert_eq!(format(&plan), EXAMPLE, "few-shot must already be canonical");
    assert_eq!(parsed(&plan).unwrap(), plan);
    parse(SCHEMA, &Limits::default()).unwrap();
    assert!(PROMPT.lines().count() <= 40);
}

#[test]
fn canonical_sorting_escaping_and_prose_preserve_values() {
    let mut plan = example();
    plan.source.files.reverse();
    let node = plan.nodes.get_mut("cache").unwrap();
    node.story = "First sentence. Second sentence!\tThird?  Fourth.\n\nA paragraph.\n\"Quoted\" and \\\\ and \u{0001}.\n".into();
    node.summary = "One. Two.".into();
    node.visual = Some("\n''' \"\"\"\n{ calls = 'not TOML' }\r\n\tαβ\\\n".into());
    let text = format(&plan);
    assert!(text.contains("First sentence. \\\nSecond sentence!\\t\\\nThird?  \\\nFourth."));
    let decoded = parse(&text, &Limits::default()).unwrap();
    assert_eq!(decoded.nodes, plan.nodes);
    assert_eq!(format(&decoded), text);
    assert!(decoded
        .source
        .files
        .windows(2)
        .all(|w| w[0].path < w[1].path));
    let before = format(&decoded);
    let mut refreshed = decoded.clone();
    refreshed.nodes.get_mut("worker").unwrap().story = "An affected fact changed.".into();
    let after = format(&refreshed);
    // Node and edge serialization is independent of another node's prose.
    assert_eq!(
        before.split("[nodes.worker]").next(),
        after.split("[nodes.worker]").next()
    );
    assert_eq!(
        before.split("[edges]").nth(1),
        after.split("[edges]").nth(1)
    );
}

#[test]
fn toml_spellings_and_optional_fields() {
    let text = "arch = {version=0, scope='x/', title='X', prompt='arch/0', generator='unknown'}\n\
                source = {files=[], overview='Unknown.'}\n\
                nodes = {a={kind='component', title='A', summary='Unknown', story='Unknown.', refs=[]}}\n";
    let plan = parse(text, &Limits::default()).unwrap();
    assert!(plan.lanes.is_empty() && plan.edges.is_empty());
    assert_eq!(parsed(&plan).unwrap(), plan);
    let alternate = "[arch]\nversion=0\nscope='x/'\ntitle='X'\nprompt='arch/0'\ngenerator='unknown'\n\
                     [source]\nfiles=[]\noverview='Unknown.'\n\
                     [nodes.'a']\nkind='component'\ntitle='A'\nsummary='Unknown'\nstory='Unknown.'\nrefs=[]\n";
    assert_eq!(parse(alternate, &Limits::default()).unwrap(), plan);
    for zero in ["+0", "-0", "0x0", "0o0", "0b0", "0x0_0"] {
        assert_eq!(
            parse(
                &alternate.replace("version=0", &format!("version={zero}")),
                &Limits::default()
            )
            .unwrap(),
            plan
        );
    }
}

#[test]
fn root_scope_and_relative_path_aliases_track_the_same_files() {
    let fixture = Fixture::example();
    let mut p = example();
    p.arch.scope = ".".into();
    p.source.files[0].path = "./fictional/src/cache.rs".into();
    p.nodes.get_mut("cache").unwrap().refs = vec!["./fictional/src/cache.rs:1::Cache".into()];
    assert!(validate(&p, &fixture.root).is_valid());
    assert!(changes_since(&p, &fixture.root).unchanged);
    fixture.write("fictional/src/cache.rs", "// changed");
    let changes = changes_since(&p, &fixture.root);
    assert_eq!(changes.modified, [PathBuf::from("fictional/src/cache.rs")]);
    assert_eq!(changes.affected_nodes, ["cache"]);
    p.source.files.push(FileHash {
        path: "fictional/src/cache.rs".into(),
        hash: p.source.files[0].hash.clone(),
    });
    assert_eq!(parsed(&p).unwrap_err().kind, ErrorKind::Duplicate);
}

#[test]
fn rejects_every_unknown_field_with_its_name() {
    for (needle, replacement, expected) in [
        ("[arch]", "intruder = 1\n[arch]", "intruder"),
        ("version = 0", "version = 0\nmodule = 'v1'", "arch.module"),
        ("[source]", "[source]\nrule = 'v1'", "source.rule"),
        ("{ path =", "{ extra = 1, path =", "source.files[0].extra"),
        (
            "background = { title =",
            "background = { owner = 'x', title =",
            "lanes.background.owner",
        ),
        (
            "[nodes.cache]",
            "[nodes.cache]\nsubtype = 'v1'",
            "nodes.cache.subtype",
        ),
        (
            "allocate = { from =",
            "allocate = { refs = [], from =",
            "edges.allocate.refs",
        ),
    ] {
        let error = rejects(
            &EXAMPLE.replacen(needle, replacement, 1),
            ErrorKind::UnknownField,
        );
        assert_eq!(error.field, expected);
        assert!(error.to_string().contains(expected));
    }
    rejects(
        &format!("overview = 'wrong table'\n{EXAMPLE}"),
        ErrorKind::UnknownField,
    );
}

#[test]
fn syntax_missing_type_version_kind_duplicate_and_dangling_errors() {
    rejects("[", ErrorKind::Syntax);
    rejects(
        &EXAMPLE.replace("version = 0\n", ""),
        ErrorKind::MissingField,
    );
    rejects(
        &EXAMPLE.replace("version = 0", "version = '0'"),
        ErrorKind::Type,
    );
    rejects(
        &EXAMPLE.replace("version = 0", "version = 1"),
        ErrorKind::Version,
    );
    for value in ["0.0", "0e0", "1e-999", "0.00000000000000000000001"] {
        rejects(
            &EXAMPLE.replace("version = 0", &format!("version = {value}")),
            ErrorKind::Type,
        );
    }
    for value in ["subsystem", "lifecycle", "Component", ""] {
        rejects(
            &EXAMPLE.replacen("kind = \"component\"", &format!("kind = \"{value}\""), 1),
            ErrorKind::InvalidKind,
        );
    }
    rejects(
        &EXAMPLE.replacen("kind = \"calls\"", "kind = \"receives\"", 1),
        ErrorKind::InvalidKind,
    );
    rejects(
        &format!("{EXAMPLE}\nallocate = {{ from = 'decoder', to = 'memory', kind = 'calls' }}"),
        ErrorKind::Duplicate,
    );
    rejects(&format!("{EXAMPLE}\n[nodes.cache]\n"), ErrorKind::Duplicate);
    rejects(
        &EXAMPLE.replacen("version = 0", "version = 0\nversion = 0", 1),
        ErrorKind::Duplicate,
    );
    let mut p = example();
    p.source.files.push(p.source.files[0].clone());
    assert_eq!(parsed(&p).unwrap_err().kind, ErrorKind::Duplicate);
    for field in ["from", "to", "lane"] {
        let mut p = example();
        match field {
            "from" => p.edges.get_mut("allocate").unwrap().from = "absent".into(),
            "to" => p.edges.get_mut("allocate").unwrap().to = "absent".into(),
            _ => p.nodes.get_mut("cache").unwrap().lane = Some("absent".into()),
        }
        let error = parsed(&p).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Dangling);
        assert!(error.field.ends_with(field));
    }
    rejects(
        &EXAMPLE.replacen(
            "refs = [\"fictional/src/cache.rs::Cache\"]",
            "refs = [2]",
            1,
        ),
        ErrorKind::Type,
    );
    rejects(
        &EXAMPLE.replacen("[nodes.cache]", "[nodes.cache]\nvisual = 2", 1),
        ErrorKind::Type,
    );
    rejects(
        &EXAMPLE.replacen("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391", "not-hex", 1),
        ErrorKind::InvalidHash,
    );
}

#[test]
fn ids_are_bounded_and_all_namespaces_are_checked() {
    for id in [
        "",
        "A",
        "2bad",
        "has space",
        "has.dot",
        "α",
        &"a".repeat(65),
    ] {
        for section in ["nodes", "edges", "lanes"] {
            let mut p = example();
            match section {
                "nodes" => {
                    p.nodes.insert(id.into(), p.nodes["cache"].clone());
                }
                "edges" => {
                    p.edges.insert(id.into(), p.edges["allocate"].clone());
                }
                _ => {
                    p.lanes.insert(id.into(), p.lanes["caller"].clone());
                }
            }
            assert_eq!(
                parsed(&p).unwrap_err().kind,
                ErrorKind::InvalidId,
                "{section}: {id}"
            );
        }
    }
    let mut p = example();
    p.nodes
        .insert(format!("a{}_-", "9".repeat(61)), p.nodes["cache"].clone());
    parsed(&p).unwrap();
}

#[test]
fn node_edge_lane_and_file_limits_accept_boundary_reject_overflow() {
    for section in ["nodes", "edges", "lanes", "files"] {
        let mut p = example();
        let max = match section {
            "nodes" => 24,
            "edges" => 48,
            "lanes" => 8,
            _ => 512,
        };
        match section {
            "nodes" => {
                let row = p.nodes["cache"].clone();
                p.nodes.clear();
                p.edges.clear();
                for i in 0..max {
                    p.nodes.insert(format!("n{i}"), row.clone());
                }
            }
            "edges" => {
                let row = p.edges["allocate"].clone();
                p.edges.clear();
                for i in 0..max {
                    p.edges.insert(format!("e{i}"), row.clone());
                }
            }
            "lanes" => {
                let row = p.lanes["caller"].clone();
                for i in p.lanes.len()..max {
                    p.lanes.insert(format!("l{i}"), row.clone());
                }
            }
            _ => {
                let hash = p.source.files[0].hash.clone();
                p.source.files.clear();
                for i in 0..max {
                    p.source.files.push(FileHash {
                        path: format!("fictional/{i}.rs").into(),
                        hash: hash.clone(),
                    });
                }
            }
        }
        parsed(&p).unwrap();
        match section {
            "nodes" => {
                p.nodes.insert("overflow".into(), p.nodes["n0"].clone());
            }
            "edges" => {
                p.edges.insert("overflow".into(), p.edges["e0"].clone());
            }
            "lanes" => {
                p.lanes.insert("overflow".into(), p.lanes["caller"].clone());
            }
            _ => p.source.files.push(FileHash {
                path: "overflow.rs".into(),
                hash: p.source.files[0].hash.clone(),
            }),
        }
        let error = parsed(&p).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{section}: {error}");
        assert_eq!(
            error.field,
            if section == "files" {
                "source.files"
            } else {
                section
            }
        );
        let mut relaxed = Limits::default();
        relaxed.max_nodes = usize::MAX;
        relaxed.max_edges = usize::MAX;
        relaxed.max_lanes = usize::MAX;
        relaxed.max_files = usize::MAX;
        assert_eq!(
            parse(&format(&p), &relaxed).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn prose_limits_count_unicode_scalars_at_exact_boundaries() {
    for (field, max) in [("overview", 1200), ("summary", 120), ("story", 600)] {
        let mut p = example();
        let set = |p: &mut Plan, value: String| match field {
            "overview" => p.overview = value,
            "summary" => p.nodes.get_mut("cache").unwrap().summary = value,
            _ => p.nodes.get_mut("cache").unwrap().story = value,
        };
        set(&mut p, "🦀".repeat(max));
        parsed(&p).unwrap();
        set(&mut p, "🦀".repeat(max + 1));
        let error = parsed(&p).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert!(error.field.ends_with(field));
    }
}

#[test]
fn every_limit_can_be_tightened() {
    let base = Limits::default();
    let cases = [
        Limits {
            max_input_bytes: EXAMPLE.len() - 1,
            ..base.clone()
        },
        Limits {
            max_depth: 3,
            ..base.clone()
        },
        Limits {
            max_values: 1,
            ..base.clone()
        },
        Limits {
            max_string_bytes: 4,
            ..base.clone()
        },
        Limits {
            max_nodes: 11,
            ..base.clone()
        },
        Limits {
            max_edges: 11,
            ..base.clone()
        },
        Limits {
            max_lanes: 1,
            ..base.clone()
        },
        Limits {
            max_files: 7,
            ..base.clone()
        },
        Limits {
            max_overview_chars: 1,
            ..base.clone()
        },
        Limits {
            max_summary_chars: 1,
            ..base.clone()
        },
        Limits {
            max_story_chars: 1,
            ..base.clone()
        },
    ];
    for limits in cases {
        assert_eq!(parse(EXAMPLE, &limits).unwrap_err().kind, ErrorKind::Limit);
    }
    parse(
        EXAMPLE,
        &Limits {
            max_input_bytes: EXAMPLE.len(),
            max_depth: 4,
            ..base
        },
    )
    .unwrap();
    // Find the exact structural value count without exposing parser internals.
    let count = (1..8192)
        .find(|n| {
            parse(
                EXAMPLE,
                &Limits {
                    max_values: *n,
                    ..Limits::default()
                },
            )
            .is_ok()
        })
        .unwrap();
    assert_eq!(
        parse(
            EXAMPLE,
            &Limits {
                max_values: count - 1,
                ..Limits::default()
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
}

#[test]
fn string_limits_are_checked_before_allocating_decoded_values() {
    let mut p = example();
    p.arch.title = "x".repeat(16 * 1024);
    parsed(&p).unwrap();
    p.arch.title.push('x');
    assert_eq!(parsed(&p).unwrap_err().kind, ErrorKind::Limit);
    p.arch.title = "🦀".repeat(4096);
    parsed(&p).unwrap();
    // Decoded bytes, not the length of \u/\U escape spelling, are bounded.
    let escaped = EXAMPLE.replacen(
        "title = \"Fictional thumbnail service\"",
        &format!("title = \"{}\"", "\\U0001F980".repeat(4096)),
        1,
    );
    let decoded = parse(&escaped, &Limits::default()).unwrap();
    assert_eq!(decoded.arch.title.len(), 16 * 1024);
    assert_eq!(parsed(&decoded).unwrap(), decoded);
    p.arch.title = "Short".into();
    p.nodes.get_mut("cache").unwrap().visual = Some("\n".repeat(16 * 1024));
    assert_eq!(parsed(&p).unwrap(), p);
}

#[test]
fn canonical_expansion_cannot_escape_parser_limits() {
    let compact = "arch={version=0,scope='x',title='X',prompt='arch/0',generator='x'}\nsource={files=[],overview='Unknown.'}\nnodes={}\n";
    let canonical = format(&parse(compact, &Limits::default()).unwrap());
    assert!(canonical.len() > compact.len());
    assert_eq!(
        parse(
            compact,
            &Limits {
                max_input_bytes: compact.len(),
                ..Limits::default()
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
    let limits = Limits {
        max_input_bytes: canonical.len(),
        ..Limits::default()
    };
    let plan = parse(compact, &limits).unwrap();
    assert_eq!(parse(&format(&plan), &limits).unwrap(), plan);
}

#[test]
fn malformed_deep_nesting_huge_strings_and_many_values_are_refused_fast() {
    let mut deep = String::from("x=");
    deep.push_str(&"[".repeat(100_000));
    let huge = format!("x=\"{}", "x".repeat(10 * 1024 * 1024));
    let dotted = format!("[{}x]\n", "x.".repeat(30_000));
    let nested = format!("[a.b.c.d.e.f.g.h]\nx={}", "[".repeat(20));
    let values = format!("x=[{}]\n", "0,".repeat(9000));
    for (name, text) in [
        ("deep", deep),
        ("10 MiB string", huge),
        ("dotted header", dotted),
        ("combined depth", nested),
        ("values", values),
    ] {
        let start = Instant::now();
        rejects(&text, ErrorKind::Limit);
        let elapsed = start.elapsed();
        eprintln!("bounded refusal {name}: {elapsed:?}");
        assert!(elapsed < Duration::from_secs(1), "{name}: {elapsed:?}");
    }
    let input = format!("#{}", "x".repeat(256 * 1024));
    rejects(&input, ErrorKind::Limit);
    for text in [
        "x=\"\\uD800\"",
        "x=\"\\UFFFFFFFF\"",
        "x=\"unterminated",
        "x={a=}",
        "x=[1 2]",
        "x=\"\"\"too many\"\"\"\"\"\"",
    ] {
        rejects(text, ErrorKind::Syntax);
    }
}

#[test]
fn validate_example_fixture_and_distinguish_warnings() {
    let fixture = Fixture::example();
    let mut p = example();
    let validation = validate(&p, &fixture.root);
    assert!(validation.is_valid(), "{validation:?}");
    assert!(validation.warnings.is_empty(), "{validation:?}");
    p.arch.prompt = "arch/older".into();
    p.overview.clear();
    p.nodes.get_mut("cache").unwrap().refs.clear();
    p.nodes.get_mut("cache").unwrap().story.clear();
    fixture.write("fictional/src/added.rs", "");
    let validation = validate(&p, &fixture.root);
    assert!(validation.is_valid());
    assert_eq!(validation.warnings.len(), 5);
    fixture.write("outside.rs", "");
    p.nodes.get_mut("cache").unwrap().refs = vec!["outside.rs".into()];
    assert!(validate(&p, &fixture.root)
        .warnings
        .iter()
        .any(|w| w.message.contains("absent from source.files")));
}

#[test]
fn validate_checks_all_path_sources_and_missing_paths() {
    let fixture = Fixture::example();
    for path in [
        "../escape.rs",
        "/tmp/absolute.rs",
        "C:/drive.rs",
        "\\server\\file.rs",
        "fictional/../escape.rs",
        "",
        "bad\0path",
    ] {
        for field in ["scope", "source", "refs", "child"] {
            if field == "child" && path.is_empty() {
                continue;
            }
            let mut p = example();
            match field {
                "scope" => p.arch.scope = path.into(),
                "source" => p.source.files[0].path = path.into(),
                "refs" => p.nodes.get_mut("cache").unwrap().refs = vec![path.into()],
                _ => p.nodes.get_mut("cache").unwrap().child = Some(path.into()),
            }
            assert!(!validate(&p, &fixture.root).is_valid(), "{field} {path}");
        }
    }
    for field in ["source", "refs", "child"] {
        let mut p = example();
        match field {
            "source" => p.source.files[0].path = "fictional/src/missing.rs".into(),
            "refs" => {
                p.nodes.get_mut("cache").unwrap().refs = vec!["fictional/src/missing.rs".into()]
            }
            _ => {
                p.nodes.get_mut("cache").unwrap().child = Some("fictional/arch/missing.toml".into())
            }
        }
        assert!(validate(&p, &fixture.root)
            .errors
            .iter()
            .any(|e| e.kind == ErrorKind::MissingPath));
    }
    let mut p = example();
    p.nodes.get_mut("cache").unwrap().refs = vec!["fictional/src".into()];
    assert!(validate(&p, &fixture.root)
        .errors
        .iter()
        .any(|e| e.kind == ErrorKind::InvalidPath));
    p.source.files[0].path = "fictional/arch/worker.toml".into();
    assert!(!validate(&p, &fixture.root).is_valid());
    fixture.write("file.rs", "");
    let validation = validate(&example(), fixture.root.join("file.rs/child"));
    assert!(
        validation.errors.iter().any(|e| e.kind == ErrorKind::Io),
        "{validation:?}"
    );
    assert_eq!(
        validate(&example(), fixture.root.join("file.rs")).errors[0].kind,
        ErrorKind::InvalidPath
    );
}

#[test]
fn reference_suffixes_validate_without_resolving_symbols_or_line_ranges() {
    let fixture = Fixture::example();
    let mut p = example();
    for suffix in [
        "",
        ":1",
        "::Cache",
        ":4294967295::Cache::get",
        "::模块::item",
    ] {
        p.nodes.get_mut("cache").unwrap().refs = vec![format!("fictional/src/cache.rs{suffix}")];
        assert!(validate(&p, &fixture.root).is_valid(), "{suffix}");
    }
    for suffix in [
        ":0",
        ":-1",
        ":no",
        ":4294967296",
        ":1:2",
        "::",
        "::Cache::",
        "::Bad symbol",
        "::a/b",
        "::a:b",
    ] {
        p.nodes.get_mut("cache").unwrap().refs = vec![format!("fictional/src/cache.rs{suffix}")];
        assert!(
            validate(&p, &fixture.root)
                .errors
                .iter()
                .any(|e| matches!(e.kind, ErrorKind::InvalidRef | ErrorKind::InvalidPath)),
            "{suffix}"
        );
    }
}

#[test]
fn changes_hash_added_removed_modified_and_map_affected_nodes() {
    let fixture = Fixture::example();
    let p = example();
    let baseline = changes_since(&p, &fixture.root);
    assert!(baseline.unchanged, "{baseline:?}");
    fixture.write("unrelated.rs", "unrelated");
    fixture.write("fictional/arch/extra.rs", "excluded");
    fixture.write("fictional/notes.txt", "excluded");
    assert!(changes_since(&p, &fixture.root).unchanged);
    fixture.write("fictional/src/gpu.rs", "// changed bytes\n");
    fs::remove_file(fixture.root.join("fictional/src/cache.rs")).unwrap();
    fixture.write("fictional/src/added.rs", "// added\n");
    let changes = changes_since(&p, &fixture.root);
    assert!(!changes.unchanged);
    assert!(changes.errors.is_empty(), "{changes:?}");
    assert_eq!(changes.added, [PathBuf::from("fictional/src/added.rs")]);
    assert_eq!(changes.removed, [PathBuf::from("fictional/src/cache.rs")]);
    assert_eq!(changes.modified, [PathBuf::from("fictional/src/gpu.rs")]);
    assert_eq!(changes.affected_nodes, ["cache", "gpu", "uploader"]);
    let mut refreshed = p.clone();
    refreshed.source.files = try_manifest("fictional/", &fixture.root).unwrap();
    assert!(changes_since(&refreshed, &fixture.root).unchanged);
    refreshed
        .nodes
        .get_mut("worker")
        .unwrap()
        .refs
        .push("fictional/src/future.rs::Future".into());
    fixture.write("fictional/src/future.rs", "");
    assert_eq!(
        changes_since(&refreshed, &fixture.root).affected_nodes,
        ["worker"]
    );
}

#[test]
fn changes_checks_each_recorded_file_and_never_hides_errors() {
    let fixture = Fixture::example();
    let mut p = example();
    p.arch.scope = "fictional/src/lib.rs".into();
    fixture.write("fictional/src/gpu.rs", "changed outside file scope");
    fixture.write("fictional/src/added.rs", "not discovered for file scope");
    let changes = changes_since(&p, &fixture.root);
    assert!(changes.added.is_empty());
    assert_eq!(changes.modified, [PathBuf::from("fictional/src/gpu.rs")]);
    assert_eq!(changes.affected_nodes, ["gpu", "uploader"]);
    p.source.files[0].path = "../outside.rs".into();
    let changes = changes_since(&p, &fixture.root);
    assert!(!changes.unchanged && !changes.errors.is_empty());
    assert!(changes.removed.is_empty());
    let missing = changes_since(&example(), fixture.root.join("missing"));
    assert!(!missing.unchanged && !missing.errors.is_empty());
    let mut p = example();
    p.source.files[0].hash = "bad".into();
    assert!(!changes_since(&p, &fixture.root).errors.is_empty());
    fs::remove_file(fixture.root.join("fictional/src/gpu.rs")).unwrap();
    fs::create_dir(fixture.root.join("fictional/src/gpu.rs")).unwrap();
    assert!(changes_since(&example(), &fixture.root)
        .errors
        .iter()
        .any(|e| e.kind == ErrorKind::InvalidPath));
}

#[test]
fn manifest_matches_hash_and_order_with_every_corpus_exclusion() {
    let fixture = Fixture::new();
    for path in ["crate/z.rs", "crate/src/a.rs", "b.rs"] {
        fixture.write(path, "");
    }
    let excluded = [
        "old/a.rs",
        "local/a.rs",
        "libs/windows/a.rs",
        "libs/apple_sys/a.rs",
        "libs/jni-sys/a.rs",
        "libs/objc-sys/a.rs",
        "libs/vulkan/a.rs",
        "crate/target/a.rs",
        "crate/target-web/a.rs",
        "crate/.hidden/a.rs",
        "crate/arch/a.rs",
        "arch/b.rs",
        "crate/Cargo.toml",
        "crate/a.RS",
        "crate/readme.txt",
        "crate/nested/a.rs",
    ];
    for path in excluded {
        fixture.write(path, "excluded");
    }
    fixture.write("crate/nested/.git", "gitdir: elsewhere");
    let hashes = try_manifest(".", &fixture.root).unwrap();
    assert_eq!(
        hashes.iter().map(|h| h.path.clone()).collect::<Vec<_>>(),
        [
            PathBuf::from("b.rs"),
            "crate/src/a.rs".into(),
            "crate/z.rs".into()
        ]
    );
    assert!(hashes
        .iter()
        .all(|h| h.hash == "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"));
    fixture.write("b.rs", "hello\n");
    assert_eq!(
        manifest("b.rs", &fixture.root)[0].hash,
        "ce013625030ba8dba906f756967f9e9ca394464a"
    );
    assert_eq!(manifest("crate/", &fixture.root).len(), 2);
    for path in excluded {
        assert!(manifest(path, &fixture.root).is_empty(), "{path}");
    }
    assert!(manifest("../escape", &fixture.root).is_empty());
    assert_eq!(
        try_manifest("../escape", &fixture.root).unwrap_err().kind,
        ErrorKind::InvalidPath
    );
}

#[test]
fn manifest_overflow_is_explicit_and_never_truncated_or_unchanged() {
    let fixture = Fixture::new();
    for i in 0..512 {
        fixture.write(format!("crate/{i:04}.rs"), "");
    }
    let files = try_manifest("crate", &fixture.root).unwrap();
    assert_eq!(files.len(), 512);
    assert_eq!(
        files.iter().map(|h| &h.path).collect::<BTreeSet<_>>().len(),
        512
    );
    fixture.write("crate/overflow.rs", "");
    assert_eq!(
        try_manifest("crate", &fixture.root).unwrap_err().kind,
        ErrorKind::Limit
    );
    assert!(manifest("crate", &fixture.root).is_empty());
    let mut p = example();
    p.arch.scope = "crate".into();
    p.source.files = files;
    let changes = changes_since(&p, &fixture.root);
    assert!(!changes.unchanged);
    assert!(changes.errors.iter().any(|e| e.kind == ErrorKind::Limit));
}

#[test]
fn discovery_depth_and_entry_limits_accept_boundary_reject_overflow() {
    let fixture = Fixture::new();
    let mut path = PathBuf::from("deep");
    for _ in 0..128 {
        path.push("d");
    }
    fs::create_dir_all(fixture.root.join(&path)).unwrap();
    assert!(try_manifest("deep", &fixture.root).unwrap().is_empty());
    path.push("overflow");
    fs::create_dir(fixture.root.join(&path)).unwrap();
    let error = try_manifest("deep", &fixture.root).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.field, "discovery depth");
    let wide = fixture.root.join("wide");
    fs::create_dir(&wide).unwrap();
    for i in 0..100_000 {
        fs::File::create(wide.join(format!("{i}.txt"))).unwrap();
    }
    assert!(try_manifest("wide", &fixture.root).unwrap().is_empty());
    fs::File::create(wide.join("overflow.txt")).unwrap();
    let error = try_manifest("wide", &fixture.root).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.field, "discovery entries");
}

#[test]
fn validation_rechecks_mutated_plans() {
    let fixture = Fixture::example();
    let mut p = example();
    p.nodes.insert("BAD".into(), p.nodes["cache"].clone());
    assert!(validate(&p, &fixture.root)
        .errors
        .iter()
        .any(|e| e.kind == ErrorKind::InvalidId));
    p.nodes.remove("BAD");
    p.nodes.get_mut("cache").unwrap().story = "x".repeat(601);
    assert!(validate(&p, &fixture.root)
        .errors
        .iter()
        .any(|e| e.kind == ErrorKind::Limit));
}

#[cfg(unix)]
#[test]
fn symlinks_are_contained_and_excluded_from_discovery() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::example();
    let outside = Fixture::new();
    outside.write("escape.rs", "private outside bytes");
    symlink(&outside.root, fixture.root.join("fictional/src/outside")).unwrap();
    symlink(
        outside.root.join("escape.rs"),
        fixture.root.join("fictional/src/alias.rs"),
    )
    .unwrap();
    symlink(
        fixture.root.join("fictional/src/cache.rs"),
        fixture.root.join("fictional/src/internal.rs"),
    )
    .unwrap();
    let files = try_manifest("fictional", &fixture.root).unwrap();
    assert_eq!(files.len(), 8);
    assert!(manifest("fictional/src/internal.rs", &fixture.root).is_empty());
    assert_eq!(
        try_manifest("fictional/src/outside", &fixture.root)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidPath
    );
    let mut p = example();
    for path in [
        "fictional/src/alias.rs",
        "fictional/src/outside/escape.rs",
        "fictional/src/outside/missing.rs",
    ] {
        p.nodes.get_mut("cache").unwrap().refs = vec![path.into()];
        assert!(validate(&p, &fixture.root)
            .errors
            .iter()
            .any(|e| e.kind == ErrorKind::InvalidPath));
        p.source.files[0].path = path.into();
        let changes = changes_since(&p, &fixture.root);
        assert!(!changes.unchanged && !changes.errors.is_empty());
        assert!(!changes.removed.contains(&PathBuf::from(path)));
    }
    symlink("loop", fixture.root.join("loop")).unwrap();
    assert_eq!(
        try_manifest("loop", &fixture.root).unwrap_err().kind,
        ErrorKind::Io
    );
}
