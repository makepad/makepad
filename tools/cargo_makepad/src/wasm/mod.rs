mod compile;
mod sdk;
mod size_report;
use compile::WasmConfig;

fn enable_strip_pipeline(config: &mut WasmConfig) {
    config.strip = true;
    config.optimize_size = true;
}

fn enable_production_pipeline(config: &mut WasmConfig) {
    config.production = true;
    enable_strip_pipeline(config);
    config.brotli = true;
    config.wasm_opt = true;
}

fn enable_split_pipeline(config: &mut WasmConfig, threshold: Option<usize>) {
    config.split = true;
    config.split_auto = threshold.is_none();
    if let Some(threshold) = threshold {
        config.split_functions_threshold = threshold;
    }
}

fn parse_wasm_option(config: &mut WasmConfig, v: &str) -> bool {
    if let Some(opt) = v.strip_prefix("--port=") {
        config.port = Some(opt.parse::<u16>().unwrap_or(8010));
        true
    } else if v == "--strip-custom-sections" {
        config.strip = true;
        true
    } else if v == "--strip" {
        enable_strip_pipeline(config);
        true
    } else if v == "--production" {
        enable_production_pipeline(config);
        true
    } else if v == "--lto" {
        config.lto = true;
        true
    } else if v == "--wasm-opt" {
        config.wasm_opt = true;
        true
    } else if v == "--no-location-detail" {
        config.no_location_detail = true;
        true
    } else if v == "--size-report" {
        config.size_report = true;
        true
    } else if v == "--keep-names" {
        config.keep_names = true;
        true
    } else if v == "--split" {
        enable_split_pipeline(config, None);
        true
    } else if let Some(threshold) = v.strip_prefix("--split=") {
        enable_split_pipeline(config, Some(threshold.parse::<usize>().unwrap_or(200)));
        true
    } else if v == "--small-fonts" {
        eprintln!(
            "warning: --small-fonts is ignored; the application's makepad.font-assets.v1 manifest selects fonts"
        );
        true
    } else if v == "--brotli" {
        config.brotli = true;
        true
    } else if v == "--lan" {
        config.lan = true;
        true
    } else if v == "--bindgen" {
        config.bindgen = true;
        true
    } else if v == "--no-threads" {
        config.threads = false;
        true
    } else if v == "--split-functions" {
        config.split_functions = true;
        true
    } else if let Some(threshold) = v.strip_prefix("--split-functions=") {
        config.split_functions = true;
        config.split_functions_threshold = threshold.parse::<usize>().unwrap_or(200);
        true
    } else {
        false
    }
}

fn apply_production_profile(config: &WasmConfig, args: Vec<String>) -> Vec<String> {
    if !config.production {
        return args;
    }

    let mut out = Vec::with_capacity(args.len() + 1);
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--profile" {
            index += 2;
            continue;
        }
        if arg == "--release" || arg.starts_with("--profile=") {
            index += 1;
            continue;
        }
        out.push(arg.clone());
        index += 1;
    }
    if config.lto && config.threads {
        out.push("--profile=small".to_string());
    } else {
        out.push("--release".to_string());
    }
    out
}

fn strip_wasm_options(config: &mut WasmConfig, args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for v in args {
        if !parse_wasm_option(config, v) {
            out.push(v.clone());
        }
    }
    out
}

pub fn handle_wasm(mut args: &[String]) -> Result<(), String> {
    let mut config = WasmConfig {
        strip: false,
        lan: false,
        brotli: false,
        port: None,
        bindgen: false,
        threads: true,
        optimize_size: false,
        wasm_opt: false,
        production: false,
        lto: false,
        no_location_detail: false,
        size_report: false,
        keep_names: false,
        split: false,
        split_auto: false,
        split_functions: false,
        split_functions_threshold: 200,
        hot_reload: false,
    };

    // pull out options
    for i in 0..args.len() {
        let v = &args[i];
        if !parse_wasm_option(&mut config, v) {
            args = &args[i..];
            break;
        }
    }

    match args[0].as_ref() {
        "rustup-install-toolchain" => sdk::rustup_toolchain_install(),
        "install-toolchain" => sdk::rustup_toolchain_install(),
        "build" => {
            let build_args = strip_wasm_options(&mut config, &args[1..]);
            let build_args = apply_production_profile(&config, build_args);
            compile::build(config, &build_args)?;
            Ok(())
        }
        "run" => {
            let run_args = strip_wasm_options(&mut config, &args[1..]);
            let run_args = apply_production_profile(&config, run_args);
            compile::run(config, &run_args)?;
            Ok(())
        }
        _ => Err(format!("{} is not a valid command or option", args[0])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn default_config() -> WasmConfig {
        WasmConfig {
            strip: false,
            lan: false,
            brotli: false,
            port: None,
            bindgen: false,
            threads: true,
            optimize_size: false,
            wasm_opt: false,
            production: false,
            lto: false,
            no_location_detail: false,
            size_report: false,
            keep_names: false,
            split: false,
            split_auto: false,
            split_functions: false,
            split_functions_threshold: 200,
            hot_reload: false,
        }
    }

    #[test]
    fn delivery_options_are_consumed_without_a_font_policy_side_effect() {
        for option in ["--strip", "--brotli", "--split", "--no-threads"] {
            let mut config = default_config();
            let cargo_args = strip_wasm_options(
                &mut config,
                &args(&[option, "--release", "-p", "app"]),
            );
            assert_eq!(cargo_args, vec!["--release", "-p", "app"]);
        }
    }

    #[test]
    fn deprecated_small_fonts_flag_is_consumed_and_ignored() {
        let mut config = default_config();
        let cargo_args = strip_wasm_options(
            &mut config,
            &args(&["--small-fonts", "--release", "-p", "app"]),
        );
        assert_eq!(cargo_args, vec!["--release", "-p", "app"]);
    }

    #[test]
    fn production_and_reporting_flags_are_parsed_and_not_forwarded() {
        let mut config = default_config();
        let cargo_args = strip_wasm_options(
            &mut config,
            &args(&[
                "--production",
                "--no-location-detail",
                "--size-report",
                "--keep-names",
                "--profile=small",
                "-p",
                "app",
            ]),
        );
        let cargo_args = apply_production_profile(&config, cargo_args);

        assert!(config.production);
        assert!(config.strip);
        assert!(config.optimize_size);
        assert!(config.brotli);
        assert!(config.wasm_opt);
        assert!(config.no_location_detail);
        assert!(config.size_report);
        assert!(config.keep_names);
        assert!(!config.lto);
        assert_eq!(cargo_args, vec!["-p", "app", "--release"]);
    }

    #[test]
    fn production_lto_opts_into_small_profile() {
        let mut config = default_config();
        let cargo_args = strip_wasm_options(
            &mut config,
            &args(&["--production", "--lto", "--release", "-p", "app"]),
        );

        assert_eq!(
            apply_production_profile(&config, cargo_args),
            vec!["-p", "app", "--profile=small"]
        );
        assert!(config.lto);
    }

    #[test]
    fn production_lto_does_not_force_small_profile_without_threads() {
        let mut config = default_config();
        let cargo_args = strip_wasm_options(
            &mut config,
            &args(&[
                "--production",
                "--lto",
                "--no-threads",
                "--profile=small",
                "-p",
                "app",
            ]),
        );

        assert_eq!(
            apply_production_profile(&config, cargo_args),
            vec!["-p", "app", "--release"]
        );
        assert!(!config.threads);
        assert!(config.lto);
    }
}
