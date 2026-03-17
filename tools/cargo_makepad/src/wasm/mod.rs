mod compile;
mod sdk;
use compile::WasmConfig;

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

fn args_have_profile(args: &[String]) -> bool {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--release" {
            return true;
        }
        if arg == "--profile" {
            return iter.next().is_some();
        }
        if arg.starts_with("--profile=") {
            return true;
        }
    }
    false
}

fn apply_ship_defaults(config: &mut WasmConfig, args: &mut Vec<String>) {
    config.shipping_build = true;
    enable_strip_pipeline(config);
    if !config.brotli_explicit {
        config.brotli = true;
    }
    if !config.split_explicit {
        enable_split_pipeline(config, None);
    }
    if !config.small_fonts_explicit {
        config.small_fonts = true;
        config.full_fonts = false;
    }
    if !config.threads_explicit {
        config.threads = false;
    }
    if !args_have_profile(args) {
        args.push("--profile=small".to_string());
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
        config.split_explicit = true;
        true
    } else if let Some(threshold) = v.strip_prefix("--split=") {
        enable_split_pipeline(config, Some(threshold.parse::<usize>().unwrap_or(200)));
        config.split_explicit = true;
        true
    } else if v == "--no-split" {
        config.split = false;
        config.split_auto = false;
        config.split_explicit = true;
        true
    } else if v == "--small-fonts" {
        config.small_fonts = true;
        config.full_fonts = false;
        config.small_fonts_explicit = true;
        true
    } else if v == "--full-fonts" {
        config.small_fonts = false;
        config.full_fonts = true;
        config.small_fonts_explicit = true;
        true
    } else if v == "--brotli" {
        config.brotli = true;
        config.brotli_explicit = true;
        true
    } else if v == "--no-brotli" {
        config.brotli = false;
        config.brotli_explicit = true;
        true
    } else if v == "--lan" {
        config.lan = true;
        true
    } else if v == "--bindgen" {
        config.bindgen = true;
        true
    } else if v == "--threads" {
        config.threads = true;
        config.threads_explicit = true;
        true
    } else if v == "--no-threads" {
        config.threads = false;
        config.threads_explicit = true;
        true
    } else if v == "--serve" {
        config.serve = true;
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
        serve: false,
        shipping_build: false,
        full_fonts: false,
        brotli_explicit: false,
        threads_explicit: false,
        small_fonts_explicit: false,
        split_explicit: false,
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
            compile::build(config, &build_args)?;
            Ok(())
        }
        "run" => {
            let run_args = strip_wasm_options(&mut config, &args[1..]);
            compile::run(config, &run_args)?;
            Ok(())
        }
        "ship" => {
            let mut ship_args = strip_wasm_options(&mut config, &args[1..]);
            apply_ship_defaults(&mut config, &mut ship_args);
            compile::ship(config, &ship_args)?;
            Ok(())
        }
        _ => Err(format!("{} is not a valid command or option", args[0])),
    }
}
