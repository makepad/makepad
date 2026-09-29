use super::*;
use makepad_zip_file::{ZipMethod, ZipWriter};

const EXAMPLE: &str = include_str!("../../../docs/agents/change-report-example/report.json");
const SCHEMA: &str = include_str!("../../../docs/agents/change-report.schema.json");

fn example() -> Report {
    parse_report(EXAMPLE.as_bytes()).expect("the documented example is a valid report")
}

fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new();
    for (name, bytes) in files {
        zip.add(name, bytes, ZipMethod::Deflate).unwrap();
    }
    zip.finish().unwrap()
}

fn with_json(json: &str) -> Vec<u8> {
    let report = parse_report(json.as_bytes()).unwrap();
    zip(&[(REPORT_JSON, json.as_bytes()), (REPORT_MD, report.to_markdown().as_bytes())])
}

fn edited(from: &str, to: &str) -> String {
    assert!(EXAMPLE.contains(from), "{from}");
    EXAMPLE.replacen(from, to, 1)
}

#[test]
fn documented_example_round_trips_and_opens() {
    let report = example();
    assert_eq!(report.app, "calculator");
    assert_eq!(report.changes.len(), 2);
    assert_eq!(report.changes[1].snippet.as_ref().unwrap().language, "splash");
    assert_eq!(parse_report(report.to_json().as_bytes()).unwrap(), report);
    let package = open(&with_json(EXAMPLE)).unwrap();
    assert_eq!(package.report, report);
    assert!(package.markdown.contains("## 1. Enter repeats the last operation (feature)"));
    assert!(package.markdown.contains("- home folder path × 2"));
    assert!(package.diff.is_none());
}

#[test]
fn schema_file_matches_the_checker() {
    let schema = makepad_strict_json::parse_depth(SCHEMA.as_bytes(), 32).expect("the schema is JSON");
    let changes = schema.get("properties").and_then(|p| p.get("changes")).unwrap();
    let item = changes.get("items").and_then(|i| i.get("properties")).unwrap();
    let kinds: Vec<&str> = item.get("kind").and_then(|k| k.get("enum")).and_then(Value::as_arr).unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(kinds, KINDS);
    let max = |v: &Value, key: &str| v.get(key).and_then(Value::as_i64).unwrap() as usize;
    assert_eq!(max(changes, "maxItems"), MAX_CHANGES);
    assert_eq!(max(item.get("title").unwrap(), "maxLength"), MAX_TITLE);
    assert_eq!(max(item.get("what").unwrap(), "maxLength"), MAX_WHAT);
    assert_eq!(max(item.get("why").unwrap(), "maxLength"), MAX_WHY);
    assert_eq!(max(item.get("areas").unwrap(), "maxItems"), MAX_AREAS);
    let snippet = item.get("snippet").and_then(|s| s.get("properties")).and_then(|p| p.get("text")).unwrap();
    assert_eq!(max(snippet, "maxLength"), MAX_SNIPPET);
    let required: Vec<&str> = schema.get("required").and_then(Value::as_arr).unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(required, ["format", "version", "app", "changes", "redactions", "diff"]);
    assert_eq!(schema.get("properties").and_then(|p| p.get("format")).and_then(|f| f.get("const")).and_then(Value::as_str), Some(FORMAT));
}

#[test]
fn schema_rules_are_enforced() {
    let cases = [
        (edited("\"makepad-change-report\"", "\"something-else\""), "format"),
        (edited("\"version\": 1", "\"version\": 2"), "version"),
        (edited("\"kind\": \"feature\"", "\"kind\": \"bugfix\""), "must be one of"),
        (edited("\"summary\"", "\"author\""), "unknown field \"author\""),
        (edited("\"what\": \"Pressing", "\"how\": \"x\", \"what\": \"Pressing"), "unknown field \"how\""),
        (edited("\"title\": \"Larger result text\",", ""), "title is missing"),
        (edited("\"release\": \"2026-09-20\"", "\"release\": \"../etc\""), "identifiers"),
        (edited("\"commit\": \"7a533b61ee0c4e1f9d2b3a4c5d6e7f8091a2b3c4\"", "\"commit\": \"main\""), "hexadecimal"),
        (edited("\"diff\": false", "\"diff\": \"no\""), "\"diff\" must be true or false"),
        (edited("\"count\": 2", "\"count\": 0"), "count"),
        (edited("\"Larger result text\"", "\"Larger\\nresult\""), "one line"),
    ];
    for (json, expected) in cases {
        let error = parse_report(json.as_bytes()).unwrap_err();
        assert!(error.contains(expected), "{expected}: {error}");
    }
    let no_redactions = EXAMPLE.split("\"redactions\"").next().unwrap().to_owned() + "\"diff\": false\n}";
    assert!(parse_report(no_redactions.as_bytes()).unwrap_err().contains("redactions"));
    let empty = EXAMPLE.split("\"changes\"").next().unwrap().to_owned() + "\"changes\": [], \"redactions\": [], \"diff\": false}";
    assert!(parse_report(empty.as_bytes()).unwrap_err().contains("no changes"));
    let long = edited("Larger result text", &"x".repeat(MAX_TITLE + 1));
    assert!(parse_report(long.as_bytes()).unwrap_err().contains("longer than 120"));
}

#[test]
fn zip_contents_and_sizes_are_checked() {
    let json = EXAMPLE.as_bytes();
    let markdown = example().to_markdown();
    // Too large as a whole: incompressible bytes, so the zip itself is big.
    let mut noise = String::new();
    let mut x = 1u64;
    while noise.len() < 700 * 1024 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        noise.push_str(&format!("{:016x}\n", x));
    }
    let big = zip(&[(REPORT_JSON, json), (REPORT_MD, markdown.as_bytes()), (CHANGES_DIFF, noise.as_bytes())]);
    assert!(big.len() > MAX_ZIP_BYTES);
    assert!(open(&big).unwrap_err().contains("at most 256 KB"));
    // Small zipped but over a file's own limit unpacked.
    let spread = "+ line\n".repeat(MAX_DIFF_BYTES / 7 + 10);
    let with_diff = edited("\"diff\": false", "\"diff\": true");
    let over = zip(&[(REPORT_JSON, with_diff.as_bytes()), (REPORT_MD, markdown.as_bytes()), (CHANGES_DIFF, spread.as_bytes())]);
    assert!(over.len() < MAX_ZIP_BYTES);
    assert!(open(&over).unwrap_err().contains("changes.diff is larger than 1024 KB"));
    // Only the three files, each once, as text.
    let extra = zip(&[(REPORT_JSON, json), (REPORT_MD, markdown.as_bytes()), ("notes.txt", b"hello")]);
    assert!(open(&extra).unwrap_err().contains("\"notes.txt\""));
    let binary = zip(&[(REPORT_JSON, json), (REPORT_MD, &[0x89, b'P', b'N', b'G', 0, 1, 2])]);
    assert!(open(&binary).unwrap_err().contains("REPORT.md"));
    assert!(open(&zip(&[(REPORT_JSON, json)])).unwrap_err().contains("no REPORT.md"));
    assert!(open(b"not a zip").unwrap_err().contains("not a zip"));
    // REPORT.md shows the same changes.
    let partial = zip(&[(REPORT_JSON, json), (REPORT_MD, b"# Changes\n\n1. Enter repeats the last operation\n")]);
    assert!(open(&partial).unwrap_err().contains("Larger result text"));
    // The diff is there exactly when the report says so, and holds no binary patch.
    let unannounced = zip(&[(REPORT_JSON, json), (REPORT_MD, markdown.as_bytes()), (CHANGES_DIFF, b"--- a/x\n+++ b/x\n")]);
    assert!(open(&unannounced).unwrap_err().contains("\"diff\": false"));
    assert!(open(&zip(&[(REPORT_JSON, with_diff.as_bytes()), (REPORT_MD, markdown.as_bytes())])).unwrap_err().contains("no changes.diff"));
    let patch = zip(&[(REPORT_JSON, with_diff.as_bytes()), (REPORT_MD, markdown.as_bytes()), (CHANGES_DIFF, b"diff --git a/i.png b/i.png\nGIT binary patch\nliteral 12\n")]);
    assert!(open(&patch).unwrap_err().contains("binary patch"));
    let diff = "--- a/makepad/apps/calculator/src/app.rs\n+++ b/makepad/apps/calculator/src/app.rs\n@@ -1 +1 @@\n-font_size: 20\n+font_size: 28\n";
    let fine = zip(&[(REPORT_JSON, with_diff.as_bytes()), (REPORT_MD, markdown.as_bytes()), (CHANGES_DIFF, diff.as_bytes())]);
    assert_eq!(open(&fine).unwrap().diff.as_deref(), Some(diff));
}

/// The anonymisation checklist of docs/agents/change-report.md, line by
/// line: each personal or secret item is found, its redacted form is not.
#[test]
fn anonymisation_checklist() {
    let flagged = [
        ("Reported by jane.doe@gmail.com", EMAIL),
        ("// ask Bob <bob.smith@acme-corp.io> first", EMAIL),
        ("path = \"/Users/janedoe/Projects/notes\"", HOME_PATH),
        ("cd /home/jd/makepad-builder && ./makepad build scope", HOME_PATH),
        ("let root = \"C:\\\\Users\\\\Jane\\\\makepad\";", HOME_PATH),
        ("logs in D:/work/makepad", HOME_PATH),
        ("copy \\\\fileserver\\share\\report", HOME_PATH),
        ("OPENAI_API_KEY=sk-proj-Abcdefghijklmnopqrstuvwxyz0123", SECRET),
        ("let key = \"ghp_0123456789abcdefghijABCDEFGHIJ012345\";", SECRET),
        ("aws AKIAIOSFODNN7EXAMPLE", SECRET),
        ("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig", SECRET),
        ("api_key = \"q8Zr2mXw5Tn\"", SECRET),
        ("\"password\": \"hunter2hunter2\"", SECRET),
        ("-----BEGIN OPENSSH PRIVATE KEY-----", SECRET),
        ("fetch(\"https://user:pa55word@example.org/api\")", URL_CREDENTIALS),
        ("const SERVER: &str = \"http://192.168.1.20:8080/api\";", PRIVATE_HOST),
        ("ssh arch@10.0.0.165", PRIVATE_HOST),
        ("see https://wiki.internal/team/page", PRIVATE_HOST),
        ("ssh me@nas.local", EMAIL),
    ];
    for (line, what) in flagged {
        let found = scan("REPORT.md", line);
        assert!(found.iter().any(|f| f.what == what), "{line:?} should find {what}, found {found:?}");
        assert_eq!(found[0].line, 1);
    }
    let clean = [
        "Reported by [name] ([email])",
        "path = \"[path]/Projects/notes\"",
        "--- a/makepad/apps/calculator/src/app.rs",
        "OPENAI_API_KEY=[secret]",
        "api_key = \"${OPENAI_API_KEY}\"",
        "let token = env::var(\"TOKEN\")?;",
        "password: String,",
        "The password field now hides what is typed.",
        "Send to info@makepad.nl or test@example.com",
        "git clone git@github.com:makepad/makepad.git",
        "https://makepad.nl/api/feedback and http://127.0.0.1:8080/ and http://localhost:3000",
        "let s = \"line\\\\nnext\"; // an escaped newline",
        "std::fs::read(path)? ; x: 10:30",
        "@scope/package and npm i @types/node",
        "version 10.0.0 and 1.2.3.4.5",
        "ask-your-agent: sk-short",
    ];
    for line in clean {
        assert_eq!(scan("report.json", line), Vec::new(), "{line:?}");
    }
}

#[test]
fn a_report_with_personal_data_is_refused_with_every_finding() {
    let json = edited("keyboard-first workflow.", "keyboard-first workflow at /Users/janedoe (jane@corp.com).");
    let report = parse_report(json.as_bytes()).unwrap();
    let error = open(&zip(&[(REPORT_JSON, json.as_bytes()), (REPORT_MD, report.to_markdown().as_bytes())])).unwrap_err();
    assert!(error.starts_with("The report still holds 4 things to take out:"), "{error}");
    assert!(error.contains("report.json line 6: email address (jane@corp.com)"), "{error}");
    assert!(error.contains("REPORT.md line 3: absolute or home folder path (/Users/janedoe)"), "{error}");
}

#[test]
fn a_folder_is_packed_and_checked() {
    let folder = std::env::temp_dir().join(format!("makepad-change-report-{}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join(REPORT_JSON), EXAMPLE).unwrap();
    fs::write(folder.join(REPORT_MD), example().to_markdown()).unwrap();
    fs::write(folder.join(".DS_Store"), [0u8, 1, 2]).unwrap();
    let package = load(&folder).unwrap();
    assert_eq!(package.report, example());
    assert_eq!(open(&package.zip).unwrap().report, example());
    // The zip written from it loads the same.
    let file = folder.with_extension("zip");
    fs::write(&file, &package.zip).unwrap();
    assert_eq!(load(&file).unwrap().markdown, package.markdown);
    fs::write(folder.join("notes.txt"), "x").unwrap();
    assert!(load(&folder).unwrap_err().contains("notes.txt"));
    let _ = fs::remove_dir_all(&folder);
    let _ = fs::remove_file(&file);
}

#[test]
fn endpoint_and_answer() {
    assert_eq!(endpoint(None), DEFAULT_ENDPOINT);
    assert_eq!(endpoint(Some("  ")), DEFAULT_ENDPOINT);
    assert_eq!(endpoint(Some("http://127.0.0.1:9000/api/feedback/")), "http://127.0.0.1:9000/api/feedback/changes");
    assert_eq!(response_id(b"{\"ok\":true,\"id\":42}"), Ok(42));
    assert_eq!(response_id(b"{\"error\":\"Too much feedback\"}"), Err("Too much feedback".into()));
    assert!(response_id(b"<html>").is_err());
}
