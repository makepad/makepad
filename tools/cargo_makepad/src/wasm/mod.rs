mod compile;
mod sdk;
use compile::WasmConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WasmCommand {
    InstallToolchain,
    Build,
    Run,
}

fn enable_strip_pipeline(config: &mut WasmConfig) {
    config.strip = true;
    config.optimize_size = true;
}

fn enable_split_pipeline(config: &mut WasmConfig, threshold: Option<usize>) {
    config.split = true;
    config.split_auto = threshold.is_none();
    if let Some(threshold) = threshold {
        config.split_functions_threshold = threshold;
    }
}

fn removed_wasm_option_error(v: &str) -> Option<String> {
    match v {
        "--ship" => Some(
            "`--ship` has been removed; use explicit flags like `--profile=small --strip --brotli --split --no-threads` with `cargo makepad wasm build`.".to_string(),
        ),
        "--serve" => Some(
            "`--serve` has been removed; `cargo makepad wasm run` always serves and `cargo makepad wasm build` never serves.".to_string(),
        ),
        "--small-fonts" => Some(
            "`--small-fonts` has been removed from the wasm CLI.".to_string(),
        ),
        "--full-fonts" => Some(
            "`--full-fonts` has been removed from the wasm CLI.".to_string(),
        ),
        "--threads" => Some(
            "`--threads` has been removed; threaded wasm remains the default, so omit the flag.".to_string(),
        ),
        "--no-split" => Some(
            "`--no-split` has been removed; omit `--split` to leave split output disabled.".to_string(),
        ),
        "--no-brotli" => Some(
            "`--no-brotli` has been removed; omit `--brotli` to leave Brotli disabled.".to_string(),
        ),
        _ => None,
    }
}

fn build_uses_package_layout(config: &WasmConfig) -> bool {
    config.optimize_size || config.brotli || config.split || !config.threads
}

fn apply_packaged_web_defaults(config: &mut WasmConfig) {
    if config.package_layout && !config.full_fonts {
        config.small_fonts = true;
    }
}

fn parse_wasm_option(config: &mut WasmConfig, v: &str) -> Result<bool, String> {
    if let Some(error) = removed_wasm_option_error(v) {
        Err(error)
    } else if let Some(opt) = v.strip_prefix("--port=") {
        config.port = Some(opt.parse::<u16>().unwrap_or(8010));
        Ok(true)
    } else if v == "--strip-custom-sections" {
        config.strip = true;
        Ok(true)
    } else if v == "--strip" {
        enable_strip_pipeline(config);
        Ok(true)
    } else if v == "--wasm-opt" {
        config.wasm_opt = true;
        Ok(true)
    } else if v == "--split" {
        enable_split_pipeline(config, None);
        Ok(true)
    } else if let Some(threshold) = v.strip_prefix("--split=") {
        enable_split_pipeline(config, Some(threshold.parse::<usize>().unwrap_or(200)));
        Ok(true)
    } else if v == "--brotli" {
        config.brotli = true;
        Ok(true)
    } else if v == "--lan" {
        config.lan = true;
        Ok(true)
    } else if v == "--bindgen" {
        config.bindgen = true;
        Ok(true)
    } else if v == "--no-threads" {
        config.threads = false;
        Ok(true)
    } else if v == "--split-functions" {
        config.split_functions = true;
        Ok(true)
    } else if let Some(threshold) = v.strip_prefix("--split-functions=") {
        config.split_functions = true;
        config.split_functions_threshold = threshold.parse::<usize>().unwrap_or(200);
        Ok(true)
    } else {
        Ok(false)
    }
}

fn strip_wasm_options(config: &mut WasmConfig, args: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for v in args {
        if !parse_wasm_option(config, v)? {
            out.push(v.clone());
        }
    }
    Ok(out)
}

fn parse_wasm_command(
    mut args: &[String],
) -> Result<(WasmCommand, WasmConfig, Vec<String>), String> {
    let mut config = WasmConfig {
        strip: false,
        lan: false,
        brotli: false,
        port: None,
        small_fonts: false,
        bindgen: false,
        threads: true,
        optimize_size: false,
        wasm_opt: false,
        split: false,
        split_auto: false,
        split_functions: false,
        split_functions_threshold: 200,
        hot_reload: false,
        package_layout: false,
        full_fonts: false,
    };

    // pull out options
    for i in 0..args.len() {
        let v = &args[i];
        if !parse_wasm_option(&mut config, v)? {
            args = &args[i..];
            break;
        }
    }

    if args.is_empty() {
        return Err("missing wasm command".to_string());
    }

    match args[0].as_ref() {
        "rustup-install-toolchain" | "install-toolchain" => {
            Ok((WasmCommand::InstallToolchain, config, Vec::new()))
        }
        "build" => {
            let build_args = strip_wasm_options(&mut config, &args[1..])?;
            config.package_layout = build_uses_package_layout(&config);
            apply_packaged_web_defaults(&mut config);
            Ok((WasmCommand::Build, config, build_args))
        }
        "run" => {
            let run_args = strip_wasm_options(&mut config, &args[1..])?;
            config.hot_reload = true;
            config.package_layout = false;
            Ok((WasmCommand::Run, config, run_args))
        }
        "ship" => Err(
            "`cargo makepad wasm ship` has been removed; use `cargo makepad wasm build` with explicit flags or `cargo makepad wasm run` for the dev server.".to_string(),
        ),
        _ => Err(format!("{} is not a valid command or option", args[0])),
    }
}

pub fn handle_wasm(args: &[String]) -> Result<(), String> {
    let (command, config, command_args) = parse_wasm_command(args)?;
    match command {
        WasmCommand::InstallToolchain => sdk::rustup_toolchain_install(),
        WasmCommand::Build => {
            compile::build(config, &command_args)?;
            Ok(())
        }
        WasmCommand::Run => {
            compile::run(config, &command_args)?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<(WasmCommand, WasmConfig, Vec<String>), String> {
        let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        parse_wasm_command(&args)
    }

    #[test]
    fn build_without_packaging_flags_keeps_plain_layout() {
        let (command, config, command_args) =
            parse(&["build", "-p", "makepad-example-splash"]).unwrap();
        assert_eq!(command, WasmCommand::Build);
        assert!(!config.package_layout);
        assert!(!config.small_fonts);
        assert_eq!(command_args, vec!["-p", "makepad-example-splash"]);
    }

    #[test]
    fn build_with_packaging_flags_uses_package_layout() {
        for args in [
            vec!["build", "--strip", "-p", "app"],
            vec!["build", "--brotli", "-p", "app"],
            vec!["build", "--split", "-p", "app"],
            vec!["build", "--no-threads", "-p", "app"],
        ] {
            let (command, config, _) = parse(&args).unwrap();
            assert_eq!(command, WasmCommand::Build);
            assert!(
                config.package_layout,
                "expected package layout for {:?}",
                args
            );
            assert!(
                config.small_fonts,
                "expected packaged font defaults for {:?}",
                args
            );
        }
    }

    #[test]
    fn build_with_profile_only_does_not_use_package_layout() {
        let (command, config, command_args) =
            parse(&["build", "-p", "app", "--profile=small"]).unwrap();
        assert_eq!(command, WasmCommand::Build);
        assert!(!config.package_layout);
        assert!(!config.small_fonts);
        assert_eq!(command_args, vec!["-p", "app", "--profile=small"]);
    }

    #[test]
    fn build_with_strip_custom_sections_only_keeps_plain_layout() {
        let (command, config, command_args) =
            parse(&["build", "--strip-custom-sections", "-p", "app"]).unwrap();
        assert_eq!(command, WasmCommand::Build);
        assert!(!config.package_layout);
        assert!(config.strip);
        assert!(!config.optimize_size);
        assert!(!config.small_fonts);
        assert_eq!(command_args, vec!["-p", "app"]);
    }

    #[test]
    fn run_always_stays_on_dev_server_path() {
        let (command, config, _) =
            parse(&["run", "--strip", "--split", "--no-threads", "-p", "app"]).unwrap();
        assert_eq!(command, WasmCommand::Run);
        assert!(config.hot_reload);
        assert!(!config.package_layout);
        assert!(!config.small_fonts);
        assert!(!config.threads);
        assert!(config.optimize_size);
        assert!(config.split);
    }

    #[test]
    fn removed_ship_command_errors_with_replacement_hint() {
        let error = parse(&["ship", "-p", "app"]).unwrap_err();
        assert!(error.contains("wasm ship"));
        assert!(error.contains("wasm build"));
    }

    #[test]
    fn removed_wasm_flags_error_with_hints() {
        for removed in [
            "--ship",
            "--serve",
            "--small-fonts",
            "--full-fonts",
            "--threads",
            "--no-split",
            "--no-brotli",
        ] {
            let error = parse(&["build", removed, "-p", "app"]).unwrap_err();
            assert!(
                error.contains(removed),
                "missing removed flag hint for {removed}"
            );
        }
    }
}
