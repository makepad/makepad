use super::*;
use std::{fs, path::PathBuf};

const ENV: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Fixture"),
    ("GIT_AUTHOR_EMAIL", "fixture@example.com"),
    ("GIT_COMMITTER_NAME", "Fixture"),
    ("GIT_COMMITTER_EMAIL", "fixture@example.com"),
];

/// A throwaway repository in the system's temporary directory.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("source-slice-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"], None, ENV).unwrap();
        Fixture { dir }
    }
    fn write(&self, path: &str, text: &str) {
        let path = self.dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn run(&self, args: &[&str]) -> String {
        text(git(&self.dir, args, None, ENV).unwrap()).unwrap()
    }
    fn commit(&self) -> String {
        self.run(&["add", "-A"]);
        self.run(&["-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", "fixture"]);
        self.run(&["rev-parse", "HEAD"])
    }
    fn files(&self, commit: &str) -> Vec<String> {
        self.run(&["ls-tree", "-r", "--name-only", commit]).lines().map(str::to_owned).collect()
    }
    fn show(&self, spec: &str) -> String {
        String::from_utf8(git(&self.dir, &["cat-file", "blob", spec], None, &[]).unwrap()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn package(name: &str, rest: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{rest}")
}

fn roots(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// The commercial repository's shape: a members package at the root, Stage
/// with Amp and its libraries, Scope with its own deps, and repository files
/// no slice ships.
fn commercial(name: &str) -> Fixture {
    let f = Fixture::new(name);
    f.write(
        "Cargo.toml",
        &package(
            "makepad-commercial-members",
            "publish = false\n[target.'cfg(any())'.dependencies]\nmakepad-amp = { path = \"stage/apps/amp\" }\nmakepad-scope = { path = \"scope\" }\nmakepad-stage = { path = \"stage/app\" }\n",
        ),
    );
    f.write("src/lib.rs", "");
    f.write("README.md", "private\n");
    f.write("slices.json", "{\"amp\":[\"makepad-amp\"],\"scope\":[\"makepad-scope\"]}\n");
    f.write("local/notes.txt", "private notes\n");
    f.write(
        "stage/apps/amp/Cargo.toml",
        &package(
            "makepad-amp",
            "[[bin]]\nname = \"makepad-amp\"\npath = \"src/main.rs\"\n\
             [dependencies]\n\
             makepad-widgets = { path = \"../../../../widgets\", default-features = false }\n\
             milkdrop = { path = \"../../libs/milkdrop\", optional = true }\n\
             serde = \"1\"\n\
             [dependencies.visuals]\npath = \"../../libs/visuals\"\n\
             [build-dependencies]\namp-build = { path = \"../../libs/amp_build\" }\n\
             [target.'cfg(windows)'.dependencies]\namp-win = { path = \"../../libs/amp_win\" }\n\
             [target.'cfg(unix)'.build-dependencies]\namp-unix-build = { path = \"../../libs/amp_unix_build\" }\n\
             [dev-dependencies]\nstage-test = { path = \"../../libs/stage_test\" }\n\
             [package.metadata.commercial]\ninclude = [\"../../resources/effects\", \"../../shared.txt\"]\n",
        ),
    );
    f.write(
        "stage/apps/amp/src/main.rs",
        "const A: &str = include_str!(\"../../../resources/effects/a.glsl\");\n\
         const S: &str = include_str!(\"../../../resources/secret.txt\");\n\
         const W: &str = include_str!(\"../../../../../widgets/x.rs\");\n\
         // const C: &str = include_str!(\"../../../resources/commented.txt\");\n\
         const M: &[u8] = include_bytes!(\"../../../libs/milkdrop/src/table.bin\");\n\
         fn main() {\n\
             let _ = crate_resource(\"self:resources/icon.svg\");\n\
             let _ = crate_resource(\"self:../../resources/effects/b.glsl\");\n\
             let _ = crate_resource(\"self:../../resources/secret.txt\");\n\
             let _ = crate_resource(\"makepad_widgets:resources/Inter.ttf\");\n\
             let _ = crate_resource(\"makepad_scope:resources/logo.svg\");\n\
         }\n",
    );
    f.write("stage/apps/amp/src/ui.splash", "icon: crate_resource(\"self:../../app/resources/stage.svg\")\n");
    f.write("stage/apps/amp/build.rs", "fn main() { let _ = (\"../../resources/effects\", \"../../resources/build-only.txt\"); }\n");
    f.write("stage/apps/amp/resources/icon.svg", "<svg/>\n");
    f.write("stage/apps/amp/tests/fixture/Cargo.toml", &package("amp-fixture", ""));
    f.write("stage/apps/amp/tests/fixture/src/lib.rs", "");
    f.write("stage/apps/amp/tests/smoke.rs", "#[test] fn t() {}\n");
    f.write("stage/libs/milkdrop/Cargo.toml", &package("milkdrop", "[dependencies]\nshared = { path = \"../shared\" }\n"));
    f.write("stage/libs/milkdrop/src/lib.rs", "");
    f.write("stage/libs/milkdrop/src/table.bin", "bin");
    f.write("stage/libs/shared/Cargo.toml", &package("stage-shared", ""));
    f.write("stage/libs/shared/src/lib.rs", "");
    f.write("stage/libs/visuals/Cargo.toml", &package("visuals", ""));
    f.write("stage/libs/visuals/src/lib.rs", "");
    f.write("stage/libs/amp_build/Cargo.toml", &package("amp-build", ""));
    f.write("stage/libs/amp_build/src/lib.rs", "");
    f.write("stage/libs/amp_win/Cargo.toml", &package("amp-win", ""));
    f.write("stage/libs/amp_win/src/lib.rs", "");
    f.write("stage/libs/amp_unix_build/Cargo.toml", &package("amp-unix-build", ""));
    f.write("stage/libs/amp_unix_build/src/lib.rs", "");
    f.write("stage/libs/stage_test/Cargo.toml", &package("stage-test", ""));
    f.write("stage/libs/stage_test/src/lib.rs", "");
    f.write("stage/resources/effects/a.glsl", "void main(){}\n");
    f.write("stage/resources/effects/b.glsl", "void main(){}\n");
    f.write("stage/resources/secret.txt", "stage only\n");
    f.write("stage/resources/build-only.txt", "x\n");
    f.write("stage/shared.txt", "shared\n");
    f.write("stage/app/Cargo.toml", &package("makepad-stage", "[dependencies]\nmilkdrop = { path = \"../libs/milkdrop\" }\n"));
    f.write("stage/app/src/main.rs", "fn main() {}\n");
    f.write("stage/app/resources/stage.svg", "<svg/>\n");
    f.write("scope/Cargo.toml", &package("makepad-scope", "[dependencies]\nscope-dep = { path = \"deps/scope_dep\" }\nmakepad-widgets = { path = \"../../../widgets\" }\n"));
    f.write("scope/src/main.rs", "fn main() {}\n");
    f.write("scope/resources/logo.svg", "<svg/>\n");
    f.write("scope/deps/scope_dep/Cargo.toml", &package("scope-dep", ""));
    f.write("scope/deps/scope_dep/src/lib.rs", "");
    f.write("scope/ci.splash", "nil\n");
    f
}

#[test]
fn closure_follows_normal_build_and_target_dependencies_but_not_dev_or_external_ones() {
    let f = commercial("closure");
    let commit = f.commit();
    let source = Source::load(&f.dir, &commit).unwrap();
    let names: Vec<&str> = source.closure(&roots(&["makepad-amp"])).unwrap().iter().map(|c| c.name.as_str()).collect();
    // stage-test is a dev-dependency, makepad-widgets leaves the repository,
    // serde is from crates.io; everything else is reached, the optional,
    // dotted-table, build and both target tables included.
    assert_eq!(names, vec!["makepad-amp", "amp-build", "amp-unix-build", "amp-win", "milkdrop", "stage-shared", "visuals"]);
    let scope: Vec<&str> = source.closure(&roots(&["makepad-scope"])).unwrap().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(scope, vec!["makepad-scope", "scope-dep"]);
    let both = source.closure(&roots(&["makepad-scope", "makepad-amp"])).unwrap();
    assert_eq!(both.len(), 9);
    assert!(source.closure(&roots(&["makepad-nothing"])).unwrap_err().contains("No package makepad-nothing"));
    assert!(source.closure(&[]).is_err());
    assert!(source.closure(&roots(&["makepad-commercial-members"])).unwrap_err().contains("root package"));
}

#[test]
fn a_slice_holds_exactly_its_crates_their_includes_and_the_generated_members_package() {
    let f = commercial("tree");
    let commit = f.commit();
    let slice = slice(&f.dir, &commit, "amp", &roots(&["makepad-amp"])).unwrap();
    let files = f.files(&slice.commit);
    let expected = [
        "Cargo.toml",
        "src/lib.rs",
        "stage/apps/amp/Cargo.toml",
        "stage/apps/amp/build.rs",
        "stage/apps/amp/resources/icon.svg",
        "stage/apps/amp/src/main.rs",
        "stage/apps/amp/src/ui.splash",
        "stage/apps/amp/tests/smoke.rs",
        "stage/libs/amp_build/Cargo.toml",
        "stage/libs/amp_build/src/lib.rs",
        "stage/libs/amp_unix_build/Cargo.toml",
        "stage/libs/amp_unix_build/src/lib.rs",
        "stage/libs/amp_win/Cargo.toml",
        "stage/libs/amp_win/src/lib.rs",
        "stage/libs/milkdrop/Cargo.toml",
        "stage/libs/milkdrop/src/lib.rs",
        "stage/libs/milkdrop/src/table.bin",
        "stage/libs/shared/Cargo.toml",
        "stage/libs/shared/src/lib.rs",
        "stage/libs/visuals/Cargo.toml",
        "stage/libs/visuals/src/lib.rs",
        "stage/resources/effects/a.glsl",
        "stage/resources/effects/b.glsl",
        "stage/shared.txt",
    ];
    assert_eq!(files, expected, "the nested fixture package, dev-dependencies, Scope, Stage's app, repository files and local/ stay out");
    assert_eq!(slice.files, expected.len() as u64);
    assert_eq!(slice.crates.len(), 7);
    let manifest = f.show(&format!("{}:Cargo.toml", slice.commit));
    assert_eq!(manifest, members_manifest(&slice.crates));
    assert!(manifest.contains("\n[target.'cfg(any())'.dependencies]\n\"amp-build\" = { path = \"stage/libs/amp_build\" }\n"));
    assert!(manifest.contains("\n\"makepad-amp\" = { path = \"stage/apps/amp\" }\n"));
    assert!(manifest.contains("name = \"makepad-commercial-members\""));
    assert!(!manifest.contains("makepad-scope") && !manifest.contains("makepad-stage\""));
    // The generated manifest reads as the members package it claims to be.
    let doc = parse_toml(&manifest).unwrap();
    let members = doc.get_path(&["target", "cfg(any())", "dependencies"]).and_then(Toml::as_table).unwrap();
    assert_eq!(members.len(), 7);
    assert_eq!(members["milkdrop"].as_table().unwrap()["path"].as_str(), Some("stage/libs/milkdrop"));
    assert_eq!(f.show(&format!("{}:src/lib.rs", slice.commit)), "");
    // Unchanged blobs keep their identity: the slice ships the source's files.
    assert_eq!(f.run(&["rev-parse", &format!("{}:stage/libs/milkdrop", slice.commit)]), f.run(&["rev-parse", &format!("{commit}:stage/libs/milkdrop")]));

    let scope = super::slice(&f.dir, &commit, "scope", &roots(&["makepad-scope"])).unwrap();
    assert_eq!(
        f.files(&scope.commit),
        vec!["Cargo.toml", "scope/Cargo.toml", "scope/ci.splash", "scope/deps/scope_dep/Cargo.toml", "scope/deps/scope_dep/src/lib.rs", "scope/resources/logo.svg", "scope/src/main.rs", "src/lib.rs"]
    );
}

#[test]
fn the_slice_commit_is_parentless_fixed_and_deterministic() {
    let f = commercial("determinism");
    let commit = f.commit();
    let a = slice(&f.dir, &commit, "amp", &roots(&["makepad-amp"])).unwrap();
    let b = slice(&f.dir, &commit, "amp", &roots(&["makepad-amp"])).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.source, commit);
    let raw = f.run(&["cat-file", "commit", &a.commit]);
    assert!(!raw.contains("\nparent "), "{raw}");
    assert!(raw.contains("\nauthor Makepad <info@makepad.nl> 0 +0000\ncommitter Makepad <info@makepad.nl> 0 +0000\n"), "{raw}");
    assert!(raw.ends_with(&format!("\n\ncommercial {} amp", &commit[..12])), "{raw}");
    assert!(!raw.contains("fixture"), "nothing of the source commit's message: {raw}");
    // Another app name is another commit over the same tree.
    let c = slice(&f.dir, &commit, "music", &roots(&["makepad-amp"])).unwrap();
    assert_ne!(a.commit, c.commit);
    assert_eq!(f.run(&["rev-parse", &format!("{}^{{tree}}", a.commit)]), f.run(&["rev-parse", &format!("{}^{{tree}}", c.commit)]));
    // A shallow bare clone elsewhere (what the server fetches) cuts the same commit.
    let bare = std::env::temp_dir().join(format!("source-slice-determinism-bare-{}", std::process::id()));
    let _ = fs::remove_dir_all(&bare);
    let url = format!("file://{}", f.dir.display());
    git(Path::new("/"), &["clone", "-q", "--bare", "--depth=1", &url, &bare.display().to_string()], None, ENV).unwrap();
    let d = slice(&bare, &commit, "amp", &roots(&["makepad-amp"])).unwrap();
    fs::remove_dir_all(&bare).unwrap();
    assert_eq!(a.commit, d.commit);
    // Changing a file outside the slice changes its source commit only.
    f.write("scope/src/main.rs", "fn main() { println!(); }\n");
    let next = f.commit();
    let e = slice(&f.dir, &next, "amp", &roots(&["makepad-amp"])).unwrap();
    assert_eq!(f.run(&["rev-parse", &format!("{}^{{tree}}", a.commit)]), f.run(&["rev-parse", &format!("{}^{{tree}}", e.commit)]));
    assert_ne!(a.commit, e.commit, "the message names the source commit");
}

#[test]
fn includes_must_exist_inside_the_repository_and_path_dependencies_must_be_packages() {
    let f = commercial("errors");
    f.write("stage/libs/visuals/Cargo.toml", &package("visuals", "[package.metadata.commercial]\ninclude = [\"../../../../outside\"]\n"));
    let commit = f.commit();
    let error = slice(&f.dir, &commit, "amp", &roots(&["makepad-amp"])).unwrap_err();
    assert!(error.contains("outside the repository"), "{error}");

    f.write("stage/libs/visuals/Cargo.toml", &package("visuals", "[package.metadata.commercial]\ninclude = [\"../../resources/missing\"]\n"));
    let commit = f.commit();
    let error = slice(&f.dir, &commit, "amp", &roots(&["makepad-amp"])).unwrap_err();
    assert!(error.contains("does not exist"), "{error}");

    f.write("stage/libs/visuals/Cargo.toml", &package("visuals", "[dependencies]\nplain = { path = \"../../resources\" }\n"));
    let commit = f.commit();
    let error = slice(&f.dir, &commit, "amp", &roots(&["makepad-amp"])).unwrap_err();
    assert!(error.contains("stage/resources, which has no package Cargo.toml"), "{error}");

    f.write("stage/libs/visuals/Cargo.toml", &package("visuals", "[dependencies]\ngone = { path = \"../gone\" }\n"));
    let commit = f.commit();
    let error = slice(&f.dir, &commit, "amp", &roots(&["makepad-amp"])).unwrap_err();
    assert!(error.contains("stage/libs/gone, which does not exist"), "{error}");

    // A dev-dependency's path is never looked at.
    f.write("stage/libs/visuals/Cargo.toml", &package("visuals", "[dev-dependencies]\ngone = { path = \"../gone\" }\n"));
    let commit = f.commit();
    slice(&f.dir, &commit, "amp", &roots(&["makepad-amp"])).unwrap();
}

#[cfg(unix)]
#[test]
fn symbolic_links_and_submodules_in_a_slice_are_refused() {
    let f = commercial("links");
    std::os::unix::fs::symlink("../../../README.md", f.dir.join("stage/libs/visuals/readme")).unwrap();
    let commit = f.commit();
    let error = slice(&f.dir, &commit, "amp", &roots(&["makepad-amp"])).unwrap_err();
    assert!(error.contains("stage/libs/visuals/readme is a symbolic link"), "{error}");
    // Outside the slice it does not matter.
    slice(&f.dir, &commit, "scope", &roots(&["makepad-scope"])).unwrap();

    fs::remove_file(f.dir.join("stage/libs/visuals/readme")).unwrap();
    f.commit();
    f.run(&["update-index", "--add", "--cacheinfo", &format!("160000,{commit},scope/deps/vendored")]);
    f.run(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "submodule"]);
    let commit = f.run(&["rev-parse", "HEAD"]);
    let error = slice(&f.dir, &commit, "scope", &roots(&["makepad-scope"])).unwrap_err();
    assert!(error.contains("scope/deps/vendored is a submodule"), "{error}");
    slice(&f.dir, &commit, "amp", &roots(&["makepad-amp"])).unwrap();
}

#[test]
fn lint_reports_references_outside_the_crate_that_its_include_does_not_cover() {
    let f = commercial("lint");
    let commit = f.commit();
    let findings = lint(&f.dir, &commit, &roots(&["makepad-amp"])).unwrap();
    let shown: Vec<String> = findings.iter().map(|x| format!("{}:{} {} {}", x.file, x.line, x.target, x.problem)).collect();
    assert_eq!(
        shown,
        vec![
            "stage/apps/amp/build.rs:1 stage/resources/build-only.txt outside the crate and not in its include",
            "stage/apps/amp/src/main.rs:2 stage/resources/secret.txt outside the crate and not in its include",
            "stage/apps/amp/src/main.rs:9 stage/resources/secret.txt outside the crate and not in its include",
            "stage/apps/amp/src/main.rs:11 scope names makepad-scope, which is not part of the slice",
            "stage/apps/amp/src/ui.splash:1 stage/app/resources/stage.svg outside the crate and not in its include",
        ],
        "covered: the included effects, a dependency's own file, the crate's own resources; Makepad's paths and comments are not references"
    );
    assert!(findings[1].reference.starts_with("include_str!("), "{}", findings[1]);
    assert!(lint(&f.dir, &commit, &roots(&["makepad-scope"])).unwrap().is_empty());
}

#[test]
fn materialize_writes_the_slice_tree() {
    let f = commercial("materialize");
    let commit = f.commit();
    let slice = slice(&f.dir, &commit, "scope", &roots(&["makepad-scope"])).unwrap();
    let out = std::env::temp_dir().join(format!("source-slice-materialize-out-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);
    materialize(&f.dir, &slice.commit, &out).unwrap();
    assert!(out.join("scope/deps/scope_dep/src/lib.rs").is_file());
    assert!(out.join("src/lib.rs").is_file());
    assert!(!out.join("stage").exists() && !out.join("README.md").exists());
    assert_eq!(fs::read_to_string(out.join("Cargo.toml")).unwrap(), members_manifest(&slice.crates));
    assert!(materialize(&f.dir, &slice.commit, &out).unwrap_err().contains("not empty"));
    fs::remove_dir_all(&out).unwrap();
}

#[test]
fn paths_resolve_lexically_and_never_leave_the_repository() {
    assert_eq!(resolve("stage/apps/amp", "../../libs/x").as_deref(), Some("stage/libs/x"));
    assert_eq!(resolve("stage/apps/amp", "./src/../res").as_deref(), Some("stage/apps/amp/res"));
    assert_eq!(resolve("stage/apps/amp", "../../../").as_deref(), Some(""));
    assert_eq!(resolve("stage/apps/amp", "../../../../widgets"), None);
    assert_eq!(resolve("scope", "/abs/path"), None);
    assert!(under("stage/libs/x/y", "stage/libs/x") && under("a", "") && !under("stage/libsx", "stage/libs"));
}
