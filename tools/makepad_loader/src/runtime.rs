//! Private toolchain paths shared by the compiler and the embedded terminal.
use crate::{
    catalog::{self, Release},
    progress,
};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Debug)]
pub enum Dependency {
    Rust,
    Msvc,
    Cuda,
    System,
}

pub fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.into()
    }
}
pub fn rust_dir(root: &Path, version: &str) -> PathBuf {
    root.join("toolchain/rust")
        .join(format!("{version}-{}", catalog::platform()))
}

/// Same checks as the auditable Unix bootstrap; never install during a probe.
pub fn system_tools_ready() -> Result<(), String> {
    if cfg!(windows) { return Ok(()); }
    let output = Command::new("sh")
        .args(["-c", include_str!("../check-tools.sh"), "makepad-tools", "--check"])
        .output().map_err(|e| format!("Check system tools: {e}"))?;
    if !output.status.success() {
        return Err("System developer tools are not ready. Choose Download compiler to finish platform setup.".into());
    }
    Ok(())
}

pub fn setup_system_tools() -> Result<(), String> {
    if cfg!(windows) { return Ok(()); }
    let status = Command::new("sh")
        .args(["-c", include_str!("../check-tools.sh"), "makepad-tools"])
        .status().map_err(|e| format!("Set up system tools: {e}"))?;
    if !status.success() { return Err("System tool setup did not complete".into()); }
    system_tools_ready()
}

pub fn dependency(root: &Path, release: &Release, kind: Dependency) -> Result<String, String> {
    let cache = root.join("cache");
    let tools = root.join("toolchain");
    match kind {
        Dependency::Rust => {
            crate::rustc::install_version(&cache, &rust_dir(root, &release.rust), &release.rust)?
        }
        Dependency::Msvc => {
            if !cfg!(all(windows, target_arch = "x86_64")) {
                return Err("MSVC setup is only available on x64 Windows".into());
            }
            crate::msvc::install(&cache, &tools.join("msvc"))?;
        }
        Dependency::Cuda => {
            if !crate::cuda::supported() {
                return Err("The optional CUDA toolkit is available on x64 Windows".into());
            }
            crate::cuda::install(&cache, &tools.join("cuda"))?;
        }
        Dependency::System => system_tools_ready()?,
    }
    Ok(format!("{kind:?} ready"))
}

#[derive(Clone, Debug)]
pub struct Environment {
    pub root: PathBuf,
    pub vars: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub script: PathBuf,
    pub build: PathBuf,
}

impl Environment {
    pub fn prepare(root: &Path, release: &Release, cuda: bool) -> Result<Self, String> {
        system_tools_ready()?;
        let absolute_root = root.canonicalize().map_err(|e| e.to_string())?;
        let root = absolute_root.as_path();
        let rust = rust_dir(root, &release.rust);
        if !rust.join("bin").join(exe("cargo")).is_file() {
            return Err("Install the pinned Rust toolchain first".into());
        }
        crate::rustc::prepare_host_tools(&rust)?;
        let mut vars = BTreeMap::new();
        let build = root.join("target");
        let tmp = root.join("tmp");
        let cargo = root.join("cargo-home");
        for d in [&build, &tmp, &cargo] {
            fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut path = vec![rust.join("bin"), cargo.join("bin")];
        for (k, v) in [
            ("CARGO_HOME", cargo),
            ("CARGO_TARGET_DIR", build.clone()),
            ("RUSTC", rust.join("bin").join(exe("rustc"))),
            ("RUSTDOC", rust.join("bin").join(exe("rustdoc"))),
            ("CARGO", rust.join("bin").join(exe("cargo"))),
            ("TEMP", tmp.clone()),
            ("TMP", tmp.clone()),
            ("TMPDIR", tmp),
        ] {
            vars.insert(k.into(), v.to_string_lossy().into_owned());
        }
        vars.insert("MAKEPAD_LOADER_EMAIL".into(), String::new());
        vars.insert("RUSTUP_TOOLCHAIN".into(), String::new());
        vars.insert(
            "RUSTUP_HOME".into(),
            root.join("rustup-home").to_string_lossy().into_owned(),
        );
        vars.insert("CARGO_TERM_PROGRESS_WHEN".into(), "always".into());
        vars.insert("CARGO_TERM_PROGRESS_WIDTH".into(), "100".into());
        vars.insert("CARGO_TERM_COLOR".into(), "always".into());
        vars.insert("MAKEPAD_PACKAGE_DIR".into(), ".".into());
        vars.insert("MAKEPAD_BUILDER_APP".into(), release.id.clone());
        vars.insert("CUDA_PATH".into(), String::new());
        vars.insert("CUDA_HOME".into(), String::new());
        vars.insert("CUDACXX".into(), String::new());
        vars.insert(
            "MAKEPAD_GGML_NO_CUDA".into(),
            if cuda { "0" } else { "1" }.into(),
        );
        vars.insert(
            "MAKEPAD_GGML_REQUIRE_CUDA".into(),
            if cuda { "1" } else { "0" }.into(),
        );
        if cfg!(windows) {
            let linker = rust.join("lib/rustlib/x86_64-pc-windows-msvc/bin/rust-lld.exe");
            if !linker.is_file() {
                return Err("The private Rust toolchain is missing its bundled LLD linker".into());
            }
            vars.insert("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER".into(), linker.to_string_lossy().into_owned());
            vars.insert("RUSTFLAGS".into(), "-C linker-flavor=lld-link -C target-feature=+crt-static".into());
            let msvc = root.join("toolchain/msvc");
            let bin = crate::msvc::msvc_bin_dir(&msvc)
                .ok_or("Install Microsoft Build Tools + Windows SDK first")?;
            vars.insert("NVCC_CCBIN".into(), bin.to_string_lossy().into_owned());
            path.push(bin);
            let mut includes = Vec::new();
            let mut libs = Vec::new();
            let tools =
                crate::msvc::msvc_root_tools(&msvc).ok_or("Incomplete MSVC installation")?;
            includes.push(tools.join("include"));
            libs.push(tools.join("lib/x64"));
            let kits = msvc.join("Windows Kits/10");
            let latest = |p: &Path| -> Result<PathBuf, String> {
                let mut entries: Vec<_> = fs::read_dir(p)
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect();
                entries.sort();
                entries.pop().ok_or_else(|| "Missing Windows SDK".into())
            };
            let inc = latest(&kits.join("Include"))?;
            let lib = latest(&kits.join("Lib"))?;
            for p in ["ucrt", "shared", "um", "winrt"] {
                includes.push(inc.join(p));
            }
            for p in ["ucrt/x64", "um/x64"] {
                libs.push(lib.join(p));
            }
            vars.insert("INCLUDE".into(), join_paths(&includes)?);
            vars.insert("LIB".into(), join_paths(&libs)?);
            let system = env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            path.push(system.join("System32"));
            path.push(system.clone());
            vars.insert("SystemRoot".into(), system.to_string_lossy().into_owned());
        } else {
            path.extend([
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                PathBuf::from("/usr/sbin"),
                PathBuf::from("/sbin"),
            ]);
        }
        if cuda {
            let cuda = root.join("toolchain/cuda");
            if !crate::cuda::supported() || !cuda.join("bin").join(exe("nvcc")).is_file() {
                return Err("Install optional CUDA first, or turn it off".into());
            }
            path.insert(1, cuda.join("bin"));
            vars.insert("CUDA_PATH".into(), cuda.to_string_lossy().into_owned());
        }
        // Private compilers take precedence; installed shells and agents
        // remain reachable without changing the parent or global PATH.
        if let Some(inherited) = env::var_os("PATH") {
            path.extend(env::split_paths(&inherited));
        }
        vars.insert("PATH".into(), join_paths(&path)?);
        let cwd = release.source(root);
        let script = release.directory(root).join(if cfg!(windows) {
            "environment.bat"
        } else {
            "environment.sh"
        });
        let out = Self {
            root: root.to_path_buf(),
            vars,
            cwd,
            script,
            build,
        };
        out.write()?;
        Ok(out)
    }
    fn write(&self) -> Result<(), String> {
        let mut s = if cfg!(windows) {
            "@echo off\r\n".to_string()
        } else {
            "# Private Makepad toolchain; source in this shell only.\n".to_string()
        };
        for (key, value) in &self.vars {
            if cfg!(windows) {
                s += &format!("set \"{key}={}\"\r\n", batch(value)?);
            } else {
                s += &format!("export {key}={}\n", shell(value));
            }
        }
        if cfg!(windows) {
            s += &format!("cd /d \"{}\"\r\n", batch(&self.cwd.to_string_lossy())?);
        } else {
            s += &format!("cd {}\n", shell(&self.cwd.to_string_lossy()));
        }
        fs::write(&self.script, s).map_err(|e| e.to_string())
    }
    pub fn terminal_command(&self) -> String {
        if cfg!(windows) {
            format!("cmd /d /v:off /k call \"{}\"", self.script.display())
        } else {
            format!(
                "/bin/bash --noprofile --rcfile {} -i",
                shell(&self.script.to_string_lossy())
            )
        }
    }
    pub fn build_command(&self, release: &Release, remote: bool) -> String {
        let binary = self.app_binary(release);
        let command = format!(
            "cargo build --release -p {} --bin {}{}",
            release.package, release.binary, if release.features.is_empty() { String::new() } else { format!(" --features {}", release.features.join(",")) }
        );
        if cfg!(windows) {
            format!(
                "cd /d \"{}\" && {command} && \"{}\"{}",
                self.cwd.display(),
                binary.display(),
                if remote { " --remote" } else { "" }
            )
        } else {
            format!(
                "cd {} && {command} && {}{}",
                shell(&self.cwd.to_string_lossy()),
                shell(&binary.to_string_lossy()),
                if remote { " --remote" } else { "" }
            )
        }
    }
    pub fn build(&self, release: &Release) -> Result<(), String> {
        let cargo = self.vars.get("CARGO").ok_or("Missing Cargo path")?;
        let mut cmd = Command::new(cargo);
        cmd.current_dir(&self.cwd)
            .envs(&self.vars)
            .env_remove("MAKEPAD_LOADER_EMAIL")
            .env_remove("RUSTUP_TOOLCHAIN")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env("MAKEPAD_PACKAGE_DIR", ".")
            .args([
                "build",
                "--release",
                "--message-format=json-render-diagnostics",
                "-p",
                &release.package,
                "--bin",
                &release.binary,
            ]);
        if self.cwd.join("Cargo.lock").is_file() { cmd.arg("--locked"); }
        if !release.features.is_empty() { cmd.args(["--features", &release.features.join(",")]); }
        let status = run_build_logged(&mut cmd, &self.build.join("builder-build.log"))?;
        if !status.success() {
            return Err(format!("Build failed: {status}"));
        }
        self.publish_app(release)?;
        Ok(())
    }

    pub fn app_binary(&self, release: &Release) -> PathBuf {
        self.root.join(if cfg!(windows) { exe(&release.binary) } else { format!("{}.bin", release.binary) })
    }

    fn publish_app(&self, release: &Release) -> Result<(), String> {
        use makepad_strict_json::{self as json, Value};
        progress::stage("Finishing", "Linking resources to the downloaded source", 0.0);
        use std::io::BufRead;
        let root = self.root.canonicalize().map_err(|e| e.to_string())?;
        let sources = release.directory(&root);
        let log = fs::File::open(self.build.join("builder-build.log")).map_err(|e| e.to_string())?;
        let mut paths = BTreeMap::new();
        for line in std::io::BufReader::new(log).lines() {
            let line = line.map_err(|e| e.to_string())?;
            let Ok(artifact) = json::parse(line.as_bytes()) else { continue };
            if artifact.get("reason").and_then(Value::as_str) != Some("compiler-artifact") { continue; }
            let target = artifact.get("target").ok_or("Missing Cargo artifact target")?;
            if target.get("kind").and_then(Value::as_arr).is_none_or(|kinds| kinds.iter().all(|k| k.as_str() == Some("custom-build"))) { continue; }
            let name = target.get("name").and_then(Value::as_str).ok_or("Missing Cargo crate name")?.replace('-', "_");
            let manifest = artifact.get("manifest_path").and_then(Value::as_str).ok_or("Missing Cargo artifact manifest")?;
            let directory = Path::new(manifest).parent().ok_or("Missing crate directory")?.canonicalize().map_err(|e| e.to_string())?;
            if !directory.starts_with(&sources) && !directory.join("resources").is_dir() { continue; }
            let relative = directory.strip_prefix(&root).map_err(|_| format!("Resources for {name} are outside the portable installation"))?;
            let relative = relative.to_str().ok_or("Non-UTF8 resource path")?.replace('\\', "/");
            if relative.contains(['\t', '\r', '\n']) { return Err("Unsupported character in resource path".into()); }
            if let Some(previous) = paths.insert(name.clone(), relative.clone()) {
                if previous != relative { return Err(format!("Conflicting resource crates: {name}")); }
            }
        }
        if paths.is_empty() { return Err("Cargo produced no application resource paths".into()); }
        let text: String = paths.iter().map(|(name, path)| format!("{name}\t{path}\n")).collect();
        let binary = self.app_binary(release);
        let app_map = self.root.join(format!("{}.makepad-package-paths", binary.file_name().ok_or("Missing binary name")?.to_string_lossy()));
        fs::write(&app_map, &text).map_err(|e| e.to_string())?;
        let map = self.root.join("makepad-package-paths");
        // Legacy runtimes read the shared map. Keep other apps' entries when
        // publishing one app; current runtimes prefer their own map above.
        if let Ok(previous) = fs::read_to_string(&map) {
            for line in previous.lines() {
                if let Some((name, path)) = line.split_once('\t') { paths.entry(name.to_owned()).or_insert_with(|| path.to_owned()); }
            }
        }
        let text: String = paths.into_iter().map(|(name, path)| format!("{name}\t{path}\n")).collect();
        let next = map.with_extension("next");
        fs::write(&next, text).map_err(|e| e.to_string())?;
        replace_file(&next, &map)?;
        let next = binary.with_extension("next");
        fs::copy(self.build.join("release").join(exe(&release.binary)), &next).map_err(|e| e.to_string())?;
        replace_file(&next, &binary)?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            let command = self.root.join(&release.binary);
            fs::write(&command, include_str!("../launcher.sh").replace("@APP_BINARY@", &release.binary)).map_err(|e| e.to_string())?;
            fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "macos")]
        {
            let project = release.repositories.iter().find(|repo| repo.name == "makepad")
                .map(|repo| release.directory(&self.root).join(&repo.path))
                .unwrap_or_else(|| release.source(&self.root));
            crate::desktop::prepare(&self.root, release, &project)?;
        }
        progress::stage("Ready", &format!("{} is ready in the installation folder", exe(&release.binary)), 1.0);
        Ok(())
    }
}

fn replace_file(next: &Path, destination: &Path) -> Result<(), String> {
    // Windows cannot replace an existing file with std::fs::rename.
    let backup = destination.with_extension("previous");
    let existed = destination.is_file();
    if existed {
        if backup.exists() { fs::remove_file(&backup).map_err(|e| e.to_string())?; }
        fs::rename(destination, &backup).map_err(|e| e.to_string())?;
    }
    if let Err(error) = fs::rename(next, destination) {
        if existed { let _ = fs::rename(&backup, destination); }
        return Err(error.to_string());
    }
    if existed { let _ = fs::remove_file(backup); }
    Ok(())
}
/// Console applications launched for setup/build must not allocate a second
/// Windows console. Interactive agents still run through the inherited PTY.
pub fn hide_console(command: &mut Command) {
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
    #[cfg(not(windows))] let _ = command;
}
fn run_build_logged(command: &mut Command, log: &Path) -> Result<std::process::ExitStatus, String> {
    use std::{io::Read, process::Stdio, time::Duration};
    hide_console(command);
    command.env("CARGO_TERM_COLOR", "never").env("CARGO_TERM_PROGRESS_WHEN", "never");
    let output = fs::File::create(log).map_err(|e| e.to_string())?;
    let mut input = fs::File::open(log).map_err(|e| e.to_string())?;
    let child = command.stdin(Stdio::null()).stdout(output.try_clone().map_err(|e| e.to_string())?).stderr(output).spawn().map_err(|e| e.to_string())?;
    struct Running(Option<std::process::Child>);
    impl Drop for Running { fn drop(&mut self) { if let Some(child) = &mut self.0 { let _ = child.kill(); let _ = child.wait(); } } }
    let mut child = Running(Some(child));
    let mut pending = Vec::new();
    let mut last = "Starting Cargo release build".to_string();
    loop {
        let status = child.0.as_mut().unwrap().try_wait().map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 16384];
        loop {
            let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 { break; }
            pending.extend_from_slice(&buffer[..n]);
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line = String::from_utf8_lossy(&pending[..end]).trim_end().to_owned();
                pending.drain(..=end);
                // Cargo artifacts are machine data for resource publication.
                // Diagnostics and normal Cargo status remain visible.
                if makepad_strict_json::parse(line.as_bytes()).ok().is_some_and(|v| v.get("reason").is_some()) { continue; }
                last = line;
                if !progress::active() { eprintln!("{last}"); }
                progress::stage("Compiling Rust", &last, 0.0);
            }
            if pending.len() > 16384 { pending.drain(..pending.len() - 16384); }
        }
        if let Some(status) = status {
            if !pending.is_empty() { progress::stage("Compiling Rust", &String::from_utf8_lossy(&pending), 0.0); }
            child.0.take();
            if status.success() { progress::stage("Ready", "Release build complete", 1.0); }
            return Ok(status);
        }
        progress::stage("Compiling Rust", &last, 0.0);
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn join_paths(paths: &[PathBuf]) -> Result<String, String> {
    env::join_paths(paths)
        .map(|s| s.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}
pub fn shell(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
pub fn batch(s: &str) -> Result<String, String> {
    if s.contains(['"', '\r', '\n', '\0']) {
        return Err("Path cannot be represented in a Windows command script".into());
    }
    Ok(s.replace('%', "%%"))
}

pub fn prepare(
    root: &Path,
    service: &str,
    key: &str,
    release: &Release,
    cuda: bool,
) -> Result<Environment, String> {
    if cuda && !crate::cuda::supported() {
        return Err("The optional CUDA toolkit is available on x64 Windows".into());
    }
    system_tools_ready()?;
    progress::stage("Rust", &format!("Private Rust {}", release.rust), 0.05);
    dependency(root, release, Dependency::Rust)?;
    if cfg!(windows) {
        progress::stage("Microsoft", "Private Build Tools + Windows SDK", 0.35);
        dependency(root, release, Dependency::Msvc)?;
    }
    if cuda {
        progress::stage("NVIDIA", "Optional private CUDA toolkit", 0.6);
        dependency(root, release, Dependency::Cuda)?;
    }
    catalog::checkout(service, key, root, release)?;
    progress::stage("Ready", "Private toolchain and sources ready", 1.0);
    Environment::prepare(root, release, cuda)
}

/// Rebuild the edited, already installed sources without refreshing or downloading.
pub fn rebuild_local() -> Result<(), String> {
    let root = crate::default_root();
    let selected = env::var("MAKEPAD_BUILDER_APP").ok().or_else(|| fs::read_to_string(root.join("selected-app")).ok()).filter(|s| catalog::identifier(s));
    let installed = selected.as_ref().map(|s| root.join("installed").join(format!("{s}.json")));
    let release_path = installed.filter(|p| p.is_file()).unwrap_or_else(|| root.join("latest.json"));
    let release = Release::parse(&makepad_strict_json::parse(&fs::read(release_path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?)?;
    if !release.installed(&root) { return Err("Download the app sources first".into()); }
    let cuda = fs::read_to_string(root.join("installed-cuda")).unwrap_or_default() == "1";
    let environment = Environment::prepare(&root, &release, cuda)?;
    environment.build(&release)?;
    release.save(&root.join("installed-release.json"))?;
    println!("Ready: {}", environment.app_binary(&release).display());
    Ok(())
}

pub fn cli_main() -> Result<(), String> {
    let bootstrap = makepad_loader_bundle::load()?.map(|(_, config)| config);
    let mut app = bootstrap.as_ref().map(|config| config.app.clone());
    let mut address = env::var("MAKEPAD_LOADER_EMAIL").ok()
        .or_else(|| bootstrap.map(|config| config.email));
    let mut root = crate::default_root();
    let mut service =
        env::var("MAKEPAD_LOADER_SERVICE").unwrap_or_else(|_| catalog::DEFAULT_SERVICE.into());
    let mut sources = false;
    let mut setup = false;
    let mut build = false;
    let mut run = false;
    let mut cuda = false;
    let mut accept_rust = false;
    let mut accept_ms = false;
    let mut accept_nv = false;
    let mut accept_apple = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--app" => app = Some(args.next().ok_or("--app needs an app ID")?),
            "--email" => address = Some(args.next().ok_or("--email needs an address")?),
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a folder")?),
            "--service" => service = args.next().ok_or("--service needs a URL")?,
            "--fetch-latest" => (),
            "--sources-only" => sources = true,
            "--prepare" => setup = true,
            "--build" => build = true,
            "--run" => run = true,
            "--cuda" => cuda = true,
            "--accept-rust" => accept_rust = true,
            "--accept-microsoft" => accept_ms = true,
            "--accept-nvidia" => accept_nv = true,
            "--accept-apple" => accept_apple = true,
            _ => return Err(format!("Unknown Makepad Builder option {arg}")),
        }
    }
    let app = app.ok_or("--app is required")?;
    let key = address.unwrap_or_default();
    let release = if app == "scope" {
        catalog::fetch(&service, &key)?.into_iter().find(|r| r.id == app).ok_or("App is not included in this email address")?
    } else {
        let selected = if app == "makepad" { "calculator" } else { &app };
        let definition = catalog::apps()?.into_iter().find(|a| a.get("id").and_then(makepad_strict_json::Value::as_str) == Some(selected)).ok_or("Unknown public app")?;
        catalog::fetch_public(&service)?.for_app(&definition)?
    };
    println!(
        "{} release={} rust={} platform={}",
        release.title,
        release.release,
        release.rust,
        catalog::platform()
    );
    if !release.supported() {
        return Err("Release does not support this platform".into());
    }
    if setup {
        if !accept_rust
            || cfg!(windows) && !accept_ms
            || cfg!(target_os = "macos") && !accept_apple
            || cuda && !accept_nv
        {
            return Err("Setup requires explicit --accept-rust and the platform vendor terms; optional CUDA requires --accept-nvidia".into());
        }
        prepare(&root, &service, &key, &release, cuda)?;
    } else if sources {
        catalog::checkout(&service, &key, &root, &release)?;
    }
    if build || run {
        let environment = Environment::prepare(&root, &release, cuda)?;
        environment.build(&release)?;
        if run {
            let status = Command::new(environment.app_binary(&release))
                .current_dir(&environment.cwd)
                .envs(&environment.vars)
                .env_remove("MAKEPAD_LOADER_EMAIL")
                .arg("--remote")
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("App exited {status}"));
            }
        }
    }
    Ok(())
}
