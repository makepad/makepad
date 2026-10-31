// rustc --edition=2021 --test lazy_reexport.rs
// makepad test lazy_reexport.rs
// lazy_reload_tests::deferred_reexport_signatures also loads this fixture with
// child.rs and other.rs forcibly deferred, exercising the signature retry loop.

#[path = "lazy_reexport/child.rs"]
mod child;
#[path = "lazy_reexport/other.rs"]
mod other;
use child::T;

mod fallback {
    #[allow(non_snake_case)]
    pub fn T() -> u32 { 7 }
}

struct Holder {
    value: T,
}

#[test]
fn child_reexport_resolves_type_and_value_independently() {
    let holder = Holder { value: T { value: 42 } };
    assert_eq!(holder.value.value, 42);
    assert_eq!(T(), 7);
}

// `makepad test` itself loads single-file inputs eagerly. Exercise its lazy
// package driver in a fresh process; rustc checks the same Rust semantics above.
#[cfg(relax)]
#[test]
fn lazy_package_check_retries_the_type_namespace() {
    let dir = std::env::temp_dir().join(format!("relax-lazy-reexport-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("lazy_reexport")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"lazy_reexport\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[[bin]]\nname=\"lazy_reexport\"\npath=\"main.rs\"\n").unwrap();
    let source = include_str!("lazy_reexport.rs").to_string()
        + "\nfn main() { let h = Holder { value: T { value: 42 } }; assert_eq!(h.value.value, 42); assert_eq!(T(), 7); }\n";
    std::fs::write(dir.join("main.rs"), source).unwrap();
    std::fs::write(dir.join("lazy_reexport/child.rs"), include_str!("lazy_reexport/child.rs")).unwrap();
    std::fs::write(dir.join("lazy_reexport/other.rs"), include_str!("lazy_reexport/other.rs")).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["check", "lazy_reexport"])
        .current_dir(&dir)
        .env("MAKEPAD_RUN_LAZY_DEFER", "lazy_reexport/child.rs,lazy_reexport/other.rs")
        .env("MAKEPAD_RUN_DEFERRED_TRACE", "1")
        .output().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stderr.contains("signatures name types in"), "lazy signature recovery was not exercised: {stderr}");
}
