//! Read-only discovery of an explicitly installed CEF distribution.
use std::path::{Path, PathBuf};

pub(crate) fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macosarm64"),
        ("macos", "x86_64") => Some("macosx64"),
        ("linux", "x86_64") => Some("linux64"),
        ("linux", "aarch64") => Some("linuxarm64"),
        ("windows", "x86_64") => Some("windows64"),
        ("windows", "aarch64") => Some("windowsarm64"),
        _ => None,
    }
}

fn pointer(prebuilt: &Path, platform: &str) -> Option<PathBuf> {
    if let Ok(text) = std::fs::read_to_string(prebuilt.join(format!("current-{platform}.txt"))) {
        let name = text.lines().next().unwrap_or("").trim();
        if !name.is_empty() {
            let dist = prebuilt.join(name);
            if dist.join("include").is_dir() {
                return Some(dist);
            }
        }
    }
    // Keep accepting the older symlink/directory pointer.
    let dist = prebuilt.join(format!("current-{platform}"));
    dist.join("include").is_dir().then_some(dist)
}

fn from_ancestors(start: &Path, platform: &str) -> Option<PathBuf> {
    for root in start.ancestors() {
        if let Some(dist) = pointer(&root.join("local/cef-prebuilt"), platform) {
            return Some(dist);
        }
        // Worktrees share the main checkout's installed distribution. Resolve
        // relative gitdir paths against the directory containing the .git file.
        if let Ok(text) = std::fs::read_to_string(root.join(".git")) {
            if let Some(gitdir) = text.trim().strip_prefix("gitdir:") {
                if let Ok(gitdir) = root.join(gitdir.trim()).canonicalize() {
                    if let Some(main) = gitdir
                        .ancestors()
                        .find(|path| path.file_name().is_some_and(|name| name == ".git"))
                        .and_then(Path::parent)
                    {
                        if let Some(dist) = pointer(&main.join("local/cef-prebuilt"), platform) {
                            return Some(dist);
                        }
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn find() -> Result<PathBuf, String> {
    let platform = platform().ok_or("CEF distribution is unsupported on this platform")?;
    let hint = format!(
        "Set MAKEPAD_CEF_DIST_DIR to an extracted distribution, or fetch explicitly with \
         cargo run --release -p makepad-cef-fetch -- --fetch --platform {platform}"
    );
    let dist = if let Some(dir) = std::env::var_os("MAKEPAD_CEF_DIST_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.join("include").is_dir() {
            return Err(format!(
                "{} is not a CEF distribution (missing include/). {hint}",
                dir.display()
            ));
        }
        dir
    } else {
        // Prefer the executable's installation when moved to another checkout.
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().and_then(|dir| from_ancestors(dir, platform)))
            .or_else(|| from_ancestors(Path::new(env!("CARGO_MANIFEST_DIR")), platform))
            .ok_or_else(|| format!("No installed CEF distribution for {platform}. {hint}"))?
    };
    dist.canonicalize()
        .map_err(|err| format!("Cannot resolve {}: {err}. {hint}", dist.display()))
}

pub(crate) fn version() -> Option<String> {
    let header = std::fs::read_to_string(find().ok()?.join("include/cef_version.h")).ok()?;
    header.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next()? == "#define" && fields.next()? == "CEF_VERSION").then(|| {
            fields
                .next()
                .map(|value| value.trim_matches('"').to_owned())
        })?
    })
}
