//! What an application asks of its toolchain folder: what is installed, which toolchain its native
//! builds use, the install, and the environment such a build runs in.
//!
//! Windows has two toolchains: Rust's GNU one, which the application already has, and Microsoft's
//! (the MSVC Rust, the Build Tools and the Windows SDK), which CUDA's compiler needs. A build with
//! MSVC and the toolkit gets the GPU kernels. On Linux an NVIDIA driver and a toolkit already on the
//! machine give a build CUDA, with nothing to choose; on macOS there is no CUDA.
use crate::{cuda, exe, jobs, msvc, plain_paths, remove_inside, remove_marker, rustc};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};

pub const MSVC_TRIPLE: &str = "x86_64-pc-windows-msvc";
/// The features an app turns local AI and its GPU kernels on with.
pub const LOCAL_AI_FEATURES: [&str; 2] = ["localai", "cuda"];
/// The licenses installing this folder's parts asks for: (id, name, URL).
pub const MICROSOFT_LICENSES: [(&str, &str, &str); 2] = [
    ("vs", "Microsoft Visual Studio Build Tools", "https://visualstudio.microsoft.com/license-terms/vs2022-ga-diagnosticbuildtools/"),
    ("sdk", "Microsoft Windows SDK", "https://learn.microsoft.com/legal/windows-sdk/windows-sdk-license"),
];
pub const NVIDIA_LICENSE: (&str, &str, &str) = ("cuda", "NVIDIA CUDA Toolkit", "https://docs.nvidia.com/cuda/eula/");
/// What the person reads before anything starts.
pub const GPU_NOTICE: &str = "Makepad relies on your GPU to draw its UI and implement AI functionality just like a videogame does. Old hardware and broken drivers can cause your computer to reboot unexpectedly.";

/// The row of the check at the end of an install, next to the components' rows.
pub const CHECK: &str = "Check";
/// Every target folder a native build can use, so what a stopped build left is found in any of them.
pub const TARGETS: [&str; 2] = ["install-target", "install-target-msvc"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chain {
    Gnu,
    Msvc,
}

/// What a finished install verified; a part it did not reach is `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Installed {
    pub build_tools: Option<String>,
    pub sdk: Option<String>,
    pub rust: Option<String>,
    pub cuda: Option<String>,
}
impl Installed {
    pub fn msvc(&self) -> bool {
        self.build_tools.is_some() && self.sdk.is_some() && self.rust.is_some()
    }
}

/// How one native build compiles.
#[derive(Clone, Debug, Default)]
pub struct Build {
    /// The Rust to compile with; `None`: the application's own.
    pub rust_bin: Option<PathBuf>,
    /// Set on the build's process, over what it inherits.
    pub vars: Vec<(String, String)>,
    /// Folders to look in first, before the process's PATH.
    pub path: Vec<PathBuf>,
    /// Added to the application's own compiler flags.
    pub rustflags: String,
    /// Toolchains do not share compiled artifacts: the name of this one's target folder.
    pub target: &'static str,
    /// The build asks for the GPU kernels: apps get `LOCAL_AI_FEATURES`.
    pub local_ai: bool,
    /// The kernels failing is an error the build recovers from (`without_cuda`).
    pub require_cuda: bool,
    /// The toolkit's runtime DLLs, which belong beside the built executable.
    pub cuda_runtime: Option<PathBuf>,
    /// Names the compiler and CUDA on or off: part of a build's identity.
    pub stamp: String,
}
impl Build {
    /// This build with the GPU kernels off: what is left when they do not compile here.
    pub fn without_cuda(&self) -> Build {
        let mut build = self.clone();
        for (key, value) in &mut build.vars {
            match key.as_str() {
                "MAKEPAD_GGML_NO_CUDA" => *value = "1".into(),
                "MAKEPAD_GGML_REQUIRE_CUDA" => *value = "0".into(),
                _ => {}
            }
        }
        build.local_ai = false;
        build.require_cuda = false;
        build.cuda_runtime = None;
        build.stamp = build.stamp.replace(" cuda", " cpu");
        build
    }
}

/// What a failed check asks to install again.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    Msvc,
    Rust,
    Cuda,
}

#[derive(Clone, Debug)]
pub struct Folder {
    root: PathBuf,
}

impl Folder {
    pub fn new(root: PathBuf) -> Folder {
        Folder { root }
    }
    fn msvc(&self) -> PathBuf {
        self.root.join("msvc")
    }
    fn cuda(&self) -> PathBuf {
        self.root.join("cuda")
    }
    fn rust(&self, version: &str) -> PathBuf {
        self.root.join("rust").join(format!("{version}-{MSVC_TRIPLE}"))
    }

    /// The toolchain native builds use. A folder nothing was chosen in compiles with GNU.
    pub fn chain(&self) -> Chain {
        match fs::read_to_string(self.root.join("selected-compiler")).unwrap_or_default().trim() {
            "msvc" => Chain::Msvc,
            _ => Chain::Gnu,
        }
    }
    pub fn set_chain(&self, chain: Chain) -> Result<(), String> {
        let text = match chain {
            Chain::Gnu => "gnu",
            Chain::Msvc => "msvc",
        };
        write_replacing(&self.root.join("selected-compiler"), text)
    }

    /// What the last finished install verified, while its parts are still there.
    pub fn installed(&self) -> Installed {
        let record = fs::read_to_string(self.root.join("verified")).unwrap_or_default();
        let field = |key: &str, there: bool| -> Option<String> {
            let value = record.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))?;
            there.then(|| value.to_owned())
        };
        let msvc = self.msvc();
        let version = field("rust", true).unwrap_or_default();
        Installed {
            build_tools: field("build-tools", msvc::build_tools_ready(&msvc)),
            sdk: field("sdk", msvc::sdk_ready(&msvc)),
            rust: field("rust", self.rust(&version).join("bin").join(exe("rustc")).is_file()),
            cuda: field("cuda", cuda::ready(&self.cuda())),
        }
    }

    /// Why the GPU kernels were given up on here, when they were: builds in this folder then stay
    /// CPU-only, so their flags and target stay the same. Installing again tries them again.
    pub fn kernels_failed(&self) -> Option<String> {
        cuda::kernels_failed(&self.root)
    }
    pub fn record_kernels_failed(&self, reason: &str) -> Result<(), String> {
        write_replacing(&self.root.join(cuda::KERNELS_FAILED_FILE), &format!("{reason}\nDelete this file to try CUDA again"))
    }

    /// The licenses accepted in this folder, by id.
    pub fn accepted(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("agreements-accepted")).unwrap_or_default().split_whitespace().map(str::to_owned).collect()
    }
    pub fn accept(&self, ids: &[&str]) -> Result<(), String> {
        let mut all = self.accepted();
        for id in ids {
            if !all.iter().any(|a| a == id) {
                all.push((*id).to_owned());
            }
        }
        write_replacing(&self.root.join("agreements-accepted"), &all.join("\n"))
    }

    /// The rows `install` will show, in order: the components it has work on, then the check.
    pub fn plan(&self, rust_version: &str, with_cuda: bool) -> Vec<&'static str> {
        let mut rows = Vec::new();
        if !msvc::ready(&self.msvc()) {
            rows.extend(["Build tools", "Windows SDK"]);
        }
        if !self.rust_stamped(rust_version) {
            rows.push("Rust");
        }
        if with_cuda && !cuda::ready(&self.cuda()) {
            rows.push("CUDA");
        }
        rows.push(CHECK);
        rows
    }
    fn rust_stamped(&self, version: &str) -> bool {
        fs::read_to_string(self.rust(version).join(".toolchain-version")).is_ok_and(|stamp| stamp == format!("{version} {MSVC_TRIPLE}"))
    }

    /// Installs what is missing and proves it works. Safe to run again at any point: a finished
    /// download is reused after its hash and an unfinished one continues; what a stopped install
    /// unpacked is removed and unpacked again, so nothing of two attempts mixes. Progress goes to the
    /// calling thread's `progress` hook.
    pub fn install(&self, rust_version: &str, with_cuda: bool, stop: Arc<AtomicBool>) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let (msvc, cuda_dir, rust) = (self.msvc(), self.cuda(), self.rust(rust_version));
        let cache = self.root.join("cache");
        // A failed attempt must never leave an earlier verification record active.
        remove_marker(&self.root.join("verified"))?;
        let plan = self.plan(rust_version, with_cuda);
        let catalog = msvc::Catalog::default();
        let mut work = Vec::new();
        if plan.contains(&"Build tools") {
            remove_inside(&self.root, &msvc)?;
            work = msvc::install_jobs(&cache, &msvc, &catalog, false);
        }
        if plan.contains(&"Rust") {
            // Rust arrives whole and with this stamp: anything else in its place is no install of it
            remove_inside(&self.root, &rust)?;
            work.push(jobs::Job::new("Rust", || rustc::install_version_for(&cache, &rust, rust_version, MSVC_TRIPLE)));
        }
        if plan.contains(&"CUDA") {
            remove_inside(&self.root, &cuda_dir)?;
            work.push(jobs::Job::new("CUDA", || cuda::install(&cache, &cuda_dir)));
        }
        let result = jobs::run(work, stop.clone());
        let stopped = || stop.load(Ordering::Relaxed);
        if stopped() {
            return Err(jobs::CANCELLED.into());
        }
        result?;
        let mut suspects: &[Part] = &[];
        let checked = self.verify(rust_version, with_cuda, &stopped, &mut suspects);
        // A stop during the last step of a passing check still stops the install.
        let verified = match checked.and_then(|record| if stopped() { Err(jobs::CANCELLED.into()) } else { Ok(record) }) {
            Ok(verified) => verified,
            Err(error) if error == jobs::CANCELLED => {
                crate::progress::deliver(jobs::finished(CHECK, jobs::STOPPED, &error));
                return Err(error);
            }
            Err(error) => {
                let error = self.clear_suspects(error, suspects, rust_version);
                crate::progress::deliver(jobs::finished(CHECK, jobs::FAILED, &error));
                return Err(format!("{CHECK}: {error}"));
            }
        };
        write_replacing(&self.root.join("verified"), &verified)?;
        crate::progress::deliver(jobs::finished(CHECK, jobs::DONE, ""));
        let _ = fs::remove_file(self.root.join(cuda::KERNELS_FAILED_FILE));
        Ok(())
    }

    /// Clears the marker of what a failed check suspects, so that it, and only it, is installed
    /// again. The check's error stays the report, with whatever could not be cleared after it.
    fn clear_suspects(&self, error: String, suspects: &[Part], rust_version: &str) -> String {
        let mut report = error;
        for part in suspects {
            let cleared = match part {
                Part::Msvc => msvc::invalidate(&self.msvc()),
                Part::Rust => remove_marker(&self.rust(rust_version).join(".toolchain-version")),
                Part::Cuda => cuda::invalidate(&self.cuda()),
            };
            if let Err(stuck) = cleared {
                report = format!("{report}\n{stuck}");
            }
        }
        report
    }

    /// Runs what was installed the way a build will, in `check/`: Rust links a program against the
    /// SDK's libraries and it runs, the C++ compiler compiles against the SDK's headers, and nvcc
    /// compiles a kernel through that compiler. Returns the record of what was proved. `suspects`
    /// says as it goes what a failure of the step at hand shows to be broken: nothing while it only
    /// prepares its own folder.
    fn verify(&self, rust_version: &str, with_cuda: bool, stopped: &dyn Fn() -> bool, suspects: &mut &'static [Part]) -> Result<String, String> {
        let dir = self.root.join("check");
        remove_inside(&self.root, &dir)?;
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let write = |name: &str, text: &str| fs::write(dir.join(name), text).map_err(|e| e.to_string());
        write("check.rs", "fn main() { println!(\"linked and running\"); }\n")?;
        write("check.c", "#include <windows.h>\n#include <stdio.h>\nint main(void) { printf(\"%lu\", GetCurrentProcessId()); return 0; }\n")?;
        write("check.cu", "__global__ void check(int* a) { a[0] = 1; }\n")?;
        *suspects = &[Part::Rust];
        let linker = self.rust_linker(rust_version)?;
        *suspects = &[Part::Msvc];
        let mut vars = self.msvc_vars(rust_version, with_cuda)?;
        vars.push(("PATH".to_owned(), with_inherited_path(&self.msvc_path(rust_version, with_cuda)?)?));
        let run = |what: &str, command: &mut Command| -> Result<String, String> {
            let output = verification_output(command.current_dir(&dir).envs(vars.iter().map(|(k, v)| (k, v))), &dir.join("output.log"), stopped).map_err(|e| if e == jobs::CANCELLED { e } else { format!("{what}: {e}") })?;
            let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            if !output.status.success() {
                let tail: Vec<&str> = text.lines().rev().take(12).collect();
                return Err(format!("{what} failed ({}):\n{}", output.status, tail.into_iter().rev().collect::<Vec<_>>().join("\n")));
            }
            Ok(text)
        };
        let steps = if with_cuda { 3 } else { 2 };
        let step = |done: usize, what: &str| check_progress(what, done, steps);

        step(0, "Rust with the Microsoft linker");
        let rustc = self.rust(rust_version).join("bin").join(exe("rustc"));
        // Compiling needs Rust alone. Linking needs the libraries of the Build Tools and the SDK as
        // well, and its failure can't tell which of them it is, so both go.
        *suspects = &[Part::Rust];
        run("Rust", Command::new(&rustc).args(["check.rs", "--edition=2021", "--emit=obj"]))?;
        *suspects = &[Part::Rust, Part::Msvc];
        run("Rust", Command::new(&rustc).args(["check.rs", "--edition=2021", "-C", "linker-flavor=lld-link", "-C", "target-feature=+crt-static"]).arg("-C").arg(format!("linker={}", plain_paths(&linker.to_string_lossy()))))?;
        let said = run("The program Rust built", &mut Command::new(dir.join(exe("check"))))?;
        if !said.contains("linked and running") {
            return Err(format!("The program Rust built said: {said}"));
        }

        step(1, "the C++ compiler with the Windows SDK");
        *suspects = &[Part::Msvc];
        let msvc = self.msvc();
        let bin = msvc::msvc_bin_dir(&msvc).ok_or("The Build Tools have no compiler")?;
        run("The C++ compiler", Command::new(bin.join("cl.exe")).args(["/nologo", "/c", "/EHsc", "/MD", "check.c"]))?;

        let mut record = format!(
            "build-tools {}\nsdk {}\nrust {rust_version}\n",
            msvc::msvc_root_tools(&msvc).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).ok_or("The Build Tools have no version")?,
            latest_dir(&msvc.join("Windows Kits/10/Include"))?.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("The Windows SDK has no version")?,
        );
        if with_cuda {
            step(2, "nvcc with the C++ compiler");
            // The C++ compiler it goes through has just been proved.
            *suspects = &[Part::Cuda];
            run("nvcc", Command::new(self.cuda().join("bin").join(exe("nvcc"))).args(["-std=c++17", "-Xcompiler", "/EHsc", "-Xcompiler", "/MD", "-c", "check.cu", "-o", "check.obj", "-ccbin"]).arg(plain_paths(&bin.to_string_lossy())))?;
            record.push_str(&format!("cuda {}\n", cuda::CUDA_VERSION));
        }
        Ok(record)
    }

    /// How a native build compiles on this machine now, with the application's Rust `rust_version`.
    pub fn build(&self, rust_version: &str) -> Result<Build, String> {
        if cfg!(windows) {
            return match self.chain() {
                Chain::Gnu => Ok(Build {
                    vars: vec![("MAKEPAD_GGML_NO_CUDA".into(), "1".into()), ("MAKEPAD_GGML_REQUIRE_CUDA".into(), "0".into())],
                    target: "install-target",
                    stamp: "gnu cpu".into(),
                    ..Build::default()
                }),
                Chain::Msvc => {
                    let installed = self.installed();
                    if !installed.msvc() {
                        return Err("Apps are set to compile with MSVC, which isn't installed. Install it in Settings, or choose GNU there.".into());
                    }
                    if installed.rust.as_deref() != Some(rust_version) {
                        return Err(format!("MSVC here has Rust {}, and apps now need Rust {rust_version}. Install it again in Settings, or choose GNU there.", installed.rust.unwrap_or_default()));
                    }
                    let kernels = installed.cuda.is_some() && cuda::gpu_present() && self.kernels_failed().is_none();
                    let rust = self.rust(rust_version);
                    Ok(Build {
                        rust_bin: Some(rust.join("bin")),
                        vars: self.msvc_vars(rust_version, kernels)?,
                        path: self.msvc_path(rust_version, kernels)?,
                        rustflags: "-C linker-flavor=lld-link -C target-feature=+crt-static".into(),
                        target: "install-target-msvc",
                        local_ai: kernels,
                        require_cuda: kernels,
                        cuda_runtime: kernels.then(|| self.cuda().join("bin").join("x64")),
                        stamp: format!("msvc {}", if kernels { "cuda" } else { "cpu" }),
                    })
                }
            };
        }
        if cfg!(target_os = "linux") {
            return Ok(match linux_cuda() {
                Some(toolkit) => {
                    let root = toolkit.root.to_string_lossy().into_owned();
                    let mut library = toolkit.lib.as_os_str().to_owned();
                    if let Some(inherited) = std::env::var_os("LD_LIBRARY_PATH").filter(|v| !v.is_empty()) {
                        library.push(":");
                        library.push(inherited);
                    }
                    Build {
                        vars: vec![
                            ("CUDA_PATH".into(), root.clone()),
                            ("CUDA_HOME".into(), root.clone()),
                            ("MAKEPAD_CUDA_ROOT".into(), root.clone()),
                            ("CUDACXX".into(), format!("{root}/bin/nvcc")),
                            ("MAKEPAD_GGML_NO_CUDA".into(), "0".into()),
                            ("MAKEPAD_GGML_REQUIRE_CUDA".into(), "0".into()),
                            ("LD_LIBRARY_PATH".into(), library.to_string_lossy().into_owned()),
                        ],
                        path: vec![toolkit.root.join("bin")],
                        target: "install-target",
                        local_ai: true,
                        stamp: "linux cuda".into(),
                        ..Build::default()
                    }
                }
                None => Build {
                    vars: vec![
                        ("CUDA_PATH".into(), String::new()),
                        ("CUDA_HOME".into(), String::new()),
                        ("CUDACXX".into(), String::new()),
                        ("MAKEPAD_CUDA_ROOT".into(), String::new()),
                        ("MAKEPAD_GGML_NO_CUDA".into(), "1".into()),
                        ("MAKEPAD_GGML_REQUIRE_CUDA".into(), "0".into()),
                    ],
                    target: "install-target",
                    stamp: "linux cpu".into(),
                    ..Build::default()
                },
            });
        }
        Ok(Build { target: "install-target", stamp: "macos".into(), ..Build::default() })
    }

    /// Where a build with Microsoft's tools and this folder's Rust looks for its programs first.
    fn msvc_path(&self, rust_version: &str, with_cuda: bool) -> Result<Vec<PathBuf>, String> {
        let msvc = self.msvc();
        let bin = msvc::msvc_bin_dir(&msvc).ok_or("Install Microsoft's Build Tools and the Windows SDK first")?;
        let system = system_root();
        let mut path = vec![self.rust(rust_version).join("bin")];
        if with_cuda {
            // CUDA 13 keeps its runtime DLLs (cudart, cuBLAS, cuBLASLt) in bin\x64: without it on
            // PATH a program stops with "cublas64_13.dll was not found".
            path.extend([self.cuda().join("bin"), self.cuda().join("bin").join("x64")]);
        }
        path.extend([bin, system.join("System32"), system]);
        Ok(path.iter().map(|p| PathBuf::from(plain_paths(&p.to_string_lossy()))).collect())
    }

    /// The environment of a build with Microsoft's tools and this folder's Rust, and the toolkit
    /// when `with_cuda`. Its paths are plain: cl.exe does not take the verbatim `\\?\` prefix (with
    /// it in INCLUDE, nvcc's host compiler cannot open crtdefs.h and every kernel fails).
    fn rust_linker(&self, rust_version: &str) -> Result<PathBuf, String> {
        let linker = self.rust(rust_version).join("lib/rustlib").join(MSVC_TRIPLE).join("bin").join(exe("rust-lld"));
        if linker.is_file() { Ok(linker) } else { Err("The MSVC Rust is missing its bundled linker".into()) }
    }
    fn msvc_vars(&self, rust_version: &str, with_cuda: bool) -> Result<Vec<(String, String)>, String> {
        let rust = self.rust(rust_version);
        let linker = self.rust_linker(rust_version)?;
        let msvc = self.msvc();
        let bin = msvc::msvc_bin_dir(&msvc).ok_or("Install Microsoft's Build Tools and the Windows SDK first")?;
        let tools = msvc::msvc_root_tools(&msvc).ok_or("Microsoft's Build Tools are incomplete")?;
        let kits = msvc.join("Windows Kits/10");
        let (include, lib) = (latest_dir(&kits.join("Include"))?, latest_dir(&kits.join("Lib"))?);
        let mut includes = vec![tools.join("include")];
        includes.extend(["ucrt", "shared", "um", "winrt"].map(|p| include.join(p)));
        let libs = vec![tools.join("lib/x64"), lib.join("ucrt/x64"), lib.join("um/x64")];
        let mut vars = vec![
            ("RUSTC".to_owned(), rust.join("bin").join(exe("rustc"))),
            ("CARGO".to_owned(), rust.join("bin").join(exe("cargo"))),
            ("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER".to_owned(), linker),
            ("NVCC_CCBIN".to_owned(), bin),
            ("SystemRoot".to_owned(), system_root()),
        ];
        let toolkit = self.cuda();
        if with_cuda {
            // The only toolkit a build may link: this folder's own.
            vars.push(("CUDA_PATH".to_owned(), toolkit.clone()));
            vars.push(("MAKEPAD_CUDA_ROOT".to_owned(), toolkit.clone()));
        }
        let joined = |paths: Vec<PathBuf>| -> Result<String, String> {
            Ok(std::env::join_paths(paths).map_err(|e| e.to_string())?.to_string_lossy().into_owned())
        };
        let mut out: Vec<(String, String)> = vars.into_iter().map(|(k, v)| (k, v.to_string_lossy().into_owned())).collect();
        out.extend([
            ("INCLUDE".to_owned(), joined(includes)?),
            ("LIB".to_owned(), joined(libs)?),
            ("MAKEPAD_GGML_NO_CUDA".to_owned(), if with_cuda { "0" } else { "1" }.to_owned()),
            ("MAKEPAD_GGML_REQUIRE_CUDA".to_owned(), if with_cuda { "1" } else { "0" }.to_owned()),
            ("CUDA_HOME".to_owned(), String::new()),
            ("CUDACXX".to_owned(), String::new()),
        ]);
        if !with_cuda {
            out.push(("CUDA_PATH".to_owned(), String::new()));
        }
        for (_, value) in &mut out {
            *value = plain_paths(value);
        }
        Ok(out)
    }
}

fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

fn with_inherited_path(first: &[PathBuf]) -> Result<String, String> {
    let mut path = first.to_vec();
    if let Some(inherited) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&inherited));
    }
    Ok(std::env::join_paths(path).map_err(|e| e.to_string())?.to_string_lossy().into_owned())
}

/// The check's row: its steps are real ones, done or not.
fn check_progress(what: &str, done: usize, steps: usize) {
    let fraction = done as f64 / steps as f64;
    crate::progress::deliver(crate::progress::Progress {
        stage: "Checking".into(),
        detail: what.into(),
        loaded: done as u64,
        total: steps as u64,
        frac: fraction as f32,
        unit: crate::progress::Unit::None,
        package: crate::progress::Package::default(),
        overall: Some(crate::progress::Overall { label: CHECK.into(), fraction, ..Default::default() }),
        row: CHECK.into(),
    });
}

fn write_replacing(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut next = path.as_os_str().to_owned();
    next.push(".new");
    fs::write(&next, format!("{text}\n")).map_err(|e| format!("{}: {e}", path.display()))?;
    fs::rename(&next, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn latest_dir(parent: &Path) -> Result<PathBuf, String> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(parent).map_err(|e| format!("{}: {e}", parent.display()))?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.pop().ok_or_else(|| format!("Nothing in {}", parent.display()))
}

/// Why a build failed, when the reason is the CUDA kernels: nvcc's own words, from its log.
pub fn cuda_kernels_failure(log: &Path) -> Option<String> {
    let text = fs::read(log).ok()?;
    let text = String::from_utf8_lossy(&text);
    text.lines().find(|line| line.contains("MAKEPAD_GGML_REQUIRE_CUDA=1, but the CUDA backend build failed"))?;
    let diagnostic = text
        .lines()
        .find(|line| line.contains("nvcc:") && (line.contains("error") || line.contains("fatal")))
        .map(|line| line.split("nvcc:").nth(1).unwrap_or(line).trim().to_owned());
    Some(plain_paths(&diagnostic.unwrap_or_else(|| "nvcc failed".into())).chars().take(200).collect())
}

/// Whether a Cargo manifest's `[features]` table declares `feature` (a `feature = ...` line in it).
pub fn declares_feature(manifest: &str, feature: &str) -> bool {
    let mut in_features = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_features = line == "[features]";
        } else if in_features && line.strip_prefix(feature).is_some_and(|rest| rest.trim_start().starts_with('=')) {
            return true;
        }
    }
    false
}

/// Puts the toolkit's runtime DLLs beside a built executable: Windows looks for DLLs there and on
/// PATH, not in this folder. Hard links (no second copy of the 435 MB cuBLASLt), copies where a
/// link is refused.
pub fn publish_cuda_runtime(runtime: &Path, beside: &Path) -> Result<(), String> {
    let entries = fs::read_dir(runtime).map_err(|e| format!("{}: {e}", runtime.display()))?;
    for entry in entries.flatten() {
        let from = entry.path();
        if from.extension().is_none_or(|e| !e.eq_ignore_ascii_case("dll")) {
            continue;
        }
        let to = beside.join(entry.file_name());
        let same = fs::metadata(&to).ok().zip(entry.metadata().ok()).is_some_and(|(a, b)| a.len() == b.len());
        if same {
            continue;
        }
        let _ = fs::remove_file(&to);
        if fs::hard_link(&from, &to).is_err() {
            fs::copy(&from, &to).map_err(|e| format!("CUDA runtime {}: {e}", entry.file_name().to_string_lossy()))?;
        }
    }
    Ok(())
}

/// An NVIDIA driver on a Linux machine.
pub fn linux_driver() -> bool {
    Path::new("/proc/driver/nvidia/version").exists() || on_path("nvidia-smi").is_some()
}

/// A CUDA toolkit on a Linux machine.
#[derive(Clone, Debug, PartialEq)]
pub struct LinuxCuda {
    pub root: PathBuf,
    pub lib: PathBuf,
    pub version: Option<String>,
}

/// An NVIDIA driver and a toolkit with nvcc and libcudart (the test libs/ai/cuda/build.rs makes of
/// MAKEPAD_CUDA_ROOT), from CUDA_HOME, CUDA_PATH, /usr/local/cuda, /usr/local/cuda-*, /opt/cuda or
/// the nvcc on PATH. Nothing is installed: a Linux machine gets its toolkit from its distribution.
pub fn linux_cuda() -> Option<LinuxCuda> {
    if !linux_driver() {
        return None;
    }
    let mut roots: Vec<PathBuf> = ["CUDA_HOME", "CUDA_PATH"].iter().filter_map(std::env::var_os).filter(|v| !v.is_empty()).map(PathBuf::from).collect();
    roots.push(PathBuf::from("/usr/local/cuda"));
    let mut versioned: Vec<PathBuf> = fs::read_dir("/usr/local").into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("cuda-"))).collect();
    versioned.sort();
    roots.extend(versioned);
    roots.push(PathBuf::from("/opt/cuda"));
    if let Some(nvcc) = on_path("nvcc") {
        if let Some(root) = nvcc.parent().and_then(Path::parent) {
            roots.push(root.to_path_buf());
        }
    }
    roots.into_iter().find_map(|root| linux_toolkit(root))
}

fn linux_toolkit(root: PathBuf) -> Option<LinuxCuda> {
    if !root.join("bin/nvcc").is_file() {
        return None;
    }
    let lib = ["lib64", "targets/x86_64-linux/lib", "lib"].iter().map(|l| root.join(l)).find(|dir| {
        fs::read_dir(dir).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with("libcudart.so"))
    })?;
    let version = fs::read_to_string(root.join("version.json")).ok().and_then(|text| cuda_version_of(&text));
    Some(LinuxCuda { root, lib, version })
}

/// The toolkit's version in its `version.json`: `{"cuda": {"version": "13.2.2"}, ...}`.
fn cuda_version_of(text: &str) -> Option<String> {
    let value = makepad_strict_json::parse(text.as_bytes()).ok()?;
    value.get("cuda")?.get("version")?.as_str().map(str::to_owned)
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(name)).find(|p| p.is_file())
}

fn stop_verification(child: &mut std::process::Child) -> Result<(), String> {
    #[cfg(unix)]
    {
        extern "C" { fn kill(pid: i32, signal: i32) -> i32; }
        if unsafe { kill(-(child.id() as i32), 9) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(3) { return Err(format!("could not stop verification: {error}")); }
        }
    }
    #[cfg(windows)]
    {
        let mut stop = Command::new("taskkill");
        crate::hide_console(&mut stop);
        let output = stop.args(["/T", "/F", "/PID", &child.id().to_string()]).output().map_err(|e| format!("could not stop verification: {e}"))?;
        if !output.status.success() && child.try_wait().map_err(|e| e.to_string())?.is_none() {
            return Err(format!("could not stop verification ({}): {}{}", output.status, String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)));
        }
    }
    #[cfg(not(any(unix, windows)))]
    child.kill().map_err(|e| format!("could not stop verification: {e}"))?;
    child.wait().map_err(|e| format!("could not wait for verification: {e}"))?;
    Ok(())
}

fn verification_output(command: &mut Command, log: &Path, stopped: &dyn Fn() -> bool) -> Result<std::process::Output, String> {
    use std::process::Stdio;
    if stopped() { return Err(jobs::CANCELLED.into()); }
    // A file cannot fill a pipe and deadlock while the worker watches cancellation.
    let output = fs::File::create(log).map_err(|e| e.to_string())?;
    command.stdin(Stdio::null()).stdout(output.try_clone().map_err(|e| e.to_string())?).stderr(output);
    #[cfg(unix)]
    { use std::os::unix::process::CommandExt; command.process_group(0); }
    crate::hide_console(command);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let status = loop {
        if stopped() {
            stop_verification(&mut child)?;
            return Err(jobs::CANCELLED.into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(error) => { stop_verification(&mut child)?; return Err(error.to_string()); }
        }
    };
    if stopped() {
        stop_verification(&mut child)?;
        return Err(jobs::CANCELLED.into());
    }
    Ok(std::process::Output { status, stdout: fs::read(log).map_err(|e| e.to_string())?, stderr: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Temp {
            let path = std::env::temp_dir().join(format!("makepad-toolchain-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Temp(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stop_ends_a_verification_step_that_would_not_end() {
        // The step never ends by itself: without Stop reaching it this test does not end either.
        #[cfg(windows)]
        let mut command = { let mut c = Command::new("cmd.exe"); c.args(["/d", "/c", "ping -n 100000 127.0.0.1 >nul"]); c };
        #[cfg(not(windows))]
        let mut command = { let mut c = Command::new("sh"); c.args(["-c", "sleep 100000; exit 0"]); c };
        let calls = std::cell::Cell::new(0);
        // Stop is pressed once the step has been started.
        let stopped = || { calls.set(calls.get() + 1); calls.get() > 1 };
        let temp = Temp::new("cancel-check");
        let result = verification_output(&mut command, &temp.0.join("output.log"), &stopped);
        assert_eq!(result.err().as_deref(), Some(jobs::CANCELLED));
    }

    #[test]
    fn a_folder_nothing_was_chosen_in_compiles_with_gnu_and_a_choice_stays() {
        let temp = Temp::new("chain");
        let folder = Folder::new(temp.0.clone());
        assert_eq!(folder.chain(), Chain::Gnu);
        folder.set_chain(Chain::Msvc).unwrap();
        assert_eq!(Folder::new(temp.0.clone()).chain(), Chain::Msvc);
        folder.set_chain(Chain::Gnu).unwrap();
        assert_eq!(folder.chain(), Chain::Gnu);
    }

    /// A stopped install leaves files without its stamps: those files say nothing.
    #[test]
    fn only_a_finished_and_verified_install_counts_as_installed() {
        let temp = Temp::new("installed");
        let folder = Folder::new(temp.0.clone());
        let msvc = temp.0.join("msvc/VC/Tools/MSVC/14.44/bin/Hostx64/x64");
        fs::create_dir_all(&msvc).unwrap();
        fs::write(msvc.join("cl.exe"), "").unwrap();
        let nvcc = temp.0.join("cuda/bin");
        fs::create_dir_all(&nvcc).unwrap();
        fs::write(nvcc.join(exe("nvcc")), "").unwrap();
        assert_eq!(folder.installed(), Installed::default(), "the files of a stopped install");
        assert!(!msvc::ready(&temp.0.join("msvc")) && !cuda::ready(&temp.0.join("cuda")));
        // everything unpacked, the check not passed
        fs::write(temp.0.join("msvc/.build-tools-ready"), "").unwrap();
        fs::write(temp.0.join("msvc/.sdk-ready"), msvc::SDK_READY_RECORD).unwrap();
        fs::write(temp.0.join("cuda/.ready"), "").unwrap();
        assert_eq!(folder.installed(), Installed::default(), "not verified yet");
        // verified: what the record says, while its parts are there
        let rust = temp.0.join(format!("rust/1.98.0-{MSVC_TRIPLE}/bin"));
        fs::create_dir_all(&rust).unwrap();
        fs::write(rust.join(exe("rustc")), "").unwrap();
        write_replacing(&temp.0.join("verified"), "build-tools 14.44\nsdk 10.0.26100.0\nrust 1.98.0\ncuda 13.2.2\n").unwrap();
        let installed = folder.installed();
        assert!(installed.msvc());
        assert_eq!((installed.build_tools.as_deref(), installed.sdk.as_deref(), installed.rust.as_deref(), installed.cuda.as_deref()), (Some("14.44"), Some("10.0.26100.0"), Some("1.98.0"), Some("13.2.2")));
        fs::remove_file(temp.0.join("cuda/.ready")).unwrap();
        assert_eq!(folder.installed().cuda, None, "a part that went");
    }

    #[test]
    fn a_failed_check_clears_only_what_it_found_broken_and_its_row_fails() {
        let temp = Temp::new("failed-check");
        let folder = Folder::new(temp.0.clone());
        let version = "1.98.0";
        let (msvc_dir, cuda_dir, rust) = (folder.msvc(), folder.cuda(), folder.rust(version));
        for dir in [&msvc_dir, &cuda_dir, &rust] {
            fs::create_dir_all(dir).unwrap();
        }
        let stamp = format!("{version} {MSVC_TRIPLE}");
        let mark = || {
            fs::write(msvc_dir.join(".build-tools-ready"), "").unwrap();
            fs::write(msvc_dir.join(".sdk-ready"), msvc::SDK_READY_RECORD).unwrap();
            fs::write(cuda_dir.join(".ready"), "").unwrap();
            fs::write(rust.join(".toolchain-version"), &stamp).unwrap();
        };
        let state = || (msvc::ready(&msvc_dir), folder.rust_stamped(version), cuda::ready(&cuda_dir));
        mark();
        // The whole way, nothing to download: Rust's linker isn't there, so it alone goes, and the row says why.
        let rows = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = rows.clone();
        let result = crate::progress::scope(move |p| seen.borrow_mut().push(p), || folder.install(version, false, Default::default()));
        let error = result.unwrap_err();
        assert!(error.contains("bundled linker"), "{error}");
        assert_eq!(state(), (true, false, true));
        assert!(rows.borrow().iter().any(|p| p.row == CHECK && p.detail.contains("bundled linker")));

        // With Rust's linker there, the step that next fails suspects the Build Tools.
        let bin = rust.join("lib/rustlib").join(MSVC_TRIPLE).join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join(exe("rust-lld")), "").unwrap();
        let mut suspects: &[Part] = &[];
        assert!(folder.verify(version, false, &|| false, &mut suspects).unwrap_err().contains("Build Tools"));
        assert_eq!(suspects, &[Part::Msvc]);

        for (part, after) in [(Part::Cuda, (true, true, false)), (Part::Rust, (true, false, true)), (Part::Msvc, (false, true, true))] {
            mark();
            assert_eq!(folder.clear_suspects("broken".into(), &[part], version), "broken");
            assert_eq!(state(), after, "{part:?}");
        }
        // A marker that can't go doesn't lose the check's error.
        fs::remove_file(rust.join(".toolchain-version")).unwrap();
        fs::create_dir_all(rust.join(".toolchain-version/held")).unwrap();
        let report = folder.clear_suspects("broken".into(), &[Part::Rust], version);
        assert!(report.starts_with("broken\nremove ") && report.contains(".toolchain-version"), "{report}");
    }

    #[test]
    fn licenses_accumulate() {
        let temp = Temp::new("accept");
        let folder = Folder::new(temp.0.clone());
        assert!(folder.accepted().is_empty());
        folder.accept(&["vs", "sdk"]).unwrap();
        folder.accept(&["sdk", "cuda"]).unwrap();
        assert_eq!(folder.accepted(), ["vs", "sdk", "cuda"]);
    }

    #[test]
    fn a_build_without_cuda_names_itself_so_and_asks_for_no_kernels() {
        let build = Build {
            vars: vec![("MAKEPAD_GGML_NO_CUDA".into(), "0".into()), ("MAKEPAD_GGML_REQUIRE_CUDA".into(), "1".into()), ("LIB".into(), "x".into())],
            local_ai: true,
            require_cuda: true,
            cuda_runtime: Some(PathBuf::from("x")),
            stamp: "msvc cuda".into(),
            ..Build::default()
        };
        let cpu = build.without_cuda();
        assert_eq!(cpu.stamp, "msvc cpu");
        assert!(!cpu.local_ai && !cpu.require_cuda && cpu.cuda_runtime.is_none());
        assert_eq!(cpu.vars, vec![("MAKEPAD_GGML_NO_CUDA".to_owned(), "1".to_owned()), ("MAKEPAD_GGML_REQUIRE_CUDA".to_owned(), "0".to_owned()), ("LIB".to_owned(), "x".to_owned())]);
    }

    #[test]
    fn the_kernel_failure_reason_comes_from_nvcc() {
        let temp = Temp::new("kernels");
        let log = temp.0.join("log");
        fs::write(&log, "  Compiling x\nnothing about kernels\n").unwrap();
        assert_eq!(cuda_kernels_failure(&log), None, "another failure");
        fs::write(&log, "MAKEPAD_GGML_REQUIRE_CUDA=1, but the CUDA backend build failed: x\nkernels: nvcc: fatal error: no input\n").unwrap();
        assert_eq!(cuda_kernels_failure(&log).as_deref(), Some("fatal error: no input"));
        fs::write(&log, "MAKEPAD_GGML_REQUIRE_CUDA=1, but the CUDA backend build failed: x\n").unwrap();
        assert_eq!(cuda_kernels_failure(&log).as_deref(), Some("nvcc failed"));
    }

    #[test]
    fn a_feature_counts_only_inside_the_features_table() {
        let manifest = "[package]\nlocalai = 1\n[features]\ndefault = []\nlocalai = [\"x\"]\ncuda2 = []\n[dependencies]\ncuda = \"1\"\n";
        assert!(declares_feature(manifest, "localai"));
        assert!(!declares_feature(manifest, "cuda"), "a dependency, and a longer name");
        assert!(!declares_feature("[package]\nlocalai = 1\n", "localai"));
    }

    #[test]
    fn a_toolkit_needs_nvcc_and_its_runtime_and_says_its_version() {
        let temp = Temp::new("linux");
        let root = temp.0.join("cuda");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/nvcc"), "").unwrap();
        assert_eq!(linux_toolkit(root.clone()), None, "no runtime");
        fs::create_dir_all(root.join("lib64")).unwrap();
        fs::write(root.join("lib64/libcudart.so.13"), "").unwrap();
        fs::write(root.join("version.json"), "{\"cuda\":{\"name\":\"CUDA SDK\",\"version\":\"13.2.2\"}}").unwrap();
        let found = linux_toolkit(root.clone()).unwrap();
        assert_eq!((found.lib, found.version.as_deref()), (root.join("lib64"), Some("13.2.2")));
    }
}
