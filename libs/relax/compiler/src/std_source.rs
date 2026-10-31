//! Select the binary's std from a git mount only when every path and byte matches the build.

pub const DIGEST: &str = env!("MAKEPAD_RUN_STD_DIGEST");
static SELECTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Automatic choices are marked so a runner's child can select against its own repositories.
/// An explicit MAKEPAD_RUN_STD (including an empty one) always wins.
pub fn select(workspace: &str, fallback: &std::path::Path) {
    let current = std::env::var_os("MAKEPAD_RUN_STD");
    if current.is_some() && current != std::env::var_os("MAKEPAD_RUN_STD_AUTO") {
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
        let expected = if std::env::var_os("MAKEPAD_RUN_TEST_STD_MISMATCH").is_some() {
            "test-mismatch"
        } else {
            DIGEST
        };
        let result = crate::vfs::verify_std(&candidate, expected);
        match result {
            Ok(()) => std::path::PathBuf::from(candidate),
            Err(reason) => {
                eprintln!("makepad-run: using disk std {}: {reason}", fallback.display());
                fallback.to_path_buf()
            }
        }
    } else {
        fallback.to_path_buf()
    };
    std::env::set_var("MAKEPAD_RUN_STD", &chosen);
    std::env::set_var("MAKEPAD_RUN_STD_AUTO", &chosen);
}
