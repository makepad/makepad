//! Build scripts use this loader as both CARGO and RUSTC. The initial script launch consumes
//! its role; later calls are rustc's option-led probes or cargo's subcommands, never loader runs.
//! (The script's own option-led RUSTC questions are answered in its process, relax_compiler
//! tool.rs, and start no loader; a probe through a wrapper the script runs still comes here.)

pub fn dispatch(raw_args: &[Vec<u8>]) -> bool {
    match std::env::var("RELAX_AS_RUSTC").as_deref() {
        Ok("build-script") => {
            std::env::set_var("RELAX_AS_RUSTC", "rustc,cargo");
            return false;
        }
        Ok("rustc,cargo") => {}
        _ => return false,
    }
    let args: Vec<String> = raw_args.iter().skip(1).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    if args.first().is_some_and(|a| a.starts_with('-')) {
        return relax_compiler::relax_as_rustc();
    }
    let result = (|| {
        if args.first().map(String::as_str) != Some("metadata") {
            return Err(format!("unsupported subcommand {:?}", args.first().map(String::as_str).unwrap_or("<missing>")));
        }
        // The native loader has no virtual cwd until the inherited mounts are installed.
        relax_compiler::vfs::mount_from_env()?;
        let dir = match std::env::var("RELAX_WORKSPACE").ok().filter(|d| !d.is_empty()) {
            Some(d) => d,
            None => std::env::current_dir().map_err(|e| e.to_string())?.display().to_string(),
        };
        relax_compiler::resolve::cargo_metadata(&relax_compiler::session_from_env(&[]), &dir, &args[1..])
    })();
    match result {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("relax: cargo: {}", e.lines().collect::<Vec<_>>().join(" "));
            std::process::exit(1);
        }
    }
    true
}
