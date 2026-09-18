//! Session-only instructions for agents editing a portable source installation.
use crate::{catalog::Release, runtime::Environment};
use std::{fs, path::PathBuf, process::Command};

const DIRECTIVE: &str = "Before editing read the installation instructions in the file named by the MAKEPAD_AGENT_CONTEXT environment variable. Follow those paths and rebuild instructions for this portable Makepad application. Preserve the existing repository instructions and wait for the user to choose a change.";

pub fn write_context(release: &Release, environment: &Environment) -> Result<PathBuf, String> {
    let context = environment.root.join("agent-context.txt");
    let root = environment.root.display();
    let repositories: String = release.repositories.iter().map(|r| format!("{} repository: {}\n", r.name, release.directory(&environment.root).join(&r.path).display())).collect();
    fs::write(&context, format!(
        "You are helping customize {} in a portable compile-on-device installation.\n\
        Installation root for this session: {root}\n\
        App Cargo workspace: {}\n{repositories}\
        Package: {}. Binary: {}. Pinned Rust: {}.\n\
        The source repositories are separate shallow clones. Read their AGENTS.md and existing widget/Splash examples before editing. Preserve local edits. Do not fetch a fresh release or reset either repository to rebuild.\n\
        This process already has the selected Rust toolchain, a private Cargo home, build directory, Microsoft SDK include/library paths and bundled LLD linker in its environment. Use these; do not install another compiler, change global PATH, or run rustup.\n\
        Check from the app Cargo workspace with cargo check -p {} and run the relevant existing tests.\n\
        Rebuild and publish the edited app using Makepad Builder beside the executable: makepad-builder.exe rebuild on Windows. Invoke it with PowerShell Start-Process -Wait -PassThru and inspect ExitCode; this command is a GUI-subsystem executable. On macOS/Linux use INSTALL_ROOT/makepad-builder rebuild. Use the absolute installation root shown above for these commands. No download or license credential is needed for a rebuild.\n\
        The rebuild command runs cargo build --release (with --locked after the first build), sets MAKEPAD_PACKAGE_DIR=., refreshes makepad-package-paths, and places the executable in the installation root (scope.exe on Windows, scope.bin beside the scope shell command on Unix). Cargo output remains under {}.\n\
        Fonts, SVGs, icons, images and other crate resources stay in the downloaded source tree. makepad-package-paths maps actual Cargo artifact crate names to source directories relative to the executable. Never hardcode the installation root or copy resources elsewhere.\n\
        Launch {} --cwd PROJECT_DIRECTORY --remote to verify on the native GPU. Keep the same project/state across rebuilds, gracefully close only your own app before replacing its executable, and use its app remote capture/quit routes for inspection. Do not use a simulated GPU or display screenshots.\n\
        The entire install directory can move. Reopen the agent through Makepad Builder after moving it to refresh paths. Do not read or expose makepad-builder.json, email credentials, unrelated user files or authentication storage.\n",
        release.title, environment.cwd.display(), release.package, release.binary, release.rust,
        release.package, environment.build.display(), environment.app_binary(release).display(),
    )).map_err(|e| e.to_string())?;
    Ok(context)
}

pub fn command(name: &str, release: &Release, environment: &Environment) -> Result<Command, String> {
    let context = write_context(release, environment)?;
    let arguments = match name {
        "codex" => vec!["-c".to_owned(), format!("developer_instructions='{}'", DIRECTIVE)],
        "claude" => vec!["--append-system-prompt".to_owned(), DIRECTIVE.to_owned()],
        _ => return Err("Unknown app agent".into()),
    };
    let mut command = if cfg!(windows) {
        // Only fixed agent names and our constant instruction enter CMD; paths
        // and credentials never do. This also supports installed npm .cmd shims.
        let mut command = Command::new(std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()));
        command.args(["/d", "/v:off", "/s", "/c"]);
        let args = arguments.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" ");
        command.arg(format!("{name} {args}"));
        command
    } else {
        let mut command = Command::new(name); command.args(arguments); command
    };
    command.current_dir(&environment.cwd).envs(&environment.vars)
        .env("MAKEPAD_AGENT_CONTEXT", context)
        .env_remove("MAKEPAD_LOADER_EMAIL").env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTC_WRAPPER").env_remove("RUSTC_WORKSPACE_WRAPPER");
    Ok(command)
}
