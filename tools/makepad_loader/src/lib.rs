pub mod cab;
pub mod cuda;
pub mod extract;
pub mod gitclone;
pub mod http;
pub mod lzx;
pub mod msi;
pub mod msvc;
pub mod progress;
pub mod rustc;
pub mod sha256;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;

#[derive(Clone)]
pub struct InstallOpts {
    pub root: PathBuf,
    pub git_url: String,
    pub branch: Option<String>,
    pub skip_build: bool,
    pub skip_cuda: bool,
    pub skip_git: bool,
    pub src: Option<PathBuf>,
    pub package: String,
}

impl Default for InstallOpts {
    fn default() -> Self {
        Self {
            root: default_root(),
            git_url: "https://github.com/makepad/makepad".to_string(),
            branch: Some("work".to_string()),
            skip_build: false,
            skip_cuda: true,
            skip_git: false,
            src: None,
            package: "makepad-example-counter".to_string(),
        }
    }
}

pub fn default_root() -> PathBuf {
    if let Ok(p) = env::var("MAKEPAD_LOADER_ROOT") {
        return PathBuf::from(p);
    }
    // Unzip-and-run: the folder that contains the exe (or cwd) is the
    // whole install. Cache, toolchain, tmp, and src land next to it.
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.as_os_str().len() > 0 {
                return dir.to_path_buf();
            }
        }
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[derive(Clone, Debug)]
pub struct AppInfo {
    pub package: String,
    pub title: String,
}

pub fn run_install(
    opts: &InstallOpts,
    progress: impl FnMut(progress::Progress) + 'static,
) -> Result<Vec<AppInfo>, String> {
    progress::scope(progress, || run_install_inner(opts))
}

fn run_install_inner(opts: &InstallOpts) -> Result<Vec<AppInfo>, String> {
    fs::create_dir_all(&opts.root).map_err(|e| e.to_string())?;
    let tmp = tmp_dir(&opts.root)?;
    let _ = tmp;
    let cache = opts.root.join("cache");
    let toolchain = opts.root.join("toolchain");
    let rust_dir = toolchain.join("rust");
    let msvc_dir = toolchain.join("msvc");
    let cuda_dir = toolchain.join("cuda");
    let cloned = opts.root.join("src");
    let src_dir = opts.src.clone().unwrap_or_else(|| cloned.clone());

    #[cfg(windows)]
    {
        dwell("Rust", "stable rustc / cargo / rust-std", 0.08);
        rustc::install(&cache, &rust_dir)?;
        dwell("Rust", "ready", 0.22);
        dwell("MSVC", "Build Tools + Windows SDK", 0.28);
        msvc::install(&cache, &msvc_dir)?;
        dwell("MSVC", "ready", 0.48);
        dwell("CUDA", "NVIDIA toolkit", 0.55);
        if cuda_dir.join("bin").join("nvcc.exe").is_file() {
            dwell("CUDA", "already extracted", 0.72);
        } else if !opts.skip_cuda {
            match cuda::install(&cache, &cuda_dir) {
                Ok(()) => dwell("CUDA", "ready", 0.72),
                Err(e) => {
                    dwell(
                        "CUDA",
                        &format!("redist unavailable ({e}); copying local NVIDIA runtime"),
                        0.62,
                    );
                    cuda::harvest_runtime(&cuda_dir)?;
                    dwell("CUDA", "runtime ready", 0.72);
                }
            }
        } else {
            dwell("CUDA", "copying local NVIDIA runtime", 0.62);
            cuda::harvest_runtime(&cuda_dir)?;
            dwell("CUDA", "runtime ready", 0.72);
        }
        write_env_scripts(&opts.root, &rust_dir, &msvc_dir, &cuda_dir, &src_dir)?;
    }
    #[cfg(not(windows))]
    {
        let _ = (&cache, &rust_dir, &msvc_dir, &cuda_dir);
        progress::stage("Checkout", "using local workspace", 0.4);
    }

    if !opts.skip_git {
        dwell(
            "Git",
            &format!("clone {} (depth 1, work)", opts.git_url),
            0.8,
        );
        gitclone::clone_depth1(&opts.git_url, &cloned, opts.branch.as_deref())?;
        dwell("Git", "checkout ready", 0.92);
    } else {
        dwell("Git", "using existing checkout", 0.92);
    }

    if !opts.skip_build {
        #[cfg(windows)]
        {
            progress::stage("Build", &opts.package, 0.95);
            isolated_build(opts, &rust_dir, &msvc_dir, &cuda_dir, &src_dir)?;
        }
    }

    dwell("Ready", "scanning apps", 1.0);
    list_apps(&src_dir)
}

fn dwell(stage: &str, detail: &str, frac: f32) {
    progress::stage(stage, detail, frac);
    std::thread::sleep(std::time::Duration::from_millis(450));
}

const APP_CATALOG: &[(&str, &str)] = &[
    ("makepad-vj", "VJ"),
    ("makepad-studio", "Studio"),
    ("makepad-wm", "Desktop"),
    ("makepad-files", "Files"),
    ("makepad-browser", "Browser"),
    ("makepad-terminal", "Terminal"),
    ("makepad-app-asset-ui", "Assets"),
    ("makepad-example-counter", "Counter"),
    ("makepad-example-splash", "Splash"),
    ("makepad-example-todo", "Todo"),
];

pub fn list_apps(src: &Path) -> Result<Vec<AppInfo>, String> {
    let mut apps = Vec::new();
    for &(package, title) in APP_CATALOG {
        if crate_exists(src, package) {
            apps.push(AppInfo {
                package: package.to_string(),
                title: title.to_string(),
            });
        }
    }
    if apps.is_empty() {
        return Err(format!("no catalog apps in {}", src.display()));
    }
    Ok(apps)
}

fn crate_exists(src: &Path, package: &str) -> bool {
    fn walk(dir: &Path, package: &str, depth: usize) -> bool {
        if depth > 4 {
            return false;
        }
        let Ok(rd) = fs::read_dir(dir) else {
            return false;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if walk(&p, package, depth + 1) {
                    return true;
                }
            } else if p.file_name().is_some_and(|n| n == "Cargo.toml") {
                if package_name(&p).as_deref() == Some(package) {
                    return p.parent().map(|d| d.join("src/main.rs").is_file()).unwrap_or(false);
                }
            }
        }
        false
    }
    walk(src, package, 0)
}

fn package_name(cargo: &Path) -> Option<String> {
    let text = fs::read_to_string(cargo).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name") {
            if let Some((_, val)) = rest.split_once('=') {
                return Some(val.trim().trim_matches('"').to_string());
            }
        }
        if line.starts_with('[') && line != "[package]" {
            break;
        }
    }
    None
}

pub fn env_bat_path(root: &Path) -> PathBuf {
    root.join("toolchain").join("env.bat")
}

pub fn tmp_dir(root: &Path) -> Result<PathBuf, String> {
    let tmp = root.join("tmp");
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    Ok(tmp)
}

pub fn build_dir(root: &Path) -> Result<PathBuf, String> {
    let dir = root.join("build");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(windows)]
fn write_env_scripts(
    root: &Path,
    rust_dir: &Path,
    msvc_dir: &Path,
    cuda_dir: &Path,
    src_dir: &Path,
) -> Result<(), String> {
    let mut path = vec![
        rust_dir.join("bin").display().to_string(),
        cuda_dir.join("bin").display().to_string(),
    ];
    if let Some(cl) = msvc::msvc_bin_dir(msvc_dir) {
        path.push(cl.display().to_string());
    }
    path.push(r"C:\Windows\System32".into());
    path.push(r"C:\Windows".into());
    let mut include = Vec::new();
    let mut lib = Vec::new();
    if let Some(tools) = msvc::msvc_root_tools(msvc_dir) {
        include.push(tools.join("include").display().to_string());
        lib.push(tools.join("lib/x64").display().to_string());
    }
    push_sdk_paths(msvc_dir, &mut include, &mut lib);
    let cargo_home = root.join("cargo-home");
    fs::create_dir_all(&cargo_home).map_err(|e| e.to_string())?;
    let tmp = tmp_dir(root)?;
    let build = build_dir(root)?;
    let src = if src_dir.is_dir() {
        src_dir.to_path_buf()
    } else {
        root.to_path_buf()
    };
    let bat = format!(
        "@echo off\r\n\
         set PATH={};\r\n\
         set INCLUDE={};\r\n\
         set LIB={};\r\n\
         set CARGO_HOME={}\r\n\
         set CARGO_TARGET_DIR={}\r\n\
         set RUSTC={}\r\n\
         set CARGO={}\r\n\
         set CUDA_PATH={}\r\n\
         set CARGO_TERM_PROGRESS_WHEN=always\r\n\
         set CARGO_TERM_PROGRESS_WIDTH=80\r\n\
         set CARGO_TERM_COLOR=always\r\n\
         set TEMP={}\r\n\
         set TMP={}\r\n\
         set TMPDIR={}\r\n\
         set SystemRoot=C:\\Windows\r\n\
         cd /d {}\r\n",
        path.join(";"),
        include.join(";"),
        lib.join(";"),
        cargo_home.display(),
        build.display(),
        rust_dir.join("bin").join("rustc.exe").display(),
        rust_dir.join("bin").join("cargo.exe").display(),
        cuda_dir.display(),
        tmp.display(),
        tmp.display(),
        tmp.display(),
        src.display(),
    );
    extract::write_file(&env_bat_path(root), bat.as_bytes())?;
    Ok(())
}

#[cfg(windows)]
fn isolated_build(
    opts: &InstallOpts,
    rust_dir: &Path,
    msvc_dir: &Path,
    cuda_dir: &Path,
    src_dir: &Path,
) -> Result<(), String> {
    let cargo = rust_dir.join("bin").join(exe("cargo"));
    let rustc = rust_dir.join("bin").join(exe("rustc"));
    if !cargo.is_file() {
        return Err(format!("missing {}", cargo.display()));
    }
    let cl_dir = msvc::msvc_bin_dir(msvc_dir).ok_or("cl.exe not found in unpacked msvc")?;
    let tools = msvc::msvc_root_tools(msvc_dir);
    let cargo_home = opts.root.join("cargo-home");
    fs::create_dir_all(&cargo_home).map_err(|e| e.to_string())?;

    #[allow(unused_mut)]
    let mut path_parts = vec![
        rust_dir.join("bin").display().to_string(),
        cl_dir.display().to_string(),
        cuda_dir.join("bin").display().to_string(),
    ];
    #[cfg(windows)]
    {
        path_parts.push(r"C:\Windows\System32".into());
        path_parts.push(r"C:\Windows".into());
    }

    let mut include = Vec::new();
    let mut lib = Vec::new();
    if let Some(tools) = &tools {
        include.push(tools.join("include").display().to_string());
        lib.push(tools.join("lib/x64").display().to_string());
    }
    push_sdk_paths(msvc_dir, &mut include, &mut lib);

    let path_join = if cfg!(windows) { ";" } else { ":" };
    let mut cmd = Command::new(&cargo);
    cmd.env_clear();
    cmd.env("PATH", path_parts.join(path_join));
    cmd.env("CARGO_HOME", &cargo_home);
    cmd.env("RUSTC", &rustc);
    cmd.env("CARGO", &cargo);
    cmd.env("CUDA_PATH", cuda_dir);
    cmd.env("INCLUDE", include.join(path_join));
    cmd.env("LIB", lib.join(path_join));
    if let Some(ccbin) = msvc::msvc_bin_dir(msvc_dir) {
        cmd.env("CCC_OVERRIDE_OPTIONS", "");
        cmd.env("NVCC_CCBIN", &ccbin);
    }
    #[cfg(windows)]
    {
        cmd.env("SystemRoot", r"C:\Windows");
        cmd.env("WINDIR", r"C:\Windows");
        cmd.env("COMSPEC", r"C:\Windows\System32\cmd.exe");
        let tmp = tmp_dir(&opts.root)?;
        let build = build_dir(&opts.root)?;
        cmd.env("TEMP", &tmp);
        cmd.env("TMP", &tmp);
        cmd.env("TMPDIR", &tmp);
        cmd.env("CARGO_TARGET_DIR", &build);
        if let Ok(profile) = env::var("USERPROFILE") {
            cmd.env("USERPROFILE", profile);
        }
        cmd.env("HOMEDRIVE", "C:");
    }
    cmd.arg("build")
        .arg("--release")
        .arg("-p")
        .arg(&opts.package)
        .current_dir(src_dir);
    println!("  cargo {}", cargo.display());
    println!("  PATH {}", path_parts.join(path_join));
    let st = cmd.status().map_err(|e| format!("spawn cargo: {e}"))?;
    if !st.success() {
        return Err(format!("cargo exited {st}"));
    }
    Ok(())
}

#[cfg(windows)]
fn push_sdk_paths(msvc_dir: &Path, include: &mut Vec<String>, lib: &mut Vec<String>) {
    let kits = msvc_dir.join("Windows Kits/10");
    let inc_root = kits.join("Include");
    if let Ok(rd) = fs::read_dir(&inc_root) {
        let mut vers: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        vers.sort();
        if let Some(v) = vers.last() {
            for sub in ["ucrt", "shared", "um", "winrt"] {
                include.push(v.join(sub).display().to_string());
            }
        }
    }
    let lib_root = kits.join("Lib");
    if let Ok(rd) = fs::read_dir(&lib_root) {
        let mut vers: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        vers.sort();
        if let Some(v) = vers.last() {
            lib.push(v.join("ucrt/x64").display().to_string());
            lib.push(v.join("um/x64").display().to_string());
        }
    }
}

#[cfg(windows)]
fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

pub fn cli_main() -> Result<(), String> {
    let args = parse_args()?;
    if let Some(path) = &args.dump_msi {
        let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        return msi::dump(&bytes);
    }
    if let Some(path) = &args.dump_cab {
        let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let names = cab::list(&bytes)?;
        println!("cab members={}", names.len());
        for n in &names {
            println!("  {n}");
        }
        return Ok(());
    }
    let opts = InstallOpts {
        root: args.root,
        git_url: args.git_url,
        branch: args.branch,
        skip_build: args.skip_build,
        skip_cuda: args.skip_cuda,
        skip_git: args.skip_git,
        src: args.src,
        package: args.package,
    };
    run_install(&opts, |_| {})?;
    Ok(())
}

struct CliArgs {
    root: PathBuf,
    git_url: String,
    branch: Option<String>,
    skip_build: bool,
    skip_cuda: bool,
    skip_git: bool,
    src: Option<PathBuf>,
    package: String,
    dump_msi: Option<PathBuf>,
    dump_cab: Option<PathBuf>,
}

fn parse_args() -> Result<CliArgs, String> {
    let mut root = default_root();
    let mut git_url = "https://github.com/makepad/makepad".to_string();
    let mut branch = None;
    let mut skip_build = false;
    let mut skip_cuda = true;
    let mut skip_git = false;
    let mut src = None;
    let mut package = "makepad-example-counter".to_string();
    let mut dump_msi = None;
    let mut dump_cab = None;
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => {
                root = PathBuf::from(args.next().ok_or("--root needs a path")?);
            }
            "--git" => {
                git_url = args.next().ok_or("--git needs a url")?;
            }
            "--branch" => {
                branch = Some(args.next().ok_or("--branch needs a name")?);
            }
            "--package" | "-p" => {
                package = args.next().ok_or("-p needs a package")?;
            }
            "--skip-build" => skip_build = true,
            "--skip-cuda" => skip_cuda = true,
            "--cuda" => skip_cuda = false,
            "--skip-git" => skip_git = true,
            "--src" => {
                src = Some(PathBuf::from(args.next().ok_or("--src needs a path")?));
            }
            "--dump-msi" => {
                dump_msi = Some(PathBuf::from(args.next().ok_or("--dump-msi needs a path")?));
            }
            "--dump-cab" => {
                dump_cab = Some(PathBuf::from(args.next().ok_or("--dump-cab needs a path")?));
            }
            "-h" | "--help" => {
                println!(
                    "makepad-loader-cli [--root DIR] [--git URL] [--branch NAME] [-p PKG]\n\
                     [--skip-build] [--skip-cuda|--cuda] [--skip-git]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg {other}")),
        }
    }
    Ok(CliArgs {
        root,
        git_url,
        branch,
        skip_build,
        skip_cuda,
        skip_git,
        src,
        package,
        dump_msi,
        dump_cab,
    })
}
