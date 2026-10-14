//! The shared app registry: one table (`apps.json`) of every Makepad app,
//! read by the WM (its menu and launch table), Builder and Stage (an app run
//! names its app by registry id).
//!
//! The table is embedded (`apps.json`, read at first use by table.rs), so
//! `apps()` is a static slice and the lookups never allocate. The rest of the crate answers the checkout
//! questions every reader asks, the one way the WM always answered them:
//! where the sources are (a developer's clone, or the snapshot a Makepad
//! Builder downloaded), whether an app's release binary is there, and the
//! command that builds it (in a Builder installation: the Builder's own).
//!
//! Notes the JSON cannot carry:
//! - code is the local coding agent: a chat window over the AI Hub's model
//!   that reads, searches, edits and runs in a chosen folder, with diffs
//!   and approvals.
//! - amp is the music player over the music systems installed on the
//!   machine (Spotify, local files) with MilkDrop visuals.
//! - photos is the picture wall over a baked library; finance the ledger
//!   (accounts, imports, charts over its own database); fabric makes sewing
//!   patterns from a body measurement (camera, body model, PDF/SVG).
//! - video, image and pdf are `always_new`: a viewer instance per file, so
//!   previews never steal each other's window. image and pdf are hidden:
//!   they open through Files and previews, not the menu.
//! - aichat is the assistant the WM seats in its pane slot, never a menu
//!   row or a tile; splash and counter are the WM's pacing/protocol rigs.
//! - scope and stage come from the optional private checkout of
//!   makepad/commercial at apps/commercial (Scope at scope/, Stage's app at
//!   stage/app, or a Builder's slice of it holding
//!   only the licensed app): present, they are members of this workspace and
//!   build into its target/; absent, `is_available` is false and the WM
//!   drops the row.
//! - studio (apps/studio) is absent in this checkout today; its row stays
//!   and is filtered out the same way.
//! - wm, wm-all and ai-hub are built by Builder but are not
//!   windows the WM hosts (`wm_launchable: false`).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a second launch of an app does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchPolicy {
    /// Every launch starts a new instance (the terminal, the viewers).
    AlwaysNew,
    /// A running instance is focused; otherwise one starts.
    OrFocus,
}

/// One app of the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppEntry {
    /// Registry id: what the WM, Builder and Stage name the app by.
    pub id: &'static str,
    /// The name a human reads in a menu or picker.
    pub label: &'static str,
    /// Cargo package: what `cargo build -p` gets.
    pub package: &'static str,
    /// Package directory relative to the checkout root.
    pub dir: &'static str,
    /// The binary cargo builds and the WM starts. Unique in the table.
    pub bin: &'static str,
    /// A crate outside the root workspace names its own manifest, relative
    /// to the checkout root.
    pub manifest: Option<&'static str>,
    /// Arguments every launch passes.
    pub args: &'static [&'static str],
    pub policy: LaunchPolicy,
    /// A row of the WM's app menu; a hidden app (the viewers, the
    /// assistant, test rigs) opens only by id.
    pub menu_visible: bool,
    /// A window the WM hosts as a client. Builder-only rows (the WM itself,
    /// the AI hub, Stage) are not runnable windows: an app picker filters
    /// on this.
    pub wm_launchable: bool,
    /// Builder's release feature set for it (Builder builds with
    /// `--no-default-features --features <these>`; `build_argv` builds
    /// with the package's default features).
    pub features: &'static [&'static str],
    /// Runs on a phone: starts, draws and is usable by touch at phone size (checked on the Pixel).
    /// The phone WM's home, drawer and app lists offer only these; the desktop ignores it.
    pub mobile: bool,
}

mod table;
mod launch;
pub mod child_env;

pub use launch::{cargo_progress, launch_argv, read_output, AppLaunch, BuildPolicy, LaunchEvent, LaunchKind, ProcessOutput};

impl AppEntry {
    /// What the checkout helpers need to know about it.
    pub fn target(&self) -> AppTarget<'static> {
        AppTarget { id: self.id, package: self.package, dir: self.dir, bin: self.bin, manifest: self.manifest }
    }
}

/// The build/run identity of an app, borrowed: from an `AppEntry`, or from
/// a reader's own copy of one (the WM's `AppDef`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppTarget<'a> {
    pub id: &'a str,
    pub package: &'a str,
    pub dir: &'a str,
    pub bin: &'a str,
    pub manifest: Option<&'a str>,
}

/// The `makepad` tool that runs apps from source (`makepad run`, tools/run).
pub const MAKEPAD_RUN: AppTarget<'static> =
    AppTarget { id: "makepad-run", package: "makepad-run", dir: "tools/run", bin: "makepad", manifest: None };

/// Every app: the menu rows first, in menu order (table.rs enforces it),
/// then the hidden ones.
pub fn apps() -> &'static [AppEntry] {
    table::apps()
}

/// The app with registry id `id`.
pub fn find(id: &str) -> Option<&'static AppEntry> {
    table::apps().iter().find(|entry| entry.id == id)
}

/// The app whose binary is `bin` (file associations name binaries).
pub fn find_by_bin(bin: &str) -> Option<&'static AppEntry> {
    table::apps().iter().find(|entry| entry.bin == bin)
}

// ----------------------------------------------------------------------
// The checkout
// ----------------------------------------------------------------------

/// The checkout root: `MAKEPAD_WM_ROOT`, else the sources of the Builder
/// installation this executable belongs to, else the checkout above the
/// running exe (`target/<profile>/<bin>`), else the checkout at or above the
/// current directory — started from the repo root with its target dir
/// elsewhere (CARGO_TARGET_DIR) is still running out of a checkout. None:
/// an installed layout without sources.
pub fn repo_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("MAKEPAD_WM_ROOT") {
        return Some(PathBuf::from(root));
    }
    // A binary the Makepad Builder published: its sources are in the
    // Builder's folder, whether the Builder started it (in them) or it was
    // opened on its own (wm.exe, the Dock, the wm command).
    if let Some(install) = builder_install() {
        return install.checkout();
    }
    let from_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .and_then(|dir| checkout_at_or_above(&dir));
    from_exe.or_else(|| std::env::current_dir().ok().and_then(|cwd| checkout_at_or_above(&cwd)))
}

/// The Makepad Builder installation a published binary runs from: `home` is
/// the folder people see (makepad-builder.exe or the `makepad` command, and
/// the apps), `state` the Builder's own `builder/` folder in it (sources,
/// toolchains, the Unix `<app>.bin`s). Found from the executable: Windows
/// `home/makepad-app-wm.exe`, Unix `builder/makepad-app-wm.bin`, a macOS bundle's `installation`
/// link. Apps build through the Builder there (`build_argv`), so they get its
/// compiler, flags, features and target however the reader was started.
pub struct BuilderInstall {
    pub home: PathBuf,
    pub state: PathBuf,
}

impl BuilderInstall {
    /// The Builder's command that builds one app offline.
    pub fn command(&self) -> Option<PathBuf> {
        let command = if cfg!(windows) { self.home.join("makepad-builder.exe") } else { self.home.join("makepad-builder.sh") };
        command.is_file().then_some(command)
    }
    /// Where the Builder publishes `bin`.
    pub fn published(&self, bin: &str) -> PathBuf {
        if cfg!(windows) {
            self.home.join(format!("{bin}.exe"))
        } else {
            self.state.join(format!("{bin}.bin"))
        }
    }
    /// The Makepad source to build from: the snapshot the working directory
    /// is in (the Builder starts its apps there), else the newest one with
    /// the WM in it.
    pub fn checkout(&self) -> Option<PathBuf> {
        let sources = self.state.join("sources");
        if let Some(cwd) = std::env::current_dir().ok().and_then(|cwd| checkout_at_or_above(&cwd)) {
            if cwd.starts_with(&sources) {
                return Some(cwd);
            }
        }
        std::fs::read_dir(&sources)
            .ok()?
            .flatten()
            .map(|snapshot| snapshot.path().join("makepad"))
            .filter(|root| root.join("apps/wm/Cargo.toml").is_file())
            .max_by_key(|root| std::fs::metadata(root.join("Cargo.toml")).and_then(|m| m.modified()).ok())
    }
}

/// The Builder installation the running executable belongs to, if any.
pub fn builder_install() -> Option<BuilderInstall> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for state in [dir.join("builder"), dir.to_path_buf(), dir.join("installation")] {
        // Its downloaded sources and its compiler (the email record is
        // missing in a folder nobody logged into).
        if state.join("sources").is_dir() && state.join("toolchain").is_dir() {
            let state = state.canonicalize().unwrap_or(state);
            // An installation from before the builder/ folder keeps
            // everything in the folder itself.
            let home = if state.file_name().is_some_and(|name| name == "builder") {
                state.parent()?.to_path_buf()
            } else {
                state.clone()
            };
            return Some(BuilderInstall { home, state });
        }
    }
    None
}

/// The nearest directory at or above `start` that is a makepad checkout: the
/// workspace `Cargo.toml` with the WM's crate in it. Every ancestor is
/// checked (crates sit as deep as apps/commercial/stage/libs/app_run). A
/// developer's clone and the source a Makepad Builder downloaded (which
/// starts its apps in it, with its compiler environment) both are; the
/// Builder's has no `local/`.
pub fn checkout_at_or_above(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| dir.join("Cargo.toml").exists() && dir.join("apps/wm/Cargo.toml").exists()).map(Path::to_path_buf)
}

/// The manifest that holds `app`'s package, relative to the checkout root.
fn package_manifest(app: &AppTarget) -> String {
    app.manifest.map(str::to_string).unwrap_or_else(|| format!("{}/Cargo.toml", app.dir))
}

/// Its crate is in the checkout at `root`.
pub fn has_sources(app: &AppTarget, root: &Path) -> bool {
    root.join(package_manifest(app)).exists()
}

/// The release binary a checkout's cargo builds for `app`: under
/// `CARGO_TARGET_DIR` when set (the Makepad Builder points it at its one
/// shared target), else the workspace's own `target/`.
pub fn checkout_binary(app: &AppTarget, root: &Path, target_dir: Option<&OsStr>) -> PathBuf {
    let workspace = app
        .manifest
        .and_then(|manifest| root.join(manifest).parent().map(Path::to_path_buf))
        .unwrap_or_else(|| root.to_path_buf());
    // A relative target dir is cargo's, relative to where it runs: here.
    let target = target_dir.map(|dir| workspace.join(dir)).unwrap_or_else(|| workspace.join("target"));
    let mut path = target.join("release").join(app.bin);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}

/// `bin` beside the running executable (`.exe` on Windows, where a bare
/// name never exists): an installed layout's apps.
pub fn installed_binary(bin: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut path = exe.parent()?.join(bin);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path.exists().then_some(path)
}

/// The binary that starts `app` without compiling anything: the Builder's
/// published one in a Builder installation, else a sibling of the running
/// executable, else the checkout's release build (`root`). None: not built.
pub fn built_binary(app: &AppTarget, root: Option<&Path>) -> Option<PathBuf> {
    if let Some(install) = builder_install() {
        return Some(install.published(app.bin)).filter(|path| path.is_file());
    }
    // Beside the running executable first: whatever target directory this
    // build used, its sibling binaries are right there.
    if let Some(path) = installed_binary(app.bin) {
        return Some(path);
    }
    let root = root?;
    let target_dir = std::env::var_os("CARGO_TARGET_DIR");
    Some(checkout_binary(app, root, target_dir.as_deref())).filter(|path| path.is_file())
}

/// The cargo to launch with: the one running us, else the rustup default
/// (a Dock/GUI-launched process has no `~/.cargo/bin` on PATH), else PATH's.
pub fn cargo_bin() -> PathBuf {
    if let Ok(cargo) = std::env::var("CARGO") {
        return PathBuf::from(cargo);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let rustup = PathBuf::from(home).join(".cargo/bin/cargo");
        if rustup.exists() {
            return rustup;
        }
    }
    PathBuf::from("cargo")
}

/// The build of `app` in the checkout at `root`: `cargo build --release`
/// of exactly its package and binary, with the package's default features
/// (children are ALWAYS release builds, never debug). `--manifest-path`
/// keeps it independent of the cwd. In a Builder installation the Builder
/// builds it — the same command, flags, features and target as its own
/// builds, so nothing it compiled compiles again (`makepad-builder build
/// APP`, Unix `makepad-builder.sh build APP`).
pub fn build_argv(app: &AppTarget, root: &Path) -> (PathBuf, Vec<String>) {
    if let Some(command) = builder_install().and_then(|install| install.command()) {
        return (command, vec!["build".to_string(), app.id.to_string()]);
    }
    cargo_argv(app, root)
}

/// `cargo run` of `app` from the checkout `root` with `app_args` after `--`: cargo
/// recompiles what changed (nothing, when fresh) and then starts the app in its place
/// (Unix: the same process). How a checkout launches a hosted app: it never starts a
/// binary older than its sources. Release, always.
pub fn run_argv(app: &AppTarget, root: &Path, app_args: &[String]) -> (PathBuf, Vec<String>) {
    let (program, mut args) = cargo_argv(app, root);
    args[0] = "run".to_string();
    args.push("--".to_string());
    args.extend(app_args.iter().cloned());
    (program, args)
}

/// Variables that change what cargo compiles (the profile, rustc flags, the
/// build options platform/build.rs reads, values `option_env!` bakes into a
/// crate) and that a launcher's environment may carry from wherever it was
/// started (a lane shell's CARGO_INCREMENTAL=0, a packaging tool's bundle
/// name). [`cargo_env`] clears them so every launcher builds a package with
/// the user's plain `cargo run --release -p <pkg>` fingerprint: the WM, Code,
/// DECK, the deploy script and a terminal share one set of artifacts and
/// never recompile each other's.
pub const CARGO_SPLIT_ENV: &[&str] = &[
    "CARGO_INCREMENTAL",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_RUSTFLAGS",
    "RUSTDOCFLAGS",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_TARGET",
    "MAKEPAD_BUILD",
    "MAKEPAD_PACKAGE_DIR",
    "MAKEPAD_BUNDLE_NAME",
    "MAKEPAD_BUNDLE_IDENTIFIER",
    "MAKEPAD_APP_ICON_32",
    "MAKEPAD_APP_ICON_64",
    "MAKEPAD_APP_ICON_128",
    "MAKEPAD_APP_ICON_256",
    "MAKEPAD_APP_ICON_512",
    "MAKEPAD_APP_ICON_1024",
];

/// The environment of every cargo a launcher starts: [`CARGO_SPLIT_ENV`] and
/// any `CARGO_PROFILE_*` override cleared (the workspace's Cargo.toml profile
/// decides) and cargo's progress as plain lines the hosts parse
/// (cargo_progress). Leaves `CARGO_TARGET_DIR` (where the binary is, see
/// `checkout_binary`) and the host's own scrub (child_env) to the host.
pub fn cargo_env(command: &mut Command) {
    for key in CARGO_SPLIT_ENV {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|key| key.starts_with("CARGO_PROFILE_")) {
            command.env_remove(key);
        }
    }
    command
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TERM_PROGRESS_WHEN", "always")
        .env("CARGO_TERM_PROGRESS_WIDTH", "100")
        .env("MAKEPAD_BUILD_PROGRESS_LINES", "1");
}

/// `program args` (a [`build_argv`] / [`run_argv`]) as a command in `root`
/// with [`cargo_env`]: how every launcher starts cargo.
pub fn cargo_command(program: &Path, args: &[String], root: &Path) -> Command {
    let mut command = Command::new(program);
    command.args(args).current_dir(root);
    cargo_env(&mut command);
    command
}

fn cargo_argv(app: &AppTarget, root: &Path) -> (PathBuf, Vec<String>) {
    let manifest = root.join(app.manifest.unwrap_or("Cargo.toml"));
    let manifest = manifest.to_string_lossy();
    let args = ["build", "--release", "--manifest-path", &manifest, "-p", app.package, "--bin", app.bin];
    (cargo_bin(), args.iter().map(|arg| arg.to_string()).collect())
}

// ----------------------------------------------------------------------
// Per entry, against the current checkout
// ----------------------------------------------------------------------

/// Where its binary is or will be: the Builder's published path, the
/// checkout's release path, or an installed sibling (None: none there).
pub fn binary_path(entry: &AppEntry) -> Option<PathBuf> {
    if let Some(install) = builder_install() {
        return Some(install.published(entry.bin));
    }
    match repo_root() {
        Some(root) => Some(checkout_binary(&entry.target(), &root, std::env::var_os("CARGO_TARGET_DIR").as_deref())),
        None => installed_binary(entry.bin),
    }
}

/// Its binary exists: starting it compiles nothing.
pub fn is_built(entry: &AppEntry) -> bool {
    built_binary(&entry.target(), repo_root().as_deref()).is_some()
}

/// It can be started now: out of a checkout its crate is there (opening it
/// builds it when needed), installed its binary is.
pub fn is_available(entry: &AppEntry) -> bool {
    match repo_root() {
        Some(root) => has_sources(&entry.target(), &root),
        None => installed_binary(entry.bin).is_some(),
    }
}

/// The command that builds `entry`: `build_argv` in the checkout root with
/// the launchers' cargo environment (`cargo_command`), so a build from a tool
/// (Stage's app runs) leaves what the WM, Code and DECK find fresh. None
/// outside a checkout.
pub fn build_command(entry: &AppEntry) -> Option<Command> {
    let root = repo_root()?;
    let (program, args) = build_argv(&entry.target(), &root);
    Some(cargo_command(&program, &args, &root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_strict_json::Value;

    /// Builder ids that are deliberately not registry apps, with the reason.
    /// Every other app Builder builds is in the registry (the ones the WM
    /// does not host are `wm_launchable: false`).
    const BUILDER_ONLY: &[(&str, &str)] =
        &[("gltf", "makepad-fab-loader-gltf is Fab's loader library: it builds no binary to run")];

    /// The checkout these tests were compiled in.
    fn test_root() -> PathBuf {
        checkout_at_or_above(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("tests run in a checkout")
    }

    fn builder_apps() -> Vec<Value> {
        let bytes = std::fs::read(test_root().join("tools/makepad_builder/apps.json")).expect("Builder's apps.json");
        match makepad_strict_json::parse(&bytes).expect("Builder's apps.json parses") {
            Value::Arr(rows) => rows,
            _ => panic!("Builder's apps.json is not an array"),
        }
    }

    fn field<'a>(row: &'a Value, key: &str) -> &'a str {
        row.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("Builder row without {key}: {row:?}"))
    }

    #[test]
    fn every_builder_app_is_in_the_registry_and_agrees_with_it() {
        for row in builder_apps() {
            let id = field(&row, "id");
            let Some(entry) = find(id) else {
                assert!(
                    BUILDER_ONLY.iter().any(|(only, _)| *only == id),
                    "Builder's {id} is neither in the registry nor in BUILDER_ONLY"
                );
                continue;
            };
            assert_eq!(entry.package, field(&row, "package"), "{id}: package");
            assert_eq!(entry.bin, field(&row, "binary"), "{id}: binary");
            assert_eq!(field(&row, "workspace"), format!("makepad/{}", entry.dir), "{id}: dir");
            let features: Vec<&str> = match row.get("features") {
                Some(Value::Arr(items)) => items.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            assert_eq!(
                entry.features,
                features.as_slice(),
                "{id}: features differ from Builder's (order included: a drift alarm)"
            );
        }
        for (id, reason) in BUILDER_ONLY {
            assert!(!reason.is_empty(), "{id}: BUILDER_ONLY needs a reason");
            assert!(find(id).is_none(), "{id} is in the registry, drop it from BUILDER_ONLY");
        }
    }

    /// Where a manifest's tables start: `(header, lines until the next)`.
    fn tables(manifest: &str) -> Vec<(&str, Vec<&str>)> {
        let mut out: Vec<(&str, Vec<&str>)> = vec![("", Vec::new())];
        for line in manifest.lines().map(str::trim) {
            if line.starts_with('[') {
                out.push((line, Vec::new()));
            } else {
                out.last_mut().unwrap().1.push(line);
            }
        }
        out
    }

    /// `key = "value"` in a table's lines.
    fn value<'a>(lines: &[&'a str], key: &str) -> Option<&'a str> {
        lines.iter().find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == key).then(|| v.trim().trim_matches('"'))
        })
    }

    #[test]
    fn every_app_names_a_real_package_and_binary() {
        let root = test_root();
        for entry in apps() {
            let target = entry.target();
            let manifest = root.join(package_manifest(&target));
            if !manifest.is_file() {
                // The optional commercial checkout (scope, stage) and absent apps.
                assert!(!has_sources(&target, &root), "{} claims to be available", entry.id);
                continue;
            }
            assert!(has_sources(&target, &root), "{} should be available", entry.id);
            let text = std::fs::read_to_string(&manifest).unwrap();
            let tables = tables(&text);
            let package = tables.iter().find(|(h, _)| *h == "[package]").and_then(|(_, l)| value(l, "name"));
            assert_eq!(package, Some(entry.package), "{}: wrong package", entry.id);
            // A named [[bin]], an auto-discovered src/bin/<bin>.rs, or the
            // package's own src/main.rs when no [[bin]] claims that path.
            let bins: Vec<&Vec<&str>> = tables.iter().filter(|(h, _)| *h == "[[bin]]").map(|(_, l)| l).collect();
            let dir = manifest.parent().unwrap();
            let main_claimed = bins.iter().any(|l| value(l, "path") == Some("src/main.rs"));
            let builds = bins.iter().any(|l| value(l, "name") == Some(entry.bin))
                || dir.join("src/bin").join(format!("{}.rs", entry.bin)).is_file()
                || (entry.bin == entry.package && dir.join("src/main.rs").is_file() && !main_claimed);
            assert!(builds, "{}: {} builds no binary {}", entry.id, entry.package, entry.bin);
        }
    }

    #[test]
    fn ids_bins_and_menu_rows_are_consistent() {
        let mut ids: Vec<&str> = apps().iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), apps().len(), "duplicate ids");
        let first_hidden = apps().iter().position(|e| !e.menu_visible).unwrap_or(apps().len());
        assert!(apps()[first_hidden..].iter().all(|e| !e.menu_visible), "a menu row after a hidden app");
        assert_eq!(find("terminal").unwrap().policy, LaunchPolicy::AlwaysNew);
        assert_eq!(find_by_bin("makepad-app-mixer").unwrap().id, "mixer");
        assert!(find("nonesuch").is_none() && find_by_bin("nonesuch").is_none());
        let stage = find("stage").unwrap();
        assert!(stage.menu_visible && stage.wm_launchable);
        let amp = find("amp").unwrap();
        assert!(amp.menu_visible && amp.wm_launchable);
        assert_eq!(find_by_bin("makepad-app-amp").unwrap().id, "amp");
        // The commercial repository's apps sit in its checkout inside Makepad.
        for id in ["scope", "stage", "sandbox"] {
            assert!(find(id).unwrap().target().dir.starts_with("apps/commercial/"), "{id}");
        }
    }

    #[test]
    fn an_absent_private_checkout_is_not_available() {
        let root = std::env::temp_dir().join(format!("app-registry-absent-{}", std::process::id()));
        assert!(!has_sources(&find("stage").unwrap().target(), &root));
        assert!(!has_sources(&find("scope").unwrap().target(), &root));
    }

    #[test]
    fn a_cargo_build_is_a_release_build_of_exactly_the_binary() {
        let root = PathBuf::from("/checkout");
        let (program, args) = cargo_argv(&find("route").unwrap().target(), &root);
        assert!(program.to_string_lossy().ends_with("cargo"), "{program:?}");
        let manifest = root.join("Cargo.toml").to_string_lossy().into_owned();
        assert_eq!(
            args,
            ["build", "--release", "--manifest-path", manifest.as_str(), "-p", "makepad-app-route", "--bin", "makepad-app-route"]
        );
        let own = AppTarget { manifest: Some("apps/x/Cargo.toml"), ..find("notes").unwrap().target() };
        let (_, args) = cargo_argv(&own, &root);
        assert_eq!(args[3], root.join("apps/x/Cargo.toml").to_string_lossy());
    }

    /// Every launcher's cargo builds with the workspace profile and no
    /// inherited flags: a lane's CARGO_INCREMENTAL=0, RUSTFLAGS, a profile
    /// override or a packaging tool's MAKEPAD_BUILD would each give the same
    /// package another fingerprint and recompile it for the next launcher.
    #[test]
    fn every_launcher_builds_with_the_same_cargo_environment() {
        let root = Path::new("/checkout");
        let (program, args) = build_argv(&MAKEPAD_RUN, root);
        let command = cargo_command(&program, &args, root);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned())))
            .collect();
        for key in CARGO_SPLIT_ENV {
            assert!(envs.contains(&(key.to_string(), None)), "{key} passes to cargo");
        }
        assert!(!envs.iter().any(|(k, _)| k == "MAKEPAD"), "the runtime MAKEPAD is not a build input");
        assert!(!envs.iter().any(|(k, _)| k == "CARGO_TARGET_DIR"), "the target dir stays the launcher's");
        assert_eq!(command.get_current_dir(), Some(root));
        let args: Vec<String> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(&args[..2], ["build", "--release"]);
    }

    /// From a checkout an app starts as `cargo run --release` of exactly its binary, the
    /// app's own arguments after `--`: cargo compiles what changed, then runs it.
    #[test]
    fn a_checkout_launch_is_a_release_cargo_run_with_the_apps_arguments() {
        let root = PathBuf::from("/checkout");
        let app_args = vec!["--stdin-loop".to_string(), "--demo".to_string()];
        let (program, args) = run_argv(&find("route").unwrap().target(), &root, &app_args);
        assert!(program.to_string_lossy().ends_with("cargo"), "{program:?}");
        let manifest = root.join("Cargo.toml").to_string_lossy().into_owned();
        assert_eq!(
            args,
            ["run", "--release", "--manifest-path", manifest.as_str(), "-p", "makepad-app-route", "--bin", "makepad-app-route", "--", "--stdin-loop", "--demo"]
        );
    }

    #[test]
    fn binary_paths_follow_the_workspace_and_target_dir() {
        let root = Path::new("/checkout");
        let notes = find("notes").unwrap().target();
        let exe = |p: &str| if cfg!(windows) { format!("{p}.exe") } else { p.to_string() };
        assert_eq!(checkout_binary(&notes, root, None), PathBuf::from(exe("/checkout/target/release/makepad-app-notes")));
        assert_eq!(checkout_binary(&notes, root, Some("/t".as_ref())), PathBuf::from(exe("/t/release/makepad-app-notes")));
        let own = AppTarget { manifest: Some("apps/x/Cargo.toml"), ..notes };
        assert_eq!(checkout_binary(&own, root, None), PathBuf::from(exe("/checkout/apps/x/target/release/makepad-app-notes")));
    }
}
