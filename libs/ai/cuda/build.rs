use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

// The nvcc build for THE CUDA store (aiarch.md §1 + §4, lane T4), and the
// single authority on whether this machine gets CUDA at all.
//
// Three answers, one decision point:
//   * kernels compiled and archived -> `cargo:rustc-cfg=makepad_ai_cuda_kernels`
//     + the `rustc-link-search` / `rustc-link-lib` lines for cudart, cuBLAS
//     and cuBLASLt, taken from the toolkit we actually found;
//   * no usable toolkit (or MAKEPAD_GGML_NO_CUDA) -> none of the above, and
//     `src/link_gate.rs` turns every extern block into a link-clean stub, so
//     the build SUCCEEDS with no CUDA;
//   * MAKEPAD_GGML_REQUIRE_CUDA=1 -> the second case panics instead, which is
//     how a fleet box refuses to silently lose its GPU.
//
// Because this crate sets `links = "makepad_ai_cuda"`, the answer travels to
// its immediate dependents as `DEP_MAKEPAD_AI_CUDA_KERNELS` (=1) and
// `DEP_MAKEPAD_AI_CUDA_ARCH`. makepad-ai-llm, makepad-ai-metal,
// makepad-ai-common and makepad-voice gate their CUDA code on exactly that
// and MUST NOT probe for a toolkit themselves: "nvcc exists on this machine"
// and "kernels were built and will link" are different questions, and a
// dependent that answers the first one locally is how a machine WITH the
// CUDA toolkit still failed to link with `unresolved external symbol
// cudaFree`.
fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_CUDA_ARCH");
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_REQUIRE_CUDA");
    println!("cargo:rerun-if-env-changed=MAKEPAD_GGML_NO_CUDA");
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    // The Windows toolkit scan roots at %ProgramFiles%, so it is an input to
    // the decision as much as CUDA_PATH is — without this, installing (or
    // hiding) a toolkit leaves a stale cached answer behind.
    println!("cargo:rerun-if-env-changed=ProgramFiles");
    println!("cargo:rustc-check-cfg=cfg(makepad_ai_cuda_kernels)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let require_cuda = env_flag("MAKEPAD_GGML_REQUIRE_CUDA");
    if require_cuda && target_os != "linux" && target_os != "windows" {
        panic!(
            "MAKEPAD_GGML_REQUIRE_CUDA=1, but CUDA kernels are unsupported for target OS {target_os:?}"
        );
    }
    // A standalone app build (the VJ) must not link CUDA merely because the
    // machine happens to carry the toolkit — the exe would then demand the
    // CUDA DLLs on every machine it ships to. NO_CUDA forces the kernel-less
    // stub the no-toolkit path already produces; it outranks REQUIRE.
    if env_flag("MAKEPAD_GGML_NO_CUDA") {
        println!(
            "cargo:warning=makepad-ai-cuda: MAKEPAD_GGML_NO_CUDA set — building without CUDA kernels"
        );
        return;
    }
    if target_os == "linux" || target_os == "windows" {
        build_cuda_backends(&target_os, require_cuda);
    }
    // macos/other targets: nothing to do, no kernels, no cfg, no
    // cargo:kernels line — dependents see DEP_MAKEPAD_AI_CUDA_KERNELS unset.
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}


/// The compute capability of this machine's first NVIDIA GPU, in nvcc arch
/// form ("7.5" -> "75"; Blackwell "12.0" -> "120a"), via nvidia-smi. `None`
/// when there is no driver to ask (cross/CI builds keep the env/default).
fn detect_local_arch() -> Option<String> {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=compute_cap", "--format=csv,noheader"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let cap = String::from_utf8_lossy(&out.stdout);
    let cap = cap.lines().next()?.trim();
    let (major, minor) = cap.split_once('.')?;
    let (major, minor): (u32, u32) = (major.trim().parse().ok()?, minor.trim().parse().ok()?);
    let arch = format!("{major}{minor}");
    // sm_120 kernels are built with the arch-specific 'a' suffix (the
    // repo's Blackwell convention); earlier generations use the plain form.
    Some(if major >= 12 { format!("{arch}a") } else { arch })
}

fn build_cuda_backends(target_os: &str, require_cuda: bool) {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_paths = [
        manifest_dir.join("kernels/affine.cu"),
        manifest_dir.join("kernels/diffusion_ops.cu"),
        manifest_dir.join("kernels/gated_delta_net.cu"),
        manifest_dir.join("kernels/kquants.cu"),
        manifest_dir.join("kernels/llm/kernels.cu"),
        manifest_dir.join("kernels/nvfp4.cu"),
        manifest_dir.join("kernels/nvfp4_mmq.cu"),
        manifest_dir.join("kernels/ops.cu"),
        manifest_dir.join("kernels/paint_extras.cu"),
        manifest_dir.join("kernels/rife.cu"),
        manifest_dir.join("kernels/roformer.cu"),
        manifest_dir.join("kernels/splat.cu"),
        manifest_dir.join("kernels/ssm_conv.cu"),
    ];
    for src_path in &src_paths {
        println!("cargo:rerun-if-changed={}", src_path.display());
    }
    rerun_if_changed_tree(&manifest_dir.join("kernels/llm"));

    let Some(cuda_root) = cuda_root(target_os) else {
        cuda_unavailable(require_cuda, "CUDA toolkit root not found");
        return;
    };

    let nvcc = nvcc_path(&cuda_root, target_os);
    if !nvcc.exists() {
        cuda_unavailable(
            require_cuda,
            &format!("CUDA nvcc not found at {}", nvcc.display()),
        );
        return;
    }
    // Resolve the import/shared libs BEFORE spending minutes in nvcc: a
    // toolkit whose libs we cannot find would compile kernels and then fail
    // the final link with `unresolved external symbol cudaFree`, which is
    // the failure this whole path exists to prevent.
    let Some(lib_dir) = cuda_lib_dir(&cuda_root, target_os) else {
        cuda_unavailable(
            require_cuda,
            &format!(
                "CUDA link libraries not found under {} (looked for {})",
                cuda_root.display(),
                lib_dir_candidates(&cuda_root, target_os)
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        return;
    };
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let lib_path = if target_os == "windows" {
        out_dir.join("ggml_cuda_affine.lib")
    } else {
        out_dir.join("libggml_cuda_affine.a")
    };
    let obj_ext = if target_os == "windows" { "obj" } else { "o" };
    // Source builds run on the machine that will run the exe, so the right
    // default arch is THIS machine's GPU — ask the driver. A wrong default
    // is silent sabotage: kernels for another generation compile fine and
    // then refuse to load at runtime. Env overrides for cross-builds.
    let arch = env::var("MAKEPAD_GGML_CUDA_ARCH")
        .ok()
        .or_else(detect_local_arch)
        .unwrap_or_else(|| "120a".to_string());
    // The permanent record, in the build log. It lands only when this script
    // exits (cargo buffers `cargo:warning` — measured, not assumed), so it
    // cannot be the thing that reassures somebody mid-build; that is what
    // `Console` below is for.
    println!(
        "cargo:warning=makepad-ai-cuda: building {} CUDA kernels for sm_{arch}, toolkit {}",
        src_paths.len(),
        cuda_root.display()
    );
    let include_dir = cuda_root.join("include");
    let msvc_bin_dir = if target_os == "windows" {
        find_msvc_tool("cl.exe").and_then(|path| path.parent().map(Path::to_path_buf))
    } else {
        None
    };
    let lib_exe = if target_os == "windows" {
        match find_msvc_tool("lib.exe") {
            Some(path) => Some(path),
            None => {
                cuda_unavailable(require_cuda, "MSVC lib.exe not found");
                return;
            }
        }
    } else {
        None
    };

    // A fresh CUDA build is minutes of silent ptxas under a cargo line that
    // never moves. Tell the terminal what is happening, up front and as it
    // goes; and run the kernels concurrently, because one nvcc at a time
    // leaves most of a modern machine idle.
    let total = src_paths.len();
    let jobs = kernel_jobs(total);
    let mut console = Console::open();
    console.line(&format!(
        "makepad-ai-cuda: compiling {total} CUDA kernels for sm_{arch} (toolkit {}), \
         {jobs} at a time.\n\
         makepad-ai-cuda: one-time, and typically a couple of minutes. Progress below.",
        cuda_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| cuda_root.display().to_string()),
    ));
    let kernels_started = Instant::now();

    // Derived from the source list, never from completion order, so the
    // archive below is byte-identical however the jobs happen to interleave.
    let obj_paths = src_paths
        .iter()
        .map(|src_path| {
            let stem = src_path.file_stem().unwrap().to_string_lossy();
            out_dir.join(format!("ggml_cuda_{stem}.{obj_ext}"))
        })
        .collect::<Vec<_>>();

    let mut pending = src_paths.iter().zip(&obj_paths).enumerate();
    let mut running: Vec<Kernel> = Vec::new();
    let mut done = 0usize;
    let mut failed: Option<(PathBuf, std::io::Result<Output>)> = None;
    let mut since_tick = Instant::now();
    loop {
        while failed.is_none() && running.len() < jobs {
            let Some((index, (src_path, obj_path))) = pending.next() else {
                break;
            };
            let arch_flag = format!("arch=compute_{arch},code=sm_{arch}");
            let mut command = Command::new(&nvcc);
            command.args(["-std=c++17", "-O3"]);
            if target_os == "windows" {
                if let Some(msvc_bin_dir) = &msvc_bin_dir {
                    command.arg("-ccbin").arg(msvc_bin_dir);
                }
                command.args(["-Xcompiler", "/EHsc"]);
                command.args(["-Xcompiler", "/MD"]);
            } else {
                command.args(["-Xcompiler", "-fPIC"]);
            }
            if let Some(src_dir) = src_path.parent() {
                command.arg("-I").arg(src_dir);
            }
            // Piped, not inherited: when nvcc refuses (an arch this toolkit
            // dropped, an MSVC environment it cannot drive) its own message
            // is the only thing that tells the user what to do, and a build
            // script's child stderr is otherwise swallowed. The pipes must
            // be drained or a chatty nvcc deadlocks on a full buffer, so
            // each `wait_with_output` gets its own thread and this loop
            // stays free to tick.
            let spawned = command
                .args([
                    "-c",
                    "-I",
                    include_dir.to_string_lossy().as_ref(),
                    "-gencode",
                    arch_flag.as_str(),
                    "-o",
                    obj_path.to_string_lossy().as_ref(),
                    src_path.to_string_lossy().as_ref(),
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();
            match spawned {
                Ok(child) => running.push(Kernel {
                    index,
                    waiter: std::thread::spawn(move || child.wait_with_output()),
                }),
                Err(err) => failed = Some((src_path.clone(), Err(err))),
            }
        }
        if running.is_empty() {
            break;
        }
        let mut slot = 0;
        while slot < running.len() {
            if !running[slot].waiter.is_finished() {
                slot += 1;
                continue;
            }
            let kernel = running.remove(slot);
            let result = kernel
                .waiter
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("nvcc wait thread panicked")));
            if result.as_ref().is_ok_and(|out| out.status.success()) {
                done += 1;
                console.line(&progress_line(done, total, kernels_started, running.len()));
                since_tick = Instant::now();
            } else if failed.is_none() {
                failed = Some((src_paths[kernel.index].clone(), result));
            }
        }
        if since_tick.elapsed() >= KERNEL_HEARTBEAT {
            console.line(&progress_line(done, total, kernels_started, running.len()));
            since_tick = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    if let Some((src_path, result)) = failed {
        if let Ok(out) = &result {
            report_tool_output("nvcc", out);
        }
        cuda_unavailable(
            require_cuda,
            &format!(
                "failed to compile CUDA backend source {} for sm_{arch} \
                 (override the arch with MAKEPAD_GGML_CUDA_ARCH)",
                src_path.display()
            ),
        );
        return;
    }

    let archive = if target_os == "windows" {
        let mut lib = Command::new(lib_exe.unwrap());
        lib.arg("/NOLOGO")
            .arg(format!("/OUT:{}", lib_path.to_string_lossy()));
        for obj_path in &obj_paths {
            lib.arg(obj_path);
        }
        lib.output()
    } else {
        let mut ar = Command::new("ar");
        ar.arg("crus").arg(lib_path.to_string_lossy().as_ref());
        for obj_path in &obj_paths {
            ar.arg(obj_path.to_string_lossy().as_ref());
        }
        ar.output()
    };
    if !archive.as_ref().is_ok_and(|out| out.status.success()) {
        if let Ok(out) = &archive {
            report_tool_output("archiver", out);
        }
        cuda_unavailable(require_cuda, "failed to archive CUDA backends");
        return;
    }

    let kernels_took = fmt_duration(kernels_started.elapsed());
    console.line(&format!(
        "makepad-ai-cuda: CUDA kernels built in {kernels_took}. Back to cargo."
    ));
    println!("cargo:warning=makepad-ai-cuda: CUDA kernels built in {kernels_took}");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=ggml_cuda_affine");
    // Always by IMPORT-LIB NAME (`cudart`), never by the versioned runtime
    // name (`cudart64_12.dll` / `cudart64_13.dll`): the import lib in the
    // detected toolkit's lib dir names its own DLL, so one rule spans every
    // CUDA major version. `-L` from a build script propagates to the final
    // binary link even from a transitive dependency, so nothing downstream
    // needs LIB / LD_LIBRARY_PATH set by hand.
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=cudart");
    println!("cargo:rustc-link-lib=dylib=cublas");
    println!("cargo:rustc-link-lib=dylib=cublasLt");
    if target_os == "linux" {
        println!("cargo:rustc-link-lib=dylib=stdc++");
        if dir_has_file_starting_with(&lib_dir, "libcudnn.so") {
            println!("cargo:rustc-link-lib=dylib=cudnn");
        }
    } else if target_os == "windows" && lib_dir.join("cudnn.lib").exists() {
        println!("cargo:rustc-link-lib=dylib=cudnn");
    }
    println!("cargo:rustc-cfg=makepad_ai_cuda_kernels");
    // links-metadata handshake: flows to immediate dependents (ai-llm,
    // ai-metal, ai-common, voice) as DEP_MAKEPAD_AI_CUDA_KERNELS=1 /
    // DEP_MAKEPAD_AI_CUDA_ARCH. Those crates must not probe for a toolkit
    // themselves: "nvcc exists" and "kernels built and will link" are
    // different questions, and answering the first one locally is how a
    // machine WITH the toolkit ended up with unresolved `cudaFree`.
    println!("cargo:kernels=1");
    println!("cargo:arch={arch}");
}

/// The terminal cargo is writing to, when there is one.
///
/// A build script cannot reach the user while it runs: cargo captures its
/// stdout and stderr, and holds every `cargo:warning` line until the script
/// EXITS (measured — a warning printed six seconds before the script ended
/// still appeared only at the end). For a script that then spends minutes
/// inside ptxas, that means the only honest live channel is the console
/// device itself. No console (CI, a piped build, a GUI invocation) simply
/// means no progress lines; the `cargo:warning` summary still records what
/// happened.
struct Console(Option<fs::File>);

impl Console {
    fn open() -> Self {
        let device = if cfg!(windows) { "CONOUT$" } else { "/dev/tty" };
        Console(fs::OpenOptions::new().write(true).open(device).ok())
    }

    fn line(&mut self, message: &str) {
        if let Some(console) = self.0.as_mut() {
            let _ = writeln!(console, "{message}");
            let _ = console.flush();
        }
    }
}

/// Heartbeat interval while kernels are compiling. Short enough that a
/// watching user always sees motion inside half a minute — which is the
/// whole bar — without turning the build into a log firehose.
const KERNEL_HEARTBEAT: Duration = Duration::from_secs(20);

/// One nvcc job in flight. The join handle owns the drain-and-wait.
struct Kernel {
    index: usize,
    waiter: std::thread::JoinHandle<std::io::Result<Output>>,
}

/// How many nvcc processes to run at once.
///
/// Two ceilings, because either one alone gets a machine wrong. CPU: cargo's
/// own `-j` (NUM_JOBS), which is the parallelism the user actually asked
/// for. Memory: nvcc's cicc/ptxas peak near a gigabyte per source, so a
/// 16-thread laptop with 8GB would swap itself to death long before it ran
/// out of cores — 1.5GB of headroom each. When the memory is unknowable we
/// assume a normal modern machine (8) rather than creep, since the common
/// case is a box that can easily take it.
fn kernel_jobs(total: usize) -> usize {
    const BYTES_PER_JOB: u64 = 1536 * 1024 * 1024;
    let by_cpu = env::var("NUM_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse::<usize>().ok())
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(8);
    let by_memory = available_memory_bytes()
        .map(|bytes| (bytes / BYTES_PER_JOB) as usize)
        .unwrap_or(8);
    by_cpu.min(by_memory).min(total).max(1)
}

/// Physical memory this machine can actually hand to a wave of compilers.
/// std-only, because a build script that needs a crates.io dependency to
/// decide how hard to work has its priorities wrong.
#[cfg(target_os = "windows")]
fn available_memory_bytes() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.avail_phys)
}

#[cfg(target_os = "linux")]
fn available_memory_bytes() -> Option<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    let line = meminfo
        .lines()
        .find(|line| line.starts_with("MemAvailable:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn available_memory_bytes() -> Option<u64> {
    // No CUDA host here (kernels only build on linux/windows), so this only
    // exists to keep the call site honest on every host.
    None
}

/// One progress line, with an ETA that corrects itself from the completion
/// throughput measured so far rather than from a number somebody guessed
/// once. Early on — before the first wave lands — it reads pessimistically,
/// which is the safe direction to be wrong in.
fn progress_line(done: usize, total: usize, started: Instant, running: usize) -> String {
    let elapsed = started.elapsed();
    if done == 0 {
        return format!(
            "makepad-ai-cuda: CUDA kernels 0/{total} — {} elapsed, {running} compiling \
             (the first finishers set the pace)",
            fmt_duration(elapsed)
        );
    }
    if done == total {
        return format!(
            "makepad-ai-cuda: CUDA kernels {done}/{total} — done in {}",
            fmt_duration(elapsed)
        );
    }
    let left = Duration::from_secs_f64(elapsed.as_secs_f64() / done as f64 * (total - done) as f64);
    format!(
        "makepad-ai-cuda: CUDA kernels {done}/{total} — {} elapsed, ~{} left, {running} compiling",
        fmt_duration(elapsed),
        fmt_duration(left)
    )
}

fn fmt_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 60 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// Surface a failed child tool's own words as cargo warnings, trimmed to
/// something readable in a build log.
fn report_tool_output(tool: &str, output: &std::process::Output) {
    let text = String::from_utf8_lossy(&output.stderr);
    let text = if text.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        text.into_owned()
    };
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    // The first real diagnostic, then the tail. nvcc prefaces a failure with
    // hundreds of warnings out of the vendored headers, so a fixed window at
    // the start reports the weather and never the crash — which is how a
    // compile failure here used to read as twelve lines of "floating-point
    // value does not fit" and no reason at all. The tail is included
    // unconditionally because that is where "N errors detected" lands, and
    // because a tool that died without printing a diagnostic still has to say
    // something.
    let is_diagnostic =
        |line: &&str| line.contains("error:") || line.contains("error C") || line.contains("Error:");
    let head = lines.iter().position(is_diagnostic).map(|at| {
        let start = at.saturating_sub(2);
        (start, (start + REPORTED_TOOL_LINES).min(lines.len()))
    });
    let tail_start = lines.len().saturating_sub(REPORTED_TOOL_TAIL_LINES);
    let mut printed_to = 0usize;
    for (start, end) in head.into_iter().chain(std::iter::once((tail_start, lines.len()))) {
        let start = start.max(printed_to);
        if start >= end {
            continue;
        }
        if start > printed_to {
            println!("cargo:warning={tool}: ... {} line(s) elided", start - printed_to);
        }
        for line in &lines[start..end] {
            println!("cargo:warning={tool}: {line}");
        }
        printed_to = end;
    }
}

/// How much of a failing tool's first diagnostic to forward as cargo warnings.
const REPORTED_TOOL_LINES: usize = 30;
/// And how much of its tail, where the summary line lives.
const REPORTED_TOOL_TAIL_LINES: usize = 15;

fn dir_has_file_starting_with(dir: &Path, prefix: &str) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(prefix)
        })
    })
}

fn rerun_if_changed_tree(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            rerun_if_changed_tree(&child);
        } else {
            println!("cargo:rerun-if-changed={}", child.display());
        }
    }
}

/// The one exit for "this machine gets no CUDA". Loud and fatal when a fleet
/// box asked for CUDA; otherwise a warning and a stub build that still LINKS
/// — see `src/link_gate.rs` for why that is the same decision.
fn cuda_unavailable(required: bool, reason: &str) {
    if required {
        panic!("MAKEPAD_GGML_REQUIRE_CUDA=1, but the CUDA backend build failed: {reason}");
    }
    println!("cargo:warning=makepad-ai-cuda: {reason}");
    println!(
        "cargo:warning=makepad-ai-cuda: building WITHOUT CUDA — GPU inference is unavailable, \
         everything else builds and runs normally (set MAKEPAD_GGML_REQUIRE_CUDA=1 to make this fatal)"
    );
}

fn nvcc_path(cuda_root: &Path, target_os: &str) -> PathBuf {
    if target_os == "windows" {
        cuda_root.join("bin").join("nvcc.exe")
    } else {
        cuda_root.join("bin").join("nvcc")
    }
}

/// Where a toolkit of this version keeps the libs we link against. Ordered
/// most-specific first; every CUDA major version so far uses one of these,
/// and an unknown future layout simply reads as "no usable toolkit" (a
/// stub build) instead of a link failure.
fn lib_dir_candidates(cuda_root: &Path, target_os: &str) -> Vec<PathBuf> {
    if target_os == "windows" {
        vec![cuda_root.join("lib").join("x64"), cuda_root.join("lib")]
    } else {
        vec![
            cuda_root.join("lib64"),
            cuda_root.join("targets").join("x86_64-linux").join("lib"),
            cuda_root.join("lib"),
        ]
    }
}

fn cuda_lib_dir(cuda_root: &Path, target_os: &str) -> Option<PathBuf> {
    let prefix = if target_os == "windows" {
        "cudart.lib"
    } else {
        "libcudart.so"
    };
    lib_dir_candidates(cuda_root, target_os)
        .into_iter()
        .find(|dir| dir_has_file_starting_with(dir, prefix))
}

/// A directory is a usable toolkit root only if it has BOTH the compiler and
/// the libs. A bare `CUDA_PATH` left behind by an uninstall, or a driver-only
/// install, is not one — and falling through to the version scan finds the
/// real toolkit next to it instead of disabling CUDA.
fn is_toolkit_root(path: &Path, target_os: &str) -> bool {
    nvcc_path(path, target_os).exists() && cuda_lib_dir(path, target_os).is_some()
}

fn cuda_root(target_os: &str) -> Option<PathBuf> {
    for var in ["CUDA_HOME", "CUDA_PATH"] {
        if let Some(path) = env::var_os(var).map(PathBuf::from) {
            if is_toolkit_root(&path, target_os) {
                return Some(path);
            }
        }
    }
    if target_os == "windows" {
        newest_versioned_root(
            &env::var_os("ProgramFiles")
                .map(PathBuf::from)
                .map(|program_files| {
                    program_files
                        .join("NVIDIA GPU Computing Toolkit")
                        .join("CUDA")
                })?,
            target_os,
        )
    } else {
        // `/usr/local/cuda` is the distribution's own "newest" symlink; only
        // scan for `cuda-13.3`-style siblings when it is missing or unusable.
        let default = Path::new("/usr/local/cuda");
        if is_toolkit_root(default, target_os) {
            return Some(default.to_path_buf());
        }
        newest_versioned_root(Path::new("/usr/local"), target_os)
            .or_else(|| newest_versioned_root(Path::new("/opt"), target_os))
    }
}

/// Newest **usable** toolkit under `base`, by numeric version — `v13.3` beats
/// `v12.4`, and (unlike a filename sort) `v12.4` beats `v9.0`. Directory
/// names are `vMAJOR.MINOR` on Windows and `cuda-MAJOR.MINOR` on Linux; both
/// are read by simply taking the digits.
fn newest_versioned_root(base: &Path, target_os: &str) -> Option<PathBuf> {
    let mut roots = fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_toolkit_root(path, target_os))
        .map(|path| {
            let version = path
                .file_name()
                .map(|name| version_key(&name.to_string_lossy()))
                .unwrap_or_default();
            (version, path)
        })
        .collect::<Vec<_>>();
    roots.sort_by(|a, b| a.0.cmp(&b.0));
    roots.pop().map(|(_, path)| path)
}

/// `"v13.3"` / `"cuda-13.3"` -> `(13, 3)`; anything unparsable sorts lowest.
fn version_key(name: &str) -> (u32, u32) {
    let digits = name.trim_start_matches(|c: char| !c.is_ascii_digit());
    let mut parts = digits.split('.');
    let major = parts.next().unwrap_or_default().parse().unwrap_or(0);
    let minor = parts
        .next()
        .map(|part| part.trim_end_matches(|c: char| !c.is_ascii_digit()))
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);
    (major, minor)
}

fn find_msvc_tool(tool_name: &str) -> Option<PathBuf> {
    if let Some(paths) = env::var_os("PATH") {
        if let Some(path) = env::split_paths(&paths)
            .map(|path| path.join(tool_name))
            .find(|candidate| candidate.exists())
        {
            return Some(path);
        }
    }

    let find_pattern = format!(r"VC\Tools\MSVC\**\bin\Hostx64\x64\{tool_name}");
    for installer_root in [
        r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe",
        r"C:\Program Files\Microsoft Visual Studio\Installer\vswhere.exe",
    ] {
        let vswhere = Path::new(installer_root);
        if !vswhere.exists() {
            continue;
        }
        let output = match Command::new(vswhere)
            .args([
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-find",
                find_pattern.as_str(),
            ])
            .output()
        {
            Ok(output) => output,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }
        if let Some(path) = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.exists())
        {
            return Some(path);
        }
    }
    None
}
