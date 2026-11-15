//! Select the binary's std from a git mount only when every path and byte matches the build.

pub const DIGEST: &str = env!("RELAX_STD_DIGEST");
static SELECTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Automatic choices are marked so a runner's child can select against its own repositories.
/// An explicit RELAX_STD (including an empty one) always wins.
pub fn select(workspace: &str, fallback: &std::path::Path) {
    let current = std::env::var_os("RELAX_STD");
    if current.is_some() && current != std::env::var_os("RELAX_STD_AUTO") {
        return;
    }
    // Recheck an inherited automatic choice in a child (also installs the immutable std
    // overlay there). The loader and cmd_run in the same process only need one check.
    if SELECTED.set(()).is_err() {
        return;
    }
    // a build script's child has its package folder as workspace: the std is the mount root's
    let root = crate::vfs::outer_root(workspace).unwrap_or_else(|| workspace.to_string());
    let candidate = format!("{}/libs/relax/compiler/std", root.trim_end_matches(['/', '\\']));
    let chosen = if crate::vfs::is_virtual(workspace) {
        // Diagnostic hook only forces rejection; it can never approve an incompatible std.
        let expected = if std::env::var_os("RELAX_TEST_STD_MISMATCH").is_some() {
            "test-mismatch"
        } else {
            DIGEST
        };
        let result = crate::vfs::verify_std(&candidate, expected);
        match result {
            Ok(()) => std::path::PathBuf::from(candidate),
            Err(reason) => {
                // (a silent run's stderr is the program's: its parent run said it already)
                if !crate::run::silent() {
                    eprintln!("relax: using disk std {}: {reason}", fallback.display());
                }
                fallback.to_path_buf()
            }
        }
    } else {
        fallback.to_path_buf()
    };
    std::env::set_var("RELAX_STD", &chosen);
    std::env::set_var("RELAX_STD_AUTO", &chosen);
}

/// What runs a build script here (unitgraph::run_build_scripts `tool`), apart from where this
/// program lies: this build's codegen identity (the compiler that compiles and runs the script,
/// and its answers to the script's RUSTC questions: rustc_answer, RUSTC_VERSION) and the std the
/// script compiles against (`std_stamp`), which a run takes from its tree or RELAX_STD, not from
/// its build. Read at each resolve that runs scripts, never kept for the process: a run that ends
/// in place and starts again after a std edit keys its scripts by the std it now compiles against
/// (the walk of ~400 entries is a millisecond; restart_statics).
pub fn script_tool() -> u128 {
    let mut h = crate::sip::Sip128::new();
    h.str("relax-script-tool");
    h.u128(crate::codegen_id::id());
    h.str(&std_stamp(&crate::std_dir()));
    h.finish128()
}

/// The std tree at `dir` as a key: its mount's commit when a repository serves it (vfs identity),
/// else, below it in order, each entry's path, kind, length and modification time (an edit and an
/// added, removed or replaced file change it: cargo's rule for a package's files, per file).
fn std_stamp(dir: &str) -> String {
    if let Some(id) = crate::vfs::identity(dir) {
        return format!("mount {id}");
    }
    fn walk(p: &std::path::Path, rel: &str, h: &mut crate::sip::Sip128) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut entries: Vec<std::fs::DirEntry> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = format!("{rel}/{}", e.file_name().to_string_lossy());
            let Ok(m) = e.path().symlink_metadata() else { continue };
            let time = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            h.str(&name);
            h.u8(if m.is_dir() { 1 } else if m.is_symlink() { 2 } else { 0 });
            h.u64(m.len());
            h.u128(time);
            if m.is_dir() {
                walk(&e.path(), &name, h);
            }
        }
    }
    let mut h = crate::sip::Sip128::new();
    h.str("relax-std-stamp");
    walk(std::path::Path::new(dir), "", &mut h);
    format!("disk {:032x}", h.finish128())
}
