mod compile;
mod sdk;
mod size_report;
use compile::WasmConfig;

fn should_default_to_small_fonts(config: &WasmConfig) -> bool {
    config.optimize_size || config.brotli || config.split || !config.threads
}

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
        config.small_fonts = true;
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
    if !config.production || !config.threads {
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
    out.push("--profile=small".to_string());
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
        small_fonts: false,
        bindgen: false,
        threads: true,
        optimize_size: false,
        wasm_opt: false,
        production: false,
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
            if should_default_to_small_fonts(&config) && !config.small_fonts {
                config.small_fonts = true;
            }
            compile::build(config, &build_args)?;
            Ok(())
        }
        "run" => {
            let run_args = strip_wasm_options(&mut config, &args[1..]);
            let run_args = apply_production_profile(&config, run_args);
            if should_default_to_small_fonts(&config) && !config.small_fonts {
                config.small_fonts = true;
            }
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

    #[test]
    fn packaged_build_defaults_to_small_fonts() {
        for args in [
            args(&["build", "--strip", "-p", "app"]),
            args(&["build", "--brotli", "-p", "app"]),
            args(&["build", "--split", "-p", "app"]),
            args(&["build", "--no-threads", "-p", "app"]),
        ] {
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
                production: false,
                no_location_detail: false,
                size_report: false,
                keep_names: false,
                split: false,
                split_auto: false,
                split_functions: false,
                split_functions_threshold: 200,
                hot_reload: false,
            };

            let build_args = strip_wasm_options(&mut config, &args[1..]);
            assert!(!build_args.is_empty());
            if should_default_to_small_fonts(&config) && !config.small_fonts {
                config.small_fonts = true;
            }
            assert!(config.small_fonts, "expected small fonts for {:?}", args);
        }
    }

    #[test]
    fn packaged_run_defaults_to_small_fonts() {
        for args in [
            args(&["run", "--strip", "-p", "app"]),
            args(&["run", "--brotli", "-p", "app"]),
            args(&["run", "--split", "-p", "app"]),
            args(&["run", "--no-threads", "-p", "app"]),
        ] {
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
                production: false,
                no_location_detail: false,
                size_report: false,
                keep_names: false,
                split: false,
                split_auto: false,
                split_functions: false,
                split_functions_threshold: 200,
                hot_reload: false,
            };

            let run_args = strip_wasm_options(&mut config, &args[1..]);
            assert!(!run_args.is_empty());
            if should_default_to_small_fonts(&config) && !config.small_fonts {
                config.small_fonts = true;
            }
            assert!(config.small_fonts, "expected small fonts for {:?}", args);
        }
    }

    #[test]
    fn profile_small_alone_keeps_full_fonts() {
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
            production: false,
            no_location_detail: false,
            size_report: false,
            keep_names: false,
            split: false,
            split_auto: false,
            split_functions: false,
            split_functions_threshold: 200,
            hot_reload: false,
        };

        let build_args = strip_wasm_options(&mut config, &args(&["-p", "app", "--profile=small"]));
        assert_eq!(build_args, vec!["-p", "app", "--profile=small"]);
        if should_default_to_small_fonts(&config) && !config.small_fonts {
            config.small_fonts = true;
        }
        assert!(!config.small_fonts);
    }

    #[test]
    fn production_and_reporting_flags_are_parsed_and_not_forwarded() {
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
            production: false,
            no_location_detail: false,
            size_report: false,
            keep_names: false,
            split: false,
            split_auto: false,
            split_functions: false,
            split_functions_threshold: 200,
            hot_reload: false,
        };
        let cargo_args = strip_wasm_options(
            &mut config,
            &args(&[
                "--production",
                "--no-location-detail",
                "--size-report",
                "--keep-names",
                "--release",
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
        assert_eq!(cargo_args, vec!["-p", "app", "--profile=small"]);
    }

    #[test]
    fn production_does_not_force_small_profile_without_threads() {
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
            production: false,
            no_location_detail: false,
            size_report: false,
            keep_names: false,
            split: false,
            split_auto: false,
            split_functions: false,
            split_functions_threshold: 200,
            hot_reload: false,
        };
        let cargo_args = strip_wasm_options(
            &mut config,
            &args(&["--production", "--no-threads", "--release", "-p", "app"]),
        );

        assert_eq!(
            apply_production_profile(&config, cargo_args),
            vec!["--release", "-p", "app"]
        );
        assert!(!config.threads);
    }
}
