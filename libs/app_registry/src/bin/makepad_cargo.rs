//! `makepad-cargo build|run|print <app>...`: cargo with the launchers' exact command and
//! environment (makepad_app_registry::cargo_command), for scripts such as a deploy build, so
//! what they compile is what the WM, Code, DECK and a terminal's `cargo run --release -p <pkg>`
//! find fresh.
//!
//! - `build <app>...`: `cargo build --release -p <pkg> --bin <bin>`, one cargo per app (one
//!   `-p a -p b` invocation would unify their features and build artifacts no launcher uses);
//! - `run <app> [-- args]`: `cargo run` of one app with its arguments;
//! - `print build|run <app>...`: the commands, one per line, as a shell would run them.
//!
//! `<app>` is a registry id (`stage`), a package (`makepad-app-stage`) or `makepad-run` (the
//! `makepad` tool). The checkout is the one the working directory is in.

use makepad_app_registry::{self as registry, AppTarget};
use std::path::Path;
use std::process::{Command, ExitCode};

fn target(name: &str) -> Option<AppTarget<'static>> {
    if name == registry::MAKEPAD_RUN.id || name == registry::MAKEPAD_RUN.package {
        return Some(registry::MAKEPAD_RUN);
    }
    registry::find(name).or_else(|| registry::apps().iter().find(|a| a.package == name)).map(|a| a.target())
}

fn quote(text: &str) -> String {
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:+@%".contains(c)) {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// `env -u A -u B K=V cargo args` for `command`.
fn shell_line(command: &Command) -> String {
    let mut line = vec!["env".to_string()];
    for (key, value) in command.get_envs() {
        match value {
            None => line.extend(["-u".to_string(), quote(&key.to_string_lossy())]),
            Some(value) => line.push(format!("{}={}", key.to_string_lossy(), quote(&value.to_string_lossy()))),
        }
    }
    line.push(quote(&command.get_program().to_string_lossy()));
    line.extend(command.get_args().map(|a| quote(&a.to_string_lossy())));
    line.join(" ")
}

fn command(verb: &str, app: &AppTarget, root: &Path, app_args: &[String]) -> Command {
    let (program, args) = if verb == "run" { registry::run_argv(app, root, app_args) } else { registry::build_argv(app, root) };
    registry::cargo_command(&program, &args, root)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (print, rest) = match args.first().map(String::as_str) {
        Some("print") => (true, &args[1..]),
        _ => (false, &args[..]),
    };
    let Some((verb, rest)) = rest.split_first().filter(|(v, _)| matches!(v.as_str(), "build" | "run")) else {
        eprintln!("usage: makepad-cargo [print] build <app>... | [print] run <app> [-- args]");
        return ExitCode::from(2);
    };
    let Some(root) = std::env::current_dir().ok().and_then(|cwd| registry::checkout_at_or_above(&cwd)) else {
        eprintln!("makepad-cargo: not inside a makepad checkout");
        return ExitCode::from(2);
    };
    let (names, app_args) = match rest.iter().position(|a| a == "--") {
        Some(i) => (&rest[..i], rest[i + 1..].to_vec()),
        None => (rest, Vec::new()),
    };
    if names.is_empty() || verb == "run" && names.len() != 1 {
        eprintln!("makepad-cargo: {verb} takes {}", if verb == "run" { "one app" } else { "one or more apps" });
        return ExitCode::from(2);
    }
    for name in names {
        let Some(app) = target(name) else {
            eprintln!("makepad-cargo: no app or package {name}");
            return ExitCode::from(2);
        };
        let mut cmd = command(verb, &app, &root, &app_args);
        if print {
            println!("{}", shell_line(&cmd));
            continue;
        }
        match cmd.status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("makepad-cargo: {verb} {name}: {status}");
                return ExitCode::from(status.code().unwrap_or(1).clamp(1, 255) as u8);
            }
            Err(e) => {
                eprintln!("makepad-cargo: cannot start cargo: {e}");
                return ExitCode::from(1);
            }
        }
    }
    ExitCode::SUCCESS
}
