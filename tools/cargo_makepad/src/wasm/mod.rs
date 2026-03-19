mod compile;
mod sdk;
use compile::WasmConfig;

fn should_default_to_small_fonts(config: &WasmConfig) -> bool {
    config.optimize_size || config.brotli || config.split || !config.threads
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
    } else if v == "--wasm-opt" {
        config.wasm_opt = true;
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
            if should_default_to_small_fonts(&config) && !config.small_fonts {
                config.small_fonts = true;
            }
            compile::build(config, &build_args)?;
            Ok(())
        }
        "run" => {
            let run_args = strip_wasm_options(&mut config, &args[1..]);
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
}
