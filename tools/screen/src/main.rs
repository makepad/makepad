#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn main() {
    if let Err(error) = native_main() {
        eprintln!("makepad-screen: {error}");
        std::process::exit(1);
    }
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn main() {
    eprintln!("makepad-screen supports macOS, Linux and Windows");
    std::process::exit(2);
}
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn native_main() -> Result<(), String> {
    use makepad_screen::{
        client,
        protocol::{valid_session, SessionLocation},
        server::{self, StartOptions},
    };
    use std::{collections::BTreeSet, ffi::OsString, path::PathBuf};
    let mut args = std::env::args_os().skip(1).collect::<Vec<_>>().into_iter();
    let operation = args.next().unwrap_or_else(|| "agents".into());
    // tools/agents forwards through the browser operation; allow its name subcommand.
    let operation = if operation == "agents" {
        let mut remaining = args.clone();
        if remaining.next().as_deref() == Some(std::ffi::OsStr::new("name")) {
            args.next();
            OsString::from("name")
        } else {
            operation
        }
    } else {
        operation
    };
    let operation = operation.to_str().ok_or("Screen operation must be UTF-8")?;
    if operation == "--version" {
        if args.next().is_some() {
            return Err("--version takes no other arguments".into());
        }
        println!("makepad-screen {} (protocol 1)", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if ![
        "agents", "tui", "start", "serve", "attach", "status", "list", "stop", "name",
    ]
    .contains(&operation)
    {
        return Err("Unknown screen operation".into());
    }
    let mut name_parts = Vec::new();
    let mut state_dir = None;
    let mut session = None;
    let mut cwd = None;
    let mut cols = 120u16;
    let mut rows = 40u16;
    let mut read_only = false;
    let mut restart = false;
    let mut attach = false;
    let mut theme = makepad_screen::theme::Theme::default();
    let mut instance = None;
    let mut lock_fd = None;
    let mut command = Vec::<OsString>::new();
    let mut seen = BTreeSet::new();
    while let Some(flag) = args.next() {
        if operation == "name" && !flag.to_string_lossy().starts_with("--") {
            name_parts.push(flag.into_string().map_err(|_| "Name must be UTF-8")?);
            continue;
        }
        if flag == "--" {
            command.extend(args);
            break;
        }
        let flag = flag.to_str().ok_or("Invalid screen option")?;
        if !seen.insert(flag.to_owned()) {
            return Err(format!("Duplicate screen option {flag}"));
        }
        match flag {
            "--state-dir" => {
                state_dir = Some(PathBuf::from(
                    args.next().ok_or("--state-dir requires a path")?,
                ))
            }
            "--session" => {
                session = Some(
                    args.next()
                        .ok_or("--session requires an ID")?
                        .into_string()
                        .map_err(|_| "Session ID must be ASCII")?,
                )
            }
            "--cwd" if matches!(operation, "agents" | "tui" | "start" | "serve") => {
                cwd = Some(PathBuf::from(args.next().ok_or("--cwd requires a path")?))
            }
            "--cols" | "--rows" if matches!(operation, "start" | "serve") => {
                let value = args
                    .next()
                    .ok_or("Dimension requires a value")?
                    .into_string()
                    .map_err(|_| "Dimension must be numeric")?
                    .parse::<u16>()
                    .map_err(|_| "Dimension must be numeric")?;
                if flag == "--cols" {
                    cols = value;
                } else {
                    rows = value;
                }
            }
            "--read-only" if operation == "attach" => read_only = true,
            "--restart" if operation == "start" => restart = true,
            "--attach" if operation == "start" => attach = true,
            "--theme" if operation == "serve" => {
                theme = makepad_screen::theme::Theme::decode(
                    &args
                        .next()
                        .ok_or("--theme requires colors")?
                        .into_string()
                        .map_err(|_| "Invalid theme")?,
                )?;
            }
            "--foreground" | "--background" if operation == "start" => {
                let value = args
                    .next()
                    .ok_or("Color requires an RGB value")?
                    .into_string()
                    .map_err(|_| "Invalid color")?;
                let rgb = makepad_screen::term::color::parse_color_spec(&value)
                    .ok_or("Invalid RGB color")?;
                theme.colors[if flag == "--foreground" { 256 } else { 257 }] = Some(rgb);
            }
            "--instance" if operation == "serve" => {
                instance = Some(
                    args.next()
                        .ok_or("Internal serve requires an instance")?
                        .into_string()
                        .map_err(|_| "Invalid instance")?,
                )
            }
            "--lock-fd" if operation == "serve" => {
                lock_fd = Some(
                    args.next()
                        .ok_or("Internal serve requires a lock descriptor")?
                        .into_string()
                        .map_err(|_| "Invalid lock descriptor")?
                        .parse::<i32>()
                        .map_err(|_| "Invalid lock descriptor")?,
                )
            }
            _ => return Err(format!("Unsupported screen option {flag} for {operation}")),
        }
    }
    if matches!(operation, "agents" | "tui") {
        if session.is_some() || !command.is_empty() {
            return Err("agents accepts only --state-dir and --cwd".into());
        }
        return makepad_screen::launcher::run(state_dir, cwd);
    }
    let state_dir = if operation == "list" {
        Some(makepad_screen::launcher::resolve_state_dir(
            state_dir,
            &cwd.clone()
                .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?),
        )?)
    } else {
        state_dir
    };
    let state_dir = state_dir
        .or_else(|| std::env::var_os("MAKEPAD_SCREEN_STATE_DIR").map(PathBuf::from))
        .ok_or("--state-dir is required (or run inside a makepad-screen session)")?;
    if !state_dir.is_absolute() {
        return Err("--state-dir must be absolute".into());
    }
    if operation == "list" {
        if session.is_some() || !command.is_empty() {
            return Err("list accepts only --state-dir".into());
        }
        println!("{}", server::list(&state_dir)?.to_json());
        return Ok(());
    }
    let session = session
        .or_else(|| std::env::var("MAKEPAD_SCREEN_SESSION").ok())
        .ok_or("--session is required (or run inside a makepad-screen session)")?;
    if !valid_session(&session) {
        return Err(
            "Session ID must be 1..48 ASCII letters, digits, hyphens or underscores".into(),
        );
    }
    if operation == "name" {
        println!(
            "{}",
            server::name(
                &state_dir,
                &session,
                &name_parts
                    .into_iter()
                    .chain(
                        command
                            .into_iter()
                            .map(|v| v.to_string_lossy().into_owned())
                    )
                    .collect::<Vec<_>>()
                    .join(" ")
            )?
            .to_json()
        );
        return Ok(());
    }
    match operation {
        "start" | "serve" => {
            if command.is_empty() {
                return Err("start requires -- PROGRAM ARGS...".into());
            }
            let options = StartOptions {
                state_dir,
                session_id: session,
                cwd: cwd.ok_or("--cwd is required")?,
                program: command.remove(0),
                args: command,
                cols,
                rows,
                theme,
            };
            if operation == "start" {
                if attach {
                    client::start_attached(options, restart)?;
                } else {
                    println!("{}", server::start(options, restart)?.to_json());
                }
            } else {
                server::serve(
                    options,
                    &instance.ok_or("serve is internal and requires a claimed instance")?,
                    lock_fd.ok_or("serve is internal and requires an inherited lock")?,
                )?;
            }
        }
        "attach" | "status" | "stop" => {
            if !command.is_empty() {
                return Err("Only start accepts a command".into());
            }
            match operation {
                "attach" => client::attach(
                    &SessionLocation::open(&state_dir, &session, false)?,
                    read_only,
                )?,
                "status" => println!("{}", server::status(&state_dir, &session)?.to_json()),
                "stop" => println!("{}", server::stop(&state_dir, &session)?.to_json()),
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}
