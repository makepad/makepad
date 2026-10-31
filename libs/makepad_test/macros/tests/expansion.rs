//! Exercise the real proc-macro bridge. std::proc_macro cannot run in unit
//! tests, so compile the existing expansion cases with the built macro.
use std::{fs, path::PathBuf, process::Command};

fn compile(name: &str, source: &str) -> (std::process::Output, PathBuf) {
    let deps = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let library = fs::read_dir(&deps)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            let file = path.file_name().unwrap().to_string_lossy();
            (file.starts_with("libmakepad_test_macros-") || file.starts_with("makepad_test_macros-"))
                && path.extension().is_some_and(|ext| ext == std::env::consts::DLL_EXTENSION)
        })
        .expect("built proc macro");
    let dir = deps.join(format!("macro-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("case.rs");
    fs::write(&input, source).unwrap();
    let binary = dir.join(format!("case{}", std::env::consts::EXE_SUFFIX));
    let output = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args(["--edition=2021", "--test", "--deny=unused_attributes"])
        .arg(&input)
        .arg("--extern")
        .arg(format!("makepad_test_macros={}", library.display()))
        .arg("-o").arg(&binary)
        .output().unwrap();
    (output, binary)
}

const RUNTIME: &str = r#"
extern crate self as makepad_test;
use makepad_test_macros::makepad_test;
struct TestApp;
mod __private {
    pub fn run_current_package_test<R>(package: &str, manifest: &str, module: &str, name: &str,
        body: fn(super::TestApp) -> R) {
        assert_eq!(package, env!("CARGO_PKG_NAME"));
        assert_eq!(manifest, env!("CARGO_MANIFEST_DIR"));
        assert_eq!(module, "case");
        assert!(!name.is_empty());
        body(super::TestApp);
    }
}
"#;

fn run_case(name: &str, body: &str, args: &[&str]) -> String {
    let (output, binary) = compile(name, &format!("{RUNTIME}\n{body}"));
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let output = Command::new(&binary).args(args).output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8(output.stdout).unwrap();
    fs::remove_dir_all(binary.parent().unwrap()).unwrap();
    stdout
}

#[test]
fn expansion_wraps_test_body() {
    let stdout = run_case("body", r#"
        static CALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        #[makepad_test]
        pub(crate) fn return_submits(mut app: TestApp) -> Result<(), ()> {
            let _ = &mut app;
            CALLED.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        #[test]
        fn verify_wrapper_and_inner() {
            return_submits();
            assert!(CALLED.load(std::sync::atomic::Ordering::SeqCst));
            assert!(__makepad_test_inner_return_submits(TestApp).is_ok());
        }
    "#, &[]);
    assert!(stdout.contains("2 passed"), "{stdout}");
}

#[test]
fn expansion_rejects_arguments() {
    let (output, binary) = compile("arguments", &format!("{RUNTIME}\n#[makepad_test] fn bad(one: TestApp, two: TestApp) {{}}"));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exactly one TestApp argument"));
    fs::remove_dir_all(binary.parent().unwrap()).unwrap();
}

#[test]
fn expansion_preserves_test_attrs_on_wrapper() {
    let stdout = run_case("attrs", r#"
        #[makepad_test]
        #[cfg(all())]
        #[ignore]
        #[should_panic(expected = "boom")]
        fn smoke(_app: TestApp) { panic!("boom"); }

        #[makepad_test]
        #[cfg(any())]
        fn disabled(_app: MissingType) {}

        #[test]
        fn inner_exists() {
            let _inner: fn(TestApp) = __makepad_test_inner_smoke;
        }
    "#, &[]);
    assert!(stdout.contains("1 passed") && stdout.contains("1 ignored"), "{stdout}");
    let stdout = run_case("attrs_ignored", r#"
        #[makepad_test]
        #[ignore]
        #[should_panic(expected = "boom")]
        fn smoke(_app: TestApp) { panic!("boom"); }
    "#, &["--ignored"]);
    assert!(stdout.contains("1 passed"), "{stdout}");
}
